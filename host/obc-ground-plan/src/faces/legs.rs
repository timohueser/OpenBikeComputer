//! The legs of a way that passes beside itself, and the ground between two parts of roads and rails
//! that pass beside each other a step apart. Two points are two parts of a line where they lie within
//! the reach of its class (their cores at their widest touch) while the line between them leaves both
//! those cores. Two parts conflict where their heights differ by more than the step at which
//! heights stop blending two lines. Conflicts whose spans share points between their parts form one
//! cluster, and the middle point of that common span cuts the line, so no leg holds both parts of a
//! conflict. Consecutive legs share the cut point. Each leg is a line of its own in the records, with the
//! frame of a line that ends at its cut. Every candidate that gives the way's label, and the way's own
//! shoulders, split between its legs: the medial lines between any two legs cut it, and each piece goes
//! to its nearest leg, a tie (the outer side of a cut, where the cut point is nearest both) to the side
//! of the cut's mitre line. The ground between two conflicting parts, of one line or of two, is their
//! bridge (`bridges`): it takes the higher part as its host, so one wall stands at the lower one.

use super::geom::P;
use std::collections::BTreeMap;

/// The cuts of a line, as ascending indices of its points `p`: `half` is the carriageway's half-width
/// at each point (units), `z` the height (metres), `dz` the step past which two parts conflict and
/// `reach(o)` twice the widest core of the line's class in units at o. A cluster with a pair whose
/// carriageways touch is one paved surface: it does not cut, so no wall stands on the asphalt.
pub fn cuts(p: &[P], half: &[f64], z: &[f64], dz: f64, reach: &dyn Fn(P) -> f64) -> Vec<usize> {
    let n = p.len();
    let span = libm::ceil(p.iter().map(|&o| reach(o)).fold(0.0, f64::max)) as i64;
    let mut by_x: Vec<usize> = (0..n).collect();
    by_x.sort_by_key(|&i| (p[i][0], i));
    // The conflicts, each with whether the carriageways of its two parts touch.
    let mut pairs: Vec<(usize, usize, bool)> = Vec::new();
    for i in 0..n {
        let x = i64::from(p[i][0]);
        let lo = by_x.partition_point(|&k| i64::from(p[k][0]) < x - span);
        let hi = by_x.partition_point(|&k| i64::from(p[k][0]) <= x + span);
        let r = reach(p[i]);
        for &j in &by_x[lo..hi] {
            let d = dist(p[i], p[j]);
            if j >= i + 2 && (z[i] - z[j]).abs() > dz && d <= r && leaves(p, r / 2.0, i, j) {
                pairs.push((i, j, d <= half[i] + half[j]));
            }
        }
    }
    // In the order of their first part, the pairs whose spans keep a point strictly inside all of
    // them form a cluster; its cut is the middle point of that common span. A later cluster starts
    // where that span would close, past the earlier cut.
    pairs.sort_unstable();
    let mut out = Vec::new();
    let mut span: Option<(usize, usize, bool)> = None;
    for (i, j, touch) in pairs {
        span = match span {
            Some((a, b, t)) if i.max(a) + 1 < j.min(b) => Some((i.max(a), j.min(b), t || touch)),
            Some((a, b, t)) => {
                out.extend((!t).then_some((a + b) / 2));
                Some((i, j, touch))
            }
            None => Some((i, j, touch)),
        };
    }
    out.extend(span.filter(|s| !s.2).map(|(a, b, _)| (a + b) / 2));
    out
}

fn dist(a: P, b: P) -> f64 {
    libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1]))
}

/// Whether the line from point i to point j leaves the discs of radius r about both: a point of it lies
/// outside both. Exact per segment: the parts of a segment inside the two discs do not cover it.
fn leaves(p: &[P], r: f64, i: usize, j: usize) -> bool {
    // The parameters of segment a b (relative to the disc's centre) inside a disc of radius r.
    let inside = |a: [f64; 2], b: [f64; 2], r: f64| -> Option<(f64, f64)> {
        let d = [b[0] - a[0], b[1] - a[1]];
        let qa = d[0] * d[0] + d[1] * d[1];
        let qb = d[0] * a[0] + d[1] * a[1];
        let qc = a[0] * a[0] + a[1] * a[1] - r * r;
        if qa == 0.0 {
            return (qc <= 0.0).then_some((0.0, 1.0));
        }
        let disc = qb * qb - qa * qc;
        if disc < 0.0 {
            return None;
        }
        let s = disc.sqrt();
        let (lo, hi) = (((-qb - s) / qa).max(0.0), ((-qb + s) / qa).min(1.0));
        (lo <= hi).then_some((lo, hi))
    };
    let rel = |k: usize, c: usize| [f64::from(p[k][0] - p[c][0]), f64::from(p[k][1] - p[c][1])];
    (i..j).any(|k| {
        let a = inside(rel(k, i), rel(k + 1, i), r);
        let b = inside(rel(k, j), rel(k + 1, j), r);
        match (a, b) {
            (None, None) => true,
            (Some((l, h)), None) | (None, Some((l, h))) => l > 0.0 || h < 1.0,
            (Some(a), Some(b)) => {
                let (first, second) = if a.0 <= b.0 { (a, b) } else { (b, a) };
                first.0 > 0.0 || second.0 > first.1 || first.1.max(second.1) < 1.0
            }
        }
    })
}

/// The leg of a line with `cuts` that an arm at point `at` leaves along: forward (`dir` > 0) the leg of
/// the segment from `at`, else the leg of the segment to it. A point that is not a cut lies on one leg.
pub fn leg(cuts: &[usize], at: usize, dir: i8) -> u8 {
    cuts.iter().filter(|&&c| if dir > 0 { c <= at } else { c < at }).count() as u8
}

/// The first and last point of leg `leg` of a line of `n` points.
pub fn range(cuts: &[usize], n: usize, leg: u8) -> (usize, usize) {
    let k = usize::from(leg);
    (if k == 0 { 0 } else { cuts[k - 1] }, cuts.get(k).copied().unwrap_or(n - 1))
}

/// The first and last point of each leg of a line of `n` points.
pub fn ranges(cuts: &[usize], n: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut lo = 0;
    for &c in cuts {
        out.push((lo, c));
        lo = c;
    }
    out.push((lo, n - 1));
    out
}

/// The distance from q to the polyline p, both relative to one origin.
fn to_line(p: &[[f64; 2]], q: [f64; 2]) -> f64 {
    let mut best = f64::INFINITY;
    for w in p.windows(2) {
        best = best.min(to_segment(w[0], w[1], q));
    }
    if p.len() == 1 {
        best = libm::hypot(q[0] - p[0][0], q[1] - p[0][1]);
    }
    best
}

fn to_segment(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let f = if l2 > 0.0 { (((q[0] - a[0]) * ab[0] + (q[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    libm::hypot(q[0] - a[0] - f * ab[0], q[1] - a[1] - f * ab[1])
}

fn unit(v: [f64; 2]) -> [f64; 2] {
    let l = libm::hypot(v[0], v[1]);
    if l > 0.0 {
        [v[0] / l, v[1] / l]
    } else {
        [1.0, 0.0]
    }
}

/// The legs of one line in a cell: their points and the cuts with their mitre directions, in cell
/// units, and the reach of each leg from its class. Every value derives from differences to a leg's
/// or a cut's point, so two cells that hold the legs give the same results.
pub struct Legs {
    /// Per leg, its points.
    legs: Vec<Vec<P>>,
    /// Per leg, its points relative to its first one.
    rel: Vec<Vec<[f64; 2]>>,
    /// Per leg, its box: [x0, y0, x1, y1].
    boxes: Vec<[f64; 4]>,
    /// Per leg, twice the widest core of its class (units at its first cut): the largest distance
    /// between the centres of two parts that can conflict, as their cores touch there (`cuts`).
    reach: Vec<f64>,
    /// Per cut, its point and the direction of the line there (the bisector of its two chords).
    mitres: Vec<(P, [f64; 2])>,
}

impl Legs {
    /// The legs of points `p` at `cuts`; `reach(o)` is twice the widest core of the line's class in
    /// units at o.
    pub fn new(p: &[P], cuts: &[usize], reach: &dyn Fn(P) -> f64) -> Legs {
        let ranges = ranges(cuts, p.len());
        let legs: Vec<Vec<P>> = ranges.iter().map(|&(lo, hi)| p[lo..=hi].to_vec()).collect();
        let rel = (legs.iter())
            .map(|l| l.iter().map(|v| [f64::from(v[0] - l[0][0]), f64::from(v[1] - l[0][1])]).collect())
            .collect();
        let boxes = (legs.iter())
            .map(|l| {
                l.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |m, v| {
                    let v = [f64::from(v[0]), f64::from(v[1])];
                    [m[0].min(v[0]), m[1].min(v[1]), m[2].max(v[0]), m[3].max(v[1])]
                })
            })
            .collect();
        // A leg's first cut: the one at its start, or for the first leg the one at its end.
        let reach = (0..legs.len()).map(|k| reach(p[cuts[k.saturating_sub(1)]])).collect();
        let mitres = (cuts.iter().enumerate())
            .map(|(n, &k)| {
                let o = p[k];
                let (a, b) = (legs[n][legs[n].len() - 2], legs[n + 1][1]);
                let (ua, ub) = (
                    unit([f64::from(o[0] - a[0]), f64::from(o[1] - a[1])]),
                    unit([f64::from(b[0] - o[0]), f64::from(b[1] - o[1])]),
                );
                (o, unit([ua[0] + ub[0], ua[1] + ub[1]]))
            })
            .collect();
        Legs { legs, rel, boxes, reach, mitres }
    }

    /// The reach of legs m and n (`reach`).
    fn reach(&self, m: usize, n: usize) -> f64 {
        self.reach[m].max(self.reach[n])
    }

    /// The units from q to leg k.
    fn to_leg(&self, k: usize, q: [f64; 2]) -> f64 {
        let o = self.legs[k][0];
        to_line(&self.rel[k], [q[0] - f64::from(o[0]), q[1] - f64::from(o[1])])
    }

    /// The medial lines between any two legs, stepping `step(o)` units along a leg (o the pair's
    /// cut) and reaching as far from it as their parts can conflict.
    pub fn medial(&self, step: &dyn Fn(P) -> f64) -> Vec<[P; 2]> {
        let mut lines = Vec::new();
        for m in 0..self.legs.len() {
            for n in m + 1..self.legs.len() {
                let reach = self.reach(m, n);
                let (x, y, gap) = (self.boxes[m], self.boxes[n], 2.0 * reach);
                if x[0] > y[2] + gap || y[0] > x[2] + gap || x[1] > y[3] + gap || y[1] > x[3] + gap {
                    continue;
                }
                // About the end of leg m: a cut, shared with leg n if they are consecutive.
                let (o, t) = self.mitres[m];
                let rel = |q: &P| [f64::from(q[0] - o[0]), f64::from(q[1] - o[1])];
                let a: Vec<[f64; 2]> = self.legs[m].iter().map(rel).collect();
                let b: Vec<[f64; 2]> = self.legs[n].iter().map(rel).collect();
                for run in medial(&a, &b, (n == m + 1).then_some(t), step(o), reach) {
                    let pts: Vec<P> = run
                        .iter()
                        .map(|v| [o[0] + libm::round(v[0]) as i32, o[1] + libm::round(v[1]) as i32])
                        .collect();
                    lines.extend(pts.windows(2).filter(|w| w[0] != w[1]).map(|w| [w[0], w[1]]));
                }
            }
        }
        lines
    }

    /// Per leg, the stretches of it whose bands can come within reach of another leg, with the leg's
    /// reach: a segment one of whose ends lies within twice the reach of another leg, as a point of a
    /// band lies within the reach of its leg.
    pub fn runs(&self) -> Vec<(usize, Vec<P>, f64)> {
        let mut out = Vec::new();
        for (k, leg) in self.legs.iter().enumerate() {
            let near: Vec<bool> = (leg.iter())
                .map(|v| {
                    let q = [f64::from(v[0]), f64::from(v[1])];
                    (0..self.legs.len()).any(|j| j != k && self.to_leg(j, q) <= 2.0 * self.reach(k, j))
                })
                .collect();
            let mut i = 0;
            while i + 1 < leg.len() {
                if !(near[i] || near[i + 1]) {
                    i += 1;
                    continue;
                }
                let lo = i;
                while i + 1 < leg.len() && (near[i] || near[i + 1]) {
                    i += 1;
                }
                out.push((k, leg[lo..=i].to_vec(), self.reach[k]));
            }
        }
        out
    }

    /// The leg nearest q (cell units); a tie between two consecutive legs goes to the side of their
    /// cut's mitre line.
    pub fn nearest(&self, q: [f64; 2]) -> u8 {
        let mut best = (f64::INFINITY, 0);
        for k in 0..self.legs.len() {
            let d = self.to_leg(k, q);
            let tie = (d - best.0).abs() <= 1e-9 * d.max(1.0);
            if tie && k == best.1 + 1 {
                let (c, t) = self.mitres[best.1];
                let v = [q[0] - f64::from(c[0]), q[1] - f64::from(c[1])];
                if v[0] * t[0] + v[1] * t[1] >= 0.0 {
                    best = (d, k);
                }
            } else if d < best.0 && !tie {
                best = (d, k);
            }
        }
        best.1 as u8
    }
}

/// A leg of a line of a road or rail class, for its bridges: its line and leg, its points, the
/// half-width of its carriageway (units) and its height (metres) at each, the step past which its
/// heights conflict with another part's, and twice the widest core of its class (units).
pub struct Part<'a> {
    pub line: usize,
    pub leg: u8,
    pub p: &'a [P],
    pub half: &'a [f64],
    pub z: &'a [f64],
    pub dz: f64,
    pub reach: f64,
}

/// The ground between parts that pass beside each other a step apart: per pair of parts, the higher
/// one (an index into `parts`), the other, and the triangles (counter-clockwise) between each two
/// consecutive points of one part, `step(o)` units apart at most (o its first point), that conflict with the other and their
/// feet there. A point conflicts with another part where its foot there lies within the parts' mean
/// reach, their heights differ by more than the greater `dz`, and their carriageways do not touch: one
/// paved surface has no wall. Every value derives from differences to a part's first point, so two
/// cells that hold the parts give the same bridges.
pub fn bridges(parts: &[Part], step: &dyn Fn(P) -> f64) -> Vec<(usize, usize, Vec<Vec<P>>)> {
    let lerp = |x: f64, y: f64, f: f64| x + f * (y - x);
    let boxes: Vec<[i32; 4]> = (parts.iter())
        .map(|a| {
            a.p.iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |m, v| {
                [m[0].min(v[0]), m[1].min(v[1]), m[2].max(v[0]), m[3].max(v[1])]
            })
        })
        .collect();
    let mut out: BTreeMap<(usize, usize), Vec<Vec<P>>> = BTreeMap::new();
    for (a, pa) in parts.iter().enumerate() {
        let o = pa.p[0];
        let (rel, step) = (|v: &P| [f64::from(v[0] - o[0]), f64::from(v[1] - o[1])], step(o));
        // Its points every `step` at most, with the height and the half-width there.
        let mut dense = vec![(rel(&pa.p[0]), pa.z[0], pa.half[0])];
        for k in 0..pa.p.len() - 1 {
            let (u, v) = (rel(&pa.p[k]), rel(&pa.p[k + 1]));
            let m = libm::ceil(libm::hypot(v[0] - u[0], v[1] - u[1]) / step).max(1.0) as usize;
            for e in 1..=m {
                let f = e as f64 / m as f64;
                let q = [lerp(u[0], v[0], f), lerp(u[1], v[1], f)];
                dense.push((q, lerp(pa.z[k], pa.z[k + 1], f), lerp(pa.half[k], pa.half[k + 1], f)));
            }
        }
        for (b, pb) in parts.iter().enumerate() {
            let reach = (pa.reach + pb.reach) / 2.0;
            let (x, y, gap) = (boxes[a].map(f64::from), boxes[b].map(f64::from), reach);
            if a == b || x[0] > y[2] + gap || y[0] > x[2] + gap || x[1] > y[3] + gap || y[1] > x[3] + gap {
                continue;
            }
            let rb: Vec<[f64; 2]> = pb.p.iter().map(rel).collect();
            let near = Near::new(&rb);
            // Per point: its foot on b and b's height there, where they conflict.
            let feet: Vec<Option<([f64; 2], f64)>> = (dense.iter())
                .map(|&(q, z, h)| {
                    let (d, c, s, f) = near.nearest(q, reach)?;
                    let (zb, hb) = (lerp(pb.z[s], pb.z[s + 1], f), lerp(pb.half[s], pb.half[s + 1], f));
                    (d <= reach && d > h + hb && (z - zb).abs() > pa.dz.max(pb.dz)).then_some((c, zb))
                })
                .collect();
            let pt = |v: [f64; 2]| [o[0] + libm::round(v[0]) as i32, o[1] + libm::round(v[1]) as i32];
            for i in 0..dense.len() - 1 {
                let (Some((c0, zb)), Some((c1, _))) = (feet[i], feet[i + 1]) else { continue };
                let host = if dense[i].1 > zb { a } else { b };
                let quad = [pt(dense[i].0), pt(dense[i + 1].0), pt(c1), pt(c0)];
                for t in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                    let w = |m: usize, c: usize| i64::from(t[m][c] - t[0][c]);
                    let area = w(1, 0) * w(2, 1) - w(1, 1) * w(2, 0);
                    if area != 0 {
                        let ring = if area > 0 { t.to_vec() } else { vec![t[0], t[2], t[1]] };
                        out.entry((host, a + b - host)).or_default().push(ring);
                    }
                }
            }
        }
    }
    out.into_iter().map(|((h, o), t)| (h, o, t)).collect()
}

/// A polyline's segments in boxes of `BOX` segments, for its nearest point to q within a pad.
struct Near<'a> {
    p: &'a [[f64; 2]],
    boxes: Vec<[f64; 4]>,
}

const BOX: usize = 16;

impl<'a> Near<'a> {
    fn new(p: &'a [[f64; 2]]) -> Self {
        let boxes = (0..p.len().saturating_sub(1))
            .step_by(BOX)
            .map(|s| {
                p[s..(s + BOX + 1).min(p.len())].iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |m, v| {
                    [m[0].min(v[0]), m[1].min(v[1]), m[2].max(v[0]), m[3].max(v[1])]
                })
            })
            .collect();
        Near { p, boxes }
    }

    /// The nearest point to q of the segments within `pad` of it: its distance, the point, and the
    /// segment and fraction along it.
    fn nearest(&self, q: [f64; 2], pad: f64) -> Option<(f64, [f64; 2], usize, f64)> {
        let mut best: Option<(f64, [f64; 2], usize, f64)> = None;
        for (k, bx) in self.boxes.iter().enumerate() {
            if q[0] < bx[0] - pad || q[0] > bx[2] + pad || q[1] < bx[1] - pad || q[1] > bx[3] + pad {
                continue;
            }
            for s in k * BOX..((k + 1) * BOX).min(self.p.len() - 1) {
                let (a, b) = (self.p[s], self.p[s + 1]);
                let ab = [b[0] - a[0], b[1] - a[1]];
                let l2 = ab[0] * ab[0] + ab[1] * ab[1];
                let f =
                    if l2 > 0.0 { (((q[0] - a[0]) * ab[0] + (q[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let c = [a[0] + f * ab[0], a[1] + f * ab[1]];
                let d = libm::hypot(q[0] - c[0], q[1] - c[1]);
                if best.is_none_or(|(e, ..)| d < e) {
                    best = Some((d, c, s, f));
                }
            }
        }
        best
    }
}

/// The medial lines between legs A and B, relative to the end of A: the points as near to both,
/// traced in steps of `step` while they lie within `reach` of A, and on `reach` past their ends. Where
/// B starts at A's end, a cut with
/// the line's direction t there, the first line runs from `reach` out on the cut's mitre line on the
/// outer side of the turn through the cut, and on along the medial line on the inner side. Every
/// other part of the medial line starts where a point of A, searched along its normal every `step`,
/// finds one that no line passes yet, and runs both ways from it.
fn medial(a: &[[f64; 2]], b: &[[f64; 2]], t: Option<[f64; 2]>, step: f64, reach: f64) -> Vec<Vec<[f64; 2]>> {
    let na = a.len();
    if na < 2 || b.len() < 2 {
        return Vec::new();
    }
    let (near_a, near_b) = (Near::new(a), Near::new(b));
    let pad = 2.0 * reach;
    let dist = |near: &Near, v: [f64; 2]| near.nearest(v, pad).map_or(f64::INFINITY, |r| r.0);
    // Positive on A's side of the medial line.
    let side = |v: [f64; 2]| dist(&near_b, v) - dist(&near_a, v);
    // The root of h between lo and hi, where h changes sign, by the false position with the Illinois
    // halving: `side` is nearly linear across the medial line. To a twentieth of a unit of `side`
    // (a fortieth of a unit across): the points round to whole units.
    let root = |h: &dyn Fn(f64) -> f64, mut lo: f64, mut hi: f64| {
        let (mut fl, mut fh) = (h(lo), h(hi));
        let mut last = 0;
        for _ in 0..60 {
            if hi - lo <= 0.01 {
                break;
            }
            let mid = if fh != fl { (lo * fh - hi * fl) / (fh - fl) } else { 0.5 * (lo + hi) };
            let mid = mid.clamp(lo, hi);
            let fm = h(mid);
            if fm.abs() <= 0.05 {
                return mid;
            }
            if (fm > 0.0) == (fl > 0.0) {
                (lo, fl) = (mid, fm);
                if last == -1 {
                    fh *= 0.5;
                }
                last = -1;
            } else {
                (hi, fh) = (mid, fm);
                if last == 1 {
                    fl *= 0.5;
                }
                last = 1;
            }
        }
        0.5 * (lo + hi)
    };
    // From v along the direction d: each step ahead, the point of the medial line on the step's
    // normal within two steps, until none is or it lies past `reach` from A.
    let limit =
        (a.windows(2).chain(b.windows(2)).map(|w| libm::hypot(w[1][0] - w[0][0], w[1][1] - w[0][1])).sum::<f64>()
            / step) as usize
            + 1;
    let trace = |mut v: [f64; 2], mut d: [f64; 2]| -> Vec<[f64; 2]> {
        let mut out = Vec::new();
        for _ in 0..limit {
            let w = [v[0] + d[0] * step, v[1] + d[1] * step];
            let n = [-d[1], d[0]];
            let h = |x: f64| side([w[0] + n[0] * x, w[1] + n[1] * x]);
            if (h(-2.0 * step) > 0.0) == (h(2.0 * step) > 0.0) {
                break;
            }
            let x = root(&h, -2.0 * step, 2.0 * step);
            let next = [w[0] + n[0] * x, w[1] + n[1] * x];
            if dist(&near_a, next) > reach {
                break;
            }
            d = unit([next[0] - v[0], next[1] - v[1]]);
            v = next;
            out.push(v);
        }
        out
    };
    let mut runs: Vec<Vec<[f64; 2]>> = Vec::new();
    if let Some(t) = t {
        // The cut's mitre line on the outer side: the side away from the turn.
        let left = [-t[1], t[0]];
        let (ua, ub) = (unit([-a[na - 2][0], -a[na - 2][1]]), unit(b[1]));
        let outer = if ua[0] * ub[1] - ua[1] * ub[0] > 0.0 { [-left[0], -left[1]] } else { left };
        let mut run = vec![[outer[0] * reach, outer[1] * reach], [0.0, 0.0]];
        run.extend(trace([0.0, 0.0], [-outer[0], -outer[1]]));
        runs.push(run);
    }
    // The runs' points in bins of two steps, for the run points within two steps of a point.
    let bin = |v: [f64; 2]| [libm::floor(v[0] / (2.0 * step)) as i64, libm::floor(v[1] / (2.0 * step)) as i64];
    let mut bins: BTreeMap<[i64; 2], Vec<[f64; 2]>> = BTreeMap::new();
    let add = |bins: &mut BTreeMap<[i64; 2], Vec<[f64; 2]>>, run: &[[f64; 2]]| {
        for &v in run {
            bins.entry(bin(v)).or_default().push(v);
        }
    };
    for run in &runs {
        add(&mut bins, run);
    }
    let covered = |bins: &BTreeMap<[i64; 2], Vec<[f64; 2]>>, v: [f64; 2]| {
        let c = bin(v);
        (-1..=1).any(|i| {
            (-1..=1).any(|j| {
                (bins.get(&[c[0] + i, c[1] + j]).into_iter().flatten())
                    .any(|w| libm::hypot(w[0] - v[0], w[1] - v[1]) <= 2.0 * step)
            })
        })
    };
    for s in (0..na - 1).rev() {
        let (p0, p1) = (a[s], a[s + 1]);
        let len = libm::hypot(p1[0] - p0[0], p1[1] - p0[1]);
        // A medial point within `reach` of A has B within twice that: a segment farther from B seeds
        // none.
        if near_b.nearest(p0, 2.0 * reach + len).is_none_or(|r| r.0 > 2.0 * reach + len) {
            continue;
        }
        let u = unit([p1[0] - p0[0], p1[1] - p0[1]]);
        let m = libm::ceil(len / step).max(1.0) as usize;
        for e in (0..m).rev() {
            let f = e as f64 / m as f64;
            let q = [p0[0] + f * (p1[0] - p0[0]), p0[1] + f * (p1[1] - p0[1])];
            // Along A's normal on either side, the first point of the medial line: the nearer one.
            let found = [1.0, -1.0]
                .into_iter()
                .filter_map(|sign| {
                    let n = [u[1] * sign, -u[0] * sign];
                    let h = |x: f64| side([q[0] + n[0] * x, q[1] + n[1] * x]);
                    (h(reach) <= 0.0).then(|| {
                        let x = root(&h, 0.0, reach);
                        (x, [q[0] + n[0] * x, q[1] + n[1] * x])
                    })
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let Some((_, v)) = found else { continue };
            if covered(&bins, v) {
                continue;
            }
            // The medial line's direction there: normal to the change of `side`.
            let e = step / 4.0;
            let g = [side([v[0] + e, v[1]]) - side([v[0] - e, v[1]]), side([v[0], v[1] + e]) - side([v[0], v[1] - e])];
            let d = unit([-g[1], g[0]]);
            let mut run: Vec<[f64; 2]> = trace(v, [-d[0], -d[1]]).into_iter().rev().collect();
            run.push(v);
            run.extend(trace(v, d));
            if run.len() >= 2 {
                add(&mut bins, &run);
                runs.push(run);
            }
        }
    }
    // Each run goes on `reach` past its ends, where a third part of a leg ends the trace inside the
    // legs' overlap; the one from a cut starts out on the mitre line already.
    for (k, run) in runs.iter_mut().enumerate() {
        let n = run.len();
        if n < 2 {
            continue;
        }
        let d = unit([run[n - 1][0] - run[n - 2][0], run[n - 1][1] - run[n - 2][1]]);
        run.push([run[n - 1][0] + d[0] * reach, run[n - 1][1] + d[1] * reach]);
        if k > 0 || t.is_none() {
            let d = unit([run[0][0] - run[1][0], run[0][1] - run[1][1]]);
            run.insert(0, [run[0][0] + d[0] * reach, run[0][1] + d[1] * reach]);
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hairpin: leg A up x = −r, a half circle over the top, leg B down x = +r, rising `grade` per
    /// unit along it.
    fn hairpin(r: f64, legs: i32, grade: f64) -> (Vec<P>, Vec<f64>) {
        let mut p: Vec<P> = (0..legs).rev().map(|k| [-r as i32, -k * 100]).collect();
        for k in 1..18 {
            let a = std::f64::consts::PI * f64::from(k) / 18.0;
            p.push([libm::round(-r * libm::cos(a)) as i32, libm::round(r * libm::sin(a)) as i32]);
        }
        p.extend((0..legs).map(|k| [r as i32, -k * 100]));
        let mut z = vec![0.0];
        for w in p.windows(2) {
            z.push(z.last().unwrap() + grade * dist(w[0], w[1]));
        }
        (p, z)
    }

    #[test]
    fn a_hairpin_splits_at_its_apex_and_a_level_one_or_a_wide_one_does_not() {
        let (p, z) = hairpin(300.0, 20, 0.001);
        assert_eq!(cuts(&p, &vec![200.0; p.len()], &z, 1.0, &|_| 800.0), vec![(p.len() - 1) / 2]);
        // Steeper, the turn's own core touches each leg's at another height: the turn is a leg too.
        let (_, z) = hairpin(300.0, 20, 0.01);
        assert_eq!(cuts(&p, &vec![200.0; p.len()], &z, 1.0, &|_| 800.0).len(), 2);
        let flat = vec![0.0; p.len()];
        assert!(cuts(&p, &vec![200.0; p.len()], &flat, 1.0, &|_| 800.0).is_empty());
        let (p, z) = hairpin(900.0, 20, 0.001);
        assert!(cuts(&p, &vec![200.0; p.len()], &z, 1.0, &|_| 800.0).is_empty(), "legs 1,800 apart lie out of reach");
    }

    #[test]
    fn a_hairpin_whose_legs_share_their_asphalt_further_down_does_not_split() {
        // Only the lower legs' carriageways touch (600 apart, 300 wide each); the upper legs' conflict
        // lies in the same cluster, on the same paved surface.
        let (p, z) = hairpin(300.0, 20, 0.001);
        let half: Vec<f64> = p.iter().map(|v| if v[1] < -1000 { 300.0 } else { 200.0 }).collect();
        assert!(cuts(&p, &half, &z, 1.0, &|_| 800.0).is_empty());
    }

    #[test]
    fn a_steep_bend_wider_than_its_core_does_not_split() {
        // A quarter circle of radius 1,000 sampled every 2°, rising 1 per 5 samples: the line between
        // two points within reach stays in the hull of their cores.
        let p: Vec<P> = (0..=45)
            .map(|k| f64::from(k) * 2.0f64.to_radians())
            .map(|a| [libm::round(1000.0 * libm::cos(a)) as i32, libm::round(1000.0 * libm::sin(a)) as i32])
            .collect();
        let z: Vec<f64> = (0..p.len()).map(|k| k as f64 * 0.2).collect();
        assert!(cuts(&p, &vec![200.0; p.len()], &z, 1.0, &|_| 800.0).is_empty());
    }

    #[test]
    fn a_zigzag_splits_once_per_turn_and_every_conflict_has_a_cut_between_its_parts() {
        // Up x = −300, over a turn about (0, 0), down x = 300, under a turn about (600, −1,000), up
        // x = 900.
        let arc = |c: [f64; 2], from: f64| -> Vec<P> {
            (1..18)
                .map(|k| from + std::f64::consts::PI * f64::from(k) / 18.0)
                .map(|a| {
                    [libm::round(c[0] + 300.0 * libm::cos(a)) as i32, libm::round(c[1] + 300.0 * libm::sin(a)) as i32]
                })
                .collect()
        };
        let mut p: Vec<P> = (0..=10).map(|k| [-300, -1000 + 100 * k]).collect();
        p.extend(arc([0.0, 0.0], std::f64::consts::PI).into_iter().map(|v| [v[0], -v[1]]));
        p.extend((0..=10).map(|k| [300, -100 * k]));
        p.extend(arc([600.0, -1000.0], std::f64::consts::PI));
        p.extend((0..=10).map(|k| [900, -1000 + 100 * k]));
        let mut z = vec![0.0];
        for w in p.windows(2) {
            z.push(z.last().unwrap() + 0.001 * dist(w[0], w[1]));
        }
        let found = cuts(&p, &vec![200.0; p.len()], &z, 1.0, &|_| 800.0);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0] < found[1]);
        assert_eq!(p[found[0]][1], 300, "the first cut at the top of its turn: {:?}", p[found[0]]);
        assert_eq!(p[found[1]][1], -1300, "the second at the bottom of its turn: {:?}", p[found[1]]);
        assert!(leg(&found, found[0], 1) == 1 && leg(&found, found[0], -1) == 0);
    }

    #[test]
    fn the_medial_line_divides_a_hairpin_between_its_legs() {
        let (p, z) = hairpin(300.0, 20, 0.001);
        let found = cuts(&p, &vec![200.0; p.len()], &z, 1.0, &|_| 800.0);
        let legs = Legs::new(&p, &found, &|_| 1000.0);
        let lines = legs.medial(&|_| 25.0);
        // The medial line runs up x = 0 between the legs, and the outer side of the apex splits on the
        // mitre line, which is x = 0 there too.
        let off: Vec<i32> = lines.iter().flatten().filter(|v| v[1] < 0 && v[1] > -1500).map(|v| v[0].abs()).collect();
        assert!(!off.is_empty() && off.iter().all(|&x| x <= 1), "{off:?}");
        assert!(lines.iter().flatten().any(|v| v[1] > 300), "the mitre line on the outer side");
        assert_eq!(legs.nearest([-150.0, -800.0]), 0);
        assert_eq!(legs.nearest([150.0, -800.0]), 1);
        assert_eq!(legs.nearest([-10.0, 600.0]), 0);
        assert_eq!(legs.nearest([10.0, 600.0]), 1);
    }

    /// The bridges of parts along x from 0 to 2,000 at y and height z each, 200 wide, reach 800.
    fn bridges_of(parts: &[(i32, f64)]) -> Vec<(usize, usize, Vec<Vec<P>>)> {
        let p: Vec<Vec<P>> = parts.iter().map(|&(y, _)| (0..=20).map(|k| [100 * k, y]).collect()).collect();
        let z: Vec<Vec<f64>> = parts.iter().map(|&(_, z)| vec![z; 21]).collect();
        let half = vec![200.0; 21];
        let parts: Vec<Part> = (0..parts.len())
            .map(|k| Part { line: k, leg: 0, p: &p[k], half: &half, z: &z[k], dz: 1.0, reach: 800.0 })
            .collect();
        bridges(&parts, &|_| 50.0)
    }

    /// The bridge of `found` (its host) that holds the point q.
    fn holds(found: &[(usize, usize, Vec<Vec<P>>)], q: P) -> Option<usize> {
        let at = [2 * i64::from(q[0]), 2 * i64::from(q[1])];
        found.iter().find(|b| super::super::geom::winding(&super::super::stroke::union(&b.2, q), at) != 0).map(|b| b.0)
    }

    #[test]
    fn the_ground_between_two_parts_a_step_apart_takes_the_higher_one() {
        let found = bridges_of(&[(0, 0.0), (600, 2.0)]);
        assert_eq!(found.iter().map(|b| (b.0, b.1)).collect::<Vec<_>>(), vec![(1, 0)]);
        assert_eq!(holds(&found, [1000, 300]), Some(1));
        assert_eq!(holds(&found, [1000, -300]), None, "the far side of the lower part");
        // Each of two upper parts holds the ground between it and the lower part in the middle.
        let found = bridges_of(&[(0, 0.0), (600, 2.0), (-700, 4.0)]);
        assert_eq!(holds(&found, [1000, 300]), Some(1));
        assert_eq!(holds(&found, [1000, -300]), Some(2));
        assert!(found.iter().all(|b| (b.0, b.1) != (2, 1)), "the upper parts lie out of reach");
    }

    #[test]
    fn parts_that_blend_lie_out_of_reach_or_share_their_asphalt_have_no_bridge() {
        assert!(bridges_of(&[(0, 0.0), (600, 0.8)]).is_empty());
        assert!(bridges_of(&[(0, 0.0), (900, 2.0)]).is_empty());
        assert!(bridges_of(&[(0, 0.0), (350, 2.0)]).is_empty());
    }
}
