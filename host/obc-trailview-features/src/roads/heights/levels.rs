//! Levels: a crossing without a shared point needs room, and the ground next to the crossing gives it. The
//! terrain that a ground node reads is lowered or raised near such a crossing by a ramp at the class grade,
//! along the network from the crossing. A tunnel's way sinks; at a bridge the way of the softer class moves.
//! The pass reads the terrain and the facts only: no solved height.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::*;
use crate::geometry::key;

/// What a crossing gets over its need, for what the smoothing of the ground takes back.
const MARGIN_M: f64 = 0.5;
/// The most that the terrain at the crossing may add to the need of a ramp.
const SLACK_M: f64 = 3.0;
/// A shift under this is none.
const LEAST_M: f64 = 0.05;
/// How far a deck's landing is searched along the deck.
const LANDING_M: f64 = 2000.0;

/// A node: its feature, its part, and its index in the part.
type Id = (u32, u32, u32);

#[derive(Default, Debug)]
pub(super) struct LevelMetrics {
    pub crossings: usize,
    /// Ramps by mover: a tunnel's way sinks; at a bridge the softer way moves (the bridge's way rises, or
    /// the way under it sinks).
    pub tunnel_sinks: usize,
    pub bridge_rises: usize,
    pub under_bridge_sinks: usize,
    /// Of the tunnels that sink: those whose way is of a stiffer class than the way over it.
    pub stiffer_tunnels: usize,
    /// Ramps that move a ground node.
    pub ramps: usize,
    /// Ramps that a junction with a stiffer way cuts short at the crossing, and the most that one misses.
    pub anchored: usize,
    pub anchored_worst: f64,
    pub nodes: usize,
    pub nodes_over_2m: usize,
    /// Nodes that one ramp lowers and another raises.
    pub both: usize,
    pub most_down: f64,
    pub most_up: f64,
}

pub(super) struct Levels {
    /// Per ground node the metres that its terrain moves: up positive.
    shift: HashMap<Id, f32>,
    pub metrics: LevelMetrics,
}

struct Ramp {
    down: bool,
    source: Id,
    level: f64,
    need: f64,
}

struct Graph<'a> {
    features: &'a [&'a Feature],
    classes: &'a Classes,
    segments: &'a Segments<'a>,
    layout: &'a Layout,
    /// Each OSM vertex that two or more parts share: its point key and its node, sorted.
    joins: Vec<([u64; 2], Id)>,
}

impl Graph<'_> {
    fn vertices(&self, f: u32, p: u32) -> &[u32] {
        let list = &self.layout.first[f as usize][p as usize];
        &list[..list.len().saturating_sub(1)]
    }

    /// The OSM vertex at or before a node, and the node's number after it.
    fn locate(&self, (f, p, i): Id) -> (usize, u32) {
        let vertices = self.vertices(f, p);
        let k = vertices.partition_point(|&first| first <= i) - 1;
        (k, i - vertices[k])
    }

    fn xy(&self, id: Id) -> [f64; 2] {
        let (k, c) = self.locate(id);
        let vertices = self.vertices(id.0, id.1);
        let points = &self.segments.parts[self.segments.index[&(id.0, id.1)]].2;
        if c == 0 {
            return points[k];
        }
        let t = f64::from(c) / f64::from(vertices[k + 1] - vertices[k]);
        let (a, b) = (points[k], points[k + 1]);
        [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
    }

    /// The nodes of other parts at the point of a node.
    fn joined(&self, id: Id) -> &[([u64; 2], Id)] {
        if self.locate(id).1 != 0 {
            return &[];
        }
        let point = key(self.xy(id));
        let from = self.joins.partition_point(|j| j.0 < point);
        let to = self.joins.partition_point(|j| j.0 <= point);
        &self.joins[from..to]
    }

    fn neighbours(&self, id: Id) -> Vec<(Id, f64)> {
        let (f, p, i) = id;
        let count = self.vertices(f, p).last().map_or(0, |&last| last + 1);
        let here = self.xy(id);
        let mut out = Vec::with_capacity(4);
        if i > 0 {
            out.push(((f, p, i - 1), span(self.xy((f, p, i - 1)), here)));
        }
        if i + 1 < count {
            out.push(((f, p, i + 1), span(here, self.xy((f, p, i + 1)))));
        }
        out.extend(self.joined(id).iter().filter(|j| j.1 != id).map(|j| (j.1, 0.0)));
        out
    }

    fn grade(&self, f: u32) -> f64 {
        self.classes.rows[self.features[f as usize].rule].grade
    }

    /// How soft a way is: the softer of two ways moves for the other. A class without a grade is the softest.
    fn rank(&self, f: u32) -> f64 {
        let grade = self.grade(f);
        if grade.is_finite() {
            grade
        } else {
            1.0
        }
    }

    fn slope(&self, f: u32) -> f64 {
        let grade = self.grade(f);
        if grade.is_finite() {
            grade
        } else {
            RAMP_GRADE
        }
    }

    /// The rank of the stiffest way at the point of a node.
    fn stiffest(&self, id: Id) -> f64 {
        self.joined(id).iter().map(|j| self.rank(j.1 .0)).fold(self.rank(id.0), f64::min)
    }

    /// Dijkstra from the sources (each with its first metres) over the nodes that `open` lets in, out to
    /// `reach` metres. `stop` ends the search at the first node that it names.
    fn spread(&self, sources: &[(Id, f64)], reach: f64, open: &dyn Fn(Id) -> bool, stop: &dyn Fn(Id) -> bool) -> Vec<(Id, f64)> {
        let mut best: HashMap<Id, f64> = HashMap::new();
        let mut heap: BinaryHeap<Reverse<(u64, Id)>> = BinaryHeap::new();
        for &(id, d) in sources {
            if best.get(&id).map_or(true, |&old| d < old) {
                best.insert(id, d);
                heap.push(Reverse((d.to_bits(), id)));
            }
        }
        let mut out = Vec::new();
        while let Some(Reverse((bits, id))) = heap.pop() {
            let d = f64::from_bits(bits);
            if d > best[&id] || !open(id) {
                continue;
            }
            out.push((id, d));
            if stop(id) {
                break;
            }
            for (next, metres) in self.neighbours(id) {
                let nd = d + metres;
                if nd <= reach && best.get(&next).map_or(true, |&old| nd < old) {
                    best.insert(next, nd);
                    heap.push(Reverse((nd.to_bits(), next)));
                }
            }
        }
        out
    }

    /// The terrain at the nearest landing of the deck of a node: the first ground node along the deck.
    fn landing(&self, terrain: &dyn Terrain, source: Id) -> Option<f64> {
        let ground = |id: Id| !self.features[id.0 as usize].free;
        let reached = self.spread(&[(source, 0.0)], LANDING_M, &|_| true, &ground);
        let &(last, _) = reached.last()?;
        ground(last).then(|| terrain.height(self.xy(last))).filter(|h| h.is_finite())
    }

    /// The ground nodes that a ramp moves, each with its metres (positive), and what the ramp misses at its
    /// crossing because of a junction with a stiffer way.
    fn ramp(&self, terrain: &dyn Terrain, ramp: &Ramp) -> (Vec<(Id, f32)>, f64) {
        let mover = ramp.source.0;
        let (rank, slope) = (self.rank(mover), self.slope(mover));
        let at_source = terrain.height(self.xy(ramp.source));
        let depth = if ramp.down { at_source - ramp.level } else { ramp.level - at_source };
        if !(depth > LEAST_M) {
            return (Vec::new(), 0.0);
        }
        let depth = depth.min(ramp.need + MARGIN_M + SLACK_M);
        let reach = depth / slope;
        // The ramp ends at a junction with a stiffer way: that point and what lies past it do not move, and
        // the ramp comes back to the terrain there at its slope.
        let stiff = |id: Id| self.stiffest(id) < rank - 1e-9;
        let reached = self.spread(&[(ramp.source, 0.0)], reach, &|id| !stiff(id), &|_| false);
        let inside: HashSet<Id> = reached.iter().map(|r| r.0).collect();
        let mut beside: Vec<(Id, f64)> = Vec::new();
        for &(id, _) in &reached {
            for (next, metres) in self.neighbours(id) {
                if stiff(next) {
                    beside.push((id, metres));
                }
            }
        }
        let from_anchor: HashMap<Id, f64> =
            self.spread(&beside, f64::INFINITY, &|id| inside.contains(&id), &|_| false).into_iter().collect();
        let room = |id: Id| from_anchor.get(&id).map_or(f64::INFINITY, |&d| slope * d);
        let ground: Vec<(Id, f64)> =
            reached.iter().copied().filter(|&(id, _)| !self.features[id.0 as usize].free).collect();
        let points: Vec<[f64; 2]> = ground.iter().map(|g| self.xy(g.0)).collect();
        let heights = terrain.heights(&points);
        let mut out = Vec::new();
        for (&(id, d), &h) in ground.iter().zip(&heights) {
            if !h.is_finite() {
                continue;
            }
            let tent = depth - slope * d;
            // No further than the level with its ramp asks, and no further than the tent under the terrain.
            let amount = if ramp.down {
                h - h.min((ramp.level + slope * d).max(h - tent))
            } else {
                h.max((ramp.level - slope * d).min(h + tent)) - h
            };
            let amount = amount.min(room(id));
            if amount > LEAST_M {
                out.push((id, amount as f32));
            }
        }
        let missed = if inside.contains(&ramp.source) { (depth - room(ramp.source)).max(0.0) } else { depth };
        (out, missed)
    }
}

impl Levels {
    pub fn none() -> Levels {
        Levels { shift: HashMap::new(), metrics: LevelMetrics::default() }
    }

    pub fn new(
        features: &[&Feature],
        classes: &Classes,
        terrain: &dyn Terrain,
        segments: &Segments,
        layout: &Layout,
        facts: &AreaFacts,
    ) -> Levels {
        let mut joins: Vec<([u64; 2], Id)> = Vec::new();
        for (f, p, points, _) in &segments.parts {
            let vertices = &layout.first[*f][*p];
            joins.extend(points.iter().enumerate().map(|(k, &point)| (key(point), (*f as u32, *p as u32, vertices[k]))));
        }
        joins.sort_unstable();
        let mut shared = Vec::new();
        for group in joins.chunk_by(|a, b| a.0 == b.0).filter(|g| g.len() > 1) {
            shared.extend_from_slice(group);
        }
        let graph = Graph { features, classes, segments, layout, joins: shared };
        let id = |m: Node| (m.0, m.1, layout.index(m) as u32);
        let along = |m: Node, share: f32| {
            let (a, b) = (point_of(features, layout, m), point_of(features, layout, layout.next(m)));
            let t = f64::from(share);
            [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
        };
        let mut metrics = LevelMetrics::default();
        let mut ramps = Vec::new();
        for &(tunnel, road, share, need) in &facts.unders {
            metrics.crossings += 1;
            let need = f64::from(need);
            // A tunnel is dug: its way sinks, whatever the classes of the two ways.
            let level = terrain.height(along(road, share)) - need - MARGIN_M;
            ramps.push(Ramp { down: true, source: id(tunnel), level, need });
            metrics.tunnel_sinks += 1;
            metrics.stiffer_tunnels += usize::from(graph.rank(tunnel.0) < graph.rank(road.0));
        }
        for &(bridge, under, share, need) in &facts.over_ways {
            metrics.crossings += 1;
            let need = f64::from(need);
            if graph.rank(bridge.0) >= graph.rank(under.0) {
                let level = terrain.height(along(under, share)) + need + MARGIN_M;
                ramps.push(Ramp { down: false, source: id(bridge), level, need });
                metrics.bridge_rises += 1;
            } else if let Some(landing) = graph.landing(terrain, id(bridge)) {
                ramps.push(Ramp { down: true, source: id(under), level: landing - need - MARGIN_M, need });
                metrics.under_bridge_sinks += 1;
            }
        }
        let moved: Vec<(Vec<(Id, f32)>, f64)> =
            ramps.par_iter().map(|ramp| graph.ramp(terrain, ramp)).collect();
        let (mut down, mut up): (HashMap<Id, f32>, HashMap<Id, f32>) = Default::default();
        for (ramp, (nodes, missed)) in ramps.iter().zip(moved) {
            metrics.ramps += usize::from(!nodes.is_empty());
            if missed > 0.1 {
                metrics.anchored += 1;
                metrics.anchored_worst = metrics.anchored_worst.max(missed);
            }
            let into = if ramp.down { &mut down } else { &mut up };
            for (node, amount) in nodes {
                let slot = into.entry(node).or_insert(0.0);
                *slot = slot.max(amount);
            }
        }
        let mut shift: HashMap<Id, f32> = HashMap::new();
        for (&node, &amount) in &up {
            shift.insert(node, amount);
            metrics.most_up = metrics.most_up.max(f64::from(amount));
        }
        for (&node, &amount) in &down {
            metrics.both += usize::from(up.contains_key(&node));
            *shift.entry(node).or_insert(0.0) -= amount;
            metrics.most_down = metrics.most_down.max(f64::from(amount));
        }
        metrics.nodes = shift.len();
        metrics.nodes_over_2m = shift.values().filter(|s| s.abs() > 2.0).count();
        if let Ok(at) = std::env::var("LEVELS_AT") {
            // "lat,lon": the shifts within 60 m of a place, for a check by hand.
            let want: Vec<f64> = at.split(',').filter_map(|v| v.parse().ok()).collect();
            let want = if want.len() == 2 { want } else { vec![f64::NAN, f64::NAN] };
            let mut near: Vec<(f64, [f64; 2], f32, Id)> = shift
                .iter()
                .map(|(&node, &s)| (degrees(graph.xy(node)), s, node))
                .map(|(ll, s, node)| (((ll[0] - want[0]) * 111_195.0).hypot((ll[1] - want[1]) * 76_000.0), ll, s, node))
                .filter(|n| n.0 < 60.0)
                .collect();
            near.sort_by(|a, b| a.3.cmp(&b.3));
            for (d, ll, s, node) in near {
                eprintln!("LEVELS_AT {:.6},{:.6} {d:5.1} m away: {s:+.2} m, node {node:?}", ll[0], ll[1]);
            }
        }
        Levels { shift, metrics }
    }

    /// The terrain of the nodes of a chain with its shifts.
    pub fn apply(&self, layout: &Layout, names: &[Vec<Node>], mut h: Vec<f64>) -> Vec<f64> {
        if self.shift.is_empty() {
            return h;
        }
        for (i, names) in names.iter().enumerate() {
            let shift = names
                .iter()
                .filter_map(|&m| self.shift.get(&(m.0, m.1, layout.index(m) as u32)))
                .fold(0.0f32, |a, &b| if b.abs() > a.abs() { b } else { a });
            h[i] += f64::from(shift);
        }
        h
    }
}
