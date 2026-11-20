//! The tunnel parts that the bake places: per chain of tunnel ways its openings, the kind of part at each and its
//! numbers from the ground of the ground plan there, written as the features `tunnel-tube` and `tunnel-end` and the
//! tag `parts` of the tunnel ways (`specs/trailview-features.md`, Tunnel parts).

mod cut;
mod floor;
mod ladder;
mod measure;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

use obc_trailview_parts::{Bake, Ladder, Rules};

use obc_ground_plan::graph::OpenEnd;
use obc_ground_plan::sections::{is_track, WidthRules};
use serde_json::{json, Map, Value};

use crate::geometry::key;
use crate::roads::Classes;
use crate::tiles::{frame_point, Tiles};
use crate::{metres, roads, Part, Tags, Terrain};

pub use ladder::Kind;

/// What the pass reads: the ways of kind `tunnel` that the zoom-13 tiles hold with an id (their tags and parts as
/// placed), every way of the road solver (kind, tags, parts) and the deck depth of each bridge way by its place, the
/// tiles and their tags of the levels, the terrain, the rules and the road classes.
pub(crate) struct Input<'a> {
    pub tunnels: &'a HashMap<u32, (Tags, Vec<Part>)>,
    pub ways: Vec<(&'a str, &'a Tags, &'a [Part])>,
    pub decks: &'a HashMap<usize, f64>,
    /// The ways that the tiles hold as the kind `tunnel`, with an id or none, by their place in `ways`.
    pub tunnel_ways: &'a HashSet<usize>,
    pub tiles: &'a Tiles,
    pub levels: &'a HashMap<u32, Tags>,
    pub terrain: &'a dyn Terrain,
    pub rules: &'a Ladder,
    pub widths: &'a WidthRules,
    pub class_names: &'a [String],
    pub classes: &'a Classes,
}

/// What the pass gives: the features to place (kind, tags, the part with the floor heights), the tag `parts` of each
/// tunnel way by id, and its counts.
pub(crate) struct Placed {
    pub features: Vec<(&'static str, Tags, Part)>,
    pub tags: HashMap<u32, Tags>,
    pub metrics: Value,
    /// Per opening its way's id and its decision.
    #[cfg(test)]
    pub ends: Vec<(u32, ladder::End)>,
}

/// A point of a chain's curve: world point, floor height, metres along the chain, and metres along the stroke of its
/// way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Point {
    pub p: [f64; 2],
    pub z: f64,
    pub s: f64,
    pub along: f64,
}

/// The curve of a chain: per way of the chain in its order, the way's id and its points from where the chain enters
/// it to where it leaves; consecutive stretches share their node.
#[derive(Clone, Debug, Default)]
pub(crate) struct Curve {
    pub stretches: Vec<(u32, Vec<Point>)>,
    pub len: f64,
}

impl Curve {
    /// The point at metres s along the chain (clamped to it), and the way and the chord there.
    pub fn at(&self, s: f64) -> (Point, u32) {
        let s = s.clamp(0.0, self.len);
        for (way, ps) in &self.stretches {
            for w in ps.windows(2) {
                if s <= w[1].s {
                    let t = if w[1].s > w[0].s { ((s - w[0].s) / (w[1].s - w[0].s)).clamp(0.0, 1.0) } else { 0.0 };
                    let mix = |a: f64, b: f64| a + t * (b - a);
                    let p = [mix(w[0].p[0], w[1].p[0]), mix(w[0].p[1], w[1].p[1])];
                    return (Point { p, z: mix(w[0].z, w[1].z), s, along: mix(w[0].along, w[1].along) }, *way);
                }
            }
        }
        let (way, ps) = self.stretches.last().unwrap();
        (*ps.last().unwrap(), *way)
    }

    /// The unit direction of rising metres at s, in world units (Mercator keeps the angles): over a metre each way.
    pub fn dir(&self, s: f64) -> [f64; 2] {
        let (a, b) = (self.at(s - 1.0).0.p, self.at(s + 1.0).0.p);
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = d[0].hypot(d[1]).max(f64::MIN_POSITIVE);
        [d[0] / l, d[1] / l]
    }
}

/// The point `metres` metres from p along the unit direction d.
pub(crate) fn offset(p: [f64; 2], d: [f64; 2], metres_: f64) -> [f64; 2] {
    let u = metres_ / metres(p[1]);
    [p[0] + d[0] * u, p[1] + d[1] * u]
}

/// The left of a unit direction, x east and y south.
pub(crate) fn left_of(d: [f64; 2]) -> [f64; 2] {
    [d[1], -d[0]]
}

/// Metres from a to b.
pub(crate) fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1]) * metres((a[1] + b[1]) / 2.0)
}

/// The metres of b − a along the unit direction d.
pub(crate) fn along_dir(a: [f64; 2], b: [f64; 2], d: [f64; 2]) -> f64 {
    ((b[0] - a[0]) * d[0] + (b[1] - a[1]) * d[1]) * metres((a[1] + b[1]) / 2.0)
}

/// The curve of a part of a way, with the metres along its stroke at each point: `roads::polyline`, whose points
/// hold the way's vertices, with the metres along the chords between them.
fn way_curve(part: &Part) -> Option<Vec<Point>> {
    let plan = part.plan.as_ref()?;
    let poly = roads::polyline(part);
    let z = poly.z.as_ref()?;
    let mut out: Vec<Point> = Vec::with_capacity(poly.points.len());
    let mut at = 0;
    for k in 0..part.points.len() {
        let start = if k == 0 { 0 } else { at + 1 };
        let next = (start..poly.points.len()).find(|&j| poly.points[j] == part.points[k])?;
        // The chords from the vertex before to this one share the metres between their two vertices.
        let chord = |j: usize| distance(poly.points[j - 1], poly.points[j]);
        let total: f64 = (start.max(1)..=next).map(chord).sum();
        let mut run = 0.0;
        for (j, (&p, &zj)) in poly.points.iter().zip(z).enumerate().take(next + 1).skip(start) {
            if j > 0 && k > 0 {
                run += chord(j);
            }
            let along = if k == 0 || total <= 0.0 {
                plan[k].along
            } else {
                plan[k - 1].along + (plan[k].along - plan[k - 1].along) * (run / total)
            };
            out.push(Point { p, z: zj, s: 0.0, along });
        }
        at = next;
    }
    Some(out)
}

/// A chain of tunnel ways with its curve, the end vertex of each end in the ground plan's frame and the way there, and
/// its section.
#[derive(Clone, Debug)]
pub(crate) struct Tube {
    pub curve: Curve,
    pub ids: Vec<u32>,
    /// Every vertex has a height.
    pub heights: bool,
    pub ends: [([i64; 2], u32); 2],
    /// Of its widest way: its id, its class and its group, the half-width of its carriageway and that with its
    /// widening; the inside half-width and height of the tube.
    pub widest: u32,
    pub class: String,
    pub group: usize,
    pub rail: bool,
    pub road: f64,
    pub way_half: f64,
    pub half: f64,
    pub height: f64,
    pub margin: f64,
}

/// A part of a way in a chain: the way's id, the part, and whether the chain runs against it.
type Link = (u32, usize, bool);

/// Per node, the number of ends of the ways `of` there: an end counts one, a vertex inside a way two.
fn degrees(input: &Input, of: &dyn Fn(usize) -> bool) -> HashMap<[u64; 2], u32> {
    let mut out: HashMap<[u64; 2], u32> = HashMap::new();
    for (_, _, parts) in input.ways.iter().enumerate().filter(|(w, _)| of(*w)).map(|(_, w)| w) {
        for part in parts.iter() {
            let n = part.points.len();
            for (i, p) in part.points.iter().enumerate() {
                *out.entry(key(*p)).or_default() += if i == 0 || i + 1 == n { 1 } else { 2 };
            }
        }
    }
    out
}

/// The chains of the tunnel ways with an id: from each end of a part at a node that is not two ends of tunnel ways (an
/// opening, or a node in the hill where more go on), on through each node where the part meets one other tunnel way
/// end to end. Each part lies in one chain; a ring of parts has no end and no chain.
fn chains(input: &Input, degree: &HashMap<[u64; 2], u32>) -> Vec<Vec<Link>> {
    let ends_of = |part: &Part| [key(part.points[0]), key(part.points[part.points.len() - 1])];
    let mut ids: Vec<u32> = input.tunnels.keys().copied().collect();
    ids.sort_unstable();
    let parts = || {
        (ids.iter()).flat_map(|&id| {
            (input.tunnels[&id].1.iter().enumerate())
                .filter(|(_, p)| p.points.len() >= 2)
                .map(move |(p, part)| (id, p, part))
        })
    };
    let mut at: HashMap<[u64; 2], Vec<(u32, usize, usize)>> = HashMap::new();
    for (id, p, part) in parts() {
        for (e, k) in ends_of(part).into_iter().enumerate() {
            at.entry(k).or_default().push((id, p, e));
        }
    }
    let mut seen: HashSet<(u32, usize)> = HashSet::new();
    let mut out = Vec::new();
    for (id, p, part) in parts() {
        for (e, k) in ends_of(part).into_iter().enumerate() {
            if degree.get(&k) == Some(&2) || seen.contains(&(id, p)) {
                continue;
            }
            let mut chain = Vec::new();
            let mut cur: Link = (id, p, e == 1);
            loop {
                seen.insert((cur.0, cur.1));
                chain.push(cur);
                let far = ends_of(&input.tunnels[&cur.0].1[cur.1])[usize::from(!cur.2)];
                if degree.get(&far) != Some(&2) {
                    break;
                }
                let next = at.get(&far).into_iter().flatten().find(|n| !seen.contains(&(n.0, n.1)));
                let Some(&(nid, np, ne)) = next else { break };
                cur = (nid, np, ne == 1);
            }
            out.push(chain);
        }
    }
    out
}

/// The tube of a chain, or None where a part of it has no curve.
fn tube(chain: &[Link], input: &Input) -> Option<Tube> {
    let mut curve = Curve::default();
    let mut heights = true;
    let (mut widest, mut way_half, mut best) = (chain[0].0, 0.0, f64::NEG_INFINITY);
    let mut ends = [([0, 0], chain[0].0), ([0, 0], chain[chain.len() - 1].0)];
    for (k, &(id, p, back)) in chain.iter().enumerate() {
        let (tags, parts) = &input.tunnels[&id];
        let part = &parts[p];
        let n = part.points.len();
        let mut stretch = way_curve(part)?;
        if back {
            stretch.reverse();
        }
        heights &= stretch.iter().all(|q| q.z.is_finite());
        if k == 0 {
            ends[0].0 = frame_point(parts, p, if back { n - 1 } else { 0 });
        }
        if k + 1 == chain.len() {
            ends[1].0 = frame_point(parts, p, if back { 0 } else { n - 1 });
        }
        let w = input.widths.way("tunnel", tags, false);
        let widen = part.plan.as_ref()?.iter().map(|v| v.widen).fold(0.0, f64::max);
        if w.half_m + widen > best {
            (best, widest, way_half) = (w.half_m + widen, id, w.half_m + widen);
        }
        // The chain's metres go on over the shared node.
        let mut s = curve.len;
        for i in 0..stretch.len() {
            if i > 0 {
                s += distance(stretch[i - 1].p, stretch[i].p);
            }
            stretch[i].s = s;
        }
        curve.len = s;
        curve.stretches.push((id, stretch));
    }
    let (tags, _) = &input.tunnels[&widest];
    let w = input.widths.way("tunnel", tags, false);
    let class = input.class_names.get(w.rank).cloned().unwrap_or_default();
    let group = input.rules.group(&class)?;
    let height = crate::structures::bore(tags);
    let b = &input.rules.bake;
    let margin = if height <= b.narrow_bore_m { b.margin_narrow_m } else { b.margin_m };
    let mut ids: Vec<u32> = chain.iter().map(|l| l.0).collect();
    ids.dedup();
    Some(Tube {
        ids,
        heights: heights && curve.len > 0.0,
        curve,
        ends,
        widest,
        class,
        group,
        rail: is_track(tags),
        road: w.half_m,
        way_half,
        half: way_half + margin,
        height,
        margin,
    })
}

/// An opening of a tube: its tube and end, the end vertex of its way and the way, the metres along the way from the
/// vertex to it, its place on the chain, its point, its unit direction into the tube, its floor, and the arms of the
/// other ways at the vertex (an end of a way is one arm, a vertex inside it two).
#[derive(Clone, Debug)]
pub(crate) struct Opening {
    pub tube: usize,
    pub end: usize,
    pub way: [i64; 2],
    pub way_id: u32,
    pub shift: f64,
    pub s: f64,
    pub p: [f64; 2],
    pub dir: [f64; 2],
    pub floor: f64,
    pub others: usize,
}

impl Opening {
    /// Moved to `shift` metres in from its vertex.
    fn at(&self, tube: &Tube, shift: f64) -> Opening {
        let s = if self.end == 0 { shift } else { tube.curve.len - shift };
        let (q, _) = tube.curve.at(s);
        let d = tube.curve.dir(s);
        let dir = if self.end == 0 { d } else { [-d[0], -d[1]] };
        Opening { shift, s, p: q.p, dir, floor: q.z, ..self.clone() }
    }

    pub fn heading(&self) -> f64 {
        self.dir[1].atan2(self.dir[0])
    }
}

/// The metres of the stretch of the chain from its end `end` that lies on the way there.
fn first_way_m(tube: &Tube, end: usize) -> f64 {
    let (_, ps) = if end == 0 { &tube.curve.stretches[0] } else { tube.curve.stretches.last().unwrap() };
    ps.last().unwrap().s - ps[0].s
}

/// The shift of an opening where it meets two ways or more: the first whole metre in where both ends of its frame line
/// (`half` + `shell_m` across) lie off the outer half-width of each other way there, at most `junction_shift_max_m`,
/// half the chain and the stretch on its own way.
fn junction_shift(o: &Opening, tube: &Tube, others: &[(f64, Vec<[f64; 2]>)], b: &Bake) -> f64 {
    let most = b.junction_shift_max_m.min(tube.curve.len / 2.0).min(first_way_m(tube, o.end)).max(0.0);
    let clear = |q: [f64; 2]| {
        others.iter().all(|(outer, line)| {
            let near = line.windows(2).map(|w| segment_m(q, w[0], w[1])).fold(f64::INFINITY, f64::min);
            near > *outer
        })
    };
    let mut m = 0.0;
    while m < most {
        let at = o.at(tube, m);
        let left = left_of(at.dir);
        let reach = tube.half + b.shell_m;
        if clear(offset(at.p, left, reach)) && clear(offset(at.p, left, -reach)) {
            return m;
        }
        m += 1.0;
    }
    most
}

/// Metres from q to the segment a-b.
pub(crate) fn segment_m(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    distance(q, [a[0] + t * d[0], a[1] + t * d[1]])
}

/// A number of metres as a tag holds it: at most two decimals.
pub(crate) fn num(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 {
        "0".into()
    } else {
        format!("{r}")
    }
}

/// The placement of the tunnel parts, measured on the ground plan.
pub(crate) fn place(input: &Input, memory: usize) -> Placed {
    place_with(input, &|asks, ends| measure::sites(asks, input, ends, memory))
}

/// The placement with the sites that `measure` gives of the asks, with the open ends of all openings.
pub(crate) fn place_with(input: &Input, measure: &ladder::Measure) -> Placed {
    let b = &input.rules.bake;
    let degree = degrees(input, &|w| input.tunnel_ways.contains(&w));
    let chains = chains(input, &degree);
    let tubes: Vec<Tube> = chains.iter().filter_map(|c| tube(c, input)).collect();
    let lost = chains.len() - tubes.len();
    // The ends of the shallow tunnels (a way of the kind `tunnel` in the map that the tiles hold as a way on the
    // ground): they open on no part.
    let shallow_way = |w: usize| input.ways[w].0 == "tunnel" && !input.tunnel_ways.contains(&w);
    let all = degrees(input, &|w| input.tunnel_ways.contains(&w) || shallow_way(w));
    let shallow = (input.ways.iter().enumerate())
        .filter(|(w, _)| shallow_way(*w))
        .flat_map(|(_, (_, _, parts))| parts.iter().flat_map(|p| [p.points[0], p.points[p.points.len() - 1]]))
        .filter(|p| all.get(&key(*p)) == Some(&1))
        .count();
    // Per end vertex of a tube, the ways there with their arms: an end of a part is one arm, a vertex inside it two.
    let mut at_end: HashMap<[u64; 2], Vec<(usize, u32)>> = HashMap::new();
    for t in &tubes {
        for p in [t.curve.at(0.0).0.p, t.curve.at(t.curve.len).0.p] {
            at_end.entry(key(p)).or_default();
        }
    }
    for (w, (_, _, parts)) in input.ways.iter().enumerate() {
        for part in parts.iter() {
            let n = part.points.len();
            for (i, p) in part.points.iter().enumerate() {
                if let Some(list) = at_end.get_mut(&key(*p)) {
                    let arms = if i == 0 || i + 1 == n { 1 } else { 2 };
                    match list.last_mut() {
                        Some(last) if last.0 == w => last.1 += arms,
                        _ => list.push((w, arms)),
                    }
                }
            }
        }
    }
    let (mut forks, mut tees) = (0, 0);
    // Parts in a ring of tunnel ways (no end), and tunnel ways with no id: no chain holds them.
    let chained: usize = chains.iter().map(Vec::len).sum();
    let rings = input.tunnels.values().flat_map(|(_, parts)| parts).filter(|p| p.points.len() >= 2).count() - chained;
    let without_id = input.tunnel_ways.len() - input.tunnels.len();
    let mut openings: Vec<Opening> = Vec::new();
    for (ti, t) in tubes.iter().enumerate() {
        for e in 0..2 {
            let (way, way_id) = t.ends[e];
            let s = if e == 0 { 0.0 } else { t.curve.len };
            let vertex = t.curve.at(s).0.p;
            // Where another tunnel way has a vertex at the end, the tube goes on in the hill: no opening. The ways at
            // the vertex but the tube's own: a way's id is its place in the ways of the bake.
            if degree.get(&key(vertex)) != Some(&1) {
                forks += 1;
                continue;
            }
            let others: Vec<(usize, u32)> =
                at_end.get(&key(vertex)).into_iter().flatten().copied().filter(|w| w.0 != way_id as usize).collect();
            let arms: u32 = others.iter().map(|w| w.1).sum();
            tees += usize::from(others.iter().any(|w| w.1 >= 2));
            let o = Opening {
                tube: ti,
                end: e,
                way,
                way_id,
                shift: 0.0,
                s,
                p: vertex,
                dir: [1.0, 0.0],
                floor: 0.0,
                others: 0,
            };
            let mut o = o.at(t, 0.0);
            o.others = arms as usize;
            if t.heights && arms >= 2 {
                let lines: Vec<(f64, Vec<[f64; 2]>)> = (others.iter())
                    .flat_map(|&(w, _)| {
                        let (kind, tags, parts) = input.ways[w];
                        let outer = input.widths.way(kind, tags, false).outer_m();
                        parts.iter().map(move |p| (outer, roads::polyline(p).points))
                    })
                    .collect();
                let shift = junction_shift(&o, t, &lines, b);
                o = Opening { others: o.others, ..o.at(t, shift) };
            }
            openings.push(o);
        }
    }
    let mut decided = ladder::decide(&tubes, &mut openings, input, measure);
    if let Value::Object(m) = &mut decided.metrics {
        m.insert("shallow_ends".into(), shallow.into());
        m.insert("fork_ends".into(), forks.into());
        m.insert("t_ends".into(), tees.into());
        m.insert("ring_parts".into(), rings.into());
        m.insert("tunnel_ways_without_id".into(), without_id.into());
        m.insert("chains_without_tube".into(), lost.into());
    }
    let mut placed = records(&tubes, &openings, &decided, input);
    // The cut of each built end, on the ground with the stretches of the built chains open, as the device builds it.
    let kinds: Vec<Option<Kind>> = decided.ends.iter().map(|d| d.kind).collect();
    let rules = Rules { ladder: input.rules.clone(), ..Rules::embedded().clone() };
    let cut = cut::add(&mut placed.features, &open_ends(&built_openings(&openings, &kinds)), &rules, measure);
    if let (Value::Object(m), Value::Object(c)) = (&mut placed.metrics, cut) {
        m.extend(c);
    }
    placed
}

/// The features of the placement and the tag `parts` of the tunnel ways.
fn records(tubes: &[Tube], openings: &[Opening], decided: &ladder::Decided, input: &Input) -> Placed {
    let (mut features, mut tags) = (Vec::new(), HashMap::new());
    let part_name = |k: Option<Kind>| k.map_or("none", Kind::name);
    let mut ends_of: Vec<[Option<Kind>; 2]> = vec![[None, None]; tubes.len()];
    for (o, d) in openings.iter().zip(&decided.ends) {
        ends_of[o.tube][o.end] = d.kind;
    }
    for (ti, t) in tubes.iter().enumerate() {
        let kinds = ends_of[ti];
        if kinds.iter().all(Option::is_none) {
            continue;
        }
        let group = &input.rules.groups[t.group];
        let half = decided.half[ti];
        let section = if kinds.contains(&Some(Kind::Box)) { "box" } else { "round" };
        let (wtags, _) = &input.tunnels[&t.widest];
        let mut common = Tags::new();
        common.insert("section".into(), section.into());
        common.insert("half".into(), num(half));
        common.insert("look".into(), group.look.clone());
        // The tube from opening to opening, as its floor.
        let fl = decided.floors[ti].as_ref();
        let (s0, s1) = fl.map_or((0.0, 0.0), |f| (f.s[0], f.s[f.s.len() - 1]));
        for (way, ps) in &t.curve.stretches {
            let rising = ps.last().unwrap().along >= ps[0].along;
            let order = if rising { [kinds[0], kinds[1]] } else { [kinds[1], kinds[0]] };
            let ends = format!("{} {}", part_name(order[0]), part_name(order[1]));
            let mut way_tags = Tags::new();
            way_tags.insert("parts".into(), ends.clone());
            tags.insert(*way, way_tags);
            // The points of the stretch within the openings: its ends and each vertex of the floor between.
            let Some(fl) = fl else { continue };
            let (a, z) = (ps[0].s.max(s0), ps.last().unwrap().s.min(s1));
            if z - a <= 1e-6 {
                continue;
            }
            let inside = |s: f64| s > a + 1e-6 && s < z - 1e-6;
            let mut pts: Vec<Point> = vec![t.curve.at(a).0];
            pts.extend(fl.s.iter().filter(|&&s| inside(s)).map(|&s| t.curve.at(s).0));
            pts.push(t.curve.at(z).0);
            for w in pts.windows(2) {
                let (p, q) = if rising { (w[0], w[1]) } else { (w[1], w[0]) };
                let ((pz, ph), (qz, qh)) = (fl.at(p.s), fl.at(q.s));
                let mut tube_tags = common.clone();
                tube_tags.insert("height".into(), num(ph.min(qh)));
                tube_tags.insert("class".into(), t.class.clone());
                if let Some(surface) = wtags.get("surface") {
                    tube_tags.insert("surface".into(), surface.clone());
                }
                tube_tags.insert("road".into(), num(t.road));
                tube_tags.insert("along".into(), num(p.along));
                tube_tags.insert("ends".into(), ends.clone());
                let part = Part { outer: false, points: vec![p.p, q.p], z: Some(vec![pz, qz]), plan: None };
                features.push(("tunnel-tube", tube_tags, part));
            }
        }
    }
    for (o, d) in openings.iter().zip(&decided.ends) {
        let (Some(kind), Some(n)) = (d.kind, &d.numbers) else { continue };
        let mut t = Tags::new();
        let section = if kind == Kind::Box { "box" } else { "round" };
        t.insert("section".into(), section.into());
        t.insert("half".into(), num(decided.half[o.tube]));
        t.insert("height".into(), num(n.height));
        let look = if kind == Kind::Rock { "rock" } else { input.rules.groups[tubes[o.tube].group].look.as_str() };
        t.insert("look".into(), look.into());
        t.insert("part".into(), kind.name().into());
        t.insert("heading".into(), format!("{}", (o.heading() * 1000.0).round() / 1000.0));
        t.insert("collar".into(), num(n.collar));
        if kind != Kind::Free {
            t.insert("face".into(), num(n.face));
            t.insert("left".into(), n.left.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" "));
            t.insert("right".into(), n.right.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" "));
        }
        t.insert("foot".into(), num(n.foot));
        t.insert("way".into(), format!("{} {}", o.way[0], o.way[1]));
        t.insert("shift".into(), num(o.shift));
        if let Some((mate, m)) = n.mate.as_ref().map(|(i, m)| (&openings[*i], *m)) {
            t.insert("mate".into(), num(m));
            t.insert("mate_way".into(), format!("{} {}", mate.way[0], mate.way[1]));
            t.insert("mate_shift".into(), num(mate.shift));
        }
        let part = Part { outer: false, points: vec![o.p], z: Some(vec![o.floor]), plan: None };
        features.push(("tunnel-end", t, part));
    }
    Placed {
        features,
        tags,
        metrics: decided.metrics.clone(),
        #[cfg(test)]
        ends: openings.iter().map(|o| o.way_id).zip(decided.ends.iter().cloned()).collect(),
    }
}

/// The openings of the chains whose ends are all of built kinds (`obc_trailview_parts::BUILT`) by the kind of each
/// opening: those that the device opens.
pub(crate) fn built_openings(openings: &[Opening], kind: &[Option<Kind>]) -> Vec<Opening> {
    let of =
        |tube: usize, end: usize| openings.iter().position(|o| o.tube == tube && o.end == end).and_then(|j| kind[j]);
    (openings.iter()).filter(|o| obc_trailview_parts::built(&[of(o.tube, 0), of(o.tube, 1)])).cloned().collect()
}

/// The open ends of the openings: each moved opening on its way, for the ground plan's `graph::build_open`.
pub(crate) fn open_ends(openings: &[Opening]) -> Vec<OpenEnd> {
    (openings.iter().filter(|o| o.shift > 0.0))
        .map(|o| OpenEnd { way: obc_ground_plan::frame::GlobalPoint { x: o.way[0], y: o.way[1] }, shift_m: o.shift })
        .collect()
}

/// Counts by name, as metrics.
pub(crate) fn counts(names: impl Iterator<Item = String>) -> Value {
    let mut by: BTreeMap<String, u64> = BTreeMap::new();
    for n in names {
        *by.entry(n).or_default() += 1;
    }
    Value::Object(by.into_iter().map(|(k, v)| (k, json!(v))).collect::<Map<String, Value>>())
}
