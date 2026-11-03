//! The checks of a face plan: the planar partition (check 1) and the cells' shared borders (check 2).

use std::collections::BTreeMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{ContourDirection, IntOverlayOptions, Overlay};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;

use super::geom::{area2, cross, P};
use super::types::{FacePlan, Owner, OUTSIDE};
use crate::frame::CELL_UNITS;
use crate::graph::WayGraph;

/// A broken invariant.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub check: &'static str,
    pub what: String,
}

fn ring_points(plan: &FacePlan, ring: &[u32]) -> Vec<P> {
    ring.iter().map(|&v| plan.vertices[v as usize]).collect()
}

/// Whether segments ab and cd meet anywhere but at a shared end.
fn meet(a: P, b: P, c: P, d: P) -> bool {
    let (d1, d2, d3, d4) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
    if d1.signum() * d2.signum() < 0 && d3.signum() * d4.signum() < 0 {
        return true;
    }
    let on = |p: P, q: P, x: P| {
        cross(p, q, x) == 0
            && x[0] >= p[0].min(q[0])
            && x[0] <= p[0].max(q[0])
            && x[1] >= p[1].min(q[1])
            && x[1] <= p[1].max(q[1])
    };
    let shared = |x: P| x == c || x == d;
    (on(c, d, a) && !shared(a))
        || (on(c, d, b) && !shared(b))
        || (on(a, b, c) && c != a && c != b)
        || (on(a, b, d) && d != a && d != b)
}

/// Whether a ring is simple: no repeated vertex and no two segments that meet but at a shared end.
fn simple(ring: &[P]) -> bool {
    let n = ring.len();
    let mut sorted = ring.to_vec();
    sorted.sort();
    if sorted.windows(2).any(|w| w[0] == w[1]) {
        return false;
    }
    // Segments by the grid cells of their boxes; each pair meets once in some cell.
    const SHIFT: u32 = 10;
    let mut cells: Vec<(i64, usize)> = Vec::new();
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        for x in (a[0].min(b[0]) >> SHIFT)..=(a[0].max(b[0]) >> SHIFT) {
            for y in (a[1].min(b[1]) >> SHIFT)..=(a[1].max(b[1]) >> SHIFT) {
                cells.push(((i64::from(x) << 32) ^ i64::from(y as u32), i));
            }
        }
    }
    cells.sort_unstable();
    for group in cells.chunk_by(|x, y| x.0 == y.0) {
        for (k, &(_, i)) in group.iter().enumerate() {
            for &(_, j) in &group[k + 1..] {
                if (i + 1) % n == j || (j + 1) % n == i {
                    continue;
                }
                if meet(ring[i], ring[(i + 1) % n], ring[j], ring[(j + 1) % n]) {
                    return false;
                }
            }
        }
    }
    true
}

/// Check 1: the faces tile the window exactly, every ring is simple and rightly turned, every ring
/// segment is an edge with the face on its side, and no pinch is left.
pub fn partition(plan: &FacePlan, window: [i32; 4]) -> Vec<Fault> {
    let mut faults = Vec::new();
    let mut fault = |what: String| faults.push(Fault { check: "1", what });
    let [x0, y0, x1, y1] = window;
    let want = 2 * i128::from(x1 - x0) * i128::from(y1 - y0);
    let mut sum = 0i128;
    let mut edges: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for e in &plan.edges {
        if edges.insert((e.a, e.b), (e.left, e.right)).is_some() || e.a >= e.b || e.left == e.right {
            fault(format!("edge {}-{}: twice, unordered or one face on both sides", e.a, e.b));
        }
    }
    let mut seen: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for f in &plan.faces {
        for (k, ring) in f.rings.iter().enumerate() {
            let pts = ring_points(plan, ring);
            let a = area2(&pts);
            sum += a;
            if (k == 0) != (a > 0) || a == 0 {
                fault(format!("face {} ring {k}: area {a}", f.id));
            }
            if !simple(&pts) {
                fault(format!("face {} ring {k}: not simple", f.id));
            }
            for i in 0..ring.len() {
                let (u, v) = (ring[i], ring[(i + 1) % ring.len()]);
                let side = match edges.get(&(u.min(v), u.max(v))) {
                    Some(&(left, right)) => {
                        if u < v {
                            left
                        } else {
                            right
                        }
                    }
                    None => OUTSIDE,
                };
                if side != f.id {
                    fault(format!("face {} ring {k}: segment {u}-{v} is no edge with the face on its side", f.id));
                }
                *seen.entry((u.min(v), u.max(v))).or_default() += 1;
            }
        }
    }
    for (&(a, b), &(left, right)) in &edges {
        let want = if left == OUTSIDE || right == OUTSIDE { 1 } else { 2 };
        if seen.get(&(a, b)).copied().unwrap_or(0) != want {
            fault(format!("edge {a}-{b}: in {:?} rings, not {want}", seen.get(&(a, b))));
        }
    }
    if sum != want {
        fault(format!("the faces' areas sum to {sum}, the window's is {want}"));
    }
    let contours: Vec<Vec<IntPoint>> = plan
        .faces
        .iter()
        .flat_map(|f| &f.rings)
        .map(|r| ring_points(plan, r).into_iter().map(|p| IntPoint::new(p[0], p[1])).collect())
        .collect();
    let options = IntOverlayOptions {
        preserve_input_collinear: true,
        output_direction: ContourDirection::CounterClockwise,
        preserve_output_collinear: true,
        min_output_area: 0,
    };
    let solver = Solver { strategy: Strategy::Frag, precision: Precision::ABSOLUTE, multithreading: None };
    let union =
        Overlay::with_contours_custom(&contours, &[], options, solver).overlay(OverlayRule::Subject, FillRule::NonZero);
    let union: i128 = union.iter().flatten().map(|c| area2(&c.iter().map(|p| [p.x, p.y]).collect::<Vec<P>>())).sum();
    if union != want {
        fault(format!("the union of the faces has area {union}, the window {want}"));
    }
    if plan.stats.pinches_left > 0 {
        fault(format!("{} pinches left", plan.stats.pinches_left));
    }
    faults
}

/// A global name of an owner near a point: a way by its class and its sample nearest the point (two
/// cells number ways apart), a junction by its position.
pub fn owner_name(graph: &WayGraph, owner: Owner, near: [i64; 2]) -> String {
    let o = graph.cell.origin();
    let way = |w: u32| {
        let way = &graph.ways[w as usize];
        let samples = &graph.samples[way.samples.start as usize..way.samples.end as usize];
        let g = |p: [i32; 2]| [o.x + i64::from(p[0]), o.y + i64::from(p[1])];
        let d = |p: [i64; 2]| (p[0] - near[0]).pow(2) + (p[1] - near[1]).pow(2);
        let best = samples.iter().map(|s| g(s.pos)).min_by_key(|&p| (d(p), p));
        format!("way {} {:?}", way.class, best)
    };
    match owner {
        Owner::Terrain => "terrain".into(),
        Owner::Way(w) => way(w),
        Owner::Junction(j) => {
            let p = graph.junctions[j as usize].pos;
            format!("junction {:?}", [o.x + i64::from(p[0]), o.y + i64::from(p[1])])
        }
        Owner::Bend(w, dm) => format!("bend {} {dm}", way(w)),
        Owner::Platform(f) => format!("platform {f}"),
    }
}

/// The border of a plan on one side of its cell (0 west, 1 north, 2 east, 3 south): its vertices in
/// global positions along the side, and per border segment the attributes of the face inside.
fn border(plan: &FacePlan, graph: &WayGraph, side: usize) -> (Vec<[i64; 2]>, Vec<(String, String)>) {
    let c = CELL_UNITS as i32;
    let o = plan.cell.origin();
    let on = |p: P| match side {
        0 => p[0] == 0 && (0..=c).contains(&p[1]),
        1 => p[1] == 0 && (0..=c).contains(&p[0]),
        2 => p[0] == c && (0..=c).contains(&p[1]),
        _ => p[1] == c && (0..=c).contains(&p[0]),
    };
    let along = |p: P| if side % 2 == 0 { p[1] } else { p[0] };
    let mut verts: Vec<(i32, u32)> =
        (plan.vertices.iter().enumerate()).filter(|(_, &p)| on(p)).map(|(i, &p)| (along(p), i as u32)).collect();
    verts.sort();
    let global = |p: P| [o.x + i64::from(p[0]), o.y + i64::from(p[1])];
    let mut by_pair: BTreeMap<(u32, u32), (u32, u32)> = BTreeMap::new();
    for e in &plan.edges {
        by_pair.insert((e.a, e.b), (e.left, e.right));
    }
    let mut attrs = Vec::new();
    for w in verts.windows(2) {
        let (u, v) = (w[0].1, w[1].1);
        let Some(&(left, right)) = by_pair.get(&(u.min(v), u.max(v))) else {
            attrs.push(("no edge".into(), String::new()));
            continue;
        };
        let mid = global(plan.vertices[u as usize]);
        let name = |f: u32| {
            if f == OUTSIDE {
                return "outside".to_string();
            }
            let f = &plan.faces[f as usize];
            let way = |x: Option<u32>| x.map(|w| owner_name(graph, Owner::Way(w), mid));
            format!(
                "{:?} {:?} {} {} {:?} {:?}",
                f.surface,
                f.rule,
                f.rank,
                owner_name(graph, f.owner, mid),
                way(f.material),
                way(f.crossing)
            )
        };
        let inside = |f: u32| f != OUTSIDE && plan.faces[f as usize].inside;
        let (i, x) = if inside(left) { (left, right) } else { (right, left) };
        attrs.push((name(i), name(x)));
    }
    (verts.iter().map(|&(_, i)| global(plan.vertices[i as usize])).collect(), attrs)
}

/// Check 2: cell `b` east of `a` (`east`) or south of it: the same vertices on their shared border,
/// and the same attributes of the faces on each side of each border segment.
pub fn neighbours(a: (&FacePlan, &WayGraph), b: (&FacePlan, &WayGraph), east: bool) -> Vec<Fault> {
    let (sa, sb) = if east { (2, 0) } else { (3, 1) };
    let (va, fa) = border(a.0, a.1, sa);
    let (vb, fb) = border(b.0, b.1, sb);
    let mut faults = Vec::new();
    if va != vb {
        let first = va.iter().zip(&vb).position(|(x, y)| x != y);
        faults.push(Fault {
            check: "2",
            what: format!("border vertices differ: {} and {}, first at {first:?}", va.len(), vb.len()),
        });
        return faults;
    }
    // The face inside one cell is the face outside the other.
    for (i, (x, y)) in fa.iter().zip(&fb).enumerate() {
        if x.0 != y.1 || x.1 != y.0 {
            faults.push(Fault { check: "2", what: format!("segment {i} at {:?}: {x:?} | {y:?}", va[i]) });
        }
    }
    faults
}
