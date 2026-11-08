//! The wet faces of a crossing at grade: a ford, stepping stones and a way along the water, every crossing
//! of water and a way on the ground with no structure. The water's level W at a point is its own curve at
//! the point's foot on its line, as the records paint it. From the crossing outward along the way, each
//! cross line of the way and its slope band is wet where the way, or the way's own cut beside it, lies under
//! W, from the way's lower edge; between two lines two triangles, and a tip where the way's lower edge rises
//! out of the water. The face ends there, or square at its reach along the way past the water's band.

use std::cell::Cell;

use super::{normal_at, signed, Axis, CrossingStats, Ctx, Wet, ZONE_ALONG_M};
use crate::faces::geom::P;
use crate::faces::heights::{fit, release, round_back};
use crate::faces::stroke::{push_positive, Rings};
use crate::graph::{CrossingKind, Family, Level};

/// The bisections of a wet part's end: to under a millimetre over a way's widest band.
const BISECT: u32 = 16;
/// A cross line every this along the way: the wet edge between two lines is their chord, within a few
/// millimetres of W where the way climbs out of the water.
const WET_STEP_M: f64 = 0.25;

pub(super) fn wets(ctx: &Ctx, near: &dyn Fn(P) -> bool, stats: &mut CrossingStats) -> Vec<Wet> {
    let g = ctx.graph;
    let mut out = Vec::new();
    let at_grade =
        |k: CrossingKind| matches!(k, CrossingKind::Ford | CrossingKind::SteppingStones | CrossingKind::Stacked);
    for c in g.crossings.iter().filter(|c| at_grade(c.kind) && near(c.pos)) {
        let (Some(w), Some(s)) = (g.way_at(&c.over, Level::Ground), g.way_at(&c.under, Level::Ground)) else {
            continue;
        };
        if g.ways[w as usize].family == Family::Water || g.ways[s as usize].family != Family::Water {
            continue;
        }
        match wet(ctx, c.pos, w, s, stats) {
            Some(x) => {
                stats.wet += 1;
                out.push(x);
            }
            None => stats.ford_dry += 1,
        }
    }
    out
}

/// A cross line of the way: its point, unit normal (+ left), height, bank, shoulder edge and paved edge
/// (metres), and metres along.
#[derive(Clone, Copy)]
struct Cross {
    p: [f64; 2],
    n: [f64; 2],
    z: f64,
    bank: f64,
    e: f64,
    paved: f64,
    m: f64,
}

fn wet(ctx: &Ctx, pos: P, w: u32, s: u32, stats: &mut CrossingStats) -> Option<Wet> {
    let g = ctx.graph;
    let way = Axis::through(g, w, g.sample_near(w, pos), ZONE_ALONG_M);
    let axis = Axis::through(g, s, g.sample_near(s, pos), ZONE_ALONG_M);
    if way.points.len() < 2 || axis.points.len() < 2 {
        return None;
    }
    // W at q: the water's surface as the records paint it, in the mitre quad of its line that holds q: Hermite
    // on its samples' heights and grades at the quad's share. The search walks from the last quad to the one
    // that holds q: the points it is asked for lie close together.
    let line = &ctx.lines[*ctx.of_way.get(&s)?];
    let r = g.ways[s as usize].samples.clone();
    if line.p.len() != r.len() || line.p.len() < 2 {
        return None;
    }
    let p = &ctx.rules.plan.heights;
    let health = p.health_margin_m + p.health_width_m;
    let least: Vec<f64> = (line.p.iter().zip(&line.h)).map(|(q, h)| h + health / ctx.mpu(q.map(f64::from))).collect();
    let mitres = crate::faces::records::mitres(&line.p, &least, p.frame_tilt_deg);
    let xy = |k: usize| line.p[k].map(f64::from);
    let sample = |k: usize| &g.samples[r.start as usize + k];
    let n_s = line.p.len();
    let hint = Cell::new((g.sample_near(s, pos) - r.start) as usize);
    let level = |q: [f64; 2]| {
        let mut k = hint.get().min(n_s - 2);
        for _ in 0..n_s {
            let da = (q[0] - xy(k)[0]) * mitres[k][0] + (q[1] - xy(k)[1]) * mitres[k][1];
            let db = (xy(k + 1)[0] - q[0]) * mitres[k + 1][0] + (xy(k + 1)[1] - q[1]) * mitres[k + 1][1];
            if da < 0.0 && k > 0 {
                k -= 1;
            } else if db < 0.0 && k + 2 < n_s {
                k += 1;
            } else {
                hint.set(k);
                let t = if da + db > 0.0 { (da / (da + db)).clamp(0.0, 1.0) } else { 0.5 };
                let len = libm::hypot(xy(k + 1)[0] - xy(k)[0], xy(k + 1)[1] - xy(k)[1]) * ctx.mpu(xy(k));
                let (a, b) = (sample(k), sample(k + 1));
                let z = |x: f64| f64::from(x as f32);
                return crate::faces::heights::hermite(z(a.z_m), z(b.z_m), z(a.grade) * len, z(b.grade) * len, t);
            }
        }
        f64::NAN
    };
    let sec = ctx.way_section(w);
    let water = ctx.water_section(s);
    let paved_m = if sec.shoulder_paved { sec.shoulder_m } else { 0.0 };
    let n = way.points.len();
    let pos_k = |k: usize| way.pos(k);
    let cross = |k: usize| {
        let x = &g.samples[way.points[k].sample as usize];
        Cross {
            p: pos_k(k),
            n: normal_at(&pos_k, n, k),
            z: x.z_m,
            bank: x.bank,
            e: sec.half_m + x.widen_m + sec.shoulder_m,
            paved: sec.half_m + x.widen_m + paved_m,
            m: way.points[k].m,
        }
    };
    // The cross line at share f of segment k: its place and normal linear, its height the curve's Hermite.
    let between = |k: usize, f: f64| {
        let (a, b) = (cross(k), cross(k + 1));
        let (sa, sb) = (&g.samples[way.points[k].sample as usize], &g.samples[way.points[k + 1].sample as usize]);
        let len = (b.m - a.m).abs();
        let lerp = |x: f64, y: f64| x + f * (y - x);
        let nn = [lerp(a.n[0], b.n[0]), lerp(a.n[1], b.n[1])];
        let l = libm::hypot(nn[0], nn[1]).max(1e-12);
        let (t2, t3) = (f * f, f * f * f);
        let z = (2.0 * t3 - 3.0 * t2 + 1.0) * a.z
            + (t3 - 2.0 * t2 + f) * sa.grade * len
            + (-2.0 * t3 + 3.0 * t2) * b.z
            + (t3 - t2) * sb.grade * len;
        Cross {
            p: [lerp(a.p[0], b.p[0]), lerp(a.p[1], b.p[1])],
            n: [nn[0] / l, nn[1] / l],
            z,
            bank: lerp(a.bank, b.bank),
            e: lerp(a.e, b.e),
            paved: lerp(a.paved, b.paved),
            m: lerp(a.m, b.m),
        }
    };
    let place = |c: &Cross, t: f64| {
        let m = ctx.mpu(c.p);
        [c.p[0] + c.n[0] * t / m, c.p[1] + c.n[1] * t / m]
    };
    // A side's last wet metres across: past the shoulder edge, the strip of its slope band where the way's own
    // cut from its paved edge (the heights' no-room rule, round and release) still lies under W. The cut
    // rises from the edge, so the strip ends where it reaches W, at most at the band's outer edge.
    let wall = if sec.leg_wall { p.wall_max_m } else { 0.0 };
    let out = |c: &Cross, side: f64| {
        let top = c.z + c.bank * side * c.paved;
        let go = ctx.terrain(place(c, side * (c.paved + sec.reach_m * (1.0 - p.fade_share))));
        let (sc, wc) = fit(sec.cut, sec.cut_max, [go - top; 2], wall, p, sec.reach_m, 0.0);
        let cut = |d: f64| top + wc[0] + sc * d - round_back(sec.cut, d, p) + release(d, sec.reach_m, p).1;
        let wet = |d: f64| cut(d) < level(place(c, side * (c.paved + d)));
        let (mut a, mut b) = (c.e - c.paved, c.e - c.paved + sec.reach_m);
        // A side whose terrain lies under its edge where the cones let go is a fill: its ground falls away.
        let cut_side = go > top;
        if !cut_side || !wet(a) {
            return side * c.e;
        }
        if wet(b) {
            return side * (c.paved + b);
        }
        for _ in 0..BISECT {
            let m = (a + b) / 2.0;
            if wet(m) {
                a = m;
            } else {
                b = m;
            }
        }
        side * (c.paved + a)
    };
    // The way's height less W at t metres across, and at its two edges.
    let gap = |c: &Cross, t: f64| c.z + c.bank * t - level(place(c, t));
    let edges = |c: &Cross| [gap(c, -c.e), gap(c, c.e)];
    let wet_at = |c: &Cross| {
        let [a, b] = edges(c);
        a.min(b) < 0.0
    };
    // The wet part of a cross line (least, greatest metres across), from its lower edge; None for a dry one.
    let part = |c: &Cross| -> Option<(f64, f64)> {
        if !wet_at(c) {
            return None;
        }
        let [a, b] = edges(c);
        let low = if a <= b { -1.0 } else { 1.0 };
        let near = out(c, low);
        let far = if a.max(b) < 0.0 {
            out(c, -low)
        } else {
            let (mut lo, mut hi) = (low * c.e, -low * c.e);
            for _ in 0..BISECT {
                let m = (lo + hi) / 2.0;
                if gap(c, m) < 0.0 {
                    lo = m;
                } else {
                    hi = m;
                }
            }
            hi
        };
        Some((near.min(far), near.max(far)))
    };
    // From the crossing outward each way, a cross line every `WET_STEP_M`: (segment, share from, share to).
    let k0 = way.points.iter().position(|q| q.m >= 0.0).unwrap_or(0);
    let steps = |k: usize| libm::ceil((way.points[k + 1].m - way.points[k].m).abs() / WET_STEP_M).max(1.0) as usize;
    let mut on = (k0..n - 1).flat_map(|k| {
        let s = steps(k);
        (0..s).map(move |i| (k, i as f64 / s as f64, (i + 1) as f64 / s as f64))
    });
    let mut back = (0..k0).rev().flat_map(|k| {
        let s = steps(k);
        (0..s).rev().map(move |i| (k, (i + 1) as f64 / s as f64, i as f64 / s as f64))
    });
    let start = cross(k0);
    let first = part(&start)?;
    let reach = water.crossing.ford_reach_m;
    let pt = |q: [f64; 2]| [libm::round(q[0]) as i32, libm::round(q[1]) as i32];
    let mut pieces: Rings = Vec::new();
    let mut long = false;
    for run in [&mut on as &mut dyn Iterator<Item = (usize, f64, f64)>, &mut back] {
        let (mut a, mut pa) = (start, first);
        // Where the way's curve leaves the water's band: its reach runs from there.
        let mut exit: Option<f64> = None;
        for (k, from, to) in run {
            let b = between(k, to);
            let along = (b.m - start.m).abs();
            if exit.is_none() && signed(&axis, b.p, ctx.mpu(b.p)).0.abs() > water.half_m {
                exit = Some(along);
            }
            let Some(pb) = part(&b) else {
                // The tip: where the way's lower edge rises out of the water between the two lines.
                let (mut lo, mut hi) = (from, to);
                for _ in 0..BISECT {
                    let f = (lo + hi) / 2.0;
                    if wet_at(&between(k, f)) {
                        lo = f;
                    } else {
                        hi = f;
                    }
                }
                let c = between(k, lo);
                let [l, r] = edges(&c);
                let tip = place(&c, if l <= r { -c.e } else { c.e });
                push_positive(vec![pt(tip), pt(place(&a, pa.0)), pt(place(&a, pa.1))], &mut pieces);
                break;
            };
            let [a0, a1, b0, b1] = [place(&a, pa.0), place(&a, pa.1), place(&b, pb.0), place(&b, pb.1)].map(pt);
            push_positive(vec![a0, a1, b1], &mut pieces);
            push_positive(vec![a0, b1, b0], &mut pieces);
            (a, pa) = (b, pb);
            if exit.is_some_and(|x| along >= x + reach) {
                long = true;
                break;
            }
        }
    }
    if pieces.is_empty() {
        return None;
    }
    stats.ford_long += u32::from(long);
    Some(Wet { rings: pieces, way: w, water: s, seed: ctx.global(pos) })
}
