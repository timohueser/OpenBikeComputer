//! [`Dirty`] — the per-frame repaint signal the render-on-demand host drains.

/// Which display planes changed this frame and so must be repainted.
///
/// The display composites two planes independently: the expensive **map** (base-map render, tens
/// of ms) and the cheap transient **overlay** chrome (hold bulge / confirm ring, a couple of ms).
/// Tracking them separately lets an animating ring repaint over an unchanged map without
/// re-rendering the map.
///
/// [`App`](crate::App) accumulates this as state mutates; the host drains it once per frame with
/// [`App::take_dirty`](crate::App::take_dirty), rendering each plane only when its flag is set. A
/// static screen with no input, no fresh fix and no pending animation drains [`Dirty::CLEAN`] and
/// renders nothing (the render-on-demand model, replacing a blind full-map heartbeat wasteful on a
/// MIP / battery target).
///
/// Guiding rule: **over-redraw is safe, under-redraw is a bug** — a spurious flag costs one extra
/// frame, a missed one leaves stale pixels. So every mutation that *might* affect a plane sets it;
/// the flags are deliberately conservative, not minimal.
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
