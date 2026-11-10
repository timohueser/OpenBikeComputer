//! The heights of the ground, phase B of the road solver: each ground chain is solved once per bake, after
//! every chain that it reads, into one store of node heights.

use std::collections::{BTreeMap, HashMap, HashSet};

use rayon::prelude::*;

use super::chain::{self, Step};
use super::chains::{AreaFacts, Arm, Chains, Fact, Node, Place, ReadKind, Segments};
use super::network::{distinct, dot, hypot, pieces, sub};
use super::{Classes, Feature, APPROACH_M, APPROACH_PATH_M, HUBER_M, RAMP_GRADE};
use crate::{degrees, metres, Terrain};

/// A way on the ground lies within BAND_M of the terrain at each node: half the census gate.
pub(crate) const BAND_M: f64 = crate::gate::GATE_M / 2.0;
/// A grade that changes by more than this between two segments is a kink.
const KINK: f64 = 0.05;
/// A node this far off the terrain counts in the metrics by its cause.
const OFF_M: f64 = 3.0;
/// A ground way rises or sinks at most this much per metre in the zone of a join on a structure: 1:1.
const HANG_GRADE: f64 = 1.0;
/// A ground way that hangs on a structure leaves its band within this many metres of the join, at 1:1.
const HANG_M: f64 = super::JOINT_M;

/// The metrics of the ground heights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroundMetrics {
    pub chains: usize,
    pub plain: usize,
    pub steps: usize,
    pub steps_max: usize,
    pub unconverged: usize,
    pub pivots: usize,
    /// Cuts by layer (0 fixed, 1 band, 2 pass, 3 cover, 4 grade): count and metres, and the largest cuts:
    /// metres, layer, latitude, longitude. Openings by layer, and the largest excess of an opened slope per
    /// metre.
    pub cuts: [usize; 5],
    pub cut_m: [f64; 5],
    pub cut_worst: Vec<(f64, u8, [f64; 2])>,
    pub openings: [usize; 5],
    pub excess_max: f64,
    /// Opened metres by cause: band (no fixed node at the ends of the opened run), read, cycle.
    pub opened_m: [f64; 3],
    /// Nodes over OFF_M off the terrain: band with a held grade, fixed by a read, pass, cover.
    pub off: [usize; 4],
    /// Of those, the nodes outside the band (over BAND_M off).
    pub off_band: [usize; 4],
    /// The change of grade at fixed nodes, frozen nodes and string contacts: count over KINK, p99, max.
    pub kinks: [usize; 3],
    pub kink_p99: [f64; 3],
    pub kink_max: [f64; 3],
    /// The worst kinks at frozen nodes: change, latitude, longitude.
    pub frozen_worst: Vec<(f64, [f64; 2])>,
    /// Chains in a cycle and in none: km of chain, metres steeper than the class grade + 1 %, metres
    /// where the grade changes by more than KINK between two segments.
    pub split_km: [f64; 2],
    pub split_grade_m: [f64; 2],
    pub split_kink_m: [f64; 2],
    pub seconds: f64,
    /// Arms that two facts give different partners: 0 when the facts are local.
    pub conflicts: usize,
    pub largest_cycle_chains: usize,
    pub largest_cycle_seconds: f64,
    /// Approaches that end at a node outside its band, and the worst: metres past the band, place.
    pub approach_ended: usize,
    pub approach_ended_worst: Vec<(f64, [f64; 2])>,
    /// Ground passes that hold, and those dropped because their bound leaves the band: by need in
    /// decimetres, and the worst.
    pub passes_held: usize,
    pub passes_dropped: usize,
    pub passes_dropped_by_need: BTreeMap<u32, usize>,
    pub passes_dropped_worst: Vec<Dropped>,
    /// Ground passes left out: their reason (0: the other way's crossing segment is the reader itself or
    /// ends on it there; 1: the other way has no height) and place. Held, dropped and left out are all the
    /// ground passes.
    pub passes_left_out: usize,
    pub passes_left_out_worst: Vec<(u8, [f64; 2])>,
    /// The ground passes of the bake.
    pub passes: usize,
    /// Reads, the reads whose node holds another height at the end (0: a read height never changes), the
    /// largest such change, and the reads left out because their node was frozen.
    pub reads: usize,
    pub read_changed: usize,
    pub read_changed_max: f64,
    pub reads_skipped: usize,
    /// Points that several parts share, with two heights in the store: 0.
    pub shared_apart: usize,
    /// The worst reads that changed and shared points apart: metres, place.
    pub read_changed_worst: Vec<(f64, [f64; 2])>,
    pub shared_apart_worst: Vec<(f64, [f64; 2])>,
    /// Segments whose class grade opened to over twice the grade: count, metres, and the worst.
    pub grade_broken: usize,
    pub grade_broken_m: f64,
    pub grade_broken_worst: Vec<Broken>,
    /// Structure chains: count, by kind (bridge, tunnel, mixed) and by the ends that a read fixes (0, 1, 2).
    pub structure_chains: usize,
    pub structure_kinds: [usize; 3],
    pub structure_ends: [usize; 3],
    /// Chains steeper than their cap between two fixed nodes: count, metres, the steepest slope, the worst
    /// places. Segments steeper than their cap elsewhere: none, as the string holds the cap where any curve
    /// does.
    pub structure_opened: usize,
    pub structure_opened_m: f64,
    pub structure_opened_worst: Vec<(f64, [f64; 2])>,
    pub structure_over_cap: usize,
    pub structure_over_cap_worst: Vec<(f64, [f64; 2])>,
    /// Decks short of their least height by kind (over a way, over water, over a structure), and every one.
    pub deck_short: [usize; 3],
    pub deck_short_all: Vec<Short>,
    /// Tunnels less than their need under a road, every one; and tunnel nodes over the terrain: their count,
    /// those that a cap of 1:1 would have held, and the worst.
    pub bore_exposed: usize,
    pub bore_exposed_all: Vec<Short>,
    pub tunnel_over: usize,
    pub tunnel_over_at_1_1: usize,
    pub tunnel_over_worst: Vec<(f64, [f64; 2])>,
    /// Structure chains whose string is not the line between their fixed ends, and the change of grade at
    /// each corner of a string.
    pub structure_on_string: usize,
    pub corners: Vec<(f64, [f64; 2])>,
    pub points_apart: Vec<Apart>,
    /// The held bounds that the stored heights miss by more than 1 mm: kind (0 a ground pass, 1 a cover, 2
    /// a deck over a way, 3 over water, 4 over a structure, 5 a tunnel under a road, 6 a tunnel under the
    /// terrain), place, metres.
    pub held_unmet: usize,
    pub held_unmet_all: Vec<(u8, [f64; 2], f64)>,
    /// The ground gate at the nodes: those more than GATE_M off the terrain, outside the stretches where the
    /// band is cut and the 10 m of a held join, with their places; and those inside the cut stretches.
    pub ground_gate: usize,
    pub ground_gate_all: Vec<(f64, [f64; 2])>,
    pub ground_gate_cut: usize,
    /// The tunnel gate at the nodes: those more than GATE_M over the terrain; those that a 1:1 line down from
    /// an origin of their chain cannot bring under the terrain (skipped), with the 10 largest; the origins
    /// that skip a node (kind: 0 a fixed end, 1 a point shared with another structure, 2 a bridge that runs
    /// into a tunnel; its place; the largest height over the terrain that it skips); and the nodes that
    /// fail, every one.
    pub tunnel_gate_over: usize,
    pub tunnel_gate_skipped: usize,
    pub tunnel_gate_skipped_worst: Vec<(f64, [f64; 2])>,
    pub tunnel_gate_origins: Vec<(u8, [f64; 2], f64)>,
    pub tunnel_gate_fail: usize,
    pub tunnel_gate_fail_all: Vec<(f64, [f64; 2])>,
    /// Ground ways that hang on a structure: joins held (the worst by metres off the terrain) and the zone
    /// nodes outside the plain band; joins dropped, out of reach, with metres off the terrain, class, place.
    pub joins_held: usize,
    pub joins_held_worst: Vec<Join>,
    pub joins_zone_off: usize,
    pub joins_dropped: usize,
    pub joins_dropped_all: Vec<Join>,
}

/// A join of a ground way on a structure: metres between the structure there and the terrain, the class of
/// the way, the place.
pub type Join = (f64, usize, [f64; 2]);

/// A crossing of a structure that misses its bound: its kind (0 over a way, 1 over water, 2 over a
/// structure, 3 a tunnel under a road), the metres that it misses by, its place, the metres along the
/// structure to its nearer fixed node, the classes of the structure and of the other way, and whether 1:1
/// would have held it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Short {
    pub kind: u8,
    pub metres: f64,
    pub place: [f64; 2],
    pub to_end: f64,
    pub rules: [usize; 2],
    /// A cap of 1:1 in place of the class cap would have held it.
    pub at_1_1: bool,
}

/// A node where a structure chain reads the owner of its point while its own string lies more than 5 m off:
/// the place, the reader's own height there and the owner's, and of each chain its first feature, its class
/// and its fixed ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Apart {
    pub place: [f64; 2],
    pub own: f64,
    pub owner: f64,
    pub reader: (usize, usize, u8),
    pub by: (usize, usize, u8),
}

/// A segment whose class grade opened to over twice the grade: its opened slope bound, its class, its
/// place and its metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Broken {
    pub slope: f64,
    pub rule: usize,
    pub place: [f64; 2],
    pub metres: f64,
}

/// A ground pass that was dropped: metres by which its bound leaves the band, its clearance, the place of
/// the reader's crossing segment, and the classes of the reader and of the other way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dropped {
    pub miss: f64,
    pub need: f64,
    pub place: [f64; 2],
    pub rules: [usize; 2],
}

/// The heights of the nodes of the ways of a bake.
pub(crate) struct Heights {
    /// Per feature, per part: its node heights in the order of the way; NaN = none.
    z: Vec<Vec<Vec<f32>>>,
    layout: Layout,
    /// The nodes that a bound holds (the alignment keeps them), and the covers that hold.
    held: HashSet<Node>,
    /// Per cover that holds: its lift over the plain solve of its chain.
    covered: HashMap<Node, f64>,
    pub metrics: GroundMetrics,
    /// Per solved piece of a chain: its nodes, its tube and its heights.
    #[cfg(test)]
    pub pieces: Vec<Piece>,
}

#[cfg(test)]
use super::chain::Tube;

#[cfg(test)]
pub(crate) struct Piece {
    pub h: Vec<f64>,
    pub l: Vec<f64>,
    pub tube: Tube,
    pub z: Vec<f64>,
    /// The piece is a structure chain, and its profile is the plain solve.
    pub structure: bool,
    pub plain: bool,
}

/// The point of a node.
fn point_of(features: &[&Feature], layout: &Layout, (f, p, k, c): Node) -> [f64; 2] {
    let points = distinct(&features[f as usize].parts[p as usize]);
    let a = points[k as usize];
    if c == 0 {
        return a;
    }
    let (b, t) = (points[k as usize + 1], f64::from(c) / layout.pieces(f as usize, p as usize, k as usize) as f64);
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}

/// Per feature, per part: the index of the first node of each OSM vertex, and one past the last node.
struct Layout {
    first: Vec<Vec<Vec<u32>>>,
}

impl Layout {
    fn new(features: &[&Feature], segments: &Segments) -> Self {
        let mut first: Vec<Vec<Vec<u32>>> = features.iter().map(|f| vec![Vec::new(); f.parts.len()]).collect();
        for (f, p, points, _) in &segments.parts {
            let alone = features[*f].free && points.len() == 2;
            let mut at = 0u32;
            let mut list = Vec::with_capacity(points.len() + 1);
            for pair in points.windows(2) {
                list.push(at);
                at += pieces(span(pair[0], pair[1]), alone) as u32;
            }
            list.push(at);
            list.push(at + 1);
            first[*f][*p] = list;
        }
        Layout { first }
    }

    fn index(&self, (f, p, k, c): Node) -> usize {
        self.first[f as usize][p as usize][k as usize] as usize + c as usize
    }

    /// The pieces of OSM segment k of a part.
    fn pieces(&self, f: usize, p: usize, k: usize) -> usize {
        (self.first[f][p][k + 1] - self.first[f][p][k]) as usize
    }

    /// The node after `node` along its way.
    fn next(&self, (f, p, k, c): Node) -> Node {
        if (c as usize) + 1 < self.pieces(f as usize, p as usize, k as usize) {
            (f, p, k, c + 1)
        } else {
            (f, p, k + 1, 0)
        }
    }
}

/// The metres between two points.
fn span(a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = sub(b, a);
    hypot(d) * metres(a[1] + d[1] / 2.0)
}

/// The nodes of a chain in its order: per node its names (two where two ways of the chain meet) and its
/// point; per segment its feature and the node that starts it in the order of its way.
struct Nodes {
    names: Vec<Vec<Node>>,
    xy: Vec<[f64; 2]>,
    feature: Vec<usize>,
    first: Vec<Node>,
}

impl Nodes {
    fn new(arms: &[Arm], segments: &Segments, layout: &Layout) -> Self {
        let mut out = Nodes { names: Vec::new(), xy: Vec::new(), feature: Vec::new(), first: Vec::new() };
        for &((f, p, k), step) in arms {
            let (_, _, points, _) = &segments.parts[segments.index[&(f, p)]];
            let (k, s) = (k as usize, if step > 0 { k as usize } else { k as usize - 1 });
            let n = layout.pieces(f as usize, p as usize, s);
            let at = |c: usize| {
                let (a, b) = (points[s], points[s + 1]);
                let t = c as f64 / n as f64;
                [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
            };
            let node = |c: usize| -> Node {
                if c == n {
                    (f, p, s as u32 + 1, 0)
                } else {
                    (f, p, s as u32, c as u16)
                }
            };
            let order: Vec<usize> = if step > 0 { (0..=n).collect() } else { (0..=n).rev().collect() };
            let start = node(order[0]);
            match out.names.last_mut() {
                Some(last) if !last.contains(&start) => last.push(start),
                Some(_) => {}
                None => {
                    out.names.push(vec![start]);
                    out.xy.push(at(order[0]));
                }
            }
            debug_assert!(k == s || k == s + 1);
            for j in 1..order.len() {
                out.names.push(vec![node(order[j])]);
                out.xy.push(at(order[j]));
                out.feature.push(f as usize);
                out.first.push(node(order[j].min(order[j - 1])));
            }
        }
        out
    }
}

/// What a solve of a chain needs from the rest of the bake.
struct Context<'a> {
    features: &'a [&'a Feature],
    classes: &'a Classes,
    terrain: &'a dyn Terrain,
    chains: &'a Chains,
    segments: &'a Segments<'a>,
    layout: &'a Layout,
    /// Per segment its chain and its arm number in the chain.
    place_of: Vec<(u32, u32)>,
    /// Per chain its side reads: the arm of the side at its junction, the through arm.
    side: Vec<Vec<(Arm, Arm)>>,
    approach: Vec<Vec<(Node, Arm)>>,
    copies: Vec<Vec<(Node, Node, f32)>>,
    passes: Vec<Vec<usize>>,
    covers: Vec<Vec<usize>>,
    /// Per chain the points where it reads the owner of a point that a structure has: its node, the owner's
    /// node, the kind; per structure chain its loose ends, and the indices of its facts in `facts`: its decks
    /// over ways and over water, its tunnels under roads, and the structures under it.
    points: Vec<Vec<(Node, Node, ReadKind)>>,
    loose: Vec<Vec<Node>>,
    over_ways: Vec<Vec<usize>>,
    over_water: Vec<Vec<usize>>,
    unders: Vec<Vec<usize>>,
    stacks: Vec<Vec<usize>>,
    facts: &'a AreaFacts,
    leaders: HashSet<(usize, usize)>,
    ramps: HashSet<Node>,
    ungraded: HashSet<Node>,
    /// The node of a side at its junction, and the through chain's node there, whose height it takes.
    roots: HashMap<Node, Node>,
    /// Per chain: it reads itself.
    reads_itself: Vec<bool>,
    /// The nodes at points that two structure chains share.
    shared: HashSet<Node>,
}

impl Context<'_> {
    /// The node whose height a node takes: past the junctions of sides, the through chain's node. A read of
    /// a side's junction reads the through chain there, so that the junction is never read itself and its
    /// side read always holds.
    fn root(&self, mut node: Node) -> Node {
        for _ in 0..self.roots.len() {
            match self.roots.get(&node) {
                Some(&next) if next != node => node = next,
                _ => break,
            }
        }
        node
    }

    /// The chain of the segment that an arm points into.
    fn chain_of_arm(&self, arm: Arm) -> usize {
        self.place_of[self.segments.end(arm).0].0 as usize
    }

    /// The chains whose segments a node lies on.
    fn chains_of(&self, (f, p, k, c): Node) -> Vec<usize> {
        let mut out = Vec::new();
        let last = self.layout.first[f as usize][p as usize].len() - 2;
        if (k as usize) < last {
            out.push(self.chain_of_arm(((f, p, k), 1)));
        }
        if c == 0 && k > 0 {
            out.push(self.chain_of_arm(((f, p, k), -1)));
        }
        out.dedup();
        out
    }
}

/// A solved component: its writes by (feature, part, node index), its outcome and its seconds.
type Done = (HashMap<(u32, u32, u32), f32>, Outcome, f64);

/// The heights that a component reads and writes: the store of the depths before it, and its own writes.
struct View<'a> {
    store: &'a [Vec<Vec<f32>>],
    layout: &'a Layout,
    own: HashMap<(u32, u32, u32), f32>,
}

impl View<'_> {
    fn at(&self, node: Node) -> f64 {
        let (f, p) = (node.0, node.1);
        let i = self.layout.index(node) as u32;
        f64::from(*self.own.get(&(f, p, i)).unwrap_or(&self.store[f as usize][p as usize][i as usize]))
    }

    fn set(&mut self, node: Node, z: f32) {
        self.own.insert((node.0, node.1, self.layout.index(node) as u32), z);
    }
}

/// What one chain's solve gives: its writes, its held nodes and covers, and its numbers.
#[derive(Default)]
struct Outcome {
    held: Vec<Node>,
    covered: Vec<(Node, f64)>,
    /// The bounds that the solves held, to check again on the stored heights.
    bounds: Vec<Held>,
    /// Every read: the reading chain, the node read and the height that it took.
    reads: Vec<(usize, Node, f32)>,
    metrics: GroundMetrics,
    kinks: [Vec<f64>; 3],
    #[cfg(test)]
    pieces: Vec<Piece>,
}

impl Heights {
    /// The height of a node; NaN where it has none.
    pub fn at(&self, node: Node) -> f64 {
        f64::from(self.z[node.0 as usize][node.1 as usize][self.layout.index(node)])
    }

    pub fn held(&self, node: Node) -> bool {
        self.held.contains(&node)
    }

    pub fn covered(&self, node: Node) -> bool {
        self.covered.contains_key(&node)
    }

    pub fn lift(&self, node: Node) -> f64 {
        self.covered.get(&node).copied().unwrap_or(0.0)
    }

    /// Per ground chain in read order: nodes, terrain, tube, profile. Components of one depth run on
    /// `threads` threads; each chain writes only its own nodes, and a depth is stored before the next.
    pub fn solve(
        features: &[&Feature],
        classes: &Classes,
        terrain: &dyn Terrain,
        chains: &Chains,
        facts: &AreaFacts,
        threads: usize,
    ) -> Heights {
        let clock = std::time::Instant::now();
        let segments = Segments::new(features);
        let layout = Layout::new(features, &segments);
        let mut place_of = vec![(u32::MAX, 0u32); segments.part.len()];
        for (c, chain) in chains.chains.iter().enumerate() {
            for (j, &arm) in chain.arms.iter().enumerate() {
                place_of[segments.end(arm).0] = (c as u32, j as u32);
            }
        }
        let n = chains.chains.len();
        let mut context = Context {
            features,
            classes,
            terrain,
            chains,
            segments: &segments,
            layout: &layout,
            place_of,
            side: vec![Vec::new(); n],
            approach: vec![Vec::new(); n],
            copies: vec![Vec::new(); n],
            passes: vec![Vec::new(); n],
            covers: vec![Vec::new(); n],
            points: vec![Vec::new(); n],
            loose: vec![Vec::new(); n],
            over_ways: vec![Vec::new(); n],
            over_water: vec![Vec::new(); n],
            unders: vec![Vec::new(); n],
            stacks: vec![Vec::new(); n],
            facts,
            leaders: chains.reads.iter().filter(|r| r.2 == ReadKind::Copy).map(|r| (r.0, r.1)).collect(),
            ramps: facts.ramps.iter().copied().collect(),
            ungraded: facts.ungraded.iter().copied().collect(),
            roots: HashMap::new(),
            reads_itself: vec![false; n],
            shared: chains
                .meets
                .iter()
                .filter(|m| matches!(m.2, ReadKind::Branch | ReadKind::Cross))
                .flat_map(|m| [m.0, m.1])
                .collect(),
        };
        for read in chains.reads.iter().filter(|r| r.0 == r.1) {
            context.reads_itself[read.0] = true;
        }
        for fact in &facts.facts {
            if let Fact::Read { kind: ReadKind::Side, reader, from } = *fact {
                let c = context.chain_of_arm(reader);
                context.side[c].push((reader, from));
                let node = |(f, p, k): Place| (f, p, k, 0);
                context.roots.insert(node(reader.0), node(from.0));
            }
        }
        for &(node, through) in &facts.approach {
            for c in context.chains_of(node) {
                context.approach[c].push((node, through));
            }
        }
        for &copy in &facts.copies {
            for c in context.chains_of(copy.0) {
                context.copies[c].push(copy);
            }
        }
        for (i, pass) in facts.passes.iter().enumerate() {
            for c in context.chains_of(pass.0).into_iter().chain(context.chains_of(pass.1)) {
                context.passes[c].push(i);
            }
        }
        // A pass of a chain over itself is the chain's once.
        for passes in &mut context.passes {
            passes.dedup();
        }
        for (i, cover) in facts.covers.iter().enumerate() {
            for c in context.chains_of(cover.0) {
                context.covers[c].push(i);
            }
        }
        // The facts of each structure chain, by the chain of its own node.
        let structures = |context: &Context, m: Node| -> Vec<usize> {
            context.chains_of(m).into_iter().filter(|&c| chains.chains[c].structure).collect()
        };
        for &(reader, owner, kind) in &chains.meets {
            for c in context.chains_of(reader) {
                let structure = chains.chains[c].structure;
                if structure != (kind == ReadKind::Hanger) {
                    context.points[c].push((reader, owner, kind));
                }
            }
        }
        for &m in &chains.loose {
            for c in structures(&context, m) {
                context.loose[c].push(m);
            }
        }
        let mut lists: [Vec<(usize, usize)>; 7] = Default::default();
        for (i, fact) in facts.over_ways.iter().enumerate() {
            lists[3].extend(structures(&context, fact.0).into_iter().map(|c| (c, i)));
        }
        for (i, fact) in facts.over_water.iter().enumerate() {
            lists[4].extend(structures(&context, fact.0).into_iter().map(|c| (c, i)));
        }
        for (i, fact) in facts.unders.iter().enumerate() {
            lists[5].extend(structures(&context, fact.0).into_iter().map(|c| (c, i)));
        }
        for (i, fact) in facts.stacks.iter().enumerate() {
            lists[6].extend(structures(&context, fact.0).into_iter().map(|c| (c, i)));
        }
        let [_, _, _, over_ways, over_water, unders, stacks] = lists;
        for (list, into) in [
            (over_ways, &mut context.over_ways),
            (over_water, &mut context.over_water),
            (unders, &mut context.unders),
            (stacks, &mut context.stacks),
        ] {
            for (c, i) in list {
                into[c].push(i);
            }
            for facts in into.iter_mut() {
                facts.dedup();
            }
        }
        let mut z: Vec<Vec<Vec<f32>>> = layout
            .first
            .iter()
            .map(|parts| parts.iter().map(|first| vec![f32::NAN; first.last().map_or(0, |&e| e as usize)]).collect())
            .collect();
        let mut by_depth: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, &d) in chains.depth.iter().enumerate() {
            by_depth.entry(d).or_default().push(i);
        }
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).build().expect("a thread pool");
        let (mut held, mut covered) = (HashSet::new(), HashMap::new());
        let mut bounds = Vec::new();
        let mut metrics = GroundMetrics::default();
        let mut kinks: [Vec<f64>; 3] = Default::default();
        let mut reads = Vec::new();
        #[cfg(test)]
        let mut solved_pieces = Vec::new();
        for components in by_depth.values() {
            let done: Vec<Done> =
                pool.install(|| components.par_iter().map(|&i| component(&context, &z, &chains.order[i])).collect());
            for (i, (writes, outcome, seconds)) in components.iter().zip(done) {
                for ((f, p, k), v) in writes {
                    z[f as usize][p as usize][k as usize] = v;
                }
                held.extend(outcome.held);
                covered.extend(outcome.covered);
                bounds.extend(outcome.bounds);
                reads.extend(outcome.reads);
                add(&mut metrics, &outcome.metrics);
                for (all, some) in kinks.iter_mut().zip(outcome.kinks) {
                    all.extend(some);
                }
                let size = chains.order[*i].len();
                if size > 1 && size >= metrics.largest_cycle_chains {
                    (metrics.largest_cycle_chains, metrics.largest_cycle_seconds) = (size, seconds);
                }
                #[cfg(test)]
                solved_pieces.extend(outcome.pieces);
            }
        }
        // A point that the parts of several ways share: one height in the store.
        let mut at: HashMap<[u64; 2], f32> = HashMap::new();
        let mut apart = HashSet::new();
        for (f, feature) in features.iter().enumerate() {
            for (p, part) in feature.parts.iter().enumerate() {
                for (k, &point) in distinct(part).iter().enumerate() {
                    let v = z[f][p][layout.index((f as u32, p as u32, k as u32, 0))];
                    if !v.is_finite() {
                        continue;
                    }
                    let key = crate::geometry::key(point);
                    match at.get(&key) {
                        Some(w) if w.to_bits() != v.to_bits() => {
                            if apart.insert(key) {
                                metrics.shared_apart_worst.push((f64::from((w - v).abs()), degrees(point)));
                            }
                        }
                        Some(_) => {}
                        None => {
                            at.insert(key, v);
                        }
                    }
                }
            }
        }
        metrics.shared_apart = apart.len();
        // Every bound that a solve held, again on the stored heights.
        let at = |m: Node| f64::from(z[m.0 as usize][m.1 as usize][layout.index(m)]);
        let along = |m: Node, share: f32| at(m) + f64::from(share) * (at(layout.next(m)) - at(m));
        let segment = |m: Node| [at(m), at(layout.next(m))];
        for bound in bounds {
            let (kind, node, short) = match bound {
                Held::Pass(k) => {
                    let (upper, lower, u, x, need, upper_reads) = facts.passes[k];
                    let need = f64::from(need);
                    if upper_reads {
                        (0, upper, along(lower, x) + need - segment(upper).into_iter().fold(f64::INFINITY, f64::min))
                    } else {
                        (
                            0,
                            lower,
                            segment(lower).into_iter().fold(f64::NEG_INFINITY, f64::max) - along(upper, u) + need,
                        )
                    }
                }
                Held::Cover(k) => {
                    let (node, least, _, _) = facts.covers[k];
                    (1, node, least - segment(node).into_iter().fold(f64::INFINITY, f64::min))
                }
                Held::Deck(deck, k) => {
                    let (node, least) = match deck {
                        0 => {
                            let (node, under, share, least) = facts.over_ways[k];
                            (node, along(under, share) + f64::from(least))
                        }
                        1 => facts.over_water[k],
                        _ => {
                            let (upper, lower, _, x, need) = facts.stacks[k];
                            (upper, along(lower, x) + f64::from(need))
                        }
                    };
                    (2 + deck, node, least - segment(node).into_iter().fold(f64::INFINITY, f64::min))
                }
                Held::Under(k) => {
                    let (bore, road, share, need) = facts.unders[k];
                    let most = along(road, share) - f64::from(need);
                    (5, bore, segment(bore).into_iter().fold(f64::NEG_INFINITY, f64::max) - most)
                }
                Held::Terrain(node, h) => (6, node, at(node) - h),
            };
            if short > 1e-3 {
                metrics.held_unmet += 1;
                metrics.held_unmet_all.push((kind, degrees(point_of(features, &layout, node)), short));
            }
        }
        metrics.reads = reads.len();
        for &(_, m, v) in &reads {
            let now = z[m.0 as usize][m.1 as usize][layout.index(m)];
            if now.to_bits() != v.to_bits() {
                metrics.read_changed += 1;
                metrics.read_changed_max = metrics.read_changed_max.max(f64::from((now - v).abs()));
                let at = degrees(point_of(features, &layout, m));
                metrics.read_changed_worst.push((f64::from((now - v).abs()), at));
            }
        }
        for (k, values) in kinks.iter_mut().enumerate() {
            metrics.kinks[k] = values.iter().filter(|&&v| v > KINK).count();
            metrics.kink_p99[k] = crate::geometry::percentile(values, 99.0).unwrap_or(0.0);
            metrics.kink_max[k] = values.iter().copied().fold(0.0, f64::max);
        }
        for worst in [
            &mut metrics.frozen_worst,
            &mut metrics.approach_ended_worst,
            &mut metrics.read_changed_worst,
            &mut metrics.shared_apart_worst,
        ] {
            worst.sort_by(|a, b| b.0.total_cmp(&a.0));
            worst.truncate(10);
        }
        metrics.passes_dropped_worst.sort_by(|a, b| b.miss.total_cmp(&a.miss));
        metrics.passes_dropped_worst.truncate(10);
        metrics.passes_left_out_worst.truncate(10);
        for worst in
            [&mut metrics.structure_opened_worst, &mut metrics.structure_over_cap_worst, &mut metrics.tunnel_over_worst]
        {
            worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        }
        metrics.tunnel_over_worst.truncate(100);
        metrics.corners.sort_by(|a, b| b.0.total_cmp(&a.0));
        for worst in
            [&mut metrics.ground_gate_all, &mut metrics.tunnel_gate_skipped_worst, &mut metrics.tunnel_gate_fail_all]
        {
            worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        }
        metrics.tunnel_gate_skipped_worst.truncate(10);
        metrics.tunnel_gate_origins.sort_by(|a, b| b.2.total_cmp(&a.2));
        metrics.held_unmet_all.sort_by(|a, b| b.2.total_cmp(&a.2));
        metrics.points_apart.sort_by(|a, b| (b.own - b.owner).abs().total_cmp(&(a.own - a.owner).abs()));
        for a in &metrics.points_apart {
            eprintln!(
                "roads: point {:.6},{:.6}: owner {:.2} m (feature {}, class {}, {} fixed ends), reader's own {:.2} m \
                 (feature {}, class {}, {} fixed ends)",
                a.place[0], a.place[1], a.owner, a.by.0, a.by.1, a.by.2, a.own, a.reader.0, a.reader.1, a.reader.2
            );
        }
        for all in [&mut metrics.deck_short_all, &mut metrics.bore_exposed_all] {
            all.sort_by(|a, b| b.metres.total_cmp(&a.metres));
        }
        for all in [&mut metrics.joins_held_worst, &mut metrics.joins_dropped_all] {
            all.sort_by(|a, b| b.0.total_cmp(&a.0));
        }
        metrics.joins_held_worst.truncate(10);
        metrics.passes = facts.passes.len();
        debug_assert_eq!(
            metrics.passes_held + metrics.passes_dropped + metrics.passes_left_out,
            metrics.passes,
            "every ground pass is held, dropped or left out"
        );
        metrics.grade_broken_worst.sort_by(|a, b| b.slope.total_cmp(&a.slope));
        metrics.grade_broken_worst.truncate(10);
        metrics.cut_worst.sort_by(|a, b| b.0.total_cmp(&a.0));
        metrics.cut_worst.truncate(10);
        metrics.seconds = clock.elapsed().as_secs_f64();
        Heights {
            z,
            layout,
            held,
            covered,
            metrics,
            #[cfg(test)]
            pieces: solved_pieces,
        }
    }
}

fn add(all: &mut GroundMetrics, one: &GroundMetrics) {
    all.chains += one.chains;
    all.plain += one.plain;
    all.steps += one.steps;
    all.steps_max = all.steps_max.max(one.steps_max);
    all.unconverged += one.unconverged;
    all.pivots += one.pivots;
    for k in 0..5 {
        all.cuts[k] += one.cuts[k];
        all.cut_m[k] += one.cut_m[k];
        all.openings[k] += one.openings[k];
    }
    all.excess_max = all.excess_max.max(one.excess_max);
    all.cut_worst.extend(one.cut_worst.iter().copied());
    for k in 0..3 {
        all.opened_m[k] += one.opened_m[k];
    }
    for k in 0..4 {
        all.off[k] += one.off[k];
        all.off_band[k] += one.off_band[k];
    }
    for k in 0..2 {
        all.split_km[k] += one.split_km[k];
        all.split_grade_m[k] += one.split_grade_m[k];
        all.split_kink_m[k] += one.split_kink_m[k];
    }
    all.frozen_worst.extend(one.frozen_worst.iter().copied());
    all.approach_ended += one.approach_ended;
    all.approach_ended_worst.extend(one.approach_ended_worst.iter().copied());
    all.passes_held += one.passes_held;
    all.passes_dropped += one.passes_dropped;
    for (&need, &count) in &one.passes_dropped_by_need {
        *all.passes_dropped_by_need.entry(need).or_default() += count;
    }
    all.passes_dropped_worst.extend(one.passes_dropped_worst.iter().copied());
    all.passes_left_out += one.passes_left_out;
    all.passes_left_out_worst.extend(one.passes_left_out_worst.iter().copied());
    all.reads_skipped += one.reads_skipped;
    all.grade_broken += one.grade_broken;
    all.grade_broken_m += one.grade_broken_m;
    all.grade_broken_worst.extend(one.grade_broken_worst.iter().copied());
    all.structure_chains += one.structure_chains;
    for k in 0..3 {
        all.structure_kinds[k] += one.structure_kinds[k];
        all.structure_ends[k] += one.structure_ends[k];
        all.deck_short[k] += one.deck_short[k];
    }
    all.structure_opened += one.structure_opened;
    all.structure_opened_m += one.structure_opened_m;
    all.structure_opened_worst.extend(one.structure_opened_worst.iter().copied());
    all.deck_short_all.extend(one.deck_short_all.iter().copied());
    all.bore_exposed += one.bore_exposed;
    all.bore_exposed_all.extend(one.bore_exposed_all.iter().copied());
    all.tunnel_over += one.tunnel_over;
    all.tunnel_over_worst.extend(one.tunnel_over_worst.iter().copied());
    all.structure_on_string += one.structure_on_string;
    all.points_apart.extend(one.points_apart.iter().copied());
    all.ground_gate += one.ground_gate;
    all.ground_gate_all.extend(one.ground_gate_all.iter().copied());
    all.ground_gate_cut += one.ground_gate_cut;
    all.tunnel_gate_over += one.tunnel_gate_over;
    all.tunnel_gate_skipped += one.tunnel_gate_skipped;
    all.tunnel_gate_skipped_worst.extend(one.tunnel_gate_skipped_worst.iter().copied());
    all.tunnel_gate_origins.extend(one.tunnel_gate_origins.iter().copied());
    all.tunnel_gate_fail += one.tunnel_gate_fail;
    all.tunnel_gate_fail_all.extend(one.tunnel_gate_fail_all.iter().copied());
    all.corners.extend(one.corners.iter().copied());
    all.structure_over_cap += one.structure_over_cap;
    all.structure_over_cap_worst.extend(one.structure_over_cap_worst.iter().copied());
    all.tunnel_over_at_1_1 += one.tunnel_over_at_1_1;
    all.joins_held += one.joins_held;
    all.joins_held_worst.extend(one.joins_held_worst.iter().copied());
    all.joins_zone_off += one.joins_zone_off;
    all.joins_dropped += one.joins_dropped;
    all.joins_dropped_all.extend(one.joins_dropped_all.iter().copied());
}

/// A component of the read graph: its ground chains in its order; a cycle in two rounds. Returns its writes,
/// its outcome and its seconds.
fn component(context: &Context, store: &[Vec<Vec<f32>>], members: &[usize]) -> Done {
    let clock = std::time::Instant::now();
    let mut view = View { store, layout: context.layout, own: HashMap::new() };
    let mut outcome = Outcome::default();
    let cycle = members.len() > 1
        || members.iter().any(|&c| context.chains.chains[c].closed)
        || context.reads_itself[members[0]];
    let mut state = Cycle { solved: HashSet::new(), read: HashSet::new(), weights: HashMap::new(), cycle };
    let solve = |view: &mut View, state: &mut Cycle, c: usize, second: bool, outcome: &mut Outcome| {
        if context.chains.chains[c].structure {
            solve_structure(context, view, state, c, second, outcome)
        } else {
            solve_chain(context, view, state, c, second, outcome)
        }
    };
    let mut again = Vec::new();
    for &c in members {
        let waited = solve(&mut view, &mut state, c, false, &mut outcome);
        state.solved.insert(c);
        if waited {
            again.push(c);
        }
    }
    for &c in &again {
        solve(&mut view, &mut state, c, true, &mut outcome);
    }
    (view.own, outcome, clock.elapsed().as_secs_f64())
}

/// The state of a component while it is solved: its chains solved so far, the nodes that a read took a
/// height from, and the Huber weights of each first solve.
struct Cycle {
    solved: HashSet<usize>,
    read: HashSet<Node>,
    weights: HashMap<usize, Vec<f64>>,
    cycle: bool,
}

/// A bound that a solve held, by its fact: a ground pass, a cover, a deck (over a way, over water, over a
/// structure), a tunnel under a road, and a tunnel node under the terrain (its node and the terrain there).
#[derive(Clone, Copy, Debug)]
enum Held {
    Pass(usize),
    Cover(usize),
    Deck(u8, usize),
    Under(usize),
    Terrain(Node, f64),
}

/// What a read takes: heights, nothing yet, or a node of the reading chain itself (its index, or none
/// when the read mixes it with another chain's node).
enum Took {
    Height(Vec<f64>),
    Waits,
    Own(usize),
}

/// Why a node is fixed.
#[derive(Clone, Copy, PartialEq)]
enum Fix {
    Read,
    Frozen,
}

/// A deck bound of a structure: its kind (0 over a way, 1 over water, 2 over a structure), the nodes of its
/// segment, the least height, the class of the other way, and its fact.
type Deck = (u8, [usize; 2], f64, usize, usize);

/// A tunnel bound under a road: the nodes of its segment, the most height, the class of the road, its fact.
type Bound = ([usize; 2], f64, usize, usize);

/// The bounds of a structure chain: its decks, its tunnel crossings, and its tunnel nodes with the terrain.
type Bounds<'a> = (&'a [Deck], &'a [&'a Vec<Bound>], &'a [(usize, f64)]);

/// What the reach of a structure's tube holds, under its slope bounds: the lower bounds (the line where it
/// has one, and the held decks) and the upper bounds (the held tunnel bounds); per deck its miss on the reach
/// of the fixed nodes and the slopes; per tunnel crossing and per tunnel node under the terrain its miss on
/// the reach after the held decks, with that reach. A miss of at most 1e-9 holds.
struct Reach {
    lo: Vec<f64>,
    hi: Vec<f64>,
    decks: Vec<f64>,
    bores: Vec<f64>,
    terrain: Vec<f64>,
    lower: Vec<f64>,
}

fn reach(
    soft: &chain::Chain,
    fixed: &[f64],
    (slo, shi): (&[f64], &[f64]),
    line: &[f64],
    (decks, bores, terrain): Bounds,
) -> Reach {
    let n = fixed.len();
    let slopes = || Step::Slopes { layer: 1, lo: slo, hi: shi };
    let (_, upper) = chain::sweep(&chain::tube(soft, &[Step::Fixed(fixed), slopes()]));
    let mut lo = line.to_vec();
    let decks: Vec<f64> = decks
        .iter()
        .map(|&(_, pair, least, _, _)| {
            let miss = pair.iter().map(|&i| least - upper[i]).fold(f64::NEG_INFINITY, f64::max);
            if miss <= 1e-9 {
                pair.iter().for_each(|&i| lo[i] = lo[i].max(least));
            }
            miss
        })
        .collect();
    let infinite = vec![f64::INFINITY; n];
    let t2 = chain::tube(soft, &[Step::Fixed(fixed), slopes(), Step::Bounds { layer: 2, lo: &lo, hi: &infinite }]);
    let (lower, _) = chain::sweep(&t2);
    let mut hi = vec![f64::INFINITY; n];
    let bores: Vec<f64> = bores
        .iter()
        .map(|crossing| {
            let at = crossing.iter().flat_map(|&(pair, most, _, _)| pair.map(|i| lower[i] - most));
            let miss = at.fold(f64::NEG_INFINITY, f64::max);
            if miss <= 1e-9 {
                for &(pair, most, _, _) in crossing.iter() {
                    pair.iter().for_each(|&i| hi[i] = hi[i].min(most));
                }
            }
            miss
        })
        .collect();
    let terrain: Vec<f64> = terrain
        .iter()
        .map(|&(i, h)| {
            let miss = lower[i] - h;
            if miss <= 1e-9 {
                hi[i] = hi[i].min(h);
            }
            miss
        })
        .collect();
    Reach { lo, hi, decks, bores, terrain, lower }
}

/// The heights of a structure chain from its fixed nodes: its line (straight between its first and its last
/// fixed node, level past them), what its bounds hold under its caps and under 1:1, its tube, and the tight
/// string through the tube.
struct Shape {
    z: Vec<f64>,
    line: Vec<f64>,
    held: Reach,
    loose: Reach,
    tube: chain::Tube,
}

fn shape(
    soft: &chain::Chain,
    fixed: &[f64],
    (along, caps, bridge): (&[f64], &[f64], &[bool]),
    bounds: Bounds,
) -> Shape {
    let n = fixed.len();
    let held_at: Vec<usize> = (0..n).filter(|&i| fixed[i].is_finite()).collect();
    let (p, q) = (held_at[0], held_at[held_at.len() - 1]);
    let line: Vec<f64> = (0..n)
        .map(|i| match i {
            i if i <= p => fixed[p],
            i if i >= q => fixed[q],
            i => fixed[p] + (fixed[q] - fixed[p]) * (along[i] - along[p]) / (along[q] - along[p]),
        })
        .collect();
    let both = |s: usize| fixed[s].is_finite() && fixed[s + 1].is_finite();
    let slopes_of = |cap: &dyn Fn(usize) -> f64| -> (Vec<f64>, Vec<f64>) {
        (0..n - 1).map(|s| if both(s) { (f64::NEG_INFINITY, f64::INFINITY) } else { (-cap(s), cap(s)) }).unzip()
    };
    let (slo, shi) = slopes_of(&|s| caps[s]);
    // A bridge node is not under the line between the chain's two ends where both are fixed.
    let mut line_lo = vec![f64::NEG_INFINITY; n];
    if fixed[0].is_finite() && fixed[n - 1].is_finite() {
        for i in (0..n).filter(|&i| bridge[i]) {
            line_lo[i] = line[i];
        }
    }
    let held = reach(soft, fixed, (&slo, &shi), &line_lo, bounds);
    let (slo1, shi1) = slopes_of(&|_| 1.0);
    let loose = reach(soft, fixed, (&slo1, &shi1), &line_lo, bounds);
    let (infinite, none) = (vec![f64::INFINITY; n], vec![f64::NEG_INFINITY; n]);
    let steps = [
        Step::Fixed(fixed),
        Step::Slopes { layer: 1, lo: &slo, hi: &shi },
        Step::Bounds { layer: 2, lo: &held.lo, hi: &infinite },
        Step::Bounds { layer: 3, lo: &none, hi: &held.hi },
    ];
    let tube = chain::tube(soft, &steps);
    debug_assert!(
        (0..n).all(|i| {
            let (a, b) = if tube.fixed[i].is_nan() { (tube.lo[i], tube.hi[i]) } else { (tube.fixed[i], tube.fixed[i]) };
            a >= held.lo[i] - 1e-9 && b <= held.hi[i] + 1e-9
        }),
        "the tube meets every held bound of a structure in full"
    );
    // The tight string through the gates of the tube, over metres times the cap: it holds each cap wherever a
    // curve of the tube can. Its heights are the sum of its rises from a fixed node.
    let gate = |i: usize, side: &[f64]| if tube.fixed[i].is_nan() { side[i] } else { tube.fixed[i] };
    let (gate_lo, gate_hi): (Vec<f64>, Vec<f64>) = (0..n).map(|i| (gate(i, &tube.lo), gate(i, &tube.hi))).unzip();
    let x: Vec<f64> = std::iter::once(0.0)
        .chain((0..n - 1).scan(0.0, |x, s| {
            *x += soft.l[s] * caps[s];
            Some(*x)
        }))
        .collect();
    let rises = chain::string(&x, &gate_lo, &gate_hi);
    let start = (0..n).find(|&i| !tube.fixed[i].is_nan()).expect("a structure has a fixed node");
    let mut z = vec![tube.fixed[start]; n];
    for i in start..n - 1 {
        z[i + 1] = z[i] + rises[i];
    }
    for i in (0..start).rev() {
        z[i] = z[i + 1] - rises[i];
    }
    Shape { z, line, held, loose, tube }
}

/// Solves one structure chain into the view; returns whether a read waited for a chain of its component.
/// Its ends take the heights of the ground ways, of the terrain where a bridge meets no way, or of the
/// main chain of a branch; a chain that no read fixes takes the terrain at its two ends. No segment is
/// steeper than twice the grade of its class (twice the ramp grade for a class without one), but between
/// two fixed nodes that force it. Its decks hold over what they cross and its tunnels under the terrain and
/// the roads over them, each bound whole where the reach of the tube meets it, else not at all. Its heights
/// are the tight string through its fixed nodes and its held bounds: straight runs between them.
fn solve_structure(
    context: &Context,
    view: &mut View,
    state: &mut Cycle,
    c: usize,
    second: bool,
    out: &mut Outcome,
) -> bool {
    let chain = &context.chains.chains[c];
    let nodes = Nodes::new(&chain.arms, context.segments, context.layout);
    let n = nodes.xy.len();
    let index: HashMap<Node, usize> =
        nodes.names.iter().enumerate().flat_map(|(i, names)| names.iter().map(move |&m| (m, i))).collect();
    let h = context.terrain.heights(&nodes.xy);
    let l: Vec<f64> = (0..n - 1).map(|s| span(nodes.xy[s], nodes.xy[s + 1]).max(chain::MIN_STEP_M)).collect();
    let along: Vec<f64> = std::iter::once(0.0)
        .chain(l.iter().scan(0.0, |x, l| {
            *x += l;
            Some(*x)
        }))
        .collect();
    let (mut fixed, mut cause) = (vec![f64::NAN; n], vec![None; n]);
    let mut frozen = vec![false; n];
    if second {
        for i in 0..n {
            if nodes.names[i].iter().any(|m| state.read.contains(m)) {
                (fixed[i], cause[i], frozen[i]) = (view.at(nodes.names[i][0]), Some(Fix::Frozen), true);
            }
        }
    }
    let (mut waited, mut log, mut skipped) = (false, Vec::new(), 0);
    let mut take = |source: &[Node], from: &[usize], state: &mut Cycle| {
        read(context, view, (c, second, &index), (source, from), state, (&mut waited, &mut log))
    };
    let pair = |m: Node| [m, context.layout.next(m)];
    // The nodes of the chain on the segment from node m of its way, in either direction.
    let segment = |m: Node| -> Option<[usize; 2]> { Some([*index.get(&m)?, *index.get(&context.layout.next(m))?]) };
    let end = |m: Node| [0, n - 1].into_iter().find(|&e| nodes.names[e].contains(&m)).or(index.get(&m).copied());
    // The fixed nodes: an end on a ground way, on the terrain or on the chain that owns its point; a node
    // where another chain that owns the point runs through.
    let mut owned: Vec<(usize, f64, Node)> = Vec::new();
    for &(mine, from, kind) in &context.points[c] {
        let Some(i) = end(mine) else { continue };
        if frozen[i] {
            skipped += 1;
            continue;
        }
        match take(&[from], &context.chains_of(from), state) {
            Took::Height(z) => {
                (fixed[i], cause[i]) = (z[0], Some(Fix::Read));
                if kind != ReadKind::End {
                    owned.push((i, z[0], from));
                }
            }
            Took::Own(_) => skipped += 1,
            Took::Waits => {}
        }
    }
    for &m in &context.loose[c] {
        if let Some(i) = end(m).filter(|&i| !frozen[i] && h[i].is_finite()) {
            (fixed[i], cause[i]) = (h[i], Some(Fix::Read));
        }
    }
    let fixed_ends = usize::from(fixed[0].is_finite()) + usize::from(fixed[n - 1].is_finite());
    // A chain that nothing fixes takes the terrain at its two ends; without terrain there it has no heights.
    if fixed.iter().all(|v| v.is_nan()) {
        for e in [0, n - 1].into_iter().filter(|&e| h[e].is_finite()) {
            (fixed[e], cause[e]) = (h[e], Some(Fix::Read));
        }
        if fixed.iter().all(|v| v.is_nan()) {
            out.reads.extend(log.into_iter().map(|(m, v)| (c, m, v)));
            return waited;
        }
    }
    let held_at: Vec<usize> = (0..n).filter(|&i| fixed[i].is_finite()).collect();
    let feature = |s: usize| context.features[nodes.feature[s.min(n - 2)]];
    let cap = |s: usize| {
        let grade = context.classes.rows[feature(s).rule].grade;
        2.0 * if grade.is_finite() { grade } else { RAMP_GRADE }
    };
    let caps: Vec<f64> = (0..n - 1).map(cap).collect();
    let both = |s: usize| fixed[s].is_finite() && fixed[s + 1].is_finite();
    let on = |i: usize, tunnel: bool| {
        [i.checked_sub(1), (i + 1 < n).then_some(i)].into_iter().flatten().any(|s| {
            let f = context.features[nodes.feature[s]];
            f.tunnel == tunnel
        })
    };
    let bridge: Vec<bool> = (0..n).map(|i| on(i, false)).collect();
    // The bounds: the decks over what they cross; the tunnels under the roads over them, a crossing (the
    // road's middle and its two edges) as one; the tunnels under the terrain, node by node.
    let rule = |m: Node| context.features[m.0 as usize].rule;
    let mut decks: Vec<Deck> = Vec::new();
    for &k in &context.over_ways[c] {
        let (deck, under, share, least) = context.facts.over_ways[k];
        let Some(i) = segment(deck) else { continue };
        if let Took::Height(z) = take(&pair(under), &context.chains_of(under), state) {
            decks.push((0, i, z[0] + f64::from(share) * (z[1] - z[0]) + f64::from(least), rule(under), k));
        }
    }
    for &k in &context.over_water[c] {
        let (deck, least) = context.facts.over_water[k];
        if let Some(i) = segment(deck) {
            decks.push((1, i, least, context.features[deck.0 as usize].rule, k));
        }
    }
    for &k in &context.stacks[c] {
        let (upper, lower, _, x, need) = context.facts.stacks[k];
        let Some(i) = segment(upper) else { continue };
        if let Took::Height(z) = take(&pair(lower), &context.chains_of(lower), state) {
            decks.push((2, i, z[0] + f64::from(x) * (z[1] - z[0]) + f64::from(need), rule(lower), k));
        }
    }
    let mut bores: BTreeMap<(Node, u32), Vec<Bound>> = BTreeMap::new();
    for &k in &context.unders[c] {
        let (bore, road, share, need) = context.facts.unders[k];
        let Some(i) = segment(bore) else { continue };
        if let Took::Height(z) = take(&pair(road), &context.chains_of(road), state) {
            let most = z[0] + f64::from(share) * (z[1] - z[0]) - f64::from(need);
            bores.entry((road, share.to_bits())).or_default().push((i, most, rule(road), k));
        }
    }
    let crossings: Vec<&Vec<Bound>> = bores.values().collect();
    let terrain: Vec<(usize, f64)> = (0..n).filter(|&i| on(i, true) && h[i].is_finite()).map(|i| (i, h[i])).collect();
    let bounds = (&decks[..], &crossings[..], &terrain[..]);
    // The tube reads only the lengths of its chain.
    let (zeros, flat) = (vec![0.0; n], vec![0.0; n - 1]);
    let soft = chain::Chain { l: &l, fit: &zeros, target: &zeros, slope: &flat, bend: &zeros };
    let Shape { z, line, held, loose, tube } = shape(&soft, &fixed, (&along, &caps, &bridge), bounds);
    for (i, names) in nodes.names.iter().enumerate() {
        for &m in names {
            view.set(m, z[i] as f32);
        }
    }
    // Where this chain reads the owner of a point, its own string without that read, if it lies more than
    // 5 m off the owner's height there.
    if second || !waited {
        for &(i, height, from) in &owned {
            let mut alone = fixed.clone();
            alone[i] = f64::NAN;
            if alone.iter().all(|v| v.is_nan()) {
                continue;
            }
            let own = shape(&soft, &alone, (&along, &caps, &bridge), bounds).z[i];
            if (own - height).abs() > 5.0 {
                let chain_of = |c: usize| {
                    let chain = &context.chains.chains[c];
                    (chain.arms[0].0 .0 as usize, chain.rule, context.chains.fixed_ends[c])
                };
                let owner = context.chains_of(from).into_iter().find(|&b| b != c).unwrap_or(c);
                out.metrics.points_apart.push(Apart {
                    place: degrees(nodes.xy[i]),
                    own,
                    owner: height,
                    reader: chain_of(c),
                    by: chain_of(owner),
                });
            }
        }
    }
    let straight = (0..n).all(|i| (z[i] - line[i]).abs() <= 1e-6);
    if second || !waited {
        let m = &mut out.metrics;
        m.reads_skipped += skipped;
        m.structure_chains += 1;
        let tunnels = (0..n - 1).filter(|&s| context.features[nodes.feature[s]].tunnel).count();
        m.structure_kinds[if tunnels == 0 {
            0
        } else if tunnels == n - 1 {
            1
        } else {
            2
        }] += 1;
        m.structure_ends[fixed_ends] += 1;
        m.structure_on_string += usize::from(!straight);
        for i in (0..n).filter(|&i| tube.fixed[i].is_nan() && (z[i] - tube.lo[i] <= 1e-9 || tube.hi[i] - z[i] <= 1e-9))
        {
            out.held.extend(nodes.names[i].iter().copied());
        }
        // A corner of the string: where its grade changes.
        for i in 1..n - 1 {
            let change = ((z[i + 1] - z[i]) / l[i] - (z[i] - z[i - 1]) / l[i - 1]).abs();
            if change > 1e-9 {
                m.corners.push((change, degrees(nodes.xy[i])));
            }
        }
        // A segment steeper than its cap: between two fixed nodes that force it, or not at all.
        let opened: HashSet<usize> = tube.opened.iter().map(|o| o.segment).collect();
        let steep: Vec<usize> = (0..n - 1).filter(|&s| (z[s + 1] - z[s]).abs() > cap(s) * l[s] + 1e-9).collect();
        let (forced, over): (Vec<usize>, Vec<usize>) = steep.iter().partition(|&&s| both(s) || opened.contains(&s));
        let slope = |s: usize| ((z[s + 1] - z[s]) / l[s]).abs();
        if !forced.is_empty() {
            m.structure_opened += 1;
            m.structure_opened_m += forced.iter().map(|&s| l[s]).sum::<f64>();
            let steepest = forced.iter().map(|&s| slope(s)).fold(0.0, f64::max);
            m.structure_opened_worst.push((steepest, degrees(nodes.xy[forced[0]])));
        }
        m.structure_over_cap += over.len();
        m.structure_over_cap_worst.extend(over.iter().map(|&s| (slope(s), degrees(nodes.xy[s]))));
        // The metres along the chain from node i to its nearest fixed node.
        let to_end = |i: usize| held_at.iter().map(|&a| (along[a] - along[i]).abs()).fold(f64::NAN, f64::min);
        for (k, &(kind, [i, j], _, other, _)) in decks.iter().enumerate().filter(|&(k, _)| held.decks[k] > 1e-9) {
            m.deck_short[usize::from(kind)] += 1;
            let (place, rules, at_1_1) =
                (degrees(nodes.xy[i]), [feature(i.min(j)).rule, other], loose.decks[k] <= 1e-9);
            let to_end = to_end(i).min(to_end(j));
            m.deck_short_all.push(Short { kind, metres: held.decks[k], place, to_end, rules, at_1_1 });
        }
        let point = |m: Node| point_of(context.features, context.layout, m);
        for (k, (&(road, share), crossing)) in bores.iter().enumerate().filter(|&(k, _)| held.bores[k] > 1e-9) {
            // It misses by what the tunnel misses under the road's middle: on the segment nearest to it.
            let (a, b, u) = (point(road), point(context.layout.next(road)), f64::from(f32::from_bits(share)));
            let middle = [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])];
            let gap = |[i, j]: [usize; 2]| {
                let (p, d) = (nodes.xy[i], sub(nodes.xy[j], nodes.xy[i]));
                let t = (dot(sub(middle, p), d) / dot(d, d).max(1e-300)).clamp(0.0, 1.0);
                hypot(sub(middle, [p[0] + t * d[0], p[1] + t * d[1]]))
            };
            let &(pair, most, other, _) = crossing.iter().min_by(|x, y| gap(x.0).total_cmp(&gap(y.0))).unwrap();
            let miss = pair.map(|i| held.lower[i] - most).into_iter().fold(f64::NEG_INFINITY, f64::max);
            m.bore_exposed += 1;
            let (rules, at_1_1) = ([feature(pair[0]).rule, other], loose.bores[k] <= 1e-9);
            let place = degrees(middle);
            m.bore_exposed_all.push(Short { kind: 3, metres: miss, place, to_end: to_end(pair[0]), rules, at_1_1 });
        }
        for (k, &(i, _)) in terrain.iter().enumerate().filter(|&(k, _)| held.terrain[k] > 1e-9) {
            m.tunnel_over += 1;
            m.tunnel_over_at_1_1 += usize::from(loose.terrain[k] <= 1e-9);
            m.tunnel_over_worst.push((held.terrain[k], degrees(nodes.xy[i])));
        }
        // The tunnel gate: a tunnel node more than GATE_M over the terrain fails, unless a 1:1 line down from
        // an origin of the chain cannot bring it under the terrain there.
        let tunnel_at = |s: usize| context.features[nodes.feature[s]].tunnel;
        let shared = |i: usize| nodes.names[i].iter().any(|m| context.shared.contains(m));
        let origins: Vec<(usize, u8)> = (0..n)
            .filter_map(|i| {
                let fixed_end = (i == 0 || i == n - 1) && fixed[i].is_finite();
                let joint = i > 0 && i < n - 1 && tunnel_at(i - 1) != tunnel_at(i);
                [(fixed_end, 0), (shared(i), 1), (joint, 2)].into_iter().find(|f| f.0).map(|(_, kind)| (i, kind))
            })
            .collect();
        let mut skipping: BTreeMap<usize, f64> = BTreeMap::new();
        for i in (0..n).filter(|&i| on(i, true) && h[i].is_finite() && z[i] - h[i] > crate::gate::GATE_M) {
            let over = z[i] - h[i];
            m.tunnel_gate_over += 1;
            let origin = origins.iter().find(|&&(o, _)| h[i] < z[o] - (along[o] - along[i]).abs());
            match origin {
                Some(&(o, _)) => {
                    m.tunnel_gate_skipped += 1;
                    m.tunnel_gate_skipped_worst.push((over, degrees(nodes.xy[i])));
                    let largest = skipping.entry(o).or_insert(0.0);
                    *largest = largest.max(over);
                }
                None => {
                    m.tunnel_gate_fail += 1;
                    m.tunnel_gate_fail_all.push((over, degrees(nodes.xy[i])));
                }
            }
        }
        for (o, largest) in skipping {
            let kind = origins.iter().find(|f| f.0 == o).map_or(0, |f| f.1);
            m.tunnel_gate_origins.push((kind, degrees(nodes.xy[o]), largest));
        }
        // The bounds that this solve holds, to check again on the stored heights.
        for (_, deck) in decks.iter().enumerate().filter(|&(k, _)| held.decks[k] <= 1e-9) {
            out.bounds.push(Held::Deck(deck.0, deck.4));
        }
        for (_, crossing) in crossings.iter().enumerate().filter(|&(k, _)| held.bores[k] <= 1e-9) {
            out.bounds.extend(crossing.iter().map(|b| Held::Under(b.3)));
        }
        for (_, &(i, height)) in terrain.iter().enumerate().filter(|&(k, _)| held.terrain[k] <= 1e-9) {
            out.bounds.push(Held::Terrain(nodes.names[i][0], height));
        }
    }
    #[cfg(test)]
    out.pieces.push(Piece { h, l, tube, z, structure: true, plain: straight });
    out.reads.extend(log.into_iter().map(|(m, v)| (c, m, v)));
    waited
}

/// The index of a node that only chain c holds: its own node.
fn own(context: &Context, index: &HashMap<Node, usize>, c: usize, m: Node) -> Option<usize> {
    index.get(&m).copied().filter(|_| context.chains_of(m).iter().all(|&b| b == c))
}

/// One read of chain c: the heights of `source`, which the chains `from` hold. A read takes a height that
/// exists; one from a chain of this component that is not solved yet waits. A read of a junction reads its
/// root. A read of a node that only this chain holds is its own: the caller resolves it after its other
/// reads, or leaves it out. Each height taken is logged, and its node is marked as read.
fn read(
    context: &Context,
    view: &View,
    (c, second, index): (usize, bool, &HashMap<Node, usize>),
    (source, from): (&[Node], &[usize]),
    state: &mut Cycle,
    (waited, log): (&mut bool, &mut Vec<(Node, f32)>),
) -> Took {
    let roots: Vec<Node> = source.iter().map(|&m| context.root(m)).collect();
    if let [root] = roots[..] {
        if let Some(j) = own(context, index, c, root) {
            return Took::Own(j);
        }
    }
    if roots.iter().any(|&m| own(context, index, c, m).is_some()) {
        return Took::Own(usize::MAX);
    }
    let moved: Vec<usize> =
        roots.iter().zip(source).filter(|(r, m)| r != m).flat_map(|(&r, _)| context.chains_of(r)).collect();
    let z: Vec<f64> = roots.iter().map(|&m| view.at(m)).collect();
    if z.iter().any(|v| v.is_nan()) {
        let pending = from.iter().chain(&moved).any(|&b| b == c || state.cycle && !state.solved.contains(&b));
        *waited |= pending && !second;
        return Took::Waits;
    }
    state.read.extend(roots.iter().copied());
    log.extend(roots.iter().zip(&z).map(|(&m, &v)| (m, v as f32)));
    Took::Height(z)
}

/// Solves one ground chain into the view; returns whether a read waited for a chain of its component.
fn solve_chain(
    context: &Context,
    view: &mut View,
    state: &mut Cycle,
    c: usize,
    second: bool,
    out: &mut Outcome,
) -> bool {
    let chain = &context.chains.chains[c];
    let nodes = Nodes::new(&chain.arms, context.segments, context.layout);
    let n = nodes.xy.len();
    let index: HashMap<Node, usize> =
        nodes.names.iter().enumerate().flat_map(|(i, names)| names.iter().map(move |&m| (m, i))).collect();
    let h = context.terrain.heights(&nodes.xy);
    let l: Vec<f64> = (0..n - 1).map(|s| span(nodes.xy[s], nodes.xy[s + 1]).max(chain::MIN_STEP_M)).collect();
    let (mut fixed, mut cause) = (vec![f64::NAN; n], vec![None; n]);
    // In the second round each node that another chain has read keeps the height that was read, before any
    // read of this chain: a read aimed at such a node is left out.
    let mut frozen = vec![false; n];
    if second {
        for i in 0..n {
            if nodes.names[i].iter().any(|m| state.read.contains(m)) {
                (fixed[i], cause[i], frozen[i]) = (view.at(nodes.names[i][0]), Some(Fix::Frozen), true);
            }
        }
    }
    let mut skipped = 0;
    let mut waited = false;
    // Every read: its node and the height that it took.
    let mut log: Vec<(Node, f32)> = Vec::new();
    let mut take = |source: &[Node], from_chains: &[usize], state: &mut Cycle| {
        read(context, view, (c, second, &index), (source, from_chains), state, (&mut waited, &mut log))
    };
    // Reads of a node of this chain: (the node that reads, the node read).
    let mut deferred: Vec<(usize, usize)> = Vec::new();
    // The names of a junction that the through chain holds: that chain writes them.
    let mut borrowed: HashSet<Node> = HashSet::new();
    // A side read fixes the end of this chain whose arm is the side's: a chain that ends on its own middle,
    // or with both ends at one point, holds the name of that point twice.
    let last = chain.arms[chain.arms.len() - 1];
    let back = if last.1 > 0 {
        ((last.0 .0, last.0 .1, last.0 .2 + 1), -1)
    } else {
        ((last.0 .0, last.0 .1, last.0 .2 - 1), 1)
    };
    let end = |arm: Arm| {
        let name = (arm.0 .0, arm.0 .1, arm.0 .2, 0);
        (if arm == chain.arms[0] {
            Some(0)
        } else if arm == back {
            Some(n - 1)
        } else {
            None
        })
        .or(index.get(&name).copied())
    };
    for &(reader, from) in &context.side[c] {
        let (Some(i), source) = (end(reader), (from.0 .0, from.0 .1, from.0 .2, 0)) else {
            continue;
        };
        let b = context.chain_of_arm(from);
        if b != c {
            borrowed.extend(nodes.names[i].iter().copied().filter(|&m| context.chains_of(m).contains(&b)));
        }
        if frozen[i] {
            skipped += 1;
            continue;
        }
        match take(&[source], &[b], state) {
            Took::Height(z) => (fixed[i], cause[i]) = (z[0], Some(Fix::Read)),
            Took::Own(j) => {
                let root = context.root(source);
                let j = if j == i { (0..n).find(|&k| k != i && nodes.names[k].contains(&root)) } else { Some(j) };
                match j {
                    Some(j) if j != usize::MAX => deferred.push((i, j)),
                    _ => skipped += 1,
                }
            }
            Took::Waits => {}
        }
    }
    // The approach nodes of each side junction, from the junction outward. An approach holds only inside
    // the band: at the first node whose through height lies outside the band of its own terrain it ends,
    // and that node and the later ones stay free.
    let junctions: Vec<(usize, Arm)> =
        context.side[c].iter().filter_map(|&(reader, from)| end(reader).map(|j| (j, from))).collect();
    let mut approaches: BTreeMap<usize, Vec<(usize, Arm)>> = BTreeMap::new();
    for &(node, through) in &context.approach[c] {
        let Some(&i) = index.get(&node) else { continue };
        let at = junctions.iter().filter(|&&(_, from)| from == through).min_by_key(|&&(j, _)| j.abs_diff(i));
        if let Some(&(j, _)) = at {
            approaches.entry(j).or_default().push((i, through));
        }
    }
    let mut ended = Vec::new();
    for (j, mut approach) in approaches {
        approach.sort_by_key(|&(i, _)| i.abs_diff(j));
        for (i, through) in approach {
            if frozen[i] {
                skipped += 1;
                continue;
            }
            let b = context.chain_of_arm(through);
            let Some((pair, share)) = foot(context, nodes.xy[i], through, b) else { continue };
            let z = match take(&pair, &[b], state) {
                Took::Height(z) => z[0] + share * (z[1] - z[0]),
                Took::Own(_) => {
                    skipped += 1;
                    continue;
                }
                Took::Waits => continue,
            };
            if h[i].is_finite() && (z - h[i]).abs() > BAND_M {
                ended.push(((z - h[i]).abs() - BAND_M, degrees(nodes.xy[i])));
                break;
            }
            (fixed[i], cause[i]) = (z, Some(Fix::Read));
        }
    }
    for &(node, from, share) in &context.copies[c] {
        let Some(&i) = index.get(&node) else { continue };
        let b = context.chains_of(from)[0];
        if b == c || !context.leaders.contains(&(c, b)) {
            continue;
        }
        if !fixed[i].is_nan() {
            skipped += usize::from(frozen[i]);
            continue;
        }
        match take(&[from, context.layout.next(from)], &[b], state) {
            Took::Height(z) => (fixed[i], cause[i]) = (z[0] + f64::from(share) * (z[1] - z[0]), Some(Fix::Read)),
            Took::Own(_) => skipped += 1,
            Took::Waits => {}
        }
    }
    if chain.closed {
        // The last node of a closed chain reads its first.
        deferred.push((n - 1, 0));
    }
    // A node on a point that a structure runs through takes the structure's height there.
    let mut hangers: Vec<(usize, f64)> = Vec::new();
    for &(mine, from, _) in &context.points[c] {
        let Some(&i) = index.get(&mine) else { continue };
        if frozen[i] {
            skipped += 1;
            continue;
        }
        match take(&[from], &context.chains_of(from), state) {
            Took::Height(z) => {
                (fixed[i], cause[i]) = (z[0], Some(Fix::Read));
                hangers.push((i, z[0]));
            }
            Took::Own(_) => skipped += 1,
            Took::Waits => {}
        }
    }
    // A ground pass holds whole or not at all: its bound must lie inside the band of both nodes of the
    // reader's crossing segment. The other way's height is read either way, as the decision rests on it.
    let (mut pass_lo, mut pass_hi) = (vec![f64::NEG_INFINITY; n], vec![f64::INFINITY; n]);
    let mut passed = vec![false; n];
    let (mut held, mut dropped, mut left_out) = (0, Vec::new(), Vec::new());
    let mut held_passes = Vec::new();
    for &k in &context.passes[c] {
        let (upper, lower, u, x, need, upper_reads) = context.facts.passes[k];
        let pair = |m: Node| [m, context.layout.next(m)];
        let on = |m: Node| pair(m).map(|m| index.get(&m).copied());
        let (mine, other, other_share) = match (upper_reads, on(upper), on(lower)) {
            (true, [Some(a), Some(b)], _) => ([a, b], lower, x),
            (false, _, [Some(a), Some(b)]) => ([a, b], upper, u),
            _ => continue,
        };
        let b = context.chains_of(other)[0];
        let z = match take(&pair(other), &[b], state) {
            Took::Height(z) => z,
            Took::Own(_) => {
                // The other way's crossing segment is this chain itself, or ends on it there.
                skipped += 1;
                left_out.push((0, degrees(nodes.xy[mine[0]])));
                continue;
            }
            Took::Waits => {
                left_out.push((1, degrees(nodes.xy[mine[0]])));
                continue;
            }
        };
        let (at, need) = (z[0] + f64::from(other_share) * (z[1] - z[0]), f64::from(need));
        let bound = if upper_reads { at + need } else { at - need };
        let past = |i: usize| if upper_reads { bound - (h[i] + BAND_M) } else { h[i] - BAND_M - bound };
        // A crossing node that a read fixes holds its height: the pass must not break it either.
        let broken = |i: usize| if upper_reads { bound - fixed[i] } else { fixed[i] - bound } - 1e-9;
        let miss = mine
            .iter()
            .flat_map(|&i| [(h[i].is_finite(), past(i)), (fixed[i].is_finite(), broken(i))])
            .filter(|&(applies, _)| applies)
            .map(|(_, m)| m)
            .fold(f64::NEG_INFINITY, f64::max);
        if miss > 0.0 {
            let rules =
                [context.features[nodes.feature[mine[0].min(n - 2)]].rule, context.features[other.0 as usize].rule];
            dropped.push(Dropped { miss, need, place: degrees(nodes.xy[mine[0]]), rules });
            continue;
        }
        held += 1;
        held_passes.push(k);
        for i in mine {
            passed[i] = true;
            if upper_reads {
                pass_lo[i] = pass_lo[i].max(bound);
            } else {
                pass_hi[i] = pass_hi[i].min(bound);
            }
        }
    }
    // A read of a node of this chain takes the height that the node has in this solve: fixed by a read, or
    // its height so far, which it then keeps. A shared point so keeps one height.
    for (i, j) in deferred {
        if frozen[i] {
            skipped += 1;
            continue;
        }
        let v = if fixed[j].is_nan() { view.at(nodes.names[j][0]) } else { fixed[j] };
        if v.is_nan() {
            waited |= !second;
            continue;
        }
        if fixed[j].is_nan() {
            (fixed[j], cause[j]) = (v, Some(Fix::Frozen));
        }
        (fixed[i], cause[i]) = (v, Some(Fix::Read));
        state.read.insert(nodes.names[j][0]);
        log.push((nodes.names[j][0], v as f32));
    }
    out.reads.extend(log.into_iter().map(|(m, v)| (c, m, v)));
    // Each piece between nodes without terrain is a chain of its own. A join that a piece cannot reach
    // is dropped: its node has no height and cuts the chain, and the piece is solved again without it.
    let mut broken = Vec::new();
    let (mut h, mut joins_dropped, mut joined) = (h, Vec::new(), Vec::new());
    let mut start = 0;
    let mut z = vec![f64::NAN; n];
    while start < n {
        if !h[start].is_finite() {
            start += 1;
            continue;
        }
        let mut end = start;
        while end + 1 < n && h[end + 1].is_finite() {
            end += 1;
        }
        if end > start {
            let joins: Vec<(usize, f64)> =
                hangers.iter().copied().filter(|&(j, _)| (start..=end).contains(&j)).collect();
            let last = second || !waited;
            let piece = Span { context, nodes: &nodes, c, range: start..end + 1, h: &h, l: &l, joins: &joins, last };
            let mut dropped = Vec::new();
            let solved = piece.solve(
                &fixed,
                &cause,
                (&pass_lo, &pass_hi, &passed),
                state,
                (second, &mut broken, &mut dropped, &mut joined),
                out,
            );
            if !dropped.is_empty() {
                for j in dropped {
                    let at = hangers.iter().position(|&(i, _)| i == j).expect("a dropped join is a hanger");
                    let (_, zj) = hangers.remove(at);
                    let rule = context.features[nodes.feature[j.min(n - 2)]].rule;
                    joins_dropped.push(((zj - h[j]).abs(), rule, degrees(nodes.xy[j])));
                    (h[j], fixed[j], cause[j]) = (f64::NAN, f64::NAN, None);
                }
                continue;
            }
            z[start..=end].copy_from_slice(&solved);
        }
        start = end + 1;
    }
    for (i, names) in nodes.names.iter().enumerate() {
        let v = z[i] as f32;
        for &m in names.iter().filter(|m| !borrowed.contains(m)) {
            view.set(m, v);
        }
    }
    // The numbers of a chain from its last solve.
    if second || !waited {
        out.bounds.extend(held_passes.into_iter().map(Held::Pass));
        // The ground gate: a node more than GATE_M off the terrain counts, but within HANG_M of a held join.
        let along: Vec<f64> = std::iter::once(0.0)
            .chain(l.iter().scan(0.0, |x, l| {
                *x += l;
                Some(*x)
            }))
            .collect();
        let near_join = |i: usize| hangers.iter().any(|&(j, _)| (along[i] - along[j]).abs() <= HANG_M);
        for i in (0..n).filter(|&i| (z[i] - h[i]).abs() > crate::gate::GATE_M && !near_join(i)) {
            out.metrics.ground_gate += 1;
            out.metrics.ground_gate_all.push((z[i] - h[i], degrees(nodes.xy[i])));
        }
        measure(context, (c, &nodes), &l, &z, &h, state.cycle, out);
        out.metrics.approach_ended += ended.len();
        out.metrics.approach_ended_worst.extend(ended);
        out.metrics.passes_held += held;
        out.metrics.passes_dropped += dropped.len();
        for pass in &dropped {
            *out.metrics.passes_dropped_by_need.entry((pass.need * 10.0).round() as u32).or_default() += 1;
        }
        out.metrics.passes_dropped_worst.extend(dropped);
        out.metrics.passes_left_out += left_out.len();
        out.metrics.passes_left_out_worst.extend(left_out);
        out.metrics.joins_dropped += joins_dropped.len();
        out.metrics.joins_dropped_all.extend(joins_dropped);
        out.metrics.joins_held += joined.len();
        out.metrics.joins_held_worst.extend(joined);
        out.metrics.grade_broken += broken.len();
        out.metrics.grade_broken_m += broken.iter().map(|b: &Broken| b.metres).sum::<f64>();
        out.metrics.grade_broken_worst.extend(broken);
        out.metrics.reads_skipped += skipped;
    }
    out.metrics.chains += usize::from(!second);
    waited
}

/// The foot of a point on the through chain `b` within the approach distance of the junction at arm
/// `through`: the two nodes of its segment there and the share along it.
fn foot(context: &Context, point: [f64; 2], through: Arm, b: usize) -> Option<([Node; 2], f64)> {
    let chain = &context.chains.chains[b];
    let j = context.place_of[context.segments.end(through).0].1 as usize;
    let arms = &chain.arms[j.saturating_sub(2)..(j + 3).min(chain.arms.len())];
    let nodes = Nodes::new(arms, context.segments, context.layout);
    let junction = (through.0 .0, through.0 .1, through.0 .2, 0);
    let at = nodes.names.iter().position(|names| names.contains(&junction))?;
    let rule = context.features[nodes.feature[at.min(nodes.feature.len() - 1)]].rule;
    let reach = if rule >= context.classes.path { APPROACH_PATH_M } else { APPROACH_M };
    let unit = metres(point[1]);
    let mut best: Option<(f64, [Node; 2], f64)> = None;
    let mut consider = |s: usize| {
        let (a, q) = (nodes.xy[s], nodes.xy[s + 1]);
        let d = sub(q, a);
        let t = (((point[0] - a[0]) * d[0] + (point[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-300))
            .clamp(0.0, 1.0);
        let gap = hypot([point[0] - a[0] - t * d[0], point[1] - a[1] - t * d[1]]) * unit;
        if best.is_none_or(|(g, _, _)| gap < g) {
            best = Some((gap, [nodes.names[s][0], nodes.names[s + 1][0]], t));
        }
    };
    let (mut along, mut s) = (0.0, at);
    while s > 0 && along < reach {
        consider(s - 1);
        along += span(nodes.xy[s - 1], nodes.xy[s]);
        s -= 1;
    }
    let (mut along, mut s) = (0.0, at);
    while s + 1 < nodes.xy.len() && along < reach {
        consider(s);
        along += span(nodes.xy[s], nodes.xy[s + 1]);
        s += 1;
    }
    best.map(|(_, pair, t)| (pair, t))
}

/// A piece of a chain between nodes without terrain.
struct Span<'a> {
    context: &'a Context<'a>,
    nodes: &'a Nodes,
    c: usize,
    range: std::ops::Range<usize>,
    h: &'a [f64],
    l: &'a [f64],
    /// The joins on structures in the piece: the node and the structure's height there.
    joins: &'a [(usize, f64)],
    /// Its solve is the chain's last: it records the bounds that it holds.
    last: bool,
}

impl Span<'_> {
    fn solve(
        &self,
        fixed: &[f64],
        cause: &[Option<Fix>],
        (pass_lo, pass_hi, passed): (&[f64], &[f64], &[bool]),
        state: &mut Cycle,
        (second, broken, dropped, joined): (bool, &mut Vec<Broken>, &mut Vec<usize>, &mut Vec<Join>),
        out: &mut Outcome,
    ) -> Vec<f64> {
        let context = self.context;
        let r = self.range.clone();
        let n = r.len();
        let h = &self.h[r.clone()];
        let l = &self.l[r.start..r.end - 1];
        let fixed = &fixed[r.clone()];
        // A free end of an open chain weighs as a full node, as if the chain ran on: with half a node, the
        // bend term pulls the end off the terrain.
        let open = !context.chains.chains[self.c].closed;
        let (first, last) = (open && r.start == 0, open && r.end == self.h.len());
        let metres_at = |i: usize| {
            let before = if i > 0 || first { l[i.max(1) - 1] } else { 0.0 };
            let after = if i + 1 < n || last { l[i.min(n - 2)] } else { 0.0 };
            (before + after) / 2.0
        };
        let class = |s: usize| &context.classes.rows[context.features[self.nodes.feature[r.start + s]].rule];
        let bend: Vec<f64> = (0..n).map(|i| class(i.min(n - 2)).smooth_m.powi(4)).collect();
        let slope = vec![0.0; n - 1];
        let metres: Vec<f64> = (0..n).map(metres_at).collect();
        let soft = chain::Chain { l, fit: &metres, target: h, slope: &slope, bend: &bend };
        let weights = match state.weights.get(&self.key()) {
            Some(w) if second => w.clone(),
            _ => {
                let z0 = match chain::plain(&soft, fixed) {
                    Ok(z0) => z0,
                    Err(_) => {
                        out.metrics.pivots += 1;
                        return fixed.to_vec();
                    }
                };
                let w: Vec<f64> =
                    (0..n).map(|i| metres[i] * (HUBER_M / (z0[i] - h[i]).abs().max(1e-300)).min(1.0)).collect();
                state.weights.insert(self.key(), w.clone());
                w
            }
        };
        let soft = chain::Chain { fit: &weights, ..soft };
        let z0 = match chain::plain(&soft, fixed) {
            Ok(z0) => z0,
            Err(_) => {
                out.metrics.pivots += 1;
                return fixed.to_vec();
            }
        };
        let mut band: (Vec<f64>, Vec<f64>) =
            (h.iter().map(|h| h - BAND_M).collect(), h.iter().map(|h| h + BAND_M).collect());
        // The zone of a join: within HANG_M of it the band widens to the 1:1 cone from the join, and every
        // segment with a node in it, the one that leaves it too, is no steeper than 1:1.
        let along: Vec<f64> = std::iter::once(0.0)
            .chain(l.iter().scan(0.0, |x, l| {
                *x += l;
                Some(*x)
            }))
            .collect();
        let mut zone: Vec<Option<usize>> = vec![None; n];
        for (k, &(j, zj)) in self.joins.iter().enumerate() {
            let j = j - r.start;
            for i in 0..n {
                let d = (along[i] - along[j]).abs();
                if d <= HANG_M {
                    band.0[i] = band.0[i].min(zj - d);
                    band.1[i] = band.1[i].max(zj + d);
                    zone[i] = Some(k);
                }
            }
        }
        let mut lo3 = vec![f64::NEG_INFINITY; n];
        let mut covered = vec![false; n];
        let mut covers = Vec::new();
        let local = |m: Node| self.nodes.names[r.clone()].iter().position(|names| names.contains(&m));
        for &k in &context.covers[self.c] {
            let (node, least, lift, _) = context.facts.covers[k];
            let (Some(a), Some(b)) = (local(node), local(context.layout.next(node))) else { continue };
            let up = least - z0[a].min(z0[b]);
            if up <= lift {
                lo3[a] = lo3[a].max(least);
                lo3[b] = lo3[b].max(least);
                covered[a] = true;
                out.covered.push((node, up));
                covers.push(k);
            }
        }
        let (mut slo, mut shi) = (vec![f64::NEG_INFINITY; n - 1], vec![f64::INFINITY; n - 1]);
        for s in 0..n - 1 {
            let first = self.nodes.first[r.start + s];
            if !fixed[s].is_nan() && !fixed[s + 1].is_nan() {
                continue;
            }
            if zone[s].is_some() || zone[s + 1].is_some() {
                (slo[s], shi[s]) = (-HANG_GRADE, HANG_GRADE);
            } else if context.ungraded.contains(&first) {
                continue;
            } else if context.ramps.contains(&first) {
                let rise = (h[s + 1] - h[s]) / l[s];
                (slo[s], shi[s]) = (rise - RAMP_GRADE, rise + RAMP_GRADE);
            } else if class(s).grade.is_finite() {
                (slo[s], shi[s]) = (-class(s).grade, class(s).grade);
            }
        }
        let infinite = vec![f64::INFINITY; n];
        let steps = [
            Step::Fixed(fixed),
            Step::Bounds { layer: 1, lo: &band.0, hi: &band.1 },
            Step::Bounds { layer: 2, lo: &pass_lo[r.clone()], hi: &pass_hi[r.clone()] },
            Step::Bounds { layer: 3, lo: &lo3, hi: &infinite },
            Step::Slopes { layer: 4, lo: &slo, hi: &shi },
        ];
        let tube = chain::tube(&soft, &steps);
        // A join is out of reach where the string opens a segment with a node in its zone.
        for o in tube.opened.iter().filter(|o| o.layer == 4) {
            for k in [zone[o.segment], zone[o.segment + 1]].into_iter().flatten() {
                let j = self.joins[k].0;
                if !dropped.contains(&j) {
                    dropped.push(j);
                }
            }
        }
        if !dropped.is_empty() {
            return z0;
        }
        let profile = match chain::profile(&soft, &tube, None) {
            Ok(p) => p,
            Err(_) => {
                out.metrics.pivots += 1;
                return z0;
            }
        };
        if self.last {
            out.bounds.extend(covers.into_iter().map(Held::Cover));
        }
        for &(j, zj) in self.joins {
            let rule = context.features[self.nodes.feature[j.min(self.nodes.feature.len() - 1)]].rule;
            joined.push(((zj - h[j - r.start]).abs(), rule, degrees(self.nodes.xy[j])));
        }
        out.metrics.joins_zone_off +=
            (0..n).filter(|&i| zone[i].is_some() && (profile.z[i] - h[i]).abs() > BAND_M).count();
        let m = &mut out.metrics;
        m.plain += usize::from(profile.plain);
        m.steps += profile.steps;
        m.steps_max = m.steps_max.max(profile.steps);
        m.unconverged += usize::from(!profile.converged);
        for cut in &tube.cuts {
            m.cuts[cut.layer as usize] += 1;
            m.cut_m[cut.layer as usize] += cut.metres;
            m.cut_worst.push((cut.metres, cut.layer, degrees(self.nodes.xy[r.start + cut.node])));
        }
        for open in &tube.opened {
            m.openings[open.layer as usize] += 1;
            m.excess_max = m.excess_max.max(open.excess);
            // An opened class grade whose bound is now over twice the grade.
            let (s, grade) = (open.segment, class(open.segment).grade);
            let slope = (tube.b[s].max(-tube.a[s])) / l[s];
            if grade.is_finite() && shi[s] == grade && slope > 2.0 * grade {
                let place = degrees(self.nodes.xy[r.start + s]);
                broken.push(Broken {
                    slope,
                    rule: context.features[self.nodes.feature[r.start + s]].rule,
                    place,
                    metres: l[s],
                });
            }
        }
        // An opened run of segments: its cause by its end nodes.
        let z = &profile.z;
        let opened: HashSet<usize> = tube.opened.iter().map(|o| o.segment).collect();
        let mut s = 0;
        while s < n - 1 {
            if !opened.contains(&s) {
                s += 1;
                continue;
            }
            let mut e = s;
            while e + 1 < n - 1 && opened.contains(&(e + 1)) {
                e += 1;
            }
            let ends = [cause[r.start + s], cause[r.start + e + 1]];
            let k = if ends.contains(&Some(Fix::Frozen)) {
                2
            } else if ends.contains(&Some(Fix::Read)) {
                1
            } else {
                0
            };
            m.opened_m[k] += l[s..=e].iter().sum::<f64>();
            for i in [s, e + 1] {
                if i > 0 && i < n - 1 {
                    out.kinks[2].push(((z[i + 1] - z[i]) / l[i] - (z[i] - z[i - 1]) / l[i - 1]).abs());
                }
            }
            s = e + 1;
        }
        for i in 0..n {
            let change = |i: usize| ((z[i + 1] - z[i]) / l[i] - (z[i] - z[i - 1]) / l[i - 1]).abs();
            if i > 0 && i < n - 1 {
                match cause[r.start + i] {
                    Some(Fix::Read) => out.kinks[0].push(change(i)),
                    Some(Fix::Frozen) => {
                        out.kinks[1].push(change(i));
                        let ll = degrees(self.nodes.xy[r.start + i]);
                        out.metrics.frozen_worst.push((change(i), ll));
                    }
                    None => {}
                }
            }
            if (z[i] - h[i]).abs() > OFF_M {
                let k = if cause[r.start + i].is_some() {
                    1
                } else if passed[r.start + i] {
                    2
                } else if covered[i] {
                    3
                } else {
                    0
                };
                out.metrics.off[k] += 1;
                out.metrics.off_band[k] += usize::from((z[i] - h[i]).abs() > BAND_M);
            }
            if profile.at_lo[i] || profile.at_hi[i] {
                out.held.extend(self.nodes.names[r.start + i].iter().copied());
            }
        }
        #[cfg(test)]
        out.pieces.push(Piece {
            h: h.to_vec(),
            l: l.to_vec(),
            tube,
            z: z.clone(),
            structure: false,
            plain: profile.plain,
        });
        profile.z
    }

    fn key(&self) -> usize {
        self.c * 1_000_003 + self.range.start
    }
}

/// The numbers of a solved chain against its class, split by whether it lies in a cycle.
fn measure(
    context: &Context,
    (nodes_chain, nodes): (usize, &Nodes),
    l: &[f64],
    z: &[f64],
    h: &[f64],
    cycle: bool,
    out: &mut Outcome,
) {
    let k = usize::from(!cycle);
    let n = z.len();
    let rise = |s: usize| (z[s + 1] - z[s]) / l[s];
    out.metrics.split_km[k] += context.chains.chains[nodes_chain].metres / 1000.0;
    for s in 0..n - 1 {
        if !(z[s].is_finite() && z[s + 1].is_finite() && h[s].is_finite()) {
            continue;
        }
        let grade = context.classes.rows[context.features[nodes.feature[s]].rule].grade;
        if grade.is_finite() && rise(s).abs() > grade + 0.01 {
            out.metrics.split_grade_m[k] += l[s];
        }
        if s > 0 && z[s - 1].is_finite() && (rise(s) - rise(s - 1)).abs() > KINK {
            out.metrics.split_kink_m[k] += l[s];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::area::{area_of, each_area, Area};
    use super::super::chains::{area_facts, assemble};
    use super::super::network::Network;
    use super::super::tests::{bridge, corner_network, engelberg, plain, table, tunnel, way};
    use super::super::{River, CLEAR_PATH_M, DECK_M};
    use super::*;
    use std::collections::BTreeSet;

    /// Phases A and B of features on one terrain, by area, on `threads` threads.
    fn ground(features: &[Feature], terrain: &dyn Terrain, threads: usize) -> (Heights, Chains) {
        over(features, terrain, &[], threads)
    }

    /// As `ground`, with rivers.
    fn over(features: &[Feature], terrain: &dyn Terrain, rivers: &[River], threads: usize) -> (Heights, Chains) {
        let refs: Vec<&Feature> = features.iter().collect();
        let near: Vec<&River> = rivers.iter().collect();
        let classes = table().1;
        let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
            let net = Network::new(subset, &classes).unwrap();
            area_facts(&net, &classes, terrain, (&near, &[]), chosen, &|xy| area_of(xy) == area)
        };
        let facts = AreaFacts::join(each_area(&refs, &[], false, (usize::MAX, 4), &job));
        let chains = assemble(&refs, &facts.facts);
        let heights = Heights::solve(&refs, &classes, terrain, &chains, &facts, threads);
        // Every held bound is met on the stored heights.
        assert_eq!(heights.metrics.held_unmet, 0, "{:?}", heights.metrics.held_unmet_all);
        (heights, chains)
    }

    /// The stored heights of every node of a feature's first part.
    fn of(heights: &Heights, f: u32) -> Vec<f32> {
        heights.z[f as usize][0].clone()
    }

    /// Noise in [−amplitude, amplitude] from a point, the same on every run.
    fn noise(amplitude: f64) -> impl Fn(f64, f64) -> f64 + Sync + Copy {
        move |e: f64, n: f64| {
            let k = ((e * 7.31 + n * 3.17).abs() / 0.5) as u64;
            ((k.wrapping_mul(2654435761) % 1000) as f64 / 500.0 - 1.0) * amplitude
        }
    }

    #[test]
    fn a_through_road_does_not_see_its_side_roads() {
        let place = engelberg();
        let rough = noise(0.8);
        let terrain = place.terrain(move |e, n| 1000.0 + 0.06 * e + rough(e, n));
        let road = way("tertiary", place.line(&[(0.0, 0.0), (400.0, 0.0)], Some(20.0)), plain);
        let alone = ground(std::slice::from_ref(&road), &terrain, 1).0;
        let mut features = vec![road];
        for (k, e) in [60.0, 160.0, 260.0].into_iter().enumerate() {
            let north = if k % 2 == 0 { 120.0 } else { -120.0 };
            features.push(way("residential", place.line(&[(e, 0.0), (e + 30.0, north)], Some(20.0)), plain));
        }
        for e in [100.0, 320.0] {
            features.push(way("path", place.line(&[(e, 0.0), (e - 40.0, 80.0)], Some(20.0)), plain));
        }
        let with = ground(&features, &terrain, 1).0;
        let (a, b) = (of(&alone, 0), of(&with, 0));
        assert!(a.iter().zip(&b).all(|(a, b)| a.to_bits() == b.to_bits()), "{a:?}\n{b:?}");
    }

    #[test]
    fn the_heights_do_not_depend_on_the_areas() {
        let place = engelberg();
        let (features, _, terrain) = corner_network(&place);
        let refs: Vec<&Feature> = features.iter().collect();
        let classes = table().1;
        let net = Network::new(&refs, &classes).unwrap();
        let facts = AreaFacts::join(vec![area_facts(
            &net,
            &classes,
            &terrain,
            (&[], &[]),
            &(0..refs.len()).collect::<Vec<_>>(),
            &|_| true,
        )]);
        let chains = assemble(&refs, &facts.facts);
        let whole = Heights::solve(&refs, &classes, &terrain, &chains, &facts, 1);
        let (apart, _) = ground(&features, &terrain, 4);
        assert!(whole.z.iter().flatten().flatten().any(|v| v.is_finite()));
        let bits = |h: &Heights| h.z.iter().flatten().flatten().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&whole), bits(&apart));
        assert!(whole.held.iter().all(|&m| apart.held(m)) && whole.held.len() == apart.held.len());
        assert!(whole.covered.keys().all(|&m| apart.covered(m)) && whole.covered.len() == apart.covered.len());
        assert_eq!((whole.metrics.chains, whole.metrics.steps), (apart.metrics.chains, apart.metrics.steps));
    }

    #[test]
    fn a_long_climb_has_no_step_at_an_area_border() {
        let place = engelberg();
        let n = (1u64 << super::super::area::AREA_ZOOM) as f64;
        let corner = (((place.x0 * n).floor() + 1.0) / n - place.x0) / place.unit;
        let corner = (corner, (place.y0 - (place.y0 * n).floor() / n) / place.unit);
        let rough = noise(1.0);
        let start = (corner.0 - 1250.0, corner.1 - 600.0);
        let (de, dn) = (2500.0 * 0.9, 2500.0 * 0.436);
        let s_of = move |e: f64, n: f64| ((e - start.0) * 0.9 + (n - start.1) * 0.436).max(0.0);
        let terrain = place.terrain(move |e, n| 1000.0 + 0.124 * s_of(e, n) + rough(e, n));
        let road = way("primary", place.line(&[start, (start.0 + de, start.1 + dn)], Some(50.0)), plain);
        let areas: HashSet<(i64, i64)> = road.parts[0].iter().map(|&p| area_of(p)).collect();
        assert!(areas.len() >= 2, "the climb crosses an area border");
        let (heights, _) = ground(&[road], &terrain, 2);
        let grade = table().1.rows[table().0.iter().position(|c| c == "primary").unwrap()].grade;
        let mut worst = 0.0f64;
        for piece in &heights.pieces {
            for s in 0..piece.l.len() {
                let rise = piece.z[s + 1] - piece.z[s];
                assert!(rise <= piece.tube.b[s] + 1e-9 && rise >= piece.tube.a[s] - 1e-9);
                let opened = piece.tube.opened.iter().any(|o| o.segment == s);
                assert!(opened || rise.abs() <= grade * piece.l[s] + 1e-9);
                if s > 0 {
                    worst = worst.max((rise / piece.l[s] - (piece.z[s] - piece.z[s - 1]) / piece.l[s - 1]).abs());
                }
            }
        }
        assert!(worst < 0.02, "{worst}");
    }

    #[test]
    fn a_shared_point_has_one_height() {
        let place = engelberg();
        let terrain = place.terrain(|e, n| 1000.0 + 0.03 * e + 0.02 * n);
        let features = [
            way("residential", place.line(&[(-50.0, 0.0), (0.0, 0.0), (50.0, 0.0), (50.0, 40.0)], Some(10.0)), plain),
            way("residential", place.line(&[(100.0, 40.0), (50.0, 40.0), (0.0, 40.0), (0.0, 0.0)], Some(10.0)), plain),
            way(
                "residential",
                place.line(&[(200.0, 0.0), (250.0, 0.0), (270.0, 20.0), (250.0, 40.0), (250.0, 0.0)], Some(10.0)),
                plain,
            ),
            way(
                "tertiary",
                place.line(&[(400.0, 0.0), (420.0, 0.0), (420.0, 20.0), (400.0, 20.0), (400.0, 0.0)], Some(5.0)),
                plain,
            ),
            way("residential", place.line(&[(350.0, 0.0), (400.0, 0.0)], Some(10.0)), plain),
            way("residential", place.line(&[(470.0, 20.0), (420.0, 20.0)], Some(10.0)), plain),
            // A way that ends on its own middle where its end runs straight on from its start: its middle
            // leaves the point as the side.
            way(
                "residential",
                place.line(&[(600.0, 0.0), (650.0, 0.0), (650.0, 40.0), (670.0, 20.0), (650.0, 0.0)], Some(10.0)),
                plain,
            ),
        ];
        let (heights, _) = ground(&features, &terrain, 1);
        let mut at: HashMap<[u64; 2], Vec<u32>> = HashMap::new();
        for (f, feature) in features.iter().enumerate() {
            let points = super::super::network::distinct(&feature.parts[0]);
            for (k, &p) in points.iter().enumerate() {
                at.entry(crate::geometry::key(p))
                    .or_default()
                    .push((heights.at((f as u32, 0, k as u32, 0)) as f32).to_bits());
            }
        }
        let shared: Vec<&Vec<u32>> = at.values().filter(|v| v.len() >= 2).collect();
        assert_eq!(shared.len(), 6);
        assert!(shared.iter().all(|v| v.iter().all(|&b| b == v[0])), "{shared:?}");
        assert_eq!(heights.metrics.shared_apart, 0);
    }

    #[test]
    fn a_double_track_lies_at_one_height() {
        let place = engelberg();
        // Noise along the line: the two tracks see one ground, so that each node beside the other track copies.
        let rough = noise(0.3);
        let terrain = place.terrain(move |e, _| 1000.0 + 0.01 * e + rough(e, 0.0));
        let track = |from: (f64, f64), to: (f64, f64)| way("rail", place.line(&[from, to], Some(20.0)), plain);
        let features = [
            track((0.0, 0.0), (200.0, 0.0)),
            track((0.0, 4.0), (200.0, 4.0)),
            track((200.0, 4.0), (400.0, 4.0)),
            track((200.0, 0.0), (400.0, 0.0)),
        ];
        let refs: Vec<&Feature> = features.iter().collect();
        let classes = table().1;
        let net = Network::new(&refs, &classes).unwrap();
        let facts = AreaFacts::join(vec![area_facts(&net, &classes, &terrain, (&[], &[]), &[0, 1, 2, 3], &|_| true)]);
        let chains = assemble(&refs, &facts.facts);
        let heights = Heights::solve(&refs, &classes, &terrain, &chains, &facts, 1);
        let leaders: HashSet<(usize, usize)> =
            chains.reads.iter().filter(|r| r.2 == ReadKind::Copy).map(|r| (r.0, r.1)).collect();
        let segments = Segments::new(&refs);
        let layout = Layout::new(&refs, &segments);
        let chain_of = |m: Node| {
            let s = segments.end(((m.0, m.1, m.2), 1)).0;
            chains.chains.iter().position(|c| c.arms.iter().any(|&a| segments.end(a).0 == s)).unwrap()
        };
        let mut checked = 0;
        for &(node, from, share) in &facts.copies {
            if node.3 == 0 && node.2 as usize + 1 == layout.first[node.0 as usize][0].len() - 1 {
                continue;
            }
            if leaders.contains(&(chain_of(node), chain_of(from))) {
                let foot = heights.at(from) + f64::from(share) * (heights.at(layout.next(from)) - heights.at(from));
                assert!((heights.at(node) - foot).abs() <= 1e-4, "{node:?}");
                checked += 1;
            }
        }
        assert!(checked >= 190, "{checked}");
        // Where a track is split, its two ways share the height of the point and run on without a step.
        for (a, b) in [(0u32, 3u32), (1, 2)] {
            let (za, zb) = (of(&heights, a), of(&heights, b));
            assert_eq!(za[za.len() - 1].to_bits(), zb[0].to_bits());
            assert!(((za[za.len() - 1] - za[za.len() - 2]) - (zb[1] - zb[0])).abs() <= 0.01);
        }
    }

    #[test]
    fn the_grade_holds_up_to_the_band_and_opens_past_it() {
        let place = engelberg();
        let rough = noise(0.5);
        let grade = table().1.rows[table().0.iter().position(|c| c == "tertiary").unwrap()].grade;
        // 300 m and 600 m of 15.5 % fall 1.5 m and 3 m behind a grade of 15 %: the band holds that. 2.5 km
        // fall 12.5 m behind: past the band.
        for (length, noisy, opens) in [(300.0, false, false), (600.0, true, false), (2500.0, true, true)] {
            let terrain = place.terrain(move |e, n| 1000.0 + 0.155 * e + if noisy { rough(e, n) } else { 0.0 });
            let road = way("tertiary", place.line(&[(0.0, 0.0), (length, 0.0)], Some(20.0)), plain);
            let (heights, _) = ground(&[road], &terrain, 1);
            for piece in &heights.pieces {
                assert!(piece.z.iter().zip(&piece.h).all(|(z, h)| (z - h).abs() <= BAND_M + 1e-9));
                for s in 0..piece.l.len() {
                    let rise = piece.z[s + 1] - piece.z[s];
                    match piece.tube.opened.iter().find(|o| o.segment == s) {
                        None => assert!(rise.abs() <= grade * piece.l[s] + 1e-9, "{length}: {s}"),
                        Some(o) => {
                            let string = (grade + o.excess) * piece.l[s];
                            assert!(rise.abs() <= 1.1 * string + 1e-6 * piece.l[s] + 1e-9, "{length}: {s}");
                        }
                    }
                }
            }
            let opened: usize = heights.pieces.iter().map(|p| p.tube.opened.len()).sum();
            assert_eq!(opened > 0, opens, "{length} m: {opened} opened");
        }
    }

    /// A path from the middle of a level road northward on terrain that rises by `rise(north)` past the road;
    /// the ground heights with and without the path.
    fn approach_case(rise: fn(f64) -> f64) -> (Heights, Heights) {
        let place = engelberg();
        let terrain = place.terrain(move |_, n| if n <= 0.0 { 1000.0 } else { 1000.0 + rise(n) });
        let road = way("unclassified", place.line(&[(-100.0, 0.0), (100.0, 0.0)], Some(10.0)), plain);
        let path = way("path", place.line(&[(0.0, 0.0), (0.0, 60.0)], None), plain);
        let alone = ground(std::slice::from_ref(&road), &terrain, 1).0;
        (alone, ground(&[road, path], &terrain, 1).0)
    }

    /// The height of the road at (0, 0) and the heights of the path's nodes, every 2 m from it.
    fn approach_heights(heights: &Heights) -> (f64, Vec<f64>) {
        (heights.at((0, 0, 10, 0)), (0..=30).map(|c| heights.at((1, 0, 0, c))).collect())
    }

    #[test]
    fn an_approach_ends_where_the_through_road_leaves_the_band_of_the_sides_terrain() {
        let rise = |n: f64| (3.5 * n).min(20.0);
        let (alone, with) = approach_case(rise);
        let (road, path) = approach_heights(&with);
        assert_eq!(path[0].to_bits(), road.to_bits());
        assert!(path[1] >= 1002.0 && path[2] >= 1009.0, "{path:?}");
        for (k, z) in path.iter().enumerate().skip(1) {
            assert!((z - 1000.0 - rise(2.0 * k as f64)).abs() <= BAND_M, "{k}: {z}");
        }
        assert_eq!(with.metrics.approach_ended, 1);
        let bits = |h: &Heights| of(h, 0).iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&alone), bits(&with));
    }

    #[test]
    fn an_approach_inside_the_band_stays_on_the_through_road() {
        let (_, with) = approach_case(|n| n.min(20.0));
        let (road, path) = approach_heights(&with);
        assert!((path[1] - road).abs() < 1e-4 && (path[2] - road).abs() < 1e-4, "{road} {path:?}");
        assert_eq!(with.metrics.approach_ended, 0);
    }

    #[test]
    fn an_approach_ends_at_its_first_node_outside_the_band_not_after_it() {
        let rise = |n: f64| if n <= 3.0 { 1.0 } else { (1.0 + 4.5 * (n - 3.0)).min(20.0) };
        let (_, with) = approach_case(rise);
        let (road, path) = approach_heights(&with);
        assert!((path[1] - road).abs() < 1e-4, "{road} {path:?}");
        assert!((path[2] - 1000.0 - rise(4.0)).abs() <= BAND_M && (path[2] - road).abs() > 1e-3, "{path:?}");
        assert_eq!(with.metrics.approach_ended, 1);
    }

    /// A feature edit: a way on the ground with a layer tag.
    fn on_layer(layer: i64) -> impl FnOnce(&mut Feature) {
        move |f: &mut Feature| (f.layer, f.layered) = (layer, true)
    }

    fn flat(_: f64, _: f64) -> f64 {
        1000.0
    }

    /// The largest change of grade between two segments of a feature's node heights, 2 m apart.
    fn largest_change(z: &[f32]) -> f64 {
        z.windows(3)
            .map(|w| (f64::from(w[2]) - 2.0 * f64::from(w[1]) + f64::from(w[0])).abs() / 2.0)
            .fold(0.0, f64::max)
    }

    #[test]
    fn a_ground_pass_that_does_not_fit_in_the_band_is_dropped() {
        let place = engelberg();
        let terrain = place.terrain(flat);
        // No vertex at north 0: a vertex that two ways share is a junction, not a crossing.
        let mut features = vec![way("service", place.line(&[(-50.0, 0.0), (50.0, 0.0)], Some(10.0)), on_layer(1))];
        for x in [-4.0, 0.0, 4.0] {
            let track = place.line(&[(x, -30.0), (x, -5.0), (x, 5.0), (x, 30.0)], Some(10.0));
            features.push(way("narrow_gauge", track, plain));
        }
        let (heights, _) = ground(&features, &terrain, 1);
        for f in 0..features.len() as u32 {
            assert!(of(&heights, f).iter().all(|z| (f64::from(*z) - 1000.0).abs() <= 0.05), "{f}");
        }
        assert_eq!((heights.metrics.passes_dropped, heights.metrics.passes_held), (3, 0));
        assert_eq!(heights.pieces.iter().map(|p| p.tube.opened.len()).sum::<usize>(), 0);
        assert!(largest_change(&of(&heights, 0)) < 0.01);
    }

    #[test]
    fn a_ground_pass_that_meets_a_fixed_node_is_dropped() {
        let place = engelberg();
        let terrain = place.terrain(flat);
        // The service way is a side of the road; the footway crosses it 2 m from the junction, on its approach.
        let features = [
            way("residential", place.line(&[(-60.0, 0.0), (0.0, 0.0), (60.0, 0.0)], None), plain),
            way("service", place.line(&[(0.0, 0.0), (0.0, -50.0)], None), on_layer(1)),
            way("footway", place.line(&[(-20.0, -2.0), (20.0, -2.0)], None), plain),
        ];
        let (heights, _) = ground(&features, &terrain, 1);
        assert_eq!((heights.metrics.passes_dropped, heights.metrics.passes_held), (1, 0));
        assert_eq!(heights.metrics.cuts[2], 0);
        for f in 0..3 {
            assert!(of(&heights, f).iter().all(|z| (f64::from(*z) - 1000.0).abs() <= 0.05), "{f}");
        }
    }

    #[test]
    fn a_ground_pass_that_fits_is_held_whole_and_the_way_reaches_it_at_its_grade() {
        let place = engelberg();
        let terrain = place.terrain(flat);
        let features = [
            way("service", place.line(&[(-80.0, 0.0), (80.0, 0.0)], None), on_layer(1)),
            way("footway", place.line(&[(0.0, -40.0), (0.0, 40.0)], None), plain),
        ];
        let (heights, _) = ground(&features, &terrain, 1);
        let (over, under) = (heights.at((0, 0, 0, 40)), heights.at((1, 0, 0, 20)));
        let need = CLEAR_PATH_M + DECK_M;
        assert!(over - under >= need - 1e-4 && over - under <= need + 0.3, "{over} {under}");
        assert!((under - 1000.0).abs() < 1e-6);
        let service = of(&heights, 0);
        for w in service.windows(2) {
            assert!((f64::from(w[1]) - f64::from(w[0])).abs() <= 0.25 * 2.0 + 1e-9, "{service:?}");
        }
        assert!(service.iter().all(|z| (f64::from(*z) - 1000.0).abs() <= BAND_M));
        assert_eq!(heights.metrics.passes_held, 1);
        assert_eq!(heights.metrics.cuts[2], 0);
    }

    #[test]
    fn the_way_whose_layer_lies_further_from_0_reads_and_with_equal_distance_the_worse_class() {
        let place = engelberg();
        let terrain = place.terrain(flat);
        let need = CLEAR_PATH_M + DECK_M;
        let steps = [
            way("steps", place.line(&[(0.0, -20.0), (0.0, 20.0)], None), plain),
            way("service", place.line(&[(-60.0, 0.0), (60.0, 0.0)], None), on_layer(1)),
        ];
        let (heights, _) = ground(&steps, &terrain, 1);
        let (under, over) = (heights.at((0, 0, 0, 10)), heights.at((1, 0, 0, 30)));
        assert!((under - 1000.0).abs() < 1e-6);
        assert!(over - under >= need - 1e-4 && over - under <= need + 0.3, "{over} {under}");
        let below = [
            way("footway", place.line(&[(0.0, -30.0), (0.0, 30.0)], None), on_layer(-1)),
            way("service", place.line(&[(-60.0, 0.0), (60.0, 0.0)], None), on_layer(1)),
        ];
        let (heights, _) = ground(&below, &terrain, 1);
        let (under, over) = (heights.at((0, 0, 0, 15)), heights.at((1, 0, 0, 30)));
        assert!(of(&heights, 1).iter().all(|z| (f64::from(*z) - 1000.0).abs() < 1e-6));
        assert!(over - under >= need - 1e-4, "{over} {under}");
        assert!(of(&heights, 0).iter().all(|z| (f64::from(*z) - 1000.0).abs() <= BAND_M));
    }

    #[test]
    fn three_layers_at_one_place_read_upward_and_make_no_cycle() {
        let place = engelberg();
        let terrain = place.terrain(flat);
        let features = [
            way("footway", place.line(&[(0.0, -40.0), (0.0, 40.0)], None), plain),
            way("service", place.line(&[(-60.0, 0.0), (60.0, 0.0)], None), on_layer(1)),
            way("footway", place.line(&[(-40.0, -30.0), (40.0, 50.0)], None), on_layer(2)),
        ];
        let (heights, chains) = ground(&features, &terrain, 1);
        let feature = |c: usize| chains.chains[c].arms[0].0 .0;
        let passes: BTreeSet<(u32, u32)> =
            chains.reads.iter().filter(|r| r.2 == ReadKind::GroundPass).map(|r| (feature(r.0), feature(r.1))).collect();
        assert_eq!(passes, BTreeSet::from([(1, 0), (2, 0), (2, 1)]));
        assert!(chains.order.iter().all(|c| c.len() == 1));
        assert!(of(&heights, 0).iter().all(|z| (f64::from(*z) - 1000.0).abs() <= 0.05));
        let (over, under) = (heights.at((1, 0, 0, 30)), heights.at((0, 0, 0, 20)));
        assert!(over - under >= CLEAR_PATH_M + DECK_M - 1e-4, "{over} {under}");
        assert!(heights.metrics.passes_dropped >= 1);
        assert!(of(&heights, 2).iter().all(|z| (f64::from(*z) - 1000.0).abs() <= BAND_M));
    }

    #[test]
    fn the_reader_of_a_ground_pass_is_the_same_in_every_area() {
        let place = engelberg();
        let n = (1u64 << super::super::area::AREA_ZOOM) as f64;
        let corner = (((place.x0 * n).floor() + 1.0) / n - place.x0) / place.unit;
        let corner = (corner, (place.y0 - (place.y0 * n).floor() / n) / place.unit);
        let near =
            |points: &[(f64, f64)]| points.iter().map(|&(e, n)| (corner.0 + e, corner.1 + n)).collect::<Vec<_>>();
        let terrain = place.terrain(|e, n| 1000.0 + 0.01 * e + 0.02 * n);
        // The ways of test 6 (a), turned so that they cross at the corner and their ends lie in four areas.
        let features = [
            way("steps", place.line(&near(&[(-10.0, -20.0), (10.0, 20.0)]), None), plain),
            way("service", place.line(&near(&[(-60.0, 30.0), (60.0, -30.0)]), None), on_layer(1)),
        ];
        let refs: Vec<&Feature> = features.iter().collect();
        let areas: HashSet<(i64, i64)> = refs.iter().flat_map(|f| &f.parts[0]).map(|&p| area_of(p)).collect();
        assert_eq!(areas.len(), 4);
        let classes = table().1;
        let net = Network::new(&refs, &classes).unwrap();
        let numbers: Vec<usize> = (0..refs.len()).collect();
        let one = AreaFacts::join(vec![area_facts(&net, &classes, &terrain, (&[], &[]), &numbers, &|_| true)]);
        let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
            let net = Network::new(subset, &classes).unwrap();
            area_facts(&net, &classes, &terrain, (&[], &[]), chosen, &|xy| area_of(xy) == area)
        };
        let four = AreaFacts::join(each_area(&refs, &[], false, (usize::MAX, 4), &job));
        assert_eq!(one.passes.len(), 1);
        assert_eq!(one.passes, four.passes);
        let whole = Heights::solve(&refs, &classes, &terrain, &assemble(&refs, &one.facts), &one, 1);
        let (apart, _) = ground(&features, &terrain, 4);
        let bits = |h: &Heights| h.z.iter().flatten().flatten().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&whole), bits(&apart));
        assert_eq!(whole.metrics.passes_held, 1);
    }

    /// The reads of a solve and whether any read node holds another height at the end.
    fn reads_hold(name: &str, features: &[Feature], terrain: &dyn Terrain, cycle: bool) {
        let (heights, chains) = ground(features, terrain, 1);
        assert!(heights.metrics.reads > 0, "{name}: no read");
        assert_eq!(chains.order.iter().any(|c| c.len() > 1), cycle, "{name}");
        assert_eq!(heights.metrics.read_changed, 0, "{name}: worst {}", heights.metrics.read_changed_max);
        assert_eq!(heights.metrics.shared_apart, 0, "{name}");
    }

    #[test]
    fn a_height_that_a_chain_read_never_changes() {
        let place = engelberg();
        // Two streets that end on each other 5 m apart, and their mirror, which swaps the ranks.
        for sign in [1.0, -1.0] {
            let terrain = place.terrain(move |e, n| 1000.0 + 0.15 * n + 0.04 * sign * e + 0.3 * (e * 1.3).sin());
            let p = |pts: &[(f64, f64)]| place.line(&pts.iter().map(|&(e, n)| (sign * e, n)).collect::<Vec<_>>(), None);
            let features = [
                way("residential", p(&[(-60.0, 0.0), (-5.0, 0.0), (0.0, 0.0)]), plain),
                way("residential", p(&[(0.0, -60.0), (0.0, 0.0), (1.0, 4.0), (-1.0, 1.07), (-5.0, 0.0)]), plain),
            ];
            reads_hold("triangle", &features, &terrain, true);
        }
        // A dual carriageway that splits from a road and merges into it again.
        let terrain = place.terrain(|e, _| 1000.0 + 0.04 * e + 0.8 * (e / 9.0).sin());
        let dual = [
            way("secondary", place.line(&[(-200.0, 0.0), (0.0, 0.0)], Some(10.0)), plain),
            way("secondary", place.line(&[(0.0, 0.0), (20.0, 1.5), (180.0, 2.0), (200.0, 0.0)], Some(10.0)), plain),
            way("secondary", place.line(&[(200.0, 0.0), (180.0, -1.5), (20.0, -2.0), (0.0, 0.0)], Some(10.0)), plain),
            way("secondary", place.line(&[(200.0, 0.0), (400.0, 0.0)], Some(10.0)), plain),
        ];
        reads_hold("dual carriageway", &dual, &terrain, true);
        let layers = [
            way("footway", place.line(&[(0.0, -40.0), (0.0, 40.0)], None), plain),
            way("service", place.line(&[(-60.0, 0.0), (60.0, 0.0)], None), on_layer(1)),
            way("footway", place.line(&[(-40.0, -30.0), (40.0, 50.0)], None), on_layer(2)),
        ];
        reads_hold("three layers", &layers, &place.terrain(flat), false);
        let (features, _, terrain) = corner_network(&place);
        reads_hold("corner network", &features, &terrain, false);
        // Structures: a deck over a road, and a shaft that branches off a tunnel.
        let deck = [
            way("primary", place.line(&[(-200.0, 0.0), (-100.0, 0.0)], Some(20.0)), plain),
            way("primary", place.line(&[(-100.0, 0.0), (100.0, 0.0)], Some(20.0)), bridge),
            way("primary", place.line(&[(100.0, 0.0), (200.0, 0.0)], Some(20.0)), plain),
            way("residential", place.line(&[(0.0, -50.0), (0.0, 50.0)], Some(20.0)), plain),
        ];
        reads_hold("deck", &deck, &place.terrain(flat), false);
        let shaft = [
            way("primary", place.line(&[(-50.0, 0.0), (0.0, 0.0)], Some(20.0)), plain),
            way("primary", place.line(&[(0.0, 0.0), (200.0, 0.0), (400.0, 0.0)], Some(20.0)), tunnel),
            way("primary", place.line(&[(400.0, 0.0), (450.0, 0.0)], Some(20.0)), plain),
            way("steps", place.line(&[(200.0, 0.0), (200.0, 30.0)], Some(20.0)), tunnel),
            way("path", place.line(&[(200.0, 30.0), (200.0, 80.0)], Some(20.0)), plain),
        ];
        reads_hold("shaft", &shaft, &place.terrain(|_, n| 1000.0 + 2.0 * n.max(0.0)), false);
        reads_hold("steps on a deck", &steps_onto_a_deck(true), &place.terrain(flat), false);
    }

    /// The structure pieces of a solve.
    fn structures(heights: &Heights) -> Vec<&Piece> {
        heights.pieces.iter().filter(|p| p.structure).collect()
    }

    /// The heights of a feature's nodes, every 2 m from its first point, as f64.
    fn nodes_of(heights: &Heights, f: u32) -> Vec<f64> {
        of(heights, f).iter().map(|v| f64::from(*v)).collect()
    }

    /// A straight line with a vertex every 20 m.
    fn line20(place: &super::super::tests::Place, from: (f64, f64), to: (f64, f64)) -> Vec<[f64; 2]> {
        place.line(&[from, to], Some(20.0))
    }

    /// The line of a structure: its heights against the straight line between its ends.
    fn off_line(z: &[f64]) -> f64 {
        let n = z.len() - 1;
        (0..=n).map(|i| (z[i] - (z[0] + (z[n] - z[0]) * i as f64 / n as f64)).abs()).fold(0.0, f64::max)
    }

    /// Two roads at 1,000 m on both sides of a gorge at 970 m, and the bridge between them.
    fn gorge() -> (Vec<Feature>, impl Fn(f64, f64) -> f64 + Sync) {
        let place = engelberg();
        let terrain = place.terrain(|e, _| if e > 5.0 && e < 55.0 { 970.0 } else { 1000.0 });
        let features = vec![
            way("primary", line20(&place, (-100.0, 0.0), (0.0, 0.0)), plain),
            way("primary", line20(&place, (60.0, 0.0), (160.0, 0.0)), plain),
            way("primary", line20(&place, (0.0, 0.0), (60.0, 0.0)), bridge),
        ];
        (features, terrain)
    }

    #[test]
    fn a_bridge_over_a_gorge_is_on_its_line() {
        let (features, terrain) = gorge();
        let (alone, _) = ground(&features[..2], &terrain, 1);
        let (heights, _) = ground(&features, &terrain, 1);
        let z = nodes_of(&heights, 2);
        assert_eq!(z[0].to_bits(), heights.at((0, 0, 5, 0)).to_bits());
        assert_eq!(z[z.len() - 1].to_bits(), heights.at((1, 0, 0, 0)).to_bits());
        assert!(off_line(&z) < 1e-6, "{z:?}");
        assert!(structures(&heights).iter().all(|p| p.plain));
        for f in 0..2 {
            assert_eq!(of(&alone, f), of(&heights, f));
        }
    }

    #[test]
    fn a_structure_that_no_bound_moves_is_its_line() {
        let (features, terrain) = gorge();
        let (heights, _) = ground(&features, &terrain, 1);
        assert!(structures(&heights).iter().all(|p| p.plain));
        assert_eq!(heights.metrics.structure_on_string, 0);
    }

    /// A primary road with a bridge from east −100 to 100, and residential roads under it at `east`.
    fn deck_over_roads(east: &[f64]) -> (Heights, Chains) {
        let place = engelberg();
        let mut features = vec![
            way("primary", line20(&place, (-200.0, 0.0), (-100.0, 0.0)), plain),
            way("primary", line20(&place, (-100.0, 0.0), (100.0, 0.0)), bridge),
            way("primary", line20(&place, (100.0, 0.0), (200.0, 0.0)), plain),
        ];
        for &east in east {
            features.push(way("residential", place.line(&[(east, -50.0), (east, 50.0)], Some(20.0)), plain));
        }
        ground(&features, &place.terrain(flat), 1)
    }

    fn deck_over_road(east: f64) -> (Heights, Chains) {
        deck_over_roads(&[east])
    }

    /// Whether z runs on the straight line from node a to node b, to a few units of the `f32` of the store.
    fn straight(z: &[f64], a: usize, b: usize) -> bool {
        (a..=b).all(|i| (z[i] - (z[a] + (z[b] - z[a]) * (i - a) as f64 / (b - a) as f64)).abs() < 1e-4)
    }

    /// The steepest rise per metre of nodes 2 m apart.
    fn steepest(z: &[f64]) -> f64 {
        z.windows(2).map(|w| ((w[1] - w[0]) / 2.0).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn a_deck_rises_in_straight_runs_over_the_road_that_it_crosses() {
        let (heights, _) = deck_over_road(0.0);
        let z = nodes_of(&heights, 1);
        // The road crosses at node 50: the crossing segment runs from node 50 to 51.
        assert!((z[50] - 1005.5).abs() < 1e-6 && (z[51] - 1005.5).abs() < 1e-6, "{:?}", &z[48..54]);
        assert!(straight(&z, 0, 50) && straight(&z, 51, z.len() - 1), "{z:?}");
        assert!(steepest(&z) <= 0.24 + 1e-9, "{}", steepest(&z));
        assert!(nodes_of(&heights, 3).iter().all(|z| (z - 1000.0).abs() < 1e-6));
    }

    #[test]
    fn a_deck_keeps_its_line_where_the_cap_does_not_reach_the_clearance() {
        // 20 m from the landing the cap of 0.24 reaches 4.8 m: short of 5.5 m; 1:1 would reach it.
        let (heights, _) = deck_over_road(-80.0);
        let z = nodes_of(&heights, 1);
        assert!(z.iter().all(|z| (z - 1000.0).abs() < 1e-6), "{:?}", &z[..12]);
        assert_eq!(heights.metrics.deck_short, [1, 0, 0]);
        assert!(heights.metrics.deck_short_all[0].at_1_1);
        assert_eq!(z[0].to_bits(), heights.at((0, 0, 5, 0)).to_bits());
    }

    #[test]
    fn a_deck_clears_the_crossing_in_its_reach_and_keeps_its_line_at_the_one_beside_its_landing() {
        let (heights, _) = deck_over_roads(&[0.0, -95.0]);
        let z = nodes_of(&heights, 1);
        assert!(z[50] - 1000.0 >= 5.5 - 1e-4, "{}", z[50]);
        assert_eq!(heights.metrics.deck_short, [1, 0, 0]);
        assert!(z[2] - 1000.0 <= 0.3, "{}", z[2]);
    }

    /// A primary road with a tunnel from east 0 to 200, and a residential road over it at `east`.
    fn tunnel_under_road(east: f64) -> (Heights, Chains) {
        let place = engelberg();
        let features = [
            way("primary", line20(&place, (-100.0, 0.0), (0.0, 0.0)), plain),
            way("primary", line20(&place, (0.0, 0.0), (200.0, 0.0)), tunnel),
            way("primary", line20(&place, (200.0, 0.0), (300.0, 0.0)), plain),
            way("residential", place.line(&[(east, -50.0), (east, 50.0)], Some(20.0)), plain),
        ];
        ground(&features, &place.terrain(flat), 1)
    }

    #[test]
    fn a_tunnel_sinks_in_straight_runs_under_the_road_over_it() {
        let (heights, _) = tunnel_under_road(100.0);
        let z = nodes_of(&heights, 1);
        // The road and its edges, 3 m to each side, bound the nodes from east 96 to 104.
        assert!((48..=52).all(|i| (z[i] - 994.0).abs() < 1e-6), "{:?}", &z[46..55]);
        assert!(straight(&z, 0, 48) && straight(&z, 52, z.len() - 1), "{z:?}");
        assert!(steepest(&z) <= 0.24 + 1e-9, "{}", steepest(&z));
        assert!(z.iter().all(|z| *z >= 994.0 - 1e-9), "{z:?}");
        assert!(nodes_of(&heights, 3).iter().all(|z| (z - 1000.0).abs() < 1e-6));
    }

    #[test]
    fn a_tunnel_keeps_its_line_where_its_portal_lies_right_under_a_road() {
        let (heights, _) = tunnel_under_road(3.0);
        let z = nodes_of(&heights, 1);
        assert!(z.iter().all(|z| (z - 1000.0).abs() < 1e-6), "{:?}", &z[..5]);
        assert_eq!(heights.metrics.bore_exposed, 1, "{:?}", heights.metrics.bore_exposed_all);
        assert_eq!(z[0].to_bits(), heights.at((0, 0, 5, 0)).to_bits());
        assert!(nodes_of(&heights, 3).iter().all(|z| (z - 1000.0).abs() < 1e-6));
    }

    #[test]
    fn a_tunnel_bound_that_a_deck_bound_contradicts_is_dropped() {
        let place = engelberg();
        let features = [
            way("primary", line20(&place, (-100.0, 0.0), (0.0, 0.0)), plain),
            way("primary", line20(&place, (0.0, 0.0), (40.0, 0.0)), bridge),
            way("primary", line20(&place, (40.0, 0.0), (140.0, 0.0)), tunnel),
            way("primary", line20(&place, (140.0, 0.0), (240.0, 0.0)), plain),
            way("rail", place.line(&[(36.0, -40.0), (36.0, 40.0)], None), plain),
            way("residential", place.line(&[(44.0, -40.0), (44.0, 40.0)], None), plain),
        ];
        let (heights, _) = ground(&features, &place.terrain(flat), 1);
        let (deck, bore) = (nodes_of(&heights, 1), nodes_of(&heights, 2));
        assert!(deck[18] - 1000.0 >= 7.0 - 1e-4, "{:?}", &deck[15..]);
        let m = &heights.metrics;
        assert_eq!(m.bore_exposed, 1, "{:?}", m.bore_exposed_all);
        assert!(m.tunnel_over >= 1);
        let z: Vec<f64> = deck.iter().chain(&bore[1..]).copied().collect();
        assert!(z.windows(2).all(|w| (w[1] - w[0]).abs() <= 0.24 * 2.0 + 1e-9), "{z:?}");
    }

    #[test]
    fn a_shaft_is_the_straight_line_between_its_fixed_ends() {
        let place = engelberg();
        let terrain = place.terrain(|_, n| 1000.0 + 2.0 * n.max(0.0));
        let main = [
            way("primary", place.line(&[(-50.0, 0.0), (0.0, 0.0)], Some(20.0)), plain),
            way("primary", place.line(&[(0.0, 0.0), (200.0, 0.0), (400.0, 0.0)], Some(20.0)), tunnel),
            way("primary", place.line(&[(400.0, 0.0), (450.0, 0.0)], Some(20.0)), plain),
        ];
        let shaft = [
            way("steps", place.line(&[(200.0, 0.0), (200.0, 30.0)], Some(20.0)), tunnel),
            way("path", place.line(&[(200.0, 30.0), (200.0, 80.0)], Some(20.0)), plain),
        ];
        let (alone, _) = ground(&main, &terrain, 1);
        let all: Vec<Feature> = main.iter().chain(&shaft).cloned().collect();
        let (heights, chains) = ground(&all, &terrain, 1);
        let chain_of = |f: u32| chains.chains.iter().position(|c| c.arms.iter().any(|a| a.0 .0 == f)).unwrap();
        let (main_chain, steps, path) = (chain_of(1), chain_of(3), chain_of(4));
        assert!(chains.reads.contains(&(steps, main_chain, ReadKind::Branch)), "{:?}", chains.reads);
        assert!(chains.reads.contains(&(steps, path, ReadKind::End)), "{:?}", chains.reads);
        assert_eq!(heights.metrics.structure_opened, 1);
        // 30 m to 1 mm: the scale of the map changes over the 30 m north.
        assert!((heights.metrics.structure_opened_m - 30.0).abs() < 1e-3, "{}", heights.metrics.structure_opened_m);
        assert!((heights.metrics.structure_opened_worst[0].0 - 2.0).abs() < 0.1);
        assert!(off_line(&nodes_of(&heights, 3)) < 1e-3);
        assert_eq!(of(&alone, 1), of(&heights, 1));
    }

    #[test]
    fn steps_in_a_tunnel_between_two_ground_heights_far_apart() {
        let place = engelberg();
        let terrain = place.terrain(|e, _| {
            if e <= 0.0 {
                1000.0
            } else if e < 10.0 {
                1000.0 - 1.2 * e
            } else {
                988.0
            }
        });
        let features = [
            way("footway", place.line(&[(-40.0, 0.0), (0.0, 0.0)], Some(20.0)), plain),
            way("steps", place.line(&[(0.0, 0.0), (10.0, 0.0)], None), tunnel),
            way("footway", place.line(&[(10.0, 0.0), (50.0, 0.0)], Some(20.0)), plain),
        ];
        let (heights, _) = ground(&features, &terrain, 1);
        let z = nodes_of(&heights, 1);
        assert_eq!(z[0].to_bits(), heights.at((0, 0, 2, 0)).to_bits());
        assert_eq!(z[z.len() - 1].to_bits(), heights.at((2, 0, 0, 0)).to_bits());
        assert_eq!(heights.metrics.structure_opened, 1);
        assert!((heights.metrics.structure_opened_m - 10.0).abs() < 1e-6);
        assert!(off_line(&z) < 1e-3, "{z:?}");
    }

    #[test]
    fn three_stacked_bridges_are_solved_from_the_bottom_up() {
        let place = engelberg();
        let deck = |degrees: f64, layer: i64| {
            let (e, n) = (50.0 * degrees.to_radians().cos(), 50.0 * degrees.to_radians().sin());
            way("primary", place.line(&[(-e, -n), (e, n)], None), move |f: &mut Feature| {
                bridge(f);
                (f.layer, f.layered) = (layer, true);
            })
        };
        let (heights, chains) = ground(&[deck(0.0, 1), deck(60.0, 2), deck(120.0, 3)], &place.terrain(flat), 1);
        // The centre lies 50 m along each way, on its node 25 (2 m apart, 50 pieces): the crossing segment
        // runs from node 25 to 26.
        let at = |f: u32, height: f64| [25, 26].iter().all(|&i| (nodes_of(&heights, f)[i] - height).abs() < 1e-6);
        assert!(nodes_of(&heights, 0).iter().all(|z| (z - 1000.0).abs() < 1e-6));
        assert!(at(1, 1005.5), "{:?}", &nodes_of(&heights, 1)[22..29]);
        assert!(at(2, 1011.0), "{:?}", &nodes_of(&heights, 2)[22..29]);
        assert_eq!(heights.metrics.deck_short, [0, 0, 0]);
        for f in [1, 2] {
            let z = nodes_of(&heights, f);
            assert!(straight(&z, 0, 25) && straight(&z, 26, z.len() - 1), "{f}: {z:?}");
        }
        assert_eq!(chains.order.len(), 3);
        assert_eq!(chains.depth, vec![0, 1, 2]);
    }

    #[test]
    fn a_bridge_that_runs_into_a_tunnel_is_one_chain() {
        let place = engelberg();
        let terrain = place.terrain(|e, _| {
            if e > 5.0 && e < 35.0 {
                970.0
            } else if e > 40.0 && e <= 90.0 {
                1000.0 + 0.5 * (e - 40.0)
            } else if e > 90.0 && e < 140.0 {
                1025.0 - 0.5 * (e - 90.0)
            } else {
                1000.0
            }
        });
        let features = [
            way("primary", line20(&place, (-100.0, 0.0), (0.0, 0.0)), plain),
            way("primary", line20(&place, (0.0, 0.0), (40.0, 0.0)), bridge),
            way("primary", line20(&place, (40.0, 0.0), (140.0, 0.0)), tunnel),
            way("primary", line20(&place, (140.0, 0.0), (240.0, 0.0)), plain),
        ];
        let (heights, chains) = ground(&features, &terrain, 1);
        let structure: Vec<&super::super::chains::Chain> = chains.chains.iter().filter(|c| c.structure).collect();
        assert_eq!(structure.len(), 1);
        assert!((structure[0].metres - 140.0).abs() < 1e-6);
        for f in [1, 2] {
            assert!(nodes_of(&heights, f).iter().all(|z| (z - 1000.0).abs() < 1e-3), "{f}");
        }
        assert!(structures(&heights).iter().all(|p| p.plain));
    }

    #[test]
    fn a_structure_that_meets_nothing_lies_between_the_terrain_at_its_ends() {
        let place = engelberg();
        let terrain = place.terrain(|e, _| 1000.0 + 0.05 * e);
        let (heights, _) = ground(&[way("primary", line20(&place, (0.0, 0.0), (100.0, 0.0)), tunnel)], &terrain, 1);
        let z = nodes_of(&heights, 0);
        assert!((z[0] - 1000.0).abs() < 1e-4 && (z[z.len() - 1] - 1005.0).abs() < 1e-4 && straight(&z, 0, z.len() - 1));
    }

    #[test]
    fn a_deck_over_a_stream_clears_the_water() {
        let place = engelberg();
        let terrain = place.terrain(|e, _| if e > 5.0 && e < 55.0 { 970.0 } else { 1000.0 });
        let features = [
            way("primary", line20(&place, (-100.0, 0.0), (0.0, 0.0)), plain),
            way("primary", line20(&place, (0.0, 0.0), (60.0, 0.0)), bridge),
            way("primary", line20(&place, (60.0, 0.0), (160.0, 0.0)), plain),
        ];
        let river = |level: f64| River {
            points: vec![place.at(30.0, -40.0), place.at(30.0, 40.0)],
            levels: vec![level, level],
            half: 2.0,
            cover: 1.0,
            lift: 1.5,
            untagged_culvert: false,
        };
        let (heights, _) = over(&features, &terrain, &[river(990.0)], 1);
        let z = nodes_of(&heights, 1);
        assert!(z[15] >= 992.5 && off_line(&z) < 1e-6);
        assert!(structures(&heights).iter().all(|p| p.tube.cuts.is_empty()));
        let (heights, _) = over(&features, &terrain, &[river(999.5)], 1);
        let z = nodes_of(&heights, 1);
        assert!(z[14].min(z[15]) >= 1002.0 - 1e-4, "{:?}", &z[12..18]);
        assert!(z.windows(2).all(|w| (w[1] - w[0]).abs() <= 2.0 + 1e-9));
    }

    #[test]
    fn the_heights_of_structures_do_not_depend_on_the_areas() {
        let place = engelberg();
        let n = (1u64 << super::super::area::AREA_ZOOM) as f64;
        let corner = (((place.x0 * n).floor() + 1.0) / n - place.x0) / place.unit;
        let corner = (corner, (place.y0 - (place.y0 * n).floor() / n) / place.unit);
        // The features of the deck over a road, turned by 30 degrees about the corner of four areas.
        let (sin, cos) = 30f64.to_radians().sin_cos();
        let near = |points: &[(f64, f64)]| {
            points
                .iter()
                .map(|&(e, n)| (corner.0 + e * cos - n * sin, corner.1 + e * sin + n * cos))
                .collect::<Vec<_>>()
        };
        let features = [
            way("primary", place.line(&near(&[(-200.0, 0.0), (-100.0, 0.0)]), Some(20.0)), plain),
            way("primary", place.line(&near(&[(-100.0, 0.0), (100.0, 0.0)]), Some(20.0)), bridge),
            way("primary", place.line(&near(&[(100.0, 0.0), (200.0, 0.0)]), Some(20.0)), plain),
            way("residential", place.line(&near(&[(0.0, -50.0), (0.0, 50.0)]), Some(20.0)), plain),
        ];
        let refs: Vec<&Feature> = features.iter().collect();
        let areas: HashSet<(i64, i64)> = refs.iter().flat_map(|f| &f.parts[0]).map(|&p| area_of(p)).collect();
        assert_eq!(areas.len(), 4);
        let terrain = place.terrain(|e, n| 1000.0 + 0.01 * e + 0.02 * n);
        let classes = table().1;
        let net = Network::new(&refs, &classes).unwrap();
        let numbers: Vec<usize> = (0..refs.len()).collect();
        let one = AreaFacts::join(vec![area_facts(&net, &classes, &terrain, (&[], &[]), &numbers, &|_| true)]);
        let whole = Heights::solve(&refs, &classes, &terrain, &assemble(&refs, &one.facts), &one, 1);
        let (apart, _) = ground(&features, &terrain, 4);
        let bits = |h: &Heights| h.z.iter().flatten().flatten().map(|v| v.to_bits()).collect::<Vec<_>>();
        assert!(of(&whole, 1).iter().all(|v| v.is_finite()));
        assert_eq!(bits(&whole), bits(&apart));
    }

    /// A tertiary tunnel from (−200, 0) to (200, 0) with a vertex at (0, 0), between two roads.
    fn tunnel_a(place: &super::super::tests::Place) -> Vec<Feature> {
        vec![
            way("tertiary", line20(place, (-300.0, 0.0), (-200.0, 0.0)), plain),
            way("tertiary", place.line(&[(-200.0, 0.0), (0.0, 0.0), (200.0, 0.0)], Some(20.0)), tunnel),
            way("tertiary", line20(place, (200.0, 0.0), (300.0, 0.0)), plain),
        ]
    }

    #[test]
    fn two_tunnels_that_cross_at_one_node_have_one_height_there() {
        let place = engelberg();
        let terrain = place.terrain(|_, n| 1000.0 + 0.1 * n);
        let mut features = tunnel_a(&place);
        features.push(way("service", line20(&place, (0.0, -300.0), (0.0, -200.0)), plain));
        features.push(way("service", place.line(&[(0.0, -200.0), (0.0, 0.0), (0.0, 200.0)], Some(20.0)), tunnel));
        let (heights, _) = ground(&features, &terrain, 1);
        // Both run through (0, 0), vertex 10 of each; A has two fixed ends and owns the point.
        let (a, b) = (heights.at((1, 0, 10, 0)), heights.at((4, 0, 10, 0)));
        assert_eq!(a.to_bits(), b.to_bits());
        let a = nodes_of(&heights, 1);
        assert!(straight(&a, 0, a.len() - 1) && (a[0] - 1000.0).abs() < 1e-4, "{a:?}");
        // B in the order of its chain, from (0, 200) to its south portal: level to (0, 0), then straight.
        let piece = structures(&heights)
            .into_iter()
            .find(|p| {
                p.tube.fixed.iter().filter(|v| v.is_finite()).count() == 2
                    && p.z.iter().any(|z| (z - 980.0).abs() < 1e-6)
            })
            .expect("the chain of B");
        let middle = piece.tube.fixed.iter().position(|v| (v - 1000.0).abs() < 1e-9).unwrap();
        let z = &piece.z;
        assert!(z[..=middle].iter().all(|v| (v - 1000.0).abs() < 1e-9), "{z:?}");
        assert!(straight(z, middle, z.len() - 1) && (z[z.len() - 1] - 980.0).abs() < 1e-9, "{z:?}");
        assert_eq!(heights.metrics.shared_apart, 0);
    }

    #[test]
    fn a_bridge_that_ends_on_a_node_inside_another_bridge_takes_its_height() {
        let place = engelberg();
        let features = [
            way("primary", line20(&place, (-200.0, 0.0), (-100.0, 0.0)), plain),
            way("primary", place.line(&[(-100.0, 0.0), (20.0, 0.0), (100.0, 0.0)], Some(20.0)), bridge),
            way("primary", line20(&place, (100.0, 0.0), (200.0, 0.0)), plain),
            way("residential", place.line(&[(0.0, -50.0), (0.0, 50.0)], Some(20.0)), plain),
            way("footway", line20(&place, (20.0, 0.0), (20.0, 60.0)), bridge),
            way("footway", line20(&place, (20.0, 60.0), (20.0, 100.0)), plain),
        ];
        let (heights, _) = ground(&features, &place.terrain(flat), 1);
        let (deck, foot) = (nodes_of(&heights, 1), nodes_of(&heights, 4));
        assert_eq!(foot[0].to_bits(), deck[60].to_bits());
        assert!(straight(&foot, 0, foot.len() - 1), "{foot:?}");
    }

    #[test]
    fn a_ground_way_at_a_point_where_a_tunnel_ends_on_another_tunnel_hangs_on_the_one_that_runs_through() {
        let place = engelberg();
        let terrain = place.terrain(|_, n| 1000.0 + 0.1 * n);
        let mut features = tunnel_a(&place);
        features.push(way("tertiary", line20(&place, (0.0, -60.0), (0.0, 0.0)), tunnel));
        features.push(way("tertiary", line20(&place, (0.0, -100.0), (0.0, -60.0)), plain));
        features.push(way("path", line20(&place, (0.0, 0.0), (40.0, 40.0)), plain));
        let (heights, _) = ground(&features, &terrain, 1);
        let (a, side, path) = (nodes_of(&heights, 1), nodes_of(&heights, 3), nodes_of(&heights, 5));
        assert_eq!(side[side.len() - 1].to_bits(), a[100].to_bits());
        assert_eq!(path[0].to_bits(), a[100].to_bits());
        assert_eq!(heights.metrics.joins_held, 1);
    }

    /// A long structure from east 0 to `length` between two roads, vertices every 50 m, and a residential
    /// road across it at `east`.
    fn long_structure(class: &str, length: f64, edit: fn(&mut Feature), east: f64) -> Vec<f64> {
        let place = engelberg();
        let line = |a: f64, b: f64| place.line(&[(a, 0.0), (b, 0.0)], Some(50.0));
        let features = [
            way(class, line(-50.0, 0.0), plain),
            way(class, line(0.0, length), edit),
            way(class, line(length, length + 50.0), plain),
            way("residential", place.line(&[(east, -50.0), (east, 50.0)], Some(20.0)), plain),
        ];
        nodes_of(&ground(&features, &place.terrain(flat), 1).0, 1)
    }

    #[test]
    fn a_bound_near_one_end_does_not_bend_a_long_tunnel() {
        let z = long_structure("rail", 4000.0, tunnel, 100.0);
        assert!((48..=52).all(|i| (z[i] - 992.5).abs() < 1e-6), "{:?}", &z[46..55]);
        assert!((z[24] - 996.25).abs() < 1e-6, "{}", z[24]);
        assert!((z[1026] - 996.25).abs() < 1e-3, "{}", z[1026]);
        assert!(z.iter().all(|z| (992.5 - 1e-9..=1000.0).contains(z)));
        assert!(z[..=48].windows(2).all(|w| w[1] < w[0]) && z[52..].windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn a_clearance_near_one_landing_does_not_hump_a_long_bridge() {
        let z = long_structure("primary", 500.0, bridge, 40.0);
        assert!((z[20] - 1005.5).abs() < 1e-6 && (z[21] - 1005.5).abs() < 1e-6, "{:?}", &z[18..24]);
        assert!(z.iter().all(|z| (1000.0..=1005.5 + 1e-9).contains(z)), "{:?}", &z[..40]);
        assert!((1002.9..=1003.1).contains(&z[125]), "{}", z[125]);
    }

    /// A footway over a railway on a bridge with a vertex at east 20, and steps from there to the south.
    fn steps_onto_a_deck(steps: bool) -> Vec<Feature> {
        let place = engelberg();
        let mut features = vec![
            way("footway", line20(&place, (-40.0, 0.0), (0.0, 0.0)), plain),
            way("footway", place.line(&[(0.0, 0.0), (20.0, 0.0), (60.0, 0.0)], Some(20.0)), bridge),
            way("footway", line20(&place, (60.0, 0.0), (100.0, 0.0)), plain),
            way("rail", place.line(&[(30.0, -40.0), (30.0, 40.0)], Some(20.0)), plain),
        ];
        if steps {
            features.push(way("steps", place.line(&[(20.0, 0.0), (20.0, -40.0)], None), plain));
        }
        features
    }

    #[test]
    fn steps_onto_a_deck_hang_on_it_inside_their_reach() {
        let place = engelberg();
        let terrain = place.terrain(flat);
        let (alone, _) = ground(&steps_onto_a_deck(false), &terrain, 1);
        let (heights, _) = ground(&steps_onto_a_deck(true), &terrain, 1);
        let deck = nodes_of(&heights, 1);
        assert!(deck[15] - 1000.0 >= 6.4 - 1e-6, "{}", deck[15]);
        let steps = nodes_of(&heights, 4);
        assert_eq!(steps[0].to_bits(), heights.at((1, 0, 1, 0)).to_bits());
        assert!(steps[..=5].windows(2).all(|w| (w[1] - w[0]).abs() <= 2.0 + 1e-9), "{steps:?}");
        assert!(steps[6..].iter().all(|z| (z - 1000.0).abs() <= BAND_M), "{steps:?}");
        assert_eq!(heights.metrics.joins_held, 1);
        assert_eq!(of(&alone, 1), of(&heights, 1));
    }

    /// A rail tunnel under a hill with a vertex at east 200, and maybe a way of a class that OSM joins to it
    /// there.
    fn a_way_on_a_deep_tunnel(road: Option<&str>) -> Vec<Feature> {
        let place = engelberg();
        let mut features = vec![
            way("rail", line20(&place, (-50.0, 0.0), (0.0, 0.0)), plain),
            way("rail", place.line(&[(0.0, 0.0), (200.0, 0.0), (400.0, 0.0)], Some(20.0)), tunnel),
            way("rail", line20(&place, (400.0, 0.0), (450.0, 0.0)), plain),
        ];
        if let Some(class) = road {
            features.push(way(class, place.line(&[(200.0, 60.0), (200.0, 0.0)], Some(20.0)), plain));
        }
        features
    }

    /// Flat 1,000 m with a plateau over the tunnel of `a_way_on_a_deep_tunnel`, which rises and falls at 0.6.
    fn plateau(place: &super::super::tests::Place, top: f64) -> impl Fn(f64, f64) -> f64 + Sync {
        let rise = (top - 1000.0) / 0.6;
        place.terrain(move |e, _| {
            if e <= 0.0 || e >= 400.0 {
                1000.0
            } else if e < rise {
                1000.0 + 0.6 * e
            } else if e <= 400.0 - rise {
                top
            } else {
                top - 0.6 * (e - 400.0 + rise)
            }
        })
    }

    #[test]
    fn a_way_that_joins_a_tunnel_far_under_the_ground_stays_on_the_ground() {
        let place = engelberg();
        let terrain = plateau(&place, 1030.0);
        let (alone, _) = ground(&a_way_on_a_deep_tunnel(None), &terrain, 1);
        let (heights, _) = ground(&a_way_on_a_deep_tunnel(Some("service")), &terrain, 1);
        assert!(nodes_of(&heights, 1).iter().all(|z| (z - 1000.0).abs() <= 0.01));
        let road = nodes_of(&heights, 3);
        assert!(road[road.len() - 1].is_nan(), "{road:?}");
        assert!(road[..road.len() - 1].iter().all(|z| (z - 1030.0).abs() <= BAND_M), "{road:?}");
        assert_eq!(heights.metrics.joins_dropped, 1);
        assert_eq!(of(&alone, 1), of(&heights, 1));
    }

    #[test]
    fn a_join_holds_where_1_to_1_from_it_reaches_the_band_and_is_dropped_where_not_for_every_class() {
        let place = engelberg();
        for class in ["path", "service"] {
            for (top, holds) in [(1012.0, true), (1020.0, false)] {
                let terrain = plateau(&place, top);
                let (alone, _) = ground(&a_way_on_a_deep_tunnel(None), &terrain, 1);
                let (heights, _) = ground(&a_way_on_a_deep_tunnel(Some(class)), &terrain, 1);
                // The way's nodes from (200, 60), 2 m apart: the last is the join, 12 m before it the 24th.
                let (tunnel, road) = (nodes_of(&heights, 1), nodes_of(&heights, 3));
                let join = road.len() - 1;
                assert_eq!(join, 30);
                if holds {
                    assert_eq!(road[join].to_bits(), tunnel[100].to_bits(), "{class} {top}");
                    assert!(road[24..].windows(2).all(|w| (w[1] - w[0]).abs() <= 2.0 + 1e-9), "{class} {road:?}");
                    assert!(road[..24].iter().all(|z| (z - top).abs() <= BAND_M), "{class} {road:?}");
                    assert_eq!(heights.metrics.joins_held, 1, "{class} {top}");
                } else {
                    assert!(road[join].is_nan(), "{class} {road:?}");
                    assert!(road[..join].iter().all(|z| (z - top).abs() <= BAND_M), "{class} {road:?}");
                    assert_eq!(heights.metrics.joins_dropped, 1, "{class} {top}");
                    assert_eq!(of(&alone, 1), of(&heights, 1), "{class} {top}");
                }
            }
        }
    }
}
