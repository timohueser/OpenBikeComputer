//! The corner engine: one circular fill between two edge chains that start at one point, with rings of
//! the layers past the seam round its arc. Junction corners, tight bends and kinks all use it.

use super::bands::{metres, normal, steps, WaySec, LEFT, S};
use super::geom::{cross, round, P};
use crate::frame::{metres_per_unit, GlobalPoint};

/// Each failed fit shrinks the radius by this.
const SHRINK: f64 = 0.8;
/// A foot this close to a sample (frame units) takes the sample.
const SNAP_UNITS: i64 = 2;

/// An edge chain: the samples of one way from the corner's point, in walking order.
pub struct Chain<'a> {
    pub w: &'a WaySec,
    pub idx: Vec<usize>,
    /// The corner's side of the way, and the boundary k of the seam edge on it.
    pub side: usize,
    pub seam: usize,
    pub room_m: f64,
}

impl Chain<'_> {
    fn s(&self, j: usize) -> &S {
        &self.w.s[self.idx[j]]
    }

    fn dir(&self, j: usize) -> [f64; 2] {
        let n = normal(self.s(j).t);
        if self.side == LEFT {
            n
        } else {
            [-n[0], -n[1]]
        }
    }

    fn edge(&self, j: usize, k: usize) -> P {
        self.s(j).p[S::bound(self.side, k)]
    }
}

/// A foot: the sample to insert on its way (None where an existing sample serves), and its seam point.
#[derive(Clone, Debug)]
pub struct Foot {
    pub way: u32,
    pub s: S,
    pub new: bool,
}

/// A layer band that a corner removes: (way, side, k, from ord, to ord).
pub type Cut = (u32, usize, usize, f64, f64);

/// A seam edge from the corner's point to a foot: its way, side and boundary, and the stations (ord) of
/// its two ends. The fill reads its points after every foot is in the way's samples.
#[derive(Clone, Copy, Debug)]
pub struct Seam {
    pub way: u32,
    pub side: usize,
    pub k: usize,
    pub from: f64,
    pub to: f64,
}

impl Seam {
    /// The seam's points from the corner's point to the foot.
    pub fn points(&self, w: &WaySec) -> Vec<P> {
        let (lo, hi) = (self.from.min(self.to), self.from.max(self.to));
        let mut pts: Vec<P> = (w.s.iter())
            .filter(|s| s.ord >= lo - 1e-12 && s.ord <= hi + 1e-12)
            .map(|s| s.p[S::bound(self.side, self.k)])
            .collect();
        if self.from > self.to {
            pts.reverse();
        }
        pts
    }
}

pub struct Corner {
    /// The fill's first point, its two seams and its arc: the fill is c, seam a, arc, seam b back.
    pub c: P,
    pub seams: [Seam; 2],
    /// (k, ring) for each layer past the seam that runs round the arc.
    pub rings: Vec<(usize, Vec<P>)>,
    pub feet: [Foot; 2],
    pub cuts: Vec<Cut>,
    pub centre: P,
    pub radius_m: f64,
    pub arc: Vec<P>,
    /// The seam points of the two feet.
    pub t: [P; 2],
    pub shrunk: bool,
}

/// What the engine needs besides the chains: the radius, the construction's point and extent, the
/// fill's first point.
pub struct Spec {
    pub r0: f64,
    pub x: [i64; 2],
    pub extent_m: f64,
    pub least_m: f64,
    pub c: P,
}

fn sub(a: [i64; 2], b: [i64; 2]) -> [f64; 2] {
    [(a[0] - b[0]) as f64, (a[1] - b[1]) as f64]
}

fn global(origin: GlobalPoint, p: P) -> [i64; 2] {
    [origin.x + i64::from(p[0]), origin.y + i64::from(p[1])]
}

/// Whether segments ab and cd cross properly (not at a shared end).
fn crosses(a: P, b: P, c: P, d: P) -> bool {
    let (d1, d2) = (cross(a, b, c), cross(a, b, d));
    let (d3, d4) = (cross(c, d, a), cross(c, d, b));
    d1.signum() * d2.signum() < 0 && d3.signum() * d4.signum() < 0
}

pub fn corner(origin: GlobalPoint, a: &Chain, b: &Chain, spec: &Spec) -> Option<Corner> {
    let mut r = spec.r0;
    let mut shrunk = false;
    while r >= spec.least_m {
        if let Some(mut c) = fit(origin, a, b, spec, r) {
            c.shrunk = shrunk;
            return Some(c);
        }
        r *= SHRINK;
        shrunk = true;
    }
    None
}

fn fit(origin: GlobalPoint, a: &Chain, b: &Chain, spec: &Spec, r: f64) -> Option<Corner> {
    let anchor = a.s(0).g;
    // The seam edge moved by r toward the corner side, in frame units from the anchor.
    let moved = |ch: &Chain, j: usize| {
        let s = ch.s(j);
        let u = (s.o[ch.side][ch.seam] + r) / metres_per_unit(s.g[1]);
        let d = ch.dir(j);
        let base = sub(s.g, anchor);
        [base[0] + u * d[0], base[1] + u * d[1]]
    };
    let along = |ch: &Chain| {
        let mut m = vec![0.0];
        for j in 1..ch.idx.len() {
            m.push(m[j - 1] + metres(ch.s(j - 1).g, ch.s(j).g));
        }
        m
    };
    let (ma, mb) = (along(a), along(b));
    let shared = a.w.way == b.w.way && a.idx[0] == b.idx[0] && a.side == b.side;
    let mut best: Option<(f64, f64, usize, usize, [f64; 2])> = None;
    for i in 0..a.idx.len() - 1 {
        if ma[i] > a.room_m {
            break;
        }
        let (p0, p1) = (moved(a, i), moved(a, i + 1));
        for k in 0..b.idx.len() - 1 {
            if mb[k] > b.room_m {
                break;
            }
            let (q0, q1) = (moved(b, k), moved(b, k + 1));
            let (e, f) = ([p1[0] - p0[0], p1[1] - p0[1]], [q1[0] - q0[0], q1[1] - q0[1]]);
            let den = e[0] * f[1] - e[1] * f[0];
            if den == 0.0 {
                continue;
            }
            let g = [q0[0] - p0[0], q0[1] - p0[1]];
            let (s, t) = ((g[0] * f[1] - g[1] * f[0]) / den, (g[0] * e[1] - g[1] * e[0]) / den);
            if !(0.0..=1.0).contains(&s)
                || !(0.0..=1.0).contains(&t)
                || shared && i == 0 && k == 0 && s < 1e-9 && t < 1e-9
            {
                continue;
            }
            let (m1, m2) = (ma[i] + s * (ma[i + 1] - ma[i]), mb[k] + t * (mb[k + 1] - mb[k]));
            let o = [p0[0] + s * e[0], p0[1] + s * e[1]];
            if best.is_none_or(|(x, y, ..)| (m1 + m2, m1) < (x + y, x)) {
                best = Some((m1, m2, i, k, o));
            }
        }
    }
    let (_, _, ia, ib, o) = best?;
    let foot = |ch: &Chain, i: usize, ms: &[f64]| -> Option<Foot> {
        let (e0, e1) =
            (sub(global(origin, ch.edge(i, ch.seam)), anchor), sub(global(origin, ch.edge(i + 1, ch.seam)), anchor));
        let d = [e1[0] - e0[0], e1[1] - e0[1]];
        let len = d[0] * d[0] + d[1] * d[1];
        let tau = if len == 0.0 { 0.0 } else { ((o[0] - e0[0]) * d[0] + (o[1] - e0[1]) * d[1]) / len };
        if !(0.0..=1.0).contains(&tau) || ms[i] + tau * (ms[i + 1] - ms[i]) > ch.room_m {
            return None;
        }
        let s = S::lerp(ch.s(i), ch.s(i + 1), tau, origin);
        let near = |x: &S| (x.g[0] - s.g[0]).abs() <= SNAP_UNITS && (x.g[1] - s.g[1]).abs() <= SNAP_UNITS;
        Some(if near(ch.s(i)) {
            Foot { way: ch.w.way, s: ch.s(i).clone(), new: false }
        } else if near(ch.s(i + 1)) {
            Foot { way: ch.w.way, s: ch.s(i + 1).clone(), new: false }
        } else {
            Foot { way: ch.w.way, s, new: true }
        })
    };
    let (fa, fb) = (foot(a, ia, &ma)?, foot(b, ib, &mb)?);
    let (pa, pb) = (fa.s.p[S::bound(a.side, a.seam)], fb.s.p[S::bound(b.side, b.seam)]);
    let ru = |rho: f64| rho / metres_per_unit(anchor[1] + round(o[1]));
    let angle = |p: P| {
        let v = sub(global(origin, p), anchor);
        libm::atan2(v[1] - o[1], v[0] - o[0])
    };
    let (phi_a, phi_b) = (angle(pa), angle(pb));
    let mut sweep = phi_b - phi_a;
    while sweep > core::f64::consts::PI {
        sweep -= 2.0 * core::f64::consts::PI;
    }
    while sweep <= -core::f64::consts::PI {
        sweep += 2.0 * core::f64::consts::PI;
    }
    let n = steps(sweep, false);
    // Arc j of the layers out to width w(j) past the seam: radius max(0, r − w).
    let arc = |w: &dyn Fn(f64) -> f64| -> Vec<P> {
        (1..n)
            .map(|j| {
                let f = j as f64 / n as f64;
                let rho = ru((r - w(f)).max(0.0));
                let phi = phi_a + sweep * f;
                let x = anchor[0] + round(o[0] + rho * libm::cos(phi)) - origin.x;
                let y = anchor[1] + round(o[1] + rho * libm::sin(phi)) - origin.y;
                [x as i32, y as i32]
            })
            .collect()
    };
    let main = arc(&|_| 0.0);
    // The arc must not cross either seam edge between the corner's point and its foot.
    let mut path = vec![pa];
    path.extend(main.iter().copied());
    path.push(pb);
    let seam_a: Vec<P> = (0..=ia).map(|j| a.edge(j, a.seam)).chain([pa]).collect();
    let seam_b: Vec<P> = (0..=ib).map(|j| b.edge(j, b.seam)).chain([pb]).collect();
    for seam in [&seam_a, &seam_b] {
        for w in path.windows(2) {
            if seam.windows(2).any(|e| crosses(w[0], w[1], e[0], e[1])) {
                return None;
            }
        }
    }
    let mut fill = vec![spec.c];
    fill.extend(seam_a.iter().copied());
    fill.extend(main.iter().copied());
    fill.extend(seam_b.iter().rev().copied());
    let seam = |ch: &Chain, f: &Foot| Seam { way: ch.w.way, side: ch.side, k: ch.seam, from: ch.s(0).ord, to: f.s.ord };
    let seams = [seam(a, &fa), seam(b, &fb)];
    let mut rings = Vec::new();
    let mut cuts = Vec::new();
    let width = |ch: &Chain, f: &Foot, k: usize| f.s.o[ch.side][k] - f.s.o[ch.side][ch.seam];
    let mut inner = main.clone();
    // Ring m runs round the arc from the m-th layer past each seam's inner edge to its outer one.
    for m in 1..3 {
        let (ka, kb) = ((a.seam + m).min(2), (b.seam + m).min(2));
        let (ka0, kb0) = ((a.seam + m - 1).min(2), (b.seam + m - 1).min(2));
        let (wa, wb) = (width(a, &fa, ka), width(b, &fb, kb));
        if wa == width(a, &fa, ka0) && wb == width(b, &fb, kb0) {
            continue;
        }
        let cut_a = ka != ka0 && r - wa > 0.0;
        let cut_b = kb != kb0 && r - wb > 0.0;
        if !cut_a && !cut_b {
            break;
        }
        let outer = arc(&|f| wa + (wb - wa) * f);
        let mut ring = vec![fa.s.p[S::bound(a.side, ka0)], fa.s.p[S::bound(a.side, ka)]];
        ring.extend(outer.iter().copied());
        ring.push(fb.s.p[S::bound(b.side, kb)]);
        ring.push(fb.s.p[S::bound(b.side, kb0)]);
        ring.extend(inner.iter().rev().copied());
        rings.push((if ka != ka0 { ka } else { kb }, ring));
        for (ch, f, cut, k) in [(a, &fa, cut_a, ka), (b, &fb, cut_b, kb)] {
            if cut {
                let (lo, hi) = (ch.s(0).ord.min(f.s.ord), ch.s(0).ord.max(f.s.ord));
                cuts.push((ch.w.way, ch.side, k, lo, hi));
            }
        }
        inner = outer;
    }
    let far = |p: &P| metres(global(origin, *p), spec.x) > spec.extent_m;
    if fill.iter().chain(rings.iter().flat_map(|r| &r.1)).any(far) {
        return None;
    }
    let centre = [(anchor[0] + round(o[0]) - origin.x) as i32, (anchor[1] + round(o[1]) - origin.y) as i32];
    Some(Corner {
        c: spec.c,
        seams,
        rings,
        feet: [fa, fb],
        cuts,
        centre,
        radius_m: r,
        arc: main,
        t: [pa, pb],
        shrunk: false,
    })
}
