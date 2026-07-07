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
    /// property), clip each changed span around a live overlay's `exclude` rows, assert the oracle is
    /// satisfied, then reconstruct the pushed rows into `presented` + the texture. Always `true` — a
    /// host texture write never faults.
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

        // 3. Oracle: independently compute the rows that *actually* changed and assert the pushed
        //    spans (plus the overlay's excluded rows, which the overlay plane owns) covered every
        //    one. First present: `presented` is all-black, forced full-frame span.
        let mut oracle_spans = spans.clone();
        if let Some(ex) = exclude {
            oracle_spans.push(ex);
        }
        let missed = spans_missed_changes(&self.presented, &self.fb, stride, rows, &oracle_spans, &mut self.covered);
        debug_assert_eq!(missed, 0, "self-diff missed {missed} changed row(s) — a systematic hash-diff bug");

        // 4. Push only the changed spans into `presented` (device-64) and the texture (RGB888). The
        //    displayed texture is this partial-push buffer.
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
        use obc_app::{App, AppState, InputClock};
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
        assert!(pan > H as usize / 2, "a map pan pushes ~all rows, got {pan}");
    }

    /// Real-data pack→parse of the POI section (#423): the committed `monaco.obcm` — a POI-dense
    /// coastal fixture the packer produced from a real OSM extract — must parse as v8 and expose a
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
        let tables = MapTables::parse(&src).expect("monaco.obcm parses as a valid v8 map");
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);

        assert_eq!(r.version, 8, "the fixture is OBCM v8");
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
