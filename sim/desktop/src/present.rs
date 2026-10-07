//! The simulator's display presenter: the host stand-in for the device's panel, and the second
//! live backend of the generic display contracts.
//!
//! Like the device, the simulator keeps one resident device-64 frame, one byte per pixel, that the
//! shared renderer draws the whole frame into. [`Present::present_now`] then pushes it through the
//! same self-diffing [`diff_rows`](obc_display::ls021::diff_rows) core the device runs, re-pushing
//! only the rows whose per-row hash changed and honouring a live overlay's exclude span, and
//! [`Present::present_overlay_now`] composites through the same shared helper. The only
//! host-specific step is expanding the pushed device-64 rows to the RGB888 texture egui uploads.
//!
//! On top of that it keeps a full copy of the last-presented frame, computes which rows actually
//! changed, and asserts the hash-diff's spans covered every one. That oracle catches a systematic
//! diff bug in CI. Because the uploaded texture is reconstructed from the partial pushes, a diff
//! bug also surfaces as a stale row on glass and not only as a failed assert.
//!
//! The contracts type frame geometry at compile time, but the sim's device resolution is a runtime
//! CLI knob, so the GUI loop drives the presenter through the inherent
//! [`present_now`](Present::present_now) and [`present_overlay_now`](Present::present_overlay_now)
//! engine, and the contract impls are one-line delegations to those bodies. The conformance suite
//! below exercises the contract surface at several geometries, so the engine the GUI runs is the
//! one the contracts certify.
//!
//! The contract methods are `async` and complete synchronously on the host, so the tests drive them
//! with `pollster::block_on`.

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use obc_display::display_contracts::{Device64Frame, OverlayPresenter, Presenter};
use obc_display::ls021::{clip_span, diff_rows, row_hash, spans_missed_changes, RowDamage, RowWindow};
use obc_display::{composite_overlay_window, device64_to_rgb565, Band};
use obc_reader::rgb565_to_rgb888;

/// Last present's push metric, surfaced in the render-stats panel.
#[derive(Clone, Copy, Default)]
pub struct PresentStats {
    /// Rows pushed this present: the sum of the changed spans. `0` when the frame was
    /// byte-identical to the last, which is what makes a spurious coarse dirty free.
    pub pushed_rows: usize,
    /// Number of contiguous changed-row spans.
    pub spans: usize,
    /// The frame's total row count, so `pushed_rows` over it is the push fraction.
    pub total_rows: usize,
}

/// The simulator's presenter: the per-row hash store, the partial-push reconstruction, which is
/// both the oracle's previous frame and the source of the uploaded texture, and the RGB888 texture.
/// The resident device-64 frame lives with the host and is passed into each present. The buffers
/// are runtime-sized, because the sim window size is a CLI knob, but the [`diff_rows`] core and the
/// stride are the device's.
pub struct Present {
    /// The device-64 bytes on glass: seeded all black and updated only on changed spans, so it is
    /// reconstructed from partial pushes. It is the oracle's previous frame and the source the
    /// texture expands from.
    pub(crate) presented: Vec<u8>,
    /// The RGB888 texture egui uploads: the `presented` frame expanded per changed row.
    tex: Vec<u8>,
    /// Per-row hash of the last-presented frame, runtime-sized.
    pub(crate) hashes: Vec<u32>,
    /// `false` until the first present: the store holds no prior frame, so the first present
    /// pushes and seeds the whole frame.
    primed: bool,
    /// Coverage scratch the oracle rewrites each present, so nothing allocates per frame.
    covered: Vec<bool>,
    /// Per-row dedupe flags for the oracle diagnostics. A missed row stays byte-different every
    /// frame until it next changes, and presents run at frame rate, so an undeduped log would flood
    /// the console. Set when a row's miss is first reported, cleared when the row stops missing.
    pub(crate) miss_reported: Vec<bool>,
    /// How many `miss_reported` flags are set, so the path with no miss skips the clear sweep.
    pub(crate) misses_flagged: usize,
    /// Frame width in pixels, which is the device-64 row stride.
    pub(crate) width: usize,
    /// Frame height in rows.
    pub(crate) rows: usize,
    /// Last present's metric, read by the control panel.
    pub stats: PresentStats,
}

impl Present {
    /// A presenter for a `width` by `height` frame, primed empty, so the first present pushes the
    /// whole frame. The resident plane it presents is device-64, like the device's.
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

    /// The RGB888 texture reconstructed from this session's partial pushes, which is what the GUI
    /// uploads. `width * height * 3` bytes.
    pub fn texture(&self) -> &[u8] {
        &self.tex
    }

    /// Expand one device-64 row of `presented` into the RGB888 texture, which is the host-specific
    /// step. It uses the panel's own ramp, so the texture is what the glass would show.
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

    /// Self-diff the resident frame `fb` and push only the changed rows, as the device does: diff
    /// every row against the hash store, updating it for all rows so a miss self-heals, clip each
    /// changed span around a live overlay's `exclude` rows, reconstruct the pushed rows into
    /// `presented` and the texture, then assert the oracle is satisfied. The push runs first, so a
    /// failed oracle cannot desync later frames.
    pub fn present_now(&mut self, fb: &[u8], exclude: Option<(u16, u16)>) {
        let (stride, rows) = (self.width, self.rows);
        debug_assert!(fb.len() >= stride * rows, "frame shorter than the presenter's geometry");

        // Diff against the hash store for contiguous changed-row spans. The store updates to this
        // frame for every row, pushed or not, which is what makes a miss self-heal.
        let mut raw: Vec<(u16, u16)> = Vec::new();
        diff_rows(fb, stride, &mut self.hashes, !self.primed, row_hash, |y0, n| raw.push((y0, n)));
        self.primed = true;

        // Clip each changed span around a live overlay's rows, as the device's present does. The
        // excluded rows belong to the overlay plane this frame, so they are not pushed here.
        // `clip_span` takes a half-open interval, so the overlay span is converted the same way the
        // device's `RowDiff::diff_clipped` converts it.
        let ex = exclude.map(|(y0, rows)| (y0, y0 + rows));
        let mut spans: Vec<(u16, u16)> = Vec::new();
        for &(y0, n) in &raw {
            clip_span(y0, n, ex, &mut |s, c| spans.push((s, c)));
        }

        // Push only the changed spans into `presented` and the texture. The displayed texture is
        // this partial-push buffer. The push runs before the oracle check, so a failed check aborts
        // after the frame landed and one miss cannot desync every later present.
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

        // The oracle computes which rows actually changed and asserts the pushed spans, plus the
        // overlay's excluded rows, covered every one. Rows inside the spans were just pushed, so
        // checking after the push is equivalent: a miss is an uncovered row whose bytes still
        // differ. Such a row self-heals, because its store hash already tracks `fb`.
        let mut oracle_spans = spans.clone();
        if let Some(ex) = exclude {
            oracle_spans.push(ex);
        }
        let missed = spans_missed_changes(&self.presented, fb, stride, rows, &oracle_spans, &mut self.covered);
        if missed != 0 {
            // Diagnostics before the assert, kept in release too, where the assert compiles out
            // and the miss would be a silent stale row: which row, its stored hash, and the
            // differing column range. Deduped per row, because a missed row on a parked screen
            // stays byte-different until it next changes.
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
                        // The row healed or was covered this frame, so re-arm its report.
                        self.miss_reported[y] = false;
                        self.misses_flagged -= 1;
                    }
                    _ => {}
                }
            }
        } else if self.misses_flagged != 0 {
            // Every previously reported row healed this present, so re-arm all reports in one
            // sweep. Skipped entirely when nothing was ever reported.
            self.miss_reported.fill(false);
            self.misses_flagged = 0;
        }
        debug_assert_eq!(missed, 0, "self-diff missed {missed} changed row(s) — a systematic hash-diff bug");
    }

    /// Re-present `region` with `draw_overlay` composited over the clean framebuffer backdrop,
    /// through the shared [`composite_overlay_window`], as the device does. The clean device-64
    /// `fb` is never written, because the overlay is transient chrome.
    ///
    /// Like the row-addressed panel, the push re-latches the full-width rows from the clean frame:
    /// `presented` takes the clean `fb` bytes for those rows, the texture re-expands them, and only
    /// the overlay's own columns then carry the composite. That is what keeps the excluded rows'
    /// glass tracking the clean frame through an around-present and a trailing clear.
    pub fn present_overlay_now(&mut self, fb: &[u8], region: RowWindow, draw_overlay: &mut dyn FnMut(&mut Band)) {
        let (w, rows) = (region.w as usize, region.rows as usize);
        let frame = Size::new(self.width as u32, self.rows as u32);
        let window = Rectangle::new(
            Point::new(region.x0 as i32, region.y0 as i32),
            Size::new(region.w as u32, region.rows as u32),
        );
        // Composite the overlay over the clean device-64 backdrop into an RGB565 scratch, which is
        // byte-for-byte the device's overlay push.
        let mut scratch = vec![0u16; w * rows];
        composite_overlay_window(fb, frame, window, &mut scratch, draw_overlay);
        // Re-latch the full-width rows from the clean frame, because the device pushes whole
        // composited rows, then blit the composited window columns over them in the texture. `fb`
        // itself is never written.
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

// The display contracts: the sim presenter paired with a `Device64Frame` of any geometry. These
// are one-line delegations to the runtime engine above. The frame type's geometry must match the
// presenter's constructed geometry, which is checked on every present in debug builds.

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
                // Full damage re-seeds the store and pushes every row.
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
