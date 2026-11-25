//! Storey 0's guard rails and delineator posts: where they stand (`place`, in the cell build) and their solids in the
//! storey mesh (`solids`). Both stand on their way's own shoulder, so each corner takes the label of the face under it
//! and the drawn road's height, and a post or a delineator starts on paved ground and goes down into the fill.
//!
//! A chain is the ground road ways that joints link. Its rings are the graph samples and a post point at each `POST_M`
//! of stroke metres. Per side, a ring needs a rail where the paved edge lies `DROP_M` over the plain terrain
//! `DROP_OUT_M` out, or where a mapped guard rail lies near; `shares` gives each ring its share by the stretch rules.
//! A rail sinks into the ground before a stop: a chain's end at a junction or past the graph, the zone of a junction
//! on its own arms, a sidewalk, a town or a fold. At a level change (a deck's end, a tunnel's opening) it runs on and
//! ends square. The rules read only the graph, the plain terrain and the features, and no farther past the cell than
//! `reach` states, so two cells give a ring the same share.

use super::shell::{clip, prism, solid, Build, DELINEATOR, P, RAIL};
use crate::faces::records::find;
use crate::frame::{metres_per_unit_at, GlobalPoint, CELL_UNITS};
use crate::graph::{point as curve_point, u_at, Arm, CrossingKind, Family, Level, Network, PointRef, Sample, WayGraph};
use crate::sections::{Reach, Sections, HALF_MAX_M};
use crate::tile::Geometry;
use crate::CellInput;
use std::collections::BTreeMap;

/// The classes with guard rails and delineators outside towns, and their links.
const RAILED: [&str; 5] = ["trunk", "primary", "secondary", "tertiary", "unclassified"];
/// A rail stands where the edge lies this much over the plain terrain this far out.
const DROP_M: f64 = 1.35;
const DROP_OUT_M: f64 = 4.0;
/// A rail runs on this far past the last ring that needs it, then ramps into the ground over `RAMP_M`, as a terminal
/// does; it ramps down over `RAMP_M` before a stop too.
const LEAD_M: f64 = 4.0;
const RAMP_M: f64 = 6.0;
/// Two stretches stay this far apart past their ramps, else they join: a narrower break reads as a notch.
const BREAK_M: f64 = 8.0;
const JOIN_M: f64 = 2.0 * (LEAD_M + RAMP_M) + BREAK_M;
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
/// The beam, a box: its front and back in from the shoulder's outer edge, its foot and top over the road.
const BEAM_IN_M: [f64; 2] = [0.22, 0.13];
const BEAM_UP_M: [f64; 2] = [0.44, 0.75];
/// A beam that does not stand has sunk this far: its top 5 cm under the shoulder.
const SINK_M: f64 = 0.8;
/// A post behind the beam: its middle in from the edge, half its depth and width, its foot and top. Its back keeps
/// 5 cm off the edge, more than the most that the face plan's chord of the edge leaves the curve.
const POST_IN_M: f64 = 0.09;
const POST_HALF_M: [f64; 2] = [0.04, 0.04];
const POST_UP_M: [f64; 2] = [-1.0, 0.71];
/// A delineator: its middle in from the edge, half its depth and width, and its foot, black band and top. The piece
/// shader draws a triangle between the band's two offsets black (`plan/piece.wgsl`).
const DELI_IN_M: f64 = 0.10;
const DELI_HALF_M: [f64; 2] = [0.05, 0.06];
const DELI_UP_M: [f64; 4] = [-0.5, 0.74, 0.9, 1.0];
/// Nothing stands within this past a junction's widest way and corner, or past a level crossing's widest way.
const MOUTH_M: f64 = 5.0;
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
#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    Flush,
    Stop,
    Open,
}

/// The row of the reach table: how far past the cell a ring's share reads along its chain (the next ring, a stub, the
/// lead and ramp, a gap that joins, and a level change's need past that gap), then about a ring of that need (its
/// way's half-width, then the town test).
pub fn reach() -> Reach {
    let terms = [
        ("POST_M", POST_M),
        ("2·RAMP_M", 2.0 * RAMP_M),
        ("LEAD_M + RAMP_M", LEAD_M + RAMP_M),
        ("JOIN_M", JOIN_M),
        ("a level change's JOIN_M", JOIN_M),
        ("HALF_MAX_M", HALF_MAX_M),
        ("TOWN_M", TOWN_M),
    ];
    let terms = terms.iter().map(|&(what, m)| (what.to_string(), m)).collect();
    Reach { name: "edges shares".into(), owner: "edges", chain: true, terms }
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

    /// Whether `f` holds of an item whose bucket meets the square of half side r about q. An item in several buckets
    /// comes once for each.
    fn any(&self, q: [f64; 2], r: f64, mut f: impl FnMut(u32) -> bool) -> bool {
        for y in self.key(q[1] - r)..=self.key(q[1] + r) {
            for x in self.key(q[0] - r)..=self.key(q[0] + r) {
                if self.0.get(&(x, y)).is_some_and(|ids| ids.iter().any(|&k| f(k))) {
                    return true;
                }
            }
        }
        false
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

/// The distance from q to the segment a → b.
fn to_segment(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = sub(b, a);
    let l2 = dot(d, d);
    let t = if l2 > 0.0 { (dot(sub(q, a), d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    dist(q, add(a, d, t))
}

/// What the rules read of the features: the buildings' boxes and the mapped guard rails.
struct Context<'a> {
    buildings: (Grid, Vec<[f64; 4]>),
    rails: (Grid, Vec<[[f64; 2]; 2]>),
    input: &'a CellInput,
}

impl<'a> Context<'a> {
    fn new(input: &'a CellInput) -> Context<'a> {
        let o = input.key.origin();
        let local = |p: &GlobalPoint| [(p.x - o.x) as f64, (p.y - o.y) as f64];
        let bbox = |pts: &[[f64; 2]]| {
            let (lo, hi) = (f64::NEG_INFINITY, f64::INFINITY);
            pts.iter().fold([hi, hi, lo, lo], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])])
        };
        let (mut boxes, mut segments) = (Vec::new(), Vec::new());
        for f in &input.features {
            for part in &f.parts {
                let pts: Vec<[f64; 2]> = part.points.iter().map(local).collect();
                if f.kind == "building" && f.geometry == Geometry::Area && part.outer {
                    boxes.push(bbox(&pts));
                }
                if f.kind == "guard-rail" && f.geometry == Geometry::Line {
                    segments.extend(pts.windows(2).map(|w| [w[0], w[1]]));
                }
            }
        }
        let buildings = (Grid::of(4.0 * BUCKET, &boxes), boxes);
        let rails = (Grid::of(BUCKET, &segments.iter().map(|s| bbox(s)).collect::<Vec<_>>()), segments);
        Context { buildings, rails, input }
    }

    fn town(&self, q: [f64; 2], mpu: f64) -> bool {
        let (r, (grid, boxes)) = (TOWN_M / mpu, &self.buildings);
        let (mut found, mut n) = ([u32::MAX; TOWN_BUILDINGS], 0);
        grid.any(q, r, |k| {
            let [x0, y0, x1, y1] = boxes[k as usize];
            let d = libm::hypot((x0 - q[0]).max(q[0] - x1).max(0.0), (y0 - q[1]).max(q[1] - y1).max(0.0));
            if d <= r && !found.contains(&k) {
                (found[n], n) = (k, n + 1);
            }
            n == TOWN_BUILDINGS
        })
    }

    fn mapped(&self, q: [f64; 2], mpu: f64) -> bool {
        let (r, (grid, segments)) = (MAPPED_M / mpu, &self.rails);
        grid.any(q, r, |k| to_segment(q, segments[k as usize][0], segments[k as usize][1]) <= r)
    }

    fn terrain(&self, q: [f64; 2]) -> Option<f64> {
        let o = self.input.key.origin();
        self.input.terrain.height(GlobalPoint { x: o.x + libm::round(q[0]) as i64, y: o.y + libm::round(q[1]) as i64 })
    }
}

/// A point of a chain: its place (frame units), its unit tangent in the chain's direction, its height, bank and
/// widening, the sides of its way with a sidewalk (1 its left, 2 its right), its stroke metres, whether it is a post
/// point, its way, the way's direction in the chain and its span.
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
    span: u32,
}

/// The points of way w in direction d: its graph samples and a post point at each `POST_M` of stroke metres
/// between them; a sample within a metre of a post point gives way to it. The graph leaves out the spans that do not
/// meet its area, and posts stand only between the samples of one span or of two spans that meet.
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
        span: s.span,
    };
    let mut pts: Vec<Point> = Vec::new();
    for (s, n) in samples.iter().zip(samples.iter().skip(1).map(Some).chain([None])) {
        pts.push(of(s));
        let Some(n) = n.filter(|n| n.span <= s.span + 1) else { continue };
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

/// Whether the way is a ground road with samples.
fn ground(g: &WayGraph, w: u32) -> bool {
    let way = &g.ways[w as usize];
    way.family == Family::Road && way.level == Level::Ground && way.samples.len() >= 2
}

/// Whether the arm ends its way: at its way's start it points along it (+1), at its end back (−1).
fn at_end(g: &WayGraph, a: &Arm) -> bool {
    a.at == if a.dir > 0 { g.ways[a.way as usize].from } else { g.ways[a.way as usize].to }
}

/// At a way's end (way, +1 its start, −1 its end): the way and the direction on, or how the chain ends.
type Next = Result<(u32, i8), End>;
/// Ground road ways through joints, each with its direction, and how the chain ends at its first and last way.
type Chain = (Vec<(u32, i8)>, [End; 2]);

/// The ground road ways in chains through the joints and the junctions' arms in `through`, each with its
/// direction, and how each chain ends at its first and its last way. The chains start at the ways in the order of
/// their first vertex, so a loop has its two ends at one place in every cell.
fn chains(g: &WayGraph, through: &[((u32, i8), Next)]) -> Vec<Chain> {
    let mut next: BTreeMap<(u32, i8), Next> = BTreeMap::new();
    for j in &g.joints {
        for (a, b) in [(&j.arms[0], &j.arms[1]), (&j.arms[1], &j.arms[0])].into_iter().filter(|(a, _)| at_end(g, a)) {
            let other = &g.ways[b.way as usize];
            let on = match (ground(g, b.way), other.family == Family::Road) {
                (true, _) => Ok((b.way, b.dir)),
                (false, true) if other.level != Level::Ground => Err(End::Flush),
                _ => Err(End::Stop),
            };
            next.insert((a.way, a.dir), on);
        }
    }
    let ends = g.junctions.iter().flat_map(|j| &j.arms).chain(g.ends.iter().map(|e| &e.arm));
    next.extend(ends.filter(|a| at_end(g, a)).map(|a| ((a.way, a.dir), Err(End::Stop))));
    next.extend(through.iter().copied());
    let mut order: Vec<u32> = (0..g.ways.len() as u32).filter(|&w| ground(g, w)).collect();
    order.sort_by_key(|&w| {
        let way = &g.ways[w as usize];
        (g.parts[way.part as usize].vertices[way.from.span as usize].pos, way.from.u.to_bits())
    });
    let mut seen = vec![false; g.ways.len()];
    let mut out = Vec::new();
    for w in order {
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
/// no stretch runs over a stop.
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

/// Takes away a stretch of `shares` shorter than `least` metres, but one at an open end of the chain, which may go on
/// past the graph.
fn without_stubs(shares: &mut [f64], m: &[f64], least: f64, ends: [End; 2]) {
    let n = shares.len();
    let mut from = None;
    for j in 0..=n {
        match (j < n && shares[j] > 0.0, from) {
            (true, None) => from = Some(j),
            (false, Some(f)) => {
                let open = (f == 0 && ends[0] == End::Open) || (j == n && ends[1] == End::Open);
                if !open && m[j.min(n - 1)] - m[f] < least {
                    shares[f..j].fill(0.0);
                }
                from = None;
            }
            _ => {}
        }
    }
}

/// Per ring its share by the stretch rules, from its stroke metres, need and stops. A level change at an end of the
/// chain passes as a need does where a need lies within `JOIN_M` of it, as the deck's parapet or the portal carries the
/// rail on: a rail that reaches it runs on full and ends square there.
fn shares(m: &[f64], need: &[bool], stops: &[bool], ends: [End; 2]) -> Vec<f64> {
    let n = m.len();
    let to_need = metres_to(m, stops, need);
    let mut passes = need.to_vec();
    passes[0] |= ends[0] == End::Flush && to_need[0] <= JOIN_M;
    passes[n - 1] |= ends[1] == End::Flush && to_need[n - 1] <= JOIN_M;
    let to_rail = metres_to(m, stops, &stretches(m, stops, &passes));
    let to_stop = metres_to(m, &vec![false; n], stops);
    let smooth = |t: f64| t.clamp(0.0, 1.0) * t.clamp(0.0, 1.0) * (3.0 - 2.0 * t.clamp(0.0, 1.0));
    let mut out: Vec<f64> = (0..n).map(|i| smooth(((LEAD_M + RAMP_M - to_rail[i]).min(to_stop[i])) / RAMP_M)).collect();
    without_stubs(&mut out, m, 2.0 * RAMP_M, ends);
    out
}

/// `f` of each ring.
fn flat(n: usize, f: impl Fn(usize) -> bool) -> Vec<bool> {
    (0..n).map(f).collect()
}

/// A zone where nothing stands: its middle, its radius, the way it stops (all where none), and the side of that way
/// it stops (each side where 0): a ring's side stops where its normal out has a dot of 0 or more with it.
type Zone = ([f64; 2], f64, Option<u32>, [f64; 2]);

/// The guard rails and delineators about a cell: see the module.
pub fn place(g: &WayGraph, input: &CellInput, sections: &Sections) -> Edges {
    let origin_y = input.key.origin().y as f64;
    let mpu_at = |q: [f64; 2]| metres_per_unit_at(origin_y + q[1]);
    let section = |part: u32| {
        let f = &input.features[g.parts[part as usize].feature.0 as usize];
        sections.way(&f.kind, &f.tags, false)
    };
    let road = |w: u32| g.ways[w as usize].family == Family::Road;
    let change = |w: u32| road(w) && g.ways[w as usize].level != Level::Ground;
    // Per road way: whether it is railed, and the half-width of its carriageway and shoulder.
    let info: Vec<Option<(bool, f64)>> = (0..g.ways.len() as u32)
        .map(|w| road(w).then(|| section(g.ways[w as usize].part)))
        .map(|s| s.map(|s| (RAILED.contains(&s.class.trim_end_matches("_link")), s.half_m + s.shoulder_m)))
        .collect();
    // The zones: about a junction, on its own arms, its widest way's paved half-width and corner and `MOUTH_M`; about
    // a level crossing, its widest way's and `MOUTH_M`. Where an arm leads to a level change within the zone (it is a
    // deck or tunnel road, or its next point is the joint to one), the arm most nearly opposite runs on into it as
    // through a joint, and the zone stops only their side that the other arms lie on.
    let mut zones: Vec<Zone> = Vec::new();
    let mut through = Vec::new();
    for j in g.junctions.iter().filter(|j| j.network == Network::Way && j.arms.len() > 2) {
        let r = (j.arms.iter().map(|a| section(g.ways[a.way as usize].part)))
            .map(|s| s.half_m + s.widen_m + s.shoulder_m + s.corner_radius_m + MOUTH_M)
            .fold(0.0, f64::max);
        let p = j.pos.map(f64::from);
        let r = r / mpu_at(p);
        let leads = |a: &Arm| match a.next {
            _ if change(a.way) => true,
            Some(PointRef::Joint(k)) => {
                let x = &g.joints[k as usize];
                dist(x.pos.map(f64::from), p) < r && x.arms.iter().any(|b| change(b.way))
            }
            _ => false,
        };
        let pair = j.arms.iter().position(leads).map(|l| {
            let opposite = |k: usize| libm::cos(j.arms[k].angle - j.arms[l].angle);
            let t = (0..j.arms.len()).filter(|&k| k != l).min_by(|&a, &b| opposite(a).total_cmp(&opposite(b)));
            [l, t.expect("a junction has three arms")]
        });
        for (k, a) in j.arms.iter().enumerate() {
            let partner = match pair {
                Some([l, t]) if k == l => t,
                Some([l, t]) if k == t => l,
                _ => {
                    zones.push((p, r, Some(a.way), [0.0; 2]));
                    continue;
                }
            };
            let b = &j.arms[partner];
            if at_end(g, a) && ground(g, a.way) {
                let on = if ground(g, b.way) && at_end(g, b) {
                    Ok((b.way, b.dir))
                } else {
                    Err(if change(b.way) { End::Flush } else { End::Stop })
                };
                through.push(((a.way, a.dir), on));
            }
            let side = [-a.heading[1], a.heading[0]];
            let others = j.arms.iter().enumerate().filter(|&(c, _)| c != k && c != partner);
            zones.extend(others.map(|(_, c)| (p, r, Some(a.way), side.map(|v| v * dot(side, c.heading).signum()))));
        }
    }
    for x in g.crossings.iter().filter(|x| x.kind == CrossingKind::LevelCrossing) {
        let p = x.pos.map(f64::from);
        let r = [x.over, x.under].map(|s| section(s.part)).iter().map(|s| s.half_m + s.shoulder_m).fold(0.0, f64::max);
        zones.push((p, (r + MOUTH_M) / mpu_at(p), None, [0.0; 2]));
    }
    let in_zones =
        Grid::of(BUCKET, &zones.iter().map(|&(q, r, ..)| [q[0] - r, q[1] - r, q[0] + r, q[1] + r]).collect::<Vec<_>>());
    let in_zone = |p: &Point, out: [f64; 2]| {
        in_zones.any(p.pos, 0.0, |k| {
            let (q, r, way, side) = zones[k as usize];
            dist(q, p.pos) < r && way.is_none_or(|w| w == p.way) && dot(out, side) >= 0.0
        })
    };
    let c = CELL_UNITS as f64;
    // A ring this near the cell square may own a piece in it.
    let near_cell = |q: [f64; 2], mpu: f64| {
        let r = 2.0 * POST_M / mpu;
        q[0] > -r && q[1] > -r && q[0] < c + r && q[1] < c + r
    };
    let mapped_any = input.features.iter().any(|f| f.kind == "guard-rail");
    let railed = |w: u32| info[w as usize].is_some_and(|i| i.0);
    let mut ctx: Option<Context> = None;
    let mut out = Edges::default();
    for (chain, ends) in chains(g, &through) {
        if !mapped_any && !chain.iter().any(|&(w, _)| railed(w)) {
            continue;
        }
        let mut pts = Vec::new();
        for &(w, d) in &chain {
            way_points(g, w, d, &mut pts);
        }
        pts.dedup_by(|b, a| dist(a.pos, b.pos) < 1.0);
        // The chain ends past the graph where the graph leaves out a span.
        let cuts: Vec<usize> = (1..pts.len())
            .filter(|&i| pts[i].way == pts[i - 1].way && pts[i].span.abs_diff(pts[i - 1].span) > 1)
            .collect();
        let bounds = [0].into_iter().chain(cuts.iter().copied()).zip(cuts.iter().copied().chain([pts.len()]));
        for (from, to) in bounds {
            let ends = [if from == 0 { ends[0] } else { End::Open }, if to == pts.len() { ends[1] } else { End::Open }];
            let pts = &pts[from..to];
            let n = pts.len();
            let mpu: Vec<f64> = pts.iter().map(|p| mpu_at(p.pos)).collect();
            if n < 2 || !(0..n).any(|i| near_cell(pts[i].pos, mpu[i])) {
                continue;
            }
            // Stroke metres in whole 1/1024 m: their sums and differences are exact, so two cells, whose chains start
            // at different rings, decide each threshold alike.
            let mut m = vec![0.0; n];
            for i in 1..n {
                m[i] = m[i - 1] + libm::round(dist(pts[i].pos, pts[i - 1].pos) * mpu[i] * 1024.0) / 1024.0;
            }
            let ctx = &*ctx.get_or_insert_with(|| Context::new(input));
            for side in [1.0, -1.0] {
                // Per ring its point on the shoulder's outer edge, the normal out (the chain's left, as the frame's y
                // points south, then this side's), whether its way is railed and its paved half-width.
                let rings: Vec<([f64; 2], [f64; 2], bool, f64)> = (0..n)
                    .map(|i| {
                        let (railed, e) = info[pts[i].way as usize].unwrap_or((false, 0.0));
                        let nrm = [side * pts[i].t[1], -side * pts[i].t[0]];
                        (add(pts[i].pos, nrm, (e + pts[i].widen) / mpu[i]), nrm, railed, e + pts[i].widen)
                    })
                    .collect();
                let mapped = flat(n, |i| ctx.mapped(rings[i].0, mpu[i]));
                let drops = flat(n, |i| {
                    // The bank lifts the way's left: this side is its left where it is the chain's left along the way.
                    let (p, e) = (&pts[i], rings[i].3);
                    let z = p.z + p.bank * side * p.dir * e;
                    let t = ctx.terrain(add(rings[i].0, rings[i].1, DROP_OUT_M / mpu[i]));
                    rings[i].2 && t.is_some_and(|t| z - t > DROP_M)
                });
                // The town test runs only where a rail may stand.
                let may = metres_to(&m, &vec![false; n], &flat(n, |i| mapped[i] || drops[i]));
                let town = flat(n, |i| rings[i].2 && may[i] <= JOIN_M && ctx.town(rings[i].0, mpu[i]));
                let need = flat(n, |i| mapped[i] || (drops[i] && !town[i]));
                let mut towns: Vec<f64> = town.iter().map(|&t| f64::from(u8::from(t))).collect();
                without_stubs(&mut towns, &m, BREAK_M, ends);
                let fold = |j: usize| dot(sub(rings[j + 1].0, rings[j].0), add(pts[j].t, pts[j + 1].t, 1.0)) <= 0.0;
                let stops = flat(n, |i| {
                    let bit = if side * pts[i].dir > 0.0 { 1 } else { 2 };
                    (i == 0 && ends[0] == End::Stop)
                        || (i + 1 == n && ends[1] == End::Stop)
                        || in_zone(&pts[i], rings[i].1)
                        || pts[i].walk & bit != 0
                        || (towns[i] > 0.0 && !mapped[i])
                        || (i > 0 && fold(i - 1))
                        || (i + 1 < n && fold(i))
                });
                let share = shares(&m, &need, &stops, ends);
                let ring = |i: usize| Ring {
                    edge: rings[i].0,
                    out: rings[i].1,
                    along: pts[i].t,
                    mpu: mpu[i],
                    share: share[i],
                    post: pts[i].post && share[i] > 0.0,
                };
                // Runs: the rings with a share and the sunk ring on each side; one sunk ring between two runs joins
                // them.
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
                    let run: Vec<Ring> = (i.saturating_sub(1)..=(j + 1).min(n - 1)).map(ring).collect();
                    if run.iter().any(|r| near_cell(r.edge, r.mpu)) {
                        out.rails.push(run);
                    }
                    i = j + 1;
                }
                // Delineators where no rail stands, clear of every stop and outside towns.
                let to_stop = metres_to(&m, &vec![false; n], &stops);
                let turn = |i: usize| {
                    let lo = (0..=i).rev().find(|&k| m[i] - m[k] >= BEND_M).unwrap_or(0);
                    let hi = (i..n).find(|&k| m[k] - m[i] >= BEND_M).unwrap_or(n - 1);
                    libm::acos(dot(pts[lo].t, pts[hi].t).clamp(-1.0, 1.0)) > BEND_RAD
                };
                for i in (0..n).filter(|&i| rings[i].2 && pts[i].post && to_stop[i] >= CLEAR_M) {
                    let on = |step: f64| libm::fmod(pts[i].along, step) == 0.0;
                    if share[i.saturating_sub(1)..(i + 2).min(n)].iter().all(|&v| v <= 0.0)
                        && (on(STEP_M) || (on(BEND_STEP_M) && turn(i)))
                        && near_cell(rings[i].0, mpu[i])
                        && !ctx.town(rings[i].0, mpu[i])
                    {
                        out.delineators.push(ring(i));
                    }
                }
            }
        }
    }
    out
}

/// A point of a beam segment's plan: its place, its share of the way from the segment's first ring, and whether the
/// side that starts at it is the segment's end at its first ring, at its second, or a long side.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Vertex {
    q: [f64; 2],
    t: f64,
    side: Side,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Side {
    Long,
    First,
    Second,
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
                // The cut point starts the side that a went on, or the cell line, a long side that the solid leaves open.
                out.push(Vertex { q, t: p.t + f * (r.t - p.t), side: if inside(&a) { Side::Long } else { a.side } });
            }
        }
        poly = out;
    }
    poly
}

fn rounded(q: [f64; 2]) -> P {
    q.map(|v| libm::round(v) as i32)
}

/// The solids of a cell's guard rails and delineators in its storey mesh, each corner on the label that the block
/// finds at its point; a piece whose point finds none is left out. A beam is cut on the cell lines and stays open
/// there, and so is a post or a delineator, whose corners take the label at its middle where the cell holds it.
pub(super) fn solids(b: &mut Build, edges: &Edges, words: &[u32]) {
    let c = CELL_UNITS as f64;
    let sunk = |share: f64| -SINK_M * (1.0 - share);
    let at = |r: &Ring, d: f64, a: f64| add(add(r.edge, r.out, -d / r.mpu), r.along, a / r.mpu);
    // A box at ring r, `d` in from the edge, of half depth and width `half`, cut by the cell square: a prism between
    // each two offsets of `up`, on the label at its middle, or where a neighbour holds that, at its first point.
    let boxed = |b: &mut Build, part: usize, r: &Ring, d: f64, [hd, ha]: [f64; 2], up: &[f64]| {
        let mid = at(r, d, 0.0);
        let o = rounded(mid);
        let corner = |x: f64, y: f64| sub(at(r, d + x, y), o.map(f64::from));
        let ring = clip(o, &[corner(-hd, -ha), corner(hd, -ha), corner(hd, ha), corner(-hd, ha)]);
        let inside = mid.iter().all(|v| (0.0..=c).contains(v));
        if let Some(label) = ring.first().and_then(|&p| find(words, (if inside { o } else { p }).map(f64::from))) {
            for w in up.windows(2) {
                prism(b, part, &ring, (label, w[1]), (label, w[0]));
            }
        }
    };
    for run in &edges.rails {
        for (k, w) in run.windows(2).enumerate() {
            let (ra, rb) = (&w[0], &w[1]);
            let [f, bk] = BEAM_IN_M;
            let plan = clip_square(vec![
                Vertex { q: at(ra, f, 0.0), t: 0.0, side: Side::Long },
                Vertex { q: at(rb, f, 0.0), t: 1.0, side: Side::Second },
                Vertex { q: at(rb, bk, 0.0), t: 1.0, side: Side::Long },
                Vertex { q: at(ra, bk, 0.0), t: 0.0, side: Side::First },
            ]);
            let ring: Vec<P> = plan.iter().map(|v| rounded(v.q)).collect();
            let Some(labels) = ring.iter().map(|p| find(words, p.map(f64::from))).collect::<Option<Vec<u32>>>() else {
                continue;
            };
            let s = |i: usize| sunk(ra.share + plan[i].t * (rb.share - ra.share));
            // The end faces stand at the run's ends only.
            let wall = |i: usize| match plan[i].side {
                Side::First => k == 0,
                Side::Second => k + 2 == run.len(),
                Side::Long => true,
            };
            solid(b, RAIL, &ring, |i| (labels[i], BEAM_UP_M[1] + s(i)), |i| (labels[i], BEAM_UP_M[0] + s(i)), wall);
        }
        for r in run.iter().filter(|r| r.post) {
            boxed(b, RAIL, r, POST_IN_M, POST_HALF_M, &POST_UP_M.map(|up| up + sunk(r.share)));
        }
    }
    for r in &edges.delineators {
        boxed(b, DELINEATOR, r, DELI_IN_M, DELI_HALF_M, &DELI_UP_M);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rings every 2 m over `len` metres, with need where `need(m)` holds and stops where `stop(m)` holds.
    fn chain(len: f64, need: impl Fn(f64) -> bool, stop: impl Fn(f64) -> bool, ends: [End; 2]) -> (Vec<f64>, Vec<f64>) {
        let m: Vec<f64> = (0..=(len / 2.0) as usize).map(|i| 2.0 * i as f64).collect();
        let (need, stops): (Vec<bool>, Vec<bool>) = m.iter().map(|&x| (need(x), stop(x))).unzip();
        let s = shares(&m, &need, &stops, ends);
        (m, s)
    }

    fn at(m: &[f64], s: &[f64], x: f64) -> f64 {
        s[m.iter().position(|&v| v == x).unwrap()]
    }

    const OPEN: [End; 2] = [End::Open; 2];

    #[test]
    fn a_need_stands_full_with_its_lead_then_ramps_down() {
        let (m, s) = chain(200.0, |x| (80.0..=120.0).contains(&x), |_| false, OPEN);
        assert_eq!(at(&m, &s, 100.0), 1.0);
        assert_eq!(at(&m, &s, 124.0), 1.0, "the lead");
        assert!(at(&m, &s, 128.0) > 0.0 && at(&m, &s, 128.0) < 1.0, "the ramp");
        assert_eq!(at(&m, &s, 130.0), 0.0);
        assert_eq!(at(&m, &s, 70.0), 0.0);
    }

    #[test]
    fn a_short_gap_joins_and_a_long_one_breaks() {
        let (m, s) = chain(300.0, |x| (50.0..=80.0).contains(&x) || (100.0..=130.0).contains(&x), |_| false, OPEN);
        assert_eq!(at(&m, &s, 90.0), 1.0, "a gap of 20 m closes");
        let (m, s) = chain(300.0, |x| (50.0..=80.0).contains(&x) || (120.0..=150.0).contains(&x), |_| false, OPEN);
        assert_eq!(at(&m, &s, 100.0), 0.0, "a gap of 40 m keeps its break");
    }

    #[test]
    fn a_junction_stop_sinks_the_rail_before_it() {
        let (m, s) = chain(200.0, |x| (40.0..=160.0).contains(&x), |x| (96.0..=104.0).contains(&x), OPEN);
        assert_eq!(at(&m, &s, 100.0), 0.0);
        assert_eq!(at(&m, &s, 96.0), 0.0);
        assert!(at(&m, &s, 94.0) < 1.0 && at(&m, &s, 94.0) > 0.0, "it ramps over RAMP_M");
        assert_eq!(at(&m, &s, 90.0), 1.0);
    }

    #[test]
    fn a_rail_runs_on_to_a_deck_end_and_none_starts_at_one() {
        let (_, s) = chain(2.0, |_| false, |_| false, [End::Flush; 2]);
        assert_eq!(s, [0.0, 0.0], "a stub between two level changes");
        let (_, s) = chain(100.0, |x| x <= 80.0, |_| false, [End::Open, End::Flush]);
        assert!(s[s.len() - 4..].iter().all(|&v| v == 1.0), "full to the end line, 20 m past the last need");
        let (_, s) = chain(100.0, |x| x <= 40.0, |_| false, [End::Open, End::Flush]);
        assert_eq!(s[s.len() - 1], 0.0, "60 m past the last need, the deck end starts no rail");
        let (_, s) = chain(100.0, |x| x >= 98.0, |x| x == 96.0, OPEN);
        assert!(s[s.len() - 1] > 0.0, "a short stretch at an open end stays: the chain may go on past the graph");
    }

    #[test]
    fn a_stop_just_before_a_deck_end_leaves_no_stub() {
        let (_, s) = chain(100.0, |x| x >= 98.0, |x| x == 96.0, [End::Open, End::Flush]);
        assert_eq!(s[s.len() - 1], 0.0);
    }

    #[test]
    fn a_cell_line_cuts_a_beam_exactly_on_it_and_alike_from_both_sides() {
        let c = CELL_UNITS as f64;
        let v = |q: [f64; 2], t: f64, side: Side| Vertex { q, t, side };
        let quad = |x0: f64, x1: f64| {
            vec![
                v([x0, 100.0], 0.0, Side::Long),
                v([x1, 117.0], 1.0, Side::Second),
                v([x1, 130.0], 1.0, Side::Long),
                v([x0, 113.0], 0.0, Side::First),
            ]
        };
        let cut = clip_square(quad(c - 300.0, c + 500.0));
        let on_line: Vec<Vertex> = cut.iter().copied().filter(|p| p.q[0] == c).collect();
        assert_eq!(on_line.len(), 2);
        assert!(on_line.iter().all(|p| (p.t - 300.0 / 800.0).abs() < 1e-12));
        assert!(cut.iter().all(|p| p.side != Side::Second), "the far end lies past the cell");
        // The neighbour holds the same beam 1 cell west: its cut points lie on its own x = 0.
        let shifted = clip_square(quad(-300.0, 500.0));
        let theirs: Vec<f64> = shifted.iter().filter(|p| p.q[0] == 0.0).map(|p| p.q[1]).collect();
        let ours: Vec<f64> = on_line.iter().map(|p| p.q[1]).collect();
        assert_eq!(theirs, ours);
    }
}
