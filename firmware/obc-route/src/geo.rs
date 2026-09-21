//! Route-specific projections over the shared local-equirectangular metric.

use obc_map_scene::{delta_m, BBox, M_PER_DEG};

/// Project point `p` onto the segment `a` to `b`, clamped to the segment's ends, in the
/// local-equirectangular metric for a band with precomputed `cl = cos_lat`. Returns `(t, dist_m)`:
/// `t` in `0.0..=1.0` is the fractional position of the nearest point along the segment, and
/// `dist_m` the cross-track distance from `p` to it. All three points are `(lon, lat)`
/// microdegrees.
pub(crate) fn project_to_segment(a: (i32, i32), b: (i32, i32), p: (i32, i32), cl: f32) -> (f32, f32) {
    let (bx, by) = delta_m(a, b, cl);
    let (px, py) = delta_m(a, p, cl);
    let len2 = bx * bx + by * by;
    if len2 <= 1e-9 {
        // Zero-length segment: the distance to the shared endpoint.
        return (0.0, libm::sqrtf(px * px + py * py));
    }
    let t = ((px * bx + py * by) / len2).clamp(0.0, 1.0);
    let (dx, dy) = (px - bx * t, py - by * t);
    (t, libm::sqrtf(dx * dx + dy * dy))
}

/// Effective area (m²) of the triangle `a, b, c`: the Visvalingam-Whyatt significance of vertex
/// `b`, which is the area the line loses if `b` is dropped and `a` joins straight to `c`. A
/// fixed-budget simplifier drops the smallest-area vertex, so a straight run yields its points
/// before a bend does. The metric self-spreads, because removing a vertex widens its neighbours'
/// triangles and protects them next time.
pub fn tri_area_m2_cl(a: (i32, i32), b: (i32, i32), c: (i32, i32), cl: f32) -> f32 {
    let (ux, uy) = delta_m(a, b, cl);
    let (vx, vy) = delta_m(a, c, cl);
    0.5 * (ux * vy - uy * vx).abs()
}

/// Union bbox of `pts`, inflated by `pad_m` metres converted to microdegrees at band `cl`. It is
/// the cheap reject in front of a per-segment proximity test. An empty `pts` yields the all-zero
/// bbox, which no real coordinate hits.
///
/// The `.max(0.05)` floor on `cl` keeps the longitude padding finite near the poles. The `+ 1` on
/// each pad is truncation slack, so the inflated box never falls inside `pad_m`.
pub(crate) fn inflated_bbox(pts: impl IntoIterator<Item = (i32, i32)>, cl: f32, pad_m: f32) -> BBox {
    let mut bbox = BBox { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN };
    let mut any = false;
    for (lon, lat) in pts {
        any = true;
        bbox.min_lon = bbox.min_lon.min(lon);
        bbox.max_lon = bbox.max_lon.max(lon);
        bbox.min_lat = bbox.min_lat.min(lat);
        bbox.max_lat = bbox.max_lat.max(lat);
    }
    if !any {
        return BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 };
    }
    let m_per_udeg_lat = M_PER_DEG as f32 * 1e-6;
    let pad_lat = (pad_m / m_per_udeg_lat) as i32 + 1;
    let pad_lon = (pad_m / (m_per_udeg_lat * cl.max(0.05))) as i32 + 1;
    bbox.min_lon -= pad_lon;
    bbox.max_lon += pad_lon;
    bbox.min_lat -= pad_lat;
    bbox.max_lat += pad_lat;
    bbox
}
