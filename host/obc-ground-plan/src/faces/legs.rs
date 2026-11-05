//! SCRATCH (question 4, not for merge yet): the legs of a way whose carriageway comes back beside
//! itself at another height, and the medial line that splits their carriageway faces.
//!
//! Two samples of one line are two parts of it where their cores (carriageway, shoulder and verge)
//! overlap or touch while the line between them leaves both cores. Two parts whose heights differ by
//! more than the step limit conflict. The line splits into legs so that no leg holds both samples of
//! a conflict: each cluster of conflicts with a common span gives a cut at the middle of that span.
//! Consecutive legs meet at the cut sample, and their faces there: on the mitre line outside and on
//! the medial line between the legs.

use super::geom::P;

/// A cut of a line between two legs: leg ranges end at `hi` and start at `lo` (both `k` now).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut {
    pub lo: usize,
    pub k: usize,
    pub hi: usize,
}

/// The cuts of a line: samples `p` (units), core half-widths `core` (units), heights `z` (metres),
/// the step limit `dz` (metres).
pub fn cuts(p: &[P], core: &[f64], z: &[f64], dz: f64) -> Vec<Cut> {
    let n = p.len();
    let d = |a: usize, b: usize| f64::from(p[a][0] - p[b][0]).hypot(f64::from(p[a][1] - p[b][1]));
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for i in 0..n {
        for j in i + 2..n {
            if (z[i] - z[j]).abs() <= dz || d(i, j) > core[i] + core[j] {
                continue;
            }
            // The line between them, every quarter of the lesser core along each segment.
            let step = 0.25 * core[i].min(core[j]);
            let leaves = (i..j).any(|k| {
                let len = d(k, k + 1);
                let m = (len / step).ceil().max(1.0) as usize;
                (1..m).chain(std::iter::once(m)).any(|e| {
                    let f = e as f64 / m as f64;
                    let q = [
                        f64::from(p[k][0]) + f * f64::from(p[k + 1][0] - p[k][0]),
                        f64::from(p[k][1]) + f * f64::from(p[k + 1][1] - p[k][1]),
                    ];
                    let to = |s: usize| (q[0] - f64::from(p[s][0])).hypot(q[1] - f64::from(p[s][1]));
                    to(i) > core[i] && to(j) > core[j]
                })
            });
            if leaves {
                pairs.push((i, j));
            }
        }
    }
    pairs.sort_unstable();
    let mut out: Vec<Cut> = Vec::new();
    let mut span: Option<(usize, usize)> = None;
    for (i, j) in pairs {
        span = match span {
            Some((a, b)) if i.max(a) + 1 < j.min(b) => Some((i.max(a), j.min(b))),
            Some((a, b)) => {
                push(&mut out, a, b);
                Some((i, j))
            }
            None => Some((i, j)),
        };
    }
    if let Some((a, b)) = span {
        push(&mut out, a, b);
    }
    out
}

fn push(out: &mut Vec<Cut>, a: usize, b: usize) {
    let (lo, hi) = (a + 1, b - 1);
    // The legs meet at the cut sample: each has the whole line's frame up to its mitre line there and
    // no surface past it, so no point reads both and none reads neither.
    let k = (lo + hi) / 2;
    let cut = Cut { lo: k, k, hi: k };
    // SCRATCH: legs must follow each other along the line; a cluster that does not is left out.
    if out.last().is_none_or(|c| cut.lo > c.k && cut.k > c.hi) {
        out.push(cut);
    }
}

/// The sample ranges of the legs as records: leg 0 from the first sample to the first cut's `hi`,
/// then from each cut's `lo` to the next cut's `hi`, the last to the last sample.
pub fn record_ranges(cuts: &[Cut], n: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut lo = 0;
    for c in cuts {
        out.push((lo, c.hi));
        lo = c.lo;
    }
    out.push((lo, n - 1));
    out
}

/// The leg whose face holds sample i.
pub fn face_leg(cuts: &[Cut], i: usize) -> u8 {
    cuts.iter().filter(|c| c.k < i).count() as u8
}

/// The distance from q to the polyline p.
fn to_line(p: &[[f64; 2]], q: [f64; 2]) -> f64 {
    let mut best = f64::INFINITY;
    for w in p.windows(2) {
        let ab = [w[1][0] - w[0][0], w[1][1] - w[0][1]];
        let l2 = ab[0] * ab[0] + ab[1] * ab[1];
        let f = if l2 > 0.0 { (((q[0] - w[0][0]) * ab[0] + (q[1] - w[0][1]) * ab[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min((q[0] - w[0][0] - f * ab[0]).hypot(q[1] - w[0][1] - f * ab[1]));
    }
    if p.len() == 1 {
        best = (q[0] - p[0][0]).hypot(q[1] - p[0][1]);
    }
    best
}

/// Whether q lies inside rings (NonZero).
pub fn inside(rings: &[Vec<P>], q: [f64; 2]) -> bool {
    let mut w = 0;
    for ring in rings {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let (a, b) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
            let side = (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0]);
            if a[1] <= q[1] {
                if b[1] > q[1] && side > 0.0 {
                    w += 1;
                }
            } else if b[1] <= q[1] && side < 0.0 {
                w -= 1;
            }
        }
    }
    w != 0
}

/// Which of two polylines (A, B) lies nearer q: true for A.
/// A point as near to both (the pie at the cut's outer side, where both are nearest at the cut)
/// takes the side of the cut's mitre line.
pub fn nearer_a(a: &[P], b: &[P], q: [f64; 2]) -> bool {
    let f = |p: &[P]| p.iter().map(|v| [f64::from(v[0]), f64::from(v[1])]).collect::<Vec<_>>();
    let (fa, fb) = (f(a), f(b));
    let (da, db) = (to_line(&fa, q), to_line(&fb, q));
    if (da - db).abs() > 1e-6 * da.max(1.0) || fa.len() < 2 || fb.len() < 2 {
        return da < db;
    }
    let k = fa[fa.len() - 1];
    let t = [fb[1][0] - fa[fa.len() - 2][0], fb[1][1] - fa[fa.len() - 2][1]];
    (q[0] - k[0]) * t[0] + (q[1] - k[1]) * t[1] < 0.0
}

/// The medial line between A (ending at the cut) and B (starting there), as segments in units: from
/// `reach` units out on the mitre line at the cut's outer side, through the cut, then the points
/// equidistant from both along A's normals toward B, every `step` units along A, while B lies
/// within `reach`; then on `reach` units past its last point.
pub fn medial(a: &[P], b: &[P], step: f64, reach: f64) -> Vec<[P; 2]> {
    let fa: Vec<[f64; 2]> = a.iter().map(|v| [f64::from(v[0]), f64::from(v[1])]).collect();
    let fb: Vec<[f64; 2]> = b.iter().map(|v| [f64::from(v[0]), f64::from(v[1])]).collect();
    let (na, nb) = (fa.len(), fb.len());
    if na < 2 || nb < 2 {
        return Vec::new();
    }
    let unit = |v: [f64; 2]| {
        let l = v[0].hypot(v[1]);
        if l > 0.0 {
            [v[0] / l, v[1] / l]
        } else {
            [1.0, 0.0]
        }
    };
    let k = fa[na - 1];
    // The cut's mitre normal toward the inner side (where the turn goes).
    let (ua, ub) = (unit([k[0] - fa[na - 2][0], k[1] - fa[na - 2][1]]), unit([fb[1][0] - k[0], fb[1][1] - k[1]]));
    let t = unit([ua[0] + ub[0], ua[1] + ub[1]]);
    let turn = ua[0] * ub[1] - ua[1] * ub[0];
    let left = [t[1], -t[0]];
    let inner = if turn < 0.0 { left } else { [-left[0], -left[1]] };
    let mut pts: Vec<[f64; 2]> = vec![[k[0] - inner[0] * reach, k[1] - inner[1] * reach], k];
    // Walk A back from the cut.
    'walk: for s in (0..na - 1).rev() {
        let (p0, p1) = (fa[s], fa[s + 1]);
        let u = unit([p1[0] - p0[0], p1[1] - p0[1]]);
        let len = (p1[0] - p0[0]).hypot(p1[1] - p0[1]);
        let m = (len / step).ceil().max(1.0) as usize;
        for e in (0..m).rev() {
            let f = e as f64 / m as f64;
            let q = [p0[0] + f * (p1[0] - p0[0]), p0[1] + f * (p1[1] - p0[1])];
            // Toward B's side of A.
            let nl = [u[1], -u[0]];
            let bn = fb.iter().min_by(|x, y| {
                (x[0] - q[0]).hypot(x[1] - q[1]).total_cmp(&(y[0] - q[0]).hypot(y[1] - q[1]))
            });
            let Some(bn) = bn else { break 'walk };
            let side = if (bn[0] - q[0]) * nl[0] + (bn[1] - q[1]) * nl[1] >= 0.0 { 1.0 } else { -1.0 };
            let n = [nl[0] * side, nl[1] * side];
            let g = |t: f64| to_line(&fb, [q[0] + n[0] * t, q[1] + n[1] * t]) - t;
            if g(reach) > 0.0 {
                break 'walk;
            }
            let (mut lo, mut hi) = (0.0, reach);
            for _ in 0..50 {
                let mid = 0.5 * (lo + hi);
                if g(mid) > 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            pts.push([q[0] + n[0] * lo, q[1] + n[1] * lo]);
        }
    }
    if pts.len() >= 3 {
        let (l, m) = (pts[pts.len() - 1], pts[pts.len() - 2]);
        let d = unit([l[0] - m[0], l[1] - m[1]]);
        pts.push([l[0] + d[0] * reach, l[1] + d[1] * reach]);
    }
    let pts: Vec<P> = pts.iter().map(|v| [v[0].round() as i32, v[1].round() as i32]).collect();
    let mut out: Vec<[P; 2]> = pts.windows(2).filter(|w| w[0] != w[1]).map(|w| [w[0], w[1]]).collect();
    out.dedup();
    out
}
