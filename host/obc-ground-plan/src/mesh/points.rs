//! The points and constraints of D's triangulation:
//! - every face edge with a sharp face on a side, split into equal pieces no longer than the
//!   `l_max_m` of its faces;
//! - D's border, locked: each side of a cell of D toward a cell outside D, with its middle (a grid
//!   point). A span of the cell line also holds the plan vertices that either cell has on it, and its
//!   middle only where that lies farther than `GRID_CLEARANCE_M` from them, so both cells give it the
//!   same points;
//! - the grid points inside D farther than `GRID_CLEARANCE_M` from every face edge, in every face.
//!
//! Every point is an integer of the cell's frame. Ground faces' edges are no constraints: the ground
//! is one continuous field, so a triangle may span two ground faces.

use super::cdt::{Segment, INSIDE};
use super::params::{l_max_m, sharp, GRID, GRID_CLEARANCE_M, SIDE};
use super::region::Region;
use crate::faces::{EdgeKind, FacePlan, OUTSIDE};
use crate::frame::{metres_per_unit_at, CELL_UNITS};

pub type P = [i32; 2];

pub struct Input {
    /// Sorted by (y, x), each once.
    pub points: Vec<P>,
    pub segments: Vec<Segment>,
    /// `segments[..edges]` are face edges, the rest D's border.
    pub edges: usize,
    /// Per face edge piece, whether it lies on a deck end line (`EdgeKind::DeckEdge`): its slot is an end face.
    pub end_faces: Vec<bool>,
    pub grid_points: u32,
    /// Face edges of a sharp face that do not lie in D: none, as R grows past every sharp face.
    pub outside_d: u32,
}

/// The cell's four side lines in the order of `Rim::lines`: north, east, south, west.
pub(super) fn side(a: P, b: P) -> Option<usize> {
    let c = CELL_UNITS as i32;
    match (a, b) {
        (a, b) if a[1] == 0 && b[1] == 0 => Some(0),
        (a, b) if a[0] == c && b[0] == c => Some(1),
        (a, b) if a[1] == c && b[1] == c => Some(2),
        (a, b) if a[0] == 0 && b[0] == 0 => Some(3),
        _ => None,
    }
}

/// The point at k/n of the way from a to b, rounded to the frame unit.
fn part(a: P, b: P, k: i64, n: i64) -> P {
    [0, 1].map(|i| {
        let v = i64::from(a[i]) * (n - k) + i64::from(b[i]) * k;
        (2 * v + n).div_euclid(2 * n) as i32
    })
}

fn dist(a: P, b: P, p: [f64; 2]) -> f64 {
    let (e, w) = ([f64::from(b[0] - a[0]), f64::from(b[1] - a[1])], [p[0] - f64::from(a[0]), p[1] - f64::from(a[1])]);
    let l2 = e[0] * e[0] + e[1] * e[1];
    let t = if l2 > 0.0 { ((w[0] * e[0] + w[1] * e[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    libm::hypot(w[0] - t * e[0], w[1] - t * e[1])
}

/// The input of D's triangulation. `lines`: the plan vertices of the cell and its neighbour on each
/// side line, as coordinates along it; `origin_y`: the cell's global row, for the metres per unit.
pub fn input(plan: &FacePlan, region: &Region, lines: &[Vec<i32>; 4], origin_y: f64) -> Input {
    let mpu = |y: f64| metres_per_unit_at(origin_y + y);
    let mut ends: Vec<(P, P, u32, u32)> = Vec::new();
    let mut end_faces = Vec::new();
    let mut outside_d = 0;
    for e in plan.edges.iter().filter(|e| e.left != OUTSIDE && e.right != OUTSIDE) {
        let rule = |f: u32| plan.faces[f as usize].rule;
        if !sharp(rule(e.left)) && !sharp(rule(e.right)) {
            continue;
        }
        let (a, b) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
        let mid = [(a[0] + b[0]).div_euclid(2), (a[1] + b[1]).div_euclid(2)];
        if !region.d(mid[0].div_euclid(2 * GRID), mid[1].div_euclid(2 * GRID)) {
            outside_d += 1;
            continue;
        }
        let l = [e.left, e.right]
            .map(|f| {
                let f = &plan.faces[f as usize];
                l_max_m(f.surface, f.rule)
            })
            .into_iter()
            .fold(f64::INFINITY, f64::min);
        let len = libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])) * mpu(f64::from(mid[1]));
        let n = (len / l).ceil().max(1.0) as i64;
        let mut last = a;
        for k in 1..=n {
            let p = if k == n { b } else { part(a, b, k, n) };
            if p != last {
                ends.push((last, p, e.left, e.right));
                end_faces.push(e.kind == EdgeKind::DeckEdge);
                last = p;
            }
        }
    }
    let edges = ends.len();
    // Grid points within the clearance of a face edge stay out.
    let n = SIDE as usize;
    let mut blocked = vec![false; n * n];
    for &(a, b, _, _) in &ends {
        let clear = GRID_CLEARANCE_M / mpu(f64::from(a[1] + b[1]) / 2.0);
        let range = |lo: i32, hi: i32| {
            let first = ((f64::from(lo) - clear) / f64::from(GRID)).ceil().max(0.0) as i32;
            let last = ((f64::from(hi) + clear) / f64::from(GRID)).floor().min(f64::from(SIDE - 1)) as i32;
            first..=last
        };
        for j in range(a[1].min(b[1]), a[1].max(b[1])) {
            for i in range(a[0].min(b[0]), a[0].max(b[0])) {
                if dist(a, b, [f64::from(i * GRID), f64::from(j * GRID)]) <= clear {
                    blocked[(j * SIDE + i) as usize] = true;
                }
            }
        }
    }
    // D's border, D on the left of each piece.
    for bound in region.bounds() {
        let (a, b) = (bound.a, bound.b);
        let mid = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
        let mut pts = vec![a, b];
        match side(a, b).filter(|_| bound.span) {
            Some(s) => {
                let k = if s % 2 == 0 { 0 } else { 1 };
                let (lo, hi) = (a[k].min(b[k]), a[k].max(b[k]));
                let on: Vec<i32> = lines[s].iter().copied().filter(|&v| v > lo && v < hi).collect();
                let clear = GRID_CLEARANCE_M / mpu(f64::from(mid[1]));
                if on.iter().all(|&v| f64::from((v - mid[k]).abs()) > clear) {
                    pts.push(mid);
                }
                pts.extend(on.into_iter().map(|v| if k == 0 { [v, a[1]] } else { [a[0], v] }));
            }
            None => pts.push(mid),
        }
        let k = if a[0] == b[0] { 1 } else { 0 };
        let toward = (b[k] - a[k]).signum();
        pts.sort_unstable_by_key(|p| p[k] * toward);
        pts.dedup();
        for w in pts.windows(2) {
            ends.push((w[0], w[1], INSIDE, OUTSIDE));
        }
    }
    let mut points: Vec<P> = ends.iter().flat_map(|e| [e.0, e.1]).collect();
    let mut grid_points = 0;
    for j in 0..SIDE {
        for i in 0..SIDE {
            if region.inside_d(i, j) && !blocked[(j * SIDE + i) as usize] {
                points.push([i * GRID, j * GRID]);
                grid_points += 1;
            }
        }
    }
    points.sort_unstable_by_key(|p| (p[1], p[0]));
    points.dedup();
    let at = |p: P| points.binary_search_by_key(&(p[1], p[0]), |q| (q[1], q[0])).unwrap() as u32;
    let segments = ends.iter().map(|&(a, b, left, right)| Segment { a: at(a), b: at(b), left, right }).collect();
    Input { segments, edges, end_faces, grid_points, outside_d, points }
}

/// The face of the plan at each grid point of the cell, index j × `SIDE` + i, by scan lines half a
/// unit south of the rows: no plan vertex lies on such a line, so no two crossings tie at a vertex. A
/// grid point on a face edge takes a face beside it: it lies within `GRID_CLEARANCE_M` of the edge or
/// on an edge between two ground faces, where either face is right. `OUTSIDE` on the cell's lines.
pub fn grid_faces(plan: &FacePlan) -> Vec<u32> {
    let n = SIDE as usize;
    let mut rows: Vec<Vec<(f64, u32)>> = vec![Vec::new(); n];
    for e in &plan.edges {
        let (a, b) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
        if a[1] == b[1] {
            continue;
        }
        // The rows y with min ≤ y < max: their scan lines y + ½ cross the edge.
        let (lo, hi) = (a[1].min(b[1]), a[1].max(b[1]));
        let first = lo.div_euclid(GRID) + i32::from(lo.rem_euclid(GRID) != 0);
        let last = (hi - 1).div_euclid(GRID);
        // The face east of the edge: on its right where it runs south.
        let east = if b[1] > a[1] { e.right } else { e.left };
        for j in first.max(1)..=last.min(SIDE - 2) {
            let y = f64::from(j * GRID) + 0.5;
            let x = f64::from(a[0]) + (y - f64::from(a[1])) * f64::from(b[0] - a[0]) / f64::from(b[1] - a[1]);
            rows[j as usize].push((x, east));
        }
    }
    let mut out = vec![OUTSIDE; n * n];
    for (j, row) in rows.iter_mut().enumerate() {
        row.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut k = 0;
        let mut face = OUTSIDE;
        for i in 1..n - 1 {
            let x = (i as i32 * GRID) as f64;
            while k < row.len() && row[k].0 <= x {
                face = row[k].1;
                k += 1;
            }
            out[j * n + i] = face;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faces::{Edge, EdgeKind};
    use crate::frame::CellKey;

    #[test]
    fn a_face_tip_on_a_grid_row_gives_the_row_east_of_it_the_ground_in_either_edge_order() {
        // Face 1, a triangle whose north tip lies on row 10; face 0 about it.
        let edge = |a, b, left, right| Edge { a, b, left, right, kind: EdgeKind::Continuous, flags: 0 };
        let orders = [
            vec![edge(0, 1, 0, 1), edge(2, 0, 0, 1), edge(1, 2, 1, 0)],
            vec![edge(2, 0, 0, 1), edge(0, 1, 0, 1), edge(1, 2, 1, 0)],
        ];
        for edges in orders {
            let plan = FacePlan {
                cell: CellKey { x: 0, y: 0 },
                level: 0,
                vertices: vec![[5000, 10 * GRID], [3000, 14000], [7000, 14000]],
                faces: Vec::new(),
                edges,
                corners: Vec::new(),
                mouths: Vec::new(),
                stats: Default::default(),
            };
            let row = &grid_faces(&plan)[10 * SIDE as usize..];
            assert_eq!(row[6..10], [0, 0, 0, 0], "{:?}", plan.edges);
        }
    }
}
