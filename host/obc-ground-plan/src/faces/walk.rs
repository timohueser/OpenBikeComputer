//! The sidewalks: per way line and side that the bake's `walk` bits mark, a slab from inside the carriageway out
//! to the class's sidewalk width past the kerb. The kerb is the carriageway's edge and stays straight; only the
//! back edge moves. At a graph point a slab goes on into the next road's slab through an open sector, follows the
//! kerb round a two-foot round where both arms walk (a ring), runs at full width into another road, whose
//! carriageway cuts it, or tapers against nothing: its width falls along a ramp with round kinks to 0 at its tip
//! (`ramp`). Stations are the records' metres along, so two cells build the same slabs.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use super::geom::P;
use super::points::{Arm, Rounded, TwoFoot, INSET};
use super::stroke::{left, pie_between, push_positive, side_along, union, unit, Rings, STEPS};
use super::{Cand, Ctx, Label, Owner, Point, Rank, WayLine, TERRAIN, VERGE};
use crate::faces::{HeightRule, Surface};
use crate::graph::{Level, WayGraph};
use crate::CellInput;

/// A width rule of a line side: from metres along `at` (as the records' samples) toward `dir` (+1: more metres
/// along), the slab rises from `target` metres (0 at a taper's tip) to its full width (`ramp`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct End {
    pub at: f64,
    pub dir: f64,
    pub target: f64,
}

/// Per line side with a slab (its line, and 1 for the stroke's left normal, 0 for its right), its width rules.
pub(crate) type Ends = BTreeMap<(usize, u8), Vec<End>>;

#[derive(Default)]
pub(super) struct Walk {
    pub cands: Vec<Cand>,
    pub ends: Ends,
}

/// One arc step: a join this close to half a turn has no gap worth a round, and the strokes mitre it.
const STEP: f64 = 2.0 * PI / STEPS as f64;
/// A deck's stroke is searched for over a way's sections this far apart, then to a hundredth of it.
const DECK_STEP_M: f64 = 0.5;

/// The width `x` metres past a rule's station toward its slab: a straight ramp at slope `k` from `target` to
/// `full`, its two kinks rounded with radius `r` (less where the two rounds would meet), so that it leaves the
/// target at the station and meets the full width tangentially.
pub(crate) fn ramp(x: f64, target: f64, full: f64, k: f64, r: f64) -> f64 {
    let rise = full - target;
    if rise <= 0.0 {
        return full;
    }
    let phi = libm::atan(k);
    let (s, c) = (libm::sin(phi), libm::cos(phi));
    let r = r.min(rise * (1.0 + c) / (2.0 * s * s));
    let t = r * s / (1.0 + c);
    let top = t + rise / k;
    let x = x.max(0.0);
    if x <= r * s {
        target + r - (r * r - x * x).max(0.0).sqrt()
    } else if x <= top - t * c {
        target + k * (x - t)
    } else if x < top + t {
        let y = x - top - t;
        full - r + (r * r - y * y).max(0.0).sqrt()
    } else {
        full
    }
}

/// The places of a ramp's rounds in steps of at most one arc step of turn, metres past its station: a stroke
/// through them follows the rounds.
fn ramp_points(target: f64, full: f64, k: f64, r: f64) -> Vec<f64> {
    let rise = full - target;
    if rise <= 0.0 {
        return Vec::new();
    }
    let phi = libm::atan(k);
    let (s, c) = (libm::sin(phi), libm::cos(phi));
    let r = r.min(rise * (1.0 + c) / (2.0 * s * s));
    let t = r * s / (1.0 + c);
    let top = t + rise / k;
    let n = libm::ceil(phi / STEP).max(1.0) as usize;
    let mut out = Vec::with_capacity(2 * n + 2);
    for j in 0..=n {
        let a = phi * j as f64 / n as f64;
        out.push(r * libm::sin(a));
        out.push(top + t - r * libm::sin(a));
    }
    out
}

/// The width at metres along `at` of a side of full width `full` with the rules `ends`: the least ramp whose slab
/// side holds it. With `tips`, of the taper tips alone: the kerb's rise, which a narrower way beyond leaves.
pub(crate) fn width(at: f64, ends: &[End], full: f64, k: f64, r: f64, tips: bool) -> f64 {
    (ends.iter())
        .filter(|e| (at - e.at) * e.dir >= 0.0 && (!tips || e.target == 0.0))
        .map(|e| ramp((at - e.at).abs(), e.target, full, k, r))
        .fold(full, f64::min)
}

/// A line's metres along at each vertex, as its record's samples hold them (`records::line_words`): its graph
/// samples' and, between them, by share of the chord.
pub(crate) fn alongs(graph: &WayGraph, line: &WayLine) -> Vec<f64> {
    let way = &graph.ways[line.way as usize];
    let samples = &graph.samples[way.samples.start as usize..way.samples.end as usize];
    let mut out = vec![0.0; line.p.len()];
    for (k, win) in line.index.windows(2).enumerate() {
        let (a, b) = (line.p[win[0]], line.p[win[1]]);
        let len = libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])).max(1e-9);
        for (j, v) in out.iter_mut().enumerate().take(win[1]).skip(win[0]) {
            let p = line.p[j];
            let x = libm::hypot(f64::from(p[0] - a[0]), f64::from(p[1] - a[1])) / len;
            *v = samples[k].along_m + x * (samples[k + 1].along_m - samples[k].along_m);
        }
    }
    if let Some(&last) = line.index.last() {
        out[last] = samples[line.index.len() - 1].along_m;
    }
    out
}

/// The sidewalk width of a line's class; 0 for a class with none, water and rails.
fn full_m(line: &WayLine) -> f64 {
    if line.sec.water || line.rail {
        0.0
    } else {
        line.sec.sidewalk_m
    }
}

/// The `walk` bit of a stroke side: 2 on its left normal (the bake's right), 1 on its right.
fn bit(side: f64) -> u8 {
    if side > 0.0 {
        2
    } else {
        1
    }
}

/// Whether segment i of a line has a sidewalk on stroke side `side`.
fn walks_seg(line: &WayLine, i: usize, side: f64) -> bool {
    full_m(line) > 0.0 && i + 1 < line.p.len() && line.walk[i] & bit(side) != 0
}

/// Whether an arm's first segment has a sidewalk on its right, seen from the point, else on its left.
pub(super) fn walks(line: &WayLine, arm: Arm, right: bool) -> bool {
    let side = if (arm.dir > 0) == right { 1.0 } else { -1.0 };
    let seg = if arm.dir > 0 { Some(arm.at) } else { arm.at.checked_sub(1) };
    seg.is_some_and(|s| walks_seg(line, s, side))
}

/// A line's point and half-width at fractional vertex index x, and its segment's unit direction.
fn at_x(line: &WayLine, x: f64) -> ([f64; 2], f64, [f64; 2]) {
    let n = line.p.len();
    let i = (libm::floor(x).max(0.0) as usize).min(n.saturating_sub(2));
    let t = x - i as f64;
    let (a, b) = (line.p[i], line.p[(i + 1).min(n - 1)]);
    let p = [f64::from(a[0]) + t * f64::from(b[0] - a[0]), f64::from(a[1]) + t * f64::from(b[1] - a[1])];
    let h = line.h[i] + t * (line.h[(i + 1).min(n - 1)] - line.h[i]);
    (p, h, unit(a, b).unwrap_or([1.0, 0.0]))
}

fn along_x(along: &[f64], x: f64) -> f64 {
    let i = (libm::floor(x).max(0.0) as usize).min(along.len().saturating_sub(2));
    along[i] + (x - i as f64) * (along[(i + 1).min(along.len() - 1)] - along[i])
}

/// The fractional vertex index at metres along `a` on a line whose metres along change monotonically.
fn x_along(along: &[f64], a: f64) -> Option<f64> {
    along.windows(2).enumerate().find_map(|(i, w)| {
        let (lo, hi) = (w[0].min(w[1]), w[0].max(w[1]));
        (lo <= a && a <= hi && w[1] != w[0]).then(|| i as f64 + (a - w[0]) / (w[1] - w[0]))
    })
}

/// The fractional vertex index `d` units from x toward `dir` along a line, at most to `stop`.
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

/// What ends a piece at a point on an arm's side: its tip there, and where it moves the end from the point.
struct Cut {
    v: usize,
    dir: i8,
    tip: Tip,
    to: Option<f64>,
}

/// Each line side's pieces after the cuts: a cut at vertex v toward `dir` splits the piece that holds the segment
/// there and ends the part on that side with its tip, moved to `to`.
fn apply(pieces: &mut Vec<Piece>, cuts: &[Cut]) {
    for c in cuts {
        let (v, d) = (c.v as f64, f64::from(c.dir));
        let Some(k) = pieces.iter().position(|p| p.x[0] <= v.min(v + d) && p.x[1] >= v.max(v + d)) else { continue };
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
/// the bisector, whose points they share.
fn ring_halves(c: [f64; 2], rho: f64, a0: f64, sweep: f64, w: [f64; 2]) -> [Vec<P>; 2] {
    let at = |a: f64, r: f64| round([c[0] + r * libm::cos(a), c[1] + r * libm::sin(a)]);
    let inner = |a: f64| (rho - w[0] - (w[1] - w[0]) * (a - a0) / sweep).max(0.0);
    let mid = a0 + sweep / 2.0;
    let bisector = [at(mid, rho + INSET), at(mid, inner(mid))];
    let half = |from: f64, to: f64, start: bool| {
        let mut angles = vec![from];
        angles.extend(super::points::arc_angles(from, to - from));
        angles.push(to);
        let mut ring: Vec<P> = angles.iter().map(|&a| at(a, rho + INSET)).collect();
        ring.extend(angles.iter().rev().map(|&a| at(a, inner(a))));
        // The bisector's points, made once for both halves.
        let k = if start { angles.len() - 1 } else { 0 };
        ring[k] = bisector[0];
        let j = ring.len() - 1 - k;
        ring[j] = bisector[1];
        ring
    };
    [half(a0, mid, true), half(mid, a0 + sweep, false)]
}

/// The stations where a stroke of a side turns along its ramps: the rules' round points within the piece.
fn piece_points(along: &[f64], ends: &[End], full: f64, k: f64, r: f64, x: [f64; 2]) -> Vec<f64> {
    let mut out: Vec<f64> = vec![x[0], x[1]];
    out.extend((libm::floor(x[0]) as usize + 1..=libm::ceil(x[1]) as usize).map(|i| i as f64));
    for e in ends {
        for d in ramp_points(e.target, full, k, r) {
            out.extend(x_along(along, e.at + e.dir * d));
        }
    }
    out.retain(|&v| v >= x[0] && v <= x[1]);
    out.sort_by(f64::total_cmp);
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    out
}

/// The distance between segments ab and cd.
fn seg_dist(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> f64 {
    let cross = |o: [f64; 2], p: [f64; 2], q: [f64; 2]| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    let (d1, d2, d3, d4) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
    if d1 * d2 < 0.0 && d3 * d4 < 0.0 {
        return 0.0;
    }
    let to = |p: [f64; 2], s: [f64; 2], e: [f64; 2]| {
        let (u, v) = ([e[0] - s[0], e[1] - s[1]], [p[0] - s[0], p[1] - s[1]]);
        let l2 = u[0] * u[0] + u[1] * u[1];
        let t = if l2 > 0.0 { ((u[0] * v[0] + u[1] * v[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        libm::hypot(v[0] - t * u[0], v[1] - t * u[1])
    };
    to(a, c, d).min(to(b, c, d)).min(to(c, a, b)).min(to(d, a, b))
}

/// The first crossing of polyline `path` with segment cd: its place along the path (segment index and fraction)
/// and its fraction along cd.
fn crossing(path: &[[f64; 2]], c: [f64; 2], d: [f64; 2]) -> Option<(f64, f64)> {
    path.windows(2).enumerate().find_map(|(k, w)| {
        let (a, b) = (w[0], w[1]);
        let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
        let den = r[0] * s[1] - r[1] * s[0];
        if den == 0.0 {
            return None;
        }
        let (qx, qy) = (c[0] - a[0], c[1] - a[1]);
        let (t, u) = ((qx * s[1] - qy * s[0]) / den, (qx * r[1] - qy * r[0]) / den);
        ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((k as f64 + t, u))
    })
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
    let (k, r) = (rules.end_slope, rules.end_round_m);
    let along: Vec<Vec<f64>> =
        lines.iter().map(|l| if full_m(l) > 0.0 { alongs(ctx.graph, l) } else { Vec::new() }).collect();
    // The way's metres along grow toward its vertices' order, or fall.
    let sign = |l: usize| {
        let a = &along[l];
        if a.len() > 1 && a[a.len() - 1] < a[0] {
            -1.0
        } else {
            1.0
        }
    };
    let sides = [(1.0, 1u8), (-1.0, 0u8)];
    // The pieces from the bits, inside the read box: a run of segments with the bit; a taper at each end, but at
    // the box's edge.
    let mut pieces: BTreeMap<(usize, u8), Vec<Piece>> = BTreeMap::new();
    for (l, line) in lines.iter().enumerate() {
        let Some((first, last)) = runs[l].filter(|_| full_m(line) > 0.0) else { continue };
        for (side, s) in sides {
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
                pieces.entry((l, s)).or_default().push(Piece { x: [i as f64, j as f64], tip: [lo, hi] });
                i = j;
            }
        }
    }
    if pieces.is_empty() {
        return Walk::default();
    }
    let decks = deck_points(ctx, input, lines, bx);
    let units = |m: f64, p: [f64; 2]| ctx.units(m, round(p));
    let mut cuts: BTreeMap<(usize, u8), Vec<Cut>> = BTreeMap::new();
    let mut extra: BTreeMap<(usize, u8), Rings> = BTreeMap::new();
    // Joins between two lines: their point, and per side its line, side and arm.
    let mut joins: Vec<(P, [(usize, f64, Arm); 2])> = Vec::new();
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
                .map(|(n, &(a, b, _))| (rounded[pi].two_foot.iter().find(|t| t.pair == n), a, b))
                .collect()
        } else {
            rounded[pi].two_foot.iter().map(|t| (Some(t), t.arms[0], t.arms[1])).collect()
        };
        let deck = decks.iter().find(|d| d.0 == pt.pos).map(|d| &d.1);
        for (round_of, a, b) in pairs {
            let (arm_a, arm_b) = (pt.arms[a], pt.arms[b]);
            let (sa, sb) = (f64::from(arm_a.dir), -f64::from(arm_b.dir));
            let wa = walks(&lines[arm_a.line], arm_a, true);
            let wb = walks(&lines[arm_b.line], arm_b, false);
            if !wa && !wb {
                continue;
            }
            let key = |arm: Arm, side: f64| (arm.line, u8::from(side > 0.0));
            // At a point where a deck starts, a slab that would meet the deck's stroke ends before it.
            let cut_a = deck.and_then(|d| deck_cut(ctx, lines, arm_a, sa, d));
            let cut_b = deck.and_then(|d| deck_cut(ctx, lines, arm_b, sb, d));
            let sector = (heading(arm_b) - heading(arm_a)).rem_euclid(2.0 * PI);
            let same = !junction || (pt.hosts[a] && pt.hosts[b]);
            let through = arm_a.line == arm_b.line && arm_a.at == arm_b.at;
            let open = if same { sector >= PI - STEP } else { sector >= PI };
            if wa && wb && cut_a.is_none() && cut_b.is_none() && (open || round_of.is_some()) {
                if open {
                    if !through {
                        for (arm, side) in [(arm_a, sa), (arm_b, sb)] {
                            cuts.entry(key(arm, side)).or_default().push(Cut {
                                v: arm.at,
                                dir: arm.dir,
                                tip: Tip::Join,
                                to: None,
                            });
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
                    let ends = rule_ends(&pieces, &along, key(arm, side), sign(arm.line));
                    let m = width(along_x(&along[arm.line], t.x[n]), &ends, full_m(line), k, r, false);
                    w[n] = units(m, at_x(line, t.x[n]).0);
                    cuts.entry(key(arm, side)).or_default().push(Cut {
                        v: arm.at,
                        dir: arm.dir,
                        tip: Tip::Square,
                        to: Some(foot),
                    });
                }
                let c = [f64::from(pt.pos[0]) + t.c[0], f64::from(pt.pos[1]) + t.c[1]];
                let halves = ring_halves(c, t.rho, t.a0, t.sweep, w);
                for ((arm, side), half) in [(arm_a, sa), (arm_b, sb)].into_iter().zip(halves) {
                    push_positive(half, extra.entry(key(arm, side)).or_default());
                }
                continue;
            }
            for (arm, side, other, walking, cut) in [(arm_a, sa, arm_b, wa, cut_a), (arm_b, sb, arm_a, wb, cut_b)] {
                if !walking {
                    continue;
                }
                let to = match cut {
                    Some(x) => Some((Tip::Taper, x)),
                    None if same => None,
                    None => run_in(ctx, lines, pt, arm, side, other).map(|x| (Tip::Square, x)),
                };
                let (tip, to) = to.map_or((Tip::Taper, None), |(tip, x)| (tip, Some(x)));
                cuts.entry(key(arm, side)).or_default().push(Cut { v: arm.at, dir: arm.dir, tip, to });
            }
        }
    }
    for (key, list) in &cuts {
        if let Some(p) = pieces.get_mut(key) {
            apply(p, list);
        }
    }
    // The width rules: each side's taper tips; at a join to a narrower way, that way's width; and the rules of the
    // slab beyond a join within a ramp's reach, carried on past the join.
    let mut ends: Ends = pieces.keys().map(|&key| (key, rule_ends(&pieces, &along, key, sign(key.0)))).collect();
    for (_, sides) in &joins {
        for n in 0..2 {
            let (l, side, arm) = sides[n];
            let (m, other_side, other) = sides[1 - n];
            let (full, other_full) = (full_m(&lines[l]), full_m(&lines[m]));
            let into = sign(l) * f64::from(arm.dir);
            let at = along[l][arm.at];
            let mut add = Vec::new();
            if other_full < full {
                add.push(End { at, dir: into, target: other_full });
            }
            let beyond = along[m][other.at];
            let reach = rules.reach_m(full.max(other_full));
            let theirs = ends.get(&(m, u8::from(other_side > 0.0))).cloned().unwrap_or_default();
            for e in theirs {
                let d = (e.at - beyond).abs();
                if d <= reach && e.dir * (beyond - e.at) >= 0.0 {
                    add.push(End { at: at - into * d, dir: into, target: e.target });
                }
            }
            ends.entry((l, u8::from(side > 0.0))).or_default().extend(add);
        }
    }
    // The strokes of the pieces, the joins' pies and the extras.
    let mut rings: BTreeMap<(usize, u8), Rings> = extra;
    for (&(l, s), list) in &pieces {
        let line = &lines[l];
        let side = if s == 1 { 1.0 } else { -1.0 };
        let full = full_m(line);
        let e = &ends[&(l, s)];
        for piece in list {
            let xs = piece_points(&along[l], e, full, k, r, piece.x);
            if xs.len() < 2 {
                continue;
            }
            let (mut p, mut inner, mut outer, mut dirs) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for (n, &x) in xs.iter().enumerate() {
                let (q, h, _) = at_x(line, x);
                let w = width(along_x(&along[l], x), e, full, k, r, false);
                p.push(round(q));
                inner.push(h / 2.0);
                outer.push(h + units(w, q));
                if let Some(&next) = xs.get(n + 1) {
                    dirs.push(Some(at_x(line, (x + next) / 2.0).2));
                }
            }
            side_along(&p, &inner, &outer, &dirs, side, rings.entry((l, s)).or_default());
        }
    }
    for (pos, sides) in &joins {
        // Each side's end normal and back edge at the point; the pie fills the gap on the outer side of the join.
        let edge = |(l, side, arm): (usize, f64, Arm)| {
            let line = &lines[l];
            let seg = if arm.dir > 0 { arm.at } else { arm.at.saturating_sub(1) };
            let n = left(unit(line.p[seg], line.p[(seg + 1).min(line.p.len() - 1)]).unwrap_or([1.0, 0.0]));
            let e = ends.get(&(l, u8::from(side > 0.0))).map_or(&[][..], |e| &e[..]);
            let q = [f64::from(line.p[arm.at][0]), f64::from(line.p[arm.at][1])];
            let w = width(along[l][arm.at], e, full_m(line), k, r, false);
            ([side * n[0], side * n[1]], line.h[arm.at] + units(w, q))
        };
        let ((na, da), (nb, db)) = (edge(sides[0]), edge(sides[1]));
        let turn = libm::atan2(na[0] * nb[1] - na[1] * nb[0], na[0] * nb[0] + na[1] * nb[1]);
        let mut pie = Vec::new();
        if turn > 0.0 {
            pie_between(*pos, da, db, na, nb, turn, [&[], &[]], &mut pie);
        } else if turn < 0.0 {
            pie_between(*pos, db, da, nb, na, -turn, [&[], &[]], &mut pie);
        }
        for (l, side, _) in sides {
            rings.entry((*l, u8::from(*side > 0.0))).or_default().extend(pie.iter().cloned());
        }
    }
    let rank = ctx.rules.plan.rank(Rank::Sidewalk);
    let cands = (rings.into_iter())
        .filter(|(_, r)| !r.is_empty())
        .map(|((l, s), r)| {
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
                rings: union(&r, [0, 0]),
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
    ends.retain(|_, e| !e.is_empty());
    Walk { cands, ends }
}

/// A side's own width rules: a 0-target at each taper tip of its pieces, toward the piece.
fn rule_ends(pieces: &BTreeMap<(usize, u8), Vec<Piece>>, along: &[Vec<f64>], key: (usize, u8), sign: f64) -> Vec<End> {
    let mut out = Vec::new();
    for p in pieces.get(&key).into_iter().flatten() {
        for (n, dir) in [(0, sign), (1, -sign)] {
            if p.tip[n] == Tip::Taper {
                out.push(End { at: along_x(&along[key.0], p.x[n]), dir, target: 0.0 });
            }
        }
    }
    out
}

/// Where a slab on an arm's side runs into the other arm `other`: the fractional vertex index on its own line
/// where the other arm's centre line crosses the slab's middle line (half its width past the kerb), on the arm
/// or past the point; None where it does not within `extent_m`.
fn run_in(ctx: &Ctx, lines: &[WayLine], pt: &Point, arm: Arm, side: f64, other: Arm) -> Option<f64> {
    let line = &lines[arm.line];
    let reach = ctx.units(ctx.rules.plan.extent_m, pt.pos);
    let o = &lines[other.line];
    // The other arm's centre line from the point.
    let mut path: Vec<[f64; 2]> = Vec::new();
    let mut i = other.at;
    let mut length = 0.0;
    loop {
        let q = [f64::from(o.p[i][0]), f64::from(o.p[i][1])];
        if let Some(last) = path.last() {
            length += libm::hypot(q[0] - last[0], q[1] - last[1]);
        }
        path.push(q);
        let Some(j) = i.checked_add_signed(isize::from(other.dir)).filter(|&j| j < o.p.len()) else { break };
        if length >= reach {
            break;
        }
        i = j;
    }
    // The slab's middle line: per segment of the arm's line within `extent_m` of the point, both ways, and the
    // chord over the gap at each vertex between two segments' offsets.
    let mid = |j: usize| line.h[j] + ctx.units(full_m(line) / 2.0, line.p[j]);
    let near = |j: usize| {
        let (a, b) = (line.p[j], pt.pos);
        libm::hypot(f64::from(a[0] - b[0]), f64::from(a[1] - b[1])) <= reach
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
        // The first crossing along the other arm.
        if let Some(f) = found.filter(|f| best.is_none_or(|b| f.0 < b.0)) {
            best = Some(f);
        }
    }
    best.map(|(_, x)| x)
}

/// The points of the cell's ground level where a deck starts: per point, the strokes of its deck arms' ways (their
/// points and half-widths: paved layers and `edge_band_m`, units).
type DeckPoint = (P, Vec<(Vec<[f64; 2]>, Vec<f64>)>);

fn deck_points(ctx: &Ctx, input: &CellInput, lines: &[WayLine], bx: [i32; 4]) -> Vec<DeckPoint> {
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
    let decks = super::lines(ctx, input, &|w| wanted.contains(&w) && super::on_deck(g, w), bx).0;
    let band = ctx.rules.structures.edge_band_m;
    (found.into_iter())
        .map(|(pos, ways)| {
            let strokes = (decks.iter())
                .filter(|d| ways.contains(&d.way))
                .map(|d| {
                    let pts = d.p.iter().map(|p| [f64::from(p[0]), f64::from(p[1])]).collect();
                    let half = (0..d.p.len()).map(|i| d.bands[i][VERGE] + ctx.units(band, d.p[i])).collect();
                    (pts, half)
                })
                .collect();
            (pos, strokes)
        })
        .collect()
}

/// At a point where a deck starts, the farthest station of an arm's side within `extent_m` whose section, from
/// the kerb out to the full width, meets a deck's stroke: the slab ends there with a taper, which lies wholly in
/// sections that do not meet it. None where no section does.
fn deck_cut(ctx: &Ctx, lines: &[WayLine], arm: Arm, side: f64, decks: &[(Vec<[f64; 2]>, Vec<f64>)]) -> Option<f64> {
    let line = &lines[arm.line];
    let full = full_m(line);
    let dir = f64::from(arm.dir);
    let meets = |x: f64| {
        let (q, h, u) = at_x(line, x);
        let n = left(u);
        let w = h + ctx.units(full, round(q));
        let (a, b) =
            ([q[0] + side * h * n[0], q[1] + side * h * n[1]], [q[0] + side * w * n[0], q[1] + side * w * n[1]]);
        decks.iter().any(|(pts, half)| {
            (0..pts.len().saturating_sub(1)).any(|j| seg_dist(a, b, pts[j], pts[j + 1]) < half[j].max(half[j + 1]))
        })
    };
    let step = ctx.units(DECK_STEP_M, line.p[arm.at]);
    let reach = ctx.units(ctx.rules.plan.extent_m, line.p[arm.at]);
    let end = if dir > 0.0 { (line.p.len() - 1) as f64 } else { 0.0 };
    let (mut x, mut d, mut last) = (arm.at as f64, 0.0, None);
    while d <= reach {
        if meets(x) {
            last = Some(x);
        }
        if x == end {
            break;
        }
        x = moved(line, x, dir, step, end);
        d += step;
    }
    let mut lo = last?;
    let mut hi = moved(line, lo, dir, step, end);
    for _ in 0..7 {
        let mid = 0.5 * (lo + hi);
        if meets(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(hi)
}
