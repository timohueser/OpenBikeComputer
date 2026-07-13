//! Shared local-equirectangular geometry: ground distances in meters from
//! microdegree coordinates.
//!
//! This is the Earth-model *distance core* every layer above the byte formats shares:
//! `obc-route`'s converter (exact ride stats), its elevation profile and matcher, the
//! app's breadcrumb decimator and the renderer's route-chevron walk all measure segment
//! distance through these helpers, so they can't drift from the cumulative distances the
//! route format stored. It lives here (next to [`M_PER_DEG`](crate::M_PER_DEG)) because
//! this crate is the bottom of the shared stack — `obc-route` re-exports these for its
//! callers, and `obc-render` reaches them without depending on the route format.
//!
//! The projection is a local equirectangular approximation (east scaled by `cos(lat)`):
//! accurate over the short segments of a decimated route, and cheap (no per-segment
//! haversine).
//!
//! **All math is `f32`.** The Cortex-M33 FPU is single-precision, so an `f64` op here is a
//! soft-float call (~10×) — and this runs in per-fix and per-frame hot loops. Microdegree→meter
//! *deltas* over a decimated route's short segments fit `f32` with ample precision; only a long
//! route's *cumulative* distance needs `f64`, which the callers accumulate themselves on top of
//! these per-segment `f32` measurements.
//!
//! `cos(lat)` varies slowly, so hot callers hoist [`cos_lat`] once per latitude band and pass
//! the result to the `_cl` helpers rather than recomputing `cosf` per segment.

/// Meters per degree of latitude (and of longitude at the equator) — the `f32` form of
/// the shared [`crate::M_PER_DEG`], so the route's distances, the packer's simplify
/// tolerance and the renderer's scale all derive from one Earth model.
pub(crate) const M_PER_DEG: f32 = crate::M_PER_DEG as f32;

/// `cos(latitude)` for the local east-scaling, from latitude in microdegrees. Hoist this
/// once per latitude band and pass it to the `_cl` helpers across a run of nearby
/// segments — over a route window the latitude barely changes.
pub fn cos_lat(lat_ud: i32) -> f32 {
    libm::cosf((lat_ud as f32 / 1e6).to_radians())
}

/// East/north offset in meters from `from` to `to` (each `(lon, lat)` microdegrees),
/// given a precomputed `cl = cos_lat` for the band. The microdegree *delta* is small, so
/// the `f32` cast keeps full precision.
pub fn delta_m(from: (i32, i32), to: (i32, i32), cl: f32) -> (f32, f32) {
    let dlon = (to.0 - from.0) as f32 * 1e-6;
    let dlat = (to.1 - from.1) as f32 * 1e-6;
    (dlon * M_PER_DEG * cl, dlat * M_PER_DEG)
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points, given a
/// precomputed `cl = cos_lat` for the band — hoist `cl` across a run of segments.
pub fn seg_dist_m_cl(a: (i32, i32), b: (i32, i32), cl: f32) -> f32 {
    let (mx, my) = delta_m(a, b, cl);
    libm::sqrtf(mx * mx + my * my)
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points. Computes
/// `cos_lat` from `a`'s latitude; prefer [`seg_dist_m_cl`] in a loop that can hoist it.
pub fn seg_dist_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    seg_dist_m_cl(a, b, cos_lat(a.1))
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points, given a
/// precomputed `cl = cos_lat` for the band. A public wrapper over [`seg_dist_m_cl`] for
/// the app/render hot paths that walk many segments at one latitude (the camera's `cl`).
pub fn ground_dist_m_cl(a: (i32, i32), b: (i32, i32), cl: f32) -> f32 {
    seg_dist_m_cl(a, b, cl)
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points — the public
/// wrapper over [`seg_dist_m`], so the app measures ridden distance with the metric the route
/// format stored. Prefer [`ground_dist_m_cl`] in a loop that can hoist `cos_lat`.
pub fn ground_dist_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    seg_dist_m(a, b)
}
