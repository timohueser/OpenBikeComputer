//! Faces as the viewer's structure meshes hold them (`structures/mesh.ts`), and the stations and cross-sections that
//! the parts sweep.

use std::collections::HashMap;

use crate::{metres_per_unit, Mesh, Section};

/// Words of a vertex: x, y (i32 world units), z (f32), colour and flags, across and half-width of a road face
/// (snorm16 of ±`ROAD_RANGE_M`), sleepers (f32), daylight (f32).
pub const WORDS: usize = 7;
const ROAD_RANGE_M: f64 = 16.0;
/// A face without a count of sleepers, as `NO_SLEEPERS` of `mesh.ts`.
pub const NO_SLEEPERS: f64 = -1e9;
/// Flags in a colour's alpha, as `mesh.ts`: a road face, the dark face of an opening, a lamp, a face in a bore.
pub const PAINT: u32 = 2;
pub const VOID: u32 = 4;
pub const LAMP: u32 = 16;
pub const BORE: u32 = 128;

/// A colour with flags.
pub fn flagged(colour: u32, flags: u32) -> u32 {
    colour | (flags << 24)
}

/// A point of a cross-section: metres across to the left of the line, and metres up.
pub type Corner = [f64; 2];

/// The left of a unit direction, x east and y south.
pub fn left_of(d: [f64; 2]) -> [f64; 2] {
    [d[1], -d[0]]
}

/// A world point and its height, and the world units per metre across to its left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub ax: f64,
    pub ay: f64,
}

impl Station {
    /// The station on a unit direction `d` in world units, across to its left, `scale` metres per metre across.
    pub fn across(p: [f64; 2], z: f64, d: [f64; 2], scale: f64) -> Station {
        let (l, k) = (left_of(d), scale / metres_per_unit(p[1]));
        Station { x: p[0], y: p[1], z, ax: l[0] * k, ay: l[1] * k }
    }

    pub fn at(&self, [a, up]: Corner) -> [f64; 3] {
        [self.x + self.ax * a, self.y + self.ay * a, self.z + up]
    }

    /// The station a share u of the way to t.
    pub fn mix(&self, t: &Station, u: f64) -> Station {
        let m = |a: f64, b: f64| a + (b - a) * u;
        Station { x: m(self.x, t.x), y: m(self.y, t.y), z: m(self.z, t.z), ax: m(self.ax, t.ax), ay: m(self.ay, t.ay) }
    }

    /// The same station across to the right.
    pub fn turned(&self) -> Station {
        Station { ax: -self.ax, ay: -self.ay, ..*self }
    }

    /// The direction east, north, up of a direction [across, up] in the cross-section.
    pub fn out(&self, [a, up]: Corner) -> [f64; 3] {
        let l = libm::hypot(self.ax, self.ay).max(f64::MIN_POSITIVE);
        [a * self.ax / l, -a * self.ay / l, up]
    }
}

/// The direction east, north, up of a world direction.
pub fn flat(d: [f64; 2]) -> [f64; 3] {
    [d[0], -d[1], 0.0]
}

/// The outline of a section from its right foot over its top to its left foot: half-width `half`, top `top`, a round
/// section's arch from `spring` up in `segments`. Its two sides mirror each other exactly.
pub fn outline(section: Section, half: f64, top: f64, spring: f64, segments: usize) -> Vec<Corner> {
    if section == Section::Box {
        return vec![[-half, 0.0], [-half, top], [half, top], [half, 0.0]];
    }
    let mut out = vec![[-half, 0.0]];
    for k in 0..=segments {
        let j = k.min(segments - k);
        let a = std::f64::consts::PI * j as f64 / segments as f64;
        let x = if 2 * j == segments { 0.0 } else { half * libm::cos(a) };
        out.push([if j == k { -x } else { x }, spring + (top - spring) * libm::sin(a)]);
    }
    out.push([half, 0.0]);
    out
}

/// The road words of a vertex: metres across from a road face's middle and its half-width, and its sleepers.
#[derive(Clone, Copy, Debug)]
pub struct Paint {
    pub across: f64,
    pub half: f64,
    pub sleepers: f64,
}

pub const PLAIN: Paint = Paint { across: 0.0, half: 0.0, sleepers: NO_SLEEPERS };

/// A vertical plane through two world points in plan that takes off the faces on one side of it: the side of `away`,
/// a world direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clip {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub away: [f64; 2],
}

impl Clip {
    /// How far a world point lies on the side that the plane takes off, in world units.
    fn side(&self, p: [f64; 3]) -> f64 {
        let n = [self.b[1] - self.a[1], self.a[0] - self.b[0]];
        let l = libm::hypot(n[0], n[1]).max(f64::MIN_POSITIVE);
        let sign = if n[0] * self.away[0] + n[1] * self.away[1] >= 0.0 { 1.0 } else { -1.0 };
        sign * ((p[0] - self.a[0]) * n[0] + (p[1] - self.a[1]) * n[1]) / l
    }
}

/// Collects faces. Each face is wound counter-clockwise seen from the side that `out` points to (east, north, up),
/// so the back faces cull. With a clip, the part of a face past its plane is not drawn.
pub struct Builder<L: Fn([f64; 3]) -> f64> {
    words: Vec<u32>,
    indices: Vec<u32>,
    points: Vec<[f64; 3]>,
    looks: Vec<(u32, Paint)>,
    daylight: L,
    clip: Option<Clip>,
    /// The vertex where the clip's plane cuts the edge between two vertices.
    cuts: HashMap<(u32, u32), u32>,
}

impl<L: Fn([f64; 3]) -> f64> Builder<L> {
    /// A builder whose vertices take their daylight from `daylight` of their world point.
    pub fn new(daylight: L) -> Self {
        Builder {
            words: Vec::new(),
            indices: Vec::new(),
            points: Vec::new(),
            looks: Vec::new(),
            daylight,
            clip: None,
            cuts: HashMap::new(),
        }
    }

    /// The clip of the faces from now on.
    pub fn clip(&mut self, clip: Option<Clip>) {
        if clip != self.clip {
            (self.clip, self.cuts) = (clip, HashMap::new());
        }
    }

    pub fn vertex(&mut self, p: [f64; 3], colour: u32, paint: Paint) -> u32 {
        let snorm = |m: f64| (libm::round((m / ROAD_RANGE_M).clamp(-1.0, 1.0) * 32767.0) as i32 as u32) & 0xffff;
        let light = (self.daylight)(p);
        self.words.extend([
            libm::round(p[0]) as i32 as u32,
            libm::round(p[1]) as i32 as u32,
            (p[2] as f32).to_bits(),
            colour,
            snorm(paint.across) | (snorm(paint.half) << 16),
            (paint.sleepers as f32).to_bits(),
            (light as f32).to_bits(),
        ]);
        self.points.push(p);
        self.looks.push((colour, paint));
        (self.points.len() - 1) as u32
    }

    /// The vertex where the clip's plane cuts the edge from vertex i to j, which lie at `di` and `dj` from it.
    fn cut(&mut self, (i, di): (u32, f64), (j, dj): (u32, f64)) -> u32 {
        let key = (i.min(j), i.max(j));
        if let Some(&v) = self.cuts.get(&key) {
            return v;
        }
        let (u, p, q) = (di / (di - dj), self.points[i as usize], self.points[j as usize]);
        let ((colour, a), z) = (self.looks[i as usize], self.looks[j as usize].1);
        let mix = |a: f64, b: f64| a + (b - a) * u;
        let paint = Paint { across: mix(a.across, z.across), half: a.half, sleepers: mix(a.sleepers, z.sleepers) };
        let v = self.vertex([mix(p[0], q[0]), mix(p[1], q[1]), mix(p[2], q[2])], colour, paint);
        self.cuts.insert(key, v);
        v
    }

    /// A triangle in its winding, without its part past the clip's plane.
    fn emit(&mut self, t: [u32; 3]) {
        let Some(clip) = self.clip else {
            self.indices.extend(t);
            return;
        };
        let d = t.map(|v| clip.side(self.points[v as usize]));
        // Within a hundredth of a world unit of the plane, a point lies on it.
        let off = |x: f64| x > 0.01;
        if !d.iter().any(|&x| off(x)) {
            self.indices.extend(t);
            return;
        }
        // The corners on the near side, and the points where an edge crosses the plane, in the winding.
        let mut keep = Vec::with_capacity(4);
        for k in 0..3 {
            let (i, j) = (k, (k + 1) % 3);
            if !off(d[i]) {
                keep.push(t[i]);
            }
            if off(d[i]) != off(d[j]) {
                keep.push(self.cut((t[i], d[i]), (t[j], d[j])));
            }
        }
        for k in 1..keep.len().saturating_sub(1) {
            self.indices.extend([keep[0], keep[k], keep[k + 1]]);
        }
    }

    pub fn at(&mut self, s: &Station, c: Corner, colour: u32) -> u32 {
        self.vertex(s.at(c), colour, PLAIN)
    }

    /// A quad of four vertices in order around it, turned to face `out`. A quad of no area makes no face.
    pub fn quad(&mut self, v: [u32; 4], out: [f64; 3]) {
        let n = self.normal(v[0], v[1], v[2], v[3]);
        if n.iter().all(|c| c.abs() < 1e-12) {
            return;
        }
        if n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0.0 {
            self.emit([v[0], v[1], v[2]]);
            self.emit([v[0], v[2], v[3]]);
        } else {
            self.emit([v[0], v[2], v[1]]);
            self.emit([v[0], v[3], v[2]]);
        }
    }

    /// A triangle turned to face `out`. A triangle of no area makes no face.
    pub fn triangle(&mut self, a: u32, b: u32, c: u32, out: [f64; 3]) {
        let n = self.normal(a, b, c, c);
        if n.iter().all(|c| c.abs() < 1e-12) {
            return;
        }
        let facing = n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0.0;
        self.emit(if facing { [a, b, c] } else { [a, c, b] });
    }

    /// The faces between two polylines that run side by side from a common start to a common end: the corners `a` at
    /// station `s` and the corners `b` at station `t`. The triangles go in the order of the share of the way along
    /// each, so every corner of both is a vertex of the faces beside it, and no face meets another in the middle of
    /// an edge. Two corners on each side make one quad.
    pub fn strip(&mut self, (s, a): (&Station, &[Corner]), (t, b): (&Station, &[Corner]), colour: u32, out: [f64; 3]) {
        let shares = |c: &[Corner]| {
            let mut run = vec![0.0];
            for w in c.windows(2) {
                run.push(run[run.len() - 1] + libm::hypot(w[1][0] - w[0][0], w[1][1] - w[0][1]));
            }
            let all = run[run.len() - 1];
            run.iter().map(|r| if all > 0.0 { r / all } else { 0.0 }).collect::<Vec<f64>>()
        };
        // A side runs one way: no step of it turns back against the one before.
        let folds = |c: &[Corner]| {
            c.windows(3)
                .any(|w| (w[1][0] - w[0][0]) * (w[2][0] - w[1][0]) + (w[1][1] - w[0][1]) * (w[2][1] - w[1][1]) < 0.0)
        };
        debug_assert!(!folds(a) && !folds(b), "a side of a strip folds back: {a:?} {b:?}");
        let (sa, sb) = (shares(a), shares(b));
        let va: Vec<u32> = a.iter().map(|&c| self.at(s, c, colour)).collect();
        let vb: Vec<u32> = b.iter().map(|&c| self.at(t, c, colour)).collect();
        let (mut i, mut j) = (0, 0);
        while i + 1 < va.len() || j + 1 < vb.len() {
            if i + 1 < va.len() && (j + 1 == vb.len() || sa[i + 1] <= sb[j + 1]) {
                self.triangle(va[i], va[i + 1], vb[j], out);
                i += 1;
            } else {
                self.triangle(va[i], vb[j + 1], vb[j], out);
                j += 1;
            }
        }
    }

    /// The mesh of the faces, with the vertices that a face uses.
    pub fn mesh(self) -> Mesh {
        let mut to = vec![u32::MAX; self.points.len()];
        let mut vertices = Vec::with_capacity(self.words.len());
        let indices = (self.indices.iter())
            .map(|&i| {
                if to[i as usize] == u32::MAX {
                    to[i as usize] = (vertices.len() / WORDS) as u32;
                    vertices.extend_from_slice(&self.words[i as usize * WORDS..(i as usize + 1) * WORDS]);
                }
                to[i as usize]
            })
            .collect();
        Mesh { vertices, indices }
    }

    /// The normal of a, b, c, d by their diagonals, in metres east, north and up.
    fn normal(&self, a: u32, b: u32, c: u32, d: u32) -> [f64; 3] {
        let o = self.points[a as usize];
        let m = metres_per_unit(o[1]);
        let p = |i: u32| {
            let q = self.points[i as usize];
            [(q[0] - o[0]) * m, -(q[1] - o[1]) * m, q[2] - o[2]]
        };
        let (pa, pb, pc, pd) = (p(a), p(b), p(c), p(d));
        let u = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let w = [pd[0] - pb[0], pd[1] - pb[1], pd[2] - pb[2]];
        [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]]
    }
}
