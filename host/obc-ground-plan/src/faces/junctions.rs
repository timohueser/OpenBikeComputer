//! The rules at the graph's points and along tight bends: joint tapers, fans and kinks; free end caps;
//! junction mouths (behind runs, blend parts) and corners; tip fills.

use core::f64::consts::PI;

use super::bands::{
    cap, fan_pieces, layer_cand, layer_label, metres, normal, steps, sweep, turn, way_key, WaySec, LEFT, RIGHT, S,
};
use super::cands::{Cands, Key, Label, KIND_BEND, KIND_JUNCTION};
use super::corners::{corner, Chain, Corner, Cut, Foot, Seam, Spec};
use super::geom::{cross, round, P};
use super::rules::{Mouth, Rank};
use super::types::{CornerArc, FaceStats, HeightRule, Owner, Surface};
use super::Ctx;
use crate::frame::metres_per_unit;
use crate::graph::{Arm, ROOM_MAX_M};

/// A joint is a kink where its two tangents differ by more than this.
const KINK_DEG: f64 = 2.0;

/// What the point rules hand to the bands: new samples, removed layer bands, carriageway stretches
/// that belong to a blend part, and the corner arcs.
#[derive(Default)]
pub struct Edits {
    pub feet: Vec<Foot>,
    pub cuts: Vec<Cut>,
    pub takes: Vec<(u32, f64, f64, u32, Key)>,
    pub arcs: Vec<CornerArc>,
    /// Corner fills to close once every foot is in its way: (candidate, key, first point, seams, arc).
    pub fills: Vec<(u32, Key, P, [Seam; 2], Vec<P>)>,
}

/// The arm's sample on its way's sections, and whether the way ends there.
fn at(w: &WaySec, arm: &Arm) -> (usize, bool) {
    let ord = f64::from(arm.at.span) + arm.at.u;
    let i = (0..w.s.len()).min_by(|&a, &b| (w.s[a].ord - ord).abs().total_cmp(&(w.s[b].ord - ord).abs())).unwrap();
    (i, if arm.dir > 0 { i == 0 } else { i + 1 == w.s.len() })
}

/// The arm's side facing the next arm by angle (`next`), or the previous one.
fn facing(arm: &Arm, next: bool) -> usize {
    if (arm.dir > 0) == next {
        RIGHT
    } else {
        LEFT
    }
}

/// The samples from i in direction dir until `room` metres are passed (one more), or the way ends.
fn walk(w: &WaySec, i: usize, dir: i8, room: f64) -> Vec<usize> {
    let mut idx = vec![i];
    let mut m = 0.0;
    let mut j = i;
    while m <= room {
        let next = if dir > 0 { j + 1 } else { j.wrapping_sub(1) };
        if next >= w.s.len() {
            break;
        }
        m += metres(w.s[j].g, w.s[next].g);
        idx.push(next);
        j = next;
    }
    idx
}

/// The point `m` metres from sample i in direction dir, as a sample; None past the way's end.
fn along(w: &WaySec, i: usize, dir: i8, m: f64, ctx: &Ctx) -> Option<S> {
    let mut left = m;
    let mut j = i;
    loop {
        let next = if dir > 0 { j + 1 } else { j.wrapping_sub(1) };
        if next >= w.s.len() {
            return None;
        }
        let d = metres(w.s[j].g, w.s[next].g);
        if left <= d {
            return Some(S::lerp(&w.s[j], &w.s[next], if d == 0.0 { 0.0 } else { left / d }, ctx.origin));
        }
        left -= d;
        j = next;
    }
}

fn global(ctx: &Ctx, p: P) -> [i64; 2] {
    [ctx.origin.x + i64::from(p[0]), ctx.origin.y + i64::from(p[1])]
}

fn local(ctx: &Ctx, g: [i64; 2]) -> P {
    [(g[0] - ctx.origin.x) as i32, (g[1] - ctx.origin.y) as i32]
}

/// A global point plus m metres along d.
fn shift(g: [i64; 2], d: [f64; 2], m: f64) -> [i64; 2] {
    let u = m / metres_per_unit(g[1]);
    [g[0] + round(u * d[0]), g[1] + round(u * d[1])]
}

fn heading(w: &WaySec, i: usize, dir: i8) -> [f64; 2] {
    let t = w.s[i].t;
    [f64::from(dir) * t[0], f64::from(dir) * t[1]]
}

/// The angle of a vector from +x toward +y.
fn angle(v: [f64; 2]) -> f64 {
    libm::atan2(v[1], v[0])
}

/// The lesser of two ways: the later class, then the narrower, then the greater global identity.
fn lesser<'a>(a: &'a WaySec, b: &'a WaySec) -> &'a WaySec {
    let key = |w: &WaySec| (w.class, -(w.half_m * 1e3) as i64, w.s[0].gid);
    if key(a) >= key(b) {
        a
    } else {
        b
    }
}

/// The labels of a corner's fill and rings, and its key.
struct Labels {
    owner: Owner,
    fill_rank: Rank,
    material: u32,
    key: Key,
}

#[allow(clippy::too_many_arguments)]
fn emit(
    ctx: &Ctx,
    cands: &mut Cands,
    edits: &mut Edits,
    lab: &Labels,
    lw: &WaySec,
    c: Corner,
    stats_key: &str,
    stats: &mut FaceStats,
) {
    let rank = |r: Rank| ctx.rules.plan.rank(r);
    let fill = Label {
        owner: lab.owner,
        surface: Surface::CornerFill,
        rule: HeightRule::Blend,
        material: Some(lab.material),
        crossing: None,
        rank: rank(lab.fill_rank),
        reach_m: lw.reach_m as f32,
    };
    let key = Key { rank: rank(lab.fill_rank), ..lab.key };
    let id = cands.add(fill, key);
    edits.fills.push((id, key, c.c, c.seams, c.arc.clone()));
    for (k, ring) in c.rings {
        let (surface, rule, r) = if k == 1 {
            (Surface::Shoulder, HeightRule::Blend, if lw.shoulder_paved { Rank::ShoulderPaved } else { Rank::Shoulder })
        } else {
            (Surface::Verge, HeightRule::Slope, Rank::Verge)
        };
        let label = Label { surface, rule, rank: rank(r), ..fill };
        // A ring wins over the arms' own bands of its rank (class 0), so no band needs a cut.
        let key = Key { rank: rank(r), class: 0, number: lab.key.number + k as u32, ..lab.key };
        let id = cands.add(label, key);
        cands.ring(id, key, ring);
    }
    let entry = stats.corners.entry(stats_key.to_string()).or_default();
    entry[0] += 1;
    entry[1] += u32::from(c.shrunk);
    edits.feet.extend(c.feet.into_iter().filter(|f| f.new));

    edits.arcs.push(CornerArc {
        owner: lab.owner,
        centre: c.centre,
        radius_m: c.radius_m,
        t_a: c.t[0],
        t_b: c.t[1],
        arc: c.arc,
    });
}

/// Each way's half-widths and layer edges run to their mean over `joint_taper_m` at a joint.
pub fn taper(ctx: &Ctx, secs: &mut [Option<WaySec>]) {
    let t = ctx.rules.plan.joint_taper_m;
    for joint in &ctx.graph.joints {
        let [a, b] = &joint.arms;
        let (Some(wa), Some(wb)) = (&secs[a.way as usize], &secs[b.way as usize]) else { continue };
        let ((ia, ea), (ib, eb)) = (at(wa, a), at(wb, b));
        if !ea || !eb {
            continue;
        }
        // Ways in one direction through the joint have their left sides together.
        let flip = a.dir == b.dir;
        let (oa, ob) = (wa.s[ia].o, wb.s[ib].o);
        let mean = |side: usize| -> [f64; 3] {
            let other = if flip { 1 - side } else { side };
            std::array::from_fn(|k| (oa[side][k] + ob[other][k]) / 2.0)
        };
        let target_a = [mean(LEFT), mean(RIGHT)];
        let target_b: [[f64; 3]; 2] =
            std::array::from_fn(|side| if flip { target_a[1 - side] } else { target_a[side] });
        for (arm, i, target) in [(a, ia, target_a), (b, ib, target_b)] {
            let Some(w) = secs[arm.way as usize].as_mut() else { continue };
            let total: f64 = w.s.windows(2).map(|p| metres(p[0].g, p[1].g)).sum();
            let reach = t.min(total / 2.0);
            if reach <= 0.0 {
                continue;
            }
            let end = w.s[i].o;
            let mut m = 0.0;
            let mut j = i;
            loop {
                let f = 1.0 - m / reach;
                if f <= 0.0 {
                    break;
                }
                for side in 0..2 {
                    for k in 0..3 {
                        w.s[j].o[side][k] += (target[side][k] - end[side][k]) * f;
                    }
                }
                w.s[j].place(ctx.origin);
                let next = if arm.dir > 0 { j + 1 } else { j.wrapping_sub(1) };
                if next >= w.s.len() {
                    break;
                }
                m += metres(w.s[j].g, w.s[next].g);
                j = next;
            }
        }
    }
}

/// The fan between two way ends at one point, over the wedge between their sections on the side
/// that faces away from both: a's half to the bisector, then b's.
#[allow(clippy::too_many_arguments)]
fn fan(
    ctx: &Ctx,
    cands: &mut Cands,
    lc: &mut [[Option<u32>; 5]],
    secs: &[Option<WaySec>],
    a: (&Arm, usize),
    b: (&Arm, usize),
) {
    let (wa, wb) = (secs[a.0.way as usize].as_ref().unwrap(), secs[b.0.way as usize].as_ref().unwrap());
    let (sa, sb) = (&wa.s[a.1], &wb.s[b.1]);
    if sa.g != sb.g {
        return;
    }
    let side_a = facing(a.0, true);
    let side_b = facing(b.0, false);
    let dir = |s: &S, side: usize| {
        let n = normal(s.t);
        if side == LEFT {
            n
        } else {
            [-n[0], -n[1]]
        }
    };
    let (ua, ub) = (dir(sa, side_a), dir(sb, side_b));
    let wedge = libm::atan2(ua[0] * ub[1] - ua[1] * ub[0], ua[0] * ub[0] + ua[1] * ub[1]);
    if wedge <= 0.0 {
        return;
    }
    let n = steps(wedge, true);
    let dirs = turn(ua, wedge, n);
    let pts = |s: &S, side: usize| -> [P; 3] { std::array::from_fn(|k| s.p[S::bound(side, k)]) };
    let line = sweep(ctx.origin, sa.g, &dirs, pts(sa, side_a), pts(sb, side_b), sa.o[side_a], sb.o[side_b]);
    let mut owners = Vec::new();
    for (w, s, side) in [(wa, sa, side_a), (wb, sb, side_b)] {
        let ls = if side == LEFT { [2, 3, 4] } else { [2, 1, 0] };
        let lcw = &mut lc[w.way as usize];
        let carriage = layer_cand(ctx, cands, w, lcw, 2);
        let outer: [Option<u32>; 2] = std::array::from_fn(|k| {
            let l = ls[k + 1];
            (s.p[l] != s.p[l + 1] || s.p[ls[k]] != s.p[l]).then(|| layer_cand(ctx, cands, w, lcw, l))
        });
        let keys: [Key; 3] =
            std::array::from_fn(|k| way_key(ctx, w, layer_label(ctx, w, ls[k]).1, s.gid, ls[k] as u32));
        owners.push((carriage, outer, keys));
    }
    fan_pieces(cands, sa.c, &line, &|j| owners[usize::from(j >= n / 2)]);
}

/// Every rule at the graph's points, and the tip fills of tight bends.
pub fn build(
    ctx: &Ctx,
    secs: &[Option<WaySec>],
    cands: &mut Cands,
    lc: &mut [[Option<u32>; 5]],
    stats: &mut FaceStats,
) -> Edits {
    let mut edits = Edits::default();
    let g = ctx.graph;
    let sec = |arm: &Arm| secs[arm.way as usize].as_ref();
    for end in &g.ends {
        if let Some(w) = sec(&end.arm) {
            let (i, ends) = at(w, &end.arm);
            if ends {
                cap(ctx, cands, w, &mut lc[w.way as usize], i + 1 == w.s.len());
            }
        }
    }
    for joint in &g.joints {
        let [a, b] = &joint.arms;
        let (Some(wa), Some(wb)) = (sec(a), sec(b)) else { continue };
        let ((ia, ea), (ib, eb)) = (at(wa, a), at(wb, b));
        if !ea || !eb {
            continue;
        }
        let (ha, hb) = (heading(wa, ia, a.dir), heading(wb, ib, b.dir));
        let sector = (angle(hb) - angle(ha)).rem_euclid(2.0 * PI);
        // The side from a to b by angle, and the other.
        for (x, y, s, xi, yi) in [(a, b, sector, ia, ib), (b, a, 2.0 * PI - sector, ib, ia)] {
            if s > PI {
                fan(ctx, cands, lc, secs, (x, xi), (y, yi));
            } else if s < PI - KINK_DEG.to_radians() {
                let (wx, wy) = (sec(x).unwrap(), sec(y).unwrap());
                let lw = lesser(wx, wy);
                let r0 = lw.inner_radius_m;
                let dm = round(lw.s[if lw.way == wx.way { xi } else { yi }].along_m * 10.0);
                let gx = wx.s[xi].g;
                let lab = Labels {
                    owner: Owner::Bend(lw.way, dm),
                    fill_rank: Rank::Junction,
                    material: lw.way,
                    key: Key { rank: 0, class: lw.class, kind: KIND_BEND, gid: [gx[0], gx[1], 0, 0, 0], number: 0 },
                };
                let room = ctx.rules.plan.tip_window_m;
                let ca = Chain {
                    w: wx,
                    idx: walk(wx, xi, x.dir, room),
                    side: facing(x, true),
                    seam: 0,
                    room_m: room.min(x.room_m),
                };
                let cb = Chain {
                    w: wy,
                    idx: walk(wy, yi, y.dir, room),
                    side: facing(y, false),
                    seam: 0,
                    room_m: room.min(y.room_m),
                };
                tip(ctx, cands, &mut edits, &lab, lw, &ca, &cb, r0, gx, wx.s[xi].c, "kink", stats);
            }
        }
    }
    for (jid, j) in g.junctions.iter().enumerate() {
        let arms: Vec<&Arm> = j.arms.iter().filter(|a| sec(a).is_some()).collect();
        if arms.len() < 3 {
            continue;
        }
        junction(ctx, secs, cands, lc, &mut edits, jid as u32, &arms, stats);
    }
    for w in secs.iter().flatten() {
        bends(ctx, w, cands, &mut edits, stats);
    }
    edits
}

#[allow(clippy::too_many_arguments)]
fn tip(
    ctx: &Ctx,
    cands: &mut Cands,
    edits: &mut Edits,
    lab: &Labels,
    lw: &WaySec,
    a: &Chain,
    b: &Chain,
    r0: f64,
    x: [i64; 2],
    c: P,
    stats_key: &str,
    stats: &mut FaceStats,
) {
    let plan = &ctx.rules.plan;
    if r0 < plan.least_radius_m || a.idx.len() < 2 || b.idx.len() < 2 {
        return;
    }
    let spec = Spec { r0, x, extent_m: plan.extent_m, least_m: plan.least_radius_m, c };
    match corner(ctx.origin, a, b, &spec) {
        Some(corner) => emit(ctx, cands, edits, lab, lw, corner, stats_key, stats),
        None => stats.corners.entry(stats_key.to_string()).or_default()[2] += 1,
    }
}

/// Tight bends: where the inner edge turns tighter than the class's inner radius, the sample with the
/// greatest curvature within `tip_window_m` takes a tip fill.
fn bends(ctx: &Ctx, w: &WaySec, cands: &mut Cands, edits: &mut Edits, stats: &mut FaceStats) {
    let plan = &ctx.rules.plan;
    let n = w.s.len();
    if w.inner_radius_m < plan.least_radius_m || n < 3 {
        return;
    }
    let mut m = vec![0.0];
    for i in 1..n {
        m.push(m[i - 1] + metres(w.s[i - 1].g, w.s[i].g));
    }
    // Signed curvature, positive to the left.
    let kappa: Vec<f64> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let (t0, t1) = (w.s[a].t, w.s[b].t);
            let turn = -libm::atan2(t0[0] * t1[1] - t0[1] * t1[0], t0[0] * t1[0] + t0[1] * t1[1]);
            if m[b] > m[a] {
                turn / (m[b] - m[a])
            } else {
                0.0
            }
        })
        .collect();
    let side = |k: f64| if k > 0.0 { LEFT } else { RIGHT };
    let tight = |i: usize| {
        let k = kappa[i];
        k != 0.0 && k.abs() * (w.s[i].o[side(k)][0] + w.inner_radius_m) > 1.0
    };
    for c in 1..n - 1 {
        if !tight(c) {
            continue;
        }
        let near = (0..n).filter(|&i| i != c && (m[i] - m[c]).abs() <= plan.tip_window_m);
        let beaten = near.into_iter().any(|i| {
            tight(i)
                && side(kappa[i]) == side(kappa[c])
                && (kappa[i].abs(), std::cmp::Reverse(i)) > (kappa[c].abs(), std::cmp::Reverse(c))
        });
        if beaten {
            continue;
        }
        let s = side(kappa[c]);
        let room = plan.tip_window_m;
        let ca = Chain { w, idx: walk(w, c, -1, room), side: s, seam: 0, room_m: room };
        let cb = Chain { w, idx: walk(w, c, 1, room), side: s, seam: 0, room_m: room };
        let g = w.s[c].g;
        let lab = Labels {
            owner: Owner::Bend(w.way, round(w.s[c].along_m * 10.0)),
            fill_rank: Rank::Junction,
            material: w.way,
            key: Key { rank: 0, class: w.class, kind: KIND_BEND, gid: [g[0], g[1], 0, 0, 0], number: 0 },
        };
        tip(ctx, cands, edits, &lab, w, &ca, &cb, w.inner_radius_m, g, w.s[c].p[S::bound(s, 0)], "bend", stats);
    }
}

/// A junction's mouths and corners.
#[allow(clippy::too_many_arguments)]
fn junction(
    ctx: &Ctx,
    secs: &[Option<WaySec>],
    cands: &mut Cands,
    lc: &mut [[Option<u32>; 5]],
    edits: &mut Edits,
    jid: u32,
    arms: &[&Arm],
    stats: &mut FaceStats,
) {
    let plan = &ctx.rules.plan;
    let j = &ctx.graph.junctions[jid as usize];
    let pj = global(ctx, j.pos);
    let w = |arm: &Arm| secs[arm.way as usize].as_ref().unwrap();
    let Some(host) = arms.iter().filter(|a| a.host).min_by_key(|a| (w(a).class, w(a).s[0].gid)) else { return };
    let hw = w(host);
    let (hi, _) = at(hw, host);
    let jkey = |rank: Rank, class: u8, number: u32| Key {
        rank: plan.rank(rank),
        class,
        kind: KIND_JUNCTION,
        gid: [pj[0], pj[1], 0, 0, 0],
        number,
    };
    for (n, arm) in arms.iter().enumerate().filter(|(_, a)| !a.host) {
        let gw = w(arm);
        let pair = plan.pair(&hw.group, &gw.group);
        if pair.mouth == Mouth::Join {
            continue;
        }
        let (gi, _) = at(gw, arm);
        let gs = &gw.s[gi];
        let u = heading(gw, gi, arm.dir);
        let h_h = (hw.s[hi].o[LEFT][0] + hw.s[hi].o[RIGHT][0]) / 2.0;
        let h_g = (gs.o[LEFT][0] + gs.o[RIGHT][0]) / 2.0;
        let back = [-u[0], -u[1]];
        let rect = vec![
            gs.p[2],
            gs.p[3],
            local(ctx, shift(global(ctx, gs.p[3]), back, h_h + h_g)),
            local(ctx, shift(global(ctx, gs.p[2]), back, h_h + h_g)),
        ];
        // The half disk past the host's middle line, on the side away from the guest.
        let th = hw.s[hi].t;
        let nh = normal(th);
        let sg = if u[0] * nh[0] + u[1] * nh[1] >= 0.0 { 1.0 } else { -1.0 };
        let far = [-sg * nh[0], -sg * nh[1]];
        let count = steps(PI, true);
        let disk: Vec<P> = (0..=count)
            .map(|k| {
                let a = PI * k as f64 / count as f64;
                let d = [libm::cos(a) * th[0] + libm::sin(a) * far[0], libm::cos(a) * th[1] + libm::sin(a) * far[1]];
                local(ctx, shift(pj, d, h_h + h_g + 1.0))
            })
            .collect();
        let rank = if pair.mouth == Mouth::Reach { Rank::Junction } else { Rank::JunctionLow };
        let (label, key) = if pair.blend {
            let label = Label {
                owner: Owner::Junction(jid),
                surface: Surface::Carriageway,
                rule: HeightRule::Blend,
                material: Some(gw.way),
                crossing: None,
                rank: plan.rank(rank),
                reach_m: gw.reach_m as f32,
            };
            (label, jkey(rank, gw.class, n as u32))
        } else {
            let (label, rank) = layer_label(ctx, gw, 2);
            (label, way_key(ctx, gw, rank, gs.gid, 2))
        };
        let cand = cands.add(label, key);
        cands.cands[cand as usize].minus = vec![super::geom::with_cell_lines(&disk)];
        cands.ring(cand, key, rect);
        if pair.blend {
            let hosts: Vec<(&WaySec, usize)> = arms.iter().filter(|a| a.host).map(|a| (w(a), at(w(a), a).0)).collect();
            let e = ramp(ctx, gw, gi, arm, &hosts, pair.mouth, stats);
            if e > 0.0 {
                if let Some(s) = along(gw, gi, arm.dir, e, ctx) {
                    let (lo, hi_ord) = (gs.ord.min(s.ord), gs.ord.max(s.ord));
                    edits.takes.push((gw.way, lo, hi_ord, cand, key));
                    edits.feet.push(Foot { way: gw.way, s, new: true });
                } else {
                    let end = &gw.s[if arm.dir > 0 { gw.s.len() - 1 } else { 0 }];
                    edits.takes.push((gw.way, gs.ord.min(end.ord), gs.ord.max(end.ord), cand, key));
                }
            }
        }
    }
    // Corners between neighbouring arms.
    let count = arms.len();
    for i in 0..count {
        let (a, b) = (arms[i], arms[(i + 1) % count]);
        let sector = (b.angle - a.angle).rem_euclid(2.0 * PI);
        let (wa, wb) = (w(a), w(b));
        let ((ia, ea), (ib, eb)) = (at(wa, a), at(wb, b));
        if sector > PI && ea && eb {
            fan(ctx, cands, lc, secs, (a, ia), (b, ib));
            continue;
        }
        if sector >= plan.straight_deg.to_radians() || sector == 0.0 {
            continue;
        }
        // A host and a guest take the guest's rule; two hosts the better one's group with itself; two
        // guests the rule with the better guest as host. `host_arm` takes the core seam of a stop.
        let lw = lesser(wa, wb);
        let (better, better_arm) = if lw.way == wa.way { (wb, 1) } else { (wa, 0) };
        let (rule_host, rule_guest, host_arm) = match (a.host, b.host) {
            (true, false) => (hw, wb, Some(0)),
            (false, true) => (hw, wa, Some(1)),
            (true, true) => (better, better, None),
            (false, false) => (better, lw, Some(better_arm)),
        };
        let pair = plan.pair(&rule_host.group, &rule_guest.group);
        if pair.mouth == Mouth::Join || pair.radius_m <= 0.0 {
            continue;
        }
        let (sa, sb) = (facing(a, true), facing(b, false));
        let walk_bit = |w: &WaySec, i: usize, arm: &Arm, side: usize| {
            let k = if arm.dir > 0 { i } else { i.saturating_sub(1) };
            w.s[k].walk & if side == LEFT { 1 } else { 2 } != 0
        };
        let walks = walk_bit(wa, ia, a, sa) || walk_bit(wb, ib, b, sb);
        let r0 = match pair.radius_walk_m {
            Some(r) if walks => r,
            _ => pair.radius_m,
        };
        let seam = |x: &WaySec, which: usize| {
            usize::from(pair.mouth == Mouth::Stop && host_arm == Some(which) && x.shoulder_paved && x.shoulder_m > 0.0)
        };
        let room = |arm: &Arm| {
            let r = if arm.room_m < ROOM_MAX_M { arm.room_m * plan.room_share } else { arm.room_m };
            r.min(plan.extent_m)
        };
        let ca = Chain { w: wa, idx: walk(wa, ia, a.dir, room(a)), side: sa, seam: seam(wa, 0), room_m: room(a) };
        let cb = Chain { w: wb, idx: walk(wb, ib, b.dir, room(b)), side: sb, seam: seam(wb, 1), room_m: room(b) };
        let lab = Labels {
            owner: Owner::Junction(jid),
            fill_rank: if pair.mouth == Mouth::Reach { Rank::Fill } else { Rank::JunctionLow },
            material: lw.way,
            key: jkey(Rank::Fill, lw.class, 100 + 10 * i as u32),
        };
        let name = format!("{}/{}", rule_host.group, rule_guest.group);
        tip(ctx, cands, edits, &lab, lw, &ca, &cb, r0, pj, j.pos, &name, stats);
    }
}

/// The ramp end of a guest's blend part, metres from the junction along the guest: the first crossing
/// of the guest's middle line with a host's seam edge, plus the return ramp for the height step there,
/// capped at the guest's next point and `ramp_end_m`.
fn ramp(
    ctx: &Ctx,
    gw: &WaySec,
    gi: usize,
    arm: &Arm,
    hosts: &[(&WaySec, usize)],
    mouth: Mouth,
    stats: &mut FaceStats,
) -> f64 {
    let plan = &ctx.rules.plan;
    let room = if arm.room_m < ROOM_MAX_M { arm.room_m } else { f64::INFINITY };
    let line = walk(gw, gi, arm.dir, plan.extent_m.min(room));
    let u = heading(gw, gi, arm.dir);
    let near = |hi: usize, len: usize| hi.saturating_sub(64)..(hi + 64).min(len);
    let mut s_seam: Option<f64> = None;
    for &(hw, hi) in hosts {
        let n = normal(hw.s[hi].t);
        let side = if u[0] * n[0] + u[1] * n[1] >= 0.0 { LEFT } else { RIGHT };
        let k = usize::from(mouth == Mouth::Stop && hw.shoulder_paved && hw.shoulder_m > 0.0);
        let seam: Vec<P> = near(hi, hw.s.len()).map(|i| hw.s[i].p[S::bound(side, k)]).collect();
        let mut m = 0.0;
        'line: for pair in line.windows(2) {
            let (p, q) = (gw.s[pair[0]].c, gw.s[pair[1]].c);
            let d = metres(gw.s[pair[0]].g, gw.s[pair[1]].g);
            for e in seam.windows(2) {
                let (d1, d2) = (cross(e[0], e[1], p), cross(e[0], e[1], q));
                let (d3, d4) = (cross(p, q, e[0]), cross(p, q, e[1]));
                if d1.signum() * d2.signum() <= 0 && d3.signum() * d4.signum() <= 0 && d1 != d2 {
                    let at = m + d1 as f64 / (d1 - d2) as f64 * d;
                    s_seam = Some(s_seam.map_or(at, |x: f64| x.min(at)));
                    break 'line;
                }
            }
            m += d;
        }
    }
    let s_seam = s_seam.unwrap_or_else(|| {
        stats.seam_missing += 1;
        0.0
    });
    let gp = along(gw, gi, arm.dir, s_seam.min(room), ctx).unwrap_or_else(|| gw.s[gi].clone());
    // The host's height at the guest point: the nearest station, plus the bank across.
    let mut best: Option<(f64, f64, f64, f64)> = None;
    for &(hw, hi) in hosts {
        let r = near(hi, hw.s.len());
        for i in r.start..r.end.saturating_sub(1) {
            let (a, b) = (&hw.s[i], &hw.s[i + 1]);
            let ab = [(b.g[0] - a.g[0]) as f64, (b.g[1] - a.g[1]) as f64];
            let ap = [(gp.g[0] - a.g[0]) as f64, (gp.g[1] - a.g[1]) as f64];
            let len = ab[0] * ab[0] + ab[1] * ab[1];
            let f = if len == 0.0 { 0.0 } else { ((ap[0] * ab[0] + ap[1] * ab[1]) / len).clamp(0.0, 1.0) };
            let d = [ap[0] - f * ab[0], ap[1] - f * ab[1]];
            let dist = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if best.is_none_or(|x| dist < x.0) {
                let n = normal(a.t);
                let across = (d[0] * n[0] + d[1] * n[1]) * metres_per_unit(a.g[1]);
                best = Some((dist, a.z + (b.z - a.z) * f, a.bank + (b.bank - a.bank) * f, across));
            }
        }
    }
    let Some((_, zh, bank, across)) = best else { return 0.0 };
    let dz = (gp.z - (zh + bank * across)).abs();
    let e = s_seam + plan.blend_steep * dz / gw.return_grade;
    if e > plan.ramp_end_m {
        stats.ramp_capped += 1;
    }
    e.min(plan.ramp_end_m).min(room)
}
