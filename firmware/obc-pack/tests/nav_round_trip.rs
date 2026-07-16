//! End-to-end §8 nav-graph round-trip: serialize a hand-built [`NavGraph`] with the
//! real `obc-pack` serializer, read it back with the real `obc-reader`, and assert
//! **identical topology** — nodes, adjacency (ids + inline coords + costs), edge
//! lengths, and edge geometry. The sibling byte-pinned suites (this crate's
//! `serialize.rs`, the reader's `format.rs`) pin each half against hand-coded
//! bytes; this closes the writer/reader loop the same way `round_trip.rs` does for
//! geometry, plus the §8-specific normalizations (densify, long-edge split, degree
//! cap, self-loops) that only show up through the full path.

use std::collections::BTreeMap;

use obc_pack::config::default_profiles;
use obc_pack::nav::{Edge, NavGraph, Node};
use obc_pack::{serialize_lods, LodLayer, NavProfile, Node as GeomNode};
use obc_reader::{MapCache, MapTables, NavNeighbor, Reader, SliceSource};

/// Global bbox `(min_lon, min_lat, max_lon, max_lat)` µdeg — roomy enough that the
/// node quadtree stays a single leaf for the small fixtures and subdivides for the
/// dense ones.
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 1_000_000, 1_000_000);

/// Serialize `graph` into a minimal v9 map (one empty geometry leaf, no styles, the four default
/// routing profiles).
fn map_with(graph: &NavGraph) -> Vec<u8> {
    map_with_profiles(graph, &default_profiles())
}

/// [`map_with`] with an explicit §8.6 profile table.
fn map_with_profiles(graph: &NavGraph, profiles: &[NavProfile]) -> Vec<u8> {
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 2048, root: GeomNode::Leaf { bbox: GLOBAL, features: vec![] } }];
    let (bin, dropped) = serialize_lods(&lods, &[], 0xF800, GLOBAL, &[], graph, profiles);
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
/// every distinct record's on-wire size (`13 + 15 × degree`). Returns `(fill_ratio, total_visits,
/// distinct, chunk_count)`.
fn nav_fill_and_sharing(bytes: &[u8]) -> (f64, usize, usize, usize) {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let chunk_count = r.nav_directory().chunk_count;
    let decoded = decode_all(bytes);
    let payload: usize = decoded.values().map(|d| 13 + 15 * d.neighbors.len()).sum();
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
    let whole: f64 = polyline.windows(2).map(|w| obc_reader::ground_dist_m(w[0], w[1]) as f64).sum();
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
    let view = obc_reader::BBox { min_lon: target.0, min_lat: target.1, max_lon: target.0, max_lat: target.1 };
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
        NavProfile { name: "Speedy".into(), highway: hw, surface: sf },
        NavProfile { name: "Trail".into(), highway: [24; 32], surface: [32; 8] },
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
