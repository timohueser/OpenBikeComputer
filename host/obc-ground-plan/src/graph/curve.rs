//! The curve of a span: the cubic Hermite of format 4 in frame units, its arc metres, its roots on
//! the cell lines, and its samples on a dyadic grid of span metres.

use super::parts::Rule;
use super::{Part, Sample, SampleKind, Span, Vertex, BASE_STEP_M, KNOTS, LEVELS, MIN_STEP_M, ROOT_STEPS};
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
    /// Metres per frame unit at the chord's middle.
    pub mpu: f64,
    chord: bool,
}

pub(super) fn geo(a: &Vertex, b: &Vertex, chord: bool) -> Geo {
    let d = [(b.pos[0] - a.pos[0]) as f64, (b.pos[1] - a.pos[1]) as f64];
    let c = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let unit = |angle: f64| [libm::cos(angle), libm::sin(angle)];
    let mpu = metres_per_unit((a.pos[1] + b.pos[1]).div_euclid(2));
    Geo { p0: a.pos, d, c, t0: unit(a.tangent), t1: unit(b.tangent), mpu, chord }
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
        let at = |w: [f64; 3], i: usize| w[0] * self.c * self.t0[i] + w[1] * self.d[i] + w[2] * self.c * self.t1[i];
        [[at(h, 0), at(h, 1)], [at(dh, 0), at(dh, 1)], [at(ddh, 0), at(ddh, 1)]]
    }

    fn speed_m(&self, u: f64) -> f64 {
        let d = if self.chord {
            self.d
        } else {
            let u2 = u * u;
            let dh = [3.0 * u2 - 4.0 * u + 1.0, 6.0 * u - 6.0 * u2, 3.0 * u2 - 2.0 * u];
            let at = |i: usize| dh[0] * self.c * self.t0[i] + dh[1] * self.d[i] + dh[2] * self.c * self.t1[i];
            [at(0), at(1)]
        };
        (d[0] * d[0] + d[1] * d[1]).sqrt() * self.mpu
    }

    /// The Bézier control points from p0: the curve lies in their hull.
    pub fn hull(&self) -> [[f64; 2]; 4] {
        if self.chord {
            return [[0.0; 2], self.d, self.d, self.d];
        }
        let k = self.c / 3.0;
        [[0.0; 2], [k * self.t0[0], k * self.t0[1]], [self.d[0] - k * self.t1[0], self.d[1] - k * self.t1[1]], self.d]
    }
}

/// The most that the curve of a span leaves its chord, in metres, at 32 steps of u.
pub(super) fn off_chord_m(a: &Vertex, b: &Vertex) -> f64 {
    let g = geo(a, b, false);
    let unit = [g.d[0] / g.c, g.d[1] / g.c];
    let off = |j: u32| {
        let q = g.eval(f64::from(j) / 32.0)[0];
        (unit[0] * q[1] - unit[1] * q[0]).abs()
    };
    (1..32).map(off).fold(0.0, f64::max) * g.mpu
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
        let g = geo(a, b, chord);
        let f: Vec<f64> = (0..=4 * KNOTS).map(|j| g.speed_m(j as f64 / (4 * KNOTS) as f64)).collect();
        let len_m = if chord { g.c * g.mpu } else { simpson(1.0, &[f[0], f[16], f[32], f[48], f[64]]) };
        let mut knots = [0.0; KNOTS + 1];
        for k in 0..KNOTS {
            let width = 1.0 / KNOTS as f64;
            knots[k + 1] = knots[k] + if chord { width } else { simpson(width, &f[4 * k..4 * k + 5]) };
        }
        let scale = len_m / knots[KNOTS];
        knots.iter_mut().for_each(|k| *k *= scale);
        knots[KNOTS] = len_m;
        Span { len_m, knots, along0_m, chord, linear_z: false, owner: [0; 3] }
    }
}

/// The span metres at u: linear between the knots.
pub fn s_at(span: &Span, u: f64) -> f64 {
    let x = u.clamp(0.0, 1.0) * KNOTS as f64;
    let k = (x as usize).min(KNOTS - 1);
    span.knots[k] + (x - k as f64) * (span.knots[k + 1] - span.knots[k])
}

/// The u at span metres s: the inverse of `s_at`.
pub fn u_at(span: &Span, s: f64) -> f64 {
    let s = s.clamp(0.0, span.len_m);
    let k = span.knots[1..KNOTS].partition_point(|&knot| knot <= s);
    let (a, b) = (span.knots[k], span.knots[k + 1]);
    let f = if b > a { (s - a) / (b - a) } else { 0.0 };
    ((k as f64 + f) / KNOTS as f64).clamp(0.0, 1.0)
}

/// The point of span `span` of a part at u.
pub fn point(part: &Part, span: usize, u: f64) -> SpanPoint {
    let (a, b) = (&part.vertices[span], &part.vertices[span + 1]);
    let data = &part.spans[span];
    point_of(&Of { g: geo(a, b, data.chord), a, b, span: data }, u)
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
    let (z_m, grade) = if of.span.linear_z {
        let len = of.span.len_m;
        (a.z_m + (b.z_m - a.z_m) * s_at(of.span, u) / len, (b.z_m - a.z_m) / len)
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
    let g = geo(&part.vertices[span], &part.vertices[span + 1], part.spans[span].chord);
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
        let g = geo(&part.vertices[span], &part.vertices[span + 1], part.spans[span].chord);
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
    let g = geo(&part.vertices[span], &part.vertices[span + 1], part.spans[span].chord);
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

/// The samples of one span: its vertices, its cell-line breaks and, but for water, the dyadic grid
/// refined until each chord holds the sag rules.
fn span_samples(part: &Part, rule: &Rule<'_>, span: usize) -> Vec<Pt> {
    let (a, b) = (&part.vertices[span], &part.vertices[span + 1]);
    let data = &part.spans[span];
    let g = geo(a, b, data.chord);
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
            Rule::Way(s, _) => Sag { outer_m: s.widths().outer_m(), plan_m: s.sag_plan_m, height_m: s.sag_height_m },
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
