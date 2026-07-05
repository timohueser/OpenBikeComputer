//! The simulator's **self-diffing present backend** — the host stand-in for the device's
//! partial-row push, with an exact-diff oracle bolted on.
//!
//! The device present path re-pushes only the rows whose per-row hash changed (the shared
//! [`obc_platform::diff_rows`] core). This backend drives that *same* core, then does what the
//! device can't afford: it keeps a full copy of the last-presented frame, independently computes
//! which rows *actually* changed, and asserts the hash-diff's spans covered every one
//! ([`obc_platform::spans_missed_changes`]) — catching any *systematic* diff bug in CI (a real
//! FNV-1a collision is ~2⁻³² per row-change and self-healing).
//!
//! The displayed texture is built from [`presented`](Present::presented) — mutated **only on
//! changed spans**, not a whole-frame copy — so a diff bug surfaces as a stale row on glass, not
//! just a failed assert. The metric ([`PresentStats`]) feeds the control panel.

use obc_platform::{diff_rows, row_hash, spans_missed_changes};

/// Last present's push metric, surfaced in the render-stats panel.
#[derive(Clone, Copy, Default)]
pub struct PresentStats {
    /// Rows actually pushed this present (the sum of the changed spans). `0` when the frame was
    /// byte-identical to the last — the "spurious coarse-dirty is free" property.
    pub pushed_rows: usize,
    /// Number of contiguous changed-row spans (disjoint dirty regions).
    pub spans: usize,
    /// The frame's total row count, for context (`pushed_rows` / `total_rows` = the push fraction).
    pub total_rows: usize,
}

/// The self-diffing present state: the last-pushed frame, its per-row hash store, and the oracle's
/// coverage scratch. Runtime-sized `Vec`s (the sim window height is a CLI knob) rather than the
/// device's fixed `[u32; HEIGHT]`, but driven by the identical [`diff_rows`] core.
pub struct Present {
    /// RGB888 bytes "on glass": seeded all-black and updated only on changed spans, so the texture
    /// is reconstructed from partial pushes.
    presented: Vec<u8>,
    /// Per-row hash of the last-presented frame — the device's `[u32; HEIGHT]` store, runtime-sized.
    hashes: Vec<u32>,
    /// `false` until the first present: the store holds no real prior frame, so the first present
    /// pushes (and seeds) the whole frame.
    primed: bool,
    /// `rows`-long coverage scratch the oracle rewrites each present (no per-frame alloc).
    covered: Vec<bool>,
    /// Bytes per framebuffer row (`width * 3` for RGB888).
    stride: usize,
    /// Frame height in rows.
    rows: usize,
    /// Last present's metric, read by the control panel.
    pub stats: PresentStats,
}

impl Present {
    /// A backend for a `width`×`height` RGB888 framebuffer, primed empty (the first present pushes
    /// the whole frame).
    pub fn new(width: u32, height: u32) -> Self {
        let stride = width as usize * 3;
        let rows = height as usize;
        Present {
            presented: vec![0u8; stride * rows],
            hashes: vec![0u32; rows],
            primed: false,
            covered: vec![false; rows],
            stride,
            rows,
            stats: PresentStats::default(),
        }
    }

    /// Present `rendered` (a full freshly-drawn RGB888 frame): diff it against the per-row hash
    /// store, assert the hash-diff covered every truly-changed row, push only the changed spans
    /// into [`presented`](Present::presented), and return that buffer. Records the push metric.
    pub fn present(&mut self, rendered: &[u8]) -> &[u8] {
        let (stride, rows) = (self.stride, self.rows);

        // 1. Diff against the hash store → contiguous changed-row spans. The store updates to this
        //    frame for every row (pushed or not) — the self-healing property.
        let mut spans: Vec<(u16, u16)> = Vec::new();
        diff_rows(rendered, stride, &mut self.hashes, !self.primed, row_hash, |y0, n| spans.push((y0, n)));
        self.primed = true;

        // 2. Oracle: independently compute the rows that *actually* changed and assert the spans
        //    covered every one. First present: `presented` is all-black, forced full-frame span.
        let missed = spans_missed_changes(&self.presented, rendered, stride, rows, &spans, &mut self.covered);
        debug_assert_eq!(missed, 0, "self-diff missed {missed} changed row(s) — a systematic hash-diff bug");

        // 3. Push only the changed spans. The displayed texture is this partial-push buffer.
        let mut pushed_rows = 0;
        for &(y0, n) in &spans {
            let r = y0 as usize * stride..(y0 as usize + n as usize) * stride;
            self.presented[r.clone()].copy_from_slice(&rendered[r]);
            pushed_rows += n as usize;
        }
        self.stats = PresentStats { pushed_rows, spans: spans.len(), total_rows: rows };
        &self.presented
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `width`×`height` RGB888 frame filled with `v`.
    fn frame(width: u32, height: u32, v: u8) -> Vec<u8> {
        vec![v; (width * height * 3) as usize]
    }

    #[test]
    fn first_present_pushes_the_whole_frame() {
        let mut p = Present::new(2, 4);
        let f = frame(2, 4, 0x55);
        let out = p.present(&f).to_vec();
        assert_eq!(out, f, "the first present reconstructs the whole frame");
        assert_eq!(p.stats.pushed_rows, 4);
        assert_eq!(p.stats.spans, 1);
    }

    #[test]
    fn identical_reframe_pushes_nothing() {
        let mut p = Present::new(2, 4);
        let f = frame(2, 4, 0x55);
        p.present(&f);
        p.present(&f);
        assert_eq!(p.stats.pushed_rows, 0, "a byte-identical reframe pushes nothing");
        assert_eq!(p.stats.spans, 0);
    }

    #[test]
    fn only_the_changed_band_is_pushed_and_presented_reconstructs_it() {
        let mut p = Present::new(2, 5);
        let f0 = frame(2, 5, 0x00);
        p.present(&f0);
        // Change only row 2.
        let mut f1 = f0.clone();
        let stride = 2 * 3;
        f1[2 * stride..3 * stride].fill(0xAA);
        let out = p.present(&f1).to_vec();
        assert_eq!(p.stats.pushed_rows, 1, "only the one changed row is pushed");
        assert_eq!(p.stats.spans, 1);
        // The partial push reconstructs the full frame exactly (presented == rendered).
        assert_eq!(out, f1, "the presented buffer matches the rendered frame after a partial push");
    }

    /// Driven through the real [`App`] + renderer over the demo map: an idle Home re-render pushes
    /// **zero** rows, a Home minute tick only the **clock's** rows, and a map pan **~all**. The
    /// oracle inside [`Present::present`] backs every count.
    #[test]
    fn app_scenarios_idle_is_free_tick_is_small_pan_is_most() {
        use obc_app::{App, AppState, InputClock};
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};

        use crate::framebuffer::Framebuffer;

        const W: u32 = 240;
        const H: u32 = 320;
        let bytes = include_bytes!("../assets/grimsel.obcm").to_vec();
        let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let reader = Reader::new(&src, &tables, &cache);
        let (cx, cy, zoom) = crate::initial_camera(&reader, W);

        let mut fb = Framebuffer::new(W, H);
        let render = |app: &mut App, fb: &mut Framebuffer| {
            app.render_frame(fb, &reader, None, W as f32, H as f32, |c| crate::color_of(c, false));
        };

        // --- Home idle + a minute tick ---
        let mut app = App::new_idle(AppState::new(cx, cy, zoom));
        app.reseed_home(0); // pin the contour backdrop so only the clock moves
        let mut present = Present::new(W, H);

        app.advance_animations(InputClock(0));
        render(&mut app, &mut fb);
        present.present(fb.as_rgb888()); // first present: the whole frame
        render(&mut app, &mut fb);
        present.present(fb.as_rgb888());
        let idle = present.stats.pushed_rows;

        app.advance_animations(InputClock(60_000)); // +1 min: 12:00 → 12:01
        render(&mut app, &mut fb);
        present.present(fb.as_rgb888());
        let tick = present.stats.pushed_rows;

        // --- A fresh map, then a pan ---
        let mut app = App::new(AppState::new(cx, cy, zoom));
        let mut present = Present::new(W, H);
        app.advance_animations(InputClock(0));
        render(&mut app, &mut fb);
        present.present(fb.as_rgb888()); // first present: the whole frame
        let span_lon = (reader.bbox.max_lon as i64 - reader.bbox.min_lon as i64).max(1);
        app.state.cam_lon = app.state.cam_lon.wrapping_add((span_lon / 4) as i32); // pan a quarter-map
        render(&mut app, &mut fb);
        present.present(fb.as_rgb888());
        let pan = present.stats.pushed_rows;

        assert_eq!(idle, 0, "an idle Home re-render pushes nothing");
        assert!(tick > 0 && tick < H as usize / 3, "a minute tick pushes only the clock rows, got {tick}");
        assert!(pan > H as usize / 2, "a map pan pushes ~all rows, got {pan}");
    }

    /// Real-data pack→parse of the v6 POI section (#423): the committed `monaco.obcm` — a POI-dense
    /// coastal fixture the packer produced from a real OSM extract — must parse as v6 and expose a
    /// full six-category POI directory with several **non-empty** categories, each carrying a real
    /// quadtree (non-zero node + chunk counts). This complements the reader's hand-built byte pins
    /// (`obc-reader/tests/format.rs`) by exercising the whole write→read path on real geometry, and
    /// gives the #425 POI browser a map with POIs to browse in the sim/snapshot suite.
    #[test]
    fn monaco_fixture_parses_a_populated_v6_poi_directory() {
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};

        let bytes = include_bytes!("../assets/monaco.obcm").to_vec();
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).expect("monaco.obcm parses as a valid v7 map");
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);

        assert_eq!(r.version, 7, "the fixture is OBCM v7");
        let dir = r.poi_directory();
        // The directory is always present with all six categories (spec §7.1).
        assert_eq!(dir.entries.len(), 6, "six-category POI directory");
        assert_eq!(dir.chunk_size, 512, "the packer's fixed 512-byte POI chunks");
        // Monaco is a dense coastal city → several categories populated. Each non-empty category
        // must carry a real quadtree: node_count and chunk_count both non-zero, ids in 1..=6.
        let populated: Vec<u8> = dir
            .entries
            .iter()
            .filter(|e| !e.is_empty())
            .inspect(|e| {
                assert!((1..=6).contains(&e.category_id), "category id {} in range", e.category_id);
                assert!(e.node_count > 0 && e.chunk_count > 0, "a non-empty category has a real tree");
            })
            .map(|e| e.category_id)
            .collect();
        assert!(populated.len() >= 3, "Monaco packs ≥3 POI categories, got {populated:?}");
    }

    #[test]
    fn presented_tracks_a_sequence_of_partial_changes() {
        let mut p = Present::new(3, 6);
        let stride = 3 * 3;
        let mut f = frame(3, 6, 0x10);
        p.present(&f);
        // A few disjoint edits across frames; after each, the partial-push buffer must equal the
        // freshly-rendered frame — the load-bearing property (partial pushes reconstruct the whole).
        for (row, val) in [(0usize, 0x20u8), (5, 0x30), (3, 0x40)] {
            f[row * stride..(row + 1) * stride].fill(val);
            let out = p.present(&f).to_vec();
            assert_eq!(out, f, "presented diverged from rendered at row {row}");
            assert_eq!(p.stats.pushed_rows, 1);
        }
    }
}
