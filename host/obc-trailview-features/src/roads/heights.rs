//! The heights of the ground, phase B of the road solver: each ground chain is solved once per bake, after
//! every chain that it reads, into one store of node heights.

use std::collections::{BTreeMap, HashMap, HashSet};

use rayon::prelude::*;

use super::chain::{self, Step};
use super::chains::{AreaFacts, Arm, Chains, Fact, Node, Place, ReadKind, Segments};
use super::network::{hypot, pieces, sub};
use super::{Classes, Feature, APPROACH_M, APPROACH_PATH_M, HUBER_M, RAMP_GRADE};
use crate::{degrees, metres, Terrain};

/// A way on the ground lies within BAND_M of the terrain at each node: half the census gate.
pub(crate) const BAND_M: f64 = crate::gate::GATE_M / 2.0;
/// A grade that changes by more than this between two segments is a kink.
const KINK: f64 = 0.05;
/// A node this far off the terrain counts in the metrics by its cause.
const OFF_M: f64 = 3.0;

/// The metrics of the ground heights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroundMetrics {
    pub chains: usize,
    pub plain: usize,
    pub steps: usize,
    pub steps_max: usize,
    pub unconverged: usize,
    pub pivots: usize,
    /// Cuts by layer: count and metres, and the largest cuts: metres, layer, latitude, longitude. Openings
    /// by layer, and the largest excess of an opened slope per metre.
    pub cuts: [usize; 5],
    pub cut_m: [f64; 5],
    pub cut_worst: Vec<(f64, u8, [f64; 2])>,
    pub openings: [usize; 5],
    pub excess_max: f64,
    /// Opened metres by cause: band (no fixed node at the ends of the opened run), read, cycle.
    pub opened_m: [f64; 3],
    /// Nodes over OFF_M off the terrain: band with a held grade, fixed by a read, pass, cover.
    pub off: [usize; 4],
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
    side: Vec<Vec<(Place, Arm)>>,
    approach: Vec<Vec<(Node, Arm)>>,
    copies: Vec<Vec<(Node, Node, f32)>>,
    passes: Vec<Vec<usize>>,
    covers: Vec<Vec<usize>>,
    facts: &'a AreaFacts,
    leaders: HashSet<(usize, usize)>,
    pass_readers: HashSet<(usize, usize)>,
    ramps: HashSet<Node>,
    ungraded: HashSet<Node>,
}

impl Context<'_> {
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
            facts,
            leaders: chains.reads.iter().filter(|r| r.2 == ReadKind::Copy).map(|r| (r.0, r.1)).collect(),
            pass_readers: chains.reads.iter().filter(|r| r.2 == ReadKind::GroundPass).map(|r| (r.0, r.1)).collect(),
            ramps: facts.ramps.iter().copied().collect(),
            ungraded: facts.ungraded.iter().copied().collect(),
        };
        for fact in &facts.facts {
            if let Fact::Read { kind: ReadKind::Side, reader, from } = *fact {
                let c = context.chain_of_arm(reader);
                context.side[c].push((reader.0, from));
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
        for (i, cover) in facts.covers.iter().enumerate() {
            for c in context.chains_of(cover.0) {
                context.covers[c].push(i);
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
        let mut metrics = GroundMetrics::default();
        let mut kinks: [Vec<f64>; 3] = Default::default();
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
        for (k, values) in kinks.iter_mut().enumerate() {
            metrics.kinks[k] = values.iter().filter(|&&v| v > KINK).count();
            metrics.kink_p99[k] = crate::geometry::percentile(values, 99.0).unwrap_or(0.0);
            metrics.kink_max[k] = values.iter().copied().fold(0.0, f64::max);
        }
        for worst in [&mut metrics.frozen_worst, &mut metrics.approach_ended_worst] {
            worst.sort_by(|a, b| b.0.total_cmp(&a.0));
            worst.truncate(10);
        }
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
    }
    for k in 0..2 {
        all.split_km[k] += one.split_km[k];
        all.split_grade_m[k] += one.split_grade_m[k];
        all.split_kink_m[k] += one.split_kink_m[k];
    }
    all.frozen_worst.extend(one.frozen_worst.iter().copied());
    all.approach_ended += one.approach_ended;
    all.approach_ended_worst.extend(one.approach_ended_worst.iter().copied());
}

/// A component of the read graph: its ground chains in its order; a cycle in two rounds. Returns its writes,
/// its outcome and its seconds.
fn component(context: &Context, store: &[Vec<Vec<f32>>], members: &[usize]) -> Done {
    let clock = std::time::Instant::now();
    let ground: Vec<usize> = members.iter().copied().filter(|&c| !context.chains.chains[c].structure).collect();
    let mut view = View { store, layout: context.layout, own: HashMap::new() };
    let mut outcome = Outcome::default();
    let cycle = members.len() > 1
        || ground.iter().any(|&c| context.chains.chains[c].closed)
        || context.chains.reads.iter().any(|r| r.0 == members[0] && r.1 == members[0]);
    let mut state = Cycle { solved: HashSet::new(), read: HashSet::new(), weights: HashMap::new(), cycle };
    let mut again = Vec::new();
    for &c in &ground {
        let waited = solve_chain(context, &mut view, &mut state, c, false, &mut outcome);
        state.solved.insert(c);
        if waited {
            again.push(c);
        }
    }
    for &c in &again {
        solve_chain(context, &mut view, &mut state, c, true, &mut outcome);
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

/// Why a node is fixed.
#[derive(Clone, Copy, PartialEq)]
enum Fix {
    Read,
    Frozen,
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
    let mut waited = false;
    // A read takes a height that exists; one from a chain of this component that is not solved yet waits.
    let mut take = |source: &[Node], from_chains: &[usize], state: &mut Cycle| -> Option<Vec<f64>> {
        let z: Vec<f64> = source.iter().map(|&m| view.at(m)).collect();
        if z.iter().any(|v| v.is_nan()) {
            let pending = from_chains.iter().any(|&b| b == c || state.cycle && !state.solved.contains(&b));
            waited |= pending && !second;
            return None;
        }
        state.read.extend(source.iter().copied());
        Some(z)
    };
    for &(place, from) in &context.side[c] {
        let (Some(&i), source) = (index.get(&(place.0, place.1, place.2, 0)), (from.0 .0, from.0 .1, from.0 .2, 0))
        else {
            continue;
        };
        if let Some(z) = take(&[source], &[context.chain_of_arm(from)], state) {
            (fixed[i], cause[i]) = (z[0], Some(Fix::Read));
        }
    }
    // The approach nodes of each side junction, from the junction outward. An approach holds only inside
    // the band: at the first node whose through height lies outside the band of its own terrain it ends,
    // and that node and the later ones stay free.
    let junctions: Vec<(usize, Arm)> = context.side[c]
        .iter()
        .filter_map(|&(place, from)| index.get(&(place.0, place.1, place.2, 0)).map(|&j| (j, from)))
        .collect();
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
            let b = context.chain_of_arm(through);
            let Some((pair, share)) = foot(context, nodes.xy[i], through, b) else { continue };
            let Some(z) = take(&pair, &[b], state) else { continue };
            let z = z[0] + share * (z[1] - z[0]);
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
        if !fixed[i].is_nan() || b == c || !context.leaders.contains(&(c, b)) {
            continue;
        }
        if let Some(z) = take(&[from, context.layout.next(from)], &[b], state) {
            (fixed[i], cause[i]) = (z[0] + f64::from(share) * (z[1] - z[0]), Some(Fix::Read));
        }
    }
    if chain.closed {
        // The last node of a closed chain reads its first.
        if let Some(z) = take(&[nodes.names[0][0]], &[c], state) {
            (fixed[n - 1], cause[n - 1]) = (z[0], Some(Fix::Read));
        }
    }
    let (mut lo1, mut hi1) = (vec![f64::NEG_INFINITY; n], vec![f64::INFINITY; n]);
    let mut passed = vec![false; n];
    for &k in &context.passes[c] {
        let (upper, lower, u, x, need) = context.facts.passes[k];
        let pair = |m: Node| [m, context.layout.next(m)];
        let on = |m: Node| pair(m).map(|m| index.get(&m).copied());
        let (reader_upper, other, other_share, mine) = match (on(upper), on(lower)) {
            ([Some(a), Some(b)], _) if context.pass_readers.contains(&(c, context.chains_of(lower)[0])) => {
                (true, lower, x, [a, b])
            }
            (_, [Some(a), Some(b)]) if context.pass_readers.contains(&(c, context.chains_of(upper)[0])) => {
                (false, upper, u, [a, b])
            }
            _ => continue,
        };
        let b = context.chains_of(other)[0];
        if let Some(z) = take(&pair(other), &[b], state) {
            let at = z[0] + f64::from(other_share) * (z[1] - z[0]);
            for i in mine {
                passed[i] = true;
                if reader_upper {
                    lo1[i] = lo1[i].max(at + f64::from(need));
                } else {
                    hi1[i] = hi1[i].min(at - f64::from(need));
                }
            }
        }
    }
    if second {
        // Each node that another chain read keeps the height that it read.
        for i in 0..n {
            if fixed[i].is_nan() && nodes.names[i].iter().any(|m| state.read.contains(m)) {
                (fixed[i], cause[i]) = (view.at(nodes.names[i][0]), Some(Fix::Frozen));
            }
        }
    }
    // Each piece between nodes without terrain is a chain of its own.
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
            let piece = Span { context, nodes: &nodes, c, range: start..end + 1, h: &h, l: &l };
            let solved = piece.solve(&fixed, &cause, (&lo1, &hi1, &passed), state, second, out);
            z[start..=end].copy_from_slice(&solved);
        }
        start = end + 1;
    }
    for (i, names) in nodes.names.iter().enumerate() {
        let v = z[i] as f32;
        for &m in names {
            view.set(m, v);
        }
    }
    // The numbers of a chain from its last solve.
    if second || !waited {
        measure(context, (c, &nodes), &l, &z, &h, state.cycle, out);
        out.metrics.approach_ended += ended.len();
        out.metrics.approach_ended_worst.extend(ended);
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
}

impl Span<'_> {
    fn solve(
        &self,
        fixed: &[f64],
        cause: &[Option<Fix>],
        (lo1, hi1, passed): (&[f64], &[f64], &[bool]),
        state: &mut Cycle,
        second: bool,
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
        let band: (Vec<f64>, Vec<f64>) =
            (h.iter().map(|h| h - BAND_M).collect(), h.iter().map(|h| h + BAND_M).collect());
        let mut lo3 = vec![f64::NEG_INFINITY; n];
        let mut covered = vec![false; n];
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
            }
        }
        let (mut slo, mut shi) = (vec![f64::NEG_INFINITY; n - 1], vec![f64::INFINITY; n - 1]);
        for s in 0..n - 1 {
            let first = self.nodes.first[r.start + s];
            if !fixed[s].is_nan() && !fixed[s + 1].is_nan() || context.ungraded.contains(&first) {
                continue;
            }
            if context.ramps.contains(&first) {
                let rise = (h[s + 1] - h[s]) / l[s];
                (slo[s], shi[s]) = (rise - RAMP_GRADE, rise + RAMP_GRADE);
            } else if class(s).grade.is_finite() {
                (slo[s], shi[s]) = (-class(s).grade, class(s).grade);
            }
        }
        let infinite = vec![f64::INFINITY; n];
        let steps = [
            Step::Fixed(fixed),
            Step::Bounds { layer: 1, lo: &lo1[r.clone()], hi: &hi1[r.clone()] },
            Step::Bounds { layer: 2, lo: &band.0, hi: &band.1 },
            Step::Bounds { layer: 3, lo: &lo3, hi: &infinite },
            Step::Slopes { layer: 4, lo: &slo, hi: &shi },
        ];
        let tube = chain::tube(&soft, &steps);
        let profile = match chain::profile(&soft, &tube, None) {
            Ok(p) => p,
            Err(_) => {
                out.metrics.pivots += 1;
                return z0;
            }
        };
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
            }
            if profile.at_lo[i] || profile.at_hi[i] {
                out.held.extend(self.nodes.names[r.start + i].iter().copied());
            }
        }
        #[cfg(test)]
        out.pieces.push(Piece { h: h.to_vec(), l: l.to_vec(), tube, z: z.clone() });
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
    use super::super::tests::{corner_network, engelberg, plain, table, way};
    use super::*;

    /// Phases A and B of features on one terrain, by area, on `threads` threads.
    fn ground(features: &[Feature], terrain: &dyn Terrain, threads: usize) -> (Heights, Chains) {
        let refs: Vec<&Feature> = features.iter().collect();
        let classes = table().1;
        let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
            let net = Network::new(subset, &classes).unwrap();
            area_facts(&net, &classes, terrain, (&[], &[]), chosen, &|xy| area_of(xy) == area)
        };
        let facts = AreaFacts::join(each_area(&refs, &[], usize::MAX, &job));
        let chains = assemble(&refs, &facts.facts);
        (Heights::solve(&refs, &classes, terrain, &chains, &facts, threads), chains)
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
        assert_eq!(shared.len(), 5);
        assert!(shared.iter().all(|v| v.iter().all(|&b| b == v[0])), "{shared:?}");
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
}
