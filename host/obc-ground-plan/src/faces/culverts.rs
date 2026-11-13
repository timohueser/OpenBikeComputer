//! The culverts of line water under ways on the ground, as candidates of the face slice: a zone where the water
//! does not show. A ford is a culvert too: water at grade under a way. A road's or a railway's zone is the water's
//! band under the way and out over its verge, and as far past it as the way's fill stands more than the mouth's
//! height over the water, at least as far as the fill falls by that height. The water's face ends there and casts
//! no cone into the zone (`records`), so the ground of the slopes over it rises from the water's sides only: the
//! step from the water up to the zone is the mouth, an arch as wide as the water and at most as high as its bank's
//! steepest cut at the half-width. Where open water goes on in its tunnel, a zone over the tunnel's start makes the
//! same mouth. A path takes no zone: its surface lies across the water. Each zone is a function of global data
//! within its reach, so two cells build it alike.

use std::collections::BTreeMap;

use i_overlay::core::overlay_rule::OverlayRule;

use super::geom::{winding, P};
use super::stroke::{pieces_along, shapes, union, Rings};
use super::{heights, Cand, Ctx, HeightRule, Label, Owner, Surface, WayLine, TERRAIN, VERGE};
use crate::faces::rules::Rank;
use crate::frame::metres_per_unit;
use crate::graph::{Crossing, CrossingKind, Family, Level, WalkPoint};
use crate::sections::{Reach, HALF_MAX_M};

/// A zone reads the water this far along from its crossing: it holds a way's verge and fill past it at a
/// crossing angle down to about 30°.
pub const ZONE_ALONG_M: f64 = 25.0;
/// A zone reads its way this far along from its crossing: `ZONE_ALONG_M` and a way's half-width and verge.
pub const ZONE_READ_M: f64 = 35.0;
/// A zone lies within this of its seed: a crossing's zone within `ZONE_ALONG_M` along the water and the water's
/// band (its half-width and bank top), a tunnel start's within the water's reach along it and a round end of its
/// half-width and reach. The widest river (`HALF_MAX_M`, a reach of 15 m) meets it with the tunnel start's.
pub const REACH_M: f64 = ZONE_ALONG_M + 20.0;
/// Faces are final this far past the cell: the heights to 32.5 m, and a height reads the crossings' faces
/// within the water term of its reach, 15.5 m.
const FINAL_M: f64 = 48.0;

/// The rows of the culverts in the reach table, from the greatest bank reach and `culvert_max_m` of a water row.
/// The zone: what it reads, where it changes faces, and `FINAL_M`. An open culvert (`graph::open_culverts`): a
/// sample's level reads the crossing on its chain within `culvert_max_m`, whose class reads its chain as far again,
/// and the faces read the level; the graph's levels read no water of a culvert crossing.
pub fn reach_rows(bank_m: f64, culvert_max_m: f64) -> Vec<Reach> {
    let row = |name: &str, terms: &[(&str, f64)]| Reach {
        name: name.into(),
        owner: "faces::culverts",
        chain: true,
        terms: terms.iter().map(|&(what, m)| (what.to_string(), m)).collect(),
    };
    vec![
        row(
            "culvert zone",
            &[
                ("ZONE_READ_M: its way, water and terrain", ZONE_READ_M),
                ("ZONE_ALONG_M and the water's band", ZONE_ALONG_M + HALF_MAX_M + bank_m),
                ("FINAL_M", FINAL_M),
            ],
        ),
        row(
            "open culvert",
            &[("the crossing on its chain", culvert_max_m), ("its class", culvert_max_m), ("FINAL_M", FINAL_M)],
        ),
    ]
}

/// The zones of the culverts whose crossing lies in `bx` (cell units), over the lines of the faces.
pub(super) fn candidates(ctx: &Ctx, lines: &[WayLine], bx: [i32; 4]) -> Vec<Cand> {
    let g = ctx.graph;
    let of_way: BTreeMap<u32, usize> = lines.iter().enumerate().map(|(i, l)| (l.way, i)).collect();
    let mut out = Vec::new();
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    let at_grade = |c: &&Crossing| matches!(c.kind, CrossingKind::Culvert | CrossingKind::Ford);
    for c in g.crossings.iter().filter(at_grade).filter(|c| near(c.pos)) {
        let (Some(w), Some(s)) = (g.way_at(&c.over, Level::Ground), g.way_at(&c.under, Level::Ground)) else {
            continue;
        };
        let (Some(&lw), Some(&ls)) = (of_way.get(&w), of_way.get(&s)) else { continue };
        if !matches!(g.ways[w as usize].family, Family::Road | Family::Rail) || !lines[ls].sec.water {
            continue;
        }
        let (way, water) = (&lines[lw].sec, &lines[ls].sec);
        let (ws, ss) = (g.sample_near(w, c.pos), g.sample_near(s, c.pos));
        // The fill past the verge down to the mouth's height over the water (the bank's steepest cut, which
        // holds a way's fill, at the half-width), and at least as far as the fill falls by that height: the mouth
        // stands at the foot of the way's bank, never at its edge.
        let mouth = water.cut_max * water.half_m;
        let over = g.samples[ws as usize].z_m - g.samples[ss as usize].z_m;
        let past = ((over - mouth).max(mouth) / way.fill.max(1e-3)).clamp(0.0, way.reach_m);
        let bank = water.half_m + heights::cone_reach(water.cut, water.rise_m, &ctx.rules.plan.heights);
        let band_s = water_band(ctx, s, ss, bank);
        let band_w = band(ctx, lines, &of_way, w, ws, past, bounds(&band_s));
        let q2 = [2 * i64::from(c.pos[0]), 2 * i64::from(c.pos[1])];
        let found = shapes(&band_w, &band_s, OverlayRule::Intersect, c.pos).into_iter().find(|s| winding(s, q2) != 0);
        let Some(rings) = found else { continue };
        out.push(zone(ctx, rings, Some((w, way.rank as u8)), c.pos));
    }
    // Where open water goes on in its tunnel, a zone over the tunnel's start: the water's band out to its reach,
    // where its slopes have let go, and as long. The step there is the same mouth.
    for j in g.joints.iter().filter(|j| near(j.pos)) {
        let level = |k: usize| g.ways[j.arms[k].way as usize].level;
        let Some(k) = (0..2).find(|&k| level(k) == Level::Tunnel && level(1 - k) == Level::Ground) else { continue };
        let (tunnel, open) = (&j.arms[k], &j.arms[1 - k]);
        let Some(&l) = of_way.get(&open.way).filter(|&&l| lines[l].sec.water) else { continue };
        if g.ways[tunnel.way as usize].family != Family::Water {
            continue;
        }
        let sec = &lines[l].sec;
        let r = &g.ways[tunnel.way as usize].samples;
        let start = if tunnel.dir > 0 { r.start } else { r.end - 1 };
        let over = g.walk(tunnel.way, start, tunnel.dir, sec.reach_m, false).swap_remove(0);
        let p: Vec<P> = over.iter().map(|w| w.pos).collect();
        let h: Vec<f64> = p.iter().map(|q| (sec.half_m + sec.reach_m) / mpu(ctx, q.map(f64::from))).collect();
        let mut rings = Vec::new();
        stroke(&p, &h, [false, true], &mut rings);
        if !rings.is_empty() {
            out.push(zone(ctx, union(&rings, j.pos), None, j.pos));
        }
    }
    out
}

/// A zone's candidate: of a way (its line and class), or of a tunnel's start, named by its seed.
fn zone(ctx: &Ctx, rings: Rings, way: Option<(u32, u8)>, seed: P) -> Cand {
    let rank = ctx.rules.plan.rank(Rank::Zone);
    let owner = way.map_or(Owner::Terrain, |w| Owner::Way(w.0));
    let material = way.map(|w| w.0);
    let label = Label { owner, surface: Surface::Zone, rule: HeightRule::Slope, material, rank, ..TERRAIN };
    let g = [ctx.origin.x + i64::from(seed[0]), ctx.origin.y + i64::from(seed[1])];
    Cand {
        rings,
        label,
        key: (rank, way.map_or(u8::MAX, |w| w.1), 5),
        glob: Some([g[0], g[1], 0, 0, 0]),
        line: 0,
        relabel: None,
        beyond: Vec::new(),
        fill: None,
        discs: Vec::new(),
    }
}

/// Metres per frame unit at a point.
fn mpu(ctx: &Ctx, p: [f64; 2]) -> f64 {
    metres_per_unit(ctx.origin.y + libm::round(p[1]) as i64)
}

/// The band of the ways that a walk of `ZONE_READ_M` each way from sample `at` of `way` passes, out to their
/// verges' outer edge and `d` metres on: the strokes of the faces' own lines over the box `bx` (units) grown by
/// that offset, as rings that may overlap (the booleans that read them take their union, NonZero). A way of the
/// walk with no line of the faces (a deck or a tunnel) holds no ground of this level and adds nothing.
fn band(ctx: &Ctx, lines: &[WayLine], of_way: &BTreeMap<u32, usize>, way: u32, at: u32, d: f64, bx: [f64; 4]) -> Rings {
    let g = ctx.graph;
    let walks = [-1, 1].map(|dir| g.walk(way, at, dir, ZONE_READ_M, false).swap_remove(0));
    let mut ways: Vec<u32> = walks.iter().flatten().map(|p| p.way).collect();
    ways.sort_unstable();
    ways.dedup();
    let r = (HALF_MAX_M + d) / mpu(ctx, g.samples[at as usize].pos.map(f64::from));
    // A segment whose box meets the grown box: a long segment may cross it with both ends outside.
    let near = |a: P, b: P| {
        let (lo, hi) =
            ([a[0].min(b[0]), a[1].min(b[1])].map(f64::from), [a[0].max(b[0]), a[1].max(b[1])].map(f64::from));
        hi[0] >= bx[0] - r && lo[0] <= bx[2] + r && hi[1] >= bx[1] - r && lo[1] <= bx[3] + r
    };
    let mut out = Vec::new();
    for w in ways {
        let Some(&i) = of_way.get(&w) else { continue };
        let l = &lines[i];
        let n = l.p.len().saturating_sub(1);
        let mut k = 0;
        while k < n {
            let Some(from) = (k..n).find(|&i| near(l.p[i], l.p[i + 1])) else { break };
            let to = (from..n).find(|&i| !near(l.p[i], l.p[i + 1])).unwrap_or(n);
            let run = from.saturating_sub(1)..(to + 2).min(l.p.len());
            let off: Vec<f64> = run.clone().map(|i| l.bands[i][VERGE] + d / mpu(ctx, l.p[i].map(f64::from))).collect();
            if run.len() >= 2 {
                pieces_along(&l.p[run], &off, &[], [&[], &[]], [false, false], false, &mut out);
            }
            k = to;
        }
    }
    out
}

/// The water's band about sample `at` of water way `s`: the network within `ZONE_ALONG_M` along it, on through
/// its joints and into every arm of a junction, `bank` metres wide each side.
fn water_band(ctx: &Ctx, s: u32, at: u32, bank: f64) -> Rings {
    let g = ctx.graph;
    let mut lines: Vec<(Vec<WalkPoint>, [bool; 2])> = Vec::new();
    let mut main: Vec<WalkPoint> = Vec::new();
    for (k, d) in [-1i8, 1].into_iter().enumerate() {
        let mut walks = g.walk(s, at, d, ZONE_ALONG_M, true);
        let first = walks.remove(0);
        if k == 0 {
            main = first.into_iter().rev().collect();
        } else {
            main.extend(first.into_iter().skip(1));
        }
        lines.extend(walks.into_iter().map(|w| (w, [true, false])));
    }
    lines.push((main, [false, false]));
    let mut out = Vec::new();
    for (line, round) in lines {
        let p: Vec<P> = line.iter().map(|w| w.pos).collect();
        let h: Vec<f64> = line.iter().map(|w| bank / mpu(ctx, w.pos.map(f64::from))).collect();
        stroke(&p, &h, round, &mut out);
    }
    out
}

/// The box of rings (units).
fn bounds(rings: &Rings) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in rings.iter().flatten().map(|p| p.map(f64::from)) {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    b
}

/// The stroke of a polyline (units) with a half-width per point, square ends or round ones, as the band layers
/// are stroked, into `out`.
fn stroke(p: &[P], h: &[f64], round: [bool; 2], out: &mut Rings) {
    let mut keep: Vec<usize> = Vec::with_capacity(p.len());
    for k in 0..p.len() {
        if keep.last().is_none_or(|&j| p[j] != p[k]) {
            keep.push(k);
        }
    }
    let (p, h): (Vec<P>, Vec<f64>) = keep.iter().map(|&k| (p[k], h[k])).unzip();
    if p.len() >= 2 {
        pieces_along(&p, &h, &[], [&[], &[]], round, false, out);
    }
}
