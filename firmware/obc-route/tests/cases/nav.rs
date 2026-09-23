//! Host tests for the A* router. Fixture graphs are serialized with the real `obc-pack` writer and
//! parsed with the real `obc-reader`, so the router runs end to end over genuine on-wire bytes.

use crate::common::nav::{digest, plan_p};

use crate::common::{decode, route_points, VecSink};
use obc_elevation::NullElevation;
use obc_formats::io::SliceSource;
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::nav::{plan_route, NavError, NavPhase, NavPlanner, NavScratch};
use obc_route::{BikeType, RouteIndex, RouteObjectInfo, RouteReader};

/// Global bbox, microdegrees. Roomy, so the node quadtree subdivides around the fixtures and the
/// tile cache is exercised.
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 1_000_000, 1_000_000);

/// Grid spacing, microdegrees (~1 113 m of latitude; near the equator both axes match).
const SP: i32 = 10_000;
const BASE: (i32, i32) = (500_000, 500_000);
/// Every grid edge's cost, meters. Above the ~1 113 m straight line between adjacent nodes, so the
/// heuristic stays admissible and every monotone corner-to-corner path costs `4 * EDGE_COST`.
const EDGE_COST: u32 = 1_200;
/// Above its ~3 148 m straight line, and below the 4 800 m grid alternative, so it is the unique
/// optimum when present.
const SHORTCUT_COST: u32 = 3_200;

/// An all-1.0x profile. The weighted search then reduces to an unweighted A*, so the
/// kind-agnostic fixtures below read as plain distance.
fn neutral_profile() -> NavProfile {
    NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }
}

/// A test profile from highway-class overrides, each `(class, 1/16 multiplier)`. Fixture edges
/// carry `kind = highway_class` with surface class 0, so the effective multiplier is the listed
/// byte.
fn profile(name: &str, overrides: &[(u8, u8)]) -> NavProfile {
    let mut highway = [16u8; 32];
    for &(class, m) in overrides {
        highway[class as usize] = m;
    }
    NavProfile { name: name.into(), highway, surface: [16; 8], climb_weight: 0 }
}

/// Serialize `graph` into a minimal map with the given routing profiles. At least one profile must
/// be present.
fn map_with_profiles(graph: &NavGraph, profiles: &[NavProfile]) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, profiles, &mut NullElevation);
    assert_eq!(dropped, 0);
    bin
}

/// Serialize `graph` under a single neutral profile.
fn map_with(graph: &NavGraph) -> Vec<u8> {
    map_with_profiles(graph, &[neutral_profile()])
}

fn at(row: i32, col: i32) -> (i32, i32) {
    (BASE.0 + col * SP, BASE.1 + row * SP)
}

/// A 3x3 grid (node id = `row*3 + col`) with orthogonal edges. Each edge has one interior shape
/// point nudged off-axis, so the geometry survives the OBCR decimator. `shortcut` adds a diagonal
/// edge `0 -> 8` that beats every grid path.
fn grid3(shortcut: bool) -> NavGraph {
    let mut nodes = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            nodes.push(Node { id: (row * 3 + col) as u32, coord: at(row, col) });
        }
    }
    let mut edges = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            let a = (row * 3 + col) as u32;
            if col < 2 {
                let (ca, cb) = (at(row, col), at(row, col + 1));
                let mid = ((ca.0 + cb.0) / 2, ca.1 + 500); // nudge lat
                edges.push(Edge { a, b: a + 1, polyline: vec![ca, mid, cb], length_m: EDGE_COST, kind: 0 });
            }
            if row < 2 {
                let (ca, cb) = (at(row, col), at(row + 1, col));
                let mid = (ca.0 + 500, (ca.1 + cb.1) / 2); // nudge lon
                edges.push(Edge { a, b: a + 3, polyline: vec![ca, mid, cb], length_m: EDGE_COST, kind: 0 });
            }
        }
    }
    if shortcut {
        let (ca, cb) = (at(0, 0), at(2, 2));
        let mid = ((ca.0 + cb.0) / 2 + 500, (ca.1 + cb.1) / 2 - 500);
        edges.push(Edge { a: 0, b: 8, polyline: vec![ca, mid, cb], length_m: SHORTCUT_COST, kind: 0 });
    }
    NavGraph { nodes, edges }
}

/// A straight west-to-east path graph of `n` nodes. Reaching the far end forces the router to
/// track every node on the line, which makes it the deterministic range fixture.
fn line_graph(n: u32, step_udeg: i32, cost_m: u32) -> NavGraph {
    let nodes = (0..n).map(|i| Node { id: i, coord: (BASE.0 + i as i32 * step_udeg, BASE.1) }).collect::<Vec<_>>();
    let edges = (0..n - 1)
        .map(|i| {
            let (ca, cb) = (nodes[i as usize].coord, nodes[i as usize + 1].coord);
            Edge { a: i, b: i + 1, polyline: vec![ca, cb], length_m: cost_m, kind: 0 }
        })
        .collect();
    NavGraph { nodes, edges }
}

fn plan(
    bytes: &[u8],
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
) -> (Result<obc_route::RouteStats, NavError>, Vec<u8>, obc_reader::NavCacheStats) {
    plan_p(bytes, from, to, name, BikeType::Road)
}

/// The optimum is `4 * EDGE_COST` whatever monotone path A* picks, so the expectation does not
/// depend on the path shape.
#[test]
fn grid_route_matches_known_optimum_and_round_trips() {
    let bytes = map_with(&grid3(false));
    let from = at(0, 0);
    let goal = at(2, 2);
    let to = goal;
    let (res, obcr, stats) = plan(&bytes, from, to, "Water stop");
    let route = res.expect("a grid route plans");

    assert_eq!(route.total_distance_m, 4474, "summed edge costs, the known optimum");
    assert_eq!(route.point_count, 9, "5 nodes + 4 nudged midpoints survive the decimator");
    assert_eq!((route.total_ascent_m, route.total_descent_m), (0, 0), "no DEM — flat by construction");
    assert_eq!((route.min_ele_m, route.max_ele_m), (0, 0));
    assert_eq!(route.waypoint_count, 0);
    assert!(stats.hits > 0, "consecutive settles re-hit resident graph tiles (got {stats:?})");
    assert!(stats.misses > 0, "the graph was actually read (got {stats:?})");

    let src = SliceSource(&obcr);
    let idx = RouteIndex::read(&src).expect("round trip");
    assert_eq!(idx.name(), "Water stop");
    assert_ne!(obcr[5] & obc_formats::obcr::FLAG_TEMPORARY, 0);
    let info = RouteObjectInfo::read(&src).unwrap();
    assert_eq!(info.distance_m, 4474, "header length = summed edge costs");
    assert_eq!(info.ascent_m, 0);

    let pts = route_points(&obcr);
    assert_eq!(pts.len(), 9);
    assert_eq!((pts[0].lon, pts[0].lat), at(0, 0), "starts at the snapped start node");
    assert_eq!((pts[8].lon, pts[8].lat), goal, "ends at the snapped goal node");
    assert!(pts.iter().all(|p| p.elevation().is_none()), "every point is elevation-none (stored 0)");
}

/// Routed backwards, the shortcut edge is traversed `b -> a`, which pins the reversed geometry
/// decode.
#[test]
fn shortcut_wins_and_reversed_edge_geometry_is_exact() {
    let bytes = map_with(&grid3(true));
    let (c0, c8) = (at(0, 0), at(2, 2));
    let mid = ((c0.0 + c8.0) / 2 + 500, (c0.1 + c8.1) / 2 - 500);

    let (res, obcr, _) = plan(&bytes, c0, c8, "Fwd");
    assert_eq!(res.unwrap().total_distance_m, 3152, "the shortcut is the unique optimum");
    let pts = route_points(&obcr);
    assert_eq!(
        pts.iter().map(|p| (p.lon, p.lat)).collect::<Vec<_>>(),
        vec![c0, mid, c8],
        "forward traversal keeps record order"
    );

    let (res, obcr, _) = plan(&bytes, c8, c0, "Rev");
    assert_eq!(res.unwrap().total_distance_m, 3152);
    let pts = route_points(&obcr);
    assert_eq!(
        pts.iter().map(|p| (p.lon, p.lat)).collect::<Vec<_>>(),
        vec![c8, mid, c0],
        "b→a traversal emits the polyline reversed"
    );
}

/// Two components in snap range of the endpoints but not of each other.
#[test]
fn disconnected_graph_is_no_path() {
    let a0 = (500_000, 500_000);
    let a1 = (505_000, 500_000);
    let b0 = (550_000, 500_000); // ~5 km east of component A
    let b1 = (555_000, 500_000);
    let graph = NavGraph {
        nodes: vec![
            Node { id: 0, coord: a0 },
            Node { id: 1, coord: a1 },
            Node { id: 2, coord: b0 },
            Node { id: 3, coord: b1 },
        ],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a0, a1], length_m: 600, kind: 0 },
            Edge { a: 2, b: 3, polyline: vec![b0, b1], length_m: 600, kind: 0 },
        ],
    };
    let (res, obcr, _) = plan(&map_with(&graph), a0, b0, "x");
    assert_eq!(res, Err(NavError::NoPath));
    assert!(obcr.is_empty(), "a failed plan writes nothing");
}

/// A 4-entry scratch cannot track the 5 or more nodes a corner-to-corner grid path needs.
#[test]
fn tiny_scratch_exhausts() {
    let bytes = map_with(&grid3(false));
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<4>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let goal = at(2, 2);
    let res =
        plan_route(&r, at(0, 0), goal, "x", BikeType::Road, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted));
}

/// A goal discovered before the table fills is still reached when a later insert fails. The start
/// has two ways to the goal: one expensive direct edge, tracked at the first settle, and a cheap
/// chain. A* settles the chain, fills the table mid-chain, and the goal then pops on the direct
/// edge. That route exceeds the rung's bound, which is the accepted full-table outcome.
#[test]
fn goal_tracked_before_fill_survives_exhaustion() {
    // 9 chain nodes west to east; the goal is node 8.
    let step = 3_000;
    let coord = |i: u32| (BASE.0 + i as i32 * step, BASE.1);
    let mut nodes: Vec<Node> = (0..9).map(|i| Node { id: i, coord: coord(i) }).collect();
    let mut edges: Vec<Edge> = (0..8)
        .map(|i| Edge { a: i, b: i + 1, polyline: vec![coord(i), coord(i + 1)], length_m: 400, kind: 0 })
        .collect();
    // The expensive direct escape: 20 km for a ~1.8 km straight line, still admissible.
    edges.push(Edge { a: 0, b: 8, polyline: vec![coord(0), coord(8)], length_m: 20_000, kind: 0 });
    let _ = &mut nodes;
    let bytes = map_with(&NavGraph { nodes, edges });

    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    // A 6-node table fills at node 4, before the chain reaches node 5, so the goal never relaxes
    // through the chain.
    let mut scratch = NavScratch::<6>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let (from, to) = (coord(0), coord(8));
    let res =
        plan_route(&r, from, to, "Salvaged", BikeType::Road, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
    let route = res.expect("the goal was tracked before the fill ⇒ salvage returns it");
    assert_eq!(route.total_distance_m, 2671, "the direct edge, the full-table path");
    assert_eq!(route_points(&sink.buf).len(), 2, "start → goal over the single direct edge");
}

/// An endpoint with no routable edge within the snap radius fails to snap. The wider internal
/// lookup square finds nodes and anchors only.
#[test]
fn unsnappable_endpoint_is_no_path() {
    let bytes = map_with(&grid3(false));
    let near0 = at(0, 0);
    // ~4.4 km from the nearest road, well past the acceptance radius.
    let lost = (BASE.0 + 60_000, BASE.1 + 60_000);
    let (res, _, _) = plan(&bytes, lost, near0, "x");
    assert_eq!(res, Err(NavError::NoPath), "`from` out of snap range");
    let (res, _, _) = plan(&bytes, near0, lost, "x");
    assert_eq!(res, Err(NavError::NoPath), "`to` out of snap range");
}

/// A rider beside the middle of a kilometre-long road is more than 250 m from either graph node.
/// The route must still start at the perpendicular projection, not at the anchor or a junction.
#[test]
fn sparse_anchor_discovers_long_road_for_exact_projection() {
    let a = BASE;
    let b = (BASE.0 + SP, BASE.1);
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 1_113, kind: 0 }],
    };
    let bytes = map_with(&graph);
    // Deliberately not on an anchor: the anchor only finds the edge, the projection uses the
    // complete polyline.
    let along = SP * 43 / 100;
    let rider = (BASE.0 + along, BASE.1 + 400); // ~44 m north, 479 m from the nearer endpoint
    let (result, obcr, _) = plan(&bytes, rider, b, "Mid-edge");
    let route = result.expect("the sparse anchor finds the long road");
    assert_eq!(route.total_distance_m, 634, "only the edge after the projection is traversed");
    let points = route_points(&obcr);
    assert_eq!(points.first().map(|p| (p.lon, p.lat)), Some((BASE.0 + along, BASE.1)));
    assert_eq!(points.last().map(|p| (p.lon, p.lat)), Some(b));
    assert_ne!(points[0].lat, rider.1, "the route begins on the road, not at the off-road GPS fix");
}

/// Exact projection is the normal path, not a fallback after node snapping fails: a rider starts
/// abreast of the road even with a junction 49 m away. This edge is below the anchor threshold, so
/// discovery comes from its endpoints.
#[test]
fn near_a_node_still_starts_at_the_exact_road_projection() {
    let a = BASE;
    let b = (BASE.0 + 2_000, BASE.1); // ~223 m, so no interior lookup anchor
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 223, kind: 0 }],
    };
    let rider = (BASE.0 + 400, BASE.1 + 180); // ~20 m off-road and ~49 m from node a
    let (result, obcr, _) = plan(&map_with(&graph), rider, b, "Near node");
    let route = result.expect("endpoint discovery finds the short edge");
    assert_eq!(route.total_distance_m, 178);
    let points = route_points(&obcr);
    assert_eq!(points.first().map(|p| (p.lon, p.lat)), Some((BASE.0 + 400, BASE.1)));
    assert_eq!(points.last().map(|p| (p.lon, p.lat)), Some(b));
}

/// Both virtual endpoints can sit inside one curving edge. The route clips at both projections,
/// keeps the bends between them, and takes neither graph endpoint.
#[test]
fn both_endpoints_project_and_clip_inside_one_curving_edge() {
    let a = BASE;
    let bend_a = (BASE.0 + 4_000, BASE.1);
    let bend_b = (BASE.0 + 4_000, BASE.1 + 4_000);
    let b = (BASE.0 + 8_000, BASE.1 + 4_000);
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, bend_a, bend_b, b], length_m: 1_335, kind: 0 }],
    };
    let from = (BASE.0 + 2_000, BASE.1 - 250);
    let to = (BASE.0 + 6_000, BASE.1 + 4_250);
    let (result, obcr, _) = plan(&map_with(&graph), from, to, "Curved");
    let route = result.expect("sparse anchors discover the curved edge from both ends");
    assert_eq!(route.total_distance_m, 890, "only two thirds of the rounded wire length is traversed");
    assert_eq!(
        route_points(&obcr).iter().map(|p| (p.lon, p.lat)).collect::<Vec<_>>(),
        vec![(BASE.0 + 2_000, BASE.1), bend_a, bend_b, (BASE.0 + 6_000, BASE.1 + 4_000)]
    );
}

/// A map with an empty nav section can snap nothing.
#[test]
fn empty_graph_is_no_path() {
    let bytes = map_with(&NavGraph::default());
    let (res, _, _) = plan(&bytes, (500_000, 500_000), (505_000, 500_000), "x");
    assert_eq!(res, Err(NavError::NoPath));
}

/// There is no distance cap: a far target is attempted and fails by exhausting the table. Every
/// node on this ~30 km line must be tracked, so the table fills long before the goal.
#[test]
fn far_beyond_range_target_exhausts_instead_of_precheck() {
    let bytes = map_with(&line_graph(2000, 135, 15)); // ~30 km end to end
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let from = BASE;
    let to = (BASE.0 + 1_999 * 135, BASE.1);
    let res = plan_route(&r, from, to, "x", BikeType::Road, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted), "the search ends at the table, not at a pre-check");
    assert!(sink.buf.is_empty(), "an exhausted plan writes nothing");
}

/// Two identical positions give a single-point route that is still a valid OBCR.
#[test]
fn same_snap_node_emits_single_point_route() {
    let bytes = map_with(&grid3(false));
    let (res, obcr, _) = plan(&bytes, at(0, 0), at(0, 0), "Here");
    let route = res.expect("a degenerate route still plans");
    assert_eq!(route.total_distance_m, 0);
    assert_eq!(route.point_count, 1);
    let pts = route_points(&obcr);
    assert_eq!(pts.len(), 1);
    assert_eq!((pts[0].lon, pts[0].lat), at(0, 0));
}

/// A model of the device slot lifecycle: a `static mut MaybeUninit<NavPlanner>` written per
/// request and stepped the way `ride.rs` does, including overwrite without drop and the cancel
/// interleaving. Under Miri this proves the pattern free of aliasing and uninitialized reads.
#[test]
fn device_slot_lifecycle_is_uninit_and_alias_clean() {
    use core::mem::MaybeUninit;
    use obc_route::nav::NAV_MAX_NODES;

    struct DeviceNav {
        scratch: &'static mut NavScratch<NAV_MAX_NODES>,
        tiles: &'static mut NavTileCache,
        planner: &'static mut MaybeUninit<NavPlanner>,
    }
    static mut SCRATCH: NavScratch<NAV_MAX_NODES> = NavScratch::new();
    static mut TILES: NavTileCache = NavTileCache::new();
    static mut PLANNER: MaybeUninit<NavPlanner> = MaybeUninit::uninit();
    // SAFETY: this test is the statics' only user (test-local names), each borrowed exactly once.
    let nav = DeviceNav {
        scratch: unsafe { &mut *core::ptr::addr_of_mut!(SCRATCH) },
        tiles: unsafe { &mut *core::ptr::addr_of_mut!(TILES) },
        planner: unsafe { &mut *core::ptr::addr_of_mut!(PLANNER) },
    };
    let bytes = map_with(&grid3(false));
    let tables_src = SliceSource(&bytes);
    let tables = MapTables::parse(&tables_src).unwrap();
    let cache = MapCache::new();
    let from = at(0, 0);
    let goal = at(2, 2);
    let to = goal;

    // One device-shaped step: a fresh source, reader and sink view, a phase read, then the step
    // with the planner, scratch and tiles borrows all live across the call.
    fn step_once(
        nav: &mut DeviceNav,
        bytes: &[u8],
        tables: &MapTables,
        cache: &MapCache,
        sink: &mut VecSink,
    ) -> obc_route::Step {
        // SAFETY (as on device): only called while a plan is active — the slot was written.
        let _phase = unsafe { nav.planner.assume_init_ref() }.phase();
        let src = SliceSource(bytes);
        let reader = Reader::new(&src, tables, cache);
        let planner = unsafe { nav.planner.assume_init_mut() };
        planner.step(&reader, &mut *nav.scratch, &mut *nav.tiles, &mut NullElevation, sink)
    }
    let mut nav = nav;

    // Write the slot and step to completion.
    nav.planner.write(NavPlanner::new(from, to, "Dev", BikeType::Road));
    let mut sink = VecSink::default();
    let stats = loop {
        match step_once(&mut nav, &bytes, &tables, &cache, &mut sink) {
            obc_route::Step::Running => {}
            obc_route::Step::Done(stats) => break stats,
            obc_route::Step::Failed(e) => panic!("device-model plan failed: {e:?}"),
        }
    };
    assert_eq!(stats.total_distance_m, 4474);
    let first = sink.buf.clone();

    // A second request is cancelled mid-search: the slot is overwritten without a drop, stepped
    // twice, then never stepped again.
    nav.planner.write(NavPlanner::new(from, to, "Cancelled", BikeType::Road));
    let mut cancelled_sink = VecSink::default();
    for _ in 0..2 {
        assert!(matches!(step_once(&mut nav, &bytes, &tables, &cache, &mut cancelled_sink), obc_route::Step::Running));
    }
    assert!(cancelled_sink.buf.is_empty(), "a cancelled (abandoned) plan wrote nothing");

    // A third request replaces the abandoned plan and must emit the first request's bytes.
    nav.planner.write(NavPlanner::new(from, to, "Dev", BikeType::Road));
    let mut sink3 = VecSink::default();
    loop {
        match step_once(&mut nav, &bytes, &tables, &cache, &mut sink3) {
            obc_route::Step::Running => {}
            obc_route::Step::Done(_) => break,
            obc_route::Step::Failed(e) => panic!("replacement plan failed: {e:?}"),
        }
    }
    assert_eq!(sink3.buf, first, "a slot overwrite leaks no state between plans");
}

/// The record head is odd-length, so the first neighbor entry of every record begins at an odd
/// offset and its multi-byte fields decode unaligned. This keeps the byte-wise decode contract
/// exercised. The full UB tripwire is Miri over this suite.
#[test]
fn record_stride_keeps_odd_offsets_exercised() {
    assert_eq!(obc_formats::obcm::NAV_NODE_FIXED_LEN % 2, 1, "the fixed record head is odd-length");
    assert_eq!(obc_formats::obcm::NAV_NEIGHBOR_LEN, 17, "v12 neighbor entries are 17 bytes");
    // The neighbor entry length is odd too, so consecutive record starts keep varying parity and
    // record heads decode at odd offsets in multi-record chunks as well.
}

/// The 26 B/node layout holds: a 24 B entry plus a 2 B heap slot, plus the two length fields.
#[test]
fn scratch_fits_the_per_target_budget() {
    let size = core::mem::size_of::<NavScratch<{ obc_route::NAV_MAX_NODES }>>();
    assert!(size <= 26 * obc_route::NAV_MAX_NODES + 8, "NavScratch is {size} B — the 26 B/node layout drifted");
}

/// Every node on this ~9 km line must be tracked to reach the goal, so a 300-node table exhausts
/// and the shipped 1536-node table plans it. This pins the table size against the planning range.
#[test]
fn long_line_exhausts_old_table_but_plans_on_the_sim_table() {
    let bytes = map_with(&line_graph(600, 135, 15));
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let from = BASE;
    let to = (BASE.0 + 599 * 135, BASE.1);

    // 300 tracked nodes is fewer than the 600 the path needs.
    let mut small = NavScratch::<300>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, "x", BikeType::Road, &mut small, &mut tiles, &mut NullElevation, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted), "a 300-node table cannot span ~9 km");

    // The shipped table plans the same route.
    let mut big = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, "x", BikeType::Road, &mut big, &mut tiles, &mut NullElevation, &mut sink);
    let route = res.expect("the shipped table spans the ~9 km line");
    assert_eq!(route.total_distance_m, 9001, "summed edge costs over the whole line");
}

/// The weighted `g` saturates instead of wrapping, so a path costing more than 65 535 m still
/// plans. The displayed total is the unweighted `length_m` sum and does not clamp at that ceiling.
#[test]
fn saturated_costs_plan_without_panicking() {
    // Three nodes ~100 m apart with 60 km edge costs, so `g` saturates on the second hop. `n1` is
    // nudged off-axis so the decimator keeps it.
    let (n0, n1, n2) = (BASE, (BASE.0 + 900, BASE.1 + 500), (BASE.0 + 1_800, BASE.1));
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: n0 }, Node { id: 1, coord: n1 }, Node { id: 2, coord: n2 }],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![n0, n1], length_m: 60_000, kind: 0 },
            Edge { a: 1, b: 2, polyline: vec![n1, n2], length_m: 60_000, kind: 0 },
        ],
    };
    let (res, obcr, _) = plan(&map_with(&graph), n0, n2, "Far");
    let route = res.expect("a saturated-cost path still plans");
    assert_eq!(
        route.total_distance_m, 229,
        "the displayed total is the u32 length sum, past the u16 weighted-g ceiling"
    );
    assert_eq!(route_points(&obcr).len(), 3, "the geometry is intact");
}

/// Manual stepping produces a byte-identical OBCR to the one-shot `plan_route`, every search step
/// respects its budgets, and the plan really spans several steps.
#[test]
fn stepped_plan_matches_one_shot_and_respects_budgets() {
    use obc_route::nav::{NAV_MISSES_PER_STEP, NAV_SETTLES_PER_STEP_CAP};
    let bytes = map_with(&line_graph(120, 135, 15)); // ~1.8 km line, a multi-step search
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let from = BASE;
    let to = (BASE.0 + 119 * 135, BASE.1);

    let mut scratch = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut one_shot = VecSink::default();
    let reference = plan_route(
        &r,
        from,
        to,
        "Stepped",
        BikeType::Road,
        &mut scratch,
        &mut tiles,
        &mut NullElevation,
        &mut one_shot,
    )
    .expect("the line plans one-shot");

    let mut planner = NavPlanner::new(from, to, "Stepped", BikeType::Road);
    let mut tiles = NavTileCache::new();
    let mut stepped = VecSink::default();
    let mut steps = 0u32;
    let mut saw = (false, false, false); // (snap, search, emit) phases seen
    let stats = loop {
        let phase = planner.phase();
        match phase {
            NavPhase::Snap => saw.0 = true,
            NavPhase::Search => saw.1 = true,
            NavPhase::Emit => saw.2 = true,
            NavPhase::Done => panic!("stepping past the terminal outcome"),
        }
        let settles_before = planner.settles();
        let misses_before = tiles.stats().misses;
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut stepped);
        if phase == NavPhase::Search {
            // The budget check trails each settle, so the final settle can add one more read. The
            // extra +1 is slack for a coordinate that lands on a leaf boundary, where the walk
            // visits the sibling leaf too.
            let miss_delta = tiles.stats().misses - misses_before;
            let settle_delta = planner.settles() - settles_before;
            assert!(
                miss_delta <= NAV_MISSES_PER_STEP + 1,
                "a search step's reads stay within the miss budget + one-settle spillover (got {miss_delta})"
            );
            assert!(settle_delta <= NAV_SETTLES_PER_STEP_CAP, "the settle cap is hard ({settle_delta} settles)");
        } else {
            assert_eq!(planner.settles(), settles_before, "only search steps settle");
        }
        steps += 1;
        assert!(steps < 10_000, "the step machine must terminate");
        match step {
            obc_route::Step::Running => {}
            obc_route::Step::Done(stats) => break stats,
            obc_route::Step::Failed(e) => panic!("the stepped plan failed: {e:?}"),
        }
    };
    assert!(saw.0 && saw.1 && saw.2, "all three phases ran (saw {saw:?})");
    assert!(steps > 3, "a real plan spans multiple steps (got {steps})");
    assert_eq!(stats.total_distance_m, reference.total_distance_m);
    assert_eq!(stepped.buf, one_shot.buf, "stepping is byte-identical to the one-shot");

    let len = stepped.buf.len();
    assert!(matches!(
        planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut stepped),
        obc_route::Step::Done(_)
    ));
    assert_eq!(stepped.buf.len(), len, "a terminal step writes nothing");
}

/// The step budget at both ends, on a graph that fits inside the tile cache. Cold, a search step
/// reads no more than the miss budget. Warm, a step settles far more, because only the settle cap
/// paces it.
#[test]
fn search_step_budget_is_miss_paced_cold_and_cap_opened_warm() {
    use obc_route::nav::{NAV_MISSES_PER_STEP, NAV_SETTLES_PER_STEP_CAP};
    // This line's whole node section fits inside the cache, so after the first pass every settle
    // is a hit.
    let bytes = map_with(&line_graph(60, 200, 20));
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let from = BASE;
    let to = (BASE.0 + 59 * 200, BASE.1);

    let mut scratch = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let mut planner = NavPlanner::new(from, to, "Budget", BikeType::Road);
    let mut max_step_settles = 0u32;
    let mut first_search_misses: Option<u32> = None;
    loop {
        let phase = planner.phase();
        let settles_before = planner.settles();
        let misses_before = tiles.stats().misses;
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
        if phase == NavPhase::Search {
            let settle_delta = planner.settles() - settles_before;
            let miss_delta = tiles.stats().misses - misses_before;
            first_search_misses.get_or_insert(miss_delta);
            assert!(settle_delta <= NAV_SETTLES_PER_STEP_CAP, "the settle cap is hard ({settle_delta})");
            max_step_settles = max_step_settles.max(settle_delta);
        }
        match step {
            obc_route::Step::Running => {}
            obc_route::Step::Done(_) => break,
            obc_route::Step::Failed(e) => panic!("the budget-fixture plan failed: {e:?}"),
        }
    }
    assert!(
        first_search_misses.unwrap() <= NAV_MISSES_PER_STEP,
        "a cold search step reads ≤ the miss budget (got {:?})",
        first_search_misses
    );
    assert!(max_step_settles > 8, "a warm step settles well past 8 (got {max_step_settles})");
}

/// A cancel is simply not stepping again. Nothing reaches the sink before the emit phase, so an
/// abandoned plan leaves it pristine.
#[test]
fn abandoned_mid_search_plan_wrote_nothing() {
    let bytes = map_with(&line_graph(600, 135, 15));
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let from = BASE;
    let to = (BASE.0 + 599 * 135, BASE.1);
    let mut planner = NavPlanner::new(from, to, "x", BikeType::Road);
    while planner.phase() == NavPhase::Snap {
        assert_eq!(planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut sink), obc_route::Step::Running);
    }
    for _ in 0..8 {
        assert_eq!(planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut sink), obc_route::Step::Running);
    }
    assert_eq!(planner.phase(), NavPhase::Search, "still searching when abandoned");
    drop(planner); // the cancel
    assert!(sink.buf.is_empty(), "snap and search are read-only, so a cancelled plan wrote nothing");
}

// Profile-weighted A*. The fixtures above route under a neutral profile; those below build
// explicit profiles and per-edge kinds. Edges carry `kind = highway_class` with surface class 0,
// so the effective multiplier is the profile's `highway[kind]` byte.

/// Arbitrary highway classes. The tests build both the edge `kind` and the matching profile bytes,
/// so these need not be real OSM class ids.
const K_CYCLE: u8 = 1;
const K_PRIMARY: u8 = 2;
const K_STEPS: u8 = 3;

/// The edge weight for a `(length_m, kind)` under a profile, replicated in-test to compute the
/// Dijkstra reference and the found path's cost.
fn weighted(length_m: u32, kind: u8, p: &NavProfile) -> u32 {
    let mh = p.highway[(kind & 0x1F) as usize] as u32;
    let ms = p.surface[(kind >> 5) as usize] as u32;
    (length_m * ((mh * ms) >> 4)) >> 4
}

/// Two parallel corridors of equal length, one all-cycleway and one all-primary. The router must
/// take the one the profile prefers, which guards multiplier indexing end to end.
#[test]
fn profile_steers_between_equal_length_corridors() {
    let a = (500_000, 500_000);
    let b = (500_000 + 2 * SP, 500_000);
    let c1 = (500_000 + SP, 500_000 + SP); // north (cycleway corridor)
    let c2 = (500_000 + SP, 500_000 - SP); // south (primary corridor)
    let hop = 2_000; // above the ~1 574 m straight line, so admissible at 1.0x
    let graph = NavGraph {
        nodes: vec![
            Node { id: 0, coord: a },
            Node { id: 1, coord: c1 },
            Node { id: 2, coord: c2 },
            Node { id: 3, coord: b },
        ],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a, c1], length_m: hop, kind: K_CYCLE },
            Edge { a: 1, b: 3, polyline: vec![c1, b], length_m: hop, kind: K_CYCLE },
            Edge { a: 0, b: 2, polyline: vec![a, c2], length_m: hop, kind: K_PRIMARY },
            Edge { a: 2, b: 3, polyline: vec![c2, b], length_m: hop, kind: K_PRIMARY },
        ],
    };
    let cycle_loving = profile("cycle", &[(K_CYCLE, 16), (K_PRIMARY, 48)]); // primary 3.0×
    let primary_loving = profile("primary", &[(K_CYCLE, 48), (K_PRIMARY, 16)]); // cycleway 3.0×
    let bytes = map_with_profiles(&graph, &[cycle_loving, primary_loving]);
    let (from, to) = (a, b);

    let (res, obcr, _) = plan_p(&bytes, from, to, "Cycle", BikeType::Road);
    assert_eq!(res.unwrap().total_distance_m, 3148, "same-length corridors ⇒ identical ground distance");
    let pts = route_points(&obcr);
    assert!(pts.iter().any(|p| (p.lon, p.lat) == c1), "the cycle-loving profile takes the cycleway (north) corridor");
    assert!(!pts.iter().any(|p| (p.lon, p.lat) == c2), "…and not the primary (south) one");

    let (res, obcr, _) = plan_p(&bytes, from, to, "Primary", BikeType::Gravel);
    assert_eq!(res.unwrap().total_distance_m, 3148);
    let pts = route_points(&obcr);
    assert!(pts.iter().any(|p| (p.lon, p.lat) == c2), "the primary-loving profile takes the primary (south) corridor");
    assert!(!pts.iter().any(|p| (p.lon, p.lat) == c1), "…and not the cycleway (north) one");
}

/// Pins the arithmetic, not just the ordering: the cycleway detour is 1.4x the direct primary
/// edge, so it wins only while the primary multiplier exceeds 1.4x. Only the primary byte moves
/// between the two profiles.
#[test]
fn detour_is_taken_exactly_when_the_multiplier_math_says_so() {
    let a = (500_000, 500_000);
    let b = (500_000 + 2 * SP, 500_000);
    let d = (500_000 + SP, 500_000 + SP); // detour apex (north)
    let direct = 2_400; // above the ~2 226 m straight line, so admissible
    let leg = 1_680; // 2 * 1 680 = 3 360 = 1.4 * direct; each above its ~1 574 m straight line
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }, Node { id: 2, coord: d }],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a, b], length_m: direct, kind: K_PRIMARY },
            Edge { a: 0, b: 2, polyline: vec![a, d], length_m: leg, kind: K_CYCLE },
            Edge { a: 2, b: 1, polyline: vec![d, b], length_m: leg, kind: K_CYCLE },
        ],
    };
    let primary_2x = profile("p2", &[(K_PRIMARY, 32), (K_CYCLE, 16)]);
    let primary_125x = profile("p1.25", &[(K_PRIMARY, 20), (K_CYCLE, 16)]); // 1.25x = 20/16
    let bytes = map_with_profiles(&graph, &[primary_2x, primary_125x]);
    let (from, to) = (a, b);

    let (res, _, _) = plan_p(&bytes, from, to, "Detour", BikeType::Road);
    assert_eq!(res.unwrap().total_distance_m, 3148, "primary 2.0× > 1.4× ⇒ the cycleway detour wins");

    let (res, _, _) = plan_p(&bytes, from, to, "Direct", BikeType::Gravel);
    assert_eq!(res.unwrap().total_distance_m, 2226, "primary 1.25× < 1.4× ⇒ the direct primary wins");
}

/// A forbidden class is skipped in relaxation, not routed. With the direct edge forbidden the
/// router detours; with every edge forbidden the frontier drains to `NoPath`.
#[test]
fn forbidden_class_detours_then_no_paths() {
    let a = (500_000, 500_000);
    let b = (500_000 + 2 * SP, 500_000);
    let d = (500_000 + SP, 500_000 + SP);
    let no_steps = profile("no-steps", &[(K_STEPS, 0)]); // steps forbidden, cycleway stays 1.0x

    // The direct edge is forbidden steps, and the cycleway detour is legal.
    let detourable = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }, Node { id: 2, coord: d }],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 2_400, kind: K_STEPS },
            Edge { a: 0, b: 2, polyline: vec![a, d], length_m: 1_680, kind: K_CYCLE },
            Edge { a: 2, b: 1, polyline: vec![d, b], length_m: 1_680, kind: K_CYCLE },
        ],
    };
    let bytes = map_with_profiles(&detourable, std::slice::from_ref(&no_steps));
    let (res, obcr, _) = plan_p(&bytes, a, b, "Around", BikeType::Road);
    assert_eq!(res.unwrap().total_distance_m, 3148, "the forbidden direct edge is skipped ⇒ detour");
    assert!(route_points(&obcr).iter().any(|p| (p.lon, p.lat) == d), "the route goes around via the legal apex");

    // The only edge is forbidden steps, so A snaps but has no legal escape.
    let dead = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 2_400, kind: K_STEPS }],
    };
    let bytes = map_with_profiles(&dead, &[no_steps]);
    let (res, obcr, _) = plan_p(&bytes, a, b, "Dead", BikeType::Road);
    assert_eq!(res, Err(NavError::NoPath), "every escape forbidden ⇒ the frontier drains to NoPath");
    assert!(obcr.is_empty());
}

/// The emitted `total_distance_m` is the raw summed edge length, not the weighted `g`.
#[test]
fn displayed_distance_is_raw_length_not_weighted_g() {
    let a = (500_000, 500_000);
    let b = (500_000 + SP, 500_000);
    let length = 2_000;
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: length, kind: K_PRIMARY }],
    };
    let bytes = map_with_profiles(&graph, &[profile("p2", &[(K_PRIMARY, 32)])]); // 2.0x
    let (res, _, _) = plan_p(&bytes, a, b, "Honest", BikeType::Road);
    let route = res.expect("a single-edge route plans");
    assert_eq!(route.total_distance_m, 1113, "displayed distance is the raw ground length");
    assert_ne!(route.total_distance_m, 2 * length, "and not the weighted g, which is 2x");
}

/// An out-of-range profile index falls back to profile 0 and plans byte-identically.
#[test]
fn out_of_range_profile_index_falls_back_to_zero() {
    let a = (500_000, 500_000);
    let b = (500_000 + 2 * SP, 500_000);
    let c1 = (500_000 + SP, 500_000 + SP);
    let c2 = (500_000 + SP, 500_000 - SP);
    let hop = 2_000;
    let graph = NavGraph {
        nodes: vec![
            Node { id: 0, coord: a },
            Node { id: 1, coord: c1 },
            Node { id: 2, coord: c2 },
            Node { id: 3, coord: b },
        ],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a, c1], length_m: hop, kind: K_CYCLE },
            Edge { a: 1, b: 3, polyline: vec![c1, b], length_m: hop, kind: K_CYCLE },
            Edge { a: 0, b: 2, polyline: vec![a, c2], length_m: hop, kind: K_PRIMARY },
            Edge { a: 2, b: 3, polyline: vec![c2, b], length_m: hop, kind: K_PRIMARY },
        ],
    };
    // The second profile exists only to prove that a type past the table does not select it.
    let bytes =
        map_with_profiles(&graph, &[profile("cycle", &[(K_PRIMARY, 48)]), profile("primary", &[(K_CYCLE, 48)])]);
    let (from, to) = (a, b);
    let (res0, obcr0, _) = plan_p(&bytes, from, to, "Fallback", BikeType::Road);
    let (res3, obcr3, _) = plan_p(&bytes, from, to, "Fallback", BikeType::Touring);
    assert_eq!(res0.unwrap().total_distance_m, res3.unwrap().total_distance_m);
    assert_eq!(route_points(&obcr0), route_points(&obcr3), "a type past the table plans as profile 0");
    // It took the profile-0 corridor, not a fallback-to-neutral tie.
    assert!(route_points(&obcr3).iter().any(|p| (p.lon, p.lat) == c1));
}

/// A grid with mixed per-edge kinds. The found path's weighted cost must stay within the rung's
/// bound of the true optimum, which an in-test Dijkstra computes.
#[test]
fn found_cost_is_within_epsilon_of_dijkstra_reference() {
    let mut nodes = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            nodes.push(Node { id: (row * 3 + col) as u32, coord: at(row, col) });
        }
    }
    let mut edges = Vec::new();
    for row in 0..3 {
        for col in 0..3 {
            let a = (row * 3 + col) as u32;
            if col < 2 {
                let (ca, cb) = (at(row, col), at(row, col + 1));
                let mid = ((ca.0 + cb.0) / 2, ca.1 + 500);
                let kind = if row == 2 { K_CYCLE } else { K_PRIMARY }; // the bottom row is cheap
                edges.push(Edge { a, b: a + 1, polyline: vec![ca, mid, cb], length_m: EDGE_COST, kind });
            }
            if row < 2 {
                let (ca, cb) = (at(row, col), at(row + 1, col));
                let mid = (ca.0 + 500, (ca.1 + cb.1) / 2);
                edges.push(Edge { a, b: a + 3, polyline: vec![ca, mid, cb], length_m: EDGE_COST, kind: K_CYCLE });
            }
        }
    }
    let graph = NavGraph { nodes, edges };
    let prof = profile("mixed", &[(K_PRIMARY, 32), (K_CYCLE, 16)]);
    let bytes = map_with_profiles(&graph, std::slice::from_ref(&prof));

    let from = at(0, 0);
    let goal = at(2, 2);
    let (res, obcr, _) = plan_p(&bytes, from, goal, "Mixed", BikeType::Road);
    res.expect("the mixed-kind grid plans");

    // Dijkstra over the fixture graph with the same weighted edge costs.
    let n = graph.nodes.len();
    let mut adj = vec![Vec::<(usize, u32)>::new(); n];
    for e in &graph.edges {
        let w = weighted(e.length_m, e.kind, &prof);
        adj[e.a as usize].push((e.b as usize, w));
        adj[e.b as usize].push((e.a as usize, w));
    }
    let mut dist = vec![u32::MAX; n];
    let mut done = vec![false; n];
    dist[0] = 0;
    for _ in 0..n {
        let Some(u) = (0..n).filter(|&i| !done[i] && dist[i] != u32::MAX).min_by_key(|&i| dist[i]) else { break };
        done[u] = true;
        for &(v, w) in &adj[u] {
            dist[v] = dist[v].min(dist[u].saturating_add(w));
        }
    }
    let reference = dist[8];
    assert!(reference > 0 && reference != u32::MAX, "the reference is a real finite cost");

    // Reconstruct the found path's node sequence from the emitted geometry and sum its cost.
    let pts = route_points(&obcr);
    let mut seq: Vec<u32> = Vec::new();
    for pt in &pts {
        if let Some(node) = graph.nodes.iter().find(|nd| nd.coord == (pt.lon, pt.lat)) {
            if seq.last() != Some(&node.id) {
                seq.push(node.id);
            }
        }
    }
    let found: u32 = seq
        .windows(2)
        .map(|w| {
            let e = graph
                .edges
                .iter()
                .find(|e| (e.a == w[0] && e.b == w[1]) || (e.a == w[1] && e.b == w[0]))
                .expect("consecutive nodes share an edge");
            weighted(e.length_m, e.kind, &prof)
        })
        .sum();

    assert!(
        found <= reference * 13 / 10,
        "found weighted cost {found} exceeds the ε = 1.3 bound over Dijkstra reference {reference}"
    );
}

// The escalation ladder. The fixtures below are diagonal grids: A* on a uniform grid expands the
// whole diamond of monotone-optimal nodes at the tight rung but only a narrow band at a greedier
// one, so a fixed sub-diamond table exhausts tight and completes greedy.

/// A full 4-connected grid, every edge one `EDGE_COST` hop of the given `kind`. The diagonal
/// corner-to-corner route over it is the rung-sensitive exhaustion fixture.
fn grid_diag(rows: i32, cols: i32, kind: u8) -> NavGraph {
    let mut nodes = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            nodes.push(Node { id: (row * cols + col) as u32, coord: at(row, col) });
        }
    }
    let mut edges = Vec::new();
    for row in 0..rows {
        for col in 0..cols {
            let a = (row * cols + col) as u32;
            if col < cols - 1 {
                let (ca, cb) = (at(row, col), at(row, col + 1));
                let mid = ((ca.0 + cb.0) / 2, ca.1 + 500);
                edges.push(Edge { a, b: a + 1, polyline: vec![ca, mid, cb], length_m: EDGE_COST, kind });
            }
            if row < rows - 1 {
                let (ca, cb) = (at(row, col), at(row + 1, col));
                let mid = (ca.0 + 500, (ca.1 + cb.1) / 2);
                edges.push(Edge { a, b: a + cols as u32, polyline: vec![ca, mid, cb], length_m: EDGE_COST, kind });
            }
        }
    }
    NavGraph { nodes, edges }
}

/// The same grid with a cheap bottom corridor. The true optimum dives to the bottom row; a greedy
/// search cuts diagonally through the expensive interior, so an escalated route is suboptimal but
/// bounded.
fn grid_diag_mixed(rows: i32, cols: i32) -> NavGraph {
    let mut g = grid_diag(rows, cols, K_CYCLE);
    for e in &mut g.edges {
        let (ra, ca) = ((e.a as i32) / cols, (e.a as i32) % cols);
        let (rb, _cb) = ((e.b as i32) / cols, (e.b as i32) % cols);
        let horizontal = ra == rb;
        if horizontal && ra != rows - 1 {
            e.kind = K_PRIMARY; // interior horizontals are the expensive class
        }
        let _ = ca;
    }
    g
}

/// Plain Dijkstra with the profile's weighted edge costs: the in-test optimum the bound is
/// measured against.
fn dijkstra_weighted(graph: &NavGraph, prof: &NavProfile, goal: usize) -> u32 {
    let n = graph.nodes.len();
    let mut adj = vec![Vec::<(usize, u32)>::new(); n];
    for e in &graph.edges {
        let w = weighted(e.length_m, e.kind, prof);
        adj[e.a as usize].push((e.b as usize, w));
        adj[e.b as usize].push((e.a as usize, w));
    }
    let mut dist = vec![u32::MAX; n];
    let mut done = vec![false; n];
    dist[0] = 0;
    for _ in 0..n {
        let Some(u) = (0..n).filter(|&i| !done[i] && dist[i] != u32::MAX).min_by_key(|&i| dist[i]) else { break };
        done[u] = true;
        for &(v, w) in &adj[u] {
            dist[v] = dist[v].min(dist[u].saturating_add(w));
        }
    }
    dist[goal]
}

/// `(result, epsilon_used, cumulative_settles, settles_at_first_escalation, obcr_bytes)`. The
/// fourth is `None` when the plan never escalated.
type LadderOutcome = (Result<obc_route::RouteStats, NavError>, (u32, u32), u32, Option<u32>, Vec<u8>);

/// Step a plan to its terminal outcome on an `N`-slot table, watching `epsilon_used()` change.
fn plan_ladder<const N: usize>(bytes: &[u8], from: (i32, i32), to: (i32, i32), bike: BikeType) -> LadderOutcome {
    use obc_route::nav::Step;
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<N>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let mut planner = NavPlanner::new(from, to, "x", bike);
    let mut eps = planner.epsilon_used();
    let mut first_escalation: Option<u32> = None;
    let res = loop {
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
        if planner.epsilon_used() != eps {
            first_escalation.get_or_insert(planner.settles());
            eps = planner.epsilon_used();
        }
        match step {
            Step::Running => {}
            Step::Done(s) => break Ok(s),
            Step::Failed(e) => break Err(e),
        }
    };
    (res, planner.epsilon_used(), planner.settles(), first_escalation, sink.buf)
}

/// A grid whose tight-rung diamond overruns a 50-slot table but whose greedier corridor fits. The
/// plan escalates once and completes. `settles` is cumulative, so it exceeds both the table size
/// and the count at the moment of escalation.
#[test]
fn escalation_succeeds_on_second_rung() {
    let bytes = map_with(&grid_diag(9, 12, 0));
    let from = at(0, 0);
    let goal = at(8, 11);
    let to = goal;
    let (res, eps, settles, first_esc, obcr) = plan_ladder::<50>(&bytes, from, to, BikeType::Road);
    res.expect("the ε = 2.0 rung completes the route the tight bound couldn't fit");
    assert_eq!(eps, (2, 1), "the plan escalated exactly one rung");
    assert!(!obcr.is_empty(), "a completed plan emits an OBCR");
    let escalated_at = first_esc.expect("the plan escalated, so a first-escalation settle count exists");
    assert!(settles > escalated_at, "settles accumulated across the retry (cumulative {settles} > {escalated_at})");
    assert!(settles > 50, "cumulative settles exceed the table size");
}

/// A 20-slot table fits no rung's corridor, so all three exhaust and the plan fails at the top of
/// the ladder.
#[test]
fn ladder_exhausts_honestly_at_top_rung() {
    let bytes = map_with(&grid_diag(9, 12, 0));
    let from = at(0, 0);
    let goal = at(8, 11);
    let to = goal;
    let (res, eps, settles, _, obcr) = plan_ladder::<20>(&bytes, from, to, BikeType::Road);
    assert_eq!(res, Err(NavError::Exhausted), "too dense for every rung");
    assert_eq!(eps, (3, 1), "the ladder climbed to and failed at the top rung");
    assert!(obcr.is_empty(), "an exhausted plan writes nothing");
    assert!(settles > 20, "three passes burned more than one table of settles ({settles})");
}

/// The frontier drains without the table filling, so the plan fails at rung 0. A greedier rung
/// cannot connect an island and must not triple the latency.
#[test]
fn no_retry_on_disconnect_fails_fast_at_rung_zero() {
    let a0 = (500_000, 500_000);
    let a1 = (505_000, 500_000);
    let b0 = (550_000, 500_000);
    let b1 = (555_000, 500_000);
    let graph = NavGraph {
        nodes: vec![
            Node { id: 0, coord: a0 },
            Node { id: 1, coord: a1 },
            Node { id: 2, coord: b0 },
            Node { id: 3, coord: b1 },
        ],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a0, a1], length_m: 600, kind: 0 },
            Edge { a: 2, b: 3, polyline: vec![b0, b1], length_m: 600, kind: 0 },
        ],
    };
    let bytes = map_with(&graph);
    let (res, eps, _, first_esc, obcr) =
        plan_ladder::<{ obc_route::NAV_MAX_NODES }>(&bytes, (a0.0 + 100, a0.1), (b0.0 - 100, b0.1), BikeType::Road);
    assert_eq!(res, Err(NavError::NoPath), "disconnected");
    assert_eq!(eps, (13, 10), "NoPath never escalates");
    assert!(first_esc.is_none(), "the plan never retried");
    assert!(obcr.is_empty());
}

/// The bound is measured against the rung the search ends on, not a fixed one. A small table
/// forces escalation and a suboptimal path; a large table completes optimally. Both stay within
/// `epsilon_used()` times the Dijkstra optimum.
#[test]
fn found_cost_is_within_used_rung_epsilon_of_dijkstra() {
    let (rows, cols) = (7, 10);
    let graph = grid_diag_mixed(rows, cols);
    let prof = profile("mixed", &[(K_PRIMARY, 32), (K_CYCLE, 16)]); // primary 2.0x, cycleway 1.0x
    let bytes = map_with_profiles(&graph, std::slice::from_ref(&prof));
    let from = at(0, 0);
    let goal_n = ((rows - 1) * cols + (cols - 1)) as usize;
    let goal = at(rows - 1, cols - 1);
    let to = goal;
    let reference = dijkstra_weighted(&graph, &prof, goal_n);
    assert!(reference > 0 && reference != u32::MAX, "the reference is a real finite cost");

    // Reconstruct the found path's node sequence from the emitted geometry and sum its cost.
    let found_weighted = |obcr: &[u8]| -> u32 {
        let pts = route_points(obcr);
        let mut seq: Vec<u32> = Vec::new();
        for pt in &pts {
            if let Some(node) = graph.nodes.iter().find(|nd| nd.coord == (pt.lon, pt.lat)) {
                if seq.last() != Some(&node.id) {
                    seq.push(node.id);
                }
            }
        }
        seq.windows(2)
            .map(|w| {
                let e = graph
                    .edges
                    .iter()
                    .find(|e| (e.a == w[0] && e.b == w[1]) || (e.a == w[1] && e.b == w[0]))
                    .expect("consecutive nodes share an edge");
                weighted(e.length_m, e.kind, &prof)
            })
            .sum()
    };

    // A 40-slot table exhausts the first two rungs and completes greedy, on a suboptimal path.
    let (res, eps, _, _, obcr) = plan_ladder::<40>(&bytes, from, to, BikeType::Road);
    res.expect("the ε = 3.0 rung completes the mixed grid");
    assert_eq!(eps, (3, 1), "the small table drove the plan to the top rung");
    let found = found_weighted(&obcr);
    assert!(found > reference, "the escalated greedy path is genuinely suboptimal (found {found} > opt {reference})");
    assert!(
        found <= reference * eps.0 / eps.1,
        "found weighted {found} exceeds the ε = {}/{} bound over Dijkstra optimum {reference}",
        eps.0,
        eps.1
    );

    // A roomy table completes optimally on the first try.
    let (res, eps, _, first_esc, obcr) = plan_ladder::<120>(&bytes, from, to, BikeType::Road);
    res.expect("a roomy table completes at rung 0");
    assert_eq!(eps, (13, 10), "no escalation with room to spare");
    assert!(first_esc.is_none());
    let found = found_weighted(&obcr);
    assert_eq!(found, reference, "at ε = 1.3 with a roomy table the found path is the optimum here");
    assert!(found <= reference * eps.0 / eps.1);
}

/// A route that completes on the first rung never escalates and emits the same bytes as the
/// one-shot plan.
#[test]
fn first_try_success_takes_rung_zero_unchanged() {
    let bytes = map_with(&grid3(true));
    let (c0, c8) = (at(0, 0), at(2, 2));
    let (from, to) = (c0, c8);

    // The name matches `plan_ladder`'s, so the comparison is of route bytes, not the name field.
    let (reference, one_shot, _) = plan(&bytes, from, to, "x");
    let reference = reference.expect("the shortcut grid plans at 1.3");

    let (res, eps, settles, first_esc, obcr) =
        plan_ladder::<{ obc_route::NAV_MAX_NODES }>(&bytes, from, to, BikeType::Road);
    let route = res.expect("still plans");
    assert_eq!(eps, (13, 10), "a first-rung success never escalates");
    assert!(first_esc.is_none(), "no retry happened");
    assert_eq!(route.total_distance_m, reference.total_distance_m);
    assert_eq!(obcr, one_shot, "byte-identical OBCR to the unchanged one-shot plan");
    assert!(settles > 0 && settles < obc_route::NAV_MAX_NODES as u32, "a single search, well within one table");
}

// Emit-time elevation fill.

/// A tent-shaped ridge in longitude. The peak sits between two grid-edge vertices, so a
/// vertex-only fill cannot see it, which makes this a densification probe. Latitude is ignored.
struct Ridge;

/// Half-way along the first east-west grid edge, and on none of the interpolated points.
const CREST_LON: i32 = 502_500;
const CREST_M: i32 = 1_000;

fn ridge_height(lon: i32) -> i16 {
    (CREST_M - (lon - CREST_LON).abs() / 25).max(300) as i16
}

impl obc_route::ElevationSource for Ridge {
    fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        Some(ridge_height(lon_udeg))
    }
}

/// The same ridge with a hole across a band of longitude: the `NODATA` case.
struct HolyRidge;

/// The band, half-open, inside which [`HolyRidge`] answers `None`.
const HOLE: core::ops::Range<i32> = 505_000..515_000;

impl obc_route::ElevationSource for HolyRidge {
    fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        if HOLE.contains(&lon_udeg) {
            return None;
        }
        Some(ridge_height(lon_udeg))
    }
}

/// Terrain that starts past the route's opening: the crop a real sidecar has when the nav graph
/// reaches beyond the extract it was baked for. West of [`COVERAGE_LON`] there is no data; east of
/// it the covered part has an exactly known climb.
struct CroppedTerrain;

/// Past the grid's first column, so the route's first points are all outside the raster.
const COVERAGE_LON: i32 = 507_000;
/// High enough that booking it as ascent would be unmissable.
const COVERED_BASE_M: i32 = 1_412;

fn covered_height(lon: i32) -> i16 {
    (COVERED_BASE_M + (lon - COVERAGE_LON) / 25) as i16
}

impl obc_route::ElevationSource for CroppedTerrain {
    fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        (lon_udeg >= COVERAGE_LON).then(|| covered_height(lon_udeg))
    }
}

fn plan_with_elevation(bytes: &[u8], elev: &mut dyn obc_route::ElevationSource) -> (obc_route::RouteStats, Vec<u8>) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let (c0, c8) = (at(0, 0), at(2, 2));
    let res = plan_route(&r, c0, c8, "Water stop", BikeType::Road, &mut scratch, &mut tiles, elev, &mut sink);
    (res.expect("the grid plans"), sink.buf)
}

/// With [`NullElevation`] the emitted OBCR carries no densification, no elevation and no stats.
/// The digest must not move: if it does, terrain support has changed the no-terrain output.
#[test]
fn a_null_elevation_plan_emits_the_pre_terrain_bytes() {
    let bytes = map_with(&grid3(false));
    let (route, obcr) = plan_with_elevation(&bytes, &mut NullElevation);

    assert_eq!(digest(&obcr), NULL_PATH_DIGEST, "the no-terrain OBCR must stay byte-identical");
    assert_eq!(route.point_count, 9, "no densification without terrain");
    assert_eq!((route.total_ascent_m, route.total_descent_m), (0, 0));
    assert_eq!((route.min_ele_m, route.max_ele_m), (0, 0));
    assert!(route_points(&obcr).iter().all(|p| p.elevation().is_none()), "every stored height is zero");
}

/// FNV-1a of the no-terrain emit for the fixture above.
const NULL_PATH_DIGEST: u64 = 1853384897492501207;

/// A real source fills every point's height and the header's min, max and dead-banded climb. The
/// crest is reachable only through the densification: a vertex-only fill tops out at 900 m.
#[test]
fn terrain_fills_every_point_and_the_header_stats() {
    let bytes = map_with_terrain(&grid3(false), &[neutral_profile()], &mut Ridge);
    let (route, obcr) = plan_with_elevation(&bytes, &mut Ridge);
    let pts = route_points(&obcr);

    assert!(route.point_count > 9, "terrain densifies the polyline (got {})", route.point_count);
    assert!(pts.iter().all(|p| p.ele >= 300), "every point carries a real height");
    assert!(
        route.max_ele_m > 950,
        "the crest between two vertices is captured (got {} m; vertex-only would be 900)",
        route.max_ele_m
    );
    assert_eq!(route.max_ele_m, pts.iter().map(|p| p.ele).max().unwrap(), "header max = stored max");
    assert_eq!(route.min_ele_m, pts.iter().map(|p| p.ele).min().unwrap(), "header min = stored min");
    assert!(route.total_ascent_m > 0 && route.total_descent_m > 0, "the route climbs the ridge and comes down");
    // Densifying the geometry must not touch the distance.
    assert_eq!(route.total_distance_m, 4474);
}

/// An imported copy of a plan measures what the plan measures, whatever heights it carries, so an
/// easier comparison finds no saving on the same road.
#[test]
fn an_imported_copy_of_a_plan_measures_what_the_plan_measured() {
    use crate::common::{build_obcr, ChunkIn, RouteSpec};
    use obc_route::easier::Costs;
    let bytes = map_with_terrain(&grid3(false), &[neutral_profile()], &mut Ridge);
    let (plan, obcr) = plan_with_elevation(&bytes, &mut Ridge);
    let (_, planned) = Costs::candidate(&SliceSource(&obcr), [0, plan.total_distance_m], &mut Ridge).unwrap();
    let points = route_points(&obcr).iter().map(|p| (p.lon, p.lat, 0)).collect();
    let (imported, _) = build_obcr(&RouteSpec {
        chunks: &[ChunkIn { points, cum_distance_m: 0, cum_ascent_m: 0 }],
        totals: (plan.total_distance_m, 0, 0),
        ..RouteSpec::default()
    });
    let source = SliceSource(&imported);
    let index = RouteIndex::read(&source).unwrap();
    let map = obc_formats::obcr::RouteSourceKey { store: [0; 16], object: 1, revision: 1 };
    let copy = Costs::remaining(&RouteReader::new(&index, &source), 0, map, &mut Ridge).unwrap();
    assert!(planned.elevation_complete && planned.ascent_m > 0);
    assert_eq!((copy.distance_m, copy.ascent_m), (planned.distance_m, planned.ascent_m));
}

/// Densification is bounded by ground distance, not by vertex count, so the bound is asserted on
/// the sampling rather than on the points that survive the decimator.
#[test]
fn terrain_samples_at_least_once_per_step_of_ground() {
    struct Counting(u32);
    impl obc_route::ElevationSource for Counting {
        fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
            self.0 += 1;
            Some(ridge_height(lon_udeg))
        }
    }
    let bytes = map_with_terrain(&grid3(false), &[neutral_profile()], &mut Ridge);
    let mut counting = Counting(0);
    let (route, _) = plan_with_elevation(&bytes, &mut counting);

    // The emitted polyline is longer than the summed edge costs, so the route's own distance is a
    // safe lower bound on the ground the fill must cover.
    let want = route.total_distance_m / 250;
    assert!(
        counting.0 >= want,
        "{} samples for {} m of route: under one per 250 m",
        counting.0,
        route.total_distance_m
    );
    // And not far more: the fill samples points, it does not sweep the raster.
    assert!(counting.0 < want * 4, "{} samples is far more than the step implies", counting.0);
}

/// Re-integrating the stored heights with the shared dead-band, which is what a GPX re-import
/// does, reproduces the header stats.
#[test]
fn the_header_stats_are_the_shared_dead_band_over_the_stored_points() {
    let bytes = map_with_terrain(&grid3(false), &[neutral_profile()], &mut Ridge);
    let (route, obcr) = plan_with_elevation(&bytes, &mut Ridge);

    let mut band = obc_elevation::DeadBand::<f64>::new();
    for p in route_points(&obcr) {
        band.push(f64::from(p.ele));
    }
    assert_eq!(
        (band.ascent() as u32, band.descent() as u32),
        (route.total_ascent_m, route.total_descent_m),
        "a re-import of these very points books the header's climb"
    );
}

/// A hole in coverage carries the last known height forward, so the span is flat and never a
/// phantom climb, while the sampled parts keep their real stats.
#[test]
fn a_coverage_hole_carries_the_last_height_forward() {
    let bytes = map_with_terrain(&grid3(false), &[neutral_profile()], &mut Ridge);
    let (route, obcr) = plan_with_elevation(&bytes, &mut HolyRidge);
    let pts = route_points(&obcr);

    assert!(pts.iter().all(|p| p.elevation().is_none_or(|e| e >= 300)));
    // Every point inside the hole repeats the last height that resolved, so the dead-band books
    // nothing across it.
    let inside: Vec<i16> = pts.iter().filter(|p| HOLE.contains(&p.lon)).map(|p| p.ele).collect();
    assert!(!inside.is_empty(), "the route does cross the hole");
    assert!(inside.iter().all(|&e| e == i16::MIN), "the hole is flat at the carried height (got {inside:?})");
    assert!(route.total_ascent_m > 0, "the sampled part still books its climb (got {route:?})");
}

/// A route that starts outside coverage has no height to carry, so the integrator must not run
/// until the first sample resolves. If it does, the band anchors on the `0` placeholder and the
/// first real height lands in the header as ascent.
#[test]
fn a_route_that_starts_outside_coverage_books_no_phantom_ascent() {
    let bytes = map_with_terrain(&grid3(false), &[neutral_profile()], &mut Ridge);
    let (route, obcr) = plan_with_elevation(&bytes, &mut CroppedTerrain);
    let pts = route_points(&obcr);

    // The route straddles the coverage edge.
    assert!(pts.iter().any(|p| p.lon < COVERAGE_LON), "the route starts outside coverage");
    let covered: Vec<&obc_route::RoutePoint> = pts.iter().filter(|p| p.lon >= COVERAGE_LON).collect();
    assert!(covered.len() >= 2, "and crosses well into it");

    // The header's climb is the covered climb only: the step into coverage is not a hill.
    let mut band = obc_elevation::DeadBand::<f64>::new();
    for p in &covered {
        band.push(f64::from(p.ele));
    }
    let covered_ascent = band.ascent() as u32;
    assert!(covered_ascent > 0, "the covered part does climb (otherwise this test proves nothing)");
    assert_eq!(
        route.total_ascent_m, covered_ascent,
        "header ascent must be the post-coverage climb only, not {} m of phantom step",
        COVERED_BASE_M
    );
    assert!(route.total_ascent_m < COVERED_BASE_M as u32, "a {} m first sample was booked as ascent", COVERED_BASE_M);
    // The stored per-point cumulative must read 0 at the first covered point too.
    let first_covered_cum = cum_ascent_at(&obcr, |p| p.lon >= COVERAGE_LON);
    assert_eq!(first_covered_cum, 0, "cum_ascent at the first covered point is poisoned");
    // The uncovered opening still stores 0, because OBCR has no "unknown".
    assert!(pts.iter().filter(|p| p.lon < COVERAGE_LON).all(|p| p.elevation().is_none()));
}

/// The stored cumulative ascent at the first point matching `pred`, read through the same
/// `ChunkMeta` the profile builder reads, so this checks what is written.
fn cum_ascent_at(obcr: &[u8], pred: impl Fn(&obc_route::RoutePoint) -> bool) -> u32 {
    let src = SliceSource(obcr);
    let idx = RouteIndex::read(&src).expect("the emitted OBCR parses");
    let r = RouteReader::new(&idx, &src);
    for k in 0..idx.chunks().len() {
        let chunk = decode(&r, k);
        if chunk.iter().any(&pred) {
            return idx.chunks()[k].cum_ascent_m;
        }
    }
    panic!("no point matched")
}

/// A source that never resolves gives the null source's bytes and zeroed stats, so a flat route is
/// never reported as real terrain.
#[test]
fn a_source_that_never_resolves_leaves_the_stats_zeroed() {
    struct Blind;
    impl obc_route::ElevationSource for Blind {
        fn sample(&mut self, _lat: i32, _lon: i32) -> Option<i16> {
            None
        }
    }
    let bytes = map_with_terrain(&grid3(false), &[neutral_profile()], &mut Ridge);
    let (route, obcr) = plan_with_elevation(&bytes, &mut Blind);
    let (_, null_obcr) = plan_with_elevation(&bytes, &mut NullElevation);

    assert_eq!((route.min_ele_m, route.max_ele_m), (0, 0));
    assert_eq!((route.total_ascent_m, route.total_descent_m), (0, 0));
    assert_eq!(obcr, null_obcr, "an always-None source emits exactly the null source's bytes");
}

// Climb-aware relaxation. `ascent_m` is baked by the packer at serialize time, so these fixtures
// hand `serialize_lods` a synthetic terrain and route over the resulting bytes. No plan below
// mounts terrain at emit: what is under test is the search, not the fill.

/// [`map_with_profiles`] over a caller-supplied terrain. Without it the packer bakes a zero ascent
/// everywhere and the climb term is unreachable from a test.
fn map_with_terrain(
    graph: &NavGraph,
    profiles: &[NavProfile],
    terrain: &mut dyn obc_route::ElevationSource,
) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, profiles, terrain);
    assert_eq!(dropped, 0);
    bin
}

/// A neutral-multiplier profile carrying nothing but a climb weight, so a test that moves it moves
/// only the climb term.
fn climb_profile(name: &str, climb_weight: u8) -> NavProfile {
    NavProfile { name: name.into(), highway: [16; 32], surface: [16; 8], climb_weight }
}

/// A north-facing hillside: the ground rises with latitude above the grid's base row and is flat
/// at or below it. One variable, so every ascent below is arithmetic: a leg from row 0 to row 1
/// climbs 400 m and the same leg southbound climbs nothing.
struct Hillside;

/// The climb of one grid row of the [`Hillside`].
const ROW_CLIMB_M: u32 = 400;

impl obc_route::ElevationSource for Hillside {
    fn sample(&mut self, lat_udeg: i32, _lon_udeg: i32) -> Option<i16> {
        Some(((lat_udeg - BASE.1).max(0) / 25) as i16)
    }
}

/// A conical knoll on the grid's middle node. Unlike [`Hillside`] it makes the monotone
/// corner-to-corner paths differ in climb while they tie in distance, which is what the Dijkstra
/// ground truth needs.
struct Knoll;

const SUMMIT_M: i32 = 600;

impl obc_route::ElevationSource for Knoll {
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        let summit = at(1, 1);
        let d = (lon_udeg - summit.0).abs().max((lat_udeg - summit.1).abs());
        Some((SUMMIT_M - d / 20).max(0) as i16)
    }
}

/// Two nodes on the flat, joined either north over a pass, two short legs through an apex the
/// [`Hillside`] raises [`ROW_CLIMB_M`] above them, or south through a valley, two long legs that
/// stay flat. Every leg is longer than its own straight line, so the heuristic stays admissible.
///
/// The arithmetic, once: the pass costs `2 * PASS_LEG + ROW_CLIMB_M * w` and the valley
/// `2 * VALLEY_LEG`, so they cross at `w = 5`. Only the uphill leg books ascent.
///
/// The legs are kilometres and the nodes hundreds of metres apart on purpose. A frontier node
/// carries heuristic inflation the goal does not, so with shorter legs the rung, not the cost
/// model, would decide the tie.
fn pass_vs_valley() -> NavGraph {
    let (a, b) = (at(0, 0), at(0, 2));
    let (pass, valley) = (at(1, 1), at(-1, 1));
    NavGraph {
        nodes: vec![
            Node { id: 0, coord: a },
            Node { id: 1, coord: pass },
            Node { id: 2, coord: valley },
            Node { id: 3, coord: b },
        ],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a, pass], length_m: PASS_LEG, kind: 0 },
            Edge { a: 1, b: 3, polyline: vec![pass, b], length_m: PASS_LEG, kind: 0 },
            Edge { a: 0, b: 2, polyline: vec![a, valley], length_m: VALLEY_LEG, kind: 0 },
            Edge { a: 2, b: 3, polyline: vec![valley, b], length_m: VALLEY_LEG, kind: 0 },
        ],
    }
}

/// One leg of the pass corridor, m. Above both its own ~1 574 m straight line and the inflation
/// the frontier carries.
const PASS_LEG: u32 = 3_000;
/// One leg of the valley corridor, m. It is 2 000 m of ground more than the pass over both legs,
/// which the climb term matches at `w = 5`.
const VALLEY_LEG: u32 = 4_000;

/// Plan over a [`pass_vs_valley`] map and report `(raw distance, took the pass?)`.
fn plan_corridor(bytes: &[u8]) -> (u32, bool) {
    let (a, b) = (at(0, 0), at(0, 2));
    let (res, obcr, _) = plan_p(bytes, a, b, "Corridor", BikeType::Road);
    let route = res.expect("both corridors are legal, so one of them plans");
    let over_the_pass = route_points(&obcr).iter().any(|p| (p.lon, p.lat) == at(1, 1));
    (route.total_distance_m, over_the_pass)
}

/// Same map and endpoints, only the climb weight moves: a climb-blind router takes the short steep
/// pass and a road-weighted one takes the long flat valley.
#[test]
fn the_climb_weight_steers_from_the_pass_to_the_valley() {
    let graph = pass_vs_valley();

    // The packer really baked the climb the assertions below spend.
    let (up, down) = obc_pack::nav::integrate_edge_ascent(&[at(0, 0), at(1, 1)], &mut Hillside);
    assert!(
        (up as i64 - ROW_CLIMB_M as i64).abs() <= 4,
        "the pass leg bakes about {ROW_CLIMB_M} m of ascent, baked {up}"
    );
    assert_eq!(down, 0, "and nothing coming back down: ascent is directional");

    let blind = map_with_terrain(&graph, &[climb_profile("Blind", 0)], &mut Hillside);
    assert_eq!(plan_corridor(&blind), (3148, true), "climb-blind, the pass is the shorter way");

    let road = map_with_terrain(&graph, &[climb_profile("Road", 10)], &mut Hillside);
    assert_eq!(plan_corridor(&road), (3148, false), "at the Road weight the climb outprices 2 km");
}

/// The corridors tie at `w = 5`, so `w = 4` still takes the pass and `w = 6` takes the valley. A
/// cost model that folded the climb term inside the shift, scaled it by the way-kind multiplier, or
/// charged both directions of an edge would miss this.
#[test]
fn the_climb_crossover_lands_where_the_formula_says() {
    let graph = pass_vs_valley();
    let below = map_with_terrain(&graph, &[climb_profile("w4", 4)], &mut Hillside);
    let above = map_with_terrain(&graph, &[climb_profile("w6", 6)], &mut Hillside);
    assert_eq!(plan_corridor(&below), (3148, true), "4 * 400 = 1 600 < 2 000, so the pass wins");
    assert_eq!(plan_corridor(&above), (3148, false), "6 * 400 = 2 400 > 2 000, so the valley wins");
}

/// A map baked with real ascents, routed under a `climb_weight = 0` profile, must emit the same
/// bytes as a terrain-free map. The two legal zeroes are proved to give the same costing, not
/// assumed to.
#[test]
fn climb_weight_zero_over_real_ascents_is_the_pre_elevation_router() {
    let graph = grid3(false);
    let flat = map_with(&graph);
    let hilly = map_with_terrain(&graph, &[neutral_profile()], &mut Hillside);

    // The two maps differ on the wire and a grid column really climbs a row of the hillside.
    assert_ne!(flat, hilly, "the terrain-baked map must differ on the wire");
    let (up, _) = obc_pack::nav::integrate_edge_ascent(&[at(0, 0), at(0, 500), at(1, 0)], &mut Hillside);
    assert!(up >= ROW_CLIMB_M as u16 - 4, "a grid column climbs a row of the hillside, baked {up} m");

    let (route, obcr) = plan_with_elevation(&hilly, &mut NullElevation);
    assert_eq!(digest(&obcr), NULL_PATH_DIGEST, "a climb-blind plan over baked ascents moves no byte");
    assert_eq!(route.total_distance_m, 4474);
    let (_, flat_obcr) = plan_with_elevation(&flat, &mut NullElevation);
    assert_eq!(obcr, flat_obcr, "and is the same route the terrain-free map plans");
}

/// Over the [`Knoll`], whose paths differ in climb but tie in distance, the search finds the exact
/// optimum a directional Dijkstra finds. Equality is asserted, not the bound, because an
/// inadmissible `h` shows up here as an unreachable cheaper reference long before it breaks the
/// bound.
#[test]
fn the_climb_aware_optimum_matches_a_directional_dijkstra() {
    let graph = grid3(false);
    let prof = climb_profile("Climby", 10);
    let bytes = map_with_terrain(&graph, std::slice::from_ref(&prof), &mut Knoll);

    // The packer's own integrator supplies each direction's ascent, so the ground truth comes from
    // the code that wrote the bytes.
    let n = graph.nodes.len();
    let mut adj = vec![Vec::<(usize, u32)>::new(); n];
    for e in &graph.edges {
        let (fwd, back) = obc_pack::nav::integrate_edge_ascent(&e.polyline, &mut Knoll);
        let base = weighted(e.length_m, e.kind, &prof);
        let charge = |asc: u16| base + asc as u32 * prof.climb_weight as u32;
        adj[e.a as usize].push((e.b as usize, charge(fwd)));
        adj[e.b as usize].push((e.a as usize, charge(back)));
    }
    assert!(
        adj.iter().flatten().any(|&(_, w)| w > weighted(EDGE_COST, 0, &prof)),
        "the knoll must make some edge cost more than its ground length"
    );
    let mut dist = vec![u32::MAX; n];
    let mut done = vec![false; n];
    dist[0] = 0;
    for _ in 0..n {
        let Some(u) = (0..n).filter(|&i| !done[i] && dist[i] != u32::MAX).min_by_key(|&i| dist[i]) else { break };
        done[u] = true;
        for &(v, w) in &adj[u] {
            dist[v] = dist[v].min(dist[u].saturating_add(w));
        }
    }
    let reference = dist[8];
    assert!(reference > 0 && reference != u32::MAX, "the reference is a real finite cost");

    let (c0, c8) = (at(0, 0), at(2, 2));
    let (res, obcr, _) = plan_p(&bytes, c0, c8, "Knoll", BikeType::Road);
    res.expect("the knoll grid plans");
    let mut seq: Vec<usize> = Vec::new();
    for pt in route_points(&obcr) {
        if let Some(node) = graph.nodes.iter().find(|nd| nd.coord == (pt.lon, pt.lat)) {
            if seq.last() != Some(&(node.id as usize)) {
                seq.push(node.id as usize);
            }
        }
    }
    let found: u32 = seq
        .windows(2)
        .map(|w| adj[w[0]].iter().find(|&&(v, _)| v == w[1]).expect("consecutive nodes share an edge").1)
        .sum();

    assert_eq!(found, reference, "the search did not find the climb-aware optimum ({found} vs {reference})");
    assert!(found <= reference * 13 / 10);
}

/// A single leg at `w = 100` accumulates a `g` twenty-six times its ground length, and the header
/// still reports the ground length.
#[test]
fn the_displayed_distance_ignores_the_climb_term_entirely() {
    let (a, pass) = (at(0, 0), at(1, 1));
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: pass }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, pass], length_m: PASS_LEG, kind: 0 }],
    };
    let bytes = map_with_terrain(&graph, &[climb_profile("Heavy", 100)], &mut Hillside);
    let (res, _, _) = plan_p(&bytes, a, pass, "Uphill", BikeType::Road);
    let route = res.expect("a single uphill edge plans");
    assert_eq!(route.total_distance_m, 1574, "the header total is the raw ground length");
    assert!(route.total_distance_m < ROW_CLIMB_M * 100, "and not the weighted g");
}

/// At `w = 255` the pass leg's climb term alone passes `u16::MAX`, so the frontier's `g` clamps. A
/// saturated node must be maximally unattractive, never wrapped: the plan still completes and still
/// takes the flat valley.
#[test]
fn a_saturating_climb_weight_clamps_instead_of_wrapping() {
    assert!(ROW_CLIMB_M * 255 > u16::MAX as u32, "the fixture must actually reach saturation");
    let bytes = map_with_terrain(&pass_vs_valley(), &[climb_profile("Absurd", 255)], &mut Hillside);
    assert_eq!(plan_corridor(&bytes), (3148, false), "a saturated pass is unattractive, not cheap");
}

/// When every route saturates there is no ordering left, and the contract is only that the search
/// degrades: it must still terminate and return a real route.
#[test]
fn a_wholly_saturated_frontier_still_returns_a_route() {
    let graph = grid3(false);
    let bytes = map_with_terrain(&graph, &[climb_profile("Absurd", 255)], &mut Knoll);
    let (c0, c8) = (at(0, 0), at(2, 2));
    let (res, obcr, _) = plan_p(&bytes, c0, c8, "Saturated", BikeType::Road);
    let route = res.expect("a saturated frontier still drains to the goal");
    assert!(route.total_distance_m >= 4400, "a real path, not a wrapped shortcut");
    let pts = route_points(&obcr);
    assert_eq!((pts[0].lon, pts[0].lat), c0);
    assert_eq!((pts[pts.len() - 1].lon, pts[pts.len() - 1].lat), c8);
}

#[test]
fn imported_surface_requires_unique_directed_continuous_graph_attribution() {
    let a = BASE;
    let b = (BASE.0 + 10_000, BASE.1);
    let from = (BASE.0 + 3_000, BASE.1);
    let to = (BASE.0 + 3_500, BASE.1);
    let mut graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 1113, kind: 32 }],
    };
    let check = |graph: &NavGraph, from, to| {
        let bytes = map_with(graph);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        obc_route::attribution::attribute_segment(&reader, &mut NavTileCache::new(), from, to).unwrap()
    };
    assert_eq!(check(&graph, from, to), 1);
    assert_eq!(check(&graph, to, from), 1);
    assert_eq!(check(&graph, from, (to.0, to.1 + 150)), 0);
    let c = (a.0, a.1 + 40);
    let d = (b.0, b.1 + 40);
    graph.nodes.extend([Node { id: 2, coord: c }, Node { id: 3, coord: d }]);
    graph.edges.push(Edge { a: 2, b: 3, polyline: vec![c, d], length_m: 1113, kind: 96 });
    assert_eq!(check(&graph, from, to), 0, "a parallel road inside the tolerance is ambiguous");
    graph.edges.truncate(1);
    graph.edges[0].polyline = vec![a, b, a, b];
    graph.edges[0].length_m = 3339;
    assert_eq!(check(&graph, from, to), 0, "repeated occurrences on one edge are ambiguous");
}

#[test]
fn graph_interior_terrain_gaps_survive_valid_emit_samples() {
    let graph = grid3(false);
    let incomplete = map_with_terrain(&graph, &[neutral_profile()], &mut HolyRidge);
    let complete = map_with_terrain(&graph, &[neutral_profile()], &mut Ridge);
    for (map, expected_complete) in [(&incomplete, false), (&complete, true)] {
        let (stats, bytes) = plan_with_elevation(map, &mut Ridge);
        let src = SliceSource(&bytes);
        let index = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&index, &src);
        assert!(route_points(&bytes).iter().all(|p| p.elevation().is_some()));
        let facts = route.interval_facts(0, stats.total_distance_m).unwrap();
        assert_eq!(facts.complete_elevation(), expected_complete);
        assert_eq!(facts.ascent_m, stats.total_ascent_m);
        assert_eq!(facts.descent_m, stats.total_descent_m);
    }
}

#[test]
fn unreadable_parallel_edge_cannot_prove_unique_import_attribution() {
    use obc_formats::io::{ByteSource, Error};
    struct UnreadableEdge<'a> {
        bytes: &'a [u8],
        offset: u64,
        failures: std::cell::Cell<usize>,
    }
    impl ByteSource for UnreadableEdge<'_> {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            if offset == self.offset {
                self.failures.set(self.failures.get() + 1);
                return Err(Error::Io);
            }
            SliceSource(self.bytes).read_at(offset, out)
        }
    }
    let mut graph = NavGraph::default();
    for lane in 0..2 {
        let polyline: Vec<_> = (0..124).map(|i| (BASE.0 + i * 80, BASE.1 + lane * 40)).collect();
        let a = graph.nodes.len() as u32;
        graph.nodes.extend([Node { id: a, coord: polyline[0] }, Node { id: a + 1, coord: polyline[123] }]);
        graph.edges.push(Edge { a, b: a + 1, polyline, length_m: 1095, kind: if lane == 0 { 32 } else { 96 } });
    }
    let bytes = map_with(&graph);
    let source = SliceSource(&bytes);
    let tables = MapTables::parse(&source).unwrap();
    let cache = MapCache::new();
    let readable = Reader::new(&source, &tables, &cache);
    assert_eq!(readable.nav_directory().edge_chunk_count, 2, "one edge per source block");
    let fault = UnreadableEdge {
        bytes: &bytes,
        offset: readable.nav_directory().edge_pool_offset + 512,
        failures: std::cell::Cell::new(0),
    };
    let reader = Reader::new(&fault, &tables, &cache);
    for offset in [1_000, 4_000] {
        let from = (BASE.0 + offset, BASE.1);
        let to = (from.0 + 500, from.1);
        assert_eq!(obc_route::attribution::attribute_segment(&readable, &mut NavTileCache::new(), from, to), Ok(0));
        assert_eq!(
            obc_route::attribution::attribute_segment(&reader, &mut NavTileCache::new(), from, to),
            Err(Error::Io),
            "node and interior-anchor queries must both refuse incomplete evidence"
        );
    }
    assert_eq!(fault.failures.get(), 2);
}
