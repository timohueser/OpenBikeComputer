//! Storey 0's guard rails and delineator posts: where they stand (`place`, in the cell build) and their solids in the
//! storey mesh (`solids`). Both stand on their way's own shoulder, so each corner takes the label of the face under it
//! and the drawn road's height, and a post or a delineator starts on paved ground and goes down into the fill.
//!
//! A chain is the ground road ways that joints link. Its rings are the graph samples and a post point at each `POST_M`
//! of stroke metres. Per side, a ring needs a rail where the paved edge lies `DROP_M` over the plain terrain
//! `DROP_OUT_M` out, or where a mapped guard rail lies near; `shares` gives each ring its share by the stretch rules of
//! `roads/standing.ts`. A rail sinks into the ground before a stop: a chain's end at a junction or past the graph, the
//! zone of a junction on its own arms, a sidewalk, a town, a fold, or a ring whose beam or post leaves its own shoulder
//! or carriageway. At a level change (a deck's end, a tunnel's opening, a junction at one) it runs on and ends square.
//! The rules read the graph, the plain terrain and the features, which two cells hold alike about their shared line,
//! and the face plan, which they hold alike only near it: the face test runs only near the cell.

use super::shell::{clip, prism, Build, BAND, DELINEATOR, P, RAIL, REFLECTOR};
use crate::faces::records::find;
use crate::faces::{FacePlan, Owner, Surface, OUTSIDE};
use crate::frame::{metres_per_unit_at, GlobalPoint, CELL_UNITS};
use crate::graph::{point as curve_point, u_at, Arm, CrossingKind, Family, Level, Network, Sample, WayGraph};
use crate::sections::Sections;
use crate::tile::Geometry;
use crate::CellInput;
use std::collections::BTreeMap;

/// The classes with guard rails and delineators outside towns (`roads/standing.ts`).
const RAILED: [&str; 9] = [
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
    "unclassified",
];
/// Today's test: a rail stands where the edge lies this much over the plain terrain this far out.
const DROP_M: f64 = 1.35;
const DROP_OUT_M: f64 = 4.0;
/// A rail runs on this far past the last ring that needs it, then ramps down over `RAMP_M`, as a terminal does.
const LEAD_M: f64 = 4.0;
const RAMP_M: f64 = 6.0;
/// Two stretches stay this far apart past their ramps, else they join: a narrower break reads as a notch.
const BREAK_M: f64 = 8.0;
const JOIN_M: f64 = 2.0 * (LEAD_M + RAMP_M) + BREAK_M;
/// A rail ramps down over this before a stop.
const ZONE_RAMP_M: f64 = 5.0;
/// A town: this many buildings within this far. A shorter town than `BREAK_M` stops nothing.
const TOWN_BUILDINGS: usize = 3;
const TOWN_M: f64 = 40.0;
/// A mapped guard rail counts within this of the shoulder's outer edge.
const MAPPED_M: f64 = 2.5;
/// A post every this many stroke metres; a ring at each, so a straight beam runs this far between rings.
const POST_M: f64 = 4.0;
/// A delineator every `STEP_M` of stroke metres, and every `BEND_STEP_M` where the line turns `BEND_RAD` within
/// `BEND_M` each way; none within `CLEAR_M` of a stop.
const STEP_M: f64 = 40.0;
const BEND_STEP_M: f64 = 20.0;
const BEND_RAD: f64 = 0.35;
const BEND_M: f64 = 15.0;
const CLEAR_M: f64 = 5.0;
/// The beam, a box: its front and back in from the shoulder's outer edge, its foot and top over the road. The rail
/// keeps 5 cm off the edge, more than the most that the face plan's chord of the edge leaves the curve.
const BEAM_IN_M: [f64; 2] = [0.22, 0.13];
const BEAM_UP_M: [f64; 2] = [0.44, 0.75];
/// A beam that does not stand has sunk this far: its top 5 cm under the shoulder.
const SINK_M: f64 = 0.8;
/// A post behind the beam: in from the edge, half its width along, its foot and top.
const POST_IN_M: [f64; 2] = [0.13, 0.05];
const POST_HALF_M: f64 = 0.04;
const POST_UP_M: [f64; 2] = [-1.0, 0.71];
/// A delineator: its middle in from the edge, half its depth and width, its foot, band and top; its reflector's half
/// width, rise and how far proud of the band it lies.
const DELI_IN_M: f64 = 0.10;
const DELI_HALF_M: [f64; 2] = [0.05, 0.06];
const DELI_UP_M: [f64; 4] = [-0.5, 0.74, 0.9, 1.0];
const REFLECTOR_M: [f64; 4] = [0.03, 0.77, 0.87, 0.004];
/// Nothing stands within this past a junction's widest way and corner, or past a level crossing's rail bed, whose
/// half-width this is (the rail classes' half and shoulder).
const MOUTH_M: f64 = 5.0;
const RAIL_BED_M: f64 = 2.25;
/// The face test finds the nearest face edge within this; a point farther from every edge passes it.
const FACE_REACH_M: f64 = 6.0;
const BUCKET: f64 = 8192.0;

/// A ring of an edge feature: its point on the shoulder's outer edge (frame units from the cell's origin), the unit
/// normal out of the road and the unit tangent along it, the metres per unit there, the share that it stands by
/// (0 sunk, 1 full) and whether a post stands at it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ring {
    pub edge: [f64; 2],
    pub out: [f64; 2],
    pub along: [f64; 2],
    pub mpu: f64,
    pub share: f64,
    pub post: bool,
}

/// A cell's guard rails, each the rings of one run, and its delineators.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Edges {
    pub rails: Vec<Vec<Ring>>,
    pub delineators: Vec<Ring>,
}

/// How a chain ends: at a level change (the rail runs on), where nothing may stand, or past the graph.
#[derive(Clone, Copy, PartialEq)]
enum End {
    Flush,
    Stop,
    Open,
}

/// Items in square buckets of frame units, by their boxes.
struct Grid(BTreeMap<(i64, i64), Vec<u32>>, f64);

impl Grid {
    fn of(size: f64, boxes: &[[f64; 4]]) -> Grid {
        let mut g = Grid(BTreeMap::new(), size);
        for (id, &[x0, y0, x1, y1]) in boxes.iter().enumerate() {
            for y in g.key(y0)..=g.key(y1) {
                for x in g.key(x0)..=g.key(x1) {
                    g.0.entry((x, y)).or_default().push(id as u32);
                }
            }
        }
        g
    }

    fn key(&self, v: f64) -> i64 {
        libm::floor(v / self.1) as i64
    }

    /// The items whose buckets meet the square of half side r about q, each once.
    fn near(&self, q: [f64; 2], r: f64) -> Vec<u32> {
        let mut out = Vec::new();
        for y in self.key(q[1] - r)..=self.key(q[1] + r) {
            for x in self.key(q[0] - r)..=self.key(q[0] + r) {
                out.extend(self.0.get(&(x, y)).into_iter().flatten());
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: [f64; 2], b: [f64; 2], k: f64) -> [f64; 2] {
    [a[0] + k * b[0], a[1] + k * b[1]]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    libm::hypot(a[0] - b[0], a[1] - b[1])
}

fn bbox(a: [f64; 2], b: [f64; 2]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}

/// The distance from q to the segment a → b.
fn to_segment(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = sub(b, a);
    let l2 = dot(d, d);
    let t = if l2 > 0.0 { (dot(sub(q, a), d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    dist(q, add(a, d, t))
}

/// What the rules read about a cell: the face plan's edges, the buildings' boxes and the mapped guard rails.
struct Context<'a> {
    plan: &'a FacePlan,
    edges: Grid,
    buildings: (Grid, Vec<[f64; 4]>),
    rails: (Grid, Vec<[[f64; 2]; 2]>),
    input: &'a CellInput,
}

impl<'a> Context<'a> {
    fn new(plan: &'a FacePlan, input: &'a CellInput) -> Context<'a> {
        let v = |k: u32| plan.vertices[k as usize].map(f64::from);
        let edges = Grid::of(BUCKET, &plan.edges.iter().map(|e| bbox(v(e.a), v(e.b))).collect::<Vec<_>>());
        let o = input.key.origin();
        let local = |p: &GlobalPoint| [(p.x - o.x) as f64, (p.y - o.y) as f64];
        let (mut boxes, mut segments) = (Vec::new(), Vec::new());
        for f in &input.features {
            for part in &f.parts {
                let pts: Vec<[f64; 2]> = part.points.iter().map(local).collect();
                if f.kind == "building" && f.geometry == Geometry::Area && part.outer {
                    let (lo, hi) = (f64::NEG_INFINITY, f64::INFINITY);
                    boxes.push(pts.iter().fold([hi, hi, lo, lo], |b, p| {
                        [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
                    }));
                }
                if f.kind == "guard-rail" && f.geometry == Geometry::Line {
                    segments.extend(pts.windows(2).map(|w| [w[0], w[1]]));
                }
            }
        }
        let buildings = (Grid::of(4.0 * BUCKET, &boxes), boxes);
        let rails = (Grid::of(BUCKET, &segments.iter().map(|s| bbox(s[0], s[1])).collect::<Vec<_>>()), segments);
        Context { plan, edges, buildings, rails, input }
    }

    /// Whether q leaves the shoulders and way `way`'s own carriageway: the face on q's side of the nearest face edge
    /// within `FACE_REACH_M`. None where the face plan does not reach q.
    fn leaves(&self, q: [f64; 2], way: u32, mpu: f64) -> bool {
        let reach = FACE_REACH_M / mpu;
        let v = |k: u32| self.plan.vertices[k as usize].map(f64::from);
        let mut best: Option<(f64, f64, u32)> = None;
        for e in self.edges.near(q, reach) {
            let edge = &self.plan.edges[e as usize];
            let (a, b) = (v(edge.a), v(edge.b));
            let (d, ab) = (to_segment(q, a, b), sub(b, a));
            let cross = ab[0] * (q[1] - a[1]) - ab[1] * (q[0] - a[0]);
            let across = cross.abs() / libm::hypot(ab[0], ab[1]).max(1e-9);
            if d <= reach && best.is_none_or(|(bd, ba, _)| d < bd - 1e-9 || (d <= bd + 1e-9 && across > ba)) {
                best = Some((d, across, if cross > 0.0 { edge.left } else { edge.right }));
            }
        }
        best.is_some_and(|(_, _, f)| {
            let face = (f != OUTSIDE).then(|| &self.plan.faces[f as usize]);
            face.is_some_and(|f| {
                f.surface != Surface::Shoulder && (f.surface != Surface::Carriageway || f.owner != Owner::Way(way))
            })
        })
    }

    fn town(&self, q: [f64; 2], mpu: f64) -> bool {
        let (r, (grid, boxes)) = (TOWN_M / mpu, &self.buildings);
        let near = grid.near(q, r).into_iter().filter(|&k| {
            let [x0, y0, x1, y1] = boxes[k as usize];
            libm::hypot((x0 - q[0]).max(q[0] - x1).max(0.0), (y0 - q[1]).max(q[1] - y1).max(0.0)) <= r
        });
        near.take(TOWN_BUILDINGS).count() >= TOWN_BUILDINGS
    }

    fn mapped(&self, q: [f64; 2], mpu: f64) -> bool {
        let (r, (grid, segments)) = (MAPPED_M / mpu, &self.rails);
        grid.near(q, r).into_iter().any(|k| to_segment(q, segments[k as usize][0], segments[k as usize][1]) <= r)
    }

    fn terrain(&self, q: [f64; 2]) -> Option<f64> {
        let o = self.input.key.origin();
        self.input.terrain.height(GlobalPoint { x: o.x + libm::round(q[0]) as i64, y: o.y + libm::round(q[1]) as i64 })
    }
}

/// A point of a chain: its place (frame units), its unit tangent in the chain's direction, its height, bank and
/// widening, the sides of its way with a sidewalk (1 its left, 2 its right), its stroke metres, whether it is a post
/// point, its way and the way's direction in the chain.
#[derive(Clone, Copy)]
struct Point {
    pos: [f64; 2],
    t: [f64; 2],
    z: f64,
    bank: f64,
    widen: f64,
    walk: u8,
    along: f64,
    post: bool,
    way: u32,
    dir: f64,
}

/// The points of way w in direction d: its graph samples and a post point at each `POST_M` of stroke metres
/// between them; a sample within a metre of a post point gives way to it.
fn way_points(g: &WayGraph, w: u32, d: i8, out: &mut Vec<Point>) {
    let way = &g.ways[w as usize];
    let part = &g.parts[way.part as usize];
    let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
    let o = g.cell.origin();
    let of = |s: &Sample| Point {
        pos: s.pos.map(f64::from),
        t: s.tangent,
        z: s.z_m,
        bank: s.bank,
        widen: s.widen_m,
        walk: s.walk,
        along: s.along_m,
        post: false,
        way: w,
        dir: f64::from(d),
    };
    let mut pts: Vec<Point> = Vec::new();
    for (s, n) in samples.iter().zip(samples.iter().skip(1).map(Some).chain([None])) {
        pts.push(of(s));
        let Some(n) = n else { break };
        let span = &part.spans[s.span as usize];
        let v = part.vertices[s.span as usize].pos;
        let sign = if part.backward { -1.0 } else { 1.0 };
        let (lo, hi) = (s.along_m.min(n.along_m), s.along_m.max(n.along_m));
        let mut posts: Vec<Point> = (libm::floor(lo / POST_M) as i64 + 1..)
            .map(|k| k as f64 * POST_M)
            .take_while(|&along| along < hi)
            .map(|along| {
                let p = curve_point(part, s.span as usize, u_at(span, (along - span.along0_m) * sign));
                let pos = [(v[0] - o.x) as f64 + p.d[0], (v[1] - o.y) as f64 + p.d[1]];
                Point { pos, t: p.tangent, z: p.z_m, bank: p.bank, widen: p.widen_m, along, post: true, ..of(s) }
            })
            .collect();
        if s.along_m > n.along_m {
            posts.reverse();
        }
        pts.extend(posts);
    }
    pts.dedup_by(|b, a| a.along == b.along);
    let n = pts.len();
    let near_post = |i: usize| {
        let post_at = |j: usize| pts[j].post && (pts[j].along - pts[i].along).abs() < 1.0;
        !pts[i].post && i > 0 && i + 1 < n && (post_at(i - 1) || post_at(i + 1))
    };
    let mut kept: Vec<Point> = (0..n).filter(|&i| !near_post(i)).map(|i| pts[i]).collect();
    if d < 0 {
        kept.reverse();
    }
    out.extend(kept.into_iter().map(|p| Point { t: p.t.map(|v| v * f64::from(d)), ..p }));
}

/// Ground road ways through joints, each with its direction, and how the chain ends at its first and last way.
type Chain = (Vec<(u32, i8)>, [End; 2]);

/// The ground road ways in chains through the joints, each with its direction, and how each chain ends at its first
/// and its last way. A junction where `flush` holds ends its arms as a level change does.
fn chains(g: &WayGraph, flush: &[bool]) -> Vec<Chain> {
    let ground = |w: u32| {
        let way = &g.ways[w as usize];
        way.family == Family::Road && way.level == Level::Ground && way.samples.len() >= 2
    };
    // An arm at its way's start points along it (+1), at its end back (−1); an arm inside a way ends nothing.
    let at_end = |a: &Arm| a.at == if a.dir > 0 { g.ways[a.way as usize].from } else { g.ways[a.way as usize].to };
    // At each way end (way, +1 its start, −1 its end): the way and the direction on, or how it ends.
    let mut next: BTreeMap<(u32, i8), Result<(u32, i8), End>> = BTreeMap::new();
    for j in &g.joints {
        for (a, b) in [(&j.arms[0], &j.arms[1]), (&j.arms[1], &j.arms[0])].into_iter().filter(|(a, _)| at_end(a)) {
            let other = &g.ways[b.way as usize];
            let on = match (ground(b.way), other.family == Family::Road) {
                (true, _) => Ok((b.way, b.dir)),
                (false, true) if other.level != Level::Ground => Err(End::Flush),
                _ => Err(End::Stop),
            };
            next.insert((a.way, a.dir), on);
        }
    }
    for (k, j) in g.junctions.iter().enumerate() {
        for a in j.arms.iter().filter(|a| at_end(a)) {
            next.insert((a.way, a.dir), Err(if flush[k] { End::Flush } else { End::Stop }));
        }
    }
    for a in g.ends.iter().map(|e| &e.arm).filter(|a| at_end(a)) {
        next.insert((a.way, a.dir), Err(End::Stop));
    }
    let mut seen = vec![false; g.ways.len()];
    let mut out = Vec::new();
    for w in (0..g.ways.len() as u32).filter(|&w| ground(w)) {
        if std::mem::replace(&mut seen[w as usize], true) {
            continue;
        }
        let mut chain = std::collections::VecDeque::from([(w, 1i8)]);
        let mut ends = [End::Open; 2];
        // Forward from the last way's exit, then back from the first way's entry.
        for back in [false, true] {
            loop {
                let (x, d) = if back { chain[0] } else { chain[chain.len() - 1] };
                match next.get(&(x, if back { d } else { -d })) {
                    Some(&Ok((y, yd))) if !seen[y as usize] => {
                        seen[y as usize] = true;
                        if back {
                            chain.push_front((y, -yd));
                        } else {
                            chain.push_back((y, yd));
                        }
                    }
                    Some(&Err(e)) => break ends[usize::from(!back)] = e,
                    _ => break,
                }
            }
        }
        out.push((chain.into_iter().collect(), ends));
    }
    out
}

/// The stretches where a rail must stand, from the rings that pass its test: a gap of `JOIN_M` or less closes, and
/// no stretch runs over a stop (`stretches` of standing.ts).
fn stretches(m: &[f64], stops: &[bool], passes: &[bool]) -> Vec<bool> {
    let mut out = vec![false; m.len()];
    let mut run: Option<(usize, usize)> = None;
    for i in 0..m.len() {
        let close = stops[i] || (passes[i] && run.is_some_and(|(_, last)| m[i] - m[last] > JOIN_M));
        if close {
            if let Some((f, last)) = run.take() {
                out[f..=last].fill(true);
            }
        }
        if passes[i] && !stops[i] {
            run = Some((run.map_or(i, |r| r.0), i));
        }
    }
    if let Some((f, last)) = run {
        out[f..=last].fill(true);
    }
    out
}

/// Per ring the metres to the nearest marked ring that no stop cuts it off from.
fn metres_to(m: &[f64], stops: &[bool], marks: &[bool]) -> Vec<f64> {
    let n = m.len();
    let mut out = vec![f64::INFINITY; n];
    for order in [false, true] {
        let mut from = None;
        for k in 0..n {
            let i = if order { n - 1 - k } else { k };
            if stops[i] {
                from = None;
            } else if marks[i] {
                from = Some(m[i]);
            }
            out[i] = out[i].min(from.map_or(f64::INFINITY, |f: f64| (m[i] - f).abs()));
        }
    }
    out
}

/// Takes away a stretch of `shares` shorter than `least` metres that lies inside the chain.
fn without_stubs(shares: &mut [f64], m: &[f64], least: f64) {
    let mut from = None;
    for j in 0..=shares.len() {
        match (j < shares.len() && shares[j] > 0.0, from) {
            (true, None) => from = Some(j),
            (false, Some(f)) => {
                if f > 0 && j < shares.len() && m[j] - m[f] < least {
                    shares[f..j].fill(0.0);
                }
                from = None;
            }
            _ => {}
        }
    }
}

/// Per ring its share by the stretch rules, from its stroke metres, need and stops. A level change at an end of the
/// chain (`flush`) joins a stretch as a need does, as the deck's parapet or the portal carries the rail on, but starts
/// none: a rail that reaches it runs on full and ends square there.
fn shares(m: &[f64], need: &[bool], stops: &[bool], flush: [bool; 2]) -> Vec<f64> {
    let n = m.len();
    let mut passes = need.to_vec();
    passes[0] |= flush[0];
    passes[n - 1] |= flush[1];
    let mut marks = stretches(m, stops, &passes);
    let mut i = 0;
    while i < n {
        let j = (i..n).find(|&k| marks[k] != marks[i]).unwrap_or(n);
        if !need[i..j].contains(&true) {
            marks[i..j].fill(false);
        }
        i = j;
    }
    let to_rail = metres_to(m, stops, &marks);
    let zone = metres_to(m, &vec![false; n], stops);
    let smooth = |t: f64| t.clamp(0.0, 1.0) * t.clamp(0.0, 1.0) * (3.0 - 2.0 * t.clamp(0.0, 1.0));
    let mut out: Vec<f64> =
        (0..n).map(|i| smooth(((LEAD_M + RAMP_M - to_rail[i]) / RAMP_M).min(zone[i] / ZONE_RAMP_M))).collect();
    without_stubs(&mut out, m, 2.0 * RAMP_M);
    out
}

/// `f` of each ring.
fn flat(n: usize, f: impl Fn(usize) -> bool) -> Vec<bool> {
    (0..n).map(f).collect()
}

/// The guard rails and delineators about a cell: see the module.
pub fn place(g: &WayGraph, plan: &FacePlan, input: &CellInput, sections: &Sections) -> Edges {
    let origin_y = input.key.origin().y as f64;
    let mpu_at = |q: [f64; 2]| metres_per_unit_at(origin_y + q[1]);
    let section = |w: u32| {
        let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
        sections.way(&f.kind, &f.tags, false)
    };
    let road = |w: u32| g.ways[w as usize].family == Family::Road;
    // Per road way: whether it is railed, and the half-width of its carriageway and shoulder.
    let info: Vec<Option<(bool, f64)>> = (0..g.ways.len() as u32)
        .map(|w| road(w).then(|| section(w)).map(|s| (RAILED.contains(&s.class), s.half_m + s.shoulder_m)))
        .collect();
    let changes: Vec<[f64; 2]> = (g.joints.iter())
        .filter(|j| {
            j.arms.iter().all(|a| road(a.way)) && j.arms.iter().any(|a| g.ways[a.way as usize].level != Level::Ground)
        })
        .map(|j| j.pos.map(f64::from))
        .collect();
    // The zones where nothing stands: about a junction, on its own arms, its widest way's paved half-width and corner
    // and `MOUTH_M`; about a level crossing, `MOUTH_M` past the rail bed. A junction whose zone holds a level change
    // is that level change: its arms end flush, and it stops nothing.
    let mut flush = vec![false; g.junctions.len()];
    let mut zones: Vec<([f64; 2], f64, Vec<u32>)> = Vec::new();
    for (k, j) in g.junctions.iter().enumerate().filter(|(_, j)| j.network == Network::Way && j.arms.len() > 2) {
        let r = (j.arms.iter().map(|a| section(a.way)))
            .map(|s| s.half_m + s.widen_m + s.shoulder_m + s.corner_radius_m + MOUTH_M)
            .fold(0.0, f64::max);
        let p = j.pos.map(f64::from);
        let r = r / mpu_at(p);
        flush[k] = j.arms.iter().any(|a| road(a.way) && g.ways[a.way as usize].level != Level::Ground)
            || changes.iter().any(|&c| dist(c, p) < r);
        if !flush[k] {
            zones.push((p, r, j.arms.iter().map(|a| a.way).collect()));
        }
    }
    for x in g.crossings.iter().filter(|x| x.kind == CrossingKind::LevelCrossing) {
        let p = x.pos.map(f64::from);
        zones.push((p, (RAIL_BED_M + MOUTH_M) / mpu_at(p), Vec::new()));
    }
    let in_zones =
        Grid::of(BUCKET, &zones.iter().map(|&(q, r, _)| [q[0] - r, q[1] - r, q[0] + r, q[1] + r]).collect::<Vec<_>>());
    let in_zone = |p: &Point| {
        in_zones.near(p.pos, 0.0).into_iter().any(|k| {
            let (q, r, ways) = &zones[k as usize];
            dist(*q, p.pos) < *r && (ways.is_empty() || ways.contains(&p.way))
        })
    };
    let c = CELL_UNITS as f64;
    // A ring within this of the cell square may reach into it.
    let near_cell = |q: [f64; 2], mpu: f64| {
        let r = 2.0 * (LEAD_M + RAMP_M + BREAK_M) / mpu;
        q[0] > -r && q[1] > -r && q[0] < c + r && q[1] < c + r
    };
    let mut ctx: Option<Context> = None;
    let mut out = Edges::default();
    for (chain, ends) in chains(g, &flush) {
        let mut pts = Vec::new();
        for &(w, d) in &chain {
            way_points(g, w, d, &mut pts);
        }
        pts.dedup_by(|b, a| dist(a.pos, b.pos) < 1.0);
        let n = pts.len();
        let mpu: Vec<f64> = pts.iter().map(|p| mpu_at(p.pos)).collect();
        if n < 2 || !(0..n).any(|i| near_cell(pts[i].pos, mpu[i])) {
            continue;
        }
        let mut m = vec![0.0; n];
        for i in 1..n {
            m[i] = m[i - 1] + dist(pts[i].pos, pts[i - 1].pos) * mpu[i];
        }
        let ctx = &*ctx.get_or_insert_with(|| Context::new(plan, input));
        for side in [1.0, -1.0] {
            // Per ring its point on the shoulder's outer edge, the normal out (the chain's left, as the frame's y
            // points south, then this side's), whether its way is railed and its paved half-width.
            let rings: Vec<([f64; 2], [f64; 2], bool, f64)> = (0..n)
                .map(|i| {
                    let p = &pts[i];
                    let (railed, e) = info[p.way as usize].unwrap_or((false, 0.0));
                    let nrm = [side * p.t[1], -side * p.t[0]];
                    (add(p.pos, nrm, (e + p.widen) / mpu[i]), nrm, railed, e + p.widen)
                })
                .collect();
            let at = |i: usize, d: f64| add(rings[i].0, rings[i].1, d / mpu[i]);
            let mapped = flat(n, |i| ctx.mapped(rings[i].0, mpu[i]));
            let drops = flat(n, |i| {
                // The bank lifts the way's left: this side is its left where it is the chain's left along the way.
                let (p, e) = (&pts[i], rings[i].3);
                let z = p.z + p.bank * side * p.dir * e;
                rings[i].2 && ctx.terrain(at(i, DROP_OUT_M)).is_some_and(|t| z - t > DROP_M)
            });
            // The town and face tests run only where a rail or a delineator may stand.
            let may = metres_to(&m, &vec![false; n], &flat(n, |i| mapped[i] || drops[i]));
            let on_step = |i: usize| {
                let grid = libm::fmod(pts[i].along.abs(), BEND_STEP_M);
                grid.min(BEND_STEP_M - grid) <= CLEAR_M + 1.0 && near_cell(rings[i].0, mpu[i])
            };
            let town =
                flat(n, |i| rings[i].2 && (may[i] <= JOIN_M + CLEAR_M || on_step(i)) && ctx.town(rings[i].0, mpu[i]));
            let need = flat(n, |i| mapped[i] || (drops[i] && !town[i]));
            let mut towns: Vec<f64> = town.iter().map(|&t| f64::from(u8::from(t))).collect();
            without_stubs(&mut towns, &m, BREAK_M);
            let fold = |j: usize| dot(sub(rings[j + 1].0, rings[j].0), add(pts[j].t, pts[j + 1].t, 1.0)) <= 0.0;
            let flush = [ends[0] == End::Flush, ends[1] == End::Flush];
            // The face test: a ring whose beam front or post leaves its own paved faces holds no rail, and the rail
            // sinks before it as before any stop. A ring on a level change's end line meets the deck or the portal
            // there by construction.
            let leaves = |i: usize| {
                [BEAM_IN_M[0], (POST_IN_M[0] + POST_IN_M[1]) / 2.0]
                    .iter()
                    .any(|&d| ctx.leaves(at(i, -d), pts[i].way, mpu[i]))
            };
            let end_line = |i: usize| (i == 0 && flush[0]) || (i + 1 == n && flush[1]);
            let fails = flat(n, |i| may[i] <= JOIN_M && !end_line(i) && near_cell(rings[i].0, mpu[i]) && leaves(i));
            let stops = flat(n, |i| {
                let bit = if side * pts[i].dir > 0.0 { 1 } else { 2 };
                (i == 0 && ends[0] == End::Stop)
                    || (i + 1 == n && ends[1] == End::Stop)
                    || in_zone(&pts[i])
                    || pts[i].walk & bit != 0
                    || (towns[i] > 0.0 && !mapped[i])
                    || (i > 0 && fold(i - 1))
                    || (i + 1 < n && fold(i))
                    || fails[i.saturating_sub(1)..(i + 2).min(n)].contains(&true)
            });
            let share = shares(&m, &need, &stops, flush);
            let ring = |i: usize| Ring {
                edge: rings[i].0,
                out: rings[i].1,
                along: pts[i].t,
                mpu: mpu[i],
                share: share[i],
                post: pts[i].post && share[i] > 0.0,
            };
            // Runs: the rings with a share and the sunk ring on each side; one sunk ring between two runs joins them.
            let mut i = 0;
            while i < n {
                if share[i] <= 0.0 {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j + 1 < n && (share[j + 1] > 0.0 || (j + 2 < n && share[j + 2] > 0.0)) {
                    j += 1;
                }
                let run: Vec<usize> = (i.saturating_sub(1)..=(j + 1).min(n - 1)).collect();
                if run.iter().any(|&k| near_cell(rings[k].0, mpu[k])) {
                    let pieces = run.split(|&k| fails[k]).filter(|p| p.len() > 1);
                    out.rails.extend(pieces.map(|p| p.iter().map(|&k| ring(k)).collect()));
                }
                i = j + 1;
            }
            // Delineators where no rail stands, clear of every stop.
            let zone = metres_to(&m, &vec![false; n], &stops);
            let turn = |i: usize| {
                let lo = (0..=i).rev().find(|&k| m[i] - m[k] >= BEND_M).unwrap_or(0);
                let hi = (i..n).find(|&k| m[k] - m[i] >= BEND_M).unwrap_or(n - 1);
                libm::acos(dot(pts[lo].t, pts[hi].t).clamp(-1.0, 1.0)) > BEND_RAD
            };
            for i in (0..n).filter(|&i| rings[i].2 && pts[i].post && !town[i] && zone[i] >= CLEAR_M) {
                let on = |step: f64| libm::fmod(pts[i].along, step).abs() < 1e-6;
                if share[i.saturating_sub(1)..(i + 2).min(n)].iter().all(|&v| v <= 0.0)
                    && (on(STEP_M) || (on(BEND_STEP_M) && turn(i)))
                    && near_cell(rings[i].0, mpu[i])
                    && !leaves(i)
                {
                    out.delineators.push(ring(i));
                }
            }
        }
    }
    out
}

/// A triangle or a quad on corners `c` at the points `p` (metres east, north and up, about any origin), turned to
/// face `out`.
fn facing(b: &mut Build, part: usize, c: &[u32], p: &[[f64; 3]], out: [f64; 3]) {
    let (u, v) = ([0, 1, 2].map(|k| p[1][k] - p[0][k]), [0, 1, 2].map(|k| p[2][k] - p[0][k]));
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let flip = n[0] * out[0] + n[1] * out[1] + n[2] * out[2] < 0.0;
    let k = |i: usize| c[if flip && i > 0 { c.len() - i } else { i }];
    if c.len() == 3 {
        b.tri(part, [k(0), k(1), k(2)]);
    } else {
        b.quad(part, [k(0), k(1), k(2), k(3)]);
    }
}

/// A point of a beam segment's plan: its place, its share of the way from the segment's first ring, and the side
/// that starts at it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vertex {
    q: [f64; 2],
    t: f64,
    side: Side,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Front,
    Back,
    /// The segment's end at its first ring, or at its second.
    First,
    Second,
    Cell,
}

/// A convex plan cut by the cell square. A cut point is set exactly on its cell line, and reckoned from the edge's
/// two ends in a fixed order, so two segments that share an edge, and two cells, cut it alike.
fn clip_square(mut poly: Vec<Vertex>) -> Vec<Vertex> {
    let c = CELL_UNITS as f64;
    for (axis, bound, low) in [(0, 0.0, false), (0, c, true), (1, 0.0, false), (1, c, true)] {
        let inside = |v: &Vertex| if low { v.q[axis] <= bound } else { v.q[axis] >= bound };
        let mut out = Vec::new();
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if inside(&a) {
                out.push(a);
            }
            if inside(&a) != inside(&b) {
                let (p, r) = if (a.q[0], a.q[1]) < (b.q[0], b.q[1]) { (a, b) } else { (b, a) };
                let f = (bound - p.q[axis]) / (r.q[axis] - p.q[axis]);
                let mut q = add(p.q, sub(r.q, p.q), f);
                q[axis] = bound;
                // The cut point starts the side that a went on; the cell line starts where the plan leaves the cell.
                out.push(Vertex { q, t: p.t + f * (r.t - p.t), side: if inside(&a) { Side::Cell } else { a.side } });
            }
        }
        poly = out;
    }
    poly
}

fn rounded(q: [f64; 2]) -> P {
    q.map(|v| libm::round(v) as i32)
}

/// A beam's foot and top corners at a plan vertex, and their places.
type Corner = ([u32; 2], [[f64; 3]; 2]);

/// The solids of a cell's guard rails and delineators in its storey mesh, each corner on the label that the block
/// finds at its point; a piece whose point finds none is left out. A beam is cut on the cell lines and stays open
/// there; a delineator stands whole in the cell or not at all.
pub(super) fn solids(b: &mut Build, edges: &Edges, words: &[u32]) {
    let c = CELL_UNITS as f64;
    let metres = |q: [f64; 2], z: f64, mpu: f64| [q[0] * mpu, -q[1] * mpu, z];
    let sunk = |share: f64| -SINK_M * (1.0 - share);
    let at = |r: &Ring, d: f64, a: f64| add(add(r.edge, r.out, -d / r.mpu), r.along, a / r.mpu);
    // Two runs that end at one ring (two ways through a junction) join there with no end face.
    let mut ends: Vec<P> =
        edges.rails.iter().flat_map(|r| [rounded(r[0].edge), rounded(r[r.len() - 1].edge)]).collect();
    ends.sort_unstable();
    let shared =
        |r: &Ring| ends.partition_point(|&p| p < rounded(r.edge)) + 1 < ends.partition_point(|&p| p <= rounded(r.edge));
    for run in &edges.rails {
        for (k, w) in run.windows(2).enumerate() {
            let (ra, rb) = (&w[0], &w[1]);
            let [f, bk] = BEAM_IN_M;
            let plan = clip_square(vec![
                Vertex { q: at(ra, f, 0.0), t: 0.0, side: Side::Front },
                Vertex { q: at(rb, f, 0.0), t: 1.0, side: Side::Second },
                Vertex { q: at(rb, bk, 0.0), t: 1.0, side: Side::Back },
                Vertex { q: at(ra, bk, 0.0), t: 0.0, side: Side::First },
            ]);
            // Per vertex its foot and top corners and places.
            let corners: Option<Vec<Corner>> = (plan.iter())
                .map(|v| {
                    let (p, s) = (rounded(v.q), sunk(ra.share + v.t * (rb.share - ra.share)));
                    let label = find(words, p.map(f64::from))?;
                    Some((
                        BEAM_UP_M.map(|up| b.corner(p, label, up + s)),
                        BEAM_UP_M.map(|up| metres(v.q, up + s, ra.mpu)),
                    ))
                })
                .collect();
            let Some(cs) = corners.filter(|cs| cs.len() >= 3) else { continue };
            let n = cs.len();
            for i in 1..n - 1 {
                for (h, up) in [(1, 1.0), (0, -1.0)] {
                    let tri = [0, i, i + 1];
                    facing(b, RAIL, &tri.map(|v| cs[v].0[h]), &tri.map(|v| cs[v].1[h]), [0.0, 0.0, up]);
                }
            }
            let (o, t) = ([ra.out[0], -ra.out[1], 0.0], [ra.along[0], -ra.along[1], 0.0]);
            for i in 0..n {
                let out = match plan[i].side {
                    Side::Front => o.map(|v| -v),
                    Side::Back => o,
                    Side::First if k == 0 && !shared(ra) => t.map(|v| -v),
                    Side::Second if k + 2 == run.len() && !shared(rb) => t,
                    _ => continue,
                };
                let (a, z) = (&cs[i], &cs[(i + 1) % n]);
                facing(b, RAIL, &[a.0[0], a.0[1], z.0[1], z.0[0]], &[a.1[0], a.1[1], z.1[1], z.1[0]], out);
            }
        }
        for r in run.iter().filter(|r| r.post) {
            let mid = at(r, (POST_IN_M[0] + POST_IN_M[1]) / 2.0, 0.0);
            let o = rounded(mid);
            let h = POST_HALF_M;
            let corner = |d: f64, a: f64| sub(at(r, d, a), o.map(f64::from));
            let ring = clip(
                o,
                &[corner(POST_IN_M[0], -h), corner(POST_IN_M[1], -h), corner(POST_IN_M[1], h), corner(POST_IN_M[0], h)],
            );
            let inside = mid[0] >= 0.0 && mid[1] >= 0.0 && mid[0] <= c && mid[1] <= c;
            let Some(label) = ring.first().and_then(|&p| find(words, (if inside { o } else { p }).map(f64::from)))
            else {
                continue;
            };
            let s = sunk(r.share);
            prism(b, RAIL, &ring, (label, POST_UP_M[1] + s), (label, POST_UP_M[0] + s));
        }
    }
    for r in &edges.delineators {
        let mid = at(r, DELI_IN_M, 0.0);
        let [hd, hw] = DELI_HALF_M;
        let pts = [[-hd, -hw], [hd, -hw], [hd, hw], [-hd, hw]].map(|[d, a]| at(r, DELI_IN_M - d, a));
        let Some(label) = find(words, rounded(mid).map(f64::from)) else { continue };
        if !pts.iter().all(|&q| q[0] > 0.0 && q[1] > 0.0 && q[0] < c && q[1] < c) {
            continue;
        }
        let ring = pts.map(rounded);
        let z = DELI_UP_M;
        for (part, lo, hi) in [(DELINEATOR, z[0], z[1]), (BAND, z[1], z[2]), (DELINEATOR, z[2], z[3])] {
            prism(b, part, &ring, (label, hi), (label, lo));
        }
        // The reflector on the band's face toward the road.
        let [rw, r0, r1, proud] = REFLECTOR_M;
        let q = [at(r, DELI_IN_M + hd + proud, -rw), at(r, DELI_IN_M + hd + proud, rw)];
        let quad = [(q[0], r0), (q[1], r0), (q[1], r1), (q[0], r1)];
        let cs = quad.map(|(p, z)| b.corner(rounded(p), label, z));
        facing(b, REFLECTOR, &cs, &quad.map(|(p, z)| metres(p, z, r.mpu)), [-r.out[0], r.out[1], 0.0]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rings every 2 m over `len` metres, with need where `need(m)` holds and stops where `stop(m)` holds.
    fn chain(
        len: f64,
        need: impl Fn(f64) -> bool,
        stop: impl Fn(f64) -> bool,
        flush: [bool; 2],
    ) -> (Vec<f64>, Vec<f64>) {
        let m: Vec<f64> = (0..=(len / 2.0) as usize).map(|i| 2.0 * i as f64).collect();
        let (need, stops): (Vec<bool>, Vec<bool>) = m.iter().map(|&x| (need(x), stop(x))).unzip();
        let s = shares(&m, &need, &stops, flush);
        (m, s)
    }

    fn at(m: &[f64], s: &[f64], x: f64) -> f64 {
        s[m.iter().position(|&v| v == x).unwrap()]
    }

    #[test]
    fn a_need_stands_full_with_its_lead_then_ramps_down() {
        let (m, s) = chain(200.0, |x| (80.0..=120.0).contains(&x), |_| false, [false; 2]);
        assert_eq!(at(&m, &s, 100.0), 1.0);
        assert_eq!(at(&m, &s, 124.0), 1.0, "the lead");
        assert!(at(&m, &s, 128.0) > 0.0 && at(&m, &s, 128.0) < 1.0, "the ramp");
        assert_eq!(at(&m, &s, 130.0), 0.0);
        assert_eq!(at(&m, &s, 70.0), 0.0);
    }

    #[test]
    fn a_short_gap_joins_and_a_long_one_breaks() {
        let (m, s) =
            chain(300.0, |x| (50.0..=80.0).contains(&x) || (100.0..=130.0).contains(&x), |_| false, [false; 2]);
        assert_eq!(at(&m, &s, 90.0), 1.0, "a gap of 20 m closes");
        let (m, s) =
            chain(300.0, |x| (50.0..=80.0).contains(&x) || (120.0..=150.0).contains(&x), |_| false, [false; 2]);
        assert_eq!(at(&m, &s, 100.0), 0.0, "a gap of 40 m keeps its break");
    }

    #[test]
    fn a_junction_stop_sinks_the_rail_before_it() {
        let (m, s) = chain(200.0, |x| (40.0..=160.0).contains(&x), |x| (96.0..=104.0).contains(&x), [false; 2]);
        assert_eq!(at(&m, &s, 100.0), 0.0);
        assert_eq!(at(&m, &s, 96.0), 0.0);
        assert!(at(&m, &s, 94.0) < 1.0 && at(&m, &s, 94.0) > 0.0, "it ramps over ZONE_RAMP_M");
        assert_eq!(at(&m, &s, 90.0), 1.0);
    }

    #[test]
    fn a_rail_runs_on_to_a_deck_end_and_none_starts_at_one() {
        let (_, s) = chain(2.0, |_| false, |_| false, [true, true]);
        assert_eq!(s, [0.0, 0.0], "a stub between two level changes");
        let (_, s) = chain(100.0, |x| x <= 80.0, |_| false, [false, true]);
        assert!(s[s.len() - 4..].iter().all(|&v| v == 1.0), "full to the end line, 20 m past the last need");
        let (_, s) = chain(100.0, |x| x <= 40.0, |_| false, [false, true]);
        assert_eq!(s[s.len() - 1], 0.0, "60 m past the last need, the deck end starts no rail");
        let (_, s) = chain(100.0, |x| x <= 80.0, |_| false, [false; 2]);
        assert_eq!(s[s.len() - 1], 0.0, "an open end sinks as before");
    }

    #[test]
    fn a_cell_line_cuts_a_beam_exactly_on_it_and_alike_from_both_sides() {
        let c = CELL_UNITS as f64;
        let v = |q: [f64; 2], t: f64, side: Side| Vertex { q, t, side };
        let quad = |x0: f64, x1: f64| {
            vec![
                v([x0, 100.0], 0.0, Side::Front),
                v([x1, 117.0], 1.0, Side::Second),
                v([x1, 130.0], 1.0, Side::Back),
                v([x0, 113.0], 0.0, Side::First),
            ]
        };
        let cut = clip_square(quad(c - 300.0, c + 500.0));
        let on_line: Vec<Vertex> = cut.iter().copied().filter(|p| p.q[0] == c).collect();
        assert_eq!(on_line.len(), 2);
        assert!(on_line.iter().all(|p| (p.t - 300.0 / 800.0).abs() < 1e-12));
        assert_eq!(cut.iter().filter(|p| p.side == Side::Cell).count(), 1, "the cell line side stays open");
        // The neighbour holds the same beam 1 cell west: its cut points lie on its own x = 0.
        let shifted = clip_square(quad(-300.0, 500.0));
        let theirs: Vec<f64> = shifted.iter().filter(|p| p.q[0] == 0.0).map(|p| p.q[1]).collect();
        let ours: Vec<f64> = on_line.iter().map(|p| p.q[1]).collect();
        assert_eq!(theirs, ours);
    }
}
