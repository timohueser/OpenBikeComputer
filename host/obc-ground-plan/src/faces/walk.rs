//! PROBE (GP4, throwaway): sidewalks as one-sided strokes of chains. Chains join walking stretches
//! across open sectors; the width is the lower envelope of cones (class widths and end targets); a
//! two-walk round carries a ring; at a side road without a sidewalk the slab ends at full width where
//! the side road's centre line crosses its middle line, so the side road's carriageway cuts it.

use std::collections::BTreeMap;

use super::geom::P;
use super::points::{angle, arc, sweep_toward, Arm, Feet, Rounded};
use super::stroke::{left, off, pie_between, push_positive, unit, Rings};
use super::WayLine;

/// A facing side walks at a sector where its bit is set within this of the point.
const LOOK_M: f64 = 6.0;
/// The radius of a one-walk side-road corner.
pub const ONE_WALK_M: f64 = 0.8;
/// A join that turns less than this is mitred (as the strokes).
const MITRE: f64 = 2.0 * std::f64::consts::PI / 36.0;

pub struct PointInfo {
    pub pos: P,
    pub arms: Vec<Arm>,
    pub walk_pairs: Vec<bool>,
    pub hosts: Vec<bool>,
}

/// The stroke side +1 (a quarter turn toward +y, the map's right) is `walk` bit 2.
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

fn dist(a: P, b: P) -> f64 {
    let (x, y) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    (x * x + y * y).sqrt()
}

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

/// The direction (in sample order) of the segment that holds the part of a run next to x: above x
/// (`up`) or below.
fn seg_dir(line: &WayLine, x: f64, up: bool) -> [f64; 2] {
    let n = line.p.len();
    let mut k = if up { (x.floor() as usize).min(n - 2) } else { ((x.ceil() as usize).max(1) - 1).min(n - 2) };
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

/// The one-sided stroke of points `p` at offsets `d` on `side`, with segment directions `dirs`: a
/// quad per segment, a mitre point at a small turn, a pie on the outer side of a larger one.
fn side_pieces(p: &[P], d_in: &[f64], d: &[f64], dirs: &[[f64; 2]], side: f64, out: &mut Rings) {
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
    let mut inners: Vec<[P; 2]> = (0..n - 1)
        .map(|i| {
            let nl = left(dirs[i]);
            let nn = [side * nl[0], side * nl[1]];
            [off(p[i], d_in[i], nn), off(p[i + 1], d_in[i + 1], nn)]
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
            let qi = off(p[i + 1], d_in[i + 1], m);
            inners[i][1] = qi;
            inners[i + 1][0] = qi;
            continue;
        }
        if (turn > 0.0 && side < 0.0) || (turn < 0.0 && side > 0.0) {
            pies.push((i + 1, turn, n1, n2));
        }
    }
    // The inner edge lies inside the carriageway (no edge along the centre line, where the fans run).
    for i in 0..n - 1 {
        push_positive(vec![inners[i][0], inners[i][1], corners[i][1], corners[i][0]], out);
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

/// A run end at a sample: line, side index, sample.
type Key = (usize, usize, usize);

/// What a run end is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum End {
    /// A cone constraint: the width at the end.
    Target(f64),
    /// No constraint: the slab keeps its width (a side road's carriageway cuts it).
    Full,
    /// The run goes on in the partner's run (an open sector where both sides walk).
    Open(Key),
}

const ZERO: End = End::Target(0.0);

#[derive(Clone, Copy, Debug)]
struct Run {
    li: usize,
    side: i8,
    lo: f64,
    hi: f64,
    ends: [End; 2],
}

#[derive(Default, Debug)]
pub struct WalkStats {
    pub chains: u32,
    pub rings: u32,
    pub rings_too_tight: u32,
    pub one_walk_ends: u32,
}

fn side_ix(s: i8) -> usize {
    usize::from(s > 0)
}

/// The fractional index on line `lw` where the centre line of `other` (from its point outward)
/// crosses lw's line offset by its half-width plus `extra` on `side`, nearest the sample `at`.
fn crossing_on(lines: &[WayLine], lw: usize, side: i8, extra: f64, other: Arm, at: usize, reach: f64) -> Option<f64> {
    let l = &lines[lw];
    let o = &lines[other.line];
    let f = |p: P| [f64::from(p[0]), f64::from(p[1])];
    let mut cen = vec![f(o.p[other.at])];
    let mut i = other.at as isize;
    let mut d = 0.0;
    while d < reach {
        let j = i + isize::from(other.dir);
        if j < 0 || j as usize >= o.p.len() {
            break;
        }
        d += dist(o.p[i as usize], o.p[j as usize]);
        cen.push(f(o.p[j as usize]));
        i = j;
    }
    let s = f64::from(side);
    let mut best: Option<(f64, f64)> = None;
    for k in 0..l.p.len() - 1 {
        let Some(u) = unit(l.p[k], l.p[k + 1]) else { continue };
        let nl = left(u);
        let at_k =
            |m: usize| [f(l.p[m])[0] + s * (l.h[m] + extra) * nl[0], f(l.p[m])[1] + s * (l.h[m] + extra) * nl[1]];
        let (a, b) = (at_k(k), at_k(k + 1));
        for w in cen.windows(2) {
            let (c, e) = (w[0], w[1]);
            let (r, q) = ([b[0] - a[0], b[1] - a[1]], [e[0] - c[0], e[1] - c[1]]);
            let den = r[0] * q[1] - r[1] * q[0];
            if den == 0.0 {
                continue;
            }
            let (x, y) = (c[0] - a[0], c[1] - a[1]);
            let t = (x * q[1] - y * q[0]) / den;
            let v = (x * r[1] - y * r[0]) / den;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&v) {
                let xi = k as f64 + t;
                let gap = (xi - at as f64).abs();
                if best.is_none_or(|b| gap < b.1) {
                    best = Some((xi, gap));
                }
            }
        }
    }
    best.map(|b| b.0)
}

/// A stretch from x toward the point `at` and the end kinds at its two bounds: `at_x` at x.
fn toward(x: f64, at: f64, at_x: End, at_point: End) -> (f64, f64, End, End) {
    if x < at {
        (x, at, at_x, at_point)
    } else {
        (at, x, at_point, at_x)
    }
}

/// The sidewalk rings of a cell.
pub fn sidewalks(
    lines: &[WayLine],
    points: &[PointInfo],
    rounded: &[Rounded],
    per_m: &dyn Fn(P) -> f64,
    glob: &dyn Fn(P) -> [i64; 2],
    stats: &mut WalkStats,
) -> Rings {
    let mut out: Rings = Vec::new();
    let nl = lines.len();
    // Per (line, side): run-ons to add; stretches to remove, with the end kind at each bound; open
    // ends at a point sample.
    let mut adds: Vec<[Vec<(f64, f64)>; 2]> = vec![[Vec::new(), Vec::new()]; nl];
    let mut cuts: Vec<[Vec<(f64, f64, End, End)>; 2]> = vec![[Vec::new(), Vec::new()]; nl];
    let mut opens: BTreeMap<Key, End> = BTreeMap::new();
    let mut marks: BTreeMap<(usize, usize, u64), End> = BTreeMap::new();
    let wc = |l: usize| lines[l].sec.sidewalk_m * per_m(lines[l].p[0]);
    for (pt, r) in points.iter().zip(rounded) {
        let road: Vec<usize> = (0..pt.arms.len()).filter(|&k| lines[pt.arms[k].line].sec.group == "road").collect();
        if road.len() < 2 {
            continue;
        }
        let pm = per_m(pt.pos);
        for k in 0..road.len() {
            let (ka, kb) = (road[k], road[(k + 1) % road.len()]);
            let (aa, ab) = (pt.arms[ka], pt.arms[kb]);
            let round = r.feet.iter().find(|f| (f.a, f.b) == (ka, kb));
            let (wa, wb) = (arm_walk(lines, aa, 1, pm), arm_walk(lines, ab, -1, pm));
            if std::env::var_os("GP4_GAPS").is_some() && !(pt.hosts[ka] && pt.hosts[kb]) {
                for (arm, sd) in [(aa, 1i8), (ab, -1)] {
                    let near = arm_walk_at(lines, arm, sd, 0.5 * pm).is_some();
                    let far = arm_walk_at(lines, arm, sd, 30.0 * pm);
                    if walks(&lines[arm.line]) {
                        let gap = far.map(|x| dist(lines[arm.line].p[arm.at], point_at(&lines[arm.line], x)) / pm);
                        eprintln!("gap first {near} at {:?}", gap.map(|g| (g * 10.0).round() / 10.0));
                    }
                }
            }
            let round = round.filter(|f| pt.walk_pairs.get(f.pair).copied().unwrap_or(false));
            let same_road = pt.hosts[ka] && pt.hosts[kb];
            if let (Some(f), true) = (round, wa && wb) {
                // Two walk: the ring carries the slab round the corner.
                let end = End::Target(wc(aa.line).min(wc(ab.line)));
                for (arm, x, s) in [(aa, f.xa, 1i8), (ab, f.xb, -1)] {
                    let at = arm.at as f64;
                    let si = side_ix(s * arm.dir);
                    cuts[arm.line][si].push(toward(x, at, end, ZERO));
                    if let Some(b) = arm_walk_at(lines, arm, s, LOOK_M * pm) {
                        if (b - at).abs() > (x - at).abs() {
                            adds[arm.line][si].push((x.min(b), x.max(b)));
                        }
                    }
                }
                if std::env::var_os("GP4_NORING").is_none() {
                    ring(lines, pt.pos, aa, ab, f, wc(aa.line).min(wc(ab.line)), &mut out, stats);
                }
                continue;
            }
            let first = |a: Arm, s: i8| arm_walk_at(lines, a, s, 0.5 * pm).is_some();
            let (key_a, key_b) = ((aa.line, side_ix(aa.dir), aa.at), (ab.line, side_ix(-ab.dir), ab.at));
            if round.is_none() && first(aa, 1) && first(ab, -1) {
                // An open sector where both walk: the chain goes on through the point.
                opens.insert(key_a, End::Open(key_b));
                opens.insert(key_b, End::Open(key_a));
                continue;
            }
            if wa == wb {
                continue;
            }
            let (w_arm, other, s, x, key) =
                if wa { (aa, ab, 1i8, round.map(|f| f.xa), key_a) } else { (ab, aa, -1, round.map(|f| f.xb), key_b) };
            let si = side_ix(s * w_arm.dir);
            let at = w_arm.at as f64;
            if same_road {
                // Along one road: a bit change, an angled end at the foot or at the point.
                match x {
                    Some(x) => cuts[w_arm.line][si].push(toward(x, at, ZERO, ZERO)),
                    None => {
                        opens.insert(key, ZERO);
                    }
                }
                continue;
            }
            // Against another road: the slab keeps its width up to where that road's centre line
            // crosses its middle line, and ends there; the road's carriageway cuts it.
            stats.one_walk_ends += 1;
            let p = if std::env::var_os("GP4_NOP").is_some() {
                None
            } else {
                crossing_on(lines, w_arm.line, s * w_arm.dir, wc(w_arm.line) / 2.0, other, w_arm.at, 30.0 * pm)
            }
            .unwrap_or(at);
            let outward = |y: f64| (y - at) * f64::from(w_arm.dir) > 0.0;
            let far = pt.arms.iter().find(|a| a.line == w_arm.line && a.at == w_arm.at && a.dir == -w_arm.dir);
            let far_walks = far.is_some_and(|&a| arm_walk(lines, a, -s, pm));
            if outward(p) {
                if !far_walks {
                    cuts[w_arm.line][si].push(toward(p, at, End::Full, ZERO));
                }
            } else if p != at {
                adds[w_arm.line][si].push((p.min(at), p.max(at)));
                marks.insert((w_arm.line, si, p.to_bits()), End::Full);
            } else {
                marks.insert((w_arm.line, si, at.to_bits()), End::Full);
            }
            if let Some(b) = arm_walk_at(lines, w_arm, s, LOOK_M * pm) {
                if outward(b) && (b - at).abs() > (p - at).abs() {
                    adds[w_arm.line][si].push((p.min(b), p.max(b)));
                }
            }
        }
    }
    // The runs per line side: the bits with the run-ons, less the corners.
    let mut runs: Vec<Run> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        if !walks(line) {
            continue;
        }
        for side in [1i8, -1] {
            let si = side_ix(side);
            let n = line.p.len();
            let mut ivs: Vec<(f64, f64)> = Vec::new();
            let mut start: Option<usize> = None;
            for s in 0..n {
                let on = s + 1 < n && line.walk[s] & bit(side) != 0;
                match (on, start) {
                    (true, None) => start = Some(s),
                    (false, Some(a)) => {
                        ivs.push((a as f64, s as f64));
                        start = None;
                    }
                    _ => {}
                }
            }
            ivs.extend(adds[li][si].iter().copied());
            ivs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut part: Vec<Run> = Vec::new();
            for (lo, hi) in ivs {
                match part.last_mut() {
                    Some(m) if lo <= m.hi + 1e-9 => m.hi = m.hi.max(hi),
                    _ => part.push(Run { li, side, lo, hi, ends: [ZERO; 2] }),
                }
            }
            for &(lo, hi, elo, ehi) in &cuts[li][si] {
                let mut next = Vec::new();
                for r in part {
                    if hi <= r.lo || lo >= r.hi {
                        next.push(r);
                        continue;
                    }
                    if lo > r.lo {
                        next.push(Run { hi: lo, ends: [r.ends[0], elo], ..r });
                    }
                    if hi < r.hi {
                        next.push(Run { lo: hi, ends: [ehi, r.ends[1]], ..r });
                    }
                }
                part = next;
            }
            for mut r in part {
                if r.hi - r.lo <= 1e-9 {
                    continue;
                }
                for (k, x) in [(0, r.lo), (1, r.hi)] {
                    if let Some(&e) = marks.get(&(li, si, x.to_bits())) {
                        r.ends[k] = e;
                    }
                    if x.fract() == 0.0 && r.ends[k] == ZERO {
                        if let Some(&e) = opens.get(&(li, si, x as usize)) {
                            r.ends[k] = e;
                        }
                    }
                }
                runs.push(r);
            }
        }
    }
    // An open end goes on only where the partner's run ends at the partner's sample.
    let end_at = |runs: &[Run], k: Key| -> Option<(usize, usize)> {
        runs.iter().enumerate().find_map(|(i, r)| {
            if r.li != k.0 || side_ix(r.side) != k.1 {
                None
            } else if r.lo == k.2 as f64 {
                Some((i, 0))
            } else if r.hi == k.2 as f64 {
                Some((i, 1))
            } else {
                None
            }
        })
    };
    let snapshot = runs.clone();
    for r in &mut runs {
        for e in &mut r.ends {
            if let End::Open(k) = *e {
                if end_at(&snapshot, k).is_none() {
                    *e = ZERO;
                }
            }
        }
    }
    // Chains: follow the open ends, from a run with an end that is not open (loops last).
    let open = |e: End| matches!(e, End::Open(_));
    let mut used = vec![false; runs.len()];
    let mut order: Vec<usize> = (0..runs.len()).collect();
    order.sort_by_key(|&i| open(runs[i].ends[0]) && open(runs[i].ends[1]));
    for &start in &order {
        if used[start] {
            continue;
        }
        let fwd = !open(runs[start].ends[0]) || open(runs[start].ends[1]);
        let mut chain: Vec<(usize, bool)> = vec![(start, fwd)];
        used[start] = true;
        let mut cur = (start, fwd);
        loop {
            let r = &runs[cur.0];
            let End::Open(k) = (if cur.1 { r.ends[1] } else { r.ends[0] }) else { break };
            let Some((ni, at_end)) = end_at(&runs, k) else { break };
            if used[ni] {
                break;
            }
            used[ni] = true;
            cur = (ni, at_end == 0);
            chain.push(cur);
        }
        stats.chains += 1;
        if let Some(at) = debug_at() {
            let near = |p: P| {
                let g = glob(p);
                let (dx, dy) = ((g[0] - at[0]) as f64, (g[1] - at[1]) as f64);
                (dx * dx + dy * dy).sqrt() < at[2] as f64
            };
            let pts = |i: usize| [point_at(&lines[runs[i].li], runs[i].lo), point_at(&lines[runs[i].li], runs[i].hi)];
            if chain.iter().any(|&(i, _)| pts(i).iter().any(|&p| near(p))) {
                for &(i, f) in &chain {
                    let r = &runs[i];
                    eprintln!(
                        "chain piece way {} side {} {:.3}..{:.3} fwd {f} ends {:?}",
                        lines[r.li].way, r.side, r.lo, r.hi, r.ends
                    );
                }
                eprintln!("--");
            }
        }
        if std::env::var_os("GP4_NOCHAIN").is_none() {
            stroke_chain(lines, &runs, &chain, per_m, &mut out);
        }
    }
    out
}

/// The one-sided stroke of a chain: its points in travel order with the breakpoints, the width by
/// the lower envelope of cones, one stroke through every join.
fn stroke_chain(lines: &[WayLine], runs: &[Run], chain: &[(usize, bool)], per_m: &dyn Fn(P) -> f64, out: &mut Rings) {
    let line_of = |pi: usize| &lines[runs[chain[pi].0].li];
    // Where piece pi starts and ends in travel order.
    let start_x = |pi: usize| {
        let (r, f) = (&runs[chain[pi].0], chain[pi].1);
        if f {
            r.lo
        } else {
            r.hi
        }
    };
    // (piece, x) in travel order; a join is the previous piece's last point.
    let mut xs: Vec<(usize, f64)> = Vec::new();
    for (pi, &(ri, fwd)) in chain.iter().enumerate() {
        let r = &runs[ri];
        let mut v = vec![r.lo];
        let mut k = r.lo.floor() + 1.0;
        while k < r.hi {
            v.push(k);
            k += 1.0;
        }
        v.push(r.hi);
        if !fwd {
            v.reverse();
        }
        xs.extend(v.into_iter().skip(usize::from(pi > 0)).map(|x| (pi, x)));
    }
    let pos = |q: (usize, f64)| point_at(line_of(q.0), q.1);
    // The x of the start of segment i on the line of its end.
    let seg_start = |xs: &[(usize, f64)], i: usize| if xs[i].0 == xs[i + 1].0 { xs[i].1 } else { start_x(xs[i + 1].0) };
    let lens = |xs: &[(usize, f64)]| {
        let mut s = vec![0.0];
        for w in xs.windows(2) {
            s.push(s.last().unwrap() + dist(pos(w[0]), pos(w[1])));
        }
        s
    };
    let s = lens(&xs);
    let len = *s.last().unwrap();
    if len <= 0.0 || xs.len() < 2 {
        return;
    }
    // Each piece's extent along the chain and class width.
    let ext: Vec<(f64, f64, f64)> = (0..chain.len())
        .map(|pi| {
            let a = xs.iter().position(|q| q.0 == pi).unwrap_or(0).saturating_sub(usize::from(pi > 0));
            let b = xs.iter().rposition(|q| q.0 == pi).unwrap_or(0);
            let l = line_of(pi);
            (s[a], s[b], l.sec.sidewalk_m * per_m(l.p[0]))
        })
        .collect();
    let end_of = |pi: usize, last: bool| {
        let (r, f) = (&runs[chain[pi].0], chain[pi].1);
        if f == last {
            r.ends[1]
        } else {
            r.ends[0]
        }
    };
    let n = chain.len();
    let (k, r_m) = slope_round();
    let phi = libm::atan(k);
    let tan_half = libm::tan(phi / 2.0);
    let r_units = r_m * per_m(line_of(0).p[0]);
    // Ring targets are cones; a 0 target is an angled end with round corners.
    let mut cons: Vec<(f64, f64, f64)> = Vec::new();
    let mut zeros: Vec<(f64, f64, f64)> = Vec::new(); // (station, direction into the chain, class width)
    for (e, at, dir, w) in [(end_of(0, false), 0.0, 1.0, ext[0].2), (end_of(n - 1, true), len, -1.0, ext[n - 1].2)] {
        match e {
            End::Target(t) if t <= 0.0 => zeros.push((at, dir, w)),
            End::Target(t) => cons.push((at, t, w)),
            _ => {}
        }
    }
    // The end profile at dd from the end station: a fillet into the kerb, the taper, a fillet into
    // the full width.
    let end_profile = |dd: f64, w: f64| -> f64 {
        let r = r_units.min((w / k) / (phi.sin() - tan_half * (1.0 - phi.cos())));
        let t = r * tan_half;
        let dk = t + w / k;
        if dd <= 0.0 {
            0.0
        } else if dd <= r * phi.sin() {
            r - (r * r - dd * dd).max(0.0).sqrt()
        } else if dd <= dk - t * phi.cos() {
            k * (dd - t)
        } else if dd <= dk + t {
            let x = dd - dk - t;
            w - r + (r * r - x * x).max(0.0).sqrt()
        } else {
            w
        }
    };
    let width = |d: f64| -> f64 {
        let mut w = f64::INFINITY;
        for &(a, b, c) in &ext {
            let g = if d < a {
                a - d
            } else if d > b {
                d - b
            } else {
                0.0
            };
            w = w.min(c + k * g);
        }
        for &(at, t, _) in &cons {
            w = w.min(t + k * (d - at).abs());
        }
        for &(at, dir, cw) in &zeros {
            w = w.min(end_profile((d - at) * dir, cw));
        }
        w
    };
    // Breakpoints: the cone kinks, and the fillets' points at even steps of their turn.
    let mut bps: Vec<f64> = Vec::new();
    for &(at, t, w) in &cons {
        let g = (w - t).max(0.0) / k;
        bps.extend([at - g, at + g]);
    }
    for w in ext.windows(2) {
        let g = (w[0].2 - w[1].2).abs() / k;
        bps.push(if w[0].2 > w[1].2 { w[0].1 - g } else { w[1].0 + g });
    }
    let steps = libm::ceil(phi / (10f64).to_radians()).max(1.0) as usize;
    for &(at, dir, w) in &zeros {
        let r = r_units.min((w / k) / (phi.sin() - tan_half * (1.0 - phi.cos())));
        let t = r * tan_half;
        let dk = t + w / k;
        for i in 0..=steps {
            let a = phi * i as f64 / steps as f64;
            bps.push(at + dir * r * libm::sin(a));
            bps.push(at + dir * (dk + t - r * libm::sin(a)));
        }
    }
    bps.sort_by(f64::total_cmp);
    for d in bps.into_iter().filter(|&d| d > 0.0 && d < len) {
        let s = lens(&xs);
        let Some(i) = (0..xs.len() - 1).find(|&i| s[i] <= d && d <= s[i + 1] && s[i + 1] > s[i]) else { continue };
        let (xa, xb) = (seg_start(&xs, i), xs[i + 1].1);
        let x = xa + (d - s[i]) / (s[i + 1] - s[i]) * (xb - xa);
        if (x - xa).abs() > 1e-6 && (x - xb).abs() > 1e-6 {
            xs.insert(i + 1, (xs[i + 1].0, x));
        }
    }
    let s = lens(&xs);
    let pts: Vec<P> = xs.iter().map(|&q| pos(q)).collect();
    let d: Vec<f64> = xs.iter().zip(&s).map(|(&(pi, x), &sd)| h_at(line_of(pi), x) + width(sd)).collect();
    let dirs: Vec<[f64; 2]> = (0..xs.len() - 1)
        .map(|i| {
            let pi = xs[i + 1].0;
            let u = seg_dir(line_of(pi), 0.5 * (seg_start(&xs, i) + xs[i + 1].1), true);
            if chain[pi].1 {
                u
            } else {
                [-u[0], -u[1]]
            }
        })
        .collect();
    let side = f64::from(runs[chain[0].0].side) * if chain[0].1 { 1.0 } else { -1.0 };
    let d_in: Vec<f64> = xs.iter().map(|&(pi, x)| 0.5 * h_at(line_of(pi), x)).collect();
    side_pieces(&pts, &d_in, &d, &dirs, side, out);
}

/// The ring of a both-walk round: the arms' centre lines up to the feet, the chains' end corners
/// there and the arc of radius rho − w about the centre.
#[allow(clippy::too_many_arguments)]
fn ring(lines: &[WayLine], pos: P, aa: Arm, ab: Arm, f: &Feet, w: f64, out: &mut Rings, stats: &mut WalkStats) {
    let inner = f.rho - w;
    if inner < 3.0 {
        stats.rings_too_tight += 1;
        return;
    }
    let corner = |arm: Arm, x: f64, s: i8| {
        let line = &lines[arm.line];
        let u = seg_dir(line, x, x > arm.at as f64);
        let nl = left(u);
        off(point_at(line, x), h_at(line, x) + w, [f64::from(s) * nl[0], f64::from(s) * nl[1]])
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
    let ca = corner(aa, f.xa, aa.dir);
    let cb = corner(ab, f.xb, -ab.dir);
    let rel = |p: P| [f64::from(p[0]) - f.c[0], f64::from(p[1]) - f.c[1]];
    let (a0, a1) = (angle(rel(ca)), angle(rel(cb)));
    let sweep = sweep_toward(a0, a1, angle(rel(pos)));
    // An annulus sector from the arc at rho − w out past the carriageway's arc (into the fan), between
    // the feet's normals at half the half-width.
    let inner_pt = |arm: Arm, x: f64, s: i8| {
        let line = &lines[arm.line];
        let u = seg_dir(line, x, x > arm.at as f64);
        let nl = left(u);
        off(point_at(line, x), 0.5 * h_at(line, x), [f64::from(s) * nl[0], f64::from(s) * nl[1]])
    };
    let _ = centre;
    let mut ring: Vec<P> = vec![inner_pt(aa, f.xa, aa.dir), ca];
    for q in arc(f.c, inner, a0, sweep) {
        ring.push([libm::round(q[0]) as i32, libm::round(q[1]) as i32]);
    }
    ring.push(cb);
    ring.push(inner_pt(ab, f.xb, -ab.dir));
    let outer = f.rho + 3.0;
    let (b0, b1) = (angle(rel(inner_pt(ab, f.xb, -ab.dir))), angle(rel(inner_pt(aa, f.xa, aa.dir))));
    for q in arc(f.c, outer, b0, sweep_toward(b0, b1, angle(rel(pos)))) {
        ring.push([libm::round(q[0]) as i32, libm::round(q[1]) as i32]);
    }
    stats.rings += 1;
    push_positive(ring, out);
}

/// PROBE: `GP4_DEBUG=x,y,r` (global units) prints the chains near that point.
fn debug_at() -> Option<[i64; 3]> {
    let v = std::env::var("GP4_DEBUG").ok()?;
    let p: Vec<i64> = v.split(',').filter_map(|t| t.parse().ok()).collect();
    (p.len() == 3).then(|| [p[0], p[1], p[2]])
}

/// PROBE: the end slope and the fillet radius in metres (`GP4_SLOPE`, `GP4_ROUND`).
fn slope_round() -> (f64, f64) {
    let get = |k: &str, d: f64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    (get("GP4_SLOPE", 1.0), get("GP4_ROUND", 1.0))
}
