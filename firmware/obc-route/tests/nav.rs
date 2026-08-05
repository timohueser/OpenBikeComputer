//! Host tests for the §8 A* router (R3, #465): fixture graphs are serialized with the
//! real `obc-pack` writer and parsed with the real `obc-reader` (the same
//! writer↔reader loop `obc-pack/tests/nav_round_trip.rs` pins), so the router is
//! exercised end to end over genuine on-wire bytes — snap, search, exhaustion, the
//! graph-tile cache, and the emitted OBCR's round trip through `RouteReader`.

mod common;

use common::{decode, route_points, VecSink};
use obc_elevation::NullElevation;
use obc_formats::io::SliceSource;
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::nav::{plan_route, NavError, NavPhase, NavPlanner, NavScratch};
use obc_route::{RouteIndex, RouteObjectInfo, RouteReader};

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
    NavProfile { name: "Neutral".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 }
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
    NavProfile { name: name.into(), highway, surface: [16; 8], climb_weight: 0 }
}

/// Serialize `graph` into a minimal v9 map (one empty geometry leaf, no styles) with the given
/// routing `profiles` (index-selectable by the router). At least one profile must be present.
fn map_with_profiles(graph: &NavGraph, profiles: &[NavProfile]) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, profiles, &mut NullElevation);
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
    let res = plan_route(&r, from, to, name, profile_idx, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
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
    let res = plan_route(
        &r,
        (BASE.0 + 100, BASE.1),
        (goal.0 - 100, goal.1),
        "x",
        0,
        &mut scratch,
        &mut tiles,
        &mut NullElevation,
        &mut sink,
    );
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
    let res = plan_route(&r, from, to, "Salvaged", 0, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
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
    let res = plan_route(&r, from, to, "x", 0, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
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
        planner.step(&reader, &mut *nav.scratch, &mut *nav.tiles, &mut NullElevation, sink)
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
    assert_eq!(obc_formats::obcm::NAV_NODE_FIXED_LEN % 2, 1, "the fixed record head is odd-length");
    assert_eq!(obc_formats::obcm::NAV_NEIGHBOR_LEN, 17, "v12 neighbor entries are 17 bytes");
    // ⇒ every record's first neighbor entry begins at record_start + 13 (odd), so its multi-byte
    // fields decode at odd, unaligned offsets. The 17-byte entry is odd too, so consecutive record
    // starts keep varying parity and record heads are exercised at odd offsets in multi-record
    // chunks as well — the v12 widening did not accidentally align the layout.
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
    let res = plan_route(&r, from, to, "x", 0, &mut small, &mut tiles, &mut NullElevation, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted), "the pre-fix 300-node table can't span ~9 km");

    // The capped sim/LM20-size table (1536 = the 40 kB nav budget) plans the same route.
    let mut big = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, "x", 0, &mut big, &mut tiles, &mut NullElevation, &mut sink);
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
    let reference = plan_route(&r, from, to, "Stepped", 0, &mut scratch, &mut tiles, &mut NullElevation, &mut one_shot)
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
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut stepped);
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
    assert!(matches!(
        planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut stepped),
        obc_route::Step::Done(_)
    ));
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
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut sink);
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
        assert_eq!(planner.step(&r, &mut scratch, &mut tiles, &mut NullElevation, &mut sink), obc_route::Step::Running);
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
/// whichever lands first — both plans stay well inside the 1536-node table. A future re-pack
/// from a newer
/// OSM snapshot could still move the graph enough to need a re-pin — the sweep in this PR's
/// description is the recipe.
// Reads the real grimsel fixture from disk, which Miri's default isolation forbids (and the 6.5 MB
// parse is glacial under Miri anyway) — skip it there. The UB tripwire this suite exists for is the
// §8 record decode over the synthetic writer→reader fixtures, which stay in the Miri run.
#[cfg_attr(miri, ignore)]
#[test]
fn road_vs_mtb_diverge_over_grimsel() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/obc-sim/assets/grimsel.obcm"))
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

// ---------------------------------------------------------------------------------------------
// ε-escalation retry ladder (epic #533, N8). On `NavError::Exhausted` (the fixed table filled then
// the frontier drained short of the goal) the planner retries the SAME snapped endpoints at the
// next `NAV_EPSILON_LADDER` rung (1.3× → 2.0× → 3.0×), keeping the warm tile cache and accumulating
// `settles`. `NavError::NoPath` (disconnected — the table never filled) never retries. The fixtures
// below are diagonal grids: A* on a uniform grid expands the whole diamond of monotone-optimal nodes
// at ε = 1.3 but only a narrow band at higher ε, so a fixed sub-diamond table exhausts tight and
// completes greedy — the deterministic ε-sensitive exhaustion shape.
// ---------------------------------------------------------------------------------------------

/// A full `rows × cols` 4-connected grid (node id = `row*cols + col`), every edge one `EDGE_COST`
/// hop of the given `kind`, one nudged interior shape point per edge (survives the decimator). The
/// diagonal corner-to-corner route over it is the ε-sensitive exhaustion fixture.
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

/// Same diagonal grid but with a **cheap bottom corridor**: the bottom row + all verticals are cheap
/// (`K_CYCLE`, 1.0×), every other horizontal is expensive (`K_PRIMARY`). The true optimum dives to
/// the bottom row and runs across it; a greedy (high-ε) search cuts diagonally through the expensive
/// interior — so an escalated route is genuinely *suboptimal but bounded*, the bound-per-rung case.
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

/// Plain Dijkstra over a fixture graph with the profile's weighted edge costs — the in-test optimum
/// the ε bound is measured against (O(n²), n small).
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

/// A stepped-ladder plan outcome: `(result, epsilon_used, cumulative_settles,
/// settles_at_first_escalation, obcr_bytes)`. `settles_at_first_escalation` is `None` when the plan
/// never escalated (a first-try success or a fast `NoPath`).
type LadderOutcome = (Result<obc_route::RouteStats, NavError>, (u32, u32), u32, Option<u32>, Vec<u8>);

/// Step a plan to its terminal outcome on an `N`-slot table, watching `epsilon_used()` change so the
/// caller can see the rung ladder.
fn plan_ladder<const N: usize>(bytes: &[u8], from: (i32, i32), to: (i32, i32), profile_idx: u8) -> LadderOutcome {
    use obc_route::nav::Step;
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized v9 map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<N>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let mut planner = NavPlanner::new(from, to, "x", profile_idx);
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

/// **Escalation succeeds** — a diagonal grid whose ε = 1.3 diamond overruns a 50-slot table but
/// whose ε = 2.0 corridor fits: the plan exhausts at rung 0, escalates once, and completes at 2.0×,
/// reporting `epsilon_used() == (2, 1)`. `settles` is **cumulative** — the exhausted rung-0 pass
/// alone fills the table (≈ N settles) before the successful rung-1 pass adds more, so the total
/// exceeds both N and the settles reported the instant it escalated (a single-search counter, reset
/// per rung, could report neither).
#[test]
fn escalation_succeeds_on_second_rung() {
    let bytes = map_with(&grid_diag(9, 12, 0));
    let from = (at(0, 0).0 + 100, at(0, 0).1 - 100);
    let goal = at(8, 11);
    let to = (goal.0 - 100, goal.1 + 100);
    let (res, eps, settles, first_esc, obcr) = plan_ladder::<50>(&bytes, from, to, 0);
    res.expect("the ε = 2.0 rung completes the route the tight bound couldn't fit");
    assert_eq!(eps, (2, 1), "the plan escalated exactly one rung: 1.3 exhausted, 2.0 completed");
    assert!(!obcr.is_empty(), "a completed plan emits an OBCR");
    let escalated_at = first_esc.expect("the plan escalated, so a first-escalation settle count exists");
    assert!(settles > escalated_at, "settles accumulated across the retry (cumulative {settles} > {escalated_at})");
    assert!(settles > 50, "cumulative settles exceed the table size — a single sub-N success never could");
}

/// **Ladder exhausts honestly** — a 20-slot table can't fit even the ε = 3.0 corridor of the same
/// diagonal grid, so all three rungs exhaust and the plan fails `Exhausted` at the top of the ladder
/// (`epsilon_used() == (3, 1)`). `settles` is the honest total burned across the three passes.
#[test]
fn ladder_exhausts_honestly_at_top_rung() {
    let bytes = map_with(&grid_diag(9, 12, 0));
    let from = (at(0, 0).0 + 100, at(0, 0).1 - 100);
    let goal = at(8, 11);
    let to = (goal.0 - 100, goal.1 + 100);
    let (res, eps, settles, _, obcr) = plan_ladder::<20>(&bytes, from, to, 0);
    assert_eq!(res, Err(NavError::Exhausted), "too dense for every rung ⇒ honest Exhausted");
    assert_eq!(eps, (3, 1), "the ladder climbed to and failed at the top rung");
    assert!(obcr.is_empty(), "an exhausted plan writes nothing");
    assert!(settles > 20, "three exhaustion passes burned more than one table's worth of settles ({settles})");
}

/// **No retry on disconnect** — two components each in snap range of an endpoint but not of each
/// other: the frontier drains without the table ever filling, so the terminal is `NoPath` and the
/// planner fails **immediately at rung 0** (`epsilon_used() == (13, 10)`) — retrying a greedier ε
/// can't connect an island, and must not triple the island-case latency.
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
        plan_ladder::<{ obc_route::NAV_MAX_NODES }>(&bytes, (a0.0 + 100, a0.1), (b0.0 - 100, b0.1), 0);
    assert_eq!(res, Err(NavError::NoPath), "disconnected ⇒ NoPath");
    assert_eq!(eps, (13, 10), "NoPath never escalates — the plan stays at the tight rung 0");
    assert!(first_esc.is_none(), "the plan never retried");
    assert!(obcr.is_empty());
}

/// **Bound per rung** — the ε bound is measured against the *rung the search ends on*, not a fixed
/// 1.3. On the cheap-bottom-corridor mixed grid a small table forces escalation to ε = 3.0 and the
/// greedy pass takes a genuinely **suboptimal** interior diagonal; a large table completes optimally
/// at 1.3. In both cases the found weighted cost stays within `epsilon_used()` × the in-test
/// Dijkstra optimum — the N3 bound test parameterized over the ladder.
#[test]
fn found_cost_is_within_used_rung_epsilon_of_dijkstra() {
    let (rows, cols) = (7, 10);
    let graph = grid_diag_mixed(rows, cols);
    let prof = profile("mixed", &[(K_PRIMARY, 32), (K_CYCLE, 16)]); // primary 2.0×, cycleway 1.0×
    let bytes = map_with_profiles(&graph, std::slice::from_ref(&prof));
    let from = (at(0, 0).0 + 100, at(0, 0).1 - 100);
    let goal_n = ((rows - 1) * cols + (cols - 1)) as usize;
    let goal = at(rows - 1, cols - 1);
    let to = (goal.0 - 100, goal.1 + 100);
    let reference = dijkstra_weighted(&graph, &prof, goal_n);
    assert!(reference > 0 && reference != u32::MAX, "the reference is a real finite cost");

    // Reconstruct the found path's node sequence from the emitted geometry and sum its weighted cost.
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

    // Rung 2 (ε = 3.0): a 40-slot table exhausts rungs 0 and 1, completes greedy — a suboptimal path.
    let (res, eps, _, _, obcr) = plan_ladder::<40>(&bytes, from, to, 0);
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

    // Rung 0 (ε = 1.3): a roomy table completes optimally on the first try.
    let (res, eps, _, first_esc, obcr) = plan_ladder::<120>(&bytes, from, to, 0);
    res.expect("a roomy table completes at rung 0");
    assert_eq!(eps, (13, 10), "no escalation with room to spare");
    assert!(first_esc.is_none());
    let found = found_weighted(&obcr);
    assert_eq!(found, reference, "at ε = 1.3 with a roomy table the found path is the optimum here");
    assert!(found <= reference * eps.0 / eps.1);
}

/// **Success path untouched** — a route that completes at ε = 1.3 must be indistinguishable from a
/// pre-N8 plan: it takes rung 0 (`epsilon_used() == (13, 10)`, no retry), and its emitted OBCR is
/// byte-for-byte the one-shot output (the existing exact-geometry fixtures — `grid_route_matches_
/// known_optimum_and_round_trips`, `shortcut_wins_…` — pin the bytes themselves and still pass
/// unchanged, so the pin here is the *no-escalation* guarantee on that same shape).
#[test]
fn first_try_success_takes_rung_zero_unchanged() {
    let bytes = map_with(&grid3(true));
    let (c0, c8) = (at(0, 0), at(2, 2));
    let (from, to) = ((c0.0 + 100, c0.1), (c8.0 - 100, c8.1));

    // The one-shot reference (the pre-N8 path — plain plan_route is unchanged for a 1.3 success).
    // Same name "x" as `plan_ladder` so the comparison is of route bytes, not the header name field.
    let (reference, one_shot, _) = plan(&bytes, from, to, "x");
    let reference = reference.expect("the shortcut grid plans at 1.3");

    // The stepped ladder path over the identical request.
    let (res, eps, settles, first_esc, obcr) = plan_ladder::<{ obc_route::NAV_MAX_NODES }>(&bytes, from, to, 0);
    let route = res.expect("still plans");
    assert_eq!(eps, (13, 10), "a 1.3 success never escalates — the ladder is invisible on the success path");
    assert!(first_esc.is_none(), "no retry happened");
    assert_eq!(route.total_distance_m, reference.total_distance_m, "same route as pre-N8");
    assert_eq!(obcr, one_shot, "byte-identical OBCR to the unchanged one-shot plan");
    assert!(settles > 0 && settles < obc_route::NAV_MAX_NODES as u32, "a single search, well within one table");
}

// --- EL7: emit-time elevation fill (epic #1068, #1075) -------------------------------------------

/// A synthetic terrain: a tent-shaped ridge in longitude, peaking at [`CREST_LON`] and falling 1 m
/// per 25 µdeg either side. The peak sits deliberately **between** two grid-edge vertices, so a
/// vertex-only fill cannot see it — that is what makes it a densification probe rather than a
/// sampling one. Latitude is ignored: one variable keeps every expectation below arithmetic.
struct Ridge;

/// Longitude of the ridge line — half-way along the first east-west grid edge (whose vertices sit
/// at 500 000 / 505 000 µdeg), and on none of the interpolated points either.
const CREST_LON: i32 = 502_500;
/// The ridge's height at [`CREST_LON`], m.
const CREST_M: i32 = 1_000;

fn ridge_height(lon: i32) -> i16 {
    (CREST_M - (lon - CREST_LON).abs() / 25).max(300) as i16
}

impl obc_route::ElevationSource for Ridge {
    fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        Some(ridge_height(lon_udeg))
    }
}

/// The same ridge with a **hole** across a band of longitude — the coverage-edge / `NODATA` case.
struct HolyRidge;

/// The hole's longitude band (half-open), inside which [`HolyRidge`] answers `None`.
const HOLE: core::ops::Range<i32> = 505_000..515_000;

impl obc_route::ElevationSource for HolyRidge {
    fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        if HOLE.contains(&lon_udeg) {
            return None;
        }
        Some(ridge_height(lon_udeg))
    }
}

/// Terrain that **starts** past the route's opening — the coverage crop a real sidecar has when the
/// nav graph reaches beyond the extract it was baked for (complete-way retention). Everything west
/// of [`COVERAGE_LON`] is `None`; east of it the ground climbs 1 m per 25 µdeg from
/// [`COVERED_BASE_M`], so the covered part has an exactly known climb.
struct CroppedTerrain;

/// Where coverage begins — past the grid's first column (500 000), so the route's first points and
/// the interpolated ones between them are all outside the raster.
const COVERAGE_LON: i32 = 507_000;
/// The height at [`COVERAGE_LON`] — high enough that booking it as ascent would be unmissable.
const COVERED_BASE_M: i32 = 1_412;

fn covered_height(lon: i32) -> i16 {
    (COVERED_BASE_M + (lon - COVERAGE_LON) / 25) as i16
}

impl obc_route::ElevationSource for CroppedTerrain {
    fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        (lon_udeg >= COVERAGE_LON).then(|| covered_height(lon_udeg))
    }
}

/// Plan the standard corner-to-corner grid route through `elev`, returning `(stats, obcr bytes)`.
fn plan_with_elevation(bytes: &[u8], elev: &mut dyn obc_route::ElevationSource) -> (obc_route::RouteStats, Vec<u8>) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized v9 map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let (c0, c8) = (at(0, 0), at(2, 2));
    let res = plan_route(
        &r,
        (c0.0 + 100, c0.1 - 100),
        (c8.0 - 100, c8.1 + 100),
        "Water stop",
        0,
        &mut scratch,
        &mut tiles,
        elev,
        &mut sink,
    );
    (res.expect("the grid plans"), sink.buf)
}

/// FNV-1a over the emitted bytes — a compact stand-in for pasting a whole OBCR into the test.
fn digest(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// **The null-path pin (EL7, non-negotiable).** With [`NullElevation`] the emitted OBCR is
/// byte-for-byte what the pre-terrain router wrote: no densification, no elevation, no stats. The
/// digest below was taken from `develop` at the commit before EL7 (the same fixture and route
/// through the then-argumentless `plan_route`) and must not move — if it does, adding terrain
/// support changed the *no-terrain* output and the epic's "removable" claim is broken.
#[test]
fn a_null_elevation_plan_emits_the_pre_terrain_bytes() {
    let bytes = map_with(&grid3(false));
    let (route, obcr) = plan_with_elevation(&bytes, &mut NullElevation);

    assert_eq!(digest(&obcr), NULL_PATH_DIGEST, "the no-terrain OBCR must stay byte-identical");
    assert_eq!(route.point_count, 9, "no densification without terrain");
    assert_eq!((route.total_ascent_m, route.total_descent_m), (0, 0));
    assert_eq!((route.min_ele_m, route.max_ele_m), (0, 0));
    assert!(route_points(&obcr).iter().all(|p| p.ele == 0), "every stored height is zero");
}

/// FNV-1a of the pre-EL7 emit for the fixture above; see the test's doc comment. Verified against
/// `develop` (c566880b) by running the identical plan through the pre-EL7 `plan_route`.
const NULL_PATH_DIGEST: u64 = 0x9469_2b0b_b07b_523e;

/// The unlock: a real source fills every point's height, and the header carries real min/max and
/// dead-banded ascent/descent instead of the zero stub. The crest is only reachable through the
/// 250 m densification — a vertex-only fill would top out at the 900 m the edge's endpoints see.
#[test]
fn terrain_fills_every_point_and_the_header_stats() {
    let bytes = map_with(&grid3(false));
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
    // Distance is still the summed raw edge cost (N3) — densifying the geometry must not touch it.
    assert_eq!(route.total_distance_m, 4 * EDGE_COST);
}

/// Densification is bounded by **ground distance**, not by vertex count: the route is sampled at
/// least once per 250 m of it, whatever the graph's vertex spacing. What *survives* into the OBCR
/// is then the decimator's business — a densified point on a flat straight run carries neither
/// shape nor height and is correctly dropped again — so the bound is asserted on the sampling.
#[test]
fn terrain_samples_at_least_once_per_step_of_ground() {
    /// [`Ridge`] with a sample counter.
    struct Counting(u32);
    impl obc_route::ElevationSource for Counting {
        fn sample(&mut self, _lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
            self.0 += 1;
            Some(ridge_height(lon_udeg))
        }
    }
    let bytes = map_with(&grid3(false));
    let mut counting = Counting(0);
    let (route, _) = plan_with_elevation(&bytes, &mut counting);

    // The emitted polyline is longer than the summed edge costs (the nudged shape points), so the
    // route's own distance is a safe lower bound on what has to be covered at ≤ 250 m a step.
    let want = route.total_distance_m / 250;
    assert!(
        counting.0 >= want,
        "{} samples for {} m of route — under one per 250 m",
        counting.0,
        route.total_distance_m
    );
    // …and not wildly more: the fill samples points, it does not sweep the raster.
    assert!(counting.0 < want * 4, "{} samples is far more than the step implies", counting.0);
}

/// The stats are the **shared** dead-band's, over the emitted point stream: re-integrating the
/// stored heights with `obc_elevation::DeadBand` — exactly what the GPX converter does over an
/// import, and therefore what a re-imported export of this route does — reproduces the header.
#[test]
fn the_header_stats_are_the_shared_dead_band_over_the_stored_points() {
    let bytes = map_with(&grid3(false));
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

/// A hole in coverage carries the last known height forward — a flat span, never a guess and never
/// a phantom climb — while the sampled parts keep their real stats.
#[test]
fn a_coverage_hole_carries_the_last_height_forward() {
    let bytes = map_with(&grid3(false));
    let (route, obcr) = plan_with_elevation(&bytes, &mut HolyRidge);
    let pts = route_points(&obcr);

    assert!(pts.iter().all(|p| p.ele >= 300), "no point falls back to 0 inside the hole");
    // Every point inside the hole repeats one height — the last one that resolved before it — so
    // the span is flat, and the dead-band books nothing across it.
    let inside: Vec<i16> = pts.iter().filter(|p| HOLE.contains(&p.lon)).map(|p| p.ele).collect();
    assert!(!inside.is_empty(), "the route does cross the hole");
    assert!(inside.iter().all(|&e| e == inside[0]), "the hole is flat at the carried height (got {inside:?})");
    assert!(route.total_ascent_m > 0, "the sampled part still books its climb (got {route:?})");
}

/// The end-to-end article, on the committed fixtures: the Grimsel map's nav graph planned through
/// the Grimsel **terrain sidecar** (EL2's `grimsel.obcd`, the same file the simulator mounts).
/// Nothing synthetic — this is the number a rider would see on the Route overview.
// Reads the committed fixtures from disk, which Miri's default isolation forbids — skip it there,
// like `road_vs_mtb_diverge_over_grimsel`. (Missed when EL7 landed; the module's standing
// `cargo +nightly miri test -p obc-route --test nav` aborted on it.)
#[cfg_attr(miri, ignore)]
#[test]
fn a_real_grimsel_plan_carries_the_pass_road_profile() {
    let map = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/obc-sim/assets/grimsel.obcm"))
        .expect("grimsel.obcm fixture present");
    let dem = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/obc-sim/assets/grimsel.obcd"))
        .expect("grimsel.obcd terrain fixture present");
    let terrain_src = SliceSource(&dem);
    let mut terrain = obc_elevation::TerrainElevation::<{ obc_elevation::DEFAULT_TILE_SLOTS }>::parse(&terrain_src)
        .expect("the baked terrain parses");

    // Innertkirchen → up the pass road (the profile-divergence fixture's endpoints).
    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let src = SliceSource(&map);
    let tables = MapTables::parse(&src).expect("grimsel parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<{ obc_route::NAV_MAX_NODES }>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let route = plan_route(&r, from, to, "Grimsel", 0, &mut scratch, &mut tiles, &mut terrain, &mut sink)
        .expect("the pass road plans");

    // Alpine valley floor to well up the pass: heights in the hundreds-to-thousands, never the
    // 0 m a missing fill would leave, and a climb that is real without being absurd.
    assert!((500..=2_200).contains(&route.min_ele_m), "min {} m is not alpine ground", route.min_ele_m);
    assert!((500..=2_600).contains(&route.max_ele_m), "max {} m is not alpine ground", route.max_ele_m);
    assert!(route.max_ele_m > route.min_ele_m + 100, "a pass road is not flat ({route:?})");
    assert!((100..=3_000).contains(&route.total_ascent_m), "ascent {} m is implausible", route.total_ascent_m);
    let pts = route_points(&sink.buf);
    assert!(pts.iter().all(|p| p.ele > 0), "every stored point has a real height");
    let (hits, misses) = terrain.stats();
    assert!(hits > misses, "the 4-tile cache serves the walk ({hits} hit / {misses} miss)");
}

/// **Round-trip parity** — the property the shared dead-band exists for: write a planned route out
/// as GPX (what any exporter does with the stored points) and re-import it through
/// [`gpx_to_obcr`](obc_route::gpx_to_obcr); the re-imported route's own climb agrees with the
/// header the planner wrote. Without the emit-time fill both sides are 0 and the check is vacuous;
/// with it, the two independently-computed totals have to land on each other.
///
/// The route is **wholly inside** `grimsel.obcd`'s coverage, and the `p.ele > 0` assertion in the
/// export loop is what holds it there. That is the parity claim's boundary, by construction: a
/// route whose opening lies outside the raster stores `0` for those points (the module docs' hole
/// policy — the format has no "unknown"), so its export re-imports with a step the converter's
/// dead-band books. Inside coverage — every route on a map whose terrain was baked for it — the
/// two integrations agree.
#[cfg_attr(miri, ignore)] // reads the committed fixtures from disk — see the note above
#[test]
fn a_planned_route_exported_to_gpx_and_reimported_keeps_its_climb() {
    let map = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/obc-sim/assets/grimsel.obcm"))
        .expect("grimsel.obcm fixture present");
    let dem = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/obc-sim/assets/grimsel.obcd"))
        .expect("grimsel.obcd terrain fixture present");
    let terrain_src = SliceSource(&dem);
    let mut terrain =
        obc_elevation::TerrainElevation::<{ obc_elevation::DEFAULT_TILE_SLOTS }>::parse(&terrain_src).unwrap();

    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let src = SliceSource(&map);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<{ obc_route::NAV_MAX_NODES }>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let planned =
        plan_route(&r, from, to, "Grimsel", 0, &mut scratch, &mut tiles, &mut terrain, &mut sink).expect("plans");

    // The export: one `<trkpt>` per stored point, exactly the fields an exporter has to hand.
    let mut gpx = String::from("<gpx><trk><trkseg>");
    for p in route_points(&sink.buf) {
        assert!(p.ele > 0, "an exported point with no height would export a lie");
        gpx.push_str(&format!(
            "<trkpt lat=\"{:.6}\" lon=\"{:.6}\"><ele>{}</ele></trkpt>",
            p.lat as f64 / 1e6,
            p.lon as f64 / 1e6,
            p.ele
        ));
    }
    gpx.push_str("</trkseg></trk></gpx>");

    let mut back = VecSink::default();
    let reimported = obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Grimsel", &mut back).expect("re-imports");

    assert_eq!((reimported.min_ele_m, reimported.max_ele_m), (planned.min_ele_m, planned.max_ele_m));
    let (a, b) = (planned.total_ascent_m as i64, reimported.total_ascent_m as i64);
    assert!(
        (a - b).abs() * 20 <= a.max(1),
        "planned +{a} m vs re-imported +{b} m — the two dead-band integrations must agree within 5%"
    );
}

/// **A leading hole books nothing.** A route that starts outside coverage has no known height to
/// carry yet, so the integrator must not run until the first sample resolves — otherwise the band
/// anchors on the `0` placeholder and the first real height (1 412 m here) lands in the header as
/// ascent, poisoning every stored `cum_ascent` after it as well.
#[test]
fn a_route_that_starts_outside_coverage_books_no_phantom_ascent() {
    let bytes = map_with(&grid3(false));
    let (route, obcr) = plan_with_elevation(&bytes, &mut CroppedTerrain);
    let pts = route_points(&obcr);

    // The route genuinely straddles the coverage edge: points on both sides of it.
    assert!(pts.iter().any(|p| p.lon < COVERAGE_LON), "the route starts outside coverage");
    let covered: Vec<&obc_route::RoutePoint> = pts.iter().filter(|p| p.lon >= COVERAGE_LON).collect();
    assert!(covered.len() >= 2, "and crosses well into it");

    // The header's climb is the *covered* climb only — the 1 412 m step into coverage is not a hill.
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
    assert!(
        route.total_ascent_m < COVERED_BASE_M as u32,
        "a {} m first sample was booked as ascent (the leading-hole bug)",
        COVERED_BASE_M
    );
    // …and the same on the stored per-point cumulative: the first covered point must still read 0.
    let first_covered_cum = cum_ascent_at(&obcr, |p| p.lon >= COVERAGE_LON);
    assert_eq!(first_covered_cum, 0, "cum_ascent at the first covered point is poisoned");
    // The uncovered opening still *stores* 0 (OBCR has no "unknown") — the documented wart.
    assert!(pts.iter().filter(|p| p.lon < COVERAGE_LON).all(|p| p.ele == 0));
}

/// The stored cumulative ascent (a chunk-level field) at the first point matching `pred` — read
/// through the same `ChunkMeta` the profile builder reads, so this checks what is *written*, not a
/// recomputation.
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

/// A source that never resolves is the null source as far as the route is concerned: same bytes,
/// same zeroed stats — a flat 0 m route is never reported as real terrain.
#[test]
fn a_source_that_never_resolves_leaves_the_stats_zeroed() {
    struct Blind;
    impl obc_route::ElevationSource for Blind {
        fn sample(&mut self, _lat: i32, _lon: i32) -> Option<i16> {
            None
        }
    }
    let bytes = map_with(&grid3(false));
    let (route, obcr) = plan_with_elevation(&bytes, &mut Blind);
    let (_, null_obcr) = plan_with_elevation(&bytes, &mut NullElevation);

    assert_eq!((route.min_ele_m, route.max_ele_m), (0, 0));
    assert_eq!((route.total_ascent_m, route.total_descent_m), (0, 0));
    assert_eq!(obcr, null_obcr, "an always-None source emits exactly the null source's bytes");
}

// --- EL6: climb-aware A* relaxation (epic #1068, #1074) ------------------------------------------
//
// The relaxation now costs an edge `(cost_m × effective) >> 4 + ascent_m × climb_weight` (§8.6).
// `ascent_m` is baked by the packer from an `ElevationSource` at serialize time, so these fixtures
// hand `serialize_lods` a synthetic terrain and then route over the resulting **real v12 bytes** —
// the same writer→reader loop the rest of this file uses. Nothing below mounts terrain at *emit*
// (every plan runs through `NullElevation`), which keeps the emitted OBCR comparable to the
// pre-terrain pins: what is under test here is the search, not the fill.

/// [`map_with_profiles`] over a caller-supplied terrain — the v12 ascent path. Without this the
/// packer bakes `Ascent M = 0` everywhere and the climb term is unreachable from a test.
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

/// A neutral-multiplier profile carrying nothing but a climb weight — so a test that moves `w` moves
/// **only** the climb term and every other input to the cost stays where the other fixtures put it.
fn climb_profile(name: &str, climb_weight: u8) -> NavProfile {
    NavProfile { name: name.into(), highway: [16; 32], surface: [16; 8], climb_weight }
}

/// A north-facing hillside: the ground rises 1 m per 25 µdeg of latitude above the fixture grid's
/// base row and is dead flat at or below it. One variable (latitude), so every ascent below is
/// arithmetic: a leg from row 0 to row 1 climbs `SP / 25 = 400 m`, and the same leg southbound
/// climbs nothing at all.
struct Hillside;

/// The climb, in metres, of one grid row of the [`Hillside`] — `SP / 25`.
const ROW_CLIMB_M: u32 = 400;

impl obc_route::ElevationSource for Hillside {
    fn sample(&mut self, lat_udeg: i32, _lon_udeg: i32) -> Option<i16> {
        Some(((lat_udeg - BASE.1).max(0) / 25) as i16)
    }
}

/// A conical knoll centred on the 3×3 grid's middle node: 600 m at the summit, falling 1 m per
/// 20 µdeg of Chebyshev distance and flat at the grid's rim. Unlike [`Hillside`] it makes the grid's
/// monotone corner-to-corner paths differ *in climb while tying in distance* — over the top costs
/// 500 m, around the rim costs nothing — which is what the Dijkstra ground truth needs to bite on.
struct Knoll;

/// The knoll's summit height, m.
const SUMMIT_M: i32 = 600;

impl obc_route::ElevationSource for Knoll {
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        let summit = at(1, 1);
        let d = (lon_udeg - summit.0).abs().max((lat_udeg - summit.1).abs());
        Some((SUMMIT_M - d / 20).max(0) as i16)
    }
}

/// The pass-vs-valley fixture, the product behaviour in four nodes: A and B a grid row apart on the
/// flat, joined either **north over a pass** (two short legs through the apex, which the
/// [`Hillside`] puts [`ROW_CLIMB_M`] above them) or **south through a valley** (two long legs that
/// never leave the flat). Every leg is longer than its own straight line, so the great-circle
/// heuristic stays admissible whatever the profile.
///
/// The arithmetic, once, so every assertion below is a reading of it: the pass corridor costs
/// `2 × PASS_LEG + ROW_CLIMB_M × w` and the valley `2 × VALLEY_LEG`, so they cross at
/// `w = 2 × (VALLEY_LEG − PASS_LEG) / ROW_CLIMB_M = 5` exactly. Only the *uphill* leg books ascent —
/// the descent off the far side books none, which is the §8.6 asymmetry made visible.
///
/// **Why the legs are kilometres and the nodes are hundreds of metres apart.** The search is
/// *weighted* A\*, so a frontier node carries `ε·h` of inflation the goal (at `h = 0`) does not, and
/// on a fixture whose legs were comparable to `ε·h` the ε rung — not the cost model — would decide
/// the tie. Making each leg longer than `0.3 × h(pass → B)` puts the corridor choice back where
/// these tests mean to read it: `A*` settles the pass before the goal pops whenever the pass is
/// genuinely cheaper, so its answer here is the true climb-aware optimum, not a bounded
/// approximation of one. (The bound itself is exercised, deliberately, in
/// `the_climb_aware_optimum_matches_a_directional_dijkstra`.)
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

/// One leg of the pass corridor, m — comfortably above both its own ~1 574 m straight line and the
/// ~472 m of `ε·h` inflation the frontier carries (see [`pass_vs_valley`]).
const PASS_LEG: u32 = 3_000;
/// One leg of the valley corridor, m. `2 × 4 000 = 8 000` is 2 000 m of ground more than the pass —
/// which the climb term matches at exactly `w = 5`.
const VALLEY_LEG: u32 = 4_000;

/// Plan A→B over a [`pass_vs_valley`] map and report `(raw distance, took the pass?)`.
fn plan_corridor(bytes: &[u8]) -> (u32, bool) {
    let (a, b) = (at(0, 0), at(0, 2));
    let (res, obcr, _) = plan_p(bytes, (a.0 + 30, a.1), (b.0 - 30, b.1), "Corridor", 0);
    let route = res.expect("both corridors are legal — one of them must plan");
    let over_the_pass = route_points(&obcr).iter().any(|p| (p.lon, p.lat) == at(1, 1));
    (route.total_distance_m, over_the_pass)
}

/// **The steering test** — the product behaviour EL6 exists for, pinned. Same map, same endpoints,
/// same everything except the profile's climb weight: a climb-blind router takes the short steep
/// pass (it is 1 600 m of ground cheaper) and a road-weighted one takes the long flat valley. This
/// is the grimsel A/B in miniature, and it is deterministic.
#[test]
fn the_climb_weight_steers_from_the_pass_to_the_valley() {
    let graph = pass_vs_valley();

    // Non-vacuity first: the packer really did bake the climb the assertions below spend. The
    // uphill direction books a row of the hillside; the downhill one books nothing.
    let (up, down) = obc_pack::nav::integrate_edge_ascent(&[at(0, 0), at(1, 1)], &mut Hillside);
    assert!(
        (up as i64 - ROW_CLIMB_M as i64).abs() <= 4,
        "the pass leg should bake ≈ {ROW_CLIMB_M} m of ascent, baked {up}"
    );
    assert_eq!(down, 0, "and nothing at all coming back down — ascent is directional (§8.3)");

    let blind = map_with_terrain(&graph, &[climb_profile("Blind", 0)], &mut Hillside);
    assert_eq!(plan_corridor(&blind), (2 * PASS_LEG, true), "climb-blind, the pass is simply the shorter way");

    let road = map_with_terrain(&graph, &[climb_profile("Road", 10)], &mut Hillside);
    assert_eq!(plan_corridor(&road), (2 * VALLEY_LEG, false), "at the stock Road weight the climb outprices 2 km");
}

/// The crossover sits **exactly** where the §8.6 arithmetic puts it, not merely somewhere sensible:
/// the corridors tie at `w = 5` (`400 × 5 = 2 000` = the valley's extra ground), so `w = 4` still
/// takes the pass and `w = 6` already takes the valley. A cost model that folded the climb term
/// inside the `>> 4`, or scaled it by the way-kind multiplier, or charged both directions of the
/// edge, would miss this by a rung.
#[test]
fn the_climb_crossover_lands_where_the_formula_says() {
    let graph = pass_vs_valley();
    let below = map_with_terrain(&graph, &[climb_profile("w4", 4)], &mut Hillside);
    let above = map_with_terrain(&graph, &[climb_profile("w6", 6)], &mut Hillside);
    assert_eq!(plan_corridor(&below), (2 * PASS_LEG, true), "4 × 400 = 1 600 < 2 000 ⇒ the pass still wins");
    assert_eq!(plan_corridor(&above), (2 * VALLEY_LEG, false), "6 × 400 = 2 400 > 2 000 ⇒ the valley wins");
}

/// **The null-path pin (EL6, non-negotiable).** A map baked *with* terrain — every §8.3 `Ascent M`
/// a real number — routed under a `climb_weight = 0` profile must emit the bytes `develop`'s
/// pre-elevation router emitted. The digest is [`NULL_PATH_DIGEST`], the very constant EL7 took from
/// `develop` at the commit before the epic touched this crate, so the two zeroes the spec calls
/// legal (`Ascent M = 0` on a terrain-free map, `Climb Weight = 0` on a climb-blind profile) are
/// *proved* to reproduce v11 costing rather than assumed to.
#[test]
fn climb_weight_zero_over_real_ascents_is_the_pre_elevation_router() {
    let graph = grid3(false);
    let flat = map_with(&graph);
    let hilly = map_with_terrain(&graph, &[neutral_profile()], &mut Hillside);

    // The fixture is not vacuous: the two maps differ (the ascent field is populated) and a grid
    // column really does climb a row of the hillside.
    assert_ne!(flat, hilly, "the terrain-baked map must differ on the wire, or this pins nothing");
    let (up, _) = obc_pack::nav::integrate_edge_ascent(&[at(0, 0), at(0, 500), at(1, 0)], &mut Hillside);
    assert!(up >= ROW_CLIMB_M as u16 - 4, "a grid column climbs a row of the hillside, baked {up} m");

    let (route, obcr) = plan_with_elevation(&hilly, &mut NullElevation);
    assert_eq!(digest(&obcr), NULL_PATH_DIGEST, "a climb-blind plan over baked ascents must not move a byte");
    assert_eq!(route.total_distance_m, 4 * EDGE_COST);
    let (_, flat_obcr) = plan_with_elevation(&flat, &mut NullElevation);
    assert_eq!(obcr, flat_obcr, "…and is the same route the terrain-free map plans");
}

/// **The committed-fixture pin.** `apps/obc-sim/assets/grimsel.obcm` is packed *without* `--terrain`
/// (`assets/repack.sh` passes no such flag), so every `Ascent M` on it is `0` and the climb term
/// vanishes whatever the profile's weight — even though the shipped table carries the stock
/// Road 10 / Gravel 8 / MTB 6 / Touring 8. All four profiles' routes are therefore byte-frozen at
/// the digests `develop` produced before EL6; if one moves, the "no terrain ⇒ no change" claim (and
/// with it every committed UI snapshot) is broken.
#[cfg_attr(miri, ignore)] // reads the 6.5 MB fixture from disk — Miri's isolation forbids it
#[test]
fn the_terrain_free_grimsel_fixture_routes_byte_identically_on_every_profile() {
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/obc-sim/assets/grimsel.obcm"))
        .expect("grimsel.obcm fixture present");
    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    for (idx, want) in GRIMSEL_PRE_EL6_DIGESTS.iter().enumerate() {
        let (res, obcr, _) = plan_p(&bytes, from, to, "Grimsel", idx as u8);
        res.unwrap_or_else(|e| panic!("profile {idx} plans on grimsel, got {e:?}"));
        assert_eq!(digest(&obcr), *want, "profile {idx}'s route on the terrain-free fixture moved");
    }
}

/// FNV-1a of each stock profile's Innertkirchen→Grimsel route on the committed fixture, captured on
/// `develop` at 16de566c (EL7's merge, the commit this branch forked from) and unchanged by EL6.
const GRIMSEL_PRE_EL6_DIGESTS: [u64; 4] =
    [0xd6e8_1a83_6000_7fb9, 0xb605_bfd0_f318_e1b3, 0xfa4b_d3c0_6c4c_1e92, 0x407d_5d79_4678_0248];

/// **Admissibility in practice, not just in prose**: over a graph whose paths genuinely differ in
/// climb (the [`Knoll`] — over the top costs 500 m, around the rim costs nothing, and every path
/// ties on distance), weighted A\*'s answer is the *exact* optimum a plain Dijkstra over the same
/// climb-aware, **directional** edge costs finds. The ε bound would allow 1.3× of it; the point of
/// asserting equality is that an inadmissible `h` would show up here as a cheaper reference than the
/// search could reach, long before it ever showed up as a 1.3.
#[test]
fn the_climb_aware_optimum_matches_a_directional_dijkstra() {
    let graph = grid3(false);
    let prof = climb_profile("Climby", 10);
    let bytes = map_with_terrain(&graph, std::slice::from_ref(&prof), &mut Knoll);

    // Reference: Dijkstra over the fixture graph with the SAME per-direction costs — the packer's
    // own `integrate_edge_ascent` supplies each direction's ascent, so the ground truth is computed
    // from the same integrator that wrote the bytes rather than from a re-derivation of it.
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
        "the knoll must make some edge cost more than its ground length, or this test is vacuous"
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

    // The found path, re-costed through the same directional table.
    let (c0, c8) = (at(0, 0), at(2, 2));
    let (res, obcr, _) = plan_p(&bytes, (c0.0 + 100, c0.1 - 100), (c8.0 - 100, c8.1 + 100), "Knoll", 0);
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

    assert_eq!(found, reference, "weighted A* did not find the climb-aware optimum ({found} vs {reference})");
    assert!(found <= reference * 13 / 10, "…and it is inside the ε = 1.3 bound by construction");
}

/// **Distance honesty under a dominant climb term** (N3's rule, re-pinned where EL6 could break it):
/// a single hillside leg weighted at `w = 100` accumulates a `g` of `1 600 + 400 × 100 = 41 600`,
/// twenty-six times its ground length — and the header still reports the ground length. Nothing on a
/// display path reads `g`; `emit_hop` sums each hop's raw `length_m` and the climb term never enters
/// that sum.
#[test]
fn the_displayed_distance_ignores_the_climb_term_entirely() {
    let (a, pass) = (at(0, 0), at(1, 1));
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: pass }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, pass], length_m: PASS_LEG, kind: 0 }],
    };
    let bytes = map_with_terrain(&graph, &[climb_profile("Heavy", 100)], &mut Hillside);
    let (res, _, _) = plan_p(&bytes, (a.0 + 30, a.1), (pass.0 - 30, pass.1), "Uphill", 0);
    let route = res.expect("a single uphill edge plans");
    assert_eq!(route.total_distance_m, PASS_LEG, "the header total is the raw ground length");
    assert!(route.total_distance_m < ROW_CLIMB_M * 100, "…and emphatically not the weighted g");
}

/// **The saturation edge.** At `w = 255` the pass leg's climb term alone is `400 × 255 = 102 000` —
/// past `u16::MAX`, so the frontier's 16-bit `g` clamps. The documented consequence is that a
/// saturated node is *maximally unattractive*, never wrapped and never mis-ordered: the plan must
/// still complete, still take the flat valley, and still report its honest raw distance. (The
/// arithmetic's own extremes — every input at its wire maximum — are unit-pinned next to
/// `ProfileMult::edge_cost` in `src/nav.rs`.)
#[test]
fn a_saturating_climb_weight_clamps_instead_of_wrapping() {
    assert!(ROW_CLIMB_M * 255 > u16::MAX as u32, "the fixture must actually reach saturation");
    let bytes = map_with_terrain(&pass_vs_valley(), &[climb_profile("Absurd", 255)], &mut Hillside);
    assert_eq!(plan_corridor(&bytes), (2 * VALLEY_LEG, false), "a saturated pass is unattractive, not cheap");
}

/// The other half of the saturation story: when **every** route saturates there is no ordering left
/// to exploit, and the contract is only that the search degrades — it must still terminate and still
/// return a real route, never panic and never wrap into a bogus shortcut. The knoll grid at `w = 255`
/// saturates on the first climbing edge out of the start.
#[test]
fn a_wholly_saturated_frontier_still_returns_a_route() {
    let graph = grid3(false);
    let bytes = map_with_terrain(&graph, &[climb_profile("Absurd", 255)], &mut Knoll);
    let (c0, c8) = (at(0, 0), at(2, 2));
    let (res, obcr, _) = plan_p(&bytes, (c0.0 + 100, c0.1), (c8.0 - 100, c8.1), "Saturated", 0);
    let route = res.expect("a saturated frontier still drains to the goal");
    assert!(route.total_distance_m >= 4 * EDGE_COST, "a real path, not a wrapped shortcut");
    let pts = route_points(&obcr);
    assert_eq!((pts[0].lon, pts[0].lat), c0);
    assert_eq!((pts[pts.len() - 1].lon, pts[pts.len() - 1].lat), c8);
}
