//! Levels: a crossing without a shared point needs room, and the ground next to the crossing gives it. The
//! bake solves the heights twice. The first solve has no ramps; it shows what each crossing of a tunnel or a
//! bridge lacks. Then the terrain that a ground node reads is lowered or raised by a ramp from each crossing
//! that is short, along the network, and the second solve runs on that terrain. A tunnel's way sinks; at a
//! bridge the way of the softer class moves. Inside its structure a ramp may take the cap of the structure;
//! on the ground it takes the class grade of the mover and ends where it has given back what it took.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::*;
use crate::geometry::key;

/// What a short crossing gets over its need, for what the smoothing of the ground takes back.
const MARGIN_M: f64 = 0.5;
/// A crossing is short if it lacks more than this.
const SHORT_M: f64 = 0.02;
/// The most that a ramp moves a node over the need of its crossing.
const SLACK_M: f64 = 3.0;
/// A shift under this is none.
const LEAST_M: f64 = 0.05;

/// A node: its feature, its part, and its index in the part.
type Id = (u32, u32, u32);

#[derive(Default, Debug)]
pub(super) struct LevelMetrics {
    pub crossings: usize,
    /// The crossings that the first solve leaves short, by mover: a tunnel's way sinks; at a bridge the softer
    /// way moves (the bridge's way rises, or the way under it sinks).
    pub tunnel_sinks: usize,
    pub bridge_rises: usize,
    pub under_bridge_sinks: usize,
    /// Ramps that move a ground node.
    pub ramps: usize,
    /// Ramps that a junction with a stiffer way cuts, and the most that one loses there.
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

/// A crossing that is short: the nodes of the mover's segment at it, the level that the mover must reach there,
/// and the need of the crossing.
struct Ramp {
    down: bool,
    sources: [Id; 2],
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

    /// The slope that a structure may take: twice the slope of its class.
    fn cap(&self, f: u32) -> f64 {
        2.0 * self.slope(f)
    }

    /// The ground nodes that a ramp moves, each with its metres (positive), and the most that a junction with
    /// a stiffer way takes from one of them. `z` is the height of a node in the first solve.
    fn ramp(&self, z: &dyn Fn(Id) -> f64, ramp: &Ramp) -> (Vec<(Id, f32)>, f64) {
        let mover = ramp.sources[0].0;
        let (rank, grade) = (self.rank(mover), self.slope(mover));
        let free = |id: Id| self.features[id.0 as usize].free;
        let stiff = |id: Id| self.stiffest(id) < rank - 1e-9;
        // How far a node lies past the level: positive where the ramp has work.
        let past = |id: Id, level: f64| if ramp.down { z(id) - level } else { level - z(id) };
        let most = ramp.need + MARGIN_M + SLACK_M;
        // Per node: the metres that the level has given back on the way to it, whether the way has left the
        // structure of the crossing, and the metres that the node moves.
        let mut label: HashMap<Id, (f64, bool, f64)> = HashMap::new();
        let mut heap: BinaryHeap<Reverse<(u64, Id)>> = BinaryHeap::new();
        for &source in &ramp.sources {
            let amount = past(source, ramp.level);
            if amount.is_finite() && !stiff(source) {
                label.insert(source, (0.0, !free(source), amount.min(most)));
                heap.push(Reverse((0f64.to_bits(), source)));
            }
        }
        let mut reached: Vec<(Id, f64)> = Vec::new();
        while let Some(Reverse((bits, id))) = heap.pop() {
            let (given, out, amount) = label[&id];
            if f64::from_bits(bits) > given || amount <= LEAST_M {
                continue;
            }
            reached.push((id, amount));
            for (next, metres) in self.neighbours(id) {
                if stiff(next) || !z(next).is_finite() {
                    continue;
                }
                let next_out = out || !free(next);
                let rate = if next_out { grade } else { self.cap(next.0) };
                let next_given = given + rate * metres;
                // On the ground the ramp also ends where it has given back what it took at the node before.
                let tent = if out { amount - grade * metres } else { f64::INFINITY };
                let next_amount = (past(next, ramp.level) - next_given).min(tent).min(most);
                if label.get(&next).is_none_or(|old| next_given < old.0) {
                    label.insert(next, (next_given, next_out, next_amount));
                    heap.push(Reverse((next_given.to_bits(), next)));
                }
            }
        }
        // The ramp comes back to the terrain at a junction with a stiffer way, at its grade.
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
        let (mut out, mut lost) = (Vec::new(), 0.0f64);
        for &(id, amount) in &reached {
            if free(id) {
                continue;
            }
            let room = from_anchor.get(&id).map_or(f64::INFINITY, |&d| grade * d);
            lost = lost.max(amount - room);
            let amount = amount.min(room);
            if amount > LEAST_M {
                out.push((id, amount as f32));
            }
        }
        (out, lost)
    }
}

impl Levels {
    pub fn none() -> Levels {
        Levels { shift: HashMap::new(), metrics: LevelMetrics::default() }
    }

    /// The ramps of the crossings that the heights of the first solve leave short.
    pub fn new(
        features: &[&Feature],
        classes: &Classes,
        segments: &Segments,
        layout: &Layout,
        facts: &AreaFacts,
        first: &Heights,
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
        let z = |id: Id| f64::from(first.z[id.0 as usize][id.1 as usize][id.2 as usize]);
        let at = |m: Node| z(id(m));
        let along = |m: Node, share: f32| at(m) + f64::from(share) * (at(layout.next(m)) - at(m));
        let pair = |m: Node| [id(m), id(layout.next(m))];
        let mut metrics = LevelMetrics::default();
        let mut ramps = Vec::new();
        for &(tunnel, road, share, need) in &facts.unders {
            metrics.crossings += 1;
            let need = f64::from(need);
            let (floor, top) = (at(tunnel).max(at(layout.next(tunnel))), along(road, share));
            if need - (top - floor) > SHORT_M {
                // A tunnel is dug: its way sinks, whatever the classes of the two ways.
                ramps.push(Ramp { down: true, sources: pair(tunnel), level: top - need - MARGIN_M, need });
                metrics.tunnel_sinks += 1;
            }
        }
        for &(bridge, under, share, need) in &facts.over_ways {
            metrics.crossings += 1;
            let need = f64::from(need);
            let (deck, top) = (at(bridge).min(at(layout.next(bridge))), along(under, share));
            if need - (deck - top) > SHORT_M {
                if graph.rank(bridge.0) >= graph.rank(under.0) {
                    ramps.push(Ramp { down: false, sources: pair(bridge), level: top + need + MARGIN_M, need });
                    metrics.bridge_rises += 1;
                } else {
                    ramps.push(Ramp { down: true, sources: pair(under), level: deck - need - MARGIN_M, need });
                    metrics.under_bridge_sinks += 1;
                }
            }
        }
        let moved: Vec<(Vec<(Id, f32)>, f64)> = ramps.par_iter().map(|ramp| graph.ramp(&z, ramp)).collect();
        let (mut down, mut up): (HashMap<Id, f32>, HashMap<Id, f32>) = Default::default();
        for (ramp, (nodes, lost)) in ramps.iter().zip(moved) {
            metrics.ramps += usize::from(!nodes.is_empty());
            if lost > 0.1 {
                metrics.anchored += 1;
                metrics.anchored_worst = metrics.anchored_worst.max(lost);
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
        if let Ok(place) = std::env::var("LEVELS_AT") {
            // "lat,lon": the shifts within 60 m of a place, for a check by hand.
            let want: Vec<f64> = place.split(',').filter_map(|v| v.parse().ok()).collect();
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
