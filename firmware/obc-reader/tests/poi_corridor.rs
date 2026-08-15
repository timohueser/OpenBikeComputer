//! Query-contract tests for the route-corridor POI scan (`Reader::corridor_pois`, epic #946 U2).
//!
//! Each test builds a synthetic map whose POI section is a real per-category quadtree (via
//! `obcm-testkit`'s `build_poi_map`, which mirrors the packer's tree build) and drives the query
//! against a hand-built [`RoutePath`] — a chunked polyline with the same seam-sharing and
//! cumulative-distance convention OBCR uses, so the projections here are the ones a real route
//! produces. `obc-reader` sits below `obc-route`, so the route side is a fixture, not a `RouteReader`
//! (the end-to-end pin over a real `.obcr` lives in `obc-route`'s `corridor.rs`).
//!
//! What is pinned: the **signed** offset (positive = right of travel), the switchback single-entry
//! dedupe, the corridor and behind-progress rejects, the 16-cap + ascending order, and the SD-read
//! cost — including that a POI-dense route stops the walk early instead of paying for its length.

use std::cell::Cell;

use obc_map_scene::{cos_lat, ground_dist_m_cl, BBox};
use obc_reader::{
    CorridorPoi, MapCache, MapTables, PoiCategory, PoiCategorySet, Reader, RoutePath, SliceSource, MAX_CORRIDOR_RESULTS,
};
use obcm_testkit::{build_poi_map, PoiSpec};

mod common;
use common::CountingSource;

/// The fixture map bbox `(min_lon, min_lat, max_lon, max_lat)` — the 1°×1° square the other POI
/// suites use.
const BBOX: (i32, i32, i32, i32) = (7_000_000, 43_000_000, 8_000_000, 44_000_000);
/// The latitude the fixture routes run along. At 43.5° N one µdeg of latitude is ≈0.111 m and one
/// µdeg of longitude ≈0.081 m, so the offsets below are easy to state in meters.
const LAT: i32 = 43_500_000;
/// POI chunk size the fixtures pack at (the packer's §7.1 default).
const CS: usize = 512;

// ============================== the route fixture ==============================

/// A hand-built [`RoutePath`]: chunks of `(lon, lat)` µdeg with their cumulative along-route
/// distances precomputed exactly the way OBCR does it — segment lengths from
/// [`ground_dist_m_cl`] at each chunk's **first-point** `cos_lat`, and chunk `k`'s last point
/// repeated as chunk `k+1`'s first (seam sharing), so distances stitch without a gap.
struct FixturePath {
    chunks: Vec<Vec<(i32, i32)>>,
    starts: Vec<u32>,
    total_m: u32,
    /// How many times the query asked for a chunk's points — the "did it stop early?" counter.
    visits: Cell<u32>,
}

impl FixturePath {
    fn new(chunks: Vec<Vec<(i32, i32)>>) -> FixturePath {
        let mut starts = Vec::with_capacity(chunks.len());
        let mut acc = 0.0f32;
        for c in &chunks {
            starts.push(acc as u32);
            acc += chunk_len_m(c);
        }
        FixturePath { chunks, starts, total_m: acc as u32, visits: Cell::new(0) }
    }

    /// A straight eastbound route at [`LAT`] from `lon0`, `segs` segments of `step` µdeg each,
    /// split into chunks of `per_chunk` segments (seam-shared, like OBCR).
    fn straight(lon0: i32, step: i32, segs: usize, per_chunk: usize) -> FixturePath {
        let pts: Vec<(i32, i32)> = (0..=segs).map(|i| (lon0 + step * i as i32, LAT)).collect();
        FixturePath::new(chunked(&pts, per_chunk))
    }
}

/// Split a polyline into seam-shared chunks of `per_chunk` segments each.
fn chunked(pts: &[(i32, i32)], per_chunk: usize) -> Vec<Vec<(i32, i32)>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < pts.len() {
        let end = (i + per_chunk).min(pts.len() - 1);
        out.push(pts[i..=end].to_vec());
        i = end;
    }
    out
}

/// The along-route length of one chunk, in the reader's (and OBCR's) metric.
fn chunk_len_m(c: &[(i32, i32)]) -> f32 {
    let cl = cos_lat(c[0].1);
    c.windows(2).map(|w| ground_dist_m_cl(w[0], w[1], cl)).sum()
}

impl RoutePath for FixturePath {
    fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    fn chunk_start_m(&self, k: usize) -> u32 {
        self.starts.get(k).copied().unwrap_or(self.total_m)
    }
    fn chunk_bbox(&self, k: usize) -> BBox {
        let Some(c) = self.chunks.get(k) else {
            return BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 };
        };
        BBox {
            min_lon: c.iter().map(|p| p.0).min().unwrap(),
            min_lat: c.iter().map(|p| p.1).min().unwrap(),
            max_lon: c.iter().map(|p| p.0).max().unwrap(),
            max_lat: c.iter().map(|p| p.1).max().unwrap(),
        }
    }
    fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        self.visits.set(self.visits.get() + 1);
        if let Some(c) = self.chunks.get(k) {
            visit(c);
        }
    }
}

// ============================== harness ==============================

/// Run the corridor query over a built map + route and return the results.
fn query(bytes: &[u8], cats: PoiCategorySet, path: &FixturePath, progress_m: u32) -> Vec<CorridorPoi> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = heapless::Vec::<CorridorPoi, MAX_CORRIDOR_RESULTS>::new();
    r.corridor_pois(cats, path, progress_m, &mut out).unwrap();
    out.into_iter().collect()
}

/// The name of each result, in order — the readable assertion for membership + ordering.
fn names(got: &[CorridorPoi]) -> Vec<&str> {
    got.iter().map(|c| c.poi.name.as_str()).collect()
}

/// A named Water POI (subtype 1) at `(lon, lat)`.
fn water(name: &str, lon: i32, lat: i32) -> PoiSpec {
    PoiSpec { lat, lon, subtype: 1, name: name.into(), hours_ref: 0xFFFF }
}

// ============================== tests ==============================

/// The offset's **sign** is the side of travel: eastbound, a POI north of the line is on the
/// rider's left (negative) and one south of it on the right (positive), with the magnitude the
/// perpendicular ground distance. This is what U3 renders `←` / `→` from.
#[test]
fn offset_sign_is_positive_to_the_right_of_travel() {
    // 1000 µdeg of latitude ≈ 111 m.
    let pois = vec![
        water("Left spring", 7_120_000, LAT + 1_000), // north of an eastbound line ⇒ left
        water("Right spring", 7_140_000, LAT - 1_000), // south ⇒ right
    ];
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);

    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(names(&got), ["Left spring", "Right spring"], "route-ordered");
    assert!(got[0].offset_m < 0, "north of an eastbound route is on the LEFT ⇒ negative");
    assert!(got[1].offset_m > 0, "south of an eastbound route is on the RIGHT ⇒ positive");
    assert_eq!(got[0].offset_m, -111, "1000 µdeg of latitude ≈ 111 m");
    assert_eq!(got[1].offset_m, 111);
}

/// Reversing the direction of travel flips the side: the identical POI reads left on an eastbound
/// route and right on a westbound one. (The sign is about *travel*, not about north.)
#[test]
fn reversing_the_route_flips_the_side() {
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Spring", 7_150_000, LAT + 1_000)])]);
    let east = FixturePath::straight(7_100_000, 10_000, 10, 4);
    let west = FixturePath::new(chunked(&(0..=10).map(|i| (7_200_000 - 10_000 * i, LAT)).collect::<Vec<_>>(), 4));

    let e = query(&bytes, PoiCategorySet::ALL, &east, 0);
    let w = query(&bytes, PoiCategorySet::ALL, &west, 0);
    assert_eq!(e.len(), 1);
    assert_eq!(w.len(), 1);
    assert!(e[0].offset_m < 0 && w[0].offset_m > 0, "the same POI is left eastbound, right westbound");
    assert_eq!(e[0].offset_m, -w[0].offset_m, "same magnitude, opposite sign");
}

/// The along-route distance is the projection onto the route axis — the same axis stored waypoints
/// and live progress use — so a POI beside the 4th segment reports that segment's distance, not its
/// straight-line range from anywhere.
#[test]
fn dist_along_projects_onto_the_route_axis() {
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Spring", 7_140_000, LAT + 500)])]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);
    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(got.len(), 1);
    // 40 000 µdeg of longitude east of the start, at the fixture's own metric.
    let want = ground_dist_m_cl((7_100_000, LAT), (7_140_000, LAT), cos_lat(LAT)) as u32;
    assert!(got[0].dist_along_m.abs_diff(want) <= 1, "got {} want {}", got[0].dist_along_m, want);
    // `poi.distance_m` carries the along-route distance still to go from the progress anchor.
    let got_ahead = query(&bytes, PoiCategorySet::ALL, &path, 1_000);
    assert_eq!(got_ahead[0].poi.distance_m, got_ahead[0].dist_along_m - 1_000);
}

/// A POI outside the 300 m half-width is not "up ahead on my route" — it's somewhere else, and the
/// query drops it. The boundary is checked from both sides.
#[test]
fn off_corridor_pois_are_rejected() {
    let pois = vec![
        water("Just inside", 7_120_000, LAT + 2_600),  // ≈289 m north
        water("Just outside", 7_140_000, LAT + 3_000), // ≈334 m north
        water("Far off", 7_160_000, LAT + 20_000),     // ≈2.2 km north
    ];
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);
    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(names(&got), ["Just inside"], "only the POI inside the corridor survives");
    assert!(got[0].offset_m.abs() <= 300);
}

/// Only what is **ahead** qualifies: a POI the rider has already passed is dropped, even though it
/// sits squarely in the corridor. The boundary is inclusive (a POI exactly at progress is ahead).
#[test]
fn pois_behind_progress_are_rejected() {
    let pois = vec![water("Passed", 7_110_000, LAT + 500), water("Ahead", 7_190_000, LAT + 500)];
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);

    assert_eq!(names(&query(&bytes, PoiCategorySet::ALL, &path, 0)), ["Passed", "Ahead"]);
    // Ride past the first one (it sits ≈808 m along).
    let got = query(&bytes, PoiCategorySet::ALL, &path, 2_000);
    assert_eq!(names(&got), ["Ahead"], "the passed POI is gone, the one ahead stays");
    // Anchored exactly on the survivor's projection it is still ahead (inclusive boundary).
    let at = got[0].dist_along_m;
    assert_eq!(names(&query(&bytes, PoiCategorySet::ALL, &path, at)), ["Ahead"]);
    assert_eq!(query(&bytes, PoiCategorySet::ALL, &path, at + 1).len(), 0);
}

/// A hairpin projects a POI in its crook onto **two** legs — and, because the legs land in
/// different route chunks, onto two separate scans. It must appear **once**, at its nearest
/// projection (the near leg), not twice.
#[test]
fn switchback_double_projection_yields_one_entry() {
    // Out east along LAT, up 2000 µdeg (≈222 m), back west — a hairpin whose two legs are inside
    // each other's corridor. Chunked at 2 segments so the two legs are scanned separately.
    let pts = vec![
        (7_100_000, LAT),
        (7_150_000, LAT),
        (7_150_000, LAT + 2_000),
        (7_100_000, LAT + 2_000),
        (7_050_000, LAT + 2_000),
    ];
    let path = FixturePath::new(chunked(&pts, 2));
    assert!(path.chunk_count() >= 2, "the legs must fall in different chunks for this to bite");
    // 600 µdeg (≈67 m) above the outbound leg, i.e. ≈155 m below the return leg — inside both
    // corridors, nearer the outbound one.
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Crook spring", 7_120_000, LAT + 600)])]);

    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(got.len(), 1, "one POI, one row — a switchback must not double it");
    assert_eq!(got[0].offset_m, -67, "the nearest projection wins (the outbound leg, on the left)");
    let leg = ground_dist_m_cl(pts[0], pts[1], cos_lat(LAT)) as u32;
    assert!(got[0].dist_along_m < leg, "and it is placed on the outbound leg, not the return");
}

/// When the *nearer* projection is the one found later, the held entry is replaced (and re-sorted),
/// so the dedupe is by nearest projection rather than by first sighting.
#[test]
fn dedupe_keeps_the_nearest_projection_even_when_found_later() {
    // Return leg passes much closer to the POI than the outbound leg does.
    let pts = vec![
        (7_100_000, LAT),
        (7_150_000, LAT),
        (7_150_000, LAT + 2_500),
        (7_100_000, LAT + 2_500),
        (7_050_000, LAT + 2_500),
    ];
    let path = FixturePath::new(chunked(&pts, 2));
    // 2300 µdeg up: ≈256 m from the outbound leg, ≈22 m from the return leg.
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Near the return", 7_120_000, LAT + 2_300)])]);

    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].offset_m, -22, "the return leg's projection replaced the outbound one");
    let leg = ground_dist_m_cl(pts[0], pts[1], cos_lat(LAT)) as u32;
    assert!(got[0].dist_along_m > leg, "and the entry moved to the return leg's along-distance");
}

/// The category filter scopes the walk: "Everything" returns both categories interleaved in route
/// order, a single-category filter returns only its own, and the empty set returns nothing.
#[test]
fn the_category_filter_scopes_the_result() {
    let water_pois = vec![water("W1", 7_110_000, LAT + 500), water("W2", 7_170_000, LAT + 500)];
    let shops = vec![
        PoiSpec { lat: LAT - 500, lon: 7_140_000, subtype: 18, name: "S1".into(), hours_ref: 0xFFFF },
        PoiSpec { lat: LAT - 500, lon: 7_190_000, subtype: 18, name: "S2".into(), hours_ref: 0xFFFF },
    ];
    let bytes = build_poi_map(BBOX, CS, &[(1, water_pois), (6, shops)]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);

    assert_eq!(names(&query(&bytes, PoiCategorySet::ALL, &path, 0)), ["W1", "S1", "W2", "S2"]);
    let only_water = PoiCategorySet::only(PoiCategory::Water);
    assert_eq!(names(&query(&bytes, only_water, &path, 0)), ["W1", "W2"]);
    let two = only_water.with(PoiCategory::BikeShop);
    assert_eq!(names(&query(&bytes, two, &path, 0)).len(), 4);
    assert!(query(&bytes, PoiCategorySet::EMPTY, &path, 0).is_empty(), "no categories ⇒ no rows");
    // A category the map doesn't carry is a valid empty answer, not an error.
    assert!(query(&bytes, PoiCategorySet::only(PoiCategory::Pharmacy), &path, 0).is_empty());
}

/// The cap is 16 and the order is ascending along-route distance: a route with 40 POIs beside it
/// returns the **first** 16, in route order, and nothing farther.
#[test]
fn cap_and_ordering_are_pinned() {
    // 40 water points, one every 5000 µdeg (≈404 m) along an 80-segment route.
    let pois: Vec<PoiSpec> = (0..40).map(|i| water(&format!("P{i:02}"), 7_102_000 + 5_000 * i, LAT + 500)).collect();
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 5_000, 80, 8);

    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(got.len(), MAX_CORRIDOR_RESULTS, "capped at 16 per snapshot");
    assert_eq!(names(&got), (0..16).map(|i| format!("P{i:02}")).collect::<Vec<_>>(), "the first 16, in route order");
    assert!(got.windows(2).all(|w| w[0].dist_along_m <= w[1].dist_along_m), "ascending along-route");

    // Riding on re-anchors the window: the same query from 4 km in returns the *next* 16.
    let later = query(&bytes, PoiCategorySet::ALL, &path, 4_000);
    assert_eq!(later.len(), MAX_CORRIDOR_RESULTS);
    assert!(later[0].dist_along_m >= 4_000, "only what is still ahead");
    assert_ne!(names(&later)[0], "P00");
}

/// A route with no POIs beside it, an empty map, and a zero-chunk route are all valid empty answers
/// — never an error, never a panic.
#[test]
fn empty_answers_are_not_errors() {
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Far away", 7_900_000, 43_900_000)])]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);
    assert!(query(&bytes, PoiCategorySet::ALL, &path, 0).is_empty(), "no POI near this route");

    let empty_route = FixturePath::new(Vec::new());
    assert!(query(&bytes, PoiCategorySet::ALL, &empty_route, 0).is_empty(), "a route with no chunks");

    // Progress past the end of the route leaves nothing ahead.
    assert!(query(&bytes, PoiCategorySet::ALL, &path, 1_000_000).is_empty());
}

/// **The cost pin** (the epic's acceptance measurement, deterministic half). A POI-dense fixture with
/// a long remaining route must not pay for the whole route: the walk stops once the 16 slots are
/// filled by nearer entries, so both the visited-chunk count and the SD reads stay bounded by the
/// prefix that produced the answer — not by the route length.
#[test]
fn dense_route_stops_early_and_bounds_its_reads() {
    // 120 water POIs over a 240-segment (~19 km) route: the first 16 are all inside the first ~7 km.
    let pois: Vec<PoiSpec> = (0..120).map(|i| water(&format!("P{i:03}"), 7_101_000 + 2_000 * i, LAT + 400)).collect();
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 1_000, 240, 8); // 30 chunks

    let src = CountingSource::new(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let parse_reads = src.reads.get();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = heapless::Vec::<CorridorPoi, MAX_CORRIDOR_RESULTS>::new();
    r.corridor_pois(PoiCategorySet::ALL, &path, 0, &mut out).unwrap();
    let query_reads = src.reads.get() - parse_reads;

    assert_eq!(out.len(), MAX_CORRIDOR_RESULTS);
    assert!(
        path.visits.get() < path.chunk_count() as u32,
        "the walk stopped early: visited {} of {} chunks",
        path.visits.get(),
        path.chunk_count()
    );
    // The pin is a ceiling, not an equality — a reader-internal cache change may move it, but a
    // regression that walks the whole route (or drops the index-block coalescing) blows past it.
    assert!(query_reads <= 200, "worst-case snapshot cost regressed: {query_reads} source reads");
    // With no early exit this route would visit all 30 chunks × 1 category; assert we did far less.
    assert!(path.visits.get() <= 12, "visited {} chunks", path.visits.get());
}

/// A chunk that fails to decode (the [`RoutePath`] contract's "just don't call `visit`") loses that
/// stretch of corridor but never fails the query — the same posture the map overlay takes.
#[test]
fn an_undecodable_chunk_is_skipped_not_fatal() {
    struct Holey(FixturePath);
    impl RoutePath for Holey {
        fn chunk_count(&self) -> usize {
            self.0.chunk_count()
        }
        fn chunk_start_m(&self, k: usize) -> u32 {
            self.0.chunk_start_m(k)
        }
        fn chunk_bbox(&self, k: usize) -> BBox {
            self.0.chunk_bbox(k)
        }
        fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
            if k == 0 {
                return; // pretend chunk 0's geometry read failed
            }
            self.0.visit_chunk_points(k, visit);
        }
    }
    let pois = vec![water("In chunk 0", 7_110_000, LAT + 500), water("In chunk 2", 7_190_000, LAT + 500)];
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = Holey(FixturePath::straight(7_100_000, 10_000, 10, 4));

    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = heapless::Vec::<CorridorPoi, MAX_CORRIDOR_RESULTS>::new();
    r.corridor_pois(PoiCategorySet::ALL, &path, 0, &mut out).expect("a bad chunk is not a query error");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].poi.name.as_str(), "In chunk 2");
}
