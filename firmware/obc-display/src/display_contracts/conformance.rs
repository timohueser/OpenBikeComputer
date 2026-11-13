//! The presenter conformance suite: the contracts' invariants, written once so the host double and
//! the simulator presenter run the same checks.
//!
//! The checks observe a presenter only through the contracts plus one test-side hook,
//! [`GlassProbe`], which answers "what colour is on glass at (x, y)?". The overlay window and the
//! probe points come from the concrete test.
//!
//! Each check takes a newly made presenter: its seeding present is the presenter's first
//! `present(frame, None)`, which pushes every row. Colour arguments must stay pairwise
//! distinguishable after device-64 quantization.
//!
//! `no_std`, allocation-free and executor-agnostic. Generic functions that are never instantiated
//! produce no code, so nothing here reaches a shipping image.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::frame::Device64Frame;
use super::presenter::{OverlayPresenter, Presenter};
use crate::ls021::RowWindow;

/// Test-side readback of what a presenter put on glass.
pub trait GlassProbe {
    /// The colour currently displayed at frame position `(x, y)`.
    fn glass(&self, x: u32, y: u32) -> Rgb565;
}

/// One pixel probe point, frame-absolute.
pub type Probe = (u32, u32);

fn pixel(at: Probe) -> Rectangle {
    Rectangle::new(Point::new(at.0 as i32, at.1 as i32), Size::new(1, 1))
}

/// The first present pushes every row, and a change of every row pushes every row.
pub async fn check_full_present<const W: usize, const H: usize, P>(
    frame: &mut Device64Frame<'_, W, H>,
    p: &mut P,
    a: Rgb565,
    b: Rgb565,
) where
    P: Presenter<W, H> + GlassProbe,
{
    let (right, bottom) = (W as u32 - 1, H as u32 - 1);
    frame.draw_target().clear(a).unwrap();
    let s = p.present(frame, None).await.expect("a present reports transport success");
    assert_eq!(s.total_units, H as u32, "a frame has one unit per row");
    assert_eq!(s.pushed_units, H as u32, "the first present pushes every row");
    let ga = p.glass(0, 0);
    assert_eq!(p.glass(right, bottom), ga, "the whole frame reached glass");

    frame.draw_target().clear(b).unwrap();
    let s = p.present(frame, None).await.unwrap();
    assert_eq!(s.pushed_units, H as u32, "a change of every row pushes every row");
    let gb = p.glass(0, 0);
    assert_ne!(gb, ga, "the probe colours must stay distinct after native quantization");
    assert_eq!(p.glass(right, bottom), gb, "the repaint reached glass everywhere");
}

/// The self-diff: a localized change lands on glass and pushes only its row, and an unchanged
/// frame pushes nothing.
pub async fn check_self_diff<const W: usize, const H: usize, P>(
    frame: &mut Device64Frame<'_, W, H>,
    p: &mut P,
    base: Rgb565,
    mark: Rgb565,
) where
    P: Presenter<W, H> + GlassProbe,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, None).await.unwrap();
    let g_base = p.glass(0, 0);

    // One changed pixel in the frame's centre.
    let centre = (W as u32 / 2, H as u32 / 2);
    frame.draw_target().fill_solid(&pixel(centre), mark).unwrap();
    let s = p.present(frame, None).await.unwrap();
    assert_ne!(p.glass(centre.0, centre.1), g_base, "the changed pixel reached glass");
    assert_eq!(p.glass(0, 0), g_base, "unchanged pixels keep their glass");
    assert_eq!(s.pushed_units, 1, "a one-pixel change pushes only its row");
    let s2 = p.present(frame, None).await.unwrap();
    assert_eq!(s2.pushed_units, 0, "an unchanged frame pushes nothing (a spurious redraw is free)");
}

/// Overlay backdrop composition: the composite reads the clean frame as its backdrop, so pixels
/// `draw` does not paint show the frame, and the frame's backing is byte-identical when the call
/// returns. A mutate-and-restore composite passes only if its restore is exact.
///
/// `mark_at` is a pixel inside `window` that `draw` paints, and `backdrop_at` one it leaves alone.
/// `snapshot` captures the frame's backing for the equality comparison; the harness is
/// allocation-free, so the caller owns the representation.
#[allow(clippy::too_many_arguments)] // a test harness taking explicit probe points, not an API to hold small
pub async fn check_overlay_backdrop<const W: usize, const H: usize, P, S>(
    frame: &mut Device64Frame<'_, W, H>,
    p: &mut P,
    base: Rgb565,
    mark: Rgb565,
    window: RowWindow,
    mark_at: Probe,
    backdrop_at: Probe,
    snapshot: impl Fn(&Device64Frame<'_, W, H>) -> S,
) where
    P: OverlayPresenter<W, H> + GlassProbe,
    S: PartialEq,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, None).await.unwrap();
    let g_base = p.glass(mark_at.0, mark_at.1);

    let clean = snapshot(frame);
    p.present_overlay(frame, window, |t| {
        let _ = t.fill_solid(&pixel(mark_at), mark);
    })
    .await
    .expect("overlay present reports transport success");
    assert!(snapshot(frame) == clean, "present_overlay must leave the frame's backing byte-identical");
    assert_ne!(p.glass(mark_at.0, mark_at.1), g_base, "the overlay mark reached glass");
    assert_eq!(p.glass(backdrop_at.0, backdrop_at.1), g_base, "un-drawn overlay pixels show the clean-frame backdrop");
}

/// Live-overlay exclusion: a present that excludes the window's rows updates the rest of the glass
/// without flashing the overlay off, the overlay's next re-present composites over the current
/// clean frame, and the excluded rows' store kept tracking the clean frame, so after the trailing
/// clear a present pushes nothing.
///
/// `outside_at` must lie outside the window's rows. `snapshot` is as in [`check_overlay_backdrop`].
#[allow(clippy::too_many_arguments)] // a test harness taking explicit probe points, not an API to hold small
pub async fn check_overlay_exclusion<const W: usize, const H: usize, P, S>(
    frame: &mut Device64Frame<'_, W, H>,
    p: &mut P,
    base: Rgb565,
    mark: Rgb565,
    fresh: Rgb565,
    window: RowWindow,
    mark_at: Probe,
    backdrop_at: Probe,
    outside_at: Probe,
    snapshot: impl Fn(&Device64Frame<'_, W, H>) -> S,
) where
    P: OverlayPresenter<W, H> + GlassProbe,
    S: PartialEq,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, None).await.unwrap();
    let g_base = p.glass(outside_at.0, outside_at.1);

    // The bulge pops over the base frame.
    let clean = snapshot(frame);
    p.present_overlay(frame, window, |t| {
        let _ = t.fill_solid(&pixel(mark_at), mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "present_overlay must leave the frame's backing byte-identical");
    let g_bulge = p.glass(mark_at.0, mark_at.1);
    assert_ne!(g_bulge, g_base, "the bulge is on glass");

    // The map plane re-renders the whole frame and presents around the live overlay.
    frame.draw_target().clear(fresh).unwrap();
    p.present(frame, Some((window.y0, window.rows))).await.unwrap();
    assert_eq!(p.glass(mark_at.0, mark_at.1), g_bulge, "a present around a live overlay never flashes it off");
    let g_fresh = p.glass(outside_at.0, outside_at.1);
    assert_ne!(g_fresh, g_base, "the rest of the frame was updated around the overlay");

    // The overlay plane's next tick: same bulge, composited over the current clean frame.
    let clean = snapshot(frame);
    p.present_overlay(frame, window, |t| {
        let _ = t.fill_solid(&pixel(mark_at), mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "the live re-present must leave the frame's backing byte-identical");
    assert_eq!(p.glass(mark_at.0, mark_at.1), g_bulge, "the re-presented bulge is intact");
    assert_eq!(p.glass(backdrop_at.0, backdrop_at.1), g_fresh, "the overlay backdrop is the current clean frame");

    // The trailing clear: a drawer that paints nothing restores the clean frame, and the excluded
    // rows' store tracked it through the around-present, so nothing stale is left to push.
    p.present_overlay(frame, window, |_| {}).await.unwrap();
    assert_eq!(p.glass(mark_at.0, mark_at.1), g_fresh, "the trailing clear restores the current clean frame");
    let s = p.present(frame, None).await.unwrap();
    assert_eq!(s.pushed_units, 0, "excluded rows tracked the clean frame, so nothing stale is re-pushed");
}

/// Pop, retract and the trailing clear: each overlay composite starts from the clean frame, so a
/// shrinking bulge needs no undo pass, a drawer that paints nothing restores the clean frame with
/// no map re-render, every overlay call leaves the frame's backing byte-identical, and the next
/// present pushes nothing.
#[allow(clippy::too_many_arguments)] // a test harness taking explicit probe points, not an API to hold small
pub async fn check_overlay_pop_retract_clear<const W: usize, const H: usize, P, S>(
    frame: &mut Device64Frame<'_, W, H>,
    p: &mut P,
    base: Rgb565,
    mark: Rgb565,
    window: RowWindow,
    tip_at: Probe,
    edge_at: Probe,
    snapshot: impl Fn(&Device64Frame<'_, W, H>) -> S,
) where
    P: OverlayPresenter<W, H> + GlassProbe,
    S: PartialEq,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, None).await.unwrap();
    let g_base = p.glass(tip_at.0, tip_at.1);
    let clean = snapshot(frame);

    // Pop: the bulge covers both probe pixels.
    p.present_overlay(frame, window, |t| {
        let _ = t.fill_solid(&pixel(tip_at), mark);
        let _ = t.fill_solid(&pixel(edge_at), mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "pop: present_overlay must leave the frame's backing byte-identical");
    assert_ne!(p.glass(tip_at.0, tip_at.1), g_base, "pop: the bulge tip is on glass");
    assert_ne!(p.glass(edge_at.0, edge_at.1), g_base, "pop: the bulge edge is on glass");

    // Retract: the next tick draws a smaller bulge, so the tip pixel is withdrawn and must revert
    // to the clean backdrop without an explicit erase, because composites are not cumulative.
    p.present_overlay(frame, window, |t| {
        let _ = t.fill_solid(&pixel(edge_at), mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "retract: present_overlay must leave the frame's backing byte-identical");
    assert_eq!(p.glass(tip_at.0, tip_at.1), g_base, "retract: withdrawn pixels revert to the clean backdrop");
    assert_ne!(p.glass(edge_at.0, edge_at.1), g_base, "retract: the remaining bulge stays");

    // Quiet: the trailing clear restores the clean frame under the whole window.
    p.present_overlay(frame, window, |_| {}).await.unwrap();
    assert!(snapshot(frame) == clean, "trailing clear: present_overlay must leave the frame's backing byte-identical");
    assert_eq!(p.glass(edge_at.0, edge_at.1), g_base, "trailing clear: the clean frame is restored");
    assert_eq!(p.glass(tip_at.0, tip_at.1), g_base);

    let s = p.present(frame, None).await.unwrap();
    assert_eq!(s.pushed_units, 0, "after the trailing clear the row store already agrees with the clean frame");
}
