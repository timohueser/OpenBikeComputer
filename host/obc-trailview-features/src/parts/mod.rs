//! The tunnel parts that the bake places: per chain of tunnel ways its openings, the kind of part at each and its
//! numbers from the ground of the ground plan there, written as the features `tunnel-tube` and `tunnel-end` and the
//! tag `parts` of the tunnel ways (`specs/trailview-features.md`, Tunnel parts).

mod ladder;
mod measure;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

use obc_ground_plan::graph::level_tags::Chain;
use obc_ground_plan::graph::OpenEnd;
use obc_ground_plan::sections::{is_track, WidthRules};
use serde_json::{json, Map, Value};

use crate::geometry::key;
use crate::tiles::{frame_point, Tiles};
use crate::{metres, roads, Part, Tags, Terrain};

pub use ladder::Kind;

/// Two metres along a feature that the cells and the bake read at one node are this near.
const LINK_M: f64 = 0.5;
/// The rule values of the bake, in the order of `Bake`.
const BAKE_FIELDS: [&str; 18] = [
    "margin_m",
    "margin_narrow_m",
    "shell_m",
    "band_m",
    "coping_m",
    "face_over_crown_max_m",
    "cover_m",
    "wing_ground_m",
    "foot_m",
    "underpass_max_m",
    "pair_max_m",
    "pair_max_deg",
    "slab_m",
    "same_tube_gap_m",
    "same_tube_stagger_m",
    "rock_cover_max_m",
    "free_max_m",
    "junction_shift_max_m",
];
const GROUP_FIELDS: [&str; 5] = ["classes", "look", "collar_max_m", "wing_max_m", "least_height_m"];
/// The groups whose ends covered within `rock_cover_max_m` open in the rock.
const ROCK_GROUPS: [&str; 2] = ["path", "minor"];
/// A bore of this height or less takes `margin_narrow_m` past the way's half-width, as `boreHalf` of `tunnels.ts`.
const NARROW_BORE_M: f64 = 3.0;

/// The values of a group of road classes (`groups` of `parts.json`).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub classes: Vec<String>,
    pub look: String,
    pub collar_max_m: f64,
    pub wing_max_m: f64,
    pub least_height_m: f64,
}

/// The rule values of the placement (`bake` of `parts.json`), metres unless named.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bake {
    pub margin_m: f64,
    pub margin_narrow_m: f64,
    pub shell_m: f64,
    pub band_m: f64,
    pub coping_m: f64,
    pub face_over_crown_max_m: f64,
    pub cover_m: f64,
    pub wing_ground_m: f64,
    pub foot_m: f64,
    pub underpass_max_m: f64,
    pub pair_max_m: f64,
    pub pair_max_deg: f64,
    pub slab_m: f64,
    pub same_tube_gap_m: f64,
    pub same_tube_stagger_m: f64,
    pub rock_cover_max_m: f64,
    pub free_max_m: f64,
    pub junction_shift_max_m: f64,
}

/// The rules of `data/trailview/parts.json`, the option `parts` of the step.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    pub groups: Vec<Group>,
    pub bake: Bake,
}

impl Rules {
    /// The rules of the option `parts` (`kinds::part_rows`), or an error that names the field that breaks one.
    pub fn parse(rows: &Value) -> Result<Rules, String> {
        let number = |row: &Value, field: &str, at: &str| {
            row[field].as_f64().filter(|v| *v >= 0.0).ok_or(format!("parts {at}{field}: not a number of 0 or more"))
        };
        let bake = &rows["bake"];
        if bake.as_object().is_none_or(|b| b.len() != BAKE_FIELDS.len()) {
            return Err(format!("parts bake: the fields {BAKE_FIELDS:?}"));
        }
        let v: Vec<f64> = BAKE_FIELDS.iter().map(|f| number(bake, f, "bake.")).collect::<Result<_, _>>()?;
        let bake = Bake {
            margin_m: v[0],
            margin_narrow_m: v[1],
            shell_m: v[2],
            band_m: v[3],
            coping_m: v[4],
            face_over_crown_max_m: v[5],
            cover_m: v[6],
            wing_ground_m: v[7],
            foot_m: v[8],
            underpass_max_m: v[9],
            pair_max_m: v[10],
            pair_max_deg: v[11],
            slab_m: v[12],
            same_tube_gap_m: v[13],
            same_tube_stagger_m: v[14],
            rock_cover_max_m: v[15],
            free_max_m: v[16],
            junction_shift_max_m: v[17],
        };
        let mut groups = Vec::new();
        for (name, row) in rows["groups"].as_object().ok_or("parts groups: not an object")? {
            let at = format!("groups.{name}.");
            if row.as_object().is_none_or(|r| r.len() != GROUP_FIELDS.len()) {
                return Err(format!("parts {at}: the fields {GROUP_FIELDS:?}"));
            }
            let classes = row["classes"].as_array().ok_or(format!("parts {at}classes: not a list"))?;
            let classes: Vec<String> = classes.iter().filter_map(|c| c.as_str().map(String::from)).collect();
            let look = row["look"].as_str().ok_or(format!("parts {at}look: not a string"))?.to_string();
            groups.push(Group {
                name: name.clone(),
                classes,
                look,
                collar_max_m: number(row, "collar_max_m", &at)?,
                wing_max_m: number(row, "wing_max_m", &at)?,
                least_height_m: number(row, "least_height_m", &at)?,
            });
        }
        let mut seen = HashSet::new();
        if let Some(class) = groups.iter().flat_map(|g| &g.classes).find(|c| !seen.insert(c.as_str())) {
            return Err(format!("parts groups: the class {class} is in two groups"));
        }
        Ok(Rules { groups, bake })
    }

    /// The group of a road class.
    pub fn group(&self, class: &str) -> Option<usize> {
        self.groups.iter().position(|g| g.classes.iter().any(|c| c == class))
    }
}

/// What the pass reads: the chains of the levels pass, the ways of kind `tunnel` that the zoom-13 tiles hold with an
/// id (their tags and parts as placed), every way of the road solver (kind, tags, parts), the tiles and their tags of
/// the levels, the terrain, and the rules.
pub(crate) struct Input<'a> {
    pub chains: &'a [Chain],
    pub tunnels: &'a HashMap<u32, (Tags, Vec<Part>)>,
    pub ways: Vec<(&'a str, &'a Tags, &'a [Part])>,
    pub tiles: &'a Tiles,
    pub levels: &'a HashMap<u32, Tags>,
    pub terrain: &'a dyn Terrain,
    pub rules: &'a Rules,
    pub widths: &'a WidthRules,
    pub class_names: &'a [String],
}

/// What the pass gives: the features to place (kind, tags, the part with the floor heights), the tag `parts` of each
/// tunnel way by id, and its counts.
pub(crate) struct Placed {
    pub features: Vec<(&'static str, Tags, Part)>,
    pub tags: HashMap<u32, Tags>,
    pub metrics: Value,
    /// Per opening its way's id and its decision.
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
        for j in start..=next {
            if j > 0 && k > 0 {
                run += chord(j);
            }
            let along = if k == 0 || total <= 0.0 {
                plan[k].along
            } else {
                plan[k - 1].along + (plan[k].along - plan[k - 1].along) * (run / total)
            };
            out.push(Point { p: poly.points[j], z: z[j], s: 0.0, along });
        }
        at = next;
    }
    Some(out)
}

/// A run of tunnel ways of a chain with its ends: per end, the way's end vertex in the ground plan's frame and the way
/// there, or None where the run has no opening (a node where more tunnel ways go on).
#[derive(Clone, Debug)]
pub(crate) struct Tube {
    pub curve: Curve,
    pub ids: Vec<u32>,
    /// Every vertex has a height (`heights`), and each end with an opening is an end vertex of its way (`vertices`).
    pub heights: bool,
    pub vertices: bool,
    pub ends: [Option<(Option<[i64; 2]>, u32)>; 2],
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

/// The runs of a chain's ways of kind `tunnel`: a way of another kind lies on the ground in the new roads (a shallow
/// tunnel), so the run next to it opens there. Per run its ways, and per end whether it opens.
fn runs(chain: &Chain, tunnels: &HashMap<u32, (Tags, Vec<Part>)>) -> Vec<(Vec<(u32, f64, f64)>, [bool; 2])> {
    let mut out = Vec::new();
    let n = chain.ways.len();
    let mut k = 0;
    while k < n {
        if !tunnels.contains_key(&chain.ways[k].0) {
            k += 1;
            continue;
        }
        let start = k;
        while k < n && tunnels.contains_key(&chain.ways[k].0) {
            k += 1;
        }
        let open = [if start == 0 { chain.tag[0] } else { true }, if k == n { chain.tag[1] } else { true }];
        out.push((chain.ways[start..k].to_vec(), open));
    }
    out
}

/// The tube of a run of tunnel ways, or None where a way of it has no curve.
fn tube(run: &[(u32, f64, f64)], open: [bool; 2], input: &Input) -> Option<Tube> {
    let mut curve = Curve::default();
    let (mut heights, mut vertices) = (true, true);
    let mut ends = [None, None];
    let (mut widest, mut way_half, mut best) = (run[0].0, 0.0, f64::NEG_INFINITY);
    for (k, &(id, m_in, m_out)) in run.iter().enumerate() {
        let (tags, parts) = input.tunnels.get(&id)?;
        let (lo, hi) = (m_in.min(m_out) - LINK_M, m_in.max(m_out) + LINK_M);
        // The part of the way whose metres hold the stretch.
        let (p, points) = (parts.iter().enumerate())
            .filter_map(|(p, part)| Some((p, way_curve(part)?)))
            .max_by_key(|(_, ps)| ps.iter().filter(|q| q.along >= lo && q.along <= hi).count())?;
        let mut stretch: Vec<Point> = points.into_iter().filter(|q| q.along >= lo && q.along <= hi).collect();
        stretch.sort_by(|a, b| a.along.total_cmp(&b.along));
        if m_out < m_in {
            stretch.reverse();
        }
        let (first, last) = (stretch.first()?.along, stretch.last()?.along);
        vertices &= (first - m_in).abs() <= LINK_M && (last - m_out).abs() <= LINK_M;
        heights &= stretch.iter().all(|q| q.z.is_finite());
        // The end vertex of the way at an end of the run.
        let part = &parts[p];
        let plan = part.plan.as_ref()?;
        let vertex = |m: f64| {
            let n = part.points.len();
            [0, n - 1].into_iter().find(|&i| (plan[i].along - m).abs() <= LINK_M).map(|i| frame_point(parts, p, i))
        };
        if k == 0 && open[0] {
            ends[0] = Some((vertex(m_in), id));
        }
        if k + 1 == run.len() && open[1] {
            ends[1] = Some((vertex(m_out), id));
        }
        let w = input.widths.way("tunnel", tags, false);
        let widen = plan.iter().filter(|v| v.along >= lo && v.along <= hi).map(|v| v.widen).fold(0.0, f64::max);
        if w.half_m + widen > best {
            (best, widest, way_half) = (w.half_m + widen, id, w.half_m + widen);
        }
        // The chain's metres go on over the shared node.
        let base = curve.len;
        let mut s = base;
        for i in 0..stretch.len() {
            if i > 0 {
                s += distance(stretch[i - 1].p, stretch[i].p);
            }
            stretch[i].s = s;
        }
        curve.len = s;
        curve.stretches.push((id, stretch));
    }
    vertices &= ends.iter().flatten().all(|e| e.0.is_some());
    let (tags, _) = &input.tunnels[&widest];
    let w = input.widths.way("tunnel", tags, false);
    let class = input.class_names.get(w.rank).cloned().unwrap_or_default();
    let group = input.rules.group(&class)?;
    let height = crate::structures::bore(tags);
    let b = &input.rules.bake;
    let margin = if height <= NARROW_BORE_M { b.margin_narrow_m } else { b.margin_m };
    Some(Tube {
        ids: run.iter().map(|r| r.0).collect(),
        heights: heights && curve.len > 0.0,
        vertices,
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
/// vertex to it, its place on the chain, its point, its unit direction into the tube, its floor, and the ways other
/// than its own at the vertex.
#[derive(Clone, Debug)]
pub(crate) struct Opening {
    pub tube: usize,
    pub end: usize,
    /// None where the end is no end vertex of its way: its tube is not known.
    pub way: Option<[i64; 2]>,
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
    let runs: Vec<_> = input.chains.iter().flat_map(|c| runs(c, input.tunnels)).collect();
    let tubes: Vec<Tube> = runs.iter().filter_map(|(run, open)| tube(run, *open, input)).collect();
    // The chain ends that open on no part: on a shallow tunnel, where more tunnel ways go on, in a run with no tube.
    let shallow = (input.chains.iter())
        .flat_map(|c| [(c.tag[0], c.ways[0].0), (c.tag[1], c.ways[c.ways.len() - 1].0)])
        .filter(|&(tag, id)| tag && !input.tunnels.contains_key(&id))
        .count();
    let forks = tubes.iter().flat_map(|t| &t.ends).filter(|e| e.is_none()).count();
    let lost = runs.len() - tubes.len();
    // The ways at the end vertices of the tubes.
    let mut at_end: HashMap<[u64; 2], Vec<usize>> = HashMap::new();
    for t in &tubes {
        for (e, end) in t.ends.iter().enumerate() {
            if end.is_some() {
                let p = if e == 0 { t.curve.at(0.0).0.p } else { t.curve.at(t.curve.len).0.p };
                at_end.entry(key(p)).or_default();
            }
        }
    }
    for (w, (_, _, parts)) in input.ways.iter().enumerate() {
        for p in parts.iter().flat_map(|part| &part.points) {
            if let Some(list) = at_end.get_mut(&key(*p)) {
                if list.last() != Some(&w) {
                    list.push(w);
                }
            }
        }
    }
    let mut openings: Vec<Opening> = Vec::new();
    for (ti, t) in tubes.iter().enumerate() {
        for e in 0..2 {
            let Some((way, way_id)) = t.ends[e] else { continue };
            let s = if e == 0 { 0.0 } else { t.curve.len };
            let vertex = t.curve.at(s).0.p;
            // The ways at the vertex but the tube's own: a way's id is its place in the ways of the bake.
            let others: Vec<usize> =
                at_end.get(&key(vertex)).into_iter().flatten().copied().filter(|&w| w != way_id as usize).collect();
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
            o.others = others.len();
            if t.heights && t.vertices && others.len() >= 2 {
                let lines: Vec<(f64, Vec<[f64; 2]>)> = (others.iter())
                    .flat_map(|&w| {
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
        m.insert("runs_without_tube".into(), lost.into());
    }
    records(&tubes, &openings, &decided, input)
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
        let (half, height) = (decided.half[ti], decided.height[ti]);
        let section = if kinds.contains(&Some(Kind::Box)) { "box" } else { "round" };
        let (wtags, _) = &input.tunnels[&t.widest];
        let mut common = Tags::new();
        common.insert("section".into(), section.into());
        common.insert("half".into(), num(half));
        common.insert("height".into(), num(height));
        common.insert("look".into(), group.look.clone());
        // The tube from opening to opening; an end with no opening holds the end vertex.
        let s0 = openings.iter().find(|o| o.tube == ti && o.end == 0).map_or(0.0, |o| o.s);
        let s1 = openings.iter().find(|o| o.tube == ti && o.end == 1).map_or(t.curve.len, |o| o.s);
        for (way, ps) in &t.curve.stretches {
            let rising = ps.last().unwrap().along >= ps[0].along;
            let order = if rising { [kinds[0], kinds[1]] } else { [kinds[1], kinds[0]] };
            let ends = format!("{} {}", part_name(order[0]), part_name(order[1]));
            let mut way_tags = Tags::new();
            way_tags.insert("parts".into(), ends.clone());
            tags.insert(*way, way_tags);
            // The points of the stretch within the openings, with a point at each opening.
            let (a, z) = (ps[0].s.max(s0), ps.last().unwrap().s.min(s1));
            if z - a <= 1e-6 {
                continue;
            }
            let mut pts: Vec<Point> = vec![t.curve.at(a).0];
            pts.extend(ps.iter().filter(|q| q.s > a + 1e-6 && q.s < z - 1e-6).copied());
            pts.push(t.curve.at(z).0);
            for w in pts.windows(2) {
                let (p, q) = if rising { (w[0], w[1]) } else { (w[1], w[0]) };
                let mut tube_tags = common.clone();
                tube_tags.insert("class".into(), t.class.clone());
                if let Some(surface) = wtags.get("surface") {
                    tube_tags.insert("surface".into(), surface.clone());
                }
                tube_tags.insert("road".into(), num(t.road));
                tube_tags.insert("along".into(), num(p.along));
                tube_tags.insert("ends".into(), ends.clone());
                let part = Part { outer: false, points: vec![p.p, q.p], z: Some(vec![p.z, q.z]), plan: None };
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
        t.insert("height".into(), num(decided.height[o.tube]));
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
        let way = o.way.expect("a drawn opening of a known tube");
        t.insert("way".into(), format!("{} {}", way[0], way[1]));
        t.insert("shift".into(), num(o.shift));
        if let Some((mate, m)) = n.mate.as_ref().map(|(i, m)| (&openings[*i], *m)) {
            t.insert("mate".into(), num(m));
            let way = mate.way.expect("a drawn opening of a known tube");
            t.insert("mate_way".into(), format!("{} {}", way[0], way[1]));
            t.insert("mate_shift".into(), num(mate.shift));
        }
        let part = Part { outer: false, points: vec![o.p], z: Some(vec![o.floor]), plan: None };
        features.push(("tunnel-end", t, part));
    }
    let ends = openings.iter().map(|o| o.way_id).zip(decided.ends.iter().cloned()).collect();
    Placed { features, tags, metrics: decided.metrics.clone(), ends }
}

/// The open ends of the openings: each moved opening on its way, for the ground plan's `graph::build_open`.
pub(crate) fn open_ends(openings: &[Opening]) -> Vec<OpenEnd> {
    (openings.iter().filter(|o| o.shift > 0.0))
        .filter_map(|o| {
            Some(OpenEnd { way: obc_ground_plan::frame::GlobalPoint { x: o.way?[0], y: o.way?[1] }, shift_m: o.shift })
        })
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
