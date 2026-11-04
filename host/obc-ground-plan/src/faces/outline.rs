//! The rounds at the inward corners of the paved outline that no point's round shapes: where two
//! ways' carriageways meet without a graph point between them, where a third piece lies between a
//! point's two arms, and where a way turns inside a point's region. A corner tighter than the least
//! inner radius of the ways whose edges form it gets the circle tangent to its two edges, off the
//! paved area, of that radius or the largest of the ladder that fits.

use super::check::{corners, turn_at, KINK_SHARE, SUPPORT_M};
use super::cover::Cover;
use super::geom::P;
use super::points::corner_round;
use super::stroke::Rings;
use super::WayLine;

/// A way forms a corner where its carriageway's edge runs within this many units of the corner.
const ON_EDGE: f64 = 4.0;

/// The ways at a point: the least `inner_radius_m` and the greatest `sag_plan_m` of the ways whose
/// carriageway edge runs through q, if any.
pub fn at_corner(lines: &[WayLine], q: P) -> Option<(f64, f64)> {
    let (x, y) = (f64::from(q[0]), f64::from(q[1]));
    let mut out: Option<(f64, f64)> = None;
    for line in lines {
        let b = line.bbox;
        let pad = ON_EDGE as i32;
        if q[0] < b[0] - pad || q[0] > b[2] + pad || q[1] < b[1] - pad || q[1] > b[3] + pad {
            continue;
        }
        let touches = (0..line.p.len().saturating_sub(1)).any(|i| {
            let (a, c) = (line.p[i], line.p[i + 1]);
            let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
            let (u, v) = ([f64::from(c[0]) - ax, f64::from(c[1]) - ay], [x - ax, y - ay]);
            let l2 = u[0] * u[0] + u[1] * u[1];
            let t = if l2 > 0.0 { ((v[0] * u[0] + v[1] * u[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (dx, dy) = (v[0] - t * u[0], v[1] - t * u[1]);
            (dx * dx + dy * dy).sqrt() - (line.h[i] + t * (line.h[i + 1] - line.h[i])) <= ON_EDGE
        });
        if touches {
            let (r, s) = (line.sec.inner_radius_m, line.sec.sag_plan_m);
            out = Some(out.map_or((r, s), |(r0, s0)| (r0.min(r), s0.max(s))));
        }
    }
    out
}

/// The least inner radius of the ways whose carriageway edge runs through q.
pub fn inner_m(lines: &[WayLine], q: P) -> Option<f64> {
    at_corner(lines, q).map(|(r, _)| r)
}

/// A round at a corner of the paved outline: its ring, its circle (centre, radius in units), and a
/// point inside the paved area at the corner (doubled), whose label the ring takes.
pub struct Fill {
    pub ring: Vec<P>,
    pub centre: P,
    pub radius: f64,
    pub inside: [i64; 2],
}

/// The segments of the paved outline by a grid of 1,024 units, for the distance of a circle.
struct Edges {
    cells: std::collections::BTreeMap<(i32, i32), Vec<[P; 2]>>,
}

const SHIFT: u32 = 10;

impl Edges {
    fn new(rings: &Rings) -> Edges {
        let mut cells: std::collections::BTreeMap<(i32, i32), Vec<[P; 2]>> = Default::default();
        for ring in rings {
            for i in 0..ring.len() {
                let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                for gx in (a[0].min(b[0]) >> SHIFT)..=(a[0].max(b[0]) >> SHIFT) {
                    for gy in (a[1].min(b[1]) >> SHIFT)..=(a[1].max(b[1]) >> SHIFT) {
                        cells.entry((gx, gy)).or_default().push([a, b]);
                    }
                }
            }
        }
        Edges { cells }
    }

    /// Whether every segment lies at least d from point c.
    fn clear(&self, c: [f64; 2], d: f64) -> bool {
        let g = |v: f64, s: f64| (libm::floor(v + s) as i32) >> SHIFT;
        for gx in g(c[0], -d)..=g(c[0], d) {
            for gy in g(c[1], -d)..=g(c[1], d) {
                for &[a, b] in self.cells.get(&(gx, gy)).into_iter().flatten() {
                    let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
                    let (u, v) = ([f64::from(b[0]) - ax, f64::from(b[1]) - ay], [c[0] - ax, c[1] - ay]);
                    let l2 = u[0] * u[0] + u[1] * u[1];
                    let t = if l2 > 0.0 { ((v[0] * u[0] + v[1] * u[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    let (dx, dy) = (v[0] - t * u[0], v[1] - t * u[1]);
                    if dx * dx + dy * dy < d * d {
                        return false;
                    }
                }
            }
        }
        true
    }
}

/// The rounds at the corners of the paved outline `paved` (outer rings counter-clockwise, holes
/// clockwise: the paved side on the left) whose sharpest vertex lies `near` the window and on no
/// round of `circles` (centre, radius in units). `units(m, p)` turns metres into units at p; the
/// edges are read `len` units each way of a corner.
pub fn rounds(
    paved: &Rings,
    lines: &[WayLine],
    circles: &[(P, f64)],
    near: &dyn Fn(P) -> bool,
    units: &dyn Fn(f64, P) -> f64,
    len: f64,
) -> Vec<Fill> {
    let edges = Edges::new(paved);
    let bbox = paved.iter().flatten().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, p| {
        [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
    });
    let mut out = Vec::new();
    if bbox[0] > bbox[2] {
        return out;
    }
    let cover = Cover::new(bbox, &[paved.as_slice()]);
    for ring in paved {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let turn: Vec<Option<f64>> = (0..n).map(|i| Some(turn_at(ring, i))).collect();
        for c in corners(ring, &turn, units(SUPPORT_M, ring[0])) {
            let v = ring[c.at];
            if !near(v) {
                continue;
            }
            let Some((inner, sag)) = at_corner(lines, v) else { continue };
            let r = units(inner, v);
            let on_round = circles.iter().any(|&(o, rho)| {
                let d = (f64::from(o[0] - v[0])).hypot(f64::from(o[1] - v[1]));
                (d - rho).abs() < units(0.1, v)
            });
            if c.radius >= KINK_SHARE * r || on_round {
                continue;
            }
            // The outline's edges from the corner: walked backward, and ahead.
            let walk = |dir: isize| {
                let mut chain = vec![v];
                let mut s = 0.0;
                for k in 1..n as isize {
                    let p = ring[(c.at as isize + dir * k).rem_euclid(n as isize) as usize];
                    let q = chain[chain.len() - 1];
                    s += (f64::from(p[0] - q[0])).hypot(f64::from(p[1] - q[1]));
                    chain.push(p);
                    if s >= len {
                        break;
                    }
                }
                chain
            };
            let off = |q: [f64; 2], rho: f64| {
                let a = [f64::from(v[0]) + q[0], f64::from(v[1]) + q[1]];
                let doubled = [libm::round(2.0 * a[0]) as i64, libm::round(2.0 * a[1]) as i64];
                cover.at(doubled).is_empty() && edges.clear(a, rho - units(sag, v))
            };
            if let Some(f) = corner_round(v, &walk(-1), &walk(1), r, units(sag, v), &off) {
                out.push(f);
            }
        }
    }
    out
}
