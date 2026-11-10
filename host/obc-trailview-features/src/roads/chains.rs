//! The chains of the road heights and the reads between them: per area the facts at the points that it
//! owns, and for the bake the chains that the facts join and the order in which they are solved.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::network::{self, distinct, hypot, sub, Network};
use super::rows;
use super::{Classes, Feature, River, CULVERT_M, CULVERT_RISE_M};
use crate::geometry::key;
use crate::graph::components;
use crate::{metres, Terrain};

/// A vertex of a way: (feature, part, vertex of the part after `network::distinct`).
pub type Place = (u32, u32, u32);
/// A direction out of a place along its way: +1 to the next vertex, −1 to the one before.
pub type Arm = (Place, i8);
/// A node of a way: (feature, part, vertex of the part after `network::distinct`, cut after that vertex;
/// 0 = the vertex itself).
pub type Node = (u32, u32, u32, u16);

/// The names in the bake of the places, arms and nodes of a job's network.
pub(crate) struct Names<'a> {
    net: &'a Network,
    feature_of: &'a [usize],
    /// Per network vertex its way, and the OSM occurrence at or before it.
    way: Vec<usize>,
    osm_at: Vec<usize>,
}

impl<'a> Names<'a> {
    pub fn new(net: &'a Network, feature_of: &'a [usize]) -> Self {
        let total = net.vertex_xy.len();
        let mut way = vec![0; total];
        for w in 0..net.way_first.len() {
            way[net.way_first[w]..=net.way_last[w]].fill(w);
        }
        let mut osm_at = vec![0; total];
        for (o, &v) in net.osm_position.iter().enumerate() {
            osm_at[v] = o;
        }
        for v in 1..total {
            if !net.vertex_osm[v] {
                osm_at[v] = osm_at[v - 1];
            }
        }
        Names { net, feature_of, way, osm_at }
    }

    fn place(&self, o: usize) -> Place {
        let w = self.net.osm_way[o];
        let (f, p) = self.net.way_feature[w];
        (self.feature_of[f] as u32, p as u32, (o - self.net.osm_first[w]) as u32)
    }

    fn arm(&self, (o, step): (usize, i64)) -> Arm {
        (self.place(o), step as i8)
    }

    pub fn node(&self, v: usize) -> Node {
        let o = self.osm_at[v];
        let (f, p, k) = self.place(o);
        (f, p, k, (v - self.net.osm_position[o]) as u16)
    }

    /// The arm of the OSM segment of the network segment from vertex v; at a way's last vertex, of the
    /// one before it.
    fn segment(&self, v: usize) -> Arm {
        let v = if v == self.net.way_last[self.way[v]] { v - 1 } else { v };
        (self.place(self.osm_at[v]), 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ReadKind {
    Side,
    Copy,
    GroundPass,
    End,
    Clearance,
    Under,
    StructurePass,
    Branch,
    Hanger,
}

/// What one area knows about the chains at the points that it owns.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fact {
    /// The two arms of a ground point that one chain runs through. With one arm the chain ends there.
    Through { a: Arm, b: Option<Arm> },
    /// Two ends of free ways that one structure chain runs through; each arm points into its way.
    Link { a: Arm, b: Arm },
    /// The segment of `reader` takes a height from the segment of `from`.
    Read { kind: ReadKind, reader: Arm, from: Arm },
}

/// The facts of a job's network at the points that `owned` marks. `feature_of[f]` is the number of the
/// job's feature f in the bake.
pub(crate) fn facts(
    net: &Network,
    classes: &Classes,
    terrain: &dyn Terrain,
    rivers: &[&River],
    feature_of: &[usize],
    owned: &dyn Fn([f64; 2]) -> bool,
) -> Vec<Fact> {
    use ReadKind::*;
    let names = Names::new(net, feature_of);
    let arm = |o: (usize, i64)| names.arm(o);
    let segment = |v: usize| names.segment(v);
    let ordered = |a: Arm, b: Arm| if a <= b { (a, b) } else { (b, a) };
    let last_of = |w: usize| net.osm_first.get(w + 1).map_or(net.osm_xy.len(), |&f| f) - 1;
    let mut out = Vec::new();
    let mut push = |anchor: [f64; 2], fact: Fact| {
        if owned(anchor) {
            out.push(fact);
        }
    };

    let ground: Vec<bool> = net.free.iter().map(|&free| !free).collect();
    let (sides, pairs) = net.grouping(&net.directions(&ground), classes);
    let mut through_at: HashMap<usize, (usize, i64)> = HashMap::new();
    for pair in &pairs {
        let (a, b) = ordered(arm(pair[0]), arm(pair[1]));
        push(net.osm_xy[pair[0].0], Fact::Through { a, b: Some(b) });
        through_at.entry(net.osm_var[pair[0].0]).or_insert(pair[0]);
    }
    for i in 0..sides.occurrence.len() {
        let (o, [first, second]) = (sides.occurrence[i], sides.through[i]);
        let Some(first) = first else { continue };
        if second.is_none() {
            push(net.osm_xy[o], Fact::Through { a: arm(first), b: None });
        }
        push(net.osm_xy[o], Fact::Read { kind: Side, reader: arm((o, sides.step[i])), from: arm(first) });
        through_at.entry(net.osm_var[o]).or_insert(first);
    }

    let end_of = |(w, end): (usize, usize)| if end == 0 { (net.osm_first[w], 1) } else { (last_of(w), -1) };
    let mut linked = HashSet::new();
    for [p, q] in rows::chain_links(net, &net.free) {
        linked.extend([p, q]);
        let (a, b) = ordered(arm(end_of(p)), arm(end_of(q)));
        push(net.osm_xy[end_of(p).0], Fact::Link { a, b });
    }

    for (v, s) in rows::parallels(net, classes, terrain, false).1 {
        push(net.vertex_xy[v], Fact::Read { kind: Copy, reader: segment(v), from: segment(net.segment_start[s]) });
    }
    let (passes, uppers) = rows::passes(net, classes);
    for (i, &(upper, v)) in uppers.iter().enumerate() {
        let lower = passes.lower_way[i];
        let (over, under) = (segment(v), segment(passes.place[i].0));
        let (kind, reader, from) = match (net.free[upper], net.free[lower], net.tunnel[lower]) {
            (false, false, _) => (GroundPass, over, under),
            (false, true, true) => (Under, under, over),
            _ => (StructurePass, over, under),
        };
        push(net.vertex_xy[v], Fact::Read { kind, reader, from });
    }
    for (v, below) in rows::clearances(net, rivers, classes, terrain).2 {
        if let Some((_, k)) = below {
            push(net.vertex_xy[v], Fact::Read { kind: Clearance, reader: segment(v), from: segment(k) });
        }
    }

    // The ends of the free ways, and the ground ways that meet a structure inside it.
    let mut here: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for o in (0..net.osm_xy.len()).filter(|&o| net.junction[net.osm_var[o]]) {
        here.entry(net.osm_var[o]).or_default().push(o);
    }
    let arms_of = |o: usize| {
        let w = net.osm_way[o];
        [(o > net.osm_first[w]).then_some((o, -1)), (o < last_of(w)).then_some((o, 1))].into_iter().flatten()
    };
    let free_at = |o: usize| net.free[net.osm_way[o]];
    let least = |at: &[usize], keep: &dyn Fn(usize) -> bool| -> Option<Arm> {
        at.iter().filter(|&&o| keep(o)).flat_map(|&o| arms_of(o)).map(arm).min()
    };
    for w in (0..net.free.len()).filter(|&w| net.free[w]) {
        for end in [0, 1] {
            let (o, step) = end_of((w, end));
            let Some(at) = here.get(&net.osm_var[o]) else { continue };
            let reader = arm((o, step));
            // A structure reads the ground, or a branch its main chain, only where its chain ends.
            if linked.contains(&(w, end)) {
                continue;
            }
            if let Some(nearest) = least(at, &|q| !free_at(q)) {
                let from = through_at.get(&net.osm_var[o]).map_or(nearest, |&t| arm(t));
                push(net.osm_xy[o], Fact::Read { kind: End, reader, from });
            }
            if let Some(from) = least(at, &|q| free_at(q) && net.osm_way[q] != w) {
                push(net.osm_xy[o], Fact::Read { kind: Branch, reader, from });
            }
        }
    }
    let mut on = vec![0u32; net.count];
    for s in (0..net.a.len()).filter(|&s| net.free[net.segment_way[s]]) {
        on[net.a[s]] += 1;
        on[net.b[s]] += 1;
    }
    for (&var, at) in &here {
        let Some(from) = least(at, &free_at).filter(|_| on[var] >= 2) else { continue };
        for &o in at.iter().filter(|&&o| !free_at(o)) {
            for a in arms_of(o) {
                push(net.osm_xy[o], Fact::Read { kind: Hanger, reader: arm(a), from });
            }
        }
    }
    out
}

/// What one area knows about the ground heights at the points and nodes that it owns.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AreaFacts {
    /// Chains and reads, as in slice 0.
    pub facts: Vec<Fact>,
    /// A node of a side road that lies on the surface of its through road: (the node, the through arm at
    /// the junction). From the rows of `rows::approaches`.
    pub approach: Vec<(Node, Arm)>,
    /// A node that copies another way: (the node, the first node of the segment that it follows, the share
    /// along that segment). From `rows::parallels`, both ways round, so that either chain can lead.
    pub copies: Vec<(Node, Node, f32)>,
    /// Two ground ways of different layers that cross: the first nodes of the upper and of the lower
    /// segment, the shares along them, and the clearance. From `rows::passes`.
    pub passes: Vec<(Node, Node, f32, f32, f32)>,
    /// A cover: the first node of the crossing segment, the least height (water + cover), the most lift,
    /// and whether it is an untagged crossing. From `rows::covers`.
    pub covers: Vec<(Node, f64, f64, bool)>,
    /// The first nodes of the segments that are ramps, and of those that have no grade bound because they
    /// lie on an approach. From `rows::approaches`.
    pub ramps: Vec<Node>,
    pub ungraded: Vec<Node>,
}

impl AreaFacts {
    /// The facts of all areas: their lists joined, sorted, and each entry once.
    pub fn join(parts: Vec<AreaFacts>) -> AreaFacts {
        let mut all = AreaFacts::default();
        for part in parts {
            all.facts.extend(part.facts);
            all.approach.extend(part.approach);
            all.copies.extend(part.copies);
            all.passes.extend(part.passes);
            all.covers.extend(part.covers);
            all.ramps.extend(part.ramps);
            all.ungraded.extend(part.ungraded);
        }
        all.facts.sort();
        all.facts.dedup();
        all.approach.sort();
        all.approach.dedup();
        all.copies.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)).then(a.2.total_cmp(&b.2)));
        all.copies.dedup();
        let pass = |p: &(Node, Node, f32, f32, f32)| (p.0, p.1, p.2.to_bits(), p.3.to_bits(), p.4.to_bits());
        all.passes.sort_by_key(pass);
        all.passes.dedup();
        all.covers
            .sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)).then(a.2.total_cmp(&b.2)).then(a.3.cmp(&b.3)));
        all.covers.dedup();
        for list in [&mut all.ramps, &mut all.ungraded] {
            list.sort();
            list.dedup();
        }
        all
    }
}

/// Phase A of a job: its facts, and the nodes that the heights of the ground read, at the points and
/// nodes that `owned` marks.
pub(crate) fn area_facts(
    net: &Network,
    classes: &Classes,
    terrain: &dyn Terrain,
    (rivers, culverts): (&[&River], &[&River]),
    feature_of: &[usize],
    owned: &dyn Fn([f64; 2]) -> bool,
) -> AreaFacts {
    let names = Names::new(net, feature_of);
    let mine = |v: usize| owned(net.vertex_xy[v]);
    let mut out = AreaFacts { facts: facts(net, classes, terrain, rivers, feature_of, owned), ..Default::default() };
    let ground: Vec<bool> = net.free.iter().map(|&free| !free).collect();
    let (sides, _) = net.grouping(&net.directions(&ground), classes);
    let (_, approached, ramp, feet) = rows::approaches(net, &sides, classes);
    for (p, i) in feet.into_iter().filter(|&(p, _)| mine(p)) {
        if let Some(through) = sides.through[i][0] {
            out.approach.push((names.node(p), names.arm(through)));
        }
    }
    for (s, &v) in net.segment_start.iter().enumerate().filter(|&(_, &v)| mine(v)) {
        if ramp[s] {
            out.ramps.push(names.node(v));
        }
        if approached[s] {
            out.ungraded.push(names.node(v));
        }
    }
    let (beside, followed) = rows::parallels(net, classes, terrain, true);
    for (i, &(v, s)) in followed.iter().enumerate().filter(|(_, &(v, _))| mine(v)) {
        out.copies.push((names.node(v), names.node(net.segment_start[s]), -beside.coefficients[i][2] as f32));
    }
    let (passes, uppers) = rows::passes(net, classes);
    for (i, &(upper, v)) in uppers.iter().enumerate() {
        let (j, x) = passes.place[i];
        if !net.free[upper] && !net.free[passes.lower_way[i]] && mine(v) {
            let u = passes.upper[i].1[1];
            out.passes.push((names.node(v), names.node(j), u as f32, x as f32, passes.margin[i] as f32));
        }
    }
    let untagged = rows::untagged_crossings(net, rivers, classes);
    let (cover, ways, crossing, first) = rows::covers(net, (culverts, rivers), &untagged, classes, terrain);
    for i in (0..cover.values.len()).filter(|&i| !net.free[ways[i].0] && mine(first[i])) {
        out.covers.push((names.node(first[i]), cover.values[i], ways[i].1, crossing[i].is_some()));
    }
    out
}

/// Phase 0: per feature, whether it is a culvert, a short bridge on the terrain. A free feature that is no
/// tunnel, of one part shorter than CULVERT_M, whose two end points no other free feature has, and whose
/// straight line between the terrain at its ends lies no more than CULVERT_RISE_M over the terrain at each
/// of its nodes.
pub(crate) fn culverts(features: &[&Feature], terrain: &dyn Terrain) -> Vec<bool> {
    let mut free_at: HashMap<[u64; 2], BTreeSet<usize>> = HashMap::new();
    for (f, feature) in features.iter().enumerate().filter(|(_, f)| f.free) {
        for p in feature.parts.iter().flatten() {
            free_at.entry(key(*p)).or_default().insert(f);
        }
    }
    let alone = |f: usize, p: [f64; 2]| free_at.get(&key(p)).is_none_or(|on| on.iter().all(|&g| g == f));
    features
        .iter()
        .enumerate()
        .map(|(f, feature)| {
            if !feature.free || feature.tunnel || feature.parts.len() != 1 {
                return false;
            }
            let points = distinct(&feature.parts[0]);
            if points.len() < 2 || !alone(f, points[0]) || !alone(f, points[points.len() - 1]) {
                return false;
            }
            // The nodes of the part and the metres along it at each.
            let (mut nodes, mut along) = (vec![points[0]], vec![0.0]);
            for pair in points.windows(2) {
                let d = sub(pair[1], pair[0]);
                let length = hypot(d) * metres(pair[0][1] + d[1] / 2.0);
                let count = network::pieces(length, points.len() == 2);
                let at = along[along.len() - 1];
                for k in 1..=count {
                    let t = k as f64 / count as f64;
                    nodes.push([pair[0][0] + t * d[0], pair[0][1] + t * d[1]]);
                    along.push(at + t * length);
                }
            }
            let total = along[along.len() - 1];
            if total >= CULVERT_M {
                return false;
            }
            let h = terrain.heights(&nodes);
            let (h0, h1) = (h[0], h[h.len() - 1]);
            h.iter().zip(&along).all(|(&h, &s)| h.is_finite() && h0 + (h1 - h0) * s / total - h <= CULVERT_RISE_M)
        })
        .collect()
}

pub struct Chain {
    /// Its segments in order, each as the arm that leaves the segment's first vertex along the chain.
    pub arms: Vec<Arm>,
    pub structure: bool,
    pub closed: bool,
    pub key: [u64; 2],
    /// The key of its last point; of a closed chain, of its second point.
    pub last_key: [u64; 2],
    pub metres: f64,
    /// The best class (smallest rule) of its ways.
    pub rule: usize,
}

pub struct Chains {
    pub chains: Vec<Chain>,
    /// (reader, read from, kind), each pair and kind once. A chain can read itself.
    pub reads: Vec<(usize, usize, ReadKind)>,
    /// The strongly connected components of the reads, each after every component that it reads.
    pub order: Vec<Vec<usize>>,
    /// Per component: 0 if it reads nothing, else 1 + the largest depth of what it reads.
    pub depth: Vec<usize>,
    /// Arms that two facts give different partners: 0 when the facts are local.
    pub conflicts: usize,
}

/// The OSM segments of the features: per way part its points, and the number of its first segment.
pub(crate) struct Segments<'a> {
    features: &'a [&'a Feature],
    pub parts: Vec<(usize, usize, Vec<[f64; 2]>, usize)>,
    pub index: HashMap<(u32, u32), usize>,
    /// Per segment its part.
    pub part: Vec<usize>,
}

/// An end of a segment (segment, 0 its first vertex or 1 its last).
pub(crate) type Tip = (usize, usize);

impl<'a> Segments<'a> {
    pub fn new(features: &'a [&'a Feature]) -> Self {
        let (mut parts, mut index, mut part) = (Vec::new(), HashMap::new(), Vec::new());
        for (f, feature) in features.iter().enumerate() {
            for (p, points) in feature.parts.iter().enumerate() {
                let points = distinct(points);
                if points.len() >= 2 {
                    index.insert((f as u32, p as u32), parts.len());
                    let first = part.len();
                    part.extend(std::iter::repeat_n(parts.len(), points.len() - 1));
                    parts.push((f, p, points, first));
                }
            }
        }
        Segments { features, parts, index, part }
    }

    /// The end of the segment that an arm points into.
    pub fn end(&self, ((f, p, k), step): Arm) -> Tip {
        let first = self.parts[self.index[&(f, p)]].3;
        if step > 0 {
            (first + k as usize, 0)
        } else {
            (first + k as usize - 1, 1)
        }
    }

    /// The arm at an end of a segment, pointing into it.
    pub fn arm(&self, (s, e): Tip) -> Arm {
        let (f, p, _, first) = &self.parts[self.part[s]];
        let k = (s - first + e) as u32;
        ((*f as u32, *p as u32, k), if e == 0 { 1 } else { -1 })
    }

    pub fn point(&self, ((f, p, k), _): Arm) -> [f64; 2] {
        self.parts[self.index[&(f, p)]].2[k as usize]
    }

    pub fn feature(&self, s: usize) -> &Feature {
        self.features[self.parts[self.part[s]].0]
    }

    pub fn metres(&self, s: usize) -> f64 {
        let (_, _, points, first) = &self.parts[self.part[s]];
        let (a, b) = (points[s - first], points[s - first + 1]);
        let d = sub(b, a);
        hypot(d) * metres(a[1] + d[1] / 2.0)
    }

    /// The other arm of the vertex of an end inside its way.
    fn across(&self, (s, e): Tip) -> Option<Tip> {
        let (_, _, points, first) = &self.parts[self.part[s]];
        let k = s - first + e;
        (k > 0 && k + 1 < points.len()).then(|| if e == 1 { (s + 1, 0) } else { (s - 1, 1) })
    }
}

/// The chains that the facts join, and their reads and order. A chain starts at its end with the
/// smaller point key; a closed chain at its vertex with the smallest key, toward the smaller of its two
/// neighbours.
pub fn assemble(features: &[&Feature], facts: &[Fact]) -> Chains {
    let segments = Segments::new(features);
    let mut facts = facts.to_vec();
    facts.sort();
    facts.dedup();
    let (mut partner, mut named, mut conflicts) = (HashMap::new(), HashSet::new(), 0);
    for fact in &facts {
        match *fact {
            Fact::Through { a, b: Some(b) } | Fact::Link { a, b } => {
                let (x, y) = (segments.end(a), segments.end(b));
                let taken = [(x, y), (y, x)].iter().filter(|(p, q)| partner.get(p).is_some_and(|r| r != q)).count();
                if taken > 0 {
                    conflicts += taken;
                } else {
                    partner.insert(x, y);
                    partner.insert(y, x);
                }
            }
            Fact::Through { a, b: None } | Fact::Read { kind: ReadKind::Side, reader: a, .. } => {
                named.insert(segments.end(a));
            }
            Fact::Read { .. } => {}
        }
    }
    // The next end along a chain: by a fact, or the other arm of a vertex inside a way that no fact names.
    let next = |end: Tip| -> Option<Tip> {
        if let Some(&other) = partner.get(&end) {
            return Some(other);
        }
        let other = segments.across(end)?;
        (!named.contains(&end) && !named.contains(&other) && !partner.contains_key(&other)).then_some(other)
    };
    let count = segments.part.len();
    let mut chain_of = vec![usize::MAX; count];
    let mut chains = Vec::new();
    for start in 0..count {
        if chain_of[start] != usize::MAX {
            continue;
        }
        // Back to the first segment, entered at end `from`; or round to `start`: a closed chain.
        let (mut first, mut from, mut closed) = (start, 0, false);
        for _ in 0..count {
            let Some((s, e)) = next((first, from)) else { break };
            if s == start {
                closed = true;
                break;
            }
            (first, from) = (s, 1 - e);
        }
        let (mut s, mut e) = if closed { (start, 0) } else { (first, from) };
        let mut ends = Vec::new();
        loop {
            chain_of[s] = chains.len();
            ends.push((s, e));
            match next((s, 1 - e)) {
                Some((t, f)) if chain_of[t] == usize::MAX => (s, e) = (t, f),
                _ => break,
            }
        }
        chains.push(orient(&segments, ends, closed));
    }
    let chain_of = |arm: Arm| chain_of[segments.end(arm).0];
    let rank = |c: usize| (chains[c].rule, chains[c].key, chains[c].last_key, c);
    let sides: HashSet<(usize, usize)> = facts
        .iter()
        .filter_map(|fact| match *fact {
            Fact::Read { kind: ReadKind::Side, reader, from } => Some((chain_of(reader), chain_of(from))),
            _ => None,
        })
        .collect();
    let mut reads = BTreeSet::new();
    for fact in &facts {
        let Fact::Read { kind, reader, from } = *fact else { continue };
        let (mut r, mut f) = (chain_of(reader), chain_of(from));
        if matches!(kind, ReadKind::Copy | ReadKind::GroundPass) {
            // The reader of a copy is the chain that reads the other as a side, where only one of the two
            // does: at an acute junction the side's carriageway overlaps its through road. Else the larger
            // rank.
            let (rf, fr) = (sides.contains(&(r, f)), sides.contains(&(f, r)));
            let side_first = kind == ReadKind::Copy && rf != fr;
            if (side_first && fr) || (!side_first && rank(r) < rank(f)) {
                (r, f) = (f, r);
            }
            if kind == ReadKind::Copy && r == f {
                continue;
            }
        }
        reads.insert((r, f, kind));
    }
    let mut after = vec![Vec::new(); chains.len()];
    for &(r, f, _) in &reads {
        after[r].push(f);
    }
    after.iter_mut().for_each(|a| a.dedup());
    let order: Vec<Vec<usize>> = components(&after.iter().map(Vec::as_slice).collect::<Vec<_>>())
        .into_iter()
        .map(|mut c| {
            c.sort_by_key(|&x| rank(x));
            c
        })
        .collect();
    let depth = depths(&order, &after);
    Chains { chains, reads: reads.into_iter().collect(), order, depth, conflicts }
}

/// A walk of segment ends as a chain, turned to start at its end with the smaller key.
fn orient(segments: &Segments, mut ends: Vec<Tip>, closed: bool) -> Chain {
    let flip = |ends: &[Tip]| -> Vec<Tip> { ends.iter().rev().map(|&(s, e)| (s, 1 - e)).collect() };
    let key_of = |end: Tip| key(segments.point(segments.arm(end)));
    if closed {
        // Each start and direction: the key of its vertex, then of the vertex after it.
        let n = ends.len();
        let back = flip(&ends);
        let lead = |ends: &[Tip], i: usize| (key_of(ends[i]), key_of(ends[(i + 1) % n]));
        let best =
            (0..n).map(|i| (lead(&ends, i), false, i)).chain((0..n).map(|i| (lead(&back, i), true, i))).min().unwrap();
        let turned = if best.1 { back } else { ends };
        ends = turned[best.2..].iter().chain(&turned[..best.2]).copied().collect();
    } else {
        let &(s, e) = ends.last().unwrap();
        if key_of((s, 1 - e)) < key_of(ends[0]) {
            ends = flip(&ends);
        }
    }
    Chain {
        arms: ends.iter().map(|&end| segments.arm(end)).collect(),
        structure: segments.feature(ends[0].0).free,
        closed,
        key: key_of(ends[0]),
        last_key: if closed {
            key_of(ends[1 % ends.len()])
        } else {
            key_of((ends[ends.len() - 1].0, 1 - ends[ends.len() - 1].1))
        },
        metres: ends.iter().map(|&(s, _)| segments.metres(s)).sum(),
        rule: ends.iter().map(|&(s, _)| segments.feature(s).rule).min().unwrap(),
    }
}

/// Per component of `order`: 0 if it reads nothing outside itself, else 1 + the largest depth of what it
/// reads.
pub(crate) fn depths(order: &[Vec<usize>], after: &[Vec<usize>]) -> Vec<usize> {
    let mut component = vec![0; after.len()];
    for (i, members) in order.iter().enumerate() {
        members.iter().for_each(|&c| component[c] = i);
    }
    let mut depth = vec![0; order.len()];
    for (i, members) in order.iter().enumerate() {
        let read = members.iter().flat_map(|&c| &after[c]).map(|&c| component[c]).filter(|&j| j != i);
        depth[i] = read.map(|j| depth[j] + 1).max().unwrap_or(0);
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::super::area::{area_of, each_area, Area};
    use super::super::census::{census, chain_census};
    use super::super::tests::{bridge, corner_network, engelberg, plain, table, tunnel, way};
    use super::super::Surroundings;
    use super::*;

    /// The facts of one network of all features, owned all, and their chains.
    fn build(features: &[Feature], terrain: &dyn Terrain) -> (Vec<Fact>, Chains) {
        let refs: Vec<&Feature> = features.iter().collect();
        let classes = table().1;
        let net = Network::new(&refs, &classes).unwrap();
        let numbers: Vec<usize> = (0..refs.len()).collect();
        let facts = facts(&net, &classes, terrain, &[], &numbers, &|_| true);
        let chains = assemble(&refs, &facts);
        (facts, chains)
    }

    fn reads(chains: &Chains, kind: ReadKind) -> Vec<(usize, usize)> {
        chains.reads.iter().filter(|r| r.2 == kind).map(|r| (r.0, r.1)).collect()
    }

    /// The chain of the segment that leaves vertex k of feature f.
    fn chain_at(chains: &Chains, f: u32, k: u32) -> usize {
        let has = |c: &Chain| {
            c.arms.iter().any(|&((g, _, j), step)| g == f && (j == k && step == 1 || j == k + 1 && step == -1))
        };
        chains.chains.iter().position(has).unwrap()
    }

    fn component_of(chains: &Chains, c: usize) -> usize {
        chains.order.iter().position(|m| m.contains(&c)).unwrap()
    }

    #[test]
    fn a_ring_of_a_better_class_is_one_closed_chain() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let features = [
            way("tertiary", place.line(&[(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0), (0.0, 0.0)], None), plain),
            way("residential", place.line(&[(-50.0, 0.0), (0.0, 0.0)], None), plain),
            way("residential", place.line(&[(70.0, 20.0), (20.0, 20.0)], None), plain),
        ];
        let (_, chains) = build(&features, &terrain);
        assert_eq!(chains.chains.len(), 3);
        let ring = chain_at(&chains, 0, 0);
        let c = &chains.chains[ring];
        assert!(c.closed && (c.metres - 80.0).abs() < 0.01, "{} m", c.metres);
        let corners: Vec<[f64; 2]> =
            [(0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0)].map(|(e, n)| place.at(e, n)).to_vec();
        let least = corners.iter().map(|&p| key(p)).min().unwrap();
        assert_eq!(c.key, least);
        let ((_, _, k), _) = c.arms[0];
        assert_eq!(key(features[0].parts[0][k as usize]), least);
        let streets = [chain_at(&chains, 1, 0), chain_at(&chains, 2, 0)];
        assert_eq!(reads(&chains, ReadKind::Side), vec![(streets[0], ring), (streets[1], ring)]);
        assert_eq!(chains.order.len(), 3);
        assert_eq!(chains.order[0], vec![ring]);
        assert_eq!(chains.depth, vec![0, 1, 1]);
    }

    #[test]
    fn a_way_that_ends_on_its_own_middle_reads_itself() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let points = place.line(&[(-50.0, 0.0), (0.0, 0.0), (20.0, 0.0), (20.0, 20.0), (0.0, 20.0), (0.0, 0.0)], None);
        let (_, chains) = build(&[way("residential", points, plain)], &terrain);
        assert_eq!(chains.chains.len(), 1);
        let c = &chains.chains[0];
        assert!(!c.closed && (c.metres - 130.0).abs() < 0.01, "{} m", c.metres);
        assert_eq!(chains.reads, vec![(0, 0, ReadKind::Side)]);
        assert_eq!(chains.order.len(), 1);
    }

    #[test]
    fn two_streets_that_end_on_each_other_are_one_cycle() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let features = [
            way("residential", place.line(&[(-50.0, 0.0), (0.0, 0.0), (50.0, 0.0), (50.0, 40.0)], None), plain),
            way("residential", place.line(&[(100.0, 40.0), (50.0, 40.0), (0.0, 40.0), (0.0, 0.0)], None), plain),
        ];
        let (_, chains) = build(&features, &terrain);
        assert_eq!(chains.chains.len(), 2);
        let (a, b) = (chain_at(&chains, 0, 0), chain_at(&chains, 1, 0));
        let mut side = reads(&chains, ReadKind::Side);
        side.sort();
        let mut want = vec![(a, b), (b, a)];
        want.sort();
        assert_eq!(side, want);
        assert_eq!(chains.order, vec![vec![a.min(b), a.max(b)]]);
    }

    #[test]
    fn a_double_track_split_in_alternating_order_has_one_leader() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let track = |from: (f64, f64), to: (f64, f64)| way("rail", place.line(&[from, to], Some(20.0)), plain);
        let features = [
            track((0.0, 0.0), (200.0, 0.0)),
            track((0.0, 4.0), (200.0, 4.0)),
            track((200.0, 4.0), (400.0, 4.0)),
            track((200.0, 0.0), (400.0, 0.0)),
        ];
        let (facts, chains) = build(&features, &terrain);
        assert_eq!(chains.chains.len(), 2);
        assert!(chains.chains.iter().all(|c| (c.metres - 400.0).abs() < 0.01));
        let copies = reads(&chains, ReadKind::Copy);
        assert_eq!(copies.len(), 1, "{copies:?}");
        let (reader, from) = copies[0];
        assert!(chains.chains[reader].key > chains.chains[from].key);
        assert_eq!(chains.order.len(), 2);
        let refs: Vec<&Feature> = features.iter().collect();
        let numbers = census(&refs, facts, &terrain);
        assert_eq!(
            (numbers["copies"]["pairs"].as_u64(), numbers["copies"]["switching_pairs"].as_u64()),
            (Some(1), Some(1))
        );
    }

    #[test]
    fn three_stacked_ramps_are_solved_from_the_bottom_up() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let ramp = |degrees: f64, layer: i64| {
            let (e, n) = (50.0 * degrees.to_radians().cos(), 50.0 * degrees.to_radians().sin());
            // From 45 m before (0, 0) to 55 m past it: no vertex lies on the crossing.
            way("primary", place.line(&[(-0.9 * e, -0.9 * n), (1.1 * e, 1.1 * n)], None), |f| {
                bridge(f);
                f.layered = true;
                f.layer = layer;
            })
        };
        let features = [ramp(0.0, 1), ramp(60.0, 2), ramp(120.0, 3)];
        let (_, chains) = build(&features, &terrain);
        assert_eq!(chains.chains.len(), 3);
        assert!(chains.chains.iter().all(|c| c.structure));
        let [one, two, three] = [0, 1, 2].map(|f| chain_at(&chains, f, 0));
        let mut passes = reads(&chains, ReadKind::StructurePass);
        passes.sort();
        let mut want = vec![(two, one), (three, two), (three, one)];
        want.sort();
        assert_eq!(passes, want);
        assert_eq!(chains.order, vec![vec![one], vec![two], vec![three]]);
        assert_eq!(chains.depth, vec![0, 1, 2]);
    }

    #[test]
    fn a_branch_reads_its_main_tunnel() {
        let place = engelberg();
        let terrain = place.terrain(|_, north| 1000.0 + 2.0 * north);
        let features = [
            way("primary", place.line(&[(0.0, 0.0), (200.0, 0.0), (400.0, 0.0)], None), tunnel),
            way("primary", place.line(&[(-50.0, 0.0), (0.0, 0.0)], None), plain),
            way("primary", place.line(&[(400.0, 0.0), (450.0, 0.0)], None), plain),
            way("steps", place.line(&[(200.0, 0.0), (200.0, 30.0)], None), tunnel),
            way("path", place.line(&[(200.0, 30.0), (200.0, 80.0)], None), plain),
        ];
        let (facts, chains) = build(&features, &terrain);
        let structures: Vec<usize> = (0..chains.chains.len()).filter(|&c| chains.chains[c].structure).collect();
        assert_eq!(structures.len(), 2);
        let [main, west, east, steps, path] = [0, 1, 2, 3, 4].map(|f| chain_at(&chains, f, 0));
        assert_eq!(reads(&chains, ReadKind::Branch), vec![(steps, main)]);
        let mut ends = reads(&chains, ReadKind::End);
        ends.sort();
        let mut want = vec![(main, west), (main, east), (steps, path)];
        want.sort();
        assert_eq!(ends, want);
        let refs: Vec<&Feature> = features.iter().collect();
        assert_eq!(census(&refs, facts, &terrain)["structures"]["shafts"].as_u64(), Some(1));
        assert!(component_of(&chains, steps) > component_of(&chains, main));
    }

    #[test]
    fn the_facts_of_four_areas_are_the_facts_of_one() {
        let place = engelberg();
        let (features, rivers, terrain) = corner_network(&place);
        let refs: Vec<&Feature> = features.iter().collect();
        let near: Vec<&River> = rivers.iter().collect();
        let classes = table().1;
        let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
            let net = Network::new(subset, &classes).unwrap();
            facts(&net, &classes, &terrain, &near, chosen, &|xy| area_of(xy) == area)
        };
        let jobs = each_area(&refs, &[], usize::MAX, &job);
        assert_eq!(jobs.len(), 4);
        let mut apart: Vec<Fact> = jobs.concat();
        apart.sort();
        let net = Network::new(&refs, &classes).unwrap();
        let mut whole = facts(&net, &classes, &terrain, &near, &(0..refs.len()).collect::<Vec<_>>(), &|_| true);
        whole.sort();
        assert!(!whole.is_empty());
        assert_eq!(apart, whole);
        let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
            let net = Network::new(subset, &classes).unwrap();
            area_facts(&net, &classes, &terrain, (&near, &[]), chosen, &|xy| area_of(xy) == area)
        };
        let apart = AreaFacts::join(each_area(&refs, &[], usize::MAX, &job));
        let one = area_facts(&net, &classes, &terrain, (&near, &[]), &(0..refs.len()).collect::<Vec<_>>(), &|_| true);
        let one = AreaFacts::join(vec![one]);
        assert!(!one.approach.is_empty(), "{one:?}");
        assert_eq!(apart, one);
        let around = Surroundings { rivers, ..Default::default() };
        let numbers = chain_census(&refs, &[], &terrain, &around, &classes, usize::MAX);
        assert_eq!((numbers["facts"]["conflicts"].as_u64(), numbers["jobs"]["areas"].as_u64()), (Some(0), Some(4)));
    }

    #[test]
    fn a_side_road_that_runs_into_its_through_road_at_a_small_angle_copies_it_and_makes_no_cycle() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let features = [
            way("residential", place.line(&[(-100.0, 0.0), (0.0, 0.0), (100.0, 0.0)], Some(10.0)), plain),
            way("residential", place.line(&[(-80.0, 10.0), (0.0, 0.0)], Some(10.0)), plain),
        ];
        let (_, chains) = build(&features, &terrain);
        assert_eq!(chains.chains.len(), 2);
        let (y, x) = (chain_at(&chains, 0, 0), chain_at(&chains, 1, 0));
        assert_eq!(reads(&chains, ReadKind::Side), vec![(x, y)]);
        assert_eq!(reads(&chains, ReadKind::Copy), vec![(x, y)]);
        assert!(chains.reads.iter().all(|r| (r.0, r.1) != (y, x)), "{:?}", chains.reads);
        assert_eq!(chains.order.len(), 2);
    }

    #[test]
    fn a_ground_way_at_the_joint_of_two_linked_bridges_hangs_and_the_bridges_do_not_read_it() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let features = [
            way("primary", place.line(&[(-50.0, 0.0), (0.0, 0.0)], None), plain),
            way("primary", place.line(&[(0.0, 0.0), (50.0, 0.0)], None), bridge),
            way("primary", place.line(&[(50.0, 0.0), (100.0, 0.0)], None), bridge),
            way("primary", place.line(&[(100.0, 0.0), (150.0, 0.0)], None), plain),
            way("footway", place.line(&[(50.0, 0.0), (50.0, -40.0)], None), plain),
        ];
        let (facts, chains) = build(&features, &terrain);
        let structures: Vec<usize> = (0..chains.chains.len()).filter(|&c| chains.chains[c].structure).collect();
        assert_eq!(structures.len(), 1);
        let deck = structures[0];
        assert!((chains.chains[deck].metres - 100.0).abs() < 0.01, "{} m", chains.chains[deck].metres);
        let [west, east, footway] = [0, 3, 4].map(|f| chain_at(&chains, f, 0));
        let mut ends = reads(&chains, ReadKind::End);
        ends.sort();
        let mut want = vec![(deck, west), (deck, east)];
        want.sort();
        assert_eq!(ends, want);
        let joint = (1, 0, 1);
        assert!(!facts
            .iter()
            .any(|f| matches!(f, Fact::Read { kind: ReadKind::End, reader: (place, _), .. } if *place == joint)));
        assert_eq!(reads(&chains, ReadKind::Hanger), vec![(footway, deck)]);
        assert!(chains.order.iter().all(|m| m.len() == 1));
    }

    #[test]
    fn a_side_road_at_a_bridge_head_reads_the_road_not_the_bridge() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let features = [
            way("primary", place.line(&[(-50.0, 0.0), (0.0, 0.0)], None), plain),
            way("primary", place.line(&[(0.0, 0.0), (60.0, 0.0)], None), bridge),
            way("primary", place.line(&[(60.0, 0.0), (110.0, 0.0)], None), plain),
            way("residential", place.line(&[(0.0, 0.0), (0.0, -50.0)], None), plain),
        ];
        let (_, chains) = build(&features, &terrain);
        let [west, deck, east, side] = [0, 1, 2, 3].map(|f| chain_at(&chains, f, 0));
        let mut ends = reads(&chains, ReadKind::End);
        ends.sort();
        let mut want = vec![(deck, west), (deck, east)];
        want.sort();
        assert_eq!(ends, want);
        assert_eq!(reads(&chains, ReadKind::Side), vec![(side, west)]);
        assert!(reads(&chains, ReadKind::Hanger).is_empty());
        assert!(chains.order.iter().all(|m| m.len() == 1));
    }

    #[test]
    fn the_chains_of_a_cycle_are_in_rank_order() {
        let place = engelberg();
        let terrain = place.terrain(|_, _| 1000.0);
        let features = [
            way("residential", place.line(&[(-50.0, 0.0), (0.0, 0.0), (50.0, 0.0), (50.0, 40.0)], None), plain),
            way("residential", place.line(&[(100.0, 40.0), (50.0, 40.0), (0.0, 40.0), (0.0, 0.0)], None), plain),
        ];
        let (_, chains) = build(&features, &terrain);
        let rank = |c: usize| {
            let chain = &chains.chains[c];
            (chain.rule, chain.key, chain.last_key, c)
        };
        assert_eq!(chains.order.len(), 1);
        let cycle = &chains.order[0];
        assert_eq!(cycle.len(), 2);
        assert!(rank(cycle[0]) < rank(cycle[1]), "{cycle:?}");
    }

    #[test]
    fn a_short_bridge_on_the_terrain_is_a_culvert_and_one_over_a_gorge_is_not() {
        let place = engelberg();
        for (depth, culvert) in [(0.5, true), (4.0, false)] {
            let terrain = place.terrain(move |east, _| 1000.0 - if east.abs() < 2.0 { depth } else { 0.0 });
            let features = [
                way("residential", place.line(&[(-50.0, 0.0), (-3.0, 0.0)], None), plain),
                way("residential", place.line(&[(-3.0, 0.0), (3.0, 0.0)], None), bridge),
                way("residential", place.line(&[(3.0, 0.0), (50.0, 0.0)], None), plain),
            ];
            let refs: Vec<&Feature> = features.iter().collect();
            assert_eq!(culverts(&refs, &terrain), [false, culvert, false], "a ditch {depth} m deep");
        }
    }
}
