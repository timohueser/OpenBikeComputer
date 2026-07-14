//! [`Presenter`] / [`OverlayPresenter`] — the presentation capability contracts a panel backend
//! implements for the [`NativeFrame`] it pairs with.
//!
//! A presenter owns the *transport* and its own damage bookkeeping; it never owns render or domain
//! policy. Damage is deliberately presenter-typed ([`Presenter::Damage`]): one pairing self-diffs
//! and pushes masked row spans, another may diff tiles or use a controller dirty window, and
//! neither model leaks into the other. The neutral vocabulary every
//! caller can speak is small and constructor-shaped — [`damage_full`](Presenter::damage_full),
//! [`damage_unknown`](Presenter::damage_unknown), and (for overlay-capable pairings)
//! [`damage_around`](OverlayPresenter::damage_around) a live overlay region — so generic host code
//! never names rows, tiles, or hashes.
//!
//! The methods are `async` with no `Send` bound and no boxed futures: a coprocessor-driven backend
//! awaits its frame ack while the executor runs other futures, and a host backend completes
//! synchronously under the same signature.

use core::fmt::Debug;

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::primitives::Rectangle;

use super::frame::NativeFrame;

/// What one present cost, in the presenter's own damage grain (**units** — rows for a
/// row-addressed pairing, tiles for a tiled one). `pushed_units == 0` is the "spurious redraw is free" outcome;
/// `pushed_units == total_units` is a full push. `regions` counts the disjoint pushed regions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentStats {
    /// Damage units actually pushed to glass this present.
    pub pushed_units: u32,
    /// The frame's total unit count, for context (`pushed_units / total_units` = push fraction).
    pub total_units: u32,
    /// Disjoint regions pushed (contiguous span/tile runs).
    pub regions: u32,
}

/// Base-frame presentation: put the **clean resident frame** on glass.
///
/// The frame is borrowed **shared** for the whole present — the caller keeps rendering statically
/// impossible until the transport is done with the bytes (on the shipping board the coprocessor
/// scans the resident frame directly, so this borrow is load-bearing, not stylistic).
///
/// Errors are transport outcomes ([`Error`](Self::Error)): the frame did not (fully) reach glass
/// and the caller may retry with the same bytes. Presenting must never report success for pixels
/// it dropped.
#[allow(async_fn_in_trait)] // single-core executors on-device + a synchronous host backend — no Send bound wanted
pub trait Presenter<F: NativeFrame> {
    /// This presenter's damage description for one present — rows, tiles, a controller window…
    /// Constructed by the caller only through the neutral constructors below (or
    /// [`OverlayPresenter::damage_around`]), so the grain never leaks into generic code.
    type Damage;
    /// A transport fault: the push did not reach glass; the caller keeps the frame and may retry.
    type Error: Debug;

    /// Damage meaning "push every unit" — a forced full repaint (panel re-init, transport
    /// recovery). The constructor is a plain value; the contract is on the [`present`](Self::present)
    /// driven with it, which must also resynchronize whatever damage state the presenter keeps, so
    /// the next [`damage_unknown`](Self::damage_unknown) present is diffed against what is actually
    /// on glass.
    fn damage_full() -> Self::Damage;
    /// Damage meaning "the frame may have changed anywhere" — the immediate-mode redraw case. A
    /// presenter with its own damage strategy refines this (a self-diffing pairing pushes only the
    /// units that changed); one without pushes fully. The choice is the *pairing's*, made at compile time — generic code
    /// never falls back to a slow path silently, because there is no generic fallback.
    fn damage_unknown() -> Self::Damage;

    /// Present the clean frame under `damage`. Returns what was pushed, or the transport fault.
    async fn present(&mut self, frame: &F, damage: Self::Damage) -> Result<PresentStats, Self::Error>;
}

/// Transient overlay presentation over a [`Presenter`]'s clean frame — the capability the
/// preemptive hold-bulge plane needs, kept separate because not every panel can re-present a
/// bounded region cheaply (and a board that needs it simply requires this bound statically).
///
/// The overlay is **never rendered into the frame by the caller**: the presenter reads the clean
/// frame as the backdrop, hands the closure a bounded, frame-absolute draw target, and puts the
/// composite on glass. It may stage the composite in bounded scratch or transiently in
/// hardware/the resident frame — but the frame's backing must be **byte-identical when the call
/// returns** (that is why the frame is borrowed `&mut`: a backend whose transport scans the
/// resident frame composites into those bytes and restores them, which shared access could not
/// express).
/// Because each composite starts from the clean frame, a shrinking bulge redraws less and the
/// backdrop shows through — retraction needs no undo pass — and
/// [`clear_overlay`](Self::clear_overlay) (compositing nothing) *is* the trailing clear.
///
/// Coordination with the base present: while an overlay is live, the map plane presents **around**
/// it with [`damage_around`](Self::damage_around)`(region)` — the presenter must keep its damage
/// state tracking the *clean* frame for the excluded units (so the trailing clear leaves nothing
/// stale), while not pushing them (pushing clean bytes under a live bulge would flash it off).
#[allow(async_fn_in_trait)]
pub trait OverlayPresenter<F: NativeFrame>: Presenter<F> {
    /// A bounded overlay region in this presenter's grain. Constructed from a frame-space
    /// rectangle by [`region`](Self::region); the presenter may widen it to its grain (a
    /// row-addressed panel widens to full-width rows, a tiled panel to whole tiles).
    type Region: Copy;
    /// The frame-absolute draw target the composite closure paints into. Writes outside the
    /// region are clipped, exactly like off-frame writes. The target borrows call-local composite
    /// scratch, deliberately **not** the presenter (no `Self: 't` clause): that keeps
    /// `for<'t> …OverlayTarget<'t> = Concrete<'t>` bounds usable without forcing `Self: 'static`.
    type OverlayTarget<'t>: DrawTarget<Color = F::Color>;

    /// The smallest region of this presenter's grain covering `rect` (frame coordinates).
    fn region(rect: Rectangle) -> Self::Region;
    /// The base-present damage that self-diffs the frame while **excluding** `region`'s units —
    /// how a map redraw goes *around* a live bulge so it never flashes off.
    fn damage_around(region: Self::Region) -> Self::Damage;

    /// Composite `draw` over the clean-frame backdrop within `region` and push only that region.
    /// `draw` is called **once** per overlay present (never per row/tile), so a caller's brief
    /// input-plane lock inside it is taken once per overlay frame. The frame's backing is
    /// byte-identical on return — see the trait docs.
    async fn present_overlay(
        &mut self,
        frame: &mut F,
        region: Self::Region,
        draw: impl for<'t> FnOnce(&mut Self::OverlayTarget<'t>),
    ) -> Result<PresentStats, Self::Error>;

    /// The trailing clear: re-present `region` with nothing composited — the clean frame restored
    /// under a just-retracted overlay, **without a map re-render** (the backdrop is read from the
    /// resident frame the caller never touched).
    async fn clear_overlay(&mut self, frame: &mut F, region: Self::Region) -> Result<PresentStats, Self::Error> {
        self.present_overlay(frame, region, |_target| {}).await
    }
}
