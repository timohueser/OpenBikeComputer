//! The sidewalks: per way line and side that the bake's `walk` bits mark, a slab from inside the carriageway out
//! to the class's sidewalk width past the kerb. The kerb is the carriageway's edge and stays straight; only the
//! back edge moves. At a graph point a slab goes on into the next road's slab through an open sector, follows the
//! kerb round a two-foot round where both arms walk (a ring), runs at full width into another road, whose
//! carriageway cuts it, or tapers against nothing: its width falls along a ramp with round kinks to 0 at its tip
//! (`Ramp`). A width rule is a point and a direction, so the slab's width and its kerb's rise are one continuous
//! function of position, and two cells build the same slabs.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use super::geom::P;
use super::points::{Arm, Rounded, TwoFoot, INSET};
use super::stroke::{left, pie_between, push_positive, side_along, union, unit, Rings, STEPS};
use super::{Cand, Ctx, Label, Owner, Point, Rank, WayLine, TERRAIN};
use crate::faces::{HeightRule, Surface};
use crate::graph::Level;
use crate::CellInput;

/// A width rule of a line side: from a point of the line (frame units) along the line's unit direction `dir`, the
/// slab on the `left` of `dir` (else on its right) rises from `target` metres (0 at a taper's tip) to its full width
/// (`Ramp`). A point takes the rule at its metres along `dir` past `at`, on its side of the line through `at` and at
/// most `across` metres from it (the slab's back edge there) and its metres along: a slab turns away from the line by
/// no more than its length, and the rule leaves the slabs of other places of its line side alone. The values are those that the records hold (`records::WALK_RULE`), so a block gives the same
/// widths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct End {
    pub at: [i32; 2],
    pub dir: [f64; 2],
    pub left: bool,
    pub target: f64,
    pub across: f64,
}

impl End {
    fn new(at: [f64; 2], dir: [f64; 2], left: bool, target: f64, across: f64) -> End {
        let q = |v: f64| libm::round(v.clamp(-1.0, 1.0) * 32767.0) / 32767.0;
        let cm = |m: f64| libm::round(m * 100.0) / 100.0;
        End { at: round(at), dir: [q(dir[0]), q(dir[1])], left, target: cm(target), across: cm(across) }
    }

    /// The metres along the rule past its point of q, frame units from the origin, with `mpu` metres per unit; None
    /// where q lies before it, on the other side of the line through its point, or farther across than its slab. The
    /// point is its place rounded to a unit, so the rule holds from one unit before it on, at its target there.
    #[inline(never)]
    fn past(&self, q: [f64; 2], mpu: f64) -> Option<f64> {
        let v = [q[0] - f64::from(self.at[0]), q[1] - f64::from(self.at[1])];
        let x = v[0] * self.dir[0] + v[1] * self.dir[1];
        let across = (self.dir[0] * v[1] - self.dir[1] * v[0]) * if self.left { mpu } else { -mpu };
        let along = x.max(0.0) * mpu;
        (x >= -1.0 && across > 0.0 && across <= self.across + along).then_some(along)
    }
}

/// Per line side, at `side_of`, its width rules.
pub(crate) type Ends = Vec<Vec<End>>;

/// The place of a line side in the per-side lists: two per line, the stroke's right normal first.
pub(crate) fn side_of(line: usize, side: f64) -> usize {
    2 * line + usize::from(side > 0.0)
}

#[derive(Default)]
pub(super) struct Walk {
    pub cands: Vec<Cand>,
    pub ends: Ends,
}

/// One arc step: a join this close to half a turn has no gap worth a round, and the strokes mitre it.
const STEP: f64 = 2.0 * PI / STEPS as f64;
/// A stroke that a slab must not enter is searched for over the slab's sections this far apart, then finer.
const STROKE_STEP_M: f64 = 0.5;

/// The ramp from `target` to `full` at slope `k`, its two kinks rounded with radius `r` (less where the two rounds
/// would meet), so that it leaves the target at its station and meets the full width tangentially.
struct Ramp {
    target: f64,
    full: f64,
    k: f64,
    r: f64,
    /// The tangent length of a round, and where the upper kink lies.
    t: f64,
    top: f64,
    phi: f64,
}

impl Ramp {
    #[inline(never)]
    fn new(target: f64, full: f64, k: f64, r: f64) -> Ramp {
        let phi = libm::atan(k);
        let (s, c) = (libm::sin(phi), libm::cos(phi));
        let r = r.min((full - target).max(0.0) * (1.0 + c) / (2.0 * s * s));
        let t = r * s / (1.0 + c);
        Ramp { target, full, k, r, t, top: t + (full - target) / k, phi }
    }

    /// The width x metres past the station.
    #[inline(never)]
    fn at(&self, x: f64) -> f64 {
        let (r, s, c) = (self.r, libm::sin(self.phi), libm::cos(self.phi));
        let x = x.max(0.0);
        if self.full <= self.target {
            self.full
        } else if x <= r * s {
            self.target + r - (r * r - x * x).max(0.0).sqrt()
        } else if x <= self.top - self.t * c {
            self.target + self.k * (x - self.t)
        } else if x < self.top + self.t {
            let y = x - self.top - self.t;
            self.full - r + (r * r - y * y).max(0.0).sqrt()
        } else {
            self.full
        }
    }

    /// The places of its rounds in steps of at most one arc step of turn, metres past the station: a stroke through
    /// them follows the rounds.
    fn points(&self) -> Vec<f64> {
        let n = libm::ceil(self.phi / STEP).max(1.0) as usize;
        let a = |j: usize| self.r * libm::sin(self.phi * j as f64 / n as f64);
        (0..=n).flat_map(|j| [a(j), self.top + self.t - a(j)]).collect()
    }
}

/// The width at q (frame units, `mpu` metres per unit) of a side of full width `full` with the rules `ends`: the least
/// ramp of the rules that hold q. With `tips`, of the taper tips alone: the kerb's rise, which a narrower way beyond
/// leaves.
#[inline(never)]
pub(crate) fn width(q: [f64; 2], ends: &[End], full: f64, k: f64, r: f64, mpu: f64, tips: bool) -> f64 {
    (ends.iter().filter(|e| !tips || e.target == 0.0))
        .filter_map(|e| e.past(q, mpu).map(|x| Ramp::new(e.target, full, k, r).at(x)))
        .fold(full, f64::min)
}

/// The sidewalk width of a line's class; 0 for a class with none, water and rails.
fn full_m(line: &WayLine) -> f64 {
    if line.sec.water || line.rail {
        0.0
    } else {
        line.sec.sidewalk_m
    }
}

/// Whether segment i of a line has a sidewalk on stroke side `side` (the `walk` bit 2 on the stroke's left normal,
/// the bake's right, 1 on its right).
fn walks_seg(line: &WayLine, i: usize, side: f64) -> bool {
    let bit = if side > 0.0 { 2 } else { 1 };
    full_m(line) > 0.0 && i + 1 < line.p.len() && line.walk[i] & bit != 0
}

/// Where a side's `walk` bits start a short way out from a junction on its own line, that side walks from the junction
/// on, so that its slab meets the junction's corner by the corner rules and does not taper short of it. Short: less
/// than the class's corner round and a taper's reach together, the room in which a taper would end at the corner.
pub(super) fn run_on(ctx: &Ctx, lines: &mut [WayLine], of_way: &BTreeMap<u32, usize>, bx: [i32; 4]) {
    let g = ctx.graph;
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    for j in g.junctions.iter().filter(|j| j.arms.len() > 2 && near(j.pos)) {
        for a in &j.arms {
            let Some(&l) = of_way.get(&a.way) else { continue };
            let line = &lines[l];
            let full = full_m(line);
            if full <= 0.0 {
                continue;
            }
            let at = super::sample_at(g, line, &a.at, j.pos);
            let reach = ctx.units(line.sec.corner_radius_m + ctx.rules.plan.walk.reach_m(full), j.pos);
            // The arm's segments from the point on, and the length before each.
            let segs: Vec<usize> = if a.dir > 0 { (at..line.p.len() - 1).collect() } else { (0..at).rev().collect() };
            let len = |i: usize| {
                let (p, q) = (line.p[i], line.p[i + 1]);
                libm::hypot(f64::from(q[0] - p[0]), f64::from(q[1] - p[1]))
            };
            let mut fill = Vec::new();
            for bit in [1u8, 2] {
                let mut before = 0.0;
                for (k, &i) in segs.iter().enumerate() {
                    if line.walk[i] & bit != 0 {
                        fill.extend(segs[..k].iter().map(|&i| (i, bit)));
                        break;
                    }
                    before += len(i);
                    if before >= reach {
                        break;
                    }
                }
            }
            for (i, bit) in fill {
                lines[l].walk[i] |= bit;
            }
        }
    }
}

/// Whether an arm's first segment has a sidewalk on its right, seen from the point, else on its left.
pub(super) fn walks(line: &WayLine, arm: Arm, right: bool) -> bool {
    let side = if (arm.dir > 0) == right { 1.0 } else { -1.0 };
    let seg = if arm.dir > 0 { Some(arm.at) } else { arm.at.checked_sub(1) };
    seg.is_some_and(|s| walks_seg(line, s, side))
}

/// A line's point and half-width at fractional vertex index x, and its segment's unit direction.
#[inline(never)]
fn at_x(line: &WayLine, x: f64) -> ([f64; 2], f64, [f64; 2]) {
    let n = line.p.len();
    let i = (libm::floor(x).max(0.0) as usize).min(n.saturating_sub(2));
    let t = x - i as f64;
    let (a, b) = (line.p[i], line.p[(i + 1).min(n - 1)]);
    let p = [f64::from(a[0]) + t * f64::from(b[0] - a[0]), f64::from(a[1]) + t * f64::from(b[1] - a[1])];
    let h = line.h[i] + t * (line.h[(i + 1).min(n - 1)] - line.h[i]);
    (p, h, unit(a, b).unwrap_or([1.0, 0.0]))
}

/// The point at fractional vertex index x of a line, moved `d` units across onto stroke side `side`.
#[inline(never)]
fn beside(line: &WayLine, x: f64, side: f64, d: f64) -> [f64; 2] {
    let (q, _, u) = at_x(line, x);
    let n = left(u);
    [q[0] + side * d * n[0], q[1] + side * d * n[1]]
}

/// The fractional vertex index `d` units from x toward `dir` along a line, at most to `stop`.
#[inline(never)]
fn moved(line: &WayLine, x: f64, dir: f64, d: f64, stop: f64) -> f64 {
    let (mut x, mut d) = (x, d);
    while d > 0.0 && (stop - x) * dir > 0.0 {
        let i = if dir > 0.0 { libm::floor(x) as usize } else { (libm::ceil(x) as usize).saturating_sub(1) };
        let i = i.min(line.p.len() - 2);
        let (a, b) = (line.p[i], line.p[i + 1]);
        let len = libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])).max(1e-9);
        let end = if dir > 0.0 { (i + 1) as f64 } else { i as f64 };
        let room = (end - x).abs() * len;
        if room >= d {
            x += dir * d / len;
            d = 0.0;
        } else {
            x = end;
            d -= room;
        }
    }
    if (x - stop) * dir > 0.0 {
        stop
    } else {
        x
    }
}

fn round(q: [f64; 2]) -> P {
    [libm::round(q[0]) as i32, libm::round(q[1]) as i32]
}

/// How a piece of a slab ends: at the read box, at a taper's tip, square (at a ring's foot, where it runs into
/// another road), or on a join to another line's slab.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tip {
    Open,
    Taper,
    Square,
    Join,
}

/// A run of one line side's slab between fractional vertex indices.
#[derive(Clone, Copy, Debug)]
struct Piece {
    x: [f64; 2],
    tip: [Tip; 2],
}

/// A join between two lines' slabs at a graph point: the point, and per side its line, stroke side and arm.
type Join = (P, [(usize, f64, Arm); 2]);

/// What ends a piece at a place of a line side (a fractional vertex index) toward a direction: its tip there, and where
/// it moves the end from the place.
struct Cut {
    v: f64,
    dir: i8,
    tip: Tip,
    to: Option<f64>,
}

/// Each line side's pieces after the cuts: a cut at v toward `dir` splits the piece that goes on from v that way and
/// ends the part on that side with its tip, moved to `to`.
fn apply(pieces: &mut Vec<Piece>, cuts: &[Cut]) {
    for c in cuts {
        let (v, d) = (c.v, f64::from(c.dir));
        let holds = |p: &Piece| if d > 0.0 { p.x[0] <= v && v < p.x[1] } else { p.x[0] < v && v <= p.x[1] };
        let Some(k) = pieces.iter().position(holds) else { continue };
        let p = pieces[k];
        let end = usize::from(d < 0.0);
        if p.x[0] < v && v < p.x[1] {
            let mut other = p;
            other.x[1 - end] = v;
            other.tip[1 - end] = Tip::Square;
            pieces.push(other);
        }
        let piece = &mut pieces[k];
        piece.x[end] = v;
        piece.tip[end] = c.tip;
        if let Some(to) = c.to {
            piece.x[end] = to;
        }
        if piece.x[1] - piece.x[0] <= 1e-9 {
            pieces.remove(k);
        }
    }
}

/// The slab's half of a ring about centre c (absolute, units) of radius rho: the annulus sector from angle `a0`
/// through `sweep`, out to rho + `INSET` (into the round's fan), in to rho less the slab's width, which runs from
/// `w[0]` at the first foot to `w[1]` at the second by angle and stops at the centre. Its two halves, split at
/// the bisector, whose points both compute alike.
fn ring_halves(c: [f64; 2], rho: f64, a0: f64, sweep: f64, w: [f64; 2]) -> [Vec<P>; 2] {
    let at = |a: f64, r: f64| round([c[0] + r * libm::cos(a), c[1] + r * libm::sin(a)]);
    let inner = |a: f64| (rho - w[0] - (w[1] - w[0]) * (a - a0) / sweep).max(0.0);
    let half = |from: f64, to: f64| {
        let mut angles = vec![from];
        angles.extend(super::points::arc_angles(from, to - from));
        angles.push(to);
        let mut ring: Vec<P> = angles.iter().map(|&a| at(a, rho + INSET)).collect();
        ring.extend(angles.iter().rev().map(|&a| at(a, inner(a))));
        ring
    };
    let mid = a0 + sweep / 2.0;
    [half(a0, mid), half(mid, a0 + sweep)]
}

/// A stroke that a slab must not enter: its centre line and the half-width at each point (units), and the boxes of the
/// whole and of each segment, grown by the half-widths. Its ends are flat on their end lines.
pub(crate) struct Stroke {
    pts: Vec<[f64; 2]>,
    half: Vec<f64>,
    bx: [f64; 4],
    segs: Vec<[f64; 4]>,
}

/// The box of points a and b grown by g.
fn grow(a: [f64; 2], b: [f64; 2], g: f64) -> [f64; 4] {
    [a[0].min(b[0]) - g, a[1].min(b[1]) - g, a[0].max(b[0]) + g, a[1].max(b[1]) + g]
}

fn in_box(b: &[f64; 4], q: [f64; 2]) -> bool {
    (b[0]..=b[2]).contains(&q[0]) && (b[1]..=b[3]).contains(&q[1])
}

impl Stroke {
    pub(crate) fn of(line: &WayLine) -> Stroke {
        let pts: Vec<[f64; 2]> = line.p.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect();
        let segs: Vec<[f64; 4]> =
            (0..pts.len().saturating_sub(1)).map(|i| grow(pts[i], pts[i + 1], line.h[i].max(line.h[i + 1]))).collect();
        let bx = segs.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, s| {
            [b[0].min(s[0]), b[1].min(s[1]), b[2].max(s[2]), b[3].max(s[3])]
        });
        Stroke { pts, half: line.h.clone(), bx, segs }
    }

    /// Whether q lies inside by more than `margin` units: within the half-width of a segment's foot that falls
    /// inside the segment, or of a vertex between two segments. A point on an end line lies outside.
    pub(crate) fn holds(&self, q: [f64; 2], margin: f64) -> bool {
        let (pts, half, n) = (&self.pts, &self.half, self.pts.len());
        in_box(&self.bx, q)
            && (0..n.saturating_sub(1)).filter(|&i| in_box(&self.segs[i], q)).any(|i| {
                let (a, b) = (pts[i], pts[i + 1]);
                let (u, v) = ([b[0] - a[0], b[1] - a[1]], [q[0] - a[0], q[1] - a[1]]);
                let l2 = u[0] * u[0] + u[1] * u[1];
                let t = if l2 > 0.0 { (u[0] * v[0] + u[1] * v[1]) / l2 } else { 0.0 };
                let flat = (i == 0 && t <= 0.0) || (i + 2 == n && t >= 1.0);
                let t = t.clamp(0.0, 1.0);
                let d = libm::hypot(v[0] - t * u[0], v[1] - t * u[1]);
                !flat && d < half[i] + t * (half[i + 1] - half[i]) - margin
            })
    }
}

/// Whether the section of a line side at x, from the kerb out to the full width, enters one of `strokes`.
fn enters(ctx: &Ctx, line: &WayLine, x: f64, side: f64, strokes: &[&Stroke]) -> bool {
    let (c, h, _) = at_x(line, x);
    let w = ctx.units(full_m(line), round(c));
    (0..=8).any(|k| {
        let q = beside(line, x, side, h + w * f64::from(k) / 8.0);
        strokes.iter().any(|s| s.holds(q, 0.0))
    })
}

/// The runs of a line side's sections between fractional vertex indices `x` that enter one of `strokes`, found
/// `STROKE_STEP_M` apart: per run, the last place before it and the first past it whose sections do not enter
/// (bisected), or the end of `x`. A slab cut out there ends with tapers that lie wholly in sections that do not enter.
fn entering(ctx: &Ctx, line: &WayLine, side: f64, strokes: &[&Stroke], x: [f64; 2]) -> Vec<[f64; 2]> {
    let meets = |x: f64| enters(ctx, line, x, side, strokes);
    // The place between an outside one and an inside one where the sections start to enter, on the outside.
    let edge = |mut out: f64, mut inside: f64| {
        for _ in 0..7 {
            let mid = 0.5 * (out + inside);
            if meets(mid) {
                inside = mid;
            } else {
                out = mid;
            }
        }
        out
    };
    let step = ctx.units(STROKE_STEP_M, line.p[0]);
    let (mut at, mut runs) = (x[0], Vec::new());
    let (mut last, mut start): (f64, Option<f64>) = (at, None);
    loop {
        match (meets(at), start) {
            (true, None) => start = Some(if at == x[0] { at } else { edge(last, at) }),
            (false, Some(a)) => {
                runs.push([a, edge(at, last)]);
                start = None;
            }
            _ => {}
        }
        if at == x[1] {
            break;
        }
        last = at;
        at = moved(line, at, 1.0, step, x[1]);
    }
    runs.extend(start.map(|a| [a, x[1]]));
    runs
}

/// The unit directions of a line's segments between its places `xs`.
fn segment_dirs(line: &WayLine, xs: &[f64]) -> Vec<[f64; 2]> {
    xs.windows(2).map(|w| at_x(line, (w[0] + w[1]) / 2.0).2).collect()
}

/// A slab's stroke along a line's places `xs` on stroke side `side`: from half the half-width, inside the carriageway,
/// out to `outer` (units from the line) at each place.
fn slab_stroke(line: &WayLine, xs: &[f64], side: f64, outer: Vec<f64>, out: &mut Rings) {
    let p: Vec<P> = xs.iter().map(|&x| round(at_x(line, x).0)).collect();
    let inner: Vec<f64> = xs.iter().map(|&x| at_x(line, x).1 / 2.0).collect();
    let dirs: Vec<Option<[f64; 2]>> = segment_dirs(line, xs).into_iter().map(Some).collect();
    side_along(&p, &inner, &outer, &dirs, side, out);
}

/// The places along a piece where a stroke of its side turns: its ends, the line's vertices and its rules' round
/// points, found where the kerb's metres along a rule reach each round point.
fn piece_points(
    line: &WayLine,
    side: f64,
    ends: &[End],
    full: f64,
    ramp: (f64, f64),
    mpu: f64,
    x: [f64; 2],
) -> Vec<f64> {
    let mut out: Vec<f64> = vec![x[0], x[1]];
    out.extend((libm::floor(x[0]) as usize + 1..=libm::ceil(x[1]) as usize).map(|i| i as f64));
    for e in ends {
        let along = |x: f64| {
            let q = beside(line, x, side, at_x(line, x).1);
            let v = [q[0] - f64::from(e.at[0]), q[1] - f64::from(e.at[1])];
            (v[0] * e.dir[0] + v[1] * e.dir[1]) * mpu
        };
        for d in Ramp::new(e.target, full, ramp.0, ramp.1).points() {
            for i in libm::floor(x[0]) as usize..libm::ceil(x[1]) as usize {
                let (fa, fb) = (along(i as f64) - d, along((i + 1) as f64) - d);
                if fa * fb <= 0.0 && fa != fb {
                    out.push(i as f64 + fa / (fa - fb));
                }
            }
        }
    }
    out.retain(|&v| v >= x[0] && v <= x[1]);
    out.sort_by(f64::total_cmp);
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    out
}

/// The first crossing of polyline `path` with segment cd: its place along the path (segment index and fraction)
/// and its fraction along cd.
fn crossing(path: &[[f64; 2]], c: [f64; 2], d: [f64; 2]) -> Option<(f64, f64)> {
    (path.windows(2).enumerate())
        .find_map(|(k, w)| super::points::crossing(w[0], w[1], c, d).map(|(t, u)| (k as f64 + t, u)))
}

/// A line's centre line from vertex `at` toward `dir`, up to `reach` units (and its next vertex).
#[inline(never)]
fn path_from(line: &WayLine, at: usize, dir: i8, reach: f64) -> Vec<[f64; 2]> {
    let mut path: Vec<[f64; 2]> = Vec::new();
    let (mut i, mut length) = (at, 0.0);
    loop {
        let q = [f64::from(line.p[i][0]), f64::from(line.p[i][1])];
        if let Some(last) = path.last() {
            length += libm::hypot(q[0] - last[0], q[1] - last[1]);
        }
        path.push(q);
        let Some(j) = i.checked_add_signed(isize::from(dir)).filter(|&j| j < line.p.len()) else { break };
        if length >= reach {
            break;
        }
        i = j;
    }
    path
}

/// Where a slab on an arm's side runs into the other arm `other`: where the other arm's centre line first crosses the
/// slab's middle line (half its width past the kerb) within `extent_m`, on the arm's own line (the fractional vertex
/// index there), or past the point on its through partner's line (the other host arm on another line: that arm, the
/// slab's side there and the index). None where it does not.
fn run_in(ctx: &Ctx, lines: &[WayLine], pt: &Point, a: usize, side: f64, other: Arm) -> Option<Run> {
    let arm = pt.arms[a];
    let reach = ctx.units(ctx.rules.plan.extent_m, pt.pos);
    let path = path_from(&lines[other.line], other.at, other.dir, reach);
    let w = full_m(&lines[arm.line]);
    // The first crossing along the other arm with a line's slab middle line on its side: its place along the
    // path and on the line. The chord over the gap at a vertex between two segments' offsets counts too.
    let first = |line: &WayLine, side: f64| {
        let mid = |j: usize| line.h[j] + ctx.units(w / 2.0, line.p[j]);
        let near = |j: usize| {
            let (p, q) = (line.p[j], pt.pos);
            libm::hypot(f64::from(p[0] - q[0]), f64::from(p[1] - q[1])) <= reach
        };
        let off = |v: usize, n: [f64; 2]| {
            let p = line.p[v];
            [f64::from(p[0]) + side * mid(v) * n[0], f64::from(p[1]) + side * mid(v) * n[1]]
        };
        let mut best: Option<(f64, f64)> = None;
        let mut last: Option<(usize, [f64; 2])> = None;
        for j in (0..line.p.len().saturating_sub(1)).filter(|&j| near(j) || near(j + 1)) {
            let Some(n) = unit(line.p[j], line.p[j + 1]).map(left) else { continue };
            let mut found = crossing(&path, off(j, n), off(j + 1, n)).map(|(at, f)| (at, j as f64 + f));
            if let Some((_, m)) = last.filter(|&(i, _)| i + 1 == j) {
                found = found.or(crossing(&path, off(j, m), off(j, n)).map(|(at, _)| (at, j as f64)));
            }
            last = Some((j, n));
            if let Some(f) = found.filter(|f| best.is_none_or(|b| f.0 < b.0)) {
                best = Some(f);
            }
        }
        best
    };
    let own = first(&lines[arm.line], side).map(|(at, x)| (at, Run::Own(x)));
    // The partner goes on from the point the other way, so the same side of the road is its side times the turn of
    // the arms' directions.
    let partner = (0..pt.arms.len())
        .filter(|&k| k != a && pt.hosts[k] && pt.hosts[a] && pt.arms[k].line != arm.line)
        .map(|k| (pt.arms[k], -side * f64::from(arm.dir) * f64::from(pt.arms[k].dir)))
        .find_map(|(p, s)| {
            let past = |x: f64| (x - p.at as f64) * f64::from(p.dir) > 0.0;
            first(&lines[p.line], s).filter(|f| past(f.1)).map(|(at, x)| (at, Run::Partner(p, s, x)))
        });
    [own, partner].into_iter().flatten().min_by(|x, y| x.0.total_cmp(&y.0)).map(|f| f.1)
}

/// Where a slab runs into another road: on its own line, or past the point along the through partner's.
#[derive(Clone, Copy)]
enum Run {
    Own(f64),
    Partner(Arm, f64, f64),
}

/// The points of the cell's ground level where a deck starts, each with the strokes of its deck arms
/// (`deck_lines`: their paved layers and `edge_band_m`, as the ceilings').
fn deck_points(ctx: &Ctx, input: &CellInput, lines: &[WayLine], bx: [i32; 4]) -> Vec<(P, Vec<Stroke>)> {
    let g = ctx.graph;
    let arms = (g.junctions.iter().map(|j| (j.pos, &j.arms[..]))).chain(g.joints.iter().map(|j| (j.pos, &j.arms[..])));
    let found: Vec<(P, Vec<u32>)> = arms
        .filter(|(pos, arms)| {
            pos[0] >= bx[0]
                && pos[0] <= bx[2]
                && pos[1] >= bx[1]
                && pos[1] <= bx[3]
                && arms.iter().any(|a| a.level == Level::Deck)
                && arms.iter().any(|a| lines.iter().any(|l| l.way == a.way))
        })
        .map(|(pos, arms)| (pos, arms.iter().filter(|a| a.level == Level::Deck).map(|a| a.way).collect()))
        .collect();
    if found.is_empty() {
        return Vec::new();
    }
    let wanted: Vec<u32> = found.iter().flat_map(|f| f.1.iter().copied()).collect();
    let decks = super::deck_lines(ctx, input, &|w| wanted.contains(&w) && super::on_deck(g, w), bx);
    (found.into_iter())
        .map(|(pos, ways)| (pos, decks.iter().filter(|d| ways.contains(&d.way)).map(Stroke::of).collect()))
        .collect()
}

/// The direction at a piece's end x into the piece (`s`: +1 at its start, −1 at its end), of its segment there.
#[inline(never)]
fn dir_in(line: &WayLine, x: f64, s: f64) -> [f64; 2] {
    let i = if s > 0.0 { libm::floor(x) } else { libm::ceil(x) - 1.0 };
    let u = at_x(line, i.max(0.0) + 0.5).2;
    [s * u[0], s * u[1]]
}

/// A side's own width rules: a 0-target at each taper tip of its pieces, toward the piece.
fn rule_ends(ctx: &Ctx, line: &WayLine, pieces: &[Piece], side: f64) -> Vec<End> {
    let mut out = Vec::new();
    for p in pieces {
        for (n, s) in [(0, 1.0), (1, -1.0)] {
            if p.tip[n] == Tip::Taper {
                let (at, h, _) = at_x(line, p.x[n]);
                let across = h / ctx.units(1.0, round(at)) + full_m(line);
                out.push(End::new(at, dir_in(line, p.x[n], s), side * s > 0.0, 0.0, across));
            }
        }
    }
    out
}

/// The sidewalks of a cell's ground level: the candidates and each line side's width rules.
#[allow(clippy::too_many_arguments)]
pub(super) fn build(
    ctx: &Ctx,
    input: &CellInput,
    lines: &[WayLine],
    runs: &[Option<(usize, usize)>],
    points: &[Point],
    rounded: &[Rounded],
    bx: [i32; 4],
) -> Walk {
    let rules = &ctx.rules.plan.walk;
    let ramp = (rules.end_slope, rules.end_round_m);
    let (k, r) = ramp;
    let units = |m: f64, p: [f64; 2]| ctx.units(m, round(p));
    let mpu = |p: [f64; 2]| 1.0 / units(1.0, p);
    // The pieces from the bits, inside the read box: a run of segments with the bit; a taper at each end, but at
    // the box's edge.
    let mut pieces: Vec<Vec<Piece>> = vec![Vec::new(); 2 * lines.len()];
    for (l, line) in lines.iter().enumerate() {
        let Some((first, last)) = runs[l].filter(|_| full_m(line) > 0.0) else { continue };
        for side in [1.0, -1.0] {
            let mut i = first;
            while i + 1 < last {
                if !walks_seg(line, i, side) {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j + 1 < last && walks_seg(line, j, side) {
                    j += 1;
                }
                let lo = if i == first && first > 0 { Tip::Open } else { Tip::Taper };
                let hi = if j + 1 == last && last < line.p.len() { Tip::Open } else { Tip::Taper };
                pieces[side_of(l, side)].push(Piece { x: [i as f64, j as f64], tip: [lo, hi] });
                i = j;
            }
        }
    }
    if pieces.iter().all(Vec::is_empty) {
        return Walk::default();
    }
    let decks = deck_points(ctx, input, lines, bx);
    let mut cuts: Vec<Vec<Cut>> = (0..pieces.len()).map(|_| Vec::new()).collect();
    let mut rings: Vec<Rings> = vec![Vec::new(); pieces.len()];
    let mut joins: Vec<Join> = Vec::new();
    // Run-ins past the point along the partner line: the slab's side and arm, the partner arm, its side and the end.
    let mut onto: Vec<(usize, Arm, Arm, f64, f64)> = Vec::new();
    let heading = |a: Arm| {
        let line = &lines[a.line];
        let next = a.at.checked_add_signed(isize::from(a.dir)).filter(|&j| j < line.p.len());
        next.and_then(|j| unit(line.p[a.at], line.p[j])).map_or(0.0, |u| libm::atan2(u[1], u[0]))
    };
    for (pi, pt) in points.iter().enumerate() {
        let junction = matches!(pt.label.owner, Owner::Junction(_));
        // A turn's rounds are its only sectors: the inner side of its turn.
        let pairs: Vec<(Option<&TwoFoot>, usize, usize)> = if pt.seams {
            (pt.pairs.iter().enumerate())
                .map(|(n, &(a, b, _, _))| (rounded[pi].two_foot.iter().find(|t| t.pair == n), a, b))
                .collect()
        } else {
            rounded[pi].two_foot.iter().map(|t| (Some(t), t.arms[0], t.arms[1])).collect()
        };
        // No ring or join where a deck leaves the ground: the cut-out below ends the slabs there.
        let deck = decks.iter().any(|d| d.0 == pt.pos);
        for (round_of, a, b) in pairs {
            let (arm_a, arm_b) = (pt.arms[a], pt.arms[b]);
            let (sa, sb) = (f64::from(arm_a.dir), -f64::from(arm_b.dir));
            let wa = walks(&lines[arm_a.line], arm_a, true);
            let wb = walks(&lines[arm_b.line], arm_b, false);
            if !wa && !wb {
                continue;
            }
            let key = |arm: Arm, side: f64| side_of(arm.line, side);
            let sector = (heading(arm_b) - heading(arm_a)).rem_euclid(2.0 * PI);
            let same = !junction || (pt.hosts[a] && pt.hosts[b]);
            let through = arm_a.line == arm_b.line && arm_a.at == arm_b.at;
            let open = if same { sector >= PI - STEP } else { sector >= PI };
            if wa && wb && !deck && (open || round_of.is_some()) {
                if open {
                    if !through {
                        for (arm, side) in [(arm_a, sa), (arm_b, sb)] {
                            cuts[key(arm, side)].push(Cut { v: arm.at as f64, dir: arm.dir, tip: Tip::Join, to: None });
                        }
                        joins.push((pt.pos, [(arm_a.line, sa, arm_a), (arm_b.line, sb, arm_b)]));
                    }
                    continue;
                }
                let t = round_of.unwrap();
                // A ring: each arm's slab ends `INSET` units past its foot, inside its half of the ring.
                let mut w = [0.0; 2];
                for (n, (arm, side)) in [(arm_a, sa), (arm_b, sb)].into_iter().enumerate() {
                    let line = &lines[arm.line];
                    let foot = moved(line, t.x[n], -f64::from(arm.dir), INSET, arm.at as f64);
                    let ends = rule_ends(ctx, line, &pieces[key(arm, side)], side);
                    let q = beside(line, t.x[n], side, at_x(line, t.x[n]).1);
                    w[n] = units(width(q, &ends, full_m(line), k, r, mpu(q), false), q);
                    cuts[key(arm, side)].push(Cut { v: arm.at as f64, dir: arm.dir, tip: Tip::Square, to: Some(foot) });
                }
                let c = [f64::from(pt.pos[0]) + t.c[0], f64::from(pt.pos[1]) + t.c[1]];
                let halves = ring_halves(c, t.rho, t.a0, t.sweep, w);
                for ((arm, side), half) in [(arm_a, sa), (arm_b, sb)].into_iter().zip(halves) {
                    push_positive(half, &mut rings[key(arm, side)]);
                }
                continue;
            }
            for (n, arm, side, other, walking) in [(a, arm_a, sa, arm_b, wa), (b, arm_b, sb, arm_a, wb)] {
                if !walking {
                    continue;
                }
                let run = if same { None } else { run_in(ctx, lines, pt, n, side, other) };
                let to = match run {
                    None => None,
                    Some(Run::Own(x)) => Some((Tip::Square, x)),
                    Some(Run::Partner(p, s, x)) => {
                        onto.push((key(arm, side), arm, p, s, x));
                        Some((Tip::Square, arm.at as f64))
                    }
                };
                let (tip, to) = to.map_or((Tip::Taper, None), |(tip, x)| (tip, Some(x)));
                cuts[key(arm, side)].push(Cut { v: arm.at as f64, dir: arm.dir, tip, to });
            }
        }
    }
    // A slab ends with tapers before the strokes that it must not enter: a deck's where it leaves the ground, and a
    // rail's. Each run of a line side's sections that enters one is cut out toward both ends.
    let mut forbidden: Vec<Stroke> = lines.iter().filter(|l| l.rail).map(Stroke::of).collect();
    for (_, strokes) in decks {
        for s in strokes {
            if !forbidden.iter().any(|f| f.pts == s.pts) {
                forbidden.push(s);
            }
        }
    }
    for (l, line) in
        lines.iter().enumerate().filter(|&(l, _)| !pieces[2 * l].is_empty() || !pieces[2 * l + 1].is_empty())
    {
        // The strokes whose box meets the line's, grown by its slabs' reach.
        let g = line.h.iter().copied().fold(0.0, f64::max) + ctx.units(full_m(line), line.p[0]);
        let bx = line.p.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
            let p = [f64::from(p[0]), f64::from(p[1])];
            [b[0].min(p[0] - g), b[1].min(p[1] - g), b[2].max(p[0] + g), b[3].max(p[1] + g)]
        });
        let near: Vec<&Stroke> = (forbidden.iter())
            .filter(|s| s.bx[0] <= bx[2] && s.bx[2] >= bx[0] && s.bx[1] <= bx[3] && s.bx[3] >= bx[1])
            .collect();
        for side in [1.0, -1.0].into_iter().filter(|_| !near.is_empty()) {
            let n = side_of(l, side);
            for [a, b] in pieces[n].iter().flat_map(|p| entering(ctx, line, side, &near, p.x)).collect::<Vec<_>>() {
                let v = 0.5 * (a + b);
                cuts[n].push(Cut { v, dir: -1, tip: Tip::Taper, to: Some(a) });
                cuts[n].push(Cut { v, dir: 1, tip: Tip::Taper, to: Some(b) });
            }
        }
    }
    for (p, list) in pieces.iter_mut().zip(&cuts) {
        apply(p, list);
    }
    // The width rules: each side's taper tips; at a join to a narrower way, that way's width; and the rules of the
    // slab beyond a join within a ramp's reach.
    let own: Ends = (pieces.iter().enumerate())
        .map(|(n, p)| rule_ends(ctx, &lines[n / 2], p, if n % 2 == 1 { 1.0 } else { -1.0 }))
        .collect();
    let mut ends = own.clone();
    for (pos, sides) in &joins {
        let v = [f64::from(pos[0]), f64::from(pos[1])];
        for n in 0..2 {
            let (l, side, arm) = sides[n];
            let (m, other_side, _) = sides[1 - n];
            let (full, other_full) = (full_m(&lines[l]), full_m(&lines[m]));
            let mut add = Vec::new();
            if other_full < full {
                let into = dir_in(&lines[l], arm.at as f64, f64::from(arm.dir));
                let across = lines[l].h[arm.at] / units(1.0, v) + full;
                add.push(End::new(v, into, side * f64::from(arm.dir) > 0.0, other_full, across));
            }
            let reach = units(rules.reach_m(full.max(other_full)), v);
            for e in &own[side_of(m, other_side)] {
                let d = [v[0] - f64::from(e.at[0]), v[1] - f64::from(e.at[1])];
                if libm::hypot(d[0], d[1]) <= reach && d[0] * e.dir[0] + d[1] * e.dir[1] >= 0.0 {
                    add.push(*e);
                }
            }
            ends[side_of(l, side)].extend(add);
        }
    }
    // The strokes of the pieces, the joins' pies, the run-ins past their point and the rings. A back edge's width is
    // the width at its kerb point across the stroke's segment there (`u`), so a tip at a vertex where the line turns
    // takes its own rule.
    let back = |l: usize, side: f64, x: f64, u: [f64; 2]| {
        let line = &lines[l];
        let (c, h, _) = at_x(line, x);
        let n = left(u);
        let q = [c[0] + side * h * n[0], c[1] + side * h * n[1]];
        h + units(width(q, &ends[side_of(l, side)], full_m(line), k, r, mpu(q), false), q)
    };
    for (n, list) in pieces.iter().enumerate() {
        let (l, side) = (n / 2, if n % 2 == 1 { 1.0 } else { -1.0 });
        let line = &lines[l];
        for piece in list {
            let q0 = at_x(line, piece.x[0]).0;
            let xs = piece_points(line, side, &ends[n], full_m(line), ramp, mpu(q0), piece.x);
            if xs.len() < 2 {
                continue;
            }
            let dirs = segment_dirs(line, &xs);
            let outer = xs.iter().enumerate().map(|(j, &x)| back(l, side, x, dirs[j.min(dirs.len() - 1)])).collect();
            slab_stroke(line, &xs, side, outer, &mut rings[n]);
        }
    }
    // Each side's end normal and back edge at a point; a pie fills the gap on the outer side of two lines' ends.
    let edge = |(l, side, arm): (usize, f64, Arm)| {
        let line = &lines[l];
        let seg = if arm.dir > 0 { arm.at } else { arm.at.saturating_sub(1) };
        let u = unit(line.p[seg], line.p[(seg + 1).min(line.p.len() - 1)]).unwrap_or([1.0, 0.0]);
        let n = left(u);
        ([side * n[0], side * n[1]], back(l, side, arm.at as f64, u))
    };
    let pie = |pos: P, (na, da): ([f64; 2], f64), (nb, db): ([f64; 2], f64)| {
        let turn = libm::atan2(na[0] * nb[1] - na[1] * nb[0], na[0] * nb[0] + na[1] * nb[1]);
        let mut out = Vec::new();
        if turn > 0.0 {
            pie_between(pos, da, db, na, nb, turn, [&[], &[]], &mut out);
        } else if turn < 0.0 {
            pie_between(pos, db, da, nb, na, -turn, [&[], &[]], &mut out);
        }
        out
    };
    // A pie joins two pieces that both still end at the point.
    let joined = |(l, side, arm): (usize, f64, Arm)| {
        let x = arm.at as f64;
        pieces[side_of(l, side)].iter().any(|p| (0..2).any(|k| p.tip[k] == Tip::Join && p.x[k] == x))
    };
    for (pos, sides) in joins.iter().filter(|(_, sides)| sides.iter().all(|&s| joined(s))) {
        let fill = pie(*pos, edge(sides[0]), edge(sides[1]));
        for (l, side, _) in sides {
            rings[side_of(*l, *side)].extend(fill.iter().cloned());
        }
    }
    for &(n, arm, p, s, x) in &onto {
        // The slab goes on past the point along the partner at its full width, to where the other road cuts it.
        let (l, side) = (n / 2, if n % 2 == 1 { 1.0 } else { -1.0 });
        let partner = &lines[p.line];
        let full = full_m(&lines[l]);
        let (lo, hi) = ((p.at as f64).min(x), (p.at as f64).max(x));
        let mut xs = vec![lo];
        xs.extend((libm::floor(lo) as usize + 1..libm::ceil(hi) as usize).map(|i| i as f64));
        xs.push(hi);
        let outer = xs.iter().map(|&v| at_x(partner, v).1 + units(full, at_x(partner, v).0)).collect();
        slab_stroke(partner, &xs, s, outer, &mut rings[n]);
        let pos = partner.p[p.at];
        let start = partner.h[p.at] + units(full, at_x(partner, p.at as f64).0);
        let at_partner = (edge((p.line, s, p)).0, start);
        rings[n].extend(pie(pos, edge((l, side, arm)), at_partner));
    }
    let rank = ctx.rules.plan.rank(Rank::Sidewalk);
    let cands = (rings.into_iter().enumerate())
        .filter(|(_, r)| !r.is_empty())
        .map(|(n, rings)| {
            let (l, s) = (n / 2, (n % 2) as u8);
            let line = &lines[l];
            let label = Label {
                owner: Owner::Terrain,
                surface: Surface::Sidewalk,
                rule: HeightRule::KerbOffset,
                material: Some(line.way),
                rank,
                reach_m: line.sec.reach_m as f32,
                ..TERRAIN
            };
            Cand {
                rings: union(&rings, [0, 0]),
                label,
                key: (rank, line.sec.rank as u8, s),
                glob: None,
                line: l,
                relabel: None,
                beyond: Vec::new(),
                fill: None,
                discs: Vec::new(),
            }
        })
        .collect();
    Walk { cands, ends }
}

#[cfg(test)]
mod tests {
    use crate::faces::check::sidewalk_on_decks;
    use crate::faces::heights::check::terrain_at;
    use crate::faces::heights::{height_at, height_at_sides, kerb, Step};
    use crate::faces::records::{self, find, label};
    use crate::faces::tests::{at, cell, input_of, line, plane, road, Line};
    use crate::faces::{Rules, Surface};
    use crate::graph::build;
    use crate::CellInput;

    struct Built {
        words: Vec<u32>,
        input: CellInput,
    }

    fn built(lines: &[Line]) -> Built {
        let rules = Rules::embedded();
        let mut input = input_of(cell(), lines);
        input.terrain = plane(cell(), &|_, _| 500.0);
        let graph = build(cell(), &input, &rules.sections);
        let words = records::build(&graph, &input, rules, cell(), 0, true).near.words;
        Built { words, input }
    }

    impl Built {
        fn q(&self, m: [f64; 2]) -> [f64; 2] {
            let (g, o) = (at(m), cell().origin());
            [(g.x - o.x) as f64, (g.y - o.y) as f64]
        }

        fn surface(&self, m: [f64; 2]) -> Option<u32> {
            find(&self.words, self.q(m)).map(|id| label(&self.words, id)[0] & 0xff)
        }

        fn walks(&self, m: [f64; 2]) -> bool {
            self.surface(m) == Some(Surface::Sidewalk as u32)
        }

        fn h(&self, m: [f64; 2]) -> f64 {
            let q = self.q(m);
            let t = terrain_at(&self.input.terrain, cell(), q).unwrap();
            height_at(&self.words, q, find(&self.words, q), t).h
        }

        /// The kerb's rise at m read with the sidewalk label found at `on`.
        fn rise(&self, m: [f64; 2], on: [f64; 2]) -> f64 {
            kerb::rise_at(&self.words, self.q(m), find(&self.words, self.q(on)))
        }
    }

    /// A residential road east from x0 with a sidewalk on its south (the bake's right: bit 2) up to x = 0.
    fn walking(points: &[[f64; 2]], bits: &[u8]) -> Line {
        let mut l = road(points);
        l.walk = bits.to_vec();
        l
    }

    const KERB_M: f64 = 0.12;
    const H: f64 = 2.5;
    const W: f64 = 1.5;

    #[test]
    fn a_sidewalk_whose_bit_ends_tapers_to_its_kerb_which_stays_straight() {
        let c = built(&[walking(&[[-300.0, 0.0], [0.0, 0.0], [300.0, 0.0]], &[2, 0, 0])]);
        let walk = Rules::embedded().plan.walk;
        let full = -(walk.reach_m(W));
        // Full width, and the full kerb, a ramp's reach before the tip.
        assert!(c.walks([full, H + W - 0.05]) && c.walks([full, H + 0.05]));
        assert!((c.h([full, H + 0.5]) - c.h([full, H - 0.05]) - KERB_M).abs() < 1e-3);
        // The kerb stays straight through the taper and past its tip.
        for x in [-20.0, full, -1.0, -0.2, 0.5, 20.0] {
            assert_eq!(c.surface([x, H - 0.05]), Some(Surface::Carriageway as u32), "{x}");
            assert!(!c.walks([x, -H - 0.2]), "{x}: no sidewalk on the north");
        }
        assert!(!c.walks([0.5, H + 0.1]) && !c.walks([-0.3, H + 0.6]));
        // No width and no rise at the tip; the rise grows with the width.
        let on = [full, H + 0.5];
        assert!(c.rise([0.0, H + 0.01], on).abs() < 1e-3);
        assert!(c.rise([-0.5, H + 0.01], on) < c.rise([-1.5, H + 0.01], on));
        assert!((c.rise([full, H + 0.01], on) - KERB_M).abs() < 1e-6);
    }

    #[test]
    fn a_joint_bent_five_degrees_keeps_one_slab_on_both_sides_with_its_full_kerb() {
        let a = 5f64.to_radians();
        let end = [100.0 * libm::cos(a), 100.0 * libm::sin(a)];
        let c = built(&[walking(&[[-100.0, 0.0], [0.0, 0.0]], &[3, 0]), walking(&[[0.0, 0.0], end], &[3, 0])]);
        // Across the joint on both sides, along the bisector's normal at the back edge less 1 cm.
        let n = [-libm::sin(a / 2.0), libm::cos(a / 2.0)];
        for side in [1.0, -1.0] {
            for d in [H + 0.3, H + W - 0.01] {
                let m = [side * d * n[0], side * d * n[1]];
                assert!(c.walks(m), "side {side}, {d} m out: {:?}", c.surface(m));
            }
            let m = [side * (H + W / 2.0) * n[0], side * (H + W / 2.0) * n[1]];
            assert!((c.rise(m, m) - KERB_M).abs() < 1e-6, "side {side}: no taper at the joint");
        }
    }

    #[test]
    fn a_t_where_both_sides_walk_carries_the_slab_round_its_corner() {
        // A road east to west walks on its south; one south from it walks on its east (the bake's left: bit 1).
        let main = walking(&[[-100.0, 0.0], [0.0, 0.0], [100.0, 0.0]], &[2, 2, 0]);
        let side = walking(&[[0.0, 0.0], [0.0, 100.0]], &[1, 0]);
        let c = built(&[main, side]);
        let sections = &Rules::embedded().sections;
        let rho = sections.way("road", &[("highway".into(), "residential".into())].into(), false).corner_radius_walk_m;
        // The round's centre lies a radius off both kerbs; the ring between it and the kerb, either side of the
        // bisector, and its rise is the kerb's.
        let c0 = H + rho;
        for deg in [10.0, 30.0, 44.0, 45.0, 46.0, 60.0, 80.0] {
            let a = f64::to_radians(180.0 + deg);
            let m = [c0 + (rho - W / 2.0) * libm::cos(a), c0 + (rho - W / 2.0) * libm::sin(a)];
            assert!(c.walks(m), "{deg}°: {:?}", c.surface(m));
            assert!((c.rise(m, m) - KERB_M).abs() < 1e-6, "{deg}°");
        }
    }

    #[test]
    fn a_t_where_one_side_walks_runs_the_slab_at_full_width_to_the_other_road() {
        let main = walking(&[[-100.0, 0.0], [0.0, 0.0], [100.0, 0.0]], &[2, 2, 0]);
        let c = built(&[main, road(&[[0.0, 0.0], [0.0, 100.0]])]);
        for x in [-H - 0.1, H + 0.1] {
            assert!(c.walks([x, H + W - 0.05]), "{x}: the back edge at full width");
        }
        assert_eq!(c.surface([0.0, H + W / 2.0]), Some(Surface::Carriageway as u32));
        // No slab in the sector past the back edge.
        for m in [[H + 1.0, H + W + 0.3], [H + 0.2, H + 5.0], [H + 3.0, H + 3.0]] {
            assert!(!c.walks(m), "{m:?}");
        }
    }

    #[test]
    fn a_slab_runs_into_a_side_road_past_the_point_on_the_through_partner_line() {
        // The through road is split at the junction; the side road leans east, so it crosses the west slab's middle
        // line past the point, on the east line.
        let west = walking(&[[-100.0, 0.0], [0.0, 0.0]], &[2, 0]);
        let east = walking(&[[0.0, 0.0], [100.0, 0.0]], &[2, 0]);
        let c = built(&[west, east, road(&[[0.0, 0.0], [40.0, 100.0]])]);
        for m in [[-1.6, H + W - 0.05], [-2.2, H + 0.3], [-3.0, H + W - 0.05]] {
            assert!(c.walks(m), "{m:?}: the back edge at full width");
            assert!((c.rise(m, m) - KERB_M).abs() < 1e-6, "{m:?}: the full kerb");
        }
    }

    #[test]
    fn a_side_whose_bits_start_short_of_a_junction_runs_on_to_it() {
        // The south side walks up to 3 m before the junction; the side road leaves it to the south.
        let main = walking(&[[-100.0, 0.0], [-3.0, 0.0], [0.0, 0.0], [100.0, 0.0]], &[2, 0, 0, 0]);
        let c = built(&[main, road(&[[0.0, 0.0], [0.0, 100.0]])]);
        for m in [[-H - 0.3, H + W - 0.05], [-H - 0.3, H + 0.3]] {
            assert!(c.walks(m), "{m:?}: the slab at full width at the side road");
            assert!((c.rise(m, m) - KERB_M).abs() < 1e-6, "{m:?}: the full kerb");
        }
    }

    #[test]
    fn a_slab_tapers_before_a_rail_arm() {
        let main = walking(&[[-100.0, 0.0], [0.0, 0.0], [100.0, 0.0]], &[2, 2, 0]);
        let rail = line("railway", &[("railway", "rail")], &[[0.0, 0.0], [0.0, 100.0]]);
        let c = built(&[main, rail]);
        let sections = &Rules::embedded().sections;
        let half = sections.way("railway", &[("railway".into(), "rail".into())].into(), false).half_m;
        // Its tip lies at the rail's stroke, and the ramp starts there.
        assert!(!c.walks([-(half + 0.5), H + 0.5]) && c.walks([-(half + 0.5), H + 0.05]));
        assert!(c.walks([-(half + Rules::embedded().plan.walk.reach_m(W)), H + W - 0.05]));
    }

    #[test]
    fn a_slab_at_a_junction_where_a_deck_starts_stays_off_the_deck() {
        let deck =
            line("road", &[("highway", "residential"), ("bridge", "yes"), ("deck", "1")], &[[0.0; 2], [60.0, 0.0]]);
        let main = walking(&[[-100.0, 0.0], [0.0, 0.0]], &[3, 0]);
        let side = walking(&[[0.0, 0.0], [0.0, 100.0]], &[3, 0]);
        let lines = [main, side, deck];
        let rules = Rules::embedded();
        let input = input_of(cell(), &lines);
        let graph = build(cell(), &input, &rules.sections);
        let plan = crate::faces::build(&graph, &input, rules, cell(), 0, true);
        assert!(plan.faces.iter().any(|f| f.surface == Surface::Sidewalk));
        assert_eq!(sidewalk_on_decks(&plan, &graph, &input, rules), Vec::<[i32; 2]>::new());
    }

    #[test]
    fn a_straight_kerb_is_the_kerb_step() {
        let c = built(&[walking(&[[-300.0, 0.0], [300.0, 0.0]], &[2, 0])]);
        let (q, a, b) = (c.q([0.0, H]), c.q([0.0, H + 0.1]), c.q([0.0, H - 0.1]));
        let labels = [find(&c.words, a), find(&c.words, b)];
        let t = terrain_at(&c.input.terrain, cell(), q).unwrap();
        let sides = height_at_sides(&c.words, q, labels, t);
        assert_eq!(sides[0].step, Step::Kerb);
        assert!((sides[0].h - sides[1].h - KERB_M).abs() < 1e-3);
    }
}
