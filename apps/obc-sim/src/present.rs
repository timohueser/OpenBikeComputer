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
    pub(crate) presented: Vec<u8>,
    /// The RGB888 texture egui uploads — the `presented` device-64 frame expanded per changed row.
    tex: Vec<u8>,
    /// Per-row hash of the last-presented frame — the device's `[u32; HEIGHT]` store, runtime-sized.
    pub(crate) hashes: Vec<u32>,
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
    pub(crate) miss_reported: Vec<bool>,
    /// How many `miss_reported` flags are set — lets the happy path (no miss ever reported, the
    /// universal case) skip the clear sweep entirely.
    pub(crate) misses_flagged: usize,
    /// Frame width in pixels — the device-64 row stride (one byte per pixel).
    pub(crate) width: usize,
    /// Frame height in rows.
    pub(crate) rows: usize,
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
