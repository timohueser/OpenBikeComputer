//! Stage 2b: the slope region, every point within its face's reach of an outer edge (an edge between a
//! non-terrain face and the terrain), as the union of a rectangle per outer segment and kites at its
//! convex vertices.

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{ContourDirection, IntOverlayOptions, Overlay};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;

use super::cands::{Label, Labels, TERRAIN};
use super::geom::{area2, round, with_cell_lines, P};
use super::rules::{Rank, Rules};
use super::slice::{Map, NONE};
use super::types::{HeightRule, Owner, Surface};
use super::Ctx;
use crate::frame::metres_per_unit;

/// The buffer passes the reach by this many frame units, so the region holds every point within it.
const PAD: f64 = 2.0;

pub fn label(rules: &Rules) -> Label {
    Label {
        owner: Owner::Terrain,
        surface: Surface::Slope,
        rule: HeightRule::Slope,
        material: None,
        crossing: None,
        rank: rules.plan.rank(Rank::Slope),
        reach_m: 0.0,
    }
}

fn unit(d: [f64; 2]) -> [f64; 2] {
    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
    [d[0] / l, d[1] / l]
}

/// The kite at v between the outward normals n1 and n2 out to r, halved while it turns over 90°.
fn kite(v: P, n1: [f64; 2], n2: [f64; 2], r: f64, out: &mut Vec<Vec<P>>) {
    let cos = n1[0] * n2[0] + n1[1] * n2[1];
    let mid = unit([n1[0] + n2[0], n1[1] + n2[1]]);
    if cos < 0.0 {
        kite(v, n1, mid, r, out);
        kite(v, mid, n2, r, out);
        return;
    }
    let half = ((1.0 + cos) / 2.0).sqrt();
    let at = |n: [f64; 2], m: f64| [v[0] + round(n[0] * m) as i32, v[1] + round(n[1] * m) as i32];
    let mut ring = vec![v, at(n1, r), at(mid, r / half), at(n2, r)];
    if area2(&ring) < 0 {
        ring.reverse();
    }
    out.push(ring);
}

pub fn region(ctx: &Ctx, map: &Map, labels: &[u32], names: &Labels) -> Vec<Vec<P>> {
    let groups = map.groups(labels, true);
    let label = |g: u32| names.list[labels[g as usize] as usize];
    // An outer edge seen from the terrain: a terrain boundary edge with a face of reach across it.
    // The buffer lies on the terrain's side (the edge's left), and a kite fills each vertex where the
    // terrain's boundary turns right, whichever faces the two edges belong to.
    let reach = |t: u32| {
        let g = groups[map.half[t as usize].patch as usize];
        let across = map.across(&groups, t);
        if labels[g as usize] != TERRAIN || across == NONE {
            return None;
        }
        let l = label(across);
        let open = !matches!(l.surface, Surface::Terrain | Surface::Slope) && l.reach_m > 0.0;
        open.then_some(f64::from(l.reach_m))
    };
    let units = |m: f64, p: P| m / metres_per_unit(ctx.origin.y + i64::from(p[1])) + PAD;
    let mut buffers: Vec<Vec<P>> = Vec::new();
    for t in 0..map.half.len() as u32 {
        if !map.bounds(&groups, t) {
            continue;
        }
        let Some(m) = reach(t) else { continue };
        let e = map.half[t as usize];
        let (a, b) = (map.point(e.a), map.point(e.b));
        let r = units(m, a);
        let d = unit([f64::from(b[0] - a[0]), f64::from(b[1] - a[1])]);
        let n = [-d[1], d[0]];
        let at = |p: P| [p[0] + round(n[0] * r) as i32, p[1] + round(n[1] * r) as i32];
        // Positive, as every buffer: NonZero joins them where they overlap.
        buffers.push(vec![a, b, at(b), at(a)]);
        let next = map.next(&groups, t);
        if let Some(m2) = reach(next) {
            let c = map.point(map.half[next as usize].b);
            let d2 = unit([f64::from(c[0] - b[0]), f64::from(c[1] - b[1])]);
            if d[0] * d2[1] - d[1] * d2[0] < 0.0 {
                kite(b, n, [-d2[1], d2[0]], units(m.max(m2), b), &mut buffers);
            }
        }
    }
    if buffers.is_empty() {
        return Vec::new();
    }
    let contours: Vec<Vec<IntPoint>> =
        buffers.iter().map(|r| r.iter().map(|p| IntPoint::new(p[0], p[1])).collect()).collect();
    let options = IntOverlayOptions {
        preserve_input_collinear: true,
        output_direction: ContourDirection::CounterClockwise,
        preserve_output_collinear: true,
        min_output_area: 0,
    };
    let solver = Solver { strategy: Strategy::Frag, precision: Precision::ABSOLUTE, multithreading: None };
    let shapes =
        Overlay::with_contours_custom(&contours, &[], options, solver).overlay(OverlayRule::Subject, FillRule::NonZero);
    shapes
        .into_iter()
        .flatten()
        .map(|c| with_cell_lines(&c.into_iter().map(|p| [p.x, p.y]).collect::<Vec<P>>()))
        .collect()
}
