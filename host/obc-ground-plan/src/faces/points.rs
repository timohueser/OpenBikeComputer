//! The rounds where ways meet or turn. Per graph point (a junction, a joint, or a tight turn of one
//! way) and pair of neighbouring arms, a circle of the pair's radius or the largest smaller one that
//! fits: tangent to the two facing carriageway edges where they cross, else tangent to one arm's edge
//! where it leaves the point's other pieces (another arm's square end, a gap fill). Its fan: the arms'
//! centre lines up to the circle's feet and the arc between. A junction's or joint's region: per arm,
//! a stub up to a straight seam past the farthest foot or overlap. Ways that do not meet at a point
//! are never joined.

use super::geom::P;
use super::stroke::{left as left_of, off, pie_between, pieces_along, push_positive, unit as unit_of, Rings, STEPS};
use super::WayLine;

/// A fan's straight sides lie this many units inside the carriageway or the band it joins, so its
/// arc crosses their edge at an angle and no sliver is left between two edges that run together.
const INSET: f64 = 3.0;

/// The sides of a round at a corner of the paved outline run this many units inside the paved side:
/// closer, they would run along its edges within the overlay's snap, and the slice splits such
/// near-coincident edges again and again.
const FILL_INSET: f64 = 32.0;

/// The unit mean of two unit vectors.
fn mean(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let m = [a[0] + b[0], a[1] + b[1]];
    let l = (m[0] * m[0] + m[1] * m[1]).sqrt();
    if l > 0.0 {
        [m[0] / l, m[1] / l]
    } else {
        a
    }
}

/// A round smaller than this many units is none: the corner stays sharp. Below it an arc step is
/// under 11 units long, and the rounding of its ends to the unit grid turns it by up to 8°, near the
/// 10° of the step itself. The value is in units, as its reason is the unit grid.
const MIN_RADIUS: f64 = 60.0;

/// A circle touches an edge where its distance from the centre line is at most this many units over
/// the half-width plus its radius: the edge's samples and the circle's centre each lie up to 0.71
/// units off their exact place on the unit grid.
const TOUCH: f64 = 2.0;

/// The halvings of the scan for the place where an edge leaves the other pieces.
const SEARCH: usize = 10;

/// Each radius of the search for the largest round that fits is this share of the one before: a
/// round that wins is under the next radius up, which does not fit, by at most 15 %, and a 9 m
/// round tries at most 28 radii at 47° N.
const LADDER: f64 = 0.85;

/// The radii that a round of radius r tries, largest first, down to `MIN_RADIUS`. Whether a circle
/// fits is no monotonic function of its radius (a smaller circle can meet a piece that a larger one
/// clears), so each radius is tested in full and the first that fits wins.
fn ladder(r: f64) -> impl Iterator<Item = f64> {
    std::iter::successors(Some(r), |&rho| Some(rho * LADDER)).take_while(|&rho| rho >= MIN_RADIUS)
}

/// A stub of a point's region is this much wider than the carriageway on each side, in units (0.48 m
/// at 47° N, 0.35 m at 60° N): a margin for the strokes' mitres and tapers past the samples'
/// half-widths. The region relabels only its arms' carriageways, so a wider stub changes no other
/// face; a seam reaches as far past the carriageway before it grows across the fans (`grow`).
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
    /// Per arm, its seam: a cut across its carriageway and past it on both sides, from the arm's
    /// right to its left (past the cut lies away from the point).
    pub seams: Vec<Option<[P; 2]>>,
    /// The circle of each round.
    pub circles: Vec<Circle>,
    /// Per arm, its run and the station of the farthest foot of a round or crossing of facing edges.
    runs: Vec<Vec<RunPoint>>,
    pub far: Vec<f64>,
}

/// A round's circle: its centre and radius in units, its feet (the tangent points on the two edges
/// it joins: the ends of its arc), and its arc's points at the global directions strictly between
/// the feet, as its fan has them. A sharp corner is a circle of radius 0 with the corner as its
/// centre, its feet and its arc. Per foot, the width of the shoulder ring past the edge in units, and
/// the line of that edge (`usize::MAX` for none): the heights read the corner's core outline so.
#[derive(Clone, Debug)]
pub struct Circle {
    pub centre: P,
    pub radius: f64,
    pub feet: [P; 2],
    pub arc: Vec<P>,
    pub ring: [f64; 2],
    pub lines: [usize; 2],
}

impl Circle {
    /// The circle about c (relative, `abs` makes it absolute) of radius rho whose arc runs from
    /// angle a0 through `sweep` radians.
    fn new(abs: &dyn Fn([f64; 2]) -> P, c: [f64; 2], rho: f64, a0: f64, sweep: f64) -> Self {
        let at = |a: f64| abs([c[0] + rho * libm::cos(a), c[1] + rho * libm::sin(a)]);
        let arc = arc(c, rho, a0, sweep).into_iter().map(abs).collect();
        let (ring, lines) = ([0.0; 2], [usize::MAX; 2]);
        Circle { centre: abs(c), radius: rho, feet: [at(a0), at(a0 + sweep)], arc, ring, lines }
    }

    fn sharp(k: P) -> Self {
        Circle { centre: k, radius: 0.0, feet: [k, k], arc: vec![k], ring: [0.0; 2], lines: [usize::MAX; 2] }
    }

    /// The circle with the shoulder ring and the line at each foot.
    fn at_edges(self, ring: [f64; 2], lines: [usize; 2]) -> Self {
        Circle { ring, lines, ..self }
    }
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

/// A seam's cut grown so that it crosses every fan that reaches past it: `fans` by the side each
/// lies on (+1 the cut's own point's, −1 the far side). Past its seam a fan belongs to the face on
/// the other side, so the cut must split it from edge to edge. The cut grows by whole multiples of
/// its half, so it stays on the line of the stub's end that it was.
pub fn grow(cut: [P; 2], fans: &[(&[Vec<P>], f64)]) -> [P; 2] {
    let end = [(cut[0][0] + cut[1][0]) / 2, (cut[0][1] + cut[1][1]) / 2];
    let e = [cut[1][0] - end[0], cut[1][1] - end[1]];
    let half = (f64::from(e[0]) * f64::from(e[0]) + f64::from(e[1]) * f64::from(e[1])).sqrt();
    if half == 0.0 {
        return cut;
    }
    // Along the cut, and past it: the cut runs from the arm's right to its left.
    let n = [f64::from(e[0]) / half, f64::from(e[1]) / half];
    let out = [n[1], -n[0]];
    let rel = |v: P| [f64::from(v[0] - end[0]), f64::from(v[1] - end[1])];
    let dot = |a: [f64; 2], b: [f64; 2]| a[0] * b[0] + a[1] * b[1];
    let mut reach = half;
    for &(rings, side) in fans {
        for ring in rings {
            for (i, &v) in ring.iter().enumerate() {
                let (a, b) = (rel(v), rel(ring[(i + 1) % ring.len()]));
                let (sa, sb) = (side * dot(a, out), side * dot(b, out));
                if sa > 0.0 {
                    reach = reach.max(dot(a, n).abs() + INSET);
                }
                if (sa > 0.0) != (sb > 0.0) {
                    reach = reach.max(dot(lerp(a, b, sa / (sa - sb)), n).abs() + INSET);
                }
            }
        }
    }
    let k = libm::ceil(reach / half) as i32;
    [[end[0] - k * e[0], end[1] - k * e[1]], [end[0] + k * e[0], end[1] + k * e[1]]]
}

/// Each cut `old` of a point's seams and parts, as `new`; the same cut seen from the far side (a
/// shared seam's other point) reversed.
pub fn replace_cut(r: &mut Rounded, old: [P; 2], new: [P; 2]) {
    let swap = |c: &mut [P; 2]| {
        if *c == old {
            *c = new;
        } else if *c == [old[1], old[0]] {
            *c = [new[1], new[0]];
        }
    };
    r.seams.iter_mut().flatten().for_each(swap);
    r.spans.iter_mut().flat_map(|p| p.cuts.iter_mut().flatten()).for_each(swap);
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

/// The foot of q on a run: its station, the foot point, the half-width and band offsets there, and its
/// distance from q.
struct Foot {
    s: f64,
    at: [f64; 2],
    h: f64,
    bands: [f64; 4],
    dist: f64,
}

fn foot(run: &[RunPoint], q: [f64; 2]) -> Foot {
    let mut best = Foot { s: 0.0, at: run[0].q, h: run[0].h, bands: run[0].bands, dist: f64::INFINITY };
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
            best = Foot { s: s0 + t * len, at, h: w[0].h + t * (w[1].h - w[0].h), bands, dist: d };
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
/// carriageway edges at feet inside their runs and lies off every arm's carriageway (square at the
/// point) and off the point's gap fills and the other carriageways (`off`). It may cut `tol` units
/// into the arms' strokes: their chords leave the curve by as much.
fn round_at(
    runs: &[Vec<RunPoint>],
    a: usize,
    b: usize,
    rho: f64,
    tol: f64,
    off: &dyn Fn([f64; 2], f64) -> bool,
) -> Option<([f64; 2], Foot, Foot)> {
    // A foot at a joint whose two edges leave a gap is no tangent point: the circle misses the edge.
    let inside = |f: &Foot, run: &[RunPoint]| {
        let d = f.dist - f.h - rho;
        f.s > 0.0 && f.s < length(run) - 1.0 && d <= TOUCH && d >= -tol
    };
    crossings(&runs[a], &runs[b], rho).into_iter().find_map(|(_, c)| {
        let (fa, fb) = (foot(&runs[a], c), foot(&runs[b], c));
        let clear = (0..runs.len()).all(|k| to_piece(runs, k, c).d >= rho - tol);
        (inside(&fa, &runs[a]) && inside(&fb, &runs[b]) && clear && off(c, rho)).then_some((c, fa, fb))
    })
}

/// Whether a circle about c (relative to the point at `pos`) of radius rho lies off the carriageways
/// of `lines` but for the parts of its arms' lines on their runs (`own`: line, first and last
/// sample index).
fn off_lines(lines: &[WayLine], pos: P, own: &[(usize, f64, f64)], c: [f64; 2], rho: f64) -> bool {
    let q = [f64::from(pos[0]) + c[0], f64::from(pos[1]) + c[1]];
    lines.iter().enumerate().all(|(li, line)| {
        let mine = |i: usize| own.iter().any(|&(l, lo, hi)| l == li && i as f64 + 1.0 > lo && (i as f64) < hi);
        line.near(q, rho).filter(|&i| !mine(i)).all(|i| {
            let (a, b) = (line.p[i], line.p[i + 1]);
            let (a, b) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
            let (u, v) = ([b[0] - a[0], b[1] - a[1]], [q[0] - a[0], q[1] - a[1]]);
            let l2 = u[0] * u[0] + u[1] * u[1];
            let t = if l2 > 0.0 { ((v[0] * u[0] + v[1] * u[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
            dist(lerp(a, b, t), q) >= line.h[i] + t * (line.h[i + 1] - line.h[i]) + rho - 1.0
        })
    })
}

/// The nearest point of a piece to a point outside it: the distance (0 inside), the point, whether
/// it is a vertex of the piece's outline, and its arm and station (none on a fill).
#[derive(Clone, Copy)]
struct Near {
    d: f64,
    at: [f64; 2],
    vertex: bool,
    arm: Option<(usize, f64)>,
}

/// The nearest point of arm k's carriageway piece: its stroke along the run, square at the point.
fn to_piece(runs: &[Vec<RunPoint>], k: usize, q: [f64; 2]) -> Near {
    let run = &runs[k];
    let f = foot(run, q);
    if f.s > 0.0 {
        let n = unit(f.at, q);
        let at = [f.at[0] + f.h * n[0], f.at[1] + f.h * n[1]];
        return Near { d: (f.dist - f.h).max(0.0), at, vertex: false, arm: Some((k, f.s)) };
    }
    let (p, n, h) = (run[0].q, left(unit(run[0].q, run[1].q)), run[0].h);
    let e = ((q[0] - p[0]) * n[0] + (q[1] - p[1]) * n[1]).clamp(-h, h);
    let at = [p[0] + e * n[0], p[1] + e * n[1]];
    Near { d: dist(at, q), at, vertex: e.abs() == h, arm: Some((k, 0.0)) }
}

/// The nearest point of a ring.
fn to_ring(ring: &[[f64; 2]], q: [f64; 2]) -> Near {
    let (mut best, mut inside) = (Near { d: f64::INFINITY, at: q, vertex: false, arm: None }, false);
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1]], [q[0] - a[0], q[1] - a[1]]);
        if (a[1] > q[1]) != (b[1] > q[1]) && v[0] < v[1] / u[1] * u[0] {
            inside = !inside;
        }
        let l2 = u[0] * u[0] + u[1] * u[1];
        let t = if l2 > 0.0 { ((v[0] * u[0] + v[1] * u[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let at = lerp(a, b, t);
        if dist(at, q) < best.d {
            best = Near { d: dist(at, q), at, vertex: t == 0.0 || t == 1.0, arm: None };
        }
    }
    if inside {
        best.d = 0.0;
    }
    best
}

/// The distance from q to the point's pieces other than arm k: the other arms' carriageways and the
/// gap fills. With the nearest point, and its arm and station (none on a fill).
fn to_others(runs: &[Vec<RunPoint>], fills: &[Vec<[f64; 2]>], k: usize, q: [f64; 2]) -> Near {
    let arms = (0..runs.len()).filter(|&j| j != k).map(|j| to_piece(runs, j, q));
    let fills = fills.iter().map(|f| to_ring(f, q));
    arms.chain(fills).min_by(|x, y| x.d.total_cmp(&y.d)).unwrap_or(Near {
        d: f64::INFINITY,
        at: q,
        vertex: false,
        arm: None,
    })
}

/// The point at station s on a run's edge on `side`, `extra` units out.
fn edge_at(run: &[RunPoint], s: f64, side: f64, extra: f64) -> [f64; 2] {
    let mut s0 = 0.0;
    for (i, w) in run.windows(2).enumerate() {
        let len = dist(w[0].q, w[1].q);
        if s0 + len >= s || i + 2 == run.len() {
            let t = if len > 0.0 { ((s - s0) / len).clamp(0.0, 1.0) } else { 0.0 };
            let (q, n) = (lerp(w[0].q, w[1].q, t), left(unit(w[0].q, w[1].q)));
            let d = side * (w[0].h + t * (w[1].h - w[0].h) + extra);
            return [q[0] + d * n[0], q[1] + d * n[1]];
        }
        s0 += len;
    }
    run[0].q
}

/// A round where an arm's edge leaves the point's other pieces: its centre, the arm whose edge it
/// touches and the foot there, and where it touches the other pieces.
struct Leave {
    c: [f64; 2],
    k: usize,
    foot: Foot,
    touch: Near,
}

/// The scan step along an edge for the place where it leaves the other pieces, in units (0.2 m at
/// 47° N, a fifth of `SUPPORT_M`): an edge that leaves the pieces for less and comes back reads as
/// not leaving, a shape under the length over which the kink check reads a corner.
const SCAN: f64 = 128.0;

/// The round of radius rho on arm k's edge on `side` where that edge leaves the point's other pieces
/// (`fills` and the other arms' carriageways, square at the point): the circle tangent to the edge
/// at the first station where it lies rho off them. None where the edge starts clear of them (no
/// corner), or where the circle leaves past an arm's run or meets the arm's own carriageway or
/// another one (`off`).
fn leave(
    runs: &[Vec<RunPoint>],
    fills: &[Vec<[f64; 2]>],
    k: usize,
    side: f64,
    rho: f64,
    tol: f64,
    off: &dyn Fn([f64; 2], f64) -> bool,
) -> Option<Leave> {
    let run = &runs[k];
    let total = length(run);
    let gap = |s: f64| to_others(runs, fills, k, edge_at(run, s, side, rho)).d - rho;
    if gap(0.0) > -TOUCH {
        return None;
    }
    let (mut lo, mut hi) = (0.0, SCAN);
    while gap(hi) < 0.0 {
        lo = hi;
        hi += SCAN;
        if hi >= total - 1.0 {
            return None;
        }
    }
    for _ in 0..SEARCH + 2 {
        let mid = 0.5 * (lo + hi);
        if gap(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let c = edge_at(run, hi, side, rho);
    let near = to_others(runs, fills, k, c);
    if to_piece(runs, k, c).d < rho - tol || near.arm.is_some_and(|(j, s)| s >= length(&runs[j]) - 1.0) || !off(c, rho)
    {
        return None;
    }
    Some(Leave { c, k, foot: foot(run, c), touch: near })
}

/// The fan of a round where an arm's edge leaves the other pieces: the arm's centre line to the foot,
/// the arc that faces the corner (under half a turn), and back through the touched piece to the
/// point. Its chords run into the two pieces, or end on the touched piece's corner itself, so no step
/// is left beside it. With the arc's start angle and sweep. None for an arc under one arc step: that
/// corner stays sharp, as the strokes mitre such a join.
fn leave_fan(runs: &[Vec<RunPoint>], l: &Leave, rho: f64) -> Option<(Vec<[f64; 2]>, f64, f64)> {
    let pi = std::f64::consts::PI;
    let n = unit(l.foot.at, l.c);
    let t = [l.foot.at[0] + l.foot.h * n[0], l.foot.at[1] + l.foot.h * n[1]];
    let into = |p: [f64; 2]| {
        let u = unit(l.c, p);
        [p[0] + INSET * u[0], p[1] + INSET * u[1]]
    };
    let a0 = angle([t[0] - l.c[0], t[1] - l.c[1]]);
    let a1 = angle([l.touch.at[0] - l.c[0], l.touch.at[1] - l.c[1]]);
    let sweep = (a1 - a0 + pi).rem_euclid(2.0 * pi) - pi;
    if sweep.abs() < 2.0 * pi / f64::from(STEPS) {
        return None;
    }
    let mut fan = up_to(&runs[l.k], l.foot.s);
    fan.push(into(t));
    fan.extend(arc(l.c, rho, a0, sweep));
    fan.push(if l.touch.vertex { l.touch.at } else { into(l.touch.at) });
    match l.touch.arm {
        Some((j, s)) => fan.extend(up_to(&runs[j], s).into_iter().rev()),
        None => fan.push([0.0, 0.0]),
    }
    Some((fan, a0, sweep))
}

/// A round's circle: tangent to the facing edges of both arms, or to one arm's edge where it leaves
/// the point's other pieces.
enum Fit {
    Edges([f64; 2], Foot, Foot),
    Leaves(Leave),
}

/// The angle of v.
fn angle(v: [f64; 2]) -> f64 {
    libm::atan2(v[1], v[0])
}

/// The arc about c of radius rho from angle a0 sweeping `sweep` radians (signed): its points at the
/// global directions strictly between its ends (36 steps, as the strokes'). Its ends, the tangent
/// points on an edge, are left out: a rounded tangent point lies a unit off the stroke's own edge
/// and would leave a step there. The fan's chord from inside the edge to the first point crosses
/// the edge at an angle instead.
fn arc(c: [f64; 2], rho: f64, a0: f64, sweep: f64) -> Vec<[f64; 2]> {
    arc_angles(a0, sweep).into_iter().map(|a| [c[0] + rho * libm::cos(a), c[1] + rho * libm::sin(a)]).collect()
}

/// The global directions strictly between angle a0 and a0 + sweep, in order from a0.
fn arc_angles(a0: f64, sweep: f64) -> Vec<f64> {
    let step = 2.0 * std::f64::consts::PI / f64::from(STEPS);
    let mut out = Vec::new();
    let a1 = a0 + sweep;
    let eps = 0.01;
    if sweep >= 0.0 {
        let mut k = libm::floor(a0 / step) as i64 + 1;
        while (k as f64) * step < a1 - eps {
            if (k as f64) * step > a0 + eps {
                out.push(k as f64 * step);
            }
            k += 1;
        }
    } else {
        let mut k = libm::ceil(a0 / step) as i64 - 1;
        while (k as f64) * step > a1 + eps {
            if (k as f64) * step < a0 - eps {
                out.push(k as f64 * step);
            }
            k -= 1;
        }
    }
    out
}

/// The signed sweep from angle a0 to a1 that passes the direction `toward`.
fn sweep_toward(a0: f64, a1: f64, toward: f64) -> f64 {
    let tau = 2.0 * std::f64::consts::PI;
    let ccw = (a1 - a0).rem_euclid(tau);
    let mid = (toward - a0).rem_euclid(tau);
    if mid <= ccw {
        ccw
    } else {
        ccw - tau
    }
}

/// One side of a round: the path from the corner to its tangent point on the paved side, and the
/// band layers' outer offsets past the carriageway's edge there.
struct Side {
    path: Vec<[f64; 2]>,
    w: [f64; 4],
}

/// The band rings of a round about c of radius rho, its arc from angle a0 turning `sweep`: per layer
/// (paved shoulder, shoulder, verge, slope), the arc at an offset that runs from the first side's own
/// band to the second side's, so each end meets its side's band smoothly and a band that one side
/// lacks tapers out along the arc. None for a layer no wider than
/// the layer inside it on both sides, or as wide as the round.
fn band_rings(c: [f64; 2], rho: f64, a0: f64, sweep: f64, sides: [&Side; 2]) -> [Option<Vec<[f64; 2]>>; 4] {
    let [a, b] = sides;
    // Each layer's own width past the layer inside it, per side.
    let own = |s: &Side, k: usize| (s.w[k] - if k == 0 { 0.0 } else { s.w[k - 1] }).max(0.0);
    // The width of a layer at the share t of the arc: a cubic with no slope at a side that has the
    // layer (it meets that band's edge smoothly) and the straight slope at a side that has none (a
    // taper with an angle).
    let width = |k: usize, t: f64| {
        let (wa, wb) = (own(a, k), own(b, k));
        let (ma, mb) = (if wa > 0.0 { 0.0 } else { wb - wa }, if wb > 0.0 { 0.0 } else { wb - wa });
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * wa + (t3 - 2.0 * t2 + t) * ma + (-2.0 * t3 + 3.0 * t2) * wb + (t3 - t2) * mb
    };
    std::array::from_fn(|k| {
        let outer = a.w[k].max(b.w[k]);
        if outer <= 0.0 || rho - outer < INSET || (own(a, k) <= 0.0 && own(b, k) <= 0.0) {
            return None;
        }
        let at = |phi: f64| {
            let t = ((phi - a0) / sweep).clamp(0.0, 1.0);
            let r = rho - (0..=k).map(|j| width(j, t)).sum::<f64>();
            [c[0] + r * libm::cos(phi), c[1] + r * libm::sin(phi)]
        };
        let mut ring = a.path.clone();
        ring.push(at(a0));
        ring.extend(arc_angles(a0, sweep).into_iter().map(at));
        ring.push(at(a0 + sweep));
        ring.extend(b.path.iter().rev());
        Some(ring)
    })
}

/// The fills between the square ends of a point's arms where two neighbouring arms open by half a
/// turn or more: a fan about the point from one arm's corner to the other's, built from the strokes'
/// own corner points. An arm whose square end lies inside the other arms' layer is no neighbour: it
/// splits no gap between them.
fn fill_gaps(lines: &[WayLine], pos: P, arms: &[Arm], layer: usize, out: &mut Vec<Vec<P>>) {
    let tau = 2.0 * std::f64::consts::PI;
    let cuts = cuts(lines, arms);
    let by: Vec<(f64, [f64; 2], [f64; 5], Arm)> = arms
        .iter()
        .filter_map(|&a| {
            let line = &lines[a.line];
            let next = line.p.get(a.at.checked_add_signed(isize::from(a.dir))?)?;
            let u = unit_of(pos, *next)?;
            Some((libm::atan2(u[1], u[0]), u, line.offsets(a.at), a))
        })
        .collect();
    let inside = |q: [f64; 2], k: usize| {
        let (u, w) = (by[k].1, by[k].2[layer]);
        q[0] * u[0] + q[1] * u[1] > 1.0 && (q[0] * u[1] - q[1] * u[0]).abs() < w - 1.0
    };
    let buried: Vec<bool> = (0..by.len())
        .map(|k| {
            let (n, w) = (left_of(by[k].1), by[k].2[layer]);
            [1.0, -1.0].iter().all(|&side| {
                let q = [side * w * n[0], side * w * n[1]];
                (0..by.len()).any(|j| j != k && inside(q, j))
            })
        })
        .collect();
    let mut by: Vec<_> = by.iter().zip(&buried).filter(|(_, &b)| !b).map(|(a, _)| *a).collect();
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
/// r, tol) of neighbouring arms, a turning counter-clockwise to b through their sector (at a way's
/// own turn, the inner side), with the radius and how far the round may cut into the arms' strokes.
/// With `gaps`, a fill between square ends of arms that open by half a turn or more (junctions and
/// joints).
#[allow(clippy::too_many_arguments)]
pub fn round(
    lines: &[WayLine],
    stops: &[Vec<usize>],
    pos: P,
    arms: &[Arm],
    pairs: &[(usize, usize, f64, f64)],
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
        seams: vec![None; arms.len()],
        circles: Vec::new(),
        runs: Vec::new(),
        far: vec![0.0; arms.len()],
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
    // A round lies off every carriageway: its arms' runs, and the other lines but for those runs.
    let own: Vec<(usize, f64, f64)> = (runs.iter().zip(arms))
        .map(|(r, a)| {
            let (x0, x1) = (r[0].x, r[r.len() - 1].x);
            (a.line, x0.min(x1), x0.max(x1))
        })
        .collect();
    // The gap fills about the point: a round where an arm's edge leaves them touches them.
    let fills: Vec<Vec<[f64; 2]>> = (out.fans.iter())
        .map(|r| r.iter().map(|p| [f64::from(p[0] - pos[0]), f64::from(p[1] - pos[1])]).collect())
        .collect();
    let off = |c: [f64; 2], rho: f64| {
        fills.iter().all(|f| to_ring(f, c).d >= rho - 1.0) && off_lines(lines, pos, &own, c, rho)
    };
    let (tau, step) = (2.0 * std::f64::consts::PI, 2.0 * std::f64::consts::PI / f64::from(STEPS));
    for &(i, j, r, tol) in pairs {
        // a turns counter-clockwise to b through the pair's sector: as given at a junction or a
        // joint, the inner side at a way's own turn.
        let (a, b) =
            if gaps || (heading(j) - heading(i)).rem_euclid(tau) <= std::f64::consts::PI { (i, j) } else { (j, i) };
        // Under half a turn the arms' carriageways meet; past it a gap fill or the stroke's own join
        // lies between them, and only an arm's edge that leaves a fill has a corner.
        let open = (heading(b) - heading(a)).rem_euclid(tau) < std::f64::consts::PI;
        let through = arms[a].line == arms[b].line && arms[a].at == arms[b].at;
        if !open && (through || !gaps) {
            continue;
        }
        let corner = if open { crossings(&runs[a], &runs[b], 0.0).first().map(|&(_, k)| k) } else { None };
        if let Some(k) = corner {
            far[a] = far[a].max(foot(&runs[a], k).s);
            far[b] = far[b].max(foot(&runs[b], k).s);
        }
        let leaves = |rho: f64| {
            leave(&runs, &fills, a, 1.0, rho, tol, &off).or_else(|| leave(&runs, &fills, b, -1.0, rho, tol, &off))
        };
        if r < MIN_RADIUS {
            // A pair of no radius keeps its sharp corner: a circle of radius 0 there.
            if let Some(k) = corner.or_else(|| leaves(2.0 * TOUCH).map(|l| l.touch.at)).filter(|_| r == 0.0) {
                out.circles.push(Circle::sharp(abs(k)));
            }
            continue;
        }
        // Facing edges that cross make the corner; else one arm's edge leaves the other pieces.
        let fit = |rho: f64| match corner {
            Some(_) => round_at(&runs, a, b, rho, tol, &off).map(|(c, fa, fb)| Fit::Edges(c, fa, fb)),
            None => leaves(rho).map(Fit::Leaves),
        };
        let Some((rho, fit)) = ladder(r).find_map(|rho| fit(rho).map(|f| (rho, f))) else { continue };
        let (c, fa, fb) = match fit {
            Fit::Edges(c, fa, fb) => (c, fa, fb),
            Fit::Leaves(l) => {
                if let Some((fan, a0, sweep)) = leave_fan(&runs, &l, rho) {
                    far[l.k] = far[l.k].max(l.foot.s);
                    if let Some((j, s)) = l.touch.arm {
                        far[j] = far[j].max(s);
                    }
                    let ring = l.foot.bands[1] - l.foot.h;
                    let lines = [arms[l.k].line, l.touch.arm.map_or(usize::MAX, |(j, _)| arms[j].line)];
                    out.circles.push(Circle::new(&abs, l.c, rho, a0, sweep).at_edges([ring; 2], lines));
                    push_positive(fan.iter().map(|&q| abs(q)).collect(), &mut out.fans);
                }
                continue;
            }
        };
        // The arc faces the point. A corner that turns less than one arc step stays sharp, as the
        // strokes mitre such a join.
        let (na, nb) = (unit(fa.at, c), unit(fb.at, c));
        let (a0, a1) = (angle([-na[0], -na[1]]), angle([-nb[0], -nb[1]]));
        let sweep = sweep_toward(a0, a1, angle([-c[0], -c[1]]));
        if sweep.abs() < step {
            continue;
        }
        far[a] = far[a].max(fa.s);
        far[b] = far[b].max(fb.s);
        let ring = [fa.bands[1] - fa.h, fb.bands[1] - fb.h];
        out.circles.push(Circle::new(&abs, c, rho, a0, sweep).at_edges(ring, [arms[a].line, arms[b].line]));
        let side_a = up_to(&runs[a], fa.s);
        let side_b = up_to(&runs[b], fb.s);
        let mut fan: Vec<[f64; 2]> = side_a.clone();
        let edge_point = |f: &Foot, n: [f64; 2], d: f64| [f.at[0] + d * n[0], f.at[1] + d * n[1]];
        fan.push(edge_point(&fa, na, fa.h - INSET));
        fan.extend(arc(c, rho, a0, sweep));
        fan.push(edge_point(&fb, nb, fb.h - INSET));
        fan.extend(side_b.iter().rev());
        push_positive(fan.iter().map(|&q| abs(q)).collect(), &mut out.fans);
        let side = |path: Vec<[f64; 2]>, f: &Foot| Side { path, w: std::array::from_fn(|k| f.bands[k] - f.h) };
        let rings = band_rings(c, rho, a0, sweep, [&side(side_a, &fa), &side(side_b, &fb)]);
        for (k, ring) in rings.into_iter().enumerate() {
            if let Some(ring) = ring {
                push_positive(ring.iter().map(|&q| abs(q)).collect(), &mut out.rings[k]);
            }
        }
    }
    // An arm belongs to the point's face as far as a fan of the point lies over its carriageway: a
    // fan of two other arms can reach over an arm that runs close beside one of them.
    for fan in &out.fans {
        for &v in fan {
            let q = [f64::from(v[0] - pos[0]), f64::from(v[1] - pos[1])];
            for (k, run) in runs.iter().enumerate() {
                let f = foot(run, q);
                if f.s > 0.0 && f.dist < f.h - INSET {
                    far[k] = far[k].max(f.s);
                }
            }
        }
    }
    out.far = far;
    out.runs = runs;
    out
}

/// The round at a corner `v` of the paved outline: the circle tangent to the outline's edge before
/// the corner (`back`, walked backward from it, the land on its left) and the edge after it (`ahead`,
/// the land on its right), off the paved area (`off`), with its feet within `reach` along the edges,
/// of radius r or the first of the ladder that fits. Its ring: the corner, the edges to the feet and
/// the arc between, with the corner and edges `FILL_INSET` units into the paved side, so its sides
/// run inside the paved area and leave no sliver beside its edges. None where the arc would turn
/// less than one arc step.
#[allow(clippy::too_many_arguments)]
pub fn corner_round(
    v: P,
    back: &[P],
    ahead: &[P],
    r: f64,
    tol: f64,
    reach: f64,
    off: &dyn Fn([f64; 2], f64) -> bool,
    bands: &dyn Fn([f64; 2]) -> [f64; 4],
) -> Option<super::outline::Fill> {
    let run = |chain: &[P]| -> Vec<RunPoint> {
        (chain.iter().enumerate())
            .map(|(k, p)| RunPoint {
                x: k as f64,
                q: [f64::from(p[0] - v[0]), f64::from(p[1] - v[1])],
                h: 0.0,
                bands: [0.0; 4],
            })
            .collect()
    };
    let runs = [run(back), run(ahead)];
    if runs.iter().any(|r| r.len() < 2) {
        return None;
    }
    // The arc faces the corner and turns less than half a turn: the land between two edges that
    // meet at a corner is narrower than a half plane. A circle may cut into the edges by `tol` (their
    // chords leave the curve by as much), at most a quarter of its radius: an edge then crosses the
    // circle within 41° (acos 0.75) of its tangent.
    let arc_of = |rho: f64| {
        let (c, fa, fb) = round_at(&runs, 0, 1, rho, tol.min(rho / 4.0), off)?;
        if fa.s > reach || fb.s > reach {
            return None;
        }
        let (a0, a1) = (angle([fa.at[0] - c[0], fa.at[1] - c[1]]), angle([fb.at[0] - c[0], fb.at[1] - c[1]]));
        let sweep = sweep_toward(a0, a1, angle([-c[0], -c[1]]));
        (sweep.abs() < std::f64::consts::PI).then_some((rho, c, fa, fb, a0, sweep))
    };
    let (rho, c, fa, fb, a0, sweep) = ladder(r).find_map(arc_of)?;
    if sweep.abs() < 2.0 * std::f64::consts::PI / f64::from(STEPS) {
        return None;
    }
    // Each edge to its foot, moved `FILL_INSET` units into the paved side along its normals: the
    // land lies on the left of `back` and on the right of `ahead`.
    let side = |chain: Vec<[f64; 2]>, land: f64| -> Vec<[f64; 2]> {
        let normal = |a: [f64; 2], b: [f64; 2]| {
            let n = left(unit(a, b));
            [-land * n[0], -land * n[1]]
        };
        let m = chain.len();
        (1..m)
            .map(|k| {
                let n0 = normal(chain[k - 1], chain[k]);
                let n = if k + 1 < m { mean(n0, normal(chain[k], chain[k + 1])) } else { n0 };
                [chain[k][0] + FILL_INSET * n[0], chain[k][1] + FILL_INSET * n[1]]
            })
            .collect()
    };
    let (a_side, b_side) = (side(up_to(&runs[0], fa.s), 1.0), side(up_to(&runs[1], fb.s), -1.0));
    let at = |q: [f64; 2]| [f64::from(v[0]) + q[0], f64::from(v[1]) + q[1]];
    let first = |run: &[RunPoint], land: f64| {
        let n = left(unit(run[0].q, run[1].q));
        [-land * n[0], -land * n[1]]
    };
    let nv = mean(first(&runs[0], 1.0), first(&runs[1], -1.0));
    let corner = [FILL_INSET * nv[0], FILL_INSET * nv[1]];
    // The band rings about the arc, at the band offsets of the edges at the feet, as a point's round
    // has them.
    let edge_side = |path: &[[f64; 2]], f: &Foot| {
        let mut path = path.to_vec();
        path.insert(0, corner);
        Side { path, w: bands(at(f.at)) }
    };
    let band = band_rings(c, rho, a0, sweep, [&edge_side(&a_side, &fa), &edge_side(&b_side, &fb)]);
    let mut ring = vec![corner];
    ring.extend(a_side);
    ring.extend(arc(c, rho, a0, sweep));
    ring.extend(b_side.into_iter().rev());
    let abs = |q: [f64; 2]| [v[0] + libm::round(q[0]) as i32, v[1] + libm::round(q[1]) as i32];
    let mut pts: Vec<P> = ring.iter().map(|&q| abs(q)).collect();
    pts.dedup();
    let mut rings = Vec::new();
    push_positive(pts, &mut rings);
    // The label comes from the face along the edge before the corner, halfway to its foot: at the
    // corner itself faces of other ways can meet.
    let inside = edge_at(&runs[0], fa.s / 2.0, -1.0, 2.0 * FILL_INSET);
    let abs_ring = |ring: Vec<[f64; 2]>| {
        let mut out = Vec::new();
        push_positive(ring.iter().map(|&q| abs(q)).collect(), &mut out);
        out
    };
    Some(super::outline::Fill {
        ring: rings.pop()?,
        bands: band.map(|ring| ring.map(abs_ring).unwrap_or_default()),
        circle: Circle::new(&abs, c, rho, a0, sweep).at_edges([&fa, &fb].map(|f| bands(at(f.at))[1]), [usize::MAX; 2]),
        inside: [
            2 * i64::from(v[0]) + libm::round(2.0 * inside[0]) as i64,
            2 * i64::from(v[1]) + libm::round(2.0 * inside[1]) as i64,
        ],
    })
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
        r.seams[k] = Some(cut);
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
