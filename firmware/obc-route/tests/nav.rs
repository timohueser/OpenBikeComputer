//! Host tests for the §8 A* router (R3, #465): fixture graphs are serialized with the
//! real `obc-pack` writer and parsed with the real `obc-reader` (the same
//! writer↔reader loop `obc-pack/tests/nav_round_trip.rs` pins), so the router is
//! exercised end to end over genuine on-wire bytes — snap, search, exhaustion, the
//! graph-tile cache, and the emitted OBCR's round trip through `RouteReader`.

mod common;

use common::{decode, VecSink};
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::nav::{plan_route, NavError, NavPhase, NavPlanner, NavScratch};
use obc_route::{RouteIndex, RouteObjectInfo, RouteReader, SliceSource};

/// Global bbox `(min_lon, min_lat, max_lon, max_lat)` µdeg — roomy so the node
/// quadtree genuinely subdivides around the fixtures (multiple chunks ⇒ the tile
/// cache is exercised for real).
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 1_000_000, 1_000_000);

/// Grid spacing, µdeg (~1 113 m of latitude — near the equator both axes match).
const SP: i32 = 10_000;
/// Grid origin (lon, lat) µdeg.
const BASE: (i32, i32) = (500_000, 500_000);
/// Every grid edge's cost, meters. Uniform and comfortably above the ~1 113 m
/// straight line between adjacent nodes, so the heuristic stays admissible and every
/// monotone corner-to-corner path costs exactly `4 × EDGE_COST` — the known optimum
/// is path-shape independent.
const EDGE_COST: u32 = 1_200;
/// The diagonal shortcut's cost: above its ~3 148 m straight line (admissible), well
/// below the 4 800 m grid alternative (the unique optimum when present).
const SHORTCUT_COST: u32 = 3_200;

/// A **neutral** all-1.0× profile (every highway/surface multiplier `16`): with it the weighted
/// search reduces to the pre-N3 unweighted A*, so the kind-agnostic fixtures below stay meaningful.
/// The shipped defaults are opinionated (Road/Gravel/MTB/Touring); the fixtures deliberately pin a
/// neutral baseline so a profile-weighting test's math is the *only* thing under test.
fn neutral_profile() -> NavProfile {
    NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8] }
}

/// A test profile from an explicit set of highway-class overrides (surface all neutral). Each
/// override is `(highway_class, u8 1/16 multiplier)` — `16` = 1.0×, `0` = forbidden; every unlisted
/// class stays 1.0×. Edges in these fixtures carry `kind = highway_class` (surface class 0), so the
/// effective multiplier is exactly the listed highway byte.
fn profile(name: &str, overrides: &[(u8, u8)]) -> NavProfile {
    let mut highway = [16u8; 32];
    for &(class, m) in overrides {
        highway[class as usize] = m;
    }
    NavProfile { name: name.into(), highway, surface: [16; 8] }
}

/// Serialize `graph` into a minimal v9 map (one empty geometry leaf, no styles) with the given
/// routing `profiles` (index-selectable by the router). At least one profile must be present.
fn map_with_profiles(graph: &NavGraph, profiles: &[NavProfile]) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, profiles);
    assert_eq!(dropped, 0);
    bin
}

/// Serialize `graph` into a minimal v9 map with a single **neutral** profile (the default fixture:
/// kind-agnostic tests route under all-1.0× weights).
fn map_with(graph: &NavGraph) -> Vec<u8> {
    map_with_profiles(graph, &[neutral_profile()])
}

/// Grid node coord: column → lon, row → lat.
fn at(row: i32, col: i32) -> (i32, i32) {
    (BASE.0 + col * SP, BASE.1 + row * SP)
}

/// A 3×3 grid (node id = `row*3 + col`) with orthogonal edges, each carrying one
/// interior shape point nudged 500 µdeg (~55 m) off-axis so the geometry survives the
/// OBCR decimator and pins edge stitching. `shortcut` adds one diagonal edge
/// `0 → 8` that beats every grid path.
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

/// A straight west→east path graph: `n` nodes `step_udeg` of longitude apart (near the
/// fixture's ~0.5° latitude, ~0.11 m/µdeg), consecutive nodes joined by one
/// `cost_m`-meter edge. Reaching the far end forces the router to track every node on
/// the line — the deterministic range fixture.
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

/// Parse `bytes` and run the router under bike profile `profile_idx` with a full-size scratch +
/// fresh tile cache, returning `(result, obcr_bytes, cache_stats)`.
fn plan_p(
    bytes: &[u8],
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
    profile_idx: u8,
) -> (Result<obc_route::RouteStats, NavError>, Vec<u8>, obc_reader::NavCacheStats) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized v9 map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, name, profile_idx, &mut scratch, &mut tiles, &mut sink);
    (res, sink.buf, tiles.stats())
}

/// [`plan_p`] under profile 0 (the fixtures' neutral profile) — the kind-agnostic helper.
fn plan(
    bytes: &[u8],
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
) -> (Result<obc_route::RouteStats, NavError>, Vec<u8>, obc_reader::NavCacheStats) {
    plan_p(bytes, from, to, name, 0)
}

/// Decode an emitted OBCR's full point list (all chunks stitched).
fn route_points(obcr: &[u8]) -> Vec<obc_route::RoutePoint> {
    let src = SliceSource(obcr);
    let idx = RouteIndex::read(&src).expect("the emitted OBCR parses");
    let r = RouteReader::new(&idx, &src);
    let mut pts = Vec::new();
    for k in 0..idx.chunks().len() {
        let chunk = decode(&r, k);
        // Chunks share their seam point; drop the duplicate when stitching.
        let skip = usize::from(k > 0);
        pts.extend_from_slice(&chunk[skip..]);
    }
    pts
}

/// Corner-to-corner over the uniform grid: the optimum is `4 × EDGE_COST` whatever
/// monotone path A* picks, the stitched geometry keeps every shape point (9 points:
/// 5 nodes + 4 nudged midpoints), and consecutive settles hit the graph-tile cache.
/// The emitted OBCR round-trips through `RouteReader` with the summed-cost length,
/// zero elevation everywhere, and no waypoints.
#[test]
fn grid_route_matches_known_optimum_and_round_trips() {
    let bytes = map_with(&grid3(false));
    let from = (BASE.0 + 100, BASE.1 - 100); // ~15 m from node 0
    let goal = at(2, 2);
    let to = (goal.0 - 100, goal.1 + 100); // ~15 m from node 8
    let (res, obcr, stats) = plan(&bytes, from, to, "Water stop");
    let route = res.expect("a grid route plans");

    assert_eq!(route.total_distance_m, 4 * EDGE_COST, "summed edge costs, the known optimum");
    assert_eq!(route.point_count, 9, "5 nodes + 4 nudged midpoints survive the decimator");
    assert_eq!((route.total_ascent_m, route.total_descent_m), (0, 0), "no DEM — flat by construction");
    assert_eq!((route.min_ele_m, route.max_ele_m), (0, 0));
    assert_eq!(route.waypoint_count, 0);
    assert!(stats.hits > 0, "consecutive settles re-hit resident graph tiles (got {stats:?})");
    assert!(stats.misses > 0, "the graph was actually read (got {stats:?})");

    // Round trip through the normal route path.
    let src = SliceSource(&obcr);
    let idx = RouteIndex::read(&src).expect("round trip");
    assert_eq!(idx.name(), "Water stop");
    let info = RouteObjectInfo::read(&src).unwrap();
    assert_eq!(info.distance_m, 4 * EDGE_COST, "header length = summed edge costs");
    assert_eq!(info.ascent_m, 0);

    let pts = route_points(&obcr);
    assert_eq!(pts.len(), 9);
    assert_eq!((pts[0].lon, pts[0].lat), at(0, 0), "starts at the snapped start node");
    assert_eq!((pts[8].lon, pts[8].lat), goal, "ends at the snapped goal node");
    assert!(pts.iter().all(|p| p.ele == 0), "every point is elevation-none (stored 0)");
}

/// The diagonal shortcut beats every grid path — A* must find the unique optimum;
/// routed *backwards* (node 8 → node 0) the edge is traversed `b → a`, pinning the
/// reversed geometry decode: the polyline comes out end-to-start, seam-exact.
#[test]
fn shortcut_wins_and_reversed_edge_geometry_is_exact() {
    let bytes = map_with(&grid3(true));
    let (c0, c8) = (at(0, 0), at(2, 2));
    let mid = ((c0.0 + c8.0) / 2 + 500, (c0.1 + c8.1) / 2 - 500);

    // Forward: the shortcut runs a→b, traversed as stored.
    let (res, obcr, _) = plan(&bytes, (c0.0 + 100, c0.1), (c8.0 - 100, c8.1), "Fwd");
    assert_eq!(res.unwrap().total_distance_m, SHORTCUT_COST, "the shortcut is the unique optimum");
    let pts = route_points(&obcr);
    assert_eq!(
        pts.iter().map(|p| (p.lon, p.lat)).collect::<Vec<_>>(),
        vec![c0, mid, c8],
        "forward traversal keeps record order"
    );

    // Backward: same edge, traversed b→a — the decode must reverse it exactly.
    let (res, obcr, _) = plan(&bytes, (c8.0 - 100, c8.1), (c0.0 + 100, c0.1), "Rev");
    assert_eq!(res.unwrap().total_distance_m, SHORTCUT_COST);
    let pts = route_points(&obcr);
    assert_eq!(
        pts.iter().map(|p| (p.lon, p.lat)).collect::<Vec<_>>(),
        vec![c8, mid, c0],
        "b→a traversal emits the polyline reversed"
    );
}

/// Two components in snap range of the endpoints but not of each other: the frontier
/// empties without reaching the goal ⇒ `NoPath`.
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
    let (res, obcr, _) = plan(&map_with(&graph), (a0.0 + 100, a0.1), (b0.0 - 100, b0.1), "x");
    assert_eq!(res, Err(NavError::NoPath));
    assert!(obcr.is_empty(), "a failed plan writes nothing");
}

/// A 4-entry scratch can't track the ≥5 nodes any corner-to-corner grid path needs:
/// the table fills mid-relaxation ⇒ `Exhausted`, deterministically.
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
        plan_route(&r, (BASE.0 + 100, BASE.1), (goal.0 - 100, goal.1), "x", 0, &mut scratch, &mut tiles, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted));
}

/// **Exhaustion salvage** (N4): a goal discovered *before* the table fills must still be reached
/// even when a later insert fails — the pre-N4 code aborted `Exhausted` on that first failed insert.
/// Fixture: a start with two ways to the goal — a single **expensive direct** edge (goal tracked at
/// the very first settle) and a **cheap chain** of many nodes. A* settles the cheap chain (better
/// `f`) and fills the 6-slot table mid-chain; the next new-node insert fails and latches `table_full`
/// while the chain still hasn't relaxed the goal. Old behavior: `Exhausted` right there. New: the
/// search continues, the frontier drains to the still-queued goal, and it pops — success via the
/// direct edge (a route that *exceeds the ε bound*, the documented accepted consequence of a full
/// table). A genuinely-unreachable goal (`tiny_scratch_exhausts`, `far_beyond_range_*`) still
/// `Exhausted`s, and a disconnected one (`disconnected_graph_is_no_path`) still `NoPath`s.
#[test]
fn goal_tracked_before_fill_survives_exhaustion() {
    // 9 chain nodes 0..=8 west→east, 3 000 µdeg apart (~230 m/edge at this lat); goal = node 8.
    let step = 3_000;
    let coord = |i: u32| (BASE.0 + i as i32 * step, BASE.1);
    let mut nodes: Vec<Node> = (0..9).map(|i| Node { id: i, coord: coord(i) }).collect();
    let mut edges: Vec<Edge> = (0..8)
        .map(|i| Edge { a: i, b: i + 1, polyline: vec![coord(i), coord(i + 1)], length_m: 400, kind: 0 })
        .collect();
    // The expensive direct escape 0→8: 20 km for a ~1.8 km straight line — admissible, and 6× the
    // 3 200 m chain, so it's plainly past the ε bound (the accepted full-table outcome).
    edges.push(Edge { a: 0, b: 8, polyline: vec![coord(0), coord(8)], length_m: 20_000, kind: 0 });
    let _ = &mut nodes; // (kept mutable-free; readability)
    let bytes = map_with(&NavGraph { nodes, edges });

    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    // A 6-node table: it fills at node 4 (nodes 0,1,8 tracked at settle 0, then 2,3,4) — before the
    // chain reaches node 5, so the goal-via-chain never relaxes and node 8 pops on the direct edge.
    let mut scratch = NavScratch::<6>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let (from, to) = ((coord(0).0 + 50, coord(0).1), (coord(8).0 - 50, coord(8).1));
    let res = plan_route(&r, from, to, "Salvaged", 0, &mut scratch, &mut tiles, &mut sink);
    let route = res.expect("the goal was tracked before the fill ⇒ salvage returns it");
    assert_eq!(route.total_distance_m, 20_000, "the direct (suboptimal, past-ε) edge — the full-table path");
    assert_eq!(route_points(&sink.buf).len(), 2, "start → goal over the single direct edge");
}

/// An endpoint with no routable node within 250 m fails to snap ⇒ `NoPath` — for the
/// rider fix and the POI alike.
#[test]
fn unsnappable_endpoint_is_no_path() {
    let bytes = map_with(&grid3(false));
    let near0 = (BASE.0 + 100, BASE.1);
    // ~4.4 km from the nearest node — well past the 250 m snap radius.
    let lost = (BASE.0 + 60_000, BASE.1 + 60_000);
    let (res, _, _) = plan(&bytes, lost, near0, "x");
    assert_eq!(res, Err(NavError::NoPath), "`from` out of snap range");
    let (res, _, _) = plan(&bytes, near0, lost, "x");
    assert_eq!(res, Err(NavError::NoPath), "`to` out of snap range");
}

/// A map whose nav section is empty (no routable ways) can't snap anything ⇒ `NoPath`.
#[test]
fn empty_graph_is_no_path() {
    let bytes = map_with(&NavGraph::default());
    let (res, _, _) = plan(&bytes, (500_000, 500_000), (505_000, 500_000), "x");
    assert_eq!(res, Err(NavError::NoPath));
}

/// With **no distance cap** (Timo, post-#496) a far-beyond-range target is attempted
/// and fails by **exhausting the fixed table** — the device's honest range answer,
/// which the app maps to the "Too far to route here" tier. A ~30 km line at the
/// capped sim table (1536 nodes): every node on the path must be tracked, so the
/// table fills long before the goal.
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
    let from = (BASE.0 + 30, BASE.1);
    let to = (BASE.0 + 1_999 * 135 - 30, BASE.1); // ~30 km away — pre-cap this was TooFar unseen
    let res = plan_route(&r, from, to, "x", 0, &mut scratch, &mut tiles, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted), "the burn-before-fail search ends at the table, not a pre-check");
    assert!(sink.buf.is_empty(), "an exhausted plan writes nothing");
}

/// Both endpoints snapping to the same node degenerate to a single-point route of
/// length 0 that still round-trips as a valid OBCR.
#[test]
fn same_snap_node_emits_single_point_route() {
    let bytes = map_with(&grid3(false));
    let (res, obcr, _) = plan(&bytes, (BASE.0 + 100, BASE.1), (BASE.0 - 100, BASE.1), "Here");
    let route = res.expect("a degenerate route still plans");
    assert_eq!(route.total_distance_m, 0);
    assert_eq!(route.point_count, 1);
    let pts = route_points(&obcr);
    assert_eq!(pts.len(), 1);
    assert_eq!((pts[0].lon, pts[0].lat), at(0, 0));
}

/// Miri model of the **device slot lifecycle** (#501 on-glass fault hunt): a `static mut
/// MaybeUninit<NavPlanner>` (re)written per request and stepped exactly the way `ride.rs` does —
/// `NavBuffers`-shaped `&'static mut` handles from `addr_of_mut!`, an `assume_init_ref().phase()`
/// read before every step, `assume_init_mut()` inside the step frame with the scratch/tiles field
/// borrows alive across the call, a fresh `Reader` view per pass, slot **overwrite without drop**
/// on a replacing request, and the cancel interleaving (abandon mid-search, then a new request
/// re-writes the slot). Under Miri this proves the pattern free of assume_init-before-write,
/// `&mut` aliasing, and uninitialized reads; natively it pins the interleavings.
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
    let tables = MapTables::parse(&tables_src).unwrap(); // the device's boot-parsed tables
    let cache = MapCache::new();
    let from = (BASE.0 + 100, BASE.1 - 100);
    let goal = at(2, 2);
    let to = (goal.0 - 100, goal.1 + 100);

    // One device-shaped step: fresh source + Reader + sink view, phase read, then the step with
    // the planner/scratch/tiles field borrows all live across the call — `ride.rs`'s nav_step.
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
        planner.step(&reader, &mut *nav.scratch, &mut *nav.tiles, sink)
    }
    let mut nav = nav;

    // Request 1: write the slot, step to completion (the plain short-route flow).
    nav.planner.write(NavPlanner::new(from, to, "Dev", 0));
    let mut sink = VecSink::default();
    let stats = loop {
        match step_once(&mut nav, &bytes, &tables, &cache, &mut sink) {
            obc_route::Step::Running => {}
            obc_route::Step::Done(stats) => break stats,
            obc_route::Step::Failed(e) => panic!("device-model plan failed: {e:?}"),
        }
    };
    assert_eq!(stats.total_distance_m, 4 * EDGE_COST);
    let first = sink.buf.clone();

    // Request 2 begins and is CANCELLED mid-search: the slot is overwritten (no drop — the
    // device ptr-writes over the old plan), stepped a few times, then simply never stepped again.
    nav.planner.write(NavPlanner::new(from, to, "Cancelled", 0));
    let mut cancelled_sink = VecSink::default();
    for _ in 0..2 {
        assert!(matches!(step_once(&mut nav, &bytes, &tables, &cache, &mut cancelled_sink), obc_route::Step::Running));
    }
    assert!(cancelled_sink.buf.is_empty(), "a cancelled (abandoned) plan wrote nothing");

    // Request 3 replaces the abandoned plan — another overwrite-without-drop — and completes;
    // the emitted bytes must match request 1's exactly (no state bleed through the slot).
    nav.planner.write(NavPlanner::new(from, to, "Dev", 0));
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

/// The v9 §8.3 record head is 13 bytes (**odd**), so the first neighbor entry of every record
/// begins at an **odd** offset relative to the record start. Record 0 starts at chunk offset 0, so
/// its neighbor fields (`id u32 @+13`, `dlat i16 @+17`, `dlon i16 @+19`, `edge_id u32 @+21`, …)
/// decode at odd, unaligned offsets — and the 15-byte (odd) entry then shifts each following
/// record's start parity by degree, so multi-record chunks also decode record heads at odd offsets.
/// Either way the byte-wise-decode contract (PR #501's on-glass HardFault: an ARM backend
/// `ldrd`-fusion over these bytes; fixed with `+strict-align`) stays exercised. The full UB tripwire
/// is Miri over this suite (see the module doc).
#[test]
fn record_stride_keeps_odd_offsets_exercised() {
    assert_eq!(obc_reader::NAV_NODE_FIXED_LEN % 2, 1, "the fixed record head is odd-length");
    assert_eq!(obc_reader::NAV_NEIGHBOR_LEN, 15, "v9 neighbor entries are 15 bytes");
    // ⇒ every record's first neighbor entry begins at record_start + 13 (odd), so its multi-byte
    // fields decode at odd, unaligned offsets. The 15-byte entry keeps consecutive record starts
    // parity-varying, so record heads are exercised at odd offsets in multi-record chunks too.
}

/// The slimmed entry layout holds: 26 B/node (24 B entry + 2 B heap slot) plus the two
/// length fields — per-target `NAV_MAX_NODES` sized (also compile-time asserted in the
/// module for the device profile; this keeps the numbers visible in the test log).
#[test]
fn scratch_fits_the_per_target_budget() {
    let size = core::mem::size_of::<NavScratch<{ obc_route::NAV_MAX_NODES }>>();
    assert!(size <= 26 * obc_route::NAV_MAX_NODES + 8, "NavScratch is {size} B — the 26 B/node layout drifted");
}

/// A ~9 km straight-line path graph (600 nodes, 15 m apart): the pre-range-fix table
/// (300 nodes — every node on the path must be tracked to reach the goal) provably
/// exhausts, while the capped sim/LM20-size table (1536 nodes = the 40 kB nav budget)
/// plans it — pinning the 2026-07-06 range fix (bigger per-target tables + the
/// slimmed 26 B/node entry that pays for them).
#[test]
fn long_line_exhausts_old_table_but_plans_on_the_sim_table() {
    let bytes = map_with(&line_graph(600, 135, 15));
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let from = (BASE.0 + 30, BASE.1);
    let to = (BASE.0 + 599 * 135 - 30, BASE.1);

    // The old fixed table: 300 tracked nodes < the 600 the path needs ⇒ Exhausted.
    let mut small = NavScratch::<300>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, "x", 0, &mut small, &mut tiles, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted), "the pre-fix 300-node table can't span ~9 km");

    // The capped sim/LM20-size table (1536 = the 40 kB nav budget) plans the same route.
    let mut big = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, "x", 0, &mut big, &mut tiles, &mut sink);
    let route = res.expect("the capped sim table spans the ~9 km line");
    assert_eq!(route.total_distance_m, 599 * 15, "summed edge costs over the whole line");
}

/// The **weighted `g`** saturates at `u16::MAX` meters instead of wrapping: a path whose
/// summed weighted cost exceeds 65 535 m still plans (the saturated `g` is just maximally
/// unattractive), and nothing panics or mis-orders. The **displayed** total is the honest
/// unweighted `length_m` sum in `u32` — it does *not* clamp at the `u16` search ceiling
/// (N3 distance honesty: the header total is real ground meters, not the weighted `g`).
#[test]
fn saturated_costs_plan_without_panicking() {
    // Three nodes ~100 m apart but with absurd 60 km edge costs: g saturates on hop 2.
    // n1 nudged off-axis so the collinear-point decimator keeps it.
    let (n0, n1, n2) = (BASE, (BASE.0 + 900, BASE.1 + 500), (BASE.0 + 1_800, BASE.1));
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: n0 }, Node { id: 1, coord: n1 }, Node { id: 2, coord: n2 }],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![n0, n1], length_m: 60_000, kind: 0 },
            Edge { a: 1, b: 2, polyline: vec![n1, n2], length_m: 60_000, kind: 0 },
        ],
    };
    let (res, obcr, _) = plan(&map_with(&graph), (n0.0 + 30, n0.1), (n2.0 - 30, n2.1), "Far");
    let route = res.expect("a saturated-cost path still plans");
    assert_eq!(
        route.total_distance_m, 120_000,
        "displayed total is the honest u32 length sum, past the u16 weighted-g ceiling"
    );
    assert_eq!(route_points(&obcr).len(), 3, "the geometry is intact regardless");
}

/// The resumable planner (#499): manual stepping produces a **byte-identical** OBCR to the
/// one-shot `plan_route` (which is itself the step loop), every search step respects the N4
/// miss-budget (≤ [`NAV_MISSES_PER_STEP`] tile-cache misses, capped at [`NAV_SETTLES_PER_STEP_CAP`]
/// settles), the phase sequence is snap → search → emit → done, and multiple `Running` steps
/// genuinely occur (the plan is spread across host passes, which is the whole point).
#[test]
fn stepped_plan_matches_one_shot_and_respects_budgets() {
    use obc_route::nav::{NAV_MISSES_PER_STEP, NAV_SETTLES_PER_STEP_CAP};
    let bytes = map_with(&line_graph(120, 135, 15)); // ~1.8 km line — a multi-step search
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let from = (BASE.0 + 30, BASE.1);
    let to = (BASE.0 + 119 * 135 - 30, BASE.1);

    // One-shot reference.
    let mut scratch = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut one_shot = VecSink::default();
    let reference = plan_route(&r, from, to, "Stepped", 0, &mut scratch, &mut tiles, &mut one_shot)
        .expect("the line plans one-shot");

    // Manual stepping over the same fixture.
    let mut planner = NavPlanner::new(from, to, "Stepped", 0);
    let mut tiles = NavTileCache::new();
    let mut stepped = VecSink::default();
    let mut steps = 0u32;
    let mut saw = (false, false, false); // (snap, search, emit) phases observed
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
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut stepped);
        if phase == NavPhase::Search {
            // Miss budget, with a one-settle spillover bound: the budget check trails each settle,
            // so entering the step's final settle the delta is ≤ NAV_MISSES_PER_STEP − 1, and that
            // settle's relaxation is a degenerate one-point quadtree walk — it reads the single node
            // chunk whose leaf contains the settled coord (this fixture's line-graph coords never
            // sit on a quadtree split line, so exactly one leaf matches), i.e. at most one more miss
            // ⇒ delta ≤ NAV_MISSES_PER_STEP. The +1 is documented slack for a coord landing exactly
            // on a leaf boundary (the walk then visits the sibling leaf too). Unconditional — this
            // is the assertion that actually pins the miss pacing.
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

    // Terminal idempotence: stepping again re-returns Done without touching the sink.
    let len = stepped.buf.len();
    assert!(matches!(planner.step(&r, &mut scratch, &mut tiles, &mut stepped), obc_route::Step::Done(_)));
    assert_eq!(stepped.buf.len(), len, "a terminal step writes nothing");
}

/// The N4 step budget, pinned at both ends on a small graph that fits inside the 8-slot tile cache:
/// **cold**, no search step ever reads more than [`NAV_MISSES_PER_STEP`] chunks (plus at most the
/// final settle's own reads); **warm**, once the whole graph is resident a step settles well past
/// the pre-N4 fixed budget of 8 (the [`NAV_SETTLES_PER_STEP_CAP`] opening up is the whole point —
/// a warm step does up to 8× the old work). One line, stepped, watching every search step.
#[test]
fn search_step_budget_is_miss_paced_cold_and_cap_opened_warm() {
    use obc_route::nav::{NAV_MISSES_PER_STEP, NAV_SETTLES_PER_STEP_CAP};
    // ~60-node line: its whole node section is a handful of 512 B chunks — fits inside 8 cache slots,
    // so after the first pass over it every settle is a hit and only the cap paces the step.
    let bytes = map_with(&line_graph(60, 200, 20));
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let from = (BASE.0 + 30, BASE.1);
    let to = (BASE.0 + 59 * 200 - 30, BASE.1);

    let mut scratch = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let mut planner = NavPlanner::new(from, to, "Budget", 0);
    let mut max_step_settles = 0u32;
    let mut first_search_misses: Option<u32> = None;
    loop {
        let phase = planner.phase();
        let settles_before = planner.settles();
        let misses_before = tiles.stats().misses;
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut sink);
        if phase == NavPhase::Search {
            let settle_delta = planner.settles() - settles_before;
            let miss_delta = tiles.stats().misses - misses_before;
            // Cold pace: the first search step (cache cold but for the snap) reads no more than the
            // miss budget — the step ends the moment it has spent its allowance of SD chunk reads.
            first_search_misses.get_or_insert(miss_delta);
            // Hard cap: no step ever settles past the cap, warm or cold.
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
    // Warm cap opened up: some step settled far more than the pre-N4 fixed 8 (the whole point of the
    // miss budget — a resident cache lets a step do real work instead of stopping at 8).
    assert!(max_step_settles > 8, "a warm step must settle past the old fixed-8 budget (got {max_step_settles})");
}

/// Cancelling = not stepping again: **nothing reaches the sink before the emit phase**, so a
/// plan abandoned mid-search leaves the sink pristine — the host only discards its own file.
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
    let from = (BASE.0 + 30, BASE.1);
    let to = (BASE.0 + 599 * 135 - 30, BASE.1);
    let mut planner = NavPlanner::new(from, to, "x", 0);
    for _ in 0..10 {
        // 2 snap steps + 8 search steps — well before the ~600-settle search could finish.
        assert_eq!(planner.step(&r, &mut scratch, &mut tiles, &mut sink), obc_route::Step::Running);
    }
    assert_eq!(planner.phase(), NavPhase::Search, "still searching when abandoned");
    drop(planner); // the cancel: never stepped again
    assert!(sink.buf.is_empty(), "snap + search phases are read-only — a cancelled plan wrote nothing");
}

// ---------------------------------------------------------------------------------------------
// Profile-weighted A* (epic #533, N3). The fixtures above route under a neutral all-1.0× profile;
// the tests below build explicit profiles + per-edge `kind`s and check the weighting arithmetic,
// forbidden-class skipping, distance honesty, the profile-index fallback, and the ε bound.
// Edges carry `kind = highway_class` (surface class 0), so the effective multiplier is exactly the
// profile's `highway[kind]` byte (`16` = 1.0×, `0` = forbidden).
// ---------------------------------------------------------------------------------------------

/// Arbitrary highway classes for the fixtures — the tests build both the edge `kind` and the
/// matching profile bytes, so these need not be real OSM class ids.
const K_CYCLE: u8 = 1;
const K_PRIMARY: u8 = 2;
const K_STEPS: u8 = 3;

/// The reader's exact integer edge weight for a `(length_m, kind)` under a profile:
/// `weighted = (length × ((highway[kind & 31] × surface[kind >> 5]) >> 4)) >> 4`. Replicated
/// in-test to compute the Dijkstra reference + the found path's weighted cost.
fn weighted(length_m: u32, kind: u8, p: &NavProfile) -> u32 {
    let mh = p.highway[(kind & 0x1F) as usize] as u32;
    let ms = p.surface[(kind >> 5) as usize] as u32;
    (length_m * ((mh * ms) >> 4)) >> 4
}

/// Two parallel A→B corridors of equal length: one all-cycleway, one all-primary, meeting at a
/// north (`C1`) / south (`C2`) midpoint node. The router must take the corridor the *profile*
/// prefers — cycleway under a cycle-loving profile (idx 0), primary under a primary-loving one
/// (idx 1). Guards multiplier indexing end to end (both the edge `kind` and the profile lookup).
#[test]
fn profile_steers_between_equal_length_corridors() {
    let a = (500_000, 500_000);
    let b = (500_000 + 2 * SP, 500_000);
    let c1 = (500_000 + SP, 500_000 + SP); // north (cycleway corridor)
    let c2 = (500_000 + SP, 500_000 - SP); // south (primary corridor)
    let hop = 2_000; // > the ~1 574 m straight line ⇒ admissible at 1.0×
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
    let (from, to) = ((a.0 + 30, a.1), (b.0 - 30, b.1));

    let (res, obcr, _) = plan_p(&bytes, from, to, "Cycle", 0);
    assert_eq!(res.unwrap().total_distance_m, 2 * hop, "same-length corridors ⇒ identical ground distance");
    let pts = route_points(&obcr);
    assert!(pts.iter().any(|p| (p.lon, p.lat) == c1), "the cycle-loving profile takes the cycleway (north) corridor");
    assert!(!pts.iter().any(|p| (p.lon, p.lat) == c2), "…and not the primary (south) one");

    let (res, obcr, _) = plan_p(&bytes, from, to, "Primary", 1);
    assert_eq!(res.unwrap().total_distance_m, 2 * hop);
    let pts = route_points(&obcr);
    assert!(pts.iter().any(|p| (p.lon, p.lat) == c2), "the primary-loving profile takes the primary (south) corridor");
    assert!(!pts.iter().any(|p| (p.lon, p.lat) == c1), "…and not the cycleway (north) one");
}

/// Pins the detour *arithmetic*, not just the ordering: a cycleway detour is a fixed **1.4×** the
/// direct primary edge's length. The detour (cycleway 1.0×) wins only while the primary's
/// multiplier exceeds that 1.4× — so at primary **2.0×** the detour is taken, and at primary
/// **1.25×** the direct primary is. Both profiles share the geometry; only the primary byte moves.
#[test]
fn detour_is_taken_exactly_when_the_multiplier_math_says_so() {
    let a = (500_000, 500_000);
    let b = (500_000 + 2 * SP, 500_000);
    let d = (500_000 + SP, 500_000 + SP); // detour apex (north)
    let direct = 2_400; // > the ~2 226 m straight line ⇒ admissible
    let leg = 1_680; // 2 × 1 680 = 3 360 = 1.4 × direct; each > its ~1 574 m straight line
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }, Node { id: 2, coord: d }],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a, b], length_m: direct, kind: K_PRIMARY },
            Edge { a: 0, b: 2, polyline: vec![a, d], length_m: leg, kind: K_CYCLE },
            Edge { a: 2, b: 1, polyline: vec![d, b], length_m: leg, kind: K_CYCLE },
        ],
    };
    let primary_2x = profile("p2", &[(K_PRIMARY, 32), (K_CYCLE, 16)]);
    let primary_125x = profile("p1.25", &[(K_PRIMARY, 20), (K_CYCLE, 16)]); // 1.25× = 20/16
    let bytes = map_with_profiles(&graph, &[primary_2x, primary_125x]);
    let (from, to) = ((a.0 + 30, a.1), (b.0 - 30, b.1));

    let (res, _, _) = plan_p(&bytes, from, to, "Detour", 0);
    assert_eq!(res.unwrap().total_distance_m, 2 * leg, "primary 2.0× > 1.4× ⇒ the cycleway detour wins");

    let (res, _, _) = plan_p(&bytes, from, to, "Direct", 1);
    assert_eq!(res.unwrap().total_distance_m, direct, "primary 1.25× < 1.4× ⇒ the direct primary wins");
}

/// A **forbidden** class (`mult == 0`) is skipped in relaxation, not routed. With the only direct
/// edge forbidden the router detours around it; with *every* edge forbidden the frontier drains to
/// an honest `NoPath` (no special-casing).
#[test]
fn forbidden_class_detours_then_no_paths() {
    let a = (500_000, 500_000);
    let b = (500_000 + 2 * SP, 500_000);
    let d = (500_000 + SP, 500_000 + SP);
    let no_steps = profile("no-steps", &[(K_STEPS, 0)]); // steps forbidden; cycleway stays 1.0×

    // The direct A→B edge is forbidden steps; a cycleway detour A→D→B is legal ⇒ detour.
    let detourable = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }, Node { id: 2, coord: d }],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 2_400, kind: K_STEPS },
            Edge { a: 0, b: 2, polyline: vec![a, d], length_m: 1_680, kind: K_CYCLE },
            Edge { a: 2, b: 1, polyline: vec![d, b], length_m: 1_680, kind: K_CYCLE },
        ],
    };
    let bytes = map_with_profiles(&detourable, std::slice::from_ref(&no_steps));
    let (res, obcr, _) = plan_p(&bytes, (a.0 + 30, a.1), (b.0 - 30, b.1), "Around", 0);
    assert_eq!(res.unwrap().total_distance_m, 2 * 1_680, "the forbidden direct edge is skipped ⇒ detour");
    assert!(route_points(&obcr).iter().any(|p| (p.lon, p.lat) == d), "the route goes around via the legal apex");

    // The only edge is forbidden steps: A is snappable but has no legal escape ⇒ NoPath.
    let dead = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 2_400, kind: K_STEPS }],
    };
    let bytes = map_with_profiles(&dead, &[no_steps]);
    let (res, obcr, _) = plan_p(&bytes, (a.0 + 30, a.1), (b.0 - 30, b.1), "Dead", 0);
    assert_eq!(res, Err(NavError::NoPath), "every escape forbidden ⇒ the frontier drains to NoPath");
    assert!(obcr.is_empty());
}

/// Distance honesty: the emitted `total_distance_m` is the **raw** summed edge length, not the
/// weighted `g`. A single primary edge weighted 2.0× has `g = 2 × length`, but the header total is
/// the ground length.
#[test]
fn displayed_distance_is_raw_length_not_weighted_g() {
    let a = (500_000, 500_000);
    let b = (500_000 + SP, 500_000); // ~1 113 m straight line
    let length = 2_000;
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: length, kind: K_PRIMARY }],
    };
    let bytes = map_with_profiles(&graph, &[profile("p2", &[(K_PRIMARY, 32)])]); // 2.0×
    let (res, _, _) = plan_p(&bytes, (a.0 + 30, a.1), (b.0 - 30, b.1), "Honest", 0);
    let route = res.expect("a single-edge route plans");
    assert_eq!(route.total_distance_m, length, "displayed distance is the raw ground length");
    assert_ne!(route.total_distance_m, 2 * length, "…and not the weighted g (which is 2×)");
}

/// A stale/out-of-range profile index must never brick routing: `profile_idx = 200` falls back to
/// profile 0 and plans **byte-identically**.
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
    // Profile 0 is cycle-loving; a second profile only exists to prove 200 doesn't select it.
    let bytes =
        map_with_profiles(&graph, &[profile("cycle", &[(K_PRIMARY, 48)]), profile("primary", &[(K_CYCLE, 48)])]);
    let (from, to) = ((a.0 + 30, a.1), (b.0 - 30, b.1));
    let (res0, obcr0, _) = plan_p(&bytes, from, to, "Fallback", 0);
    let (res200, obcr200, _) = plan_p(&bytes, from, to, "Fallback", 200);
    assert_eq!(res0.unwrap().total_distance_m, res200.unwrap().total_distance_m);
    assert_eq!(obcr0, obcr200, "an out-of-range index plans identically to profile 0");
    // Sanity: it really took the profile-0 (cycleway) corridor, not the fallback-to-neutral tie.
    assert!(route_points(&obcr200).iter().any(|p| (p.lon, p.lat) == c1));
}

/// A 3×3 grid with mixed per-edge kinds (bottom row + all verticals cycleway 1.0×, other
/// horizontals primary 2.0×). Weighted A* is ε = 1.3 bounded-suboptimal, so the found path's
/// **weighted** cost must be ≤ 1.3 × the true weighted optimum — computed in-test by a plain
/// Dijkstra over the same fixture graph.
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
                let kind = if row == 2 { K_CYCLE } else { K_PRIMARY }; // bottom row is the cheap corridor
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

    let from = (at(0, 0).0 + 100, at(0, 0).1 - 100);
    let goal = at(2, 2);
    let (res, obcr, _) = plan_p(&bytes, from, (goal.0 - 100, goal.1 + 100), "Mixed", 0);
    res.expect("the mixed-kind grid plans");

    // Reference: Dijkstra over the fixture graph with the same weighted edge costs (O(n²), n = 9).
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

    // Found path's weighted cost: reconstruct its node sequence from the emitted geometry and sum
    // the same weighted edge costs.
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

/// N5's acceptance ride (#538): over the **real re-packed grimsel map** (the sim's committed v9
/// asset, shipping the default Road/Gravel/MTB/Touring table), the same endpoints planned under
/// Road (profile 0) vs MTB (profile 2) produce **different polylines** — the profile weights
/// genuinely steer the search, end-to-end through the same `plan_route` both hosts call. The raw
/// lengths differ too (by ~2.8 km — the paved detour Road prefers vs the direct track MTB takes),
/// so the assert can't pass on emit jitter.
///
/// The endpoints are a pinned pair from a deterministic sweep of the map's own nav nodes, chosen
/// **inside the canonical grimsel extract bbox** (`8.15034,46.48261,8.46007,46.72070` — see
/// `obc-sim/assets/README.md`'s provenance rules; the header bbox is always somewhat wider than
/// the extract, so pinning against the extract bbox is what survives a re-pack). Verified
/// divergent on **both** the currently-committed fixture and the canonical re-pack of PR #549
/// (identical road/mtb lengths, 8 867 m / 6 051 m, on the two packs), so the test stays green
/// whichever lands first — and verified under **both** `NAV_MAX_NODES` sizes (the default 1536
/// host table and the `nrf-mem` 768 one this test gets under `--all-features` feature
/// unification), so both plans stay well inside the small table. A future re-pack from a newer
/// OSM snapshot could still move the graph enough to need a re-pin — the sweep in this PR's
/// description is the recipe.
#[test]
fn road_vs_mtb_diverge_over_grimsel() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../obc-sim/assets/grimsel.obcm"))
        .expect("grimsel.obcm fixture present");
    let from = (8_169_610, 46_694_536);
    let to = (8_217_309, 46_706_261);

    let (road, obcr_road, _) = plan_p(&bytes, from, to, "Road", 0);
    let (mtb, obcr_mtb, _) = plan_p(&bytes, from, to, "MTB", 2);
    let road = road.expect("Road plans");
    let mtb = mtb.expect("MTB plans");

    let pts_road = route_points(&obcr_road);
    let pts_mtb = route_points(&obcr_mtb);
    assert_ne!(pts_road, pts_mtb, "Road vs MTB must pick different polylines here");
    assert_ne!(road.total_distance_m, mtb.total_distance_m, "the two profiles' picks differ in raw ground length too");
}
