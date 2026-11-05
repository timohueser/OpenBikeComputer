//! The heights of the one road model: one point function over a cell's records, for the near mesh's
//! vertices and the far stamp's texels (the viewer's `terrain/heights.wgsl`, a port line for line).
//!
//! Per point, each line near it gives its own surface (its curve's height on the mitre-quad frame plus
//! bank times the metres across), its distance past its core and a presence that falls from 1 at the
//! core's edge to 0 at its feather. The paved field is a normalized painter of these surfaces in which
//! each pair of records blends as far as they agree: inside the core of the first record present it is
//! that record's surface exactly; a compatible lesser record ramps to it; an incompatible one keeps its
//! own surface and meets it in a step on the greater record's core edge. Slopes are a fill and a cut
//! cone from each line's core outline (with the corner arcs of its fans), on the paved field, released
//! at the reach: a smooth `min(cut, max(T, fill))`.

pub mod check;

use super::records::{skip_item, BAND, BINS, BIN_UNITS, CAND, DISC, HEADER, LABEL, LINE, MARGIN_M, SAMPLE};
use super::Surface;
use crate::frame::{metres_per_unit_at, CELL_UNITS};

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
    /// The host line of a paved point, the line whose cone holds a slope point, else `u32::MAX`.
    pub owner: u32,
}

/// The rule values of heights.
#[derive(Clone, Copy, Debug)]
struct Params {
    blend_steep: f64,
    health_margin_m: f64,
    health_width_m: f64,
    health_stretch: f64,
    fade_share: f64,
    fade_gap_m: f64,
    toe_per_m: f64,
    toe_max_m: f64,
    toe_span_m: f64,
    toe_gap_min: f64,
    feather_min_m: f64,
    band_fade_m: f64,
    round_m: f64,
    mitre_band_m: f64,
    legs: Legs,
}

/// Two legs of one way at different heights: one line surface over both, blended in a ramp of the
/// leg blend, or the nearer leg alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Legs {
    Ramp,
    Wall,
}

const PARAMS: Params = Params {
    blend_steep: 1.5,
    health_margin_m: 0.25,
    health_width_m: 1.0,
    health_stretch: 1.5,
    fade_share: 0.3,
    fade_gap_m: 1.0,
    toe_per_m: 1.0,
    toe_max_m: 5.0,
    toe_span_m: 6.0,
    toe_gap_min: 0.1,
    feather_min_m: 1.0,
    round_m: 0.5,
    band_fade_m: 1.0,
    mitre_band_m: 2.0,
    legs: Legs::Ramp,
};

/// The most lines that heights keeps at one point, and the most quads of one line; the GPU port
/// holds the same.
pub const SLOTS: usize = 8;
pub const QUADS: usize = 4;
/// A line's surface is a soft nearest of its quads, and of its segments where no healthy quad is near:
/// each weighs exp(−(its metres − the least) / this).
const LEG_BLEND_M: f64 = 0.25;

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// 1 at 0, 0 at 1 and past it, with zero slope at both ends.
fn phi(x: f64) -> f64 {
    1.0 - smoothstep(x)
}

/// The width of the toe's smooth min/max at d metres past the outline: slope `toe_per_m` at 0, level
/// at `toe_max_m` from twice the distance where the slope would reach it, with no kink (a kink of the
/// width is a crease of the ground along the whole slope).
fn toe_width(d: f64, p: &Params) -> f64 {
    let x = (p.toe_per_m * d / (2.0 * p.toe_max_m)).min(1.0);
    p.toe_max_m * (1.0 - (1.0 - x) * (1.0 - x))
}

/// The most toe width that the slope band's outer edge, `edge` metres away, leaves: `toe_per_m` per
/// metre near the edge, unbounded at `MARGIN_M`, past which `slope_edge` is a bound, not a distance.
fn edge_room(edge: f64, p: &Params) -> f64 {
    if edge < MARGIN_M {
        let x = edge / MARGIN_M;
        p.toe_per_m * edge / (1.0 - x * x * x * x)
    } else {
        f64::INFINITY
    }
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

/// The block's words as heights reads them, at one point.
struct Block<'a> {
    w: &'a [u32],
    /// Metres per frame unit at the point: two cells that share it read the same scale.
    mpu: f64,
}

impl<'a> Block<'a> {
    fn at(w: &'a [u32], q: [f64; 2]) -> Self {
        Block { w, mpu: metres_per_unit_at(f64::from(w[10]) * CELL_UNITS as f64 + q[1]) }
    }
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
    /// The class's steepest fill and cut of the no-room rule (rise per metre).
    fill_max: f64,
    cut_max: f64,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    p: [f64; 2],
    /// Half-width, metres.
    h: f64,
    z: f64,
    bank: f64,
    grade: f64,
    /// The plain terrain at the slope's reach on the left and the right (NaN unknown).
    reach_t: [f64; 2],
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
            fill_max: f32w(w[14]),
            cut_max: f32w(w[15]),
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
            // The records hold the right first (frame axes, y south).
            reach_t: [f32w(w[9]), f32w(w[8])],
        }
    }

    fn position(&self, ln: &Line, i: usize) -> [f64; 2] {
        let at = self.w[7] as usize + SAMPLE * (ln.first + i);
        [f64::from(self.w[at] as i32), f64::from(self.w[at + 1] as i32)]
    }

    /// Half-width at sample i, metres.
    fn half(&self, ln: &Line, i: usize) -> f64 {
        f32w(self.w[self.w[7] as usize + SAMPLE * (ln.first + i) + 2]) * self.mpu
    }

    /// The mitre direction at sample i (unit): the bisector of its two chords.
    fn mitre(&self, ln: &Line, i: usize) -> [f64; 2] {
        let w = &self.w[self.w[7] as usize + SAMPLE * (ln.first + i) + 10..][..2];
        [f32w(w[0]), f32w(w[1])]
    }

    /// The health radius on a side (+1 left, −1 right) at sample i, frame units.
    fn rho(&self, ln: &Line, i: usize, side: f64) -> f64 {
        f32w(self.w[self.w[7] as usize + SAMPLE * (ln.first + i) + if side > 0.0 { 12 } else { 13 }])
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
    /// Frame health in [0, 1].
    nu: f64,
    /// The plain terrain where q's side's cone should meet it (`meet_terrain`).
    reach_t: f64,
    /// The plain terrain's grade outward there (`terrain_grade`).
    grade_t: f64,
}

/// The plain terrain's grade outward on the cone's side (rise per metre away from the line), from the
/// plane of `meet_terrain` (NaN unknown).
fn terrain_grade(a: &Sample, b: &Sample, left: f64, t: f64, core: f64, reach: f64) -> f64 {
    let at = |k: usize| a.reach_t[k] + t * (b.reach_t[k] - a.reach_t[k]);
    let (l, r) = (at(0), at(1));
    if l.is_nan() || r.is_nan() {
        // One side known: the plane is level across, as in `meet_terrain`.
        return if l.is_nan() && r.is_nan() { f64::NAN } else { 0.0 };
    }
    (left * l + (1.0 - left) * r - (left * r + (1.0 - left) * l)) / (2.0 * (core + reach))
}

/// The plain terrain where a cone should meet it (where the cone starts to let go), on a plane across
/// the road through the terrain at the reach on the cone's side and on the other side (NaN unknown):
/// exact on a uniform hillside and beside an embankment on flat ground. `left` is the share of the
/// left side in the cone's side: 1 or 0 beside a segment, and turning with the direction round its end,
/// so the value has no jump on the line's extension.
fn meet_terrain(a: &Sample, b: &Sample, left: f64, t: f64, core: f64, reach: f64, p: &Params) -> f64 {
    let at = |k: usize| a.reach_t[k] + t * (b.reach_t[k] - a.reach_t[k]);
    let (l, r) = (at(0), at(1));
    let r = if r.is_nan() { l } else { r };
    let l = if l.is_nan() { r } else { l };
    let (side, other) = (left * l + (1.0 - left) * r, left * r + (1.0 - left) * l);
    side - (side - other) * p.fade_share * reach / (2.0 * (core + reach))
}

/// The share of the left side at a point `lateral` metres left of a line and `dist` metres from it.
fn left_share(lateral: f64, dist: f64) -> f64 {
    if dist > 0.0 {
        (0.5 + 0.5 * lateral / dist).clamp(0.0, 1.0)
    } else {
        0.5
    }
}

/// The frame's health toward one side's health radius `r`, at `seg` metres across toward that side
/// (negative on the other side): 0 within the margin of the crossing, 1 from the width farther and
/// where the stretch r / (r − seg) is at most `health_stretch`. A stretched frame steepens the curve's
/// grade by its stretch, and the steepening jumps on every mitre line. The product of both sides'
/// health is continuous across the centre line.
fn health(r: f64, seg: f64, p: &Params) -> f64 {
    let width = p.health_width_m.max(r / p.health_stretch - p.health_margin_m);
    smoothstep((r - seg - p.health_margin_m) / width)
}

/// Segment i of a line at q, with its two samples read: in its mitre quad, or past the line's first or
/// last sample along the end chord with the end grade. None where the quad does not hold q.
#[allow(clippy::too_many_arguments)]
fn segment_of(b: &Block, ln: &Line, i: usize, sa: &Sample, sb: &Sample, q: [f64; 2], p: &Params) -> Option<Frame> {
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
    // Metres from the segment's frame: the distance across beside it, the radius past an end.
    let (t, z, delta, nu, around);
    if da >= 0.0 && db >= 0.0 {
        t = if da + db > 0.0 { da / (da + db) } else { 0.5 };
        z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, t);
        // The distance to the segment, signed by side: both quads give it on their mitre line.
        let f = (dot(sub(q, sa.p), u) / len).clamp(0.0, 1.0);
        let seg = norm(sub(q, [sa.p[0] + f * ab[0], sa.p[1] + f * ab[1]])) * b.mpu;
        delta = if lateral >= 0.0 { seg } else { -seg };
        around = seg;
        // Interpolated as a curvature, so a straight neighbour (no crossing) does not swamp it.
        let r = |side: f64| b.mpu / ((1.0 - t) / b.rho(ln, i, side) + t / b.rho(ln, i + 1, side));
        nu = health(r(1.0), delta, p) * health(r(-1.0), -delta, p);
    } else if first && da < 0.0 && db >= 0.0 {
        let past = -dot(sub(q, sa.p), u) * b.mpu;
        t = 0.0;
        z = sa.z - sa.grade * past;
        delta = lateral;
        around = (delta * delta + past * past).sqrt();
        // The end vertex's health, as the first quad has it on the end's mitre line.
        nu = health(b.rho(ln, i, 1.0) * b.mpu, delta, p) * health(b.rho(ln, i, -1.0) * b.mpu, -delta, p);
    } else if last && db < 0.0 && da >= 0.0 {
        let past = dot(sub(q, sb.p), u) * b.mpu;
        t = 1.0;
        z = sb.z + sb.grade * past;
        delta = lateral;
        around = (delta * delta + past * past).sqrt();
        nu = health(b.rho(ln, i + 1, 1.0) * b.mpu, delta, p) * health(b.rho(ln, i + 1, -1.0) * b.mpu, -delta, p);
    } else {
        return None;
    }
    let bank = sa.bank + t * (sb.bank - sa.bank);
    let core = sa.h + t * (sb.h - sa.h) + ln.side;
    let reach_t = meet_terrain(sa, sb, left_share(delta, around), t, core, ln.reach, p);
    let grade_t = terrain_grade(sa, sb, left_share(delta, around), t, core, ln.reach);
    // The bank tilts about the chord's line: its distance across agrees on the mitre line with the
    // neighbour's (the mitre line bisects the two chord lines) and goes on smoothly past the quad.
    let mut z = z + bank * lateral;
    if da >= 0.0 && db >= 0.0 {
        // Neighbour quads agree in value on their mitre line, not in slope: each blends with the
        // neighbour's surface carried on past that line, over a band (`mitre_band`).
        let own = (da + db) * b.mpu;
        for (x, has, j) in [(da * b.mpu, i > 0, i.wrapping_sub(1)), (db * b.mpu, i + 2 < ln.count, i + 1)] {
            if has && x < p.mitre_band_m.min(own / 2.0) {
                if let Some((zn, other)) = surface(b, ln, j, q) {
                    z = mitre_band(z, zn, x, own, other, p);
                }
            }
        }
    }
    Some(Frame { z, delta, nu, reach_t, grade_t })
}

/// A quad's own surface `z` at x metres from its mitre line, blended with the neighbour's surface `zn`
/// carried on past it: half each on the line, its own alone from the band's edge, with a C1 weight.
/// The two agree on the line, so the blend is C1 across it whatever the band's width on each side.
/// The band is narrower than half of either quad's length through q (`own`, `other`: da + db, metres),
/// so a quad's two bands never meet and the neighbour's surface is carried on past its line by less
/// than half its length.
fn mitre_band(z: f64, zn: f64, x: f64, own: f64, other: f64, p: &Params) -> f64 {
    if other <= 0.0 {
        return z;
    }
    let w = 1.0 / (1.0 / p.mitre_band_m + 2.0 / own + 2.0 / other);
    let s = smoothstep((x / w + 1.0) / 2.0);
    s * z + (1.0 - s) * zn
}

/// Segment j's surface at q carried past its mitre quad (the quad's formula with t outside [0, 1]),
/// and its length through q (da + db, metres; None when not positive).
fn surface(b: &Block, ln: &Line, j: usize, q: [f64; 2]) -> Option<(f64, f64)> {
    let (sa, sb) = (b.sample(ln, j), b.sample(ln, j + 1));
    let da = dot(sub(q, sa.p), b.mitre(ln, j));
    let db = dot(sub(sb.p, q), b.mitre(ln, j + 1));
    let ab = sub(sb.p, sa.p);
    let len = norm(ab);
    if da + db <= 0.0 || len <= 0.0 {
        return None;
    }
    let t = da / (da + db);
    let len_m = len * b.mpu;
    let z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, t);
    let lateral = -cross([ab[0] / len, ab[1] / len], sub(q, sa.p)) * b.mpu;
    Some((z + (sa.bank + t * (sb.bank - sa.bank)) * lateral, (da + db) * b.mpu))
}

/// One line present at a point.
#[derive(Clone, Copy, Debug)]
struct Rec {
    line: u32,
    class: u32,
    z: f64,
    /// Metres past its core, 0 inside.
    d: f64,
    /// Metres past its core outline, with the core arcs of its corners.
    out: f64,
    feather: f64,
    reach: f64,
    ret: f64,
    cut: f64,
    fill: f64,
    fill_max: f64,
    cut_max: f64,
    /// The plain terrain where the line's cones should meet it, blended as its surface (NaN unknown).
    reach_t: f64,
    /// The terrain's grade outward, blended as `reach_t`.
    reach_g: f64,
    /// Per other record (by its index after sorting), how much a fan relates the two at q.
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

/// The bin of q (closed cell square, as `find`): its header's word, or None outside the cell or for
/// an empty bin.
fn bin_head(w: &[u32], q: [f64; 2]) -> Option<usize> {
    let cell = CELL_UNITS as f64;
    if !(0.0..=cell).contains(&q[0]) || !(0.0..=cell).contains(&q[1]) {
        return None;
    }
    let bin = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).min(BINS - 1);
    let head = w[9] as usize + HEADER * (bin(q[1]) * BINS + bin(q[0])) as usize;
    (w[head] != 0).then_some(head)
}

/// The items of q's bin: its height segments and its discs, as (first word, count) each.
fn bin_items(b: &Block, q: [f64; 2]) -> Option<(usize, usize, usize, usize)> {
    let w = b.w;
    let head = bin_head(w, q)?;
    let mut at = w[head] as usize;
    let (n_cands, n_segs) = ((w[head + 1] >> 16) as usize, (w[head + 1] & 0xffff) as usize);
    let (n_h, n_d) = ((w[head + 2] >> 16) as usize, (w[head + 2] & 0xffff) as usize);
    for _ in 0..n_cands {
        at = skip_item(w, at);
    }
    at += n_segs;
    Some((at, n_h, at + n_h, n_d))
}

/// Metres from q to the outer edge of the slope band (the band layer with surface slope), from the
/// edges of q's bin (those that meet the bin grown by `MARGIN_M`): exact within it, else at least it.
fn slope_edge(b: &Block, q: [f64; 2]) -> f64 {
    let w = b.w;
    let Some(head) = bin_head(w, q) else { return f64::INFINITY };
    let mut at = w[head] as usize;
    let mut best = f64::INFINITY;
    for _ in 0..(w[head + 1] >> 16) {
        let next = skip_item(w, at);
        let cand = &w[w[4] as usize + CAND * (w[at] & 0xffff) as usize..][..CAND];
        let slope = cand[0] == BAND
            && cand[3] != u32::MAX
            && w[w[5] as usize + LABEL * cand[3] as usize] & 0xff == Surface::Slope as u32;
        if slope {
            let mut run = at + 2;
            for _ in 0..w[at + 1] {
                let n = w[run] as usize;
                let v = |k: usize| [f64::from(w[run + 1 + 2 * k] as i32), f64::from(w[run + 2 + 2 * k] as i32)];
                for k in 0..n.saturating_sub(1) {
                    let (a, c) = (v(k), v(k + 1));
                    let ab = sub(c, a);
                    let l2 = dot(ab, ab);
                    let f = if l2 > 0.0 { (dot(sub(q, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
                    best = best.min(norm(sub(q, [a[0] + f * ab[0], a[1] + f * ab[1]])));
                }
                run += 1 + 2 * n;
            }
        }
        at = next;
    }
    best * b.mpu
}

/// The lines present at q: per line, its surface from the quads that hold q with some health, kept
/// while its core lies within its feather or its reach. `keep`: the material lines of the labels asked
/// for, never skipped as absent: a face's host is its material line wherever the face lies.
fn present(b: &Block, q: [f64; 2], p: &Params, keep: [u32; 2]) -> Vec<Rec> {
    let Some((hs, nh, ds, nd)) = bin_items(b, q) else { return Vec::new() };
    let mut out: Vec<Rec> = Vec::with_capacity(SLOTS);
    let mut k = 0;
    while k < nh {
        let l = (b.w[hs + k] & 0xffff) as usize;
        let ln = b.line(l);
        let mut quads = [(0.0, 0.0, 0.0, 0.0); QUADS];
        let mut deltas = [0.0; QUADS];
        let mut nq = 0;
        // Every listed segment by its nearest point: the least distance, and the fallback surface (a
        // soft nearest of the segments' surfaces at their nearest points, kept as sums relative to the
        // least so far) where no healthy quad is near.
        let (mut least, mut fb_w, mut fb_z) = (f64::INFINITY, 0.0, 0.0);
        // The fallback's terrain at the reach, over the segments that know it.
        let (mut fb_tw, mut fb_t, mut fb_g) = (0.0, 0.0, 0.0);
        let mut core_d = f64::INFINITY;
        // A line whose core lies a feather or more from q is not present: skip it before its frames.
        let feather = ln.reach.max(p.feather_min_m);
        let mut end = k;
        let mut near = f64::INFINITY;
        while end < nh && (b.w[hs + end] & 0xffff) as usize == l {
            let i = (b.w[hs + end] >> 16) as usize;
            let (pa, pb) = (b.position(&ln, i), b.position(&ln, i + 1));
            let ab = sub(pb, pa);
            let l2 = dot(ab, ab);
            let f = if l2 > 0.0 { (dot(sub(q, pa), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (ha, hb) = (b.half(&ln, i), b.half(&ln, i + 1));
            near =
                near.min(norm(sub(q, [pa[0] + f * ab[0], pa[1] + f * ab[1]])) * b.mpu - (ha + f * (hb - ha)) - ln.side);
            end += 1;
        }
        let kept = keep.contains(&(l as u32));
        if (near >= feather && !kept) || out.len() == SLOTS {
            k = end;
            continue;
        }
        while k < end {
            let i = (b.w[hs + k] >> 16) as usize;
            k += 1;
            let (sa, sb) = (b.sample(&ln, i), b.sample(&ln, i + 1));
            if let Some(f) = segment_of(b, &ln, i, &sa, &sb, q, p) {
                if f.nu > 0.0 && nq < QUADS {
                    quads[nq] = (f.z, f.nu, f.reach_t, f.grade_t);
                    deltas[nq] = f.delta.abs();
                    nq += 1;
                }
            }
            let ab = sub(sb.p, sa.p);
            let l2 = dot(ab, ab);
            let f = if l2 > 0.0 { (dot(sub(q, sa.p), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let dist = norm(sub(q, [sa.p[0] + f * ab[0], sa.p[1] + f * ab[1]])) * b.mpu;
            core_d = core_d.min(dist - (sa.h + f * (sb.h - sa.h)) - ln.side);
            // Support within the segment's own band (its half-width, side and reach): every bin
            // that q can lie in lists it.
            let support = (sa.h + f * (sb.h - sa.h)) + ln.side + ln.reach;
            if dist < least {
                let rescale = if least.is_finite() { libm::exp(-(least - dist) / LEG_BLEND_M) } else { 0.0 };
                (fb_w, fb_z, fb_tw, fb_t, fb_g) =
                    (fb_w * rescale, fb_z * rescale, fb_tw * rescale, fb_t * rescale, fb_g * rescale);
                least = dist;
            }
            // Past its support a segment adds nothing to the fallback.
            if dist >= support {
                continue;
            }
            let len_m = l2.sqrt() * b.mpu;
            let side = if cross(ab, sub(q, sa.p)) <= 0.0 { 1.0 } else { -1.0 };
            let z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, f)
                + (sa.bank + f * (sb.bank - sa.bank)) * side * dist;
            let psi = libm::exp(-(dist - least) / LEG_BLEND_M) * phi(dist / support);
            fb_w += psi;
            fb_z += psi * z;
            let lateral = -cross(unit(ab), sub(q, sa.p)) * b.mpu;
            let core = sa.h + f * (sb.h - sa.h) + ln.side;
            let rt = meet_terrain(&sa, &sb, left_share(lateral, dist), f, core, ln.reach, p);
            if !rt.is_nan() {
                fb_tw += psi;
                fb_t += psi * rt;
                fb_g += psi * terrain_grade(&sa, &sb, left_share(lateral, dist), f, core, ln.reach);
            }
        }
        let core_d = core_d.max(0.0);
        if (core_d >= feather.max(ln.reach) && !kept) || out.len() == SLOTS || (nq == 0 && fb_w <= 0.0) {
            continue;
        }
        // The line's surface: each healthy quad weighs its health times exp(−(its distance − the
        // least) / LEG_BLEND_M), so a far leg weighs nothing (or, by `legs`, the nearest quad alone);
        // the fallback takes the rest of 1.
        let (mut sa_w, mut z) = (0.0, 0.0);
        let (mut tw, mut tz, mut tg) = (0.0, 0.0, 0.0);
        let jn = (0..nq).min_by(|&a, &c| deltas[a].total_cmp(&deltas[c])).unwrap_or(0);
        for j in 0..nq {
            let a = match p.legs {
                Legs::Ramp => quads[j].1 * libm::exp(-(deltas[j] - least).max(0.0) / LEG_BLEND_M),
                Legs::Wall if j == jn => quads[j].1,
                Legs::Wall => 0.0,
            };
            sa_w += a;
            z += a * quads[j].0;
            if !quads[j].2.is_nan() {
                (tw, tz, tg) = (tw + a, tz + a * quads[j].2, tg + a * quads[j].3);
            }
        }
        let fb = (1.0 - sa_w).max(0.0);
        let zf = if fb_w > 0.0 { fb_z / fb_w } else { 0.0 };
        if fb_tw > 0.0 {
            (tw, tz, tg) = (tw + fb, tz + fb * fb_t / fb_tw, tg + fb * fb_g / fb_tw);
        }
        out.push(Rec {
            line: l as u32,
            class: ln.class,
            z: (z + fb * zf) / (sa_w + fb),
            // The distance past the core is the plain one to the nearest segment: 0 exactly inside.
            d: core_d,
            out: f64::INFINITY,
            feather,
            reach: ln.reach,
            ret: ln.ret,
            cut: ln.cut,
            fill: ln.fill,
            fill_max: ln.fill_max,
            cut_max: ln.cut_max,
            reach_t: if tw > 0.0 { tz / tw } else { f64::NAN },
            reach_g: if tw > 0.0 { tg / tw } else { f64::NAN },
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
        // The corner's outline: the arc of radius r − ring between the feet, and at a narrower foot
        // GP3's taper, an arc of the wider ring about the foot from the ring's end to that arm's own
        // band. The distance is the unsigned distance to these arcs: continuous everywhere.
        let (ring0, ring1) = (f32w(e[7]), f32w(e[8]));
        let wide = ring0.max(ring1);
        let radius = f32w(e[2]) * b.mpu;
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
            if (rec.line == l0 || rec.line == l1) && d < rec.out {
                rec.out = d;
            }
        }
    }
    // The order of the painter: class rank (rails first), then the line.
    out.sort_by_key(|r| (r.class, r.line));
    // A fan relates its two arms out to their feather past its circle. Two lines that each meet a
    // third at a corner near q are related through it as far as that third line's own feather reaches
    // past both corners: the arms of two junctions on a short link meet.
    let n = out.len();
    let mut past = [[f64::INFINITY; SLOTS]; SLOTS];
    for &(l0, l1, d) in &pairs {
        let (Some(i), Some(j)) = (out.iter().position(|r| r.line == l0), out.iter().position(|r| r.line == l1)) else {
            continue;
        };
        past[i][j] = past[i][j].min(d);
        past[j][i] = past[i][j];
    }
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let mut w = phi((past[i][j] - (out[i].feather.max(out[j].feather) - 1.0)) / 1.5);
            // Through a third line only as far as it is present: it leaves the records a feather
            // from its core, so the relation goes with its presence.
            for (k, via) in out.iter().enumerate() {
                if k != i && k != j {
                    let presence = phi((via.d - (via.feather - 1.5)) / 1.5);
                    w = w.max(phi((past[i][k].max(past[k][j]) - (via.feather - 1.0)) / 1.5) * presence);
                }
            }
            out[i].rel[j] = w;
        }
    }
    out
}

/// The paved field for host o (an index into `recs`).
fn paved(recs: &[Rec], o: usize, p: &Params) -> f64 {
    paved_as(recs, o, 0, p)
}

/// The paved field for host o (`usize::MAX` for none: a blend face). Each other record weighs as far
/// as it agrees with the host, so a face takes no part of a way it disagrees with and two disagreeing
/// ways meet in a step where the host changes: on a face edge. `arms`: a bit per record that the face
/// relates to every other such record (a junction's arms in its region).
fn paved_as(recs: &[Rec], o: usize, arms: u32, p: &Params) -> f64 {
    let n = recs.len();
    let beta: Vec<f64> = recs.iter().map(|r| phi(r.d / r.feather)).collect();
    // The pair's agreement: a fan relates them, or the rise between them is one that the lesser
    // one's return grade climbs over the greater one's feather.
    let agree = |i: usize, j: usize| {
        let (upper, lower) = if i < j { (&recs[i], &recs[j]) } else { (&recs[j], &recs[i]) };
        let wall = lower.ret * upper.feather / p.blend_steep;
        let ok = wall / 2.0;
        let dz = (recs[i].z - recs[j].z).abs();
        let face = f64::from(u8::from((arms >> i) & (arms >> j) & 1 != 0));
        phi((dz - ok) / (wall - ok)).max(recs[i].rel[j]).max(face)
    };
    // Among them, a greater record i rules a lesser j as far as they agree, and a lesser record keeps
    // a greater one off its ground as far as they disagree (both ways, but the host is never kept
    // off). Inside a greater record's core it rules every record but the host fully, so a host's core
    // is exact, and the host keeps its share (1 − c) there: disagreeing ways meet in a step where the
    // host changes, on a face edge.
    let (mut sw, mut swz) = (0.0, 0.0);
    for j in 0..n {
        let mut w = if j == o || o >= n { beta[j] } else { beta[j] * agree(o, j) };
        for i in 0..n {
            if i == j {
                continue;
            }
            let c = agree(i, j);
            if i < j {
                // A record that is not the host also keeps off a greater one's ground as far as they
                // disagree, so its weight is 0 at that core's edge where it is 0 inside.
                let keep_off = if j == o { 1.0 } else { 1.0 - beta[i] * (1.0 - c) };
                w *= if recs[i].d <= 0.0 && j != o { 0.0 } else { (1.0 - beta[i] * c) * keep_off };
            } else {
                w *= 1.0 - beta[i] * (1.0 - c);
            }
        }
        sw += w;
        swz += w * recs[j].z;
    }
    if sw > 0.0 {
        swz / sw
    } else if o < n {
        recs[o].z
    } else {
        // No host and every record ruled out: the nearest one's own surface.
        (0..n).min_by(|&a, &c| recs[a].d.total_cmp(&recs[c].d)).map_or(f64::NAN, |k| recs[k].z)
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

/// The fill and cut at d metres past the outline of a road edge at `base`, with the plain terrain
/// `reach_t` where they should meet it (NaN unknown), the walls under them, and their slopes. A cone
/// that at its own slope does not meet the terrain where it starts to let go steepens up to the
/// greatest slope; the rest is a wall at the road's edge (the no-room rule).
fn cones(base: f64, reach_t: f64, d: f64, r: &Rec, p: &Params) -> (f64, f64, f64, f64, f64, f64) {
    let run = (r.reach * (1.0 - p.fade_share) - p.round_m / 2.0).max(1e-3);
    let fit = |s: f64, max: f64, need: f64| {
        if reach_t.is_nan() || need <= s * run {
            return (s, 0.0);
        }
        let s = (need / run).min(max.max(s));
        (s, (need - s * run).max(0.0))
    };
    let (sf, wf) = fit(r.fill, r.fill_max, base - reach_t);
    let (sc, wc) = fit(r.cut, r.cut_max, reach_t - base);
    (base - wf - cone(sf, d, p), base + wc + cone(sc, d, p), wf, wc, sf, sc)
}

/// The toe and crest round's width in height for a cone pair of slopes sf (fill) and sc (cut) over
/// terrain rising `grade` per metre outward: the smooth min/max is about 2 k / (grade gap) metres
/// wide, so k = gap × `toe_span_m` / 2 rounds it over `toe_span_m` whatever the gap.
fn span_k(sf: f64, sc: f64, grade: f64, p: &Params) -> f64 {
    if grade.is_nan() {
        return f64::INFINITY;
    }
    let gap = (grade + sf).abs().min((sc - grade).abs()).max(p.toe_gap_min);
    0.5 * p.toe_span_m * gap
}

/// The slope field: the fill and cut cones of every line present, on its paved field, released at
/// the reach; then `min(cut, max(T, fill))`, smooth at the toe and crest by a width that grows from 0
/// at the nearest outline. With the line whose cone holds the ground.
fn slope(b: &Block, q: [f64; 2], recs: &[Rec], t: f64, p: &Params) -> (f64, u32) {
    let (mut lo, mut hi, mut toe) = (f64::NEG_INFINITY, f64::INFINITY, p.toe_max_m);
    let (mut lo_line, mut hi_line) = (u32::MAX, u32::MAX);
    let mut seen = false;
    for (k, r) in recs.iter().enumerate() {
        if r.reach <= 0.0 {
            continue;
        }
        seen = true;
        let d = r.d.min(r.out);
        let base = paved(recs, k, p);
        let w = r.reach * p.fade_share;
        // Near the reach a cone lets go: the fill sinks and the cut rises without bound, so at the
        // reach the ground is the terrain and a fading cut never digs into another way's fill. The
        // fade reads the line's own distance: a corner's arc can bring the cone's foot nearer, never
        // keep a line alive past its reach.
        let fade = smoothstep((r.d - (r.reach - w)) / w);
        let relax = p.fade_gap_m * fade / (1.0 - fade).max(1e-6);
        let (fill, cut, _, _, sf, sc) = cones(base, r.reach_t, d, r, p);
        // The toe's width grows from 0 at this cone's outline; a fading cone leaves it alone.
        let kq = toe_width(d, p).min(span_k(sf, sc, r.reach_g, p));
        toe = toe.min(kq + fade * (p.toe_max_m - kq));
        if fill - relax > lo {
            (lo, lo_line) = (fill - relax, r.line);
        }
        if cut + relax < hi {
            (hi, hi_line) = (cut + relax, r.line);
        }
    }
    if !seen {
        return (t, u32::MAX);
    }
    // The round ends at the band's outer edge as it starts at the outline: a wider round lifts the
    // ground by up to a quarter of its width, which the band fade then pulls down in a steep lip.
    let edge = slope_edge(b, q);
    let toe = toe.min(edge_room(edge, p));
    let g = smin(hi, smax(t, lo, toe), toe);
    // The slope band's outer edge is the terrain's: the ground meets it there whatever the band's
    // shape (corner rings, square ends at a structure).
    let g = t + smoothstep(edge / p.band_fade_m) * (g - t);
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
    let rule = |l: Option<u32>| l.map_or(0, |id| words[words[5] as usize + LABEL * id as usize] >> 24);
    if rule(label) == 0 {
        return Height { h: t, kind: Kind::Terrain, owner: u32::MAX };
    }
    let b = Block::at(words, q);
    by_label(&b, q, &present(&b, q, &PARAMS, [material(words, label), u32::MAX]), label, t)
}

/// The heights at q of the two sides of an edge, with their labels: the records present at q are
/// found once.
pub fn height_at_sides(words: &[u32], q: [f64; 2], labels: [Option<u32>; 2], t: f64) -> [Height; 2] {
    let b = Block::at(words, q);
    let recs = present(&b, q, &PARAMS, labels.map(|l| material(words, l)));
    labels.map(|l| by_label(&b, q, &recs, l, t))
}

/// The material line of a paved label (owner, blend or kerb rule), u32::MAX for none.
fn material(words: &[u32], label: Option<u32>) -> u32 {
    let Some(id) = label else { return u32::MAX };
    let a = words[5] as usize + LABEL * id as usize;
    if words[a] >> 24 >= 2 {
        words[a + 2]
    } else {
        u32::MAX
    }
}

/// The host: the label's material line, so it is the same over the whole face; for a label with
/// none (or none within the records), the first record whose core holds q, else the nearest.
fn host(recs: &[Rec], material: u32) -> Option<usize> {
    recs.iter()
        .position(|r| r.line == material)
        .or_else(|| recs.iter().position(|r| r.d <= 0.0))
        .or_else(|| (0..recs.len()).min_by(|&a, &c| recs[a].d.total_cmp(&recs[c].d)))
}

/// The height at q by the label's height rule, from the records present there.
fn by_label(b: &Block, q: [f64; 2], recs: &[Rec], label: Option<u32>, t: f64) -> Height {
    let (p, words) = (&PARAMS, b.w);
    let lw = |k: usize| label.map_or(u32::MAX, |id| words[words[5] as usize + LABEL * id as usize + k]);
    let rule = if label.is_some() { lw(0) >> 24 } else { 0 };
    if rule == 0 {
        return Height { h: t, kind: Kind::Terrain, owner: u32::MAX };
    }
    if rule == 1 {
        let (h, by) = slope(b, q, recs, t, p);
        return Height { h, kind: Kind::Slope, owner: by };
    }
    match host(recs, lw(2)) {
        // A blend face (a junction's region) has no host: every record present takes part by the
        // painter's priorities, and the face relates the junction's arms (its material, host and
        // guest lines), so they meet in it whichever lies nearest.
        Some(k) if rule == 3 => {
            let named = |line: u32| line == lw(2) || (4..12).any(|w| lw(w) == line);
            let arms = recs.iter().enumerate().filter(|(_, r)| named(r.line)).fold(0, |m, (i, _)| m | 1 << i);
            Height { h: paved_as(recs, usize::MAX, arms, p), kind: Kind::Paved, owner: recs[k].line }
        }
        Some(o) => Height { h: paved(recs, o, p), kind: Kind::Paved, owner: recs[o].line },
        None => Height { h: t, kind: Kind::Paved, owner: u32::MAX },
    }
}
