//! Where daylight comes into the bores and how far it has come: the shortest way from each point of a bore way to an
//! opening, through the bore ways alone.

use super::deck::Decks;
use super::in_bore;
use crate::graph::{Arm, WayGraph};

/// The points of the ways `ways` where daylight comes into the bores, as (way, its metres along there, the metres from
/// there to the nearest opening through the bore ways), sorted. A node (a joint or a junction) where a bore way ends
/// open, as the shell leaves it (`Decks::open`: a way there goes on at another storey, on the ground or on a deck), is an
/// opening (0 m); a node that a bore way runs through names no end of it (`WayGraph::at_end`). A node where bore ways
/// meet passes the light on. The light at a point of a way is the least over its way's entries of the metres along
/// between the two plus the entry's metres: so it comes round a bend and through a junction, and never through the rock
/// to another bore. A point that no opening reaches (behind the body's end, between two closed ends) has none. A way of
/// `ways` that is no bore lies in front of a portal line (a floor under a way on top, `Storeys::wedges`): its one entry
/// holds the negative of its length, so all of it lies in daylight, and the bore's floor fades from the line in.
pub(crate) fn daylight(g: &WayGraph, decks: &Decks, ways: &[u32]) -> Vec<(u32, f64, f64)> {
    let nodes: Vec<([i32; 2], &[Arm])> = (g.joints.iter().map(|j| (j.pos, &j.arms[..])))
        .chain(g.junctions.iter().map(|j| (j.pos, &j.arms[..])))
        .collect();
    // Each bore arm of a node: its way, its metres along at the node, its node.
    let mut points: Vec<(u32, f64, usize)> = Vec::new();
    let mut open = vec![false; nodes.len()];
    for (k, &(pos, arms)) in nodes.iter().enumerate() {
        let bores = arms.iter().filter(|a| in_bore(g, a.way));
        open[k] = bores.clone().any(|a| g.at_end(a) && decks.open[a.way as usize][usize::from(a.dir < 0)]);
        points.extend(bores.map(|a| (a.way, along(g, a.way, pos), k)));
    }
    points.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut d: Vec<f64> = points.iter().map(|p| if open[p.2] { 0.0 } else { f64::INFINITY }).collect();
    // Relax along each way (its points in order, both ways) and across each node until nothing changes: a round passes
    // the light through at least one more node.
    let steps: Vec<(usize, usize)> =
        (1..points.len()).map(|i| (i - 1, i)).chain((1..points.len()).rev().map(|i| (i, i - 1))).collect();
    for _ in 0..=nodes.len() {
        let mut changed = false;
        for &(a, b) in &steps {
            let v = d[a] + (points[b].1 - points[a].1).abs();
            if points[a].0 == points[b].0 && v < d[b] {
                d[b] = v;
                changed = true;
            }
        }
        let mut least = vec![f64::INFINITY; nodes.len()];
        for (p, &v) in points.iter().zip(&d) {
            least[p.2] = least[p.2].min(v);
        }
        for (p, v) in points.iter().zip(&mut d) {
            if least[p.2] < *v {
                *v = least[p.2];
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out: Vec<(u32, f64, f64)> = (points.iter().zip(&d))
        .filter(|(p, v)| v.is_finite() && ways.contains(&p.0))
        .map(|(p, &v)| (p.0, p.1, v))
        .collect();
    for &w in ways.iter().filter(|&&w| !in_bore(g, w)) {
        let r = &g.ways[w as usize].samples;
        let along = g.samples[r.start as usize..r.end as usize].iter().map(|s| s.along_m);
        let (lo, hi) = along.fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), m| (l.min(m), h.max(m)));
        if lo <= hi {
            out.push((w, lo, lo - hi));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    out.dedup();
    out
}

/// The metres along way w at its point nearest p (frame units), as the records' samples hold them.
fn along(g: &WayGraph, w: u32, p: [i32; 2]) -> f64 {
    let r = &g.ways[w as usize].samples;
    let s = &g.samples[r.start as usize..r.end as usize];
    let f = |v: [i32; 2]| [f64::from(v[0]), f64::from(v[1])];
    let q = f(p);
    let mut best = (f64::INFINITY, s.first().map_or(0.0, |x| x.along_m));
    for k in 0..s.len().saturating_sub(1) {
        let (a, b) = (f(s[k].pos), f(s[k + 1].pos));
        let e = [b[0] - a[0], b[1] - a[1]];
        let l2 = e[0] * e[0] + e[1] * e[1];
        let t = if l2 > 0.0 { (((q[0] - a[0]) * e[0] + (q[1] - a[1]) * e[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let dist = libm::hypot(q[0] - a[0] - t * e[0], q[1] - a[1] - t * e[1]);
        if dist < best.0 {
            best = (dist, s[k].along_m + t * (s[k + 1].along_m - s[k].along_m));
        }
    }
    best.1
}
