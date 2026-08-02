//! End-to-end §8 nav-graph round-trip: serialize a hand-built [`NavGraph`] with the
//! real `obc-pack` serializer, read it back with the real `obc-reader`, and assert
//! **identical topology** — nodes, adjacency (ids + inline coords + costs), edge
//! lengths, and edge geometry. The sibling byte-pinned suites (this crate's
//! `serialize.rs`, the reader's `format.rs`) pin each half against hand-coded
//! bytes; this closes the writer/reader loop the same way `round_trip.rs` does for
//! geometry, plus the §8-specific normalizations (densify, long-edge split, degree
//! cap, self-loops) that only show up through the full path.

use obc_elevation::{ElevationSource, NullElevation};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use obc_formats::obcm::{NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN};
use obc_pack::config::default_profiles;
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::terrain::TerrainSet;
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavNeighbor, Reader, SliceSource};

/// Global bbox `(min_lon, min_lat, max_lon, max_lat)` µdeg — roomy enough that the
/// node quadtree stays a single leaf for the small fixtures and subdivides for the
/// dense ones.
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 1_000_000, 1_000_000);

/// Serialize `graph` into a minimal map (one empty geometry leaf, no styles, the four default
/// routing profiles) with **no terrain** — the degrade path, so every §8.3 `Ascent M` is `0`.
fn map_with(graph: &NavGraph) -> Vec<u8> {
    map_with_profiles(graph, &default_profiles())
}

/// [`map_with`] with an explicit §8.6 profile table.
fn map_with_profiles(graph: &NavGraph, profiles: &[NavProfile]) -> Vec<u8> {
    map_with_terrain(graph, profiles, &mut NullElevation)
}

/// [`map_with_profiles`] over a caller-supplied elevation source — the v12 ascent path.
fn map_with_terrain(graph: &NavGraph, profiles: &[NavProfile], terrain: &mut dyn ElevationSource) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, profiles, terrain);
    assert_eq!(dropped, 0);
    bin
}

/// One decoded junction: coord as the crate's `(lon, lat)` + its adjacency entries.
#[derive(Clone, PartialEq, Eq)]
struct Decoded {
    coord: (i32, i32),
    neighbors: Vec<NavNeighbor>,
}

/// Parse `bytes` and walk the whole nav graph into `id → Decoded`. v9 bin-packs node chunks, so
/// distinct index leaves may share a chunk and the walk can hand the same junction record back more
/// than once — the documented §8.3 contract. This dedups by id and **asserts every repeat decode is
/// byte-identical** (the idempotency the reference consumers rely on).
fn decode_all(bytes: &[u8]) -> BTreeMap<u32, Decoded> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a serialized v9 map parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out: BTreeMap<u32, Decoded> = BTreeMap::new();
    let mut scratch = [0u8; 512];
    r.for_each_nav_node(&r.bbox, &mut scratch, |n| {
        let d = Decoded { coord: (n.lon, n.lat), neighbors: n.neighbors().collect() };
        match out.get(&n.id) {
            Some(prev) => assert!(*prev == d, "node {} decoded twice with different bytes (non-idempotent)", n.id),
            None => {
                out.insert(n.id, d);
            }
        }
    })
    .expect("nav walk");
    out
}

/// Node-chunk fill rate + the shared-chunk signature. Walks the whole graph counting **total**
/// visit callbacks (which double-count records in a chunk shared by multiple visited leaves) and
/// distinct node ids, and computes `payload / (chunk_count × 512)` where `payload` is the sum of
/// every distinct record's on-wire size (`13 + 17 × degree`). Returns `(fill_ratio, total_visits,
/// distinct, chunk_count)`.
fn nav_fill_and_sharing(bytes: &[u8]) -> (f64, usize, usize, usize) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let chunk_count = r.nav_directory().chunk_count;
    let decoded = decode_all(bytes);
    let payload: usize = decoded.values().map(|d| NAV_NODE_FIXED_LEN + NAV_NEIGHBOR_LEN * d.neighbors.len()).sum();
    let mut total_visits = 0usize;
    let mut scratch = [0u8; 512];
    r.for_each_nav_node(&r.bbox, &mut scratch, |_| total_visits += 1).unwrap();
    let fill = payload as f64 / (chunk_count * 512) as f64;
    (fill, total_visits, decoded.len(), chunk_count)
}

/// Fetch edge `edge_id`'s polyline + length through the reader.
fn fetch_edge(bytes: &[u8], edge_id: u32) -> (Vec<(i32, i32)>, u32) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut pts = heapless::Vec::<(i32, i32), 512>::new();
    let len = r.nav_edge(edge_id, &mut pts).expect("edge fetch");
    (pts.to_vec(), len)
}

/// A 4-way crossing (the R1 fixture shape): the crossing + 4 arm ends, 4 edges,
/// each with one interior shape point. The reader must hand back the identical
/// topology: 5 nodes at their coords, degree 4 / degree 1, inline neighbor coords,
/// per-arc costs, and each edge's exact polyline + length via its wire edge_id.
#[test]
fn four_way_crossing_round_trips_identically() {
    // Arms stay within the 30 000-µdeg densify threshold so geometry is byte-exact
    // (densification itself is pinned separately below).
    let cross = (500_000, 500_000);
    let arms = [(520_000, 500_000), (480_000, 500_000), (500_000, 520_000), (500_000, 480_000)];
    let mut nodes = vec![Node { id: 0, coord: cross }];
    let mut edges = Vec::new();
    for (k, &end) in arms.iter().enumerate() {
        let id = (k + 1) as u32;
        nodes.push(Node { id, coord: end });
        // One interior shape point halfway, nudged off-axis so geometry is distinctive.
        let mid = ((cross.0 + end.0) / 2, (cross.1 + end.1) / 2 + 1_000);
        // A distinctive packed way_kind per arm (surface 1 = paved, highway = k) so the kind byte's
        // round-trip is pinned end-to-end.
        let kind = (1u8 << 5) | (k as u8);
        edges.push(Edge { a: 0, b: id, polyline: vec![cross, mid, end], length_m: 22_000 + k as u32, kind });
    }
    let graph = NavGraph { nodes, edges: edges.clone() };
    let bytes = map_with(&graph);
    let decoded = decode_all(&bytes);

    assert_eq!(decoded.len(), 5, "all junctions survive");
    for n in &graph.nodes {
        assert_eq!(decoded[&n.id].coord, n.coord, "node {} coord", n.id);
    }
    let center = &decoded[&0];
    assert_eq!(center.neighbors.len(), 4, "the crossing has degree 4");
    for (k, e) in edges.iter().enumerate() {
        // The crossing's k-th adjacency entry: arm id, its inline coord, the cost, the kind.
        let adj = center.neighbors[k];
        assert_eq!(adj.id, e.b);
        // Exact delta reconstruction: the neighbor's stored i16 delta + the record's own coord must
        // reproduce the neighbor node's absolute coord bit-for-bit.
        assert_eq!((adj.lon, adj.lat), graph.nodes[e.b as usize].coord, "neighbor coord = node coord + i16 delta");
        assert_eq!(adj.cost_m, e.length_m);
        assert_eq!(adj.way_kind, e.kind, "the packed way_kind survives on the adjacency entry");
        // The arm's own single entry points back with the SAME wire edge_id AND the same kind.
        let back = &decoded[&e.b].neighbors;
        assert_eq!(back.len(), 1, "an arm end has degree 1");
        assert_eq!(back[0].id, 0);
        assert_eq!(back[0].edge_id, adj.edge_id, "both directions share one pooled edge");
        assert_eq!(back[0].way_kind, e.kind, "both directions carry the same kind");
        // And the pooled edge round-trips geometry + length exactly.
        let (poly, len) = fetch_edge(&bytes, adj.edge_id);
        assert_eq!(poly, e.polyline, "edge {k} polyline survives byte-exact");
        assert_eq!(len, e.length_m, "edge {k} length");
    }
}

/// An empty graph (a map with no routable ways) serializes and parses: an empty
/// directory, a no-op walk — never a sentinel-zero offset.
#[test]
fn empty_graph_round_trips() {
    let bytes = map_with(&NavGraph::default());
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("an empty graph still parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    assert!(r.nav_directory().is_empty());
    let decoded = decode_all(&bytes);
    assert!(decoded.is_empty());
}

/// A segment past the 30 000-µdeg delta bound is densified at pack time (same
/// threshold as geometry/OBCR): endpoints and length survive, midpoints appear,
/// and every stored delta fits the on-wire `i16`.
#[test]
fn long_segment_edge_is_densified() {
    let a = (100_000, 100_000);
    // 31 000 µdeg of latitude in one hop: the segment exceeds the 30 000 densify threshold, but the
    // endpoint span (31 000) still fits the i16 neighbor delta, so the edge densifies **without**
    // splitting into synthetic-node pieces.
    let b = (100_000, 131_000);
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 3_451, kind: 0 }],
    };
    let bytes = map_with(&graph);
    let decoded = decode_all(&bytes);
    assert_eq!(decoded.len(), 2, "no synthetic split — the endpoints stay within the i16 bound");
    let edge_id = decoded[&0].neighbors[0].edge_id;
    let (poly, len) = fetch_edge(&bytes, edge_id);

    assert_eq!(len, 3_451, "densify never changes the stored length");
    assert_eq!(*poly.first().unwrap(), a);
    assert_eq!(*poly.last().unwrap(), b);
    assert!(poly.len() > 2, "midpoints were inserted, got {}", poly.len());
    for w in poly.windows(2) {
        let (dlon, dlat) = ((w[1].0 - w[0].0).abs(), (w[1].1 - w[0].1).abs());
        assert!(dlon <= 30_000 && dlat <= 30_000, "every stored delta fits i16: ({dlon}, {dlat})");
    }
}

/// An edge whose densified polyline exceeds one chunk's record capacity is split
/// at pack time into pieces joined by a synthetic degree-2 junction — the §8.4
/// no-straddle guarantee. Topology stays routable and the concatenated pieces
/// reconstruct the original geometry.
#[test]
fn over_long_edge_splits_at_a_synthetic_node() {
    // 200 points, 100 µdeg apart: nothing densifies, but 200 > NAV_MAX_EDGE_PTS
    // (125 at the 512-byte chunk) forces one split.
    let polyline: Vec<(i32, i32)> = (0..200).map(|i| (100_000 + i * 100, 500_000)).collect();
    let (a, b) = (*polyline.first().unwrap(), *polyline.last().unwrap());
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: polyline.clone(), length_m: 1_478, kind: 0 }],
    };
    let bytes = map_with(&graph);
    let decoded = decode_all(&bytes);

    assert_eq!(decoded.len(), 3, "one synthetic junction was inserted");
    let synth = &decoded[&2]; // the first synthetic id is nodes.len()
    assert_eq!(synth.neighbors.len(), 2, "a split point is a degree-2 junction");
    assert_eq!(synth.coord, polyline[124], "split at the chunk-capacity vertex");

    // Reconstruct: piece 0 → synth, piece 1 synth → b; shared vertex deduped.
    let e0 = decoded[&0].neighbors[0];
    let e1 = decoded[&1].neighbors[0];
    assert_eq!((e0.id, e1.id), (2, 2), "both original endpoints now adjoin the synthetic node");
    let (p0, len0) = fetch_edge(&bytes, e0.edge_id);
    let (p1, len1) = fetch_edge(&bytes, e1.edge_id);
    let mut rebuilt = p0.clone();
    rebuilt.extend_from_slice(&p1[1..]);
    assert_eq!(rebuilt, polyline, "the pieces concatenate to the original polyline");
    // Piece costs are re-measured over their sub-polylines; the sum matches a
    // whole-polyline great-circle measure within per-piece rounding.
    let whole: f64 = polyline.windows(2).map(|w| obc_map_scene::ground_dist_m(w[0], w[1]) as f64).sum();
    assert!(
        (len0 as i64 + len1 as i64 - whole.round() as i64).abs() <= 2,
        "piece costs sum to the whole: {len0} + {len1} vs {whole:.1}"
    );
    assert_eq!(e0.cost_m, len0, "adjacency cost matches the stored edge length");
    assert_eq!(e1.cost_m, len1);
}

/// A node past the degree cap keeps its first 24 arcs; the dropped arcs survive
/// one-way through the spoke nodes' own records (documented §8.3 behavior).
#[test]
fn absurd_degree_node_is_capped_at_24() {
    let hub = (500_000, 500_000);
    let mut nodes = vec![Node { id: 0, coord: hub }];
    let mut edges = Vec::new();
    for k in 1..=30u32 {
        // Spokes stay within the i16 endpoint bound of the hub (span ≤ 30 000 µdeg), so no arc is
        // split — the hub keeps a direct edge to each spoke and the degree cap is what limits it.
        let end = (500_000 + 1_000 * k as i32, 505_000);
        nodes.push(Node { id: k, coord: end });
        edges.push(Edge { a: 0, b: k, polyline: vec![hub, end], length_m: 100 + k, kind: 0 });
    }
    let bytes = map_with(&NavGraph { nodes, edges });
    let decoded = decode_all(&bytes);

    let hub_adj = &decoded[&0].neighbors;
    assert_eq!(hub_adj.len(), 24, "degree capped at 24");
    let kept: Vec<u32> = hub_adj.iter().map(|n| n.id).collect();
    assert_eq!(kept, (1..=24).collect::<Vec<u32>>(), "the first 24 arcs (edge-pool order) are kept");
    // Every spoke — including the 6 dropped from the hub's list — still points back.
    for k in 1..=30u32 {
        let back = &decoded[&k].neighbors;
        assert_eq!(back.len(), 1, "spoke {k} keeps its arc");
        assert_eq!(back[0].id, 0);
    }
}

/// A self-loop (a == b, e.g. a lollipop loop) contributes exactly ONE adjacency
/// entry — not two — and its geometry round-trips.
#[test]
fn self_loop_gets_one_adjacency_entry() {
    let n = (500_000, 500_000);
    let loop_poly = vec![n, (502_000, 500_000), (502_000, 502_000), n];
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: n }],
        edges: vec![Edge { a: 0, b: 0, polyline: loop_poly.clone(), length_m: 668, kind: 0 }],
    };
    let bytes = map_with(&graph);
    let decoded = decode_all(&bytes);
    let adj = &decoded[&0].neighbors;
    assert_eq!(adj.len(), 1, "a self-loop is one entry, not two");
    assert_eq!(adj[0].id, 0, "…pointing at the node itself");
    let (poly, len) = fetch_edge(&bytes, adj[0].edge_id);
    assert_eq!(poly, loop_poly);
    assert_eq!(len, 668);
}

/// Many co-located-ish junctions force the node quadtree to subdivide (multiple
/// chunks); a whole-bbox walk still yields every node exactly once, and a
/// point-sized view descends to just its own leaf's chunk (the A* settle shape).
#[test]
fn dense_graph_subdivides_and_point_query_descends() {
    // A 12×12 grid of junctions, each with one arc to its right neighbor: 144 degree-≤2 records —
    // several 512-byte chunks. The 30 000-µdeg step keeps each edge within the i16 endpoint bound
    // (no synthetic split), so the node count stays exactly 144.
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let at = |gx: i32, gy: i32| (100_000 + gx * 30_000, 100_000 + gy * 30_000);
    for gy in 0..12 {
        for gx in 0..12 {
            let id = (gy * 12 + gx) as u32;
            nodes.push(Node { id, coord: at(gx, gy) });
            if gx > 0 {
                edges.push(Edge {
                    a: id - 1,
                    b: id,
                    polyline: vec![at(gx - 1, gy), at(gx, gy)],
                    length_m: 5_566,
                    kind: 0,
                });
            }
        }
    }
    let graph = NavGraph { nodes, edges };
    let bytes = map_with(&graph);

    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    assert!(r.nav_directory().chunk_count > 1, "the grid forces a multi-chunk tree");

    let decoded = decode_all(&bytes);
    assert_eq!(decoded.len(), 144, "every junction decodes exactly once");

    // Point descent: a degenerate view at one junction's coord visits its leaf
    // only — the node is found, and far fewer than all 144 decode.
    let target = at(7, 7);
    let view = obc_map_scene::BBox { min_lon: target.0, min_lat: target.1, max_lon: target.0, max_lat: target.1 };
    let mut scratch = [0u8; 512];
    let mut hit = false;
    let mut visited = 0;
    r.for_each_nav_node(&view, &mut scratch, |n| {
        visited += 1;
        if (n.lon, n.lat) == target {
            hit = true;
        }
    })
    .unwrap();
    assert!(hit, "the settle-shaped point query finds its junction");
    assert!(visited < 144, "…without decoding the whole graph ({visited} of 144)");
}

/// The §8.6 profile table round-trips: names, the quantized multipliers, forbidden classes, and the
/// effective-multiplier formula `(mh × ms) >> 4`.
#[test]
fn profile_table_round_trips() {
    // A tiny graph so the section is populated but trivial; the profiles are the point.
    let a = (100_000, 100_000);
    let b = (100_000, 110_000);
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m: 1_113, kind: (3 << 5) | 12 }],
    };
    // Profile 0: highway[12] = 4.0× (64), highway[4] forbidden (0), surface[3] = 5.0× (80), rest 1.0×.
    let mut hw = [16u8; 32];
    hw[12] = 64;
    hw[4] = 0;
    let mut sf = [16u8; 8];
    sf[3] = 80;
    let profiles = vec![
        NavProfile { name: "Speedy".into(), highway: hw, surface: sf, climb_weight: 10 },
        NavProfile { name: "Trail".into(), highway: [24; 32], surface: [32; 8], climb_weight: 0 },
    ];
    let bytes = map_with_profiles(&graph, &profiles);

    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let read = r.nav_profiles();
    assert_eq!(read.len(), 2, "both profiles resident");
    assert_eq!(read[0].name(), "Speedy");
    assert_eq!(read[1].name(), "Trail");
    assert_eq!(read[0].highway[12], 64);
    assert_eq!(read[0].highway[4], 0, "forbidden class survives as 0");
    assert_eq!(read[0].surface[3], 80);
    // Effective multiplier for way_kind (surface 3, highway 12): (64 × 80) >> 4 = 320.
    assert_eq!(read[0].multiplier((3 << 5) | 12), Some(320));
    // A forbidden highway class → not routable under this profile.
    assert_eq!(read[0].multiplier((3 << 5) | 4), None, "forbidden = not routable");
    // Profile 1 is uniform 1.5× (24) highway × 2.0× (32) surface: (24 × 32) >> 4 = 48, whatever
    // the class (here surface 1 / highway 2).
    assert_eq!(read[1].multiplier((1 << 5) | 2), Some(48));
    assert_eq!(read.len(), profiles.len());
}

/// Build a `g × g` grid of well-separated **isolated** 2-node edges (every node degree 1, so a
/// record is the minimal 28 bytes). Spreading them across the bbox makes the node quadtree
/// subdivide into many small leaves — the grimsel-like shape where bin-packing wins (v8 would give
/// each such leaf its own mostly-empty chunk).
fn grid_graph(g: i32) -> NavGraph {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut id = 0u32;
    for gy in 0..g {
        for gx in 0..g {
            // Two nodes 400 µdeg apart, one isolated edge per cell; cells ~40 000 µdeg apart.
            let base_x = 20_000 + gx * 40_000;
            let base_y = 20_000 + gy * 40_000;
            let a = (base_x, base_y);
            let b = (base_x + 400, base_y);
            nodes.push(Node { id, coord: a });
            nodes.push(Node { id: id + 1, coord: b });
            edges.push(Edge { a: id, b: id + 1, polyline: vec![a, b], length_m: 44, kind: 8 });
            id += 2;
        }
    }
    NavGraph { nodes, edges }
}

/// v9 bin-packs node chunks (§8.3): distinct index leaves share 512-byte chunks first-fit. Over a
/// multi-chunk grid this must (a) keep the node-chunk region **≥ 80 % payload** (the headline shrink
/// — v8 wasted ~58 % to `0xFF` padding), and (b) actually **share** chunks, which shows up as a
/// whole-graph walk handing some records back more than once (`total_visits > distinct`, the
/// idempotency contract). Every node still round-trips exactly once by id.
#[test]
fn bin_packed_node_chunks_share_and_stay_dense() {
    let graph = grid_graph(20); // 20×20 cells → 800 degree-1 nodes → several 512-byte chunks
    let bytes = map_with(&graph);
    let (fill, total_visits, distinct, chunk_count) = nav_fill_and_sharing(&bytes);

    assert_eq!(distinct, 800, "every junction decodes");
    assert!(chunk_count > 1, "the grid forces a multi-chunk node tree, got {chunk_count}");
    assert!(fill >= 0.80, "node-chunk fill rate {:.1}% must clear the 80% floor", fill * 100.0);
    assert!(
        total_visits > distinct,
        "bin-packing must share chunks across leaves (total visits {total_visits} > distinct {distinct})"
    );
}

// === v12 §8.3 directional ascent (epic #1068 EL5) ==============================================
//
// The ascent field is the only part of an adjacency entry that is *sampled* rather than derived
// from the graph, so these pin the whole path: a real OBCT container on disk → `--terrain`'s
// `TerrainSet` → the shared dead-banded integrator → the two bytes each direction gets.
//
// Every fixture edge stays inside the 30 000 µdeg densify bound and the 32 000 µdeg neighbour-delta
// bound, so the serializer's own split pass never fires and node ids 0/1 really are the two ends.

/// The synthetic terrain's posting and cell size. Both are legal OBCT v1 values and both are
/// deliberately *small*: 2^14 µdeg cells are 32 samples on a side, so the rectangle covering the
/// fixtures' corner of the bbox is tens of KB rather than the tens of MB a production 2^19 cell
/// pair would be. The sampler cannot tell the difference — posting and cell size are header data
/// (`OBCT_Spec.md` §1.3), which is exactly why they are.
const T_POSTING_LOG2: u8 = 9;
const T_CELL_LOG2: u8 = 14;
/// Metres of rise per lattice row of the synthetic ramp. At a 512 µdeg posting (≈ 57 m) this is a
/// ~14 % grade — steep enough that every 50 m sampling step clears the 3 m dead-band, so the booked
/// ascent tracks the surface instead of the hysteresis.
const T_RISE_PER_ROW: i32 = 8;
/// The µdeg box the fixture terrain covers, comfortably around every fixture edge below.
const T_MIN_UDEG: i64 = 460_000;
const T_MAX_UDEG: i64 = 540_000;

/// The OBCA cell index of a µdeg coordinate at [`T_CELL_LOG2`].
fn t_cell(udeg: i64) -> u32 {
    ((udeg + (1 << 28)) >> T_CELL_LOG2) as u32
}

/// The fixture rectangle's base sample index. The rectangle starts at the same cell on both axes,
/// so one function serves rows (`di`, latitude) and columns (`dj`, longitude) alike.
fn t_base_sample() -> i64 {
    (t_cell(T_MIN_UDEG) as i64) << (T_CELL_LOG2 - T_POSTING_LOG2)
}

/// The fixture rectangle's lattice offset for a µdeg coordinate — the `di`/`dj` the surface
/// functions take.
fn t_row(udeg: i64) -> u32 {
    (((udeg + (1 << 28)) >> T_POSTING_LOG2) - t_base_sample()) as u32
}

/// The exact µdeg coordinate of lattice offset `d` — the inverse of [`t_row`]. An endpoint placed
/// here lands **on** a lattice point, where bilinear interpolation returns the stored sample
/// untouched, so a test can predict the height by arithmetic instead of by re-running the sampler.
fn t_udeg(d: u32) -> i32 {
    (-(1i64 << 28) + ((t_base_sample() + d as i64) << T_POSTING_LOG2)) as i32
}

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let base = std::env::temp_dir().join(format!("obc-nav-ascent-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch dir");
        Scratch(base)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write a `.obcd` container over the fixture box whose surface is `height(di, dj)` in lattice
/// offsets from the rectangle's base sample. Returns the file's path.
fn write_terrain(dir: &Path, name: &str, height: &dyn Fn(u32, u32) -> i16) -> PathBuf {
    let (min_i, min_j) = (t_cell(T_MIN_UDEG), t_cell(T_MIN_UDEG));
    let rows = (t_cell(T_MAX_UDEG) - min_i + 1) as u16;
    let cols = (t_cell(T_MAX_UDEG) - min_j + 1) as u16;
    let bytes =
        obc_vectors::terrain_container(T_POSTING_LOG2, T_CELL_LOG2, min_i, min_j, rows, cols, &|_, _| true, height);
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write terrain");
    path
}

/// A sampler over a freshly opened container at `path`, plus the set that owns it.
fn open_terrain(path: &Path) -> TerrainSet {
    TerrainSet::open(path).expect("the container opens")
}

/// The ramp surface: height rises with latitude only, so an eastbound edge is flat and a northbound
/// one climbs. Longitude is deliberately *not* in it — a transposed lat/lon would book zero and be
/// noticed rather than silently sampling a plausible-looking plane.
fn ramp(di: u32, _dj: u32) -> i16 {
    (100 + T_RISE_PER_ROW * di as i32) as i16
}

/// One node's decoded neighbor entry toward `to`.
fn arc(decoded: &BTreeMap<u32, Decoded>, from: u32, to: u32) -> NavNeighbor {
    *decoded[&from].neighbors.iter().find(|n| n.id == to).expect("the arc exists")
}

/// A two-node graph joined by one straight edge between the given coords.
fn two_node_graph(a: (i32, i32), b: (i32, i32), length_m: u32) -> NavGraph {
    NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: vec![a, b], length_m, kind: (1 << 5) | 10 }],
    }
}

/// Metres of rise per lattice **row** (latitude) and per lattice **column** (longitude) of the
/// tilted-plane surface below. Different, coprime, and neither a multiple of the other — the whole
/// point is that swapping the two axes produces a *different* number.
const T_TILT_PER_ROW: i32 = 3;
const T_TILT_PER_COL: i32 = 11;

/// A plane tilted differently in the two axes. Bilinear interpolation of a plane is exact, so every
/// sample along a straight edge is the plane's own value and the totals below are arithmetic.
fn tilted_plane(di: u32, dj: u32) -> i16 {
    (1_000 + T_TILT_PER_ROW * di as i32 + T_TILT_PER_COL * dj as i32) as i16
}

/// **The lat/lon swap pin.** `ascent_along` samples with `source.sample(p.1, p.0)` — the polyline's
/// tuples are `(lon, lat)` and [`ElevationSource::sample`] takes `(lat, lon)`, so the two are
/// deliberately crossed at exactly one place. A silent swap there is the classic elevation bug and
/// most synthetic terrains are far too symmetric to notice it: any surface that treats the axes
/// alike, or any test edge that moves along only one of them, will pass either way round.
///
/// So this one is built to fail loudly. The surface is a plane tilted **3 m per latitude row and
/// 11 m per longitude column**; the edge climbs **4 rows and 12 columns**, both endpoints landing
/// exactly on lattice points where bilinear returns the stored sample untouched. The rise is then
/// `3×4 + 11×12 = 144 m` — and with the axes swapped it would be `11×4 + 3×12 = 80 m`. Both numbers
/// are asserted: the right one as an equality, the wrong one as an inequality that names the bug.
///
/// The dead-band cannot blur the answer either. Every ~50 m step of this plane rises far more than
/// the 3 m threshold, so every step books its whole delta and re-anchors — the sum telescopes to
/// `last − first`, which is exact because both ends are lattice points.
#[test]
fn the_sampler_reads_latitude_and_longitude_the_right_way_round() {
    let dir = Scratch::new("axes");
    let set = open_terrain(&write_terrain(&dir.0, "tilt.obcd", &tilted_plane));
    let mut terrain = set.sampler_for(None).expect("sampler");

    let (d_rows, d_cols) = (4u32, 12u32);
    let base = t_row(490_000);
    // Coords are the crate's `(lon, lat)`: longitude walks columns, latitude walks rows.
    let a = (t_udeg(base), t_udeg(base));
    let b = (t_udeg(base + d_cols), t_udeg(base + d_rows));

    let expected = T_TILT_PER_ROW * d_rows as i32 + T_TILT_PER_COL * d_cols as i32; // 144
    let swapped = T_TILT_PER_COL * d_rows as i32 + T_TILT_PER_ROW * d_cols as i32; // 80
    assert_ne!(expected, swapped, "the fixture must be able to tell the two orders apart");

    // The test's own sampler, called with the documented `(lat, lon)` order, sees the same rise.
    let ha = i32::from(terrain.sample(a.1, a.0).expect("covered"));
    let hb = i32::from(terrain.sample(b.1, b.0).expect("covered"));
    assert_eq!(hb - ha, expected, "the endpoints are lattice points, so the plane's arithmetic is exact");

    let bytes = map_with_terrain(&two_node_graph(a, b, 720), &default_profiles(), &mut terrain);
    let decoded = decode_all(&bytes);
    let up = i32::from(arc(&decoded, 0, 1).ascent_m);

    assert_eq!(up, expected, "the packer must sample (lat, lon); {up} m booked, {expected} m of plane");
    assert_ne!(
        up, swapped,
        "{up} m is what a swapped `source.sample(p.0, p.1)` would book — the polyline's (lon, lat) \
         tuple must be crossed into the sampler's (lat, lon) argument order"
    );
    assert_eq!(arc(&decoded, 1, 0).ascent_m, 0, "the plane rises monotonically in both axes");
}

/// **The headline v12 property.** A straight climb books its rise riding up and *nothing* riding
/// down — the two entries of one edge carry different `Ascent M` while agreeing on `Edge Id`,
/// `Cost M` and `Way Kind`, which is §8.3's single exception to "both sides are identical".
///
/// The expected number is not hand-written: it is the endpoint height difference read back through
/// the same sampler, so the assertion survives any future change to the surface constants. The 3 m
/// slack is the dead-band's unbookable remainder and nothing else.
#[test]
fn a_climb_books_ascent_one_way_and_zero_the_other() {
    let dir = Scratch::new("climb");
    let set = open_terrain(&write_terrain(&dir.0, "ramp.obcd", &ramp));
    let mut terrain = set.sampler_for(None).expect("sampler");

    // Northbound: lon fixed, lat 490 000 → 510 000. Coords are the crate's (lon, lat).
    let (a, b) = ((500_000, 490_000), (500_000, 510_000));
    let low = i32::from(terrain.sample(a.1, a.0).expect("covered"));
    let high = i32::from(terrain.sample(b.1, b.0).expect("covered"));
    assert!(high - low > 200, "the fixture surface must actually climb, got {} m", high - low);

    let bytes = map_with_terrain(&two_node_graph(a, b, 2_224), &default_profiles(), &mut terrain);
    let decoded = decode_all(&bytes);
    let up = arc(&decoded, 0, 1);
    let down = arc(&decoded, 1, 0);

    assert!(
        (i32::from(up.ascent_m) - (high - low)).abs() <= 3,
        "uphill books the rise: {} m booked vs {} m of surface",
        up.ascent_m,
        high - low
    );
    assert_eq!(down.ascent_m, 0, "a monotone descent books no climb at all");
    // Everything else about the two entries is the same edge.
    assert_eq!((up.edge_id, up.cost_m, up.way_kind), (down.edge_id, down.cost_m, down.way_kind));
}

/// Ascent is an **integral, not an endpoint difference**: a pass whose two junctions sit at nearly
/// the same height has real climb in both directions and almost no net change either way. This is
/// the property the whole field exists for — an endpoint delta would price a mountain pass as flat.
///
/// The exact relation is also pinned: `ascent(a→b) − ascent(b→a)` **is** the net height change,
/// because one direction's ascent is the other's descent. That holds to the dead-band's unbookable
/// remainder at each end and nothing more.
#[test]
fn a_pass_between_equal_heights_climbs_in_both_directions() {
    let dir = Scratch::new("pass");
    // A roof: height peaks at the lattice row of lat 500 000 and falls away linearly either side.
    let crest = t_row(500_000) as i32;
    let set = open_terrain(&write_terrain(&dir.0, "pass.obcd", &|di, _dj| {
        (2_000 - T_RISE_PER_ROW * (di as i32 - crest).abs()) as i16
    }));
    let mut terrain = set.sampler_for(None).expect("sampler");

    let (a, b) = ((500_000, 490_000), (500_000, 510_000));
    let ha = i32::from(terrain.sample(a.1, a.0).expect("covered"));
    let hb = i32::from(terrain.sample(b.1, b.0).expect("covered"));
    assert!((ha - hb).abs() < 20, "the two junctions sit at essentially the same height: {ha} / {hb}");

    let bytes = map_with_terrain(&two_node_graph(a, b, 2_224), &default_profiles(), &mut terrain);
    let decoded = decode_all(&bytes);
    let up = i32::from(arc(&decoded, 0, 1).ascent_m);
    let down = i32::from(arc(&decoded, 1, 0).ascent_m);
    assert!(up > 150 && down > 150, "both directions climb over the crest: {up} m / {down} m");
    assert!(
        ((up - down) - (hb - ha)).abs() <= 6,
        "the two directions differ by exactly the net change: {up} − {down} vs {hb} − {ha}"
    );
}

/// The forward and backward passes sample the **same points in the opposite order**, so one
/// direction's ascent is exactly the other's descent. That identity is not free — it is what the
/// symmetric interpolation and the direction-independent segment length buy — and it is what lets
/// EL7's emit-time profile agree with the number the route was costed by.
#[test]
fn reversing_a_polyline_swaps_the_two_directions_exactly() {
    let dir = Scratch::new("symmetry");
    // A staircase with an awkward period, so nothing about the answer is symmetric by accident.
    let set = open_terrain(&write_terrain(&dir.0, "steps.obcd", &|di, _dj| (300 + (di as i32 * 37) % 211) as i16));
    let mut terrain = set.sampler_for(None).expect("sampler");

    let (a, b) = ((500_000, 486_000), (500_000, 514_000));
    let poly = vec![a, (500_000, 495_000), (500_000, 507_000), b];
    let graph = NavGraph {
        nodes: vec![Node { id: 0, coord: a }, Node { id: 1, coord: b }],
        edges: vec![Edge { a: 0, b: 1, polyline: poly.clone(), length_m: 3_115, kind: 8 }],
    };
    let bytes = map_with_terrain(&graph, &default_profiles(), &mut terrain);
    let decoded = decode_all(&bytes);

    // Re-derive both totals independently through the public helper, then check the identity.
    let (fwd, back) = obc_pack::nav::integrate_edge_ascent(&poly, &mut terrain);
    assert_eq!(arc(&decoded, 0, 1).ascent_m, fwd, "the a→b entry is the forward integral");
    assert_eq!(arc(&decoded, 1, 0).ascent_m, back, "the b→a entry is the backward one");
    let mut reversed = poly.clone();
    reversed.reverse();
    let (rev_fwd, rev_back) = obc_pack::nav::integrate_edge_ascent(&reversed, &mut terrain);
    assert_eq!((rev_fwd, rev_back), (back, fwd), "reversing the polyline swaps the two directions exactly");
    assert!(fwd > 0 && back > 0, "the staircase climbs both ways: {fwd} / {back}");
}

/// No `--terrain` ⇒ every entry is `0`. This is the degrade path the whole bump rests on: a v12 map
/// packed without terrain is decode-valid and routes exactly as v11 did.
#[test]
fn without_terrain_every_ascent_is_zero() {
    let (a, b) = ((500_000, 490_000), (500_000, 510_000));
    let bytes = map_with(&two_node_graph(a, b, 2_224));
    let decoded = decode_all(&bytes);
    assert_eq!(arc(&decoded, 0, 1).ascent_m, 0);
    assert_eq!(arc(&decoded, 1, 0).ascent_m, 0);
}

/// A query outside the terrain's coverage is the same as no terrain — never a guessed height and
/// never a climb booked *across* the hole. The dead-band pauses rather than bridging, which is why
/// an edge that leaves coverage does not arrive with a phantom mountain on it.
#[test]
fn a_graph_outside_coverage_books_nothing() {
    let dir = Scratch::new("hole");
    let set = open_terrain(&write_terrain(&dir.0, "ramp.obcd", &ramp));
    // Far outside the fixture rectangle: the filter drops every container, so the sampler is empty.
    let mut terrain = set.sampler_for(Some((8_000_000, 47_000_000, 8_100_000, 47_100_000))).expect("sampler");
    assert!(terrain.is_empty(), "no container covers that box");

    let (a, b) = ((500_000, 490_000), (500_000, 510_000));
    let bytes = map_with_terrain(&two_node_graph(a, b, 2_224), &default_profiles(), &mut terrain);
    let decoded = decode_all(&bytes);
    assert_eq!(arc(&decoded, 0, 1).ascent_m, 0, "uncovered is silent, not zero-height");
}

/// Absurd input saturates instead of wrapping: an edge that would book more than 65 535 m becomes
/// maximally expensive, never free. (A wrap is the one failure mode a router must not have — it
/// would turn a wall into a shortcut.)
#[test]
fn an_impossible_climb_saturates_at_u16_max() {
    /// A sawtooth of ±20 000 m, which no terrain on Earth is and a corrupt file might be.
    struct Sawtooth(bool);
    impl ElevationSource for Sawtooth {
        fn sample(&mut self, _lat: i32, _lon: i32) -> Option<i16> {
            self.0 = !self.0;
            Some(if self.0 { 20_000 } else { -20_000 })
        }
    }
    let (a, b) = ((500_000, 486_000), (500_000, 514_000));
    let bytes = map_with_terrain(&two_node_graph(a, b, 3_115), &default_profiles(), &mut Sawtooth(false));
    let decoded = decode_all(&bytes);
    assert_eq!(arc(&decoded, 0, 1).ascent_m, u16::MAX, "saturated, not wrapped");
    assert_eq!(arc(&decoded, 1, 0).ascent_m, u16::MAX);
}

/// Two packs of the same graph over the same terrain are **byte-identical**. Sampling introduces a
/// file, a cache and a memo between the graph and the bytes; none of them may show up in the
/// output, or a re-bake would churn every cell's digest for nothing.
#[test]
fn packing_twice_over_the_same_terrain_is_byte_identical() {
    let dir = Scratch::new("determinism");
    let path = write_terrain(&dir.0, "ramp.obcd", &ramp);
    let graph = two_node_graph((500_000, 486_000), (500_000, 514_000), 3_115);

    let mut runs = Vec::new();
    for _ in 0..2 {
        let set = open_terrain(&path);
        let mut terrain = set.sampler_for(None).expect("sampler");
        runs.push(map_with_terrain(&graph, &default_profiles(), &mut terrain));
    }
    assert_eq!(runs[0], runs[1], "two identical runs must produce identical bytes");
    assert!(decode_all(&runs[0]).values().any(|d| d.neighbors.iter().any(|n| n.ascent_m > 0)), "…and non-zero ones");
}

/// A directory of `.obcd` cells is accepted exactly like a single container, and nothing about the
/// answer depends on which of the two shapes the operator handed over: the packer routes a query by
/// each container's own header rectangle, never by its path.
#[test]
fn a_directory_of_containers_samples_like_one_file() {
    let dir = Scratch::new("directory");
    let single = write_terrain(&dir.0, "ramp.obcd", &ramp);
    let tree = dir.0.join("cells").join("sub");
    std::fs::create_dir_all(&tree).expect("nested dir");
    std::fs::copy(&single, tree.join("copy.obcd")).expect("copy");

    let graph = two_node_graph((500_000, 490_000), (500_000, 510_000), 2_224);
    let one = {
        let set = open_terrain(&single);
        map_with_terrain(&graph, &default_profiles(), &mut set.sampler_for(None).expect("sampler"))
    };
    let many = {
        let set = TerrainSet::open(&dir.0.join("cells")).expect("directory opens");
        assert_eq!(set.len(), 1, "one container found under the tree");
        map_with_terrain(&graph, &default_profiles(), &mut set.sampler_for(None).expect("sampler"))
    };
    assert_eq!(one, many, "a directory and the file inside it must pack the same bytes");
}

/// The §8.6 climb weight round-trips per profile, `0` included — the field the router reads
/// alongside `Ascent M`. Its admissibility story is the opposite of a multiplier's: there is no
/// floor, because the term is additive and non-negative, so `0` and `255` are both legal.
#[test]
fn profile_climb_weights_round_trip() {
    let profiles = vec![
        NavProfile { name: "Blind".into(), highway: [16; 32], surface: [16; 8], climb_weight: 0 },
        NavProfile { name: "Steep".into(), highway: [16; 32], surface: [16; 8], climb_weight: 255 },
        NavProfile { name: "Road".into(), highway: [16; 32], surface: [16; 8], climb_weight: 10 },
    ];
    let bytes = map_with_profiles(&NavGraph::default(), &profiles);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("map parses");
    let read = tables.nav_profiles();
    assert_eq!(read.len(), 3);
    assert_eq!(read.iter().map(|p| p.climb_weight()).collect::<Vec<_>>(), vec![0, 255, 10]);
    assert_eq!(read.iter().map(|p| p.name()).collect::<Vec<_>>(), vec!["Blind", "Steep", "Road"]);
    // The shipped four carry the seeded values all the way to the wire.
    let bytes = map_with_profiles(&NavGraph::default(), &default_profiles());
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("map parses");
    assert_eq!(tables.nav_profiles().iter().map(|p| p.climb_weight()).collect::<Vec<_>>(), vec![10, 8, 6, 8]);
}
