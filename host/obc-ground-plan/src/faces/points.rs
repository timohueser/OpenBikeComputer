//! The rounding where ways meet or turn: one local closing per graph point (a junction, a joint, or a
//! tight turn of one way) over the strokes of that point's own arms, and the point's face: its arms up
//! to a straight seam past the farthest point that the rounding or another arm reaches, plus what the
//! closing adds there. Ways that do not meet at a point are never joined.

use super::geom::P;
use super::stroke::{boolean, closing, pieces, shapes, union, Rings, Rule};
use super::WayLine;

/// An addition counts as real where a vertex lies this many units off every arm's carriageway: the
/// closing's own rounding leaves slivers thinner than this along straight edges.
const REAL: f64 = 3.0;

/// An arm: a way's line from the point's sample, toward `dir` (+1 or −1 in sample order).
#[derive(Clone, Copy, Debug)]
pub struct Arm {
    pub line: usize,
    pub at: usize,
    pub dir: i8,
}

/// Where a point's face ends on a way: on segment `seg` (samples seg, seg + 1) at fraction `t`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seam {
    pub line: usize,
    pub seg: usize,
    pub t: f64,
}

/// One connected part of what a closing adds, and the arms it touches.
pub struct Added {
    pub rings: Rings,
    pub arms: Vec<usize>,
}

pub struct Rounded {
    pub added: Vec<Added>,
    /// Per arm, its seam.
    pub seams: Vec<Option<Seam>>,
}

/// A point of an arm's run: its place as a fractional sample index, its point and half-width.
#[derive(Clone, Copy, Debug)]
struct RunPoint {
    x: f64,
    p: P,
    h: f64,
}

/// The run of an arm: from the point along its way for `len` units, ending early at another point's
/// sample (`stops`, sorted).
fn run(line: &WayLine, arm: Arm, len: f64, stops: &[usize]) -> Vec<RunPoint> {
    let mut out = vec![RunPoint { x: arm.at as f64, p: line.p[arm.at], h: line.h[arm.at] }];
    let mut i = arm.at as isize;
    let mut s = 0.0;
    loop {
        let j = i + isize::from(arm.dir);
        if j < 0 || j as usize >= line.p.len() {
            break;
        }
        let (a, b) = (line.p[i as usize], line.p[j as usize]);
        let seg = (f64::from(b[0] - a[0]).powi(2) + f64::from(b[1] - a[1]).powi(2)).sqrt();
        if s + seg >= len && seg > 0.0 {
            let g = (len - s) / seg;
            let p = [a[0] + libm::round(g * f64::from(b[0] - a[0])) as i32, a[1] + libm::round(g * f64::from(b[1] - a[1])) as i32];
            let h = line.h[i as usize] + g * (line.h[j as usize] - line.h[i as usize]);
            out.push(RunPoint { x: i as f64 + f64::from(arm.dir) * g, p, h });
            break;
        }
        s += seg;
        out.push(RunPoint { x: j as f64, p: b, h: line.h[j as usize] });
        if stops.binary_search(&(j as usize)).is_ok() {
            break;
        }
        i = j;
    }
    out
}

fn length(run: &[RunPoint]) -> f64 {
    run.windows(2).map(|w| (f64::from(w[1].p[0] - w[0].p[0]).powi(2) + f64::from(w[1].p[1] - w[0].p[1]).powi(2)).sqrt()).sum()
}

/// The station (units along the run) of the run point nearest q and q's distance past the run's
/// carriageway edge there.
fn station(run: &[RunPoint], q: P) -> (f64, f64) {
    let mut best = (0.0, f64::INFINITY, f64::INFINITY);
    let mut s0 = 0.0;
    for w in run.windows(2) {
        let (a, b) = (w[0].p, w[1].p);
        let (dx, dy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 {
            ((f64::from(q[0] - a[0]) * dx + f64::from(q[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (x, y) = (f64::from(a[0]) + t * dx - f64::from(q[0]), f64::from(a[1]) + t * dy - f64::from(q[1]));
        let dist = (x * x + y * y).sqrt();
        if dist < best.1 {
            best = (s0 + t * len2.sqrt(), dist, dist - (w[0].h + t * (w[1].h - w[0].h)));
        }
        s0 += len2.sqrt();
    }
    (best.0, best.2)
}

/// The seam at station s of a run.
fn place(line: usize, n: usize, run: &[RunPoint], s: f64) -> Seam {
    let mut s0 = 0.0;
    let mut x = run.last().map_or(0.0, |r| r.x);
    for w in run.windows(2) {
        let len = (f64::from(w[1].p[0] - w[0].p[0]).powi(2) + f64::from(w[1].p[1] - w[0].p[1]).powi(2)).sqrt();
        if s0 + len >= s && len > 0.0 {
            x = w[0].x + (s - s0) / len * (w[1].x - w[0].x);
            break;
        }
        s0 += len;
    }
    let seg = (libm::floor(x) as usize).min(n.saturating_sub(2));
    Seam { line, seg, t: (x - seg as f64).clamp(0.0, 1.0) }
}

/// The carriageway stroke of a run at half-width plus `pad` units: round at the point, square at its
/// far end.
fn stroke(run: &[RunPoint], pad: f64) -> Vec<Vec<P>> {
    let p: Vec<P> = run.iter().map(|r| r.p).collect();
    let d: Vec<f64> = run.iter().map(|r| r.h + pad).collect();
    let mut out = Vec::new();
    pieces(&p, &d, [true, false], false, &mut out);
    out
}

/// The rounding of a point at `pos` with its arms, closing radius `r` and arm length `len` (units).
/// A part that the closing adds is real where it reaches `REAL` units off every arm; a turn's part
/// that reaches an arm's far end is dropped (the legs run on closer than 2r: the gap stays). With
/// `seams`, each arm gets its seam `seam` units past the farthest real point on it or the farthest
/// point that another arm covers, and the additions are cut there.
#[allow(clippy::too_many_arguments)]
pub fn round(
    lines: &[WayLine],
    stops: &[Vec<usize>],
    pos: P,
    arms: &[Arm],
    r: f64,
    len: f64,
    seam: f64,
    seams: bool,
) -> Rounded {
    let runs: Vec<Vec<RunPoint>> = arms
        .iter()
        .map(|&a| {
            let own: Vec<usize> = stops[a.line].iter().copied().filter(|&s| s != a.at).collect();
            run(&lines[a.line], a, len, &own)
        })
        .collect();
    let ends: Vec<f64> = runs.iter().map(|r| length(r)).collect();
    let strokes: Vec<Rings> = runs.iter().map(|run| union(&stroke(run, 0.0), pos)).collect();
    let all: Vec<Vec<P>> = strokes.iter().flatten().cloned().collect();
    let held = union(&all, pos);
    let mut added: Vec<Added> = Vec::new();
    // Per arm, the stations that its seam must pass.
    let mut far = vec![0.0f64; arms.len()];
    if r > 0.0 {
        for part in shapes(&closing(&held, r, pos), &held, Rule::Difference, pos) {
            let mut touched = Vec::new();
            let mut real = false;
            let mut at_end = false;
            let mut on: Vec<Vec<(f64, f64)>> = Vec::new();
            for ring in &part {
                on.push(ring.iter().map(|_| (0.0, 0.0)).collect());
            }
            for (k, run) in runs.iter().enumerate() {
                let mut hit = false;
                for (ri, ring) in part.iter().enumerate() {
                    for (vi, &q) in ring.iter().enumerate() {
                        let (s, off) = station(run, q);
                        on[ri][vi] = if k == 0 || off < on[ri][vi].1 { (s, off) } else { on[ri][vi] };
                        if off <= REAL {
                            hit = true;
                            at_end |= s >= ends[k] - REAL;
                        }
                    }
                }
                if hit {
                    touched.push(k);
                }
            }
            // A vertex off every arm is real; so is a vertex next to one (where the arc meets an edge).
            for (ri, ring) in part.iter().enumerate() {
                let n = ring.len();
                for vi in 0..n {
                    let off = |i: usize| on[ri][i].1 > REAL;
                    if off(vi) {
                        real = true;
                    }
                    if off(vi) || off((vi + 1) % n) || off((vi + n - 1) % n) {
                        for (k, run) in runs.iter().enumerate() {
                            let (s, o) = station(run, ring[vi]);
                            if o <= REAL + r {
                                far[k] = far[k].max(s);
                            }
                        }
                    }
                }
            }
            if real && !(at_end && !seams) {
                added.push(Added { rings: part, arms: touched });
            }
        }
    }
    if !seams {
        return Rounded { added, seams: Vec::new() };
    }
    for i in 0..arms.len() {
        for j in 0..arms.len() {
            if j != i {
                for q in boolean(&strokes[i], &strokes[j], Rule::Intersect, pos).into_iter().flatten() {
                    let (s, off) = station(&runs[i], q);
                    if off <= REAL {
                        far[i] = far[i].max(s);
                    }
                }
            }
        }
    }
    let out: Vec<Option<Seam>> = (0..arms.len())
        .map(|k| Some(place(arms[k].line, lines[arms[k].line].p.len(), &runs[k], (far[k] + seam).min(ends[k]))))
        .collect();
    // The additions end at the seams: cut them by the arms' strokes, grown by r, up to the seams.
    if !added.is_empty() {
        let mut reach = Vec::new();
        for (k, run) in runs.iter().enumerate() {
            let s = (far[k] + seam).min(ends[k]);
            let cut = cut_run(run, s);
            reach.extend(stroke(&cut, r + 2.0));
        }
        let reach = union(&reach, pos);
        added = added
            .into_iter()
            .flat_map(|a| {
                let arms = a.arms.clone();
                shapes(&a.rings, &reach, Rule::Intersect, pos).into_iter().map(move |rings| Added { rings, arms: arms.clone() })
            })
            .collect();
    }
    Rounded { added, seams: out }
}

/// The run up to station s.
fn cut_run(run: &[RunPoint], s: f64) -> Vec<RunPoint> {
    let mut out = vec![run[0]];
    let mut s0 = 0.0;
    for w in run.windows(2) {
        let len = (f64::from(w[1].p[0] - w[0].p[0]).powi(2) + f64::from(w[1].p[1] - w[0].p[1]).powi(2)).sqrt();
        if s0 + len >= s {
            let g = if len > 0.0 { (s - s0) / len } else { 0.0 };
            let (a, b) = (w[0].p, w[1].p);
            let p = [a[0] + libm::round(g * f64::from(b[0] - a[0])) as i32, a[1] + libm::round(g * f64::from(b[1] - a[1])) as i32];
            out.push(RunPoint { x: w[0].x + g * (w[1].x - w[0].x), p, h: w[0].h + g * (w[1].h - w[0].h) });
            return out;
        }
        s0 += len;
        out.push(w[1]);
    }
    out
}
