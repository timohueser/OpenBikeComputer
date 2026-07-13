//! The simulator's **`DisplayDriver` backend** — the host stand-in for the device's LS021/FLPR
//! panel, and the second live implementation of the [`obc_platform::DisplayDriver`] seam.
//!
//! Like the device, this backend owns a resident **RGB222 / device-64** framebuffer
//! ([`fb_mut`](DisplayDriver::fb_mut), one byte per pixel, `0b00_RR_GG_BB`): the shared renderer
//! draws the whole frame into it through an [`FbDevice64`](obc_platform::FbDevice64), then
//! [`present`](DisplayDriver::present) pushes it. `present` drives the *same* self-diffing
//! [`diff_rows`](obc_platform::diff_rows) core the device does — re-pushing only the rows whose
//! per-row hash changed, honouring the overlay `exclude` span with the *same*
//! [`clip_span`](obc_platform::clip_span) — and [`present_overlay`](DisplayDriver::present_overlay)
//! composites through the *same* [`composite_overlay_window`](obc_platform::composite_overlay_window)
//! helper. The only host-specific step is expanding the pushed device-64 rows to the RGB888 texture
//! egui uploads (via [`device64_to_rgb565`](obc_platform::device64_to_rgb565) → RGB888 — exactly the
//! ramp the panel shows).
//!
//! On top of the device's path it does what the device can't afford: it keeps a full copy of the
//! last-presented frame, independently computes which rows *actually* changed, and asserts the
//! hash-diff's spans covered every one ([`spans_missed_changes`](obc_platform::spans_missed_changes))
//! — the exact-diff **oracle** that catches any *systematic* diff bug in CI (a real FNV-1a collision
//! is ~2⁻³² per row-change and self-healing). Because the uploaded texture is reconstructed from the
//! partial pushes (mutated **only on changed spans**), a diff bug also surfaces as a stale row on
//! glass, not just a failed assert.
//!
//! The seam's methods are `async`; on the host they complete synchronously (a texture write never
//! faults), so the frame loop drives them with a minimal [`pollster::block_on`].

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use obc_platform::{
    clip_span, composite_overlay_window, device64_to_rgb565, diff_rows, row_hash, spans_missed_changes, Band,
    DisplayDriver, OverlayRegion,
};
use obc_reader::rgb565_to_rgb888;

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

/// The simulator's [`DisplayDriver`] backend: the resident device-64 frame the renderer draws into,
/// its per-row hash store, the partial-push reconstruction (oracle prev **and** the source of the
/// uploaded texture), and the RGB888 texture itself. Runtime-sized `Vec`s (the sim window height is a
/// CLI knob) rather than the device's fixed `[u32; HEIGHT]`, but driven by the identical
/// [`diff_rows`] core at the identical stride (one byte per pixel).
pub struct Present {
    /// The resident RGB222 / device-64 frame the renderer draws into — [`fb_mut`](DisplayDriver::fb_mut).
    fb: Vec<u8>,
    /// The device-64 bytes "on glass": seeded all-black and updated only on changed spans, so it is
    /// reconstructed from partial pushes. The oracle's `prev` frame, and the source the texture
    /// expands from.
    presented: Vec<u8>,
    /// The RGB888 texture egui uploads — the `presented` device-64 frame expanded per changed row.
    tex: Vec<u8>,
    /// Per-row hash of the last-presented frame — the device's `[u32; HEIGHT]` store, runtime-sized.
    hashes: Vec<u32>,
    /// `false` until the first present: the store holds no real prior frame, so the first present
    /// pushes (and seeds) the whole frame.
    primed: bool,
    /// `rows`-long coverage scratch the oracle rewrites each present (no per-frame alloc).
    covered: Vec<bool>,
    /// Per-row "this miss was already logged" dedupe flags for the oracle diagnostics. A missed row
    /// stays byte-different every frame until it next changes, and on a parked static screen — the
    /// exact scenario the diagnostics exist for — present() runs at frame rate, so an undeduped log
    /// would flood the console (in `--release` the assert compiles out and nothing stops the loop).
    /// Set when a row's miss is first reported, cleared the moment the row stops missing.
    miss_reported: Vec<bool>,
    /// How many `miss_reported` flags are set — lets the happy path (no miss ever reported, the
    /// universal case) skip the clear sweep entirely.
    misses_flagged: usize,
    /// Frame width in pixels — the device-64 row stride (one byte per pixel).
    width: usize,
    /// Frame height in rows.
    rows: usize,
    /// Last present's metric, read by the control panel.
    pub stats: PresentStats,
}

impl Present {
    /// A backend for a `width`×`height` frame, primed empty (the first present pushes the whole
    /// frame). The resident plane is device-64 (one byte per pixel), like the device.
    pub fn new(width: u32, height: u32) -> Self {
        let width = width as usize;
        let rows = height as usize;
        Present {
            fb: vec![0u8; width * rows],
            presented: vec![0u8; width * rows],
            tex: vec![0u8; width * rows * 3],
            hashes: vec![0u32; rows],
            primed: false,
            covered: vec![false; rows],
            miss_reported: vec![false; rows],
            misses_flagged: 0,
            width,
            rows,
            stats: PresentStats::default(),
        }
    }

    /// The RGB888 texture reconstructed from this session's partial pushes — what the GUI uploads to
    /// its egui texture (and `--screenshot` captures). `width * height * 3` bytes.
    pub fn texture(&self) -> &[u8] {
        &self.tex
    }

    /// Expand one device-64 row of `presented` into the RGB888 texture — the host-specific step (the
    /// device packs the same row to the LS021 wire instead). Uses the panel's own
    /// [`device64_to_rgb565`] → RGB888 ramp, so the texture is what the glass would show.
    fn expand_row_to_tex(&mut self, y: usize) {
        let (w, base) = (self.width, y * self.width);
        for c in 0..w {
            let (r, g, b) = rgb565_to_rgb888(device64_to_rgb565(self.presented[base + c]));
            let t = (base + c) * 3;
            self.tex[t] = r;
            self.tex[t + 1] = g;
            self.tex[t + 2] = b;
        }
    }
}

impl DisplayDriver for Present {
    fn fb_mut(&mut self) -> &mut [u8] {
        &mut self.fb
    }

    /// Self-diff the resident frame and "push" only the changed rows, exactly as the device does:
    /// diff every row against the hash store (updating it for **all** rows — the self-healing
    /// property), clip each changed span around a live overlay's `exclude` rows, reconstruct the
    /// pushed rows into `presented` + the texture, then assert the oracle is satisfied — push
    /// first, so a failed oracle can't desync later frames (#626). Always `true` — a host texture
    /// write never faults.
    async fn present(&mut self, exclude: Option<(u16, u16)>) -> bool {
        let (stride, rows) = (self.width, self.rows);

        // 1. Diff against the hash store → contiguous changed-row spans. The store updates to this
        //    frame for every row (pushed or not) — the self-healing property.
        let mut raw: Vec<(u16, u16)> = Vec::new();
        diff_rows(&self.fb, stride, &mut self.hashes, !self.primed, row_hash, |y0, n| raw.push((y0, n)));
        self.primed = true;

        // 2. Clip each changed span around a live overlay's rows (`exclude`) — the same
        //    bulge-coordination clip the device's `present(exclude)` runs. The excluded rows belong
        //    to the overlay plane this frame, so they are not pushed here. `clip_span` takes a
        //    half-open interval `[e0, e1)`, so convert the `(y0, rows)` overlay span the same way the
        //    device's `RowDiff::diff_clipped` does.
        let ex = exclude.map(|(y0, rows)| (y0, y0 + rows));
        let mut spans: Vec<(u16, u16)> = Vec::new();
        for &(y0, n) in &raw {
            clip_span(y0, n, ex, &mut |s, c| spans.push((s, c)));
        }

        // 3. Push only the changed spans into `presented` (device-64) and the texture (RGB888). The
        //    displayed texture is this partial-push buffer. The push runs BEFORE the oracle check
        //    (#626): a failed check then aborts *after* the frame landed, so every diffed row is
        //    already in `presented` and one miss can never desync — and re-assert on — every
        //    subsequent present. (Pre-fix, the first assert fired between the hash-store update and
        //    the push, leaving every changed row permanently stale: a self-sustaining panic cascade
        //    whose steady-state `missed: 1` hid the real first failure.)
        let mut pushed_rows = 0;
        for &(y0, n) in &spans {
            for y in y0 as usize..y0 as usize + n as usize {
                let r = y * stride..y * stride + stride;
                self.presented[r.clone()].copy_from_slice(&self.fb[r]);
                self.expand_row_to_tex(y);
            }
            pushed_rows += n as usize;
        }
        self.stats = PresentStats { pushed_rows, spans: spans.len(), total_rows: rows };

        // 4. Oracle: independently compute the rows that *actually* changed and assert the pushed
        //    spans (plus the overlay's excluded rows, which the overlay plane owns) covered every
        //    one. Rows inside the spans were just pushed (now byte-equal, and covered anyway), so
        //    checking after the push is equivalent — a miss is exactly an uncovered row whose bytes
        //    still differ. Such a row stays self-healing: its store hash already tracks `fb`, so
        //    the next time it changes it re-pushes cleanly.
        let mut oracle_spans = spans.clone();
        if let Some(ex) = exclude {
            oracle_spans.push(ex);
        }
        let missed = spans_missed_changes(&self.presented, &self.fb, stride, rows, &oracle_spans, &mut self.covered);
        if missed != 0 {
            // Diagnostics before the assert (kept in release too, where the assert compiles out and
            // the miss would otherwise be a silent stale row): which row, its stored hash, and the
            // differing column range. `covered` still holds the oracle's span coverage. Deduped per
            // row — a missed row on a parked screen stays byte-different until it next changes, so
            // in release this branch runs every frame; log only when a row's miss FIRST appears.
            for (y, &cov) in self.covered[..rows].iter().enumerate() {
                let r = y * stride..y * stride + stride;
                let missing = !cov && self.presented[r.clone()] != self.fb[r.clone()];
                match (missing, self.miss_reported[y]) {
                    (true, false) => {
                        let differs = |c: &usize| self.presented[r.start + c] != self.fb[r.start + c];
                        let first = (0..stride).find(differs).unwrap_or(0);
                        let last = (0..stride).rev().find(differs).unwrap_or(0);
                        eprintln!(
                            "present self-diff MISS: row {y} (stored hash {:#010x} matches fb) differs from presented at cols {first}..={last}",
                            self.hashes[y]
                        );
                        self.miss_reported[y] = true;
                        self.misses_flagged += 1;
                    }
                    (false, true) => {
                        // The row healed (or was pushed/covered this frame): re-arm its report.
                        self.miss_reported[y] = false;
                        self.misses_flagged -= 1;
                    }
                    _ => {}
                }
            }
        } else if self.misses_flagged != 0 {
            // Every previously-reported row healed this present: re-arm all reports in one sweep
            // (skipped entirely on the universal no-miss-ever path).
            self.miss_reported.fill(false);
            self.misses_flagged = 0;
        }
        debug_assert_eq!(missed, 0, "self-diff missed {missed} changed row(s) — a systematic hash-diff bug");
        true
    }

    /// Re-present `region` with `draw_overlay` composited over the **clean framebuffer backdrop** —
    /// through the shared [`composite_overlay_window`], exactly as the device does. The clean device-64
    /// `fb` is never written (the overlay is transient chrome); only the display texture picks up the
    /// composited window. Always `true` on the host.
    async fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool {
        let (w, rows) = (region.w as usize, region.rows as usize);
        let frame = Size::new(self.width as u32, self.rows as u32);
        let window = Rectangle::new(
            Point::new(region.x0 as i32, region.y0 as i32),
            Size::new(region.w as u32, region.rows as u32),
        );
        // Composite the overlay over the clean device-64 backdrop into an RGB565 scratch — the one
        // piece byte-for-byte identical to the device's overlay push.
        let mut scratch = vec![0u16; w * rows];
        composite_overlay_window(&self.fb, frame, window, &mut scratch, draw_overlay);
        // Blit the composited window into the display texture (RGB565 → RGB888), leaving `presented`
        // (the clean reconstruction) and `fb` untouched.
        for r in 0..rows {
            let fy = region.y0 as usize + r;
            for c in 0..w {
                let fx = region.x0 as usize + c;
                let (rr, gg, bb) = rgb565_to_rgb888(scratch[r * w + c]);
                let t = (fy * self.width + fx) * 3;
                self.tex[t] = rr;
                self.tex[t + 1] = gg;
                self.tex[t + 2] = bb;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::raw::RawU16;
    use embedded_graphics::pixelcolor::Rgb565;
    use obc_platform::FbDevice64;
    use pollster::block_on;

    use super::*;

    /// Drive a present over a freshly-filled device-64 frame `v`.
    fn present_fill(p: &mut Present, v: u8, exclude: Option<(u16, u16)>) {
        p.fb_mut().fill(v);
        block_on(p.present(exclude));
    }

    /// The RGB888 the texture should hold for a device-64 byte.
    fn expect_px(byte: u8) -> (u8, u8, u8) {
        rgb565_to_rgb888(device64_to_rgb565(byte))
    }

    #[test]
    fn geometry_matches_the_platform_authority() {
        // The sim's device resolution is the single obc-platform authority, not a re-declared literal.
        assert_eq!(obc_platform::FRAME_W, 240);
        assert_eq!(obc_platform::FRAME_H, 320);
        let p = Present::new(obc_platform::FRAME_W as u32, obc_platform::FRAME_H as u32);
        assert_eq!(p.width, obc_platform::FRAME_W);
        assert_eq!(p.rows, obc_platform::FRAME_H);
    }

    #[test]
    fn first_present_pushes_the_whole_frame() {
        let mut p = Present::new(2, 4);
        present_fill(&mut p, 0x15, None);
        assert_eq!(p.stats.pushed_rows, 4, "the first present pushes the whole frame");
        assert_eq!(p.stats.spans, 1);
        // The texture reconstructs every pixel from the device-64 byte.
        let (r, g, b) = expect_px(0x15);
        assert!(p.texture().chunks_exact(3).all(|px| px == [r, g, b]), "texture reconstructs the whole frame");
    }

    #[test]
    fn identical_reframe_pushes_nothing() {
        let mut p = Present::new(2, 4);
        present_fill(&mut p, 0x15, None);
        present_fill(&mut p, 0x15, None);
        assert_eq!(p.stats.pushed_rows, 0, "a byte-identical reframe pushes nothing");
        assert_eq!(p.stats.spans, 0);
    }

    #[test]
    fn only_the_changed_band_is_pushed_and_texture_reconstructs_it() {
        let mut p = Present::new(2, 5);
        present_fill(&mut p, 0x00, None);
        // Change only row 2 (device-64 stride = width = 2).
        p.fb_mut()[2 * 2..3 * 2].fill(0x2A);
        block_on(p.present(None));
        assert_eq!(p.stats.pushed_rows, 1, "only the one changed row is pushed");
        assert_eq!(p.stats.spans, 1);
        // The partial push reconstructs row 2 in the texture; the others stay black.
        let (r, g, b) = expect_px(0x2A);
        let px = |row: usize, col: usize| {
            let t = (row * 2 + col) * 3;
            (p.texture()[t], p.texture()[t + 1], p.texture()[t + 2])
        };
        assert_eq!(px(2, 0), (r, g, b));
        assert_eq!(px(2, 1), (r, g, b));
        assert_eq!(px(1, 0), (0, 0, 0), "unchanged rows stay black");
    }

    #[test]
    fn present_honours_the_overlay_exclude_span() {
        // Rows 1..=3 change; the exclude [2,4) (i.e. rows 2,3) belongs to the overlay plane, so the
        // clean present pushes only row 1 and leaves 2,3 for the overlay — the device's discipline.
        let mut p = Present::new(2, 5);
        present_fill(&mut p, 0x00, None);
        for y in 1..=3 {
            p.fb_mut()[y * 2..(y + 1) * 2].fill(0x11);
        }
        block_on(p.present(Some((2, 2))));
        assert_eq!(p.stats.pushed_rows, 1, "the excluded rows are not pushed by the clean present");
        assert_eq!(p.stats.spans, 1);
        // The oracle inside present() proved the pushed span + the exclude span cover every real
        // change; here we also confirm the excluded rows stayed black in the texture (overlay owns them).
        let row1 = expect_px(0x11);
        let at = |row: usize| {
            let t = row * 2 * 3;
            (p.texture()[t], p.texture()[t + 1], p.texture()[t + 2])
        };
        assert_eq!(at(1), row1, "row 1 pushed clean");
        assert_eq!(at(2), (0, 0, 0), "excluded row left for the overlay plane");
    }

    #[test]
    fn present_overlay_composites_the_backdrop_then_the_drawer_over_it() {
        // A device-64 backdrop (every pixel a distinct byte), then an overlay window that paints one
        // pixel red over the clean fb — through the same shared helper the device uses.
        let mut p = Present::new(8, 8);
        for (i, b) in p.fb_mut().iter_mut().enumerate() {
            *b = (i as u8) & 0b0011_1111;
        }
        // Seed the texture from a full present so unrelated pixels are defined.
        block_on(p.present(None));
        let fb_snapshot = p.fb.clone();
        // Window cols [4,8) × rows [2,6); the drawer paints frame-absolute (5,3) red.
        let region = OverlayRegion { x0: 4, y0: 2, w: 4, rows: 4 };
        block_on(p.present_overlay(region, &mut |band: &mut Band| {
            band.fill_solid(&Rectangle::new(Point::new(5, 3), Size::new(1, 1)), Rgb565::from(RawU16::new(0xF800))).ok();
        }));
        let tex_at = |x: usize, y: usize| {
            let t = (y * 8 + x) * 3;
            (p.texture()[t], p.texture()[t + 1], p.texture()[t + 2])
        };
        // Backdrop: frame (4,2) = fb byte 2*8+4 = 20, expanded to RGB888.
        assert_eq!(tex_at(4, 2), expect_px(20), "backdrop = clean fb expanded to RGB888");
        // Overlay: frame (5,3) painted pure red.
        assert_eq!(tex_at(5, 3), (255, 0, 0), "the drawer painted frame-absolute (5,3) red");
        // The clean framebuffer is never written by the overlay path.
        assert_eq!(p.fb, fb_snapshot, "present_overlay never writes the resident frame");
    }

    /// Driven through the real [`App`] + renderer over the demo map into the device-64 backend: an
    /// idle Home re-render pushes **zero** rows, a Home minute tick only the **clock's** rows, and a
    /// map pan **~all**. The oracle inside [`Present::present`] backs every count.
    #[test]
    fn app_scenarios_idle_is_free_tick_is_small_pan_is_most() {
        use obc_app::{App, AppState};
        use obc_ports::InputClock;
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};

        // The device resolution is the single obc-platform authority, not a re-declared literal.
        const W: u32 = obc_platform::FRAME_W as u32;
        const H: u32 = obc_platform::FRAME_H as u32;
        let bytes = include_bytes!("../assets/grimsel.obcm").to_vec();
        let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let reader = Reader::new(&src, &tables, &cache);
        let (cx, cy, zoom) = crate::initial_camera(&reader, W);

        // Render the whole frame into the backend's resident device-64 plane, exactly as the GUI
        // loop does — the device color path (`Rgb565` → device-64 pack), not an RGB888 side buffer.
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        let render = |app: &mut App, p: &mut Present| {
            let mut fbdev = FbDevice64::new(p.fb_mut(), W, H);
            app.render_frame(&mut fbdev, &reader, None, W as f32, H as f32, color_fn);
        };

        // --- Home idle + a minute tick ---
        let mut app = App::new_idle(AppState::new(cx, cy, zoom));
        app.reseed_home(0); // pin the contour backdrop so only the clock moves
        let mut present = Present::new(W, H);

        app.advance_animations(InputClock(0));
        render(&mut app, &mut present);
        block_on(present.present(None)); // first present: the whole frame
        render(&mut app, &mut present);
        block_on(present.present(None));
        let idle = present.stats.pushed_rows;

        app.advance_animations(InputClock(60_000)); // +1 min: 12:00 → 12:01
        render(&mut app, &mut present);
        block_on(present.present(None));
        let tick = present.stats.pushed_rows;

        // --- A fresh map, then a pan ---
        let mut app = App::new(AppState::new(cx, cy, zoom));
        let mut present = Present::new(W, H);
        app.advance_animations(InputClock(0));
        render(&mut app, &mut present);
        block_on(present.present(None)); // first present: the whole frame
        let span_lon = (reader.bbox.max_lon as i64 - reader.bbox.min_lon as i64).max(1);
        app.state.cam_lon = app.state.cam_lon.wrapping_add((span_lon / 4) as i32); // pan a quarter-map
        render(&mut app, &mut present);
        block_on(present.present(None));
        let pan = present.stats.pushed_rows;

        assert_eq!(idle, 0, "an idle Home re-render pushes nothing");
        assert!(tick > 0 && tick < H as usize / 3, "a minute tick pushes only the clock rows, got {tick}");
        // A pan invalidates far more rows than a clock tick — the exact count is content-dependent
        // (how many rows the panned view's features touch), so assert the three-tier contrast
        // (idle ≪ tick ≪ pan) rather than a brittle absolute fraction that a re-pack's new OSM
        // vintage can dip under.
        assert!(
            pan > 3 * tick && pan > H as usize / 3,
            "a map pan pushes far more than a tick, got pan {pan} tick {tick}"
        );
    }

    /// Real-data pack→parse of the POI section (#423): the committed `monaco.obcm` — a POI-dense
    /// coastal fixture the packer produced from a real OSM extract — must parse as v10 and expose a
    /// full six-category POI directory with several **non-empty** categories, each carrying a real
    /// quadtree (non-zero node + chunk counts), plus a populated §8 nav graph (#464). This
    /// complements the reader's hand-built byte pins (`obc-reader/tests/format.rs`) by exercising
    /// the whole write→read path on real geometry, and gives the #425 POI browser a map with POIs
    /// to browse in the sim/snapshot suite.
    #[test]
    fn monaco_fixture_parses_populated_poi_and_nav_sections() {
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};

        let bytes = include_bytes!("../assets/monaco.obcm").to_vec();
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).expect("monaco.obcm parses as a valid v10 map");
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);

        assert_eq!(r.version, 10, "the fixture is OBCM v10");
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

        // The §8 nav graph: a dense city extract must bake a real routable graph — a populated
        // node quadtree and a non-empty edge pool, streets everywhere in the bbox.
        let nav = r.nav_directory();
        assert!(!nav.is_empty(), "Monaco has routable streets");
        assert!(nav.chunk_count > 0 && nav.edge_chunk_count > 0, "node chunks + edge pool present");
        assert_eq!(nav.chunk_size, 512, "the packer's fixed 512-byte nav chunks");
        let mut scratch = [0u8; 512];
        let mut nodes = 0usize;
        r.for_each_nav_node(&r.bbox, &mut scratch, |n| {
            nodes += 1;
            assert!(n.degree() >= 1, "a junction always carries at least one arc");
        })
        .expect("nav walk over the whole bbox");
        assert!(nodes > 100, "a city extract yields a real junction set, got {nodes}");
    }

    /// #626 cascade-proofing: one failed oracle check must not desync — and re-assert on — later
    /// presents. Fabricate the exact aftermath a row-hash collision produces (a changed row whose
    /// store hash already matches, so the diff skips it) alongside an honestly-changed row: the
    /// assert fires, but the honest row was pushed *before* it (the #626 reorder), and the one
    /// stale row self-heals on its next change with no further panic.
    #[test]
    fn a_missed_row_cannot_poison_subsequent_presents() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let mut p = Present::new(4, 6);
        present_fill(&mut p, 0x01, None);
        // Row 2 changes but its store hash is (wrongly) already up to date — a simulated collision.
        p.fb_mut()[2 * 4..3 * 4].fill(0x22);
        p.hashes[2] = row_hash(&p.fb[2 * 4..3 * 4]);
        // Row 4 changes honestly in the same frame.
        p.fb_mut()[4 * 4..5 * 4].fill(0x2A);
        let outcome = catch_unwind(AssertUnwindSafe(|| block_on(p.present(None))));
        if cfg!(debug_assertions) {
            assert!(outcome.is_err(), "the oracle assert fires on the fabricated miss");
        }
        // The reorder guarantee: the frame landed before the assert aborted the present.
        assert_eq!(&p.presented[4 * 4..5 * 4], &[0x2A; 4], "the honest row was pushed despite the failed oracle");
        // The stale row heals the moment it changes again — and nothing else re-asserts.
        p.fb_mut()[2 * 4..3 * 4].fill(0x30);
        block_on(p.present(None));
        assert_eq!(p.presented, p.fb, "one missed row healed itself; no cascade");
    }

    /// The oracle's miss diagnostics are deduped per row: a missed row on a parked static screen
    /// stays byte-different every frame (in `--release` nothing stops the loop), so the report must
    /// fire once when the miss appears, stay quiet while it persists, and re-arm when the row
    /// heals. The `miss_reported` flags gate the log line 1:1, so pin the flag lifecycle.
    #[test]
    fn miss_diagnostics_are_deduped_per_row() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let mut p = Present::new(4, 6);
        present_fill(&mut p, 0x01, None);
        // Fabricate a persistent miss on row 2 (a collision's aftermath, as in the cascade test).
        p.fb_mut()[2 * 4..3 * 4].fill(0x22);
        p.hashes[2] = row_hash(&p.fb[2 * 4..3 * 4]);
        // First present: the miss appears → reported (the debug assert also fires; catch it).
        let _ = catch_unwind(AssertUnwindSafe(|| block_on(p.present(None))));
        assert!(p.miss_reported[2], "the new miss is reported");
        assert_eq!(p.misses_flagged, 1);
        // The screen stays parked: the same miss persists — no re-report (the flag stays set).
        for _ in 0..3 {
            let _ = catch_unwind(AssertUnwindSafe(|| block_on(p.present(None))));
            assert!(p.miss_reported[2], "the persisting miss stays flagged, not re-reported");
            assert_eq!(p.misses_flagged, 1, "no duplicate report accumulates");
        }
        // The row changes → re-pushed, healed: the report re-arms.
        p.fb_mut()[2 * 4..3 * 4].fill(0x30);
        block_on(p.present(None));
        assert!(!p.miss_reported[2], "a healed row re-arms its report");
        assert_eq!(p.misses_flagged, 0);
    }

    /// #626 deterministic repro/regression: two frames that differ only in device-64 pixels at
    /// columns 3 and 7 (both ≡ 3 mod 4 — the top byte of a hash word). Under the pre-fix word-FNV
    /// row hash this exact pair collided (measured: such deltas cancel with ~2⁻⁸ probability, not
    /// 2⁻³²), so the diff skipped the row and the oracle assert fired — the guided tour's
    /// "self-diff missed 1 changed row(s)" panic. The fixed hash must flag and push the row.
    #[test]
    fn lane3_confined_pixel_change_is_pushed() {
        let mut p = Present::new(8, 4);
        present_fill(&mut p, 0x00, None);
        // Row 2: pixels x=3 → 0x02 and x=7 → 0x2E (a measured colliding pair of the old hash).
        p.fb_mut()[2 * 8 + 3] = 0x02;
        p.fb_mut()[2 * 8 + 7] = 0x2E;
        block_on(p.present(None));
        assert_eq!(p.stats.pushed_rows, 1, "the lane-3-confined change must be diffed and pushed");
        let r = 2 * 8..3 * 8;
        assert_eq!(p.presented[r.clone()], p.fb[r], "row 2 reconstructed");
    }

    /// One full sim frame, exactly the `gui.rs::render_to_texture` skeleton: drain nav
    /// request/cancel, step an in-flight route plan, open the active route, advance the GPX replay +
    /// tick, render into the backend's device-64 plane, then present. Before presenting it predicts
    /// misses (see [`predicted_misses`]) and panics with full diagnostics on the FIRST one.
    #[allow(clippy::too_many_arguments)]
    fn tour_frame(
        app: &mut obc_app::App,
        present: &mut Present,
        player: &mut obc_replay::GpxPlayer,
        baro: &mut obc_replay::BaroSensor,
        store: &mut crate::routes::RouteStore,
        nav_plan: &mut Option<crate::NavPlan>,
        reader: &obc_reader::Reader,
        tour_active: bool,
        frame_no: &mut usize,
        label: &str,
    ) {
        use obc_route::{RouteIndex, RouteReader};

        const W: u32 = obc_platform::FRAME_W as u32;
        const H: u32 = obc_platform::FRAME_H as u32;

        // Route planner drains + one bounded step per frame (gui.rs).
        if let Some(req) = app.take_nav_request() {
            *nav_plan = Some(crate::NavPlan::start(&req, app.settings().bike_profile_idx));
        }
        if app.take_nav_cancel() {
            *nav_plan = None;
        }
        let step = nav_plan.as_mut().map(|plan| plan.step(reader));
        match step {
            None | Some(obc_route::Step::Running) => {}
            Some(obc_route::Step::Done(stats)) => {
                let plan = nav_plan.take().expect("just stepped it");
                crate::finish_nav_plan(app, store, Ok(stats), plan.bytes(), plan.tile_stats());
            }
            Some(obc_route::Step::Failed(e)) => {
                let plan = nav_plan.take().expect("just stepped it");
                crate::finish_nav_plan(app, store, Err(e), plan.bytes(), plan.tile_stats());
            }
        }

        // Open the active route's geometry (gui.rs re-opens per frame).
        store.sync_active(app.activity.active_route);
        let route_src = store.active_source();
        let route_index = route_src.as_ref().and_then(|s| RouteIndex::read(s).ok());
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };

        // Advance the replay + tick on the playback clock, then the wasm demo's ambient
        // auto-restart (suppressed while a tour runs — the branch's `!tour_active` gate).
        crate::replay_step(
            app,
            player,
            baro,
            None,
            1.0 / 60.0,
            route.as_ref(),
            None,
            obc_host_core::ReplaySensors::default(),
        );
        if !tour_active && !player.is_playing() {
            player.play();
            app.activity.start_session();
        }

        // Render the whole frame into the backend's resident device-64 plane.
        let mut fbdev = FbDevice64::new(present.fb_mut(), W, H);
        app.render_frame(&mut fbdev, reader, route.as_ref(), W as f32, H as f32, |c| Rgb565::from(RawU16::new(c)));

        // Present (the oracle inside asserts no miss, with row diagnostics on failure), then the
        // full-strength postcondition: after a clean present the reconstruction equals the frame
        // byte-for-byte on EVERY row — what the acceptance calls "texture matches the framebuffer".
        block_on(present.present(None));
        for y in 0..present.rows {
            let r = y * present.width..(y + 1) * present.width;
            assert!(
                present.presented[r.clone()] == present.fb[r],
                "presented != fb at row {y} after present (frame {frame_no} [{label}])"
            );
        }
        *frame_no += 1;
    }

    /// #626 acceptance: drive the real `App` + renderer through the guided tour's exact command
    /// sequences — the ambient ride, a demo-style app rebuild + mid-climb `GpxPlayer::seek` per
    /// `enter`, the climb demo's Back-cycle, the reroute-to-POI demo including the frame-stepped
    /// planner, and the ambient reset's backward seek — dwelling ≥300 presents on each tour screen
    /// (Map, Statistics, Climb, Menu, PoiList, PoiDetail, RouteOverview). Every frame presents
    /// under the oracle (debug asserts on) *and* the full byte-equality postcondition in
    /// [`tour_frame`], so any diff miss — the pre-fix panic — fails here with row diagnostics.
    #[test]
    fn tour_screens_dwell_with_no_present_miss() {
        use std::path::Path;

        use obc_app::screen::Screen;
        use obc_app::settings::{ClimbMode, Settings};
        use obc_app::{App, AppState, CameraMode, Gesture};
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};
        use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

        const W: u32 = obc_platform::FRAME_W as u32;
        const H: u32 = obc_platform::FRAME_H as u32;
        let bytes = include_bytes!("../assets/grimsel.obcm").to_vec();
        let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let reader = Reader::new(&src, &tables, &cache);

        // A folder-backed route store over a temp dir seeded with the demo route, so the planner's
        // `_nav.obcr` write + rescan runs the real path.
        let dir = std::env::temp_dir().join(format!("obc626-tour-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp routes dir");
        std::fs::write(dir.join("grimsel-climb.obcr"), include_bytes!("../assets/grimsel-climb.obcr"))
            .expect("seed demo route");
        let mut store = crate::routes::RouteStore::open(&dir);

        let track =
            Track::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/grimsel-climb.gpx"))).expect("gpx");
        let mut player = GpxPlayer::new(track);
        player.set_speed(3.0); // the page's ambient pace (obc-web-demo's `DEMO_SPEED`)
        let mut baro = BaroSensor::new();
        let mut nav_plan: Option<crate::NavPlan> = None;
        let mut present = Present::new(W, H);
        let mut frame_no = 0usize;

        let (cx, cy, zoom) = crate::initial_camera(&reader, W);
        let build_app = |settings: Settings, store: &crate::routes::RouteStore| {
            let mut state = AppState::new(cx, cy, zoom * 12.0);
            state.mode = CameraMode::Follow;
            state.heading_up = true;
            let mut app = App::new(state);
            app.set_nav_profiles(tables.nav_profiles());
            app.set_routes_with_ids(store.catalog(), store.ids());
            app.set_settings(settings);
            if !store.catalog().is_empty() {
                app.activity.active_route = Some(0);
                app.activity.start_session();
            }
            app
        };

        // Run frames until the top screen matches (the page's closed-loop `until` polling), then
        // park there for `$dwell` more presents (the tour's dwell — where a missed row would sit
        // stale forever). Panics if the target screen is never reached: the sequences below must
        // not drift, or the dwell wouldn't be testing the screen it claims.
        macro_rules! until_then_dwell {
            ($app:expr, $label:expr, $pat:pat, $dwell:expr) => {{
                let mut reached = false;
                for _ in 0..1200 {
                    tour_frame(
                        $app,
                        &mut present,
                        &mut player,
                        &mut baro,
                        &mut store,
                        &mut nav_plan,
                        &reader,
                        true,
                        &mut frame_no,
                        $label,
                    );
                    if matches!($app.top_screen(), $pat) {
                        reached = true;
                        break;
                    }
                }
                assert!(reached, "never reached {}", $label);
                for _ in 0..$dwell {
                    tour_frame(
                        $app,
                        &mut present,
                        &mut player,
                        &mut baro,
                        &mut store,
                        &mut nav_plan,
                        &reader,
                        true,
                        &mut frame_no,
                        $label,
                    );
                }
            }};
        }

        // --- Page load: ambient ride from the start. ---
        let mut app = build_app(Settings::default(), &store);
        player.seek(0.0);
        player.play();
        for _ in 0..120 {
            tour_frame(
                &mut app,
                &mut present,
                &mut player,
                &mut baro,
                &mut store,
                &mut nav_plan,
                &reader,
                false,
                &mut frame_no,
                "ambient",
            );
        }

        // --- The "See the climb ahead" demo (`enter` → Back-cycle to the Climb park). Runs first,
        // while route 0 is still the demo climb (after the reroute demo below, `_nav.obcr` sorts
        // to index 0 and no climb is active, so Climb would leave the Back-cycle). ---
        app = build_app(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() }, &store);
        player.seek(1500.0);
        player.play();
        until_then_dwell!(&mut app, "climb: Map", Screen::Map(_), 300);
        app.apply_gesture(Gesture::Back);
        until_then_dwell!(&mut app, "climb: Statistics", Screen::Statistics(_), 300);
        app.apply_gesture(Gesture::Back);
        until_then_dwell!(&mut app, "climb: Climb", Screen::Climb(_), 300);
        app.apply_gesture(Gesture::Back);
        until_then_dwell!(&mut app, "climb: Map again", Screen::Map(_), 60);

        // --- The "Reroute to a POI" demo. Its `enter` is a demo-style reset from deep in the
        // previous demo's session — the app rebuild plus a BACKWARD `GpxPlayer::seek`. ---
        app = build_app(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() }, &store);
        player.seek(1500.0);
        player.play();
        until_then_dwell!(&mut app, "reroute: Map", Screen::Map(_), 60);
        app.apply_gesture(Gesture::BackHold);
        until_then_dwell!(&mut app, "reroute: Menu", Screen::Menu(_), 300);
        app.apply_gesture(Gesture::Turn(2));
        app.apply_gesture(Gesture::Press);
        until_then_dwell!(&mut app, "reroute: PoiMenu", Screen::PoiMenu(_), 45);
        app.apply_gesture(Gesture::Turn(2));
        app.apply_gesture(Gesture::Press);
        until_then_dwell!(&mut app, "reroute: PoiList", Screen::PoiList(_), 300);
        app.apply_gesture(Gesture::Press);
        until_then_dwell!(&mut app, "reroute: PoiDetail", Screen::PoiDetail(_), 300);
        app.apply_gesture(Gesture::Press);
        until_then_dwell!(&mut app, "reroute: NavConfirm", Screen::NavConfirm(_), 45);
        app.apply_gesture(Gesture::Press);
        // The frame-stepped planner runs inside the `until` frames; grimsel routes fine, so the
        // outcome must be the computed-route overview, parked like the page's final step.
        until_then_dwell!(&mut app, "reroute: RouteOverview", Screen::RouteOverview(_), 300);

        // --- Back to the interactive page: ambient reset (seek 0 — a big backward jump). ---
        app = build_app(Settings::default(), &store);
        player.seek(0.0);
        player.play();
        for _ in 0..300 {
            tour_frame(
                &mut app,
                &mut present,
                &mut player,
                &mut baro,
                &mut store,
                &mut nav_plan,
                &reader,
                false,
                &mut frame_no,
                "ambient reset",
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #626 regression (b): a demo-style reset — the mid-session `App` rebuild plus a
    /// `GpxPlayer::seek` that `enter_tour_baseline` / `enter_ambient` perform — followed by
    /// repeated presents, twice (forward to mid-climb, then backward to the start). Every present
    /// runs under the oracle + the byte-equality postcondition in [`tour_frame`].
    #[test]
    fn demo_reset_rebuild_and_seek_present_clean() {
        use std::path::Path;

        use obc_app::settings::{ClimbMode, Settings};
        use obc_app::{App, AppState, CameraMode};
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};
        use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

        const W: u32 = obc_platform::FRAME_W as u32;
        const H: u32 = obc_platform::FRAME_H as u32;
        let bytes = include_bytes!("../assets/grimsel.obcm").to_vec();
        let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let reader = Reader::new(&src, &tables, &cache);

        let dir = std::env::temp_dir().join(format!("obc626-reset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp routes dir");
        std::fs::write(dir.join("grimsel-climb.obcr"), include_bytes!("../assets/grimsel-climb.obcr"))
            .expect("seed demo route");
        let mut store = crate::routes::RouteStore::open(&dir);

        let track =
            Track::load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/grimsel-climb.gpx"))).expect("gpx");
        let mut player = GpxPlayer::new(track);
        player.set_speed(3.0);
        let mut baro = BaroSensor::new();
        let mut nav_plan: Option<crate::NavPlan> = None;
        let mut present = Present::new(W, H);
        let mut frame_no = 0usize;

        let (cx, cy, zoom) = crate::initial_camera(&reader, W);
        let build_app = |settings: Settings, store: &crate::routes::RouteStore| {
            let mut state = AppState::new(cx, cy, zoom * 12.0);
            state.mode = CameraMode::Follow;
            state.heading_up = true;
            let mut app = App::new(state);
            app.set_nav_profiles(tables.nav_profiles());
            app.set_routes_with_ids(store.catalog(), store.ids());
            app.set_settings(settings);
            if !store.catalog().is_empty() {
                app.activity.active_route = Some(0);
                app.activity.start_session();
            }
            app
        };
        let mut run = |app: &mut App,
                       present: &mut Present,
                       player: &mut GpxPlayer,
                       baro: &mut BaroSensor,
                       store: &mut crate::routes::RouteStore,
                       nav_plan: &mut Option<crate::NavPlan>,
                       tour: bool,
                       n: usize,
                       label: &str| {
            for _ in 0..n {
                tour_frame(app, present, player, baro, store, nav_plan, &reader, tour, &mut frame_no, label);
            }
        };

        // A short ambient ride, then the `enter` reset: rebuild + seek forward to mid-climb.
        let mut app = build_app(Settings::default(), &store);
        player.seek(0.0);
        player.play();
        run(&mut app, &mut present, &mut player, &mut baro, &mut store, &mut nav_plan, false, 60, "ambient");
        app = build_app(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() }, &store);
        player.seek(1500.0);
        player.play();
        run(&mut app, &mut present, &mut player, &mut baro, &mut store, &mut nav_plan, true, 300, "after enter");

        // The `ambient` reset: rebuild + seek backward to the start.
        app = build_app(Settings::default(), &store);
        player.seek(0.0);
        player.play();
        run(&mut app, &mut present, &mut player, &mut baro, &mut store, &mut nav_plan, false, 300, "after ambient");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn texture_tracks_a_sequence_of_partial_changes() {
        let mut p = Present::new(3, 6);
        present_fill(&mut p, 0x04, None);
        // A few disjoint edits across frames; after each, only the one changed row is pushed and the
        // texture reconstructs it (partial pushes reconstruct the whole — the load-bearing property).
        for (row, val) in [(0usize, 0x08u8), (5, 0x0C), (3, 0x10)] {
            p.fb_mut()[row * 3..(row + 1) * 3].fill(val);
            block_on(p.present(None));
            assert_eq!(p.stats.pushed_rows, 1, "only row {row} pushed");
            let (r, g, b) = expect_px(val);
            let t = row * 3 * 3;
            assert_eq!((p.texture()[t], p.texture()[t + 1], p.texture()[t + 2]), (r, g, b), "row {row} in texture");
        }
    }
}
