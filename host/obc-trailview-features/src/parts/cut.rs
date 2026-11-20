//! The cut of each built end: where the ground of the ground plan, with the stretches to the openings of the built
//! chains on the ground, lies inside a box of its part. The device's ground plan leaves a hole in its mesh there.
//!
//! The ground is smooth inside a face of the ground plan and steps only at a face's edge. So the cut is the union, over
//! the faces about the part, of the region of each face where its own ground lies in the boxes: contoured on a grid in
//! the face's own ground, then clipped to the face. Where the ground steps, the cut's edge is the face's edge. A box
//! holds its faces: a point on the plane where two boxes meet lies in both.

use std::collections::BTreeMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{ContourDirection, IntOverlayOptions, Overlay};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;
use obc_ground_plan::graph::OpenEnd;
use obc_trailview_parts::{corners, end_boxes, metres_per_unit, outside, outside_plan, Chord, End, Rules, BOX};
use serde_json::{json, Value};

use super::ladder::Measure;
use super::measure::{Ask, Field};
use crate::tiles::{frame_point, height_code};
use crate::{Part, Tags};

/// The step of the grid of ground points over the boxes (m).
const STEP_M: f64 = 0.5;
/// The grid reads the ground where a point lies this near a box in plan (m); farther, it is outside every box.
const NEAR_M: f64 = 0.75;
/// A point counts as inside the boxes where it lies no farther outside than this (m): a point on a box's face is in it.
const EPS_M: f64 = 1e-6;
/// World units per unit of [0, 1], and frame units of the ground plan per world unit.
const WORLD: f64 = (1u64 << 30) as f64;
const FRAME: f64 = 16.0;

/// A ring of the cut: frame points of the ground plan less the end's point.
type Ring = Vec<[i64; 2]>;

/// Adds the tag `cut` to each `tunnel-end` of a built chain whose ground lies in its part, with the stretches of
/// `opened` on the ground; gives its counts.
pub(crate) fn add(
    features: &mut [(&'static str, Tags, Part)],
    opened: &[OpenEnd],
    rules: &Rules,
    measure: &Measure,
) -> Value {
    // The ends and the chords as the device reads them from the tiles, and each end's frame point.
    let (mut ends, mut at, mut origins) = (Vec::new(), Vec::new(), Vec::new());
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
                    origins.push(frame_point(std::slice::from_ref(part), 0, 0));
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
    let (mut cut, mut rings_n, mut points_n, mut faces_n, mut invalid) = (0, 0, 0, 0, 0);
    for ((e, g), site) in grids.into_iter().zip(sites) {
        let rings = g.cut(&site.fields, &boxes[e], origins[e]);
        faces_n += site.fields.len();
        invalid += usize::from(!simple(&rings));
        if rings.is_empty() {
            continue;
        }
        cut += 1;
        rings_n += rings.len();
        points_n += rings.iter().map(Vec::len).sum::<usize>();
        let text: Vec<String> = (rings.iter())
            .map(|r| r.iter().map(|p| format!("{} {}", p[0], p[1])).collect::<Vec<_>>().join(","))
            .collect();
        features[at[e]].1.insert("cut".into(), text.join(";"));
    }
    json!({ "cut_ends": cut, "cut_rings": rings_n, "cut_points": points_n, "cut_cells": cells,
            "cut_faces": faces_n, "cut_records_refused": refused, "cut_invalid_rings": invalid })
}

/// A world point in [0, 1], as the measure reads it.
fn world01(p: [f64; 2]) -> [f64; 2] {
    [p[0] / WORLD, p[1] / WORLD]
}

/// The grid of an end in the frame of its opening, `STEP_M` apart over its boxes and `NEAR_M` round them, and the points
/// whose ground it reads.
struct Grid {
    /// The first grid point, in metres in the frame of the opening.
    origin: [f64; 2],
    n: [usize; 2],
    /// Per point that it reads, its place in the grid and its world point.
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
        let origin = lo.map(|l| ((l - NEAR_M) / STEP_M).floor() * STEP_M);
        let n = [0, 1].map(|k| ((hi[k] + NEAR_M - origin[k]) / STEP_M).ceil() as usize + 1);
        let mut g = Grid { origin, n, reads: Vec::new(), at: end.at, into, left, m };
        for j in 0..n[1] {
            for i in 0..n[0] {
                let p = g.world(g.place(i, j));
                if boxes.iter().map(|b| outside_plan(b, p)).fold(f64::INFINITY, f64::min) <= NEAR_M {
                    g.reads.push((j * n[0] + i, p));
                }
            }
        }
        g
    }

    /// Grid point (i, j) in metres in the frame of the opening.
    fn place(&self, i: usize, j: usize) -> [f64; 2] {
        [self.origin[0] + i as f64 * STEP_M, self.origin[1] + j as f64 * STEP_M]
    }

    /// The world point of metres u in and v to the left.
    fn world(&self, [u, v]: [f64; 2]) -> [f64; 2] {
        let d = [self.into[0] * u + self.left[0] * v, self.into[1] * u + self.left[1] * v];
        [self.at[0] + d[0] / self.m, self.at[1] + d[1] / self.m]
    }

    /// The rings of the cut, frame points less `origin`: per face, the region where its own ground lies in the boxes,
    /// clipped to the face; then the union of those.
    fn cut(&self, fields: &[Field], boxes: &[[f64; BOX]], origin: [i64; 2]) -> Vec<Ring> {
        let to_frame = |q: [f64; 2]| {
            let w = self.world(q);
            let f = [0, 1].map(|k| ((w[k] * FRAME).round() as i64 - origin[k]) as i32);
            IntPoint::new(f[0], f[1])
        };
        let mut pieces: Vec<Vec<IntPoint>> = Vec::new();
        for field in fields {
            // A point whose ground the face does not read lies outside.
            let mut value = vec![STEP_M; self.n[0] * self.n[1]];
            for &(k, h) in &field.heights {
                let (cell, p) = self.reads[k as usize];
                let q = [p[0], p[1], h];
                value[cell] = boxes.iter().map(|b| outside(b, q)).fold(f64::INFINITY, f64::min);
            }
            let contours = self.contours(&value);
            if contours.is_empty() {
                continue;
            }
            let subject: Vec<Vec<IntPoint>> =
                contours.into_iter().map(|r| r.into_iter().map(to_frame).collect()).collect();
            let face: Vec<Vec<IntPoint>> = (field.rings.iter())
                .map(|r| {
                    r.iter().map(|q| IntPoint::new((q[0] - origin[0]) as i32, (q[1] - origin[1]) as i32)).collect()
                })
                .collect();
            let inside = Overlay::from_subj_and_clip_custom(&subject, &face, options(), solver())
                .overlay(OverlayRule::Intersect, FillRule::EvenOdd);
            pieces.extend(inside.into_iter().flatten());
        }
        if pieces.is_empty() {
            return Vec::new();
        }
        let union = Overlay::from_subj_and_clip_custom(&pieces, &Vec::<Vec<IntPoint>>::new(), options(), solver())
            .overlay(OverlayRule::Subject, FillRule::NonZero);
        union.into_iter().flatten().map(|r| r.into_iter().map(|q| [i64::from(q.x), i64::from(q.y)]).collect()).collect()
    }

    /// The rings where `value` is under `EPS_M`, in metres in the frame of the opening, the region on the left of each:
    /// marching squares, a crossing on each grid edge at the share of its two values; a cell whose diagonal corners
    /// alone are inside joins them where the mean of its corners is inside.
    fn contours(&self, value: &[f64]) -> Vec<Vec<[f64; 2]>> {
        let [nu, nv] = self.n;
        let v = |i: usize, j: usize| value[j * nu + i] - EPS_M;
        // An edge of the grid: from (i, j) along u (0) or along v (1).
        let point = |(i, j, axis): (usize, usize, usize)| {
            let (a, b) = if axis == 0 { (v(i, j), v(i + 1, j)) } else { (v(i, j), v(i, j + 1)) };
            let t = a / (a - b) * STEP_M;
            let base = self.place(i, j);
            if axis == 0 {
                [base[0] + t, base[1]]
            } else {
                [base[0], base[1] + t]
            }
        };
        let mut next = BTreeMap::new();
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
        while let Some((&start, _)) = next.first_key_value() {
            let mut ring = Vec::new();
            let mut at = start;
            while let Some(to) = next.remove(&at) {
                ring.push(point(at));
                at = to;
            }
            rings.push(ring);
        }
        rings
    }
}

fn solver() -> Solver {
    Solver { strategy: Strategy::Frag, precision: Precision::ABSOLUTE, multithreading: None }
}

fn options() -> IntOverlayOptions<u64> {
    IntOverlayOptions {
        preserve_input_collinear: false,
        output_direction: ContourDirection::CounterClockwise,
        preserve_output_collinear: false,
        min_output_area: 0,
        ogc: false,
    }
}

/// Twice the signed area of a ring.
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

/// Whether rings are simple and apart: each has at least 3 points and an area, and no two of their edges cross inside
/// both.
fn simple(rings: &[Ring]) -> bool {
    if rings.iter().any(|r| r.len() < 3 || area2(r) == 0) {
        return false;
    }
    let edges: Vec<([i64; 2], [i64; 2])> =
        rings.iter().flat_map(|r| (0..r.len()).map(move |k| (r[k], r[(k + 1) % r.len()]))).collect();
    edges.iter().enumerate().all(|(x, &(a, b))| {
        edges[x + 1..].iter().all(|&(c, d)| {
            let (d1, d2, d3, d4) = (turn(c, d, a), turn(c, d, b), turn(a, b, c), turn(a, b, d));
            !(d1.signum() * d2.signum() < 0 && d3.signum() * d4.signum() < 0)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid 6 m in and 6 m across from (−1, −3) m, every point read, at 46.8° N, its way in east.
    fn grid() -> Grid {
        let at = [0.523 * WORLD, 0.3597 * WORLD];
        let (into, left, m) = ([1.0, 0.0], [0.0, -1.0], metres_per_unit(at[1]));
        let mut g = Grid { origin: [-1.0, -3.0], n: [13, 13], reads: Vec::new(), at, into, left, m };
        g.reads = (0..13 * 13).map(|k| (k, g.world(g.place(k % 13, k / 13)))).collect();
        g
    }

    /// A flat box from u0 to u1 metres in, 2 m to each side, from 0 m to 3 m up.
    fn flat(g: &Grid, u0: f64, u1: f64) -> [f64; BOX] {
        let s = g.world([u0, 0.0]);
        [s[0], s[1], 1.0, 0.0, u1 - u0, 2.0, 3.0, 0.0, 0.0, 3.0, 0.0, 0.0]
    }

    /// The frame points of a rectangle from u0 to u1 in and v0 to v1 to the left: a face.
    fn face(g: &Grid, [u0, u1, v0, v1]: [f64; 4]) -> Vec<Vec<[i64; 2]>> {
        let f = |u, v| g.world([u, v]).map(|w| (w * FRAME).round() as i64);
        vec![vec![f(u0, v1), f(u0, v0), f(u1, v0), f(u1, v1)]]
    }

    /// A face's field with its ground at height h at every point.
    fn field(g: &Grid, rect: [f64; 4], h: f64) -> Field {
        Field { rings: face(g, rect), heights: (0..g.reads.len() as u32).map(|k| (k, h)).collect() }
    }

    fn square_metres(g: &Grid, r: &Ring) -> f64 {
        let m = g.m / FRAME;
        area2(r).unsigned_abs() as f64 / 2.0 * m * m
    }

    #[test]
    fn the_cut_follows_a_face_edge_where_the_ground_steps() {
        // A road face to 1.23 m in, its ground over the box; a hill face beyond, its ground in the box.
        let g = grid();
        let origin = g.world([0.0, 0.0]).map(|w| (w * FRAME).round() as i64);
        let (road, hill) = ([-1.0, 1.23, -3.0, 3.0], [1.23, 5.0, -3.0, 3.0]);
        let rings = g.cut(&[field(&g, road, 4.0), field(&g, hill, 1.0)], &[flat(&g, 0.0, 4.0)], origin);
        assert!(simple(&rings) && rings.len() == 1, "{rings:?}");
        // The ring's edge at the step is the face's edge: both of its ends lie on the face's own edge line.
        let edge: Vec<[i64; 2]> = face(&g, hill)[0][..2].iter().map(|p| [p[0] - origin[0], p[1] - origin[1]]).collect();
        let on = |p: [i64; 2]| turn(edge[0], edge[1], p) == 0;
        let r = &rings[0];
        assert!((0..r.len()).any(|k| on(r[k]) && on(r[(k + 1) % r.len()])), "{r:?}");
        // 2.77 m in by 4 m across.
        assert!((square_metres(&g, r) - 2.77 * 4.0).abs() < 0.01, "{}", square_metres(&g, r));
    }

    #[test]
    fn two_boxes_that_meet_on_a_grid_line_hold_the_ground_on_it() {
        let g = grid();
        let origin = g.world([0.0, 0.0]).map(|w| (w * FRAME).round() as i64);
        let ground = [field(&g, [-1.0, 5.0, -3.0, 3.0], 1.0)];
        let rings = g.cut(&ground, &[flat(&g, 0.0, 2.0), flat(&g, 2.0, 4.0)], origin);
        assert_eq!(rings.len(), 1, "{rings:?}");
        assert!((square_metres(&g, &rings[0]) - 16.0).abs() < 0.01, "{}", square_metres(&g, &rings[0]));
    }
}
