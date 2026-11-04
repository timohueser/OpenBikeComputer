//! The rounds where ways meet or turn. Per graph point (a junction, a joint, or a tight turn of one
//! way) and pair of neighbouring arms: the circle tangent to the two facing carriageway edges, of the
//! pair's radius or the largest smaller one that fits, and its fan: the two arms' centre lines up to
//! the circle's feet and the arc between. A junction's or joint's region: per arm, a stub up to a
//! straight seam past the farthest foot or overlap. Ways that do not meet at a point are never joined.

use super::geom::P;
use super::stroke::{left as left_of, off, pie_between, pieces_along, push_positive, unit as unit_of, Rings, STEPS};
use super::WayLine;

/// A fan's straight sides lie this many units inside the carriageway or the band it joins, so its
/// arc crosses their edge at an angle and no sliver is left between two edges that run together.
const INSET: f64 = 3.0;

/// A round smaller than this many units is none: the corner stays sharp.
const MIN_RADIUS: f64 = 60.0;

/// A circle touches an edge where its distance from the centre line is within this many units of
/// the half-width plus its radius.
const TOUCH: f64 = 2.0;

/// The halvings of the search for the largest round that fits.
const SEARCH: usize = 10;

/// A stub of a point's region is this much wider than the carriageway on each side, in units.
const STUB_PAD: f64 = 300.0;

/// An arm: a way's line from the point's sample, toward `dir` (+1 or −1 in sample order).
#[derive(Clone, Copy, Debug)]
pub struct Arm {
    pub line: usize,
    pub at: usize,
    pub dir: i8,
}

pub struct Rounded {
    /// One fan per round: the arms' centre lines to the feet, the round's arc between.
    pub fans: Vec<Vec<P>>,
    /// The band fans: paved shoulder, shoulder, verge, slope.
    pub rings: [Rings; 4],
    /// The point's region up to its seams (junctions and joints): it relabels its arms' carriageways.
    pub region: Rings,
    /// The parts of the arms' lines in the region. The arms of a way that runs on through the point
    /// form one part.
    pub spans: Vec<Part>,
    /// Per arm, its line and its seam: a cut across its carriageway and past it on both sides.
    pub seams: Vec<(usize, [P; 2])>,
    /// The circle of each round: centre and radius in units.
    pub circles: Vec<([i32; 2], f64)>,
    /// Per arm, its run and the station of the farthest foot of a round or crossing of facing edges.
    runs: Vec<Vec<RunPoint>>,
    pub far: Vec<f64>,
    /// PROBE (GP4): per round, the pair index, its arms a and b (a turns toward b), the feet as
    /// fractional sample indices of their lines, the centre (absolute, float) and the radius in units.
    pub feet: Vec<Feet>,
}

/// PROBE (GP4): the feet of one round.
#[derive(Clone, Copy, Debug)]
pub struct Feet {
    pub pair: usize,
    pub a: usize,
    pub xa: f64,
    pub b: usize,
    pub xb: f64,
    pub c: [f64; 2],
    pub rho: f64,
}

/// A part of a line in a point's region, between fractional sample indices `lo` and `hi`, and the
/// seam at each end (none at the point where the way ends).
#[derive(Clone, Copy, Debug)]
pub struct Part {
    pub line: usize,
    pub lo: f64,
    pub hi: f64,
    pub cuts: [Option<[P; 2]>; 2],
}

/// A seam that two points share on the link between them: its point, the link's direction there in
/// sample order, its half-length, and its place as a fractional sample index.
#[derive(Clone, Copy, Debug)]
pub struct Seam {
    p: P,
    u: [f64; 2],
    w: f64,
    x: f64,
}

/// The seam that two points at samples `a` < `b` of a line share when their own seams (`seam` past
/// the farthest feet `far`, from a and from b) would overlap: in the middle of the part between
/// the feet. It is computed from the line in sample order, so both points get the same cut.
pub fn link_seam(line: &WayLine, a: usize, b: usize, far: [f64; 2], seam: f64) -> Option<Seam> {
    let q = |i: usize| [f64::from(line.p[i][0]), f64::from(line.p[i][1])];
    let len: f64 = (a..b).map(|i| dist(q(i), q(i + 1))).sum();
    if far[0] + far[1] + 2.0 * seam <= len {
        return None;
    }
    let m = (0.5 * (far[0] + len - far[1])).clamp(0.0, len);
    let mut s0 = 0.0;
    for i in a..b {
        let l = dist(q(i), q(i + 1));
        if s0 + l >= m && l > 0.0 {
            let t = (m - s0) / l;
            let at = lerp(q(i), q(i + 1), t);
            let p = [libm::round(at[0]) as i32, libm::round(at[1]) as i32];
            let w = line.h[i] + t * (line.h[i + 1] - line.h[i]) + STUB_PAD;
            return Some(Seam { p, u: unit(q(i), q(i + 1)), w, x: i as f64 + t });
        }
        s0 += l;
    }
    None
}

/// A point of an arm's run, about the graph point: its place as a fractional sample index, its
/// point, half-width and band offsets.
#[derive(Clone, Copy, Debug)]
struct RunPoint {
    x: f64,
    q: [f64; 2],
    h: f64,
    bands: [f64; 4],
}

/// The run of an arm: from the point along its way for `len` units, ending early at another point's
/// sample (`stops`, sorted).
fn run(line: &WayLine, arm: Arm, pos: P, len: f64, stops: &[usize]) -> Vec<RunPoint> {
    let rel = |p: P| [f64::from(p[0] - pos[0]), f64::from(p[1] - pos[1])];
    let mut out =
        vec![RunPoint { x: arm.at as f64, q: rel(line.p[arm.at]), h: line.h[arm.at], bands: line.bands[arm.at] }];
    let mut i = arm.at as isize;
    let mut s = 0.0;
    loop {
        let j = i + isize::from(arm.dir);
        if j < 0 || j as usize >= line.p.len() {
            break;
        }
        let (a, b) = (out.last().unwrap().q, rel(line.p[j as usize]));
        let seg = dist(a, b);
        let (bi, bj) = (line.bands[i as usize], line.bands[j as usize]);
        if s + seg >= len && seg > 0.0 {
            let g = (len - s) / seg;
            let h = line.h[i as usize] + g * (line.h[j as usize] - line.h[i as usize]);
            let bands = std::array::from_fn(|k| bi[k] + g * (bj[k] - bi[k]));
            out.push(RunPoint { x: i as f64 + f64::from(arm.dir) * g, q: lerp(a, b, g), h, bands });
            break;
        }
        s += seg;
        if seg > 0.0 {
            out.push(RunPoint { x: j as f64, q: b, h: line.h[j as usize], bands: bj });
        }
        if stops.binary_search(&(j as usize)).is_ok() {
            break;
        }
        i = j;
    }
    out
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (x, y) = (b[0] - a[0], b[1] - a[1]);
    (x * x + y * y).sqrt()
}

fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
}

fn unit(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let d = dist(a, b);
    if d > 0.0 {
        [(b[0] - a[0]) / d, (b[1] - a[1]) / d]
    } else {
        [1.0, 0.0]
    }
}

/// The left normal (a quarter turn toward +y: toward a greater angle).
fn left(u: [f64; 2]) -> [f64; 2] {
    [-u[1], u[0]]
}

/// The run's length.
fn length(run: &[RunPoint]) -> f64 {
    run.windows(2).map(|w| dist(w[0].q, w[1].q)).sum()
}

/// The foot of q on a run: its station, the foot point, the half-width and band offsets there, and
/// the run's outward direction there.
struct Foot {
    s: f64,
    at: [f64; 2],
    h: f64,
    bands: [f64; 4],
    u: [f64; 2],
    dist: f64,
}

fn foot(run: &[RunPoint], q: [f64; 2]) -> Foot {
    let mut best = Foot { s: 0.0, at: run[0].q, h: run[0].h, bands: run[0].bands, u: [1.0, 0.0], dist: f64::INFINITY };
    let mut s0 = 0.0;
    for w in run.windows(2) {
        let (a, b) = (w[0].q, w[1].q);
        let len = dist(a, b);
        let t = if len > 0.0 {
            (((q[0] - a[0]) * (b[0] - a[0]) + (q[1] - a[1]) * (b[1] - a[1])) / (len * len)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let at = lerp(a, b, t);
        let d = dist(at, q);
        if d < best.dist {
            let bands = std::array::from_fn(|k| w[0].bands[k] + t * (w[1].bands[k] - w[0].bands[k]));
            best = Foot { s: s0 + t * len, at, h: w[0].h + t * (w[1].h - w[0].h), bands, u: unit(a, b), dist: d };
        }
        s0 += len;
    }
    best
}

/// The station of fractional sample index x on a run.
fn station(run: &[RunPoint], x: f64) -> f64 {
    let mut s0 = 0.0;
    for w in run.windows(2) {
        let len = dist(w[0].q, w[1].q);
        if w[0].x != w[1].x && (x - w[0].x) * (w[1].x - x) >= 0.0 {
            return s0 + (x - w[0].x) / (w[1].x - w[0].x) * len;
        }
        s0 += len;
    }
    s0
}

/// The fractional sample index at station s of a run.
fn place(run: &[RunPoint], s: f64) -> f64 {
    let mut s0 = 0.0;
    for w in run.windows(2) {
        let len = dist(w[0].q, w[1].q);
        if s0 + len >= s && len > 0.0 {
            return w[0].x + (s - s0) / len * (w[1].x - w[0].x);
        }
        s0 += len;
    }
    run.last().map_or(0.0, |r| r.x)
}

/// The run's points up to station s, the point there last.
fn up_to(run: &[RunPoint], s: f64) -> Vec<[f64; 2]> {
    let mut out = vec![run[0].q];
    let mut s0 = 0.0;
    for w in run.windows(2) {
        let len = dist(w[0].q, w[1].q);
        if s0 + len >= s {
            let t = if len > 0.0 { ((s - s0) / len).clamp(0.0, 1.0) } else { 0.0 };
            out.push(lerp(w[0].q, w[1].q, t));
            return out;
        }
        out.push(w[1].q);
        s0 += len;
    }
    out
}

/// The segments of a run's edge on `side` (+1 left, −1 right) at the half-width plus `extra`: one per
/// segment, and a chord over the gap at each joint that turns away from that side.
fn edge(run: &[RunPoint], side: f64, extra: f64) -> Vec<[[f64; 2]; 2]> {
    let mut out: Vec<[[f64; 2]; 2]> = Vec::new();
    for w in run.windows(2) {
        let n = left(unit(w[0].q, w[1].q));
        let off = |r: &RunPoint| [r.q[0] + side * (r.h + extra) * n[0], r.q[1] + side * (r.h + extra) * n[1]];
        let seg = [off(&w[0]), off(&w[1])];
        if let Some(last) = out.last() {
            if dist(last[1], seg[0]) > 0.0 {
                out.push([last[1], seg[0]]);
            }
        }
        out.push(seg);
    }
    out
}

/// The crossing of segments ab and cd as fractions (t on ab, u on cd).
fn crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> Option<(f64, f64)> {
    let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
    let den = r[0] * s[1] - r[1] * s[0];
    if den == 0.0 {
        return None;
    }
    let (qx, qy) = (c[0] - a[0], c[1] - a[1]);
    let t = (qx * s[1] - qy * s[0]) / den;
    let u = (qx * r[1] - qy * r[0]) / den;
    ((0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)).then_some((t, u))
}

/// The crossings of the facing edges of arms a (left side) and b (right side) at the half-width
/// plus `extra`, by summed distance from the point. On the inner side of a sharp turn an edge
/// crosses itself, so a later crossing can be the corner's.
fn crossings(ra: &[RunPoint], rb: &[RunPoint], extra: f64) -> Vec<(f64, [f64; 2])> {
    let (ea, eb) = (edge(ra, 1.0, extra), edge(rb, -1.0, extra));
    let mut out = Vec::new();
    let mut sa = 0.0;
    for x in &ea {
        let mut sb = 0.0;
        for y in &eb {
            if let Some((t, u)) = crossing(x[0], x[1], y[0], y[1]) {
                out.push((sa + t * dist(x[0], x[1]) + sb + u * dist(y[0], y[1]), lerp(x[0], x[1], t)));
            }
            sb += dist(y[0], y[1]);
        }
        sa += dist(x[0], x[1]);
    }
    out.sort_by(|p, q| p.0.total_cmp(&q.0));
    out
}

/// A round of radius rho between arms a and b: its centre, if the circle touches both facing
/// carriageway edges at feet inside their runs and lies off every arm's carriageway.
fn round_at(runs: &[Vec<RunPoint>], a: usize, b: usize, rho: f64) -> Option<([f64; 2], Foot, Foot)> {
    // A foot at a joint whose two edges leave a gap is no tangent point: the circle misses the edge.
    let inside =
        |f: &Foot, run: &[RunPoint]| f.s > 0.0 && f.s < length(run) - 1.0 && (f.dist - f.h - rho).abs() <= TOUCH;
    crossings(&runs[a], &runs[b], rho).into_iter().find_map(|(_, c)| {
        let (fa, fb) = (foot(&runs[a], c), foot(&runs[b], c));
        let clear = runs.iter().all(|r| {
            let f = foot(r, c);
            f.dist >= f.h + rho - 1.0
        });
        (inside(&fa, &runs[a]) && inside(&fb, &runs[b]) && clear).then_some((c, fa, fb))
    })
}

/// The angle of v.
pub(super) fn angle(v: [f64; 2]) -> f64 {
    libm::atan2(v[1], v[0])
}

/// The arc about c of radius rho from angle a0 sweeping `sweep` radians (signed): its points at the
/// global directions strictly between its ends (36 steps, as the strokes'). Its ends, the tangent
/// points on an edge, are left out: a rounded tangent point lies a unit off the stroke's own edge
/// and would leave a step there. The fan's chord from inside the edge to the first point crosses
/// the edge at an angle instead.
pub(super) fn arc(c: [f64; 2], rho: f64, a0: f64, sweep: f64) -> Vec<[f64; 2]> {
    let step = 2.0 * std::f64::consts::PI / f64::from(STEPS);
    let at = |a: f64| [c[0] + rho * libm::cos(a), c[1] + rho * libm::sin(a)];
    let mut out = Vec::new();
    let a1 = a0 + sweep;
    let eps = 0.01;
    if sweep >= 0.0 {
        let mut k = libm::floor(a0 / step) as i64 + 1;
        while (k as f64) * step < a1 - eps {
            if (k as f64) * step > a0 + eps {
                out.push(at(k as f64 * step));
            }
            k += 1;
        }
    } else {
        let mut k = libm::ceil(a0 / step) as i64 - 1;
        while (k as f64) * step > a1 + eps {
            if (k as f64) * step < a0 - eps {
                out.push(at(k as f64 * step));
            }
            k -= 1;
        }
    }
    out
}

/// The signed sweep from angle a0 to a1 that passes the direction `toward`.
pub(super) fn sweep_toward(a0: f64, a1: f64, toward: f64) -> f64 {
    let tau = 2.0 * std::f64::consts::PI;
    let ccw = (a1 - a0).rem_euclid(tau);
    let mid = (toward - a0).rem_euclid(tau);
    if mid <= ccw {
        ccw
    } else {
        ccw - tau
    }
}

/// The end of a band fan on an arm: from the foot inward, the taper about the carriageway's tangent
/// point `t` (radius `wide`, from the edge's normal `n` toward the arm's outward direction `u`) down
/// to `inset` units inside the arm's own band `own`, else the band arc's end pulled in by INSET.
fn band_end(t: [f64; 2], n: [f64; 2], u: [f64; 2], wide: f64, own: f64) -> Vec<[f64; 2]> {
    let at = |phi: f64| {
        let (c, s) = (libm::cos(phi), libm::sin(phi));
        [t[0] + wide * (c * n[0] + s * u[0]), t[1] + wide * (c * n[1] + s * u[1])]
    };
    if own >= wide {
        return vec![[t[0] + (wide - INSET) * n[0], t[1] + (wide - INSET) * n[1]]];
    }
    let end = libm::acos(((own - INSET) / wide).clamp(-1.0, 1.0));
    let steps = (libm::ceil(end / (2.0 * std::f64::consts::PI / f64::from(STEPS))) as usize).max(1);
    // From the taper's end (on the arm) back to the band arc's end (on the normal).
    (0..=steps).rev().map(|i| at(end * i as f64 / steps as f64)).collect()
}

/// The fills between the square ends of a point's arms where two neighbouring arms open by half a
/// turn or more: a fan about the point from one arm's corner to the other's, built from the strokes'
/// own corner points.
fn fill_gaps(lines: &[WayLine], pos: P, arms: &[Arm], layer: usize, out: &mut Vec<Vec<P>>) {
    let tau = 2.0 * std::f64::consts::PI;
    let cuts = cuts(lines, arms);
    let mut by: Vec<(f64, [f64; 2], [f64; 5], Arm)> = arms
        .iter()
        .filter_map(|&a| {
            let line = &lines[a.line];
            let next = line.p.get(a.at.checked_add_signed(isize::from(a.dir))?)?;
            let u = unit_of(pos, *next)?;
            Some((libm::atan2(u[1], u[0]), u, line.offsets(a.at), a))
        })
        .collect();
    by.sort_by(|x, y| x.0.total_cmp(&y.0));
    for k in 0..by.len() {
        let (a, b) = (by[k], by[(k + 1) % by.len()]);
        let sector = if by.len() == 1 { tau } else { (b.0 - a.0).rem_euclid(tau) };
        // A way that runs on through the point joins its own segments.
        let through = a.3.line == b.3.line && a.3.at == b.3.at;
        if sector < std::f64::consts::PI || through {
            continue;
        }
        let (na, nb) = (left_of(a.1), left_of(b.1));
        let (wa, wb) = (a.2[layer], b.2[layer]);
        // A band no wider than the layer inside it on both arms is none.
        if layer > 0 && wa <= a.2[layer - 1] && wb <= b.2[layer - 1] {
            continue;
        }
        let sweep = sector - std::f64::consts::PI;
        if sweep >= tau / f64::from(STEPS) {
            pie_between(pos, wa, wb, na, [-nb[0], -nb[1]], sweep, [&cuts, &cuts], out);
            continue;
        }
        // A gap under one arc step: each arm's layers up to the gap's bisector. A taper over so
        // small a turn would be a needle.
        let (c, s) = (libm::cos(sweep / 2.0), libm::sin(sweep / 2.0));
        let mid = [c * na[0] - s * na[1], s * na[0] + c * na[1]];
        pie_between(pos, wa, wa, na, mid, sweep / 2.0, [&cuts, &cuts], out);
        pie_between(pos, wb, wb, mid, [-nb[0], -nb[1]], sweep / 2.0, [&cuts, &cuts], out);
    }
}

/// The layer offsets of all arms of a point, ascending: every square end at the point and every
/// side of a gap between its arms passes their corners, so two ends on one line, or a gap side and
/// an end, share their vertices.
pub fn cuts(lines: &[WayLine], arms: &[Arm]) -> Vec<f64> {
    let mut all: Vec<f64> = arms.iter().flat_map(|a| lines[a.line].offsets(a.at)).collect();
    all.sort_by(f64::total_cmp);
    all.dedup();
    all
}

/// The rounding of a point at `pos` with its arms, the arm length `len` (units) and the pairs (a, b,
/// r) of neighbouring arms. With `gaps`, a fill between square ends of arms that open by half a turn
/// or more (junctions and joints).
#[allow(clippy::too_many_arguments)]
pub fn round(
    lines: &[WayLine],
    stops: &[Vec<usize>],
    pos: P,
    arms: &[Arm],
    pairs: &[(usize, usize, f64)],
    len: f64,
    gaps: bool,
) -> Rounded {
    let runs: Vec<Vec<RunPoint>> = arms
        .iter()
        .map(|&a| {
            let own: Vec<usize> = stops[a.line].iter().copied().filter(|&s| s != a.at).collect();
            run(&lines[a.line], a, pos, len, &own)
        })
        .collect();
    let mut out = Rounded {
        fans: Vec::new(),
        rings: Default::default(),
        region: Vec::new(),
        spans: Vec::new(),
        seams: Vec::new(),
        circles: Vec::new(),
        runs: Vec::new(),
        far: vec![0.0; arms.len()],
        feet: Vec::new(),
    };
    if runs.iter().any(|r| r.len() < 2) {
        return out;
    }
    let abs = |q: [f64; 2]| [pos[0] + libm::round(q[0]) as i32, pos[1] + libm::round(q[1]) as i32];
    let heading = |k: usize| angle(unit(runs[k][0].q, runs[k][1].q));
    let mut far = vec![0.0f64; arms.len()];
    if gaps {
        fill_gaps(lines, pos, arms, 0, &mut out.fans);
        for k in 0..4 {
            fill_gaps(lines, pos, arms, k + 1, &mut out.rings[k]);
        }
    }
    for (pair, &(i, j, r)) in pairs.iter().enumerate() {
        // a turns counter-clockwise to b through the pair's sector.
        let tau = 2.0 * std::f64::consts::PI;
        let (a, b) = if (heading(j) - heading(i)).rem_euclid(tau) <= std::f64::consts::PI { (i, j) } else { (j, i) };
        if let Some(&(_, k)) = crossings(&runs[a], &runs[b], 0.0).first() {
            far[a] = far[a].max(foot(&runs[a], k).s);
            far[b] = far[b].max(foot(&runs[b], k).s);
        }
        if r < MIN_RADIUS {
            continue;
        }
        // The pair's radius, else the largest smaller one that fits.
        let mut found = round_at(&runs, a, b, r).map(|f| (r, f));
        if found.is_none() {
            let (mut lo, mut hi) = (0.0, r);
            for _ in 0..SEARCH {
                let mid = 0.5 * (lo + hi);
                match round_at(&runs, a, b, mid) {
                    Some(f) => {
                        lo = mid;
                        found = Some((mid, f));
                    }
                    None => hi = mid,
                }
            }
        }
        let Some((rho, (c, fa, fb))) = found.filter(|f| f.0 >= MIN_RADIUS) else { continue };
        far[a] = far[a].max(fa.s);
        far[b] = far[b].max(fb.s);
        out.circles.push((abs(c), rho));
        out.feet.push(Feet {
            pair,
            a,
            xa: place(&runs[a], fa.s),
            b,
            xb: place(&runs[b], fb.s),
            c: [f64::from(pos[0]) + c[0], f64::from(pos[1]) + c[1]],
            rho,
        });
        // The arc faces the point.
        let (na, nb) = (unit(fa.at, c), unit(fb.at, c));
        let (a0, a1) = (angle([-na[0], -na[1]]), angle([-nb[0], -nb[1]]));
        let sweep = sweep_toward(a0, a1, angle([-c[0], -c[1]]));
        let side_a = up_to(&runs[a], fa.s);
        let side_b = up_to(&runs[b], fb.s);
        let mut fan: Vec<[f64; 2]> = side_a.clone();
        let edge_point = |f: &Foot, n: [f64; 2], d: f64| [f.at[0] + d * n[0], f.at[1] + d * n[1]];
        fan.push(edge_point(&fa, na, fa.h - INSET));
        fan.extend(arc(c, rho, a0, sweep));
        fan.push(edge_point(&fb, nb, fb.h - INSET));
        fan.extend(side_b.iter().rev());
        push_positive(fan.iter().map(|&q| abs(q)).collect(), &mut out.fans);
        // The band fans: the arc of the wider band about the same centre, tapering into a narrower
        // arm's band about the carriageway's tangent point.
        for k in 0..4 {
            let (ba, bb) = (fa.bands[k] - fa.h, fb.bands[k] - fb.h);
            let wide = ba.max(bb);
            // A band no wider than the layer inside it on both arms is none.
            let inner = |f: &Foot| if k == 0 { f.h } else { f.bands[k - 1] };
            if wide <= 0.0 || rho - wide < INSET || (fa.bands[k] <= inner(&fa) && fb.bands[k] <= inner(&fb)) {
                continue;
            }
            let (ta, tb) = (edge_point(&fa, na, fa.h), edge_point(&fb, nb, fb.h));
            let mut ring = side_a.clone();
            ring.extend(band_end(ta, na, fa.u, wide, ba));
            ring.extend(arc(c, rho - wide, a0, sweep));
            ring.extend(band_end(tb, nb, fb.u, wide, bb).into_iter().rev());
            ring.extend(side_b.iter().rev());
            push_positive(ring.iter().map(|&q| abs(q)).collect(), &mut out.rings[k]);
        }
    }
    out.far = far;
    out.runs = runs;
    out
}

/// The region of a junction or joint: per arm, a stub round about the point up to its seam, `seam`
/// units past the farthest foot on the arm or at the seam it shares with the next point (`shared`).
/// A seam is a straight cut across the arm, square to the run's own direction there.
pub fn region(pos: P, arms: &[Arm], r: &mut Rounded, seam: f64, shared: &[Option<Seam>]) {
    let abs = |q: [f64; 2]| [pos[0] + libm::round(q[0]) as i32, pos[1] + libm::round(q[1]) as i32];
    for (k, run) in r.runs.iter().enumerate() {
        let s = match shared[k] {
            Some(sm) => station(run, sm.x),
            None => (r.far[k] + seam).min(length(run)),
        };
        let rel = up_to(run, s);
        let mut stub: Vec<P> = rel.iter().map(|&q| abs(q)).collect();
        let mut d: Vec<f64> = rel.iter().map(|&q| foot(run, q).h + STUB_PAD).collect();
        let mut dirs: Vec<Option<[f64; 2]>> = rel.windows(2).map(|w| Some(unit(w[0], w[1]))).collect();
        let mut x = place(run, s);
        if let Some(sm) = shared[k] {
            // The shared cut: the same end point, half-length and direction from both points.
            let dir = f64::from(arms[k].dir);
            stub.pop();
            d.pop();
            dirs.pop();
            if stub.last() == Some(&sm.p) {
                stub.pop();
                d.pop();
                dirs.pop();
            }
            stub.push(sm.p);
            d.push(sm.w);
            dirs.push(Some([dir * sm.u[0], dir * sm.u[1]]));
            x = sm.x;
        }
        if stub.len() < 2 {
            continue;
        }
        pieces_along(&stub, &d, &dirs, [&[], &[]], [true, false], false, &mut r.region);
        let (end, n) = (stub[stub.len() - 1], left(dirs.last().copied().flatten().unwrap_or([1.0, 0.0])));
        let w = d[d.len() - 1];
        let cut = [off(end, w, [-n[0], -n[1]]), off(end, w, n)];
        r.seams.push((arms[k].line, cut));
        let (line, at) = (arms[k].line, run[0].x);
        r.spans.push(match x > at {
            true => Part { line, lo: at, hi: x, cuts: [None, Some(cut)] },
            false => Part { line, lo: x, hi: at, cuts: [Some(cut), None] },
        });
    }
    r.spans.sort_by(|a, b| (a.line, a.lo).partial_cmp(&(b.line, b.lo)).unwrap());
    r.spans.dedup_by(|b, a| {
        let joins = a.line == b.line && b.lo <= a.hi;
        if joins && b.hi > a.hi {
            a.hi = b.hi;
            a.cuts[1] = b.cuts[1];
        }
        joins
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sweep_toward_a_direction_takes_the_arc_through_it() {
        let q = std::f64::consts::FRAC_PI_2;
        assert!((sweep_toward(0.0, q, q / 2.0) - q).abs() < 1e-12);
        assert!((sweep_toward(0.0, q, -q) - (q - 4.0 * q)).abs() < 1e-12);
    }

    #[test]
    fn segments_cross_inside_both() {
        let x = crossing([0.0, 0.0], [2.0, 0.0], [1.0, -1.0], [1.0, 1.0]).unwrap();
        assert_eq!(x, (0.5, 0.5));
        assert!(crossing([0.0, 0.0], [2.0, 0.0], [3.0, -1.0], [3.0, 1.0]).is_none());
    }
}
