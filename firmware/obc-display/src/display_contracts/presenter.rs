//! [`Presenter`] and [`OverlayPresenter`]: the presentation capability contracts a panel backend
//! implements for the [`NativeFrame`] it pairs with.
//!
//! A presenter owns the transport and its own damage bookkeeping; it never owns render or domain
//! policy. Damage is presenter-typed, so one pairing can self-diff and push masked row spans
//! while another diffs tiles, and neither model leaks into the other. The neutral vocabulary
//! every caller can speak is small and constructor-shaped, so generic host code never names rows,
//! tiles or hashes.
//!
//! The methods are `async` with no `Send` bound and no boxed futures: a coprocessor-driven
//! backend awaits its frame ack while the executor runs other futures, and a host backend
//! completes synchronously under the same signature.

use core::fmt::Debug;

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::primitives::Rectangle;

use super::frame::NativeFrame;

/// What one present cost, in the presenter's own damage grain: rows for a row-addressed pairing,
/// tiles for a tiled one. `pushed_units == 0` is the "a spurious redraw is free" outcome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentStats {
    /// Damage units actually pushed to glass this present.
    pub pushed_units: u32,
    /// The frame's total unit count, for context.
    pub total_units: u32,
    /// Disjoint regions pushed (contiguous span/tile runs).
    pub regions: u32,
}

/// Base-frame presentation: put the clean resident frame on glass.
///
/// The frame is borrowed shared for the whole present, which makes rendering into it impossible
/// until the transport is done with the bytes. On the shipping board the coprocessor scans the
/// resident frame directly, so that borrow is load-bearing.
///
/// Errors are transport outcomes: the frame did not fully reach glass and the caller may retry
/// with the same bytes. Presenting must never report success for pixels it dropped.
#[allow(async_fn_in_trait)] // single-core executors on-device + a synchronous host backend — no Send bound wanted
pub trait Presenter<F: NativeFrame> {
    /// This presenter's damage description for one present. The caller constructs it only through
    /// the neutral constructors below, so the grain never leaks into generic code.
    type Damage;
    /// A transport fault: the push did not reach glass; the caller keeps the frame and may retry.
    type Error: Debug;

    /// Damage meaning "push every unit": a forced full repaint after a panel re-init or transport
    /// recovery. The [`present`](Self::present) driven with it must also resynchronize whatever
    /// damage state the presenter keeps, so the next
    /// [`damage_unknown`](Self::damage_unknown) present is diffed against what is on glass.
    fn damage_full() -> Self::Damage;
    /// Damage meaning "the frame may have changed anywhere": the immediate-mode redraw case. A
    /// presenter with its own damage strategy refines it, and one without pushes fully. The
    /// choice is the pairing's, made at compile time, so there is no silent generic fallback.
    fn damage_unknown() -> Self::Damage;

    /// Present the clean frame under `damage`. Returns what was pushed, or the transport fault.
    async fn present(&mut self, frame: &F, damage: Self::Damage) -> Result<PresentStats, Self::Error>;
}

/// Transient overlay presentation over a [`Presenter`]'s clean frame: the capability the
/// preemptive hold-bulge plane needs, kept separate because not every panel can re-present a
/// bounded region cheaply.
///
/// The caller never renders the overlay into the frame. The presenter reads the clean frame as
/// the backdrop, hands the closure a bounded, frame-absolute draw target, and puts the composite
/// on glass. It may stage the composite in bounded scratch or transiently in the resident frame,
/// but the frame's backing must be byte-identical when the call returns. That is why the frame is
/// borrowed `&mut`: a backend whose transport scans the resident frame composites into those
/// bytes and restores them, which shared access could not express.
///
/// Because each composite starts from the clean frame, a shrinking bulge redraws less and the
/// backdrop shows through, so retraction needs no undo pass, and
/// [`clear_overlay`](Self::clear_overlay) is the trailing clear.
///
/// While an overlay is live, the map plane presents around it with
/// [`damage_around`](Self::damage_around)`(region)`: the presenter keeps its damage state
/// tracking the clean frame for the excluded units, so the trailing clear leaves nothing stale,
/// while not pushing them, because pushing clean bytes under a live bulge would flash it off.
#[allow(async_fn_in_trait)]
pub trait OverlayPresenter<F: NativeFrame>: Presenter<F> {
    /// A bounded overlay region in this presenter's grain, constructed from a frame-space
    /// rectangle by [`region`](Self::region). The presenter may widen it to its own grain.
    type Region: Copy;
    /// The frame-absolute draw target the composite closure paints into. Writes outside the
    /// region are clipped, exactly like off-frame writes. The target borrows call-local composite
    /// scratch and deliberately not the presenter, which keeps `for<'t>` bounds usable without
    /// forcing `Self: 'static`.
    type OverlayTarget<'t>: DrawTarget<Color = F::Color>;

    /// The smallest region of this presenter's grain covering `rect` (frame coordinates).
    fn region(rect: Rectangle) -> Self::Region;
    /// The base-present damage that self-diffs the frame while excluding `region`'s units: how a
    /// map redraw goes around a live bulge so it never flashes off.
    fn damage_around(region: Self::Region) -> Self::Damage;

    /// Composite `draw` over the clean-frame backdrop within `region` and push only that region.
    /// `draw` is called once per overlay present, never per row or tile, so a caller's brief
    /// input-plane lock inside it is taken once per overlay frame. The frame's backing is
    /// byte-identical on return.
    async fn present_overlay(
        &mut self,
        frame: &mut F,
        region: Self::Region,
        draw: impl for<'t> FnOnce(&mut Self::OverlayTarget<'t>),
    ) -> Result<PresentStats, Self::Error>;

    /// The trailing clear: re-present `region` with nothing composited, restoring the clean frame
    /// under a just-retracted overlay with no map re-render.
    async fn clear_overlay(&mut self, frame: &mut F, region: Self::Region) -> Result<PresentStats, Self::Error> {
        self.present_overlay(frame, region, |_target| {}).await
    }
}
