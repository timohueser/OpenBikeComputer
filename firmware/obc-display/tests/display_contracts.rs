//! Contract tests: the conformance suite run against the FLPR-semantics host double,
//! `Device64Frame` plus [`SpanPresenter`], the board presenter's host-testable half. It is built
//! from the same shared parts the board backend runs (the row-hash self-diff, the [`RowDamage`] and
//! [`RowWindow`] vocabulary, and the mutate-and-restore overlay composite), with only the transport
//! faked and the exact-diff oracle asserted inside every present. The clean-frame postcondition
//! therefore checks the real save, composite, push and restore path.
//!
//! The simulator presenter, the other shipping backend, runs the same suite in
//! `sim/desktop/tests/present.rs`.

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use obc_display::display_contracts::conformance::{self, GlassProbe};
use obc_display::display_contracts::{Device64Frame, NativeFrame, OverlayPresenter, PresentStats, Presenter};
use obc_display::ls021::{
    composite_into_resident, spans_missed_changes, OverlayScratch, RowDamage, RowDiff, RowWindow,
};
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
    /// Inject one transport fault into the next present.
    fail_next: bool,
}

impl<const W: usize, const H: usize> SpanPresenter<W, H> {
    fn new() -> Self {
        Self { diff: RowDiff::new(), glass: vec![0; W * H], fail_next: false }
    }
}

impl<'b, const W: usize, const H: usize> Presenter<Device64Frame<'b, W, H>> for SpanPresenter<W, H> {
    type Damage = RowDamage;
    type Error = Fault;

    fn damage_full() -> RowDamage {
        RowDamage::Full
    }

    fn damage_unknown() -> RowDamage {
        RowDamage::SelfDiff { exclude: None }
    }

    async fn present(&mut self, frame: &Device64Frame<'b, W, H>, damage: RowDamage) -> Result<PresentStats, Fault> {
        if std::mem::take(&mut self.fail_next) {
            return Err(Fault);
        }
        let exclude = match damage {
            RowDamage::Full => {
                self.diff.reset(); // full = re-seed the store + push everything, like the board's recovery path
                None
            }
            RowDamage::SelfDiff { exclude } => exclude,
        };
        let mut scratch = [(0u16, 0u16); 16];
        let spans = self.diff.diff_clipped(frame.bytes(), W, exclude, &mut scratch);
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
        Ok(PresentStats { pushed_units: pushed, total_units: H as u32, regions })
    }
}

impl<'b, const W: usize, const H: usize> OverlayPresenter<Device64Frame<'b, W, H>> for SpanPresenter<W, H> {
    type Region = RowWindow;
    type OverlayTarget<'t> = Band<'t>;

    fn region(rect: Rectangle) -> RowWindow {
        RowWindow::from_rect(rect, W as u32, H as u32)
    }

    fn damage_around(r: RowWindow) -> RowDamage {
        RowDamage::SelfDiff { exclude: Some(r.exclude_span()) }
    }

    async fn present_overlay(
        &mut self,
        frame: &mut Device64Frame<'b, W, H>,
        r: RowWindow,
        draw: impl for<'t> FnOnce(&mut Band<'t>),
    ) -> Result<PresentStats, Fault> {
        if std::mem::take(&mut self.fail_next) {
            return Err(Fault);
        }
        // The real mutate-and-restore engine the board backend runs: composite through the shared
        // window helper, save the clean window bytes, write the re-quantized composite into the
        // resident frame, push the full-width rows to the host glass, and restore the clean
        // bytes. The conformance snapshot check outside proves the restore.
        let (w, rows) = (r.w as usize, r.rows as usize);
        let mut scratch = vec![0u16; w * rows];
        let mut save = vec![0u8; w * rows];
        let mut draw = Some(draw);
        let glass = &mut self.glass;
        composite_into_resident(
            frame.backing_mut(),
            Size::new(W as u32, H as u32),
            r,
            OverlayScratch { win: &mut scratch, save: &mut save },
            &mut |band| {
                if let Some(d) = draw.take() {
                    d(band)
                }
            },
            |fb| {
                let span = r.y0 as usize * W..(r.y0 as usize + rows) * W;
                glass[span.clone()].copy_from_slice(&fb[span]);
                Ok::<(), Fault>(())
            },
        )?;
        Ok(PresentStats { pushed_units: r.rows as u32, total_units: H as u32, regions: 1 })
    }
}

impl<'b, const W: usize, const H: usize> GlassProbe<Device64Frame<'b, W, H>> for SpanPresenter<W, H> {
    fn glass(&self, x: u32, y: u32) -> Rgb565 {
        rgb(device64_to_rgb565(self.glass[y as usize * W + x as usize]))
    }
}

// The reference pairing's probe geometry: a right-edge 4×4 overlay window on a 16×16 frame. The
// LS021 widening is full-width rows, so "outside" is a row clear of [4, 8).
const SPAN_RECT: Rectangle = Rectangle { top_left: Point::new(12, 4), size: Size::new(4, 4) };

#[test]
fn span_pairing_full_present() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_full_present(&mut frame, &mut p, rgb(RED), rgb(BLUE)));
}

#[test]
fn span_pairing_damage_translation() {
    let mut buf = [0u8; 16 * 16];
    let mut frame = Device64Frame::<16, 16>::new(&mut buf);
    let mut p = SpanPresenter::<16, 16>::new();
    block_on(conformance::check_damage_translation(&mut frame, &mut p, rgb(RED), rgb(BLUE), true));
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
        SPAN_RECT,
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
        SPAN_RECT,
        (13, 5),
        (14, 6),
        (0, 0),
        |f| f.bytes().to_vec(),
        true,
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
        SPAN_RECT,
        (12, 5),
        (15, 6),
        |f| f.bytes().to_vec(),
        true,
    ));
}
