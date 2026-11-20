//! The cut of each built end: where the ground of the ground plan, with the stretches to the openings of the built
//! chains on the ground, lies inside a box of its part. The device's ground plan leaves a hole in its mesh there.

use std::collections::HashMap;

use obc_ground_plan::graph::OpenEnd;
use obc_trailview_parts::{end_boxes, metres_per_unit, outside, Chord, End, Rules, BOX};
use serde_json::{json, Value};

use super::ladder::Measure;
use super::measure::Ask;
use super::num;
use crate::{Part, Tags};

/// The step of the grid of ground points over the boxes (m).
const STEP_M: f64 = 0.5;
/// The grid reads the ground where a point lies this near a box in plan (m); farther, it is outside every box.
const NEAR_M: f64 = 0.75;
/// A ring of the cut keeps a point that lies farther than this from the line of its neighbours (m).
const KEEP_M: f64 = 0.02;
/// World units per unit of [0, 1].
const WORLD: f64 = (1u64 << 30) as f64;
/// The tiles round a point of a part to whole units of zoom 13, and a height to centimetres.
const TILE_UNITS: f64 = 8192.0 * 16384.0;

/// A point of a part as the tiles hold it, in world units.
fn held(p: [f64; 2]) -> [f64; 2] {
    p.map(|c| (c * TILE_UNITS).round() / TILE_UNITS * WORLD)
}

fn cm(z: f64) -> f64 {
    (z * 100.0).round() / 100.0
}

/// A ring of the cut: points in metres in along the opening's heading and to the left of it.
type Ring = Vec<[f64; 2]>;

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
        let z = |k: usize| part.z.as_ref().map_or(0.0, |z| cm(z[k]));
        match *kind {
            "tunnel-end" => match End::from_tags(held(part.points[0]), z(0), tags, rules) {
                Ok(e) => {
                    ends.push(e);
                    at.push(i);
                }
                Err(_) => refused += 1,
            },
            "tunnel-tube" => {
                let (a, b) = (held(part.points[0]), held(part.points[1]));
                match Chord::from_tags([a[0], a[1], z(0)], [b[0], b[1], z(1)], tags, rules) {
                    Ok(c) => chords.push(c),
                    Err(_) => refused += 1,
                }
            }
            _ => {}
        }
    }
    let boxes = end_boxes(&ends, &chords, rules);
    // Per end with boxes, its grid: the points whose ground it reads, and the least distance in plan to a box of the
    // others.
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
    let (mut cut, mut rings_n, mut points_n, mut unknown) = (0, 0, 0, 0);
    for ((e, mut g), site) in grids.into_iter().zip(sites) {
        for (&(k, p), h) in g.reads.iter().zip(&site.points) {
            g.value[k] = match h {
                Some(h) => boxes[e].iter().map(|b| outside(b, [p[0], p[1], *h])).fold(f64::INFINITY, f64::min),
                None => {
                    unknown += 1;
                    g.value[k].max(STEP_M)
                }
            };
        }
        let rings = g.rings();
        if rings.is_empty() {
            continue;
        }
        cut += 1;
        rings_n += rings.len();
        points_n += rings.iter().map(Vec::len).sum::<usize>();
        let text: Vec<String> = (rings.iter())
            .map(|r| r.iter().map(|p| format!("{} {}", num(p[0]), num(p[1]))).collect::<Vec<_>>().join(","))
            .collect();
        features[at[e]].1.insert("cut".into(), text.join(";"));
    }
    json!({ "cut_ends": cut, "cut_rings": rings_n, "cut_points": points_n, "cut_cells": cells,
            "cut_unknown_ground": unknown, "cut_records_refused": refused })
}

/// A world point in [0, 1], as the measure reads it.
fn world01(p: [f64; 2]) -> [f64; 2] {
    [p[0] / WORLD, p[1] / WORLD]
}

/// The grid of an end in the frame of its opening, `STEP_M` apart over its boxes and `NEAR_M` round them: per point
/// its value, how far its ground lies outside the boxes (negative inside); the points whose ground it reads.
struct Grid {
    origin: [f64; 2],
    n: [usize; 2],
    value: Vec<f64>,
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
        for b in boxes {
            let bm = metres_per_unit(b[1]);
            let (u, lu) = ([b[2], b[3]], [b[3], -b[2]]);
            for c in [-b[5], b[5]] {
                for along in [b[10] * c, b[4] + b[11] * c] {
                    let q = frame([b[0] + (u[0] * along + lu[0] * c) / bm, b[1] + (u[1] * along + lu[1] * c) / bm]);
                    for k in 0..2 {
                        lo[k] = lo[k].min(q[k]);
                        hi[k] = hi[k].max(q[k]);
                    }
                }
            }
        }
        let origin = lo.map(|l| ((l - NEAR_M) / STEP_M).floor() * STEP_M);
        let n = [0, 1].map(|k| ((hi[k] + NEAR_M - origin[k]) / STEP_M).ceil() as usize + 1);
        let mut g = Grid { origin, n, value: vec![0.0; n[0] * n[1]], reads: Vec::new(), at: end.at, into, left, m };
        for j in 0..n[1] {
            for i in 0..n[0] {
                let p = g.world(i as f64, j as f64);
                // In plan alone: the least of the plan terms of `outside`, at a height inside every box.
                let plan = boxes.iter().map(|b| plan_outside(b, p)).fold(f64::INFINITY, f64::min);
                g.value[j * n[0] + i] = plan;
                if plan <= NEAR_M {
                    g.reads.push((j * n[0] + i, p));
                }
            }
        }
        g
    }

    /// The world point of grid place (i, j), which may lie between grid points.
    fn world(&self, i: f64, j: f64) -> [f64; 2] {
        let (u, v) = (self.origin[0] + i * STEP_M, self.origin[1] + j * STEP_M);
        let d = [self.into[0] * u + self.left[0] * v, self.into[1] * u + self.left[1] * v];
        [self.at[0] + d[0] / self.m, self.at[1] + d[1] / self.m]
    }

    /// The rings where the value is under 0, by marching squares with the crossings between the grid points at the
    /// share of their values; the cut on the left of each ring's way. A cell whose diagonal corners alone are inside
    /// joins them where the mean of its corners is inside.
    fn rings(&self) -> Vec<Ring> {
        let [nu, nv] = self.n;
        let v = |i: usize, j: usize| self.value[j * nu + i];
        // An edge of the grid: from (i, j) along u (0) or along v (1).
        let point = |(i, j, axis): (usize, usize, usize)| {
            let (a, b) = if axis == 0 { (v(i, j), v(i + 1, j)) } else { (v(i, j), v(i, j + 1)) };
            let t = a / (a - b);
            let (u, w) = if axis == 0 { (i as f64 + t, j as f64) } else { (i as f64, j as f64 + t) };
            [self.origin[0] + u * STEP_M, self.origin[1] + w * STEP_M]
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
                if crossings.is_empty() {
                    continue;
                }
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
        let mut out = Vec::new();
        while let Some(&start) = next.keys().min() {
            let mut ring = Vec::new();
            let mut at = start;
            while let Some(to) = next.remove(&at) {
                ring.push(point(at));
                at = to;
            }
            out.push(thin(ring));
        }
        out.retain(|r: &Ring| r.len() >= 3);
        out
    }
}

/// The plan terms of `outside`: how far a world point lies outside a box in plan.
fn plan_outside(b: &[f64; BOX], p: [f64; 2]) -> f64 {
    let m = metres_per_unit(b[1]);
    let d = [(p[0] - b[0]) * m, (p[1] - b[1]) * m];
    let (along, left) = (d[0] * b[2] + d[1] * b[3], d[0] * b[3] - d[1] * b[2]);
    (b[10] * left - along).max(along - b[4] - b[11] * left).max(left.abs() - b[5])
}

/// A ring with the points that keep it within `KEEP_M` of the ring of all its points (Douglas-Peucker on its two halves
/// from its first point and the point farthest from it).
fn thin(ring: Ring) -> Ring {
    fn keep(ring: &Ring, a: usize, b: usize, out: &mut Vec<usize>) {
        let (p, q) = (ring[a], ring[b % ring.len()]);
        let d = [q[0] - p[0], q[1] - p[1]];
        let l = f64::hypot(d[0], d[1]);
        let off = |k: usize| {
            let r = ring[k];
            if l > 0.0 {
                ((r[0] - p[0]) * d[1] - (r[1] - p[1]) * d[0]).abs() / l
            } else {
                f64::hypot(r[0] - p[0], r[1] - p[1])
            }
        };
        if let Some(k) = (a + 1..b).max_by(|&i, &j| off(i).total_cmp(&off(j))).filter(|&k| off(k) > KEEP_M) {
            keep(ring, a, k, out);
            out.push(k);
            keep(ring, k, b, out);
        }
    }
    if ring.len() < 4 {
        return ring;
    }
    let far = (1..ring.len())
        .max_by(|&i, &j| {
            let d = |k: usize| f64::hypot(ring[k][0] - ring[0][0], ring[k][1] - ring[0][1]);
            d(i).total_cmp(&d(j))
        })
        .unwrap();
    let mut kept = vec![0];
    keep(&ring, 0, far, &mut kept);
    kept.push(far);
    keep(&ring, far, ring.len(), &mut kept);
    kept.iter().map(|&k| ring[k]).collect()
}
