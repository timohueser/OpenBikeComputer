//! [`Dirty`] — the per-frame repaint signal the render-on-demand host drains.

/// Which display planes changed this frame and so must be repainted.
///
/// The display composites two planes independently (issue #46): the expensive **map**
/// (the base-map render, tens of ms) and the cheap transient **overlay** chrome (the hold
/// bulge / confirm ring, a couple of ms). Tracking the two separately is what lets an
/// animating ring repaint over an unchanged map without re-rendering the map.
///
/// [`App`](crate::App) accumulates this as state mutates, and the host drains it once per
/// frame with [`App::take_dirty`](crate::App::take_dirty) — rendering
/// [`render_map`](crate::App::render_map) only when [`map`](Dirty::map) and
/// [`render_overlay`](crate::App::render_overlay) only when [`overlay`](Dirty::overlay).
/// A static screen with no input, no fresh fix and no pending animation drains
/// [`Dirty::CLEAN`] and renders nothing — the render-on-demand model issue #47 calls for,
/// replacing the blind 1 s full-map heartbeat (a 24–51 ms map render purely to keep
/// time-based screens live, wasteful on a MIP / battery target).
///
/// The guiding rule is **over-redraw is safe, under-redraw is a bug**: a spuriously set
/// flag merely costs one extra frame, whereas a missed one leaves stale pixels on the
/// panel. So every state mutation that *might* affect a plane sets it — the flags are
/// deliberately conservative, not minimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dirty {
    /// The map plane (Layer 1) must be re-rendered: the camera moved, the zoom or pan
    /// changed, the active route or the visible screen changed, a fresh fix moved the
    /// marker on a riding view, or a screen's timed content advanced.
    pub map: bool,
    /// The overlay plane (Layer 2) must be repainted: the hold bulge is charging, popping
    /// or retracting — or it just went quiet and the last frame must be cleared off the
    /// layer.
    pub overlay: bool,
}

impl Dirty {
    /// Nothing changed — render neither plane.
    pub const CLEAN: Dirty = Dirty { map: false, overlay: false };

    /// Whether either plane needs a repaint this frame.
    pub fn any(self) -> bool {
        self.map || self.overlay
    }
}
