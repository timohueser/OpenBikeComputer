//! PROBE (GP4, throwaway): one-sided sidewalk bands from `walk`, linear end tapers, the ring of a
//! round where both facing sides walk, and a taper to the foot where only one does.

use super::geom::P;
use super::points::{angle, arc, sweep_toward, Arm, Feet, Rounded};
use super::stroke::{left, off, pie_between, push_positive, unit, Rings};
use super::WayLine;

/// A join that turns less than this is mitred (as the strokes).
/// The taper length.
const END_M: f64 = 3.0;
/// The radius of a one-walk corner.
pub const ONE_WALK_M: f64 = 0.8;

const MITRE: f64 = 2.0 * std::f64::consts::PI / 36.0;

/// The bit of a line side: +1 is the strokes' left normal (a quarter turn toward +y, the map's
/// right), which `walk` calls 2.
fn bit(side: i8) -> u8 {
    if side > 0 {
        2
    } else {
        1
    }
}

fn walks(line: &WayLine) -> bool {
    !line.rail && line.sec.sidewalk_m > 0.0
}

/// The look of the corner test: a facing side walks at a corner where its bit is set on a segment
/// that starts within this of the point (the bake clears the bits on the other way's bed).
const LOOK_M: f64 = 12.0;

/// Where the arm's side `arm_side` (+1: left of its outward direction) first walks within `look`
/// units of the point: the fractional index of that segment's near end.
pub fn arm_walk_at(lines: &[WayLine], arm: Arm, arm_side: i8, look: f64) -> Option<f64> {
    let line = &lines[arm.line];
    if !walks(line) {
        return None;
    }
    let side = arm_side * arm.dir;
    let n = line.p.len();
    let mut i = arm.at;
    let mut d = 0.0;
    loop {
        let seg = if arm.dir > 0 { i } else { i.checked_sub(1)? };
        if seg + 1 >= n {
            return None;
        }
        if line.walk[seg] & bit(side) != 0 {
            return Some(i as f64);
        }
        let j = if arm.dir > 0 { i + 1 } else { i - 1 };
        d += dist(line.p[i], line.p[j]);
        if d > look {
            return None;
        }
        i = j;
    }
}

pub fn arm_walk(lines: &[WayLine], arm: Arm, arm_side: i8, per_m: f64) -> bool {
    arm_walk_at(lines, arm, arm_side, LOOK_M * per_m).is_some()
}

/// The end of a run: its width and rise share at the end.
#[derive(Clone, Copy, Debug)]
struct End {
    w: f64,
}

const PLAIN: End = End { w: 0.0 };

fn point_at(line: &WayLine, x: f64) -> P {
    let i = (x.floor() as usize).min(line.p.len() - 1);
    let t = x - i as f64;
    if t <= 0.0 || i + 1 >= line.p.len() {
        return line.p[i];
    }
    let (a, b) = (line.p[i], line.p[i + 1]);
    let l = |u: i32, v: i32| libm::round(f64::from(u) + t * f64::from(v - u)) as i32;
    [l(a[0], b[0]), l(a[1], b[1])]
}

fn h_at(line: &WayLine, x: f64) -> f64 {
    let i = (x.floor() as usize).min(line.p.len() - 1);
    let t = x - i as f64;
    if t <= 0.0 || i + 1 >= line.p.len() {
        return line.h[i];
    }
    line.h[i] + t * (line.h[i + 1] - line.h[i])
}

/// The direction of the segment that holds the part of a run next to x: above x (`up`) or below.
fn seg_dir(line: &WayLine, x: f64, up: bool) -> [f64; 2] {
    let n = line.p.len();
    let seg = if up { (x.floor() as usize).min(n - 2) } else { ((x.ceil() as usize).max(1) - 1).min(n - 2) };
    let mut k = seg;
    // A zero segment takes the next one with a direction.
    loop {
        if let Some(u) = unit(line.p[k], line.p[k + 1]) {
            return u;
        }
        if k + 2 < n {
            k += 1;
        } else {
            return [1.0, 0.0];
        }
    }
}

fn dist(a: P, b: P) -> f64 {
    let (x, y) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    (x * x + y * y).sqrt()
}

/// The one-sided stroke of points `p` at offsets `d` on `side`, with segment directions `dirs`.
fn side_pieces(p: &[P], d: &[f64], dirs: &[[f64; 2]], side: f64, out: &mut Rings) {
    let n = p.len();
    if n < 2 {
        return;
    }
    let mut corners: Vec<[P; 2]> = (0..n - 1)
        .map(|i| {
            let nl = left(dirs[i]);
            let nn = [side * nl[0], side * nl[1]];
            [off(p[i], d[i], nn), off(p[i + 1], d[i + 1], nn)]
        })
        .collect();
    let mut pies = Vec::new();
    for i in 0..n - 2 {
        let (u1, u2) = (dirs[i], dirs[i + 1]);
        let c = u1[0] * u2[1] - u1[1] * u2[0];
        let dot = u1[0] * u2[0] + u1[1] * u2[1];
        let turn = libm::atan2(c, dot);
        let (n1, n2) = (left(u1), left(u2));
        if turn.abs() < MITRE && dot > 0.0 {
            let m = [side * (n1[0] + n2[0]) / (1.0 + dot), side * (n1[1] + n2[1]) / (1.0 + dot)];
            let q = off(p[i + 1], d[i + 1], m);
            corners[i][1] = q;
            corners[i + 1][0] = q;
            continue;
        }
        // The outer side of a turn toward the left normal is the right one.
        if (turn > 0.0 && side < 0.0) || (turn < 0.0 && side > 0.0) {
            pies.push((i + 1, turn, n1, n2));
        }
    }
    for i in 0..n - 1 {
        push_positive(vec![p[i], p[i + 1], corners[i][1], corners[i][0]], out);
    }
    for (v, turn, n1, n2) in pies {
        if side < 0.0 {
            let (a, b) = ([-n1[0], -n1[1]], [-n2[0], -n2[1]]);
            pie_between(p[v], d[v], d[v], a, b, turn.abs(), [&[], &[]], out);
        } else {
            pie_between(p[v], d[v], d[v], n2, n1, turn.abs(), [&[], &[]], out);
        }
    }
}

/// A stretch [lo, hi] of fractional sample indices on one side of a line, with its end kinds.
#[derive(Clone, Copy, Debug)]
struct Run {
    lo: f64,
    hi: f64,
    ends: [End; 2],
}

/// The probe's stats.
#[derive(Default, Debug)]
pub struct WalkStats {
    pub runs: u32,
    pub rings: u32,
    pub rings_too_tight: u32,
    pub one_walk: u32,
}

/// The sidewalk rings of a cell: one-sided bands per run and the rings of both-walk rounds.
pub fn sidewalks(
    lines: &[WayLine],
    points: &[(P, Vec<Arm>, Vec<bool>)],
    rounded: &[Rounded],
    per_m: &dyn Fn(P) -> f64,
    stats: &mut WalkStats,
) -> Rings {
    let mut out: Rings = Vec::new();
    // Cuts per (line, side): stretches from a foot to its point, with the foot's end.
    let mut cuts: Vec<[Vec<(f64, f64, End, End)>; 2]> = lines.iter().map(|_| [Vec::new(), Vec::new()]).collect();
    let side_ix = |s: i8| usize::from(s > 0);
    let mut adds: Vec<[Vec<(f64, f64)>; 2]> = lines.iter().map(|_| [Vec::new(), Vec::new()]).collect();
    // Open sectors (no round) between neighbouring road arms: the end at the point.
    type Key = (usize, usize, usize);
    let mut opens: std::collections::BTreeMap<Key, (f64, Option<Key>)> = std::collections::BTreeMap::new();
    for ((pos, arms, _), r) in points.iter().zip(rounded) {
        let road: Vec<usize> = (0..arms.len()).filter(|&k| lines[arms[k].line].sec.group == "road").collect();
        if road.len() < 2 {
            continue;
        }
        let pm = per_m(*pos);
        for k in 0..road.len() {
            let (ka, kb) = (road[k], road[(k + 1) % road.len()]);
            if r.feet.iter().any(|f| (f.a, f.b) == (ka, kb) || (f.a, f.b) == (kb, ka)) {
                continue;
            }
            let (aa, ab) = (arms[ka], arms[kb]);
            let first = |a: Arm, s: i8| arm_walk_at(lines, a, s, 0.5 * pm).is_some();
            let (wa, wb) = (first(aa, 1), first(ab, -1));
            let w = if wa && wb {
                (lines[aa.line].sec.sidewalk_m * per_m(lines[aa.line].p[0]))
                    .min(lines[ab.line].sec.sidewalk_m * per_m(lines[ab.line].p[0]))
            } else {
                0.0
            };
            let (ka_, kb_) = ((aa.line, side_ix(aa.dir), aa.at), (ab.line, side_ix(-ab.dir), ab.at));
            if wa {
                opens.entry(ka_).or_insert((w, wb.then_some(kb_)));
            }
            if wb {
                opens.entry(kb_).or_insert((w, wa.then_some(ka_)));
            }
        }
    }
    for ((pos, arms, walk_pairs), r) in points.iter().zip(rounded) {
        for f in &r.feet {
            if !walk_pairs.get(f.pair).copied().unwrap_or(false) {
                continue;
            }
            let (aa, ab) = (arms[f.a], arms[f.b]);
            let pm = per_m(*pos);
            let (wa, wb) = (arm_walk(lines, aa, 1, pm), arm_walk(lines, ab, -1, pm));
            if !wa && !wb {
                continue;
            }
            let units = |l: usize| per_m(lines[l].p[0]);
            let w_end = if wa && wb {
                (lines[aa.line].sec.sidewalk_m * units(aa.line)).min(lines[ab.line].sec.sidewalk_m * units(ab.line))
            } else {
                stats.one_walk += 1;
                0.0
            };
            let end = End { w: w_end };
            for (arm, x, s) in [(aa, f.xa, aa.dir), (ab, f.xb, -ab.dir)] {
                let at = arm.at as f64;
                let c = if x < at { (x, at, end, PLAIN) } else { (at, x, PLAIN, end) };
                cuts[arm.line][side_ix(s)].push(c);
            }
            if wa && wb {
                ring(lines, *pos, aa, ab, f, w_end, &mut out, stats);
                // The runs run on to the corner.
                for (arm, s) in [(aa, 1i8), (ab, -1)] {
                    if let Some(x) = arm_walk_at(lines, arm, s, LOOK_M * pm) {
                        let at = arm.at as f64;
                        if x != at {
                            adds[arm.line][side_ix(s * arm.dir)].push((at.min(x), at.max(x)));
                        }
                    }
                }
            }
        }
    }
    let mut all: Vec<(usize, i8, Run)> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        if !walks(line) {
            continue;
        }
        for side in [1i8, -1] {
            // Runs from the bits.
            let mut runs: Vec<Run> = Vec::new();
            let n = line.p.len();
            let mut start: Option<usize> = None;
            for s in 0..n {
                let on = s + 1 < n && line.walk[s] & bit(side) != 0;
                match (on, start) {
                    (true, None) => start = Some(s),
                    (false, Some(a)) => {
                        runs.push(Run { lo: a as f64, hi: s as f64, ends: [PLAIN, PLAIN] });
                        start = None;
                    }
                    _ => {}
                }
            }
            // With the run-ons, and the ends at open sectors.
            for &(lo, hi) in &adds[li][side_ix(side)] {
                runs.push(Run { lo, hi, ends: [PLAIN, PLAIN] });
            }
            runs.sort_by(|a, b| a.lo.total_cmp(&b.lo));
            let mut merged: Vec<Run> = Vec::new();
            for r in runs {
                match merged.last_mut() {
                    Some(m) if r.lo <= m.hi + 1e-9 => {
                        if r.hi > m.hi {
                            m.hi = r.hi;
                            m.ends[1] = r.ends[1];
                        }
                    }
                    _ => merged.push(r),
                }
            }
            let mut runs = merged;
            // Less the cuts.
            for &(lo, hi, elo, ehi) in &cuts[li][side_ix(side)] {
                let mut next = Vec::new();
                for r in runs {
                    if hi <= r.lo || lo >= r.hi {
                        next.push(r);
                        continue;
                    }
                    if lo > r.lo {
                        next.push(Run { lo: r.lo, hi: lo, ends: [r.ends[0], elo] });
                    }
                    if hi < r.hi {
                        next.push(Run { lo: hi, hi: r.hi, ends: [ehi, r.ends[1]] });
                    }
                }
                runs = next;
            }
            for r in runs {
                if r.hi - r.lo > 1e-9 {
                    all.push((li, side, r));
                }
            }
        }
    }
    // An end at an open sector keeps the width that the partner's run brings to the point.
    let ends_at = |all: &[(usize, i8, Run)], k: Key| {
        all.iter().any(|(l, s, r)| *l == k.0 && side_ix(*s) == k.1 && (r.lo == k.2 as f64 || r.hi == k.2 as f64))
    };
    let fixed: Vec<(usize, i8, Run)> = all
        .iter()
        .map(|&(li, side, mut r)| {
            for (k, x) in [(0, r.lo), (1, r.hi)] {
                if x.fract() == 0.0 {
                    if let Some(&(w, partner)) = opens.get(&(li, side_ix(side), x as usize)) {
                        let on = partner.is_some_and(|p| ends_at(&all, p));
                        r.ends[k] = End { w: if on { w } else { 0.0 } };
                    }
                }
            }
            (li, side, r)
        })
        .collect();
    for (li, side, r) in fixed {
        let line = &lines[li];
        let units = per_m(line.p[0]);
        stats.runs += 1;
        band(line, r, side, line.sec.sidewalk_m * units, END_M * units, &mut out);
    }
    out
}

/// The band of one run: its points with the taper breakpoints, and the one-sided stroke.
fn band(line: &WayLine, r: Run, side: i8, wc: f64, end_u: f64, out: &mut Rings) {
    let mut xs: Vec<f64> = vec![r.lo];
    let mut k = r.lo.floor() + 1.0;
    while k < r.hi {
        xs.push(k);
        k += 1.0;
    }
    xs.push(r.hi);
    let pts: Vec<P> = xs.iter().map(|&x| point_at(line, x)).collect();
    let mut s = vec![0.0];
    for w in pts.windows(2) {
        s.push(s.last().unwrap() + dist(w[0], w[1]));
    }
    let len = *s.last().unwrap();
    let l = end_u;
    // The taper breakpoints.
    let at_dist = |d: f64| -> f64 {
        for i in 0..xs.len() - 1 {
            if s[i + 1] >= d && s[i + 1] > s[i] {
                return xs[i] + (d - s[i]) / (s[i + 1] - s[i]) * (xs[i + 1] - xs[i]);
            }
        }
        r.hi
    };
    let mut all = xs.clone();
    for (e, d) in [(r.ends[0], l), (r.ends[1], len - l)] {
        if e.w < wc && l > 0.0 && d > 0.0 && d < len {
            all.push(at_dist(d));
        }
    }
    all.sort_by(f64::total_cmp);
    all.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let pts: Vec<P> = all.iter().map(|&x| point_at(line, x)).collect();
    let mut s = vec![0.0];
    for w in pts.windows(2) {
        s.push(s.last().unwrap() + dist(w[0], w[1]));
    }
    let len = *s.last().unwrap();
    let width = |d: f64| {
        let mut w = wc;
        if l > 0.0 {
            let t0 = (1.0 - d / l).clamp(0.0, 1.0);
            let t1 = (1.0 - (len - d) / l).clamp(0.0, 1.0);
            w -= (t0 * (wc - r.ends[0].w).max(0.0)).max(t1 * (wc - r.ends[1].w).max(0.0));
        }
        w
    };
    let d: Vec<f64> = all.iter().zip(&s).map(|(&x, &sd)| h_at(line, x) + width(sd)).collect();
    let dirs: Vec<[f64; 2]> = (0..all.len() - 1).map(|i| seg_dir(line, 0.5 * (all[i] + all[i + 1]), true)).collect();
    // The end directions as the ring builds them.
    let mut dirs = dirs;
    if !dirs.is_empty() {
        dirs[0] = seg_dir(line, r.lo, true);
        let last = dirs.len() - 1;
        dirs[last] = seg_dir(line, r.hi, false);
    }
    side_pieces(&pts, &d, &dirs, f64::from(side), out);
}

/// The ring of a both-walk round: the arms' centre lines up to the feet, the band corners there and
/// the arc of radius rho − w about the centre.
#[allow(clippy::too_many_arguments)]
fn ring(lines: &[WayLine], pos: P, aa: Arm, ab: Arm, f: &Feet, w: f64, out: &mut Rings, stats: &mut WalkStats) {
    let inner = f.rho - w;
    if inner < 3.0 {
        stats.rings_too_tight += 1;
        return;
    }
    let corner = |arm: Arm, x: f64, s: i8| {
        let line = &lines[arm.line];
        // The run lies on the far side of the foot from the point.
        let up = x > arm.at as f64;
        let u = seg_dir(line, x, up);
        let nl = left(u);
        let p = point_at(line, x);
        (p, off(p, h_at(line, x) + w, [f64::from(s) * nl[0], f64::from(s) * nl[1]]))
    };
    let centre = |arm: Arm, x: f64| -> Vec<P> {
        let line = &lines[arm.line];
        let at = arm.at as f64;
        let mut v = vec![line.p[arm.at]];
        if x > at {
            let mut k = at + 1.0;
            while k < x {
                v.push(line.p[k as usize]);
                k += 1.0;
            }
        } else {
            let mut k = at - 1.0;
            while k > x {
                v.push(line.p[k as usize]);
                k -= 1.0;
            }
        }
        v.push(point_at(line, x));
        v
    };
    let (fa, ca) = corner(aa, f.xa, aa.dir);
    let (fb, cb) = corner(ab, f.xb, -ab.dir);
    let _ = (fa, fb);
    let rel = |p: P| [f64::from(p[0]) - f.c[0], f64::from(p[1]) - f.c[1]];
    let (a0, a1) = (angle(rel(ca)), angle(rel(cb)));
    let sweep = sweep_toward(a0, a1, angle(rel(pos)));
    let mut ring: Vec<P> = centre(aa, f.xa);
    ring.push(ca);
    for q in arc(f.c, inner, a0, sweep) {
        ring.push([libm::round(q[0]) as i32, libm::round(q[1]) as i32]);
    }
    ring.push(cb);
    let mut back = centre(ab, f.xb);
    back.reverse();
    ring.extend(back);
    stats.rings += 1;
    push_positive(ring, out);
}
