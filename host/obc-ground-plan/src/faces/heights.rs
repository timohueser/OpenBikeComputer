//! Heights of the one road model: one point function over a cell's records (`records`), for the near
//! mesh's vertices (here) and the far stamp's texels (`terrain/heights.wgsl`, a port line for line).
//! Prototype of the one road model; not for merge.
//!
//! Per point, each line near it gives its own surface (its curve's height on the mitre-quad frame plus
//! bank times the metres across), its distance past its core and a presence that falls from 1 at the
//! core's edge to 0 at its feather length. The paved field is a normalized painter of these surfaces in
//! which each pair of records blends as far as they agree: inside the core of the first record present
//! it is that record's surface exactly; a compatible lesser record ramps to it; an incompatible one
//! keeps its own surface and meets it in a step on the greater record's core edge.
//! Slopes are fill and cut cones from each line's core outline (with the corner arcs of its fans), on
//! the paved field, faded into the terrain at the reach: `clamp(T, max fill, min cut)`, cut first.

use super::records::{reference, BAND, BINS, BIN_UNITS, CAND, DISC, HEADER, LABEL, LINE, SAMPLE};
use super::Surface;

/// What kind of rule gave a height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Terrain,
    Slope,
    Paved,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Height {
    pub h: f64,
    pub kind: Kind,
    /// The owner line of a paved point, the line whose cone holds a slope point, else `u32::MAX`.
    pub owner: u32,
}

/// The rule values of heights (proposed `plan.json` block `heights`).
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// The steepest grade that a feather adds, over its mean grade: the greatest slope of `phi`.
    pub blend_steep: f64,
    /// Frame health: none within this of the mitre crossing, full this much farther.
    pub health_margin_m: f64,
    pub health_width_m: f64,
    /// The cones fade into the terrain over this share of the reach.
    pub fade_share: f64,
    /// How fast a cone lets go in its fade: halfway through the fade it has moved this far.
    pub fade_gap_m: f64,
    /// The toe and crest are rounded by a smooth min/max of this width in height per metre of the
    /// distance from the outline, at most `toe_max_m`: about `toe_max_m` / (cone slope − terrain
    /// slope) metres wide, so a level-16 mesh cell (6.5 m) holds the curve and no crease saws.
    pub toe_per_m: f64,
    pub toe_max_m: f64,
    /// The feather of a line is its slope reach, at least this.
    pub feather_min_m: f64,
    /// Within this many metres of the slope band's outer edge the slope fades into the terrain.
    pub band_fade_m: f64,
    /// The slope's foot at the outline is rounded over this many metres.
    pub round_m: f64,
}

pub const PARAMS: Params = Params {
    blend_steep: 1.5,
    health_margin_m: 0.25,
    health_width_m: 1.0,
    fade_share: 0.3,
    fade_gap_m: 1.0,
    toe_per_m: 1.0,
    toe_max_m: 5.0,
    feather_min_m: 1.0,
    round_m: 0.5,
    band_fade_m: 1.0,
};

/// The most lines that heights keeps at one point, and the most quads of one line; the GPU port
/// holds the same.
pub const SLOTS: usize = 8;
pub const QUADS: usize = 4;
/// Where quads of one line overlap (two legs, a fold), the line's surface weights each by
/// exp(−metres across / this): exact away from the middle between them, a narrow ramp across it.
pub const LEG_BLEND_M: f64 = 0.25;

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// 1 at 0, 0 at 1 and past it, with zero slope at both ends.
fn phi(x: f64) -> f64 {
    1.0 - smoothstep(x)
}

fn smin(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k / 4.0
}

fn smax(a: f64, b: f64, k: f64) -> f64 {
    -smin(-a, -b, k)
}

fn f32w(w: u32) -> f64 {
    f64::from(f32::from_bits(w))
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: [f64; 2]) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: [f64; 2]) -> [f64; 2] {
    let n = norm(a);
    if n > 0.0 {
        [a[0] / n, a[1] / n]
    } else {
        [1.0, 0.0]
    }
}

/// The block's words as heights reads them.
struct Block<'a> {
    w: &'a [u32],
    mpu: f64,
}

#[derive(Clone, Copy, Debug)]
struct Line {
    first: usize,
    count: usize,
    cut: f64,
    fill: f64,
    reach: f64,
    class: u32,
    /// Shoulder plus verge, metres.
    side: f64,
    ret: f64,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    p: [f64; 2],
    /// Half-width, metres.
    h: f64,
    z: f64,
    bank: f64,
    grade: f64,
}

impl Block<'_> {
    fn line(&self, l: usize) -> Line {
        let w = &self.w[self.w[6] as usize + LINE * l..][..LINE];
        Line {
            first: w[2] as usize,
            count: w[3] as usize,
            cut: f32w(w[4]),
            fill: f32w(w[5]),
            reach: f32w(w[6]),
            class: w[7],
            side: f32w(w[8]) + f32w(w[9]),
            ret: f32w(w[10]),
        }
    }

    fn sample(&self, ln: &Line, i: usize) -> Sample {
        let w = &self.w[self.w[7] as usize + SAMPLE * (ln.first + i)..][..SAMPLE];
        Sample {
            p: [f64::from(w[0] as i32), f64::from(w[1] as i32)],
            h: f32w(w[2]) * self.mpu,
            z: f32w(w[5]),
            bank: f32w(w[6]),
            grade: f32w(w[7]),
        }
    }

    /// The mitre direction at sample i (unit): the bisector of its two chords.
    fn mitre(&self, ln: &Line, i: usize) -> [f64; 2] {
        let p = |k: usize| self.sample(ln, k).p;
        let mut d = [0.0, 0.0];
        if i > 0 {
            let a = unit(sub(p(i), p(i - 1)));
            d = [d[0] + a[0], d[1] + a[1]];
        }
        if i + 1 < ln.count {
            let b = unit(sub(p(i + 1), p(i)));
            d = [d[0] + b[0], d[1] + b[1]];
        }
        unit(d)
    }

    /// Where the mitre lines of samples i and i+1 cross: the signed offset along the left normal of
    /// i's mitre (frame units), or infinity.
    fn crossing(&self, ln: &Line, i: usize) -> f64 {
        let (a, b) = (self.sample(ln, i).p, self.sample(ln, i + 1).p);
        let (ta, tb) = (self.mitre(ln, i), self.mitre(ln, i + 1));
        // Left normals in the frame (x east, y south).
        let (ma, mb) = ([ta[1], -ta[0]], [tb[1], -tb[0]]);
        let den = cross(ma, mb);
        if den.abs() < 1e-12 {
            return f64::INFINITY;
        }
        cross(sub(b, a), mb) / den
    }

    /// The health radius on a side (+1 left, −1 right) at sample i: the nearest crossing on that
    /// side of the mitre lines of the two quads that meet there (frame units).
    fn rho(&self, ln: &Line, i: usize, side: f64) -> f64 {
        let mut r: f64 = 1e9;
        if i > 0 {
            let c = self.crossing(ln, i - 1) * side;
            if c > 0.0 {
                r = r.min(c);
            }
        }
        if i + 1 < ln.count {
            let c = self.crossing(ln, i) * side;
            if c > 0.0 {
                r = r.min(c);
            }
        }
        r
    }
}

fn hermite(z0: f64, z1: f64, m0: f64, m1: f64, t: f64) -> f64 {
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * z0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * z1 + (t3 - t2) * m1
}

/// A line's surface at a point, from one of its segments.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// Its own height there (metres).
    z: f64,
    /// Metres across (signed, + left).
    delta: f64,
    /// Metres past its core (carriageway and shoulder), 0 inside.
    d: f64,
    /// Frame health in [0, 1].
    nu: f64,
}

/// Segment i of a line at q: in its mitre quad, or past the line's first or last sample along the end
/// chord with the end grade. None where the quad does not hold q.
fn segment(b: &Block, ln: &Line, i: usize, q: [f64; 2], p: &Params) -> Option<Frame> {
    let (sa, sb) = (b.sample(ln, i), b.sample(ln, i + 1));
    let (ta, tb) = (b.mitre(ln, i), b.mitre(ln, i + 1));
    let da = dot(sub(q, sa.p), ta);
    let db = dot(sub(sb.p, q), tb);
    let (first, last) = (i == 0, i + 2 == ln.count);
    let ab = sub(sb.p, sa.p);
    let len = norm(ab);
    if len <= 0.0 {
        return None;
    }
    let u = [ab[0] / len, ab[1] / len];
    // Positive on the left of the direction: the frame's y points south.
    let lateral = -cross(u, sub(q, sa.p)) * b.mpu;
    let len_m = len * b.mpu;
    let (t, z, delta, d, nu);
    if da >= 0.0 && db >= 0.0 {
        t = if da + db > 0.0 { da / (da + db) } else { 0.5 };
        z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, t);
        // The distance to the segment, signed by side: both quads give it on their mitre line.
        let f = (dot(sub(q, sa.p), u) / len).clamp(0.0, 1.0);
        let seg = norm(sub(q, [sa.p[0] + f * ab[0], sa.p[1] + f * ab[1]])) * b.mpu;
        delta = if lateral >= 0.0 { seg } else { -seg };
        let h = sa.h + t * (sb.h - sa.h);
        d = (seg - h - ln.side).max(0.0);
        let side = if lateral >= 0.0 { 1.0 } else { -1.0 };
        // Interpolated as a curvature, so a straight neighbour (no crossing) does not swamp it.
        let r = b.mpu / ((1.0 - t) / b.rho(ln, i, side) + t / b.rho(ln, i + 1, side));
        nu = smoothstep((r - seg - p.health_margin_m) / p.health_width_m);
    } else if first && da < 0.0 && db >= 0.0 {
        let past = -dot(sub(q, sa.p), u) * b.mpu;
        t = 0.0;
        z = sa.z - sa.grade * past;
        delta = lateral;
        d = ((delta * delta + past * past).sqrt() - sa.h - ln.side).max(0.0);
        // The end vertex's health, as the first quad has it on the end's mitre line.
        let side = if lateral >= 0.0 { 1.0 } else { -1.0 };
        nu = smoothstep((b.rho(ln, i, side) * b.mpu - lateral.abs() - p.health_margin_m) / p.health_width_m);
    } else if last && db < 0.0 && da >= 0.0 {
        let past = dot(sub(q, sb.p), u) * b.mpu;
        t = 1.0;
        z = sb.z + sb.grade * past;
        delta = lateral;
        d = ((delta * delta + past * past).sqrt() - sb.h - ln.side).max(0.0);
        let side = if lateral >= 0.0 { 1.0 } else { -1.0 };
        nu = smoothstep((b.rho(ln, i + 1, side) * b.mpu - lateral.abs() - p.health_margin_m) / p.health_width_m);
    } else {
        return None;
    }
    let bank = sa.bank + t * (sb.bank - sa.bank);
    Some(Frame { z: z + bank * delta, delta, d, nu })
}

/// One line present at a point.
#[derive(Clone, Copy, Debug)]
struct Rec {
    line: u32,
    class: u32,
    z: f64,
    d: f64,
    nu: f64,
    /// Distance past its core outline, with the core arcs of its corners.
    out: f64,
    feather: f64,
    reach: f64,
    ret: f64,
    cut: f64,
    fill: f64,
    /// The segments of the line whose quads hold q, each (own height, metres past the core, health):
    /// the slope takes a cone from each, so two legs of one way give two cones.
    quads: [(f64, f64, f64); QUADS],
    nq: usize,
    /// Per other record (by its index after sorting), how much a fan relates the two at q: 1 out to
    /// the greater feather less 1 m past the fan's circle, 0 half a metre past that feather.
    rel: [f64; SLOTS],
}

/// The distance from v (relative to an arc's centre) to the arc of radius r from direction a to
/// direction c (the shorter way, under a half turn): radial where v points between them, else to the
/// nearer end.
fn arc_distance(v: [f64; 2], a: [f64; 2], c: [f64; 2], r: f64) -> f64 {
    let turn = cross(a, c);
    let inside = cross(a, v) * turn >= 0.0 && cross(v, c) * turn >= 0.0 && dot(v, [a[0] + c[0], a[1] + c[1]]) >= 0.0;
    if inside {
        return (norm(v) - r).abs();
    }
    let (ea, ec) = (unit(a), unit(c));
    norm(sub(v, [ea[0] * r, ea[1] * r])).min(norm(sub(v, [ec[0] * r, ec[1] * r])))
}

/// The items of q's bin: its height segments and its discs, as word ranges.
fn bin_items(b: &Block, q: [f64; 2]) -> Option<(usize, usize, usize, usize)> {
    let (col, row) = ((q[0] / f64::from(BIN_UNITS)).floor() as i32, (q[1] / f64::from(BIN_UNITS)).floor() as i32);
    if !(0..BINS).contains(&col) || !(0..BINS).contains(&row) {
        return None;
    }
    let w = b.w;
    let head = w[9] as usize + HEADER * (row * BINS + col) as usize;
    let mut at = w[head] as usize;
    if at == 0 {
        return None;
    }
    let (n_cands, n_segs) = ((w[head + 1] >> 16) as usize, (w[head + 1] & 0xffff) as usize);
    let (n_h, n_d) = ((w[head + 2] >> 16) as usize, (w[head + 2] & 0xffff) as usize);
    for _ in 0..n_cands {
        at += 2 + 2 * w[at + 1] as usize;
    }
    at += n_segs;
    Some((at, n_h, at + n_h, n_d))
}

/// Metres from q to the outer edge of the slope band (the band layer with surface slope), from the
/// edges of q's bin (clipped to the bin grown by 3 m): exact within 3 m, else 3 m or more.
fn slope_edge(b: &Block, q: [f64; 2]) -> f64 {
    let (col, row) = ((q[0] / f64::from(BIN_UNITS)).floor() as i32, (q[1] / f64::from(BIN_UNITS)).floor() as i32);
    if !(0..BINS).contains(&col) || !(0..BINS).contains(&row) {
        return f64::INFINITY;
    }
    let w = b.w;
    let head = w[9] as usize + HEADER * (row * BINS + col) as usize;
    let mut at = w[head] as usize;
    if at == 0 {
        return f64::INFINITY;
    }
    let rq = [q[0] - f64::from(reference(col)), q[1] - f64::from(reference(row))];
    let mut best = f64::INFINITY;
    for _ in 0..(w[head + 1] >> 16) {
        let ci = (w[at] & 0xffff) as usize;
        let n = w[at + 1] as usize;
        let cand = &w[w[4] as usize + CAND * ci..][..CAND];
        let slope = cand[0] == BAND
            && cand[3] != u32::MAX
            && w[w[5] as usize + LABEL * cand[3] as usize] & 0xff == Surface::Slope as u32;
        if slope {
            for e in 0..n {
                let pa = w[at + 2 + 2 * e];
                let pb = w[at + 3 + 2 * e];
                let a = [f64::from(pa as u16 as i16), f64::from((pa >> 16) as u16 as i16)];
                let c = [f64::from(pb as u16 as i16), f64::from((pb >> 16) as u16 as i16)];
                let ab = sub(c, a);
                let l2 = dot(ab, ab);
                let f = if l2 > 0.0 { (dot(sub(rq, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
                best = best.min(norm(sub(rq, [a[0] + f * ab[0], a[1] + f * ab[1]])));
            }
        }
        at += 2 + 2 * n;
    }
    best * b.mpu
}

/// The lines present at q: per line, its segment nearest q (least metres across) among those whose
/// quad holds q with some health, kept while within its feather or its reach.
fn present(b: &Block, q: [f64; 2], p: &Params) -> Vec<Rec> {
    let Some((hs, nh, ds, nd)) = bin_items(b, q) else { return Vec::new() };
    let mut out: Vec<Rec> = Vec::new();
    let mut k = 0;
    while k < nh {
        let l = (b.w[hs + k] & 0xffff) as usize;
        let ln = b.line(l);
        let mut quads = [(0.0, 0.0, 0.0); QUADS];
        let mut deltas = [0.0; QUADS];
        let mut nq = 0;
        // Every listed segment by its nearest point: the least distance, and the fallback surface (a
        // smooth mean of the segments' surfaces at their nearest points) where no healthy quad is near.
        let (mut least, mut fb_w, mut fb_z) = (f64::INFINITY, 0.0, 0.0);
        let mut core_d = f64::INFINITY;
        while k < nh && (b.w[hs + k] & 0xffff) as usize == l {
            let i = (b.w[hs + k] >> 16) as usize;
            k += 1;
            if let Some(f) = segment(b, &ln, i, q, p) {
                if f.nu > 0.0 && nq < QUADS {
                    quads[nq] = (f.z, f.d, f.nu);
                    deltas[nq] = f.delta.abs();
                    nq += 1;
                }
            }
            let (sa, sb) = (b.sample(&ln, i), b.sample(&ln, i + 1));
            let ab = sub(sb.p, sa.p);
            let l2 = dot(ab, ab);
            let f = if l2 > 0.0 { (dot(sub(q, sa.p), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let dist = norm(sub(q, [sa.p[0] + f * ab[0], sa.p[1] + f * ab[1]])) * b.mpu;
            least = least.min(dist);
            core_d = core_d.min(dist - (sa.h + f * (sb.h - sa.h)) - ln.side);
            let len_m = l2.sqrt() * b.mpu;
            let side = if cross(ab, sub(q, sa.p)) <= 0.0 { 1.0 } else { -1.0 };
            let z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, f)
                + (sa.bank + f * (sb.bank - sa.bank)) * side * dist;
            // Support within the segment's own band (its half-width, side and reach): every bin
            // that q can lie in lists it.
            let support = (sa.h + f * (sb.h - sa.h)) + ln.side + ln.reach;
            let psi = (1.0 / (dist * dist + 0.25) - 1.0 / (support * support + 0.25)).max(0.0);
            fb_w += psi;
            fb_z += psi * z;
        }
        let core_d = core_d.max(0.0);
        let feather = ln.reach.max(p.feather_min_m);
        if core_d >= feather.max(ln.reach) || out.len() == SLOTS || (nq == 0 && fb_w <= 0.0) {
            continue;
        }
        // The line's surface: each healthy quad weighs its health times exp(−(its distance − the
        // least) / LEG_BLEND_M), so a far leg weighs nothing; the fallback takes the rest of 1.
        let (mut sa_w, mut z, mut d) = (0.0, 0.0, 0.0);
        for j in 0..nq {
            let a = quads[j].2 * libm::exp(-(deltas[j] - least).max(0.0) / LEG_BLEND_M);
            sa_w += a;
            z += a * quads[j].0;
            d += a * quads[j].1;
        }
        let health = sa_w.min(1.0);
        let fb = (1.0 - sa_w).max(0.0);
        let zf = if fb_w > 0.0 { fb_z / fb_w } else { 0.0 };
        // The distance past the core is the plain one to the nearest segment: 0 exactly inside.
        let _ = d;
        let (z, d) = ((z + fb * zf) / (sa_w + fb), core_d);
        out.push(Rec {
            line: l as u32,
            class: ln.class,
            z,
            d,
            nu: health,
            out: f64::INFINITY,
            feather,
            reach: ln.reach,
            ret: ln.ret,
            cut: ln.cut,
            fill: ln.fill,
            quads,
            nq,
            rel: [0.0; SLOTS],
        });
    }
    let mut pairs: Vec<(u32, u32, f64)> = Vec::new();
    // The core arcs of the corners: a disc's arc between its feet, at its radius less its ring.
    let extras = b.w[8] as usize;
    for k in 0..nd {
        let item = b.w[ds + k];
        let cand = &b.w[b.w[4] as usize + CAND * (item & 0xffff) as usize..][..CAND];
        let e = &b.w[extras + cand[5] as usize + DISC * (item >> 16) as usize..][..DISC];
        let c = [f64::from(e[0] as i32), f64::from(e[1] as i32)];
        let u0 = sub([f64::from(e[3] as i32), f64::from(e[4] as i32)], c);
        let u1 = sub([f64::from(e[5] as i32), f64::from(e[6] as i32)], c);
        let v = sub(q, c);
        let (l0, l1) = (e[9], e[10]);
        // The fan relates its two arms near it.
        let past = norm(v) * b.mpu - f32w(e[2]) * b.mpu;
        if l0 != u32::MAX && l1 != u32::MAX && l0 != l1 {
            pairs.push((l0, l1, past));
        }
        // The ring's outer arc lies at the wider side's ring; at a narrower side it tapers back to
        // that arm's own band along a circle of the wider ring about the foot (GP3's band rings).
        let (ring0, ring1) = (f32w(e[7]), f32w(e[8]));
        let wide = ring0.max(ring1);
        let radius = f32w(e[2]) * b.mpu;
        // The corner's outline: the arc of radius r − ring between the feet, and at a narrower foot
        // GP3's taper, an arc of the wider ring about the foot from the ring's end to that arm's own
        // band. The distance is the unsigned distance to these arcs: continuous everywhere.
        let mut d = arc_distance(v, u0, u1, (radius - wide).max(0.0) / b.mpu) * b.mpu;
        for (foot, other, own) in [(u0, u1, ring0), (u1, u0, ring1)] {
            if own >= wide {
                continue;
            }
            let n = unit([-foot[0], -foot[1]]);
            let mut u = [n[1], -n[0]];
            if dot(u, sub(other, foot)) > 0.0 {
                u = [-u[0], -u[1]];
            }
            let phi_end = libm::acos(own / wide);
            let end = [
                libm::cos(phi_end) * n[0] + libm::sin(phi_end) * u[0],
                libm::cos(phi_end) * n[1] + libm::sin(phi_end) * u[1],
            ];
            d = d.min(arc_distance(sub(v, foot), n, end, wide / b.mpu) * b.mpu);
        }
        for rec in out.iter_mut() {
            let mine = rec.line == l0 || rec.line == l1;
            if mine && d < rec.out {
                rec.out = d;
            }
        }
    }
    // The order of the painter: class rank (rails first), then the line.
    out.sort_by_key(|r| (r.class, r.line));
    for (l0, l1, past) in pairs {
        let (Some(i), Some(j)) = (out.iter().position(|r| r.line == l0), out.iter().position(|r| r.line == l1)) else {
            continue;
        };
        // Related out to the pair's feather past the circle, and not past the bins' listing.
        let reach = out[i].feather.max(out[j].feather);
        let w = phi((past - (reach - 1.0)) / 1.5);
        out[i].rel[j] = out[i].rel[j].max(w);
        out[j].rel[i] = out[j].rel[i].max(w);
    }
    out
}

/// The paved field for owner o (an index into `recs`): the normalized priority painter of the
/// records that agree with o's surface.
fn paved(recs: &[Rec], o: usize, p: &Params) -> f64 {
    paved_as(recs, o, recs[o].z, recs[o].d, p)
}

/// The paved field for owner o with o's own height and distance taken from one of its quads.
fn paved_as(recs: &[Rec], o: usize, z: f64, d: f64, p: &Params) -> f64 {
    let n = recs.len();
    let mut rs = [recs[0]; SLOTS];
    rs[..n].copy_from_slice(recs);
    (rs[o].z, rs[o].d) = (z, d);
    let beta: Vec<f64> = rs[..n].iter().map(|r| phi(r.d / r.feather)).collect();
    // The pair's agreement: a fan relates them, or the rise between them is one that the lesser
    // one's return grade climbs over the greater one's feather.
    let agree = |i: usize, j: usize| {
        let (upper, lower) = if i < j { (&rs[i], &rs[j]) } else { (&rs[j], &rs[i]) };
        let wall = lower.ret * upper.feather / p.blend_steep;
        let ok = wall / 2.0;
        phi(((rs[i].z - rs[j].z).abs() - ok) / (wall - ok)).max(rs[i].rel[j])
    };
    // A greater record i rules a lesser j as far as they agree; inside i's core it rules it fully.
    // A lesser record j keeps a greater i off its ground as far as they disagree, except inside i's
    // core. So agreeing records blend, and disagreeing ones meet in a step on the greater one's core
    // edge.
    let (mut sw, mut swz) = (0.0, 0.0);
    for j in 0..n {
        let mut w = beta[j];
        for i in 0..n {
            if i == j {
                continue;
            }
            let c = agree(i, j);
            if i < j {
                w *= if rs[i].d <= 0.0 { 0.0 } else { 1.0 - beta[i] * c };
            } else if rs[j].d > 0.0 {
                w *= 1.0 - beta[i] * (1.0 - c);
            }
        }
        sw += w;
        swz += w * rs[j].z;
    }
    if sw > 0.0 {
        swz / sw
    } else {
        rs[o].z
    }
}

/// A cone's rise over metres d past the outline at slope s: rounded over the first `round_m`, so the
/// slope meets the paved field with no crease and a boundary a few centimetres off the true arc meets
/// it with no step.
fn cone(s: f64, d: f64, p: &Params) -> f64 {
    let a = p.round_m;
    if d < a {
        s * d * d / (2.0 * a)
    } else {
        s * (d - a / 2.0)
    }
}

/// The slope field: the fill and cut cones of every line present, on its paved field, faded into
/// the terrain at the reach; then `min(cut, max(T, fill))`, smooth at the toe and crest by a width
/// that grows from 0 at the nearest outline.
fn slope(b: &Block, q: [f64; 2], recs: &[Rec], t: f64, edge: f64, p: &Params) -> (f64, u32) {
    let (mut lo, mut hi, mut toe) = (f64::NEG_INFINITY, f64::INFINITY, p.toe_max_m);
    let (mut lo_line, mut hi_line) = (u32::MAX, u32::MAX);
    for (k, r) in recs.iter().enumerate() {
        if r.reach <= 0.0 {
            continue;
        }
        for &(z, dq, nu) in &r.quads[..r.nq] {
            let d = dq.min(r.out);
            let base = paved_as(recs, k, z, dq, p);
            let w = r.reach * p.fade_share;
            // The fade reads the quad's own distance: a corner's arc can bring the cone's foot nearer,
            // never keep a quad alive past its reach (where the line leaves the point's records).
            let fade = smoothstep((dq - (r.reach - w)) / w).max(1.0 - nu);
            let fill = base - cone(r.fill, d, p);
            let cut = base + cone(r.cut, d, p);
            // Near the reach a cone lets go: the fill sinks and the cut rises without bound, so at
            // the reach the ground is the terrain and a fading cut never digs into another way's fill.
            let relax = p.fade_gap_m * fade / (1.0 - fade).max(1e-6);
            // The toe's width grows from 0 at this cone's outline; a fading cone leaves it alone.
            let kq = (p.toe_per_m * d).min(p.toe_max_m);
            toe = toe.min(kq + fade * (p.toe_max_m - kq));
            if fill - relax > lo {
                (lo, lo_line) = (fill - relax, r.line);
            }
            if cut + relax < hi {
                (hi, hi_line) = (cut + relax, r.line);
            }
        }
    }
    // Where a line's frame is not healthy (past the crossing of its mitre lines on a bend's inner
    // side, where no quad holds q), each of its segments gives a cone from its nearest point instead:
    // continuous everywhere, and let go where the frame is healthy.
    if let Some((hs, nh, _, _)) = bin_items(b, q) {
        for k in 0..nh {
            let l = b.w[hs + k] & 0xffff;
            let Some(r) = recs.iter().find(|r| r.line == l) else { continue };
            if r.nu >= 1.0 || r.reach <= 0.0 {
                continue;
            }
            let ln = b.line(l as usize);
            let i = (b.w[hs + k] >> 16) as usize;
            let (sa, sb) = (b.sample(&ln, i), b.sample(&ln, i + 1));
            let ab = sub(sb.p, sa.p);
            let l2 = dot(ab, ab);
            let f = if l2 > 0.0 { (dot(sub(q, sa.p), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let foot = [sa.p[0] + f * ab[0], sa.p[1] + f * ab[1]];
            let dist = norm(sub(q, foot)) * b.mpu;
            let side = if cross(ab, sub(q, sa.p)) <= 0.0 { 1.0 } else { -1.0 };
            let d = (dist - (sa.h + f * (sb.h - sa.h)) - ln.side).max(0.0);
            if d >= r.reach {
                continue;
            }
            let len_m = l2.sqrt() * b.mpu;
            let z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, f)
                + (sa.bank + f * (sb.bank - sa.bank)) * side * dist;
            let w = r.reach * p.fade_share;
            let fade = smoothstep((d - (r.reach - w)) / w).max(r.nu);
            let relax = p.fade_gap_m * fade / (1.0 - fade).max(1e-6);
            let kq = (p.toe_per_m * d).min(p.toe_max_m);
            toe = toe.min(kq + fade * (p.toe_max_m - kq));
            let d = d.min(r.out);
            if z - cone(r.fill, d, p) - relax > lo {
                (lo, lo_line) = (z - cone(r.fill, d, p) - relax, r.line);
            }
            if z + cone(r.cut, d, p) + relax < hi {
                (hi, hi_line) = (z + cone(r.cut, d, p) + relax, r.line);
            }
        }
    }
    if lo == f64::NEG_INFINITY {
        return (t, u32::MAX);
    }
    let g = smin(hi, smax(t, lo, toe), toe);
    // The slope band's outer edge is the terrain's: the ground meets it there whatever the band's
    // shape (corner rings, square ends at a structure).
    let g = t + smoothstep(edge / p.band_fade_m) * (g - t);
    // The line whose cone holds the ground, for the debug tool.
    let by = if (g - hi).abs() < (g - t).abs().min((g - lo).abs()) {
        hi_line
    } else if (g - lo).abs() < (g - t).abs() {
        lo_line
    } else {
        u32::MAX
    };
    (g, by)
}

/// The height at q (frame units from the cell's origin), with the label there (`records::find`) and
/// the plain terrain t there.
pub fn height_at(words: &[u32], q: [f64; 2], label: Option<u32>, t: f64) -> Height {
    let p = &PARAMS;
    let b = Block { w: words, mpu: f32w(words[10]) };
    let lw = |k: usize| label.map_or(u32::MAX, |id| words[words[5] as usize + LABEL * id as usize + k]);
    let rule = if label.is_some() { lw(0) >> 24 } else { 0 };
    if rule == 0 {
        return Height { h: t, kind: Kind::Terrain, owner: u32::MAX };
    }
    let recs = present(&b, q, p);
    if rule == 1 {
        let (h, by) = slope(&b, q, &recs, t, slope_edge(&b, q), p);
        return Height { h, kind: Kind::Slope, owner: by };
    }
    let material = lw(2);
    // The owner: the first record whose core holds q (the shape's order puts carriageways first),
    // else the label's material line, else the nearest. A junction region's points on a guest arm
    // so belong to that arm, not to the region's material line far away.
    let o = recs
        .iter()
        .position(|r| r.d <= 0.0)
        .or_else(|| recs.iter().position(|r| r.line == material))
        .or_else(|| (0..recs.len()).min_by(|&a, &c| recs[a].d.total_cmp(&recs[c].d)));
    match o {
        Some(o) => Height { h: paved(&recs, o, p), kind: Kind::Paved, owner: recs[o].line },
        None => Height { h: t, kind: Kind::Paved, owner: u32::MAX },
    }
}

/// The records present at q, for the debug tool: line, class, own height, metres past the core,
/// health, metres past the core outline.
pub fn explain(words: &[u32], q: [f64; 2]) -> Vec<(u32, u32, f64, f64, f64, f64)> {
    let b = Block { w: words, mpu: f32w(words[10]) };
    present(&b, q, &PARAMS)
        .iter()
        .flat_map(|r| r.quads[..r.nq].iter().map(move |qd| (r.line, r.class, qd.0, qd.1, qd.2, r.out)))
        .collect()
}

/// Every segment of q's bin whose quad holds q: line, segment, metres across, health, health radius
/// at both ends (metres), for the debug tool.
pub fn explain_quads(words: &[u32], q: [f64; 2]) -> Vec<(u32, usize, f64, f64, f64, f64)> {
    let b = Block { w: words, mpu: f32w(words[10]) };
    let Some((hs, nh, _, _)) = bin_items(&b, q) else { return Vec::new() };
    let mut out = Vec::new();
    for k in 0..nh {
        let l = b.w[hs + k] & 0xffff;
        let i = (b.w[hs + k] >> 16) as usize;
        let ln = b.line(l as usize);
        if let Some(f) = segment(&b, &ln, i, q, &PARAMS) {
            let side = if f.delta >= 0.0 { 1.0 } else { -1.0 };
            out.push((l, i, f.delta, f.nu, b.rho(&ln, i, side) * b.mpu, b.rho(&ln, i + 1, side) * b.mpu));
        }
    }
    out
}

/// The discs of q's bin: arm lines, radius, ring widths, and q's metres from the centre, for the
/// debug tool.
pub fn explain_discs(words: &[u32], q: [f64; 2]) -> Vec<(u32, u32, f64, f64, f64, f64, bool)> {
    let b = Block { w: words, mpu: f32w(words[10]) };
    let Some((_, _, ds, nd)) = bin_items(&b, q) else { return Vec::new() };
    let extras = b.w[8] as usize;
    (0..nd)
        .map(|k| {
            let item = b.w[ds + k];
            let cand = &b.w[b.w[4] as usize + CAND * (item & 0xffff) as usize..][..CAND];
            let e = &b.w[extras + cand[5] as usize + DISC * (item >> 16) as usize..][..DISC];
            let c = [f64::from(e[0] as i32), f64::from(e[1] as i32)];
            let u0 = sub([f64::from(e[3] as i32), f64::from(e[4] as i32)], c);
            let u1 = sub([f64::from(e[5] as i32), f64::from(e[6] as i32)], c);
            let v = sub(q, c);
            let turn = cross(u0, u1);
            let inside = cross(u0, v) * turn >= 0.0
                && cross(v, u1) * turn >= 0.0
                && dot(v, [u0[0] + u1[0], u0[1] + u1[1]]) >= 0.0;
            (e[9], e[10], f32w(e[2]) * b.mpu, f32w(e[7]), f32w(e[8]), norm(v) * b.mpu, inside)
        })
        .collect()
}
