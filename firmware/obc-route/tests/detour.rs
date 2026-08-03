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
    detour_over_terrain(bytes, route_obcr, progress_m, target_m, &mut NullElevation)
}

/// [`detour_over`] with an explicit terrain the emit phase fills the detour's heights from (EL7) —
/// what a device with a mounted `.obcd` does, and what #1091's splice has to preserve.
fn detour_over_terrain(
    bytes: &[u8],
    route_obcr: &[u8],
    progress_m: u32,
    target_m: u32,
    elev: &mut dyn obc_route::ElevationSource,
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
        elev,
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
    let res = plan_detour(
        &r,
        road_at(0),
        south,
        "Over the bridge",
        0,
        corridor,
        &mut scratch,
        &mut tiles,
        &mut NullElevation,
        &mut sink,
    );
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
    let plain_res =
        plan_route(&r, from, to, "Same", 0, &mut scratch, &mut tiles, &mut NullElevation, &mut plain).unwrap();

    let mut scratch2 = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles2 = NavTileCache::new();
    let mut det = VecSink::default();
    let det_res =
        plan_detour(&r, from, to, "Same", 0, corridor, &mut scratch2, &mut tiles2, &mut NullElevation, &mut det)
            .unwrap();

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
    let stats =
        splice_detour(&orig, &det, 600, 2_800, dstats.total_distance_m, dstats.has_elevation, &mut sink).unwrap();
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
    let stats = splice_detour(&orig, &det, 700, 2_900, detour_len, false, &mut sink).unwrap();
    let src = SliceSource(&sink.buf[..]);
    let idx = RouteIndex::read(&src).expect("a re-spliced route still parses");
    assert_eq!(idx.name(), "Detour · Road trip", "no stacked name prefixes");
    assert_eq!(idx.total_distance_m, stats.total_distance_m);
}

// ---------------------------------------------------------------------------- rejoin-at-first-contact

/// Run [`trim_detour_to_tail`] over an original + detour OBCR at `target_m`; return the outcome and
/// the (possibly trimmed) sink bytes.
fn trim_run(
    orig_obcr: &[u8],
    detour_obcr: &[u8],
    target_m: u32,
    detour_has_elevation: bool,
) -> (Option<TrimOutcome>, Vec<u8>) {
    let osrc = SliceSource(orig_obcr);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let dsrc = SliceSource(detour_obcr);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);
    let mut sink = VecSink::default();
    let out = trim_detour_to_tail(&orig, &det, target_m, detour_has_elevation, &mut sink).unwrap();
    (out, sink.buf)
}

/// Splice an original + detour and return the spliced route's header total distance.
fn spliced_total(
    orig_obcr: &[u8],
    detour_obcr: &[u8],
    split_m: u32,
    rejoin_m: u32,
    detour_len_m: u32,
    detour_has_elevation: bool,
) -> u32 {
    let osrc = SliceSource(orig_obcr);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let dsrc = SliceSource(detour_obcr);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);
    let mut sink = VecSink::default();
    splice_detour(&orig, &det, split_m, rejoin_m, detour_len_m, detour_has_elevation, &mut sink)
        .unwrap()
        .total_distance_m
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
    let (out, trimmed) = trim_run(&obcr, &detour, target, dstats.has_elevation);
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
    let untrimmed_total = spliced_total(&obcr, &detour, 0, target, dstats.total_distance_m, dstats.has_elevation);
    let trimmed_total = spliced_total(&obcr, &trimmed, 0, out.rejoin_m, out.detour_len_m, dstats.has_elevation);
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
    let (out, _) = trim_run(&obcr, &detour, target, false);
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
    let (out, _) = trim_run(&obcr, &detour, target, false);
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
    splice_detour(&orig, &det, 600, total, dstats.total_distance_m, dstats.has_elevation, &mut sink).unwrap();
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

// ------------------------------------------------------- climb-aware dispatch (EL6, epic #1068)
//
// #882's detour dispatch is not a second router: `plan_detour` is `plan_route` plus a corridor
// blacklist, running the same `settle` and therefore the same §8.6 edge cost. EL6 added a climb
// term to that cost, so detours became climb-aware with no code of their own — and these two tests
// are what stops a future change from quietly forking the model.

/// A steep north-facing hillside for the detour fixture: 1 m of rise per 5 µdeg of latitude above
/// the road, dead flat at or below it. The **north** relief street therefore sits [`NORTH_CLIMB_M`]
/// above the road and the **south** one is on the flat, which is the only difference between them
/// the climb term can see.
struct Hillside;

/// What the north connector climbs: `STREET_OFF / 5`.
const NORTH_CLIMB_M: u32 = 720;
/// One segment of the (longer, flat) south relief street, m — chosen so the south corridor costs
/// 3 000 m more ground than the north one, three times the ε inflation the frontier carries.
const SOUTH_SEG_COST: u32 = 530;

impl obc_route::ElevationSource for Hillside {
    fn sample(&mut self, lat_udeg: i32, _lon_udeg: i32) -> Option<i16> {
        Some(((lat_udeg - BASE.1).max(0) / 5) as i16)
    }
}

/// [`map_with`] with an explicit profile and a terrain to bake §8.3 `Ascent M` from.
fn map_with_terrain(graph: &NavGraph, profile: NavProfile, terrain: &mut dyn obc_route::ElevationSource) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, &[profile], terrain);
    assert_eq!(dropped, 0);
    bin
}

/// A neutral-multiplier profile carrying nothing but a climb weight.
fn climb_profile(climb_weight: u8) -> NavProfile {
    NavProfile { name: "Climb".into(), highway: [16; 32], surface: [16; 8], climb_weight }
}

/// South relief street node `i` — the mirror of [`street_at`], the same offset below the road.
fn south_at(i: i32) -> (i32, i32) {
    (BASE.0 + i * SP, BASE.1 - STREET_OFF)
}

/// The road with **two** relief corridors: the usual street north of it (short, but reached by
/// climbing [`NORTH_CLIMB_M`] up the [`Hillside`]) and a mirror street south of it (flat, but
/// 3 000 m more ground). With the road blacklisted the detour has a genuine choice, and only the
/// climb term can make it.
fn road_graph_two_reliefs() -> NavGraph {
    let mut g = road_graph(true, false);
    let south_id = |i: i32| (2 * (SEGS + 1) + i) as u32;
    for i in 0..=SEGS {
        g.nodes.push(Node { id: south_id(i), coord: south_at(i) });
        if i < SEGS {
            g.edges.push(Edge {
                a: south_id(i),
                b: south_id(i + 1),
                polyline: vec![south_at(i), south_at(i + 1)],
                length_m: SOUTH_SEG_COST,
                kind: 0,
            });
        }
    }
    for i in [0, SEGS] {
        g.edges.push(Edge {
            a: i as u32,
            b: south_id(i),
            polyline: vec![road_at(i), south_at(i)],
            length_m: CONN_COST,
            kind: 0,
        });
    }
    g
}

/// Raw ground length of one relief corridor: two connectors plus [`SEGS`] street segments.
fn relief_len(seg_cost: u32) -> u32 {
    2 * CONN_COST + SEGS as u32 * seg_cost
}

/// Plan `from → to` over `bytes`, optionally under a corridor blacklist — the two dispatch paths
/// side by side, sharing every other argument, so a difference in their output can only come from
/// the corridor.
fn plan_either(bytes: &[u8], from: (i32, i32), to: (i32, i32), corridor: Option<Corridor>) -> (u32, Vec<u8>) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = match corridor {
        Some(c) => plan_detour(&r, from, to, "Leg", 0, c, &mut scratch, &mut tiles, &mut NullElevation, &mut sink),
        None => plan_route(&r, from, to, "Leg", 0, &mut scratch, &mut tiles, &mut NullElevation, &mut sink),
    };
    (res.expect("the fixture always has a legal path").total_distance_m, sink.buf)
}

/// **The detour dispatch is climb-aware for free.** With the road blacklisted the plan must choose
/// between the two reliefs, and it flips on the profile's climb weight exactly as a plain plan
/// would: climb-blind it takes the short hill, climb-weighted it pays 3 km of extra ground to stay
/// on the flat.
#[test]
fn a_detour_weighs_climb_the_same_way_a_plan_does() {
    let graph = road_graph_two_reliefs();
    let obcr = road_route_obcr();

    // Non-vacuity: the north connector really does bake the climb this test spends, and only in the
    // uphill direction.
    let (up, down) = obc_pack::nav::integrate_edge_ascent(&[road_at(0), street_at(0)], &mut Hillside);
    assert!(
        (up as i64 - NORTH_CLIMB_M as i64).abs() <= 4,
        "the north connector should bake ≈ {NORTH_CLIMB_M} m, got {up}"
    );
    assert_eq!(down, 0, "and nothing coming back down");

    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);
    let total = route.total_distance_m;

    let blind = map_with_terrain(&graph, climb_profile(0), &mut Hillside);
    let (dist, _) =
        plan_either(&blind, road_at(0), road_at(SEGS), Some(Corridor::build(&RouteReader::new(&idx, &rsrc), 0, total)));
    assert_eq!(dist, relief_len(SEG_COST), "climb-blind, the detour takes the short north street");

    let weighted = map_with_terrain(&graph, climb_profile(20), &mut Hillside);
    let (dist, _) = plan_either(&weighted, road_at(0), road_at(SEGS), Some(Corridor::build(&route, 0, total)));
    assert_eq!(dist, relief_len(SOUTH_SEG_COST), "at a heavy climb weight it detours south, onto the flat");
}

/// **One cost model, not two.** `detour_with_degenerate_corridor_matches_plain_plan` above pins the
/// same equality on a *flat* map; this is its v12 twin, and the difference is the whole point: the
/// map's ascents are real and the profile charges 20 flat metres for each of them, so the two paths
/// agree on a cost the climb term dominates. The corridor is then provably the only thing the
/// detour dispatch adds — the climb term is not re-derived, re-scaled or re-rounded on the way to a
/// detour.
#[test]
fn a_detour_and_a_plan_cost_identically_when_nothing_is_blacklisted() {
    let bytes = map_with_terrain(&road_graph_two_reliefs(), climb_profile(20), &mut Hillside);
    let obcr = road_route_obcr();
    let rsrc = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&rsrc).unwrap();
    let route = RouteReader::new(&idx, &rsrc);

    let empty = Corridor::build(&route, 0, MIN_DETOUR_SPAN_M / 2);
    assert!(empty.is_degenerate(), "a sub-minimum span must blacklist nothing, or this proves nothing");

    let (detour_len, detour_obcr) = plan_either(&bytes, road_at(0), road_at(SEGS), Some(empty));
    let (plan_len, plan_obcr) = plan_either(&bytes, road_at(0), road_at(SEGS), None);
    assert_eq!(detour_len, plan_len);
    assert_eq!(detour_obcr, plan_obcr, "an unblacklisted detour is a plan, byte for byte");
    // …and the shared answer is the road itself, which is flat and shorter than either relief.
    assert_eq!(plan_len, SEGS as u32 * SEG_COST, "the road is the cheapest way when nothing blocks it");
}

// --------------------------------------------- the splice keeps sampled terrain (#1091, epic #1068)
//
// Before EL7 a detour arrived with `ele == 0` throughout and the splice's only honest option was a
// seam-to-seam lerp. Now `plan_detour` samples the map's terrain at every emitted vertex, so the
// splice keeps those heights and only removes the *datum* mismatch at the two joins, by adding a
// linear blend of the two seam residuals. These tests pin both halves of that: the blend, and the
// exact identity it degrades to when there is no terrain.

/// The [`Hillside`] shifted up by a constant — a terrain whose datum disagrees with the fixture
/// route's own `<ele>` ramp everywhere, which is the GPX-imported case (canopy / barometric offset
/// against a bare-earth DEM). The residual blend must absorb the offset without flattening the
/// shape underneath it.
const DEM_OFFSET_M: i16 = 300;

struct OffsetHillside;

impl obc_route::ElevationSource for OffsetHillside {
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        Hillside.sample(lat_udeg, lon_udeg).map(|h| h + DEM_OFFSET_M)
    }
}

/// FNV-1a/64 over the spliced bytes — a cheap, stable digest so "byte-identical" is one number a
/// future change trips on, not a 3 kB literal.
fn digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Splice the mid-span (600 → 2 800 m) detour planned over `elev` into the road route; returns the
/// spliced bytes, the spliced stats and the detour plan's own stats.
fn spliced_over(elev: &mut dyn obc_route::ElevationSource) -> (Vec<u8>, obc_route::RouteStats, obc_route::RouteStats) {
    spliced_span(600, 2_800, elev)
}

/// The **whole-route** splice (`split_m = 0`, rejoin at the route end): the head and the tail are
/// both empty, so the spliced point stream *is* the blended detour and nothing has to guess where
/// its two seams sit in the output. The seam heights are then simply the original's first and last.
fn spliced_whole(elev: &mut dyn obc_route::ElevationSource) -> (Vec<u8>, obc_route::RouteStats, obc_route::RouteStats) {
    let obcr = road_route_obcr();
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let total = RouteReader::new(&idx, &src).total_distance_m;
    spliced_span(0, total, elev)
}

fn spliced_span(
    split_m: u32,
    rejoin_m: u32,
    elev: &mut dyn obc_route::ElevationSource,
) -> (Vec<u8>, obc_route::RouteStats, obc_route::RouteStats) {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let (res, detour) = detour_over_terrain(&bytes, &obcr, split_m, rejoin_m, elev);
    let dstats = res.expect("the street detour plans");

    let osrc = SliceSource(&obcr[..]);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let dsrc = SliceSource(&detour[..]);
    let didx = RouteIndex::read(&dsrc).unwrap();
    let det = RouteReader::new(&didx, &dsrc);

    let mut sink = VecSink::default();
    let stats = splice_detour(&orig, &det, split_m, rejoin_m, dstats.total_distance_m, dstats.has_elevation, &mut sink)
        .unwrap();
    (sink.buf, stats, dstats)
}

/// The two seam heights the fixture route stores at a splice's `split_m` / `rejoin_m`.
fn road_route_seams(split_m: u32, rejoin_m: u32) -> (i16, i16) {
    let obcr = road_route_obcr();
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let orig = RouteReader::new(&idx, &src);
    (orig.elevation_at(split_m).unwrap(), orig.elevation_at(rejoin_m).unwrap())
}

/// The whole-route splice's seam pair.
fn whole_route_seams() -> (i16, i16) {
    let obcr = road_route_obcr();
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let total = RouteReader::new(&idx, &src).total_distance_m;
    road_route_seams(0, total)
}

/// **The degrade is an identity.** A detour that resolved no terrain at all (`NullElevation` ⇒
/// `has_elevation == false`) splices to exactly what it spliced before #1091: every detour point on
/// the straight seam-to-seam lerp, recomputed here independently, and the whole file on a pinned
/// digest.
///
/// The digest is `origin/develop`'s six-argument `splice_detour` over this same fixture (see the PR
/// body) — it is what makes "the old behaviour is untouched" a claim about bytes rather than intent.
#[test]
fn splice_without_detour_elevation_is_the_old_seam_lerp() {
    let (spliced, _, _) = spliced_road();
    let bytes = map_with(&road_graph(true, false));
    let (res, _) = detour_over(&bytes, &road_route_obcr(), 600, 2_800);
    assert!(!res.unwrap().has_elevation, "a NullElevation plan must report no elevation — the fixture's premise");

    assert_eq!(
        digest(&spliced),
        DEVELOP_SPLICE_DIGEST,
        "an elevation-less detour must splice byte-identically to the pre-#1091 seam lerp"
    );

    // …and independently, on the whole-route splice (where the output *is* the detour span): every
    // point sits on the straight seam-to-seam interpolation, recomputed here from the detour's own
    // arc length rather than read back out of the splice.
    let (whole, _, _) = spliced_whole(&mut NullElevation);
    let (lo, hi) = whole_route_seams();
    let arcs = detour_arc_fractions(0, u32::MAX, &mut NullElevation);
    let pts = route_points(&whole);
    // With no heights to keep, the emitter's purely planar decimator collapses the straight street
    // to its corners — few points, but every one of them is on the lerp.
    assert!(pts.len() >= 4, "the fixture must have a real span to check (got {})", pts.len());
    for p in &pts {
        let t = arcs.get(&(p.lon, p.lat)).copied().expect("every spliced point is a detour point here");
        let expect = (lo as f32 + (hi as f32 - lo as f32) * t).round() as i16;
        assert!(
            (p.ele - expect).abs() <= 1,
            "an elevation-less detour must be the seam lerp: at t={t:.3} expected {expect}, got {}",
            p.ele
        );
    }
    assert_eq!(pts.first().unwrap().ele, lo, "…opening exactly on the split seam");
    assert_eq!(pts.last().unwrap().ele, hi, "…and landing exactly on the rejoin seam");
}

/// The pinned pre-#1091 digest — see [`splice_without_detour_elevation_is_the_old_seam_lerp`].
const DEVELOP_SPLICE_DIGEST: u64 = 0x3908_7aa0_c87c_ed01;

/// The planned detour's points keyed to their arc fraction along it — the blend's independent
/// denominator, measured with the same per-segment metric the splice accumulates.
fn detour_arc_fractions(
    split_m: u32,
    rejoin_m: u32,
    elev: &mut dyn obc_route::ElevationSource,
) -> std::collections::HashMap<(i32, i32), f32> {
    let obcr = road_route_obcr();
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let total = RouteReader::new(&idx, &src).total_distance_m;
    let rejoin_m = rejoin_m.min(total);
    let bytes = map_with(&road_graph(true, false));
    let (_, detour) = detour_over_terrain(&bytes, &obcr, split_m, rejoin_m, elev);

    let pts = route_points(&detour);
    let len = measured_len(&pts);
    let mut out = std::collections::HashMap::new();
    let mut along = 0.0f32;
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            along += obc_map_scene::ground_dist_m((pts[i - 1].lon, pts[i - 1].lat), (p.lon, p.lat));
        }
        out.insert((p.lon, p.lat), if len > 1e-3 { (along / len).clamp(0.0, 1.0) } else { 1.0 });
    }
    out
}

/// **Sampled heights survive, and both seams stay exact.** With the hillside mounted, the
/// whole-route detour up the 720 m north street arrives carrying that hump — and the spliced route
/// still opens and lands exactly on the stored route's own seam heights.
#[test]
fn splice_keeps_the_detour_sampled_shape_and_matches_both_seams() {
    let (spliced, stats, dstats) = spliced_whole(&mut Hillside);
    assert!(dstats.has_elevation, "the terrain answered for the plan");
    assert!(stats.has_elevation, "…so the spliced route carries elevation too");

    let (lo, hi) = whole_route_seams();
    let eles: Vec<i16> = route_points(&spliced).iter().map(|p| p.ele).collect();
    assert_eq!(*eles.first().unwrap(), lo, "no step at the split seam");
    assert_eq!(*eles.last().unwrap(), hi, "no step at the rejoin seam");

    // The interior is the terrain's, not a ramp: the street sits NORTH_CLIMB_M above the road, so
    // the span peaks far outside the seam interval a lerp could never leave.
    let peak = *eles.iter().max().unwrap();
    assert!(
        peak > hi + NORTH_CLIMB_M as i16 / 2,
        "the spliced detour must carry the street's real height (peak {peak}, seams {lo}..{hi})"
    );
    assert!(
        (peak as i32 - (NORTH_CLIMB_M as i32 + hi as i32)).abs() < 150,
        "…and it must be the sampled hump plus the blended residual, not something invented (peak {peak})"
    );

    // The mid-span twin keeps head and tail verbatim while the same blend runs between them.
    let (mid, _, _) = spliced_over(&mut Hillside);
    let mid_pts = route_points(&mid);
    assert_eq!(mid_pts.first().unwrap().ele, 100, "the head keeps the original's stored heights");
    assert_eq!(mid_pts.last().unwrap().ele, 200, "…and so does the tail");
    assert!(
        mid_pts.iter().any(|p| p.ele > 400),
        "…and the spliced middle still carries the hillside (max {})",
        mid_pts.iter().map(|p| p.ele).max().unwrap()
    );
}

/// **A DEM that disagrees with the route's datum.** `OffsetHillside` puts the whole raster
/// [`DEM_OFFSET_M`] above the fixture route's `<ele>` ramp — the GPX-imported case. The blend must
/// absorb the offset at both seams *and* leave the shape between them alone: a constant datum shift
/// moves both residuals by the same constant, so it cancels exactly and the spliced span is
/// height-for-height what the un-offset terrain produced.
#[test]
fn splice_absorbs_a_dem_datum_offset_without_flattening_the_interior() {
    let offset: Vec<i16> = route_points(&spliced_whole(&mut OffsetHillside).0).iter().map(|p| p.ele).collect();
    let plain: Vec<i16> = route_points(&spliced_whole(&mut Hillside).0).iter().map(|p| p.ele).collect();
    let (lo, hi) = whole_route_seams();

    assert_eq!(*offset.first().unwrap(), lo, "the seam is exact however far the DEM's datum sits");
    assert_eq!(*offset.last().unwrap(), hi, "…at both ends");
    assert_eq!(
        offset, plain,
        "a constant DEM offset is absorbed whole — the spliced profile cannot depend on the raster's datum"
    );

    // Non-vacuity: the shape is genuinely there, and genuinely not a lerp.
    assert!(
        offset.iter().any(|&e| e > hi + 200),
        "the interior must still rise well above the seam interval (got max {})",
        offset.iter().max().unwrap()
    );
}

/// **The spliced header's climb is recomputed over the final stream.** Independently re-integrate
/// the spliced route's own points through the shared dead-band and compare — the header must be
/// what a plain planned route's would be over those bytes, not a sum of two producers' totals.
#[test]
fn splice_stats_are_the_dead_band_over_the_final_point_stream() {
    let cases = [("no terrain", spliced_road().0, spliced_road().1), {
        let (bytes, stats, _) = spliced_over(&mut Hillside);
        ("terrain", bytes, stats)
    }];
    for (label, spliced, stats) in &cases {
        let pts = route_points(spliced);
        let mut band = obc_elevation::DeadBand::<f64>::new();
        let (mut lo, mut hi) = (i16::MAX, i16::MIN);
        for p in &pts {
            band.push(f64::from(p.ele));
            lo = lo.min(p.ele);
            hi = hi.max(p.ele);
        }
        assert_eq!(stats.total_ascent_m, band.ascent() as u32, "{label}: header ascent is the stream's");
        assert_eq!(stats.total_descent_m, band.descent() as u32, "{label}: header descent is the stream's");
        assert_eq!((stats.min_ele_m, stats.max_ele_m), (lo, hi), "{label}: header min/max are the stream's");
    }

    // The terrain case must actually exercise the descent arm, or those assertions are vacuous: the
    // detour climbs the street and comes back down.
    let terrain = &cases[1].2;
    assert!(terrain.total_ascent_m > NORTH_CLIMB_M / 2, "the hump is climbed (got {})", terrain.total_ascent_m);
    assert!(terrain.total_descent_m > NORTH_CLIMB_M / 2, "…and descended (got {})", terrain.total_descent_m);
    // The elevation-less splice books the ramp only — the pre-#1091 figures, unchanged.
    assert_eq!(cases[0].2.total_descent_m, 0);
    assert!((80..=120).contains(&cases[0].2.total_ascent_m));
}

/// The trim path is on the same contract: a trimmed detour must reach the splice with its sampled
/// heights **and** its own climb, or the residual blend has nothing to blend onto and the preview
/// has nothing to price.
#[test]
fn a_trimmed_detour_keeps_its_sampled_heights_and_reports_its_climb() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let target = 2_500;
    let (res, detour) = detour_over_terrain(&bytes, &obcr, 0, target, &mut Hillside);
    let dstats = res.expect("the street detour plans");
    assert!(dstats.has_elevation);

    let (out, trimmed) = trim_run(&obcr, &detour, target, dstats.has_elevation);
    let out = out.expect("the retrace is trimmed");
    let tpts = route_points(&trimmed);
    assert!(
        tpts.iter().any(|p| p.ele > NORTH_CLIMB_M as i16 / 2),
        "the trimmed leg must still carry the street's sampled height"
    );
    assert!(out.ascent_m > NORTH_CLIMB_M / 2, "…and report the climb it actually does (got {})", out.ascent_m);

    // The elevation-less twin still trims to the zeroed shape it always did.
    let (res, detour) = detour_over(&bytes, &obcr, 0, target);
    let (_, trimmed) = trim_run(&obcr, &detour, target, res.unwrap().has_elevation);
    assert!(route_points(&trimmed).iter().all(|p| p.ele == 0), "no elevation in, no elevation out");
}

/// `has_elevation` is the producers' own answer end to end, and never a look at the values: a
/// sea-level route is *not* an elevation-less one.
#[test]
fn has_elevation_is_the_producers_answer_not_the_values() {
    // A GPX with no `<ele>` at all, and the same track at a constant 0 m, store identical bytes —
    // so no amount of looking at the values can tell them apart. The converter can, and does.
    let track = |ele: Option<f64>| {
        let mut g = String::from("<gpx><trk><trkseg>\n");
        for i in 0..=SEGS {
            let (lon, lat) = road_at(i);
            let e = ele.map_or(String::new(), |e| format!("<ele>{e:.1}</ele>"));
            g.push_str(&format!(
                "  <trkpt lat=\"{:.7}\" lon=\"{:.7}\">{e}</trkpt>\n",
                lat as f64 * 1e-6,
                lon as f64 * 1e-6
            ));
        }
        g.push_str("</trkseg></trk></gpx>");
        g
    };
    let none = convert("Same name", &track(None));
    let sea = convert("Same name", &track(Some(0.0)));
    assert_eq!(none, sea, "the two files are byte-identical — no consumer of the bytes can tell them apart");

    // The reader, which only ever has the bytes, therefore gives the weaker honest answer for both…
    let src = SliceSource(&sea[..]);
    let idx = RouteIndex::read(&src).unwrap();
    assert!(!RouteReader::new(&idx, &src).has_elevation(), "a stored file has no better answer than its header");

    // …while the producer, which watched the parse, tells them apart — which is the whole reason
    // the bit is threaded from the plan to the splice instead of re-derived there.
    let mut sink = VecSink::default();
    assert!(obc_route::gpx_to_obcr(&SliceSource(track(Some(0.0)).as_bytes()), "n", &mut sink).unwrap().has_elevation);
    let mut sink = VecSink::default();
    assert!(!obc_route::gpx_to_obcr(&SliceSource(track(None).as_bytes()), "n", &mut sink).unwrap().has_elevation);
}
