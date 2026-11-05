//! Exact integer predicates and the few float constructions of the faces. A float reads only
//! differences of integers or global positions, so two cells compute one point alike.

use crate::frame::CELL_UNITS;

/// A cell-frame point, and a point of a ring.
pub type P = [i32; 2];

/// Half away from zero, as the plan rounds every derived point.
pub fn round(v: f64) -> i64 {
    libm::round(v) as i64
}

pub fn cross(o: P, a: P, b: P) -> i128 {
    let (ax, ay) = (i128::from(a[0]) - i128::from(o[0]), i128::from(a[1]) - i128::from(o[1]));
    let (bx, by) = (i128::from(b[0]) - i128::from(o[0]), i128::from(b[1]) - i128::from(o[1]));
    ax * by - ay * bx
}

/// The point of segment ab nearest the doubled point q, doubled, with its squared distance from q.
pub fn nearest_on(a: P, b: P, q: [i64; 2]) -> (u64, [i64; 2]) {
    let (a2, b2) = ([2.0 * f64::from(a[0]), 2.0 * f64::from(a[1])], [2.0 * f64::from(b[0]), 2.0 * f64::from(b[1])]);
    let (dx, dy) = (b2[0] - a2[0], b2[1] - a2[1]);
    let l2 = dx * dx + dy * dy;
    let t =
        if l2 == 0.0 { 0.0 } else { (((q[0] as f64 - a2[0]) * dx + (q[1] as f64 - a2[1]) * dy) / l2).clamp(0.0, 1.0) };
    let c = [libm::round(a2[0] + t * dx) as i64, libm::round(a2[1] + t * dy) as i64];
    let (ex, ey) = ((c[0] - q[0]) as i128, (c[1] - q[1]) as i128);
    ((ex * ex + ey * ey) as u64, c)
}

/// Twice the signed area: positive for a ring with x east, y south that the plan calls positive.
pub fn area2(ring: &[P]) -> i128 {
    let mut sum = 0i128;
    for (i, &a) in ring.iter().enumerate() {
        let b = ring[(i + 1) % ring.len()];
        sum += i128::from(a[0]) * i128::from(b[1]) - i128::from(b[0]) * i128::from(a[1]);
    }
    sum
}

/// The winding number of the rings about a point in doubled coordinates (2x, 2y). A point on an edge
/// counts by the half-open rule; callers keep their points off the edges.
pub fn winding(rings: &[Vec<P>], q: [i64; 2]) -> i32 {
    let mut w = 0;
    for ring in rings {
        for (i, &a) in ring.iter().enumerate() {
            let b = ring[(i + 1) % ring.len()];
            let (ax, ay) = (2 * i128::from(a[0]), 2 * i128::from(a[1]));
            let (bx, by) = (2 * i128::from(b[0]), 2 * i128::from(b[1]));
            let (qx, qy) = (i128::from(q[0]), i128::from(q[1]));
            let side = (bx - ax) * (qy - ay) - (by - ay) * (qx - ax);
            if ay <= qy {
                if by > qy && side > 0 {
                    w += 1;
                }
            } else if by <= qy && side < 0 {
                w -= 1;
            }
        }
    }
    w
}

/// Whether a point in doubled coordinates lies on a ring's edge or within `d2` (squared, doubled
/// units) of it.
pub fn near_edge(rings: &[Vec<P>], q: [i64; 2], d2: i128) -> bool {
    rings.iter().any(|ring| {
        (0..ring.len()).any(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            dist2_doubled(a, b, q) <= d2
        })
    })
}

/// The squared distance from a doubled point to segment ab, in doubled units, as an exact fraction
/// rounded up: enough to compare with a whole bound.
fn dist2_doubled(a: P, b: P, q: [i64; 2]) -> i128 {
    let (ax, ay) = (2 * i128::from(a[0]), 2 * i128::from(a[1]));
    let (bx, by) = (2 * i128::from(b[0]), 2 * i128::from(b[1]));
    let (qx, qy) = (i128::from(q[0]), i128::from(q[1]));
    let (dx, dy) = (bx - ax, by - ay);
    let t = (qx - ax) * dx + (qy - ay) * dy;
    let len = dx * dx + dy * dy;
    if t <= 0 || len == 0 {
        return (qx - ax).pow(2) + (qy - ay).pow(2);
    }
    if t >= len {
        return (qx - bx).pow(2) + (qy - by).pow(2);
    }
    let c = dx * (qy - ay) - dy * (qx - ax);
    (c * c + len - 1) / len
}

/// An inner point of a patch (section 5.4 of the plan): for each edge of ring 0 from its least vertex,
/// its middle moved 2 units along the axis nearest to its inward normal; the first that lies inside
/// the patch and more than 1 unit from every edge. Doubled coordinates.
pub fn inner_point(rings: &[Vec<P>]) -> Option<[i64; 2]> {
    // A patch under 4 units wide takes a point 1 unit in, off every edge.
    [(4, 4), (2, 0)].into_iter().find_map(|(step, clear)| inner_point_at(rings, step, clear))
}

fn inner_point_at(rings: &[Vec<P>], step: i64, clear: i128) -> Option<[i64; 2]> {
    let ring = &rings[0];
    let start = (0..ring.len()).min_by_key(|&i| (ring[i][1], ring[i][0]))?;
    for k in 0..ring.len() {
        let (a, b) = (ring[(start + k) % ring.len()], ring[(start + k + 1) % ring.len()]);
        let (dx, dy) = (i64::from(b[0]) - i64::from(a[0]), i64::from(b[1]) - i64::from(a[1]));
        let mut q = [i64::from(a[0]) + i64::from(b[0]), i64::from(a[1]) + i64::from(b[1])];
        // The inward normal of a positive ring is (−dy, dx).
        if dy.abs() >= dx.abs() {
            q[0] -= step * dy.signum();
        } else {
            q[1] += step * dx.signum();
        }
        if winding(rings, q) != 0 && !near_edge(rings, q, clear) {
            return Some(q);
        }
    }
    None
}

/// The cell lines (x or y a whole multiple of `CELL_UNITS` in the cell frame) that segment ab crosses
/// strictly, as exact points in the order from a: for x = X, y = y0 + round((X − x0)(y1 − y0) /
/// (x1 − x0)), half away from zero, from the segment's lesser end. The rule reads only the segment, in
/// either direction.
pub fn cell_line_points(a: P, b: P, out: &mut Vec<P>) {
    if a > b {
        let mut back = Vec::new();
        cell_line_points(b, a, &mut back);
        out.extend(back.into_iter().rev());
        return;
    }
    let mut found: Vec<(i128, i128, P)> = Vec::new();
    for axis in 0..2 {
        let (a0, a1) = (i64::from(a[axis]), i64::from(b[axis]));
        let (o0, o1) = (i64::from(a[1 - axis]), i64::from(b[1 - axis]));
        if a0 == a1 {
            continue;
        }
        let (lo, hi) = (a0.min(a1), a0.max(a1));
        let mut line = (lo.div_euclid(CELL_UNITS) + 1) * CELL_UNITS;
        while line < hi {
            if line > lo {
                let num = i128::from(line - a0) * i128::from(o1 - o0);
                let den = i128::from(a1 - a0);
                let q = div_round(num, den);
                let mut p = [0i32; 2];
                p[axis] = line as i32;
                p[1 - axis] = (i128::from(o0) + q) as i32;
                // The parameter along ab as the fraction (line − a0) / (a1 − a0), compared exactly.
                let (n, d) = (i128::from(line - a0), den);
                let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
                found.push((n, d, p));
            }
            line += CELL_UNITS;
        }
    }
    found.sort_by(|x, y| (x.0 * y.1).cmp(&(y.0 * x.1)).then(x.2.cmp(&y.2)));
    found.dedup_by_key(|x| x.2);
    out.extend(found.into_iter().map(|x| x.2).filter(|&p| p != a && p != b));
}

/// num / den rounded half away from zero.
fn div_round(num: i128, den: i128) -> i128 {
    let (n, d) = if den < 0 { (-num, -den) } else { (num, den) };
    if n >= 0 {
        (2 * n + d) / (2 * d)
    } else {
        -((-2 * n + d) / (2 * d))
    }
}

/// The ring with an exact vertex at each crossing of a cell line.
pub fn with_cell_lines(ring: &[P]) -> Vec<P> {
    let mut out = Vec::with_capacity(ring.len() + 4);
    for (i, &a) in ring.iter().enumerate() {
        out.push(a);
        cell_line_points(a, ring[(i + 1) % ring.len()], &mut out);
    }
    out.dedup();
    while out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// The closest points of segments ab and cd, as parameters (s on ab, t on cd), in a frame relative to
/// a: floats of integer differences.
pub fn closest(a: P, b: P, c: P, d: P) -> (f64, f64, f64) {
    let f = |p: P| [f64::from(p[0] - a[0]), f64::from(p[1] - a[1])];
    let (b, c, d) = (f(b), f(c), f(d));
    let seg = |p: [f64; 2], u: [f64; 2], v: [f64; 2]| {
        let (dx, dy) = (v[0] - u[0], v[1] - u[1]);
        let len = dx * dx + dy * dy;
        if len == 0.0 {
            0.0
        } else {
            (((p[0] - u[0]) * dx + (p[1] - u[1]) * dy) / len).clamp(0.0, 1.0)
        }
    };
    let at = |u: [f64; 2], v: [f64; 2], t: f64| [u[0] + (v[0] - u[0]) * t, u[1] + (v[1] - u[1]) * t];
    let dist = |p: [f64; 2], q: [f64; 2]| ((p[0] - q[0]) * (p[0] - q[0]) + (p[1] - q[1]) * (p[1] - q[1])).sqrt();
    let o = [0.0, 0.0];
    let mut best = (f64::INFINITY, 0.0, 0.0);
    // The minimum of two segments that do not cross is at an end of one of them.
    for (s, t) in [(0.0, seg(o, c, d)), (1.0, seg(b, c, d)), (seg(c, o, b), 0.0), (seg(d, o, b), 1.0)] {
        let dd = dist(at(o, b, s), at(c, d, t));
        if dd < best.0 {
            best = (dd, s, t);
        }
    }
    let den = b[0] * (d[1] - c[1]) - b[1] * (d[0] - c[0]);
    if den != 0.0 {
        let s = ((c[0]) * (d[1] - c[1]) - (c[1]) * (d[0] - c[0])) / den;
        let t = (c[0] * b[1] - c[1] * b[0]) / den;
        if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
            best = (0.0, s, t);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_line_points_are_exact_and_in_order() {
        let u = CELL_UNITS as i32;
        let mut out = Vec::new();
        cell_line_points([-10, -7], [u + 10, 3], &mut out);
        assert_eq!(out, [[0, -7], [183505, 0], [u, 3]]);
        let mut back = Vec::new();
        cell_line_points([u + 10, 3], [-10, -7], &mut back);
        assert_eq!(back, [[u, 3], [183505, 0], [0, -7]]);
        let mut corner = Vec::new();
        cell_line_points([-4, -4], [4, 4], &mut corner);
        assert_eq!(corner, [[0, 0]]);
        assert_eq!(div_round(5, 2), 3);
        assert_eq!(div_round(-5, 2), -3);
    }

    #[test]
    fn inner_points_and_winding() {
        let square = vec![vec![[0, 0], [10, 0], [10, 10], [0, 10]]];
        assert!(area2(&square[0]) > 0);
        let q = inner_point(&square).unwrap();
        assert_eq!(winding(&square, q), 1);
        let thin = vec![vec![[0, 0], [100, 0], [100, 2], [0, 2]]];
        assert_eq!(winding(&thin, inner_point(&thin).unwrap()), 1);
    }
}
