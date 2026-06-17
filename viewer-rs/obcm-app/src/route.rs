//! Routes — the loadable rides shown in the Route menu.
//!
//! For now this is a **static mock list**; the real list will be synced from the
//! companion app over BLE into device storage. Callers go through [`routes`]
//! rather than touching the representation, so that swap is a local change here —
//! and the [`Route`] fields are the stable interface the screens already render
//! against. Route *geometry* (the polyline + elevation profile, for drawing on the
//! Map and the Elevation screen) joins [`Route`] when route loading lands.

/// A loadable route. Distances are whole units for the v1 stat displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    pub name: &'static str,
    /// Total distance, km.
    pub distance_km: u32,
    /// Total climb, m.
    pub climb_m: u32,
}

/// Mock route list (the stand-in until routes sync over BLE). Order is the
/// Route-menu order.
const MOCK: [Route; 4] = [
    Route { name: "Alpine Loop", distance_km: 142, climb_m: 3200 },
    Route { name: "Black Forest", distance_km: 88, climb_m: 1450 },
    Route { name: "River Valley", distance_km: 56, climb_m: 620 },
    Route { name: "Vosges Crossing", distance_km: 124, climb_m: 2600 },
];

/// The available routes — today the mock list, later the BLE-synced one. The
/// Route menu lists these and [`Activity::active_route`](crate::Activity::active_route)
/// indexes into them.
pub fn routes() -> &'static [Route] {
    &MOCK
}
