//! [`Dirty`] — the per-frame repaint signal the render-on-demand host drains.

use embedded_graphics::primitives::Rectangle;

/// Which display planes changed this frame and so must be repainted.
///
/// The display composites two planes independently: the expensive **map** (base-map render, tens
/// of ms) and the cheap transient **overlay** chrome (hold bulge / confirm ring, a couple of ms).
/// Tracking them separately lets an animating ring repaint over an unchanged map without
/// re-rendering the map.
///
/// The pass produces one of these per frame ([`PassPlan::render`](crate::device_core::PassPlan)),
/// and the host renders each plane only when its flag is set. A static screen with no input, no
/// fresh fix and no pending animation plans [`Dirty::CLEAN`] and renders nothing (the
/// render-on-demand model, replacing a blind full-map heartbeat wasteful on a MIP / battery target).
///
/// Guiding rule: **over-redraw is safe, under-redraw is a bug** — a spurious flag costs one extra
/// frame, a missed one leaves stale pixels. So anything uncertain redraws; the flags are
/// deliberately conservative, not minimal.
///
/// # Where the map flag comes from
///
/// Two sources, and the split is the whole of the contract (#1447):
///
/// 1. **The declared render keys.** Every `screens!` row states a
///    [`RenderKeyKind`](crate::screen::RenderKeyKind) — the exact facts its draw reads. The pass
///    builds the visible stack's key before its stages and again after them, and a moved key sets
///    this flag. This covers every mutation that happens *inside* a pass, and it covers it per
///    screen: a heart-rate sample repaints the grid that draws it and not the map beside it.
/// 2. **An explicit request**, for a mutation no key can see. There are **five** classes, and a
///    site that is in none of them is a site that should be a key:
///    - **A host seam that runs between two passes** — `set_routes_with_ids`, `set_ble_status`,
///      `set_sensor_status`, `weather_feed_changed`, `set_rain_view`, `set_map_transfer` and their
///      siblings. The fact has already moved by the time the next pass builds its *before* key, so
///      both keys agree. They become key-covered as each seam moves into
///      [`ExternalFacts`](crate::device_core::ExternalFacts) and is consumed at stage 2.
///    - **State inside a screen** — a menu's highlighted row, a list's scroll position, a chooser's
///      anchor. It lives in the screen's own typed state, which no key names, so
///      `apply_gesture` dirties the map for every recognised gesture. Conservative by design: a
///      gesture a screen ignores still costs one redraw, and the idle path stays exact because no
///      gesture means no call.
///    - **The card scheduler's sweep** —
///      [`run_card_sweep`](crate::ui_runtime::UiRuntime::run_card_sweep). The scheduler is the one
///      writer of every host-pushed card and it already answers "did anything visible move" exactly,
///      once per sweep, at its single door onto the stack. A revision counter beside that bool would
///      be a second, resident copy of the same answer. It is in its own class because it sweeps from
///      *both* sides of the boundary: inside the pass each frame, and again from every host seam
///      that posts a fact.
///    - **A planner landing that rewrites the stack** — `land_route_plan`, `land_detour_plan`,
///      `land_detour_commit`, `end_plan`, `admit_navigator_intent`. Each replaces a screen in place
///      and moves domain state behind it, some of which no row draws.
///    - **Resident data no row declares** — the route and ride catalogs the lists draw, the derived
///      ride profile and preview, the geometry the matcher re-anchors after a splice, plus the two
///      debug doors and the non-region screen tick.
///
/// [`region`](Dirty::region) is unaffected by either: a full-frame demand still folds a region away.
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
    /// Where this frame's [`map`](Dirty::map) demand is contained, in panel pixels — `None` means
    /// anywhere (the full repaint every dirt source implies by default). `Some(r)` only when
    /// *every* accumulated map demand came from a screen tick that promised its change lies inside
    /// `r` ([`ScreenTick::region`](crate::screen::ScreenTick::region) — the nav-planning spinner's
    /// needle disc, and the Map's clock pill, whose minute rollover is a region-clipped digit tick
    /// rather than a ~97 ms map render); any other source folds the region away. A host may then clip the
    /// repaint (render + push) to `r`; ignoring it and repainting fully is always correct — the
    /// region is an optimization bound, never a requirement (over-redraw stays safe).
    pub region: Option<Rectangle>,
}

impl Dirty {
    /// Nothing changed — render neither plane.
    pub const CLEAN: Dirty = Dirty { map: false, overlay: false, region: None };

    /// Whether either plane needs a repaint this frame.
    pub fn any(self) -> bool {
        self.map || self.overlay
    }
}
