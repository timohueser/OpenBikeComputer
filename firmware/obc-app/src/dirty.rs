//! [`Dirty`] — the per-frame repaint signal the render-on-demand host drains.

use embedded_graphics::primitives::Rectangle;

/// Which display planes changed this frame and so must be repainted.
///
/// The display composites two planes independently: the expensive map (tens of ms) and the cheap
/// transient overlay chrome (a couple of ms). Tracking them apart lets an animating ring repaint
/// over an unchanged map. The pass produces one of these per frame and the host renders each plane
/// only when its flag is set, so a static screen renders nothing.
///
/// Guiding rule: over-redraw is safe, under-redraw is a bug. A spurious flag costs one extra frame,
/// a missed one leaves stale pixels, so anything uncertain redraws.
///
/// The map flag has two sources. Every `screens!` row states a
/// [`RenderKeyKind`](crate::screen::RenderKeyKind), the exact facts its draw reads, and a moved key
/// sets this flag; that covers every mutation inside a pass, per screen. Everything else is an
/// explicit request, and there are five classes of them. A site in none of these should be a key.
///
/// 1. A host seam that runs between two passes, such as `set_ble_status`. The fact has already
///    moved by the time the next pass builds its first key. Each becomes key-covered as it moves
///    into [`ExternalFacts`](crate::device_core::ExternalFacts).
/// 2. State inside a screen, such as a menu's highlighted row. It lives in the screen's own typed
///    state, which no key names, so `apply_gesture` dirties the map for every recognised gesture.
/// 3. The card scheduler's sweep, which already answers "did anything visible move" once per sweep
///    at its single door onto the stack, and which sweeps from both sides of the pass boundary.
/// 4. A planner landing that rewrites the stack and moves domain state behind it that no row draws.
/// 5. Resident data no row declares: the route and ride catalogs, the derived ride profile and
///    preview, the geometry the matcher re-anchors, the two debug doors and the non-region tick.
///
/// [`region`](Dirty::region) is unaffected by either: a full-frame demand still folds a region away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dirty {
    pub map: bool,
    /// The overlay plane (Layer 2) must be repainted: the hold bulge is charging, popping or
    /// retracting, or it just went quiet and the last frame must be cleared off the layer.
    pub overlay: bool,
    /// Where this frame's [`map`](Dirty::map) demand is contained, in panel pixels. `None` means
    /// anywhere. `Some(r)` only when every accumulated demand came from a screen tick that promised
    /// its change lies inside `r`; any other source folds the region away. A host may clip the
    /// repaint to `r`, but ignoring it and repainting fully is always correct.
    pub region: Option<Rectangle>,
    /// The [`region`](Dirty::region) is opaque chrome the screen redraws whole, so a host may
    /// repaint it without rendering the map under it. Always `false` without a region.
    pub opaque: bool,
}

impl Dirty {
    /// Nothing changed — render neither plane.
    pub const CLEAN: Dirty = Dirty { map: false, overlay: false, region: None, opaque: false };

    pub fn any(self) -> bool {
        self.map || self.overlay
    }

    /// The region this frame repaints with no map under it, or `None` when the map must render.
    pub fn map_free_region(self) -> Option<Rectangle> {
        self.region.filter(|_| self.map && self.opaque)
    }
}
