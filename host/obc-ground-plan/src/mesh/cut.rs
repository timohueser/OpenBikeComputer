//! The cut of the built tunnel ends in a cell's mesh (`FacePlan::cuts`): its rings with a whole point on each cell line
//! that an edge crosses, so two cells clip a ring at the same point; and which points it holds.

use crate::faces::FacePlan;
use crate::frame::CELL_UNITS;

pub type P = [i32; 2];

/// A ring as whole points from the cell's origin.
pub type Ring = Vec<[i64; 2]>;

/// The rings of each end's cut, each edge that crosses a cell line (x or y at 0 or the cell's side) cut there at the
/// nearest whole unit on the line.
pub fn rings(plan: &FacePlan) -> Vec<Vec<Ring>> {
    let c = CELL_UNITS;
    let lines = [(0, 0), (0, c), (1, 0), (1, c)];
    (plan.cuts.iter())
        .map(|cut| {
            (cut.rings.iter())
                .map(|ring| {
                    let mut out = Vec::with_capacity(ring.len());
                    for (k, &a) in ring.iter().enumerate() {
                        let b = ring[(k + 1) % ring.len()];
                        out.push(a);
                        // Each line that the edge crosses, at its share of the way along the edge.
                        let mut at: Vec<(f64, [i64; 2])> = (lines.iter())
                            .filter(|&&(axis, v)| (a[axis] - v).signum() * (b[axis] - v).signum() < 0)
                            .map(|&(axis, v)| {
                                let (d, o) = (b[axis] - a[axis], 1 - axis);
                                let num = i128::from(v - a[axis]) * i128::from(b[o] - a[o]);
                                let mut p = [0; 2];
                                p[axis] = v;
                                p[o] = a[o] + nearest(num, i128::from(d));
                                ((v - a[axis]) as f64 / d as f64, p)
                            })
                            .collect();
                        at.sort_by(|x, y| x.0.total_cmp(&y.0));
                        out.extend(at.into_iter().map(|(_, p)| p).filter(|&p| p != a && p != b));
                    }
                    out.dedup();
                    out
                })
                .collect()
        })
        .collect()
}

/// A face edge piece lies on a wall where its middle lies this near the wall's line (units): a wall's ends are whole
/// points off the edge, and the snap moves an edge through them.
const ON_WALL: i128 = 2;

/// The walls of the cuts.
pub fn walls(plan: &FacePlan) -> Vec<[[i64; 2]; 2]> {
    plan.cuts.iter().flat_map(|cut| cut.walls.iter().copied()).collect()
}

/// The ends of the walls inside the cell, off its lines: the mesh splits its face edges there.
pub fn wall_ends(walls: &[[[i64; 2]; 2]]) -> Vec<P> {
    let inside = |p: &[i64; 2]| p.iter().all(|&v| v > 0 && v < CELL_UNITS);
    walls.iter().flatten().filter(|p| inside(p)).map(|p| p.map(|v| v as i32)).collect()
}

/// Whether the face edge piece p-q lies on a wall: its middle within `ON_WALL` of the wall's line, between its ends.
pub fn on_wall(walls: &[[[i64; 2]; 2]], p: P, q: P) -> bool {
    let m = [i64::from(p[0]) + i64::from(q[0]), i64::from(p[1]) + i64::from(q[1])];
    walls.iter().any(|&[a, b]| {
        // In doubled units, from a.
        let (d, v) = ([b[0] - a[0], b[1] - a[1]].map(i128::from), [m[0] - 2 * a[0], m[1] - 2 * a[1]].map(i128::from));
        let (cross, dot, len2) = (d[0] * v[1] - d[1] * v[0], d[0] * v[0] + d[1] * v[1], d[0] * d[0] + d[1] * d[1]);
        len2 > 0 && cross * cross <= 4 * ON_WALL * ON_WALL * len2 && dot > 0 && dot < 2 * len2
    })
}

/// The nearest whole number to n / d, a half up.
fn nearest(n: i128, d: i128) -> i64 {
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    (2 * n + d).div_euclid(2 * d) as i64
}

/// Whether a point lies in the closed square of the cell.
fn in_cell(p: [i64; 2]) -> bool {
    (0..=CELL_UNITS).contains(&p[0]) && (0..=CELL_UNITS).contains(&p[1])
}

/// The points of the rings on the cell's lines, as `Rim::lines` holds them: north, east, south, west.
pub fn on_lines(rings: &[Vec<Ring>]) -> [Vec<i32>; 4] {
    let c = CELL_UNITS;
    let mut out: [Vec<i32>; 4] = Default::default();
    for p in rings.iter().flatten().flatten().copied().filter(|&p| in_cell(p)) {
        for (side, on, along) in
            [(0, p[1] == 0, p[0]), (1, p[0] == c, p[1]), (2, p[1] == c, p[0]), (3, p[0] == 0, p[1])]
        {
            if on {
                out[side].push(along as i32);
            }
        }
    }
    out
}

/// The edges of the rings in the closed square of the cell, as (end, ring, edge).
pub fn pieces(rings: &[Vec<Ring>]) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    for (e, end) in rings.iter().enumerate() {
        for (r, ring) in end.iter().enumerate() {
            for k in 0..ring.len() {
                let (a, b) = (ring[k], ring[(k + 1) % ring.len()]);
                if a != b && in_cell(a) && in_cell(b) {
                    out.push((e, r, k));
                }
            }
        }
    }
    out
}

/// Whether the rings of one end hold point q / `scale` (whole points times `scale`), even-odd; q lies on no edge.
pub fn holds(rings: &[Ring], q: [i64; 2], scale: i64) -> bool {
    let mut inside = false;
    for ring in rings {
        for k in 0..ring.len() {
            let (a, b) = (ring[k].map(|v| v * scale), ring[(k + 1) % ring.len()].map(|v| v * scale));
            if (a[1] > q[1]) != (b[1] > q[1]) {
                // The edge crosses the row of q: q lies west of it where the turn a, b, q has the sign of b − a in y.
                let turn = i128::from(b[0] - a[0]) * i128::from(q[1] - a[1])
                    - i128::from(b[1] - a[1]) * i128::from(q[0] - a[0]);
                if (turn > 0) == (b[1] > a[1]) {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ring_across_a_cell_line_takes_one_whole_point_on_it_in_both_cells() {
        let c = CELL_UNITS;
        // A triangle across the east line, as this cell and its east neighbour hold it.
        let ring = vec![[c - 300, 100], [c + 200, 151], [c - 100, 400]];
        let here = rings(&plan(vec![vec![ring.clone()]]));
        let there = rings(&plan(vec![vec![ring.iter().map(|p| [p[0] - c, p[1]]).collect()]]));
        let line: Vec<i64> = here[0][0].iter().filter(|p| p[0] == c).map(|p| p[1]).collect();
        let east: Vec<i64> = there[0][0].iter().filter(|p| p[0] == 0).map(|p| p[1]).collect();
        // (c, 100 + 300 · 51 / 500 = 130.6) and (c, 151 + 200 · 249 / 300 = 317.0).
        assert_eq!(line, [131, 317]);
        assert_eq!(line, east);
        assert_eq!(on_lines(&here)[1], [131, 317]);
        assert_eq!(pieces(&here).len(), 3, "the edges in the cell: two halves and the west edge");
    }

    #[test]
    fn the_rings_of_an_end_hold_a_point_even_odd() {
        let outer = vec![[0, 0], [100, 0], [100, 100], [0, 100]];
        let hole = vec![[20, 20], [20, 80], [80, 80], [80, 20]];
        let rings = [outer, hole];
        assert!(holds(&rings, [10, 50], 1));
        assert!(!holds(&rings, [50, 50], 1));
        assert!(!holds(&rings, [150, 50], 1));
        // A third of a unit inside the outer ring's west edge.
        assert!(holds(&rings, [1, 150], 3) && !holds(&rings, [-1, 150], 3));
    }

    fn plan(cuts: Vec<Vec<Ring>>) -> FacePlan {
        FacePlan {
            cell: crate::frame::CellKey { x: 0, y: 0 },
            level: 0,
            vertices: Vec::new(),
            faces: Vec::new(),
            edges: Vec::new(),
            corners: Vec::new(),
            mouths: Vec::new(),
            cuts: cuts.into_iter().map(|rings| crate::openings::Cut { rings, walls: Vec::new() }).collect(),
            stats: Default::default(),
        }
    }
}
