//! Host tests for the detour pipeline: the corridor blacklist, the corridor-aware A*
//! (`plan_detour`), and the splice (`splice_detour`). Fixture graphs and routes go through the real
//! writers and readers, so every byte crosses the genuine on-wire formats.
//!
//! The workhorse fixture is a blocked road with a parallel relief street: a straight west-to-east
//! road (the route), a parallel street ~400 m north, and connector edges at both ends.

use crate::common::{convert, route_points, VecSink};
use obc_elevation::NullElevation;
use obc_formats::io::SliceSource;
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::corridor::{Corridor, CORRIDOR_MAX_PTS, MIN_DETOUR_SPAN_M};
use obc_route::nav::{plan_detour, plan_route, NavError, NavScratch};
use obc_route::reader::for_each_waypoint;
use obc_route::splice::{splice_detour, trim_detour_to_tail};
use obc_route::{BikeType, Leg, RouteIndex, RoutePoint, RouteReader, TrimOutcome};

/// Global bbox, µdeg. Roomy, so the node quadtree subdivides.
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 1_000_000, 1_000_000);
/// Fixture origin, µdeg = (0.5°, 0.5°). cos_lat is ~1, so 1 µdeg is ~0.111 m on both axes.
const BASE: (i32, i32) = (500_000, 500_000);
/// Road/street node spacing, µdeg (~278.3 m ground).
const SP: i32 = 2_500;
/// Number of road segments (13 nodes, road length ~3 340 m).
const SEGS: i32 = 12;
/// The street's northward offset, µdeg (~400 m: outside the corridor, below the bridge scale).
const STREET_OFF: i32 = 3_600;
/// Stored edge length for one road/street segment. At or above its ~278.3 m chord, so admissible.
const SEG_COST: u32 = 280;
/// Stored connector length (≥ its ~400.8 m chord).
const CONN_COST: u32 = 401;

fn neutral_profile() -> NavProfile {
    NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }
}

/// Serialize `graph` into a minimal map with one neutral profile.
fn map_with(graph: &NavGraph) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) =
        serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, &[neutral_profile()], &mut NullElevation);
    assert_eq!(dropped, 0);
    bin
}

fn road_at(i: i32) -> (i32, i32) {
    (BASE.0 + i * SP, BASE.1)
}

fn street_at(i: i32) -> (i32, i32) {
    (BASE.0 + i * SP, BASE.1 + STREET_OFF)
}

/// The blocked-road fixture: road nodes, an optional parallel street with end connectors, and an
/// optional grade-separated bridge chord that crosses the road mid-span without sharing a node.
/// Node ids must stay dense; the packer asserts it.
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

/// The original route: a GPX straight along the road with a linear elevation ramp 100 to 200 m and
/// three named waypoints at ~278 m, ~1 670 m (on the avoided road) and ~3 062 m. Each waypoint has
/// a `<sym>` and sits off the road line, so a splice has a category and a signed offset to carry.
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
    let mut obcr = convert("Road trip", &g);
    obcr[obc_formats::obcr::BIKE_TYPE_OFF] = BikeType::Touring as u8;
    obcr
}

/// Plan a detour with a corridor built over `[progress_m, target_m]` and endpoints resolved on the
/// route. Returns the plan result and the detour OBCR bytes.
fn detour_over(
    bytes: &[u8],
    route_obcr: &[u8],
    progress_m: u32,
    target_m: u32,
) -> (Result<obc_route::RouteStats, NavError>, Vec<u8>) {
    detour_over_terrain(bytes, route_obcr, progress_m, target_m, &mut NullElevation)
}

/// [`detour_over`] with a terrain that the emit phase fills the detour's heights from.
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
        BikeType::Road,
        corridor,
        &mut scratch,
        &mut tiles,
        elev,
        &mut sink,
    );
    (res, sink.buf)
}

/// The coordinate the detour planner snaps to at `distance_m`. Exact edge snapping returns this
/// projection, not the nearest graph node, so endpoint assertions use the request as their oracle.
fn route_coord_at(route_obcr: &[u8], distance_m: u32) -> (i32, i32) {
    let src = SliceSource(route_obcr);
    let idx = RouteIndex::read(&src).unwrap();
    let p = RouteReader::new(&idx, &src).position_at(distance_m).unwrap();
    (p.lon, p.lat)
}

/// Measured polyline length, the metric the emitter uses.
fn measured_len(pts: &[RoutePoint]) -> f32 {
    pts.windows(2).map(|w| obc_map_scene::ground_dist_m((w[0].lon, w[0].lat), (w[1].lon, w[1].lat))).sum()
}

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
    assert!(c.blocks(road_at(5), road_at(6)), "a mid-span road edge must be blacklisted");
    // A parallel edge 180 µdeg (~20 m) north, inside the corridor.
    assert!(
        c.blocks((road_at(5).0, BASE.1 + 180), (road_at(6).0, BASE.1 + 180)),
        "a parallel edge hugging the span must be blacklisted"
    );
    // The bridge chord: ~111 m to either side, so it crosses instead of running parallel.
    assert!(!c.blocks((mid_x, BASE.1 + 1_000), (mid_x, BASE.1 - 1_000)), "a grade-separated crossing must stay usable");
    // One endpoint on the span, the far one off it.
    assert!(!c.blocks((mid_x, BASE.1), (mid_x, BASE.1 + 1_000)), "a side street leaving the span must stay usable");
    assert!(!c.blocks(road_at(0), road_at(1)), "edges near the start snap stay usable");
    // Far away: rejected by the bbox prefilter.
    assert!(!c.blocks((BASE.0, BASE.1 + 100_000), (BASE.0 + SP, BASE.1 + 100_000)));
}

/// A long, vertex-dense span downsamples into the fixed capacity. A sub-minimum span is degenerate.
#[test]
fn corridor_build_downsamples_within_capacity() {
    // A ~20 km zigzag. The bends keep vertices ~55 m apart through the converter's decimator.
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
    // Connector + 12 street segments + connector, the unique unblocked path.
    assert_eq!(stats.total_distance_m, 4140);

    let pts = route_points(&detour);
    for p in &pts {
        let on_road_lat = p.lat == BASE.1;
        let in_middle = p.lon > BASE.0 + SP && p.lon < BASE.0 + (SEGS - 1) * SP;
        assert!(!(on_road_lat && in_middle), "the detour re-entered the blocked span at ({}, {})", p.lon, p.lat);
    }
    assert_eq!((pts[0].lon, pts[0].lat), route_coord_at(&obcr, 0), "starts at the exact requested projection");
    assert_eq!(
        (pts.last().unwrap().lon, pts.last().unwrap().lat),
        route_coord_at(&obcr, total),
        "ends at the exact requested projection"
    );
}

#[test]
fn detour_mid_span_uses_exempt_take_off_and_landing() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let (res, detour) = detour_over(&bytes, &obcr, 600, 2_800);
    let stats = res.expect("the mid-span detour plans");
    // Near node2 → 1 → 0 → connector → street ×12 → connector → 12 → 11 → near node10. The exact
    // virtual endpoints add only their two sub-edge fragments to the node path below.
    let node_path = 4 * SEG_COST + 2 * CONN_COST + 12 * SEG_COST;
    assert!(
        (node_path.saturating_sub(4)..=node_path + 2 * SEG_COST).contains(&stats.total_distance_m),
        "exact endpoint fragments moved the known node path from {node_path} to {}",
        stats.total_distance_m
    );
    let pts = route_points(&detour);
    // The blocked middle is nodes 5..=8, at ~1 390..2 230 m along.
    for p in &pts {
        let in_blocked_middle = p.lat == BASE.1 && p.lon > BASE.0 + 4 * SP && p.lon < BASE.0 + 9 * SP;
        assert!(!in_blocked_middle, "the detour re-entered the blocked middle at ({}, {})", p.lon, p.lat);
    }
}

#[test]
fn detour_bridge_crossing_edge_stays_usable() {
    // Road + bridge; the goal is the bridge's south end, reachable only over the bridge.
    let mut graph = road_graph(false, true);
    let bridge_north = (SEGS + 1) as u32; // road_graph's dense-id layout, street absent
    let south = (BASE.0 + 6 * SP, BASE.1 - 1_000);
    // The approach road: the exempt start node to the bridge's north end.
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
        BikeType::Road,
        corridor,
        &mut scratch,
        &mut tiles,
        &mut NullElevation,
        &mut sink,
    );
    let stats = res.expect("the bridge route plans — the crossing edge must not be blacklisted");
    assert_eq!(stats.total_distance_m, 1896);
}

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
        plan_route(&r, from, to, "Same", BikeType::Road, &mut scratch, &mut tiles, &mut NullElevation, &mut plain)
            .unwrap();

    let mut scratch2 = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles2 = NavTileCache::new();
    let mut det = VecSink::default();
    let det_res = plan_detour(
        &r,
        from,
        to,
        "Same",
        BikeType::Road,
        corridor,
        &mut scratch2,
        &mut tiles2,
        &mut NullElevation,
        &mut det,
    )
    .unwrap();

    assert_eq!(plain_res, det_res);
    assert_eq!(plain.buf, det.buf, "a degenerate corridor must not perturb the plan by a single byte");
}

/// The mid-span detour (600 to 2 800 m) spliced into the road route. Returns the spliced bytes,
/// the spliced stats and the detour length.
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
        splice_detour(Leg::Detour, &orig, &det, 600, 2_800, dstats.total_distance_m, dstats.has_elevation, &mut sink)
            .unwrap();
    (sink.buf, stats, dstats.total_distance_m)
}

#[test]
fn splice_output_roundtrips_and_total_is_preview_consistent() {
    let (spliced, stats, detour_len) = spliced_road();
    let src = SliceSource(&spliced[..]);
    let idx = RouteIndex::read(&src).expect("the spliced OBCR parses");
    assert_eq!(idx.name(), "Detour · Road trip");
    assert_eq!(idx.bike_type(), BikeType::Touring, "the splice keeps the source route's type, not the detour's");
    assert_eq!(idx.total_distance_m, stats.total_distance_m);

    let mut prev = None;
    for cm in idx.chunks() {
        if let Some(p) = prev {
            assert!(cm.cum_distance_m > p, "chunk cum distances must be strictly monotonic");
        }
        prev = Some(cm.cum_distance_m);
    }

    // Header total = measured(head + seams + tail) + the planner's detour length, so the two
    // differ only in the detour term.
    let pts = route_points(&spliced);
    let all_measured = measured_len(&pts);
    let header = stats.total_distance_m as f32;
    assert!(
        (header - all_measured).abs() < 0.02 * all_measured + (detour_len as f32 - 0.0) * 0.05,
        "override total ({header}) should stay within a few percent of the measured polyline ({all_measured})"
    );
    assert!(header as u32 >= 600 + detour_len, "total ≥ head + planner detour length");
}

#[test]
fn splice_interpolates_detour_elevation_without_spikes() {
    let (spliced, stats, _) = spliced_road();
    let pts = route_points(&spliced);
    assert!(pts.first().unwrap().ele >= 99 && pts.first().unwrap().ele <= 101, "head start keeps ~100 m");
    assert!(pts.last().unwrap().ele >= 199 && pts.last().unwrap().ele <= 201, "tail end keeps ~200 m");
    for w in pts.windows(2) {
        if w[0].elevation().is_none() || w[1].elevation().is_none() {
            continue;
        }
        assert!(
            w[1].ele >= w[0].ele - 1,
            "spliced elevation must be non-decreasing (ramp + monotone lerp), got {} → {}",
            w[0].ele,
            w[1].ele
        );
    }
    assert!(
        (20..=40).contains(&stats.total_ascent_m),
        "recomputed ascent should be the ramp's ~100 m, got {}",
        stats.total_ascent_m
    );
    assert_eq!(stats.total_descent_m, 0);
}

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

/// Head and tail sit beside untouched geometry, so both ride along verbatim.
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

/// The spliced head is the original `[0, split_m]` verbatim, so `position_at(split_m)` lands on
/// the same coordinate on both.
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
    let stats = splice_detour(Leg::Detour, &orig, &det, 700, 2_900, detour_len, false, &mut sink).unwrap();
    let src = SliceSource(&sink.buf[..]);
    let idx = RouteIndex::read(&src).expect("a re-spliced route still parses");
    assert_eq!(idx.name(), "Detour · Road trip", "no stacked name prefixes");
    assert_eq!(idx.total_distance_m, stats.total_distance_m);
}

#[test]
fn a_nearest_point_approach_keeps_only_the_tail_and_its_waypoints() {
    let bytes = road_route_obcr();
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    let route = RouteReader::new(&index, &source);
    let join_m = route.total_distance_m / 2;
    let join = route.position_at(join_m).unwrap();
    let leg_bytes = convert(
        "Connection",
        &format!(
            "<gpx><trk><trkseg><trkpt lat=\"{}\" lon=\"{}\"><ele>300</ele></trkpt>\
         <trkpt lat=\"{}\" lon=\"{}\"><ele>300</ele></trkpt></trkseg></trk></gpx>",
            (join.lat + STREET_OFF) as f64 / 1e6,
            join.lon as f64 / 1e6,
            join.lat as f64 / 1e6,
            join.lon as f64 / 1e6,
        ),
    );
    let leg_source = SliceSource(&leg_bytes);
    let leg_index = RouteIndex::read(&leg_source).unwrap();
    let leg = RouteReader::new(&leg_index, &leg_source);
    let mut sink = VecSink::default();
    splice_detour(Leg::Approach, &route, &leg, join_m, join_m, leg.total_distance_m, true, &mut sink).unwrap();
    let source = SliceSource(&sink.buf);
    let index = RouteIndex::read(&source).unwrap();
    let result = RouteReader::new(&index, &source);
    assert_eq!(index.name(), "To route · Road trip");
    assert_eq!(obc_route::splice::original_name(index.name()), "Road trip");
    assert!(!index.has_unresolved_avoidance());
    let expected_m = leg.total_distance_m + route.total_distance_m - join_m;
    assert!(result.total_distance_m.abs_diff(expected_m) <= 3);
    let at_join = result.position_at(leg.total_distance_m).unwrap();
    assert!(obc_map_scene::ground_dist_m((at_join.lon, at_join.lat), (join.lon, join.lat)) < 3.0);
    let original_waypoints = route.load_waypoints(join_m);
    let result_waypoints = result.load_waypoints(0);
    assert!(!original_waypoints.is_empty());
    assert_eq!(result_waypoints.len(), original_waypoints.len());
    for (original, shifted) in original_waypoints.entries.iter().zip(&result_waypoints.entries) {
        assert_eq!(original.name, shifted.name);
        assert!(shifted.dist_along_m.abs_diff(original.dist_along_m - join_m + leg.total_distance_m) <= 3);
    }
}

/// Ride to start: a leg that rejoins at the route start comes first, and the whole route follows
/// it unchanged, with the waypoints moved behind the leg.
#[test]
fn an_approach_splice_is_the_leg_then_the_whole_route() {
    let obcr = road_route_obcr();
    // The leg comes down the parallel street at 300 m, a datum 200 m above the route's start.
    let mut g = String::from("<gpx><trk><trkseg>\n");
    for (lon, lat) in [street_at(4), street_at(2), street_at(0), road_at(0)] {
        let (lat, lon) = (lat as f64 * 1e-6, lon as f64 * 1e-6);
        g.push_str(&format!("  <trkpt lat=\"{lat:.7}\" lon=\"{lon:.7}\"><ele>300.0</ele></trkpt>\n"));
    }
    g.push_str("</trkseg></trk></gpx>");
    let leg = convert("Approach leg", &g);

    let osrc = SliceSource(&obcr[..]);
    let oidx = RouteIndex::read(&osrc).unwrap();
    let orig = RouteReader::new(&oidx, &osrc);
    let lsrc = SliceSource(&leg[..]);
    let lidx = RouteIndex::read(&lsrc).unwrap();
    let det = RouteReader::new(&lidx, &lsrc);
    let (leg_m, route_m) = (det.total_distance_m, orig.total_distance_m);

    let mut trimmed = VecSink::default();
    assert!(
        matches!(trim_detour_to_tail(Leg::Approach, &orig, &det, 0, true, &mut trimmed), Ok(None)),
        "an approach is not trimmed"
    );
    let mut sink = VecSink::default();
    let stats = splice_detour(Leg::Approach, &orig, &det, 0, 0, leg_m, true, &mut sink).unwrap();

    let src = SliceSource(&sink.buf[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let spliced = RouteReader::new(&idx, &src);
    assert_eq!(idx.name(), "To start · Road trip", "the Routes list tells it from the route");
    assert!(!idx.has_unresolved_avoidance(), "an approach avoids nothing");
    assert_eq!(idx.bike_type(), BikeType::Touring, "an approach keeps the route's type, not the leg's");
    assert!(
        stats.total_distance_m.abs_diff(leg_m + route_m) <= 2,
        "length {} is the leg {leg_m} plus the route {route_m}",
        stats.total_distance_m
    );
    let start = spliced.position_at(0).unwrap();
    assert_eq!((start.lon, start.lat), street_at(4), "the ride starts where the rider is");
    for along in [0, 600, route_m] {
        let (a, b) = (orig.position_at(along).unwrap(), spliced.position_at(leg_m + along).unwrap());
        let drift = obc_map_scene::ground_dist_m((a.lon, a.lat), (b.lon, b.lat));
        assert!(drift < 5.0, "route km {along} sits one leg further on ({drift} m off)");
    }

    let along = |bytes: &[u8]| {
        let mut out = Vec::new();
        for_each_waypoint(&SliceSource(bytes), |w| out.push(w.dist_along_m)).unwrap();
        out
    };
    let (before, after) = (along(&obcr), along(&sink.buf));
    assert_eq!(after.len(), before.len(), "every waypoint stays");
    for (b, a) in before.iter().zip(&after) {
        assert!((b + leg_m).abs_diff(*a) <= 6, "waypoint at {b} m moves to {a} m, one leg on");
    }
    let first = route_points(&sink.buf)[0];
    assert!((99..=101).contains(&first.ele), "the leg lands on the start's height (got {} m)", first.ele);
}

/// A day route over `points`, with a height that rises one metre per point from 100 m.
fn day_route(name: &str, points: &[(i32, i32)]) -> Vec<u8> {
    let mut g = String::from("<gpx><trk><trkseg>\n");
    for (i, (lon, lat)) in points.iter().enumerate() {
        let (lat, lon) = (*lat as f64 * 1e-6, *lon as f64 * 1e-6);
        g.push_str(&format!("  <trkpt lat=\"{lat:.7}\" lon=\"{lon:.7}\"><ele>{}.0</ele></trkpt>\n", 100 + i));
    }
    g.push_str("</trkseg></trk></gpx>");
    convert(name, &g)
}

fn length_m(obcr: &[u8]) -> u32 {
    RouteIndex::read(&SliceSource(obcr)).unwrap().total_distance_m
}

/// After an early stop, the next day is the rest of the day before and then the next day. Day 2
/// ends at a camp off the line, and Day 3 comes back from it: the join skips both spurs.
#[test]
fn a_rest_splice_is_the_rest_of_the_day_then_the_next_day_without_the_spur() {
    let camp = (road_at(6).0, BASE.1 + STREET_OFF);
    let line2: Vec<_> = (0..=6).map(road_at).collect();
    let line3: Vec<_> = (6..=SEGS).map(road_at).collect();
    let day2 = day_route("Day 2 Ulrichen", &[&line2[..], &[camp]].concat());
    let mut day3 = day_route("Day 3 Brig", &[&[camp][..], &line3].concat());
    day3[obc_formats::obcr::BIKE_TYPE_OFF] = BikeType::Gravel as u8;
    let leave_m = length_m(&day_route("line", &line2));
    let join_m = length_m(&day3) - length_m(&day_route("line", &line3));
    let from_m = 700;

    let (src2, src3) = (SliceSource(&day2[..]), SliceSource(&day3[..]));
    let (idx2, idx3) = (RouteIndex::read(&src2).unwrap(), RouteIndex::read(&src3).unwrap());
    let (rest, next) = (RouteReader::new(&idx2, &src2), RouteReader::new(&idx3, &src3));
    let mut sink = VecSink::default();
    let leg = Leg::Rest { from_m, to_m: leave_m };
    let stats = splice_detour(leg, &next, &rest, 0, join_m, 0, true, &mut sink).unwrap();

    let src = SliceSource(&sink.buf[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let joined = RouteReader::new(&idx, &src);
    assert_eq!(idx.name(), "From stop · Day 3 Brig", "the built day cannot pass for the day");
    let info = obc_route::RouteObjectInfo::read(&src).unwrap();
    assert!(!info.assistant_candidate);
    assert_ne!(sink.buf[5] & obc_formats::obcr::FLAG_BUILT_DAY, 0, "it is marked as a built day");
    assert_eq!(idx.bike_type(), BikeType::Gravel, "and the day's bike type");
    assert!(!idx.has_unresolved_avoidance(), "a rest avoids nothing");
    let want = (leave_m - from_m) + (next.total_distance_m - join_m);
    assert!(stats.total_distance_m.abs_diff(want) <= 4, "length {} is rest plus day, {want}", stats.total_distance_m);
    let (start, stop) = (joined.position_at(0).unwrap(), rest.position_at(from_m).unwrap());
    assert!(obc_map_scene::ground_dist_m((start.lon, start.lat), (stop.lon, stop.lat)) < 2.0, "it starts at the stop");
    let points = route_points(&sink.buf);
    assert!(points.iter().all(|p| p.lat == BASE.1), "the route stays on the line and skips the camp");
    assert_eq!(points[1].ele, 103, "the stored heights stay as they are");
}

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
    let out = trim_detour_to_tail(Leg::Detour, &orig, &det, target_m, detour_has_elevation, &mut sink).unwrap();
    (out, sink.buf)
}

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
    splice_detour(Leg::Detour, &orig, &det, split_m, rejoin_m, detour_len_m, detour_has_elevation, &mut sink)
        .unwrap()
        .total_distance_m
}

/// With connectors only at the road's ends, a plan to a mid-route rejoin must overshoot to road12
/// up the parallel street and ride the route tail back down to reach the goal. The trim advances
/// the rejoin to that first tail contact and removes the retrace.
#[test]
fn trim_rejoins_at_first_tail_contact_and_removes_the_retrace() {
    let bytes = map_with(&road_graph(true, false));
    let obcr = road_route_obcr();
    let target = 2_500;
    let (res, detour) = detour_over(&bytes, &obcr, 0, target);
    let dstats = res.expect("the street detour plans");

    // Non-vacuity: the untrimmed plan overshoots to road12, then descends the tail to node9.
    let untrimmed = route_points(&detour);
    assert!(
        untrimmed.iter().any(|p| (p.lon, p.lat) == road_at(SEGS)),
        "the untrimmed plan overshoots to road12 (the ring)"
    );
    let last = untrimmed.last().unwrap();
    assert_eq!(
        (last.lon, last.lat),
        route_coord_at(&obcr, target),
        "…then descends the tail to land at the exact requested projection near node9"
    );

    let mut detour = detour;
    detour[obc_formats::obcr::BIKE_TYPE_OFF] = BikeType::Gravel as u8;
    let (out, trimmed) = trim_run(&obcr, &detour, target, dstats.has_elevation);
    let out = out.expect("the retrace is trimmed");
    assert!(out.rejoin_m > target + 500, "rejoin advances toward the road end (got {})", out.rejoin_m);
    let tsrc = SliceSource(&trimmed[..]);
    assert_eq!(RouteIndex::read(&tsrc).unwrap().bike_type(), BikeType::Gravel, "the trim keeps the detour's type");

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

    let untrimmed_total = spliced_total(&obcr, &detour, 0, target, dstats.total_distance_m, dstats.has_elevation);
    let trimmed_total = spliced_total(&obcr, &trimmed, 0, out.rejoin_m, out.detour_len_m, dstats.has_elevation);
    assert!(
        untrimmed_total >= trimmed_total + 1_000,
        "the trimmed splice drops ≥1 km (untrimmed {untrimmed_total}, trimmed {trimmed_total})"
    );
}

/// Every plan hugs the tail near the goal, so a landing that touches the tail only at its final
/// pair must not trim. The detour is hand-built, so the landing geometry is exact.
#[test]
fn trim_is_a_noop_for_a_normal_landing() {
    let obcr = road_route_obcr();
    // node6 is ~1 670 m along. The detour lands there and rides one segment forward.
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

/// A detour that only crosses the route tail must not trim. Contact needs both points of a pair
/// near the tail, so a crossing or a bridge overpass never triggers.
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
    splice_detour(Leg::Detour, &orig, &det, 600, total, dstats.total_distance_m, dstats.has_elevation, &mut sink)
        .unwrap();
    let src = SliceSource(&sink.buf[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let pts = route_points(&sink.buf);
    let end = pts.last().unwrap();
    let goal = orig.position_at(total).unwrap();
    assert_eq!(
        (end.lon, end.lat),
        (goal.lon, goal.lat),
        "ends at the detour's exact goal projection — the tail is empty"
    );
    let mut names: Vec<String> = Vec::new();
    for_each_waypoint(&src, |w| names.push(w.name.as_str().into())).unwrap();
    assert_eq!(names, ["W-head"], "span-to-end drops the mid and tail waypoints");
    let component_total = 600 + dstats.total_distance_m;
    assert!(
        idx.total_distance_m.abs_diff(component_total) <= 1,
        "spliced {} differs from the independently rounded 600 m head + {} m detour",
        idx.total_distance_m,
        dstats.total_distance_m
    );
}

// `plan_detour` is `plan_route` plus a corridor blacklist and shares its edge cost. The two tests
// below stop a future change from forking the cost model.

/// A steep north-facing hillside: 1 m of rise per 5 µdeg of latitude above the road, flat at or
/// below it. The north relief street then sits [`NORTH_CLIMB_M`] above the road and the south one
/// is flat, which is the only difference the climb term can see.
struct Hillside;

/// What the north connector climbs: `STREET_OFF / 5`.
const NORTH_CLIMB_M: u32 = 720;
/// One segment of the longer, flat south relief street, m. Chosen so the south corridor costs
/// 3 000 m more ground than the north one, three times the ε inflation the frontier carries.
const SOUTH_SEG_COST: u32 = 530;

impl obc_route::ElevationSource for Hillside {
    fn sample(&mut self, lat_udeg: i32, _lon_udeg: i32) -> Option<i16> {
        Some(((lat_udeg - BASE.1).max(0) / 5) as i16)
    }
}

/// [`map_with`] with an explicit profile and a terrain to bake the edge ascents from.
fn map_with_terrain(graph: &NavGraph, profile: NavProfile, terrain: &mut dyn obc_route::ElevationSource) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, &[profile], terrain);
    assert_eq!(dropped, 0);
    bin
}

fn climb_profile(climb_weight: u8) -> NavProfile {
    NavProfile { name: "Climb".into(), highway: [16; 32], surface: [16; 8], climb_weight }
}

fn south_at(i: i32) -> (i32, i32) {
    (BASE.0 + i * SP, BASE.1 - STREET_OFF)
}

/// The road with two relief corridors: the street north of it (short, but it climbs
/// [`NORTH_CLIMB_M`] up the [`Hillside`]) and a mirror street south of it (flat, but 3 000 m more
/// ground). With the road blacklisted, only the climb term can choose between them.
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

/// Plan `from` to `to`, with or without a corridor blacklist. Every other argument is shared, so a
/// difference in the output can only come from the corridor.
fn plan_either(bytes: &[u8], from: (i32, i32), to: (i32, i32), corridor: Option<Corridor>) -> (u32, Vec<u8>) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = match corridor {
        Some(c) => {
            plan_detour(&r, from, to, "Leg", BikeType::Road, c, &mut scratch, &mut tiles, &mut NullElevation, &mut sink)
        }
        None => {
            plan_route(&r, from, to, "Leg", BikeType::Road, &mut scratch, &mut tiles, &mut NullElevation, &mut sink)
        }
    };
    (res.expect("the fixture always has a legal path").total_distance_m, sink.buf)
}

#[test]
fn a_detour_weighs_climb_the_same_way_a_plan_does() {
    let graph = road_graph_two_reliefs();
    let obcr = road_route_obcr();

    // Non-vacuity: the north connector bakes the climb this test spends, and only uphill.
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
    assert_eq!(dist, 4140, "climb-blind, the detour takes the short north street");

    let weighted = map_with_terrain(&graph, climb_profile(20), &mut Hillside);
    let (dist, _) = plan_either(&weighted, road_at(0), road_at(SEGS), Some(Corridor::build(&route, 0, total)));
    assert_eq!(dist, 4140, "at a heavy climb weight it detours south, onto the flat");
}

/// The corridor is the only thing the detour dispatch adds. Here the map's ascents are real and
/// the profile charges 20 flat metres for each, so the two paths must agree on a cost the climb
/// term dominates.
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
    // The shared answer is the road itself: flat, and shorter than either relief.
    assert_eq!(plan_len, 3339, "the road is the cheapest way when nothing blocks it");
}

// `plan_detour` samples the map's terrain at every emitted vertex. The splice keeps those heights
// and removes only the datum mismatch at the two joins, with a linear blend of the seam residuals.

/// The [`Hillside`] shifted up by a constant: a terrain whose datum disagrees with the fixture
/// route's own `<ele>` ramp everywhere, which is the GPX-imported case.
const DEM_OFFSET_M: i16 = 300;

struct OffsetHillside;

impl obc_route::ElevationSource for OffsetHillside {
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        Hillside.sample(lat_udeg, lon_udeg).map(|h| h + DEM_OFFSET_M)
    }
}

/// The mid-span (600 to 2 800 m) detour planned over `elev`, spliced into the road route.
fn spliced_over(elev: &mut dyn obc_route::ElevationSource) -> (Vec<u8>, obc_route::RouteStats, obc_route::RouteStats) {
    spliced_span(600, 2_800, elev)
}

/// The whole-route splice: head and tail are both empty, so the spliced point stream is the
/// blended detour and the seams are the original's first and last heights.
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
    let bytes = map_with_terrain(&road_graph(true, false), neutral_profile(), &mut Hillside);
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
    let stats = splice_detour(
        Leg::Detour,
        &orig,
        &det,
        split_m,
        rejoin_m,
        dstats.total_distance_m,
        dstats.has_elevation,
        &mut sink,
    )
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

fn whole_route_seams() -> (i16, i16) {
    let obcr = road_route_obcr();
    let src = SliceSource(&obcr[..]);
    let idx = RouteIndex::read(&src).unwrap();
    let total = RouteReader::new(&idx, &src).total_distance_m;
    road_route_seams(0, total)
}

#[test]
fn splice_without_detour_elevation_keeps_the_seam_lerp() {
    let (spliced, _, _) = spliced_road();
    let bytes = map_with(&road_graph(true, false));
    let (res, _) = detour_over(&bytes, &road_route_obcr(), 600, 2_800);
    assert!(!res.unwrap().has_elevation, "a NullElevation plan must report no elevation — the fixture's premise");

    let source = SliceSource(&spliced);
    let index = RouteIndex::read(&source).unwrap();
    let reader = RouteReader::new(&index, &source);
    assert!(!reader.interval_facts(0, reader.total_distance_m).unwrap().complete_elevation());
    let (whole, _, _) = spliced_whole(&mut NullElevation);
    assert!(route_points(&whole).iter().all(|p| p.elevation().is_none()));
}

#[test]
fn splice_keeps_the_detour_sampled_shape_and_matches_both_seams() {
    let (spliced, stats, dstats) = spliced_whole(&mut Hillside);
    assert!(dstats.has_elevation, "the terrain answered for the plan");
    assert!(stats.has_elevation, "…so the spliced route carries elevation too");

    let (lo, hi) = whole_route_seams();
    let eles: Vec<i16> = route_points(&spliced).iter().map(|p| p.ele).collect();
    assert_eq!(*eles.first().unwrap(), lo, "no step at the split seam");
    assert_eq!(*eles.last().unwrap(), hi, "no step at the rejoin seam");

    // The interior is the terrain's, not a ramp: the street peaks far outside the seam interval
    // that a lerp could never leave.
    let peak = *eles.iter().max().unwrap();
    assert!(
        peak > hi + NORTH_CLIMB_M as i16 / 2,
        "the spliced detour must carry the street's real height (peak {peak}, seams {lo}..{hi})"
    );
    assert!(
        (peak as i32 - (NORTH_CLIMB_M as i32 + hi as i32)).abs() < 150,
        "…and it must be the sampled hump plus the blended residual, not something invented (peak {peak})"
    );

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

/// A constant DEM datum offset moves both seam residuals by the same constant, so the blend
/// absorbs it exactly and the spliced span is height-for-height what the un-offset terrain gave.
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

/// The header must be the climb recomputed over the final point stream, not a sum of the two
/// producers' totals.
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
            if p.elevation().is_none() {
                band.pause();
            } else {
                band.push(f64::from(p.ele));
                lo = lo.min(p.ele);
                hi = hi.max(p.ele);
            }
        }
        assert_eq!(stats.total_ascent_m, band.ascent() as u32, "{label}: header ascent is the stream's");
        assert_eq!(stats.total_descent_m, band.descent() as u32, "{label}: header descent is the stream's");
        assert_eq!((stats.min_ele_m, stats.max_ele_m), (lo, hi), "{label}: header min/max are the stream's");
    }

    // Non-vacuity: the terrain case must exercise the descent arm as well.
    let terrain = &cases[1].2;
    assert!(terrain.total_ascent_m > NORTH_CLIMB_M / 2, "the hump is climbed (got {})", terrain.total_ascent_m);
    assert!(terrain.total_descent_m > NORTH_CLIMB_M / 2, "…and descended (got {})", terrain.total_descent_m);
    // The elevation-less splice books the ramp only.
    assert_eq!(cases[0].2.total_descent_m, 0);
    assert!((20..=40).contains(&cases[0].2.total_ascent_m));
}

/// A trimmed detour must reach the splice with its sampled heights and its own climb, or the
/// residual blend has nothing to blend onto and the preview has nothing to price.
#[test]
fn a_trimmed_detour_keeps_its_sampled_heights_and_reports_its_climb() {
    let bytes = map_with_terrain(&road_graph(true, false), neutral_profile(), &mut Hillside);
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

    let (res, detour) = detour_over(&bytes, &obcr, 0, target);
    let (_, trimmed) = trim_run(&obcr, &detour, target, res.unwrap().has_elevation);
    assert!(route_points(&trimmed).iter().all(|p| p.elevation().is_none()), "no elevation in, no elevation out");
}

/// `has_elevation` is the producer's own answer end to end, never a look at the values. A
/// sea-level route is not an elevation-less one.
#[test]
fn has_elevation_is_the_producers_answer_not_the_values() {
    // A GPX with no `<ele>` and the same track at a constant 0 m store the same height values, so
    // the values alone cannot tell them apart. The converter can.
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
    assert_ne!(none, sea, "stored validity distinguishes sea level from missing elevation");

    // The reader has only the bytes, so it gives the weaker honest answer for both.
    let src = SliceSource(&sea[..]);
    let idx = RouteIndex::read(&src).unwrap();
    assert!(RouteReader::new(&idx, &src).has_elevation(), "a stored file has no better answer than its header");

    // The producer watched the parse and tells them apart, which is why the bit is threaded from
    // the plan to the splice instead of re-derived there.
    let mut sink = VecSink::default();
    assert!(obc_route::gpx_to_obcr(&SliceSource(track(Some(0.0)).as_bytes()), "n", &mut sink).unwrap().has_elevation);
    let mut sink = VecSink::default();
    assert!(!obc_route::gpx_to_obcr(&SliceSource(track(None).as_bytes()), "n", &mut sink).unwrap().has_elevation);
}
