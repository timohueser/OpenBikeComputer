//! Host tests for the detour pipeline (#882): the corridor blacklist, the corridor-aware A*
//! (`plan_detour`), and the splice (`splice_detour`) — fixture graphs serialized with the real
//! `obc-pack` writer and parsed with the real `obc-reader` (the `tests/nav.rs` pattern), original
//! routes built through the public GPX converter, so every byte crosses genuine on-wire formats.
//!
//! The workhorse fixture is a **blocked road with a parallel relief street**: a straight
//! west→east road (the route), a parallel street ~400 m north, and connector edges at both ends.
//! The corridor over the skipped span must force the plan onto the street; a grade-separated
//! bridge chord crossing mid-span and the at-grade junction edges must stay usable.

mod common;

use common::{convert, VecSink};
use obc_elevation::NullElevation;
use obc_formats::io::SliceSource;
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::corridor::{Corridor, CORRIDOR_MAX_PTS, MIN_DETOUR_SPAN_M};
use obc_route::nav::{plan_detour, plan_route, NavError, NavScratch};
use obc_route::reader::for_each_waypoint;
use obc_route::splice::{splice_detour, trim_detour_to_tail};
use obc_route::{RouteIndex, RoutePoint, RouteReader, TrimOutcome};

/// Global bbox (µdeg) — roomy so the node quadtree genuinely subdivides (see tests/nav.rs).
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 1_000_000, 1_000_000);
/// Fixture origin (lon, lat) µdeg = (0.5°, 0.5°); cos_lat ≈ 1 so lon µdeg ≈ lat µdeg ≈ 0.111 m.
const BASE: (i32, i32) = (500_000, 500_000);
/// Road/street node spacing, µdeg (~278.3 m ground).
const SP: i32 = 2_500;
/// Number of road segments (13 nodes, road length ~3 340 m).
const SEGS: i32 = 12;
/// The street's northward offset, µdeg (~400 m — outside the corridor, below the bridge scale).
const STREET_OFF: i32 = 3_600;
/// Stored edge length for one road/street segment (≥ its ~278.3 m chord: admissible).
const SEG_COST: u32 = 280;
/// Stored connector length (≥ its ~400.8 m chord).
const CONN_COST: u32 = 401;

fn neutral_profile() -> NavProfile {
    NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }
}

/// Serialize `graph` into a minimal v9 map with one neutral profile (tests/nav.rs's `map_with`).
fn map_with(graph: &NavGraph) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) =
        serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, &[neutral_profile()], &mut NullElevation);
    assert_eq!(dropped, 0);
    bin
}

/// Road node i's coordinate.
fn road_at(i: i32) -> (i32, i32) {
    (BASE.0 + i * SP, BASE.1)
}

/// Street node i's coordinate.
fn street_at(i: i32) -> (i32, i32) {
    (BASE.0 + i * SP, BASE.1 + STREET_OFF)
}

/// The blocked-road fixture: road nodes (ids 0..=SEGS), street nodes + end connectors, and
/// optionally a grade-separated bridge chord crossing the road mid-span (endpoints ~111 m to
/// either side — sharing no node with the road). Node ids are dense (the packer asserts it):
/// street ids follow the road's, bridge ids follow whatever came before.
fn road_graph(street: bool, bridge: bool) -> NavGraph {
    let road = |i: i32| i as u32;
    let street_id = |i: i32| (SEGS + 1 + i) as u32;
    let bridge_base = if street { 2 * (SEGS as u32 + 1) } else { SEGS as u32 + 1 };

    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for i in 0..=SEGS {
        nodes.push(Node { id: road(i), coord: road_at(i) });
        if i < SEGS {
            edges.push(Edge {
                a: road(i),
                b: road(i + 1),
                polyline: vec![road_at(i), road_at(i + 1)],
                length_m: SEG_COST,
                kind: 0,
            });
        }
    }
    if street {
        for i in 0..=SEGS {
            nodes.push(Node { id: street_id(i), coord: street_at(i) });
            if i < SEGS {
                edges.push(Edge {
                    a: street_id(i),
                    b: street_id(i + 1),
                    polyline: vec![street_at(i), street_at(i + 1)],
                    length_m: SEG_COST,
                    kind: 0,
                });
            }
        }
        edges.push(Edge {
            a: road(0),
            b: street_id(0),
            polyline: vec![road_at(0), street_at(0)],
            length_m: CONN_COST,
            kind: 0,
        });
        edges.push(Edge {
            a: road(SEGS),
            b: street_id(SEGS),
            polyline: vec![road_at(SEGS), street_at(SEGS)],
            length_m: CONN_COST,
            kind: 0,
        });
    }
    if bridge {
        let x = BASE.0 + 6 * SP; // mid-span, ~1 670 m from either end
        nodes.push(Node { id: bridge_base, coord: (x, BASE.1 + 1_000) });
        nodes.push(Node { id: bridge_base + 1, coord: (x, BASE.1 - 1_000) });
        edges.push(Edge {
            a: bridge_base,
            b: bridge_base + 1,
            polyline: vec![(x, BASE.1 + 1_000), (x, BASE.1 - 1_000)],
            length_m: 223,
            kind: 0,
        });
    }
    NavGraph { nodes, edges }
}

/// The original route: a GPX straight along the road (one `<trkpt>` per node position) with a
/// linear elevation ramp 100 → 200 m and three named waypoints — head (~278 m), mid-span
/// (~1 670 m, on the avoided road), tail (~3 062 m). Each waypoint carries a `<sym>` and sits a
/// little off the (eastward) road line, so a splice has a category and a signed offset to carry.
const WPT_LAT_OFF: i32 = 900; // µdeg, ~100 m at this latitude

fn road_route_obcr() -> Vec<u8> {
    let mut g = String::from("<gpx>\n");
    for (x_seg, name, sym, side) in
        [(1, "W-head", "Drinking Water", 1), (6, "W-mid", "Campground", -1), (11, "W-tail", "Bike Shop", -1)]
    {
        let (lon, lat) = road_at(x_seg);
        g.push_str(&format!(
            "  <wpt lat=\"{:.7}\" lon=\"{:.7}\"><name>{}</name><sym>{}</sym></wpt>\n",
            (lat + side * WPT_LAT_OFF) as f64 * 1e-6,
            lon as f64 * 1e-6,
            name,
            sym
        ));
    }
    g.push_str("<trk><trkseg>\n");
    for i in 0..=SEGS {
        let (lon, lat) = road_at(i);
        let ele = 100.0 + 100.0 * i as f64 / SEGS as f64;
        g.push_str(&format!(
            "  <trkpt lat=\"{:.7}\" lon=\"{:.7}\"><ele>{ele:.1}</ele></trkpt>\n",
            lat as f64 * 1e-6,
            lon as f64 * 1e-6
        ));
    }
    g.push_str("</trkseg></trk></gpx>");
    convert("Road trip", &g)
}

/// Plan a detour over `bytes` with a corridor built from `route` over `[progress_m, target_m]`,
/// from/to resolved on the route — the host pipeline in miniature. Returns the plan result and
/// the detour OBCR bytes.
fn detour_over(
    bytes: &[u8],
    route_obcr: &[u8],
    progress_m: u32,
    target_m: u32,
) -> (Result<obc_route::RouteStats, NavError>, Vec<u8>) {
    let rsrc = SliceSource(route_obcr);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);
    let corridor = Corridor::build(&route, progress_m, target_m);
    let from_pos = route.position_at(progress_m).unwrap();
    let to_pos = route.position_at(target_m).unwrap();

    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_detour(
        &r,
        (from_pos.lon, from_pos.lat),
        (to_pos.lon, to_pos.lat),
        "Detour leg",
        0,
        corridor,
        &mut scratch,
        &mut tiles,
        &mut sink,
    );
    (res, sink.buf)
}

/// Decode an OBCR's full stitched point list.
fn route_points(obcr: &[u8]) -> Vec<RoutePoint> {
    let src = SliceSource(obcr);
    let idx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&idx, &src);
    let mut pts = Vec::new();
    for k in 0..idx.chunks().len() {
        let mut chunk = heapless::Vec::<RoutePoint, { obc_route::MAX_POINTS_PER_CHUNK }>::new();
        r.decode_chunk(k, &mut chunk).unwrap();
        let skip = usize::from(k > 0);
        pts.extend_from_slice(&chunk[skip..]);
    }
    pts
}

/// Measured polyline length of a stitched point list (the same metric the emitter uses).
fn measured_len(pts: &[RoutePoint]) -> f32 {
    pts.windows(2).map(|w| obc_map_scene::ground_dist_m((w[0].lon, w[0].lat), (w[1].lon, w[1].lat))).sum()
}

// ---------------------------------------------------------------------------- corridor unit

/// The corridor's edge test, pinned case by case against the #882 mechanism: the span's own
/// edges are blocked, a parallel edge is blocked, a grade-separated bridge chord and an
/// at-grade junction edge stay usable, exemption discs clear the take-off/landing, and the
/// bbox prefilter rejects far-away edges.
#[test]
fn corridor_blocks_span_edges_not_bridge_or_junction() {
    let obcr = road_route_obcr();
    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);
    let mut c = Corridor::build(&route, 0, route.total_distance_m);
    assert!(!c.is_degenerate());
    c.set_exempt_nodes(road_at(0), road_at(SEGS));

    let mid_x = BASE.0 + 6 * SP;
    // The road's own mid-span edge: both endpoints on the span, far from the exempts — blocked.
    assert!(c.blocks(road_at(5), road_at(6)), "a mid-span road edge must be blacklisted");
    // A hypothetical parallel edge 20 m north — inside the corridor — blocked.
    assert!(
        c.blocks((road_at(5).0, BASE.1 + 180), (road_at(6).0, BASE.1 + 180)),
        "a parallel edge hugging the span must be blacklisted"
    );
    // The bridge chord: endpoints ~111 m to either side — crossing, not parallel — usable.
    assert!(!c.blocks((mid_x, BASE.1 + 1_000), (mid_x, BASE.1 - 1_000)), "a grade-separated crossing must stay usable");
    // An at-grade junction edge: one endpoint on the span, the far one off it — usable.
    assert!(!c.blocks((mid_x, BASE.1), (mid_x, BASE.1 + 1_000)), "a side street leaving the span must stay usable");
    // Take-off exemption: a road edge within the start disc — usable.
    assert!(!c.blocks(road_at(0), road_at(1)), "edges near the start snap stay usable");
    // Far-away edge: bbox prefilter — usable.
    assert!(!c.blocks((BASE.0, BASE.1 + 100_000), (BASE.0 + SP, BASE.1 + 100_000)));
}

/// A long, vertex-dense span downsamples into the fixed corridor capacity; a sub-minimum span
/// reports degenerate and blocks nothing.
#[test]
fn corridor_build_downsamples_within_capacity() {
    // A ~20 km zigzag (vertices every ~55 m survive the converter's decimator).
    let mut g = String::from("<gpx><trk><trkseg>\n");
    for i in 0..400 {
        let lon = 0.5 + i as f64 * 0.000_45;
        let lat = 0.5 + if i % 2 == 0 { 0.0 } else { 0.000_3 };
        g.push_str(&format!("  <trkpt lat=\"{lat:.7}\" lon=\"{lon:.7}\"/>\n"));
    }
    g.push_str("</trkseg></trk></gpx>");
    let obcr = convert("Zigzag", &g);
    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);

    let c = Corridor::build(&route, 0, route.total_distance_m);
    assert!(!c.is_degenerate());
    assert!(c.len() <= CORRIDOR_MAX_PTS, "hard capacity cap (got {})", c.len());
    assert!(c.len() > CORRIDOR_MAX_PTS / 2, "a long span should use most of the capacity (got {})", c.len());

    let short = Corridor::build(&route, 0, MIN_DETOUR_SPAN_M - 1);
    assert!(short.is_degenerate());
    assert!(!short.blocks((BASE.0, BASE.1), (BASE.0 + SP, BASE.1)), "a degenerate corridor blocks nothing");
}

// ---------------------------------------------------------------------------- detour planning

/// The headline mechanism: the corridor over the whole road forces the plan onto the parallel
/// street — the emitted geometry never touches the road's blocked middle, and the total is the
/// street path's summed edge lengths.
#[test]
fn detour_routes_via_parallel_street() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);
    let total = route.total_distance_m;

    let (res, detour) = detour_over(&bytes, &obcr, 0, total);
    let stats = res.expect("the street detour plans");
    // connector + 12 street segments + connector — the unique unblocked path.
    assert_eq!(stats.total_distance_m, CONN_COST + 12 * SEG_COST + CONN_COST);

    let pts = route_points(&detour);
    for p in &pts {
        let on_road_lat = p.lat == BASE.1;
        let in_middle = p.lon > BASE.0 + SP && p.lon < BASE.0 + (SEGS - 1) * SP;
        assert!(!(on_road_lat && in_middle), "the detour re-entered the blocked span at ({}, {})", p.lon, p.lat);
    }
    assert_eq!((pts[0].lon, pts[0].lat), road_at(0), "starts at the start snap node");
    assert_eq!((pts.last().unwrap().lon, pts.last().unwrap().lat), road_at(SEGS), "ends at the goal snap node");
}

/// A mid-route span (rider at ~600 m, rejoin at ~2 800 m): the exemption discs let the plan use
/// the road near its snapped endpoints, but the blocked middle still forces the street loop.
#[test]
fn detour_mid_span_uses_exempt_take_off_and_landing() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let (res, detour) = detour_over(&bytes, &obcr, 600, 2_800);
    let stats = res.expect("the mid-span detour plans");
    // node2 →1→0 → connector → street ×12 → connector → 12→11→ node10.
    assert_eq!(stats.total_distance_m, 4 * SEG_COST + 2 * CONN_COST + 12 * SEG_COST);
    let pts = route_points(&detour);
    // The blocked middle (nodes 5..=8 at ~1 390..2 230 m) is never touched.
    for p in &pts {
        let in_blocked_middle = p.lat == BASE.1 && p.lon > BASE.0 + 4 * SP && p.lon < BASE.0 + 9 * SP;
        assert!(!in_blocked_middle, "the detour re-entered the blocked middle at ({}, {})", p.lon, p.lat);
    }
}

/// The bridge chord crossing the corridor is not blacklisted: with the road's far side reachable
/// only over the bridge, a plan to a south-side goal succeeds.
#[test]
fn detour_bridge_crossing_edge_stays_usable() {
    // Road + bridge; the goal is the bridge's south end, reachable only over the bridge.
    let mut graph = road_graph(false, true);
    let bridge_north = (SEGS + 1) as u32; // road_graph's dense-id layout, street absent
    let south = (BASE.0 + 6 * SP, BASE.1 - 1_000);
    // A direct link from the (exempt) start node to the bridge's north end — the approach road.
    graph.edges.push(Edge {
        a: 0,
        b: bridge_north,
        polyline: vec![road_at(0), (BASE.0 + 6 * SP, BASE.1 + 1_000)],
        length_m: 1_700,
        kind: 0,
    });
    let bytes = map_with(&graph);
    let obcr = road_route_obcr();

    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);
    let corridor = Corridor::build(&route, 0, route.total_distance_m);

    let src = SliceSource(&bytes[..]);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_detour(&r, road_at(0), south, "Over the bridge", 0, corridor, &mut scratch, &mut tiles, &mut sink);
    let stats = res.expect("the bridge route plans — the crossing edge must not be blacklisted");
    assert_eq!(stats.total_distance_m, 1_700 + 223);
}

/// With no relief street, the corridor seals the only connection: the frontier drains without
/// filling the table — an honest `NoPath`.
#[test]
fn detour_corridor_seals_only_path_is_nopath() {
    let bytes = map_with(&road_graph(false, false));
    let obcr = road_route_obcr();
    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);
    let (res, _) = detour_over(&bytes, &obcr, 0, route.total_distance_m);
    assert_eq!(res.unwrap_err(), NavError::NoPath);
}

/// A degenerate (sub-minimum) corridor blocks nothing: the detour plan is byte-identical to a
/// plain plan over the same endpoints — the POI path's untouched-behavior regression.
#[test]
fn detour_with_degenerate_corridor_matches_plain_plan() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);
    let corridor = Corridor::build(&route, 0, MIN_DETOUR_SPAN_M - 1);
    assert!(corridor.is_degenerate());

    let src = SliceSource(&bytes[..]);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);

    let from = road_at(0);
    let to = road_at(SEGS);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut plain = VecSink::default();
    let plain_res = plan_route(&r, from, to, "Same", 0, &mut scratch, &mut tiles, &mut plain).unwrap();

    let mut scratch2 = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles2 = NavTileCache::new();
    let mut det = VecSink::default();
    let det_res = plan_detour(&r, from, to, "Same", 0, corridor, &mut scratch2, &mut tiles2, &mut det).unwrap();

    assert_eq!(plain_res, det_res);
    assert_eq!(plain.buf, det.buf, "a degenerate corridor must not perturb the plan by a single byte");
}

// ---------------------------------------------------------------------------- splice

/// Splice fixture: the mid-span detour (600 → 2 800 m) spliced into the road route. Returns
/// `(spliced_bytes, spliced_stats, detour_len_m)`.
fn spliced_road() -> (Vec<u8>, obc_route::RouteStats, u32) {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let (res, detour) = detour_over(&bytes, &obcr, 600, 2_800);
    let dstats = res.unwrap();

    let osrc = SliceSource(&obcr[..]);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let dsrc = SliceSource(&detour[..]);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);

    let mut sink = VecSink::default();
    let stats = splice_detour(&orig, &det, 600, 2_800, dstats.total_distance_m, &mut sink).unwrap();
    (sink.buf, stats, dstats.total_distance_m)
}

/// The spliced OBCR is a completely ordinary route: it parses, its per-chunk cumulative
/// distances are strictly monotonic, its name carries the detour prefix, and its header total is
/// the measured head/seams/tail plus the planner's honest detour length.
#[test]
fn splice_output_roundtrips_and_total_is_preview_consistent() {
    let (spliced, stats, detour_len) = spliced_road();
    let src = SliceSource(&spliced[..]);
    let idx = RouteIndex::read(&src).expect("the spliced OBCR parses");
    assert_eq!(idx.name(), "Detour · Road trip");
    assert_eq!(idx.total_distance_m, stats.total_distance_m);

    let mut prev = None;
    for cm in idx.chunks() {
        if let Some(p) = prev {
            assert!(cm.cum_distance_m > p, "chunk cum distances must be strictly monotonic");
        }
        prev = Some(cm.cum_distance_m);
    }

    // Header total = measured(head + seams + tail) + planner detour length: reconstruct the
    // measured non-detour part from the stitched polyline and the detour's own measured length.
    let pts = route_points(&spliced);
    let all_measured = measured_len(&pts);
    // The spliced measured total differs from the override only in the detour term.
    let header = stats.total_distance_m as f32;
    assert!(
        (header - all_measured).abs() < 0.02 * all_measured + (detour_len as f32 - 0.0) * 0.05,
        "override total ({header}) should stay within a few percent of the measured polyline ({all_measured})"
    );
    assert!(header as u32 >= 600 + detour_len, "total ≥ head + planner detour length");
}

/// Elevation across the splice: head/tail keep the ramp verbatim, the detour is a monotone lerp
/// between the seam elevations — the whole spliced profile is non-decreasing (the ramp ascends),
/// with no spike at either seam, and the recomputed ascent matches the ramp's ~100 m.
#[test]
fn splice_interpolates_detour_elevation_without_spikes() {
    let (spliced, stats, _) = spliced_road();
    let pts = route_points(&spliced);
    assert!(pts.first().unwrap().ele >= 99 && pts.first().unwrap().ele <= 101, "head start keeps ~100 m");
    assert!(pts.last().unwrap().ele >= 199 && pts.last().unwrap().ele <= 201, "tail end keeps ~200 m");
    for w in pts.windows(2) {
        assert!(
            w[1].ele >= w[0].ele - 1,
            "spliced elevation must be non-decreasing (ramp + monotone lerp), got {} → {}",
            w[0].ele,
            w[1].ele
        );
    }
    assert!(
        (80..=120).contains(&stats.total_ascent_m),
        "recomputed ascent should be the ramp's ~100 m, got {}",
        stats.total_ascent_m
    );
    assert_eq!(stats.total_descent_m, 0);
}

/// Waypoints across the splice: the head waypoint keeps its distance, the mid-span waypoint (on
/// the avoided road) is dropped, and the tail waypoint lands at the same distance-from-end on
/// the spliced route as on the original.
#[test]
fn splice_keeps_head_drops_span_shifts_tail_waypoints() {
    let (spliced, _, _) = spliced_road();
    let src = SliceSource(&spliced[..]);
    let mut got: Vec<(String, u32)> = Vec::new();
    for_each_waypoint(&src, |w| got.push((w.name.as_str().into(), w.dist_along_m))).unwrap();
    assert_eq!(got.len(), 2, "head + tail kept, mid-span dropped (got {got:?})");
    assert_eq!(got[0].0, "W-head");
    assert!((got[0].1 as i64 - 278).abs() < 6, "head waypoint keeps its along-route distance (got {})", got[0].1);
    assert_eq!(got[1].0, "W-tail");

    // Same distance-from-end as on the original (~278 m), on the measured axis.
    let pts = route_points(&spliced);
    let measured_total = measured_len(&pts) as i64;
    assert!(
        (measured_total - got[1].1 as i64 - 278).abs() < 12,
        "tail waypoint keeps its distance from the route end (total {measured_total}, got {})",
        got[1].1
    );
}

/// …and each surviving waypoint keeps its **category** and its **signed lateral offset** through
/// the rewrite (#947): head and tail sit beside untouched geometry, so both ride along verbatim —
/// the same treatment `dist_along_m` gets on the head.
#[test]
fn splice_preserves_waypoint_categories_and_offsets() {
    let original = road_route_obcr();
    let (spliced, _, _) = spliced_road();

    let read = |bytes: &[u8]| {
        let src = SliceSource(bytes);
        let mut out: Vec<(String, u8, i16)> = Vec::new();
        for_each_waypoint(&src, |w| out.push((w.name.as_str().into(), w.category_id, w.lateral_offset_m))).unwrap();
        out
    };

    let before = read(&original);
    // North of an eastward road is left (negative); south is right (positive). ~100 m either way.
    assert_eq!(before.iter().map(|w| w.1).collect::<Vec<_>>(), [1, 2, 6], "water · campsite · bike shop");
    assert!(before[0].2 < -90 && before[0].2 > -110, "head waypoint sits ~100 m left (got {})", before[0].2);
    assert!(before[2].2 > 90 && before[2].2 < 110, "tail waypoint sits ~100 m right (got {})", before[2].2);

    let after = read(&spliced);
    assert_eq!(after, vec![before[0].clone(), before[2].clone()], "the survivors' category + offset are unchanged");
}

/// The seam contract the matcher-floor install relies on: the spliced head is the original
/// `[0, split_m]` verbatim, so `position_at(split_m)` lands on the same coordinate on both.
#[test]
fn splice_head_length_equals_split_progress() {
    let (spliced, _, _) = spliced_road();
    let obcr = road_route_obcr();

    let osrc = SliceSource(&obcr[..]);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let ssrc = SliceSource(&spliced[..]);
    let sidx = RouteIndex::read(&ssrc).unwrap();
    let spl = RouteReader::new(&sidx, &ssrc);

    let a = orig.position_at(600).unwrap();
    let b = spl.position_at(600).unwrap();
    let d = obc_map_scene::ground_dist_m((a.lon, a.lat), (b.lon, b.lat));
    assert!(d < 5.0, "the spliced head must measure split_m at the seam (drift {d} m)");
}

/// Splicing a previous splice's output works and does not stack name prefixes.
#[test]
fn splice_self_input_is_previous_output() {
    let (first, _, detour_len) = spliced_road();
    let bytes = map_with(&road_graph(true, false));
    let (_, detour) = detour_over(&bytes, &road_route_obcr(), 600, 2_800);

    let osrc = SliceSource(&first[..]);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let dsrc = SliceSource(&detour[..]);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);

    let mut sink = VecSink::default();
    let stats = splice_detour(&orig, &det, 700, 2_900, detour_len, &mut sink).unwrap();
    let src = SliceSource(&sink.buf[..]);
    let idx = RouteIndex::read(&src).expect("a re-spliced route still parses");
    assert_eq!(idx.name(), "Detour · Road trip", "no stacked name prefixes");
    assert_eq!(idx.total_distance_m, stats.total_distance_m);
}

// ---------------------------------------------------------------------------- rejoin-at-first-contact

/// Run [`trim_detour_to_tail`] over an original + detour OBCR at `target_m`; return the outcome and
/// the (possibly trimmed) sink bytes.
fn trim_run(orig_obcr: &[u8], detour_obcr: &[u8], target_m: u32) -> (Option<TrimOutcome>, Vec<u8>) {
    let osrc = SliceSource(orig_obcr);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let dsrc = SliceSource(detour_obcr);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);
    let mut sink = VecSink::default();
    let out = trim_detour_to_tail(&orig, &det, target_m, &mut sink).unwrap();
    (out, sink.buf)
}

/// Splice an original + detour and return the spliced route's header total distance.
fn spliced_total(orig_obcr: &[u8], detour_obcr: &[u8], split_m: u32, rejoin_m: u32, detour_len_m: u32) -> u32 {
    let osrc = SliceSource(orig_obcr);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let dsrc = SliceSource(detour_obcr);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);
    let mut sink = VecSink::default();
    splice_detour(&orig, &det, split_m, rejoin_m, detour_len_m, &mut sink).unwrap().total_distance_m
}

/// The headline #882 fix: with connectors only at the road's ends, a plan to a mid-route rejoin
/// (target ≈ node 9) must overshoot to road12 up the parallel street and ride the route tail back
/// down (12→…→9) to reach the goal. `trim_detour_to_tail` advances the rejoin to that first tail
/// contact (≈ the road end), truncating the retrace — and the spliced route is far shorter.
#[test]
fn trim_rejoins_at_first_tail_contact_and_removes_the_retrace() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let target = 2_500;
    let (res, detour) = detour_over(&bytes, &obcr, 0, target);
    let dstats = res.expect("the street detour plans");

    // Fixture check: the UNTRIMMED plan reproduces the bug — it overshoots to road12 and then
    // descends the tail to the goal node9 (the retrace the rider sees on glass).
    let untrimmed = route_points(&detour);
    assert!(
        untrimmed.iter().any(|p| (p.lon, p.lat) == road_at(SEGS)),
        "the untrimmed plan overshoots to road12 (the ring)"
    );
    let last = untrimmed.last().unwrap();
    assert_eq!((last.lon, last.lat), road_at(9), "…then descends the tail to land at the goal node9");

    // Trim: rejoin advances to the first contact near the road end, past the chosen minimum.
    let (out, trimmed) = trim_run(&obcr, &detour, target);
    let out = out.expect("the retrace is trimmed");
    assert!(out.rejoin_m > target + 500, "rejoin advances toward the road end (got {})", out.rejoin_m);

    // The trimmed detour ends at the ring (road12) and no longer descends the tail.
    let tpts = route_points(&trimmed);
    assert_eq!(
        (tpts.last().unwrap().lon, tpts.last().unwrap().lat),
        road_at(SEGS),
        "the trimmed detour ends at the first contact (road12), not back down at node9"
    );
    for p in &tpts {
        let descends_tail = p.lat == BASE.1 && p.lon > BASE.0 + 9 * SP && p.lon < BASE.0 + SEGS * SP;
        assert!(!descends_tail, "no trimmed point rides the tail between node9 and node12 ({}, {})", p.lon, p.lat);
    }

    // The whole splice is far shorter: the retrace is gone from both the detour and the re-ridden tail.
    let untrimmed_total = spliced_total(&obcr, &detour, 0, target, dstats.total_distance_m);
    let trimmed_total = spliced_total(&obcr, &trimmed, 0, out.rejoin_m, out.detour_len_m);
    assert!(
        untrimmed_total >= trimmed_total + 1_000,
        "the trimmed splice drops ≥1 km (untrimmed {untrimmed_total}, trimmed {trimmed_total})"
    );
}

/// A normal landing that touches the route tail only at its final pair, right at the goal, is a
/// no-op: every plan hugs the tail near the goal by construction, so the trim must not churn bytes.
/// Hand-built detour (loops north, lands on the road at node6 = the target, rides one segment to
/// node7) so the landing geometry is exact.
#[test]
fn trim_is_a_noop_for_a_normal_landing() {
    let obcr = road_route_obcr();
    // node6 ≈ 1 670 m along; the detour lands there and rides one segment forward (node7).
    let target = 1_670;
    let n = |k: i32| road_at(k);
    let north = |k: i32| (road_at(k).0, BASE.1 + STREET_OFF);
    let detour = convert(
        "Detour leg",
        &format!(
            "<gpx><trk><trkseg>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             </trkseg></trk></gpx>",
            n(2).1 as f64 * 1e-6,
            n(2).0 as f64 * 1e-6, // node2 (rider)
            north(2).1 as f64 * 1e-6,
            north(2).0 as f64 * 1e-6, // north
            north(6).1 as f64 * 1e-6,
            north(6).0 as f64 * 1e-6, // east, north of the landing
            n(6).1 as f64 * 1e-6,
            n(6).0 as f64 * 1e-6, // land on the road at the target
            n(7).1 as f64 * 1e-6,
            n(7).0 as f64 * 1e-6, // ride one segment forward
        ),
    );
    let (out, _) = trim_run(&obcr, &detour, target);
    assert_eq!(out, None, "a final-pair landing at the goal is not trimmed");
}

/// A detour that merely *crosses* the route tail once (a single near point, its neighbours ~400 m
/// off to either side) must not trim — the same both-points rule the corridor uses, so a crossing
/// or a bridge overpass never triggers.
#[test]
fn trim_ignores_a_perpendicular_crossing() {
    let obcr = road_route_obcr();
    let target = 1_670;
    // NW → (on the road at node8) → SE: the middle point is the only one near the tail.
    let cross = road_at(8);
    let detour = convert(
        "Detour leg",
        &format!(
            "<gpx><trk><trkseg>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             <trkpt lat=\"{:.7}\" lon=\"{:.7}\"/>\n\
             </trkseg></trk></gpx>",
            (BASE.1 + STREET_OFF) as f64 * 1e-6,
            (cross.0 - SP) as f64 * 1e-6, // NW
            cross.1 as f64 * 1e-6,
            cross.0 as f64 * 1e-6, // on the road (crossing)
            (BASE.1 - STREET_OFF) as f64 * 1e-6,
            (cross.0 + SP) as f64 * 1e-6, // SE
        ),
    );
    let (out, _) = trim_run(&obcr, &detour, target);
    assert_eq!(out, None, "a single-point crossing is not sustained contact");
}

/// A rejoin at the route end: the tail is empty, the spliced route ends at the detour's last
/// point, and only the head waypoint survives.
#[test]
fn splice_span_at_route_end() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let osrc = SliceSource(&obcr[..]);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let total = orig.total_distance_m;

    let (res, detour) = detour_over(&bytes, &obcr, 600, total);
    let dstats = res.unwrap();
    let dsrc = SliceSource(&detour[..]);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);

    let mut sink = VecSink::default();
    splice_detour(&orig, &det, 600, total, dstats.total_distance_m, &mut sink).unwrap();
    let src = SliceSource(&sink.buf[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let pts = route_points(&sink.buf);
    let end = pts.last().unwrap();
    assert_eq!((end.lon, end.lat), road_at(SEGS), "ends at the detour's goal node — the tail is empty");
    let mut names: Vec<String> = Vec::new();
    for_each_waypoint(&src, |w| names.push(w.name.as_str().into())).unwrap();
    assert_eq!(names, ["W-head"], "span-to-end drops the mid and tail waypoints");
    assert!(idx.total_distance_m >= 600 + dstats.total_distance_m);
}
