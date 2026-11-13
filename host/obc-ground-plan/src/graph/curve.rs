//! The curve of a span: the cubic Hermite of format 4 in frame units, its arc metres, its roots on
//! the cell lines, and its samples on a dyadic grid of span metres.

use super::parts::Rule;
use super::{
    Part, Sample, SampleKind, Span, Vertex, BASE_STEP_M, KNOTS, LEVELS, MIN_STEP_M, NEWTON_STEPS, ROOT_STEPS, SPIKE_DEG,
};
use crate::frame::{metres_per_unit, CellKey, CELL_UNITS};

/// A point of a span's curve.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanPoint {
    /// Frame units from the span's start vertex.
    pub d: [f64; 2],
    /// The unit tangent.
    pub tangent: [f64; 2],
    pub z_m: f64,
    pub grade: f64,
    pub bank: f64,
    pub widen_m: f64,
}

/// The plan of a span: the start vertex, the chord in frame units and its length, the unit tangents.
#[derive(Clone, Copy, Debug)]
pub(super) struct Geo {
    pub p0: [i64; 2],
    pub d: [f64; 2],
    pub c: f64,
    t0: [f64; 2],
    t1: [f64; 2],
    /// The length of the end tangents: the chord's length times the span's handle.
    k: f64,
    /// Metres per frame unit at the chord's middle.
    pub mpu: f64,
    chord: bool,
}

/// The plan of a span from its vertices' tangents, with a full handle.
pub(super) fn geo(a: &Vertex, b: &Vertex, chord: bool) -> Geo {
    shaped(a, b, chord, [a.tangent, b.tangent], 1.0)
}

/// The plan of a span of a part.
pub(super) fn geo_of(a: &Vertex, b: &Vertex, span: &Span) -> Geo {
    shaped(a, b, span.chord, span.ends, span.handle)
}

fn shaped(a: &Vertex, b: &Vertex, chord: bool, ends: [f64; 2], handle: f64) -> Geo {
    let d = [(b.pos[0] - a.pos[0]) as f64, (b.pos[1] - a.pos[1]) as f64];
    let mpu = metres_per_unit((a.pos[1] + b.pos[1]).div_euclid(2));
    Geo { p0: a.pos, mpu, chord, ..curve(d, ends[0], ends[1], handle) }
}

/// The curve of a span from its start: chord `d`, tangent angles t0 and t1 and the handle, in the units
/// of `d`.
fn curve(d: [f64; 2], t0: f64, t1: f64, handle: f64) -> Geo {
    let c = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let unit = |angle: f64| [libm::cos(angle), libm::sin(angle)];
    Geo { p0: [0; 2], d, c, t0: unit(t0), t1: unit(t1), k: c * handle, mpu: 1.0, chord: false }
}

impl Geo {
    /// The position from p0, the first and the second derivative in u.
    pub fn eval(&self, u: f64) -> [[f64; 2]; 3] {
        if self.chord {
            return [[u * self.d[0], u * self.d[1]], self.d, [0.0; 2]];
        }
        let (u2, u3) = (u * u, u * u * u);
        let h = [u3 - 2.0 * u2 + u, 3.0 * u2 - 2.0 * u3, u3 - u2];
        let dh = [3.0 * u2 - 4.0 * u + 1.0, 6.0 * u - 6.0 * u2, 3.0 * u2 - 2.0 * u];
        let ddh = [6.0 * u - 4.0, 6.0 - 12.0 * u, 6.0 * u - 2.0];
        let at = |w: [f64; 3], i: usize| w[0] * self.k * self.t0[i] + w[1] * self.d[i] + w[2] * self.k * self.t1[i];
        [[at(h, 0), at(h, 1)], [at(dh, 0), at(dh, 1)], [at(ddh, 0), at(ddh, 1)]]
    }

    fn speed_m(&self, u: f64) -> f64 {
        let d = if self.chord {
            self.d
        } else {
            let u2 = u * u;
            let dh = [3.0 * u2 - 4.0 * u + 1.0, 6.0 * u - 6.0 * u2, 3.0 * u2 - 2.0 * u];
            let at = |i: usize| dh[0] * self.k * self.t0[i] + dh[1] * self.d[i] + dh[2] * self.k * self.t1[i];
            [at(0), at(1)]
        };
        (d[0] * d[0] + d[1] * d[1]).sqrt() * self.mpu
    }

    /// The Bézier control points from p0: the curve lies in their hull.
    pub fn hull(&self) -> [[f64; 2]; 4] {
        if self.chord {
            return [[0.0; 2], self.d, self.d, self.d];
        }
        let k = self.k / 3.0;
        [[0.0; 2], [k * self.t0[0], k * self.t0[1]], [self.d[0] - k * self.t1[0], self.d[1] - k * self.t1[1]], self.d]
    }
}

/// The tangents of water, which the tiles do not hold, per point its angle and whether it is a corner:
/// the bisector of its unit chords to the nearest other points, which leans to neither chord's length;
/// at an end its one chord. A point where the water turns by more than `turn_max_deg` (at most
/// `SPIKE_DEG`) is a corner. The reader and the bake's added water vertices share this curve.
pub(crate) fn water_tangents(points: &[[f64; 2]], turn_max_deg: f64) -> Vec<(f64, bool)> {
    let cos_max = libm::cos(turn_max_deg.min(SPIKE_DEG).to_radians());
    let unit = |a: [f64; 2], b: [f64; 2]| {
        let d = [b[0] - a[0], b[1] - a[1]];
        let length = (d[0] * d[0] + d[1] * d[1]).sqrt();
        [d[0] / length, d[1] / length]
    };
    let tangent = |i: usize| {
        let here = points[i];
        let prev = points[..i].iter().rev().find(|&&q| q != here);
        match (prev, points[i + 1..].iter().find(|&&q| q != here)) {
            (Some(&a), Some(&b)) => {
                let (u, v) = (unit(a, here), unit(here, b));
                let sum = [u[0] + v[0], u[1] + v[1]];
                (if sum[0] * sum[0] + sum[1] * sum[1] > 1e-12 { sum } else { v }, u[0] * v[0] + u[1] * v[1] < cos_max)
            }
            (Some(&a), None) => (unit(a, here), false),
            (None, Some(&b)) => (unit(here, b), false),
            (None, None) => ([1.0, 0.0], false),
        }
    };
    (0..points.len()).map(tangent).map(|(t, corner)| (libm::atan2(t[1], t[0]), corner)).collect()
}

/// The most that this curve leaves its chord, in the units of `d`, at 32 steps of u.
pub(crate) fn off_chord(d: [f64; 2], t0: f64, t1: f64) -> f64 {
    let g = curve(d, t0, t1, 1.0);
    let unit = [g.d[0] / g.c, g.d[1] / g.c];
    let off = |j: u32| {
        let q = g.eval(f64::from(j) / 32.0)[0];
        (unit[0] * q[1] - unit[1] * q[0]).abs()
    };
    (1..32).map(off).fold(0.0, f64::max)
}

/// The shapes of the spans of water through `points`, with their tangents and corners
/// (`water_tangents`) and `metres` per unit of a span from its two points: per span None where it is
/// its chord (a corner at an end, no length, its curve leaves the chord by more than `off_max_m`, or it
/// folds back), else its end tangents and handle. A curve's end at a vertex whose other span is a chord
/// takes that chord's direction, so the outline turns smoothly there; where the curve then leaves its
/// chord by more than `off_max_m`, its tangents shorten until it leaves by that much. The curve's
/// distance from its chord scales with the handle.
pub(crate) fn water_spans(
    points: &[[f64; 2]],
    tangents: &[(f64, bool)],
    metres: impl Fn([f64; 2], [f64; 2]) -> f64,
    off_max_m: f64,
) -> Vec<Option<([f64; 2], f64)>> {
    let n = points.len().saturating_sub(1);
    let chord_of = |k: usize| [points[k + 1][0] - points[k][0], points[k + 1][1] - points[k][1]];
    let off_m = |k: usize, t0: f64, t1: f64| off_chord(chord_of(k), t0, t1) * metres(points[k], points[k + 1]);
    let chord: Vec<bool> = (0..n)
        .map(|k| {
            let (d, (t0, c0), (t1, c1)) = (chord_of(k), tangents[k], tangents[k + 1]);
            c0 || c1 || d == [0.0; 2] || off_m(k, t0, t1) > off_max_m || folds(d, t0, t1)
        })
        .collect();
    let along = |k: usize| {
        let d = chord_of(k);
        libm::atan2(d[1], d[0])
    };
    (0..n)
        .map(|k| {
            if chord[k] {
                return None;
            }
            let start = if k > 0 && chord[k - 1] { along(k - 1) } else { tangents[k].0 };
            let end = if k + 1 < n && chord[k + 1] { along(k + 1) } else { tangents[k + 1].0 };
            if folds(chord_of(k), start, end) {
                return None;
            }
            let off = off_m(k, start, end);
            Some(([start, end], if off > off_max_m { off_max_m / off } else { 1.0 }))
        })
        .collect()
}

/// The grades (rise per metre) of water at `points` with levels `z` along its line: the harmonic mean of
/// the grades of its two chords where they fall the same way, else 0, and at an end or beside an unknown
/// level its one chord's. Heights between two vertices then follow `water_level`, which never rises past
/// either level and turns smoothly at a vertex.
pub fn water_grades(points: &[[f64; 2]], z: &[Option<f64>], metres: impl Fn([f64; 2], [f64; 2]) -> f64) -> Vec<f64> {
    let chord = |k: usize| match (z[k], z[k + 1]) {
        (Some(a), Some(b)) => {
            let (p, q) = (points[k], points[k + 1]);
            let m = libm::hypot(q[0] - p[0], q[1] - p[1]) * metres(p, q);
            (m > 0.0).then(|| (b - a) / m)
        }
        _ => None,
    };
    let n = points.len();
    (0..n)
        .map(|k| {
            let before = if k > 0 { chord(k - 1) } else { None };
            let after = if k + 1 < n { chord(k) } else { None };
            match (before, after) {
                (Some(a), Some(b)) if a * b > 0.0 => 2.0 * a * b / (a + b),
                (Some(_), Some(_)) | (None, None) => 0.0,
                (Some(g), None) | (None, Some(g)) => g,
            }
        })
        .collect()
}

/// The level of water and its grade at share t of the arc of a span `len_m` long, between the levels
/// `z` and the grades `grade` (`water_grades`) of its vertices: the cubic Hermite of both.
pub fn water_level(z: [f64; 2], grade: [f64; 2], len_m: f64, t: f64) -> (f64, f64) {
    let (t2, t3) = (t * t, t * t * t);
    let h = [2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + t, 3.0 * t2 - 2.0 * t3, t3 - t2];
    let dh = [6.0 * t2 - 6.0 * t, 3.0 * t2 - 4.0 * t + 1.0, 6.0 * t - 6.0 * t2, 3.0 * t2 - 2.0 * t];
    let at = |w: [f64; 4]| w[0] * z[0] + w[1] * len_m * grade[0] + w[2] * z[1] + w[3] * len_m * grade[1];
    (at(h), at(dh) / len_m)
}

/// Per span of water through `points` its curve (`water_tangents`, `water_spans`), from the span's start,
/// with `metres` per unit of a span from its two points.
pub fn water_curves(
    points: &[[f64; 2]],
    turn_max_deg: f64,
    off_max_m: f64,
    metres: impl Fn([f64; 2], [f64; 2]) -> f64,
) -> Vec<WaterCurve> {
    let shapes = water_spans(points, &water_tangents(points, turn_max_deg), metres, off_max_m);
    let chord = |w: &[[f64; 2]]| [w[1][0] - w[0][0], w[1][1] - w[0][1]];
    points.windows(2).zip(shapes).map(|(w, shape)| WaterCurve::new(chord(w), shape)).collect()
}

/// The curve of a span of water with chord `d` and its shape from `water_spans` (None: its chord), in the
/// units of `d`, with its arc measured as a span's knots measure it.
pub struct WaterCurve {
    g: Geo,
    len: f64,
    knots: [f64; KNOTS + 1],
}

impl WaterCurve {
    fn new(d: [f64; 2], shape: Option<([f64; 2], f64)>) -> WaterCurve {
        let g = match shape {
            Some((ends, handle)) => curve(d, ends[0], ends[1], handle),
            None => Geo { chord: true, ..curve(d, 0.0, 0.0, 1.0) },
        };
        let (len, knots) = arc(&g);
        WaterCurve { g, len, knots }
    }

    /// The length of its arc in the units of `d`.
    pub fn length(&self) -> f64 {
        self.len
    }

    /// The point at a share of its arc's length, from its start.
    pub fn at_share(&self, share: f64) -> [f64; 2] {
        self.g.eval(u_of(&self.knots, self.len, share * self.len))[0]
    }

    /// The share of its arc's length at the point nearest to `p` (from its start), from a first share:
    /// the Newton steps of the reader's foot of a point on a span.
    pub fn share_near(&self, p: [f64; 2], share: f64) -> f64 {
        let mut u = u_of(&self.knots, self.len, share * self.len);
        for _ in 0..NEWTON_STEPS {
            let [q, d, dd] = self.g.eval(u);
            let v = [q[0] - p[0], q[1] - p[1]];
            let f = v[0] * d[0] + v[1] * d[1];
            let df = d[0] * d[0] + d[1] * d[1] + v[0] * dd[0] + v[1] * dd[1];
            if df <= 0.0 {
                break;
            }
            u = (u - f / df).clamp(0.0, 1.0);
        }
        if self.len > 0.0 {
            s_of(&self.knots, u) / self.len
        } else {
            0.0
        }
    }
}

/// Whether a tangent points back against the chord: the curve folds, and the span is its chord.
pub(crate) fn folds(d: [f64; 2], t0: f64, t1: f64) -> bool {
    let dot = |t: f64| libm::cos(t) * d[0] + libm::sin(t) * d[1];
    dot(t0) <= 0.0 || dot(t1) <= 0.0
}

/// The 5-point Simpson rule over an interval of width w of the values f at its quarters.
fn simpson(w: f64, f: &[f64]) -> f64 {
    w * (f[0] + 4.0 * f[1] + 2.0 * f[2] + 4.0 * f[3] + f[4]) / 12.0
}

/// The metres of a span: the bake's `arc`, one 5-point Simpson rule with the scale at the chord's middle.
pub(super) fn length(a: &Vertex, b: &Vertex, chord: bool) -> f64 {
    let g = geo(a, b, chord);
    if chord {
        return g.c * g.mpu;
    }
    simpson(1.0, &[0.0, 0.25, 0.5, 0.75, 1.0].map(|u| g.speed_m(u)))
}

impl Span {
    pub(super) fn new(a: &Vertex, b: &Vertex, chord: bool, along0_m: f64) -> Span {
        Span::shaped(a, b, chord, [a.tangent, b.tangent], 1.0, along0_m)
    }

    /// A span with its own end tangents and handle (`water_spans`).
    pub(super) fn shaped(a: &Vertex, b: &Vertex, chord: bool, ends: [f64; 2], handle: f64, along0_m: f64) -> Span {
        let (len_m, knots) = arc(&shaped(a, b, chord, ends, handle));
        Span { len_m, knots, along0_m, chord, ends, handle, water_z: false, owner: [0; 3] }
    }
}

/// The length of a curve in metres and its metres at the knots: a 5-point Simpson rule over the curve and
/// over each knot interval, scaled to the curve's length.
fn arc(g: &Geo) -> (f64, [f64; KNOTS + 1]) {
    let f: Vec<f64> = (0..=4 * KNOTS).map(|j| g.speed_m(j as f64 / (4 * KNOTS) as f64)).collect();
    let len_m = if g.chord { g.c * g.mpu } else { simpson(1.0, &[f[0], f[16], f[32], f[48], f[64]]) };
    let mut knots = [0.0; KNOTS + 1];
    for k in 0..KNOTS {
        let width = 1.0 / KNOTS as f64;
        knots[k + 1] = knots[k] + if g.chord { width } else { simpson(width, &f[4 * k..4 * k + 5]) };
    }
    let scale = len_m / knots[KNOTS];
    knots.iter_mut().for_each(|k| *k *= scale);
    knots[KNOTS] = len_m;
    (len_m, knots)
}

/// The span metres at u: linear between the knots.
pub fn s_at(span: &Span, u: f64) -> f64 {
    s_of(&span.knots, u)
}

fn s_of(knots: &[f64; KNOTS + 1], u: f64) -> f64 {
    let x = u.clamp(0.0, 1.0) * KNOTS as f64;
    let k = (x as usize).min(KNOTS - 1);
    knots[k] + (x - k as f64) * (knots[k + 1] - knots[k])
}

/// The u at span metres s: the inverse of `s_at`.
pub fn u_at(span: &Span, s: f64) -> f64 {
    u_of(&span.knots, span.len_m, s)
}

fn u_of(knots: &[f64; KNOTS + 1], len_m: f64, s: f64) -> f64 {
    let s = s.clamp(0.0, len_m);
    let k = knots[1..KNOTS].partition_point(|&knot| knot <= s);
    let (a, b) = (knots[k], knots[k + 1]);
    let f = if b > a { (s - a) / (b - a) } else { 0.0 };
    ((k as f64 + f) / KNOTS as f64).clamp(0.0, 1.0)
}

/// The point of span `span` of a part at u.
pub fn point(part: &Part, span: usize, u: f64) -> SpanPoint {
    let (a, b) = (&part.vertices[span], &part.vertices[span + 1]);
    let data = &part.spans[span];
    point_of(&Of { g: geo_of(a, b, data), a, b, span: data }, u)
}

/// The global point and the height at u of a span from its vertices.
pub(super) fn level_at(a: &Vertex, b: &Vertex, span: &Span, u: f64) -> ([i64; 2], f64) {
    let g = geo_of(a, b, span);
    let p = point_of(&Of { g, a, b, span }, u);
    (round(&g, p.d), p.z_m)
}

/// A span with its vertices and its plan.
struct Of<'a> {
    g: Geo,
    a: &'a Vertex,
    b: &'a Vertex,
    span: &'a Span,
}

fn point_of(of: &Of, u: f64) -> SpanPoint {
    let (g, a, b) = (&of.g, of.a, of.b);
    let [d, dp, _] = g.eval(u);
    let length = (dp[0] * dp[0] + dp[1] * dp[1]).sqrt();
    let tangent = if length > 0.0 { [dp[0] / length, dp[1] / length] } else { [1.0, 0.0] };
    let c_m = g.c * g.mpu;
    let (z_m, grade) = if of.span.water_z {
        let len = of.span.len_m;
        water_level([a.z_m, b.z_m], [a.grade, b.grade], len, s_at(of.span, u) / len)
    } else if g.chord {
        (a.z_m + u * (b.z_m - a.z_m), (b.z_m - a.z_m) / c_m)
    } else {
        let (u2, u3) = (u * u, u * u * u);
        let h = [2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, 3.0 * u2 - 2.0 * u3, u3 - u2];
        let dh = [6.0 * u2 - 6.0 * u, 3.0 * u2 - 4.0 * u + 1.0, 6.0 * u - 6.0 * u2, 3.0 * u2 - 2.0 * u];
        let z = |w: [f64; 4]| w[0] * a.z_m + w[1] * c_m * a.grade + w[2] * b.z_m + w[3] * c_m * b.grade;
        (z(h), z(dh) / (length * g.mpu))
    };
    let lerp = |p: f64, q: f64| p + u * (q - p);
    SpanPoint { d, tangent, z_m, grade, bank: lerp(a.bank, b.bank), widen_m: lerp(a.widen_m, b.widen_m) }
}

/// The global position of a point at u: p0 + δ rounded half away from zero.
pub(super) fn round(g: &Geo, d: [f64; 2]) -> [i64; 2] {
    [g.p0[0] + libm::round(d[0]) as i64, g.p0[1] + libm::round(d[1]) as i64]
}

/// The u where the curve crosses the line `axis` = value: the sign at the 17 knots, then
/// `ROOT_STEPS` bisections in each interval with a change. A touch without a change is no root.
pub(super) fn line_roots(g: &Geo, axis: usize, value: i64) -> Vec<f64> {
    let offset = (value - g.p0[axis]) as f64;
    let f = |u: f64| g.eval(u)[0][axis] - offset;
    let sign = |u: f64| {
        let v = f(u);
        i8::from(v > 0.0) - i8::from(v < 0.0)
    };
    let signs: Vec<i8> = (0..=KNOTS).map(|k| sign(k as f64 / KNOTS as f64)).collect();
    let mut roots = Vec::new();
    for k in 0..KNOTS {
        if k > 0 && signs[k] == 0 && signs[k - 1] * signs[k + 1] < 0 {
            roots.push(k as f64 / KNOTS as f64);
        }
        if signs[k] * signs[k + 1] < 0 {
            let (mut lo, mut hi) = (k as f64 / KNOTS as f64, (k + 1) as f64 / KNOTS as f64);
            for _ in 0..ROOT_STEPS {
                let mid = (lo + hi) / 2.0;
                if sign(mid) == signs[k] {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            roots.push((lo + hi) / 2.0);
        }
    }
    roots
}

/// One sample before evaluation: u, span metres, kind and global position.
#[derive(Clone, Copy, Debug)]
struct Pt {
    u: f64,
    s: f64,
    kind: SampleKind,
    pos: [i64; 2],
}

/// Whether the box of a span's control points meets the closed square.
pub(super) fn meets(part: &Part, span: usize, [x0, y0, x1, y1]: [i64; 4]) -> bool {
    let g = geo_of(&part.vertices[span], &part.vertices[span + 1], &part.spans[span]);
    let hull = g.hull();
    let lo = |i: usize| hull.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min).floor() as i64 + g.p0[i];
    let hi = |i: usize| hull.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64 + g.p0[i];
    hi(0) >= x0 && lo(0) <= x1 && hi(1) >= y0 && lo(1) <= y1
}

/// The samples of every span of a part whose control points' box meets the work area, ordered by
/// (span, u), and which spans those are. A vertex is (v, 0), but the part's last one.
pub(super) fn samples(part: &Part, rule: &Rule<'_>, cell: CellKey, area: [i64; 4]) -> (Vec<Sample>, Vec<bool>) {
    let mut out: Vec<Sample> = Vec::new();
    let mut sampled = vec![false; part.spans.len()];
    for span in 0..part.spans.len() {
        let before = span > 0 && sampled[span - 1];
        if !meets(part, span, area) {
            if let (true, Some(last)) = (before, out.last_mut()) {
                *last = Sample { span: span as u32, u: 0.0, s_m: 0.0, walk: part.vertices[span].walk, ..last.clone() };
            }
            continue;
        }
        if before {
            out.pop();
        }
        let g = geo_of(&part.vertices[span], &part.vertices[span + 1], &part.spans[span]);
        out.extend(span_samples(part, rule, span).into_iter().map(|pt| sample(part, g, span, pt, cell)));
        sampled[span] = true;
    }
    (out, sampled)
}

/// Insert a `Station` sample at a station of a sampled span, and give the station that it takes: a
/// sample other than a grid point within `MIN_STEP_M / 2` serves instead, and grid points that close go.
pub(super) fn insert(part: &Part, samples: &mut Vec<Sample>, st: super::Station, cell: CellKey) -> super::Station {
    let span = &part.spans[st.span as usize];
    let s = s_at(span, st.u);
    let near = MIN_STEP_M / 2.0;
    let vertex = |span: u32, u: f64| super::points::norm(part, super::Station { span, u, ..st });
    if s < near {
        return vertex(st.span, 0.0);
    }
    if span.len_m - s < near {
        return vertex(st.span, 1.0);
    }
    let same = |x: &Sample| x.span == st.span && x.u > 0.0 && (x.s_m - s).abs() < near;
    if let Some(x) = samples.iter().find(|x| same(x) && x.kind != SampleKind::Grid) {
        return super::Station { u: x.u, ..st };
    }
    samples.retain(|x| !same(x));
    let at = samples.partition_point(|x| (x.span, x.u) < (st.span, st.u));
    samples.insert(at, station_sample(part, st.span as usize, st.u, cell));
    st
}

/// A sample at a station of a span: the curve point rounded.
pub(super) fn station_sample(part: &Part, span: usize, u: f64, cell: CellKey) -> Sample {
    let g = geo_of(&part.vertices[span], &part.vertices[span + 1], &part.spans[span]);
    let pos = round(&g, g.eval(u)[0]);
    sample(part, g, span, Pt { u, s: s_at(&part.spans[span], u), kind: SampleKind::Station, pos }, cell)
}

fn sample(part: &Part, g: Geo, span: usize, pt: Pt, cell: CellKey) -> Sample {
    let (a, b) = (&part.vertices[span], &part.vertices[span + 1]);
    let p = point_of(&Of { g, a, b, span: &part.spans[span] }, pt.u);
    let o = cell.origin();
    let sign = if part.backward { -1.0 } else { 1.0 };
    Sample {
        pos: [(pt.pos[0] - o.x) as i32, (pt.pos[1] - o.y) as i32],
        span: span as u32,
        u: pt.u,
        s_m: pt.s,
        along_m: part.spans[span].along0_m + sign * pt.s,
        z_m: p.z_m,
        tangent: p.tangent,
        grade: p.grade,
        bank: p.bank,
        widen_m: p.widen_m,
        walk: a.walk,
        kind: pt.kind,
    }
}

/// The samples of one span: its vertices, its cell-line breaks and the dyadic grid refined until each
/// chord holds the sag rules.
fn span_samples(part: &Part, rule: &Rule<'_>, span: usize) -> Vec<Pt> {
    let (a, b) = (&part.vertices[span], &part.vertices[span + 1]);
    let data = &part.spans[span];
    let g = geo_of(a, b, data);
    let at = |u: f64, kind: SampleKind, pos: [i64; 2]| Pt { u, s: s_at(data, u), kind, pos };
    let mut breaks = vec![at(0.0, SampleKind::Vertex, a.pos), at(1.0, SampleKind::Vertex, b.pos)];
    let hull = g.hull();
    for axis in 0..2 {
        let lo = hull.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min).floor() as i64 + g.p0[axis];
        let hi = hull.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64 + g.p0[axis];
        for line in lo.div_euclid(CELL_UNITS) + 1..=hi.div_euclid(CELL_UNITS) {
            for u in line_roots(&g, axis, line * CELL_UNITS) {
                let mut pos = round(&g, g.eval(u)[0]);
                pos[axis] = line * CELL_UNITS;
                breaks.push(at(u, SampleKind::CellLine, pos));
            }
        }
    }
    breaks.sort_by(|p, q| p.u.total_cmp(&q.u));
    let mut merged: Vec<Pt> = Vec::with_capacity(breaks.len());
    for p in breaks {
        match merged.last_mut() {
            Some(last) if last.pos == p.pos => {
                if p.kind == SampleKind::Vertex {
                    *last = p;
                }
            }
            _ => merged.push(p),
        }
    }
    let sag = Sag::of(rule);
    let of = Of { g, a, b, span: data };
    let near_break = |s: f64| merged.iter().any(|p| (p.s - s).abs() < MIN_STEP_M / 2.0);
    let grid = |level: u32, from: f64, to: f64| -> Vec<Pt> {
        let step = BASE_STEP_M / f64::from(1u32 << level);
        let mut k = (from / step).floor() as i64 + 1;
        let mut out = Vec::new();
        while (k as f64) * step < to {
            let s = k as f64 * step;
            if s > from && !near_break(s) {
                let u = u_at(data, s);
                out.push(Pt { u, s, kind: SampleKind::Grid, pos: round(&g, g.eval(u)[0]) });
            }
            k += 1;
        }
        out
    };
    let mut out = Vec::new();
    for pair in merged.windows(2) {
        let (p, q) = (pair[0], pair[1]);
        out.push(p);
        let mut prev = p;
        for r in grid(0, p.s, q.s) {
            refine(&of, &sag, prev, r, 0, &grid, &mut out);
            out.push(r);
            prev = r;
        }
        refine(&of, &sag, prev, q, 0, &grid, &mut out);
    }
    out.extend(merged.last().copied());
    out
}

/// The sag rules of a part: its outer half-width, and the most that a chord may leave the curve.
struct Sag {
    outer_m: f64,
    plan_m: f64,
    height_m: f64,
}

impl Sag {
    fn of(rule: &Rule<'_>) -> Sag {
        match rule {
            Rule::Way(s, _) => Sag { outer_m: s.outer_m(), plan_m: s.sag_plan_m, height_m: s.sag_height_m },
            Rule::Water(w) => Sag { outer_m: w.half_m, plan_m: w.sag_plan_m, height_m: w.sag_height_m },
        }
    }
}

/// Push the grid points strictly between p and q that the sag test asks for, in order.
fn refine(
    of: &Of,
    sag: &Sag,
    p: Pt,
    q: Pt,
    mut level: u32,
    grid: &dyn Fn(u32, f64, f64) -> Vec<Pt>,
    out: &mut Vec<Pt>,
) {
    while level < LEVELS && sags(of, sag, p.u, q.u) {
        level += 1;
        let inner = grid(level, p.s, q.s);
        if inner.is_empty() {
            continue;
        }
        let mut prev = p;
        for r in inner {
            refine(of, sag, prev, r, level, grid, out);
            out.push(r);
            prev = r;
        }
        refine(of, sag, prev, q, level, grid, out);
        return;
    }
}

/// Whether the chord from u0 to u1 leaves the curve by more than the sag rules: in plan at the
/// outer edge, or in height.
fn sags(of: &Of, sag: &Sag, u0: f64, u1: f64) -> bool {
    let g = &of.g;
    let (p, q) = (point_of(of, u0), point_of(of, u1));
    let chord = [q.d[0] - p.d[0], q.d[1] - p.d[1]];
    let length2 = chord[0] * chord[0] + chord[1] * chord[1];
    let mut plan: f64 = 0.0;
    for f in [0.25, 0.5, 0.75] {
        let m = g.eval(u0 + f * (u1 - u0))[0];
        let v = [m[0] - p.d[0], m[1] - p.d[1]];
        let t = if length2 > 0.0 { ((v[0] * chord[0] + v[1] * chord[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
        let e = [v[0] - t * chord[0], v[1] - t * chord[1]];
        plan = plan.max((e[0] * e[0] + e[1] * e[1]).sqrt() * g.mpu);
    }
    let cross = p.tangent[0] * q.tangent[1] - p.tangent[1] * q.tangent[0];
    let dot = p.tangent[0] * q.tangent[0] + p.tangent[1] * q.tangent[1];
    let turn = libm::atan2(cross.abs(), dot);
    plan += (sag.outer_m + p.widen_m.max(q.widen_m)) * turn * turn / 8.0;
    let mid = point_of(of, (u0 + u1) / 2.0);
    plan > sag.plan_m || (mid.z_m - (p.z_m + q.z_m) / 2.0).abs() > sag.height_m
}
