//! Route-side local-equirectangular geometry, on top of the shared distance core.
//!
//! The core — microdegrees → ground meters ([`ground_dist_m`], [`cos_lat`], the private
//! `delta_m`/`seg_dist_m*` helpers — lives in [`obc_reader::geo`], the bottom of the shared
//! stack, so the renderer's chevron walk measures segments with the *same* metric the
//! [converter](crate::convert) stored in [`ChunkMeta::cum_distance_m`](crate::ChunkMeta) and
//! the [elevation profile](crate::profile) buckets by. This module re-exports that core for
//! the route crate's callers and keeps the two *derived* projections only the route path
//! needs: the matcher's clamped on-segment projection and the decimators'
//! Visvalingam–Whyatt triangle area.

pub use obc_reader::geo::{cos_lat, ground_dist_m, ground_dist_m_cl};
pub(crate) use obc_reader::geo::{delta_m, seg_dist_m, seg_dist_m_cl};

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
/// yields its points before a bend does; the metric self-spreads, since removing a vertex widens
/// its neighbours' triangles and protects them next time.
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
