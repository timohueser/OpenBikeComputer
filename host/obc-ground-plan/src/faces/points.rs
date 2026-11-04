//! The rounding where ways meet or turn: one local closing per graph point (a junction, a joint, or a
//! tight turn of one way) over the strokes of that point's own arms, and the point's face: its arms up
//! to a straight seam past the farthest point that the rounding or another arm reaches, plus what the
//! closing adds there. Ways that do not meet at a point are never joined.

use super::geom::P;
use super::stroke::{boolean, closing, dilate, disc, pieces, shapes, union, Rings, Rule};
use super::WayLine;

/// An addition counts as real where a vertex lies this many units off every arm's carriageway: the
/// closing's own rounding leaves slivers thinner than this along straight edges.
const REAL: f64 = 3.0;

/// A corner's zone reaches this many radii past the farthest point where its two arms overlap: the
/// round of a corner that opens at 45 degrees or more ends within it, and a gap between two arms that
/// run on side by side is filled no farther.
const ZONE: f64 = 2.5;

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

/// One connected part of what a closing of radius r adds, and the arms it touches.
struct Added {
    rings: Rings,
    arms: Vec<usize>,
    r: f64,
}

pub struct Rounded {
    /// What the closings add to the carriageway, in parts.
    pub added: Vec<Rings>,
    /// The band rings round those parts: paved shoulder, shoulder, verge, slope.
    pub rings: [Rings; 4],
    /// Per arm, its seam.
    pub seams: Vec<Option<Seam>>,
    /// The circle of each round: centre and radius in units.
    pub circles: Vec<([i32; 2], f64)>,
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
            let p = [
                a[0] + libm::round(g * f64::from(b[0] - a[0])) as i32,
                a[1] + libm::round(g * f64::from(b[1] - a[1])) as i32,
            ];
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
    run.windows(2)
        .map(|w| (f64::from(w[1].p[0] - w[0].p[0]).powi(2) + f64::from(w[1].p[1] - w[0].p[1]).powi(2)).sqrt())
        .sum()
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

/// The rounding of a point at `pos` with its arms and arm length `len` (units). Each pair (a, b, r)
/// of neighbouring arms gets a closing of radius r over those two arms' strokes. A part that a
/// closing adds stays where it touches two arms and reaches `REAL` units off every arm; a turn's part
/// that reaches an arm's far end goes (the legs run on closer than 2r: the gap stays). With `seams`,
/// each arm gets its seam `seam` units past the farthest point of a kept part on it, or the farthest
/// point that another arm covers, and the parts and their band rings end there.
#[allow(clippy::too_many_arguments)]
pub fn round(
    lines: &[WayLine],
    stops: &[Vec<usize>],
    pos: P,
    arms: &[Arm],
    pairs: &[(usize, usize, f64)],
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
    let held = union(&strokes.concat(), pos);
    // Where each two arms overlap.
    let n = arms.len();
    let mut overlap: Vec<Vec<Rings>> = vec![vec![Vec::new(); n]; n];
    for i in 0..n {
        for j in i + 1..n {
            overlap[i][j] = boolean(&strokes[i], &strokes[j], Rule::Intersect, pos);
            overlap[j][i] = overlap[i][j].clone();
        }
    }
    let mut parts: Vec<Added> = Vec::new();
    let mut circles = Vec::new();
    // Per arm, the station that its seam must pass.
    let mut far = vec![0.0f64; n];
    for &(a, b, r) in pairs.iter().filter(|p| p.2 > 0.0) {
        // The corner's zone: a disc about the point past the arms' overlap. The closing reads the
        // arms within 2r of the zone.
        let dist = |q: &P| (f64::from(q[0] - pos[0]).powi(2) + f64::from(q[1] - pos[1]).powi(2)).sqrt();
        let zone = overlap[a][b].iter().flatten().map(dist).fold(0.0, f64::max) + ZONE * r;
        let both = boolean(
            &[strokes[a].clone(), strokes[b].clone()].concat(),
            &[disc(pos, zone + 2.0 * r)],
            Rule::Intersect,
            pos,
        );
        // What the closing adds in the zone, grown by REAL units into the arms: a part's only edge is
        // its arc, so no two candidates share an edge that two booleans computed apart.
        let closed = closing(&both, r, pos);
        let added = boolean(&boolean(&closed, &held, Rule::Difference, pos), &[disc(pos, zone)], Rule::Intersect, pos);
        for part in shapes(&dilate(&added, REAL, pos), &closed, Rule::Intersect, pos) {
            // Per vertex: the least distance past an arm's edge, and which arms it touches.
            let mut touched = vec![false; arms.len()];
            let off: Vec<Vec<f64>> = part
                .iter()
                .map(|ring| {
                    ring.iter()
                        .map(|&q| {
                            let mut least = f64::INFINITY;
                            for (k, run) in runs.iter().enumerate() {
                                let o = station(run, q).1;
                                if o <= REAL {
                                    touched[k] = true;
                                }
                                least = least.min(o);
                            }
                            least
                        })
                        .collect()
                })
                .collect();
            let real = off.iter().flatten().any(|&o| o > REAL);
            if !real || touched.iter().filter(|&&t| t).count() < 2 {
                continue;
            }
            // A vertex off every arm, or next to one (where the arc meets an edge), bounds the seams.
            for (ring, off) in part.iter().zip(&off) {
                let n = ring.len();
                for v in 0..n {
                    if off[v] > REAL || off[(v + 1) % n] > REAL || off[(v + n - 1) % n] > REAL {
                        for (k, run) in runs.iter().enumerate() {
                            let (s, o) = station(run, ring[v]);
                            if o <= REAL + r {
                                far[k] = far[k].max(s);
                            }
                        }
                    }
                }
            }
            // The round's circle: through the first, middle and last arc points of its outer ring.
            let arc: Vec<P> = part[0].iter().zip(&off[0]).filter(|(_, &o)| o > REAL).map(|(&q, _)| q).collect();
            if arc.len() >= 3 {
                if let Some(c) = circle(arc[0], arc[arc.len() / 2], arc[arc.len() - 1]) {
                    circles.push((c, r));
                    // The round touches each arm it meets at the foot of its centre on that arm.
                    for (k, run) in runs.iter().enumerate().filter(|(k, _)| touched[*k]) {
                        far[k] = far[k].max(station(run, c).0);
                    }
                }
            }
            let arms = (0..arms.len()).filter(|&k| touched[k]).collect();
            parts.push(Added { rings: part, arms, r });
        }
    }
    let mut out = Rounded { added: Vec::new(), rings: Default::default(), seams: Vec::new(), circles };
    if seams {
        for i in 0..n {
            for j in 0..n {
                if j != i {
                    for &q in overlap[i][j].iter().flatten() {
                        let (s, o) = station(&runs[i], q);
                        if o <= REAL {
                            far[i] = far[i].max(s);
                        }
                    }
                }
            }
        }
        for k in 0..arms.len() {
            far[k] = (far[k] + seam).min(ends[k]);
        }
        out.seams =
            (0..arms.len()).map(|k| Some(place(arms[k].line, lines[arms[k].line].p.len(), &runs[k], far[k]))).collect();
    }
    // The parts and their band rings, cut where the seams end each arm: the arms' runs up to their
    // seams, grown past the parts' reach.
    let reach = |grow: f64| {
        let mut all = Vec::new();
        for (k, run) in runs.iter().enumerate() {
            all.extend(stroke(&cut_run(run, if seams { far[k] } else { ends[k] }), grow + 2.0));
        }
        union(&all, pos)
    };
    let r_max = parts.iter().map(|p| p.r).fold(0.0, f64::max);
    if parts.is_empty() {
        return out;
    }
    let cut = reach(r_max);
    for part in &parts {
        out.added.extend(shapes(&part.rings, &cut, Rule::Intersect, pos));
    }
    // A ring of band layer k round a part is as wide as the widest band k of the arms it touches; it
    // tapers into a narrower arm's band along its own outline.
    let band = |a: Arm, k: usize| lines[a.line].bands[a.at][k] - lines[a.line].h[a.at];
    for k in 0..4 {
        let mut grown: Vec<Vec<P>> = Vec::new();
        let mut widest = 0.0f64;
        for part in &parts {
            let d = part.arms.iter().map(|&i| band(arms[i], k)).fold(0.0, f64::max);
            let inner = if k == 0 { 0.0 } else { part.arms.iter().map(|&i| band(arms[i], k - 1)).fold(0.0, f64::max) };
            if d > inner {
                let ring = dilate(&part.rings, d, pos);
                grown.extend(ring);
                widest = widest.max(d);
            }
        }
        if !grown.is_empty() {
            out.rings[k] = boolean(&grown, &reach(r_max + widest), Rule::Intersect, pos);
        }
    }
    out
}

/// The centre of the circle through three points, if they are not on one line.
fn circle(a: P, b: P, c: P) -> Option<[i32; 2]> {
    let (bx, by) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    let (cx, cy) = (f64::from(c[0] - a[0]), f64::from(c[1] - a[1]));
    let d = 2.0 * (bx * cy - by * cx);
    if d == 0.0 {
        return None;
    }
    let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
    let ux = (cy * b2 - by * c2) / d;
    let uy = (bx * c2 - cx * b2) / d;
    Some([a[0] + libm::round(ux) as i32, a[1] + libm::round(uy) as i32])
}

/// The run up to station s.
fn cut_run(run: &[RunPoint], s: f64) -> Vec<RunPoint> {
    sub_run(run, 0.0, s)
}

/// The point of a run at station s, and the index of the run point after it.
fn at(run: &[RunPoint], s: f64) -> (RunPoint, usize) {
    let mut s0 = 0.0;
    for (k, w) in run.windows(2).enumerate() {
        let len = (f64::from(w[1].p[0] - w[0].p[0]).powi(2) + f64::from(w[1].p[1] - w[0].p[1]).powi(2)).sqrt();
        if s0 + len >= s {
            let g = if len > 0.0 { ((s - s0) / len).clamp(0.0, 1.0) } else { 0.0 };
            let (a, b) = (w[0].p, w[1].p);
            let p = [
                a[0] + libm::round(g * f64::from(b[0] - a[0])) as i32,
                a[1] + libm::round(g * f64::from(b[1] - a[1])) as i32,
            ];
            return (RunPoint { x: w[0].x + g * (w[1].x - w[0].x), p, h: w[0].h + g * (w[1].h - w[0].h) }, k + 1);
        }
        s0 += len;
    }
    (*run.last().unwrap(), run.len())
}

/// The run from station s0 to station s1.
fn sub_run(run: &[RunPoint], s0: f64, s1: f64) -> Vec<RunPoint> {
    let (first, i) = at(run, s0.max(0.0));
    let (last, j) = at(run, s1);
    let mut out = vec![first];
    out.extend(run[i.min(j)..j].iter().copied());
    if out.last().map(|r| r.p) != Some(last.p) {
        out.push(last);
    }
    out
}
