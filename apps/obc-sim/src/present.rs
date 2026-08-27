//! The simulator's **display presenter** — the host stand-in for the device's LS021/FLPR panel, and
//! the second live backend of the generic display contracts
//! ([`Presenter`]/[`OverlayPresenter`] in `obc_display::display_contracts`).
//!
//! Like the device, the simulator keeps one resident **RGB222 / device-64** frame (one byte per
//! pixel, `0b00_RR_GG_BB`) that the shared renderer draws the whole frame into — owned by the host
//! *next to* this presenter, per the contracts' borrow model. [`Present::present_now`] then pushes
//! it, driving the *same* self-diffing [`diff_rows`](obc_display::ls021::diff_rows) core the
//! device does — re-pushing only the rows whose per-row hash changed, honouring a live overlay's
//! exclude span with the *same* [`clip_span`](obc_display::ls021::clip_span) — and
//! [`Present::present_overlay_now`] composites through the *same*
//! [`composite_overlay_window`](obc_display::composite_overlay_window) helper. The only
//! host-specific step is expanding the pushed device-64 rows to the RGB888 texture egui uploads
//! (via [`device64_to_rgb565`](obc_display::device64_to_rgb565) → RGB888 — exactly the ramp the
//! panel shows).
//!
//! On top of the device's path it does what the device can't afford: it keeps a full copy of the
//! last-presented frame, independently computes which rows *actually* changed, and asserts the
//! hash-diff's spans covered every one ([`spans_missed_changes`](obc_display::ls021::spans_missed_changes))
//! — the exact-diff **oracle** that catches any *systematic* diff bug in CI (a real FNV-1a collision
//! is ~2⁻³² per row-change and self-healing). Because the uploaded texture is reconstructed from the
//! partial pushes (mutated **only on changed spans**), a diff bug also surfaces as a stale row on
//! glass, not just a failed assert.
//!
//! ## Contract impls vs. the runtime-sized GUI
//!
//! The contracts type frame geometry at compile time ([`Device64Frame`]`<W, H>`), but the sim's
//! device resolution is a **runtime CLI knob** (`--size WxH`) — so the GUI loop drives the
//! presenter through the inherent [`present_now`](Present::present_now) /
//! [`present_overlay_now`](Present::present_overlay_now) engine, and the
//! [`Presenter`]/[`OverlayPresenter`] impls (generic over any `Device64Frame<W, H>`) are one-line
//! delegations to those very bodies. The contract surface is exercised end-to-end by the
//! conformance suite below at multiple geometries — including the shipping 240×320 — so the engine
//! the GUI runs is exactly the one the contracts certify. Damage and region speak the shared
//! LS021-pairing vocabulary ([`RowDamage`]/[`RowWindow`]) — the same strategy types the board
//! presenter uses.
//!
//! The contract methods are `async`; on the host they complete synchronously (a texture write never
//! faults), so the tests drive them with a minimal `pollster::block_on`.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use obc_display::display_contracts::{Device64Frame, OverlayPresenter, Presenter};
use obc_display::ls021::{clip_span, diff_rows, row_hash, spans_missed_changes, RowDamage, RowWindow};
use obc_display::{composite_overlay_window, device64_to_rgb565, Band};
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

/// The simulator's presenter: the per-row hash store, the partial-push reconstruction (oracle prev
/// **and** the source of the uploaded texture), and the RGB888 texture itself. The resident
/// device-64 frame lives with the host (the GUI's `Vec<u8>`; a [`Device64Frame`] in the contract
/// tests), passed into each present — the contracts' render-vs-present borrow split. Runtime-sized
/// `Vec`s (the sim window size is a CLI knob) rather than the device's fixed `[u32; HEIGHT]`, but
/// driven by the identical [`diff_rows`] core at the identical stride (one byte per pixel).
pub struct Present {
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
    /// exact scenario the diagnostics exist for — presents run at frame rate, so an undeduped log
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
    /// A presenter for a `width`×`height` frame, primed empty (the first present pushes the whole
    /// frame). The resident plane it presents is device-64 (one byte per pixel), like the device.
    pub fn new(width: u32, height: u32) -> Self {
        let width = width as usize;
        let rows = height as usize;
        Present {
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
    /// its egui texture. The committed shell snapshot registry uses the device-64 `--png` path.
    /// `width * height * 3` bytes.
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

    /// Self-diff the resident frame `fb` and "push" only the changed rows, exactly as the device
    /// does: diff every row against the hash store (updating it for **all** rows — the self-healing
    /// property), clip each changed span around a live overlay's `exclude` rows, reconstruct the
    /// pushed rows into `presented` + the texture, then assert the oracle is satisfied — push
    /// first, so a failed oracle can't desync later frames (#626). The runtime-geometry engine
    /// behind the contract's [`Presenter::present`]; a host texture write never faults.
    pub fn present_now(&mut self, fb: &[u8], exclude: Option<(u16, u16)>) {
        let (stride, rows) = (self.width, self.rows);
        debug_assert!(fb.len() >= stride * rows, "frame shorter than the presenter's geometry");

        // 1. Diff against the hash store → contiguous changed-row spans. The store updates to this
        //    frame for every row (pushed or not) — the self-healing property.
        let mut raw: Vec<(u16, u16)> = Vec::new();
        diff_rows(fb, stride, &mut self.hashes, !self.primed, row_hash, |y0, n| raw.push((y0, n)));
        self.primed = true;

        // 2. Clip each changed span around a live overlay's rows (`exclude`) — the same
        //    bulge-coordination clip the device's present runs. The excluded rows belong to the
        //    overlay plane this frame, so they are not pushed here. `clip_span` takes a half-open
        //    interval `[e0, e1)`, so convert the `(y0, rows)` overlay span the same way the
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
                self.presented[r.clone()].copy_from_slice(&fb[r]);
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
        let missed = spans_missed_changes(&self.presented, fb, stride, rows, &oracle_spans, &mut self.covered);
        if missed != 0 {
            // Diagnostics before the assert (kept in release too, where the assert compiles out and
            // the miss would otherwise be a silent stale row): which row, its stored hash, and the
            // differing column range. `covered` still holds the oracle's span coverage. Deduped per
            // row — a missed row on a parked screen stays byte-different until it next changes, so
            // in release this branch runs every frame; log only when a row's miss FIRST appears.
            for (y, &cov) in self.covered[..rows].iter().enumerate() {
                let r = y * stride..y * stride + stride;
                let missing = !cov && self.presented[r.clone()] != fb[r.clone()];
                match (missing, self.miss_reported[y]) {
                    (true, false) => {
                        let differs = |c: &usize| self.presented[r.start + c] != fb[r.start + c];
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
    }

    /// Re-present `region` with `draw_overlay` composited over the **clean framebuffer backdrop** —
    /// through the shared [`composite_overlay_window`], exactly as the device does. The clean
    /// device-64 `fb` is never written (the overlay is transient chrome).
    ///
    /// Like the row-addressed panel, the push re-latches the **full-width rows** `[y0, y0 + rows)`
    /// from the clean frame (the device's FLPR scans the whole composited row): `presented` (the
    /// clean-glass reconstruction, the oracle's `prev`) takes the clean `fb` bytes for those rows,
    /// the texture re-expands them, and only the `[x0, x0 + w)` columns then carry the composited
    /// overlay. That is what keeps the excluded rows' glass tracking the clean frame through an
    /// around-present + trailing clear, exactly as on the device. The runtime-geometry engine
    /// behind the contract's [`OverlayPresenter::present_overlay`].
    pub fn present_overlay_now(&mut self, fb: &[u8], region: RowWindow, draw_overlay: &mut dyn FnMut(&mut Band)) {
        let (w, rows) = (region.w as usize, region.rows as usize);
        let frame = Size::new(self.width as u32, self.rows as u32);
        let window = Rectangle::new(
            Point::new(region.x0 as i32, region.y0 as i32),
            Size::new(region.w as u32, region.rows as u32),
        );
        // Composite the overlay over the clean device-64 backdrop into an RGB565 scratch — the one
        // piece byte-for-byte identical to the device's overlay push.
        let mut scratch = vec![0u16; w * rows];
        composite_overlay_window(fb, frame, window, &mut scratch, draw_overlay);
        // Re-latch the full-width rows from the clean frame (the device pushes whole composited
        // rows), then blit the composited window columns over them in the texture (RGB565 →
        // RGB888). `fb` itself is never written.
        for r in 0..rows {
            let fy = region.y0 as usize + r;
            let row = fy * self.width..(fy + 1) * self.width;
            self.presented[row.clone()].copy_from_slice(&fb[row]);
            self.expand_row_to_tex(fy);
            for c in 0..w {
                let fx = region.x0 as usize + c;
                let (rr, gg, bb) = rgb565_to_rgb888(scratch[r * w + c]);
                let t = (fy * self.width + fx) * 3;
                self.tex[t] = rr;
                self.tex[t + 1] = gg;
                self.tex[t + 2] = bb;
            }
        }
    }
}

// ── The display contracts: the sim presenter paired with `Device64Frame<W, H>` (any geometry —
//    the tests below run both tiny frames and the device's 240×320). One-line delegations to the
//    runtime engine above, speaking the LS021 pairing's damage/region vocabulary — the same
//    strategy types the board presenter uses. The frame type's geometry must match the presenter's
//    constructed geometry (checked on every present in debug builds). ──

impl<'b, const W: usize, const H: usize> Presenter<Device64Frame<'b, W, H>> for Present {
    type Damage = RowDamage;
    /// A host texture write never faults.
    type Error = core::convert::Infallible;

    fn damage_full() -> RowDamage {
        RowDamage::Full
    }

    fn damage_unknown() -> RowDamage {
        RowDamage::SelfDiff { exclude: None }
    }

    async fn present(
        &mut self,
        frame: &Device64Frame<'b, W, H>,
        damage: RowDamage,
    ) -> Result<obc_display::display_contracts::PresentStats, Self::Error> {
        debug_assert!(W == self.width && H == self.rows, "frame type geometry != presenter geometry");
        let exclude = match damage {
            RowDamage::Full => {
                // Full = re-seed the store + push every row (the recovery/first-present damage).
                self.primed = false;
                None
            }
            RowDamage::SelfDiff { exclude } => exclude,
        };
        self.present_now(frame.bytes(), exclude);
        Ok(obc_display::display_contracts::PresentStats {
            pushed_units: self.stats.pushed_rows as u32,
            total_units: self.stats.total_rows as u32,
            regions: self.stats.spans as u32,
        })
    }
}

impl<'b, const W: usize, const H: usize> OverlayPresenter<Device64Frame<'b, W, H>> for Present {
    type Region = RowWindow;
    type OverlayTarget<'t> = Band<'t>;

    fn region(rect: Rectangle) -> RowWindow {
        RowWindow::from_rect(rect, W as u32, H as u32)
    }

    fn damage_around(region: RowWindow) -> RowDamage {
        RowDamage::SelfDiff { exclude: Some(region.exclude_span()) }
    }

    async fn present_overlay(
        &mut self,
        frame: &mut Device64Frame<'b, W, H>,
        region: RowWindow,
        draw: impl for<'t> FnOnce(&mut Band<'t>),
    ) -> Result<obc_display::display_contracts::PresentStats, Self::Error> {
        debug_assert!(W == self.width && H == self.rows, "frame type geometry != presenter geometry");
        let mut draw = Some(draw);
        self.present_overlay_now(frame.bytes(), region, &mut |band| {
            if let Some(d) = draw.take() {
                d(band)
            }
        });
        Ok(obc_display::display_contracts::PresentStats {
            pushed_units: region.rows as u32,
            total_units: self.rows as u32,
            regions: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use embedded_graphics::pixelcolor::raw::RawU16;
    use embedded_graphics::pixelcolor::Rgb565;
    use obc_display::display_contracts::conformance::{self, GlassProbe};
    use obc_display::ls021::{FRAME_H, FRAME_W};
    #[cfg(feature = "external-fixtures")]
    use obc_display::FbDevice64;
    use pollster::block_on;

    use super::*;

    /// The sim's "glass" is the RGB888 texture; read it back in the draw colour space. The RGB565 →
    /// RGB888 expansion is bit-replication, so truncation inverts it exactly.
    impl<'b, const W: usize, const H: usize> GlassProbe<Device64Frame<'b, W, H>> for Present {
        fn glass(&self, x: u32, y: u32) -> Rgb565 {
            let t = (y as usize * self.width + x as usize) * 3;
            let (r, g, b) = (self.tex[t], self.tex[t + 1], self.tex[t + 2]);
            Rgb565::new(r >> 3, g >> 2, b >> 3)
        }
    }

    /// Drive a present over a freshly-filled device-64 frame `v`.
    fn present_fill(fb: &mut [u8], p: &mut Present, v: u8, exclude: Option<(u16, u16)>) {
        fb.fill(v);
        p.present_now(fb, exclude);
    }

    /// The RGB888 the texture should hold for a device-64 byte.
    fn expect_px(byte: u8) -> (u8, u8, u8) {
        rgb565_to_rgb888(device64_to_rgb565(byte))
    }

    fn rgb(raw: u16) -> Rgb565 {
        Rgb565::from(RawU16::new(raw))
    }

    // ── The generic conformance suite (the same checks the board-semantics double and the proof
    //    backend run in obc-platform) against THIS backend — the simulator presenter paired with
    //    `Device64Frame`, through the real contract impls. ──

    const CW: usize = 16;
    const CH: usize = 16;
    /// The reference overlay window: a right-edge 4×4 rect (the bulge shape, scaled down).
    const OVERLAY: Rectangle = Rectangle { top_left: Point::new(12, 4), size: Size::new(4, 4) };
    const RED: u16 = 0xF800;
    const GREEN: u16 = 0x07E0;
    const BLUE: u16 = 0x001F;

    #[test]
    fn conformance_full_present() {
        let mut buf = [0u8; CW * CH];
        let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
        let mut p = Present::new(CW as u32, CH as u32);
        block_on(conformance::check_full_present(&mut frame, &mut p, rgb(RED), rgb(BLUE)));
    }

    #[test]
    fn conformance_damage_translation() {
        let mut buf = [0u8; CW * CH];
        let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
        let mut p = Present::new(CW as u32, CH as u32);
        block_on(conformance::check_damage_translation(&mut frame, &mut p, rgb(RED), rgb(BLUE), true));
    }

    #[test]
    fn conformance_overlay_backdrop() {
        let mut buf = [0u8; CW * CH];
        let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
        let mut p = Present::new(CW as u32, CH as u32);
        block_on(conformance::check_overlay_backdrop(
            &mut frame,
            &mut p,
            rgb(RED),
            rgb(BLUE),
            OVERLAY,
            (13, 5),
            (14, 6),
            |f| f.bytes().to_vec(),
        ));
    }

    #[test]
    fn conformance_overlay_exclusion() {
        let mut buf = [0u8; CW * CH];
        let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
        let mut p = Present::new(CW as u32, CH as u32);
        block_on(conformance::check_overlay_exclusion(
            &mut frame,
            &mut p,
            rgb(RED),
            rgb(BLUE),
            rgb(GREEN),
            OVERLAY,
            (13, 5),
            (14, 6),
            (0, 0),
            |f| f.bytes().to_vec(),
            true,
        ));
    }

    #[test]
    fn conformance_overlay_pop_retract_clear() {
        let mut buf = [0u8; CW * CH];
        let mut frame = Device64Frame::<CW, CH>::new(&mut buf);
        let mut p = Present::new(CW as u32, CH as u32);
        block_on(conformance::check_overlay_pop_retract_clear(
            &mut frame,
            &mut p,
            rgb(RED),
            rgb(BLUE),
            OVERLAY,
            (12, 5),
            (15, 6),
            |f| f.bytes().to_vec(),
            true,
        ));
    }

    /// The device-geometry contract pairing: the conformance exclusion check at the shipping
    /// 240×320 with the real bulge window shape, so the trait path is proven at the geometry the
    /// GUI actually runs.
    #[test]
    fn conformance_overlay_exclusion_at_device_geometry() {
        let mut buf = vec![0u8; FRAME_W * FRAME_H];
        let mut frame = Device64Frame::<FRAME_W, FRAME_H>::new(&mut buf);
        let mut p = Present::new(FRAME_W as u32, FRAME_H as u32);
        // The real bulge shape: a right-edge 16-column window on rows 60..171.
        let overlay = Rectangle::new(Point::new((FRAME_W - 16) as i32, 60), Size::new(16, 111));
        block_on(conformance::check_overlay_exclusion(
            &mut frame,
            &mut p,
            rgb(RED),
            rgb(BLUE),
            rgb(GREEN),
            overlay,
            (FRAME_W as u32 - 8, 100),
            (FRAME_W as u32 - 2, 140),
            (0, 0),
            |f| f.bytes().to_vec(),
            true,
        ));
    }

    #[test]
    fn geometry_matches_the_platform_authority() {
        // The sim's device resolution is the single ls021 authority, not a re-declared literal.
        assert_eq!(FRAME_W, 240);
        assert_eq!(FRAME_H, 320);
        let p = Present::new(FRAME_W as u32, FRAME_H as u32);
        assert_eq!(p.width, FRAME_W);
        assert_eq!(p.rows, FRAME_H);
    }

    #[test]
    fn first_present_pushes_the_whole_frame() {
        let mut fb = vec![0u8; 2 * 4];
        let mut p = Present::new(2, 4);
        present_fill(&mut fb, &mut p, 0x15, None);
        assert_eq!(p.stats.pushed_rows, 4, "the first present pushes the whole frame");
        assert_eq!(p.stats.spans, 1);
        // The texture reconstructs every pixel from the device-64 byte.
        let (r, g, b) = expect_px(0x15);
        assert!(
            p.texture().as_chunks::<3>().0.iter().all(|px| *px == [r, g, b]),
            "texture reconstructs the whole frame"
        );
    }

    #[test]
    fn identical_reframe_pushes_nothing() {
        let mut fb = vec![0u8; 2 * 4];
        let mut p = Present::new(2, 4);
        present_fill(&mut fb, &mut p, 0x15, None);
        present_fill(&mut fb, &mut p, 0x15, None);
        assert_eq!(p.stats.pushed_rows, 0, "a byte-identical reframe pushes nothing");
        assert_eq!(p.stats.spans, 0);
    }

    #[test]
    fn only_the_changed_band_is_pushed_and_texture_reconstructs_it() {
        let mut fb = vec![0u8; 2 * 5];
        let mut p = Present::new(2, 5);
        present_fill(&mut fb, &mut p, 0x00, None);
        // Change only row 2 (device-64 stride = width = 2).
        fb[2 * 2..3 * 2].fill(0x2A);
        p.present_now(&fb, None);
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
        let mut fb = vec![0u8; 2 * 5];
        let mut p = Present::new(2, 5);
        present_fill(&mut fb, &mut p, 0x00, None);
        for y in 1..=3 {
            fb[y * 2..(y + 1) * 2].fill(0x11);
        }
        p.present_now(&fb, Some((2, 2)));
        assert_eq!(p.stats.pushed_rows, 1, "the excluded rows are not pushed by the clean present");
        assert_eq!(p.stats.spans, 1);
        // The oracle inside present_now proved the pushed span + the exclude span cover every real
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
        let mut fb = vec![0u8; 8 * 8];
        let mut p = Present::new(8, 8);
        for (i, b) in fb.iter_mut().enumerate() {
            *b = (i as u8) & 0b0011_1111;
        }
        // Seed the texture from a full present so unrelated pixels are defined.
        p.present_now(&fb, None);
        let fb_snapshot = fb.clone();
        // Window cols [4,8) × rows [2,6); the drawer paints frame-absolute (5,3) red.
        let region = RowWindow { x0: 4, y0: 2, w: 4, rows: 4 };
        p.present_overlay_now(&fb, region, &mut |band: &mut Band| {
            band.fill_solid(&Rectangle::new(Point::new(5, 3), Size::new(1, 1)), Rgb565::from(RawU16::new(0xF800))).ok();
        });
        let tex_at = |x: usize, y: usize| {
            let t = (y * 8 + x) * 3;
            (p.texture()[t], p.texture()[t + 1], p.texture()[t + 2])
        };
        // Backdrop: frame (4,2) = fb byte 2*8+4 = 20, expanded to RGB888.
        assert_eq!(tex_at(4, 2), expect_px(20), "backdrop = clean fb expanded to RGB888");
        // Overlay: frame (5,3) painted pure red.
        assert_eq!(tex_at(5, 3), (255, 0, 0), "the drawer painted frame-absolute (5,3) red");
        // The clean framebuffer is never written by the overlay path.
        assert_eq!(fb, fb_snapshot, "present_overlay never writes the resident frame");
    }

    /// Driven through the real [`App`] + renderer over the demo map into the device-64 frame: an
    /// idle Home re-render pushes **zero** rows, a Home minute tick only the **clock's** rows, and a
    /// map pan **~all**. The oracle inside [`Present::present_now`] backs every count.
    #[test]
    #[cfg(feature = "external-fixtures")]
    fn app_scenarios_idle_is_free_tick_is_small_pan_is_most() {
        use obc_app::{App, AppState};
        use obc_ports::InputClock;
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};

        // The device resolution is the single ls021 authority, not a re-declared literal.
        const W: u32 = FRAME_W as u32;
        const H: u32 = FRAME_H as u32;
        let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm").expect("full fixture suite requires map");
        let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let reader = Reader::new(&src, &tables, &cache);
        let (cx, cy, zoom) = crate::initial_camera(&reader, W);

        // Render the whole frame into the resident device-64 plane, exactly as the GUI loop does —
        // the device color path (`Rgb565` → device-64 pack), not an RGB888 side buffer.
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        // The host's render scratch, built once and lent to every frame below (#1146).
        let mut scratch = Box::new(obc_render::RenderScratch::new());
        let mut render = |app: &mut App, fb: &mut [u8]| {
            let mut fbdev = FbDevice64::new(fb, W, H);
            app.render_frame(Some(&mut scratch), &mut fbdev, &reader, None, W as f32, H as f32, color_fn);
        };

        // --- Home idle + a minute tick ---
        let mut app = App::new_idle(AppState::new(cx, cy, zoom));
        app.reseed_home(0); // pin the contour backdrop so only the clock moves
        let mut fb = vec![0u8; (W * H) as usize];
        let mut present = Present::new(W, H);

        app.advance_animations(InputClock(0));
        render(&mut app, &mut fb);
        present.present_now(&fb, None); // first present: the whole frame
        render(&mut app, &mut fb);
        present.present_now(&fb, None);
        let idle = present.stats.pushed_rows;

        app.advance_animations(InputClock(60_000)); // +1 min: 12:00 → 12:01
        render(&mut app, &mut fb);
        present.present_now(&fb, None);
        let tick = present.stats.pushed_rows;

        // --- A fresh map, then a pan ---
        let mut app = App::new(AppState::new(cx, cy, zoom));
        let mut fb = vec![0u8; (W * H) as usize];
        let mut present = Present::new(W, H);
        app.advance_animations(InputClock(0));
        render(&mut app, &mut fb);
        present.present_now(&fb, None); // first present: the whole frame
        let span_lon = (reader.bbox.max_lon as i64 - reader.bbox.min_lon as i64).max(1);
        app.state.cam_lon = app.state.cam_lon.wrapping_add((span_lon / 4) as i32); // pan a quarter-map
        render(&mut app, &mut fb);
        present.present_now(&fb, None);
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
    /// coastal fixture the packer produced from a real OSM extract — must parse at the current format version and expose a
    /// full six-category POI directory with several **non-empty** categories, each carrying a real
    /// quadtree (non-zero node + chunk counts), plus a populated §8 nav graph (#464). This
    /// complements the reader's hand-built byte pins (`obc-reader/tests/format.rs`) by exercising
    /// the whole write→read path on real geometry, and gives the #425 POI browser a map with POIs
    /// to browse in the sim/snapshot suite.
    #[test]
    #[cfg(feature = "external-fixtures")]
    fn monaco_fixture_parses_populated_poi_and_nav_sections() {
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};

        let bytes = obc_fixtures::read("sim-monaco", "monaco.obcm").expect("full fixture suite requires map");
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).expect("monaco.obcm parses as a valid v11 map");
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);

        assert_eq!(r.version, obc_formats::obcm::VERSION, "the fixture is the OBCM version this build reads");
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

        let mut fb = vec![0u8; 4 * 6];
        let mut p = Present::new(4, 6);
        present_fill(&mut fb, &mut p, 0x01, None);
        // Row 2 changes but its store hash is (wrongly) already up to date — a simulated collision.
        fb[2 * 4..3 * 4].fill(0x22);
        p.hashes[2] = row_hash(&fb[2 * 4..3 * 4]);
        // Row 4 changes honestly in the same frame.
        fb[4 * 4..5 * 4].fill(0x2A);
        let outcome = catch_unwind(AssertUnwindSafe(|| p.present_now(&fb, None)));
        if cfg!(debug_assertions) {
            assert!(outcome.is_err(), "the oracle assert fires on the fabricated miss");
        }
        // The reorder guarantee: the frame landed before the assert aborted the present.
        assert_eq!(&p.presented[4 * 4..5 * 4], &[0x2A; 4], "the honest row was pushed despite the failed oracle");
        // The stale row heals the moment it changes again — and nothing else re-asserts.
        fb[2 * 4..3 * 4].fill(0x30);
        p.present_now(&fb, None);
        assert_eq!(p.presented, fb, "one missed row healed itself; no cascade");
    }

    /// The oracle's miss diagnostics are deduped per row: a missed row on a parked static screen
    /// stays byte-different every frame (in `--release` nothing stops the loop), so the report must
    /// fire once when the miss appears, stay quiet while it persists, and re-arm when the row
    /// heals. The `miss_reported` flags gate the log line 1:1, so pin the flag lifecycle.
    #[test]
    fn miss_diagnostics_are_deduped_per_row() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let mut fb = vec![0u8; 4 * 6];
        let mut p = Present::new(4, 6);
        present_fill(&mut fb, &mut p, 0x01, None);
        // Fabricate a persistent miss on row 2 (a collision's aftermath, as in the cascade test).
        fb[2 * 4..3 * 4].fill(0x22);
        p.hashes[2] = row_hash(&fb[2 * 4..3 * 4]);
        // First present: the miss appears → reported (the debug assert also fires; catch it).
        let _ = catch_unwind(AssertUnwindSafe(|| p.present_now(&fb, None)));
        assert!(p.miss_reported[2], "the new miss is reported");
        assert_eq!(p.misses_flagged, 1);
        // The screen stays parked: the same miss persists — no re-report (the flag stays set).
        for _ in 0..3 {
            let _ = catch_unwind(AssertUnwindSafe(|| p.present_now(&fb, None)));
            assert!(p.miss_reported[2], "the persisting miss stays flagged, not re-reported");
            assert_eq!(p.misses_flagged, 1, "no duplicate report accumulates");
        }
        // The row changes → re-pushed, healed: the report re-arms.
        fb[2 * 4..3 * 4].fill(0x30);
        p.present_now(&fb, None);
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
        let mut fb = vec![0u8; 8 * 4];
        let mut p = Present::new(8, 4);
        present_fill(&mut fb, &mut p, 0x00, None);
        // Row 2: pixels x=3 → 0x02 and x=7 → 0x2E (a measured colliding pair of the old hash).
        fb[2 * 8 + 3] = 0x02;
        fb[2 * 8 + 7] = 0x2E;
        p.present_now(&fb, None);
        assert_eq!(p.stats.pushed_rows, 1, "the lane-3-confined change must be diffed and pushed");
        let r = 2 * 8..3 * 8;
        assert_eq!(p.presented[r.clone()], fb[r], "row 2 reconstructed");
    }

    /// One full sim frame, exactly the `gui.rs::render_to_texture` skeleton: drain nav
    /// request/cancel, step an in-flight route plan, open the active route, advance the GPX replay +
    /// tick, render into the resident device-64 plane, then present. After presenting it asserts
    /// the full byte-equality postcondition and panics with diagnostics on the FIRST miss.
    #[allow(clippy::too_many_arguments)]
    #[cfg(feature = "external-fixtures")]
    fn tour_frame(
        app: &mut obc_app::App,
        scratch: &mut obc_render::RenderScratch,
        fb: &mut [u8],
        present: &mut Present,
        player: &mut obc_replay::GpxPlayer,
        baro: &mut obc_replay::BaroSensor,
        store: &mut crate::routes::RouteStore,
        host: &mut obc_host_core::HostLoop,
        session: &mut obc_host_core::ActiveRouteSession,
        reader: &obc_reader::Reader,
        tour_active: bool,
        frame_no: &mut usize,
        label: &str,
    ) {
        use obc_route::RouteReader;

        const W: u32 = FRAME_W as u32;
        const H: u32 = FRAME_H as u32;

        // This tour uses only routes, so the ride/track/trip repositories are empty stand-ins and
        // the platform has nothing of its own to do.
        let mut rides = obc_host_core::MemRideStore::new(Vec::new());
        let mut tracks = obc_host_core::MemTrackStore::new();
        let mut no_trips = ();
        // The tour drives geometry, not terrain: the null source keeps its frames byte-comparable
        // with the pre-EL7 ones.
        let mut elev = obc_route::NullElevation;

        // Open the active route's geometry from the resident session (gui.rs's per-frame open) and
        // run one DeviceCore pass over it.
        //
        // The **UI clock stands still at zero**, which is what this tour has always run at: it
        // drives gestures directly and never had an animation clock, and the ambient reset seeks the
        // replay *backwards*, so a UI clock taken from playback time would run backwards with it.
        // A still clock advances no needle and arms no idle return — the tour asserts presents, not
        // animation phases.
        session.sync(app, store);
        let mut plan = {
            let route_src = store.active_source();
            let route = match (session.index(), route_src.as_ref()) {
                (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
                _ => None,
            };
            let (ride, sensors) =
                obc_host_core::replay_advance(player, baro, None, 1.0 / 60.0, None, Default::default());
            host.pass(
                app,
                obc_app::device_core::PassClock { ride, ui: obc_ports::InputClock(0) },
                &[],
                sensors,
                route.as_ref(),
                // The present tour mounts no weather store, so the domain has nothing to derive
                // from and stage 10 collapses its view state — which is what a host with no bundle
                // looks like.
                None,
                crate::gui::SIM_SUPPORT,
            )
        };
        // The typed executor — the same `obc-host-core::HostLoop` gui.rs drives (the route planner's
        // lifecycle, one bounded step per frame).
        host.execute(
            app,
            &mut plan,
            session,
            store,
            &mut rides,
            &mut tracks,
            &mut no_trips,
            reader,
            &mut elev,
            &mut (),
        );

        // The wasm demo's ambient auto-restart (suppressed while a tour runs — the branch's
        // `!tour_active` gate).
        if !tour_active && !player.is_playing() {
            player.play();
            app.recorder.request(obc_app::RecorderIntent::Start);
        }

        // Re-open the route for the render: the executor may have committed new geometry under it.
        session.sync(app, store);
        let route_src = store.active_source();
        let route = match (session.index(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };

        // Render the whole frame into the resident device-64 plane.
        let mut fbdev = FbDevice64::new(fb, W, H);
        app.render_frame(Some(scratch), &mut fbdev, reader, route.as_ref(), W as f32, H as f32, |c| {
            Rgb565::from(RawU16::new(c))
        });

        // Present (the oracle inside asserts no miss, with row diagnostics on failure), then the
        // full-strength postcondition: after a clean present the reconstruction equals the frame
        // byte-for-byte on EVERY row — what the acceptance calls "texture matches the framebuffer".
        present.present_now(fb, None);
        for y in 0..present.rows {
            let r = y * present.width..(y + 1) * present.width;
            assert!(
                present.presented[r.clone()] == fb[r],
                "presented != fb at row {y} after present (frame {frame_no} [{label}])"
            );
        }
        *frame_no += 1;
    }

    /// #626 acceptance: drive the real `App` + renderer through the guided tour's exact command
    /// sequences — the ambient ride, a demo-style app rebuild + mid-climb `GpxPlayer::seek` per
    /// `enter`, the climb demo's Back-cycle, the reroute-to-POI demo including the frame-stepped
    /// planner, and the ambient reset's backward seek — dwelling ≥300 presents on each tour screen
    /// (Map, Statistics, Climb, Ride menu, PoiList, PoiDetail, RouteOverview). Every frame presents
    /// under the oracle (debug asserts on) *and* the full byte-equality postcondition in
    /// [`tour_frame`], so any diff miss — the pre-fix panic — fails here with row diagnostics.
    #[test]
    #[cfg(feature = "external-fixtures")]
    fn tour_screens_dwell_with_no_present_miss() {
        use std::path::Path;

        use obc_app::screen::Screen;
        use obc_app::settings::{ClimbMode, Settings};
        use obc_app::{App, AppState, CameraMode, Gesture};
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};
        use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

        const W: u32 = FRAME_W as u32;
        const H: u32 = FRAME_H as u32;
        let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm").expect("full fixture suite requires map");
        let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let reader = Reader::new(&src, &tables, &cache);

        // A folder-backed route store over a temp dir seeded with the demo route, so the planner's
        // `_nav.obcr` write + rescan runs the real path.
        let dir = std::env::temp_dir().join(format!("obc626-tour-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp routes dir");
        std::fs::write(
            dir.join("grimsel-climb.obcr"),
            include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr"),
        )
        .expect("seed demo route");
        let mut store = crate::routes::RouteStore::open(&dir);

        let track = Track::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx"
        )))
        .expect("gpx");
        let mut player = GpxPlayer::new(track);
        player.set_speed(3.0); // the page's ambient pace (obc-web-demo's `DEMO_SPEED`)
        let mut baro = BaroSensor::new();
        let mut host = obc_host_core::HostLoop::new();
        let mut session = obc_host_core::ActiveRouteSession::new();
        let mut fb = vec![0u8; (W * H) as usize];
        let mut present = Present::new(W, H);
        let mut frame_no = 0usize;
        // One host-owned render scratch for the whole tour (#1146), lent to every frame.
        let mut scratch = Box::new(obc_render::RenderScratch::new());

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
                app.activate_route(0);
                app.recorder.request(obc_app::RecorderIntent::Start);
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
                        &mut scratch,
                        &mut fb,
                        &mut present,
                        &mut player,
                        &mut baro,
                        &mut store,
                        &mut host,
                        &mut session,
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
                        &mut scratch,
                        &mut fb,
                        &mut present,
                        &mut player,
                        &mut baro,
                        &mut store,
                        &mut host,
                        &mut session,
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
                &mut scratch,
                &mut fb,
                &mut present,
                &mut player,
                &mut baro,
                &mut store,
                &mut host,
                &mut session,
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
        until_then_dwell!(&mut app, "reroute: RideMenu", Screen::RideMenu(_), 300);
        app.apply_gesture(Gesture::Step(2));
        app.apply_gesture(Gesture::Press);
        until_then_dwell!(&mut app, "reroute: PoiMenu", Screen::PoiMenu(_), 45);
        app.apply_gesture(Gesture::Step(2));
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
                &mut scratch,
                &mut fb,
                &mut present,
                &mut player,
                &mut baro,
                &mut store,
                &mut host,
                &mut session,
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
    #[cfg(feature = "external-fixtures")]
    fn demo_reset_rebuild_and_seek_present_clean() {
        use std::path::Path;

        use obc_app::settings::{ClimbMode, Settings};
        use obc_app::{App, AppState, CameraMode};
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};
        use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};

        const W: u32 = FRAME_W as u32;
        const H: u32 = FRAME_H as u32;
        let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm").expect("full fixture suite requires map");
        let tables = MapTables::parse(&SliceSource(&bytes)).expect("valid demo map");
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let reader = Reader::new(&src, &tables, &cache);

        let dir = std::env::temp_dir().join(format!("obc626-reset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp routes dir");
        std::fs::write(
            dir.join("grimsel-climb.obcr"),
            include_bytes!("../../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr"),
        )
        .expect("seed demo route");
        let mut store = crate::routes::RouteStore::open(&dir);

        let track = Track::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/sources/sim-grimsel/tracks/grimsel-climb.gpx"
        )))
        .expect("gpx");
        let mut player = GpxPlayer::new(track);
        player.set_speed(3.0);
        let mut baro = BaroSensor::new();
        let mut host = obc_host_core::HostLoop::new();
        let mut session = obc_host_core::ActiveRouteSession::new();
        let mut fb = vec![0u8; (W * H) as usize];
        let mut present = Present::new(W, H);
        let mut frame_no = 0usize;
        // One host-owned render scratch for the whole tour (#1146), lent to every frame.
        let mut scratch = Box::new(obc_render::RenderScratch::new());

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
                app.activate_route(0);
                app.recorder.request(obc_app::RecorderIntent::Start);
            }
            app
        };
        let mut run = |app: &mut App,
                       fb: &mut Vec<u8>,
                       present: &mut Present,
                       player: &mut GpxPlayer,
                       baro: &mut BaroSensor,
                       store: &mut crate::routes::RouteStore,
                       host: &mut obc_host_core::HostLoop,
                       session: &mut obc_host_core::ActiveRouteSession,
                       tour: bool,
                       n: usize,
                       label: &str| {
            for _ in 0..n {
                tour_frame(
                    app,
                    &mut scratch,
                    fb,
                    present,
                    player,
                    baro,
                    store,
                    host,
                    session,
                    &reader,
                    tour,
                    &mut frame_no,
                    label,
                );
            }
        };

        // A short ambient ride, then the `enter` reset: rebuild + seek forward to mid-climb.
        let mut app = build_app(Settings::default(), &store);
        player.seek(0.0);
        player.play();
        run(
            &mut app,
            &mut fb,
            &mut present,
            &mut player,
            &mut baro,
            &mut store,
            &mut host,
            &mut session,
            false,
            60,
            "ambient",
        );
        app = build_app(Settings { climb_mode: ClimbMode::Manual, ..Settings::default() }, &store);
        player.seek(1500.0);
        player.play();
        run(
            &mut app,
            &mut fb,
            &mut present,
            &mut player,
            &mut baro,
            &mut store,
            &mut host,
            &mut session,
            true,
            300,
            "after enter",
        );

        // The `ambient` reset: rebuild + seek backward to the start.
        app = build_app(Settings::default(), &store);
        player.seek(0.0);
        player.play();
        run(
            &mut app,
            &mut fb,
            &mut present,
            &mut player,
            &mut baro,
            &mut store,
            &mut host,
            &mut session,
            false,
            300,
            "after ambient",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn texture_tracks_a_sequence_of_partial_changes() {
        let mut fb = vec![0u8; 3 * 6];
        let mut p = Present::new(3, 6);
        present_fill(&mut fb, &mut p, 0x04, None);
        // A few disjoint edits across frames; after each, only the one changed row is pushed and the
        // texture reconstructs it (partial pushes reconstruct the whole — the load-bearing property).
        for (row, val) in [(0usize, 0x08u8), (5, 0x0C), (3, 0x10)] {
            fb[row * 3..(row + 1) * 3].fill(val);
            p.present_now(&fb, None);
            assert_eq!(p.stats.pushed_rows, 1, "only row {row} pushed");
            let (r, g, b) = expect_px(val);
            let t = row * 3 * 3;
            assert_eq!((p.texture()[t], p.texture()[t + 1], p.texture()[t + 2]), (r, g, b), "row {row} in texture");
        }
    }
}
