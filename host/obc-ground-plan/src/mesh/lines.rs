//! The points of the locked border spans: a rule that reads only the line and the plans within
//! `FOOT_M` of it, so two cells that share a span give it the same points.
//!
//! A span's points: its ends, every plan vertex on it, each nearby plan vertex's foot f and f ± d (d
//! its distance), and every level-18 grid point farther than `BORDER_GRID_M` from those. Then, until
//! nothing changes, a segment halves (rounded to the frame unit, never below one unit) when a plan
//! vertex or a face edge that does not end at one of its ends enters its diametral circle, when it is
//! longer than the near `l_max_m` of the faces beside it, or when it is longer than `BORDER_GRADE`
//! times a neighbour.

use super::params::{l_max_m, BORDER_GRADE, BORDER_GRID_M, FOOT_M, L18};
use super::{Range, Window};
use crate::faces::OUTSIDE;
use crate::frame::CELL_UNITS;

/// One side line of the cell, in (along, across) coordinates: along the line's coordinate, across
/// the other one less the line's.
struct Side {
    vertical: bool,
    at: i32,
    /// Plan vertices within reach of the line.
    vertices: Vec<[i64; 2]>,
    /// Face edges within reach that do not lie on the line.
    edges: Vec<[[i64; 2]; 2]>,
    /// The face edges on the line: their along range and the near `l_max_m` of their face.
    on: Vec<(i64, i64, f64)>,
}

pub struct Border {
    sides: Vec<Side>,
    /// The mesh's metres per unit at a global row: from the window.
    origin_y: f64,
}

impl Border {
    pub fn new(window: &Window) -> Border {
        let c = CELL_UNITS as i32;
        let reach = (FOOT_M / window.mpu(0.0).min(window.mpu(f64::from(c))) * 1.01).ceil() as i64;
        let mut sides: Vec<Side> = [(false, 0), (true, c), (false, c), (true, 0)]
            .into_iter()
            .map(|(vertical, at)| Side { vertical, at, vertices: Vec::new(), edges: Vec::new(), on: Vec::new() })
            .collect();
        for (shift, plan) in window.plans() {
            let point = |v: u32| {
                let p = plan.vertices[v as usize];
                [i64::from(p[0] + shift[0]), i64::from(p[1] + shift[1])]
            };
            for side in sides.iter_mut() {
                let la = |p: [i64; 2]| {
                    if side.vertical {
                        [p[1], p[0] - i64::from(side.at)]
                    } else {
                        [p[0], p[1] - i64::from(side.at)]
                    }
                };
                let span = -reach..=CELL_UNITS + reach;
                for v in 0..plan.vertices.len() as u32 {
                    let p = la(point(v));
                    if p[1].abs() <= reach && span.contains(&p[0]) {
                        side.vertices.push(p);
                    }
                }
                for e in &plan.edges {
                    let (a, b) = (la(point(e.a)), la(point(e.b)));
                    if a[1] == 0 && b[1] == 0 {
                        let face = if e.left != OUTSIDE { e.left } else { e.right };
                        let f = &plan.faces[face as usize];
                        side.on.push((a[0].min(b[0]), a[0].max(b[0]), l_max_m(f.surface, f.rule, Range::Near)));
                        continue;
                    }
                    let near = (a[1].signum() != b[1].signum()) || a[1].abs().min(b[1].abs()) <= reach;
                    if near && a[0].max(b[0]) >= -reach && a[0].min(b[0]) <= CELL_UNITS + reach {
                        side.edges.push([a, b]);
                    }
                }
            }
        }
        for side in sides.iter_mut() {
            side.vertices.sort_unstable();
            side.vertices.dedup();
            side.edges.sort_unstable();
            side.edges.dedup();
            side.on.sort_by(|a, b| a.partial_cmp(b).unwrap());
        }
        Border { sides, origin_y: window.cell.origin().y as f64 }
    }

    /// The points of the span from a to b on a cell line, in order from a to b, ends included.
    pub fn split(&self, a: [i32; 2], b: [i32; 2]) -> Vec<[i32; 2]> {
        let c = CELL_UNITS as i32;
        let vertical = a[0] == b[0];
        let at = if vertical { a[0] } else { a[1] };
        let side = (self.sides.iter())
            .find(|s| s.vertical == vertical && s.at == at)
            .unwrap_or_else(|| panic!("span {a:?} {b:?} is not on a side of the cell {c}"));
        let k = usize::from(vertical);
        let (u0, u1) = (i64::from(a[k].min(b[k])), i64::from(a[k].max(b[k])));
        // The scale at the span's middle row, global: both cells read the same.
        let y = if vertical { (u0 + u1) as f64 / 2.0 } else { f64::from(at) };
        let mpu = crate::frame::metres_per_unit_at(self.origin_y + y);
        let points = span_points(side, u0, u1, mpu);
        let mut out: Vec<[i32; 2]> =
            (points.iter()).map(|&u| if vertical { [at, u as i32] } else { [u as i32, at] }).collect();
        if a[k] > b[k] {
            out.reverse();
        }
        out
    }
}

/// Twice the squared distance from the doubled point m to the segment ab, as a fraction num / den.
fn dist2(a: [i64; 2], b: [i64; 2], m: [i128; 2]) -> (i128, i128) {
    let (a2, b2) = (a.map(|v| 2 * i128::from(v)), b.map(|v| 2 * i128::from(v)));
    let e = [b2[0] - a2[0], b2[1] - a2[1]];
    let w = [m[0] - a2[0], m[1] - a2[1]];
    let len = e[0] * e[0] + e[1] * e[1];
    let t = e[0] * w[0] + e[1] * w[1];
    if len == 0 || t <= 0 {
        return (w[0] * w[0] + w[1] * w[1], 1);
    }
    if t >= len {
        let v = [m[0] - b2[0], m[1] - b2[1]];
        return (v[0] * v[0] + v[1] * v[1], 1);
    }
    let x = e[0] * w[1] - e[1] * w[0];
    (x * x, len)
}

fn span_points(side: &Side, u0: i64, u1: i64, mpu: f64) -> Vec<i64> {
    let foot = (FOOT_M / mpu).floor() as i64;
    let near = |p: &[i64; 2]| p[0] >= u0 - foot && p[0] <= u1 + foot && p[1].abs() <= foot;
    let vertices: Vec<[i64; 2]> = side.vertices.iter().copied().filter(near).collect();
    let edges: Vec<[[i64; 2]; 2]> = (side.edges.iter())
        .filter(|[a, b]| a[0].max(b[0]) >= u0 - foot && a[0].min(b[0]) <= u1 + foot)
        .copied()
        .collect();
    let mut pts = vec![u0, u1];
    for v in &vertices {
        let d = v[1].abs();
        pts.extend([v[0], v[0] - d, v[0] + d].into_iter().filter(|u| (u0..=u1).contains(u)));
    }
    pts.sort_unstable();
    pts.dedup();
    let clear = BORDER_GRID_M / mpu;
    let grid = i64::from(L18);
    let mut g = (u0 / grid + 1) * grid;
    let mut extra = Vec::new();
    while g < u1 {
        let nearest = pts.iter().map(|&p| (p - g).abs()).min().unwrap_or(i64::MAX);
        if nearest as f64 > clear {
            extra.push(g);
        }
        g += grid;
    }
    pts.extend(extra);
    pts.sort_unstable();
    pts.dedup();
    // The near l_max of the faces beside a piece: the least of those on the line that overlap it.
    let l_max = |p: i64, q: i64| {
        let m = (side.on.iter()).filter(|&&(a, b, _)| a < q && b > p).map(|&(_, _, l)| l).fold(f64::INFINITY, f64::min);
        (if m.is_finite() {
            m
        } else {
            l_max_m(crate::faces::Surface::Terrain, crate::faces::HeightRule::Terrain, Range::Near)
        }) / mpu
    };
    loop {
        let mut split = Vec::new();
        for k in 0..pts.len() - 1 {
            let (p, q) = (pts[k], pts[k + 1]);
            let len = q - p;
            if len < 2 {
                continue;
            }
            let m = [i128::from(p + q), 0];
            let r2 = i128::from(len) * i128::from(len);
            let ends = |v: &[i64; 2]| v[1] == 0 && (v[0] == p || v[0] == q);
            let enters_v = vertices.iter().any(|v| {
                let d = [2 * i128::from(v[0]) - m[0], 2 * i128::from(v[1])];
                !ends(v) && d[0] * d[0] + d[1] * d[1] < r2
            });
            let enters_e = || {
                edges.iter().any(|[a, b]| {
                    if ends(a) || ends(b) {
                        return false;
                    }
                    let (num, den) = dist2(*a, *b, m);
                    num < r2 * den
                })
            };
            let grade = |n: Option<usize>| n.is_some_and(|n| len > BORDER_GRADE * (pts[n + 1] - pts[n]));
            if enters_v
                || len as f64 > l_max(p, q)
                || grade(k.checked_sub(1))
                || grade((k + 1 < pts.len() - 1).then_some(k + 1))
                || enters_e()
            {
                split.push(p + len / 2);
            }
        }
        if split.is_empty() {
            return pts;
        }
        pts.extend(split);
        pts.sort_unstable();
        pts.dedup();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(vertices: Vec<[i64; 2]>, edges: Vec<[[i64; 2]; 2]>) -> Side {
        Side { vertical: true, at: 0, vertices, edges, on: Vec::new() }
    }

    #[test]
    fn an_edge_at_a_small_angle_grades_the_points_toward_its_crossing_and_no_segment_is_encroached() {
        let mpu = 0.0016;
        // An edge crossing the line at 2048 at about 10°: 1,000 units along, 176 across.
        let s =
            side(vec![[2048, 0], [3048, 176], [1048, -176]], vec![[[2048, 0], [3048, 176]], [[1048, -176], [2048, 0]]]);
        let pts = span_points(&s, 0, 4096, mpu);
        assert!(pts.contains(&2048) && pts.first() == Some(&0) && pts.last() == Some(&4096));
        // A one-unit segment is never halved.
        for w in pts.windows(2).filter(|w| w[1] - w[0] > 1) {
            let m = [i128::from(w[0] + w[1]), 0];
            let r2 = i128::from(w[1] - w[0]).pow(2);
            for [a, b] in &s.edges {
                let touches = |v: &[i64; 2]| v[1] == 0 && (v[0] == w[0] || v[0] == w[1]);
                if !touches(a) && !touches(b) {
                    let (num, den) = dist2(*a, *b, m);
                    assert!(num >= r2 * den, "{w:?} encroached");
                }
            }
        }
        let near = pts.iter().filter(|&&u| (u - 2048).abs() < 200).count();
        assert!(near >= 4, "graded toward the crossing: {pts:?}");
        for w in pts.windows(3) {
            let (l, r) = (w[1] - w[0], w[2] - w[1]);
            assert!(l <= BORDER_GRADE * r && r <= BORDER_GRADE * l, "{pts:?}");
        }
    }

    #[test]
    fn an_empty_span_holds_its_grid_points_and_respects_l_max() {
        let pts = span_points(&side(Vec::new(), Vec::new()), 0, 2048, 0.0016);
        assert_eq!(pts, [0, 1024, 2048]);
    }
}
