//! Heights of the one road model: one point function over a cell's records (`records`), for the near
//! mesh's vertices (here) and the far stamp's texels (`terrain/heights.wgsl`, a port line for line).
//! Prototype of the one road model; not for merge.
//!
//! Per point, each line near it gives its own surface (its curve's height on the mitre-quad frame plus
//! bank times the metres across), its distance past its core and a presence that falls from 1 at the
//! core's edge to 0 at its feather length. The paved field is a normalized priority painter of these
//! surfaces, gated by their agreement with the owner's surface: inside the core of the first record
//! present it is that record's surface exactly; a compatible lesser record ramps to it; an
//! incompatible one keeps its own surface and meets it in a step on the greater record's core edge.
//! Slopes are fill and cut cones from each line's core outline (with the corner arcs of its fans), on
//! the paved field, faded into the terrain at the reach: `clamp(T, max fill, min cut)`, cut first.

use super::records::{BINS, BIN_UNITS, CAND, DISC, HEADER, LABEL, LINE, SAMPLE};

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
    /// The owner line of a paved point, else `u32::MAX`.
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
    /// The width of the smooth toe and crest.
    pub toe_m: f64,
    /// The feather of a line is its slope reach, at least this.
    pub feather_min_m: f64,
    /// The slope's foot at the outline is rounded over this many metres.
    pub round_m: f64,
}

pub const PARAMS: Params = Params {
    blend_steep: 1.5,
    health_margin_m: 0.25,
    health_width_m: 1.0,
    fade_share: 0.3,
    fade_gap_m: 1.0,
    toe_m: 0.6,
    feather_min_m: 1.0,
    round_m: 0.5,
};

/// The most lines that heights keeps at one point; the GPU port holds the same.
pub const SLOTS: usize = 8;

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
        let r = ((1.0 - t) * b.rho(ln, i, side) + t * b.rho(ln, i + 1, side)) * b.mpu;
        nu = smoothstep((r - seg - p.health_margin_m) / p.health_width_m);
    } else if first && da < 0.0 && db >= 0.0 {
        let past = -dot(sub(q, sa.p), u) * b.mpu;
        t = 0.0;
        z = sa.z - sa.grade * past;
        delta = lateral;
        d = ((delta * delta + past * past).sqrt() - sa.h - ln.side).max(0.0);
        nu = 1.0;
    } else if last && db < 0.0 && da >= 0.0 {
        let past = dot(sub(q, sb.p), u) * b.mpu;
        t = 1.0;
        z = sb.z + sb.grade * past;
        delta = lateral;
        d = ((delta * delta + past * past).sqrt() - sb.h - ln.side).max(0.0);
        nu = 1.0;
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
    /// Per other record (by its index after sorting), how much a fan relates the two at q: 1 out to
    /// the greater feather less 1 m past the fan's circle, 0 half a metre past that feather.
    rel: [f64; SLOTS],
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

/// The lines present at q: per line, its segment nearest q (least metres across) among those whose
/// quad holds q with some health, kept while within its feather or its reach.
fn present(b: &Block, q: [f64; 2], p: &Params) -> Vec<Rec> {
    let Some((hs, nh, ds, nd)) = bin_items(b, q) else { return Vec::new() };
    let mut out: Vec<Rec> = Vec::new();
    let mut k = 0;
    while k < nh {
        let l = (b.w[hs + k] & 0xffff) as usize;
        let ln = b.line(l);
        let mut best: Option<Frame> = None;
        while k < nh && (b.w[hs + k] & 0xffff) as usize == l {
            let i = (b.w[hs + k] >> 16) as usize;
            k += 1;
            if let Some(f) = segment(b, &ln, i, q, p) {
                if f.nu > 0.0 && best.is_none_or(|g| f.delta.abs() < g.delta.abs()) {
                    best = Some(f);
                }
            }
        }
        let Some(f) = best else { continue };
        let feather = ln.reach.max(p.feather_min_m);
        if f.d >= feather.max(ln.reach) || out.len() == SLOTS {
            continue;
        }
        out.push(Rec {
            line: l as u32,
            class: ln.class,
            z: f.z,
            d: f.d,
            nu: f.nu,
            out: f.d,
            feather,
            reach: ln.reach,
            ret: ln.ret,
            cut: ln.cut,
            fill: ln.fill,
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
        let turn = cross(u0, u1);
        if cross(u0, v) * turn < 0.0 || cross(v, u1) * turn < 0.0 || dot(v, [u0[0] + u1[0], u0[1] + u1[1]]) < 0.0 {
            continue;
        }
        let whole = libm::atan2(turn.abs(), dot(u0, u1));
        let part = libm::atan2(cross(u0, v).abs(), dot(u0, v));
        let f = if whole > 0.0 { (part / whole).clamp(0.0, 1.0) } else { 0.0 };
        let ring = f32w(e[7]) + f * (f32w(e[8]) - f32w(e[7]));
        // Only the land inside the round's circle sees its arc; past the circle lie its fan and arms.
        let dist = norm(v) * b.mpu;
        if dist >= f32w(e[2]) * b.mpu {
            continue;
        }
        let d = (f32w(e[2]) * b.mpu - ring - dist).max(0.0);
        for rec in out.iter_mut() {
            let mine = rec.line == l0 || rec.line == l1 || (l0 == u32::MAX && l1 == u32::MAX);
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
    let ro = recs[o];
    let (mut sw, mut swz, mut keep) = (0.0, 0.0, 1.0);
    for (k, r) in recs.iter().enumerate() {
        let beta = phi(r.d / r.feather) * r.nu;
        let c = if k == o {
            1.0
        } else {
            // The pair's gate: the rise that the lesser one's return grade makes over the greater
            // one's feather.
            let (upper, lower) = if k < o { (r, &ro) } else { (&ro, r) };
            let wall = lower.ret * upper.feather / p.blend_steep;
            let ok = wall / 2.0;
            phi(((r.z - ro.z).abs() - ok) / (wall - ok)).max(ro.rel[k])
        };
        let w = beta * c * keep;
        sw += w;
        swz += w * r.z;
        keep *= 1.0 - beta * c;
    }
    if sw > 0.0 {
        swz / sw
    } else {
        ro.z
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
fn slope(recs: &[Rec], t: f64, p: &Params) -> f64 {
    let (mut lo, mut hi, mut near) = (f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY);
    for (k, r) in recs.iter().enumerate() {
        if r.reach <= 0.0 {
            continue;
        }
        let d = r.out;
        near = near.min(d);
        let base = paved(recs, k, p);
        let w = r.reach * p.fade_share;
        let fade = smoothstep((d - (r.reach - w)) / w).max(1.0 - r.nu);
        let fill = base - cone(r.fill, d, p);
        let cut = base + cone(r.cut, d, p);
        // Near the reach a cone lets go: the fill sinks and the cut rises without bound, so at the
        // reach the ground is the terrain and a fading cut never digs into another way's fill.
        let relax = p.fade_gap_m * fade / (1.0 - fade).max(1e-6);
        lo = lo.max(fill - relax);
        hi = hi.min(cut + relax);
    }
    if lo == f64::NEG_INFINITY {
        return t;
    }
    let k = p.toe_m * (near / (2.0 * p.toe_m)).clamp(0.0, 1.0);
    smin(hi, smax(t, lo, k), k)
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
        return Height { h: slope(&recs, t, p), kind: Kind::Slope, owner: u32::MAX };
    }
    let material = lw(2);
    let o = recs
        .iter()
        .position(|r| r.line == material)
        .or_else(|| recs.iter().position(|r| r.d <= 0.0))
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
    present(&b, q, &PARAMS).iter().map(|r| (r.line, r.class, r.z, r.d, r.nu, r.out)).collect()
}
