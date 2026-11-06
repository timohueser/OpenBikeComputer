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
#[cfg(test)]
mod tests;

use super::records::{
    blend_edges, label as label_words, other_line, skip_item, BAND, BINS, BIN_UNITS, CAND, DISC, FILL, HEAD, HEADER,
    LABEL, LEVEL, LINE, MARGIN_M, RAIL, SAMPLE,
};
use super::Surface;
use crate::frame::{metres_per_unit_at, CELL_UNITS};

/// What kind of rule gave a height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Terrain,
    Slope,
    Paved,
}

/// The rule of a step between the two sides of a face edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    None,
    /// A slope side whose cone found no room: the wall of its shift at the paved outline.
    NoRoom,
    /// A slope side that another line's cut holds, against a paved side.
    CutWins,
    /// `CutWins` or `Unrelated` between a rail line and a road.
    RailRoad,
    /// Two paved sides whose hosts do not agree.
    Unrelated,
    /// The end line of a line at a level change (a bridge, tunnel or gallery end).
    StructureEnd,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Height {
    pub h: f64,
    pub kind: Kind,
    /// The host line of a paved point, the line whose cone holds a slope point, else `u32::MAX`.
    pub owner: u32,
    /// The rule of the step at a face edge (`height_at_sides`), when the step has that rule's size.
    pub step: Step,
}

/// The rule values of heights: the block `heights` of `plan.json`. A cell's records carry them
/// (`RULES` words after the head), so the crate and the viewer read the same values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeightRules {
    /// The least feather of a line: its presence falls from 1 at its core's edge to 0 a feather on.
    pub feather_min_m: f64,
    /// Two lines blend up to half of `dz_wall` and step from it: the lesser line's return grade times
    /// the greater line's feather over this.
    pub blend_steep: f64,
    /// A line's surface weighs its healthy quads by their metres across past the least, to 0 at this.
    pub leg_reach_m: f64,
    /// Frame health: none within the margin of the mitre crossing, full the width farther. The frame's
    /// least radius is the half-width plus both, so the whole carriageway has full health.
    pub health_margin_m: f64,
    pub health_width_m: f64,
    /// The frame's mitres turn toward the least radius by at most this angle from either chord
    /// (degrees): a mitre off its chords twists the road across by the grade times its tangent.
    pub frame_tilt_deg: f64,
    /// The slope's foot at the outline is rounded over this.
    pub round_m: f64,
    /// A cone lets go over this share of its reach, and halfway through it has moved `fade_gap_m`.
    pub fade_share: f64,
    pub fade_gap_m: f64,
    /// The toe and crest round: its width in height grows at `toe_per_m` from the outline up to
    /// `toe_max_m`, and spans about `toe_span_m` whatever the grade gap (at least `toe_gap_min`).
    pub toe_per_m: f64,
    pub toe_max_m: f64,
    pub toe_span_m: f64,
    pub toe_gap_min: f64,
    /// A fan's relation of its arms, and a third line's relation of two lines that meet it, fade over
    /// this before the feather.
    pub via_fade_m: f64,
    /// Within this of the slope band's outer edge the slope fades into the terrain.
    pub band_fade_m: f64,
    /// The greatest half-width of the band about a mitre line over which neighbour quads blend.
    pub mitre_band_m: f64,
    /// A step over this between the two sides of a face edge needs a rule.
    pub step_m: f64,
    /// A junction's region fades to the faces beyond its edges over this (at most `MARGIN_M`, up to
    /// which the records give the distance to its edges).
    pub region_fade_m: f64,
    pub legs: Legs,
}

/// Two legs of one way at different heights: one line surface over both, blended in a ramp of
/// `leg_reach_m`, or the nearer leg alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Legs {
    Ramp,
    Wall,
}

/// The number fields of `HeightRules` in the order of the block's words; then `legs` (0 ramp, 1 wall).
pub const RULE_FIELDS: [&str; 18] = [
    "feather_min_m",
    "blend_steep",
    "leg_reach_m",
    "health_margin_m",
    "health_width_m",
    "frame_tilt_deg",
    "round_m",
    "fade_share",
    "fade_gap_m",
    "toe_per_m",
    "toe_max_m",
    "toe_span_m",
    "toe_gap_min",
    "via_fade_m",
    "band_fade_m",
    "mitre_band_m",
    "step_m",
    "region_fade_m",
];
pub const RULES: usize = RULE_FIELDS.len() + 1;

impl HeightRules {
    pub fn new(v: [f64; RULE_FIELDS.len()], legs: Legs) -> Self {
        let [feather_min_m, blend_steep, leg_reach_m, health_margin_m, health_width_m, frame_tilt_deg, round_m, fade_share, fade_gap_m, toe_per_m, toe_max_m, toe_span_m, toe_gap_min, via_fade_m, band_fade_m, mitre_band_m, step_m, region_fade_m] =
            v;
        HeightRules {
            feather_min_m,
            blend_steep,
            leg_reach_m,
            health_margin_m,
            health_width_m,
            frame_tilt_deg,
            round_m,
            fade_share,
            fade_gap_m,
            toe_per_m,
            toe_max_m,
            toe_span_m,
            toe_gap_min,
            via_fade_m,
            band_fade_m,
            mitre_band_m,
            step_m,
            region_fade_m,
            legs,
        }
    }

    fn values(&self) -> [f64; RULE_FIELDS.len()] {
        [
            self.feather_min_m,
            self.blend_steep,
            self.leg_reach_m,
            self.health_margin_m,
            self.health_width_m,
            self.frame_tilt_deg,
            self.round_m,
            self.fade_share,
            self.fade_gap_m,
            self.toe_per_m,
            self.toe_max_m,
            self.toe_span_m,
            self.toe_gap_min,
            self.via_fade_m,
            self.band_fade_m,
            self.mitre_band_m,
            self.step_m,
            self.region_fade_m,
        ]
    }

    /// The block's words of the rules: f32 values, then the legs rule.
    pub fn words(&self) -> [u32; RULES] {
        let mut w = [0; RULES];
        for (k, v) in self.values().into_iter().enumerate() {
            w[k] = (v as f32).to_bits();
        }
        w[RULES - 1] = u32::from(self.legs == Legs::Wall);
        w
    }

    /// The rules that a block carries.
    pub(crate) fn of(words: &[u32]) -> Self {
        let w = &words[HEAD..][..RULES];
        HeightRules::new(std::array::from_fn(|k| f32w(w[k])), if w[RULES - 1] == 1 { Legs::Wall } else { Legs::Ramp })
    }
}

/// The most lines that heights keeps at one point, and the most quads of one line; the GPU port
/// holds the same. The fixture check `slots_full` shows that no point passes them.
pub const SLOTS: usize = 10;
pub const QUADS: usize = 4;

fn smoothstep(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// 1 at 0, 0 at 1 and past it, with zero slope at both ends.
fn phi(x: f64) -> f64 {
    1.0 - smoothstep(x)
}

/// The width of the toe's smooth min/max at d metres past the outline: slope `toe_per_m` at 0, level
/// at `most` (`toe_max_m` or less) from twice the distance where the slope would reach it, with no kink
/// (a kink of the width is a crease of the ground along the whole slope).
fn toe_width(d: f64, most: f64, p: &HeightRules) -> f64 {
    let most = most.min(p.toe_max_m);
    let x = (p.toe_per_m * d / (2.0 * most)).min(1.0);
    most * (1.0 - (1.0 - x) * (1.0 - x))
}

/// The lesser of two widths with no kink where they cross: (a⁻⁴ + b⁻⁴)^(-1/4), between 0.84 and 1
/// times the hard min; the hard min where either is 0 or less.
fn soft_min(a: f64, b: f64) -> f64 {
    let (lo, hi) = (a.min(b), a.max(b));
    if lo <= 0.0 {
        return lo;
    }
    let r = lo / hi;
    lo / (1.0 + r * r * r * r).sqrt().sqrt()
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

/// The distance from q to the point at share f of segment a, a + ab: q − a first, which is exact, so
/// two cells give the same bits.
fn off_foot(q: [f64; 2], a: [f64; 2], ab: [f64; 2], f: f64) -> f64 {
    norm([q[0] - a[0] - f * ab[0], q[1] - a[1] - f * ab[1]])
}

/// The frame's units across at q in the quad of segment a, a + ab with the mitres ta and tb, at its
/// share t (signed, + left): from the chord's line along q's line of equal t, which is normal to the
/// mitres' mean at t. So on a mitre line it is the distance along it, the same in both quads.
fn across(q: [f64; 2], a: [f64; 2], ab: [f64; 2], ta: [f64; 2], tb: [f64; 2], t: f64) -> f64 {
    let m = unit([ta[0] + t * (tb[0] - ta[0]), ta[1] + t * (tb[1] - ta[1])]);
    let v = sub(q, a);
    let along = dot(ab, m);
    let f = if along != 0.0 { dot(v, m) / along } else { 0.0 };
    -cross(m, [v[0] - f * ab[0], v[1] - f * ab[1]])
}

/// The distance from q to the segment a, c: q − a first, so two cells give the same bits.
fn to_segment(q: [f64; 2], a: [f64; 2], c: [f64; 2]) -> f64 {
    let ab = sub(c, a);
    let l2 = dot(ab, ab);
    let f = if l2 > 0.0 { (dot(sub(q, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    off_foot(q, a, ab, f)
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
    rail: bool,
    /// Whether the start and the end are a level change (a bridge, tunnel or gallery end).
    level: [bool; 2],
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
            rail: w[1] & RAIL != 0,
            level: [w[12] == LEVEL, w[13] == LEVEL],
        }
    }

    /// The way of line l.
    fn way(&self, l: u32) -> u32 {
        self.w[self.w[6] as usize + LINE * l as usize]
    }

    /// Metres of q past end e (0 the start, 1 the end) of the line along its mitre there, where that
    /// end is a level change: negative before it, NaN for another end.
    fn past_end(&self, ln: &Line, e: usize, q: [f64; 2]) -> f64 {
        if !ln.level[e] {
            return f64::NAN;
        }
        let i = if e == 0 { 0 } else { ln.count - 1 };
        let along = dot(sub(q, self.position(ln, i)), self.mitre(ln, i)) * self.mpu;
        if e == 0 {
            -along
        } else {
            along
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

    /// The glob order of sample i.
    fn glob(&self, ln: &Line, i: usize) -> u32 {
        self.w[self.w[7] as usize + SAMPLE * (ln.first + i) + 4]
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
fn meet_terrain(a: &Sample, b: &Sample, left: f64, t: f64, core: f64, reach: f64, p: &HeightRules) -> f64 {
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
/// (negative on the other side): 0 within the margin of the crossing, 1 from the width farther. The
/// product of both sides' health is continuous across the centre line.
fn health(r: f64, seg: f64, p: &HeightRules) -> f64 {
    smoothstep((r - seg - p.health_margin_m) / p.health_width_m)
}

/// Segment i of a line at q, with its two samples read: in its mitre quad, or past the line's first or
/// last sample along the end chord with the end grade. None where the quad does not hold q.
#[allow(clippy::too_many_arguments)]
fn segment_of(b: &Block, ln: &Line, i: usize, sa: &Sample, sb: &Sample, q: [f64; 2], p: &HeightRules) -> Option<Frame> {
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
        // The frame's metres across: both quads give them on their mitre line.
        delta = across(q, sa.p, ab, ta, tb, t) * b.mpu;
        around = delta.abs();
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
    // The bank tilts about the frame's metres across, which agree on the mitre line with the
    // neighbour's and go on smoothly past the quad (past an end, the end chord's distance).
    let mut z = z + bank * delta;
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
fn mitre_band(z: f64, zn: f64, x: f64, own: f64, other: f64, p: &HeightRules) -> f64 {
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
    let n = across(q, sa.p, ab, b.mitre(ln, j), b.mitre(ln, j + 1), t) * b.mpu;
    Some((z + (sa.bank + t * (sb.bank - sa.bank)) * n, (da + db) * b.mpu))
}

/// The lines present at a point, and whether a cap of `SLOTS` lines or `QUADS` quads left some out.
struct Present {
    recs: Vec<Rec>,
    full: bool,
}

/// One line present at a point.
#[derive(Clone, Copy, Debug)]
struct Rec {
    line: u32,
    class: u32,
    /// The glob order of its first sample: the painter's order among equal classes. Two cells give two
    /// lines the same order, and it holds along the line.
    glob: u32,
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
    rail: bool,
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
                    best = best.min(to_segment(q, v(k), v(k + 1)));
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
fn present(b: &Block, q: [f64; 2], p: &HeightRules, keep: [u32; 2]) -> Present {
    let Some((hs, nh, ds, nd)) = bin_items(b, q) else { return Present { recs: Vec::new(), full: false } };
    let mut out: Vec<Rec> = Vec::with_capacity(SLOTS);
    let mut full = false;
    let mut k = 0;
    while k < nh {
        let l = (b.w[hs + k] & 0xffff) as usize;
        let ln = b.line(l);
        // A line whose core lies a feather or more from q is not present: skip it before its frames.
        // Every listed segment by its nearest point: the least metres to the core and to the centre.
        let feather = ln.reach.max(p.feather_min_m);
        let kept = keep.contains(&(l as u32));
        let mut end = k;
        let (mut near, mut least) = (f64::INFINITY, f64::INFINITY);
        while end < nh && (b.w[hs + end] & 0xffff) as usize == l {
            let i = (b.w[hs + end] >> 16) as usize;
            end += 1;
            let (pa, pb) = (b.position(&ln, i), b.position(&ln, i + 1));
            let ab = sub(pb, pa);
            let l2 = dot(ab, ab);
            let f = if l2 > 0.0 { (dot(sub(q, pa), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (ha, hb) = (b.half(&ln, i), b.half(&ln, i + 1));
            let dist = off_foot(q, pa, ab, f) * b.mpu;
            near = near.min(dist - (ha + f * (hb - ha)) - ln.side);
            least = least.min(dist);
        }
        if near >= feather && !kept {
            k = end;
            continue;
        }
        let mut quads = [(0.0, 0.0, 0.0, 0.0); QUADS];
        let mut deltas = [0.0; QUADS];
        let mut nq = 0;
        // The least frame metres across of the line's healthy quads so far.
        let mut across = f64::INFINITY;
        // The fallback surface where no healthy quad is near: a soft nearest of the segments'
        // surfaces at their nearest points, and its terrain at the reach over the segments that know it.
        let (mut fb_w, mut fb_z) = (0.0, 0.0);
        let (mut fb_tw, mut fb_t, mut fb_g) = (0.0, 0.0, 0.0);
        while k < end {
            let i = (b.w[hs + k] >> 16) as usize;
            k += 1;
            let (sa, sb) = (b.sample(&ln, i), b.sample(&ln, i + 1));
            // The quads that can weigh, in the order of the line's samples: the same in every cell.
            let f = segment_of(b, &ln, i, &sa, &sb, q, p);
            // The legs' reach counts from the nearest quad that holds q, healthy or not.
            across = f.map_or(across, |f| across.min(f.delta.abs()));
            if let Some(f) = f.filter(|f| f.nu > 0.0) {
                if f.delta.abs() - across < p.leg_reach_m {
                    if nq == QUADS {
                        // The quads that a nearer one put past the leg's reach weigh 0: they go.
                        let mut kept = 0;
                        for j in 0..nq {
                            if deltas[j] - across < p.leg_reach_m {
                                (quads[kept], deltas[kept]) = (quads[j], deltas[j]);
                                kept += 1;
                            }
                        }
                        nq = kept;
                    }
                    if nq == QUADS {
                        full = true;
                    } else {
                        quads[nq] = (f.z, f.nu, f.reach_t, f.grade_t);
                        deltas[nq] = f.delta.abs();
                        nq += 1;
                    }
                }
            }
            let ab = sub(sb.p, sa.p);
            let l2 = dot(ab, ab);
            let f = if l2 > 0.0 { (dot(sub(q, sa.p), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let dist = off_foot(q, sa.p, ab, f) * b.mpu;
            // Support within the segment's own band (its half-width, side and reach): every bin
            // that q can lie in lists it.
            let support = (sa.h + f * (sb.h - sa.h)) + ln.side + ln.reach;
            let psi = phi((dist - least) / p.leg_reach_m) * phi(dist / support);
            if psi <= 0.0 {
                continue;
            }
            let len_m = l2.sqrt() * b.mpu;
            let side = if cross(ab, sub(q, sa.p)) <= 0.0 { 1.0 } else { -1.0 };
            let z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, f)
                + (sa.bank + f * (sb.bank - sa.bank)) * side * dist;
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
        // The distance past the core is the plain one to the nearest segment: 0 exactly inside.
        let core_d = near.max(0.0);
        if (core_d >= feather.max(ln.reach) && !kept) || (nq == 0 && fb_w <= 0.0) {
            continue;
        }
        // The line's surface: each healthy quad weighs its health times φ of its metres across past
        // the least over `leg_reach_m`, so a far leg weighs nothing (or, by `legs`, the nearest quad
        // alone); the fallback takes the rest of 1.
        let (mut sa_w, mut z) = (0.0, 0.0);
        let (mut tw, mut tz, mut tg) = (0.0, 0.0, 0.0);
        let jn = (0..nq).min_by(|&a, &c| deltas[a].total_cmp(&deltas[c])).unwrap_or(0);
        for j in 0..nq {
            let a = match p.legs {
                Legs::Ramp => quads[j].1 * phi((deltas[j] - across) / p.leg_reach_m),
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
            glob: b.glob(&ln, 0),
            z: (z + fb * zf) / (sa_w + fb),
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
            rail: ln.rail,
            rel: [0.0; SLOTS],
        });
    }
    // The order of the painter: class rank (rails first), then the global key. Past `SLOTS` records
    // the last by it go, but never the labels' material lines.
    out.sort_by_key(|r| (r.class, r.glob, r.line));
    full |= out.len() > SLOTS;
    while out.len() > SLOTS {
        let Some(k) = out.iter().rposition(|r| !keep.contains(&r.line)) else { break };
        out.remove(k);
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
        // A point's fan relates its two arms' ways near it; a round of the paved outline does not, as
        // no graph point joins its ways, and two legs of one way relate by their heights only.
        let past = norm(v) * b.mpu - f32w(e[2]) * b.mpu;
        if cand[0] != FILL && l0 != u32::MAX && l1 != u32::MAX && b.way(l0) != b.way(l1) {
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
    // A fan relates its two arms out to the greater feather past its circle. Two lines that each meet
    // a third at a corner near q are related through it as far as that third line's own feather
    // reaches past both corners: the arms of two junctions on a short link meet. Both fade over
    // `via_fade_m` before the feather.
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
            let fade = |past: f64, feather: f64| phi((past - feather + p.via_fade_m) / p.via_fade_m);
            let mut w = fade(past[i][j], out[i].feather.max(out[j].feather));
            // Through a third line only as far as it is present: it leaves the records a feather
            // from its core, so the relation goes with its presence.
            for (k, via) in out.iter().enumerate() {
                if k != i && k != j {
                    w = w.max(fade(past[i][k].max(past[k][j]), via.feather) * fade(via.d, via.feather));
                }
            }
            out[i].rel[j] = w;
        }
    }
    Present { recs: out, full }
}

/// The paved field for host o (an index into `recs`).
fn paved(recs: &[Rec], o: usize, p: &HeightRules) -> f64 {
    paved_as(recs, o, 0, p)
}

/// The rise from which two lines step instead of blending: the one that the lesser line's return
/// grade `ret` climbs over the greater line's feather (metres) at the steepness `blend_steep`.
pub fn step_limit(ret: f64, feather: f64, p: &HeightRules) -> f64 {
    ret * feather / p.blend_steep
}

/// The agreement of records i and j: a fan relates them, the face relates them as arms (a bit per
/// record in `arms`), or the rise between them is one that the lesser one's return grade climbs over
/// the greater one's feather.
fn agree(recs: &[Rec], i: usize, j: usize, arms: u32, p: &HeightRules) -> f64 {
    let (upper, lower) = if i < j { (&recs[i], &recs[j]) } else { (&recs[j], &recs[i]) };
    let wall = step_limit(lower.ret, upper.feather, p);
    let ok = wall / 2.0;
    let dz = (recs[i].z - recs[j].z).abs();
    let face = f64::from(u8::from((arms >> i) & (arms >> j) & 1 != 0));
    phi((dz - ok) / (wall - ok)).max(recs[i].rel[j]).max(face)
}

/// The paved field for host o (`usize::MAX` for none: a blend face). Each other record weighs as far
/// as it agrees with the host, so a face takes no part of a way it disagrees with and two disagreeing
/// ways meet in a step where the host changes: on a face edge. `arms`: a bit per record that the face
/// relates to every other such record (a junction's arms in its region).
fn paved_as(recs: &[Rec], o: usize, arms: u32, p: &HeightRules) -> f64 {
    let n = recs.len();
    let mut beta = [0.0; SLOTS];
    for (b, r) in beta.iter_mut().zip(recs) {
        *b = phi(r.d / r.feather);
    }
    let agree = |i: usize, j: usize| agree(recs, i, j, arms, p);
    // Among them, a greater record i rules a lesser j as far as it is present: fully inside its core,
    // so a host's core is exact, and by (1 − β) toward it, so the weight goes to 0 at that core's edge
    // with no step. The host is ruled only as far as they agree and keeps its share (1 − c) there:
    // disagreeing ways meet in a step where the host changes, on a face edge. A lesser record keeps a
    // greater one off its ground as far as they disagree, but the host is never kept off.
    let (mut sw, mut swz) = (0.0, 0.0);
    for j in 0..n {
        let mut w = if j == o || o >= n { beta[j] } else { beta[j] * agree(o, j) };
        for (i, &bi) in beta[..n].iter().enumerate() {
            if i < j {
                w *= if j == o { 1.0 - bi * agree(i, j) } else { 1.0 - bi };
            } else if i > j && j != o {
                // The host is never kept off, so its face always has a height of its own.
                w *= 1.0 - bi * (1.0 - agree(i, j));
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
fn cone(s: f64, d: f64, p: &HeightRules) -> f64 {
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
fn cones(base: f64, reach_t: f64, d: f64, r: &Rec, p: &HeightRules) -> (f64, f64, f64, f64, f64, f64) {
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
fn span_k(sf: f64, sc: f64, grade: f64, p: &HeightRules) -> f64 {
    if grade.is_nan() {
        return f64::INFINITY;
    }
    let gap = (grade + sf).abs().min((sc - grade).abs()).max(p.toe_gap_min);
    0.5 * p.toe_span_m * gap
}

/// What gave a side its height: for a slope, the record whose cone holds it; for a paved side, its
/// host, and for a region its arms (a bit per record).
#[derive(Clone, Copy, Debug)]
struct Cause {
    by: Option<usize>,
    arms: u32,
}

const NO_CAUSE: Cause = Cause { by: None, arms: 0 };

impl Cause {
    /// The hosts of a paved side: its host, or a region's arms, which all host it.
    fn hosts(&self, n: usize) -> impl Iterator<Item = usize> + '_ {
        (0..n).filter(move |&k| if self.arms != 0 { self.arms >> k & 1 != 0 } else { self.by == Some(k) })
    }
}

/// How far a record's cone has let go at q, and by how much it has moved. Near the reach a cone lets
/// go: the fill sinks and the cut rises without bound, so at the reach the ground is the terrain and a
/// fading cut never digs into another way's fill. The fade reads the line's own distance: a corner's
/// arc can bring the cone's foot nearer, never keep a line alive past its reach.
fn release(r: &Rec, p: &HeightRules) -> (f64, f64) {
    let w = r.reach * p.fade_share;
    let fade = smoothstep((r.d - (r.reach - w)) / w);
    (fade, p.fade_gap_m * fade / (1.0 - fade).max(1e-6))
}

/// The slope field: the fill and cut cones of every line present, on its paved field, released at
/// the reach; then `min(cut, max(T, fill))`, smooth at the toe and crest by a width that grows from 0
/// at the nearest outline. With the cone that holds the ground.
fn slope(b: &Block, q: [f64; 2], recs: &[Rec], t: f64, p: &HeightRules) -> (f64, Cause) {
    let (mut lo, mut hi, mut toe) = (f64::NEG_INFINITY, f64::INFINITY, p.toe_max_m);
    let (mut lo_by, mut hi_by) = (NO_CAUSE, NO_CAUSE);
    let mut seen = false;
    for (k, r) in recs.iter().enumerate() {
        if r.reach <= 0.0 {
            continue;
        }
        seen = true;
        let d = r.d.min(r.out);
        let base = paved(recs, k, p);
        let (fade, relax) = release(r, p);
        let (fill, cut, _, _, sf, sc) = cones(base, r.reach_t, d, r, p);
        // The toe's width grows from 0 at this cone's outline, never past the gap between its cut and
        // fill: the smooth min and max move the ground by up to a quarter of the width, so a wider
        // round would lift a slope off its paved edge where the edge lies a little past the outline.
        // A hard min would crease the slope where the two widths cross. A fading cone leaves it alone.
        let kq = soft_min(toe_width(d, span_k(sf, sc, r.reach_g, p), p), cut - fill);
        toe = toe.min(kq + fade * (p.toe_max_m - kq));
        if fill - relax > lo {
            (lo, lo_by) = (fill - relax, Cause { by: Some(k), ..NO_CAUSE });
        }
        if cut + relax < hi {
            (hi, hi_by) = (cut + relax, Cause { by: Some(k), ..NO_CAUSE });
        }
    }
    if !seen {
        return (t, NO_CAUSE);
    }
    // The round lifts the ground by up to a quarter of its width, so it narrows to 0 at the band's
    // outer edge, over all of `MARGIN_M` (past which `slope_edge` is a bound, not a distance): a
    // round that narrows over less drops its lift in a crease, and one that stays wide leaves a lip
    // for the band fade.
    let edge = slope_edge(b, q);
    let toe = toe * smoothstep(edge / MARGIN_M);
    let g = smin(hi, smax(t, lo, toe), toe);
    // The slope band's outer edge is the terrain's: the ground meets it there whatever the band's
    // shape (corner rings, square ends at a structure).
    let g = t + smoothstep(edge / p.band_fade_m) * (g - t);
    let by = if (g - hi).abs() < (g - t).abs().min((g - lo).abs()) {
        hi_by
    } else if (g - lo).abs() < (g - t).abs() {
        lo_by
    } else {
        NO_CAUSE
    };
    (g, by)
}

/// The height at q (frame units from the cell's origin), with the label there (`records::find`) and
/// the plain terrain t there.
pub fn height_at(words: &[u32], q: [f64; 2], label: Option<u32>, t: f64) -> Height {
    let rule = |l: Option<u32>| l.map_or(0, |id| words[words[5] as usize + LABEL * id as usize] >> 24);
    if rule(label) == 0 {
        return Height { h: t, kind: Kind::Terrain, owner: u32::MAX, step: Step::None };
    }
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    by_label(&b, q, &present(&b, q, &p, [material(words, label), u32::MAX]).recs, label, t, &p).0
}

/// The heights at q of the two sides of an edge, with their labels, and the rule of the step between
/// them: the records present at q are found once.
pub fn height_at_sides(words: &[u32], q: [f64; 2], labels: [Option<u32>; 2], t: f64) -> [Height; 2] {
    let rule = |l: Option<u32>| l.map_or(0, |id| words[words[5] as usize + LABEL * id as usize] >> 24);
    if labels.iter().all(|&l| rule(l) == 0) {
        return [Height { h: t, kind: Kind::Terrain, owner: u32::MAX, step: Step::None }; 2];
    }
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let recs = present(&b, q, &p, labels.map(|l| material(words, l))).recs;
    let sides = labels.map(|l| by_label(&b, q, &recs, l, t, &p));
    let step = step(&b, q, &recs, &sides, &p);
    sides.map(|(h, _)| Height { step, ..h })
}

/// Within this many frame units of a line's end line, a point lies on it: the stroke's corners are
/// whole units.
const END_UNITS: f64 = 2.0;

/// The rule of the step between two sides at q, if the step lies within that rule's size (up to
/// `step_m` more): a no-room wall up to its top or up to the terrain, the disagreement of two hosts.
fn step(b: &Block, q: [f64; 2], recs: &[Rec], sides: &[(Height, Cause); 2], p: &HeightRules) -> Step {
    if (sides[0].0.h - sides[1].0.h).abs() <= p.step_m {
        return Step::None;
    }
    let within = |rise: f64, size: f64| rise * size > 0.0 && rise.abs() <= size.abs() + p.step_m;
    let rail = |i: usize, j: usize, name: Step| if recs[i].rail != recs[j].rail { Step::RailRoad } else { name };
    let n = recs.len();
    for (a, o) in [(&sides[0], &sides[1]), (&sides[1], &sides[0])] {
        let (Kind::Slope, Kind::Paved) = (a.0.kind, o.0.kind) else { continue };
        let rise = a.0.h - o.0.h;
        for h in o.1.hosts(n) {
            // A host's no-room walls at its outline: the slope side stands on one, up to the wall's
            // top or up to the terrain.
            let r = &recs[h];
            let (_, _, wf, wc, _, _) = cones(o.0.h, r.reach_t, 0.0, r, p);
            if within(rise, wc) || within(rise, -wf) {
                return Step::NoRoom;
            }
        }
        // Another line's cone holds the slope side: the edge of the higher feature.
        if let (Some(c), Some(h)) = (a.1.by, o.1.hosts(n).next()) {
            if o.1.hosts(n).all(|h| h != c) {
                return rail(c, h, Step::CutWins);
            }
        }
    }
    let [(ha, ca), (hb, cb)] = sides;
    if let (Kind::Paved, Kind::Paved) = (ha.kind, hb.kind) {
        for (i, j) in ca.hosts(n).flat_map(|i| cb.hosts(n).map(move |j| (i, j))) {
            if i != j && agree(recs, i, j, 0, p) < 1.0 && within(ha.h - hb.h, recs[i].z - recs[j].z) {
                return rail(i, j, Step::Unrelated);
            }
        }
    }
    // A paved side at the end of a host at a level change, where the slope band ends square and fades
    // into the terrain over `band_fade_m`.
    for (h, c) in sides {
        for i in c.hosts(n).filter(|_| h.kind == Kind::Paved) {
            let ln = b.line(recs[i].line as usize);
            let near = |e: usize| (-p.band_fade_m..=END_UNITS * b.mpu).contains(&b.past_end(&ln, e, q));
            if near(0) || near(1) {
                return Step::StructureEnd;
            }
        }
    }
    Step::None
}

/// At most this many regions beyond a region's edges weigh at a point (the nearest); the GPU port holds
/// the same, and the fixture check `slots_full` shows that no point has more.
pub const REGIONS: usize = 2;

/// A junction's region (a blend face) at q, with the plain terrain t: its own painter (`own`), which
/// relates its arms, and near each of its edges a fade to the face beyond. A paved face beyond gives its
/// host's painter, which is that face's height; another region gives the painter that relates both
/// regions' arms, the same from both sides; a slope or the terrain gives the arms' cone base, where the
/// slope starts. Per group of edges,
/// D its least distance, s = smoothstep(D / `region_fade_m`) and ω = σ (1 − s) / s; the height is
/// (own + Σ ω V) / (1 + Σ ω). So it is V on the edge, with V's slope there, and `own` from
/// `region_fade_m` on. σ is a host's presence times the region's agreement with it (a line that
/// disagrees with every arm keeps its step on the edge), else 1. The distances are read up to
/// `region_fade_m` only, where two cells give the same bits (the records' border check holds it).
fn region(b: &Block, q: [f64; 2], recs: &[Rec], label: u32, arms: u32, t: f64, p: &HeightRules) -> f64 {
    let n = recs.len();
    let own = paved_as(recs, usize::MAX, arms, p);
    let beta = |k: usize| phi(recs[k].d / recs[k].feather);
    let arm = |k: usize| arms >> k & 1 != 0;
    // The least metres to the edges of each group: a host beyond at its record's index, the rest at
    // `SLOTS`; the regions beyond apart, the nearest `REGIONS`.
    let mut dist = [f64::INFINITY; SLOTS + 1];
    let mut beyond = [(u32::MAX, f64::INFINITY); REGIONS];
    for e in blend_edges(b.w, q).filter(|e| e.region == label) {
        let v = |p: [i32; 2]| [f64::from(p[0]), f64::from(p[1])];
        let d = to_segment(q, v(e.a), v(e.b)) * b.mpu;
        if d >= p.region_fade_m {
            continue;
        }
        let other = e.other.unwrap_or(u32::MAX);
        let rule = if other == u32::MAX { 0 } else { label_words(b.w, other)[0] >> 24 };
        if rule == 3 {
            let k = beyond.iter().position(|g| g.0 == other).unwrap_or(REGIONS - 1);
            if d < beyond[k].1 {
                beyond[k] = (other, d);
                beyond.sort_by(|a, c| a.1.total_cmp(&c.1));
            }
            continue;
        }
        // A paved face beyond: its host line, if it has one; a slope or the terrain: the arms' cone base.
        let material = if other == u32::MAX { u32::MAX } else { label_words(b.w, other)[2] };
        let group = match rule {
            2 | 4 => recs.iter().position(|r| r.line == material),
            _ => Some(SLOTS),
        };
        if let Some(g) = group {
            dist[g] = dist[g].min(d);
        }
    }
    // Each group's distance, σ and target, in the painter's order: the hosts, the regions, the arms.
    let mut groups: Vec<(f64, f64, f64)> = Vec::with_capacity(n + REGIONS + 1);
    for g in (0..n).filter(|&g| dist[g] < p.region_fade_m) {
        let agreed = (0..n).filter(|&k| arm(k)).map(|k| beta(k) * agree(recs, k, g, arms, p)).fold(0.0, f64::max);
        groups.push((dist[g], beta(g) * agreed, paved(recs, g, p)));
    }
    let mut regions: Vec<(f64, f64, f64)> = (beyond.iter().filter(|g| g.0 != u32::MAX))
        .map(|&(other, d)| (d, 1.0, paved_as(recs, usize::MAX, arms | arms_of(b.w, recs, other), p)))
        .collect();
    regions.sort_by(|a, c| a.0.total_cmp(&c.0).then(a.2.total_cmp(&c.2)));
    groups.extend(regions);
    if dist[SLOTS] < p.region_fade_m {
        // The arms' cones as the slope has them, without their no-room walls: on an arm's outline its
        // cone base, and past it, where the region reaches past its arms' outline, their cones.
        let (mut lo, mut hi, mut seen) = (f64::NEG_INFINITY, f64::INFINITY, false);
        for (k, r) in recs.iter().enumerate().filter(|&(k, r)| arm(k) && r.reach > 0.0) {
            let (fill, cut, wf, wc, _, _) = cones(paved(recs, k, p), r.reach_t, r.d.min(r.out), r, p);
            let relax = release(r, p).1;
            (lo, hi, seen) = (lo.max(fill + wf - relax), hi.min(cut - wc + relax), true);
        }
        if seen {
            groups.push((dist[SLOTS], 1.0, hi.min(t.max(lo))));
        }
    }
    let (mut sw, mut sv) = (1.0, 0.0);
    for (d, sigma, v) in groups.into_iter().filter(|g| g.1 > 0.0) {
        let s = smoothstep(d / p.region_fade_m);
        if s <= 0.0 {
            return v;
        }
        let w = sigma * (1.0 - s) / s;
        (sw, sv) = (sw + w, sv + w * (v - own));
    }
    own + sv / sw
}

/// Whether more than `REGIONS` regions lie beyond the edges of region `label` within `region_fade_m`
/// of q: then the farthest are left out.
fn regions_full(w: &[u32], q: [f64; 2], label: u32, p: &HeightRules) -> bool {
    let mpu = Block::at(w, q).mpu;
    let mut seen: Vec<u32> = Vec::new();
    for e in blend_edges(w, q).filter(|e| e.region == label) {
        let v = |p: [i32; 2]| [f64::from(p[0]), f64::from(p[1])];
        let other = e.other.unwrap_or(u32::MAX);
        let near = to_segment(q, v(e.a), v(e.b)) * mpu < p.region_fade_m;
        if near && other != u32::MAX && label_words(w, other)[0] >> 24 == 3 && !seen.contains(&other) {
            seen.push(other);
        }
    }
    seen.len() > REGIONS
}

/// The records that a region relates as its junction's arms (its material, host and guest lines), a bit
/// each.
fn arms_of(words: &[u32], recs: &[Rec], id: u32) -> u32 {
    let l = label_words(words, id);
    let named = |line: u32| line == l[2] || l[4..12].iter().any(|&x| other_line(x) == line);
    recs.iter().enumerate().filter(|(_, r)| named(r.line)).fold(0, |m, (i, _)| m | 1 << i)
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

/// The host: the label's material line, so it is the same over the whole face; None where the records
/// lack it. A face with no material line (a band face, until it gets one) takes the first record whose
/// core holds q, else the nearest.
fn host(recs: &[Rec], material: u32) -> Option<usize> {
    if material != u32::MAX {
        return recs.iter().position(|r| r.line == material);
    }
    recs.iter().position(|r| r.d <= 0.0).or_else(|| (0..recs.len()).min_by(|&a, &c| recs[a].d.total_cmp(&recs[c].d)))
}

/// The height at q by the label's height rule, from the records present there, and its cause.
fn by_label(b: &Block, q: [f64; 2], recs: &[Rec], label: Option<u32>, t: f64, p: &HeightRules) -> (Height, Cause) {
    let words = b.w;
    let lw = |k: usize| label.map_or(u32::MAX, |id| words[words[5] as usize + LABEL * id as usize + k]);
    let rule = if label.is_some() { lw(0) >> 24 } else { 0 };
    let height = |h: f64, kind: Kind, by: Option<usize>| Height {
        h,
        kind,
        owner: by.map_or(u32::MAX, |k| recs[k].line),
        step: Step::None,
    };
    if rule == 0 {
        return (height(t, Kind::Terrain, None), NO_CAUSE);
    }
    if rule == 1 {
        let (h, cause) = slope(b, q, recs, t, p);
        return (height(h, Kind::Slope, cause.by), cause);
    }
    let host = host(recs, lw(2));
    if rule == 3 && !recs.is_empty() {
        // A blend face (a junction's region) has no host: every record present takes part by the
        // painter's priorities, and the face relates the junction's arms, so they meet in it whichever
        // lies nearest.
        let id = label.unwrap_or(u32::MAX);
        let arms = arms_of(words, recs, id);
        return (height(region(b, q, recs, id, arms, t, p), Kind::Paved, host), Cause { arms, ..NO_CAUSE });
    }
    match host {
        Some(o) => (height(paved(recs, o, p), Kind::Paved, Some(o)), Cause { by: Some(o), ..NO_CAUSE }),
        // A host missing from the records: a fault (`check::Scan::host_missing`).
        None => (height(t, Kind::Paved, None), NO_CAUSE),
    }
}
