//! The tunnel parts that the bake places: per chain of tunnel ways its openings, the kind of part at each and its
//! numbers from the ground of the ground plan there, written as the features `tunnel-tube` and `tunnel-end` and the
//! tag `parts` of the tunnel ways (`specs/trailview-features.md`, Tunnel parts).

mod floor;
mod ladder;
mod measure;
mod rooms;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

use obc_trailview_parts::{Bake, Ladder};

use obc_ground_plan::graph::OpenEnd;
use obc_ground_plan::sections::{is_track, WidthRules};
use serde_json::{json, Map, Value};

use crate::geometry::key;
use crate::roads::Classes;
use crate::tiles::{frame_point, Tiles};
use crate::{metres, roads, Part, Tags, Terrain};
use floor::Floor;

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
    /// Per opening the ids of its ways and its decision.
    #[cfg(test)]
    pub ends: Vec<(Vec<u32>, ladder::End)>,
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
    /// Of its widest way: its id, its class and its group, and the half-width of its carriageway; the inside half-width
    /// (the way's half-width with its widening and margin) and height of the tube.
    pub widest: u32,
    pub class: String,
    pub group: usize,
    pub rail: bool,
    pub road: f64,
    pub half: f64,
    pub height: f64,
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
        half: way_half + margin,
        height,
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

/// A number of metres to the millimetre, as a seam's holes hold it.
fn mm(v: f64) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 {
        "0".into()
    } else {
        format!("{r}")
    }
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
    let memo = measure::Memo::default();
    place_with(input, &|asks, ends| memo.sites(asks, input, ends, memory))
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
    // What the rooms cannot build draws as tunnels of one way: the arms of a room that cannot draw on its own account,
    // or whose seam or inside the builder does not find whole, run beside no other (`lone`); the other rooms are made
    // again. Of a head that the builder does not find whole, each opening there whose hole or chord end is not whole
    // has a head of its own (`apart`), so its room keeps its lanes. The rooms and the ladder are made again until each
    // joint is whole.
    let mut lone = vec![false; tubes.len()];
    let mut apart = vec![false; openings.len()];
    let (rooms, mut decided, placed, not_whole) = loop {
        let (rooms, mut openings) = rooms::rooms(tubes.clone(), openings.clone(), &lone, input);
        // A room draws only where its line lies in the bake's region, whose tiles hold its records; a tunnel of one way
        // anywhere.
        let leaves =
            |r: usize| rooms.tubes[r].curve.stretches.iter().flat_map(|(_, ps)| ps).any(|q| !input.tiles.holds(q.p));
        let out: Vec<bool> = (0..rooms.tubes.len()).map(|r| !rooms.alone[r] && leaves(r)).collect();
        let own_head = |o: &Opening| rooms.lanes[o.tube].iter().any(|l| l.ends[o.end].is_some_and(|i| apart[i]));
        let apart_now: Vec<bool> = openings.iter().map(own_head).collect();
        let decided = ladder::decide(&rooms.tubes, &mut openings, input, measure, (&rooms, &out, &apart_now));
        let (placed, found) = records(&rooms, &openings, &decided, input);
        let before = (lone.clone(), apart.clone());
        let kind = |t: usize, e: usize| decided.ends[openings.iter().position(|o| o.tube == t && o.end == e)?].kind;
        let own = rooms.own(&kind, &decided.out);
        for r in (0..rooms.tubes.len()).filter(|&r| !rooms.alone[r] && (!own[r] || found.rooms.contains(&r))) {
            rooms.lanes[r].iter().for_each(|l| lone[l.arm] = true);
        }
        for i in found.ends.iter().flat_map(|&(r, e)| rooms.lanes[r].iter().filter_map(move |l| l.ends[e])) {
            apart[i] = true;
        }
        if (&lone, &apart) == (&before.0, &before.1) {
            break (rooms, decided, placed, found.not_whole);
        }
    };
    if let Value::Object(m) = &mut decided.metrics {
        m.insert("shallow_ends".into(), shallow.into());
        m.insert("fork_ends".into(), forks.into());
        m.insert("t_ends".into(), tees.into());
        m.insert("ring_parts".into(), rings.into());
        m.insert("tunnel_ways_without_id".into(), without_id.into());
        m.insert("chains_without_tube".into(), lost.into());
        m.insert("rooms".into(), rooms.metrics.clone());
        m.insert("joints_not_whole".into(), not_whole.into());
        m.insert("arms_alone".into(), lone.iter().filter(|&&l| l).count().into());
        m.insert("openings_apart".into(), apart.iter().filter(|&&a| a).count().into());
        let touching = rooms::touching(&rooms, (&decided.floors, &decided.draws), &decided.half, input);
        let places = touching.iter().take(10).map(|&p| {
            let [lat, lon] = crate::degrees(p);
            format!("{lat:.5},{lon:.5}")
        });
        m.insert("rooms_that_touch".into(), touching.len().into());
        m.insert("rooms_that_touch_places".into(), places.collect::<Vec<_>>().join("; ").into());
        // The largest step of each room of lanes: the most that a lane's floor lies over the room's, and where.
        let mut steps: Vec<(f64, [f64; 2])> = (rooms.tubes.iter().zip(&decided.floors))
            .filter_map(|(t, f)| {
                let f = f.as_ref().filter(|f| f.lanes.len() > 1)?;
                let (i, up) = (0..f.s.len())
                    .map(|i| (i, f.lanes.iter().map(|l| l.0[i] - f.f[i]).fold(0.0, f64::max)))
                    .max_by(|x, y| x.1.total_cmp(&y.1))?;
                Some((up, t.curve.at(f.s[i]).0.p))
            })
            .collect();
        steps.sort_by(|x, y| y.0.total_cmp(&x.0));
        let band = |lo: f64, hi: f64| steps.iter().filter(|s| s.0 > lo && s.0 <= hi).count();
        m.insert(
            "largest_steps".into(),
            json!({"to 0.5 m": band(-1.0, 0.5), "to 1.5 m": band(0.5, 1.5), "to 3 m": band(1.5, 3.0), "to 5 m": band(3.0, 5.0), "over 5 m": band(5.0, f64::INFINITY)}),
        );
        let places = steps.iter().take(10).map(|&(up, p)| {
            let [lat, lon] = crate::degrees(p);
            format!("{lat:.5},{lon:.5} {up:.2} m")
        });
        m.insert("largest_step_places".into(), places.collect::<Vec<_>>().join("; ").into());
    }
    Placed { metrics: decided.metrics, ..placed }
}

/// A length up to the next centimetre: a room's numbers that must hold its lanes and the rooms behind a seam.
pub(crate) fn ceil_cm(v: f64) -> f64 {
    ((v * 100.0) - 1e-4).ceil() / 100.0
}

/// A hole of a joint, or a vertex of a chord, by its room and that room's end there; none at a chord's inner vertex.
type HoleOf = (usize, Option<usize>);

/// The joints that the records would not hold whole: the rooms of lanes whose seam or inside is not whole; the room
/// and the end of each opening whose head it shares with others; and how many records and chords the builder did not
/// find whole.
#[derive(Default)]
struct Faults {
    rooms: Vec<usize>,
    ends: Vec<(usize, usize)>,
    not_whole: usize,
}

impl Faults {
    /// A hole of a joint or an end of a chord that the builder does not find whole, by its room and end (none inside
    /// the room); `shared` where its head has other holes. At an opening only a shared head is not whole, and of a
    /// tunnel of one way nothing else: its chords are shorter than the tiles' buffer (`CHORD_M` of the ways' curves),
    /// so a tile that holds one holds the chords at its vertices and a head of one hole at its end.
    fn add(&mut self, rooms: &rooms::Rooms, (r, e): HoleOf, shared: bool) {
        match e.filter(|&e| !rooms.seam_at(r, e)) {
            Some(e) if shared => self.ends.push((r, e)),
            None if !rooms.alone[r] => self.rooms.push(r),
            _ => {}
        }
    }
}

/// The holes of a pair lie at most this far apart across: well under the tiles' buffer (`BUFFER_M`, 40 m), so the tiles
/// that hold a hole's chord hold its head where the chords are short; the read-back proves each joint in each tile.
pub(crate) const JOINT_MAX_M: f64 = 29.0;

/// The features of the rooms that draw and the tag `parts` of their tunnel ways, and each hole and chord of rooms that
/// draw that the builder does not find whole as the tiles hold them.
fn records(rooms: &rooms::Rooms, openings: &[Opening], decided: &ladder::Decided, input: &Input) -> (Placed, Faults) {
    let tubes = &rooms.tubes;
    let (mut features, mut tags) = (Vec::new(), HashMap::new());
    let mut faults = Faults::default();
    let draws = &decided.draws;
    // A tunnel of one way that does not draw keeps its records where an end has a part: its chords name `none` at the
    // other end, so the builder builds neither them nor that part's head.
    let writes: Vec<bool> =
        (0..tubes.len()).map(|t| draws[t] || (rooms.alone[t] && !decided.out[t] && !rooms.broken[t])).collect();
    let open_kind = |t: usize, e: usize| {
        let j = openings.iter().position(|o| o.tube == t && o.end == e)?;
        decided.ends[j].kind
    };
    let kind_at = |t: usize, e: usize| if rooms.seam_at(t, e) { Some(Kind::Seam) } else { open_kind(t, e) };
    let ends_of: Vec<[Option<Kind>; 2]> =
        (0..tubes.len()).map(|t| if writes[t] { [kind_at(t, 0), kind_at(t, 1)] } else { [None; 2] }).collect();
    let mut boxed = vec![false; tubes.len()];
    for (o, d) in openings.iter().zip(&decided.ends) {
        boxed[o.tube] |= d.kind == Some(Kind::Box);
    }
    let boxed = rooms.one_section(boxed);
    // The tag `parts` of each tunnel way of a room that draws: the parts at its arm's two ends.
    let mut arm_kinds: Vec<Option<[Option<Kind>; 2]>> = vec![None; rooms.arms.len()];
    for (t, lanes) in rooms.lanes.iter().enumerate().filter(|&(t, _)| writes[t]) {
        for l in lanes {
            let kinds = arm_kinds[l.arm].get_or_insert([None; 2]);
            for (i, kind) in l.ends.iter().zip(ends_of[t]) {
                if let Some(i) = i {
                    kinds[rooms.arm_openings[*i].end] = kind;
                }
            }
        }
    }
    let part_name = |k: Option<Kind>| k.map_or("none", Kind::name);
    let with_parts =
        rooms.arms.iter().zip(&arm_kinds).filter_map(|(a, k)| Some((a, (*k)?)).filter(|x| x.1 != [None; 2]));
    for (arm, kinds) in with_parts {
        for (way, ps) in &arm.curve.stretches {
            let rising = ps.last().unwrap().along >= ps[0].along;
            let order = if rising { [kinds[0], kinds[1]] } else { [kinds[1], kinds[0]] };
            tags.insert(
                *way,
                Tags::from([("parts".to_string(), format!("{} {}", part_name(order[0]), part_name(order[1])))]),
            );
        }
    }
    // The rooms before a seam hold the rooms behind it: per room end, their outlines on the line (metres across, half,
    // floor over the room's, height).
    let floors = &decided.floors;
    let at_end = |r: usize, e: usize| floors[r].as_ref().map(|f| if e == 0 { 0 } else { f.s.len() - 1 });
    let mut behind: Vec<Behind> = vec![[Vec::new(), Vec::new()]; tubes.len()];
    let arch_segments = obc_trailview_parts::Rules::embedded().design.arch_segments;
    for seam in &rooms.seams {
        let (pr, pe) = seam.parent;
        let (Some(fp), Some(ip)) = (floors[pr].as_ref(), at_end(pr, pe)) else { continue };
        for (&(r, e), &(across, half)) in seam.children.iter().zip(&seam.holes) {
            let (Some(fc), Some(ic)) = (floors[r].as_ref(), at_end(r, e)) else { continue };
            // The room behind the line in the parent's way: across to the parent's left there; its floor and its
            // height as its records give them, the most that its chord there is high, and its arch as the builder
            // draws it.
            let sign = if pe == 1 { 1.0 } else { -1.0 };
            let up = (100.0 * (fc.f[ic] - fp.f[ip])).round() / 100.0;
            let segments = arch_segments * rooms.lanes[r].len();
            behind[pr][pe].push((sign * across, half, up, ceil_cm(fc.h[ic]), segments));
        }
    }
    // Per feature, per vertex of a chord its room and that room's end there, and per hole of a joint its room and end.
    let mut of_feature: Vec<Vec<HoleOf>> = Vec::new();
    for (ti, t) in tubes.iter().enumerate() {
        let kinds = ends_of[ti];
        if kinds.iter().all(Option::is_none) {
            continue;
        }
        let group = &input.rules.groups[t.group];
        let multi = rooms.lanes[ti].len() > 1;
        let mut common = Tags::new();
        common.insert("section".into(), if boxed[ti] { "box" } else { "round" }.into());
        common.insert("half".into(), num(if multi { ceil_cm(decided.half[ti]) } else { decided.half[ti] }));
        common.insert("look".into(), group.look.clone());
        let fl = decided.floors[ti].as_ref();
        let first = features.len();
        if multi {
            room(rooms, ti, kinds, (fl, &common, &behind[ti]), input, &mut features);
        } else {
            lone(t, kinds, (fl, &common), &input.tunnels[&t.widest].0, &mut features);
        }
        let ends = fl.map(|f| [f.s[0], f.s[f.s.len() - 1]].map(|s| t.curve.at(s).0.p));
        let end = |p: [f64; 2]| ends.and_then(|e| e.iter().position(|&q| q == p));
        of_feature.extend(features[first..].iter().map(|f| f.2.points.iter().map(|&p| (ti, end(p))).collect()));
    }
    for (o, d) in openings.iter().zip(&decided.ends) {
        let (Some(kind), Some(n)) = (d.kind, &d.numbers) else { continue };
        let tube = |h: &ladder::Hole| openings[h.opening].tube;
        // A head holds only rooms that draw, or the one tunnel of one way that keeps its records.
        if n.holes.iter().any(|h| !writes[tube(h)]) {
            continue;
        }
        let holes: Vec<HoleOf> = n.holes.iter().map(|h| (tube(h), Some(openings[h.opening].end))).collect();
        let row = |f: &dyn Fn(&ladder::Hole) -> String| n.holes.iter().map(f).collect::<Vec<_>>().join(" ");
        let kind_of = |h: &ladder::Hole| decided.ends[h.opening].kind.unwrap_or(kind);
        // A room of lanes rounds its numbers up to the centimetre, as its chords do.
        let up_cm = |t: usize, v: f64| if rooms.lanes[t].len() > 1 { ceil_cm(v) } else { v };
        let mut t = Tags::new();
        t.insert("section".into(), row(&|h| if boxed[tube(h)] { "box" } else { "round" }.into()));
        t.insert("half".into(), row(&|h| num(up_cm(tube(h), decided.half[tube(h)]))));
        t.insert("height".into(), row(&|h| num(up_cm(tube(h), h.height))));
        // In rock where each hole opens in the rock.
        let rock = n.holes.iter().all(|h| kind_of(h) == Kind::Rock);
        let look = if rock { "rock" } else { input.rules.groups[tubes[o.tube].group].look.as_str() };
        t.insert("look".into(), look.into());
        t.insert("part".into(), kind.name().into());
        t.insert("heading".into(), format!("{}", (n.heading * 1000.0).round() / 1000.0));
        t.insert("collar".into(), row(&|h| num(h.collar)));
        if n.walls {
            t.insert("face".into(), num(n.face));
            t.insert("left".into(), n.left.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" "));
            t.insert("right".into(), n.right.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" "));
        }
        t.insert("foot".into(), num(n.foot));
        // Per hole from the right, the ways that enter it from the right.
        let ways: Vec<rooms::Way> = n.holes.iter().flat_map(|h| rooms.ways(&openings[h.opening])).collect();
        t.insert("way".into(), ways.iter().map(|w| format!("{} {}", w.way[0], w.way[1])).collect::<Vec<_>>().join(" "));
        t.insert("shift".into(), ways.iter().map(|w| num(w.shift)).collect::<Vec<_>>().join(" "));
        if n.holes.len() > 1 {
            t.insert("across".into(), row(&|h| num(h.across)));
            t.insert("up".into(), row(&|h| num(openings[h.opening].floor - o.floor)));
            t.insert("kind".into(), row(&|h| kind_of(h).name().into()));
        }
        let part = Part { outer: false, points: vec![o.p], z: Some(vec![o.floor]), plan: None };
        features.push(("tunnel-end", t, part));
        of_feature.push(holes);
    }
    // The seams of rooms that draw: at the axis of the room before the line, a hole per room, the room before first,
    // then those behind from the right.
    for seam in rooms.seams.iter().filter(|seam| draws[seam.parent.0]) {
        let (pr, pe) = seam.parent;
        let (Some(fp), Some(ip)) = (floors[pr].as_ref(), at_end(pr, pe)) else { continue };
        let at = tubes[pr].curve.at(if pe == 0 { 0.0 } else { tubes[pr].curve.len }).0.p;
        let mut holes: Vec<([f64; 4], HoleOf)> = Vec::new();
        for (&(r, e), &(across, half)) in seam.children.iter().zip(&seam.holes) {
            let (Some(fc), Some(ic)) = (floors[r].as_ref(), at_end(r, e)) else { continue };
            holes.push(([across, fc.f[ic] - fp.f[ip], half, fc.h[ic]], (r, Some(e))));
        }
        holes.sort_by(|x, y| x.0[0].total_cmp(&y.0[0]));
        let half_p = if rooms.lanes[pr].len() > 1 { ceil_cm(decided.half[pr]) } else { decided.half[pr] };
        holes.insert(0, ([0.0, 0.0, half_p, fp.h[ip]], (pr, Some(pe))));
        let (holes, members): (Vec<[f64; 4]>, Vec<HoleOf>) = holes.into_iter().unzip();
        // A hole's middle and half-width are whole millimetres, so its sides are those of the bake.
        let row = |k: usize| holes.iter().map(move |h| if k.is_multiple_of(2) { mm(h[k]) } else { num(h[k]) });
        let row = |k: usize| row(k).collect::<Vec<_>>().join(" ");
        let mut t = Tags::new();
        t.insert("part".into(), Kind::Seam.name().into());
        t.insert("heading".into(), format!("{}", (seam.dir[1].atan2(seam.dir[0]) * 1000.0).round() / 1000.0));
        for (k, name) in ["across", "up", "half", "height"].into_iter().enumerate() {
            t.insert(name.into(), row(k));
        }
        // Per lane that ends on the line, its door: across, half-width, its floor over the room's, its bore.
        let doors: Vec<String> = (seam.doors.iter())
            .filter_map(|&(across, half, arm)| {
                let k = rooms.lanes[pr].iter().position(|l| l.arm == arm)?;
                let up = fp.lanes.get(k)?.0[ip] - fp.f[ip];
                Some([mm(across), mm(half), num(up), num(rooms.arms[arm].height)].join(" "))
            })
            .collect();
        if !doors.is_empty() {
            t.insert("door".into(), doors.join(" "));
        }
        let part = Part { outer: false, points: vec![at], z: Some(vec![fp.f[ip]]), plan: None };
        features.push(("tunnel-end", t, part));
        of_feature.push(members);
    }
    // Per hole of a joint, how many holes its head has.
    let mut in_head: HashMap<(usize, usize), usize> = HashMap::new();
    for (holes, _) in of_feature.iter().zip(&features).filter(|(_, f)| f.0 == "tunnel-end") {
        in_head.extend(holes.iter().filter_map(|&(r, e)| Some(((r, e?), holes.len()))));
    }
    // Read back as the tiles hold them: each hole and chord end of rooms that draw that the builder does not find whole
    // is a fault, at its room's end where it is one.
    for (f, found) in not_whole(&features, input.tiles) {
        let holes = &of_feature[f];
        if holes.iter().all(|h| draws[h.0]) {
            faults.not_whole += 1;
            for (&(r, e), _) in holes.iter().zip(&found).filter(|(_, &ok)| !ok) {
                let shared = e.is_some_and(|e| in_head.get(&(r, e)).is_some_and(|&n| n > 1));
                faults.add(rooms, (r, e), shared);
            }
        }
    }
    faults.rooms.sort_unstable();
    faults.rooms.dedup();
    let placed = Placed {
        features,
        tags,
        metrics: Value::Null,
        #[cfg(test)]
        ends: openings
            .iter()
            .map(|o| rooms.ways(o).iter().map(|w| w.id).collect())
            .zip(decided.ends.iter().cloned())
            .collect(),
    };
    (placed, faults)
}

/// What the builder does not find whole in the part records as the zoom-13 tiles of the region hold them, each tile
/// with its buffer: per record that it does not find whole in the tile that owns its point, whether it finds each of
/// its holes there, and per chord that does not close in the tile that owns its `a`, whether each of its ends closes
/// there; by feature.
pub(crate) fn not_whole(features: &[(&'static str, Tags, Part)], region: &Tiles) -> Vec<(usize, Vec<bool>)> {
    use crate::tiles::{decode, Geometry, EXTENT, MAX_ZOOM};
    let rules = obc_trailview_parts::Rules::embedded();
    let mut tiles = region.empty();
    for (f, (kind, tags, part)) in features.iter().enumerate() {
        let geometry = if part.points.len() == 1 { Geometry::Point } else { Geometry::Line };
        tiles.add(MAX_ZOOM, kind, geometry, tags, std::slice::from_ref(part), Some(f as u32));
    }
    // The builder's world units (2^30 per side) per tile unit.
    let side = f64::from(EXTENT);
    let unit = (1u64 << 30) as f64 / ((1u64 << MAX_ZOOM) as f64 * side);
    let mut out = Vec::new();
    for (&(_, col, row), tile) in &tiles.tiles {
        let (x0, y0) = (f64::from(col) * side, f64::from(row) * side);
        let world = |xy: [i64; 2]| [(x0 + xy[0] as f64) * unit, (y0 + xy[1] as f64) * unit];
        let owns = |p: [f64; 2]| [p[0] / unit - x0, p[1] / unit - y0].iter().all(|v| (0.0..side).contains(v));
        let (mut ends, mut chords, mut of_end, mut of_chord) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (i, f) in decode(&tile.bytes()).expect("a tile of part records").into_iter().enumerate() {
            let id = tile.id(i as u32).expect("a part record of the bake") as usize;
            let (xy, z) = (&f.parts[0].xy, f.parts[0].z.as_ref().expect("a part record has heights"));
            if f.kind == "tunnel-end" {
                ends.push(obc_trailview_parts::End::from_tags(world(xy[0]), z[0], &f.tags, rules).expect("an end"));
                of_end.push(id);
            } else {
                let (a, b) = (world(xy[0]), world(xy[1]));
                let chord =
                    obc_trailview_parts::Chord::from_tags([a[0], a[1], z[0]], [b[0], b[1], z[1]], &f.tags, rules);
                chords.push(chord.expect("a chord"));
                of_chord.push(id);
            }
        }
        let (found, closed) = obc_trailview_parts::read_back(&ends, &chords, rules);
        for ((e, holes), &f) in ends.iter().zip(found).zip(&of_end) {
            if owns(e.at) && holes.contains(&false) {
                out.push((f, holes));
            }
        }
        for ((c, ok), &f) in chords.iter().zip(closed).zip(&of_chord) {
            if owns([c.a[0], c.a[1]]) && ok.contains(&false) {
                out.push((f, ok.to_vec()));
            }
        }
    }
    out.sort_unstable();
    out
}

/// The features of the placement.
type Out<'a> = &'a mut Vec<(&'static str, Tags, Part)>;

/// The chords of a room of one lane, from end to end as its floor.
fn lone(t: &Tube, kinds: [Option<Kind>; 2], (fl, common): (Option<&Floor>, &Tags), widest: &Tags, features: Out) {
    let part_name = |k: Option<Kind>| k.map_or("none", Kind::name);
    let (s0, s1) = fl.map_or((0.0, 0.0), |f| (f.s[0], f.s[f.s.len() - 1]));
    for (_, ps) in &t.curve.stretches {
        let rising = ps.last().unwrap().along >= ps[0].along;
        let order = if rising { [kinds[0], kinds[1]] } else { [kinds[1], kinds[0]] };
        let ends = format!("{} {}", part_name(order[0]), part_name(order[1]));
        // The points of the stretch within the openings: its ends and each vertex of the floor between.
        let Some(fl) = fl else { continue };
        let (a, z) = (ps[0].s.max(s0), ps.last().unwrap().s.min(s1));
        if z - a <= 1e-6 {
            continue;
        }
        let mut pts: Vec<Point> = vec![t.curve.at(a).0];
        pts.extend(fl.s.iter().filter(|&&s| s > a + 1e-6 && s < z - 1e-6).map(|&s| t.curve.at(s).0));
        pts.push(t.curve.at(z).0);
        for w in pts.windows(2) {
            let (p, q) = if rising { (w[0], w[1]) } else { (w[1], w[0]) };
            let ((pz, ph), (qz, qh)) = (fl.at(p.s), fl.at(q.s));
            let mut tube_tags = common.clone();
            tube_tags.insert("height".into(), num(ph.min(qh)));
            tube_tags.insert("class".into(), t.class.clone());
            if let Some(surface) = widest.get("surface") {
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

/// Per room end, the outlines on its line of the rooms behind a seam there.
type Behind = [Vec<rooms::Outline>; 2];

/// The chords of room r of lanes, along its centre line from end to end as its floor, with a row per lane from the
/// right wall to the left. At each vertex a chord is as high as the room there, and at a seam as the rooms behind it
/// need.
fn room(
    rooms: &rooms::Rooms,
    r: usize,
    kinds: [Option<Kind>; 2],
    (fl, common, behind): (Option<&Floor>, &Tags, &Behind),
    input: &Input,
    features: Out,
) {
    let part_name = |k: Option<Kind>| k.map_or("none", Kind::name);
    let (t, lanes) = (&rooms.tubes[r], &rooms.lanes[r]);
    let Some(fl) = fl else { return };
    let (spring, kerb) =
        (obc_trailview_parts::Rules::embedded().design.spring, obc_trailview_parts::Rules::embedded().design.kerb_m);
    let round = common["section"] == "round";
    let segments = obc_trailview_parts::Rules::embedded().design.arch_segments;
    let surface = |l: &rooms::Lane| input.tunnels[&rooms.arms[l.arm].widest].0.get("surface").cloned();
    let surfaces: Vec<Option<String>> = lanes.iter().map(surface).collect();
    // Per vertex of the floor: the room's point, its floor, and per lane its place, its floor over the room's, its
    // height and its metres along its way.
    for i in 0..fl.s.len() - 1 {
        let at = |k: usize| {
            let s = fl.s[k];
            let places = rooms.places(r, s);
            let ups: Vec<f64> = fl.lanes.iter().map(|lane| lane.0[k] - fl.f[k]).collect();
            (t.curve.at(s).0, places, ups)
        };
        let (mut a, mut b) = (at(i), at(i + 1));
        // The lesser height of the room at the two vertices; at a seam's line, as high as the rooms behind it need.
        let last = fl.s.len() - 1;
        let seam = |k: usize| [(0, 0), (1, last)].into_iter().find(|&(end, at)| k == at && !behind[end].is_empty());
        let need = |((_, places, ups), k): (&(Point, Vec<rooms::Place>, Vec<f64>), usize)| {
            let Some((end, _)) = seam(k) else { return fl.h[k] };
            let mut outlines: Vec<rooms::Outline> = (places.iter().zip(ups).zip(&fl.lanes))
                .map(|((p, &up), lane)| (p.off, p.half, up, lane.1[k], segments))
                .collect();
            let wall = (outlines[0].2).max(outlines[outlines.len() - 1].2) + kerb;
            outlines.extend_from_slice(&behind[end]);
            rooms::hold(round, t.half, spring, &outlines, segments * lanes.len(), wall).max(fl.h[k])
        };
        let mut heights = [need((&a, i)), need((&b, i + 1))];
        let mut floors = [fl.f[i], fl.f[i + 1]];
        // In the rising metres of the base lane's way.
        let mut order: Vec<usize> = (0..lanes.len()).collect();
        let mut sign = 1.0;
        if b.0.along < a.0.along {
            (a, b) = (b, a);
            floors.swap(0, 1);
            heights.swap(0, 1);
            order.reverse();
            sign = -1.0;
        }
        let along = |k: usize| {
            let arm = &rooms.arms[lanes[k].arm];
            let (from, to) = (arm.curve.at(a.1[k].s).0.along, arm.curve.at(b.1[k].s).0.along);
            if to >= from {
                from
            } else {
                -from
            }
        };
        let row = |f: &dyn Fn(usize) -> String| order.iter().map(|&k| f(k)).collect::<Vec<_>>().join(" ");
        let mut tube_tags = common.clone();
        tube_tags.insert("height".into(), format!("{} {}", num(ceil_cm(heights[0])), num(ceil_cm(heights[1]))));
        tube_tags.insert("class".into(), row(&|k| rooms.arms[lanes[k].arm].class.clone()));
        if surfaces.iter().any(Option::is_some) {
            tube_tags.insert("surface".into(), row(&|k| surfaces[k].clone().unwrap_or("-".into())));
        }
        tube_tags.insert("road".into(), row(&|k| num(rooms.arms[lanes[k].arm].road)));
        tube_tags.insert("along".into(), row(&|k| num(along(k))));
        let ends = if sign > 0.0 { [kinds[0], kinds[1]] } else { [kinds[1], kinds[0]] };
        tube_tags.insert("ends".into(), format!("{} {}", part_name(ends[0]), part_name(ends[1])));
        tube_tags.insert("off".into(), row(&|k| format!("{} {}", num(sign * a.1[k].off), num(sign * b.1[k].off))));
        let ups = row(&|k| format!("{} {}", num(a.2[k]), num(b.2[k])));
        if ups.split(' ').any(|u| u != "0") {
            tube_tags.insert("up".into(), ups);
        }
        let edge = |(_, places, _): &(Point, Vec<rooms::Place>, Vec<f64>), j: usize| {
            let (x, y) = (&places[order[j]], &places[order[j + 1]]);
            let (x, y) = if sign > 0.0 { (x, y) } else { (y, x) };
            sign * ((x.off + x.half + y.off - y.half) / 2.0)
        };
        let edges: Vec<String> =
            (0..lanes.len() - 1).map(|j| format!("{} {}", num(edge(&a, j)), num(edge(&b, j)))).collect();
        tube_tags.insert("edge".into(), edges.join(" "));
        let part = Part { outer: false, points: vec![a.0.p, b.0.p], z: Some(vec![floors[0], floors[1]]), plan: None };
        features.push(("tunnel-tube", tube_tags, part));
    }
}

/// The openings of the rooms that draw: those that the device opens.
pub(crate) fn built_openings(openings: &[Opening], draws: &[bool]) -> Vec<Opening> {
    (openings.iter()).filter(|o| draws[o.tube]).cloned().collect()
}

/// The open ends of the openings: each way of an opening moved in along it, for the ground plan's `graph::build_open`.
pub(crate) fn open_ends(openings: &[Opening], rooms: &rooms::Rooms) -> Vec<OpenEnd> {
    (openings.iter().flat_map(|o| rooms.ways(o)).filter(|w| w.shift > 0.0))
        .map(|w| OpenEnd { way: obc_ground_plan::frame::GlobalPoint { x: w.way[0], y: w.way[1] }, shift_m: w.shift })
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
