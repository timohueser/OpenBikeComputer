//! The zones: of a culvert, a pipe in stones or a crossing at grade, the way's band out to the outer edge of
//! its slope band, within the water's band; of a culvert chain's open end, the water's band over the chain;
//! of a stream that ends at a way, the way's band on the water's side. A zone that the water leaves and meets
//! again within `mouth_join_m` takes the water's band between.

use std::collections::BTreeMap;

use i_overlay::core::overlay_rule::OverlayRule;

use super::{bounds, stroke, to_segment, CrossingStats, Ctx, Zone, CHAIN_READ_M, ZONE_ALONG_M};
use crate::faces::geom::{winding, P};
use crate::faces::stroke::{shapes, union, Rings};
use crate::graph::{CrossingKind, Family, Level, Network, WalkPoint};

/// Where a way meets line water: a crossing, or an end of the water at the way.
struct Seed {
    pos: P,
    /// The way and its sample nearest `pos`; the water way and its sample at `pos`.
    w: u32,
    ws: u32,
    s: u32,
    ss: u32,
    /// An end: the water runs from it this way only.
    one: Option<i8>,
}

pub(super) fn zones(ctx: &Ctx, near: &dyn Fn(P) -> bool, stats: &mut CrossingStats) -> Vec<Zone> {
    let g = ctx.graph;
    let mut zones = Vec::new();
    // Every crossing of water with no structure: a culvert, a pipe in stones, and one at grade.
    let open = |k: CrossingKind| {
        use CrossingKind::*;
        matches!(k, Culvert | PipeInStones | Ford | SteppingStones | Stacked)
    };
    for c in g.crossings.iter().filter(|c| open(c.kind) && near(c.pos)) {
        let (Some(w), Some(s)) =
            (g.way_at(&c.over, Level::Ground), g.way_at(&c.under, Level::Ground).or(g.way_at(&c.under, Level::Tunnel)))
        else {
            continue;
        };
        if g.ways[w as usize].family == Family::Water || g.ways[s as usize].family != Family::Water {
            continue;
        }
        let seed = Seed { pos: c.pos, w, ws: g.sample_near(w, c.pos), s, ss: g.sample_near(s, c.pos), one: None };
        zones.extend(crossing_zone(ctx, &seed));
    }
    for e in g.ends.iter().filter(|e| e.network == Network::Water && near(e.pos)) {
        let s = e.arm.way;
        if g.ways[s as usize].level != Level::Ground {
            continue;
        }
        let Some(w) = way_at_end(ctx, e.pos, ctx.water_section(s).crossing.mouth_bank_m) else { continue };
        let ss = g.sample_near(s, e.pos);
        let seed = Seed { pos: e.pos, w, ws: g.sample_near(w, e.pos), s, ss, one: Some(e.arm.dir) };
        let zone = crossing_zone(ctx, &seed);
        stats.water_ends += u32::from(zone.is_some());
        zones.extend(zone);
    }
    for j in g.joints.iter().filter(|j| j.network == Network::Water && near(j.pos)) {
        let level = |k: usize| g.ways[j.arms[k].way as usize].level;
        let Some(k) = (0..2).find(|&k| level(k) == Level::Tunnel && level(1 - k) == Level::Ground) else { continue };
        zones.push(chain_zone(ctx, j.pos, &j.arms[k], j.arms[1 - k].way));
        stats.chain_ends += 1;
    }
    join(ctx, &mut zones, stats);
    stats.zones += zones.len() as u32;
    zones
}

/// The way on the ground whose slope band, or `mouth_bank_m` past it, holds a water end: the nearest by
/// the metres past its band, of the faces' lines.
fn way_at_end(ctx: &Ctx, pos: P, bank: f64) -> Option<u32> {
    let q = [f64::from(pos[0]), f64::from(pos[1])];
    let mpu = ctx.mpu(q);
    let mut best: Option<(f64, u32)> = None;
    for line in ctx.lines.iter().filter(|l| !l.sec.water && ctx.graph.ways[l.way as usize].level == Level::Ground) {
        let sec = &line.sec;
        let band = sec.shoulder_m + sec.verge_m + sec.reach_m;
        let d = (line.near(q, (band + bank) / mpu))
            .map(|i| {
                to_segment(q, line.p[i].map(f64::from), line.p[i + 1].map(f64::from)).0 * mpu
                    - line.h[i].max(line.h[i + 1]) * mpu
            })
            .fold(f64::INFINITY, f64::min);
        let past = d - band;
        if past <= bank && best.is_none_or(|b| (past, line.way) < b) {
            best = Some((past, line.way));
        }
    }
    best.map(|b| b.1)
}

/// A crossing's zone: the way's band out to the outer edge of its slope band, where its slopes have let go,
/// within the water's band. The water ends there, and the slopes shape the ground over it, so the ground meets
/// the water's end at its level as it meets its banks.
fn crossing_zone(ctx: &Ctx, seed: &Seed) -> Option<Zone> {
    let sec = ctx.way_section(seed.w);
    let band_s = water_band(ctx, seed, ctx.water_section(seed.s).crossing.mouth_bank_m);
    let band_w = ctx.band(seed.w, seed.ws, sec.verge_m + sec.reach_m, bounds(&band_s))?;
    let q2 = [2 * i64::from(seed.pos[0]), 2 * i64::from(seed.pos[1])];
    let rings = shapes(&band_w, &band_s, OverlayRule::Intersect, seed.pos).into_iter().find(|s| winding(s, q2) != 0)?;
    Some(Zone { rings, way: Some(seed.w), seed: ctx.global(seed.pos) })
}

/// The water's band about a seed: the network within `ZONE_ALONG_M` along it, on through its joints and
/// into every arm of a junction, as wide as each water way's half-width and `bank`. A water end's band is
/// round at the end.
fn water_band(ctx: &Ctx, seed: &Seed, bank: f64) -> Rings {
    let g = ctx.graph;
    let mut halves: BTreeMap<u32, f64> = BTreeMap::new();
    let mut half = |w: &WalkPoint| *halves.entry(w.way).or_insert_with(|| ctx.water_section(w.way).half_m) + bank;
    let mut lines: Vec<(Vec<WalkPoint>, [bool; 2])> = Vec::new();
    let dirs: &[i8] = match seed.one {
        Some(d) => &[d][..],
        None => &[-1, 1][..],
    };
    let mut main: Vec<WalkPoint> = Vec::new();
    for (k, &d) in dirs.iter().enumerate() {
        let mut walks = g.walk(seed.s, seed.ss, d, ZONE_ALONG_M, true);
        let first = walks.remove(0);
        if k == 0 {
            main = first.into_iter().rev().collect();
        } else {
            main.extend(first.into_iter().skip(1));
        }
        lines.extend(walks.into_iter().map(|w| (w, [true, false])));
    }
    if seed.one.is_some() {
        main.reverse();
    }
    lines.push((main, [seed.one.is_some(), false]));
    let mut out = Vec::new();
    for (line, round) in lines {
        let pts: Vec<[f64; 2]> = line.iter().map(|w| w.pos.map(f64::from)).collect();
        let h: Vec<f64> = line.iter().map(|w| half(w) / ctx.mpu(w.pos.map(f64::from))).collect();
        out.extend(stroke(&pts, &h, round));
    }
    out
}

/// A culvert chain's open end: the water's band over the chain from the end, as far as the
/// water's slopes reach past the band and at most to the chain's other end, square at both. Its way is the
/// way over the chain's nearest crossing within `CHAIN_READ_M`, else none.
fn chain_zone(ctx: &Ctx, pos: P, tunnel: &crate::graph::Arm, open: u32) -> Zone {
    let g = ctx.graph;
    let r = &g.ways[tunnel.way as usize].samples;
    let start = if tunnel.dir > 0 { r.start } else { r.end - 1 };
    let water = ctx.water_section(tunnel.way);
    let s = water.half_m + water.crossing.mouth_bank_m;
    let in_chain = |w: &&WalkPoint| g.ways[w.way as usize].level == Level::Tunnel;
    let walk = |m: f64| -> Vec<WalkPoint> {
        g.walk(tunnel.way, start, tunnel.dir, m, false).swap_remove(0).iter().take_while(in_chain).copied().collect()
    };
    let over = walk(s + ctx.way_section(open).reach_m);
    let pts: Vec<[f64; 2]> = over.iter().map(|w| w.pos.map(f64::from)).collect();
    let h: Vec<f64> = over.iter().map(|w| s / ctx.mpu(w.pos.map(f64::from))).collect();
    let rings = union(&stroke(&pts, &h, [false, false]), pos);
    let way = walk(CHAIN_READ_M)
        .iter()
        .find_map(|w| (g.crossings.iter()).find(|c| c.pos == w.pos).and_then(|c| g.way_at(&c.over, Level::Ground)));
    Zone { rings, way, seed: ctx.global(pos) }
}

/// Whether q (units) lies in a zone.
fn inside(rings: &Rings, q: [f64; 2]) -> bool {
    winding(rings, [libm::round(2.0 * q[0]) as i64, libm::round(2.0 * q[1]) as i64]) != 0
}

/// The point between an inside point a and an outside point b where the axis leaves the zone.
fn leave(rings: &Rings, mut a: [f64; 2], mut b: [f64; 2]) -> [f64; 2] {
    for _ in 0..24 {
        let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        if inside(rings, m) {
            a = m;
        } else {
            b = m;
        }
    }
    b
}

/// Each place where the axis of a water way leaves a zone: if the axis meets a zone again within
/// `mouth_join_m` along it, the water's band between joins the zone that it leaves.
fn join(ctx: &Ctx, zones: &mut [Zone], stats: &mut CrossingStats) {
    let g = ctx.graph;
    let boxes: Vec<[f64; 4]> =
        zones.iter().map(|z| bounds(&z.rings)).map(|b| [b[0] - 1.0, b[1] - 1.0, b[2] + 1.0, b[3] + 1.0]).collect();
    let mut adds: Vec<Vec<Vec<P>>> = vec![Vec::new(); zones.len()];
    for (zi, z) in zones.iter().enumerate() {
        let bx = boxes[zi];
        for w in g.ways.iter().filter(|w| w.family == Family::Water && w.level != Level::Deck) {
            let r = w.samples.start as usize..w.samples.end as usize;
            let pos = |k: usize| g.samples[k].pos.map(f64::from);
            let water = ctx.water_section(w.id);
            for k in r.start..r.end.saturating_sub(1) {
                let (a, b) = (pos(k), pos(k + 1));
                if a[0].max(b[0]) < bx[0] || a[0].min(b[0]) > bx[2] || a[1].max(b[1]) < bx[1] || a[1].min(b[1]) > bx[3]
                {
                    continue;
                }
                let (i0, i1) = (inside(&z.rings, a), inside(&z.rings, b));
                let (x, next, dir) = match (i0, i1) {
                    (true, false) => (leave(&z.rings, pos(k), pos(k + 1)), k + 1, 1i8),
                    (false, true) => (leave(&z.rings, pos(k + 1), pos(k)), k, -1i8),
                    _ => continue,
                };
                let mpu = ctx.mpu(x);
                let first = libm::hypot(pos(next)[0] - x[0], pos(next)[1] - x[1]) * mpu;
                let mut line = vec![x];
                if first < water.crossing.mouth_join_m {
                    let on = g.walk(w.id, next as u32, dir, water.crossing.mouth_join_m - first, false).swap_remove(0);
                    line.extend(on.iter().map(|p| p.pos.map(f64::from)));
                } else {
                    let f = water.crossing.mouth_join_m / first;
                    line.push([x[0] + f * (pos(next)[0] - x[0]), x[1] + f * (pos(next)[1] - x[1])]);
                }
                let entry = (1..line.len()).find_map(|i| {
                    let hit = zones.iter().position(|o| inside(&o.rings, line[i]))?;
                    Some((i, hit, leave(&zones[hit].rings, line[i], line[i - 1])))
                });
                // Each gap is walked from both its ends: the end that names the lesser (seed, place) takes it.
                let o = ctx.cell().origin();
                let name =
                    |z: &Zone, q: [f64; 2]| (z.seed, [o.x + libm::round(q[0]) as i64, o.y + libm::round(q[1]) as i64]);
                if let Some((i, _, at)) = entry.filter(|&(_, hit, at)| name(z, x) < name(&zones[hit], at)) {
                    let mut gap: Vec<[f64; 2]> = line[..i].to_vec();
                    gap.push(at);
                    let h = (water.half_m + water.crossing.mouth_bank_m) / mpu;
                    adds[zi].extend(stroke(&gap, &vec![h; gap.len()], [false, false]));
                    stats.joins += 1;
                }
            }
        }
    }
    for (z, add) in zones.iter_mut().zip(adds) {
        if !add.is_empty() {
            let mut all = z.rings.clone();
            all.extend(add);
            z.rings =
                union(&all, [(z.seed[0] - ctx.cell().origin().x) as i32, (z.seed[1] - ctx.cell().origin().y) as i32]);
        }
    }
}
