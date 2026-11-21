//! The checks of a cell's mesh: valid topology, labels as the records give them, and two cells alike
//! on their shared line.

use std::collections::BTreeMap;

use super::params::GRID;
use super::region::grid_of;
use super::{CellMesh, NO_LABEL};
use crate::faces::heights::height_at;
use crate::faces::records::{find, label};
use crate::faces::HeightRule;
use crate::frame::{CellKey, GlobalPoint, CELL_UNITS};
use crate::terrain::Terrain;

fn area(p: [[i32; 2]; 3]) -> i64 {
    let d = |a: [i32; 2], b: [i32; 2]| [i64::from(b[0] - a[0]), i64::from(b[1] - a[1])];
    let (u, v) = (d(p[0], p[1]), d(p[0], p[2]));
    u[0] * v[1] - u[1] * v[0]
}

/// The faults of a mesh's topology: a triangle that does not turn as `gridIndices` or has no area, a
/// directed edge used twice, an edge with one triangle off the cell's lines that is not an edge of
/// the terrain grid between two terrain vertices (on ∂R, or the diagonal of a terrain ear).
pub fn validity(mesh: &CellMesh) -> Vec<String> {
    let mut faults = Vec::new();
    let mut edges: BTreeMap<([i32; 2], [i32; 2]), u32> = BTreeMap::new();
    for (k, t) in mesh.triangles.iter().enumerate() {
        let p = t.map(|c| mesh.corner(c as usize).0);
        if area(p) >= 0 {
            faults.push(format!("triangle {k} at {p:?} turns the wrong way or has no area"));
        }
        for i in 0..3 {
            *edges.entry((p[i], p[(i + 1) % 3])).or_default() += 1;
        }
    }
    let c = CELL_UNITS as i32;
    let on_line = |a: [i32; 2], b: [i32; 2]| (0..2).any(|i| a[i] == b[i] && (a[i] == 0 || a[i] == c));
    // ∂R: the terrain vertices.
    let mut boundary: Vec<[i32; 2]> =
        (0..mesh.corners.len()).map(|k| mesh.corner(k)).filter(|c| c.2).map(|c| c.0).collect();
    boundary.sort_unstable();
    let on_boundary = |p: &[i32; 2]| boundary.binary_search(p).is_ok();
    let grid_edge = |a: [i32; 2], b: [i32; 2]| {
        let d = [b[0] - a[0], b[1] - a[1]];
        d[0].abs() + d[1].abs() == GRID || (d[0].abs() == GRID && d[0] == -d[1])
    };
    for (&(a, b), &n) in &edges {
        if n > 1 {
            faults.push(format!("edge {a:?}-{b:?} used {n} times"));
        }
        if !edges.contains_key(&(b, a)) && !on_line(a, b) && !(on_boundary(&a) && on_boundary(&b) && grid_edge(a, b)) {
            faults.push(format!("edge {a:?}-{b:?} has one triangle"));
        }
    }
    faults
}

/// The records and the face plan may differ this near a face edge: the slice's snap (GP3R's own
/// check allows 8 units).
const EDGE_UNITS: f64 = 8.0;

/// What `labels` counts: triangles whose label is not the records' at their centroid (sharp ones
/// with the face's label, ground ones with a ground label), where the centroid lies farther than
/// `EDGE_UNITS` from the triangle's sides; ground corners at grid points with no other corner (off
/// every face edge between two labels) whose label is not the records' there (`ground_wrong`), or
/// another ground label (`ground_other`: a point on an edge between two ground faces, whose heights
/// agree). `sharp_wrong` holds GP3R's known misses, where the records decide a patch apart from the
/// face plan (a crossing over a rail bed): the mesh takes the face plan's label.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Labels {
    pub sharp_wrong: u64,
    pub ground_wrong: u64,
    pub ground_other: u64,
    /// The first places of the wrong ones.
    pub first: Vec<String>,
}

fn is_ground(words: &[u32], l: u32) -> bool {
    l == NO_LABEL || label(words, l)[0] >> 24 <= HeightRule::Slope as u32
}

/// The labels of a mesh against `records::find` in the block it was built with.
pub fn labels(mesh: &CellMesh, words: &[u32]) -> Labels {
    let mut out = Labels::default();
    let mut points: Vec<[i32; 2]> = (0..mesh.corners.len()).map(|k| mesh.corner(k).0).collect();
    points.sort_unstable();
    let alone = |p: [i32; 2]| points.partition_point(|q| *q <= p) - points.partition_point(|q| *q < p) == 1;
    let found = |q: [f64; 2]| find(words, q).unwrap_or(NO_LABEL);
    let wrong = |out: &mut Labels, what: &str, q: [f64; 2], l: u32, f: u32| {
        if out.first.len() < 8 {
            out.first.push(format!("{what} at {q:?}: {l}, the records {f}"));
        }
    };
    for t in &mesh.triangles {
        let c = t.map(|k| mesh.corner(k as usize));
        let p = c.map(|c| [f64::from(c.0[0]), f64::from(c.0[1])]);
        let mid = [(p[0][0] + p[1][0] + p[2][0]) / 3.0, (p[0][1] + p[1][1] + p[2][1]) / 3.0];
        let inner = (0..3)
            .map(|i| {
                let (a, b) = (p[i], p[(i + 1) % 3]);
                let e = [b[0] - a[0], b[1] - a[1]];
                ((mid[0] - a[0]) * e[1] - (mid[1] - a[1]) * e[0]).abs() / libm::hypot(e[0], e[1])
            })
            .fold(f64::INFINITY, f64::min);
        let sharp = c.iter().all(|c| !c.2 && !is_ground(words, c.1));
        if inner > EDGE_UNITS {
            let f = found(mid);
            let bad = if sharp { c[0].1 != f } else { !is_ground(words, f) };
            if bad {
                *(if sharp { &mut out.sharp_wrong } else { &mut out.ground_wrong }) += 1;
                wrong(&mut out, if sharp { "sharp triangle" } else { "ground triangle" }, mid, c[0].1, f);
            }
        }
        if sharp {
            continue;
        }
        for (i, c) in c.iter().enumerate() {
            if c.2 || grid_of(c.0).is_none() || !alone(c.0) {
                continue;
            }
            let f = found(p[i]);
            if f != c.1 && is_ground(words, f) {
                out.ground_other += 1;
            } else if f != c.1 {
                out.ground_wrong += 1;
                wrong(&mut out, "ground corner", p[i], c.1, f);
            }
        }
    }
    out
}

/// A cell's mesh with the block and terrain that it reads, for `border`.
pub struct Cell<'a> {
    pub key: CellKey,
    pub mesh: &'a CellMesh,
    pub words: &'a [u32],
    pub terrain: &'a Terrain,
}

/// The distinct heights of a cell's corners on one of its lines, by place along it, as bits; a
/// terrain vertex as `None`.
fn on_line(cell: &Cell, vertical: bool, at: i32) -> BTreeMap<i32, Vec<Option<u64>>> {
    let mut out: BTreeMap<i32, Vec<Option<u64>>> = BTreeMap::new();
    let o = cell.key.origin();
    for k in 0..cell.mesh.corners.len() {
        let (p, l, terrain) = cell.mesh.corner(k);
        let (along, across) = if vertical { (p[1], p[0]) } else { (p[0], p[1]) };
        if across != at {
            continue;
        }
        let h = (!terrain).then(|| {
            let t = cell.terrain.height(GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) });
            let label = (l != NO_LABEL).then_some(l);
            t.map_or(u64::MAX, |t| height_at(cell.words, [f64::from(p[0]), f64::from(p[1])], label, t).h.to_bits())
        });
        out.entry(along).or_default().push(h);
    }
    for v in out.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    out
}

/// The places on the shared line of cell `b`, east (`east`) or south of `a`, where the two cells'
/// corners differ in place, flag or height bits, and the count of places where one cell holds a
/// further corner within `SUB_MM` of a height of both (two labels whose heights GP6 joins). A
/// terrain vertex may stand in one cell alone: the other's cell beside it lies outside R.
pub fn border(a: &Cell, b: &Cell, east: bool) -> (Vec<String>, u32) {
    let (x, y) = (on_line(a, east, CELL_UNITS as i32), on_line(b, east, 0));
    let terrain_only = |v: Option<&Vec<Option<u64>>>| v.is_none_or(|v| v.iter().all(Option::is_none));
    let (mut out, mut sub_mm) = (Vec::new(), 0);
    let mut alongs: Vec<i32> = x.keys().chain(y.keys()).copied().collect();
    alongs.sort_unstable();
    alongs.dedup();
    for along in alongs {
        let (p, q) = (x.get(&along), y.get(&along));
        if p == q || (p.is_none() && terrain_only(q)) || (q.is_none() && terrain_only(p)) {
            continue;
        }
        let (p, q) = (p.cloned().unwrap_or_default(), q.cloned().unwrap_or_default());
        let shared: Vec<f64> = p.iter().filter(|h| q.contains(h)).flatten().map(|&h| f64::from_bits(h)).collect();
        let near = |h: &Option<u64>| h.is_some_and(|h| shared.iter().any(|s| (f64::from_bits(h) - s).abs() <= SUB_MM));
        if !shared.is_empty() && p.iter().chain(&q).filter(|h| !(p.contains(h) && q.contains(h))).all(near) {
            sub_mm += 1;
        } else {
            out.push(format!("{:?}|{:?} at {along}: heights {p:?} and {q:?}", a.key, b.key));
        }
    }
    (out, sub_mm)
}

/// GP6 joins two sides whose heights differ by at most `heights.step_m` (1 mm).
const SUB_MM: f64 = 0.001;

/// Within this many units of a deck end line, a slot's ends lie on it (`faces` rounds the stroke's corners).
const END_UNITS: f64 = 2.0;
/// The ground near a deck end line that `deck_ends` reads: within this many metres of it.
pub const NEAR_END_M: f64 = 3.0;

/// A ground triangle is steeper than `face_max` past this share of it: a face at `face_max` (the ceiling's rise
/// past a deck's stroke) gives a plane a hair steeper.
const STEEP_SHARE: f64 = 1.01;

/// The deck ends of a cell's mesh: the end faces that lie on no deck end line of the records
/// (`heights::check::end_lines`), the slots that lie on one but are not end faces, and the ground triangles
/// within `NEAR_END_M` of an end line steeper than `face_max` where the plain terrain is not, with their
/// steepness.
pub fn deck_ends(
    mesh: &CellMesh,
    words: &[u32],
    terrain: &Terrain,
    cell: CellKey,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    use crate::faces::heights::{check::end_lines, HeightRules};
    let ends = end_lines(words);
    let (mut stray, mut missed, mut steep) = (Vec::new(), Vec::new(), Vec::new());
    if ends.is_empty() && mesh.end_faces == 0 {
        return (stray, missed, steep);
    }
    let at = |c: u32| mesh.corner(c as usize).0.map(f64::from);
    let on = |q: [f64; 2]| {
        ends.iter().any(|e| {
            let [along, across] = e.place(q);
            along.abs() <= END_UNITS && across.abs() <= e.half + END_UNITS
        })
    };
    let first = mesh.steps.len() - mesh.end_faces as usize;
    for (k, s) in mesh.steps.iter().enumerate() {
        match (on(at(s[0])) && on(at(s[1])), k >= first) {
            (false, true) => stray.push(format!("end face {k} at {:?} on no end line", at(s[0]))),
            (true, false) => missed.push(format!("slot {k} at {:?} on an end line", at(s[0]))),
            _ => {}
        }
    }
    let o = cell.origin();
    let p = HeightRules::of(words);
    let rule = |l: u32| if l == NO_LABEL { 0 } else { label(words, l)[0] >> 24 };
    let mpu = crate::frame::metres_per_unit(o.y + CELL_UNITS / 2);
    let near = |q: [f64; 2]| {
        ends.iter().any(|e| {
            let [along, across] = e.place(q);
            libm::hypot(along, (across.abs() - e.half).max(0.0)) * mpu <= NEAR_END_M
        })
    };
    for t in &mesh.triangles {
        let corners = t.map(|c| mesh.corner(c as usize));
        if corners.iter().any(|&(_, l, terrain)| !terrain && rule(l) > 1) {
            continue;
        }
        let q = corners.map(|(p, ..)| p.map(f64::from));
        if !near([(q[0][0] + q[1][0] + q[2][0]) / 3.0, (q[0][1] + q[1][1] + q[2][1]) / 3.0]) {
            continue;
        }
        let plain = |p: [f64; 2]| terrain.height(GlobalPoint { x: o.x + p[0] as i64, y: o.y + p[1] as i64 });
        let Some(t) = q.iter().map(|&p| plain(p)).collect::<Option<Vec<f64>>>() else { continue };
        let h: Vec<f64> = (0..3)
            .map(|k| {
                let (_, l, vertex) = corners[k];
                if vertex {
                    t[k]
                } else {
                    height_at(words, q[k], (l != NO_LABEL).then_some(l), t[k]).h
                }
            })
            .collect();
        // The plane's rise per metre through three points.
        let grade = |z: &[f64]| {
            let (u, v) = ([q[1][0] - q[0][0], q[1][1] - q[0][1]], [q[2][0] - q[0][0], q[2][1] - q[0][1]]);
            let (du, dv) = (z[1] - z[0], z[2] - z[0]);
            let det = u[0] * v[1] - u[1] * v[0];
            libm::hypot(du * v[1] - dv * u[1], dv * u[0] - du * v[0]) / det.abs() / mpu
        };
        let (g, gt) = (grade(&h), grade(&t));
        if g > STEEP_SHARE * p.face_max && gt <= p.face_max {
            steep.push(format!("ground triangle at {:?}: {g:.3} m/m, terrain {gt:.3}, heights {h:.3?}", q[0]));
        }
    }
    (stray, missed, steep)
}
