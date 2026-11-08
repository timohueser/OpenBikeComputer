//! The zones: of a culvert, a pipe in stones or a crossing at grade, the way's band out to its fill
//! line on each side, within the water's band; of a culvert chain's open end, the water's band over the
//! chain; of a stream that ends at a way, the way's band on the water's side. A zone that the water leaves
//! and meets again within `mouth_join_m` takes the water's band between.

use std::collections::BTreeMap;

use i_overlay::core::overlay_rule::OverlayRule;

use super::{
    bounds, left, signed, stroke, to_segment, Axis, CrossingStats, Ctx, Zone, MOUTH_ROOM_M, PIPE_GAP_M, SCAN_COARSE,
    SCAN_STEP_M, ZONE_ALONG_M, ZONE_READ_M,
};
use crate::faces::geom::{winding, P};
use crate::faces::heights::{edge_cone, fit, release, round_back};
use crate::faces::stroke::{shapes, union, Rings};
use crate::graph::{CrossingKind, Family, Level, Network, WalkPoint, CHAIN_READ_M};

/// Where a way's fill meets line water: a crossing, or an end of the water at the way.
struct Seed {
    pos: P,
    /// The way and its sample nearest `pos`; the water way and its sample at `pos`.
    w: u32,
    ws: u32,
    s: u32,
    ss: u32,
    /// The water's level there.
    level: f64,
    /// An end: the water runs from it this way only.
    one: Option<i8>,
}

pub(super) fn zones(ctx: &Ctx, near: &dyn Fn(P) -> bool, stats: &mut CrossingStats) -> Vec<Zone> {
    let g = ctx.graph;
    let mut zones = Vec::new();
    // Every crossing of water with no structure: a culvert, a pipe in stones, and one at grade, which is drawn
    // as a culvert.
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
        let ss = g.sample_near(s, c.pos);
        let Some(level) = g.level_at(s, ss) else { continue };
        let seed = Seed { pos: c.pos, w, ws: g.sample_near(w, c.pos), s, ss, level, one: None };
        zones.extend(crossing_zone(ctx, &seed, stats));
    }
    for e in g.ends.iter().filter(|e| e.network == Network::Water && near(e.pos)) {
        let s = e.arm.way;
        if g.ways[s as usize].level != Level::Ground {
            continue;
        }
        let Some(w) = way_at_end(ctx, e.pos, ctx.water_section(s).crossing.mouth_bank_m) else { continue };
        let ss = g.sample_near(s, e.pos);
        let seed = Seed {
            pos: e.pos,
            w,
            ws: g.sample_near(w, e.pos),
            s,
            ss,
            level: g.samples[ss as usize].z_m,
            one: Some(e.arm.dir),
        };
        let zone = crossing_zone(ctx, &seed, stats);
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

/// A crossing's zone: per side of the way, the fill line where its fill falls to the
/// pipe's need, scanned along the water; then the way's band out to it on each side within the water's band.
fn crossing_zone(ctx: &Ctx, seed: &Seed, stats: &mut CrossingStats) -> Option<Zone> {
    let g = ctx.graph;
    let p = &ctx.rules.plan.heights;
    let sec = ctx.way_section(seed.w);
    let water = ctx.water_section(seed.s);
    let cr = water.crossing;
    let way = Axis::through(g, seed.w, seed.ws, ZONE_READ_M);
    let axis = match seed.one {
        None => Axis::through(g, seed.s, seed.ss, ZONE_ALONG_M),
        Some(d) => Axis { points: g.walk(seed.s, seed.ss, d, ZONE_ALONG_M, false).swap_remove(0) },
    };
    if way.points.len() < 2 || axis.points.len() < 2 {
        return None;
    }
    // The pipes across the water: as many as its width asks, as few as fit.
    let mut n = libm::ceil(cr.pipe_ratio * water.width_m / cr.pipe_max_m).clamp(1.0, cr.pipes_max);
    let d0 = |n: f64| (cr.pipe_ratio * water.width_m / n).clamp(cr.pipe_min_m, cr.pipe_max_m);
    while n > 1.0 && n * d0(n) + (n - 1.0) * PIPE_GAP_M > 2.0 * water.half_m {
        n -= 1.0;
    }
    let over_bed = d0(n) + if sec.stone_mouth { cr.stones_cap_m } else { cr.headwall_cap_m };
    let e = sec.half_m + g.samples[seed.ws as usize].widen_m + sec.shoulder_m;
    let d_max = (sec.verge_m + sec.reach_m - MOUTH_ROOM_M).max(0.0);
    let mpu = ctx.mpu(seed.pos.map(f64::from));
    // The water's halves from the seed, each its points from the seed out, on one side of the way.
    let zero = axis.points.iter().position(|w| w.m >= 0.0).unwrap_or(0);
    let halves: Vec<Vec<usize>> = [(zero..axis.points.len()).collect(), (0..=zero).rev().collect::<Vec<_>>()]
        .into_iter()
        .filter(|h: &Vec<usize>| h.len() >= 2)
        .collect();
    let mut d = [None, None];
    for half in &halves {
        let sd: Vec<f64> = half.iter().map(|&k| signed(&way, axis.pos(k), mpu).0).collect();
        let side = if sd[sd.len() - 1] >= 0.0 { 0 } else { 1 };
        let sigma = if side == 0 { 1.0 } else { -1.0 };
        if d[side].is_some() {
            continue;
        }
        // The first point of the half at `o` metres or more on its side of the way, and its metres along.
        let q_at = |o: f64| -> Option<([f64; 2], f64)> {
            let k = (0..sd.len()).find(|&k| sigma * sd[k] >= o)?;
            if k == 0 {
                return Some((axis.pos(half[0]), axis.points[half[0]].m));
            }
            let (s0, s1) = (sigma * sd[k - 1], sigma * sd[k]);
            let f = if s1 > s0 { (o - s0) / (s1 - s0) } else { 1.0 };
            let (a, b) = (axis.pos(half[k - 1]), axis.pos(half[k]));
            let (ma, mb) = (axis.points[half[k - 1]].m, axis.points[half[k]].m);
            Some(([a[0] + f * (b[0] - a[0]), a[1] + f * (b[1] - a[1])], ma + f * (mb - ma)))
        };
        // Whether the scan stops at grid step i: past the water's half (None), or at its fill line.
        let stop = |i: usize| -> Option<bool> {
            let (q, m) = q_at(e + i as f64 * SCAN_STEP_M)?;
            let need = axis.level(m) - water.depth_m + over_bed;
            Some(fill(ctx, &way, &sec, q, sigma, p) <= need.max(seed.level + cr.dry_m))
        };
        // The first grid step that stops: every `SCAN_COARSE` steps, then the fine steps of the bracket that
        // stops. The fill falls away from the edge, so a bracket that does not stop at its end has no stop.
        let n = libm::floor(d_max / SCAN_STEP_M + 1e-9) as usize;
        let mut c = 0;
        while c < n && stop(c) == Some(false) {
            c = (c + SCAN_COARSE).min(n);
        }
        let first = (c.saturating_sub(SCAN_COARSE - 1)..=c).find(|&i| stop(i) != Some(false));
        let step = |i: usize| i as f64 * SCAN_STEP_M;
        let (found, capped, last) = match first.map(|i| (i, stop(i))) {
            Some((i, Some(true))) => (Some(step(i.max(1) - 1)), false, step(i)),
            Some((i, None)) => (None, true, step(i.max(1) - 1)),
            _ => (None, false, step(n)),
        };
        stats.zone_capped += u32::from(capped);
        stats.zone_tall += u32::from(found.is_none() && !capped);
        d[side] = Some(found.unwrap_or(last));
    }
    // The way's band out to the fill line on each side (a missing side to its edge), over the water's band.
    let band_s = water_band(ctx, seed, cr.mouth_bank_m);
    let band_w = ctx.band(&way, seed.pos, [d[0].unwrap_or(0.0), d[1].unwrap_or(0.0)], bounds(&band_s))?;
    let anchor = seed.pos;
    let q2 = [2 * i64::from(seed.pos[0]), 2 * i64::from(seed.pos[1])];
    let rings = shapes(&band_w, &band_s, OverlayRule::Intersect, anchor).into_iter().find(|s| winding(s, q2) != 0)?;
    Some(Zone { rings, way: Some(seed.w), seed: ctx.global(seed.pos), mouths: Vec::new() })
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

/// The fill of the way's edge at the foot of q on its line, on side `sigma`, as the heights shape it: the
/// edge between the foot's two samples at the owner curve's height there, its cone by the no-room rule of
/// the terrain where it lets go, rounded at its foot and released at its reach.
fn fill(
    ctx: &Ctx,
    way: &Axis,
    sec: &crate::sections::WaySection,
    q: [f64; 2],
    sigma: f64,
    p: &crate::faces::heights::HeightRules,
) -> f64 {
    let g = ctx.graph;
    let mpu = ctx.mpu(q);
    let (_, k) = signed(way, q, mpu);
    let (pa, pb) = (way.pos(k), way.pos(k + 1));
    let n = left(pa, pb);
    let (sa, sb) = (&g.samples[way.points[k].sample as usize], &g.samples[way.points[k + 1].sample as usize]);
    let e = |s: &crate::graph::Sample| sec.half_m + s.widen_m + sec.shoulder_m;
    let out = |p: [f64; 2], m: f64| [p[0] + sigma * n[0] * m / mpu, p[1] + sigma * n[1] * m / mpu];
    let (a, b) = (out(pa, e(sa)), out(pb, e(sb)));
    let h = [sa.z_m + sa.bank * sigma * e(sa), sb.z_m + sb.bank * sigma * e(sb)];
    let go = sec.reach_m * (1.0 - p.fade_share);
    let t = [ctx.terrain(out(a, go)), ctx.terrain(out(b, go))];
    let wall = if sec.leg_wall { p.wall_max_m } else { 0.0 };
    let (sf, wf) = fit(sec.fill, sec.fill_max, [h[0] - t[0], h[1] - t[1]], wall, p, sec.reach_m, 0.0);
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len = libm::hypot(ab[0], ab[1]);
    let u = if len > 0.0 { [ab[0] / len, ab[1] / len] } else { [1.0, 0.0] };
    let v = [q[0] - a[0], q[1] - a[1]];
    let (along, off, len) = ((v[0] * u[0] + v[1] * u[1]) * mpu, (u[0] * v[1] - u[1] * v[0]).abs() * mpu, len * mpu);
    let foot = along.clamp(0.0, len) - along;
    let d = libm::hypot(foot, off);
    if d >= sec.reach_m {
        return f64::NEG_INFINITY;
    }
    edge_cone(along, off, len, [h[0] - wf[0], h[1] - wf[1]], sf, 1.0, 0.0) + round_back(sec.fill, d, p)
        - release(d, sec.reach_m, p).1
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
    Zone { rings, way, seed: ctx.global(pos), mouths: Vec::new() }
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

/// Each place where the axis of a water way leaves a zone: a mouth where open water follows, else, if the
/// axis meets a zone again within `mouth_join_m` along it, the water's band between joins the zone that it
/// leaves.
fn join(ctx: &Ctx, zones: &mut [Zone], stats: &mut CrossingStats) {
    let g = ctx.graph;
    let boxes: Vec<[f64; 4]> =
        zones.iter().map(|z| bounds(&z.rings)).map(|b| [b[0] - 1.0, b[1] - 1.0, b[2] + 1.0, b[3] + 1.0]).collect();
    // Whether the water past an exit is open: its way, or the next one past a culvert's end, on the ground.
    let open = |w: u32, at: u32, dir: i8| {
        let ground = |p: &WalkPoint| g.ways[p.way as usize].level == Level::Ground;
        g.walk(w, at, dir, 0.5, false).swap_remove(0).iter().any(ground)
    };
    let mut adds: Vec<Vec<Vec<P>>> = vec![Vec::new(); zones.len()];
    let mut mouths: Vec<Vec<([i32; 2], [f64; 2])>> = vec![Vec::new(); zones.len()];
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
                match entry {
                    Some((i, hit, at)) if name(z, x) < name(&zones[hit], at) => {
                        let mut gap: Vec<[f64; 2]> = line[..i].to_vec();
                        gap.push(at);
                        let h = (water.half_m + water.crossing.mouth_bank_m) / mpu;
                        adds[zi].extend(stroke(&gap, &vec![h; gap.len()], [false, false]));
                        stats.joins += 1;
                    }
                    Some(_) => {}
                    None if open(w.id, next as u32, dir) => {
                        let d = [pos(next)[0] - x[0], pos(next)[1] - x[1]];
                        let l = libm::hypot(d[0], d[1]).max(1e-9);
                        mouths[zi].push(([libm::round(x[0]) as i32, libm::round(x[1]) as i32], [d[0] / l, d[1] / l]));
                    }
                    None => {}
                }
            }
        }
    }
    for ((z, add), m) in zones.iter_mut().zip(adds).zip(mouths) {
        z.mouths = m;
        if !add.is_empty() {
            let mut all = z.rings.clone();
            all.extend(add);
            z.rings =
                union(&all, [(z.seed[0] - ctx.cell().origin().x) as i32, (z.seed[1] - ctx.cell().origin().y) as i32]);
        }
    }
}
