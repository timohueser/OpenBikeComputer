//! The sections of each way at its samples, and the band candidates: span pieces per layer, end caps
//! and joint fans.

use core::f64::consts::PI;

use super::cands::{Cands, Key, Label, KIND_WAY};
use super::geom::{cross, round, P};
use super::rules::Rank;
use super::types::{HeightRule, Owner, Surface};
use super::Ctx;
use crate::frame::{metres_per_unit, GlobalPoint};
use crate::graph::Family;
use crate::sections::{Finish, WaySection};

pub const LEFT: usize = 0;
pub const RIGHT: usize = 1;
/// The most that one step of a cap or fan turns.
const STEP_DEG: f64 = 10.0;
/// Quads per strip piece: a piece's box stays small for the label grid.
const STRIP_MAX: usize = 16;
/// A way with no sample this near the window makes nothing in it: a span is at most 16 m from a
/// sample, a band and a corner reach 20 m + 15 m, an extent 20 m.
const FAR_M: f64 = 64.0;

/// A way's section at one sample. `o[side][k]` are the offsets in metres of the carriageway edge,
/// the shoulder's and the verge's outer edges; `p` the six boundary points by signed offset along the
/// left normal (right verge, right shoulder, right edge, left edge, left shoulder, left verge).
#[derive(Clone, Debug, PartialEq)]
pub struct S {
    pub g: [i64; 2],
    pub t: [f64; 2],
    /// span + u: the order along the part.
    pub ord: f64,
    pub along_m: f64,
    pub z: f64,
    pub bank: f64,
    pub walk: u8,
    pub o: [[f64; 3]; 2],
    pub p: [P; 6],
    pub c: P,
    pub gid: [i64; 5],
}

/// The fields of a way's class that the faces read.
#[derive(Clone, Debug)]
pub struct WaySec {
    pub way: u32,
    pub class: u8,
    pub group: String,
    pub rail: bool,
    pub half_m: f64,
    pub shoulder_m: f64,
    pub shoulder_paved: bool,
    pub verge_m: f64,
    pub reach_m: f64,
    pub inner_radius_m: f64,
    pub return_grade: f64,
    pub s: Vec<S>,
}

/// The left normal of a tangent: the spec's left, x east, y south.
pub fn normal(t: [f64; 2]) -> [f64; 2] {
    [t[1], -t[0]]
}

/// A global point plus `m` metres along the unit vector d, rounded, in the cell frame.
pub fn offset(ctx_origin: GlobalPoint, g: [i64; 2], d: [f64; 2], m: f64) -> P {
    let u = m / metres_per_unit(g[1]);
    let x = g[0] + round(u * d[0]) - ctx_origin.x;
    let y = g[1] + round(u * d[1]) - ctx_origin.y;
    [x as i32, y as i32]
}

impl S {
    /// The six boundary points and the centre from `g`, `t` and `o`.
    pub fn place(&mut self, origin: GlobalPoint) {
        let n = normal(self.t);
        let signed = [
            -self.o[RIGHT][2],
            -self.o[RIGHT][1],
            -self.o[RIGHT][0],
            self.o[LEFT][0],
            self.o[LEFT][1],
            self.o[LEFT][2],
        ];
        for (p, m) in self.p.iter_mut().zip(signed) {
            *p = offset(origin, self.g, n, m);
        }
        self.c = [(self.g[0] - origin.x) as i32, (self.g[1] - origin.y) as i32];
    }

    /// The boundary index of layer edge k (0 carriageway, 1 shoulder, 2 verge) on a side.
    pub fn bound(side: usize, k: usize) -> usize {
        if side == LEFT {
            3 + k
        } else {
            2 - k
        }
    }

    /// The sample at fraction f from a to b: positions rounded, the rest linear.
    pub fn lerp(a: &S, b: &S, f: f64, origin: GlobalPoint) -> S {
        let mix = |x: f64, y: f64| x + (y - x) * f;
        let g = [a.g[0] + round(f * (b.g[0] - a.g[0]) as f64), a.g[1] + round(f * (b.g[1] - a.g[1]) as f64)];
        let t = [mix(a.t[0], b.t[0]), mix(a.t[1], b.t[1])];
        let len = (t[0] * t[0] + t[1] * t[1]).sqrt();
        let mut o = a.o;
        for (side, row) in o.iter_mut().enumerate() {
            for (k, v) in row.iter_mut().enumerate() {
                *v = mix(a.o[side][k], b.o[side][k]);
            }
        }
        let mut s = S {
            g,
            t: [t[0] / len, t[1] / len],
            ord: mix(a.ord, b.ord),
            along_m: mix(a.along_m, b.along_m),
            z: mix(a.z, b.z),
            bank: mix(a.bank, b.bank),
            walk: a.walk,
            o,
            p: [[0; 2]; 6],
            c: [0; 2],
            gid: a.gid,
        };
        s.place(origin);
        s
    }
}

/// The layers of a way's cross-section on each side, out from its middle line: (surface, width in
/// metres, paved). The faces are mirror-symmetric; GP4 adds the kerb and the sidewalk by `walk`.
pub fn layers(sec: &WaySection, widen_m: f64) -> [(Surface, f64, bool); 3] {
    let rail = matches!(sec.finish, Finish::Rail | Finish::Narrow | Finish::Rack | Finish::NarrowRack);
    let carriage = if rail { Surface::RailBed } else { Surface::Carriageway };
    [
        (carriage, sec.half_m + widen_m, !rail),
        (Surface::Shoulder, sec.shoulder_m, sec.shoulder_paved),
        (Surface::Verge, sec.verge_m, false),
    ]
}

/// Metres between two global points.
pub fn metres(a: [i64; 2], b: [i64; 2]) -> f64 {
    let (dx, dy) = ((b[0] - a[0]) as f64, (b[1] - a[1]) as f64);
    (dx * dx + dy * dy).sqrt() * metres_per_unit((a[1] + b[1]) / 2)
}

/// The sections of every way of the level.
pub fn sections(ctx: &Ctx) -> Vec<Option<WaySec>> {
    let g = ctx.graph;
    g.ways
        .iter()
        .map(|way| {
            if !ctx.in_level(way.id) {
                return None;
            }
            let part = &g.parts[way.part as usize];
            let feature = &ctx.input.features[part.feature.0 as usize];
            let sec = ctx.rules.sections.way(&feature.kind, &feature.tags, false);
            let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
            let o = ctx.cell.origin();
            // A way farther from the window than any of its shapes reaches makes nothing in it.
            let [x0, y0, x1, y1] = ctx.window;
            let m = (FAR_M / metres_per_unit(o.y)) as i32;
            let near = |p: &[i32; 2]| p[0] > x0 - m && p[0] < x1 + m && p[1] > y0 - m && p[1] < y1 + m;
            if !samples.iter().any(|s| near(&s.pos)) {
                return None;
            }
            let s = samples
                .iter()
                .map(|x| {
                    let [(_, c, _), (_, sh, _), (_, ve, _)] = layers(&sec, x.widen_m);
                    let row = [c, c + sh, c + sh + ve];
                    let span = &part.spans[x.span as usize];
                    let v = part.vertices[x.span as usize].pos;
                    let gid =
                        [i64::from(span.owner[0]), i64::from(span.owner[1]), i64::from(span.owner[2]), v[0], v[1]];
                    let mut s = S {
                        g: [o.x + i64::from(x.pos[0]), o.y + i64::from(x.pos[1])],
                        t: x.tangent,
                        ord: f64::from(x.span) + x.u,
                        along_m: x.along_m,
                        z: x.z_m,
                        bank: x.bank,
                        walk: x.walk,
                        o: [row, row],
                        p: [[0; 2]; 6],
                        c: [0; 2],
                        gid,
                    };
                    s.place(o);
                    s
                })
                .collect();
            Some(WaySec {
                way: way.id,
                class: part.class,
                group: sec.group.to_string(),
                rail: part.family == Family::Rail,
                half_m: sec.half_m,
                shoulder_m: sec.shoulder_m,
                shoulder_paved: sec.shoulder_paved,
                verge_m: sec.verge_m,
                reach_m: sec.reach_m,
                inner_radius_m: sec.inner_radius_m,
                return_grade: sec.return_grade,
                s,
            })
        })
        .collect()
}

/// The label and key of a way's layer l (0 right verge … 4 left verge) at a sample.
pub fn layer_label(ctx: &Ctx, w: &WaySec, l: usize) -> (Label, Rank) {
    let (surface, rule, rank) = match l {
        2 if w.rail => (Surface::RailBed, HeightRule::Owner, Rank::RailBed),
        2 => (Surface::Carriageway, HeightRule::Owner, Rank::Carriageway),
        1 | 3 if w.shoulder_paved => (Surface::Shoulder, HeightRule::Owner, Rank::ShoulderPaved),
        1 | 3 => (Surface::Shoulder, HeightRule::Owner, Rank::Shoulder),
        _ => (Surface::Verge, HeightRule::Slope, Rank::Verge),
    };
    let label = Label {
        owner: Owner::Way(w.way),
        surface,
        rule,
        material: Some(w.way),
        crossing: None,
        rank: ctx.rules.plan.rank(rank),
        reach_m: w.reach_m as f32,
    };
    (label, rank)
}

pub fn way_key(ctx: &Ctx, w: &WaySec, rank: Rank, gid: [i64; 5], number: u32) -> Key {
    Key { rank: ctx.rules.plan.rank(rank), class: w.class, kind: KIND_WAY, gid, number }
}

/// Strips of convex positive quads; any other quad goes as two triangles.
struct Strip {
    a: Vec<P>,
    b: Vec<P>,
    key: Option<Key>,
}

impl Strip {
    fn flush(&mut self, cands: &mut Cands, cand: u32) {
        if let (Some(key), true) = (self.key, self.a.len() >= 2) {
            let mut ring = vec![self.a[0]];
            ring.extend(self.b.iter().copied());
            ring.extend(self.a[1..].iter().rev().copied());
            cands.ring(cand, key, ring);
        }
        self.a.clear();
        self.b.clear();
        self.key = None;
    }

    fn quad(&mut self, cands: &mut Cands, cand: u32, key: Key, q: [P; 4]) {
        let [a0, b0, b1, a1] = q;
        let convex = (0..4).all(|i| cross(q[i], q[(i + 1) % 4], q[(i + 2) % 4]) > 0);
        let joins = self.key == Some(key) && self.a.last() == Some(&a0) && self.b.last() == Some(&b0);
        if !convex || !joins || self.a.len() > STRIP_MAX {
            self.flush(cands, cand);
        }
        if !convex {
            cands.ring(cand, key, vec![a0, b0, b1]);
            cands.ring(cand, key, vec![a0, b1, a1]);
            return;
        }
        if self.a.is_empty() {
            self.a.push(a0);
            self.b.push(b0);
            self.key = Some(key);
        }
        self.a.push(a1);
        self.b.push(b1);
    }
}

/// A way's band candidates: one per layer, with the layer's pieces between consecutive samples. A
/// layer that `cut` names on an interval has no piece there; a carriageway interval of `take` goes to
/// that candidate instead (a blend part).
pub fn bands(
    ctx: &Ctx,
    cands: &mut Cands,
    w: &WaySec,
    layer_cands: &mut [Option<u32>; 5],
    cut: &dyn Fn(usize, f64, f64) -> bool,
    take: &dyn Fn(f64, f64) -> Option<(u32, Key)>,
) {
    for l in 0..5 {
        let mut strip = Strip { a: Vec::new(), b: Vec::new(), key: None };
        let mut last = None;
        for i in 0..w.s.len().saturating_sub(1) {
            let (s0, s1) = (&w.s[i], &w.s[i + 1]);
            if s0.p[l] == s0.p[l + 1] && s1.p[l] == s1.p[l + 1] || cut(l, s0.ord, s1.ord) {
                continue;
            }
            let (cand, key) = match (l, take(s0.ord, s1.ord)) {
                (2, Some(taken)) => taken,
                _ => {
                    let rank = layer_label(ctx, w, l).1;
                    (layer_cand(ctx, cands, w, layer_cands, l), way_key(ctx, w, rank, s0.gid, l as u32))
                }
            };
            if let Some(c) = last.filter(|&c| c != cand) {
                strip.flush(cands, c);
            }
            last = Some(cand);
            strip.quad(cands, cand, key, [s0.p[l], s0.p[l + 1], s1.p[l + 1], s1.p[l]]);
        }
        if let Some(c) = last {
            strip.flush(cands, c);
        }
    }
}

pub fn layer_cand(ctx: &Ctx, cands: &mut Cands, w: &WaySec, layer_cands: &mut [Option<u32>; 5], l: usize) -> u32 {
    *layer_cands[l].get_or_insert_with(|| {
        let (label, rank) = layer_label(ctx, w, l);
        cands.add(label, way_key(ctx, w, rank, w.s[0].gid, l as u32))
    })
}

/// The points of a section turning about centre c through `n` steps: per step j its direction and the
/// signed offset of each boundary on the side it faces, from section a's boundaries to section b's.
/// Points 0 and n are a's and b's points themselves.
#[allow(clippy::too_many_arguments)]
pub fn sweep(
    origin: GlobalPoint,
    centre: [i64; 2],
    dirs: &[[f64; 2]],
    from: [P; 3],
    to: [P; 3],
    from_m: [f64; 3],
    to_m: [f64; 3],
) -> Vec<[P; 3]> {
    let n = dirs.len() - 1;
    (0..=n)
        .map(|j| {
            if j == 0 {
                return from;
            }
            if j == n {
                return to;
            }
            let f = j as f64 / n as f64;
            std::array::from_fn(|k| offset(origin, centre, dirs[j], from_m[k] + (to_m[k] - from_m[k]) * f))
        })
        .collect()
}

/// Unit directions from u turning by `angle` radians in `n` equal steps.
pub fn turn(u: [f64; 2], angle: f64, n: usize) -> Vec<[f64; 2]> {
    (0..=n)
        .map(|j| {
            let a = angle * j as f64 / n as f64;
            let (s, c) = (libm::sin(a), libm::cos(a));
            [u[0] * c - u[1] * s, u[0] * s + u[1] * c]
        })
        .collect()
}

pub fn steps(angle: f64, even: bool) -> usize {
    let n = (angle.abs() / STEP_DEG.to_radians()).ceil().max(1.0) as usize;
    if even {
        n + n % 2
    } else {
        n
    }
}

/// The pieces of a swept fan: per step the carriageway's triangle from the centre and the outer
/// layers' quads, to the candidates of `owner(j)`: (carriageway candidate, layer candidates by k).
pub fn fan_pieces(cands: &mut Cands, c: P, pts: &[[P; 3]], owner: &dyn Fn(usize) -> (u32, [Option<u32>; 2], [Key; 3])) {
    for j in 0..pts.len() - 1 {
        let (carriage, outer, keys) = owner(j);
        cands.ring(carriage, keys[0], vec![c, pts[j][0], pts[j + 1][0]]);
        for k in 1..3 {
            if let Some(cand) = outer[k - 1] {
                cands.ring(cand, keys[k], vec![pts[j][k - 1], pts[j][k], pts[j + 1][k], pts[j + 1][k - 1]]);
            }
        }
    }
}

/// A free end's cap: the section turns 180° about the end point, its offsets linear from the left
/// to the right. `last`: the way ends here (the cap faces along the tangent).
pub fn cap(ctx: &Ctx, cands: &mut Cands, w: &WaySec, layer_cands: &mut [Option<u32>; 5], last: bool) {
    let s = if last { w.s.last() } else { w.s.first() }.unwrap();
    let n = normal(s.t);
    let sign = if last { 1.0 } else { -1.0 };
    let count = steps(PI, true);
    let dirs: Vec<[f64; 2]> = (0..=count)
        .map(|j| {
            let a = PI * j as f64 / count as f64;
            let (si, co) = (libm::sin(a), libm::cos(a));
            [co * n[0] + sign * si * s.t[0], co * n[1] + sign * si * s.t[1]]
        })
        .collect();
    let from = [s.p[3], s.p[4], s.p[5]];
    let to = [s.p[2], s.p[1], s.p[0]];
    let pts = sweep(ctx.origin, s.g, &dirs, from, to, s.o[LEFT], s.o[RIGHT]);
    let mut ids = [[None; 2]; 2];
    let mut keys = [[Key::TERRAIN; 3]; 2];
    let carriage = layer_cand(ctx, cands, w, layer_cands, 2);
    for (side, ls) in [(LEFT, [3, 4]), (RIGHT, [1, 0])] {
        for (k, &l) in ls.iter().enumerate() {
            if s.p[l] != s.p[l + 1] {
                ids[side][k] = Some(layer_cand(ctx, cands, w, layer_cands, l));
            }
        }
        for (k, l) in [2, ls[0], ls[1]].into_iter().enumerate() {
            keys[side][k] = way_key(ctx, w, layer_label(ctx, w, l).1, s.gid, l as u32);
        }
    }
    let half = count / 2;
    fan_pieces(cands, s.c, &pts, &|j| {
        let side = if j < half { LEFT } else { RIGHT };
        (carriage, ids[side], keys[side])
    });
}
