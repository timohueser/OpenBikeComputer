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
