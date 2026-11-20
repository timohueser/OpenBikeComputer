//! The cut of each built end: where the ground of the ground plan, with the stretches to the openings of the built
//! chains on the ground, lies inside a box of its part. The device's ground plan leaves a hole in its mesh there.
//!
//! The rings are simple and apart by construction: a point counts as inside the boxes only deeper than `EPS_M`, so a
//! value of about 0 on a grid line is outside; each crossing lies on its grid edge at whole centimetres, at least a
//! centimetre from both ends, so no two crossings meet; and a ring leaves out only a point whose removal moves no edge
//! onto another.

use std::collections::HashMap;

use obc_ground_plan::graph::OpenEnd;
use obc_trailview_parts::{
    corners, end_boxes, metres_per_unit, outside, outside_plan, outside_wall, Chord, End, Rules, BOX,
};
use serde_json::{json, Value};

use super::ladder::Measure;
use super::measure::Ask;
use super::num;
use crate::tiles::{frame_point, height_code};
use crate::{Part, Tags};

/// The step of the grid of ground points over the boxes, in centimetres.
const STEP_CM: i64 = 50;
const STEP_M: f64 = STEP_CM as f64 / 100.0;
/// The grid reads the ground where a point lies this near a box in plan (m); farther, it is outside every box.
const NEAR_M: f64 = 0.75;
/// A point counts as inside the boxes where it lies deeper than this (m).
const EPS_M: f64 = 1e-6;
/// A ring leaves out a point that lies nearer than this to the line of its neighbours (cm).
const KEEP_CM: f64 = 2.0;
/// The least margin of a box to the faces of its part (m): ground that lies no deeper than this in the sides and the
/// roof of the boxes lies in the part's wall, so a ring of only such ground goes.
const WALL_M: f64 = 0.11;
/// World units per unit of [0, 1], and frame units of the ground plan per world unit.
const WORLD: f64 = (1u64 << 30) as f64;
const FRAME: f64 = 16.0;

/// A ring of the cut: points in centimetres in along the opening's heading and to the left of it.
type Ring = Vec<[i64; 2]>;

/// Adds the tag `cut` to each `tunnel-end` of a built chain whose ground lies in its part, with the stretches of
/// `opened` on the ground; gives its counts.
pub(crate) fn add(
    features: &mut [(&'static str, Tags, Part)],
    opened: &[OpenEnd],
    rules: &Rules,
    measure: &Measure,
) -> Value {
    // The ends and the chords as the device reads them from the tiles.
    let (mut ends, mut at) = (Vec::new(), Vec::new());
    let mut chords = Vec::new();
    let mut refused = 0;
    for (i, (kind, tags, part)) in features.iter().enumerate() {
        let held = |k: usize| frame_point(std::slice::from_ref(part), 0, k).map(|v| v as f64 / FRAME);
        let z = |k: usize| part.z.as_ref().map_or(0.0, |z| height_code(z[k]) as f64 / 100.0);
        match *kind {
            "tunnel-end" => match End::from_tags(held(0), z(0), tags, rules) {
                Ok(e) => {
                    ends.push(e);
                    at.push(i);
                }
                Err(_) => refused += 1,
            },
            "tunnel-tube" => {
                let (a, b) = (held(0), held(1));
                match Chord::from_tags([a[0], a[1], z(0)], [b[0], b[1], z(1)], tags, rules) {
                    Ok(c) => chords.push(c),
                    Err(_) => refused += 1,
                }
            }
            _ => {}
        }
    }
    let boxes = end_boxes(&ends, &chords, rules);
    let grids: Vec<(usize, Grid)> =
        (0..ends.len()).filter(|&e| !boxes[e].is_empty()).map(|e| (e, Grid::new(&ends[e], &boxes[e]))).collect();
    let asks: Vec<Ask> = (grids.iter())
        .map(|(e, g)| Ask {
            p: world01(ends[*e].at),
            points: g.reads.iter().map(|&(_, p)| world01(p)).collect(),
            ..Ask::default()
        })
        .collect();
    let (sites, cells) = if asks.is_empty() { (Vec::new(), 0) } else { measure(&asks, opened) };
    let (mut cut, mut rings_n, mut points_n, mut unknown, mut walls, mut invalid) = (0, 0, 0, 0, 0, 0);
    for ((e, mut g), site) in grids.into_iter().zip(sites) {
        for (&(k, p), h) in g.reads.iter().zip(&site.points) {
            let Some(h) = h else {
                unknown += 1;
                g.value[k] = g.value[k].max(STEP_M);
                continue;
            };
            let q = [p[0], p[1], *h];
            let best = boxes[e].iter().min_by(|a, b| outside(a, q).total_cmp(&outside(b, q))).unwrap();
            (g.value[k], g.wall[k]) = (outside(best, q), -outside_wall(best, q));
        }
        let (rings, dropped) = g.rings();
        walls += dropped;
        invalid += usize::from(!simple(&rings));
        if rings.is_empty() {
            continue;
        }
        cut += 1;
        rings_n += rings.len();
        points_n += rings.iter().map(Vec::len).sum::<usize>();
        let cm = |v: i64| num(v as f64 / 100.0);
        let text: Vec<String> = (rings.iter())
            .map(|r| r.iter().map(|p| format!("{} {}", cm(p[0]), cm(p[1]))).collect::<Vec<_>>().join(","))
            .collect();
        features[at[e]].1.insert("cut".into(), text.join(";"));
    }
    json!({ "cut_ends": cut, "cut_rings": rings_n, "cut_points": points_n, "cut_cells": cells,
            "cut_unknown_ground": unknown, "cut_records_refused": refused, "cut_rings_in_the_wall": walls,
            "cut_invalid_rings": invalid })
}

/// A world point in [0, 1], as the measure reads it.
fn world01(p: [f64; 2]) -> [f64; 2] {
    [p[0] / WORLD, p[1] / WORLD]
}

/// The grid of an end in the frame of its opening, `STEP_CM` apart over its boxes and `NEAR_M` round them: per point
/// how far its ground lies outside the boxes (negative inside) and how deep it lies inside the sides and the roof of
/// the box that holds it best; the points whose ground it reads.
struct Grid {
    /// The first grid point, in centimetres in the frame of the opening.
    origin: [i64; 2],
    n: [usize; 2],
    value: Vec<f64>,
    wall: Vec<f64>,
    reads: Vec<(usize, [f64; 2])>,
    /// The end's point, unit way in and its left, and metres per world unit.
    at: [f64; 2],
    into: [f64; 2],
    left: [f64; 2],
    m: f64,
}

impl Grid {
    fn new(end: &End, boxes: &[[f64; BOX]]) -> Grid {
        let (into, m) = (end.way_in(), metres_per_unit(end.at[1]));
        let left = [into[1], -into[0]];
        let frame = |p: [f64; 2]| {
            let d = [(p[0] - end.at[0]) * m, (p[1] - end.at[1]) * m];
            [d[0] * into[0] + d[1] * into[1], d[0] * left[0] + d[1] * left[1]]
        };
        // The corners of each box in plan, in the frame of the opening.
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for q in boxes.iter().flat_map(corners).map(frame) {
            for k in 0..2 {
                lo[k] = lo[k].min(q[k]);
                hi[k] = hi[k].max(q[k]);
            }
        }
        let origin = lo.map(|l| ((l - NEAR_M) / STEP_M).floor() as i64 * STEP_CM);
        let n = [0, 1].map(|k| ((hi[k] + NEAR_M - origin[k] as f64 / 100.0) / STEP_M).ceil() as usize + 1);
        let cells = n[0] * n[1];
        let (value, wall) = (vec![0.0; cells], vec![0.0; cells]);
        let mut g = Grid { origin, n, value, wall, reads: Vec::new(), at: end.at, into, left, m };
        for j in 0..n[1] {
            for i in 0..n[0] {
                let p = g.world(i, j);
                // In plan alone: a lower bound of `outside`.
                let plan = boxes.iter().map(|b| outside_plan(b, p)).fold(f64::INFINITY, f64::min);
                g.value[j * n[0] + i] = plan;
                if plan <= NEAR_M {
                    g.reads.push((j * n[0] + i, p));
                }
            }
        }
        g
    }

    /// Grid point (i, j) in centimetres in the frame of the opening.
    fn place(&self, i: usize, j: usize) -> [i64; 2] {
        [self.origin[0] + i as i64 * STEP_CM, self.origin[1] + j as i64 * STEP_CM]
    }

    /// The world point of grid point (i, j).
    fn world(&self, i: usize, j: usize) -> [f64; 2] {
        let [u, v] = self.place(i, j).map(|c| c as f64 / 100.0);
        let d = [self.into[0] * u + self.left[0] * v, self.into[1] * u + self.left[1] * v];
        [self.at[0] + d[0] / self.m, self.at[1] + d[1] / self.m]
    }

    /// The rings of the cut, and the count of rings that go because all their ground lies in the wall. Marching
    /// squares where the value is under −`EPS_M`, the cut on the left of each ring's way; a cell whose diagonal corners
    /// alone are inside joins them where the mean of its corners is inside. A crossing lies at the share of the values
    /// on its edge, in whole centimetres from 1 to `STEP_CM` − 1.
    fn rings(&self) -> (Vec<Ring>, usize) {
        let [nu, nv] = self.n;
        let v = |i: usize, j: usize| self.value[j * nu + i] + EPS_M;
        // An edge of the grid: from (i, j) along u (0) or along v (1).
        let point = |(i, j, axis): (usize, usize, usize)| {
            let (a, b) = if axis == 0 { (v(i, j), v(i + 1, j)) } else { (v(i, j), v(i, j + 1)) };
            let t = ((a / (a - b) * STEP_CM as f64).round() as i64).clamp(1, STEP_CM - 1);
            let base = self.place(i, j);
            if axis == 0 {
                [base[0] + t, base[1]]
            } else {
                [base[0], base[1] + t]
            }
        };
        let mut next: HashMap<(usize, usize, usize), (usize, usize, usize)> = HashMap::new();
        for j in 0..nv - 1 {
            for i in 0..nu - 1 {
                // The corners and the edges counter-clockwise from (i, j), u east and v north.
                let corners = [v(i, j), v(i + 1, j), v(i + 1, j + 1), v(i, j + 1)];
                let edges = [(i, j, 0), (i + 1, j, 1), (i, j + 1, 0), (i, j, 1)];
                let inside = corners.map(|c| c < 0.0);
                // Each crossing: an exit where the way round leaves the inside, else an entry.
                let crossings: Vec<(usize, bool)> =
                    (0..4).filter(|&k| inside[k] != inside[(k + 1) % 4]).map(|k| (k, inside[k])).collect();
                let joined = crossings.len() == 4 && corners.iter().sum::<f64>() < 0.0;
                for (x, &(k, exit)) in crossings.iter().enumerate() {
                    if !exit {
                        continue;
                    }
                    // An exit goes to the entry next round, counter-clockwise where the inside joins, else clockwise.
                    let m = crossings.len();
                    let to = if joined { crossings[(x + 1) % m] } else { crossings[(x + m - 1) % m] };
                    next.insert(edges[k], edges[to.0]);
                }
            }
        }
        let mut rings = Vec::new();
        while let Some(&start) = next.keys().min() {
            let mut ring = Vec::new();
            let mut at = start;
            while let Some(to) = next.remove(&at) {
                ring.push(point(at));
                at = to;
            }
            rings.push(ring);
        }
        // An outer ring turns left (the cut on its left), a hole right. An outer ring goes, with its holes, where no
        // ground inside it lies deeper than `WALL_M` in the sides and the roof of the boxes.
        // A ring's parent is the smallest ring of the other turn that holds it; a hole goes with its parent.
        let outer: Vec<bool> = rings.iter().map(|r| area2(r) > 0).collect();
        let parent = |r: usize| {
            (0..rings.len())
                .filter(|&o| outer[o] != outer[r] && holds(&rings[o], rings[r][0]))
                .min_by_key(|&o| area2(&rings[o]).abs())
        };
        let mut keep = vec![true; rings.len()];
        let mut dropped = 0;
        for o in (0..rings.len()).filter(|&o| outer[o]) {
            // The ground of the outer ring itself: in it, and in none of its holes.
            let holes: Vec<usize> = (0..rings.len()).filter(|&h| !outer[h] && parent(h) == Some(o)).collect();
            let deepest = (0..nv)
                .flat_map(|j| (0..nu).map(move |i| (i, j)))
                .filter(|&(i, j)| {
                    let q = self.place(i, j);
                    v(i, j) < 0.0 && holds(&rings[o], q) && !holes.iter().any(|&h| holds(&rings[h], q))
                })
                .map(|(i, j)| self.wall[j * nu + i])
                .fold(f64::NEG_INFINITY, f64::max);
            if deepest < WALL_M {
                keep[o] = false;
                dropped += 1;
                holes.iter().for_each(|&h| keep[h] = false);
            }
        }
        let mut rings: Vec<Ring> = rings.into_iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r).collect();
        thin(&mut rings);
        (rings, dropped)
    }
}

/// Twice the signed area of a ring, positive where it turns left.
fn area2(r: &Ring) -> i128 {
    (0..r.len()).map(|k| cross(r[k], r[(k + 1) % r.len()])).sum()
}

fn cross(a: [i64; 2], b: [i64; 2]) -> i128 {
    i128::from(a[0]) * i128::from(b[1]) - i128::from(a[1]) * i128::from(b[0])
}

/// The side of c of the line from a to b: positive left, 0 on it.
fn turn(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> i128 {
    cross([b[0] - a[0], b[1] - a[1]], [c[0] - a[0], c[1] - a[1]])
}

/// Whether c lies in the box of the segment from a to b.
fn within(a: [i64; 2], b: [i64; 2], c: [i64; 2]) -> bool {
    (a[0].min(b[0])..=a[0].max(b[0])).contains(&c[0]) && (a[1].min(b[1])..=a[1].max(b[1])).contains(&c[1])
}

/// Whether a ring holds a point, even-odd; a point on an edge counts as held.
fn holds(r: &Ring, p: [i64; 2]) -> bool {
    let mut odd = false;
    for k in 0..r.len() {
        let (a, b) = (r[k], r[(k + 1) % r.len()]);
        if turn(a, b, p) == 0 && within(a, b, p) {
            return true;
        }
        if (a[1] > p[1]) != (b[1] > p[1]) && (turn(a, b, p) > 0) == (b[1] > a[1]) {
            odd = !odd;
        }
    }
    odd
}

/// Whether two closed segments meet.
fn meet(a: [i64; 2], b: [i64; 2], c: [i64; 2], d: [i64; 2]) -> bool {
    let (d1, d2, d3, d4) = (turn(c, d, a), turn(c, d, b), turn(a, b, c), turn(a, b, d));
    (d1.signum() * d2.signum() < 0 && d3.signum() * d4.signum() < 0)
        || (d1 == 0 && within(c, d, a))
        || (d2 == 0 && within(c, d, b))
        || (d3 == 0 && within(a, b, c))
        || (d4 == 0 && within(a, b, d))
}

/// Whether the way p → q → o turns back along one line at q.
fn folds(p: [i64; 2], q: [i64; 2], o: [i64; 2]) -> bool {
    turn(p, q, o) == 0 && (p[0] - q[0]) * (o[0] - q[0]) + (p[1] - q[1]) * (o[1] - q[1]) > 0
}

/// The edges of rings: per edge its ring, its first point's place, and its two ends.
fn edges(rings: &[Ring]) -> Vec<(usize, usize, [i64; 2], [i64; 2])> {
    (rings.iter().enumerate())
        .flat_map(|(r, ring)| (0..ring.len()).map(move |k| (r, k, ring[k], ring[(k + 1) % ring.len()])))
        .collect()
}

/// Whether rings are simple and apart: each has at least 3 points and an area, two edges of one ring that follow each
/// other meet only at their shared point, and no other two edges meet.
fn simple(rings: &[Ring]) -> bool {
    if rings.iter().any(|r| r.len() < 3 || area2(r) == 0) {
        return false;
    }
    let all = edges(rings);
    all.iter().enumerate().all(|(x, &(r, k, a, b))| {
        all[x + 1..].iter().all(|&(s, l, c, d)| {
            let n = rings[r].len();
            if r == s && l == (k + 1) % n {
                !folds(a, b, d)
            } else if r == s && k == (l + 1) % n {
                !folds(c, d, b)
            } else {
                !meet(a, b, c, d)
            }
        })
    })
}

/// Leaves out of the rings the points within `KEEP_CM` of the line of their neighbours, one at a time, the nearest
/// first, while each removal keeps the rings simple and apart (`removable`).
fn thin(rings: &mut [Ring]) {
    loop {
        let mut best: Option<(f64, usize, usize)> = None;
        for (r, ring) in rings.iter().enumerate() {
            let n = ring.len();
            for k in 0..n {
                let (a, p, b) = (ring[(k + n - 1) % n], ring[k], ring[(k + 1) % n]);
                let l = ((b[0] - a[0]) as f64).hypot((b[1] - a[1]) as f64);
                let off = if l > 0.0 { turn(a, b, p).abs() as f64 / l } else { f64::INFINITY };
                if off < KEEP_CM && best.is_none_or(|x| off < x.0) && removable(rings, r, k) {
                    best = Some((off, r, k));
                }
            }
        }
        let Some((_, r, k)) = best else { return };
        rings[r].remove(k);
    }
}

/// Whether point k of ring r can go: the ring keeps more than 3 points and its turn, the triangle that goes holds no
/// other point, and the new edge from its neighbour before to its neighbour after meets no other edge but the two
/// beside it, at its ends, and folds onto neither.
fn removable(rings: &[Ring], r: usize, k: usize) -> bool {
    let ring = &rings[r];
    let n = ring.len();
    let (i, j) = ((k + n - 1) % n, (k + 1) % n);
    let (a, p, b) = (ring[i], ring[k], ring[j]);
    let mut kept = ring.clone();
    kept.remove(k);
    if n <= 3 || area2(&kept) == 0 || (area2(&kept) > 0) != (area2(ring) > 0) {
        return false;
    }
    let triangle = vec![a, p, b];
    let others = (rings.iter().enumerate())
        .flat_map(|(s, o)| o.iter().enumerate().map(move |(l, &q)| (s, l, q)))
        .filter(|&(s, l, _)| !(s == r && [i, k, j].contains(&l)));
    if area2(&triangle) != 0 && others.into_iter().any(|(_, _, q)| holds(&triangle, q)) {
        return false;
    }
    let before = (i + n - 1) % n;
    edges(rings).into_iter().all(|(s, l, c, d)| match (s == r, l) {
        (true, l) if l == i || l == k => true,
        (true, l) if l == before => !folds(c, a, b),
        (true, l) if l == j => !folds(a, b, d),
        _ => !meet(a, b, c, d),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of `n` points along u and v with a value from the centimetres of each, its first point at the origin.
    fn grid(n: [usize; 2], value: impl Fn(i64, i64) -> f64) -> Grid {
        let (mut values, mut wall) = (Vec::new(), Vec::new());
        for j in 0..n[1] {
            for i in 0..n[0] {
                values.push(value(i as i64 * STEP_CM, j as i64 * STEP_CM));
                wall.push(1.0);
            }
        }
        let (at, into, left, m) = ([0.0, 0.0], [1.0, 0.0], [0.0, -1.0], 1.0);
        Grid { origin: [0, 0], n, value: values, wall, reads: Vec::new(), at, into, left, m }
    }

    #[test]
    fn the_rings_are_simple_and_apart_where_a_box_face_lies_on_a_grid_line() {
        // A box face on the grid line u = 100 cm, its value about 0 there and of each sign by turns, as the rounding of
        // two boxes that meet there gives; and a second region that meets the first at a corner of a cell.
        let face = |u: i64, v: i64| {
            if u == 100 {
                if (u / STEP_CM + v / STEP_CM) % 2 == 0 {
                    1e-16
                } else {
                    -1e-16
                }
            } else if ((100..=300).contains(&u) && (50..=300).contains(&v))
                || ((350..=450).contains(&u) && (350..=450).contains(&v))
            {
                -0.3
            } else {
                0.4
            }
        };
        let (rings, dropped) = grid([12, 12], face).rings();
        assert!(simple(&rings), "{rings:?}");
        assert_eq!((rings.len(), dropped), (2, 0));
        // No point of a ring is a grid point.
        assert!(rings.iter().flatten().all(|p| p[0] % STEP_CM != 0 || p[1] % STEP_CM != 0), "{rings:?}");
        // Ground that lies in the wall alone goes.
        let mut g = grid([12, 12], face);
        g.wall.iter_mut().for_each(|w| *w = WALL_M / 2.0);
        assert_eq!(g.rings(), (Vec::new(), 2));
    }

    #[test]
    fn a_ring_in_the_wall_goes_with_its_holes_but_not_with_a_deeper_ring_in_them() {
        // Ground in the wall round a hole, and in the hole ground deep in the room.
        let ring = |u: i64, v: i64, lo: i64, hi: i64| (lo..=hi).contains(&u) && (lo..=hi).contains(&v);
        let mut g = grid([14, 14], |u, v| {
            if ring(u, v, 250, 400) || (ring(u, v, 100, 550) && !ring(u, v, 200, 450)) {
                -0.3
            } else {
                0.4
            }
        });
        for j in 0..14 {
            for i in 0..14 {
                g.wall[j * 14 + i] =
                    if ring(i as i64 * STEP_CM, j as i64 * STEP_CM, 250, 400) { 1.0 } else { WALL_M / 2.0 };
            }
        }
        let (rings, dropped) = g.rings();
        assert_eq!((rings.len(), dropped), (1, 1), "{rings:?}");
        assert!(simple(&rings) && area2(&rings[0]) > 0 && holds(&rings[0], [300, 300]));
    }
}
