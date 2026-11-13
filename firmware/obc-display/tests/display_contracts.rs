//! Contract tests: the conformance suite run against the FLPR-semantics host double,
//! `Device64Frame` plus [`SpanPresenter`], the board presenter's host-testable half. It is built
//! from the same shared parts the board backend runs (the row-hash self-diff, the [`RowWindow`]
//! overlay window, and the mutate-and-restore overlay composite), with only the transport faked
//! and the exact-diff oracle asserted inside every present. The clean-frame postcondition
//! therefore checks the real save, composite, push and restore path.
//!
//! The simulator presenter, the other shipping backend, runs the same suite in
//! `sim/desktop/tests/present.rs`. It cannot fail, so the recovery test runs on the double only.

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use obc_display::display_contracts::conformance::{self, GlassProbe};
use obc_display::display_contracts::{Device64Frame, OverlayPresenter, PresentStats, Presenter};
use obc_display::ls021::{composite_into_resident, spans_missed_changes, OverlayScratch, RowDiff, RowWindow};
use obc_display::{device64_to_rgb565, Band};
use pollster::block_on;

fn rgb(raw: u16) -> Rgb565 {
    Rgb565::from(RawU16::new(raw))
}
const RED: u16 = 0xF800;
const GREEN: u16 = 0x07E0;
const BLUE: u16 = 0x001F;

/// A simulated transport fault, the outcome a caller retries on.
#[derive(Debug)]
struct Fault;

/// The board presenter minus the FLPR transport: the per-row-hash self-diff, exclusion clipped by
/// the shared [`RowDiff::diff_clipped`], the shared mutate-and-restore composite, and a host
/// "glass" reconstruction updated only from pushed spans, checked by the exact-diff oracle on
/// every present.
struct SpanPresenter<const W: usize, const H: usize> {
    diff: RowDiff<H>,
    /// Device-64 bytes on glass, reconstructed from partial pushes.
    glass: Vec<u8>,
    /// Inject one transport fault into the next present or overlay push.
    fail_next: bool,
}

impl<const W: usize, const H: usize> SpanPresenter<W, H> {
    fn new() -> Self {
        Self { diff: RowDiff::new(), glass: vec![0; W * H], fail_next: false }
    }
}

impl<const W: usize, const H: usize> Presenter<W, H> for SpanPresenter<W, H> {
    type Error = Fault;

    async fn present(
        &mut self,
        frame: &Device64Frame<'_, W, H>,
        exclude: Option<(u16, u16)>,
    ) -> Result<PresentStats, Fault> {
        let mut scratch = [(0u16, 0u16); 16];
        let spans = self.diff.diff_clipped(frame.bytes(), W, exclude, &mut scratch);
        // The board fails here too: after the diff advanced the store, before the rows land.
        if std::mem::take(&mut self.fail_next) {
            return Err(Fault);
        }
        let regions = spans.len() as u32;
        let mut pushed = 0u32;
        for &(y0, n) in spans {
            let r = y0 as usize * W..(y0 + n) as usize * W;
            self.glass[r.clone()].copy_from_slice(&frame.bytes()[r]);
            pushed += n as u32;
        }
        // The exact-diff oracle: pushed spans + the overlay-owned excluded rows cover every change.
        let mut oracle = spans.to_vec();
        if let Some(ex) = exclude {
            oracle.push(ex);
        }
        let mut covered = vec![false; H];
        assert_eq!(spans_missed_changes(&self.glass, frame.bytes(), W, H, &oracle, &mut covered), 0);
        self.diff.confirm();
        Ok(PresentStats { pushed_units: pushed, total_units: H as u32, regions })
    }
}

impl<const W: usize, const H: usize> OverlayPresenter<W, H> for SpanPresenter<W, H> {
    async fn present_overlay(
        &mut self,
        frame: &mut Device64Frame<'_, W, H>,
        window: RowWindow,
        draw: impl FnOnce(&mut Band<'_>),
    ) -> Result<PresentStats, Fault> {
        // The real mutate-and-restore engine the board backend runs: composite through the shared
        // window helper, save the clean window bytes, write the re-quantized composite into the
        // resident frame, push the full-width rows to the host glass, and restore the clean
        // bytes. The conformance snapshot check outside proves the restore.
        let (w, rows) = (window.w as usize, window.rows as usize);
        let mut scratch = vec![0u16; w * rows];
        let mut save = vec![0u8; w * rows];
        let mut draw = Some(draw);
        let fail = std::mem::take(&mut self.fail_next);
        let glass = &mut self.glass;
        let pushed = composite_into_resident(
            frame.bytes_mut(),
            Size::new(W as u32, H as u32),
            window,
            OverlayScratch { win: &mut scratch, save: &mut save },
            &mut |band| {
                if let Some(d) = draw.take() {
                    d(band)
                }
            },
            |fb| {
                if fail {
                    return Err(Fault);
                }
                let span = window.y0 as usize * W..(window.y0 as usize + rows) * W;
                glass[span.clone()].copy_from_slice(&fb[span]);
                Ok(())
            },
        );
        if pushed.is_err() {
            self.diff.reset();
        }
        pushed.map(|()| PresentStats { pushed_units: window.rows as u32, total_units: H as u32, regions: 1 })
    }
}

impl<const W: usize, const H: usize> GlassProbe for SpanPresenter<W, H> {
    fn glass(&self, x: u32, y: u32) -> Rgb565 {
        rgb(device64_to_rgb565(self.glass[y as usize * W + x as usize]))
    }
}

// The reference probe geometry: a right-edge 4×4 overlay window on a 16×16 frame. The LS021
// widening is full-width rows, so "outside" is a row clear of [4, 8).
const WINDOW: RowWindow = RowWindow { x0: 12, y0: 4, w: 4, rows: 4 };

#[test]
fn span_pairing_full_present() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_full_present(&mut frame, &mut p, rgb(RED), rgb(BLUE)));
}

#[test]
fn span_pairing_self_diff() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_self_diff(&mut frame, &mut p, rgb(RED), rgb(BLUE)));
}

#[test]
fn span_pairing_overlay_backdrop() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_overlay_backdrop(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        WINDOW,
        (13, 5),
        (14, 6),
        |f| f.bytes().to_vec(),
    ));
}

#[test]
fn span_pairing_overlay_exclusion() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_overlay_exclusion(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        rgb(GREEN),
        WINDOW,
        (13, 5),
        (14, 6),
        (0, 0),
        |f| f.bytes().to_vec(),
    ));
}

#[test]
fn span_pairing_overlay_pop_retract_clear() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_overlay_pop_retract_clear(
        &mut frame,
        &mut p,
        rgb(RED),
        rgb(BLUE),
        WINDOW,
        (12, 5),
        (15, 6),
        |f| f.bytes().to_vec(),
    ));
}

/// After a failed push, the next present around a live window repaints every row outside it and
/// leaves the window's rows to the overlay. A failed overlay push does not advance the row store,
/// so only the pushed row count shows that it reset the store.
#[test]
fn a_failed_push_makes_the_next_present_repair_the_panel() {
    for fail_overlay in [false, true] {
        let mut buf = [0x30u8; 16 * 16];
        let mut frame = Device64Frame::<16, 16>::new(&mut buf);
        let mut p = SpanPresenter::<16, 16>::new();
        block_on(p.present(&frame, None)).unwrap();
        let before = p.glass.clone();

        // One row outside the window and one inside it change.
        for y in [2, 5] {
            frame.bytes_mut()[y * 16..(y + 1) * 16].fill(0x0C);
        }
        let exclude = Some((WINDOW.y0, WINDOW.rows));
        p.fail_next = true;
        if fail_overlay {
            assert!(block_on(p.present_overlay(&mut frame, WINDOW, |_| {})).is_err());
        } else {
            assert!(block_on(p.present(&frame, exclude)).is_err());
        }

        let s = block_on(p.present(&frame, exclude)).unwrap();
        assert_eq!(s.pushed_units, 16 - 4, "every row outside the window is pushed (fail_overlay: {fail_overlay})");
        for y in 0..16 {
            let r = y * 16..(y + 1) * 16;
            let want = if (4..8).contains(&y) { &before[r.clone()] } else { &frame.bytes()[r.clone()] };
            assert_eq!(&p.glass[r], want, "row {y} (fail_overlay: {fail_overlay})");
        }
    }
}
