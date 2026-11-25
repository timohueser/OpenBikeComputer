//! Storey 0's guard rails and delineator posts: where they stand (`place`, in the cell build) and their solids in the
//! storey mesh (`solids`). Both stand on their way's own shoulder, so each corner takes the label of the face under it
//! and the drawn road's height, and a post or a delineator starts on paved ground and goes down into the fill.
//!
//! A chain is the ground road ways that joints link. Its rings are the way's graph samples, which hold the sag rules
//! in plan and height, and a post point at each `POST_M` of stroke metres. Per side, a ring needs a rail where the
//! paved edge lies `DROP_M` over the plain terrain `DROP_OUT_M` out, or where a mapped guard rail lies near; the
//! stretch rules of `roads/standing.ts` give each ring its share. A rail never stands where its ring leaves its own
//! shoulder or carriageway (a junction, a sidewalk, another way, a crossing), on a side with a sidewalk, in a town or
//! where its line folds: it dives into the ground before such a stop. At a level change (a deck's end, a tunnel's
//! opening) it runs on and ends square. Every rule reads the graph, the window's face plan, the plain terrain and the
//! features, which two cells hold alike about their shared line, so both give a ring the same share.

use super::shell::{clip, prism, Build, BAND, DELINEATOR, P, RAIL, REFLECTOR};
use crate::faces::records::find;
use crate::faces::{FacePlan, Owner, Surface, OUTSIDE};
use crate::frame::{metres_per_unit_at, GlobalPoint, CELL_UNITS};
use crate::graph::{point as curve_point, u_at, CrossingKind, Family, Level, Network, WayGraph};
use crate::sections::Sections;
use crate::tile::Geometry;
use crate::CellInput;

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
/// A rail ramps down over this before a stop.
const ZONE_RAMP_M: f64 = 5.0;
/// A town: this many buildings within this far. A shorter town than `BREAK_M` stops nothing.
const TOWN_BUILDINGS: usize = 3;
const TOWN_M: f64 = 40.0;
/// A mapped guard rail counts within this of the shoulder's outer edge.
const MAPPED_M: f64 = 2.5;
/// A post every this many stroke metres; a ring at each, so a straight beam runs this far between rings.
const POST_M: f64 = 4.0;
/// A delineator every this many stroke metres, and every `BEND_STEP_M` where the line turns `BEND_RAD` within
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
/// A post behind the beam: in from the edge, half its width along, its top and its foot.
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
/// The face test finds the nearest face edge within this; a ring farther from every edge fails it.
const FACE_REACH_M: f64 = 6.0;

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
    /// Rings of the cell left out because their beam or post left their own paved faces.
    pub left_out: u32,
}

/// How a chain ends: at a level change (the rail runs on), where nothing may stand, or past the graph.
#[derive(Clone, Copy, PartialEq)]
enum End {
    Flush,
    Stop,
    Open,
}

/// Items in square buckets of frame units, by their boxes.
struct Grid {
    size: f64,
    cells: std::collections::BTreeMap<(i64, i64), Vec<u32>>,
}

impl Grid {
    fn new(size: f64) -> Grid {
        Grid { size, cells: Default::default() }
    }

    fn key(&self, v: f64) -> i64 {
        libm::floor(v / self.size) as i64
    }

    fn insert(&mut self, [x0, y0, x1, y1]: [f64; 4], id: u32) {
        for y in self.key(y0)..=self.key(y1) {
            for x in self.key(x0)..=self.key(x1) {
                self.cells.entry((x, y)).or_default().push(id);
            }
        }
    }

    /// The items whose buckets meet the square of half side r about q, each once, in order.
    fn near(&self, q: [f64; 2], r: f64) -> Vec<u32> {
        let mut out = Vec::new();
        for y in self.key(q[1] - r)..=self.key(q[1] + r) {
            for x in self.key(q[0] - r)..=self.key(q[0] + r) {
                out.extend(self.cells.get(&(x, y)).into_iter().flatten());
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

/// The distance from q to the segment a → b and the foot's share of it.
fn to_segment(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let d = sub(b, a);
    let l2 = dot(d, d);
    let t = if l2 > 0.0 { (dot(sub(q, a), d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let f = add(a, d, t);
    (libm::hypot(q[0] - f[0], q[1] - f[1]), t)
}

/// What the rules read about a cell: the face plan's edges, the buildings' boxes and the mapped guard rails.
struct Context<'a> {
    plan: &'a FacePlan,
    edges: Grid,
    buildings: Grid,
    boxes: Vec<[f64; 4]>,
    rails: Grid,
    segments: Vec<[[f64; 2]; 2]>,
    input: &'a CellInput,
}

impl Context<'_> {
    /// Whether q lies in a shoulder or in way `way`'s own carriageway: the face on q's side of the nearest face edge.
    /// Whether q leaves the shoulders and way `way`'s own carriageway; none where the face plan does not reach q.
    fn leaves(&self, q: [f64; 2], way: u32, mpu: f64) -> bool {
        self.face_at(q, mpu).is_some_and(|(surface, owner)| {
            surface != Surface::Shoulder && (surface != Surface::Carriageway || owner != Owner::Way(way))
        })
    }

    /// The face that holds q: the one on q's side of the nearest face edge within `FACE_REACH_M`.
    fn face_at(&self, q: [f64; 2], mpu: f64) -> Option<(Surface, Owner)> {
        let reach = FACE_REACH_M / mpu;
        let v = |k: u32| self.plan.vertices[k as usize].map(f64::from);
        let mut best: Option<(f64, f64, u32)> = None;
        for e in self.edges.near(q, reach) {
            let edge = &self.plan.edges[e as usize];
            let (a, b) = (v(edge.a), v(edge.b));
            let (d, _) = to_segment(q, a, b);
            let ab = sub(b, a);
            let len = libm::hypot(ab[0], ab[1]);
            let cross = ab[0] * (q[1] - a[1]) - ab[1] * (q[0] - a[0]);
            let across = if len > 0.0 { cross.abs() / len } else { 0.0 };
            if d <= reach && best.is_none_or(|(bd, ba, _)| d < bd - 1e-9 || (d <= bd + 1e-9 && across > ba)) {
                best = Some((d, across, if cross > 0.0 { edge.left } else { edge.right }));
            }
        }
        let f = best?.2;
        (f != OUTSIDE).then(|| (self.plan.faces[f as usize].surface, self.plan.faces[f as usize].owner))
    }

    fn town(&self, q: [f64; 2], mpu: f64) -> bool {
        let r = TOWN_M / mpu;
        let count = (self.buildings.near(q, r).into_iter())
            .filter(|&k| {
                let [x0, y0, x1, y1] = self.boxes[k as usize];
                libm::hypot((x0 - q[0]).max(q[0] - x1).max(0.0), (y0 - q[1]).max(q[1] - y1).max(0.0)) <= r
            })
            .take(TOWN_BUILDINGS)
            .count();
        count >= TOWN_BUILDINGS
    }

    fn mapped(&self, q: [f64; 2], mpu: f64) -> bool {
        let r = MAPPED_M / mpu;
        (self.rails.near(q, r).into_iter())
            .any(|k| to_segment(q, self.segments[k as usize][0], self.segments[k as usize][1]).0 <= r)
    }

    fn terrain(&self, q: [f64; 2]) -> Option<f64> {
        let o = self.input.key.origin();
        self.input.terrain.height(GlobalPoint { x: o.x + libm::round(q[0]) as i64, y: o.y + libm::round(q[1]) as i64 })
    }
}

fn context<'a>(plan: &'a FacePlan, input: &'a CellInput) -> Context<'a> {
    let bucket = 8192.0;
    let mut edges = Grid::new(bucket);
    for (k, e) in plan.edges.iter().enumerate() {
        let (a, b) = (plan.vertices[e.a as usize].map(f64::from), plan.vertices[e.b as usize].map(f64::from));
        edges.insert([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])], k as u32);
    }
    let o = input.key.origin();
    let local = |p: &GlobalPoint| [(p.x - o.x) as f64, (p.y - o.y) as f64];
    let (mut buildings, mut boxes) = (Grid::new(4.0 * bucket), Vec::new());
    let (mut rails, mut segments) = (Grid::new(bucket), Vec::new());
    for f in &input.features {
        for part in &f.parts {
            if f.kind == "building" && f.geometry == Geometry::Area && part.outer {
                let pts: Vec<[f64; 2]> = part.points.iter().map(local).collect();
                let lo = |i: usize| pts.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min);
                let hi = |i: usize| pts.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max);
                let bx = [lo(0), lo(1), hi(0), hi(1)];
                buildings.insert(bx, boxes.len() as u32);
                boxes.push(bx);
            }
            if f.kind == "guard-rail" && f.geometry == Geometry::Line {
                for w in part.points.windows(2) {
                    let (a, b) = (local(&w[0]), local(&w[1]));
                    rails.insert(
                        [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])],
                        segments.len() as u32,
                    );
                    segments.push([a, b]);
                }
            }
        }
    }
    Context { plan, edges, buildings, boxes, rails, segments, input }
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
    let sign = if part.backward { -1.0 } else { 1.0 };
    let o = g.cell.origin();
    let mut pts: Vec<Point> = Vec::new();
    let of = |s: &crate::graph::Sample, post: bool| Point {
        pos: s.pos.map(f64::from),
        t: s.tangent,
        z: s.z_m,
        bank: s.bank,
        widen: s.widen_m,
        walk: s.walk,
        along: s.along_m,
        post,
        way: w,
        dir: f64::from(d),
    };
    for (i, s) in samples.iter().enumerate() {
        pts.push(of(s, false));
        let Some(n) = samples.get(i + 1) else { break };
        let (lo, hi) = (s.along_m.min(n.along_m), s.along_m.max(n.along_m));
        let span = &part.spans[s.span as usize];
        let mut posts: Vec<Point> = Vec::new();
        let mut k = libm::floor(lo / POST_M) as i64 + 1;
        while (k as f64) * POST_M < hi {
            let along = k as f64 * POST_M;
            let u = u_at(span, (along - span.along0_m) * sign);
            let p = curve_point(part, s.span as usize, u);
            let v = part.vertices[s.span as usize].pos;
            let pos = [(v[0] - o.x) as f64 + p.d[0], (v[1] - o.y) as f64 + p.d[1]];
            let walk = s.walk;
            posts.push(Point {
                pos,
                t: p.tangent,
                z: p.z_m,
                bank: p.bank,
                widen: p.widen_m,
                walk,
                along,
                post: true,
                ..of(s, true)
            });
            k += 1;
        }
        if s.along_m > n.along_m {
            posts.reverse();
        }
        pts.extend(posts);
    }
    pts.push(of(&samples[samples.len() - 1], false));
    pts.dedup_by(|b, a| a.along == b.along);
    let near_post = |i: usize| {
        !pts[i].post
            && [i.checked_sub(1), Some(i + 1)]
                .into_iter()
                .flatten()
                .any(|j| pts.get(j).is_some_and(|q| q.post && (q.along - pts[i].along).abs() < 1.0))
    };
    let keep: Vec<bool> = (0..pts.len()).map(|i| i == 0 || i + 1 == pts.len() || !near_post(i)).collect();
    let mut kept: Vec<Point> = pts.into_iter().zip(keep).filter(|x| x.1).map(|x| x.0).collect();
    if d < 0 {
        kept.reverse();
    }
    for p in &mut kept {
        p.t = [p.t[0] * f64::from(d), p.t[1] * f64::from(d)];
    }
    out.extend(kept);
}

/// Ground road ways through joints, each with its direction, and how the chain ends at its first and last way.
type Chain = (Vec<(u32, i8)>, [End; 2]);

/// The ground road ways in chains through the joints.
fn chains(g: &WayGraph) -> Vec<Chain> {
    use std::collections::BTreeMap;
    let ground = |w: u32| {
        let way = &g.ways[w as usize];
        way.family == Family::Road && way.level == Level::Ground && way.samples.len() >= 2
    };
    // At each way end (way, +1 its start, −1 its end): the way and the direction on, or how it ends.
    let mut next: BTreeMap<(u32, i8), Result<(u32, i8), End>> = BTreeMap::new();
    // An arm at its way's start points along it (+1), at its end back (−1); an arm inside a way ends nothing.
    let at_end = |a: &crate::graph::Arm| {
        let way = &g.ways[a.way as usize];
        if a.dir > 0 {
            a.at == way.from
        } else {
            a.at == way.to
        }
    };
    for j in &g.joints {
        for (a, b) in [(&j.arms[0], &j.arms[1]), (&j.arms[1], &j.arms[0])] {
            if !at_end(a) {
                continue;
            }
            let other = &g.ways[b.way as usize];
            let on = if ground(b.way) {
                Ok((b.way, b.dir))
            } else if other.family == Family::Road && other.level != Level::Ground {
                Err(End::Flush)
            } else {
                Err(End::Stop)
            };
            next.insert((a.way, a.dir), on);
        }
    }
    for arm in g.junctions.iter().flat_map(|j| &j.arms).chain(g.ends.iter().map(|e| &e.arm)).filter(|a| at_end(a)) {
        next.insert((arm.way, arm.dir), Err(End::Stop));
    }
    let mut seen = vec![false; g.ways.len()];
    let mut out = Vec::new();
    for w in 0..g.ways.len() as u32 {
        if seen[w as usize] || !ground(w) {
            continue;
        }
        seen[w as usize] = true;
        let mut chain = std::collections::VecDeque::from([(w, 1i8)]);
        let mut ends = [End::Open; 2];
        // Forward from the last way's exit, then back from the first way's entry.
        for back in [false, true] {
            loop {
                let &(x, d) = if back { chain.front().unwrap() } else { chain.back().unwrap() };
                let key = if back { (x, d) } else { (x, -d) };
                match next.get(&key).copied() {
                    Some(Ok((y, yd))) if !seen[y as usize] => {
                        seen[y as usize] = true;
                        if back {
                            chain.push_front((y, -yd));
                        } else {
                            chain.push_back((y, yd));
                        }
                    }
                    Some(Err(e)) => {
                        ends[usize::from(!back)] = e;
                        break;
                    }
                    _ => break,
                }
            }
        }
        out.push((chain.into_iter().collect(), ends));
    }
    out
}

/// The stretches where a detail must stand, from the rings that pass its test: a gap of `bridge` metres or less
/// closes, and no stretch runs over a stop (`stretches` of standing.ts).
fn stretches(m: &[f64], stops: &[bool], passes: &[bool], bridge: f64) -> Vec<bool> {
    let mut out = vec![false; m.len()];
    let (mut first, mut last) = (None, 0);
    for i in 0..m.len() {
        if stops[i] {
            if let Some(f) = first.take() {
                out[f..=last].fill(true);
            }
        } else if passes[i] {
            if let Some(f) = first {
                if m[i] - m[last] > bridge {
                    out[f..=last].fill(true);
                    first = None;
                }
            }
            first.get_or_insert(i);
            last = i;
        }
    }
    if let Some(f) = first {
        out[f..=last].fill(true);
    }
    out
}

/// Per ring the metres to the nearest marked ring that no stop cuts it off from.
fn metres_to(m: &[f64], stops: &[bool], marks: &[bool]) -> Vec<f64> {
    let n = m.len();
    let mut out = vec![f64::INFINITY; n];
    let mut from = f64::NEG_INFINITY;
    for i in 0..n {
        if stops[i] {
            from = f64::NEG_INFINITY;
        } else if marks[i] {
            from = m[i];
        }
        out[i] = m[i] - from;
    }
    let mut from = f64::INFINITY;
    for i in (0..n).rev() {
        if stops[i] {
            from = f64::INFINITY;
        } else if marks[i] {
            from = m[i];
        }
        out[i] = out[i].min(from - m[i]);
    }
    out
}

/// Per ring the metres to the nearest marked ring, zero on one.
fn zones(m: &[f64], marks: &[bool]) -> Vec<f64> {
    let none = vec![false; m.len()];
    metres_to(m, &none, marks)
}

/// Takes away a stretch of `marks` shorter than `least` metres that lies inside the chain.
fn without_stubs(shares: &mut [f64], m: &[f64], least: f64) {
    let mut from = None;
    for j in 0..=shares.len() {
        let stands = j < shares.len() && shares[j] > 0.0;
        match (stands, from) {
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

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The guard rails and delineators about a cell: see the module.
pub fn place(g: &WayGraph, plan: &FacePlan, input: &CellInput, sections: &Sections) -> Edges {
    let mut out = Edges::default();
    let origin_y = input.key.origin().y as f64;
    let info: Vec<Option<(bool, f64)>> = (g.ways.iter())
        .map(|way| {
            let feature = &input.features[g.parts[way.part as usize].feature.0 as usize];
            (way.family == Family::Road).then(|| {
                let s = sections.way(&feature.kind, &feature.tags, false);
                (RAILED.contains(&s.class), s.half_m + s.shoulder_m)
            })
        })
        .collect();
    let mut ctx: Option<Context> = None;
    let c = CELL_UNITS as f64;
    // Where nothing may stand: about each junction its widest way's paved half-width, its widest corner and `MOUTH_M`;
    // about each level crossing `MOUTH_M` past the rail bed.
    let section = |w: u32| {
        let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
        sections.way(&f.kind, &f.tags, false)
    };
    let mut mouths: Vec<([f64; 2], f64)> = Vec::new();
    for j in g.junctions.iter().filter(|j| j.network == Network::Way && j.arms.len() > 2) {
        let r = (j.arms.iter().map(|a| section(a.way)))
            .map(|s| s.half_m + s.widen_m + s.shoulder_m + s.corner_radius_m)
            .fold(0.0, f64::max);
        let p = j.pos.map(f64::from);
        mouths.push((p, (r + MOUTH_M) / metres_per_unit_at(origin_y + p[1])));
    }
    for x in g.crossings.iter().filter(|x| x.kind == CrossingKind::LevelCrossing) {
        let p = x.pos.map(f64::from);
        mouths.push((p, (RAIL_BED_M + MOUTH_M) / metres_per_unit_at(origin_y + p[1])));
    }
    let mut at_mouths = Grid::new(8192.0);
    for (k, &(q, r)) in mouths.iter().enumerate() {
        at_mouths.insert([q[0] - r, q[1] - r, q[0] + r, q[1] + r], k as u32);
    }
    // A ring within this of the cell square may reach into it.
    let near_cell = |q: [f64; 2], mpu: f64| {
        let r = 2.0 * (LEAD_M + RAMP_M + BREAK_M) / mpu;
        q[0] > -r && q[1] > -r && q[0] < c + r && q[1] < c + r
    };
    for (chain, ends) in chains(g) {
        let mut pts = Vec::new();
        for &(w, d) in &chain {
            way_points(g, w, d, &mut pts);
        }
        pts.dedup_by(|b, a| libm::hypot(a.pos[0] - b.pos[0], a.pos[1] - b.pos[1]) < 1.0);
        let n = pts.len();
        if n < 2 {
            continue;
        }
        let mpu: Vec<f64> = pts.iter().map(|p| metres_per_unit_at(origin_y + p.pos[1])).collect();
        let mut m = vec![0.0; n];
        for i in 1..n {
            m[i] =
                m[i - 1] + libm::hypot(pts[i].pos[0] - pts[i - 1].pos[0], pts[i].pos[1] - pts[i - 1].pos[1]) * mpu[i];
        }
        if !pts.iter().zip(&mpu).any(|(p, &u)| near_cell(p.pos, u)) {
            continue;
        }
        for side in [1.0, -1.0] {
            let rings: Vec<([f64; 2], [f64; 2], bool, f64)> = (pts.iter().zip(&mpu))
                .map(|(p, &u)| {
                    let (railed, e) = info[p.way as usize].unwrap_or((false, 0.0));
                    let e = e + p.widen;
                    // The normal on the chain's left (the frame's y points south), then this side's.
                    let n = [side * p.t[1], -side * p.t[0]];
                    (add(p.pos, n, e / u), n, railed, e)
                })
                .collect();
            let ctx = &*ctx.get_or_insert_with(|| context(plan, input));
            let mapped: Vec<bool> = rings.iter().zip(&mpu).map(|(r, &u)| ctx.mapped(r.0, u)).collect();
            let drops: Vec<bool> = (0..n)
                .map(|i| {
                    let (p, (edge, nrm, railed, e)) = (&pts[i], rings[i]);
                    // The bank lifts the way's left: this side is its left where it is the chain's left along the way.
                    let z = p.z + p.bank * side * p.dir * e;
                    let drop = ctx.terrain(add(edge, nrm, DROP_OUT_M / mpu[i])).map(|t| z - t);
                    railed && drop.is_some_and(|d| d > DROP_M)
                })
                .collect();
            // The town and face tests run only where a rail or a delineator may stand.
            let reach = 2.0 * (LEAD_M + RAMP_M) + BREAK_M + CLEAR_M;
            let may = zones(&m, &(0..n).map(|i| mapped[i] || drops[i]).collect::<Vec<_>>());
            let tested: Vec<bool> = (0..n)
                .map(|i| {
                    let grid = libm::fmod(pts[i].along.abs(), BEND_STEP_M);
                    let deli = rings[i].2 && grid.min(BEND_STEP_M - grid) <= CLEAR_M + 1.0;
                    may[i] <= reach || (deli && near_cell(rings[i].0, mpu[i]))
                })
                .collect();
            let town: Vec<bool> = (0..n).map(|i| tested[i] && rings[i].2 && ctx.town(rings[i].0, mpu[i])).collect();
            let need: Vec<bool> = (0..n).map(|i| mapped[i] || (drops[i] && !town[i])).collect();
            // A town shorter than BREAK_M stops nothing: a rail would dive for it and rise again.
            let mut towns: Vec<f64> = town.iter().map(|&t| f64::from(u8::from(t))).collect();
            without_stubs(&mut towns, &m, BREAK_M);
            let stops: Vec<bool> = (0..n)
                .map(|i| {
                    let p = &pts[i];
                    let bit = if side * p.dir > 0.0 { 1 } else { 2 };
                    let fold = |j: usize| dot(sub(rings[j + 1].0, rings[j].0), add(pts[j].t, pts[j + 1].t, 1.0)) <= 0.0;
                    let end = (i == 0 && ends[0] == End::Stop) || (i + 1 == n && ends[1] == End::Stop);
                    let mouth = (at_mouths.near(p.pos, 0.0).into_iter()).any(|k| {
                        let (q, r) = mouths[k as usize];
                        libm::hypot(q[0] - p.pos[0], q[1] - p.pos[1]) < r
                    });
                    end || mouth
                        || p.walk & bit != 0
                        || (towns[i] > 0.0 && !mapped[i])
                        || (i > 0 && fold(i - 1))
                        || (i + 1 < n && fold(i))
                })
                .collect();
            let marks = stretches(&m, &stops, &need, 2.0 * (LEAD_M + RAMP_M) + BREAK_M);
            let to_rail = metres_to(&m, &stops, &marks);
            let zone = zones(&m, &stops);
            let mut share: Vec<f64> =
                (0..n).map(|i| smooth(((LEAD_M + RAMP_M - to_rail[i]) / RAMP_M).min(zone[i] / ZONE_RAMP_M))).collect();
            without_stubs(&mut share, &m, 2.0 * RAMP_M);
            let ring = |i: usize| Ring {
                edge: rings[i].0,
                out: rings[i].1,
                along: pts[i].t,
                mpu: mpu[i],
                share: share[i],
                post: pts[i].post && share[i] > 0.0,
            };
            let guard = |k: usize| {
                let (p, (edge, nrm, _, _)) = (&pts[k], rings[k]);
                ![BEAM_IN_M[0], (POST_IN_M[0] + POST_IN_M[1]) / 2.0]
                    .iter()
                    .any(|&d| ctx.leaves(add(edge, nrm, -d / mpu[k]), p.way, mpu[k]))
            };
            // Runs: the rings with a share, and the sunk ring on each side of them; one sunk ring between two runs
            // joins them, so no two runs share a ring.
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
                let (a, b) = (i.saturating_sub(1), (j + 1).min(n - 1));
                if (a..=b).any(|k| near_cell(rings[k].0, mpu[k])) {
                    // A ring whose beam or post leaves its own paved faces splits the run. The face plan holds only
                    // the cell and a little more, so the rules above cannot read it, and two cells read it alike only
                    // where both hold it.
                    let mut piece: Vec<Ring> = Vec::new();
                    for k in a..=b {
                        if guard(k) {
                            piece.push(ring(k));
                            continue;
                        }
                        out.left_out += 1;
                        if piece.len() > 1 {
                            out.rails.push(std::mem::take(&mut piece));
                        }
                        piece.clear();
                    }
                    if piece.len() > 1 {
                        out.rails.push(piece);
                    }
                }
                i = j + 1;
            }
            // Delineators where no rail stands, clear of every stop.
            let turn = |i: usize| {
                let (lo, hi) = (
                    (0..=i).rev().find(|&k| m[i] - m[k] >= BEND_M).unwrap_or(0),
                    (i..n).find(|&k| m[k] - m[i] >= BEND_M).unwrap_or(n - 1),
                );
                libm::acos(dot(pts[lo].t, pts[hi].t).clamp(-1.0, 1.0)) > BEND_RAD
            };
            for i in 0..n {
                let p = &pts[i];
                let on = |step: f64| p.post && libm::fmod(p.along, step).abs() < 1e-6;
                let railed = info[p.way as usize].is_some_and(|x| x.0);
                let clear = zone[i] >= CLEAR_M
                    && [i.checked_sub(1), Some(i), Some(i + 1)]
                        .into_iter()
                        .flatten()
                        .all(|k| share.get(k).is_none_or(|&s| s <= 0.0));
                if railed
                    && !town[i]
                    && clear
                    && (on(STEP_M) || (on(BEND_STEP_M) && turn(i)))
                    && near_cell(rings[i].0, mpu[i])
                    && guard(i)
                {
                    out.delineators.push(ring(i));
                }
            }
        }
    }
    out
}

/// A triangle on corners `c` at the points `p` (as `face`), turned to face `out`.
fn triangle(b: &mut Build, part: usize, c: [u32; 3], p: [[f64; 3]; 3], out: [f64; 3]) {
    let (u, v) = ([0, 1, 2].map(|k| p[1][k] - p[0][k]), [0, 1, 2].map(|k| p[2][k] - p[0][k]));
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    let up = n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0.0;
    b.tri(part, if up { c } else { [c[0], c[2], c[1]] });
}

/// A quad on corners `c` at the points `p` (metres east, north and up, about any origin), turned to face `out`.
fn face(b: &mut Build, part: usize, c: [u32; 4], p: [[f64; 3]; 4], out: [f64; 3]) {
    let (u, v) = ([0, 1, 2].map(|k| p[1][k] - p[0][k]), [0, 1, 2].map(|k| p[2][k] - p[0][k]));
    let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    if n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0.0 {
        b.quad(part, c);
    } else {
        b.quad(part, [c[0], c[3], c[2], c[1]]);
    }
}

/// A point of a beam segment's plan: its place, its share of the way from the segment's first ring, and the side
/// that starts at it.
#[derive(Clone, Copy)]
struct Vertex {
    q: [f64; 2],
    t: f64,
    side: Side,
}

#[derive(Clone, Copy, PartialEq)]
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
                let t = p.t + f * (r.t - p.t);
                // The cut point starts the side that a went on; the cell line starts where the plan leaves the cell.
                out.push(Vertex { q, t, side: if inside(&a) { Side::Cell } else { a.side } });
            }
        }
        poly = out;
        if poly.is_empty() {
            break;
        }
    }
    poly
}

fn rounded(q: [f64; 2]) -> P {
    q.map(|v| libm::round(v) as i32)
}

/// The solids of a cell's guard rails and delineators in its storey mesh, each corner on the label that the block
/// finds at its point; a piece whose point finds none is left out. A beam is cut on the cell lines and stays open
/// there; a delineator stands whole in the cell or not at all.
pub(super) fn solids(b: &mut Build, edges: &Edges, words: &[u32]) {
    let c = CELL_UNITS as f64;
    let inside = |q: [f64; 2]| q[0] >= 0.0 && q[1] >= 0.0 && q[0] <= c && q[1] <= c;
    let metres = |q: [f64; 2], z: f64, mpu: f64| [q[0] * mpu, -q[1] * mpu, z];
    let sunk = |share: f64| -SINK_M * (1.0 - share);
    // Two runs that end at one ring (two ways through a junction) join there with no end face.
    let mut ends: Vec<P> =
        edges.rails.iter().flat_map(|r| [rounded(r[0].edge), rounded(r[r.len() - 1].edge)]).collect();
    ends.sort_unstable();
    let shared =
        |r: &Ring| ends.partition_point(|&p| p < rounded(r.edge)) + 1 < ends.partition_point(|&p| p <= rounded(r.edge));
    for run in &edges.rails {
        for (k, w) in run.windows(2).enumerate() {
            let (ra, rb) = (&w[0], &w[1]);
            let at = |r: &Ring, d: f64| add(r.edge, r.out, -d / r.mpu);
            let [f, bk] = BEAM_IN_M;
            let plan = clip_square(vec![
                Vertex { q: at(ra, f), t: 0.0, side: Side::Front },
                Vertex { q: at(rb, f), t: 1.0, side: Side::Second },
                Vertex { q: at(rb, bk), t: 1.0, side: Side::Back },
                Vertex { q: at(ra, bk), t: 0.0, side: Side::First },
            ]);
            if plan.len() < 3 {
                continue;
            }
            // Per vertex its foot and top corners and places.
            let mut cs: Vec<[u32; 2]> = Vec::new();
            let mut ps: Vec<[[f64; 3]; 2]> = Vec::new();
            for v in &plan {
                let p = rounded(v.q);
                let Some(label) = find(words, p.map(f64::from)) else { break };
                let s = sunk(ra.share + v.t * (rb.share - ra.share));
                cs.push(BEAM_UP_M.map(|up| b.corner(p, label, up + s)));
                ps.push(BEAM_UP_M.map(|up| metres(v.q, up + s, ra.mpu)));
            }
            if cs.len() < plan.len() {
                continue;
            }
            let n = plan.len();
            for i in 1..n - 1 {
                for (h, up) in [(1, 1.0), (0, -1.0)] {
                    let (c3, p3) = ([cs[0][h], cs[i][h], cs[i + 1][h]], [ps[0][h], ps[i][h], ps[i + 1][h]]);
                    triangle(b, RAIL, c3, p3, [0.0, 0.0, up]);
                }
            }
            let o = [ra.out[0], -ra.out[1], 0.0];
            let t = [ra.along[0], -ra.along[1], 0.0];
            for i in 0..n {
                let j = (i + 1) % n;
                let out = match plan[i].side {
                    Side::Front => o.map(|v| -v),
                    Side::Back => o,
                    Side::First if k == 0 && !shared(ra) => t.map(|v| -v),
                    Side::Second if k + 2 == run.len() && !shared(rb) => t,
                    _ => continue,
                };
                face(b, RAIL, [cs[i][0], cs[i][1], cs[j][1], cs[j][0]], [ps[i][0], ps[i][1], ps[j][1], ps[j][0]], out);
            }
        }
        for r in run.iter().filter(|r| r.post) {
            let mid = add(r.edge, r.out, -(POST_IN_M[0] + POST_IN_M[1]) / 2.0 / r.mpu);
            let o = rounded(mid);
            let corner = |d: f64, a: f64| {
                let q = add(add(r.edge, r.out, -d / r.mpu), r.along, a / r.mpu);
                [q[0] - f64::from(o[0]), q[1] - f64::from(o[1])]
            };
            let h = POST_HALF_M;
            let ring = clip(
                o,
                &[corner(POST_IN_M[0], -h), corner(POST_IN_M[1], -h), corner(POST_IN_M[1], h), corner(POST_IN_M[0], h)],
            );
            let Some(label) = ring.first().and_then(|&p| find(words, (if inside(mid) { o } else { p }).map(f64::from)))
            else {
                continue;
            };
            let s = sunk(r.share);
            prism(b, RAIL, &ring, (label, POST_UP_M[1] + s), (label, POST_UP_M[0] + s));
        }
    }
    for r in &edges.delineators {
        let mid = add(r.edge, r.out, -DELI_IN_M / r.mpu);
        let o = rounded(mid);
        let [hd, hw] = DELI_HALF_M;
        let at = |d: f64, a: f64| add(add(mid, r.out, d / r.mpu), r.along, a / r.mpu);
        let pts = [at(-hd, -hw), at(hd, -hw), at(hd, hw), at(-hd, hw)];
        if !pts.iter().all(|&q| q[0] > 0.0 && q[1] > 0.0 && q[0] < c && q[1] < c) {
            continue;
        }
        let Some(label) = find(words, o.map(f64::from)) else { continue };
        let ring: Vec<P> = pts.iter().map(|&q| rounded(q)).collect();
        let z = DELI_UP_M;
        for (part, lo, hi) in [(DELINEATOR, z[0], z[1]), (BAND, z[1], z[2]), (DELINEATOR, z[2], z[3])] {
            prism(b, part, &ring, (label, hi), (label, lo));
        }
        // The reflector on the band's face toward the road.
        let [rw, r0, r1, proud] = REFLECTOR_M;
        let q = [at(-hd - proud, -rw), at(-hd - proud, rw)];
        let cs = [
            b.corner(rounded(q[0]), label, r0),
            b.corner(rounded(q[1]), label, r0),
            b.corner(rounded(q[1]), label, r1),
            b.corner(rounded(q[0]), label, r1),
        ];
        let ps = [metres(q[0], r0, r.mpu), metres(q[1], r0, r.mpu), metres(q[1], r1, r.mpu), metres(q[0], r1, r.mpu)];
        face(b, REFLECTOR, cs, ps, [-r.out[0], r.out[1], 0.0]);
    }
}
