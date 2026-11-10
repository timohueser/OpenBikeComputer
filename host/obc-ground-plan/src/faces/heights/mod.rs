//! The heights of the one road model: one point function over a cell's records, for the near mesh's
//! vertices and the far stamp's texels (the viewer's `terrain/heights.wgsl`, a port line for line).
//!
//! Per point, each line near it gives its own surface (its curve's height on the mitre-quad frame plus
//! bank times the metres across), its distance past its core and a presence that falls from 1 at the
//! core's edge to 0 at its feather. The paved field is a normalized painter of these surfaces in which
//! each pair of records blends as far as they agree: inside the core of the first record present it is
//! that record's surface exactly; a compatible lesser record ramps to it; an incompatible one keeps its
//! own surface and meets it in a step on the greater record's core edge. Slopes start from the face
//! plan's paved outline, at the paved field's height there (the records' outline edges): per host line
//! a fill and a cut over its edges, released at the reach, and a smooth `min(cut, max(T, fill))`, where
//! a line's cut rises to the bank of the other lines' fills (`slope`), so one wall stands at the lower
//! line's edge.

pub mod check;
#[cfg(test)]
mod tests;

use super::records::{
    blend_edge, edge_ends, edge_line, junction, label as label_words, skip_item, BAND, BINS, BIN_UNITS, CAND, DISC,
    EDGE, FILL, GRADE_STEPS, HEAD, HEADER, JR, JR_ARM, LABEL, LEVEL, LINE, MARGIN_M, RAIL, SAMPLE, WALL,
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
    /// A slope side that another line's cone holds, against a paved side: the bank of a higher line
    /// at the edge of a lower one (the bank budget), or a cut.
    Held,
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
    /// The width (metres of height) of the smooth maximum of the fills (the bank budget and the fill
    /// envelope) and of the cut's rise to the budget, which grows to it from 0 at the outline as the
    /// toe round: two cones of two lines meet in a round, not a crease.
    pub budget_round_m: f64,
    /// The highest wall of a wall class at its edge: where a higher line's bank asks for more, the cut
    /// rises past the wall's top at its steepest.
    pub wall_max_m: f64,
    /// The steepest slope (rise per metre) that a cone takes by its class or its budget; only a need
    /// past the highest wall steepens a face more, as far as it asks.
    pub face_max: f64,
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
pub const RULE_FIELDS: [&str; 21] = [
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
    "budget_round_m",
    "wall_max_m",
    "face_max",
    "step_m",
    "region_fade_m",
];
pub const RULES: usize = RULE_FIELDS.len() + 1;

impl HeightRules {
    pub fn new(v: [f64; RULE_FIELDS.len()], legs: Legs) -> Self {
        let [feather_min_m, blend_steep, leg_reach_m, health_margin_m, health_width_m, frame_tilt_deg, round_m, fade_share, fade_gap_m, toe_per_m, toe_max_m, toe_span_m, toe_gap_min, via_fade_m, band_fade_m, mitre_band_m, budget_round_m, wall_max_m, face_max, step_m, region_fade_m] =
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
            budget_round_m,
            wall_max_m,
            face_max,
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
            self.budget_round_m,
            self.wall_max_m,
            self.face_max,
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

/// The soft greatest of the fills `at[j].0` over `js`, and the greatest one's index: the mean of the two
/// greatest, the second weighted by (1 − gap / k)² where positive, with k growing from 0 at the
/// greatest one's outline (as the toe round). It never rises above the greatest (a smooth maximum lifts
/// a tie by k / 4) and is exact at the greatest one's edge, so the ground starts there. It sinks the
/// greatest by at most 0.106 k, at a gap of 0.4 k. Where the two change places they tie, and a tie's
/// mean is its value whatever k, so the envelope has no jump; with a third fill it would have one.
fn soft_top(
    at: &[(f64, f64, f64, f64)],
    js: impl Iterator<Item = usize> + Clone,
    p: &HeightRules,
) -> (f64, Option<usize>) {
    let Some(top) = js.clone().max_by(|&a, &c| at[a].0.total_cmp(&at[c].0).then(c.cmp(&a))) else {
        return (f64::NEG_INFINITY, None);
    };
    let second = js.filter(|&j| j != top).map(|j| at[j].0).fold(f64::NEG_INFINITY, f64::max);
    let (v, k) = (at[top].0, toe_width(at[top].3, p.budget_round_m, p));
    if k <= 0.0 || !second.is_finite() || !v.is_finite() {
        return (v, Some(top));
    }
    let x = (1.0 - (v - second) / k).max(0.0);
    ((v + x * x * second) / (1.0 + x * x), Some(top))
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
    /// A class with a wall at its edge where a higher line's bank needs one (`leg_wall`).
    wall: bool,
    /// Whether the start and the end are a level change (a bridge, tunnel or gallery end).
    level: [bool; 2],
}

impl Line {
    /// The highest wall at this line's edge: `wall_max_m` for a wall class, none for another.
    fn wall_max(&self, p: &HeightRules) -> f64 {
        if self.wall {
            p.wall_max_m
        } else {
            0.0
        }
    }

    /// The slope of the highest cut toward a bank budget: the class's steepest cut, at most a rock face.
    fn steepest_cut(&self, p: &HeightRules) -> f64 {
        self.cut_max.max(self.cut).min(p.face_max)
    }
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
            fill_max: f32w(w[14]),
            cut_max: f32w(w[15]),
            rail: w[1] & RAIL != 0,
            wall: w[1] & WALL != 0,
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
        let w = &self.w[self.w[7] as usize + SAMPLE * (ln.first + i) + 8..][..2];
        [f32w(w[0]), f32w(w[1])]
    }

    /// The health radius on a side (+1 left, −1 right) at sample i, frame units.
    fn rho(&self, ln: &Line, i: usize, side: f64) -> f64 {
        f32w(self.w[self.w[7] as usize + SAMPLE * (ln.first + i) + if side > 0.0 { 10 } else { 11 }])
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
    let (t, z, delta, nu);
    if da >= 0.0 && db >= 0.0 {
        t = if da + db > 0.0 { da / (da + db) } else { 0.5 };
        z = hermite(sa.z, sb.z, sa.grade * len_m, sb.grade * len_m, t);
        // The frame's metres across: both quads give them on their mitre line.
        delta = across(q, sa.p, ab, ta, tb, t) * b.mpu;
        // Interpolated as a curvature, so a straight neighbour (no crossing) does not swamp it.
        let r = |side: f64| b.mpu / ((1.0 - t) / b.rho(ln, i, side) + t / b.rho(ln, i + 1, side));
        nu = health(r(1.0), delta, p) * health(r(-1.0), -delta, p);
    } else if first && da < 0.0 && db >= 0.0 {
        let past = -dot(sub(q, sa.p), u) * b.mpu;
        t = 0.0;
        z = sa.z - sa.grade * past;
        delta = lateral;
        // The end vertex's health, as the first quad has it on the end's mitre line.
        nu = health(b.rho(ln, i, 1.0) * b.mpu, delta, p) * health(b.rho(ln, i, -1.0) * b.mpu, -delta, p);
    } else if last && db < 0.0 && da >= 0.0 {
        let past = dot(sub(q, sb.p), u) * b.mpu;
        t = 1.0;
        z = sb.z + sb.grade * past;
        delta = lateral;
        nu = health(b.rho(ln, i + 1, 1.0) * b.mpu, delta, p) * health(b.rho(ln, i + 1, -1.0) * b.mpu, -delta, p);
    } else {
        return None;
    }
    let bank = sa.bank + t * (sb.bank - sa.bank);
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
    Some(Frame { z, delta, nu })
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
    feather: f64,
    ret: f64,
    rail: bool,
    /// Per other record (by its index after sorting), how much a fan relates the two at q.
    rel: [f64; SLOTS],
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

/// The items that the paved field reads about a point: its height segments (`line | sample << 16`),
/// its discs (`candidate | disc << 16`) and its blend edges (by index). A bin's lists at a point of the
/// cell; the build's window bins at an outline end past it, which list the same items.
pub(crate) struct Items<'a> {
    pub segs: &'a [u32],
    pub discs: &'a [u32],
    pub blend: &'a [u32],
}

/// The first word of the height segments of the bin at `head`.
fn heights_start(w: &[u32], head: usize) -> usize {
    w[head] as usize + (w[head + 3] >> 16) as usize + (w[head + 1] & 0xffff) as usize
}

/// The items of q's bin.
fn bin_items<'a>(b: &Block<'a>, q: [f64; 2]) -> Option<Items<'a>> {
    let w = b.w;
    let head = bin_head(w, q)?;
    let at = heights_start(w, head);
    let (n_h, n_d, n_b) =
        ((w[head + 2] >> 16) as usize, (w[head + 2] & 0xffff) as usize, (w[head + 3] & 0xffff) as usize);
    Some(Items { segs: &w[at..at + n_h], discs: &w[at + n_h..at + n_h + n_d], blend: &w[at + n_h + n_d..][..n_b] })
}

/// The outline edges that q's bin lists, by host line: half words after their count, the first low. A
/// block with no outline edges (the far level) lists none.
fn outline_items<'a>(b: &Block<'a>, q: [f64; 2]) -> impl Iterator<Item = u32> + 'a {
    let w = b.w;
    let (at, n) = match bin_head(w, q).filter(|_| w[14] < w[9]) {
        Some(head) => {
            let at = heights_start(w, head) + (w[head + 2] >> 16) as usize + (w[head + 2] & 0xffff) as usize;
            let at = at + (w[head + 3] & 0xffff) as usize;
            (at, (w[at] & 0xffff) as usize)
        }
        None => (0, 0),
    };
    (1..=n).map(move |k| w[at + k / 2] >> (16 * (k % 2)) & 0xffff)
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

/// The lines present at q (from `items`): per line, its surface from the quads that hold q with some
/// health, kept while its core lies within its feather or its reach. `keep`: the material lines of the
/// labels asked for, never skipped as absent: a face's host is its material line wherever the face lies.
fn present(b: &Block, q: [f64; 2], p: &HeightRules, keep: [u32; 2], items: &Items) -> Present {
    let (hs, ds) = (items.segs, items.discs);
    let (nh, nd) = (hs.len(), ds.len());
    let mut out: Vec<Rec> = Vec::with_capacity(SLOTS);
    let mut full = false;
    let mut k = 0;
    while k < nh {
        let l = (hs[k] & 0xffff) as usize;
        let ln = b.line(l);
        // A line whose core lies a feather or more from q is not present: skip it before its frames.
        // Every listed segment by its nearest point: the least metres to the core and to the centre.
        let feather = ln.reach.max(p.feather_min_m);
        let kept = keep.contains(&(l as u32));
        let mut end = k;
        let (mut near, mut least) = (f64::INFINITY, f64::INFINITY);
        while end < nh && (hs[end] & 0xffff) as usize == l {
            let i = (hs[end] >> 16) as usize;
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
        let mut quads = [(0.0, 0.0); QUADS];
        let mut deltas = [0.0; QUADS];
        let mut nq = 0;
        // The least frame metres across of the line's healthy quads so far.
        let mut across = f64::INFINITY;
        // The fallback surface where no healthy quad is near: a soft nearest of the segments'
        // surfaces at their nearest points.
        let (mut fb_w, mut fb_z) = (0.0, 0.0);
        while k < end {
            let i = (hs[k] >> 16) as usize;
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
                        crate::probe::hit(crate::probe::Cap::Quads);
                    } else {
                        quads[nq] = (f.z, f.nu);
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
        let jn = (0..nq).min_by(|&a, &c| deltas[a].total_cmp(&deltas[c])).unwrap_or(0);
        for j in 0..nq {
            let a = match p.legs {
                Legs::Ramp => quads[j].1 * phi((deltas[j] - across) / p.leg_reach_m),
                Legs::Wall if j == jn => quads[j].1,
                Legs::Wall => 0.0,
            };
            sa_w += a;
            z += a * quads[j].0;
        }
        let fb = (1.0 - sa_w).max(0.0);
        let zf = if fb_w > 0.0 { fb_z / fb_w } else { 0.0 };
        out.push(Rec {
            line: l as u32,
            class: ln.class,
            glob: b.glob(&ln, 0),
            z: (z + fb * zf) / (sa_w + fb),
            d: core_d,
            feather,
            ret: ln.ret,
            rail: ln.rail,
            rel: [0.0; SLOTS],
        });
    }
    // The order of the painter: class rank (rails first), then the global key. Past `SLOTS` records
    // the last by it go, but never the labels' material lines.
    out.sort_by_key(|r| (r.class, r.glob, r.line));
    if out.len() > SLOTS {
        full = true;
        crate::probe::hit(crate::probe::Cap::Slots);
    }
    while out.len() > SLOTS {
        let Some(k) = out.iter().rposition(|r| !keep.contains(&r.line)) else { break };
        out.remove(k);
    }
    // A point's fan relates its two arms' ways near it; a round of the paved outline does not, as no
    // graph point joins its ways, and two legs of one way relate by their heights only.
    let mut pairs: Vec<(u32, u32, f64)> = Vec::new();
    let extras = b.w[8] as usize;
    for &item in &ds[..nd] {
        let cand = &b.w[b.w[4] as usize + CAND * (item & 0xffff) as usize..][..CAND];
        let e = &b.w[extras + cand[5] as usize + DISC * (item >> 16) as usize..][..DISC];
        let c = [f64::from(e[0] as i32), f64::from(e[1] as i32)];
        let (l0, l1) = (e[3], e[4]);
        let past = norm(sub(q, c)) * b.mpu - f32w(e[2]) * b.mpu;
        if cand[0] != FILL && l0 != u32::MAX && l1 != u32::MAX && b.way(l0) != b.way(l1) {
            pairs.push((l0, l1, past));
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

/// The no-room rule of a cone of slope s from an edge whose ends stand `need` above (a fill) or below
/// (a cut) the plain terrain where the cone should meet it (NaN unknown): a cone that at its own slope
/// does not meet the terrain where it starts to let go steepens up to the greatest slope `max`; the
/// rest is a wall at that end, up to `wall` high. Past that the face steepens on, as steep as the need
/// asks: with no cap, it meets a cliff inside the run and the release has nothing left to take. The
/// slope, and the wall at each end.
fn fit(s: f64, max: f64, need: [f64; 2], wall: f64, p: &HeightRules, reach: f64) -> (f64, [f64; 2]) {
    let run = run(reach, p);
    let most = need.iter().filter(|n| !n.is_nan()).fold(f64::NEG_INFINITY, |m, &n| m.max(n));
    let (s, max) = (s.min(p.face_max), max.min(p.face_max));
    let mut s = if most <= s * run { s } else { (most / run).min(max.max(s)) };
    if most - s * run > wall {
        s = (most - wall) / run;
    }
    (s, need.map(|n| if n.is_nan() { 0.0 } else { (n - s * run).clamp(0.0, wall) }))
}

/// The metres over which a cone from its round foot should meet the terrain: up to where it lets go.
fn run(reach: f64, p: &HeightRules) -> f64 {
    (reach * (1.0 - p.fade_share) - p.round_m / 2.0).max(1e-3)
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

/// What gave a side its height: for a slope, the line whose cone or bank (in another line's budget)
/// holds it, whether two parts of that line hold each other there, and whether it stands on that wall
/// class's own wall, which a bank asks for and `wall_max_m` caps; for a paved side, its host line; for
/// a region, its arms (a bit per record).
#[derive(Clone, Copy, Debug)]
struct Cause {
    by: Option<u32>,
    arms: u32,
    parts: bool,
    wall: bool,
}

const NO_CAUSE: Cause = Cause { by: None, arms: 0, parts: false, wall: false };

impl Cause {
    /// The records that host a paved side: its host, or a region's arms, which all host it.
    fn hosts<'a>(&'a self, recs: &'a [Rec]) -> impl Iterator<Item = usize> + 'a {
        (0..recs.len())
            .filter(move |&k| if self.arms != 0 { self.arms >> k & 1 != 0 } else { self.by == Some(recs[k].line) })
    }
}

/// How far a cone has let go at d metres from its edge, and by how much it has moved. Near the reach a
/// cone lets go: the fill sinks and the cut rises without bound, so at the reach the ground is the
/// terrain and a fading cut never digs into another way's fill.
fn release(d: f64, reach: f64, p: &HeightRules) -> (f64, f64) {
    let w = reach * p.fade_share;
    let fade = smoothstep((d - (reach - w)) / w);
    (fade, p.fade_gap_m * fade / (1.0 - fade).max(1e-6))
}

/// An edge of the paved outline (`records`): its ends with the paved face on the left, its host line,
/// the paved height at each end, the plain terrain where the cones from each end start to let go (NaN
/// unknown) and the terrain's grade outward.
#[derive(Clone, Copy, Debug)]
struct Edge {
    a: [f64; 2],
    b: [f64; 2],
    line: u32,
    h: [f64; 2],
    t: [f64; 2],
    grade: f64,
}

impl Block<'_> {
    fn edge(&self, k: u32) -> Edge {
        let w = &self.w[self.w[14] as usize + EDGE * k as usize..][..EDGE];
        let (a, b) = edge_ends(w);
        let grade = (w[3] >> 16) as u16 as i16;
        Edge {
            a: [f64::from(a[0]), f64::from(a[1])],
            b: [f64::from(b[0]), f64::from(b[1])],
            line: edge_line(w),
            h: [f32w(w[4]), f32w(w[5])],
            t: [f32w(w[6]), f32w(w[7])],
            grade: if grade == i16::MIN { f64::NAN } else { f64::from(grade) / GRADE_STEPS },
        }
    }
}

/// The greatest of top(x) − c |q − x| over the points x of an edge whose top runs linearly from `top[0]`
/// to `top[1]` (sign 1, a fill), or the least of top(x) + c |q − x| (sign −1, a cut): exact. The cone's
/// slope c is √(s² + g²) for the edge's grade g, so over a straight edge the envelope falls at s across
/// it, and a cone is always steeper than its edge climbs: a neighbour edge up or down the outline never
/// passes the foot's own height. q lies `along` metres along the edge from its start and `off` metres
/// off its line; the edge is `len` metres long.
fn edge_cone(along: f64, off: f64, len: f64, top: [f64; 2], s: f64, sign: f64) -> f64 {
    let g = if len > 0.0 { (top[1] - top[0]) / len } else { 0.0 };
    let u = (along + sign * off * g / s).clamp(0.0, len);
    let d = ((u - along) * (u - along) + off * off).sqrt();
    top[0] + g * u - sign * (s * s + g * g).sqrt() * d
}

/// How much the round of a cone of slope s at its foot (`cone`) holds it back at d metres from its
/// outline. A line's envelope takes it once, at its nearest edge: a round in each edge's cone would let
/// a neighbour edge up or down the outline's grade pass the foot's own height there.
fn round_back(s: f64, d: f64, p: &HeightRules) -> f64 {
    s * d - cone(s, d, p)
}

/// An outline edge's cone reaches this many frame units past its line into its paved face before it
/// falls on as a rock face.
const SIDE_UNITS: f64 = 32.0;

/// One host line's outline near q: its released fill and cut, the cut it may rise to toward its budget
/// (`steep`: a wall class's wall up to `wall_max_m`, then any class's steepest cut), the metres to its
/// nearest edge, the least toe width of its edges, whether it is a wall class, and how far it has let
/// go.
#[derive(Clone, Copy, Debug)]
struct Group {
    line: u32,
    class: u32,
    glob: u32,
    rail: bool,
    wall: bool,
    fill: f64,
    cut: f64,
    steep: f64,
    d: f64,
    toe: f64,
    fade: f64,
}

/// The host lines of the outline edges near q (q's bin), at most `SLOTS` in the painter's order, and
/// whether that cap left some out. Per edge within its host's reach: its fill and its cut, each the
/// extreme over the edge's points, by the no-room rule of its ends and released by its own distance.
fn groups(b: &Block, q: [f64; 2], p: &HeightRules) -> (Vec<Group>, bool) {
    let mut out: Vec<Group> = Vec::new();
    let mut at: Option<(Line, Group)> = None;
    // A line's group ends where the bin's next edge has another host.
    let close = |at: Option<(Line, Group)>, out: &mut Vec<Group>| {
        let Some((ln, mut g)) = at.filter(|(_, g)| g.d.is_finite()) else { return };
        g.fade = release(g.d, ln.reach, p).0;
        g.fill += round_back(ln.fill, g.d, p);
        g.cut -= round_back(ln.cut, g.d, p);
        g.steep -= round_back(ln.steepest_cut(p), g.d, p);
        out.push(g);
    };
    for k in outline_items(b, q) {
        let e = b.edge(k);
        if at.as_ref().is_none_or(|(_, g)| g.line != e.line) {
            close(at.take(), &mut out);
            let ln = b.line(e.line as usize);
            let (class, glob, rail, wall) = (ln.class, b.glob(&ln, 0), ln.rail, ln.wall);
            let (fill, cut, steep, d, toe) =
                (f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY, f64::INFINITY, f64::INFINITY);
            at = Some((ln, Group { line: e.line, class, glob, rail, wall, fill, cut, steep, d, toe, fade: 1.0 }));
        }
        let Some((ln, g)) = at.as_mut() else { continue };
        // Metres along and off the edge from q − a, which two cells give alike.
        let (ab, v) = (sub(e.b, e.a), sub(q, e.a));
        let len = norm(ab);
        let u = if len > 0.0 { [ab[0] / len, ab[1] / len] } else { [1.0, 0.0] };
        let out = -cross(u, v);
        let (along, off, len) = (dot(v, u) * b.mpu, out.abs() * b.mpu, len * b.mpu);
        let foot = along.clamp(0.0, len) - along;
        let d = (foot * foot + off * off).sqrt();
        if d >= ln.reach {
            continue;
        }
        let (fade, relax) = release(d, ln.reach, p);
        let (sf, wf) = fit(ln.fill, ln.fill_max, [e.h[0] - e.t[0], e.h[1] - e.t[1]], ln.wall_max(p), p, ln.reach);
        let (sc, wc) = fit(ln.cut, ln.cut_max, [e.t[0] - e.h[0], e.t[1] - e.h[1]], ln.wall_max(p), p, ln.reach);
        g.d = g.d.min(d);
        // An edge's toe width lets go with its cone, so an edge that leaves at the reach leaves no step
        // in the least width.
        let w = toe_width(d, span_k(sf, sc, e.grade, p), p);
        g.toe = g.toe.min(w + fade * (p.toe_max_m - w));
        // An edge's cone stands on its outward side, the side of its slope face. Past its line into its
        // paved face (from `SIDE_UNITS` on, where a face's own whole-unit rim and a point read beside it
        // lie) it falls on at a rock face's slope: a cone from the far side of a face never holds the
        // near edge's slope at its own fit, and the ground stays continuous where an edge's line passes.
        let inside = if len > 0.0 { (-out - SIDE_UNITS).max(0.0) * b.mpu * p.face_max } else { 0.0 };
        let fill = edge_cone(along, off, len, [e.h[0] - wf[0], e.h[1] - wf[1]], sf, 1.0);
        let cut = edge_cone(along, off, len, [e.h[0] + wc[0], e.h[1] + wc[1]], sc, -1.0);
        g.fill = g.fill.max(fill - relax - inside);
        g.cut = g.cut.min(cut + relax + inside);
        // The highest cut toward the budget: a wall class's wall up to `wall_max_m` at its edge, then for
        // every class a face at its steepest cut from the wall's top (from the edge where it has none).
        let (steep, rise) = (ln.steepest_cut(p), ln.wall_max(p));
        let top = [e.h[0] + rise, e.h[1] + rise];
        let (ss, ws) = fit(steep, steep, [e.t[0] - top[0], e.t[1] - top[1]], 0.0, p, ln.reach);
        let cut = edge_cone(along, off, len, [top[0] + ws[0], top[1] + ws[1]], ss, -1.0);
        g.steep = g.steep.min(cut + relax + inside);
    }
    close(at, &mut out);
    out.sort_by_key(|g| (g.class, g.glob, g.line));
    let full = out.len() > SLOTS;
    if full {
        crate::probe::hit(crate::probe::Cap::Slots);
    }
    out.truncate(SLOTS);
    (out, full)
}

/// Whether line j's fill counts in the bank budget of line k: another line's, and a road's, not a
/// rail's. A budget comes from road surfaces, so a rail bed beside a road gives that road no wall.
fn counts(groups: &[Group], j: usize, k: usize) -> bool {
    j != k && !groups[j].rail
}

/// The slope field: per host line of the outline edges near q, its fill and cut from the paved outline;
/// then `min(cut, max(T, fill))`, smooth at the toe and crest by a width that grows from 0 at the
/// nearest outline. With the line that holds the ground.
///
/// The bank budget B of a line: the soft greatest fill (`soft_top`) of the lines that count for it.
/// The fill envelope: the soft greatest of all fills. A soft greatest never rises above the greatest
/// fill, so where two fills nearly tie (two arms at a corner, two close roads, two legs of one line) it
/// lifts nothing, and at the greatest fill's own edge it is that fill: no step stands at an upper
/// road's edge.
///
/// A line's cut rises toward B up to `steep`. A wall class's rises to B up to `wall_max_m` at its edge:
/// the higher line's bank reaches down to the lower line's edge, and the step there is the one wall of
/// the pair. Past the wall's top the cut rises at its steepest (`cut_max`, at most `face_max`), with no
/// step. Another class steepens its cut from its edge, so its own edge never walls.
fn slope(b: &Block, q: [f64; 2], t: f64, p: &HeightRules) -> (f64, Cause) {
    let (groups, _) = groups(b, q, p);
    if groups.is_empty() {
        return (t, NO_CAUSE);
    }
    let (mut hi, mut toe) = (f64::INFINITY, p.toe_max_m);
    let mut hi_by = NO_CAUSE;
    let mut at = [(f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY, 0.0); SLOTS];
    for (k, g) in groups.iter().enumerate() {
        // The toe's width grows from 0 at this line's outline, never past the gap between its cut and
        // fill: the smooth min and max move the ground by up to a quarter of the width, so a wider
        // round would lift a slope off its paved edge. A hard min would crease the slope where the
        // two widths cross. A line that lets go leaves it alone.
        let kq = soft_min(g.toe, g.cut - g.fill);
        toe = toe.min(kq + g.fade * (p.toe_max_m - kq));
        at[k] = (g.fill, g.cut, g.steep, g.d);
    }
    // A line holds the ground with two of its parts where its own fill passes its cut.
    let by = |k: Option<usize>| Cause {
        by: k.map(|k| groups[k].line),
        arms: 0,
        parts: k.is_some_and(|k| at[k].0 > at[k].1),
        wall: false,
    };
    let (lo, top) = soft_top(&at, 0..groups.len(), p);
    let lo_by = by(top);
    // The least of the cuts before their rise, and the bank that lifts it if one does.
    let (mut own, mut own_bank) = (f64::INFINITY, None);
    for k in 0..groups.len() {
        let (_, cut, steep, d) = at[k];
        let counted = || (0..groups.len()).filter(|&j| counts(&groups, j, k));
        let (budget, held) = soft_top(&at, counted(), p);
        let held = held.unwrap_or(k);
        // The round of the cut's rise to B grows from 0 at k's outline and at the outline of every line
        // that counts in B, as the toe round: a wall's foot, a bank that ends at the edge and the upper
        // line's own edge stay exact. The least distance over them is continuous where the line that
        // gives B changes.
        let w = toe_width(counted().map(|j| at[j].3).fold(d, f64::min), p.budget_round_m, p);
        // The cut rises to min(budget, steep) in the same round: where the bank begins, the cut and the
        // bank meet in a round. With the line that gives it. Two parts of one line that no leg cut parts
        // (a switchback) hold each other as two lines do, by the hard maximum with the line's own fill:
        // only where that fill passes the line's cut, so it changes nothing where the line agrees
        // with itself.
        let lift = smin(budget, steep, w);
        let v = smax(cut, lift, w).max(at[k].0.min(steep));
        if cut < own {
            let lifted = lift > cut && budget <= steep;
            (own, own_bank) = (cut, lifted.then(|| by(Some(held))));
        }
        // Where the budget passes `steep`, a wall class stands on its own wall and the face past it.
        let side = match (lift > cut, budget <= steep) {
            (true, true) => (v, by(Some(held))),
            (true, false) if groups[k].wall => (v, Cause { wall: true, ..by(Some(k)) }),
            _ => (v, by(Some(k))),
        };
        if side.0 < hi {
            (hi, hi_by) = side;
        }
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
    // The line that holds the ground is the bound that min(hi, max(t, lo)) takes before the rounds: a
    // round moves the ground toward the terrain by a few centimetres where the bounds nearly meet, and
    // must not move the cause with it. Ground that stands above the least cut stands on that cut's bank.
    let by = if let Some(bank) = own_bank.filter(|_| t.max(lo).min(hi) > own) {
        bank
    } else if hi < t.max(lo) {
        hi_by
    } else if lo > t {
        lo_by
    } else {
        NO_CAUSE
    };
    (g, by)
}

/// The no-room walls of line l's outline at q, as the tops of the fill's and the cut's wall (the top
/// equals the edge's height where a cone has no wall), and how far the line's steepest cone climbs
/// between them and q: of its edge nearest q in q's bin, at q's foot on it. None where q's bin lists no
/// edge of l.
fn walls(b: &Block, q: [f64; 2], l: u32, p: &HeightRules) -> Option<([(f64, f64); 2], f64)> {
    let ln = b.line(l as usize);
    let mut best: Option<(f64, f64, f64, f64)> = None;
    for k in outline_items(b, q) {
        let e = b.edge(k);
        if e.line != l {
            continue;
        }
        let ab = sub(e.b, e.a);
        let l2 = dot(ab, ab);
        let f = if l2 > 0.0 { (dot(sub(q, e.a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let d = off_foot(q, e.a, ab, f);
        if best.is_some_and(|w| w.0 <= d) {
            continue;
        }
        let (_, wf) = fit(ln.fill, ln.fill_max, [e.h[0] - e.t[0], e.h[1] - e.t[1]], ln.wall_max(p), p, ln.reach);
        let (_, wc) = fit(ln.cut, ln.cut_max, [e.t[0] - e.h[0], e.t[1] - e.h[1]], ln.wall_max(p), p, ln.reach);
        let at = |v: [f64; 2]| v[0] + f * (v[1] - v[0]);
        best = Some((d, at(wf), at(wc), at(e.h)));
    }
    best.map(|(d, wf, wc, h)| ([(wf, h - wf), (wc, h + wc)], p.face_max * d * b.mpu))
}

/// The height rule of a label (0 for none).
fn rule_of(words: &[u32], label: Option<u32>) -> u32 {
    label.map_or(0, |id| words[words[5] as usize + LABEL * id as usize] >> 24)
}

/// The height at q (frame units from the cell's origin), with the label there (`records::find`) and
/// the plain terrain t there.
pub fn height_at(words: &[u32], q: [f64; 2], label: Option<u32>, t: f64) -> Height {
    let rule = rule_of(words, label);
    if rule == 0 {
        return Height { h: t, kind: Kind::Terrain, owner: u32::MAX, step: Step::None };
    }
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let Some(items) = bin_items(&b, q).filter(|_| rule >= 2) else { return by_label(&b, q, &[], &[], label, t, &p).0 };
    let recs = present(&b, q, &p, [material(words, label), u32::MAX], &items).recs;
    by_label(&b, q, &recs, items.blend, label, t, &p).0
}

/// The heights at q of the two sides of an edge, with their labels, and the rule of the step between
/// them: the records present at q are found once.
pub fn height_at_sides(words: &[u32], q: [f64; 2], labels: [Option<u32>; 2], t: f64) -> [Height; 2] {
    if labels.iter().all(|&l| rule_of(words, l) == 0) {
        return [Height { h: t, kind: Kind::Terrain, owner: u32::MAX, step: Step::None }; 2];
    }
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let items = bin_items(&b, q).filter(|_| labels.iter().any(|&l| rule_of(words, l) >= 2));
    let recs = items.as_ref().map_or(Vec::new(), |i| present(&b, q, &p, labels.map(|l| material(words, l)), i).recs);
    let blend = items.as_ref().map_or(&[][..], |i| i.blend);
    let sides = labels.map(|l| by_label(&b, q, &recs, blend, l, t, &p));
    let step = step(&b, q, &recs, &sides, &p);
    sides.map(|(h, _)| Height { step, ..h })
}

/// The paved height at q by paved label `id` from the items about q, with the host line as `owner`: the
/// one read of the paved field for the outline's ends (`records` reads them through its window's bins,
/// which list a bin's items). None where the face has no height of its own there: a host face whose
/// host the records lack, a region with no record present (`check::Scan::host_missing`). Only those
/// read the terrain.
pub(crate) fn paved_end(words: &[u32], q: [f64; 2], id: u32, items: &Items) -> Option<Height> {
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let recs = present(&b, q, &p, [material(words, Some(id)), u32::MAX], items).recs;
    let h = by_label(&b, q, &recs, items.blend, Some(id), f64::NAN, &p).0;
    let own = if rule_of(words, Some(id)) == 3 { !recs.is_empty() } else { h.owner != u32::MAX };
    own.then_some(h)
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
    for (a, o) in [(&sides[0], &sides[1]), (&sides[1], &sides[0])] {
        let (Kind::Slope, Kind::Paved) = (a.0.kind, o.0.kind) else { continue };
        let rise = a.0.h - o.0.h;
        for h in o.1.hosts(recs) {
            // A host's no-room walls at its outline: the slope side stands on one, up to the wall's
            // top or up to the terrain.
            let Some((tops, climb)) = walls(b, q, recs[h].line, p) else { continue };
            let on = |top: f64| rise * (top - o.0.h) > 0.0 && rise.abs() <= (top - o.0.h).abs() + climb + p.step_m;
            if tops.iter().any(|&(wall, top)| wall > 0.0 && on(top)) {
                return Step::NoRoom;
            }
        }
        // Another line's cone or bank holds the slope side: the bank of a higher line at a lower line's
        // edge (an arm's at its region's edge), or a cut at a higher line's edge; or another part of
        // the host line, as two lines. A step at a line's own edge held by its own fill is no wall: the
        // design puts none at an upper edge.
        if let (Some(c), Some(_)) = (a.1.by, o.1.hosts(recs).next()) {
            // A side on its own line's wall stands up to `wall_max_m` above the edge, as a no-room wall.
            let walled = || rise > 0.0 && walls(b, q, c, p).is_some_and(|w| rise <= p.wall_max_m + w.1 + p.step_m);
            if a.1.parts || (a.1.wall && walled()) || o.1.hosts(recs).all(|h| recs[h].line != c) {
                return Step::Held;
            }
        }
    }
    let [(ha, ca), (hb, cb)] = sides;
    if let (Kind::Paved, Kind::Paved) = (ha.kind, hb.kind) {
        for (i, j) in ca.hosts(recs).flat_map(|i| cb.hosts(recs).map(move |j| (i, j))) {
            if i != j && agree(recs, i, j, 0, p) < 1.0 && within(ha.h - hb.h, recs[i].z - recs[j].z) {
                return Step::Unrelated;
            }
        }
    }
    // A paved side at the end of a host at a level change, where the slope band ends square and fades
    // into the terrain over `band_fade_m`.
    for (h, c) in sides {
        for i in c.hosts(recs).filter(|_| h.kind == Kind::Paved) {
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

/// A junction's region (a blend face) at q: its own painter (`own`), which relates its arms, and near
/// each of its edges toward another paved face a fade to that face. A paved face beyond gives its
/// host's painter, which is that face's height; another region gives the painter that relates both
/// regions' arms, the same from both sides. Toward a slope or the terrain the region keeps its own
/// painter: the slope starts at its outline from it. Per group of edges, D its least distance,
/// s = smoothstep(D / `region_fade_m`) and ω = σ (1 − s) / s; the height is (own + Σ ω V) / (1 + Σ ω).
/// So it is V on the edge, with V's slope there, and `own` from `region_fade_m` on. σ is a host's
/// presence times the region's agreement with it (a line that disagrees with every arm keeps its step
/// on the edge), else 1. The distances are read up to `region_fade_m` only, where two cells give the
/// same bits (the records' border check holds it).
fn region(b: &Block, q: [f64; 2], recs: &[Rec], label: u32, arms: u32, blend: &[u32], p: &HeightRules) -> f64 {
    let n = recs.len();
    let own = paved_as(recs, usize::MAX, arms, p);
    let beta = |k: usize| phi(recs[k].d / recs[k].feather);
    let arm = |k: usize| arms >> k & 1 != 0;
    // The least metres to the edges of each host beyond, at its record's index; the regions beyond
    // apart, the nearest `REGIONS`.
    let mut dist = [f64::INFINITY; SLOTS];
    let mut beyond = [(u32::MAX, f64::INFINITY); REGIONS];
    for e in blend.iter().map(|&k| blend_edge(b.w, k)).filter(|e| e.region == label) {
        let v = |p: [i32; 2]| [f64::from(p[0]), f64::from(p[1])];
        let d = to_segment(q, v(e.a), v(e.b)) * b.mpu;
        if d >= p.region_fade_m {
            continue;
        }
        let Some(other) = e.other else { continue };
        let rule = label_words(b.w, other)[0] >> 24;
        if rule == 3 {
            let k = beyond.iter().position(|g| g.0 == other).unwrap_or(REGIONS - 1);
            if beyond[k].0 != other && beyond[k].0 != u32::MAX {
                crate::probe::hit(crate::probe::Cap::Regions);
            }
            if d < beyond[k].1 {
                beyond[k] = (other, d);
                beyond.sort_by(|a, c| a.1.total_cmp(&c.1));
            }
            continue;
        }
        // A paved face beyond: its host line, if it has one.
        let material = label_words(b.w, other)[2];
        if let Some(g) = (rule == 2 || rule == 4).then(|| recs.iter().position(|r| r.line == material)).flatten() {
            dist[g] = dist[g].min(d);
        }
    }
    // Each group's distance, σ and target, in the painter's order: the hosts, then the regions.
    let mut groups: Vec<(f64, f64, f64)> = Vec::with_capacity(n + REGIONS);
    for g in (0..n).filter(|&g| dist[g] < p.region_fade_m) {
        let agreed = (0..n).filter(|&k| arm(k)).map(|k| beta(k) * agree(recs, k, g, arms, p)).fold(0.0, f64::max);
        groups.push((dist[g], beta(g) * agreed, paved(recs, g, p)));
    }
    let mut regions: Vec<(f64, f64, f64)> = (beyond.iter().filter(|g| g.0 != u32::MAX))
        .map(|&(other, d)| (d, 1.0, paved_as(recs, usize::MAX, arms | arms_of(b.w, recs, other), p)))
        .collect();
    regions.sort_by(|a, c| a.0.total_cmp(&c.0).then(a.2.total_cmp(&c.2)));
    groups.extend(regions);
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
    for e in super::records::blend_edges(w, q).filter(|e| e.region == label) {
        let v = |p: [i32; 2]| [f64::from(p[0]), f64::from(p[1])];
        let other = e.other.unwrap_or(u32::MAX);
        let near = to_segment(q, v(e.a), v(e.b)) * mpu < p.region_fade_m;
        if near && other != u32::MAX && label_words(w, other)[0] >> 24 == 3 && !seen.contains(&other) {
            seen.push(other);
        }
    }
    seen.len() > REGIONS
}

/// The records that a region relates as its junction's arms (its material line and its junction's arms,
/// or a way's host and other lines), a bit each.
fn arms_of(words: &[u32], recs: &[Rec], id: u32) -> u32 {
    let l = label_words(words, id);
    let jr = junction(words, id);
    let named = |line: u32| {
        line == l[2] || jr.map_or(l[4..12].contains(&line), |jr| jr[JR..].chunks(JR_ARM).any(|a| a[0] & 0xffff == line))
    };
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

/// The height at q by the label's height rule, from the records present there and the blend edges about
/// q (a region's), and its cause.
fn by_label(
    b: &Block,
    q: [f64; 2],
    recs: &[Rec],
    blend: &[u32],
    label: Option<u32>,
    t: f64,
    p: &HeightRules,
) -> (Height, Cause) {
    let words = b.w;
    let lw = |k: usize| label.map_or(u32::MAX, |id| words[words[5] as usize + LABEL * id as usize + k]);
    let rule = if label.is_some() { lw(0) >> 24 } else { 0 };
    let height =
        |h: f64, kind: Kind, by: Option<u32>| Height { h, kind, owner: by.unwrap_or(u32::MAX), step: Step::None };
    if rule == 0 {
        return (height(t, Kind::Terrain, None), NO_CAUSE);
    }
    if rule == 1 {
        let (h, cause) = slope(b, q, t, p);
        return (height(h, Kind::Slope, cause.by), cause);
    }
    let host = host(recs, lw(2));
    if rule == 3 && !recs.is_empty() {
        // A blend face (a junction's region) has no host: every record present takes part by the
        // painter's priorities, and the face relates the junction's arms, so they meet in it whichever
        // lies nearest.
        let id = label.unwrap_or(u32::MAX);
        let arms = arms_of(words, recs, id);
        let h = region(b, q, recs, id, arms, blend, p);
        return (height(h, Kind::Paved, host.map(|o| recs[o].line)), Cause { arms, ..NO_CAUSE });
    }
    match host {
        Some(o) => {
            let line = recs[o].line;
            (height(paved(recs, o, p), Kind::Paved, Some(line)), Cause { by: Some(line), ..NO_CAUSE })
        }
        // A host missing from the records: a fault (`check::Scan::host_missing`).
        None => (height(t, Kind::Paved, None), NO_CAUSE),
    }
}
