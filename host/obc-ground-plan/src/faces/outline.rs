//! The rounds at the inward corners of the paved outline that no point's round shapes: where two
//! ways' carriageways meet without a graph point between them, where a third piece lies between a
//! point's two arms, and where a way turns inside a point's region. A corner tighter than the least
//! inner radius of the ways whose edges form it gets the circle tangent to its two edges, off the
//! paved area, of that radius or the largest of the ladder that fits.

use super::check::{corners, turn_at, KINK_SHARE, SUPPORT_M};
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

/// The band layers' outer offsets past the carriageway edge (units) of the way whose edge runs
/// nearest q: paved shoulder, shoulder, verge, slope.
fn bands_at(lines: &[WayLine], q: [f64; 2]) -> [f64; 4] {
    let mut best = (f64::INFINITY, [0.0; 4]);
    for line in lines {
        let b = line.bbox;
        let pad = f64::from(ON_EDGE as i32) + 1.0;
        if q[0] < f64::from(b[0]) - pad
            || q[0] > f64::from(b[2]) + pad
            || q[1] < f64::from(b[1]) - pad
            || q[1] > f64::from(b[3]) + pad
        {
            continue;
        }
        for i in 0..line.p.len().saturating_sub(1) {
            let (a, c) = (line.p[i], line.p[i + 1]);
            let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
            let (u, v) = ([f64::from(c[0]) - ax, f64::from(c[1]) - ay], [q[0] - ax, q[1] - ay]);
            let l2 = u[0] * u[0] + u[1] * u[1];
            let t = if l2 > 0.0 { ((v[0] * u[0] + v[1] * u[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let h = line.h[i] + t * (line.h[i + 1] - line.h[i]);
            let off = (libm::hypot(v[0] - t * u[0], v[1] - t * u[1]) - h).abs();
            if off < best.0 {
                let w = std::array::from_fn(|k| line.bands[i][k] + t * (line.bands[i + 1][k] - line.bands[i][k]) - h);
                best = (off, w);
            }
        }
    }
    best.1
}

/// A round at a corner of the paved outline: its ring, its circle (centre, radius in units), and a
/// point inside the paved area at the corner (doubled), whose label the ring takes.
pub struct Fill {
    pub ring: Vec<P>,
    /// Its band rings: paved shoulder, shoulder, verge, slope.
    pub bands: [Vec<Vec<P>>; 4],
    pub centre: P,
    pub radius: f64,
    pub inside: [i64; 2],
}

/// Whether every segment lies at least d from point c.
fn clear(segments: &[[P; 2]], c: [f64; 2], d: f64) -> bool {
    segments.iter().all(|&[a, b]| {
        let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
        let (u, v) = ([f64::from(b[0]) - ax, f64::from(b[1]) - ay], [c[0] - ax, c[1] - ay]);
        let l2 = u[0] * u[0] + u[1] * u[1];
        let t = if l2 > 0.0 { ((v[0] * u[0] + v[1] * u[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let (dx, dy) = (v[0] - t * u[0], v[1] - t * u[1]);
        dx * dx + dy * dy >= d * d
    })
}

/// The rounds at the corners of the paved outline `paved` (outer rings counter-clockwise, holes
/// clockwise: the paved side on the left) whose sharpest vertex lies `near` the window and on no
/// round of `circles` (centre, radius in units). `units(m, p)` turns metres into units at p; the
/// edges are read `len` units each way of a corner; `r_max` is the greatest inner radius (metres).
#[allow(clippy::too_many_arguments)]
pub fn rounds(
    paved: &Rings,
    lines: &[WayLine],
    circles: &[(P, f64)],
    near: &dyn Fn(P) -> bool,
    units: &dyn Fn(f64, P) -> f64,
    len: f64,
    r_max: f64,
) -> Vec<Fill> {
    let mut out = Vec::new();
    for ring in paved {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let turn: Vec<Option<f64>> = (0..n).map(|i| Some(turn_at(ring, i))).collect();
        let on = |circles: &[(P, f64)], v: P| {
            circles.iter().any(|&(o, rho)| {
                let d = libm::hypot(f64::from(o[0] - v[0]), f64::from(o[1] - v[1]));
                (d - rho).abs() < units(0.1, v)
            })
        };
        for c in corners(ring, &turn, units(SUPPORT_M, ring[0])) {
            // A corner that a point's round shapes, at any of its vertices, is that round's.
            let mut shaped = false;
            let mut i = c.first;
            loop {
                shaped |= on(circles, ring[i]);
                if i == c.last {
                    break;
                }
                i = (i + 1) % n;
            }
            if shaped {
                continue;
            }
            for at in c.places(&turn) {
                let v = ring[at];
                // A place on a round made here before has its round.
                let made: Vec<(P, f64)> = out.iter().map(|f: &Fill| (f.centre, f.radius)).collect();
                if !near(v) || c.radius >= KINK_SHARE * units(r_max, v) || on(&made, v) {
                    continue;
                }
                let Some((inner, sag)) = at_corner(lines, v) else { continue };
                let r = units(inner, v);
                if c.radius >= KINK_SHARE * r {
                    continue;
                }
                // The outline's edges from the corner: walked backward, and ahead.
                let walk = |dir: isize| {
                    let mut chain = vec![v];
                    let mut s = 0.0;
                    for k in 1..n as isize {
                        let p = ring[(at as isize + dir * k).rem_euclid(n as isize) as usize];
                        let q = chain[chain.len() - 1];
                        s += libm::hypot(f64::from(p[0] - q[0]), f64::from(p[1] - q[1]));
                        chain.push(p);
                        if s >= len {
                            break;
                        }
                    }
                    chain
                };
                // The outline's segments that a circle about the corner can meet. A centre off them by
                // its radius lies in land: it lies on the land side of both edges.
                let reach = len + r;
                let (x0, y0, x1, y1) =
                    (v[0] as f64 - reach, v[1] as f64 - reach, v[0] as f64 + reach, v[1] as f64 + reach);
                let segments: Vec<[P; 2]> = (paved.iter())
                    .flat_map(|ring| (0..ring.len()).map(move |i| [ring[i], ring[(i + 1) % ring.len()]]))
                    .filter(|&[a, b]| {
                        let (lo_x, hi_x) = (f64::from(a[0].min(b[0])), f64::from(a[0].max(b[0])));
                        let (lo_y, hi_y) = (f64::from(a[1].min(b[1])), f64::from(a[1].max(b[1])));
                        hi_x >= x0 && lo_x <= x1 && hi_y >= y0 && lo_y <= y1
                    })
                    .collect();
                let tol = units(sag, v);
                let off = |q: [f64; 2], rho: f64| {
                    clear(&segments, [f64::from(v[0]) + q[0], f64::from(v[1]) + q[1]], rho - tol)
                };
                if let Some(f) = corner_round(v, &walk(-1), &walk(1), r, tol, &off, &|q| bands_at(lines, q)) {
                    // Two corner vertices of one bend can find the same circle: one round per circle.
                    if !out.iter().any(|g: &Fill| g.centre == f.centre && (g.radius - f.radius).abs() < 0.5) {
                        out.push(f);
                    }
                }
            }
        }
    }
    out
}
