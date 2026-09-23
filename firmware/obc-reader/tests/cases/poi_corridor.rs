//! Query-contract tests for the route-corridor POI scan (`Reader::corridor_pois`).
//!
//! Each test builds a synthetic map whose POI section is a real per-category quadtree and drives
//! the query against a hand-built [`RoutePath`] with the same seam-sharing and cumulative-distance
//! convention OBCR uses, so the projections are the ones a real route produces. `obc-reader` sits
//! below `obc-route`, so the route side is a fixture; the end-to-end pin lives in `obc-route`.

use std::cell::Cell;

use obc_map_scene::{cos_lat, ground_dist_m_cl, BBox};
use obc_reader::{
    CorridorPoi, MapCache, MapTables, PoiCategory, PoiCategorySet, Reader, RoutePath, SliceSource, MAX_CORRIDOR_RESULTS,
};
use obcm_testkit::{build_poi_map, PoiSpec};

use crate::common::CountingSource;

/// The fixture map bbox, the 1°×1° square the other POI suites use.
const BBOX: (i32, i32, i32, i32) = (7_000_000, 43_000_000, 8_000_000, 44_000_000);
/// The latitude the fixture routes run along. At 43.5° N one µdeg of latitude is about 0.111 m and
/// one µdeg of longitude about 0.081 m, so the offsets below are easy to state in metres.
const LAT: i32 = 43_500_000;
/// POI chunk size the fixtures pack at.
const CS: usize = 512;

/// A hand-built [`RoutePath`]: chunks of `(lon, lat)` µdeg with their cumulative distances
/// precomputed the way OBCR does it, taking segment lengths at each chunk's first-point `cos_lat`
/// and repeating chunk `k`'s last point as chunk `k+1`'s first, so distances stitch without a gap.
struct FixturePath {
    chunks: Vec<Vec<(i32, i32)>>,
    starts: Vec<u32>,
    total_m: u32,
    /// How many times the query asked for a chunk's points: the did-it-stop-early counter.
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

    /// A straight eastbound route at [`LAT`], `segs` segments of `step` µdeg each, split into
    /// seam-shared chunks.
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

/// The name of each result, in order.
fn names(got: &[CorridorPoi]) -> Vec<&str> {
    got.iter().map(|c| c.poi.name.as_str()).collect()
}

/// A named Water POI (subtype 1) at `(lon, lat)`.
fn water(name: &str, lon: i32, lat: i32) -> PoiSpec {
    PoiSpec { lat, lon, subtype: 1, name: name.into(), payload: 0xFFFF }
}

/// The offset's sign is the side of travel: eastbound, a POI north of the line is on the rider's
/// left and one south of it on the right, with the magnitude the perpendicular ground distance.
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

/// Reversing the direction of travel flips the side, because the sign is about travel and not
/// about north.
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

/// The along-route distance is the projection onto the route axis, the same axis stored waypoints
/// and live progress use, so a POI beside the 4th segment reports that segment's distance.
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

/// A POI outside the 300 m half-width is somewhere else, not up ahead, and the query drops it.
/// The boundary is checked from both sides.
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

/// Only what is ahead qualifies: a POI the rider has passed is dropped even though it sits in the
/// corridor. The boundary is inclusive.
#[test]
fn pois_behind_progress_are_rejected() {
    let pois = vec![water("Passed", 7_110_000, LAT + 500), water("Ahead", 7_190_000, LAT + 500)];
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);

    assert_eq!(names(&query(&bytes, PoiCategorySet::ALL, &path, 0)), ["Passed", "Ahead"]);
    // Ride past the first one (it sits ≈808 m along).
    let got = query(&bytes, PoiCategorySet::ALL, &path, 2_000);
    assert_eq!(names(&got), ["Ahead"], "the passed POI is gone, the one ahead stays");
    // Anchored exactly on the survivor's projection it is still ahead.
    let at = got[0].dist_along_m;
    assert_eq!(names(&query(&bytes, PoiCategorySet::ALL, &path, at)), ["Ahead"]);
    assert_eq!(query(&bytes, PoiCategorySet::ALL, &path, at + 1).len(), 0);
}

/// A hairpin leaves the POI radius between its two legs, creating two distinct passes.
#[test]
fn switchback_retains_distinct_route_encounters() {
    // Out east, up 2000 µdeg, back west: a hairpin whose two legs are inside each other's
    // corridor, chunked so the two legs are scanned separately.
    let pts = vec![
        (7_100_000, LAT),
        (7_150_000, LAT),
        (7_150_000, LAT + 2_000),
        (7_100_000, LAT + 2_000),
        (7_050_000, LAT + 2_000),
    ];
    let path = FixturePath::new(chunked(&pts, 2));
    assert!(path.chunk_count() >= 2, "the legs must fall in different chunks for this to bite");
    // Above the outbound leg and below the return leg: inside both corridors, nearer the outbound.
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Crook spring", 7_120_000, LAT + 600)])]);

    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(got.len(), 2, "the two route encounters have distinct along-route positions");
    assert_eq!(got[0].poi.metadata.source, got[1].poi.metadata.source);
    assert!(got[0].dist_along_m < got[1].dist_along_m);
    assert_eq!(got[0].offset_m, -67, "the nearest projection wins (the outbound leg, on the left)");
    let leg = ground_dist_m_cl(pts[0], pts[1], cos_lat(LAT)) as u32;
    assert!(got[0].dist_along_m < leg, "and it is placed on the outbound leg, not the return");
}

/// A later encounter preserves its own geometry without replacing the earlier occurrence.
#[test]
fn later_closer_encounter_preserves_the_earlier_encounter() {
    // The return leg passes much closer to the POI than the outbound leg does.
    let pts = vec![
        (7_100_000, LAT),
        (7_150_000, LAT),
        (7_150_000, LAT + 2_500),
        (7_100_000, LAT + 2_500),
        (7_050_000, LAT + 2_500),
    ];
    let path = FixturePath::new(chunked(&pts, 2));
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Near the return", 7_120_000, LAT + 2_300)])]);

    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(got.len(), 2);
    assert_eq!(got[1].offset_m, -22, "the return leg has its own approach distance");
    let leg = ground_dist_m_cl(pts[0], pts[1], cos_lat(LAT)) as u32;
    assert!(got[1].dist_along_m > leg, "the second encounter lies on the return leg");
}

/// The category filter scopes the walk: everything returns both categories interleaved in route
/// order, a single-category filter only its own, and the empty set nothing.
#[test]
fn the_category_filter_scopes_the_result() {
    let water_pois = vec![water("W1", 7_110_000, LAT + 500), water("W2", 7_170_000, LAT + 500)];
    let shops = vec![
        PoiSpec { lat: LAT - 500, lon: 7_140_000, subtype: 18, name: "S1".into(), payload: 0xFFFF },
        PoiSpec { lat: LAT - 500, lon: 7_190_000, subtype: 18, name: "S2".into(), payload: 0xFFFF },
    ];
    let bytes = build_poi_map(BBOX, CS, &[(1, water_pois), (6, shops)]);
    let path = FixturePath::straight(7_100_000, 10_000, 10, 4);

    assert_eq!(names(&query(&bytes, PoiCategorySet::ALL, &path, 0)), ["W1", "S1", "W2", "S2"]);
    let only_water = PoiCategorySet::only(PoiCategory::Water);
    assert_eq!(names(&query(&bytes, only_water, &path, 0)), ["W1", "W2"]);
    let two = only_water.with(PoiCategory::BikeShop);
    assert_eq!(names(&query(&bytes, two, &path, 0)).len(), 4);
    assert!(query(&bytes, PoiCategorySet::EMPTY, &path, 0).is_empty(), "no categories ⇒ no rows");
    // A category the map does not carry is a valid empty answer, not an error.
    assert!(query(&bytes, PoiCategorySet::only(PoiCategory::Pharmacy), &path, 0).is_empty());
}

/// The cap is 16 and the order is ascending along-route distance: a route with 40 POIs beside it
/// returns the first 16, in route order.
#[test]
fn cap_and_ordering_are_pinned() {
    // 40 water points along an 80-segment route.
    let pois: Vec<PoiSpec> = (0..40).map(|i| water(&format!("P{i:02}"), 7_102_000 + 5_000 * i, LAT + 500)).collect();
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 5_000, 80, 8);

    let got = query(&bytes, PoiCategorySet::ALL, &path, 0);
    assert_eq!(got.len(), MAX_CORRIDOR_RESULTS, "capped at 16 per snapshot");
    assert_eq!(names(&got), (0..16).map(|i| format!("P{i:02}")).collect::<Vec<_>>(), "the first 16, in route order");
    assert!(got.windows(2).all(|w| w[0].dist_along_m <= w[1].dist_along_m), "ascending along-route");

    // Riding on re-anchors the window, so the same query returns the next 16.
    let later = query(&bytes, PoiCategorySet::ALL, &path, 4_000);
    assert_eq!(later.len(), MAX_CORRIDOR_RESULTS);
    assert!(later[0].dist_along_m >= 4_000, "only what is still ahead");
    assert_ne!(names(&later)[0], "P00");
}

/// A route with no POIs beside it, an empty map, and a zero-chunk route are all valid empty
/// answers, never an error.
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

/// The cost pin: a POI-dense fixture with a long remaining route must not pay for the whole route.
/// The walk stops once the 16 slots are filled by nearer entries, so the visited-chunk count and
/// the SD reads stay bounded by the prefix that produced the answer.
#[test]
fn dense_route_bounds_each_step_and_the_first_page_reads() {
    // 120 water POIs over a 240-segment route; the first 16 are all inside the first 7 km.
    let pois: Vec<PoiSpec> = (0..120).map(|i| water(&format!("P{i:03}"), 7_101_000 + 2_000 * i, LAT + 400)).collect();
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = FixturePath::straight(7_100_000, 1_000, 240, 8); // 30 chunks

    let src = CountingSource::new(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let parse_reads = src.reads.get();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = heapless::Vec::<CorridorPoi, MAX_CORRIDOR_RESULTS>::new();
    use obc_reader::reader::places::{PlaceQuery, PlaceWindow, QueryProgress};
    let mut query = PlaceQuery::new(
        0,
        PoiCategorySet::ALL,
        PlaceWindow::Corridor { from_m: 0, to_m: u32::MAX, half_width_m: 300 },
        None,
    );
    loop {
        let before_map = src.reads.get();
        let before_route = path.visits.get();
        let state = query.step(&r, Some(&path), 0, &mut out);
        assert!(src.reads.get() - before_map <= 1, "one map index or POI chunk per step");
        assert!(path.visits.get() - before_route <= 3, "one route chunk and its two seam neighbours per step");
        if state != QueryProgress::Pending {
            assert!(matches!(state, QueryProgress::Ready { more: true, .. }));
            break;
        }
    }
    let query_reads = src.reads.get() - parse_reads;
    assert_eq!(out.len(), MAX_CORRIDOR_RESULTS);
    assert!(query_reads <= 200, "first-page source read budget: {query_reads}");
}

/// A missing route chunk cannot establish a complete corridor result.
#[test]
fn an_undecodable_chunk_fails_the_query() {
    struct Holey(FixturePath, usize);
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
            if k == self.1 {
                return; // pretend chunk 0's geometry read failed
            }
            self.0.visit_chunk_points(k, visit);
        }
    }
    let pois = vec![water("In chunk 0", 7_110_000, LAT + 500), water("In chunk 2", 7_190_000, LAT + 500)];
    let bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let path = Holey(FixturePath::straight(7_100_000, 10_000, 10, 4), 0);

    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = heapless::Vec::<CorridorPoi, MAX_CORRIDOR_RESULTS>::new();
    assert!(r.corridor_pois(PoiCategorySet::ALL, &path, 0, &mut out).is_err());
    assert!(out.is_empty(), "a missing route chunk is not a complete empty corridor");

    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Across the hole", 7_105_000, LAT + 500)])]);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let path = Holey(FixturePath::straight(7_100_000, 1_000, 10, 1), 4);
    assert!(reader.corridor_pois(PoiCategorySet::ALL, &path, 0, &mut out).is_err());
    assert!(out.is_empty(), "a pass cannot publish its nearest point before its unreadable continuation");
}

/// Small bends remain one pass even when the nearest point and the exit are many chunks apart.
/// Returning after leaving the radius creates another pass with its own signed offset and key.
#[test]
fn meanders_across_chunks_keep_one_nearest_encounter_per_pass_and_stable_pages() {
    use obc_reader::reader::places::{PlaceQuery, PlaceWindow, QueryProgress};
    let outward: Vec<_> = (0..=400)
        .map(|i| {
            (
                7_100_000 + i * 100,
                LAT + if !(180..=220).contains(&i) {
                    0
                } else if i % 2 == 0 {
                    120
                } else {
                    -120
                },
            )
        })
        .collect();
    let mut points = outward.clone();
    points.extend(outward.iter().rev().skip(1).copied());
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Meander water", 7_120_000, LAT + 1_620)])]);
    for per_chunk in [1, 2, 7, 64, 1000] {
        let path = FixturePath::new(chunked(&points, per_chunk));
        let hits = query(&bytes, PoiCategorySet::ALL, &path, 0);
        assert_eq!(hits.len(), 2, "one place, two passes; {per_chunk} segments per chunk");
        assert_eq!(hits[0].offset_m, -167);
        assert_eq!(hits[1].offset_m, 167);
        assert!(hits[0].dist_along_m < hits[1].dist_along_m);
        assert_eq!(hits[0].poi.metadata.source, hits[1].poi.metadata.source);

        let source = CountingSource::new(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut q = PlaceQuery::new(
            3,
            PoiCategorySet::ALL,
            PlaceWindow::Corridor { from_m: 0, to_m: u32::MAX, half_width_m: 300 },
            None,
        );
        let mut page = heapless::Vec::<CorridorPoi, 1>::new();
        let finish = |q: &mut PlaceQuery, page: &mut heapless::Vec<CorridorPoi, 1>| {
            for _ in 0..100_000 {
                let before_route = path.visits.get();
                let before_map = source.reads.get();
                let state = q.step(&reader, Some(&path), 3, page);
                assert!(path.visits.get() - before_route <= 1, "one route chunk per step");
                assert!(source.reads.get() - before_map <= 1, "one POI/index chunk per step");
                if state != QueryProgress::Pending {
                    assert!(matches!(state, QueryProgress::Ready { .. }));
                    return;
                }
            }
            panic!("query did not settle");
        };
        finish(&mut q, &mut page);
        assert_eq!(page[0], hits[0]);
        let first = q.key(&page[0]);
        q.next_page(first);
        page.clear();
        finish(&mut q, &mut page);
        assert_eq!(page[0], hits[1]);
        let second = q.key(&page[0]);
        q.previous_page(second);
        page.clear();
        finish(&mut q, &mut page);
        assert_eq!(q.key(&page[0]), first);

        // An anchor inside the first pass must include its nearest point once, then stop showing
        // that pass after the rider has passed it.
        let before = query(&bytes, PoiCategorySet::ALL, &path, hits[0].dist_along_m - 5);
        assert_eq!(before.len(), 2);
        assert_eq!(before[0].dist_along_m, hits[0].dist_along_m);
        let after = query(&bytes, PoiCategorySet::ALL, &path, hits[0].dist_along_m + 1);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].dist_along_m, hits[1].dist_along_m);
    }
}

#[test]
fn a_tie_inside_one_pass_keeps_the_first_segment_side() {
    let points = [(7_120_000 - 100, LAT), (7_120_000 + 100, LAT), (7_120_000 - 100, LAT)];
    let bytes = build_poi_map(BBOX, CS, &[(1, vec![water("Equal approach", 7_120_000, LAT + 500)])]);
    for per_chunk in [1, 2] {
        let path = FixturePath::new(chunked(&points, per_chunk));
        let hits = query(&bytes, PoiCategorySet::ALL, &path, 0);
        assert_eq!(hits.len(), 1, "turning within the radius does not leave the pass");
        assert_eq!(hits[0].offset_m, -56, "the eastbound first segment wins the equal-distance tie");
    }
}

/// The corridor test runs before the hours read, so a closed place must still neither fill a page
/// nor mark one as having more. A query that starts past a key returns the same page as paging.
#[test]
fn closed_places_stay_off_pages_and_a_started_page_matches_paging() {
    use obc_reader::reader::places::{PlaceKey, PlaceQuery, PlaceWindow, QueryProgress, PLACE_PAGE_SIZE};
    let mut open = [0; 29];
    for day in 0..7 {
        open[2 + day * 4] = 96;
    }
    // Two closed places before the eight open ones and four after them.
    let pois = (0..14)
        .map(|i| PoiSpec {
            payload: if (2..10).contains(&i) { 1 } else { 0 },
            ..water(&format!("W{i:02}"), 7_101_000 + 1_500 * i, LAT + 400)
        })
        .collect();
    let bytes = obcm_testkit::build_poi_map_with_hours(BBOX, CS, &[(1, pois)], &[[0; 29], open]);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let path = FixturePath::straight(7_100_000, 1_000, 240, 8);
    let window = PlaceWindow::Corridor { from_m: 0, to_m: u32::MAX, half_width_m: 300 };
    let new = || PlaceQuery::new(1, PoiCategorySet::ALL, window, Some((0, 600)));
    fn settle<const N: usize>(
        query: &mut PlaceQuery,
        reader: &Reader,
        path: &FixturePath,
        page: &mut heapless::Vec<CorridorPoi, N>,
    ) -> QueryProgress {
        page.clear();
        loop {
            let state = query.step(reader, Some(path), 1, page);
            if state != QueryProgress::Pending {
                return state;
            }
        }
    }

    let mut page = heapless::Vec::<CorridorPoi, PLACE_PAGE_SIZE>::new();
    let state = settle(&mut new(), &reader, &path, &mut page);
    assert!(matches!(state, QueryProgress::Ready { more: false, .. }), "{state:?}");
    let expected: Vec<String> = (2..10).map(|i| format!("W{i:02}")).collect();
    assert_eq!(names(&page), expected);

    let key = |p: &CorridorPoi| PlaceKey {
        distance_m: p.poi.distance_m,
        source: p.poi.metadata.source,
        occurrence: p.dist_along_m,
    };
    let mut small = heapless::Vec::<CorridorPoi, 3>::new();
    let mut paged = new();
    settle(&mut paged, &reader, &path, &mut small);
    assert_eq!(names(&small), expected[..3]);
    paged.next_page(key(&small[2]));
    let paged_state = settle(&mut paged, &reader, &path, &mut small);
    assert_eq!(names(&small), expected[3..6]);

    let mut started = new().starting_after(key(&page[2]), false);
    assert_eq!(settle(&mut started, &reader, &path, &mut small), paged_state);
    assert_eq!(names(&small), expected[3..6]);
    let mut back = new().starting_after(key(&page[3]), true);
    settle(&mut back, &reader, &path, &mut small);
    assert_eq!(names(&small), expected[..3]);
}
