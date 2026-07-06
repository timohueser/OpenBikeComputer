//! Host tests for the §8 A* router (R3, #465): fixture graphs are serialized with the
//! real `obc-pack` writer and parsed with the real `obc-reader` (the same
//! writer↔reader loop `obc-pack/tests/nav_round_trip.rs` pins), so the router is
//! exercised end to end over genuine on-wire bytes — snap, search, exhaustion, the
//! graph-tile cache, and the emitted OBCR's round trip through `RouteReader`.

mod common;

use common::{decode, VecSink};
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, Node as GeomNode};
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

/// Serialize `graph` into a minimal v8 map (one empty geometry leaf, no styles).
fn map_with(graph: &NavGraph) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph);
    assert_eq!(dropped, 0);
    bin
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
                edges.push(Edge { a, b: a + 1, polyline: vec![ca, mid, cb], length_m: EDGE_COST });
            }
            if row < 2 {
                let (ca, cb) = (at(row, col), at(row + 1, col));
                let mid = (ca.0 + 500, (ca.1 + cb.1) / 2); // nudge lon
                edges.push(Edge { a, b: a + 3, polyline: vec![ca, mid, cb], length_m: EDGE_COST });
            }
        }
    }
    if shortcut {
        let (ca, cb) = (at(0, 0), at(2, 2));
        let mid = ((ca.0 + cb.0) / 2 + 500, (ca.1 + cb.1) / 2 - 500);
        edges.push(Edge { a: 0, b: 8, polyline: vec![ca, mid, cb], length_m: SHORTCUT_COST });
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
            Edge { a: i, b: i + 1, polyline: vec![ca, cb], length_m: cost_m }
        })
        .collect();
    NavGraph { nodes, edges }
}

/// Parse `bytes` and run the router with a full-size scratch + fresh tile cache,
/// returning `(result, obcr_bytes, cache_stats)`.
fn plan(
    bytes: &[u8],
    from: (i32, i32),
    to: (i32, i32),
    name: &str,
) -> (Result<obc_route::RouteStats, NavError>, Vec<u8>, obc_reader::NavCacheStats) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized v8 map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, name, &mut scratch, &mut tiles, &mut sink);
    (res, sink.buf, tiles.stats())
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
            Edge { a: 0, b: 1, polyline: vec![a0, a1], length_m: 600 },
            Edge { a: 2, b: 3, polyline: vec![b0, b1], length_m: 600 },
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
    let res = plan_route(&r, (BASE.0 + 100, BASE.1), (goal.0 - 100, goal.1), "x", &mut scratch, &mut tiles, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted));
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
    let res = plan_route(&r, from, to, "x", &mut scratch, &mut tiles, &mut sink);
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
    nav.planner.write(NavPlanner::new(from, to, "Dev"));
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
    nav.planner.write(NavPlanner::new(from, to, "Cancelled"));
    let mut cancelled_sink = VecSink::default();
    for _ in 0..2 {
        assert!(matches!(step_once(&mut nav, &bytes, &tables, &cache, &mut cancelled_sink), obc_route::Step::Running));
    }
    assert!(cancelled_sink.buf.is_empty(), "a cancelled (abandoned) plan wrote nothing");

    // Request 3 replaces the abandoned plan — another overwrite-without-drop — and completes;
    // the emitted bytes must match request 1's exactly (no state bleed through the slot).
    nav.planner.write(NavPlanner::new(from, to, "Dev"));
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

/// The §8.3 record stride is 13 + 20·degree bytes — **odd + even** — so in any multi-record
/// chunk, consecutive records alternate start-offset parity and the suite provably decodes
/// records (and their multi-byte fields) at **odd, unaligned offsets**. That is the invariant
/// behind the byte-wise-decode contract (PR #501's on-glass HardFault: an ARM backend
/// `ldrd`-fusion over these bytes; fixed with `+strict-align` on the board build) — pinned here
/// so a format change that accidentally aligns every record doesn't silently stop exercising
/// the unaligned path. The full UB tripwire is Miri over this suite (see the module doc).
#[test]
fn record_stride_keeps_odd_offsets_exercised() {
    assert_eq!(obc_reader::NAV_NODE_FIXED_LEN % 2, 1, "the fixed record head is odd-length");
    assert_eq!(obc_reader::NAV_NEIGHBOR_LEN % 2, 0, "neighbor entries are even-length");
    // ⇒ record k+1 starts at (record k start) + odd ⇒ parity alternates ⇒ every ≥2-record
    // chunk (all the grid/line fixtures here) decodes at least one odd-offset record.
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
    let res = plan_route(&r, from, to, "x", &mut small, &mut tiles, &mut sink);
    assert_eq!(res, Err(NavError::Exhausted), "the pre-fix 300-node table can't span ~9 km");

    // The capped sim/LM20-size table (1536 = the 40 kB nav budget) plans the same route.
    let mut big = Box::new(NavScratch::<1536>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let res = plan_route(&r, from, to, "x", &mut big, &mut tiles, &mut sink);
    let route = res.expect("the capped sim table spans the ~9 km line");
    assert_eq!(route.total_distance_m, 599 * 15, "summed edge costs over the whole line");
}

/// Costs saturate at `u16::MAX` meters instead of wrapping: a path whose true summed
/// cost exceeds 65 535 m still plans (the saturated `g` is just maximally
/// unattractive), the header total pins at the saturation ceiling, and nothing
/// panics or mis-orders.
#[test]
fn saturated_costs_plan_without_panicking() {
    // Three nodes ~100 m apart but with absurd 60 km edge costs: g saturates on hop 2.
    // n1 nudged off-axis so the collinear-point decimator keeps it.
    let (n0, n1, n2) = (BASE, (BASE.0 + 900, BASE.1 + 500), (BASE.0 + 1_800, BASE.1));
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: n0 }, Node { id: 1, coord: n1 }, Node { id: 2, coord: n2 }],
        edges: vec![
            Edge { a: 0, b: 1, polyline: vec![n0, n1], length_m: 60_000 },
            Edge { a: 1, b: 2, polyline: vec![n1, n2], length_m: 60_000 },
        ],
    };
    let (res, obcr, _) = plan(&map_with(&graph), (n0.0 + 30, n0.1), (n2.0 - 30, n2.1), "Far");
    let route = res.expect("a saturated-cost path still plans");
    assert_eq!(route.total_distance_m, u16::MAX as u32, "the total pins at the u16 saturation ceiling");
    assert_eq!(route_points(&obcr).len(), 3, "the geometry is intact regardless");
}

/// The resumable planner (#499): manual stepping produces a **byte-identical** OBCR to the
/// one-shot `plan_route` (which is itself the step loop), every step respects the phase
/// budgets (≤ [`NAV_SETTLES_PER_STEP`] settles per search step), the phase sequence is
/// snap → search → emit → done, and multiple `Running` steps genuinely occur (the plan is
/// spread across host passes, which is the whole point).
#[test]
fn stepped_plan_matches_one_shot_and_respects_budgets() {
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
    let reference =
        plan_route(&r, from, to, "Stepped", &mut scratch, &mut tiles, &mut one_shot).expect("the line plans one-shot");

    // Manual stepping over the same fixture.
    let mut planner = NavPlanner::new(from, to, "Stepped");
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
        let step = planner.step(&r, &mut scratch, &mut tiles, &mut stepped);
        assert!(
            planner.settles() - settles_before <= obc_route::nav::NAV_SETTLES_PER_STEP,
            "a step must respect the settle budget"
        );
        if phase != NavPhase::Search {
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
    let mut planner = NavPlanner::new(from, to, "x");
    for _ in 0..10 {
        // 2 snap steps + 8 search steps — well before the ~600-settle search could finish.
        assert_eq!(planner.step(&r, &mut scratch, &mut tiles, &mut sink), obc_route::Step::Running);
    }
    assert_eq!(planner.phase(), NavPhase::Search, "still searching when abandoned");
    drop(planner); // the cancel: never stepped again
    assert!(sink.buf.is_empty(), "snap + search phases are read-only — a cancelled plan wrote nothing");
}
