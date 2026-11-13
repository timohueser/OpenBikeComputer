//! The rounds at the inward corners of the paved outline that no point's round shapes: where two
//! ways' carriageways meet without a graph point between them, where a third piece lies between a
//! point's two arms, and where a way turns inside a point's region. A corner tighter than the least
//! inner radius of the ways whose edges form it gets the circle tangent to its two edges, off the
//! paved area, of that radius or the largest of the ladder that fits.

use std::cell::OnceCell;

use super::check::{place, radii, turn_at, KINK_SHARE, SUPPORT_M};
use super::geom::P;
use super::points::{corner_round, Circle};
use super::stroke::Rings;
use super::{WayLine, LAYERS};

/// A way forms a corner where its carriageway's edge runs within this many units of the corner: the
/// overlay's snap (1 unit) and the rounding of the centre line's samples and of the edge to the unit
/// grid (0.71 units each) move a corner off its edge by under 3 units.
const ON_EDGE: f64 = 4.0;

/// The ways at a point: the least `inner_radius_m` and the greatest `sag_plan_m` of the ways whose
/// carriageway edge runs through q, if any. The paved outline holds no water.
pub fn at_corner(lines: &[WayLine], q: P) -> Option<(f64, f64)> {
    let (x, y) = (f64::from(q[0]), f64::from(q[1]));
    let mut out: Option<(f64, f64)> = None;
    for line in lines.iter().filter(|l| !l.sec.water) {
        let touches = line.near([x, y], ON_EDGE).any(|i| {
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

/// The way whose carriageway edge runs nearest q (an index into `lines`, `usize::MAX` for none), with
/// its band layers' outer offsets past that edge (units, `LAYERS`). A water line is no way.
fn bands_at(lines: &[WayLine], q: [f64; 2]) -> (usize, [f64; LAYERS]) {
    let mut best = (f64::INFINITY, usize::MAX, [0.0; LAYERS]);
    for (l, line) in lines.iter().enumerate().filter(|(_, l)| !l.sec.water) {
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
                best = (off, l, w);
            }
        }
    }
    (best.1, best.2)
}

/// A round at a corner of the paved outline: its ring, its circle, and a point inside the paved area
/// at the corner (doubled), whose label the ring takes.
pub struct Fill {
    pub ring: Vec<P>,
    /// Its band rings (`LAYERS`).
    pub bands: [Vec<Vec<P>>; LAYERS],
    pub circle: Circle,
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

/// A place of the paved outline: a vertex whose round the rule decides.
struct Place {
    ring: usize,
    at: usize,
    v: P,
    /// Its own turn (negative: toward the right) and the next vertex, the keys of its order.
    turn: f64,
    next: P,
    /// The least radius of the corner about it (units).
    radius: f64,
}

/// The rounds at the corners of the paved outline `paved` (outer rings counter-clockwise, holes
/// clockwise: the paved side on the left). A place gets the circle tangent to the outline's two
/// edges from it, off the paved area, unless it lies on a round of `circles` (centre, radius in
/// units) or the round of a sharper place within `len_m` that is first: a place is first if it lies
/// on the round of no sharper place within `len_m`. So a place's round depends on the places within
/// twice `len_m` only, in no order of rings. The rounds of the places within `len_m` + `r_max` of
/// the window can reach it; `beyond(p)` is the distance of p past the window (metres). `units(m, p)`
/// turns metres into units at p; `r_max` is the greatest inner radius (metres).
pub fn rounds(
    paved: &Rings,
    lines: &[WayLine],
    circles: &[(P, f64)],
    beyond: &dyn Fn(P) -> f64,
    units: &dyn Fn(f64, P) -> f64,
    len_m: f64,
    r_max: f64,
) -> Vec<Fill> {
    let reach_m = len_m + r_max;
    let mut places: Vec<Place> = Vec::new();
    for (k, ring) in paved.iter().enumerate() {
        let n = ring.len();
        if n < 3 {
            continue;
        }
        let turn: Vec<f64> = (0..n).map(|i| turn_at(ring, i)).collect();
        let some: Vec<Option<f64>> = turn.iter().map(|&t| Some(t)).collect();
        let radius = radii(ring, &some, &|p| units(SUPPORT_M, p));
        // A ring that is one corner all round has no edge to run a round into.
        if radius.iter().all(Option::is_some) {
            continue;
        }
        for at in 0..n {
            let v = ring[at];
            if beyond(v) > reach_m + 2.0 * len_m {
                continue;
            }
            if let Some(radius) = place(ring, &some, &radius, at, units(len_m, v)) {
                places.push(Place { ring: k, at, v, turn: turn[at], next: ring[(at + 1) % n], radius });
            }
        }
    }
    places.sort_by(|a, b| a.turn.total_cmp(&b.turn).then(a.v.cmp(&b.v)).then(a.next.cmp(&b.next)));
    let on = |o: P, rho: f64, v: P| {
        let d = libm::hypot(f64::from(o[0] - v[0]), f64::from(o[1] - v[1]));
        (d - rho).abs() < units(0.1, v)
    };
    // A place on a point's round has its round, as the kink check reads it: within 0.1 m of its
    // circle, over that check's tolerance for most classes (`sag_plan_m` 0.05 m and twice the
    // precision, 0.06 m). A place's round is made when a decision reads it.
    let fills: Vec<OnceCell<Option<Fill>>> = places.iter().map(|_| OnceCell::new()).collect();
    let fill = |j: usize| {
        fills[j]
            .get_or_init(|| {
                let p = &places[j];
                if circles.iter().any(|&(o, rho)| on(o, rho, p.v)) {
                    return None;
                }
                round_at(paved, lines, &paved[p.ring], p.at, p.radius, units, len_m, r_max)
            })
            .as_ref()
    };
    // Whether a sharper place within `len_m` of place j and accepted by `ok` rounds it: j lies on
    // its circle, or has the same circle (two places of one bend).
    let covered = |j: usize, ok: &dyn Fn(usize) -> bool| {
        let q = &places[j];
        (0..j).any(|i| {
            let p = &places[i];
            let close = libm::hypot(f64::from(p.v[0] - q.v[0]), f64::from(p.v[1] - q.v[1])) <= units(len_m, q.v);
            close
                && fill(i).is_some_and(|f| {
                    let c = &f.circle;
                    let same = fill(j)
                        .is_some_and(|g| g.circle.centre == c.centre && (g.circle.radius - c.radius).abs() < 0.5);
                    (on(c.centre, c.radius, q.v) || same) && ok(i)
                })
        })
    };
    let first: Vec<OnceCell<bool>> = places.iter().map(|_| OnceCell::new()).collect();
    let is_first = |i: usize| *first[i].get_or_init(|| fill(i).is_some() && !covered(i, &|_| true));
    let made: Vec<usize> = (0..places.len())
        .filter(|&j| beyond(places[j].v) <= reach_m && fill(j).is_some() && !covered(j, &is_first))
        .collect();
    let mut fills = fills;
    made.into_iter().filter_map(|j| fills[j].take().flatten()).collect()
}

/// The round at place `at` of a ring of the paved outline, at a corner of radius `radius` (units).
#[allow(clippy::too_many_arguments)]
fn round_at(
    paved: &Rings,
    lines: &[WayLine],
    ring: &[P],
    at: usize,
    radius: f64,
    units: &dyn Fn(f64, P) -> f64,
    len_m: f64,
    r_max: f64,
) -> Option<Fill> {
    let n = ring.len();
    let v = ring[at];
    if radius >= KINK_SHARE * units(r_max, v) {
        return None;
    }
    let (inner, sag) = at_corner(lines, v)?;
    let r = units(inner, v);
    if radius >= KINK_SHARE * r {
        return None;
    }
    let len = units(len_m, v);
    // The outline's edges from the corner: walked backward, and ahead, past `len` along them.
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
    // The outline's segments that a circle about the corner can meet: its centre lies within `len`
    // and its radius of the corner. A centre off them by its radius lies in land: it lies on the land
    // side of both edges.
    let reach = len + 2.0 * r;
    let (x0, y0, x1, y1) = (v[0] as f64 - reach, v[1] as f64 - reach, v[0] as f64 + reach, v[1] as f64 + reach);
    let segments: Vec<[P; 2]> = (paved.iter())
        .flat_map(|ring| (0..ring.len()).map(move |i| [ring[i], ring[(i + 1) % ring.len()]]))
        .filter(|&[a, b]| {
            let (lo_x, hi_x) = (f64::from(a[0].min(b[0])), f64::from(a[0].max(b[0])));
            let (lo_y, hi_y) = (f64::from(a[1].min(b[1])), f64::from(a[1].max(b[1])));
            hi_x >= x0 && lo_x <= x1 && hi_y >= y0 && lo_y <= y1
        })
        .collect();
    let tol = units(sag, v);
    let off = |q: [f64; 2], rho: f64| clear(&segments, [f64::from(v[0]) + q[0], f64::from(v[1]) + q[1]], rho - tol);
    corner_round(v, &walk(-1), &walk(1), r, tol, len, &off, &|q| bands_at(lines, q))
}
