//! Host tests for the §8 A* router (R3, #465): fixture graphs are serialized with the
//! real `obc-pack` writer and parsed with the real `obc-reader` (the same
//! writer↔reader loop `obc-pack/tests/nav_round_trip.rs` pins), so the router is
//! exercised end to end over genuine on-wire bytes — snap, search, exhaustion, the
//! graph-tile cache, and the emitted OBCR's round trip through `RouteReader`.

mod common;

use std::cell::Cell;

use common::{decode, VecSink};
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_route::nav::{plan_route, NavError, NavScratch};
use obc_route::{ByteSource, Error, RouteIndex, RouteObjectInfo, RouteReader, SliceSource};

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
    let b0 = (550_000, 500_000); // ~5 km east of component A — inside the 10 km cap
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
    // ~4.4 km from the nearest node (well past 250 m) but inside the 10 km crow-flies cap.
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

/// A [`ByteSource`] that counts `read_at` calls — proves the crow-flies pre-check
/// rejects before ANY graph access.
struct CountingSource<'a> {
    inner: SliceSource<'a>,
    reads: Cell<u32>,
}

impl ByteSource for CountingSource<'_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.reads.set(self.reads.get() + 1);
        self.inner.read_at(offset, buf)
    }
    fn len(&self) -> u32 {
        self.inner.len()
    }
}

/// `to` beyond 10 km crow-flies ⇒ `TooFar`, with zero reads and zero bytes written —
/// the pre-check runs before snap, search, or emit touch anything.
#[test]
fn too_far_rejects_with_zero_graph_reads() {
    let bytes = map_with(&grid3(false));
    let src = CountingSource { inner: SliceSource(&bytes), reads: Cell::new(0) };
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let reads_after_parse = src.reads.get();

    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let to = (BASE.0, BASE.1 + 100_000); // ~11.1 km north
    let res = plan_route(&r, BASE, to, "x", &mut scratch, &mut tiles, &mut sink);
    assert_eq!(res, Err(NavError::TooFar));
    assert_eq!(src.reads.get(), reads_after_parse, "TooFar must not read the map");
    assert!(sink.buf.is_empty(), "TooFar must not write");
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

/// The fixed scratch honors the locked ~10 kB budget (also compile-time asserted in
/// the module; this keeps the number visible in the test log).
#[test]
fn scratch_fits_the_budget() {
    let size = core::mem::size_of::<NavScratch<{ obc_route::NAV_MAX_NODES }>>();
    assert!(size <= 10 * 1024, "NavScratch is {size} B");
}
