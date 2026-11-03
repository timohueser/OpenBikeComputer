//! Platforms: an area's rings, or a line's buffer on both sides. A platform without heights is
//! skipped. Rail beds are the carriageway layer of a rail class (`bands`).

use super::cands::{Cands, Key, Label, KIND_PLATFORM};
use super::geom::{area2, round, P};
use super::rules::Rank;
use super::types::{FaceStats, HeightRule, Owner, Surface};
use super::Ctx;
use crate::frame::metres_per_unit;
use crate::sections::leading_number;
use crate::tile::{Geometry, Part};

fn heights(part: &Part) -> bool {
    part.heights.as_ref().is_some_and(|h| !h.is_empty() && h.iter().all(Option::is_some))
}

pub fn platforms(ctx: &Ctx, cands: &mut Cands, stats: &mut FaceStats) {
    let plan = &ctx.rules.plan;
    let o = ctx.origin;
    let [wx0, wy0, wx1, wy1] = ctx.window;
    let near = |p: &P| p[0] > wx0 - 4096 && p[0] < wx1 + 4096 && p[1] > wy0 - 4096 && p[1] < wy1 + 4096;
    for (id, f) in ctx.input.features.iter().enumerate() {
        let line = match (f.kind.as_str(), f.geometry) {
            ("platform", Geometry::Area) => false,
            ("platform-line", Geometry::Line) => true,
            _ => continue,
        };
        let local =
            |part: &Part| -> Vec<P> { part.points.iter().map(|p| [(p.x - o.x) as i32, (p.y - o.y) as i32]).collect() };
        if !f.parts.iter().any(|p| local(p).iter().any(near)) {
            continue;
        }
        if !f.parts.iter().all(heights) {
            stats.platforms_skipped += 1;
            continue;
        }
        let least = f.parts.iter().flat_map(|p| &p.points).min().map_or([0; 2], |p| [p.x, p.y]);
        let rank = plan.rank(Rank::Platform);
        let label = Label {
            owner: Owner::Platform(id as u32),
            surface: Surface::Platform,
            rule: HeightRule::Owner,
            material: None,
            crossing: None,
            rank,
            reach_m: plan.platform_reach_m as f32,
        };
        let key = Key { rank, class: 0, kind: KIND_PLATFORM, gid: [least[0], least[1], 0, 0, 0], number: 0 };
        let cand = cands.add(label, key);
        if !line {
            // Each polygon: an outer ring and the holes after it, outer positive and holes negative.
            let mut rings: Vec<Vec<P>> = Vec::new();
            for part in &f.parts {
                let mut ring = local(part);
                if ring.len() > 1 && ring.first() == ring.last() {
                    ring.pop();
                }
                if ring.len() < 3 {
                    continue;
                }
                if part.outer && !rings.is_empty() {
                    cands.rings(cand, key, std::mem::take(&mut rings));
                }
                let a = area2(&ring);
                if (a > 0) != part.outer {
                    ring.reverse();
                }
                rings.push(ring);
            }
            if !rings.is_empty() {
                cands.rings(cand, key, rings);
            }
            continue;
        }
        let half = f
            .tags
            .get("width")
            .and_then(|w| leading_number(w))
            .filter(|w| *w > 0.0)
            .map_or(plan.platform_half_m, |w| w / 2.0);
        for part in &f.parts {
            let pts = &part.points;
            let mut prev: Option<[f64; 2]> = None;
            for i in 0..pts.len().saturating_sub(1) {
                let (a, b) = ([pts[i].x, pts[i].y], [pts[i + 1].x, pts[i + 1].y]);
                let d = [(b[0] - a[0]) as f64, (b[1] - a[1]) as f64];
                let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
                if len == 0.0 {
                    continue;
                }
                let u = half / metres_per_unit(a[1]);
                let n = [-d[1] / len * u, d[0] / len * u];
                let at = |p: [i64; 2], n: [f64; 2], s: f64| {
                    [(p[0] + round(s * n[0]) - o.x) as i32, (p[1] + round(s * n[1]) - o.y) as i32]
                };
                cands.ring(cand, key, vec![at(a, n, 1.0), at(a, n, -1.0), at(b, n, -1.0), at(b, n, 1.0)]);
                // The wedge between two segments' ends at an inner vertex, on both sides.
                if let Some(m) = prev {
                    let c = [(a[0] - o.x) as i32, (a[1] - o.y) as i32];
                    for s in [1.0, -1.0] {
                        cands.ring(cand, key, vec![c, at(a, m, s), at(a, n, s)]);
                    }
                }
                prev = Some(n);
            }
        }
    }
}
