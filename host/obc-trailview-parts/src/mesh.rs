//! Faces as the viewer's structure meshes hold them (`structures/mesh.ts`), and the stations and cross-sections that
//! the parts sweep.

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
    /// 1 where this station runs across the way that `other` does, else −1: a metre across one is `sign` metres across
    /// the other, and a run from the right of one is the other's in reverse (`turned_run`).
    pub fn sign(&self, other: &Station) -> f64 {
        if self.ax * other.ax + self.ay * other.ay >= 0.0 {
            1.0
        } else {
            -1.0
        }
    }

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

/// Collects faces. Each face is wound counter-clockwise seen from the side that `out` points to (east, north, up),
/// so the back faces cull.
pub struct Builder<L: Fn([f64; 3]) -> f64> {
    words: Vec<u32>,
    indices: Vec<u32>,
    points: Vec<[f64; 3]>,
    daylight: L,
    unfilled: usize,
}

impl<L: Fn([f64; 3]) -> f64> Builder<L> {
    /// A builder whose vertices take their daylight from `daylight` of their world point.
    pub fn new(daylight: L) -> Self {
        Builder { words: Vec::new(), indices: Vec::new(), points: Vec::new(), daylight, unfilled: 0 }
    }

    /// The triangles of a face's polygon (`seam::ear_clip`); a face that it does not cut whole counts as unfilled.
    pub fn ear_clip(&mut self, poly: &[Corner]) -> Vec<[usize; 3]> {
        let (out, whole) = crate::seam::ear_clip(poly);
        self.unfilled += usize::from(!whole);
        out
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
        (self.points.len() - 1) as u32
    }

    pub fn at(&mut self, s: &Station, c: Corner, colour: u32) -> u32 {
        self.vertex(s.at(c), colour, PLAIN)
    }

    /// A quad of four vertices in order around it, turned to face `out`: two triangles, each of no area none.
    pub fn quad(&mut self, v: [u32; 4], out: [f64; 3]) {
        self.triangle(v[0], v[1], v[2], out);
        self.triangle(v[0], v[2], v[3], out);
    }

    /// A triangle turned to face `out`. A triangle of no area makes no face, nor one with two corners on one point of
    /// the mesh's words.
    pub fn triangle(&mut self, a: u32, b: u32, c: u32, out: [f64; 3]) {
        let n = self.normal(a, b, c, c);
        let at = |v: u32| &self.words[v as usize * WORDS..v as usize * WORDS + 3];
        if n.iter().all(|c| c.abs() < 1e-12) || at(a) == at(b) || at(b) == at(c) || at(c) == at(a) {
            return;
        }
        let facing = n[0] * out[0] + n[1] * out[1] + n[2] * out[2] >= 0.0;
        self.indices.extend(if facing { [a, b, c] } else { [a, c, b] });
    }

    /// The faces between two polylines that run side by side from a common start to a common end: the corners `a` at
    /// station `s` and the corners `b` at station `t`. The triangles go in the order of the share of the way along
    /// each, so every corner of both is a vertex of the faces beside it, and no face meets another in the middle of
    /// an edge. Two corners on each side make one quad.
    pub fn strip(&mut self, (s, a): (&Station, &[Corner]), (t, b): (&Station, &[Corner]), colour: u32, out: [f64; 3]) {
        let (pa, pb): (Vec<[f64; 3]>, Vec<[f64; 3]>) =
            (a.iter().map(|&c| s.at(c)).collect(), b.iter().map(|&c| t.at(c)).collect());
        self.strip_at((&pa, a), (&pb, b), colour, out);
    }

    /// The faces of `strip` between two polylines of world points, each with its corners for the shares along it.
    pub fn strip_at(
        &mut self,
        (pa, a): (&[[f64; 3]], &[Corner]),
        (pb, b): (&[[f64; 3]], &[Corner]),
        colour: u32,
        out: [f64; 3],
    ) {
        // A side runs one way: no step of it turns back against the one before.
        let folds = |c: &[Corner]| {
            c.windows(3)
                .any(|w| (w[1][0] - w[0][0]) * (w[2][0] - w[1][0]) + (w[1][1] - w[0][1]) * (w[2][1] - w[1][1]) < 0.0)
        };
        debug_assert!(!folds(a) && !folds(b), "a side of a strip folds back: {a:?} {b:?}");
        let va: Vec<u32> = pa.iter().map(|&p| self.vertex(p, colour, PLAIN)).collect();
        let vb: Vec<u32> = pb.iter().map(|&p| self.vertex(p, colour, PLAIN)).collect();
        self.strip_of((&va, a), (&vb, b), out);
    }

    /// The faces of `strip` between two polylines of vertices, each with its corners for the shares along it.
    pub fn strip_of(&mut self, (va, a): (&[u32], &[Corner]), (vb, b): (&[u32], &[Corner]), out: [f64; 3]) {
        let shares = |c: &[Corner]| {
            let mut run = vec![0.0];
            for w in c.windows(2) {
                run.push(run[run.len() - 1] + libm::hypot(w[1][0] - w[0][0], w[1][1] - w[0][1]));
            }
            let all = run[run.len() - 1];
            run.iter().map(|r| if all > 0.0 { r / all } else { 0.0 }).collect::<Vec<f64>>()
        };
        let (sa, sb) = (shares(a), shares(b));
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

    pub fn mesh(self) -> Mesh {
        Mesh { vertices: self.words, indices: self.indices, unfilled: self.unfilled }
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

/// A run from the right in the metres of a station whose `sign` to another's is `sign`, from the right in the other's:
/// each item `across` it, in reverse where the sign is −1.
pub fn turned_run<T, U>(sign: f64, items: impl IntoIterator<Item = T>, across: impl Fn(T) -> U) -> Vec<U> {
    let mut out: Vec<U> = items.into_iter().map(across).collect();
    if sign < 0.0 {
        out.reverse();
    }
    out
}
