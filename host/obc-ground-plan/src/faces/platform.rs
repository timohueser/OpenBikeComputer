//! The platforms' candidates: an area's rings, or a line's stroke of half-width `width / 2`, else
//! `platform_half_m`. A platform beats the rail bed, so the heights can stand a wall at their edge.

use super::geom::area2;
use super::stroke::{pieces_along, union};
use super::{Cand, Ctx, Label, Rank};
use crate::faces::{FaceStats, HeightRule, Owner, Surface};
use crate::sections::leading_number;
use crate::tile::Geometry;
use crate::CellInput;

/// The candidates of the input's platforms that reach the window `bx`. A platform without vertex
/// heights is skipped: the heights have no level for it.
pub(super) fn candidates(ctx: &Ctx, input: &CellInput, bx: [i32; 4], stats: &mut FaceStats) -> Vec<Cand> {
    let plan = &ctx.rules.plan;
    let mut out = Vec::new();
    for (index, f) in input.features.iter().enumerate() {
        let area = match (f.kind.as_str(), f.geometry) {
            ("platform", Geometry::Area) => true,
            ("platform-line", Geometry::Line) => false,
            _ => continue,
        };
        let local = |p: &crate::frame::GlobalPoint| [(p.x - ctx.origin.x) as i32, (p.y - ctx.origin.y) as i32];
        let near = |p: &[i32; 2]| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
        let parts: Vec<Vec<[i32; 2]>> = f.parts.iter().map(|part| part.points.iter().map(local).collect()).collect();
        let (Some(first), true) = (f.parts.first(), parts.iter().flatten().any(near)) else { continue };
        if !f.parts.iter().all(|p| p.heights.as_ref().is_some_and(|h| h.iter().any(Option::is_some))) {
            stats.platforms_skipped += 1;
            continue;
        }
        let rings = if area {
            // The winding of a ring has no meaning in the tiles: an outer ring turns positive, a
            // hole negative, for the NonZero rule.
            let mut rings = parts.clone();
            for (ring, part) in rings.iter_mut().zip(&f.parts) {
                if (area2(ring) > 0) != part.outer {
                    ring.reverse();
                }
            }
            rings
        } else {
            let half_m = f.tags.get("width").and_then(|w| leading_number(w)).map_or(plan.platform_half_m, |w| w / 2.0);
            let mut pieces = Vec::new();
            for p in &parts {
                let d: Vec<f64> = p.iter().map(|&q| ctx.units(half_m, q)).collect();
                pieces_along(p, &d, &[], [&[], &[]], [false, false], false, &mut pieces);
            }
            pieces
        };
        let anchor = parts_anchor(&rings);
        let label = Label {
            owner: Owner::Platform(index as u32),
            surface: Surface::Platform,
            rule: HeightRule::Owner,
            material: None,
            crossing: None,
            rank: plan.rank(Rank::Platform),
            reach_m: plan.platform_reach_m as f32,
        };
        let p0 = first.points[0];
        out.push(Cand {
            rings: union(&rings, anchor),
            label,
            key: (plan.rank(Rank::Platform), 0, 3),
            glob: Some([i64::from(first.tile.0), i64::from(first.tile.1), i64::from(f.index), p0.x, p0.y]),
            line: 0,
            relabel: None,
            beyond: Vec::new(),
            fill: None,
            discs: Vec::new(),
        });
    }
    out
}

/// A vertex of the rings that every cell finds alike: the least.
fn parts_anchor(rings: &[Vec<[i32; 2]>]) -> [i32; 2] {
    rings.iter().flatten().copied().min().unwrap_or([0, 0])
}
