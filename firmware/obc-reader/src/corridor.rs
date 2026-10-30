//! The route-corridor POI query's neutral parts: the category filter set, the remaining-route
//! geometry seam, the projected result record, and the point-to-segment projection math.
//! [`Reader::corridor_pois`](crate::Reader::corridor_pois) drives them against the per-category
//! POI quadtrees.
//!
//! It answers a different question from [`nearest_pois`](crate::Reader::nearest_pois): not what
//! is near me now, but what is coming up on my route, so a result carries an along-route distance
//! and a signed lateral offset instead of a plain ground distance.
//!
//! The route arrives through a trait because the OBCR format lives in `obc-route`, which sits
//! above this crate. [`RoutePath`] is the same seam shape the map overlay uses: chunked
//! `(lon, lat)` microdegree polylines with a per-chunk bbox and cumulative distance, streamed
//! through a callback so nothing is copied.

use crate::reader::Poi;
use obc_formats::obcm::PoiCategory;
#[cfg(test)]
use obc_map_scene::ground_dist_m_cl;
use obc_map_scene::{cos_lat, delta_m, BBox, M_PER_DEG};

/// Lateral half-width of the route corridor, in ground metres: a POI farther than this from the
/// route line is somewhere else, not up ahead. The one knob that trades list noise against missed
/// water.
pub const CORRIDOR_HALF_WIDTH_M: u16 = 300;

/// Max results one corridor snapshot returns. The query fills the caller's `Vec` ascending by
/// [`dist_along_m`](CorridorPoi::dist_along_m) and never exceeds it: it is a list a rider reads,
/// not a dataset.
pub const MAX_CORRIDOR_RESULTS: usize = 16;

/// A bitset over the service categories, the corridor query's filter. Bit `i-1` carries category
/// id `i`, so the set is two bytes and cheap to key a frozen snapshot on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoiCategorySet(u16);

impl PoiCategorySet {
    /// No categories, so the query returns nothing.
    pub const EMPTY: PoiCategorySet = PoiCategorySet(0);
    /// Every category.
    pub const ALL: PoiCategorySet = {
        let mut set = Self::EMPTY;
        let mut i = 0;
        while i < PoiCategory::ALL.len() {
            set = set.with(PoiCategory::ALL[i]);
            i += 1;
        }
        set
    };

    /// The single-category set.
    #[inline]
    pub const fn only(cat: PoiCategory) -> PoiCategorySet {
        PoiCategorySet(1 << (cat.id() - 1))
    }

    /// This set plus `cat`.
    #[inline]
    pub const fn with(self, cat: PoiCategory) -> PoiCategorySet {
        PoiCategorySet(self.0 | (1 << (cat.id() - 1)))
    }

    /// Whether `cat` is in the set.
    #[inline]
    pub const fn contains(self, cat: PoiCategory) -> bool {
        self.0 & (1 << (cat.id() - 1)) != 0
    }

    /// No category selected.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Number of categories selected.
    #[inline]
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// The raw bits, the stable key a frozen snapshot compares on.
    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// The selected categories in canonical id order.
    #[inline]
    pub fn iter(self) -> impl Iterator<Item = PoiCategory> {
        PoiCategory::ALL.into_iter().filter(move |c| self.contains(*c))
    }
}

/// The remaining-route geometry the corridor query projects POIs onto, implemented by `obc-route`
/// on its `RouteReader`. Chunks are in route order, which the query relies on to walk forwards and
/// stop once its slots are filled by nearer entries.
///
/// Every method must be cheap off the resident chunk index except `visit_chunk_points`, which may
/// touch the card. Each query step reads at most one route chunk.
pub trait RoutePath {
    /// Identity of immutable geometry. Paths without a generation bypass encounter caching.
    fn geometry_generation(&self) -> Option<u32> {
        None
    }

    /// Number of chunks, in route order.
    fn chunk_count(&self) -> usize;

    /// Cumulative along-route distance in metres at chunk `k`'s first point, non-decreasing in
    /// `k`. An out-of-range `k` returns the route total.
    fn chunk_start_m(&self, k: usize) -> u32;

    /// Chunk `k`'s bounding box in microdegrees; an out-of-range `k` returns a degenerate box.
    fn chunk_bbox(&self, k: usize) -> BBox;

    /// Decode chunk `k` and hand its `(lon, lat)` microdegree points to `visit` once, in route
    /// order. A chunk that fails to decode does not call `visit`. The slice is borrowed from the
    /// implementor's own scratch and must not outlive the call.
    #[allow(clippy::type_complexity)]
    fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)]));
}

/// One corridor result: a map POI placed on the route axis rather than at a radius from the fix.
///
/// `poi.distance_m` carries the along-route distance still to go, the number the Up-ahead row
/// shows, so a row can go to the POI detail screen as a plain [`Poi`]. `poi.lat` and `lon` stay
/// the POI's real coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorridorPoi {
    /// The POI record, with `distance_m` the along-route distance to go.
    pub poi: Poi,
    /// Where the POI projects onto the route, in metres from the route start, on the axis stored
    /// waypoints use. Always at or past the query's progress anchor.
    pub dist_along_m: u32,
    /// Signed lateral distance from the route line in metres: positive is right of the direction
    /// of travel. The magnitude never exceeds `CORRIDOR_HALF_WIDTH_M`.
    pub offset_m: i32,
}

/// A point's projection onto one route chunk. Floats: the query rounds once, at the end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PathProjection {
    /// Along-route distance of the projection, meters from the route start.
    pub dist_along_m: f32,
    /// Signed lateral distance in metres; positive is right of the direction of travel.
    pub offset_m: f32,
}

/// Project `p` (`(lon, lat)` µdeg) onto segment `a→b` with the latitude scale `cl`: the distance
/// from `a` to the clamped perpendicular foot, plus a signed lateral offset. A point past either
/// end measures to that endpoint.
///
/// Sign convention: with east as +x and north as +y, the cross product `dx·ey − dy·ex` is positive
/// when the point lies left of the direction of travel, and the stored `offset_m` is its negation,
/// so positive means right.
pub(crate) fn project_onto_segment(a: (i32, i32), b: (i32, i32), p: (i32, i32), cl: f32) -> PathProjection {
    let (dx, dy) = delta_m(a, b, cl);
    let (ex, ey) = delta_m(a, p, cl);
    let len2 = dx * dx + dy * dy;
    // A zero-length segment has no direction: measure straight to the vertex and leave the side
    // unsigned rather than invent one from a zero cross product.
    if len2 <= 1e-6 {
        return PathProjection { dist_along_m: 0.0, offset_m: libm::sqrtf(ex * ex + ey * ey) };
    }
    let t = ((ex * dx + ey * dy) / len2).clamp(0.0, 1.0);
    let (px, py) = (ex - t * dx, ey - t * dy);
    let d = libm::sqrtf(px * px + py * py);
    let cross = dx * ey - dy * ex;
    PathProjection { dist_along_m: t * libm::sqrtf(len2), offset_m: if cross > 0.0 { -d } else { d } }
}

/// Whether `p` lies inside segment `a→b`'s µdeg bbox grown by `(lon_pad, lat_pad)`: the cheap
/// integer reject in front of the projection math. Saturating, so a huge pad cannot wrap.
#[inline]
pub(crate) fn within_pad(a: (i32, i32), b: (i32, i32), p: (i32, i32), lon_pad: i32, lat_pad: i32) -> bool {
    let (lo_lon, hi_lon) = if a.0 <= b.0 { (a.0, b.0) } else { (b.0, a.0) };
    let (lo_lat, hi_lat) = if a.1 <= b.1 { (a.1, b.1) } else { (b.1, a.1) };
    p.0 >= lo_lon.saturating_sub(lon_pad)
        && p.0 <= hi_lon.saturating_add(lon_pad)
        && p.1 >= lo_lat.saturating_sub(lat_pad)
        && p.1 <= hi_lat.saturating_add(lat_pad)
}

/// Longitude and latitude pads in µdeg that span at least `pad_m` ground metres anywhere in `bbox`
/// grown by them: the cosine is taken at the grown box's poleward edge, and both pads round up.
pub(crate) fn pads(bbox: BBox, pad_m: f32) -> (i32, i32) {
    let lat_pad = (libm::ceilf(pad_m / (M_PER_DEG as f32 * 1e-6)) as i32).saturating_add(1);
    let edge = bbox.min_lat.unsigned_abs().max(bbox.max_lat.unsigned_abs()).saturating_add(lat_pad as u32);
    let cl = cos_lat(edge.min(90_000_000) as i32).max(1e-6);
    let lon_pad = (libm::ceilf(lat_pad as f32 / cl) as i32).saturating_add(1);
    (lon_pad, lat_pad)
}

/// Grow `bbox` by at least `pad_m` ground metres on all four sides. Every step saturates.
pub(crate) fn inflate_bbox(bbox: BBox, pad_m: f32) -> BBox {
    let (lon_pad, lat_pad) = pads(bbox, pad_m);
    BBox {
        min_lon: bbox.min_lon.saturating_sub(lon_pad),
        min_lat: bbox.min_lat.saturating_sub(lat_pad),
        max_lon: bbox.max_lon.saturating_add(lon_pad),
        max_lat: bbox.max_lat.saturating_add(lat_pad),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 43° N: 1000 µdeg of longitude is about 81 m, 1000 µdeg of latitude about 111 m.
    const LAT: i32 = 43_000_000;

    /// The foot lands at its distance along the segment, left of travel is negative, and a point
    /// past the end measures to the endpoint.
    #[test]
    fn segment_projection_is_signed_and_clamped() {
        let (a, b) = ((7_000_000, LAT), (7_010_000, LAT));
        let cl = cos_lat(LAT);
        let len = ground_dist_m_cl(a, b, cl);
        let mid = project_onto_segment(a, b, (7_005_000, LAT + 1_000), cl);
        assert!((mid.dist_along_m - len / 2.0).abs() < 0.5);
        assert!((mid.offset_m + 111.2).abs() < 0.5, "≈111 m north of an eastbound segment is left");
        let past = project_onto_segment(a, b, (7_020_000, LAT), cl);
        assert!((past.dist_along_m - len).abs() < 0.5);
        assert!((past.offset_m - len).abs() < 0.5, "measured to the endpoint");
    }

    /// The category set round-trips its members and "Everything" holds all six.
    #[test]
    fn category_set_membership() {
        assert_eq!(PoiCategorySet::ALL.len(), 10);
        assert!(PoiCategory::ALL.iter().all(|c| PoiCategorySet::ALL.contains(*c)));
        assert!(PoiCategorySet::EMPTY.is_empty());
        let only = PoiCategorySet::only(PoiCategory::Water);
        assert!(only.contains(PoiCategory::Water));
        assert!(!only.contains(PoiCategory::BikeShop));
        assert_eq!(only.iter().collect::<heapless::Vec<_, 6>>().as_slice(), &[PoiCategory::Water]);
        let two = only.with(PoiCategory::Pharmacy);
        assert_eq!(two.len(), 2);
        // Iteration is canonical id order regardless of insertion order.
        assert_eq!(
            two.iter().collect::<heapless::Vec<_, 6>>().as_slice(),
            &[PoiCategory::Water, PoiCategory::Pharmacy]
        );
    }

    /// Inflating a box grows latitude by the requested metres and longitude by more, because the
    /// same ground distance spans more µdeg of longitude away from the equator.
    #[test]
    fn inflate_pads_both_axes_in_ground_meters() {
        let b = BBox { min_lon: 7_000_000, min_lat: LAT, max_lon: 7_010_000, max_lat: LAT + 10_000 };
        let g = inflate_bbox(b, CORRIDOR_HALF_WIDTH_M as f32);
        let lat_pad = b.min_lat - g.min_lat;
        let lon_pad = b.min_lon - g.min_lon;
        assert!((lat_pad as f32 * 1e-6 * M_PER_DEG as f32 - 300.0).abs() < 1.0);
        assert!(lon_pad > lat_pad, "at 43° N a metre of easting is more µdeg than a metre of northing");
    }
}
