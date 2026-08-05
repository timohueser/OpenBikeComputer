//! The **route-corridor** POI query's neutral parts (epic #946, U2): the category filter set, the
//! remaining-route geometry seam, the projected result record, and the point-to-polyline projection
//! math. [`Reader::corridor_pois`](crate::Reader::corridor_pois) drives them against the
//! per-category POI quadtrees (spec §7.2/§7.4).
//!
//! The query answers a different question from [`nearest_pois`](crate::Reader::nearest_pois): not
//! "what is near me *now*" (a radius around the fix) but "what is coming up *on my route*" — so a
//! result carries an along-route distance and a **signed** lateral offset instead of a plain ground
//! distance.
//!
//! # Why the route arrives through a trait
//!
//! The OBCR route format lives in `obc-route`, which sits **above** this crate (obc-reader →
//! obc-route → obc-render → obc-app), so the reader cannot name a `RouteReader`. [`RoutePath`] is
//! the same seam shape `obc-render`'s `RouteOverlaySource` uses for the map overlay: chunked
//! `(lon, lat)` microdegree polylines with a per-chunk bbox and cumulative along-route distance,
//! streamed through a callback so nothing is copied or allocated. `obc-route` implements it on
//! `RouteReader`, over the **resident chunk index** the breadcrumb/progress machinery already reads
//! — no full-route re-read per snapshot.

use crate::reader::Poi;
use obc_formats::obcm::{PoiCategory, POI_CATEGORY_COUNT};
#[cfg(test)]
use obc_map_scene::ground_dist_m_cl;
use obc_map_scene::{cos_lat, delta_m, BBox, M_PER_DEG};

/// Lateral half-width of the route corridor, in ground meters (epic #946, locked): a POI farther
/// than this from the route line is not "up ahead on my route", it's somewhere else. **Tunable** —
/// the epic wants it sim-validated on real routes; it is the one knob that trades list noise
/// against missed water.
pub(crate) const CORRIDOR_HALF_WIDTH_M: f32 = 300.0;

/// Max results one corridor snapshot returns (locked on epic #946). The caller owns a
/// `heapless::Vec<CorridorPoi, MAX_CORRIDOR_RESULTS>`; the query fills it ascending by
/// [`dist_along_m`](CorridorPoi::dist_along_m) and never exceeds it. 16 × ≈44 B ≈ 700 B — the same
/// order as the nearest-16 snapshot, and the same reason: it is a list a rider reads, not a dataset.
pub const MAX_CORRIDOR_RESULTS: usize = 16;

/// A bitset over the six §7.4 POI categories — the corridor query's filter. "Everything" is
/// [`ALL`](PoiCategorySet::ALL); the Up-ahead screen's category picker (U3) passes
/// [`only`](PoiCategorySet::only). Bit `i-1` carries category id `i` (`1..=6`), so the set is one
/// byte and cheap to key a frozen snapshot on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PoiCategorySet(u8);

impl PoiCategorySet {
    /// No categories — the query returns nothing.
    pub const EMPTY: PoiCategorySet = PoiCategorySet(0);
    /// Every §7.4 category ("Everything").
    pub const ALL: PoiCategorySet = PoiCategorySet(((1u16 << POI_CATEGORY_COUNT) - 1) as u8);

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

    /// The raw bits — the stable key a frozen snapshot compares on.
    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// The selected categories in canonical id order.
    #[inline]
    pub fn iter(self) -> impl Iterator<Item = PoiCategory> {
        PoiCategory::ALL.into_iter().filter(move |c| self.contains(*c))
    }
}

/// The remaining-route geometry the corridor query projects POIs onto — implemented by `obc-route`
/// on its `RouteReader` (see the module docs for why this is a trait). Chunks are in **route
/// order**; the query relies on that to walk the route forwards and stop early once its 16 slots
/// are filled by nearer entries.
///
/// Every method must be cheap off the resident chunk index except [`visit_chunk_points`], which is
/// the one that may touch the card — the query calls it at most once per chunk it actually needs.
///
/// [`visit_chunk_points`]: RoutePath::visit_chunk_points
pub trait RoutePath {
    /// Number of chunks, in route order.
    fn chunk_count(&self) -> usize;

    /// Cumulative along-route distance (meters) at chunk `k`'s **first** point. Non-decreasing in
    /// `k`. Out-of-range `k` returns the route total (the "past the end" answer).
    fn chunk_start_m(&self, k: usize) -> u32;

    /// Chunk `k`'s bounding box in microdegrees. An out-of-range `k` returns an empty/degenerate box.
    fn chunk_bbox(&self, k: usize) -> BBox;

    /// Decode chunk `k` and hand its `(lon, lat)` microdegree points to `visit` **once**, in route
    /// order. A chunk that fails to decode simply doesn't call `visit` (the corridor loses that
    /// stretch, exactly like the map overlay loses a stroke). The slice is borrowed from the
    /// implementor's own scratch and must not outlive the call.
    // The `&mut dyn FnMut(&[…])` spelling *is* the seam (object-safe, alloc-free); a type alias
    // would only hide what implementors must write anyway — same call as `RouteOverlaySource`.
    #[allow(clippy::type_complexity)]
    fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)]));
}

/// One corridor result: a map POI placed on the **route axis** rather than at a radius from the fix.
///
/// `poi.distance_m` carries the along-route **distance still to go** (`dist_along_m − progress`) —
/// the number the Up-ahead row shows — so a row can be handed to the POI detail screen as a plain
/// [`Poi`] without a second query. `poi.lat`/`lon` stay the POI's real coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorridorPoi {
    /// The POI record, with `distance_m` = the along-route distance to go (see the struct docs).
    pub poi: Poi,
    /// Where the POI projects onto the route, on the same axis stored waypoints use (meters from
    /// the route start). Always `>=` the query's progress anchor.
    pub dist_along_m: u32,
    /// Signed lateral distance from the route line, meters: **positive = right** of the direction
    /// of travel, negative = left. U3 renders `←`/`→` from the sign once the magnitude passes its
    /// threshold. Magnitude never exceeds [`CORRIDOR_HALF_WIDTH_M`].
    pub offset_m: i32,
}

/// A point's projection onto one route chunk: where it lands on the route axis and how far to the
/// side it sits. Floats — the query rounds once, at the end, into [`CorridorPoi`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PathProjection {
    /// Along-route distance of the projection, meters from the route start.
    pub dist_along_m: f32,
    /// Signed lateral distance, meters; positive = right of the direction of travel.
    pub offset_m: f32,
}

/// Project `p` (`(lon, lat)` µdeg) onto one chunk's polyline and return its **nearest** projection:
/// the perpendicular foot on whichever segment it sits closest to, as an along-route distance plus a
/// signed lateral offset. `None` for a chunk with fewer than two points (nothing to project onto).
///
/// The along-route axis is accumulated from `chunk_start_m` with the **same** local-equirectangular
/// segment lengths (`delta_m` at the chunk's first-point `cos_lat`) `obc-route`'s own
/// `locate_progress` / waypoint placement use — so a corridor POI and a stored waypoint sitting at
/// the same spot report the same distance, and "distance to go" against live progress is consistent.
///
/// **Sign convention** (epic #946, amended 2026-07-28): with east as +x and north as +y, the 2-D
/// cross product `dx·ey − dy·ex` of the segment direction with the point offset is positive when the
/// point lies **left** of the direction of travel. The stored `offset_m` is its negation, so
/// positive means **right** — the direction U3's `→` points.
///
/// Taking the nearest segment (not the first one within range) is what makes a switchback yield one
/// entry: a POI in the crook of a hairpin projects onto both legs, and the closer leg wins.
///
/// `limit_m` bounds the answer: `None` when no segment comes within it (the corridor reject —
/// `f32::INFINITY` for an unconditional projection). It is not just a post-filter — it also prunes
/// the walk, which is what keeps the cost sane: a chunk carries up to 256 points and every candidate
/// POI is projected against it, so each segment first takes a **four-integer** µdeg-bbox test and
/// only the survivors pay for the dot/cross/`sqrt`.
pub(crate) fn project_onto_chunk(
    pts: &[(i32, i32)],
    chunk_start_m: u32,
    p: (i32, i32),
    limit_m: f32,
) -> Option<PathProjection> {
    if pts.len() < 2 {
        return None;
    }
    // One `cos_lat` for the whole chunk, taken at its first point — the convention `obc-route`'s
    // own along-route walks use, so the two agree meter for meter.
    let cl = cos_lat(pts[0].1).max(1e-3);
    // The prune window in µdeg, from `limit_m`. Saturated to `i32::MAX` for an infinite limit, so
    // the bbox test then always passes.
    let lat_pad = udeg_pad(limit_m);
    let lon_pad = udeg_pad(limit_m / cl);
    let mut s = chunk_start_m as f32;
    let mut best: Option<(f32, PathProjection)> = None; // (|offset|, projection)
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = delta_m(a, b, cl);
        // The segment length still has to be accumulated even for a pruned segment — it is what
        // carries the along-route axis forward — but nothing else does.
        let len = libm::sqrtf(dx * dx + dy * dy);
        if !within_pad(a, b, p, lon_pad, lat_pad) {
            s += len;
            continue;
        }
        let (ex, ey) = delta_m(a, p, cl);
        let len2 = dx * dx + dy * dy;
        // A degenerate (zero-length) segment has no direction: measure straight to the vertex and
        // leave the side unsigned rather than invent one from a zero cross product.
        let (t, offset) = if len2 <= 1e-6 {
            (0.0, libm::sqrtf(ex * ex + ey * ey))
        } else {
            let t = ((ex * dx + ey * dy) / len2).clamp(0.0, 1.0);
            // Distance to the **clamped** foot, so a POI past a segment's end measures to the
            // endpoint (the neighbouring segment then usually wins anyway).
            let (px, py) = (ex - t * dx, ey - t * dy);
            let d = libm::sqrtf(px * px + py * py);
            // `cross > 0` ⇒ left of travel; the stored sign is positive-is-right (see the docs).
            let cross = dx * ey - dy * ex;
            (t, if cross > 0.0 { -d } else { d })
        };
        let abs = libm::fabsf(offset);
        if abs <= limit_m && best.is_none_or(|(prev, _)| abs < prev) {
            best = Some((abs, PathProjection { dist_along_m: s + t * len, offset_m: offset }));
        }
        s += len;
    }
    best.map(|(_, proj)| proj)
}

/// `m` ground meters as a latitude-µdeg pad, saturating (an infinite/huge limit pads to `i32::MAX`,
/// so a bbox test against it always passes).
#[inline]
fn udeg_pad(m: f32) -> i32 {
    let ud = m / (M_PER_DEG as f32 * 1e-6);
    if ud >= i32::MAX as f32 {
        i32::MAX
    } else {
        ud as i32
    }
}

/// Whether `p` lies inside segment `a→b`'s µdeg bbox grown by `(lon_pad, lat_pad)` — the cheap
/// integer reject in front of the projection math. Saturating, so a huge pad can't wrap.
#[inline]
fn within_pad(a: (i32, i32), b: (i32, i32), p: (i32, i32), lon_pad: i32, lat_pad: i32) -> bool {
    let (lo_lon, hi_lon) = if a.0 <= b.0 { (a.0, b.0) } else { (b.0, a.0) };
    let (lo_lat, hi_lat) = if a.1 <= b.1 { (a.1, b.1) } else { (b.1, a.1) };
    p.0 >= lo_lon.saturating_sub(lon_pad)
        && p.0 <= hi_lon.saturating_add(lon_pad)
        && p.1 >= lo_lat.saturating_sub(lat_pad)
        && p.1 <= hi_lat.saturating_add(lat_pad)
}

/// Grow `bbox` by `pad_m` ground meters on all four sides — the corridor's search window around one
/// route chunk. The longitude pad is scaled by `1/cos_lat` so both axes span the same ground
/// distance; every step saturates so a corrupt/extreme box can't wrap `i32`.
pub(crate) fn inflate_bbox(bbox: BBox, pad_m: f32) -> BBox {
    // Meters → latitude µdeg, then longitude µdeg via the box's mid-latitude cos.
    let lat_pad = (pad_m / (M_PER_DEG as f32 * 1e-6)) as i32;
    let mid_lat = bbox.min_lat.saturating_add(bbox.max_lat) / 2;
    let cl = cos_lat(mid_lat).max(1e-3);
    let lon_pad = ((lat_pad as f32 / cl) as i32).max(1);
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

    /// A due-east chunk at 43° N: 1000 µdeg of longitude ≈ 81 m of ground, 1000 µdeg of latitude
    /// ≈ 111 m. Used to place POIs at known offsets.
    const LAT: i32 = 43_000_000;

    fn chunk() -> [(i32, i32); 3] {
        // (lon, lat) — three points marching east along a constant latitude.
        [(7_000_000, LAT), (7_010_000, LAT), (7_020_000, LAT)]
    }

    /// Travelling east, a POI **north** of the line is on the rider's left ⇒ negative offset; a POI
    /// **south** of it is on the right ⇒ positive. The magnitudes match the equirectangular metric.
    #[test]
    fn sign_is_positive_to_the_right_of_travel() {
        let pts = chunk();
        let north = project_onto_chunk(&pts, 0, (7_005_000, LAT + 1_000), f32::INFINITY).unwrap();
        let south = project_onto_chunk(&pts, 0, (7_005_000, LAT - 1_000), f32::INFINITY).unwrap();
        assert!(north.offset_m < 0.0, "north of an eastbound line is on the left (negative)");
        assert!(south.offset_m > 0.0, "south of an eastbound line is on the right (positive)");
        assert!((north.offset_m + south.offset_m).abs() < 0.01, "mirrored offsets, opposite signs");
        assert!((south.offset_m - 111.32).abs() < 0.5, "1000 µdeg of latitude ≈ 111 m");
    }

    /// The along-route distance is the chunk's cumulative start plus the walked segment length, so a
    /// projection halfway along the second segment reads the sum of both.
    #[test]
    fn along_distance_accumulates_from_the_chunk_start() {
        let pts = chunk();
        let mid = project_onto_chunk(&pts, 500, (7_015_000, LAT), f32::INFINITY).unwrap();
        // Segment one is a full 10 000 µdeg of longitude; the projection sits halfway along two.
        let seg = ground_dist_m_cl(pts[0], pts[1], cos_lat(LAT));
        assert!((mid.dist_along_m - (500.0 + seg * 1.5)).abs() < 0.5, "cum start + 1.5 segments");
        assert!(mid.offset_m.abs() < 0.01, "a point on the line has no offset");
    }

    /// A hairpin projects a POI in its crook onto **both** legs; the nearest one wins, so the
    /// projection is single-valued (the dedupe the query relies on).
    #[test]
    fn switchback_keeps_the_nearest_leg() {
        // East along `LAT`, then back west along `LAT + 2000` (≈222 m apart).
        let pts = [(7_000_000, LAT), (7_010_000, LAT), (7_010_000, LAT + 2_000), (7_000_000, LAT + 2_000)];
        // A POI just north of the outbound leg: 200 µdeg (~22 m) up, i.e. far closer to leg 1.
        let p = project_onto_chunk(&pts, 0, (7_005_000, LAT + 200), f32::INFINITY).unwrap();
        assert!(p.offset_m.abs() < 25.0, "the near leg's offset, not the far leg's ~200 m");
        assert!(p.offset_m < 0.0, "north of the eastbound leg ⇒ left");
        let leg = ground_dist_m_cl(pts[0], pts[1], cos_lat(LAT));
        assert!(p.dist_along_m < leg, "it lands on the first leg, not the return");
    }

    /// A single-point (or empty) chunk has no segment to project onto.
    #[test]
    fn degenerate_chunk_has_no_projection() {
        assert!(project_onto_chunk(&[], 0, (7_000_000, LAT), f32::INFINITY).is_none());
        assert!(project_onto_chunk(&[(7_000_000, LAT)], 0, (7_000_000, LAT), f32::INFINITY).is_none());
    }

    /// `limit_m` both rejects and prunes, and the pruning must not change the answer: inside the
    /// limit the projection is identical to the unbounded one; outside it there is no answer at all.
    #[test]
    fn the_limit_rejects_without_changing_what_it_keeps() {
        let pts = chunk();
        let p = (7_005_000, LAT + 1_000); // ≈111 m north of the line
        let unbounded = project_onto_chunk(&pts, 0, p, f32::INFINITY).unwrap();
        let bounded = project_onto_chunk(&pts, 0, p, 300.0).unwrap();
        assert_eq!(bounded, unbounded, "the prune window doesn't perturb an in-limit projection");
        assert!(project_onto_chunk(&pts, 0, p, 100.0).is_none(), "111 m is outside a 100 m limit");
        // A POI far off the *end* of the polyline is measured to the endpoint and rejected too.
        assert!(project_onto_chunk(&pts, 0, (7_100_000, LAT), 300.0).is_none());
    }

    /// The category set round-trips its members and "Everything" holds all six.
    #[test]
    fn category_set_membership() {
        assert_eq!(PoiCategorySet::ALL.len(), 6);
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

    /// Inflating a box grows latitude by the requested meters and longitude by more (the same
    /// ground distance spans more µdeg of longitude away from the equator).
    #[test]
    fn inflate_pads_both_axes_in_ground_meters() {
        let b = BBox { min_lon: 7_000_000, min_lat: LAT, max_lon: 7_010_000, max_lat: LAT + 10_000 };
        let g = inflate_bbox(b, CORRIDOR_HALF_WIDTH_M);
        let lat_pad = b.min_lat - g.min_lat;
        let lon_pad = b.min_lon - g.min_lon;
        assert!((lat_pad as f32 * 1e-6 * M_PER_DEG as f32 - 300.0).abs() < 1.0);
        assert!(lon_pad > lat_pad, "at 43° N a metre of easting is more µdeg than a metre of northing");
    }
}
