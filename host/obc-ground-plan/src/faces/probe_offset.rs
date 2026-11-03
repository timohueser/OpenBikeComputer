//! PROBE (scratch branch only), round 2. General operations only:
//! 1. each way's centreline is stroked (round joins and caps);
//! 2. at each graph junction, the union of its arms' strokes within a disc of `extent_m` is closed
//!    with the junction's corner radius (the pair rule's); the closing adds area only there;
//! 3. the junction face is every point within D_j of the junction that its arms' strokes or its
//!    closing cover, D_j = the farthest closing point + 0.5 m (so the corner's cusps lie inside);
//! 4. elsewhere the least-key way that covers a point owns it (ways that do not meet in the graph);
//! 5. shoulder and slope bands are outward outlines of the union of strokes and closings.

use std::collections::BTreeMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::outline::offset::OutlineOffset;
use i_overlay::mesh::stroke::offset::StrokeOffset;
use i_overlay::mesh::style::{LineCap, LineJoin, OutlineStyle, StrokeStyle};

use super::bands::{layer_label, way_key, WaySec};
use super::cands::{Cands, Key, Label, KIND_JUNCTION};
use super::geom::P;
use super::rules::Rank;
use super::types::{HeightRule, Owner, Surface};
use super::Ctx;
use crate::frame::metres_per_unit;

const SHOULDER_M: f64 = 0.4;
const REACH_M: f64 = 10.0;
const STEP: f64 = 10.0 * core::f64::consts::PI / 180.0;
/// Junction lookup buckets: 2^13 frame units, about 13 m.
const BUCKET: u32 = 13;

type Shapes = Vec<Vec<Vec<[f64; 2]>>>;

pub struct Junction {
    pub id: u32,
    pub p: [f64; 2],
    pub d: f64,
    pub ways: Vec<u32>,
    pub closing: Option<u32>,
}

pub struct Probe {
    pub strokes: BTreeMap<u32, u32>,
    pub junctions: Vec<Junction>,
    buckets: BTreeMap<(i32, i32), Vec<usize>>,
    pub shoulder: u32,
    pub slope: u32,
}

fn rings(shapes: &Shapes) -> Vec<Vec<P>> {
    shapes
        .iter()
        .flatten()
        .map(|c| c.iter().map(|p| [p[0].round() as i32, p[1].round() as i32]).collect())
        .filter(|r: &Vec<P>| r.len() >= 3)
        .collect()
}

fn outline(shapes: &Shapes, d: f64) -> Shapes {
    if shapes.is_empty() {
        return Vec::new();
    }
    shapes.outline(&OutlineStyle::new(d).line_join(LineJoin::Round(STEP)))
}

fn op(a: &Shapes, b: &Shapes, rule: OverlayRule) -> Shapes {
    if a.is_empty() {
        return Vec::new();
    }
    a.overlay(b, rule, FillRule::NonZero)
}

fn disc(c: [f64; 2], r: f64) -> Shapes {
    let n = 72;
    vec![vec![(0..n).map(|k| {
        let a = 2.0 * core::f64::consts::PI * k as f64 / n as f64;
        [c[0] + r * a.cos(), c[1] + r * a.sin()]
    }).collect()]]
}

pub fn candidates(ctx: &Ctx, secs: &[Option<WaySec>], cands: &mut Cands) -> Probe {
    let mpu = metres_per_unit(ctx.origin.y);
    let plan = &ctx.rules.plan;
    let [x0, y0, x1, y1] = ctx.window;
    let m = (30.0 / mpu) as i32;
    let near = |p: P| p[0] > x0 - m && p[0] < x1 + m && p[1] > y0 - m && p[1] < y1 + m;
    let mut strokes = BTreeMap::new();
    let mut shape_of: BTreeMap<u32, Shapes> = BTreeMap::new();
    for w in secs.iter().flatten() {
        // Scope: only the samples near the window.
        let path: Vec<[f64; 2]> = w.s.iter().filter(|s| near(s.c)).map(|s| [f64::from(s.c[0]), f64::from(s.c[1])]).collect();
        if path.len() < 2 {
            continue;
        }
        let half = (w.s.iter().map(|s| s.o[0][0] + s.o[1][0]).sum::<f64>() / w.s.len() as f64) / 2.0 / mpu;
        let style = StrokeStyle::new(2.0 * half)
            .line_join(LineJoin::Round(STEP))
            .start_cap(LineCap::Round(STEP))
            .end_cap(LineCap::Round(STEP));
        let shape: Shapes = path.stroke(style, false);
        let (label, rank) = layer_label(ctx, w, 2);
        let key = way_key(ctx, w, rank, w.s[0].gid, 2);
        let cand = cands.add(label, key);
        let rs = rings(&shape);
        if !rs.is_empty() {
            cands.rings(cand, key, rs);
        }
        strokes.insert(w.way, cand);
        shape_of.insert(w.way, shape);
    }
    let mut all: Shapes = shape_of.values().flatten().cloned().collect();
    let mut junctions = Vec::new();
    for (id, j) in ctx.graph.junctions.iter().enumerate() {
        let mut ways: Vec<u32> = j.arms.iter().map(|a| a.way).filter(|w| shape_of.contains_key(w)).collect();
        ways.sort();
        ways.dedup();
        if ways.len() < 2 || !near(j.pos) {
            continue;
        }
        let p = [f64::from(j.pos[0]), f64::from(j.pos[1])];
        let group = |w: u32| secs[w as usize].as_ref().map(|s| s.group.clone()).unwrap_or_default();
        let host = j.arms.iter().filter(|a| a.host).map(|a| a.way).find(|w| shape_of.contains_key(w)).unwrap_or(ways[0]);
        let r = (ways.iter())
            .filter(|&&w| w != host)
            .map(|&w| plan.pair(&group(host), &group(w)).radius_m)
            .fold(0.0, f64::max)
            / mpu;
        let mut closing = None;
        let mut d = (ways.iter())
            .filter_map(|w| secs[*w as usize].as_ref())
            .map(|s| s.half_m)
            .fold(0.0, f64::max)
            / mpu
            + 1.0 / mpu;
        if r > 0.0 {
            let arms: Shapes = ways.iter().flat_map(|w| shape_of[w].clone()).collect();
            let local = op(&op(&arms, &Vec::new(), OverlayRule::Subject), &disc(p, plan.extent_m / mpu), OverlayRule::Intersect);
            let closed = outline(&outline(&local, r), -r);
            let added = op(&closed, &local, OverlayRule::Difference);
            if !added.is_empty() {
                let far = added.iter().flatten().flatten().map(|q| ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt()).fold(0.0, f64::max);
                d = d.max(far + 0.5 / mpu);
                let label = Label {
                    owner: Owner::Junction(id as u32),
                    surface: Surface::Carriageway,
                    rule: HeightRule::Blend,
                    material: None,
                    crossing: None,
                    rank: plan.rank(Rank::Junction),
                    reach_m: 10.0,
                };
                let key = Key { rank: plan.rank(Rank::Junction), class: 0, kind: KIND_JUNCTION, gid: [i64::from(j.pos[0]), i64::from(j.pos[1]), 0, 0, 0], number: 0 };
                let cand = cands.add(label, key);
                let rs = rings(&added);
                if !rs.is_empty() {
                    cands.rings(cand, key, rs);
                }
                closing = Some(cand);
                all.extend(added);
            }
        }
        // The junction face's edge: the circle of radius d about the junction, as a string.
        let circle = rings(&disc(p, d));
        let label = Label { owner: Owner::Terrain, surface: Surface::Terrain, rule: HeightRule::Terrain, material: None, crossing: None, rank: u8::MAX, reach_m: 0.0 };
        let cand = cands.add(label, Key::TERRAIN);
        cands.cands[cand as usize].minus = circle;
        junctions.push(Junction { id: id as u32, p, d, ways, closing });
    }
    let union = op(&all, &Vec::new(), OverlayRule::Subject);
    let shoulder_s = outline(&union, SHOULDER_M / mpu);
    let slope_s = outline(&shoulder_s, REACH_M / mpu);
    let mut add = |shapes: &Shapes, surface: Surface, rule: HeightRule, rank: Rank| {
        let label = Label { owner: Owner::Terrain, surface, rule, material: None, crossing: None, rank: plan.rank(rank), reach_m: 0.0 };
        let key = Key { rank: plan.rank(rank), class: u8::MAX, kind: 9, gid: [0; 5], number: 0 };
        let cand = cands.add(label, key);
        for shape in shapes {
            let rs = rings(&vec![shape.clone()]);
            if !rs.is_empty() {
                cands.rings(cand, key, rs);
            }
        }
        cand
    };
    let shoulder = add(&shoulder_s, Surface::Shoulder, HeightRule::Owner, Rank::Shoulder);
    let slope = add(&slope_s, Surface::Slope, HeightRule::Slope, Rank::Slope);
    let mut buckets: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
    for (i, j) in junctions.iter().enumerate() {
        let (lo, hi) = (((j.p[0] - j.d) as i32) >> BUCKET, ((j.p[0] + j.d) as i32) >> BUCKET);
        let (blo, bhi) = (((j.p[1] - j.d) as i32) >> BUCKET, ((j.p[1] + j.d) as i32) >> BUCKET);
        for x in lo..=hi {
            for y in blo..=bhi {
                buckets.entry((x, y)).or_default().push(i);
            }
        }
    }
    Probe { strokes, junctions, buckets, shoulder, slope }
}

/// The ownership rule at doubled point q.
pub fn label(ctx: &Ctx, cands: &mut Cands, probe: &Probe, q: [i64; 2]) -> u32 {
    let covering = cands.covering(q);
    let pt = [q[0] as f64 / 2.0, q[1] as f64 / 2.0];
    let ways: Vec<u32> = probe.strokes.iter().filter(|(_, c)| covering.contains(c)).map(|(w, _)| *w).collect();
    let key = ((pt[0] as i32) >> BUCKET, (pt[1] as i32) >> BUCKET);
    let mut best: Option<(f64, usize)> = None;
    for &i in probe.buckets.get(&key).map_or(&[][..], |v| v.as_slice()) {
        let j = &probe.junctions[i];
        let dist = ((pt[0] - j.p[0]).powi(2) + (pt[1] - j.p[1]).powi(2)).sqrt();
        let held = ways.iter().any(|w| j.ways.contains(w)) || j.closing.is_some_and(|c| covering.contains(&c));
        if dist <= j.d && held && best.is_none_or(|b| dist < b.0) {
            best = Some((dist, i));
        }
    }
    if let Some((_, i)) = best {
        let j = &probe.junctions[i];
        let plan = &ctx.rules.plan;
        let pos = ctx.graph.junctions[j.id as usize].pos;
        let label = Label {
            owner: Owner::Junction(j.id),
            surface: Surface::Carriageway,
            rule: HeightRule::Blend,
            material: None,
            crossing: None,
            rank: plan.rank(Rank::Junction),
            reach_m: 10.0,
        };
        let key = Key { rank: plan.rank(Rank::Junction), class: 0, kind: KIND_JUNCTION, gid: [i64::from(pos[0]), i64::from(pos[1]), 0, 0, 0], number: 0 };
        return cands.labels.id(label, key);
    }
    if let Some(c) = ways.iter().map(|w| probe.strokes[w]).min_by_key(|c| cands.labels.keys[cands.cands[*c as usize].label as usize]) {
        return cands.cands[c as usize].label;
    }
    for c in [probe.shoulder, probe.slope] {
        if covering.contains(&c) {
            return cands.cands[c as usize].label;
        }
    }
    super::cands::TERRAIN
}
