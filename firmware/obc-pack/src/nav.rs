//! `nav.rs` — build an in-memory **routable navigation graph** from the ingested
//! `highway=*` ways (epic #116, sub-issue R1 #463). The graph is junction
//! **nodes** joined by undirected **edges** whose polyline interiors carry no
//! junctions; `serialize.rs`'s `serialize_nav_section` tiles and serializes it
//! into the OBCM §8 nav-graph section. Nothing here touches the `.obcm` bytes.
//!
//! Today highways are render-only geometry with no topology: `ingest.rs` drops
//! OSM node ids the moment it resolves coordinates. This pass keeps, **for
//! routable ways only**, each way's node-id sequence so shared ids can be
//! recovered as junctions. That shared-node join is the whole point — two ways
//! that touch at an OSM node become adjacent in the graph.
//!
//! The routable-class predicate is deliberately **independent of render styling**
//! (a way can be drawn but not routable, or vice-versa) and **config-free**: the
//! graph is always built with the same class set, so packing the same extract
//! always yields the same graph. It is NOT coupled to the style config.
//!
//! Coordinates are µdeg `(lon, lat)` — the same grid POIs and the serializer's
//! chunk coords live on — so edge lengths reuse `obc-reader`'s shared
//! great-circle helper ([`ground_dist_m`]) and can't drift from the route
//! format's own distances.

use std::collections::{HashMap, HashSet};

use obc_reader::ground_dist_m;

/// Dedup key for an edge: the unordered endpoint pair (canonicalized to `min <=
/// max`) plus its geometry oriented to match, so a way and its reverse-order or
/// parallel duplicate hash equal while genuinely distinct parallel edges survive.
type EdgeKey = (u32, u32, Vec<(i32, i32)>);

/// The routable `highway=*` value set. Most classes are included; **`motorway`
/// and `trunk` (and their `_link`s) are excluded** — a bike router must never
/// send a rider onto a motorway. `_link` ramps of included classes are kept so a
/// junction's slip roads stay connected. The set is a locked decision on #116;
/// it is append-only in spirit but not a normative wire contract (nothing here is
/// serialized), so R2 owns nothing of it.
///
/// Values are matched exactly against the way's `highway` tag; anything not in the
/// set (incl. `motorway`, `trunk`, `construction`, `proposed`, `raceway`, …) is
/// not routable.
const ROUTABLE_HIGHWAY: [&str; 24] = [
    // Roads, coarse → fine.
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
    "unclassified",
    "residential",
    "living_street",
    "road",
    "service",
    // Non-motorized ways a bikepacker actually uses.
    "track",
    "path",
    "cycleway",
    "bridleway",
    "footway",
    "pedestrian",
    "steps",
    "footway_link",
    "cycleway_link",
    "path_link",
    "bridleway_link",
    "living_street_link",
    "service_link",
];

/// Whether a way is routable: its `highway` value is in [`ROUTABLE_HIGHWAY`] AND
/// it is not hard-excluded by `access`. `access=no` / `access=private` are a hard
/// exclude regardless of class (a private drive or a barriered path must not carry
/// a route). Every other `access` value (incl. `permissive`, `destination`,
/// `customers`, or an absent tag) is treated as routable — v1 does not model
/// finer access nuance.
///
/// Independent of render styling by construction: it reads only `highway` and
/// `access`, never the style config.
pub fn is_routable<'a, I>(tags: I) -> bool
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut highway: Option<&str> = None;
    let mut access: Option<&str> = None;
    for (k, v) in tags {
        match k {
            "highway" => highway = Some(v),
            "access" => access = Some(v),
            _ => {}
        }
    }
    if matches!(access, Some("no") | Some("private")) {
        return false;
    }
    highway.is_some_and(|h| ROUTABLE_HIGHWAY.contains(&h))
}

/// One routable way handed to the graph builder: the OSM node-id sequence and the
/// matching µdeg `(lon, lat)` coordinates, in way order. The two vectors are
/// parallel (same length); the builder splits this at junction nodes. Kept only
/// for routable ways — the caller filters via [`is_routable`] before pushing.
#[derive(Debug, Clone)]
pub struct RoutableWay {
    pub node_ids: Vec<i64>,
    pub coords: Vec<(i32, i32)>,
}

/// A junction node in the graph: a dense pack-local id (NOT the OSM id — stable
/// only within one pack run) and its µdeg `(lon, lat)` coordinate.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: u32,
    pub coord: (i32, i32),
}

/// An undirected edge between two junction nodes. `polyline` is the full geometry
/// from `a` to `b` **inclusive of both endpoints**, so `polyline.first()` is `a`'s
/// coord and `polyline.last()` is `b`'s. `length_m` is the summed great-circle
/// length over that polyline. `oneway` is deliberately ignored in v1 (bikes ride
/// both ways).
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub polyline: Vec<(i32, i32)>,
    pub length_m: u32,
}

/// The assembled routable graph. Adjacency is derivable from `edges` (each edge
/// contributes both directions); R2 builds the tiled neighbor lists from these.
#[derive(Debug, Default)]
pub struct NavGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl NavGraph {
    /// Total edge length in kilometers, for the pack-log summary.
    pub fn total_km(&self) -> f64 {
        self.edges.iter().map(|e| e.length_m as f64).sum::<f64>() / 1000.0
    }
}

/// Great-circle length of a polyline (µdeg points) in meters, rounded to the
/// nearest integer, saturating into `u32`. Reuses the shared per-segment helper so
/// edge lengths match the metric the route format and renderer already use; the
/// sum accumulates in `f64` so a long edge doesn't lose precision to `f32`.
/// Crate-visible: the serializer re-measures the pieces of a split edge (§8.4).
pub(crate) fn polyline_len_m(pts: &[(i32, i32)]) -> u32 {
    let mut acc = 0.0f64;
    for w in pts.windows(2) {
        acc += ground_dist_m(w[0], w[1]) as f64;
    }
    acc.round().clamp(0.0, u32::MAX as f64) as u32
}

/// Build the routable graph from the routable ways.
///
/// **Junction detection.** A node is a junction if it is touched by ≥2 routable
/// ways OR it is a routable way's first/last node (endpoints are always
/// junctions). Touch-count is by occurrence across all routable ways, so a closed
/// way (first == last, e.g. a roundabout) naturally makes that node a junction —
/// intended.
///
/// **Edge split + dedup.** Each way is split at every junction it passes through,
/// so edge interiors hold only non-junction nodes. Each distinct junction node
/// gets a dense `u32` id (assignment order = first appearance, stable within the
/// run). Edges with the same unordered `(a, b)` AND identical geometry (parallel /
/// duplicate OSM ways, or a way retraced by a relation) collapse to one edge; the
/// geometry check keeps genuinely distinct parallel edges between the same pair.
pub fn build_graph(ways: &[RoutableWay]) -> NavGraph {
    // --- Pass A: touch-count every node across routable ways. ---
    // A second HashMap over highway nodes — acceptable on the std host (cost noted
    // in the PR). Reserve generously: most highway nodes are shape points touched
    // exactly once.
    let mut touch: HashMap<i64, u32> = HashMap::new();
    for w in ways {
        for &nid in &w.node_ids {
            *touch.entry(nid).or_insert(0) += 1;
        }
    }

    // Whether an OSM node id is a junction: touched ≥2 times, or (handled per-way
    // below) a way endpoint. Endpoints are folded in here so a way whose ends are
    // touched once still splits there.
    let is_junction = |nid: i64, is_endpoint: bool| is_endpoint || touch.get(&nid).copied().unwrap_or(0) >= 2;

    // Dense id assignment for junction OSM nodes, in first-seen order. The Node's
    // coord is taken from the first way that presents it (all ways agree on a
    // shared node's coord — it's the same OSM node).
    let mut dense_id: HashMap<i64, u32> = HashMap::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut intern = |nid: i64, coord: (i32, i32)| -> u32 {
        *dense_id.entry(nid).or_insert_with(|| {
            let id = nodes.len() as u32;
            nodes.push(Node { id, coord });
            id
        })
    };

    // --- Pass B: split each way at its junctions into edges. ---
    // Dedup: an [`EdgeKey`] set collapses duplicate/parallel ways but keeps two
    // distinct edges between the same pair (they differ in geometry).
    let mut seen: HashSet<EdgeKey> = HashSet::new();
    let mut edges: Vec<Edge> = Vec::new();

    for w in ways {
        let n = w.node_ids.len();
        if n < 2 {
            continue; // a degenerate 0/1-node way carries no edge.
        }
        // Walk the way, cutting at each junction. `start` indexes the current
        // edge's first junction; `i` sweeps to the next junction.
        let mut start = 0usize;
        for i in 1..n {
            let is_endpoint = i == n - 1;
            if !is_junction(w.node_ids[i], is_endpoint) {
                continue;
            }
            // `start..=i` is one edge (interiors are non-junctions by construction).
            emit_edge(w, start, i, &mut intern, &mut seen, &mut edges);
            start = i;
        }
    }

    NavGraph { nodes, edges }
}

/// Split out one edge `way[start..=end]` and push it (deduped). `start`/`end` are
/// both junctions; the interior nodes between them are not.
fn emit_edge(
    w: &RoutableWay,
    start: usize,
    end: usize,
    intern: &mut impl FnMut(i64, (i32, i32)) -> u32,
    seen: &mut HashSet<EdgeKey>,
    edges: &mut Vec<Edge>,
) {
    // A self-loop edge (a == b via distinct interior nodes, e.g. a lollipop or a
    // closed way with no other junction) is kept — it is a real routable loop —
    // but a zero-length degenerate (start == end index) can't happen here since
    // end > start always.
    let a = intern(w.node_ids[start], w.coords[start]);
    let b = intern(w.node_ids[end], w.coords[end]);
    let polyline: Vec<(i32, i32)> = w.coords[start..=end].to_vec();

    // Canonicalize for dedup: orient the key by (min,max) endpoint id, reversing
    // the geometry to match, so a way and its reverse-order duplicate hash equal.
    let (ka, kb, kgeom) = if a <= b {
        (a, b, polyline.clone())
    } else {
        let mut rev = polyline.clone();
        rev.reverse();
        (b, a, rev)
    };
    if !seen.insert((ka, kb, kgeom)) {
        return; // exact duplicate (same pair + same geometry) — already have it.
    }

    let length_m = polyline_len_m(&polyline);
    edges.push(Edge { a, b, polyline, length_m });
}

/// The pack-log summary line, alongside the POI counts line:
/// `nav graph: 1234 nodes, 1500 edges, 842.3 km`.
pub fn format_summary(g: &NavGraph) -> String {
    format!("nav graph: {} nodes, {} edges, {:.1} km", g.nodes.len(), g.edges.len(), g.total_km())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `RoutableWay` from `(node_id, lon_udeg, lat_udeg)` triples.
    fn way(pts: &[(i64, i32, i32)]) -> RoutableWay {
        RoutableWay {
            node_ids: pts.iter().map(|&(id, ..)| id).collect(),
            coords: pts.iter().map(|&(_, x, y)| (x, y)).collect(),
        }
    }

    fn tags(pairs: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        pairs.to_vec()
    }

    /// The routable-class predicate: included classes pass, motorway/trunk (+ their
    /// `_link`s) fail, and `access=no|private` is a hard exclude regardless of class.
    #[test]
    fn routable_predicate() {
        assert!(is_routable(tags(&[("highway", "residential")])));
        assert!(is_routable(tags(&[("highway", "track")])));
        assert!(is_routable(tags(&[("highway", "path")])));
        assert!(is_routable(tags(&[("highway", "primary_link")])));
        assert!(is_routable(tags(&[("highway", "steps")])));
        // Excluded classes.
        assert!(!is_routable(tags(&[("highway", "motorway")])));
        assert!(!is_routable(tags(&[("highway", "motorway_link")])));
        assert!(!is_routable(tags(&[("highway", "trunk")])));
        assert!(!is_routable(tags(&[("highway", "trunk_link")])));
        assert!(!is_routable(tags(&[("highway", "construction")])));
        // Not a highway at all.
        assert!(!is_routable(tags(&[("natural", "water")])));
        // Access hard-excludes, even on an otherwise routable class.
        assert!(!is_routable(tags(&[("highway", "service"), ("access", "private")])));
        assert!(!is_routable(tags(&[("highway", "path"), ("access", "no")])));
        // Other access values stay routable.
        assert!(is_routable(tags(&[("highway", "track"), ("access", "permissive")])));
        assert!(is_routable(tags(&[("highway", "cycleway"), ("access", "destination")])));
    }

    /// T-junction: three ways meeting at a shared node → 1 shared junction and 3
    /// edges, each between the shared node and one of the three arm ends. Degree of
    /// the shared node is 3.
    #[test]
    fn t_junction_three_ways() {
        // Shared node 100 at origin; arms reach out along three directions. Each arm
        // is its own way (node 100 + a distinct endpoint).
        let center = (100i64, 7_800_000i32, 47_990_000i32);
        let ways = [
            way(&[center, (1, 7_810_000, 47_990_000)]), // east
            way(&[center, (2, 7_790_000, 47_990_000)]), // west
            way(&[center, (3, 7_800_000, 48_000_000)]), // north
        ];
        let g = build_graph(&ways);
        // 4 distinct junction nodes: the center + 3 arm ends (all endpoints).
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.edges.len(), 3, "three arms → three edges");

        // The center is the first node interned (dense id 0) and has degree 3.
        assert_eq!(g.nodes[0].coord, (7_800_000, 47_990_000), "center is first-seen (id 0)");
        let degree = g.edges.iter().filter(|e| e.a == 0 || e.b == 0).count();
        assert_eq!(degree, 3, "the shared node has degree 3");
    }

    /// 4-way crossing: two ways crossing through one shared node yield a degree-4
    /// node (each way is split at the crossing into two edges).
    #[test]
    fn four_way_crossing() {
        let cross = (100i64, 7_800_000i32, 47_990_000i32);
        let ways = [
            // West–East way passing through the crossing node.
            way(&[(1, 7_790_000, 47_990_000), cross, (2, 7_810_000, 47_990_000)]),
            // South–North way passing through the same node.
            way(&[(3, 7_800_000, 47_980_000), cross, (4, 7_800_000, 48_000_000)]),
        ];
        let g = build_graph(&ways);
        // 5 nodes: the crossing + 4 arm ends.
        assert_eq!(g.nodes.len(), 5);
        // Each way split at the crossing → 2 edges each → 4 total.
        assert_eq!(g.edges.len(), 4);
        // The crossing node (id 1: first way's node 1 is id 0, the crossing is id 1).
        let cross_id = g.nodes.iter().position(|n| n.coord == (7_800_000, 47_990_000)).unwrap() as u32;
        let degree = g.edges.iter().filter(|e| e.a == cross_id || e.b == cross_id).count();
        assert_eq!(degree, 4, "the crossing node has degree 4");
    }

    /// Interior shape points (non-junction, touched once, not endpoints) stay inside
    /// one edge's polyline and are NOT promoted to nodes.
    #[test]
    fn interior_shape_points_are_not_junctions() {
        // A way with two interior shape points that no other way touches.
        let ways = [way(&[
            (1, 7_800_000, 47_990_000),
            (2, 7_800_000, 47_991_000), // interior
            (3, 7_800_000, 47_992_000), // interior
            (4, 7_800_000, 47_993_000),
        ])];
        let g = build_graph(&ways);
        assert_eq!(g.nodes.len(), 2, "only the two endpoints are junctions");
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].polyline.len(), 4, "the edge keeps all 4 points");
    }

    /// Two identical parallel ways (same nodes, same geometry) → ONE deduped edge.
    #[test]
    fn identical_parallel_ways_dedup() {
        let pts = [(1i64, 7_800_000i32, 47_990_000i32), (2, 7_810_000, 47_990_000)];
        let g = build_graph(&[way(&pts), way(&pts)]);
        assert_eq!(g.edges.len(), 1, "two identical ways collapse to one edge");
        assert_eq!(g.nodes.len(), 2);
    }

    /// A way given in reverse order duplicates the forward way → still ONE edge
    /// (the dedup key canonicalizes endpoint order + geometry direction).
    #[test]
    fn reversed_duplicate_dedup() {
        let fwd = way(&[(1, 7_800_000, 47_990_000), (2, 7_810_000, 47_990_000)]);
        let rev = way(&[(2, 7_810_000, 47_990_000), (1, 7_800_000, 47_990_000)]);
        let g = build_graph(&[fwd, rev]);
        assert_eq!(g.edges.len(), 1, "a way and its reverse are the same undirected edge");
    }

    /// Two DISTINCT edges between the same node pair (different intermediate
    /// geometry) both survive — dedup keys on geometry, not just the pair.
    #[test]
    fn distinct_parallel_geometry_kept() {
        // Both ways share endpoints 1 and 2 but bow out on opposite sides via a
        // distinct interior shape point.
        let a = way(&[(1, 7_800_000, 47_990_000), (9, 7_805_000, 47_991_000), (2, 7_810_000, 47_990_000)]);
        let b = way(&[(1, 7_800_000, 47_990_000), (8, 7_805_000, 47_989_000), (2, 7_810_000, 47_990_000)]);
        let g = build_graph(&[a, b]);
        assert_eq!(g.edges.len(), 2, "distinct geometry between the same pair → two edges");
    }

    /// `length_m` matches a hand-computed great-circle sum within rounding. A 3-point
    /// polyline: two ~1.1 km legs (0.01° of latitude ≈ 1113.2 m each).
    #[test]
    fn length_matches_great_circle_sum() {
        let ways = [way(&[
            (1, 7_800_000, 47_990_000),
            (2, 7_800_000, 48_000_000), // +0.01° lat
            (3, 7_800_000, 48_010_000), // +0.01° lat again
        ])];
        let g = build_graph(&ways);
        assert_eq!(g.edges.len(), 1);
        // Hand computation: two legs of 0.01° latitude, M_PER_DEG = 111_320.
        let leg = 0.01f64 * 111_320.0; // 1113.2 m
        let expected = (2.0 * leg).round() as u32; // 2226 m
        let got = g.edges[0].length_m;
        assert!((got as i64 - expected as i64).abs() <= 2, "length {got} ≈ {expected} within rounding");
    }

    /// A closed way (first == last node, no other junction) makes its shared node a
    /// junction (touched twice) → one self-loop edge.
    #[test]
    fn closed_way_is_a_loop() {
        let ways = [way(&[
            (1, 7_800_000, 47_990_000),
            (2, 7_802_000, 47_990_000),
            (3, 7_802_000, 47_992_000),
            (1, 7_800_000, 47_990_000), // back to node 1
        ])];
        let g = build_graph(&ways);
        // Node 1 is touched twice (start + end) → junction; the two interior nodes
        // are touched once and are not junctions.
        assert_eq!(g.nodes.len(), 1, "only node 1 is a junction");
        assert_eq!(g.edges.len(), 1, "the loop is one edge");
        let e = &g.edges[0];
        assert_eq!(e.a, e.b, "a self-loop: both endpoints are node 1");
        assert_eq!(e.polyline.len(), 4, "the loop keeps all four points");
    }

    #[test]
    fn summary_line_format() {
        let g = NavGraph {
            nodes: vec![Node { id: 0, coord: (0, 0) }, Node { id: 1, coord: (0, 0) }],
            edges: vec![Edge { a: 0, b: 1, polyline: vec![(0, 0), (0, 0)], length_m: 2500 }],
        };
        assert_eq!(format_summary(&g), "nav graph: 2 nodes, 1 edges, 2.5 km");
    }
}
