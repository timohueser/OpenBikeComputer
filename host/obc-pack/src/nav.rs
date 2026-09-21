//! Build an in-memory routable navigation graph from the ingested `highway=*` ways: junction nodes
//! joined by undirected edges whose polyline interiors carry no junctions. `serialize.rs` tiles and
//! serializes it into the OBCM nav-graph section; nothing here touches the `.obcm` bytes.
//!
//! The render path drops OSM node ids the moment it resolves coordinates. This pass keeps, for
//! routable ways only, each way's node-id sequence, so a node two ways share becomes a junction.
//! That shared-node join is the whole point.
//!
//! The routable-class predicate ([`is_routable`]) and the [`classify`] way-kind are independent of
//! render styling and config-free, so packing the same extract always yields the same graph.
//! [`build_graph`] also classifies way kinds, drops bike-illegal ways, prunes tiny islands, and
//! splits edges so the serialized `i16` neighbor deltas and `u16` costs hold by construction.
//!
//! Coordinates are µdeg `(lon, lat)` — the grid POIs and the serializer live on — so edge lengths
//! reuse [`ground_dist_m`] and cannot drift from the route format's own distances.

use std::collections::{HashMap, HashSet};

use obc_elevation::{ElevationSource, ProfileIntegrator};
use obc_map_scene::ground_dist_m;

/// Dedup key for an edge: the unordered endpoint pair (canonicalized to `min <= max`), its geometry
/// oriented to match, and its way-kind. A way and its reverse-order or parallel duplicate hash
/// equal, while a cycleway drawn over a road survives as a separate edge.
type EdgeKey = (u32, u32, Vec<(i32, i32)>, u8);

/// Default island-pruning threshold: keep every connected component with at least this many edges,
/// plus the single largest. The packer threads `routing.min_component_edges` through
/// [`build_graph_with`] instead; this default is for tests.
pub const DEFAULT_MIN_COMPONENT_EDGES: usize = 50;

/// Maximum endpoint-to-endpoint lat or lon delta (µdeg) an edge may span before [`build_graph`]
/// splits it. Each neighbor's coordinate is stored as an `i16` µdeg delta from the record's own
/// node, so both endpoints of every edge must sit within `i16` range of each other; `32 000` keeps a
/// margin below `i16::MAX`.
const MAX_ENDPOINT_DELTA_UDEG: i64 = 32_000;

/// Maximum edge `length_m` before [`build_graph`] splits it. A neighbor's cost is a `u16`, and
/// `60 000` keeps a margin below `u16::MAX`.
const MAX_EDGE_LEN_M: u32 = 60_000;

/// Canonical highway-class names, indexed by the 5-bit class id. Also the profile config's class
/// keys: a `routing.profiles[*].highway` map is keyed by these exact names, resolved via
/// [`highway_class_index`]. One source of truth for the packed byte and the config vocabulary.
pub const HIGHWAY_CLASS_NAMES: [&str; 14] = [
    "cycleway",      // 0
    "path",          // 1
    "track",         // 2
    "footway",       // 3
    "steps",         // 4
    "bridleway",     // 5
    "living_street", // 6
    "residential",   // 7
    "service",       // 8
    "unclassified",  // 9
    "tertiary",      // 10
    "secondary",     // 11
    "primary",       // 12
    "trunk_cycl",    // 13
];

/// Canonical surface-class names, indexed by the 3-bit class id. The other half of the profile
/// config's class vocabulary, resolved via [`surface_class_index`].
pub const SURFACE_CLASS_NAMES: [&str; 8] =
    ["unknown", "paved", "compacted", "gravel", "dirt", "rough", "cobbles", "grass"];

/// Resolve a highway-class name to its 5-bit class id, or `None` for an unknown name. The config's
/// profile parser uses this to key its per-class multipliers.
pub fn highway_class_index(name: &str) -> Option<u8> {
    HIGHWAY_CLASS_NAMES.iter().position(|&n| n == name).map(|i| i as u8)
}

/// Resolve a surface-class name to its 3-bit class id, or `None`.
pub fn surface_class_index(name: &str) -> Option<u8> {
    SURFACE_CLASS_NAMES.iter().position(|&n| n == name).map(|i| i as u8)
}

/// Map an OSM `highway=*` value to its highway class (5-bit). `None` for a value that carries no
/// class, including `motorway`, which is always bike-illegal, and `trunk`, which [`classify`]
/// handles separately (class 13, only with `bicycle=yes`).
fn highway_class(highway: &str) -> Option<u8> {
    Some(match highway {
        "cycleway" | "cycleway_link" => 0,
        "path" | "path_link" => 1,
        "track" => 2,
        "footway" | "pedestrian" | "footway_link" => 3,
        "steps" => 4,
        "bridleway" | "bridleway_link" => 5,
        "living_street" | "living_street_link" => 6,
        "residential" => 7,
        "service" | "service_link" => 8,
        "unclassified" | "road" => 9,
        "tertiary" | "tertiary_link" => 10,
        "secondary" | "secondary_link" => 11,
        "primary" | "primary_link" => 12,
        // `trunk`/`trunk_link` are class 13, but only with `bicycle=yes` — see `classify`.
        _ => return None,
    })
}

/// Map an OSM `surface=*` value to its surface class (3-bit). Absent or unrecognized gives `0`.
fn surface_class(surface: Option<&str>) -> u8 {
    match surface {
        Some("paved" | "asphalt" | "concrete" | "paving_stones" | "concrete:plates" | "concrete:lanes") => 1,
        Some("compacted" | "fine_gravel") => 2,
        Some("gravel" | "pebblestone" | "unpaved") => 3,
        Some("ground" | "dirt" | "earth") => 4,
        Some("sand" | "mud") => 5,
        Some("cobblestone" | "sett" | "unhewn_cobblestone") => 6,
        Some("grass" | "grass_paver") => 7,
        _ => 0,
    }
}

/// Classify a way into its packed way-kind byte, or `None` if the way is not routable.
///
/// The byte is `kind = (surface_class << 5) | highway_class`, from the canonical [`highway_class`]
/// and [`surface_class`] tables that `OBCM_Spec.md` mirrors. The device never sees raw tags: a
/// routing profile weights edges purely off this byte.
///
/// A way is not routable when any hard-exclude applies: `highway=motorway|motorway_link`;
/// `highway=trunk|trunk_link` unless `bicycle=yes`; `motorroad=yes`; `bicycle=no|use_sidepath`;
/// `access=no|private`. Everything else is kept, including `footway` and `steps`, which are legal to
/// walk a bike along, and `bicycle=dismount`. Preference rather than legality is the router's job.
pub fn classify<'a, I>(tags: I) -> Option<u8>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let mut highway: Option<&str> = None;
    let mut surface: Option<&str> = None;
    let mut bicycle: Option<&str> = None;
    let mut access: Option<&str> = None;
    let mut motorroad: Option<&str> = None;
    for (k, v) in tags {
        match k {
            "highway" => highway = Some(v),
            "surface" => surface = Some(v),
            "bicycle" => bicycle = Some(v),
            "access" => access = Some(v),
            "motorroad" => motorroad = Some(v),
            _ => {}
        }
    }

    // Hard bike-illegal excludes (checked before any class assignment).
    if matches!(access, Some("no") | Some("private")) {
        return None;
    }
    if motorroad == Some("yes") {
        return None;
    }
    if matches!(bicycle, Some("no") | Some("use_sidepath")) {
        return None;
    }

    let highway = highway?;
    let hclass = match highway {
        // `trunk` is legal for bikes only when explicitly allowed, and is then its own class.
        "trunk" | "trunk_link" => {
            if bicycle == Some("yes") {
                13
            } else {
                return None;
            }
        }
        other => highway_class(other)?,
    };
    Some((surface_class(surface) << 5) | hclass)
}

/// Whether a way is routable for a bike: exactly `classify(tags).is_some()`. A named predicate
/// because that is how the ingest reads it — routability first, then the kind.
pub fn is_routable<'a, I>(tags: I) -> bool
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    classify(tags).is_some()
}

/// One routable way handed to the graph builder: the OSM node-id sequence, the matching µdeg
/// `(lon, lat)` coordinates in way order, and the way's packed [`classify`] `kind`. The caller
/// filters before pushing, so `kind` is always a real class here.
#[derive(Debug, Clone)]
pub struct RoutableWay {
    pub node_ids: Vec<i64>,
    pub coords: Vec<(i32, i32)>,
    pub kind: u8,
}

/// A junction node: a dense pack-local id (not the OSM id, and stable only within one pack run) and
/// its µdeg `(lon, lat)` coordinate. Synthetic degree-2 nodes from the edge-split pass get ids past
/// the real ones.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: u32,
    pub coord: (i32, i32),
}

/// An undirected edge between two junction nodes. `polyline` runs from `a` to `b` inclusive of both
/// endpoints, and `length_m` is its summed great-circle length. `kind` is the parent way's
/// [`classify`] byte, which every split piece inherits. `oneway` is deliberately ignored: bikes ride
/// both ways.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub a: u32,
    pub b: u32,
    pub polyline: Vec<(i32, i32)>,
    pub length_m: u32,
    pub kind: u8,
}

/// The assembled routable graph. Adjacency is derivable from `edges`, since each contributes both
/// directions.
#[derive(Debug, Default)]
pub struct NavGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// Build-time statistics returned alongside the graph, for the pack-log summary
/// ([`format_summary`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NavStats {
    /// Connected components found before pruning.
    pub components_found: usize,
    /// Components kept: the largest, plus those at or over the threshold.
    pub components_kept: usize,
    /// Edges dropped with the pruned islands.
    pub edges_dropped: usize,
}

impl NavGraph {
    /// Total edge length in kilometers, for the pack-log summary.
    pub fn total_km(&self) -> f64 {
        self.edges.iter().map(|e| e.length_m as f64).sum::<f64>() / 1000.0
    }
}

/// Great-circle length of a polyline (µdeg points) in metres, rounded and saturating into `u32`. The
/// sum accumulates in `f64`, so a long edge does not lose precision to `f32`.
pub(crate) fn polyline_len_m(pts: &[(i32, i32)]) -> u32 {
    let mut acc = 0.0f64;
    for w in pts.windows(2) {
        acc += ground_dist_m(w[0], w[1]) as f64;
    }
    acc.round().clamp(0.0, u32::MAX as f64) as u32
}

/// Build the routable graph from the routable ways, plus build-time [`NavStats`].
///
/// A node is a junction if two or more routable ways touch it, or it is a way's first or last node.
/// Touch-count is by occurrence across all routable ways, so a closed way such as a roundabout makes
/// its own node a junction. Each way is then split at every junction it passes through, so edge
/// interiors hold only non-junction nodes, and each junction gets a dense `u32` id in
/// first-appearance order. Edges with the same unordered `(a, b)`, identical geometry and the same
/// [`Edge::kind`] collapse to one; keying on kind keeps a cycleway drawn over a road distinct.
///
/// Island pruning keeps the largest component plus every component with at least
/// [`DEFAULT_MIN_COMPONENT_EDGES`] edges, so a rider cannot snap onto an unroutable islet. Finally
/// any edge over [`MAX_ENDPOINT_DELTA_UDEG`] or [`MAX_EDGE_LEN_M`] is split at a polyline vertex
/// into pieces joined by synthetic degree-2 nodes, each piece's cost re-measured, so the serialized
/// records are valid by construction.
pub fn build_graph(ways: &[RoutableWay]) -> (NavGraph, NavStats) {
    build_graph_with(ways, DEFAULT_MIN_COMPONENT_EDGES)
}

/// [`build_graph`] with the island-pruning threshold supplied by the caller; the packer wires
/// `routing.min_component_edges` here.
pub fn build_graph_with(ways: &[RoutableWay], min_component_edges: usize) -> (NavGraph, NavStats) {
    // Pass A: touch-count every node across the routable ways.
    let mut touch: HashMap<i64, u32> = HashMap::new();
    for w in ways {
        for &nid in &w.node_ids {
            *touch.entry(nid).or_insert(0) += 1;
        }
    }

    // A junction: touched twice or more, or (handled per way below) a way endpoint.
    let is_junction = |nid: i64, is_endpoint: bool| is_endpoint || touch.get(&nid).copied().unwrap_or(0) >= 2;

    // Dense id assignment for junction OSM nodes, in first-seen order.
    let mut dense_id: HashMap<i64, u32> = HashMap::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut intern = |nid: i64, coord: (i32, i32)| -> u32 {
        *dense_id.entry(nid).or_insert_with(|| {
            let id = nodes.len() as u32;
            nodes.push(Node { id, coord });
            id
        })
    };

    // Pass B: split each way at its junctions into edges.
    let mut seen: HashSet<EdgeKey> = HashSet::new();
    let mut edges: Vec<Edge> = Vec::new();
    for w in ways {
        let n = w.node_ids.len();
        if n < 2 {
            continue; // a degenerate 0/1-node way carries no edge.
        }
        let mut start = 0usize;
        for i in 1..n {
            let is_endpoint = i == n - 1;
            if !is_junction(w.node_ids[i], is_endpoint) {
                continue;
            }
            emit_edge(&w.node_ids, &w.coords, w.kind, start, i, &mut intern, &mut seen, &mut edges);
            start = i;
        }
    }

    // Pass C: island pruning.
    let (nodes, edges, stats) = prune_islands(nodes, edges, min_component_edges, None);

    // Pass D: split edges so the serialized i16 deltas and u16 costs hold.
    let mut nodes = nodes;
    let mut split: Vec<Edge> = Vec::with_capacity(edges.len());
    for e in edges {
        split_edge(e, &mut nodes, &mut split);
    }

    (NavGraph { nodes, edges: split }, stats)
}

/// A junction's identity inside one cell.
///
/// A whole-extract pack identifies junctions by OSM node id. A cell cannot: the junctions that carry
/// a seam are minted at the cell edge, have no OSM id, and must come out identical in both
/// neighbours, so they are identified by their coordinate, which both neighbours compute with the
/// same integer formula ([`crate::grid::segment_crossing`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JunctionKey {
    /// A real OSM node.
    Osm(i64),
    /// A junction materialised on a cell-edge line, keyed by its `(lon, lat)` µdeg coordinate.
    Boundary(i32, i32),
}

/// One routable way as a single cell sees it: the run of its polyline that this cell owns, with a
/// junction key per vertex (parallel to `coords`) and the parent way's `kind`. The cutting itself is
/// [`crate::cut`]'s job, because it is grid work, not graph work.
#[derive(Clone, Debug)]
pub struct CutRun {
    pub keys: Vec<JunctionKey>,
    /// µdeg `(lon, lat)`, parallel to `keys`.
    pub coords: Vec<(i32, i32)>,
    pub kind: u8,
}

/// Build a cell's routable graph from the runs the cutter carved out of the source ways.
///
/// Same machinery as [`build_graph_with`] — junction split, dedup, prune, then the bounded edge
/// splits — with two rules that make a cell's graph assemble correctly. `is_junction` is supplied by
/// the caller, because junction-ness must be classified from the source snapshot's whole way set,
/// never from the ways that happen to survive inside this cell; a run's first and last vertices are
/// always junctions. `on_boundary` protects components that touch the cell edge from pruning, so a
/// good road whose continuation is in the neighbour is never dropped as an island — the real pruning
/// runs at assembly time, where component sizes are finally true.
pub fn build_graph_cut(
    runs: &[CutRun],
    min_component_edges: usize,
    is_junction: &dyn Fn(JunctionKey, (i32, i32)) -> bool,
    on_boundary: &dyn Fn((i32, i32)) -> bool,
) -> (NavGraph, NavStats) {
    let mut dense_id: HashMap<JunctionKey, u32> = HashMap::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut intern = |key: JunctionKey, coord: (i32, i32)| -> u32 {
        *dense_id.entry(key).or_insert_with(|| {
            let id = nodes.len() as u32;
            nodes.push(Node { id, coord });
            id
        })
    };

    let mut seen: HashSet<EdgeKey> = HashSet::new();
    let mut edges: Vec<Edge> = Vec::new();
    for r in runs {
        debug_assert_eq!(r.keys.len(), r.coords.len(), "a run's keys are parallel to its coords");
        let n = r.keys.len();
        if n < 2 {
            continue;
        }
        let mut start = 0usize;
        for i in 1..n {
            let is_endpoint = i == n - 1;
            if !is_endpoint && !is_junction(r.keys[i], r.coords[i]) {
                continue;
            }
            emit_edge(&r.keys, &r.coords, r.kind, start, i, &mut intern, &mut seen, &mut edges);
            start = i;
        }
    }

    let (nodes, edges, stats) = prune_islands(nodes, edges, min_component_edges, Some(on_boundary));
    let mut nodes = nodes;
    let mut split: Vec<Edge> = Vec::with_capacity(edges.len());
    for e in edges {
        split_edge(e, &mut nodes, &mut split);
    }
    (NavGraph { nodes, edges: split }, stats)
}

/// Split out one edge `keys/coords[start..=end]` and push it, deduped. `start` and `end` are both
/// junctions; the interior nodes between them are not. The edge inherits the parent way's `kind`.
///
/// Generic over the junction-identity type, so the whole-extract build (OSM node ids) and the cell
/// cutter ([`build_graph_cut`], whose boundary junctions are identified by coordinate) share one
/// dedup and interning path instead of two that can drift.
#[allow(clippy::too_many_arguments)]
fn emit_edge<K: Copy>(
    keys: &[K],
    coords: &[(i32, i32)],
    kind: u8,
    start: usize,
    end: usize,
    intern: &mut impl FnMut(K, (i32, i32)) -> u32,
    seen: &mut HashSet<EdgeKey>,
    edges: &mut Vec<Edge>,
) {
    let a = intern(keys[start], coords[start]);
    let b = intern(keys[end], coords[end]);
    let polyline: Vec<(i32, i32)> = coords[start..=end].to_vec();

    // Canonicalize for dedup: orient the key by (min, max) endpoint id, reversing the geometry to
    // match, so a way and its reverse-order duplicate hash equal. Kind is part of the key, so a
    // same-geometry way of a different class is not collapsed.
    let (ka, kb, kgeom) = if a <= b {
        (a, b, polyline.clone())
    } else {
        let mut rev = polyline.clone();
        rev.reverse();
        (b, a, rev)
    };
    if !seen.insert((ka, kb, kgeom, kind)) {
        return; // exact duplicate (same pair + geometry + kind) — already have it.
    }

    let length_m = polyline_len_m(&polyline);
    edges.push(Edge { a, b, polyline, length_m, kind });
}

/// A minimal union-find (path halving + union by size) over the dense node ids.
struct UnionFind {
    parent: Vec<u32>,
    size: Vec<u32>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self { parent: (0..n as u32).collect(), size: vec![1; n] }
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.parent[x as usize] != x {
            let gp = self.parent[self.parent[x as usize] as usize];
            self.parent[x as usize] = gp; // path halving
            x = gp;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return;
        }
        let (big, small) = if self.size[ra as usize] >= self.size[rb as usize] { (ra, rb) } else { (rb, ra) };
        self.parent[small as usize] = big;
        self.size[big as usize] += self.size[small as usize];
    }
}

/// Drop tiny disconnected components. Union-find over edge endpoints groups nodes into components;
/// the largest by node count, plus every component with at least `min_component_edges` edges, are
/// kept. Surviving nodes are re-densified in original order (an all-kept graph is an identity remap)
/// and edges re-pointed at the new ids.
///
/// `protected`, when given, names coordinates whose component must survive whatever its size. A hard
/// cut at a cell edge leaves fragments that are only small because their continuation lives in the
/// neighbour, so a cell may prune only components strictly interior to it.
fn prune_islands(
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    min_component_edges: usize,
    protected: Option<&dyn Fn((i32, i32)) -> bool>,
) -> (Vec<Node>, Vec<Edge>, NavStats) {
    if nodes.is_empty() {
        return (nodes, edges, NavStats::default());
    }

    let mut uf = UnionFind::new(nodes.len());
    for e in &edges {
        uf.union(e.a, e.b);
    }
    // Resolve each node's and edge's component root once (find needs &mut).
    let node_root: Vec<u32> = (0..nodes.len() as u32).map(|i| uf.find(i)).collect();
    let edge_root: Vec<u32> = edges.iter().map(|e| uf.find(e.a)).collect();

    let mut node_count: HashMap<u32, usize> = HashMap::new();
    for &r in &node_root {
        *node_count.entry(r).or_insert(0) += 1;
    }
    let mut edge_count: HashMap<u32, usize> = HashMap::new();
    for &r in &edge_root {
        *edge_count.entry(r).or_insert(0) += 1;
    }
    let components_found = node_count.len();

    // Largest by node count, ties by edge count then smallest root id, so the choice never depends
    // on HashMap iteration order.
    let largest = node_count
        .keys()
        .copied()
        .max_by_key(|r| (node_count[r], edge_count.get(r).copied().unwrap_or(0), std::cmp::Reverse(*r)))
        .expect("nonempty graph has ≥1 component");

    // A component holding a protected coordinate survives regardless of size; resolved up front, so
    // the keep test stays a set lookup.
    let protected_roots: HashSet<u32> = match protected {
        None => HashSet::new(),
        Some(is_protected) => {
            nodes.iter().filter(|n| is_protected(n.coord)).map(|n| node_root[n.id as usize]).collect()
        }
    };

    let kept_roots: HashSet<u32> = node_count
        .keys()
        .copied()
        .filter(|r| {
            *r == largest
                || protected_roots.contains(r)
                || edge_count.get(r).copied().unwrap_or(0) >= min_component_edges
        })
        .collect();
    let components_kept = kept_roots.len();

    // Re-densify surviving nodes (original order) and re-point edges.
    let mut id_map: HashMap<u32, u32> = HashMap::new();
    let mut new_nodes: Vec<Node> = Vec::with_capacity(nodes.len());
    for n in &nodes {
        if kept_roots.contains(&node_root[n.id as usize]) {
            let new_id = new_nodes.len() as u32;
            id_map.insert(n.id, new_id);
            new_nodes.push(Node { id: new_id, coord: n.coord });
        }
    }
    let mut new_edges: Vec<Edge> = Vec::with_capacity(edges.len());
    let mut edges_dropped = 0usize;
    for (e, &root) in edges.into_iter().zip(&edge_root) {
        if kept_roots.contains(&root) {
            let a = id_map[&e.a];
            let b = id_map[&e.b];
            new_edges.push(Edge { a, b, polyline: e.polyline, length_m: e.length_m, kind: e.kind });
        } else {
            edges_dropped += 1;
        }
    }

    (new_nodes, new_edges, NavStats { components_found, components_kept, edges_dropped })
}

/// Whether an edge exceeds either bound: endpoint-to-endpoint lat/lon delta over
/// [`MAX_ENDPOINT_DELTA_UDEG`], or `length_m` over [`MAX_EDGE_LEN_M`].
fn edge_exceeds_bounds(polyline: &[(i32, i32)], length_m: u32) -> bool {
    let a = polyline[0];
    let b = *polyline.last().unwrap();
    (a.0 as i64 - b.0 as i64).abs() > MAX_ENDPOINT_DELTA_UDEG
        || (a.1 as i64 - b.1 as i64).abs() > MAX_ENDPOINT_DELTA_UDEG
        || length_m > MAX_EDGE_LEN_M
}

/// The interior vertex whose cumulative great-circle length is nearest half the polyline's total,
/// which balances the two pieces' costs. Always strictly interior, so both pieces have at least two
/// points and strictly fewer than the parent, which guarantees termination.
fn midpoint_index(polyline: &[(i32, i32)]) -> usize {
    let mut cum = vec![0.0f64; polyline.len()];
    for i in 1..polyline.len() {
        cum[i] = cum[i - 1] + ground_dist_m(polyline[i - 1], polyline[i]) as f64;
    }
    let half = cum[polyline.len() - 1] / 2.0;
    let mut best = 1usize;
    let mut best_d = f64::MAX;
    for (i, &c) in cum.iter().enumerate().take(polyline.len() - 1).skip(1) {
        let d = (c - half).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

/// Split one edge until every piece holds the bounds, pushing the pieces onto `out`. Each split cuts
/// at [`midpoint_index`] and inserts a synthetic degree-2 junction there, and each piece's cost is
/// re-measured, so the pieces sum to the original within rounding.
///
/// A 2-point edge that still violates a bound — a single OSM segment longer than the bound with no
/// shape node — gets one interpolated midpoint on the straight line between its endpoints first.
fn split_edge(e: Edge, nodes: &mut Vec<Node>, out: &mut Vec<Edge>) {
    if !edge_exceeds_bounds(&e.polyline, e.length_m) {
        out.push(e);
        return;
    }

    let poly = if e.polyline.len() >= 3 {
        e.polyline
    } else {
        let a = e.polyline[0];
        let b = e.polyline[1];
        let mid = (((a.0 as i64 + b.0 as i64) / 2) as i32, ((a.1 as i64 + b.1 as i64) / 2) as i32);
        vec![a, mid, b]
    };

    let cut = midpoint_index(&poly);
    let left_poly = poly[..=cut].to_vec();
    let right_poly = poly[cut..].to_vec();
    let synth = nodes.len() as u32;
    nodes.push(Node { id: synth, coord: poly[cut] });

    let left = Edge { a: e.a, b: synth, length_m: polyline_len_m(&left_poly), polyline: left_poly, kind: e.kind };
    let right = Edge { a: synth, b: e.b, length_m: polyline_len_m(&right_poly), polyline: right_poly, kind: e.kind };
    split_edge(left, nodes, out);
    split_edge(right, nodes, out);
}

/// Longest ground gap (m) the ascent sampler leaves between two elevation samples on an edge.
///
/// The number is a property of the raster: OBCT data is posted at `2^9` µdeg, about 57 m in latitude
/// and less in longitude at European latitudes. Stepping at 50 m guarantees at least one sample per
/// posting cell, so a hill between two far-apart OSM shape nodes cannot be stepped over. Sampling
/// finer only re-reads the same bilinear surface.
pub const ASCENT_SAMPLE_STEP_M: f32 = 50.0;

/// Integrate a nav edge's climb in both directions, in metres, saturating into the `u16` a neighbor
/// entry carries. Returns `(a->b, b->a)` for a polyline running `a … b`.
///
/// Two directions rather than one plus a sign, because ascent is an integral: a pass between two
/// equal-height junctions has hundreds of metres of climb each way and no net change at all.
///
/// Every polyline vertex is sampled, plus interpolated points so that no two consecutive samples are
/// more than [`ASCENT_SAMPLE_STEP_M`] of ground apart. Interpolation is integer µdeg and symmetric
/// under reversal, so the forward and backward passes see the same sample coordinates in opposite
/// order — the property that makes `ascent(b->a)` exactly `descent(a->b)` on covered terrain.
///
/// The dead-band is the shared [`ELE_DEADBAND_M`](obc_elevation::ELE_DEADBAND_M), not a
/// packer-private one: the ascent a route is costed by and the ascent a rider is shown have to be
/// the same number. A sample with no height drops the dead-band's reference rather than bridging it,
/// so climb across a hole in coverage is never booked. With no terrain at all the answer is
/// `(0, 0)`, which is the degrade path.
pub fn integrate_edge_ascent(polyline: &[(i32, i32)], source: &mut dyn ElevationSource) -> (u16, u16) {
    let (forward, backward, _) = integrate_edge_facts(polyline, source);
    (forward, backward)
}

/// Directional ascent and whether every integration sample resolved.
pub fn integrate_edge_facts(polyline: &[(i32, i32)], source: &mut dyn ElevationSource) -> (u16, u16, bool) {
    let (forward, a) = ascent_along(polyline.iter().copied(), source);
    let (backward, b) = ascent_along(polyline.iter().rev().copied(), source);
    (forward, backward, a && b)
}

/// One direction of [`integrate_edge_ascent`]: densify to [`ASCENT_SAMPLE_STEP_M`], sample, fold.
fn ascent_along(pts: impl Iterator<Item = (i32, i32)>, source: &mut dyn ElevationSource) -> (u16, bool) {
    let mut it = ProfileIntegrator::<f32>::new();
    let mut complete = true;
    let mut dist = 0.0f32;
    let mut prev: Option<(i32, i32)> = None;
    fn push(
        it: &mut ProfileIntegrator<f32>,
        dist: f32,
        p: (i32, i32),
        source: &mut dyn ElevationSource,
        complete: &mut bool,
    ) {
        match source.sample(p.1, p.0) {
            Some(h) => it.push(dist, f32::from(h)),
            // No height here: keep the length, drop the reference. Booking the climb across an
            // unsampled stretch would invent metres the rider never rides.
            None => {
                *complete = false;
                it.band().pause();
            }
        }
    }
    for p in pts {
        let Some(from) = prev else {
            push(&mut it, dist, p, source, &mut complete);
            prev = Some(p);
            continue;
        };
        let seg = seg_len_m(from, p);
        let steps = (seg / ASCENT_SAMPLE_STEP_M).ceil().max(1.0) as u32;
        for t in 1..=steps {
            let at = lerp_udeg(from, p, t, steps);
            push(&mut it, dist + seg * (t as f32 / steps as f32), at, source, &mut complete);
        }
        dist += seg;
        prev = Some(p);
    }
    (it.ascent_u16(), complete)
}

/// A direction-independent segment length (m), used only to choose how many samples a segment gets.
///
/// [`ground_dist_m`] takes its `cos(lat)` from its first argument, so it is very slightly
/// asymmetric — far below a metre, but enough to make the forward and backward passes disagree on
/// `steps` for a segment sitting on a rounding boundary, and therefore sample two different point
/// sets. Edge `length_m` keeps using [`polyline_len_m`]; this governs sampling density only.
fn seg_len_m(a: (i32, i32), b: (i32, i32)) -> f32 {
    let (p, q) = if a <= b { (a, b) } else { (b, a) };
    ground_dist_m(p, q)
}

/// The point `t/k` of the way from `a` to `b` in integer µdeg, rounded half away from zero.
///
/// Deliberately expressed so that `lerp_udeg(a, b, t, k) == lerp_udeg(b, a, k - t, k)`: the forward
/// and reverse passes must land on the same coordinates, or the two directions would sample two
/// slightly different lines.
fn lerp_udeg(a: (i32, i32), b: (i32, i32), t: u32, k: u32) -> (i32, i32) {
    let one = |a: i32, b: i32| -> i32 {
        let num = a as i64 * (k - t) as i64 + b as i64 * t as i64;
        let den = k as i64;
        let half = den / 2;
        (if num >= 0 { (num + half) / den } else { -((-num + half) / den) }) as i32
    };
    (one(a.0, b.0), one(a.1, b.1))
}

/// The pack-log nav summary: graph totals, component counts, and a histogram of edges per highway
/// class (the 5 low bits of [`Edge::kind`]), most-common first, so a glance shows the graph's
/// character.
pub fn format_summary(g: &NavGraph, stats: &NavStats) -> String {
    let mut hist: HashMap<u8, usize> = HashMap::new();
    for e in &g.edges {
        *hist.entry(e.kind & 0x1F).or_insert(0) += 1;
    }
    let mut rows: Vec<(u8, usize)> = hist.into_iter().collect();
    // Most-common first; ties by class id for determinism.
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let kinds = if rows.is_empty() {
        "(none)".to_string()
    } else {
        rows.iter()
            .map(|&(cls, n)| {
                let name = HIGHWAY_CLASS_NAMES.get(cls as usize).copied().unwrap_or("reserved");
                format!("{name} {n}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    format!(
        "nav graph: {} nodes, {} edges, {:.1} km\n\
         nav components: {} found, {} kept, {} edges dropped\n\
         nav kinds: {}",
        g.nodes.len(),
        g.edges.len(),
        g.total_km(),
        stats.components_found,
        stats.components_kept,
        stats.edges_dropped,
        kinds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `RoutableWay` of kind 0 from `(node_id, lon_udeg, lat_udeg)` triples.
    fn way(pts: &[(i64, i32, i32)]) -> RoutableWay {
        way_kind(pts, 0)
    }

    fn way_kind(pts: &[(i64, i32, i32)], kind: u8) -> RoutableWay {
        RoutableWay {
            node_ids: pts.iter().map(|&(id, ..)| id).collect(),
            coords: pts.iter().map(|&(_, x, y)| (x, y)).collect(),
            kind,
        }
    }

    fn tags(pairs: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        pairs.to_vec()
    }

    /// Every canonical highway-class row maps to its id, `_link` variants and aliases included.
    #[test]
    fn classify_highway_rows() {
        let hw = |v| classify(tags(&[("highway", v)])).map(|k| k & 0x1F);
        assert_eq!(hw("cycleway"), Some(0));
        assert_eq!(hw("cycleway_link"), Some(0));
        assert_eq!(hw("path"), Some(1));
        assert_eq!(hw("path_link"), Some(1));
        assert_eq!(hw("track"), Some(2));
        assert_eq!(hw("footway"), Some(3));
        assert_eq!(hw("pedestrian"), Some(3));
        assert_eq!(hw("footway_link"), Some(3));
        assert_eq!(hw("steps"), Some(4));
        assert_eq!(hw("bridleway"), Some(5));
        assert_eq!(hw("bridleway_link"), Some(5));
        assert_eq!(hw("living_street"), Some(6));
        assert_eq!(hw("living_street_link"), Some(6));
        assert_eq!(hw("residential"), Some(7));
        assert_eq!(hw("service"), Some(8));
        assert_eq!(hw("service_link"), Some(8));
        assert_eq!(hw("unclassified"), Some(9));
        assert_eq!(hw("road"), Some(9));
        assert_eq!(hw("tertiary"), Some(10));
        assert_eq!(hw("tertiary_link"), Some(10));
        assert_eq!(hw("secondary"), Some(11));
        assert_eq!(hw("secondary_link"), Some(11));
        assert_eq!(hw("primary"), Some(12));
        assert_eq!(hw("primary_link"), Some(12));
        // Unmapped highway ⇒ not routable.
        assert_eq!(hw("construction"), None);
        assert_eq!(hw("raceway"), None);
    }

    /// The packed byte is `(surface << 5) | highway`; an unknown or absent surface is class 0.
    #[test]
    fn classify_surface_rows_and_packing() {
        let sfc = |s| classify(tags(&[("highway", "track"), ("surface", s)])).map(|k| k >> 5);
        assert_eq!(sfc("asphalt"), Some(1));
        assert_eq!(sfc("concrete:lanes"), Some(1));
        assert_eq!(sfc("compacted"), Some(2));
        assert_eq!(sfc("fine_gravel"), Some(2));
        assert_eq!(sfc("gravel"), Some(3));
        assert_eq!(sfc("unpaved"), Some(3));
        assert_eq!(sfc("ground"), Some(4));
        assert_eq!(sfc("sand"), Some(5));
        assert_eq!(sfc("sett"), Some(6));
        assert_eq!(sfc("grass"), Some(7));
        // Unknown / absent surface ⇒ 0.
        assert_eq!(sfc("moon_dust"), Some(0));
        assert_eq!(classify(tags(&[("highway", "track")])).map(|k| k >> 5), Some(0));
        // Full packed byte: cycleway (0) on asphalt (1) ⇒ 0x20; path (1) on gravel (3) ⇒ 0x61.
        assert_eq!(classify(tags(&[("highway", "cycleway"), ("surface", "asphalt")])), Some(0x20));
        assert_eq!(classify(tags(&[("highway", "path"), ("surface", "gravel")])), Some(0x61));
    }

    /// The hard-excludes reject, `trunk` plus `bicycle=yes` becomes class 13, `dismount` stays.
    #[test]
    fn classify_bike_legality() {
        // trunk is class 13 ONLY with bicycle=yes; alone it's not routable.
        assert_eq!(classify(tags(&[("highway", "trunk"), ("bicycle", "yes")])).map(|k| k & 0x1F), Some(13));
        assert_eq!(classify(tags(&[("highway", "trunk_link"), ("bicycle", "yes")])).map(|k| k & 0x1F), Some(13));
        assert_eq!(classify(tags(&[("highway", "trunk")])), None);
        assert_eq!(classify(tags(&[("highway", "trunk_link")])), None);
        // motorway is always excluded, even with bicycle=yes.
        assert_eq!(classify(tags(&[("highway", "motorway")])), None);
        assert_eq!(classify(tags(&[("highway", "motorway_link")])), None);
        assert_eq!(classify(tags(&[("highway", "motorway"), ("bicycle", "yes")])), None);
        // Hard excludes on otherwise-routable classes.
        assert_eq!(classify(tags(&[("highway", "cycleway"), ("bicycle", "no")])), None);
        assert_eq!(classify(tags(&[("highway", "path"), ("bicycle", "use_sidepath")])), None);
        assert_eq!(classify(tags(&[("highway", "primary"), ("motorroad", "yes")])), None);
        assert_eq!(classify(tags(&[("highway", "service"), ("access", "private")])), None);
        assert_eq!(classify(tags(&[("highway", "path"), ("access", "no")])), None);
        // dismount stays routable with its normal footway kind.
        assert_eq!(classify(tags(&[("highway", "footway"), ("bicycle", "dismount")])).map(|k| k & 0x1F), Some(3));
        // Other access values stay routable.
        assert!(is_routable(tags(&[("highway", "track"), ("access", "permissive")])));
        assert!(is_routable(tags(&[("highway", "cycleway"), ("access", "destination")])));
        // Not a highway at all.
        assert!(!is_routable(tags(&[("natural", "water")])));
    }

    /// Three ways meeting at a shared node: one junction of degree 3, and three edges.
    #[test]
    fn t_junction_three_ways() {
        let center = (100i64, 7_800_000i32, 47_990_000i32);
        let ways = [
            way(&[center, (1, 7_810_000, 47_990_000)]),
            way(&[center, (2, 7_790_000, 47_990_000)]),
            way(&[center, (3, 7_800_000, 48_000_000)]),
        ];
        let (g, _) = build_graph(&ways);
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.edges.len(), 3, "three arms → three edges");
        assert_eq!(g.nodes[0].coord, (7_800_000, 47_990_000), "center is first-seen (id 0)");
        let degree = g.edges.iter().filter(|e| e.a == 0 || e.b == 0).count();
        assert_eq!(degree, 3, "the shared node has degree 3");
    }

    #[test]
    fn four_way_crossing() {
        let cross = (100i64, 7_800_000i32, 47_990_000i32);
        let ways = [
            way(&[(1, 7_790_000, 47_990_000), cross, (2, 7_810_000, 47_990_000)]),
            way(&[(3, 7_800_000, 47_980_000), cross, (4, 7_800_000, 48_000_000)]),
        ];
        let (g, _) = build_graph(&ways);
        assert_eq!(g.nodes.len(), 5);
        assert_eq!(g.edges.len(), 4);
        let cross_id = g.nodes.iter().position(|n| n.coord == (7_800_000, 47_990_000)).unwrap() as u32;
        let degree = g.edges.iter().filter(|e| e.a == cross_id || e.b == cross_id).count();
        assert_eq!(degree, 4, "the crossing node has degree 4");
    }

    #[test]
    fn interior_shape_points_are_not_junctions() {
        let ways = [way(&[
            (1, 7_800_000, 47_990_000),
            (2, 7_800_000, 47_991_000),
            (3, 7_800_000, 47_992_000),
            (4, 7_800_000, 47_993_000),
        ])];
        let (g, _) = build_graph(&ways);
        assert_eq!(g.nodes.len(), 2, "only the two endpoints are junctions");
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.edges[0].polyline.len(), 4, "the edge keeps all 4 points");
    }

    #[test]
    fn identical_parallel_ways_dedup() {
        let pts = [(1i64, 7_800_000i32, 47_990_000i32), (2, 7_810_000, 47_990_000)];
        let (g, _) = build_graph(&[way(&pts), way(&pts)]);
        assert_eq!(g.edges.len(), 1, "two identical ways collapse to one edge");
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn reversed_duplicate_dedup() {
        let fwd = way(&[(1, 7_800_000, 47_990_000), (2, 7_810_000, 47_990_000)]);
        let rev = way(&[(2, 7_810_000, 47_990_000), (1, 7_800_000, 47_990_000)]);
        let (g, _) = build_graph(&[fwd, rev]);
        assert_eq!(g.edges.len(), 1, "a way and its reverse are the same undirected edge");
    }

    /// Two distinct edges between the same node pair both survive: the dedup key includes geometry.
    #[test]
    fn distinct_parallel_geometry_kept() {
        let a = way(&[(1, 7_800_000, 47_990_000), (9, 7_805_000, 47_991_000), (2, 7_810_000, 47_990_000)]);
        let b = way(&[(1, 7_800_000, 47_990_000), (8, 7_805_000, 47_989_000), (2, 7_810_000, 47_990_000)]);
        let (g, _) = build_graph(&[a, b]);
        assert_eq!(g.edges.len(), 2, "distinct geometry between the same pair → two edges");
    }

    /// A cycleway drawn over a road: identical polylines of different kinds stay two edges.
    #[test]
    fn dedup_keys_on_kind() {
        let pts = [(1i64, 7_800_000i32, 47_990_000i32), (2, 7_810_000, 47_990_000)];
        // Same nodes + geometry, different kinds.
        let road = way_kind(&pts, 7); // residential
        let cycle = way_kind(&pts, 0); // cycleway
        let (g, _) = build_graph(&[road, cycle]);
        assert_eq!(g.edges.len(), 2, "same geometry, different kinds ⇒ two edges");
        let kinds: HashSet<u8> = g.edges.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, HashSet::from([0, 7]), "both kinds survive on their own edge");
        // A same-kind duplicate still collapses.
        let (g2, _) = build_graph(&[way_kind(&pts, 7), way_kind(&pts, 7)]);
        assert_eq!(g2.edges.len(), 1, "identical geometry AND kind ⇒ one edge");
    }

    #[test]
    fn length_matches_great_circle_sum() {
        let ways = [way(&[(1, 7_800_000, 47_990_000), (2, 7_800_000, 48_000_000), (3, 7_800_000, 48_010_000)])];
        let (g, _) = build_graph(&ways);
        assert_eq!(g.edges.len(), 1);
        let leg = 0.01f64 * 111_320.0; // 1113.2 m
        let expected = (2.0 * leg).round() as u32; // 2226 m
        let got = g.edges[0].length_m;
        assert!((got as i64 - expected as i64).abs() <= 2, "length {got} ≈ {expected} within rounding");
    }

    /// A closed way makes its shared node a junction, which gives one self-loop edge.
    #[test]
    fn closed_way_is_a_loop() {
        let ways = [way(&[
            (1, 7_800_000, 47_990_000),
            (2, 7_802_000, 47_990_000),
            (3, 7_802_000, 47_992_000),
            (1, 7_800_000, 47_990_000),
        ])];
        let (g, _) = build_graph(&ways);
        assert_eq!(g.nodes.len(), 1, "only node 1 is a junction");
        assert_eq!(g.edges.len(), 1, "the loop is one edge");
        let e = &g.edges[0];
        assert_eq!(e.a, e.b, "a self-loop: both endpoints are node 1");
        assert_eq!(e.polyline.len(), 4, "the loop keeps all four points");
    }

    #[test]
    fn edges_inherit_way_kind() {
        // One way crossing a shared node → two edges, both kind 12 (primary).
        let cross = (100i64, 7_800_000i32, 47_990_000i32);
        let ways = [
            way_kind(&[(1, 7_790_000, 47_990_000), cross, (2, 7_810_000, 47_990_000)], 12),
            way_kind(&[(3, 7_800_000, 47_980_000), cross, (4, 7_800_000, 48_000_000)], 0),
        ];
        let (g, _) = build_graph(&ways);
        assert_eq!(g.edges.len(), 4);
        // Two edges of each kind (each way split at the crossing).
        assert_eq!(g.edges.iter().filter(|e| e.kind == 12).count(), 2);
        assert_eq!(g.edges.iter().filter(|e| e.kind == 0).count(), 2);
    }

    /// Chain of `edges` connected edges as separate 2-node ways, node ids offset by `id_base`, laid
    /// along a line at `origin` with 1 000-µdeg steps (short, so nothing splits).
    fn chain(id_base: i64, origin: (i32, i32), edges: usize) -> Vec<RoutableWay> {
        (0..edges)
            .map(|i| {
                let n0 = (id_base + i as i64, origin.0 + i as i32 * 1_000, origin.1);
                let n1 = (id_base + i as i64 + 1, origin.0 + (i as i32 + 1) * 1_000, origin.1);
                way(&[n0, n1])
            })
            .collect()
    }

    #[test]
    fn island_pruning_drops_small_component() {
        let mut ways = chain(0, (100_000, 100_000), 60); // giant: 60 edges, 61 nodes
                                                         // A disconnected 3-edge islet, far away, unique node ids.
        ways.extend(chain(10_000, (900_000, 900_000), 3));
        let (g, stats) = build_graph(&ways);
        assert_eq!(stats.components_found, 2, "one giant + one islet");
        assert_eq!(stats.components_kept, 1, "only the giant survives");
        assert_eq!(stats.edges_dropped, 3, "the islet's three edges are dropped");
        assert_eq!(g.nodes.len(), 61, "only the giant's nodes remain");
        assert_eq!(g.edges.len(), 60);
    }

    /// The threshold is inclusive: a component with exactly `min_component_edges` edges is kept, one
    /// edge fewer is dropped unless it is the largest.
    #[test]
    fn island_pruning_threshold_is_inclusive() {
        // Three disconnected chains with DENSE ids (the precondition build_graph's interning always
        // satisfies): a giant (9 edges), a 3-edge one, a 2-edge one.
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut mk = |count: u32, x0: i32| {
            let base = nodes.len() as u32;
            for i in 0..=count {
                nodes.push(Node { id: base + i, coord: (x0 + i as i32 * 1_000, 0) });
            }
            for i in 0..count {
                edges.push(Edge {
                    a: base + i,
                    b: base + i + 1,
                    polyline: vec![(x0 + i as i32 * 1_000, 0), (x0 + (i as i32 + 1) * 1_000, 0)],
                    length_m: 100,
                    kind: 7,
                });
            }
        };
        mk(9, 0); // giant: 10 nodes, 9 edges
        mk(3, 500_000); // 3-edge component (exactly the threshold)
        mk(2, 900_000); // 2-edge component (below)
        let (nodes, edges, stats) = prune_islands(nodes, edges, 3, None);
        assert_eq!(stats.components_found, 3);
        // Kept: giant (largest) + the 3-edge component (≥ threshold). The 2-edge one drops.
        assert_eq!(stats.components_kept, 2);
        assert_eq!(stats.edges_dropped, 2);
        assert_eq!(edges.len(), 12, "9 + 3 edges kept");
        // Surviving ids are re-densified 0..nodes.len().
        assert!(nodes.iter().enumerate().all(|(i, n)| n.id as usize == i), "node ids stay dense");
    }

    /// A fragment at a cell edge is only small until the neighbour is assembled, so a protected
    /// coordinate keeps its whole component however small.
    #[test]
    fn prune_islands_spares_protected_components() {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut mk = |count: u32, x0: i32| {
            let base = nodes.len() as u32;
            for i in 0..=count {
                nodes.push(Node { id: base + i, coord: (x0 + i as i32 * 1_000, 0) });
            }
            for i in 0..count {
                edges.push(Edge {
                    a: base + i,
                    b: base + i + 1,
                    polyline: vec![(x0 + i as i32 * 1_000, 0), (x0 + (i as i32 + 1) * 1_000, 0)],
                    length_m: 100,
                    kind: 7,
                });
            }
        };
        mk(9, 0); // the giant
        mk(1, 500_000); // a one-edge fragment reaching the "boundary" at x = 501 000
        mk(1, 900_000); // a one-edge fragment in the interior
        let protect = |c: (i32, i32)| c.0 == 501_000;
        let (kept_nodes, kept_edges, stats) = prune_islands(nodes, edges, 5, Some(&protect));
        assert_eq!(stats.components_found, 3);
        assert_eq!(stats.components_kept, 2, "giant + the protected fragment");
        assert_eq!(stats.edges_dropped, 1, "only the interior fragment goes");
        assert_eq!(kept_edges.len(), 10);
        assert!(kept_nodes.iter().any(|n| n.coord.0 == 501_000), "the protected fragment survived");
        assert!(!kept_nodes.iter().any(|n| n.coord.0 == 900_000), "the interior one did not");
    }

    fn violates(e: &Edge) -> bool {
        edge_exceeds_bounds(&e.polyline, e.length_m)
    }

    /// An edge spanning 70 000 µdeg splits into pieces inside the endpoint bound, each carrying the
    /// parent kind, concatenating to the original geometry, with costs summing to the original.
    #[test]
    fn split_long_endpoint_delta() {
        // 71 vertices, 1 000 µdeg apart in lon: endpoint delta 70 000 > 32 000.
        let poly: Vec<(i32, i32)> = (0..=70).map(|i| (100_000 + i * 1_000, 500_000)).collect();
        let (a, b) = (*poly.first().unwrap(), *poly.last().unwrap());
        let orig_len = polyline_len_m(&poly);
        let ways = [RoutableWay {
            node_ids: (0..=70).map(|i| i as i64).collect(),
            coords: poly.clone(),
            kind: 42, // an arbitrary distinctive kind byte
        }];
        // Interior vertices are touched once → the way is a single edge before split.
        let (g, _) = build_graph(&ways);
        assert!(g.edges.len() > 1, "the 70 000-µdeg edge was split, got {} pieces", g.edges.len());
        for e in &g.edges {
            assert!(!violates(e), "no piece exceeds the ±32 000 bound");
            assert_eq!(e.kind, 42, "every piece carries the parent kind");
        }
        // Endpoints preserved; pieces concatenate to the original polyline.
        assert_eq!(g.nodes[0].coord, a);
        assert_eq!(g.nodes[1].coord, b);
        let rebuilt = concat_pieces(&g, 0, 1);
        assert_eq!(rebuilt, poly, "pieces reconstruct the original geometry");
        let piece_sum: u32 = g.edges.iter().map(|e| e.length_m).sum();
        assert!(
            (piece_sum as i64 - orig_len as i64).abs() <= g.edges.len() as i64,
            "piece costs {piece_sum} sum to the original {orig_len} within per-piece rounding"
        );
    }

    /// A ~70 km hairpin whose endpoints stay within the delta bound is split until every piece is at
    /// most 60 000 m; the delta bound is untouched.
    #[test]
    fn split_long_length() {
        // Vertical zigzag: lon drifts slowly (endpoint lon delta small), lat swings
        // 0↔30 000 (≤ bound). Each 30 000-µdeg leg is ~3.3 km of ground; ~24 legs ⇒
        // ~80 km with the endpoint still inside the ±32 000 box.
        let mut poly: Vec<(i32, i32)> = Vec::new();
        for i in 0..=24i32 {
            let lat = if i % 2 == 0 { 500_000 } else { 530_000 };
            poly.push((100_000 + i * 100, lat));
        }
        let orig_len = polyline_len_m(&poly);
        assert!(orig_len > 60_000, "fixture must exceed the length bound, got {orig_len} m");
        // Endpoint delta must be inside the bound so ONLY the length bound triggers.
        let (a, b) = (*poly.first().unwrap(), *poly.last().unwrap());
        assert!((a.0 as i64 - b.0 as i64).abs() <= MAX_ENDPOINT_DELTA_UDEG);
        assert!((a.1 as i64 - b.1 as i64).abs() <= MAX_ENDPOINT_DELTA_UDEG);

        let ways = [RoutableWay { node_ids: (0..poly.len() as i64).collect(), coords: poly.clone(), kind: 2 }];
        let (g, _) = build_graph(&ways);
        assert!(g.edges.len() > 1, "the long edge was split");
        for e in &g.edges {
            assert!(e.length_m <= MAX_EDGE_LEN_M, "every piece ≤ 60 000 m");
            assert!(!violates(e));
        }
        let piece_sum: u32 = g.edges.iter().map(|e| e.length_m).sum();
        assert!((piece_sum as i64 - orig_len as i64).abs() <= g.edges.len() as i64, "costs sum to the original");
    }

    /// A 2-point edge past the bound with no interior vertex gets an interpolated midpoint, so the
    /// guarantee still holds by construction.
    #[test]
    fn split_two_point_edge_inserts_midpoint() {
        // Single 90 000-µdeg lon segment, no shape node.
        let a = (100_000, 500_000);
        let b = (190_000, 500_000);
        let ways = [RoutableWay { node_ids: vec![0, 1], coords: vec![a, b], kind: 9 }];
        let (g, _) = build_graph(&ways);
        assert!(g.edges.len() > 1, "the bare segment was split via interpolated midpoints");
        for e in &g.edges {
            assert!(!violates(e), "every piece within bounds");
            assert_eq!(e.kind, 9);
        }
        // The synthetic midpoints sit on the straight line between a and b.
        for n in &g.nodes[2..] {
            assert_eq!(n.coord.1, 500_000, "interpolated on the a–b line");
        }
        assert_eq!(concat_pieces(&g, 0, 1).first().copied(), Some(a));
        assert_eq!(concat_pieces(&g, 0, 1).last().copied(), Some(b));
    }

    /// Walk the split pieces from node `a` to node `b` — a simple chain of degree-2 synthetic nodes
    /// here — and concatenate their polylines, deduping the shared vertex at each join.
    fn concat_pieces(g: &NavGraph, a: u32, b: u32) -> Vec<(i32, i32)> {
        let mut adj: HashMap<u32, Vec<usize>> = HashMap::new();
        for (i, e) in g.edges.iter().enumerate() {
            adj.entry(e.a).or_default().push(i);
            adj.entry(e.b).or_default().push(i);
        }
        let mut out: Vec<(i32, i32)> = Vec::new();
        let mut cur = a;
        let mut prev = u32::MAX; // no previous node yet
        loop {
            let ei = *adj[&cur]
                .iter()
                .find(|&&i| {
                    let e = &g.edges[i];
                    (if e.a == cur { e.b } else { e.a }) != prev
                })
                .unwrap();
            let e = &g.edges[ei];
            let forward = e.a == cur;
            let mut seg = e.polyline.clone();
            if !forward {
                seg.reverse();
            }
            if out.is_empty() {
                out.extend(seg);
            } else {
                out.extend(&seg[1..]);
            }
            prev = cur;
            cur = if forward { e.b } else { e.a };
            if cur == b {
                break;
            }
        }
        out
    }

    /// A synthetic giant-plus-islets fixture in the shape of a real pack: the built graph reports
    /// the expected component stats and holds the split bounds on every edge.
    #[test]
    fn corpus_components_and_split_bounds() {
        let mut ways = Vec::new();
        // Giant: 60 short edges (61 nodes) + one long detour that must split.
        ways.extend(chain(0, (100_000, 100_000), 60));
        // Detour connecting giant nodes 0 and 50: endpoint lon delta 50 000 > 32 000,
        // interiors kept just above the line so a single split suffices per piece.
        let detour = {
            let mut pts = vec![(0i64, 100_000, 100_000)];
            for k in 1..=9i64 {
                pts.push((500 + k, 100_000 + k as i32 * 5_000, 105_000));
            }
            pts.push((50, 150_000, 100_000));
            way(&pts)
        };
        ways.push(detour);
        // A "medium" component with exactly the threshold's worth of edges — kept.
        ways.extend(chain(1_000, (300_000, 300_000), DEFAULT_MIN_COMPONENT_EDGES));
        // 60 disconnected islets (1 edge each) — dropped.
        for k in 0..60i64 {
            let base = 10_000 + 2 * k;
            let x = 700_000 + k as i32 * 200;
            ways.push(way(&[(base, x, 700_000), (base + 1, x + 100, 700_000)]));
        }

        let (g, stats) = build_graph(&ways);
        assert!(stats.components_found >= 60, "≥ 60 components, got {}", stats.components_found);
        assert_eq!(stats.components_kept, 2, "giant + threshold component kept");
        assert_eq!(stats.edges_dropped, 60, "the 60 islets' edges dropped");
        // The whole acceptance property: no edge exceeds either bound.
        for e in &g.edges {
            assert!(!violates(e), "edge {}→{} exceeds a v9 bound: len={}", e.a, e.b, e.length_m);
        }
        // The detour actually split (synthetic nodes were inserted past the real ones).
        assert!(g.nodes.len() > 61 + (DEFAULT_MIN_COMPONENT_EDGES + 1), "synthetic split nodes present");
    }

    #[test]
    fn summary_line_format() {
        let g = NavGraph {
            nodes: vec![Node { id: 0, coord: (0, 0) }, Node { id: 1, coord: (0, 0) }],
            edges: vec![Edge { a: 0, b: 1, polyline: vec![(0, 0), (0, 0)], length_m: 2500, kind: 7 }],
        };
        let stats = NavStats { components_found: 1, components_kept: 1, edges_dropped: 0 };
        assert_eq!(
            format_summary(&g, &stats),
            "nav graph: 2 nodes, 1 edges, 2.5 km\n\
             nav components: 1 found, 1 kept, 0 edges dropped\n\
             nav kinds: residential 1"
        );
    }
}
