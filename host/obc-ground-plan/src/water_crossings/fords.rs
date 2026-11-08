//! The wet faces of a crossing at grade: a ford, stepping stones and a way along the water, every crossing
//! of water and a way on the ground with no structure. A wet face is the water's own band, its half-width
//! and `ford_margin_m` on each side, where it crosses the way's band (carriageway and shoulders): the water
//! runs on across the way at its own surface. The way's dip under it is the feature bake's (`FORD_M`).

use i_overlay::core::overlay_rule::OverlayRule;

use super::{bounds, stroke, Axis, CrossingStats, Ctx, Wet, ZONE_ALONG_M};
use crate::faces::geom::{winding, P};
use crate::faces::stroke::shapes;
use crate::graph::{CrossingKind, Family, Level};

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
        match wet(ctx, c.pos, w, s) {
            Some(x) => {
                stats.wet += 1;
                out.push(x);
            }
            None => stats.ford_dry += 1,
        }
    }
    out
}

/// The wet face of way w over water s at their crossing pos; None where the way's lower edge there lies
/// over the water's level by `ford_dry_m`.
fn wet(ctx: &Ctx, pos: P, w: u32, s: u32) -> Option<Wet> {
    let g = ctx.graph;
    let (ws, ss) = (g.sample_near(w, pos), g.sample_near(s, pos));
    let (sec, water) = (ctx.way_section(w), ctx.water_section(s));
    // The crossing is a sample of both ways.
    let at = &g.samples[ws as usize];
    let low = at.z_m - at.bank.abs() * (sec.half_m + at.widen_m + sec.shoulder_m);
    if low >= g.samples[ss as usize].z_m + water.crossing.ford_dry_m {
        return None;
    }
    let (way, axis) = (Axis::through(g, w, ws, ZONE_ALONG_M), Axis::through(g, s, ss, ZONE_ALONG_M));
    if way.points.len() < 2 || axis.points.len() < 2 {
        return None;
    }
    let line: Vec<[f64; 2]> = (0..axis.points.len()).map(|k| axis.pos(k)).collect();
    let half = (water.half_m + water.crossing.ford_margin_m) / ctx.mpu(pos.map(f64::from));
    let band = stroke(&line, &vec![half; line.len()], [false, false]);
    let own = ctx.band(&way, pos, [0.0, 0.0], bounds(&band))?;
    // The part that holds the crossing.
    let at = [2 * i64::from(pos[0]), 2 * i64::from(pos[1])];
    let rings = shapes(&band, &own, OverlayRule::Intersect, pos).into_iter().find(|r| winding(r, at) != 0)?;
    Some(Wet { rings, way: w, water: s, seed: ctx.global(pos) })
}
