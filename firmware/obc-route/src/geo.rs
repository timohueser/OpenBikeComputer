//! Shared local-equirectangular geometry: ground distances in meters from
//! microdegree coordinates.
//!
//! Both the [converter](crate::convert) (exact ride stats) and the
//! [elevation profile](crate::profile) measure segment distance, and they must do
//! it *identically* — the profile buckets points by the same cumulative distance the
//! converter stored in [`ChunkMeta::cum_distance_m`](crate::ChunkMeta). Keeping the
//! one implementation here guarantees they can't drift.
//!
//! The projection is a local equirectangular approximation (east scaled by
//! `cos(lat)`), the same one the converter has always used — accurate over the short
//! segments of a decimated route, and cheap (no per-segment haversine).
//!
//! **All math is `f32`.** The device's Cortex-M33 FPU is single-precision, so every
//! `f64` op here was a soft-float library call (~10× an `f32` op) — and this runs in
//! per-fix and per-frame hot loops. Microdegree→meter *deltas* over a decimated route's
//! short segments are small numbers that fit `f32` with ample precision; the only place
//! that needs the dynamic range of `f64` is a long route's *cumulative* distance, which
//! the callers accumulate themselves (see [`convert`](crate::convert) /
//! [`profile`](crate::profile)) on top of these per-segment `f32` measurements.
//!
//! `cos(lat)` varies slowly, so hot callers hoist [`cos_lat`] once per latitude band and
//! pass the result down ([`delta_m`] / [`seg_dist_m_cl`] / [`project_to_segment`] /
//! [`ground_dist_m_cl`] all take a precomputed `cl`), rather than recomputing `cosf` per
//! segment.

/// Meters per degree of latitude (and of longitude at the equator) — the `f32` form of
/// the shared [`obc_reader::M_PER_DEG`], so the route's distances, the packer's simplify
/// tolerance and the renderer's scale all derive from one Earth model.
pub(crate) const M_PER_DEG: f32 = obc_reader::M_PER_DEG as f32;

/// `cos(latitude)` for the local east-scaling, from latitude in microdegrees. Hoist this
/// once per latitude band and pass it to the `_cl` helpers across a run of nearby
/// segments — over a route window the latitude barely changes.
pub fn cos_lat(lat_ud: i32) -> f32 {
    libm::cosf((lat_ud as f32 / 1e6).to_radians())
}

/// East/north offset in meters from `from` to `to` (each `(lon, lat)` microdegrees),
/// given a precomputed `cl = cos_lat` for the band. The microdegree *delta* is small, so
/// the `f32` cast keeps full precision.
pub(crate) fn delta_m(from: (i32, i32), to: (i32, i32), cl: f32) -> (f32, f32) {
    let dlon = (to.0 - from.0) as f32 * 1e-6;
    let dlat = (to.1 - from.1) as f32 * 1e-6;
    (dlon * M_PER_DEG * cl, dlat * M_PER_DEG)
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points, given a
/// precomputed `cl = cos_lat` for the band — hoist `cl` across a run of segments.
pub(crate) fn seg_dist_m_cl(a: (i32, i32), b: (i32, i32), cl: f32) -> f32 {
    let (mx, my) = delta_m(a, b, cl);
    libm::sqrtf(mx * mx + my * my)
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points. Computes
/// `cos_lat` from `a`'s latitude; prefer [`seg_dist_m_cl`] in a loop that can hoist it.
pub(crate) fn seg_dist_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    seg_dist_m_cl(a, b, cos_lat(a.1))
}

/// Project point `p` onto the segment `a → b`, **clamped** to the segment's ends, in the
/// local-equirectangular metric for a band with precomputed `cl = cos_lat`. Returns
/// `(t, dist_m)`: `t ∈ 0.0..=1.0` is the fractional position of the nearest point along the
/// segment, and `dist_m` the cross-track distance from `p` to it. All three points are
/// `(lon, lat)` microdegrees, measured exactly as [`delta_m`] (exact enough over a route's
/// short segments).
///
/// This is the clamped sibling of the converter's perpendicular-distance test (which
/// measures to the *infinite* chord for decimation); the [route matcher](crate::matcher)
/// needs the clamped on-segment distance, but both share this one projection so they
/// can't drift.
pub(crate) fn project_to_segment(a: (i32, i32), b: (i32, i32), p: (i32, i32), cl: f32) -> (f32, f32) {
    let (bx, by) = delta_m(a, b, cl);
    let (px, py) = delta_m(a, p, cl);
    let len2 = bx * bx + by * by;
    if len2 <= 1e-9 {
        // Degenerate (zero-length) segment: distance to the shared endpoint.
        return (0.0, libm::sqrtf(px * px + py * py));
    }
    let t = ((px * bx + py * by) / len2).clamp(0.0, 1.0);
    let (dx, dy) = (px - bx * t, py - by * t);
    (t, libm::sqrtf(dx * dx + dy * dy))
}

/// Effective area (m²) of the triangle `a, b, c` — the Visvalingam–Whyatt *significance* of
/// vertex `b`: the area the line loses if `b` is dropped and `a` joins straight to `c`. Computed
/// in the local-equirectangular metric (precomputed `cl = cos_lat`); cheap — no `sqrt`, no
/// divide. A fixed-budget simplifier drops the smallest-area vertex, so a straight run (area ≈ 0)
/// yields its points before a bend does, and the metric self-spreads: removing a vertex widens
/// its neighbours' triangles, protecting them next time.
pub fn tri_area_m2_cl(a: (i32, i32), b: (i32, i32), c: (i32, i32), cl: f32) -> f32 {
    let (ux, uy) = delta_m(a, b, cl);
    let (vx, vy) = delta_m(a, c, cl);
    0.5 * (ux * vy - uy * vx).abs()
}

/// Effective area (m²) of triangle `a, b, c`, computing `cos_lat` from `a`'s latitude; prefer
/// [`tri_area_m2_cl`] in a loop that can hoist it.
pub fn tri_area_m2(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> f32 {
    tri_area_m2_cl(a, b, c, cos_lat(a.1))
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points, given a
/// precomputed `cl = cos_lat` for the band. A public wrapper over [`seg_dist_m_cl`] for
/// the app/render hot paths that walk many segments at one latitude (the camera's `cl`).
pub fn ground_dist_m_cl(a: (i32, i32), b: (i32, i32), cl: f32) -> f32 {
    seg_dist_m_cl(a, b, cl)
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points. A public
/// wrapper over [`seg_dist_m`] so the app measures actually-ridden distance with the very
/// metric the route format stored. Prefer [`ground_dist_m_cl`] in a loop that can hoist
/// `cos_lat`.
pub fn ground_dist_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    seg_dist_m(a, b)
}
