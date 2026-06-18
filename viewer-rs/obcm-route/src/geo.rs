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

/// Meters per degree of latitude (and of longitude at the equator).
pub(crate) const M_PER_DEG: f64 = 111_320.0;

/// `cos(latitude)` for the local east-scaling, from latitude in microdegrees.
pub(crate) fn cos_lat(lat_ud: i32) -> f64 {
    libm::cos(lat_ud as f64 * 1e-6 * core::f64::consts::PI / 180.0)
}

/// East/north offset in meters from `from` to `to` (each `(lon, lat)` microdegrees),
/// given a precomputed `cl = cos_lat` for the band.
pub(crate) fn delta_m(from: (i32, i32), to: (i32, i32), cl: f64) -> (f64, f64) {
    let dlon = (to.0 - from.0) as f64 * 1e-6;
    let dlat = (to.1 - from.1) as f64 * 1e-6;
    (dlon * M_PER_DEG * cl, dlat * M_PER_DEG)
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points.
pub(crate) fn seg_dist_m(a: (i32, i32), b: (i32, i32)) -> f64 {
    let (mx, my) = delta_m(a, b, cos_lat(a.1));
    libm::sqrt(mx * mx + my * my)
}

/// Project point `p` onto the segment `a → b`, **clamped** to the segment's ends.
/// Returns `(t, dist_m)`: `t ∈ 0.0..=1.0` is the fractional position of the nearest
/// point along the segment, and `dist_m` the cross-track distance from `p` to it. All
/// three are `(lon, lat)` microdegrees, measured in the same local-equirectangular metric
/// as [`delta_m`] (exact enough over a route's short segments).
///
/// This is the clamped sibling of the converter's perpendicular-distance test (which
/// measures to the *infinite* chord for decimation); the [route matcher](crate::matcher)
/// needs the clamped on-segment distance, but both share this one projection so they
/// can't drift.
pub(crate) fn project_to_segment(a: (i32, i32), b: (i32, i32), p: (i32, i32)) -> (f64, f64) {
    let cl = cos_lat(a.1);
    let (bx, by) = delta_m(a, b, cl);
    let (px, py) = delta_m(a, p, cl);
    let len2 = bx * bx + by * by;
    if len2 <= 1e-9 {
        // Degenerate (zero-length) segment: distance to the shared endpoint.
        return (0.0, libm::sqrt(px * px + py * py));
    }
    let t = ((px * bx + py * by) / len2).clamp(0.0, 1.0);
    let (dx, dy) = (px - bx * t, py - by * t);
    (t, libm::sqrt(dx * dx + dy * dy))
}

/// Straight-line ground distance (m) between two `(lon, lat)` microdegree points, as
/// `f32` for the app's ride accumulators. A public wrapper over [`seg_dist_m`] so the
/// app measures actually-ridden distance with the very metric the route format stored.
pub fn ground_dist_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    seg_dist_m(a, b) as f32
}
