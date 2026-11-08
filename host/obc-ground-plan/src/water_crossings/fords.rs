//! The wet faces of a crossing at grade: a ford, stepping stones and a way along the
//! water, every crossing of water and a way on the ground with no structure. Across each sample of the way
//! near the crossing, from its lower edge, the part where the way's owner curve lies under the water's level
//! at the point's foot on the water's axis; between two samples the quad of their parts, and a tip where a
//! part ends along the way. The water's own surface runs over it.

use i_overlay::core::overlay_rule::OverlayRule;

use super::{bounds, normal_at, signed, stroke, Axis, CrossingStats, Ctx, Wet, ZONE_ALONG_M};
use crate::faces::geom::{winding, P};
use crate::faces::stroke::{push_positive, shapes, union, Rings};
use crate::graph::{CrossingKind, Family, Level};

/// The bisections of a wet part's end: to under a millimetre over a way's widest band.
const BISECT: u32 = 16;
/// A cross line every this along the way between its samples: the wet edge between two lines is their chord,
/// within a few millimetres of the water's level where the way climbs out of it.
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

/// A cross line of the way: its point, unit normal (+ left), height, bank and edge (metres).
#[derive(Clone, Copy)]
struct Cross {
    p: [f64; 2],
    n: [f64; 2],
    z: f64,
    bank: f64,
    e: f64,
}

fn wet(ctx: &Ctx, pos: P, w: u32, s: u32, stats: &mut CrossingStats) -> Option<Wet> {
    let g = ctx.graph;
    let way = Axis::through(g, w, g.sample_near(w, pos), ZONE_ALONG_M);
    let axis = Axis::through(g, s, g.sample_near(s, pos), ZONE_ALONG_M);
    if way.points.len() < 2 || axis.points.len() < 2 {
        return None;
    }
    let sec = ctx.way_section(w);
    let water = ctx.water_section(s);
    // The records hold the water's surface within its line's reach (`present`), so the wet face ends there.
    let reach = water.crossing.ford_reach_m.min(ctx.way_section(s).reach_m);
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
        }
    };
    // The cross line at share f of segment k: its place and normal linear, its height the curve's Hermite.
    let between = |k: usize, f: f64| {
        let (a, b) = (cross(k), cross(k + 1));
        let (sa, sb) = (&g.samples[way.points[k].sample as usize], &g.samples[way.points[k + 1].sample as usize]);
        let len = (way.points[k + 1].m - way.points[k].m).abs();
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
        }
    };
    let place = |c: &Cross, t: f64| {
        let m = ctx.mpu(c.p);
        [c.p[0] + c.n[0] * t / m, c.p[1] + c.n[1] * t / m]
    };
    // The way's height less the water's at t metres across.
    let gap = |c: &Cross, t: f64| {
        let q = place(c, t);
        c.z + c.bank * t - axis.level(axis.foot(q))
    };
    // Whether a cross line's lower edge lies under the water's level.
    let wet_at = |c: &Cross| gap(c, -c.e).min(gap(c, c.e)) < 0.0;
    // The wet part of a cross line from its lower edge, (from, to) metres across; None for a dry one.
    let part = |c: &Cross| -> Option<(f64, f64)> {
        let (a, b) = (gap(c, -c.e), gap(c, c.e));
        let ((start, low), (other, high)) = if a <= b { ((-c.e, a), (c.e, b)) } else { ((c.e, b), (-c.e, a)) };
        if low.is_nan() || low >= 0.0 {
            return None;
        }
        if high < 0.0 {
            return Some((start, other));
        }
        let (mut lo, mut hi) = (start, other);
        for _ in 0..BISECT {
            let m = (lo + hi) / 2.0;
            if gap(c, m) < 0.0 {
                lo = m;
            } else {
                hi = m;
            }
        }
        Some((start, hi))
    };
    // Past the water's band, its reach and the way's half-width no line is wet within the reach.
    let far = |c: &Cross| signed(&axis, c.p, ctx.mpu(c.p)).0.abs() > water.half_m + reach + c.e + 1.0;
    let wet_part = |c: &Cross| if far(c) { None } else { part(c) };
    // Cross lines at the samples, and every `WET_STEP_M` between two that are not both dry or both wet across:
    // the wet edge between two lines is their chord.
    let at: Vec<Option<(f64, f64)>> = (0..n).map(|k| wet_part(&cross(k))).collect();
    let across = |k: usize| at[k].is_some_and(|p| (p.1 - p.0).abs() >= 2.0 * cross(k).e);
    let mut stations: Vec<(usize, f64)> = Vec::new();
    for k in 0..n - 1 {
        let alike = (at[k].is_none() && at[k + 1].is_none()) || (across(k) && across(k + 1));
        let len = (way.points[k + 1].m - way.points[k].m).abs();
        let steps = if alike { 1 } else { libm::ceil(len / WET_STEP_M).max(1.0) as usize };
        stations.extend((0..steps).map(|i| (k, i as f64 / steps as f64)));
    }
    stations.push((n - 2, 1.0));
    let lines: Vec<Cross> = stations.iter().map(|&(k, f)| between(k, f)).collect();
    let parts: Vec<Option<(f64, f64)>> =
        (stations.iter().zip(&lines)).map(|(&(k, f), c)| if f == 0.0 { at[k] } else { wet_part(c) }).collect();
    let mut pieces = Vec::new();
    let pt = |q: [f64; 2]| [libm::round(q[0]) as i32, libm::round(q[1]) as i32];
    for i in 0..lines.len() - 1 {
        let (a, b) = (&lines[i], &lines[i + 1]);
        match (parts[i], parts[i + 1]) {
            (Some(pa), Some(pb)) => {
                let ring = vec![pt(place(a, pa.0)), pt(place(a, pa.1)), pt(place(b, pb.1)), pt(place(b, pb.0))];
                push_positive(ring, &mut pieces);
            }
            (Some(_), None) | (None, Some(_)) => {
                // The tip: where the lower edge comes out of the water between the two lines.
                let wet_end = parts[i].is_some();
                let (k, f0) = stations[i];
                let f1 = if stations[i + 1].0 == k { stations[i + 1].1 } else { 1.0 };
                let (mut lo, mut hi) = (f0, f1);
                for _ in 0..BISECT {
                    let f = (lo + hi) / 2.0;
                    if wet_at(&between(k, f)) == wet_end {
                        lo = f;
                    } else {
                        hi = f;
                    }
                }
                let c = between(k, if wet_end { lo } else { hi });
                let tip = part(&c).map_or_else(
                    || place(&c, if gap(&c, -c.e) <= gap(&c, c.e) { -c.e } else { c.e }),
                    |p| place(&c, p.0),
                );
                let (end, p) = if wet_end { (a, parts[i].unwrap()) } else { (b, parts[i + 1].unwrap()) };
                push_positive(vec![pt(tip), pt(place(end, p.0)), pt(place(end, p.1))], &mut pieces);
            }
            (None, None) => {}
        }
    }
    if pieces.is_empty() {
        return None;
    }
    // The parts within that reach of the water's band that meet it.
    let line: Vec<[f64; 2]> = (0..axis.points.len()).map(|k| axis.pos(k)).collect();
    let mpu = ctx.mpu(pos.map(f64::from));
    let band = |m: f64| stroke(&line, &vec![m / mpu; line.len()], [false, false]);
    let (water_band, reach_band) = (band(water.half_m), band(water.half_m + reach));
    // Cut to the way's own band: its carriageway and shoulders.
    let own = ctx.band(&way, pos, [0.0, 0.0], bounds(&pieces))?;
    let wet = shapes(&union(&pieces, pos), &own, OverlayRule::Intersect, pos).concat();
    let mut rings: Rings = Vec::new();
    for part in shapes(&wet, &reach_band, OverlayRule::Intersect, pos) {
        if !shapes(&part, &water_band, OverlayRule::Intersect, pos).is_empty() {
            rings.extend(part);
        }
    }
    if rings.is_empty() {
        return None;
    }
    // A wet part that runs on past the reach from the crossing: its edge there is a water step.
    let zero = way.points.iter().position(|p| p.m >= 0.0).unwrap_or(0);
    let zero = stations.iter().position(|&(k, _)| k >= zero).unwrap_or(0);
    let past = |i: usize| {
        let q = lines[i].p;
        winding(&reach_band, [libm::round(2.0 * q[0]) as i64, libm::round(2.0 * q[1]) as i64]) == 0
    };
    let long = |is: &mut dyn Iterator<Item = usize>| is.take_while(|&i| parts[i].is_some()).any(past);
    if long(&mut (zero..lines.len())) || long(&mut (0..=zero).rev()) {
        stats.ford_long += 1;
    }
    Some(Wet { rings, way: w, water: s, seed: ctx.global(pos) })
}
