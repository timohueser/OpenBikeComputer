//! The **presenter conformance suite** — generic, backend-agnostic checks of the contracts'
//! mandatory invariants, written once so every pairing (the test-only proof backend now; the real
//! LS021/FLPR and simulator backends when #806 migrates them) runs the *same* semantics tests.
//!
//! The checks observe a backend only through the contracts plus one test-side hook,
//! [`GlassProbe`] — "what colour is on glass at (x, y)?" — implemented by each backend's test
//! double (the simulator already keeps exactly this reconstruction for its exact-diff oracle).
//! Everything else (regions, damage, colours, probe points) is passed in by the concrete test,
//! because widening rules and quantization are pairing-specific.
//!
//! Colour arguments must remain pairwise distinguishable **after the frame's native quantization**
//! (on the RGB222 pairing: colours that land on different device-64 bytes).
//!
//! `no_std`, allocation-free, executor-agnostic (`async fn`s a host test drives with any
//! `block_on`). Nothing here is compiled into a shipping image: generic functions that are never
//! instantiated produce no code.

use core::fmt::Debug;

use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

use super::frame::NativeFrame;
use super::presenter::{OverlayPresenter, Presenter};

/// Test-side readback of what a backend actually put on glass, in the frame's draw-colour space.
pub trait GlassProbe<F: NativeFrame> {
    /// The colour currently displayed at frame position `(x, y)`.
    fn glass(&self, x: u32, y: u32) -> F::Color;
}

/// One pixel probe point, frame-absolute.
pub type Probe = (u32, u32);

/// A full-damage present puts the whole frame on glass and reports a full push.
pub async fn check_full_present<F, P>(frame: &mut F, p: &mut P, a: F::Color, b: F::Color)
where
    F: NativeFrame,
    F::Color: Copy + PartialEq + Debug,
    P: Presenter<F> + GlassProbe<F>,
{
    let (right, bottom) = (F::WIDTH as u32 - 1, F::HEIGHT as u32 - 1);
    frame.draw_target().clear(a).unwrap();
    let s = p.present(frame, P::damage_full()).await.expect("a full present reports transport success");
    assert!(s.total_units > 0, "a frame has at least one damage unit");
    assert_eq!(s.pushed_units, s.total_units, "full damage pushes every unit");
    let ga = p.glass(0, 0);
    assert_eq!(p.glass(right, bottom), ga, "the whole frame reached glass");

    frame.draw_target().clear(b).unwrap();
    p.present(frame, P::damage_full()).await.unwrap();
    let gb = p.glass(0, 0);
    assert_ne!(gb, ga, "the probe colours must stay distinct after native quantization");
    assert_eq!(p.glass(right, bottom), gb, "the repaint reached glass everywhere");
}

/// Damage translation: after a localized change, an *unknown*-damage present must still land the
/// change (and only report the units its strategy pushed). With `expect_refinement` the pairing
/// claims a real damage strategy: a localized change must not re-push the whole frame, and an
/// unchanged frame must push nothing.
pub async fn check_damage_translation<F, P>(
    frame: &mut F,
    p: &mut P,
    base: F::Color,
    mark: F::Color,
    expect_refinement: bool,
) where
    F: NativeFrame,
    F::Color: Copy + PartialEq + Debug,
    P: Presenter<F> + GlassProbe<F>,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, P::damage_full()).await.unwrap();
    let g_base = p.glass(0, 0);

    // One changed pixel in the frame's centre.
    let (cx, cy) = (F::WIDTH as i32 / 2, F::HEIGHT as i32 / 2);
    frame.draw_target().fill_solid(&Rectangle::new(Point::new(cx, cy), Size::new(1, 1)), mark).unwrap();
    let s = p.present(frame, P::damage_unknown()).await.unwrap();
    assert_ne!(p.glass(cx as u32, cy as u32), g_base, "the changed pixel reached glass");
    assert_eq!(p.glass(0, 0), g_base, "unchanged pixels keep their glass");
    if expect_refinement {
        assert!(s.pushed_units < s.total_units, "a localized change must not re-push the whole frame");
        assert!(s.pushed_units > 0, "…but it must push something");
        let s2 = p.present(frame, P::damage_unknown()).await.unwrap();
        assert_eq!(s2.pushed_units, 0, "an unchanged frame pushes nothing (a spurious redraw is free)");
    }
}

/// Overlay backdrop composition: the composite reads the **clean frame** as its backdrop — pixels
/// the overlay closure doesn't paint show the frame, painted ones show the overlay — and the
/// frame's backing is **byte-identical when the call returns** (the clean-frame postcondition; a
/// mutate-and-restore composite like the FLPR's passes only if its restore is exact).
///
/// `overlay_rect` is the frame-space overlay window; `mark_at` a pixel inside it the closure
/// paints; `backdrop_at` a pixel inside it the closure leaves alone. `snapshot` captures the
/// frame's full backing for equality comparison (the harness is `no_std`/alloc-free, so the
/// caller owns the representation — e.g. `|f| f.bytes().to_vec()` on a std host, or a copy of an
/// owned cell array).
#[allow(clippy::too_many_arguments)] // a test harness taking explicit probe points, not an API to hold small
pub async fn check_overlay_backdrop<F, P, S>(
    frame: &mut F,
    p: &mut P,
    base: F::Color,
    mark: F::Color,
    overlay_rect: Rectangle,
    mark_at: Probe,
    backdrop_at: Probe,
    snapshot: impl Fn(&F) -> S,
) where
    F: NativeFrame,
    F::Color: Copy + PartialEq + Debug,
    P: OverlayPresenter<F> + GlassProbe<F>,
    S: PartialEq,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, P::damage_full()).await.unwrap();
    let g_base = p.glass(mark_at.0, mark_at.1);

    let region = P::region(overlay_rect);
    let mark_rect = Rectangle::new(Point::new(mark_at.0 as i32, mark_at.1 as i32), Size::new(1, 1));
    let clean = snapshot(frame);
    p.present_overlay(frame, region, |t| {
        let _ = t.fill_solid(&mark_rect, mark);
    })
    .await
    .expect("overlay present reports transport success");
    assert!(snapshot(frame) == clean, "present_overlay must leave the frame's backing byte-identical");
    assert_ne!(p.glass(mark_at.0, mark_at.1), g_base, "the overlay mark reached glass");
    assert_eq!(p.glass(backdrop_at.0, backdrop_at.1), g_base, "un-drawn overlay pixels show the clean-frame backdrop");
}

/// Live-overlay exclusion: a base present *around* a live overlay updates the rest of the glass
/// without ever flashing the overlay off; the overlay's next own re-present composites over the
/// **current** clean frame (fresh backdrop, no map re-render); and the around-present must have
/// kept the presenter's damage state tracking the **clean** frame for the excluded units — after
/// the trailing clear, an unknown-damage present pushes nothing (`expect_refinement`).
///
/// `outside_at` must lie outside the presenter's *widened* region for `overlay_rect` (the concrete
/// test knows the widening rule). `snapshot` is the clean-frame postcondition capture, as in
/// [`check_overlay_backdrop`].
#[allow(clippy::too_many_arguments)] // a test harness taking explicit probe points, not an API to hold small
pub async fn check_overlay_exclusion<F, P, S>(
    frame: &mut F,
    p: &mut P,
    base: F::Color,
    mark: F::Color,
    fresh: F::Color,
    overlay_rect: Rectangle,
    mark_at: Probe,
    backdrop_at: Probe,
    outside_at: Probe,
    snapshot: impl Fn(&F) -> S,
    expect_refinement: bool,
) where
    F: NativeFrame,
    F::Color: Copy + PartialEq + Debug,
    P: OverlayPresenter<F> + GlassProbe<F>,
    S: PartialEq,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, P::damage_full()).await.unwrap();
    let g_base = p.glass(outside_at.0, outside_at.1);

    // The bulge pops over the base frame.
    let region = P::region(overlay_rect);
    let mark_rect = Rectangle::new(Point::new(mark_at.0 as i32, mark_at.1 as i32), Size::new(1, 1));
    let clean = snapshot(frame);
    p.present_overlay(frame, region, |t| {
        let _ = t.fill_solid(&mark_rect, mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "present_overlay must leave the frame's backing byte-identical");
    let g_bulge = p.glass(mark_at.0, mark_at.1);
    assert_ne!(g_bulge, g_base, "the bulge is on glass");

    // The map plane re-renders the whole frame and presents AROUND the live overlay.
    frame.draw_target().clear(fresh).unwrap();
    p.present(frame, P::damage_around(region)).await.unwrap();
    assert_eq!(p.glass(mark_at.0, mark_at.1), g_bulge, "a base present around a live overlay never flashes it off");
    let g_fresh = p.glass(outside_at.0, outside_at.1);
    assert_ne!(g_fresh, g_base, "the rest of the frame was updated around the overlay");

    // The overlay plane's next tick: same bulge, composited over the *current* clean frame.
    let clean = snapshot(frame);
    p.present_overlay(frame, region, |t| {
        let _ = t.fill_solid(&mark_rect, mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "the live re-present must leave the frame's backing byte-identical");
    assert_eq!(p.glass(mark_at.0, mark_at.1), g_bulge, "the re-presented bulge is intact");
    assert_eq!(p.glass(backdrop_at.0, backdrop_at.1), g_fresh, "the overlay backdrop is the current clean frame");

    // Retract + trailing clear: the excluded units' damage state must have tracked the CLEAN frame
    // through the around-present, so once the clear restores the clean glass, nothing is left
    // stale — an unknown-damage present finds the whole frame already agreeing.
    p.clear_overlay(frame, region).await.unwrap();
    assert_eq!(p.glass(mark_at.0, mark_at.1), g_fresh, "the trailing clear restores the current clean frame");
    if expect_refinement {
        let s = p.present(frame, P::damage_unknown()).await.unwrap();
        assert_eq!(s.pushed_units, 0, "excluded units tracked the clean frame — nothing stale to re-push");
    }
}

/// Pop, retract, and the trailing clear: each overlay composite starts from the clean frame (so a
/// shrinking bulge needs no undo pass — withdrawn pixels revert to backdrop), clearing the overlay
/// restores the clean frame with **no map re-render**, and every overlay call leaves the frame's
/// backing byte-identical (`snapshot`, as in [`check_overlay_backdrop`]). With
/// `expect_refinement`, the presenter's damage state must already agree with the clean frame
/// afterwards: the next unknown-damage present pushes nothing.
#[allow(clippy::too_many_arguments)] // a test harness taking explicit probe points, not an API to hold small
pub async fn check_overlay_pop_retract_clear<F, P, S>(
    frame: &mut F,
    p: &mut P,
    base: F::Color,
    mark: F::Color,
    overlay_rect: Rectangle,
    tip_at: Probe,
    edge_at: Probe,
    snapshot: impl Fn(&F) -> S,
    expect_refinement: bool,
) where
    F: NativeFrame,
    F::Color: Copy + PartialEq + Debug,
    P: OverlayPresenter<F> + GlassProbe<F>,
    S: PartialEq,
{
    frame.draw_target().clear(base).unwrap();
    p.present(frame, P::damage_full()).await.unwrap();
    let g_base = p.glass(tip_at.0, tip_at.1);

    let region = P::region(overlay_rect);
    let tip = Rectangle::new(Point::new(tip_at.0 as i32, tip_at.1 as i32), Size::new(1, 1));
    let edge = Rectangle::new(Point::new(edge_at.0 as i32, edge_at.1 as i32), Size::new(1, 1));
    let clean = snapshot(frame);

    // Pop: the bulge covers both probe pixels.
    p.present_overlay(frame, region, |t| {
        let _ = t.fill_solid(&tip, mark);
        let _ = t.fill_solid(&edge, mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "pop: present_overlay must leave the frame's backing byte-identical");
    assert_ne!(p.glass(tip_at.0, tip_at.1), g_base, "pop: the bulge tip is on glass");
    assert_ne!(p.glass(edge_at.0, edge_at.1), g_base, "pop: the bulge edge is on glass");

    // Retract: the next tick draws a smaller bulge — the tip pixel is withdrawn and must revert
    // to the clean backdrop without any explicit erase (composites are not cumulative).
    p.present_overlay(frame, region, |t| {
        let _ = t.fill_solid(&edge, mark);
    })
    .await
    .unwrap();
    assert!(snapshot(frame) == clean, "retract: present_overlay must leave the frame's backing byte-identical");
    assert_eq!(p.glass(tip_at.0, tip_at.1), g_base, "retract: withdrawn pixels revert to the clean backdrop");
    assert_ne!(p.glass(edge_at.0, edge_at.1), g_base, "retract: the remaining bulge stays");

    // Quiet: the trailing clear restores the clean frame under the whole region.
    p.clear_overlay(frame, region).await.unwrap();
    assert!(snapshot(frame) == clean, "trailing clear: clear_overlay must leave the frame's backing byte-identical");
    assert_eq!(p.glass(edge_at.0, edge_at.1), g_base, "trailing clear: the clean frame is restored");
    assert_eq!(p.glass(tip_at.0, tip_at.1), g_base);

    if expect_refinement {
        let s = p.present(frame, P::damage_unknown()).await.unwrap();
        assert_eq!(s.pushed_units, 0, "after the trailing clear the damage state already agrees with the clean frame");
    }
}
