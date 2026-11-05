//! The constrained Delaunay triangulation of one planar map in one range: the constraints (face edges
//! split to `l_max_m` and at each sign change of their step, and the locked lines), an exact check
//! that they neither cross nor overlap, the grid points of slope and terrain faces, and Ruppert's
//! refinement with concentric shells on spade's triangulation.
//!
//! Every point is an integer in frame units, exact in f64; spade's predicates are exact; a
//! circumcentre comes from integers by + − × ÷ and one rounding. So native and WASM builds give the
//! same points.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};

use spade::handles::{FixedFaceHandle, FixedVertexHandle, InnerTag};
use spade::{ConstrainedDelaunayTriangulation, Point2, PositionInTriangulation, Triangulation};

use super::params::{grid, grid_clearance_m, l_max_m, EQUAL_UNITS, L17, REFINE_BUDGET, SMALL_CORNER_DEG};
use super::region::Region;
use super::{MeshError, Range};
use crate::faces::{HeightRule, Surface, OUTSIDE};
use crate::frame::CELL_UNITS;

type Cdt = ConstrainedDelaunayTriangulation<Point2<f64>>;
/// Bad triangles by smallest angle, then integer centroid (y, x).
type Queue = BinaryHeap<Reverse<(u64, i64, i64, [u32; 3])>>;
pub type P = [i32; 2];

/// A constraint of the input: a face edge with the faces on its left and right (the sides where the
/// shoelace sum of (a, b, point) is positive and negative), or a locked segment with D's face on its
/// left and `OUTSIDE` on its right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub a: P,
    pub b: P,
    pub left: u32,
    pub right: u32,
    pub locked: bool,
}

/// What the triangulation reads of a face.
#[derive(Clone, Copy, Debug)]
pub struct FaceInfo {
    pub surface: Surface,
    pub rule: HeightRule,
}

pub struct Input<'a> {
    pub faces: &'a [FaceInfo],
    pub constraints: Vec<Constraint>,
    pub region: &'a Region,
    /// Metres per frame unit at a row of the cell's frame.
    pub mpu: &'a dyn Fn(f64) -> f64,
    /// Two sides whose heights differ by at most this join (GP6's `step_m`).
    pub join_m: f64,
    /// The least angle that refinement holds: `MIN_ANGLE_DEG`; a measurement may lower it.
    pub min_angle_deg: f64,
}

/// A constraint segment of the result: vertex indices, its faces, whether it is locked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: u32,
    pub b: u32,
    pub left: u32,
    pub right: u32,
    pub locked: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CdtStats {
    pub constraint_points: u32,
    pub grid_points: u32,
    pub steiner: u32,
    pub segment_splits: u32,
    /// Segments that a split would make cross or touch another constraint: they stay.
    pub held_segments: u32,
    /// Bad triangles whose circumcentre lies outside D or in a locked segment's diametral circle.
    pub held_locked: u32,
    pub refine_incomplete: u32,
    /// Skinny triangles at an input corner under 60° that no point can fix (Shewchuk's rule).
    pub small_corner: u32,
}

pub struct Triangles {
    pub points: Vec<P>,
    /// Vertex indices, turning as the terrain's `gridIndices` (negative signed area in x east, y
    /// south), and the face.
    pub tris: Vec<([u32; 3], u32)>,
    /// Per triangle: it has a locked edge or a locked line held its refinement (M2's weaker bound).
    pub locked: Vec<bool>,
    /// Per triangle: a small-corner triangle of Shewchuk's rule.
    pub small: Vec<bool>,
    pub segments: Vec<Segment>,
    pub stats: CdtStats,
}

/// The step (left height − right height, m) of two faces at a point, or None where the terrain is
/// unknown. The triangulation splits a face edge where it changes sign.
pub type Steps<'a> = dyn FnMut(P, u32, u32) -> Option<f64> + 'a;

fn cross(o: P, a: P, b: P) -> i128 {
    (i128::from(a[0]) - i128::from(o[0])) * (i128::from(b[1]) - i128::from(o[1]))
        - (i128::from(a[1]) - i128::from(o[1])) * (i128::from(b[0]) - i128::from(o[0]))
}

fn on_segment(a: P, b: P, p: P) -> bool {
    cross(a, b, p) == 0
        && p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// Whether two closed segments meet anywhere but at a shared end (and not along a shared line).
fn conflict(a: P, b: P, c: P, d: P) -> bool {
    let shared = [a, b].iter().filter(|p| **p == c || **p == d).count();
    if shared == 2 {
        return true;
    }
    if shared == 1 {
        // Only an overlap along the line conflicts.
        let (o, p, q) =
            if a == c || a == d { (a, b, if a == c { d } else { c }) } else { (b, a, if b == c { d } else { c }) };
        return cross(o, p, q) == 0
            && (i128::from(p[0] - o[0]) * i128::from(q[0] - o[0]) + i128::from(p[1] - o[1]) * i128::from(q[1] - o[1]))
                > 0;
    }
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    if ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0)) {
        return true;
    }
    on_segment(c, d, a) || on_segment(c, d, b) || on_segment(a, b, c) || on_segment(a, b, d)
}

fn round_div(num: i128, den: i128) -> i128 {
    let (n, d) = if den < 0 { (-num, -den) } else { (num, den) };
    (2 * n + d).div_euclid(2 * d)
}

/// The point at k/n of the way from a to b, rounded to the frame unit.
fn part(a: P, b: P, k: i128, n: i128) -> P {
    [0, 1].map(|i| round_div(i128::from(a[i]) * (n - k) + i128::from(b[i]) * k, n) as i32)
}

fn len(a: P, b: P) -> f64 {
    libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1]))
}

/// Buckets of segments by the squares that their boxes meet.
const BUCKET: i32 = 2 * L17;

#[derive(Clone, Copy, Debug)]
struct Seg {
    a: u32,
    b: u32,
    left: u32,
    right: u32,
    locked: bool,
    held: bool,
    alive: bool,
    /// The input segment that it is part of.
    input: u32,
}

struct State<'a> {
    cdt: Cdt,
    pts: Vec<P>,
    segs: Vec<Seg>,
    seg_of: BTreeMap<(u32, u32), u32>,
    buckets: BTreeMap<(i32, i32), Vec<u32>>,
    /// The input segments' ends.
    inputs: Vec<[P; 2]>,
    /// Per vertex, the input segments that it lies on.
    on_inputs: Vec<Vec<u32>>,
    face_cache: BTreeMap<[u32; 3], u32>,
    held: BTreeSet<[u32; 3]>,
    small: BTreeSet<[u32; 3]>,
    step_cache: BTreeMap<(u32, u32, u32), Option<f64>>,
    input: &'a Input<'a>,
    range: Range,
    stats: CdtStats,
}

fn key(mut t: [u32; 3]) -> [u32; 3] {
    t.sort_unstable();
    t
}

fn vh(i: u32) -> FixedVertexHandle {
    FixedVertexHandle::from_index(i as usize)
}

impl State<'_> {
    fn bucket_range(a: P, b: P) -> impl Iterator<Item = (i32, i32)> {
        let lo = [a[0].min(b[0]).div_euclid(BUCKET), a[1].min(b[1]).div_euclid(BUCKET)];
        let hi = [a[0].max(b[0]).div_euclid(BUCKET), a[1].max(b[1]).div_euclid(BUCKET)];
        (lo[1]..=hi[1]).flat_map(move |y| (lo[0]..=hi[0]).map(move |x| (x, y)))
    }

    fn add_seg(&mut self, s: Seg) -> u32 {
        let id = self.segs.len() as u32;
        let (a, b) = (self.pts[s.a as usize], self.pts[s.b as usize]);
        for k in Self::bucket_range(a, b) {
            self.buckets.entry(k).or_default().push(id);
        }
        self.seg_of.insert((s.a.min(s.b), s.a.max(s.b)), id);
        self.segs.push(s);
        id
    }

    /// The alive segments whose boxes come within r of p.
    fn near(&self, p: P, r: i32) -> Vec<u32> {
        let mut out: Vec<u32> = Self::bucket_range([p[0] - r, p[1] - r], [p[0] + r, p[1] + r])
            .filter_map(|k| self.buckets.get(&k))
            .flatten()
            .copied()
            .filter(|&s| self.segs[s as usize].alive)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    fn mpu(&self, y: f64) -> f64 {
        (self.input.mpu)(y)
    }

    fn l_max_units(&self, face: u32, y: f64) -> f64 {
        let f = self.input.faces[face as usize];
        l_max_m(f.surface, f.rule, self.range) / self.mpu(y)
    }

    /// The face of an inner triangle: across non-constraint edges to the nearest constraint or a
    /// triangle whose face is known.
    fn face_of(&mut self, f: FixedFaceHandle<InnerTag>) -> u32 {
        let k = key(self.cdt.face(f).vertices().map(|v| v.fix().index() as u32));
        if let Some(&face) = self.face_cache.get(&k) {
            return face;
        }
        let mut queue = VecDeque::from([f]);
        let mut seen = vec![k];
        let mut found = OUTSIDE;
        'walk: while let Some(f) = queue.pop_front() {
            for e in self.cdt.face(f).adjacent_edges() {
                let (from, to) = (e.from().fix().index() as u32, e.to().fix().index() as u32);
                if e.is_constraint_edge() {
                    if let Some(&s) = self.seg_of.get(&(from.min(to), from.max(to))) {
                        let s = self.segs[s as usize];
                        found = if s.a == from { s.left } else { s.right };
                        break 'walk;
                    }
                }
                let Some(g) = e.rev().face().as_inner() else { continue };
                let gk = key(g.vertices().map(|v| v.fix().index() as u32));
                if let Some(&face) = self.face_cache.get(&gk) {
                    found = face;
                    break 'walk;
                }
                if !seen.contains(&gk) {
                    seen.push(gk);
                    queue.push_back(g.fix());
                }
            }
        }
        for s in seen {
            self.face_cache.insert(s, found);
        }
        found
    }

    fn tri_of(&self, f: FixedFaceHandle<InnerTag>) -> [u32; 3] {
        self.cdt.face(f).vertices().map(|v| v.fix().index() as u32)
    }

    fn exists(&self, t: [u32; 3]) -> Option<FixedFaceHandle<InnerTag>> {
        let e = self.cdt.get_edge_from_neighbors(vh(t[0]), vh(t[1]))?;
        [e, e.rev()].into_iter().find_map(|d| {
            let f = d.face().as_inner()?;
            f.vertices().iter().any(|v| v.fix().index() as u32 == t[2]).then(|| f.fix())
        })
    }

    fn is_locked(&self, a: u32, b: u32) -> bool {
        self.seg_of.get(&(a.min(b), a.max(b))).is_some_and(|&s| self.segs[s as usize].locked)
    }

    /// The smallest angle (radians) of a triangle and whether it is bad: under the least angle, or
    /// an edge that is no locked segment longer than its face's `l_max`.
    fn quality(&mut self, f: FixedFaceHandle<InnerTag>) -> Option<(f64, bool, u32)> {
        let face = self.face_of(f);
        if face == OUTSIDE {
            return None;
        }
        let t = self.tri_of(f);
        let p = t.map(|v| self.pts[v as usize]);
        let l = [len(p[1], p[2]), len(p[2], p[0]), len(p[0], p[1])];
        let k = (0..3).min_by(|&i, &j| l[i].total_cmp(&l[j])).unwrap();
        let (a, b, c) = (l[k], l[(k + 1) % 3], l[(k + 2) % 3]);
        let cos = ((b * b + c * c - a * a) / (2.0 * b * c)).clamp(-1.0, 1.0);
        let angle = libm::acos(cos);
        let cy = f64::from(p[0][1] + p[1][1] + p[2][1]) / 3.0;
        let lmax = self.l_max_units(face, cy);
        let long = (0..3).any(|i| l[i] > lmax && !self.is_locked(t[(i + 1) % 3], t[(i + 2) % 3]));
        Some((angle, angle < self.input.min_angle_deg.to_radians() || long, face))
    }

    fn push(&mut self, heap: &mut Queue, f: FixedFaceHandle<InnerTag>) {
        let t = self.tri_of(f);
        let k = key(t);
        if self.held.contains(&k) || self.small.contains(&k) {
            return;
        }
        if let Some((angle, true, _)) = self.quality(f) {
            let p = t.map(|v| self.pts[v as usize]);
            let c = [0, 1].map(|i| (i64::from(p[0][i]) + i64::from(p[1][i]) + i64::from(p[2][i])).div_euclid(3));
            heap.push(Reverse((angle.to_bits(), c[1], c[0], t)));
        }
    }

    /// Pushes the triangles about vertex v and its neighbours.
    fn push_about(&mut self, heap: &mut Queue, v: u32) {
        let ring: Vec<u32> = self.cdt.vertex(vh(v)).out_edges().map(|e| e.to().fix().index() as u32).collect();
        let mut faces = Vec::new();
        for w in std::iter::once(v).chain(ring) {
            faces.extend(self.cdt.vertex(vh(w)).out_edges().filter_map(|e| e.face().as_inner()).map(|f| f.fix()));
        }
        faces.sort_unstable_by_key(|f| f.index());
        faces.dedup();
        for f in faces {
            self.push(heap, f);
        }
    }

    /// Whether the input segments of two vertices meet at a common end under 60° and the vertices lie
    /// equally far from it: Shewchuk's case that no point fixes.
    fn small_corner(&self, u: u32, v: u32) -> bool {
        let (pu, pv) = (self.pts[u as usize], self.pts[v as usize]);
        for &s in &self.on_inputs[u as usize] {
            for &t in &self.on_inputs[v as usize] {
                if s == t {
                    continue;
                }
                let ([a0, a1], [b0, b1]) = (self.inputs[s as usize], self.inputs[t as usize]);
                for o in [a0, a1] {
                    if o != b0 && o != b1 {
                        continue;
                    }
                    let (sa, sb) = (if o == a0 { a1 } else { a0 }, if o == b0 { b1 } else { b0 });
                    let angle = libm::acos(
                        ((f64::from(sa[0] - o[0]) * f64::from(sb[0] - o[0])
                            + f64::from(sa[1] - o[1]) * f64::from(sb[1] - o[1]))
                            / (len(o, sa) * len(o, sb)))
                        .clamp(-1.0, 1.0),
                    );
                    if angle < SMALL_CORNER_DEG.to_radians()
                        && (len(o, pu) - len(o, pv)).abs() <= EQUAL_UNITS
                        && pu != o
                        && pv != o
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Whether another constraint meets vertex v at under 60° with the segment toward w.
    fn acute(&self, v: u32, w: u32) -> bool {
        let (pv, pw) = (self.pts[v as usize], self.pts[w as usize]);
        self.cdt.vertex(vh(v)).out_edges().any(|e| {
            let x = e.to().fix().index() as u32;
            if x == w || !e.is_constraint_edge() {
                return false;
            }
            let px = self.pts[x as usize];
            let dot = f64::from(pw[0] - pv[0]) * f64::from(px[0] - pv[0])
                + f64::from(pw[1] - pv[1]) * f64::from(px[1] - pv[1]);
            libm::acos((dot / (len(pv, pw) * len(pv, px))).clamp(-1.0, 1.0)) < SMALL_CORNER_DEG.to_radians()
        })
    }

    /// The split point of a segment: at the power-of-two distance from an acute end nearest the
    /// middle (concentric shells), else the middle; rounded.
    fn split_point(&self, s: &Seg) -> P {
        let (a, b) = (self.pts[s.a as usize], self.pts[s.b as usize]);
        let (aa, ab) = (self.acute(s.a, s.b), self.acute(s.b, s.a));
        let l = len(a, b);
        let shell = |o: P, q: P| {
            let d = libm::exp2(libm::round(libm::log2(l / 2.0)));
            [0, 1].map(|i| (f64::from(o[i]) + f64::from(q[i] - o[i]) * d / l).round() as i32)
        };
        match (aa, ab) {
            (true, false) => shell(a, b),
            (false, true) => shell(b, a),
            _ => part(a, b, 1, 2),
        }
    }

    /// Whether a new segment from p to q conflicts with an alive constraint other than `except`.
    fn blocked(&self, p: P, q: P, except: u32) -> bool {
        let r = (q[0] - p[0]).abs().max((q[1] - p[1]).abs());
        let mid = part(p, q, 1, 2);
        self.near(mid, r / 2 + 1).into_iter().filter(|&s| s != except).any(|s| {
            let s = self.segs[s as usize];
            conflict(p, q, self.pts[s.a as usize], self.pts[s.b as usize])
        })
    }

    /// Splits face-edge segment `s` at m: false (and the segment held) where it cannot.
    fn split_at(&mut self, heap: &mut Queue, s: u32, m: P, steps: &mut Steps) -> bool {
        let seg = self.segs[s as usize];
        let (a, b) = (self.pts[seg.a as usize], self.pts[seg.b as usize]);
        let at_vertex = matches!(
            self.cdt.locate(Point2::new(f64::from(m[0]), f64::from(m[1]))),
            PositionInTriangulation::OnVertex(_)
        );
        if m == a || m == b || at_vertex || self.blocked(a, m, s) || self.blocked(m, b, s) {
            self.segs[s as usize].held = true;
            self.stats.held_segments += 1;
            return false;
        }
        let edge = self.cdt.get_edge_from_neighbors(vh(seg.a), vh(seg.b)).map(|e| e.fix().as_undirected());
        if let Some(e) = edge {
            self.cdt.remove_constraint_edge(e);
        }
        self.segs[s as usize].alive = false;
        self.seg_of.remove(&(seg.a.min(seg.b), seg.a.max(seg.b)));
        let vm =
            self.cdt.insert(Point2::new(f64::from(m[0]), f64::from(m[1]))).expect("an integer point").index() as u32;
        debug_assert_eq!(vm as usize, self.pts.len());
        self.pts.push(m);
        self.on_inputs.push(vec![seg.input]);
        self.stats.segment_splits += 1;
        let mut halves = Vec::new();
        for (p, q) in [(seg.a, vm), (vm, seg.b)] {
            let d =
                [self.pts[q as usize][0] - self.pts[p as usize][0], self.pts[q as usize][1] - self.pts[p as usize][1]];
            for e in self.cdt.try_add_constraint(vh(p), vh(q)) {
                let e = self.cdt.directed_edge(e);
                let (x, y) = (e.from().fix().index() as u32, e.to().fix().index() as u32);
                let (px, py) = (self.pts[x as usize], self.pts[y as usize]);
                let along = i64::from(py[0] - px[0]) * i64::from(d[0]) + i64::from(py[1] - px[1]) * i64::from(d[1]);
                let (x, y) = if along >= 0 { (x, y) } else { (y, x) };
                halves.push(self.add_seg(Seg { a: x, b: y, alive: true, held: false, ..seg }));
            }
        }
        // A half whose step changes sign splits at its zero point at once.
        for h in halves {
            self.split_sign(heap, h, steps);
        }
        for v in [seg.a, vm, seg.b] {
            self.push_about(heap, v);
        }
        true
    }

    fn step_at(&mut self, v: u32, left: u32, right: u32, steps: &mut Steps) -> Option<f64> {
        if left == OUTSIDE || right == OUTSIDE {
            return Some(0.0);
        }
        if let Some(&s) = self.step_cache.get(&(v, left, right)) {
            return s;
        }
        let s = steps(self.pts[v as usize], left, right);
        self.step_cache.insert((v, left, right), s);
        s
    }

    /// Splits a face-edge segment at the zero point of its step where the step changes sign
    /// beyond the join between its ends.
    fn split_sign(&mut self, heap: &mut Queue, s: u32, steps: &mut Steps) {
        let seg = self.segs[s as usize];
        if seg.locked || !seg.alive {
            return;
        }
        let join = self.input.join_m;
        let (Some(sa), Some(sb)) =
            (self.step_at(seg.a, seg.left, seg.right, steps), self.step_at(seg.b, seg.left, seg.right, steps))
        else {
            return;
        };
        if !((sa > join && sb < -join) || (sa < -join && sb > join)) {
            return;
        }
        let (a, b) = (self.pts[seg.a as usize], self.pts[seg.b as usize]);
        let t = sa / (sa - sb);
        let z = [0, 1].map(|i| (f64::from(a[i]) + f64::from(b[i] - a[i]) * t).round() as i32);
        self.split_at(heap, s, z, steps);
    }
}

/// The triangulation of D in one range.
pub fn triangulate(input: &Input, range: Range, steps: &mut Steps) -> Result<Triangles, MeshError> {
    // The input segments: face edges split into equal parts no longer than their faces' l_max.
    let mut pieces: Vec<(P, P, u32, u32, bool, u32)> = Vec::new();
    let mut inputs = Vec::new();
    for c in &input.constraints {
        if c.a == c.b {
            continue;
        }
        let id = inputs.len() as u32;
        inputs.push([c.a, c.b]);
        let n = if c.locked {
            1
        } else {
            let y = f64::from(c.a[1] + c.b[1]) / 2.0;
            let l = [c.left, c.right]
                .iter()
                .filter(|&&f| f != OUTSIDE)
                .map(|&f| {
                    let f = input.faces[f as usize];
                    l_max_m(f.surface, f.rule, range) / (input.mpu)(y)
                })
                .fold(f64::INFINITY, f64::min);
            (len(c.a, c.b) / l).ceil().max(1.0) as i128
        };
        let mut last = c.a;
        for k in 1..=n {
            let p = if k == n { c.b } else { part(c.a, c.b, k, n) };
            if p != last {
                pieces.push((last, p, c.left, c.right, c.locked, id));
                last = p;
            }
        }
    }
    // The points sorted by (y, x): spade keeps the order, so a vertex's index is its handle's.
    let mut points: Vec<P> = pieces.iter().flat_map(|p| [p.0, p.1]).collect();
    points.sort_unstable_by_key(|p| (p[1], p[0]));
    points.dedup();
    let index: BTreeMap<P, u32> = points.iter().enumerate().map(|(i, &p)| (p, i as u32)).collect();
    // The exact check: no two segments cross, overlap or touch but at a shared end.
    let mut buckets: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (k, p) in pieces.iter().enumerate() {
        for b in State::bucket_range(p.0, p.1) {
            buckets.entry(b).or_default().push(k);
        }
    }
    let mut keys: Vec<&(i32, i32)> = buckets.keys().collect();
    keys.sort_unstable();
    for b in keys {
        let list = &buckets[b];
        for (x, &i) in list.iter().enumerate() {
            for &j in &list[x + 1..] {
                let (s, t) = (pieces[i], pieces[j]);
                if conflict(s.0, s.1, t.0, t.1) {
                    let same = (s.0 == t.0 && s.1 == t.1) || (s.0 == t.1 && s.1 == t.0);
                    let at = if same { s.0 } else { part(s.0, s.1, 1, 2) };
                    return Err(MeshError::InvalidPlan {
                        at,
                        why: format!("constraints {:?}-{:?} and {:?}-{:?} meet", s.0, s.1, t.0, t.1),
                    });
                }
            }
        }
    }
    let edges: Vec<[usize; 2]> = pieces.iter().map(|p| [index[&p.0] as usize, index[&p.1] as usize]).collect();
    let vertices: Vec<Point2<f64>> = points.iter().map(|p| Point2::new(f64::from(p[0]), f64::from(p[1]))).collect();
    let cdt = Cdt::bulk_load_cdt(vertices, edges).map_err(|e| MeshError::InvalidPlan {
        at: points.first().copied().unwrap_or_default(),
        why: format!("{e:?}"),
    })?;
    let mut on_inputs = vec![Vec::new(); points.len()];
    for p in &pieces {
        for v in [p.0, p.1] {
            let list: &mut Vec<u32> = &mut on_inputs[index[&v] as usize];
            if !list.contains(&p.5) {
                list.push(p.5);
            }
        }
    }
    let mut st = State {
        cdt,
        pts: points,
        segs: Vec::new(),
        seg_of: BTreeMap::new(),
        buckets: BTreeMap::new(),
        inputs,
        on_inputs,
        face_cache: BTreeMap::new(),
        held: BTreeSet::new(),
        small: BTreeSet::new(),
        step_cache: BTreeMap::new(),
        input,
        range,
        stats: CdtStats::default(),
    };
    st.stats.constraint_points = st.pts.len() as u32;
    for p in &pieces {
        let s = Seg {
            a: index[&p.0],
            b: index[&p.1],
            left: p.2,
            right: p.3,
            locked: p.4,
            held: false,
            alive: true,
            input: p.5,
        };
        st.add_seg(s);
    }
    let mut heap = BinaryHeap::new();
    // Each face edge splits where its step changes sign.
    for s in 0..st.segs.len() as u32 {
        st.split_sign(&mut heap, s, steps);
    }
    // The grid points of slope and terrain faces, row by row, clear of every constraint.
    let g = grid(range);
    let c = CELL_UNITS as i32;
    for y in (g..c).step_by(g as usize) {
        let clear = grid_clearance_m(range) / st.mpu(f64::from(y));
        let r = clear.ceil() as i32;
        for x in (g..c).step_by(g as usize) {
            let p = [x, y];
            if !input.region.in_d([i64::from(x), i64::from(y)]) {
                continue;
            }
            let close = st.near(p, r).into_iter().any(|s| {
                let s = st.segs[s as usize];
                dist_to(st.pts[s.a as usize], st.pts[s.b as usize], p) <= clear
            });
            if close {
                continue;
            }
            let at = Point2::new(f64::from(x), f64::from(y));
            let face = match st.cdt.locate(at) {
                PositionInTriangulation::OnFace(f) => st.face_of(f),
                PositionInTriangulation::OnEdge(e) => match st.cdt.directed_edge(e).face().as_inner() {
                    Some(f) => st.face_of(f.fix()),
                    None => OUTSIDE,
                },
                _ => OUTSIDE,
            };
            let rule = (face != OUTSIDE).then(|| input.faces[face as usize].rule);
            if matches!(rule, Some(HeightRule::Slope | HeightRule::Terrain)) {
                st.cdt.insert(at).expect("an integer point");
                st.pts.push(p);
                st.on_inputs.push(Vec::new());
                st.stats.grid_points += 1;
            }
        }
    }
    refine(&mut st, &mut heap, steps);
    // The result: D's triangles turned as gridIndices.
    let faces: Vec<FixedFaceHandle<InnerTag>> = st.cdt.fixed_inner_faces().collect();
    let (mut tris, mut locked, mut small) = (Vec::new(), Vec::new(), Vec::new());
    for f in faces {
        let face = st.face_of(f);
        if face == OUTSIDE {
            continue;
        }
        let t = st.tri_of(f);
        let k = key(t);
        let on_locked = (0..3).any(|i| st.is_locked(t[i], t[(i + 1) % 3]));
        tris.push(([t[0], t[2], t[1]], face));
        locked.push(on_locked || st.held.contains(&k));
        small.push(st.small.contains(&k));
    }
    let segments = (st.segs.iter())
        .filter(|s| s.alive)
        .map(|s| Segment { a: s.a, b: s.b, left: s.left, right: s.right, locked: s.locked })
        .collect();
    Ok(Triangles { points: st.pts, tris, locked, small, segments, stats: st.stats })
}

/// The distance from p to the segment ab, in frame units.
fn dist_to(a: P, b: P, p: P) -> f64 {
    let (e, w) = ([f64::from(b[0] - a[0]), f64::from(b[1] - a[1])], [f64::from(p[0] - a[0]), f64::from(p[1] - a[1])]);
    let l2 = e[0] * e[0] + e[1] * e[1];
    let t = if l2 > 0.0 { ((w[0] * e[0] + w[1] * e[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    libm::hypot(w[0] - t * e[0], w[1] - t * e[1])
}

/// The circumcentre of a triangle of integer points, rounded to the frame unit.
fn circumcentre(p: [P; 3]) -> Option<P> {
    let [a, b, c] = p.map(|v| [i128::from(v[0]), i128::from(v[1])]);
    let (bx, by, cx, cy) = (b[0] - a[0], b[1] - a[1], c[0] - a[0], c[1] - a[1]);
    let d = 2 * (bx * cy - by * cx);
    if d == 0 {
        return None;
    }
    let (b2, c2) = (bx * bx + by * by, cx * cx + cy * cy);
    let ux = round_div(cy * b2 - by * c2, d);
    let uy = round_div(bx * c2 - cx * b2, d);
    Some([(a[0] + ux) as i32, (a[1] + uy) as i32])
}

/// Whether p lies in the closed diametral circle of ab and is not one of its ends.
fn encroaches(a: P, b: P, p: P) -> bool {
    p != a && p != b && {
        let (u, v) =
            ([i64::from(a[0] - p[0]), i64::from(a[1] - p[1])], [i64::from(b[0] - p[0]), i64::from(b[1] - p[1])]);
        u[0] * v[0] + u[1] * v[1] <= 0
    }
}

fn refine(st: &mut State, heap: &mut Queue, steps: &mut Steps) {
    let budget = REFINE_BUDGET * st.pts.len();
    let start = st.pts.len();
    loop {
        let faces: Vec<FixedFaceHandle<InnerTag>> = st.cdt.fixed_inner_faces().collect();
        for f in faces {
            st.push(heap, f);
        }
        if heap.is_empty() {
            return;
        }
        while let Some(Reverse((_, _, _, t))) = heap.pop() {
            let Some(f) = st.exists(t) else { continue };
            let k = key(t);
            if st.held.contains(&k) || st.small.contains(&k) {
                continue;
            }
            let Some((angle, true, _)) = st.quality(f) else { continue };
            if st.pts.len() - start >= budget {
                st.stats.refine_incomplete = 1;
                heap.clear();
                return;
            }
            let p = t.map(|v| st.pts[v as usize]);
            if angle < st.input.min_angle_deg.to_radians() {
                // The shortest edge's ends.
                let l = [len(p[1], p[2]), len(p[2], p[0]), len(p[0], p[1])];
                let i = (0..3).min_by(|&i, &j| l[i].total_cmp(&l[j])).unwrap();
                if st.small_corner(t[(i + 1) % 3], t[(i + 2) % 3]) {
                    st.small.insert(k);
                    st.stats.small_corner += 1;
                    continue;
                }
            }
            let Some(c) = circumcentre(p) else {
                st.held.insert(k);
                continue;
            };
            let reach = p.iter().map(|q| len(*q, c)).fold(0.0, f64::max) as i32 + 1;
            let enc: Vec<u32> = (st.near(c, reach).into_iter())
                .filter(|&s| {
                    let s = st.segs[s as usize];
                    encroaches(st.pts[s.a as usize], st.pts[s.b as usize], c)
                })
                .collect();
            let splittable: Vec<u32> =
                enc.iter().copied().filter(|&s| !st.segs[s as usize].locked && !st.segs[s as usize].held).collect();
            if !splittable.is_empty() {
                for s in splittable {
                    if st.segs[s as usize].alive {
                        let m = st.split_point(&st.segs[s as usize]);
                        st.split_at(heap, s, m, steps);
                    }
                }
                if let Some(f) = st.exists(t) {
                    st.push(heap, f);
                }
                continue;
            }
            if !enc.is_empty() || !st.input.region.in_d([i64::from(c[0]), i64::from(c[1])]) {
                st.held.insert(k);
                st.stats.held_locked += 1;
                continue;
            }
            let at = Point2::new(f64::from(c[0]), f64::from(c[1]));
            match st.cdt.locate(at) {
                PositionInTriangulation::OnFace(_) | PositionInTriangulation::OnEdge(_) => {}
                _ => {
                    st.held.insert(k);
                    continue;
                }
            }
            let v = st.cdt.insert(at).expect("an integer point").index() as u32;
            st.pts.push(c);
            st.on_inputs.push(Vec::new());
            st.stats.steiner += 1;
            st.push_about(heap, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossings_overlaps_and_touches_conflict_and_shared_ends_do_not() {
        assert!(conflict([0, 0], [10, 10], [0, 10], [10, 0]));
        assert!(conflict([0, 0], [10, 0], [5, 0], [20, 0]));
        assert!(conflict([0, 0], [10, 0], [5, 0], [5, 7]));
        assert!(conflict([0, 0], [10, 0], [0, 0], [20, 0]));
        assert!(!conflict([0, 0], [10, 0], [10, 0], [10, 7]));
        assert!(!conflict([0, 0], [10, 0], [0, 0], [-10, 0]));
        assert!(!conflict([0, 0], [10, 0], [0, 1], [10, 1]));
    }

    #[test]
    fn the_circumcentre_rounds_to_the_unit() {
        assert_eq!(circumcentre([[0, 0], [10, 0], [0, 10]]), Some([5, 5]));
        assert_eq!(circumcentre([[0, 0], [3, 0], [0, 3]]), Some([2, 2]));
        assert!(encroaches([0, 0], [10, 0], [5, 5]) && !encroaches([0, 0], [10, 0], [5, 6]));
    }
}
