//! The checks of a face plan: the planar partition (check 1) and the cells' shared borders (check 2).

use std::collections::BTreeMap;

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay::{ContourDirection, IntOverlayOptions, Overlay};
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::core::solver::{Precision, Solver, Strategy};
use i_overlay::i_float::int::point::IntPoint;

use super::geom::{area2, closest, cross, winding, P};
use super::types::{Face, FacePlan, Owner, Surface, OUTSIDE};
use crate::frame::{metres_per_unit, CELL_UNITS};
use crate::graph::{Level, WayGraph};

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

/// Check 1: the faces tile the window exactly, every ring is simple and rightly turned, and every ring
/// segment is an edge with the face on its side.
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
        // Only the window's edge has no face beyond it.
        let (a, b) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
        let rim = (a[0] == b[0] && (a[0] == x0 || a[0] == x1)) || (a[1] == b[1] && (a[1] == y0 || a[1] == y1));
        if (e.left == OUTSIDE || e.right == OUTSIDE) && !rim {
            fault(format!("edge {a:?}-{b:?}: no face on one side inside the window"));
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
        ogc: false,
    };
    let solver = Solver { strategy: Strategy::Frag, precision: Precision::ABSOLUTE, multithreading: None };
    let union = Overlay::from_subj_and_clip_custom(&contours, &Vec::<Vec<IntPoint>>::new(), options, solver)
        .overlay(OverlayRule::Subject, FillRule::NonZero);
    let union: i128 = union.iter().flatten().map(|c| area2(&c.iter().map(|p| [p.x, p.y]).collect::<Vec<P>>())).sum();
    if union != want {
        fault(format!("the union of the faces has area {union}, the window {want}"));
    }
    faults
}

/// A face part narrower than this many units (about 6 mm) is a numerical leftover: no rule shapes
/// geometry that thin, except toward a taper's tip.
const SLIVER: f64 = 4.0;

/// A thin place is a taper's tip where the boundary joins its two sides within the walk of a wedge
/// this sharp: two ways that part at a shallow angle, a band that tapers out, a round that runs into
/// an edge.
const TAPER_DEG: f64 = 1.0;

/// An edge this short between two turns of opposite sense, each sharper than `JOG_DEG`, is a jog.
const JOG: f64 = 2.0;
const JOG_DEG: f64 = 60.0;

/// The kinds of geometry that no rule shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArtefactKind {
    /// Two places of a face's boundary closer than `SLIVER` that no taper joins.
    Sliver,
    /// A 1- or 2-unit step in an edge.
    Jog,
    /// A way's or junction's face that lies across the centreline of an unrelated way of its family
    /// and of an equal or a stronger class.
    Shard,
    /// A way's or junction's face away from the cell's border that holds none of its own ways'
    /// centrelines, and is small or borders no face that holds one.
    Fragment,
    /// A border between related carriageway faces (a junction and its arm, two junctions on a link,
    /// two ways that overlap) that is no straight seam. The owner accepts it in the plan: it is no
    /// fault of check 1.
    BentBorder,
}

/// One artefact of a face, at a point in cell units.
#[derive(Clone, Debug, PartialEq)]
pub struct Artefact {
    pub face: u32,
    pub at: [i32; 2],
    pub kind: ArtefactKind,
}

fn len(a: P, b: P) -> f64 {
    let (x, y) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    (x * x + y * y).sqrt()
}

/// Whether segments ab and cd cross at a point inside both.
fn crosses(a: P, b: P, c: P, d: P) -> bool {
    let (d1, d2, d3, d4) = (cross(a, b, c), cross(a, b, d), cross(c, d, a), cross(c, d, b));
    d1.signum() * d2.signum() < 0 && d3.signum() * d4.signum() < 0
}

/// The artefacts of a plan of level 0. Edges on the cell's border are no boundary: the face goes on
/// in the next cell.
pub fn artefacts(plan: &FacePlan, graph: &WayGraph) -> Vec<Artefact> {
    let c = CELL_UNITS as i32;
    let border = |a: P, b: P| (a[0] == b[0] && (a[0] == 0 || a[0] == c)) || (a[1] == b[1] && (a[1] == 0 || a[1] == c));
    let on_line = |p: P| p[0] == 0 || p[0] == c || p[1] == 0 || p[1] == c;
    let sin_taper = libm::sin(TAPER_DEG.to_radians() / 2.0);
    let sin_jog = libm::sin(JOG_DEG.to_radians());
    let mut out = Vec::new();
    for f in &plan.faces {
        let rings: Vec<Vec<P>> = f.rings.iter().map(|r| ring_points(plan, r)).collect();
        // Segments: (ring, index, start along the ring, ring length).
        let mut segs: Vec<(usize, usize, f64, f64)> = Vec::new();
        for (k, ring) in rings.iter().enumerate() {
            let n = ring.len();
            let total: f64 = (0..n).map(|i| len(ring[i], ring[(i + 1) % n])).sum();
            let mut s = 0.0;
            for i in 0..n {
                let (prev, a, b, next) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n], ring[(i + 2) % n]);
                let l = len(a, b);
                if !border(a, b) {
                    segs.push((k, i, s, total));
                    let turn = |o: P, p: P, q: P| cross(o, p, q) as f64 / (len(o, p) * len(p, q));
                    let (t0, t1) = (turn(prev, a, b), turn(a, b, next));
                    if l <= JOG && !on_line(a) && !on_line(b) && t0 * t1 < 0.0 && t0.abs().min(t1.abs()) > sin_jog {
                        out.push(Artefact { face: f.id, at: a, kind: ArtefactKind::Jog });
                    }
                }
                s += l;
            }
        }
        // Pairs by a grid of 64 units over the segments' boxes.
        let mut grid: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
        for (id, &(k, i, _, _)) in segs.iter().enumerate() {
            let (a, b) = (rings[k][i], rings[k][(i + 1) % rings[k].len()]);
            let g = |v: i32| v.div_euclid(64);
            let pad = SLIVER as i32;
            for x in g(a[0].min(b[0]) - pad)..=g(a[0].max(b[0]) + pad) {
                for y in g(a[1].min(b[1]) - pad)..=g(a[1].max(b[1]) + pad) {
                    grid.entry((x, y)).or_default().push(id);
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for list in grid.values() {
            for (x, &i) in list.iter().enumerate() {
                for &j in &list[x + 1..] {
                    if !seen.insert((i.min(j), i.max(j))) {
                        continue;
                    }
                    let (ki, ii, si, ti) = segs[i];
                    let (kj, ij, sj, _) = segs[j];
                    let (ri, rj) = (&rings[ki], &rings[kj]);
                    let (a, b, cc, d) = (ri[ii], ri[(ii + 1) % ri.len()], rj[ij], rj[(ij + 1) % rj.len()]);
                    let (dist, s, t) = closest(a, b, cc, d);
                    if dist >= SLIVER || dist == 0.0 {
                        continue;
                    }
                    if ki == kj {
                        let walk = (si + s * len(a, b) - (sj + t * len(cc, d))).abs();
                        if walk.min(ti - walk) <= dist / sin_taper {
                            continue;
                        }
                    }
                    // The middle of pq must lie in the face.
                    let p =
                        [f64::from(a[0]) + s * f64::from(b[0] - a[0]), f64::from(a[1]) + s * f64::from(b[1] - a[1])];
                    let q = [
                        f64::from(cc[0]) + t * f64::from(d[0] - cc[0]),
                        f64::from(cc[1]) + t * f64::from(d[1] - cc[1]),
                    ];
                    let mid = [libm::round(p[0] + q[0]) as i64, libm::round(p[1] + q[1]) as i64];
                    if winding(&rings, mid) == 0 {
                        continue;
                    }
                    out.push(Artefact { face: f.id, at: [p[0] as i32, p[1] as i32], kind: ArtefactKind::Sliver });
                }
            }
        }
    }
    out.extend(owner_artefacts(plan, graph));
    out.extend(bent_borders(plan, graph));
    out
}

/// The borders between a way's carriageway and the face of a point where it meets another way, or
/// two such faces, that are no straight seam: one per border chain, on the face of the weaker rank.
fn bent_borders(plan: &FacePlan, graph: &WayGraph) -> Vec<Artefact> {
    let carriageway = |f: u32| f != OUTSIDE && plan.faces[f as usize].surface == Surface::Carriageway;
    let ways: Vec<Vec<u32>> = plan.faces.iter().map(|f| own_ways(f, graph)).collect();
    let points =
        || graph.junctions.iter().map(|j| j.arms.as_slice()).chain(graph.joints.iter().map(|j| j.arms.as_slice()));
    let related = |f: u32, g: u32| {
        let (a, b) = (&ways[f as usize], &ways[g as usize]);
        a.iter().any(|w| b.contains(w))
            || points().any(|arms| arms.iter().any(|x| a.contains(&x.way)) && arms.iter().any(|x| b.contains(&x.way)))
    };
    let mut by_pair: BTreeMap<(u32, u32), Vec<(u32, u32)>> = BTreeMap::new();
    for e in plan.edges.iter().filter(|e| carriageway(e.left) && carriageway(e.right)) {
        by_pair.entry((e.left.min(e.right), e.left.max(e.right))).or_default().push((e.a, e.b));
    }
    let mut out = Vec::new();
    for ((f, g), mut edges) in by_pair {
        if !related(f, g) {
            continue;
        }
        // Each chain of edges is one border.
        while let Some(first) = edges.pop() {
            let mut chain = vec![first.0, first.1];
            let mut grew = true;
            while grew {
                grew = false;
                edges.retain(|&(a, b)| {
                    let joins = chain.contains(&a) || chain.contains(&b);
                    if joins {
                        chain.extend([a, b]);
                        grew = true;
                    }
                    !joins
                });
            }
            let pts: Vec<P> = chain.iter().map(|&v| plan.vertices[v as usize]).collect();
            let d2 = |p: P, q: P| (i64::from(q[0] - p[0])).pow(2) + (i64::from(q[1] - p[1])).pow(2);
            // The chord: the vertex farthest from the first, and the vertex farthest from that one.
            let a = *pts.iter().max_by_key(|&&q| d2(pts[0], q)).unwrap();
            let b = *pts.iter().max_by_key(|&&q| d2(a, q)).unwrap();
            let chord = len(a, b);
            if chord == 0.0 {
                continue;
            }
            let at = *pts.iter().max_by_key(|&&q| cross(a, b, q).unsigned_abs()).unwrap();
            // Within the plan's precision a seam's vertices lie up to a grid step off its line.
            if cross(a, b, at).unsigned_abs() as f64 / chord > f64::from(2 * super::slice::PRECISION) {
                let weak = if plan.faces[f as usize].rank >= plan.faces[g as usize].rank { f } else { g };
                out.push(Artefact { face: weak, at, kind: ArtefactKind::BentBorder });
            }
        }
    }
    out
}

/// A face of a way or a junction under this area that holds none of its ways' centrelines is a
/// fragment wherever it lies: no rule leaves so small a piece of a way.
const FRAGMENT_M2: f64 = 0.01;

/// The ways of a way's or a junction's face. A joint's face takes the label of one of its two ways
/// and covers both.
fn own_ways(f: &Face, graph: &WayGraph) -> Vec<u32> {
    match f.owner {
        Owner::Terrain | Owner::Platform(_) => Vec::new(),
        Owner::Way(w) => {
            let joined = graph.joints.iter().filter(|j| j.arms.iter().any(|a| a.way == w));
            let mut ways: Vec<u32> =
                std::iter::once(w).chain(joined.flat_map(|j| j.arms.iter().map(|a| a.way))).collect();
            ways.sort_unstable();
            ways.dedup();
            ways
        }
        Owner::Junction(j) => graph.junctions[j as usize].arms.iter().map(|a| a.way).collect(),
    }
}

/// Where the centreline of way `w` enters the rings: a sample inside them or a segment across an
/// edge.
fn enters(rings: &[Vec<P>], graph: &WayGraph, w: u32) -> Option<P> {
    let bbox = rings[0].iter().fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, p| {
        [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
    });
    let off = |a: P, b: P| {
        a[0].max(b[0]) < bbox[0] || a[0].min(b[0]) > bbox[2] || a[1].max(b[1]) < bbox[1] || a[1].min(b[1]) > bbox[3]
    };
    let way = &graph.ways[w as usize];
    let line = &graph.samples[way.samples.start as usize..way.samples.end as usize];
    for k in 0..line.len() {
        let a = line[k].pos;
        if !off(a, a) && winding(rings, [2 * i64::from(a[0]), 2 * i64::from(a[1])]) != 0 {
            return Some(a);
        }
        let Some(b) = line.get(k + 1).map(|s| s.pos) else { break };
        if off(a, b) {
            continue;
        }
        for ring in rings {
            for i in 0..ring.len() {
                if crosses(a, b, ring[i], ring[(i + 1) % ring.len()]) {
                    return Some(ring[i]);
                }
            }
        }
    }
    None
}

/// The shards and fragments of the ways' and junctions' faces.
fn owner_artefacts(plan: &FacePlan, graph: &WayGraph) -> Vec<Artefact> {
    let c = CELL_UNITS as i32;
    let mpu = metres_per_unit(plan.cell.origin().y + CELL_UNITS / 2);
    let rings: Vec<Vec<Vec<P>>> =
        plan.faces.iter().map(|f| f.rings.iter().map(|r| ring_points(plan, r)).collect()).collect();
    let own: Vec<Vec<u32>> = plan.faces.iter().map(|f| own_ways(f, graph)).collect();
    let holds: Vec<Vec<u32>> = (0..plan.faces.len())
        .map(|i| own[i].iter().copied().filter(|&w| enters(&rings[i], graph, w).is_some()).collect())
        .collect();
    let mut next: Vec<Vec<u32>> = vec![Vec::new(); plan.faces.len()];
    for e in plan.edges.iter().filter(|e| e.left != OUTSIDE && e.right != OUTSIDE) {
        next[e.left as usize].push(e.right);
        next[e.right as usize].push(e.left);
    }
    let mut out = Vec::new();
    for f in plan.faces.iter().filter(|f| !own[f.id as usize].is_empty()) {
        let i = f.id as usize;
        let r = &rings[i];
        let on_border = r[0].iter().any(|p| p[0] == 0 || p[0] == c || p[1] == 0 || p[1] == c);
        if holds[i].is_empty() && !on_border {
            let small = area2(&r[0]) as f64 / 2.0 * mpu * mpu < FRAGMENT_M2;
            let joined = next[i].iter().any(|&n| holds[n as usize].iter().any(|w| own[i].contains(w)));
            if small || !joined {
                out.push(Artefact { face: f.id, at: r[0][0], kind: ArtefactKind::Fragment });
            }
        }
        // Ways that meet one of the face's own ways at a junction or a joint.
        let Some(look) = f.material.or(own[i].first().copied()).map(|w| &graph.ways[w as usize]) else { continue };
        let mut related: Vec<u32> = own[i].clone();
        let points =
            graph.junctions.iter().map(|j| j.arms.as_slice()).chain(graph.joints.iter().map(|j| j.arms.as_slice()));
        for arms in points {
            if arms.iter().any(|a| own[i].contains(&a.way)) {
                related.extend(arms.iter().map(|a| a.way));
            }
        }
        for (w, way) in graph.ways.iter().enumerate() {
            let w = w as u32;
            if way.level != Level::Ground
                || way.family != look.family
                || way.class > look.class
                || related.contains(&w)
                || f.crossing == Some(w)
            {
                continue;
            }
            if let Some(at) = enters(r, graph, w) {
                out.push(Artefact { face: f.id, at, kind: ArtefactKind::Shard });
                break;
            }
        }
    }
    out
}

/// Per feature of a cell's input, the name that every cell gives it: its first part's tile and its
/// index there.
pub fn feature_names(input: &crate::CellInput) -> Vec<[u32; 3]> {
    (input.features.iter())
        .map(|f| {
            let (x, y) = f.parts.first().map_or((0, 0), |p| p.tile);
            [x, y, f.index]
        })
        .collect()
}

/// A global name of an owner near a point: a way by its class and its sample nearest the point (two
/// cells number ways apart), a junction by its position, a platform by its feature's name
/// (`features`, from `feature_names`).
pub fn owner_name(graph: &WayGraph, features: &[[u32; 3]], owner: Owner, near: [i64; 2]) -> String {
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
        Owner::Platform(f) => format!("platform {:?}", features[f as usize]),
    }
}

/// The border of a plan on one side of its cell (0 west, 1 north, 2 east, 3 south): its vertices in
/// global positions along the side, and per border segment the attributes of the face inside.
fn border(cell: Cell, side: usize) -> (Vec<[i64; 2]>, Vec<(String, String)>) {
    let (plan, graph, features) = cell;
    let c = CELL_UNITS as i32;
    let o = plan.cell.origin();
    let on = |p: P| match side {
        0 => p[0] == 0 && (0..=c).contains(&p[1]),
        1 => p[1] == 0 && (0..=c).contains(&p[0]),
        2 => p[0] == c && (0..=c).contains(&p[1]),
        _ => p[1] == c && (0..=c).contains(&p[0]),
    };
    let along = |p: P| if side.is_multiple_of(2) { p[1] } else { p[0] };
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
            let way = |x: Option<u32>| x.map(|w| owner_name(graph, features, Owner::Way(w), mid));
            format!(
                "{:?} {:?} {} {} {:?} {:?}",
                f.surface,
                f.rule,
                f.rank,
                owner_name(graph, features, f.owner, mid),
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

/// A cell of check 2: its plan, its graph and its input's `feature_names`.
pub type Cell<'a> = (&'a FacePlan, &'a WayGraph, &'a [[u32; 3]]);

/// Check 2: cell `b` east of `a` (`east`) or south of it: the same vertices on their shared border,
/// and the same attributes of the faces on each side of each border segment.
pub fn neighbours(a: Cell, b: Cell, east: bool) -> Vec<Fault> {
    let (sa, sb) = if east { (2, 0) } else { (3, 1) };
    let (va, fa) = border(a, sa);
    let (vb, fb) = border(b, sb);
    let mut faults = Vec::new();
    if va != vb {
        let first = va.iter().zip(&vb).position(|(x, y)| x != y);
        faults.push(Fault {
            check: "2",
            what: format!("border vertices differ: {} and {}, first at {first:?}", va.len(), vb.len()),
        });
        return faults;
    }
    // A plan holds its cell square only: the faces on the two sides of the line carry one label.
    for (i, (x, y)) in fa.iter().zip(&fb).enumerate() {
        if x.0 != y.0 {
            faults.push(Fault { check: "2", what: format!("segment {i} at {:?}: {x:?} | {y:?}", va[i]) });
        }
    }
    faults
}

/// Concave vertices of one edge closer than this along it form one corner: a polygonal arc or a
/// mapped curve is one corner, not many.
pub const SUPPORT_M: f64 = 1.0;

/// A corner that turns more than this (radians, one arc step and a half degree) can be a kink: a
/// round's arc meets its edges at less.
pub const CORNER_TURN: f64 = 10.5 * std::f64::consts::PI / 180.0;

/// A corner whose radius is under this share of the inner radius of the ways that form it is a kink.
pub const KINK_SHARE: f64 = 0.9;

/// A corner of a ring: its tightest vertex and its radius there, in the ring's units, and the vertex
/// of the corner that turns the most by itself: the corner's own place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub at: usize,
    pub radius: f64,
    pub sharp: usize,
    /// The corner's first and last vertex along the ring.
    pub first: usize,
    pub last: usize,
}

/// The length of each edge of a ring, from vertex i to i + 1.
fn edge_lengths(ring: &[P]) -> Vec<f64> {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1]))
        })
        .collect()
}

/// The point `dist` from vertex i along the ring (`dir` 1 ahead, n − 1 back), or the last vertex
/// before an edge that is no boundary (a vertex without a turn).
fn along(ring: &[P], edge: &[f64], turn: &[Option<f64>], i: usize, dir: usize, dist: f64) -> [f64; 2] {
    let n = ring.len();
    let at = |p: P| [f64::from(p[0]), f64::from(p[1])];
    let (mut k, mut d) = (i, 0.0);
    loop {
        let len = if dir == 1 { edge[k] } else { edge[(k + n - 1) % n] };
        let next = (k + dir) % n;
        if d + len >= dist && len > 0.0 {
            let t = (dist - d) / len;
            let (a, b) = (at(ring[k]), at(ring[next]));
            return [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
        }
        d += len;
        if next == i || turn[next].is_none() {
            return at(ring[next]);
        }
        k = next;
    }
}

/// The corners of a ring that turn toward its right: from the signed turn at each vertex (None where
/// an edge at the vertex is no boundary), the turn within `support` along the ring about each
/// vertex, so a convex vertex there counts against it and a jog or a short needle is no corner.
/// Consecutive vertices whose turn passes `CORNER_TURN` are one corner, a convex one among them too
/// (the chord where a round's arc meets an edge). A corner holds a concave vertex, has the radius
/// `support` over the turn at its tightest concave vertex, and its own place at the concave vertex
/// that turns the most by itself. `support(p)` is the support in units at p.
pub fn corners(ring: &[P], turn: &[Option<f64>], support: &dyn Fn(P) -> f64) -> Vec<Corner> {
    let n = ring.len();
    let radius = radii(ring, turn, support);
    let mut out: Vec<Corner> = Vec::new();
    // Start past a vertex that is no corner, so no corner wraps the ring's start.
    let Some(start) = (0..n).find(|&i| radius[i].is_none()) else { return out };
    let own = |i: usize| turn[i].unwrap_or(0.0);
    let mut run: Option<Corner> = None;
    for x in 1..=n {
        let i = (start + x) % n;
        let Some(r) = radius[i] else {
            out.extend(run.take().filter(|c| own(c.sharp) < 0.0));
            continue;
        };
        let c = run.get_or_insert(Corner { at: i, radius: f64::INFINITY, sharp: i, first: i, last: i });
        c.last = i;
        if own(i) < 0.0 && r < c.radius {
            (c.at, c.radius) = (i, r);
        }
        if own(i) < own(c.sharp) {
            c.sharp = i;
        }
    }
    out.extend(run.filter(|c| own(c.sharp) < 0.0));
    out
}

/// The radius at each vertex of a ring that turns toward its right past `CORNER_TURN` within the
/// support about it (None elsewhere): the support over that turn, between the chords from the point
/// half the support back to the vertex and from the vertex to the point half the support ahead. A
/// chord reads a step of the plan's precision as a small change of direction, so a jog at the
/// support's end is no turn.
pub fn radii(ring: &[P], turn: &[Option<f64>], support: &dyn Fn(P) -> f64) -> Vec<Option<f64>> {
    let n = ring.len();
    let edge = edge_lengths(ring);
    let at = |p: P| [f64::from(p[0]), f64::from(p[1])];
    let reach = |i: usize, dir: usize| along(ring, &edge, turn, i, dir, support(ring[i]) / 2.0);
    (0..n)
        .map(|i| {
            turn[i]?;
            let (p, v, q) = (reach(i, n - 1), at(ring[i]), reach(i, 1));
            let (u, w) = ([v[0] - p[0], v[1] - p[1]], [q[0] - v[0], q[1] - v[1]]);
            let t = libm::atan2(u[0] * w[1] - u[1] * w[0], u[0] * w[0] + u[1] * w[1]);
            (-t > CORNER_TURN).then(|| support(ring[i]) / -t)
        })
        .collect()
}

impl Corner {
    /// The corner's places: each of its vertices that turns past `CORNER_TURN` by itself, the sharpest
    /// first, else its own place. Two square corners closer than the support are one corner with two
    /// places.
    pub fn places(&self, turn: &[Option<f64>]) -> Vec<usize> {
        let n = turn.len();
        let mut out = Vec::new();
        let mut i = self.first;
        loop {
            if turn[i].is_some_and(|t| -t > CORNER_TURN) {
                out.push(i);
            }
            if i == self.last {
                break;
            }
            i = (i + 1) % n;
        }
        // Ties go along the ring from the corner's first vertex, which no ring start moves.
        let along = |i: usize| (i + n - self.first) % n;
        out.sort_by(|&a, &b| turn[a].unwrap_or(0.0).total_cmp(&turn[b].unwrap_or(0.0)).then(along(a).cmp(&along(b))));
        if out.is_empty() {
            out.push(self.sharp);
        }
        out
    }
}

/// The signed turn at vertex i of a ring, toward its left positive.
pub fn turn_at(ring: &[P], i: usize) -> f64 {
    let n = ring.len();
    let (a, b, c) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
    let (x, y) = ([f64::from(b[0] - a[0]), f64::from(b[1] - a[1])], [f64::from(c[0] - b[0]), f64::from(c[1] - b[1])]);
    libm::atan2(x[0] * y[1] - x[1] * y[0], x[0] * y[0] + x[1] * y[1])
}

/// A corner where a paved face meets land, tighter than the inner radius of the ways that form it,
/// that no round of the plan shaped: its place (its sharpest vertex), and its radius.
#[derive(Clone, Debug, PartialEq)]
pub struct Kink {
    pub face: u32,
    pub at: [i32; 2],
    pub radius_m: f64,
}

/// One chain of the paved outline: its vertices, the paved face of the edge from each (`OUTSIDE`
/// after the last vertex of an open chain), and whether it closes.
struct Chain {
    pts: Vec<P>,
    faces: Vec<u32>,
    closed: bool,
}

impl Chain {
    /// The paved face at vertex i: of the edge from it, or into it at an open chain's end.
    fn face(&self, i: usize) -> u32 {
        if self.faces[i] != OUTSIDE {
            self.faces[i]
        } else {
            self.faces[i.saturating_sub(1)]
        }
    }

    /// The signed turn at each vertex; None at the ends of an open chain.
    fn turns(&self) -> Vec<Option<f64>> {
        let n = self.pts.len();
        (0..n).map(|i| (self.closed || (i > 0 && i + 1 < n)).then(|| turn_at(&self.pts, i))).collect()
    }
}

/// The paved outline of a plan: every edge between a paved face (carriageway, crossing, rail bed)
/// and land, the paved side on its left, walked across the borders of paved faces. Edges on the
/// cell border, to another paved face and to a platform (a wall) are no outline.
fn paved_outline(plan: &FacePlan) -> Vec<Chain> {
    let paved = |f: u32| {
        f != OUTSIDE
            && matches!(plan.faces[f as usize].surface, Surface::Carriageway | Surface::Crossing | Surface::RailBed)
    };
    let land = |f: u32| f != OUTSIDE && !paved(f) && plan.faces[f as usize].surface != Surface::Platform;
    // The outline's directed edges from each vertex: (to, the paved face).
    let mut from: BTreeMap<u32, Vec<(u32, u32)>> = BTreeMap::new();
    let mut into: BTreeMap<u32, u32> = BTreeMap::new();
    for e in &plan.edges {
        let (a, b, f) = match (paved(e.left) && land(e.right), paved(e.right) && land(e.left)) {
            (true, _) => (e.a, e.b, e.left),
            (_, true) => (e.b, e.a, e.right),
            _ => continue,
        };
        from.entry(a).or_default().push((b, f));
        *into.entry(b).or_default() += 1;
    }
    let pt = |v: u32| plan.vertices[v as usize];
    // At a vertex with more than one way on, the outline takes the first clockwise from the way back.
    let next = |prev: u32, v: u32, from: &BTreeMap<u32, Vec<(u32, u32)>>| -> Option<(u32, u32)> {
        let list = from.get(&v)?;
        let (o, back) = (pt(v), pt(prev));
        let ang = |q: P| libm::atan2(f64::from(q[1] - o[1]), f64::from(q[0] - o[0]));
        let a0 = ang(back);
        let tau = 2.0 * std::f64::consts::PI;
        list.iter().copied().min_by(|x, y| {
            let dx = (a0 - ang(pt(x.0))).rem_euclid(tau);
            let dy = (a0 - ang(pt(y.0))).rem_euclid(tau);
            dx.total_cmp(&dy).then(x.0.cmp(&y.0))
        })
    };
    let mut used: std::collections::BTreeSet<(u32, u32)> = Default::default();
    // Chains from the outline's open ends first (at the cell border), then the closed cycles.
    let starts: Vec<u32> =
        (from.keys().copied().filter(|v| !into.contains_key(v))).chain(from.keys().copied()).collect();
    let mut out = Vec::new();
    for s in starts {
        let Some(&(b0, f0)) = from[&s].iter().find(|&&(b, _)| !used.contains(&(s, b))) else { continue };
        // The chain: its vertices and the paved face of the edge from each.
        let mut chain = vec![(s, f0)];
        let (mut prev, mut v) = (s, b0);
        used.insert((s, b0));
        let mut closed = false;
        loop {
            if v == s {
                closed = true;
                break;
            }
            let Some((w, f)) = next(prev, v, &from).filter(|&(w, _)| !used.contains(&(v, w))) else {
                chain.push((v, OUTSIDE));
                break;
            };
            used.insert((v, w));
            chain.push((v, f));
            (prev, v) = (v, w);
        }
        if chain.len() >= 3 {
            out.push(Chain {
                pts: chain.iter().map(|&(v, _)| pt(v)).collect(),
                faces: chain.iter().map(|&(_, f)| f).collect(),
                closed,
            });
        }
    }
    out
}

/// The kinks of a plan of level 0. Along the paved outline (`paved_outline`), a corner (`corners`)
/// whose radius is under `KINK_SHARE` of the least `inner_radius_m` of the ways whose carriageway
/// edge runs through its own place is a kink, unless that place lies on the circle of a round of
/// the plan.
pub fn kinks(plan: &FacePlan, graph: &WayGraph, input: &crate::CellInput, rules: &super::Rules) -> Vec<Kink> {
    let mpu = metres_per_unit(plan.cell.origin().y + CELL_UNITS / 2);
    let lines = super::level_lines(graph, input, rules, plan.cell, plan.level);
    // A round shapes a corner whose own place lies on its circle: within the cut that a round may
    // make into its arms (`sag_plan_m`) and the plan's precision.
    let rounded = |q: P, sag: f64| {
        let tol = sag + f64::from(2 * super::slice::PRECISION) * mpu;
        plan.corners.iter().any(|c| (len(c.centre, q) * mpu - c.radius_m).abs() <= tol)
    };
    let mut out = Vec::new();
    for chain in paved_outline(plan) {
        let (pts, n) = (&chain.pts, chain.pts.len());
        let turn = chain.turns();
        for c in corners(pts, &turn, &|_| SUPPORT_M / mpu) {
            // The ways whose edges run through the corner: through any of its vertices.
            let mut ways = None;
            let mut i = c.first;
            loop {
                if let Some((r, s)) = super::outline::at_corner(&lines, pts[i]) {
                    ways = Some(ways.map_or((r, s), |(r0, s0): (f64, f64)| (r0.min(r), s0.max(s))));
                }
                if i == c.last {
                    break;
                }
                i = (i + 1) % n;
            }
            let Some((r_in, sag)) = ways else { continue };
            let radius_m = c.radius * mpu;
            if radius_m >= KINK_SHARE * r_in {
                continue;
            }
            for k in c.places(&turn) {
                let at = pts[k];
                if !rounded(at, sag) {
                    out.push(Kink { face: chain.face(k), at, radius_m });
                }
            }
        }
    }
    out
}

/// A vertex of a short feature lies this far off the outline's line before it or after it: more
/// than the plan's precision can bend an edge.
pub const SHORT_M: f64 = 0.02;

/// A step, a pocket or a needle of the paved outline: a place where the outline turns sharply but
/// runs on in its direction within the support, so the kink measure sees no corner.
#[derive(Clone, Debug, PartialEq)]
pub struct ShortFeature {
    pub face: u32,
    pub at: [i32; 2],
    /// The greatest distance of its vertices from the outline's lines on either side.
    pub size_m: f64,
    /// The signed turn at its place, toward the paved side positive (a convex vertex).
    pub turn: f64,
}

/// The short features of a plan of level 0. Along the paved outline (`paved_outline`), a vertex
/// that turns past `CORNER_TURN` by itself is part of one where the outline's direction from a
/// support back to half a support back and from half a support ahead to a support ahead agree
/// within `CORNER_TURN`, and the vertex lies more than `SHORT_M` off the line of either. Such
/// vertices closer than the support along the outline are one feature, placed at the farthest off.
pub fn short_features(plan: &FacePlan) -> Vec<ShortFeature> {
    let mpu = metres_per_unit(plan.cell.origin().y + CELL_UNITS / 2);
    let support = SUPPORT_M / mpu;
    let mut out = Vec::new();
    for chain in paved_outline(plan) {
        let (pts, n) = (&chain.pts, chain.pts.len());
        let turn = chain.turns();
        let edge = edge_lengths(pts);
        let size: Vec<Option<f64>> = (0..n)
            .map(|i| {
                if turn[i]?.abs() <= CORNER_TURN {
                    return None;
                }
                let reach = |dir: usize, d: f64| along(pts, &edge, &turn, i, dir, d);
                let (p1, p0, q0, q1) =
                    (reach(n - 1, support), reach(n - 1, support / 2.0), reach(1, support / 2.0), reach(1, support));
                let (din, dout) = ([p0[0] - p1[0], p0[1] - p1[1]], [q1[0] - q0[0], q1[1] - q0[1]]);
                let (lin, lout) = (libm::hypot(din[0], din[1]), libm::hypot(dout[0], dout[1]));
                if lin == 0.0 || lout == 0.0 {
                    return None;
                }
                let angle = libm::atan2(din[0] * dout[1] - din[1] * dout[0], din[0] * dout[0] + din[1] * dout[1]);
                if angle.abs() > CORNER_TURN {
                    return None;
                }
                let v = [f64::from(pts[i][0]), f64::from(pts[i][1])];
                let off = |a: [f64; 2], d: [f64; 2], l: f64| (d[0] * (v[1] - a[1]) - d[1] * (v[0] - a[0])).abs() / l;
                let size = off(p0, din, lin).max(off(q0, dout, lout)) * mpu;
                (size > SHORT_M).then_some(size)
            })
            .collect();
        // Group from a vertex that is part of none, so no feature wraps a closed chain's start.
        let Some(start) = (0..n).find(|&i| size[i].is_none()) else { continue };
        let mut run: Option<(ShortFeature, f64)> = None;
        let mut s = 0.0;
        for x in 1..=n {
            let i = (start + x) % n;
            s += edge[(i + n - 1) % n];
            let Some(z) = size[i] else { continue };
            match &mut run {
                Some((f, last)) if s - *last <= support => {
                    *last = s;
                    if z > f.size_m {
                        (f.at, f.size_m, f.face, f.turn) = (pts[i], z, chain.face(i), turn[i].unwrap_or(0.0));
                    }
                }
                _ => {
                    out.extend(run.take().map(|r| r.0));
                    let turn = turn[i].unwrap_or(0.0);
                    run = Some((ShortFeature { face: chain.face(i), at: pts[i], size_m: z, turn }, s));
                }
            }
        }
        out.extend(run.map(|r| r.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corners of an open chain, in units of 1: the turn at each inner vertex.
    fn chain_corners(pts: &[P], support: f64) -> Vec<Corner> {
        let n = pts.len();
        let turn: Vec<Option<f64>> = (0..n).map(|i| (i > 0 && i + 1 < n).then(|| turn_at(pts, i))).collect();
        corners(pts, &turn, &|_| support)
    }

    #[test]
    fn a_square_corner_is_one_corner_at_its_vertex() {
        // Along x, then a right turn (toward −y: concave for the side on the left).
        let pts = [[-1000, 0], [-500, 0], [0, 0], [0, -500], [0, -1000]];
        let c = chain_corners(&pts, 625.0);
        assert_eq!(c.len(), 1);
        assert_eq!(pts[c[0].sharp], [0, 0]);
        assert!((c[0].radius - 625.0 / std::f64::consts::FRAC_PI_2).abs() < 1.0);
    }

    #[test]
    fn a_step_of_the_precision_at_the_support_end_is_no_corner() {
        // A straight edge with a 4-unit step: a turn of ±45° that a window may cut in half.
        let pts = [[-1000, 0], [-312, 0], [-308, -4], [0, -4], [1000, -4]];
        for support in [600.0, 625.0, 630.0] {
            assert!(chain_corners(&pts, support).is_empty(), "support {support}");
        }
    }

    #[test]
    fn an_arc_and_its_tail_are_one_corner_placed_on_the_arc() {
        // A concave arc of radius 1200 in 10° steps, a convex step where its chord meets the
        // edge, then the edge.
        let mut pts: Vec<P> = vec![[-3000, 0], [0, 0]];
        for k in 1..=9 {
            let a = f64::from(k) * 10f64.to_radians();
            pts.push([(1200.0 * libm::sin(a)) as i32, (-1200.0 * (1.0 - libm::cos(a))) as i32]);
        }
        let end = *pts.last().unwrap();
        pts.extend([[end[0] + 3, end[1] - 300], [end[0] + 3, end[1] - 2000]]);
        let c = chain_corners(&pts, 625.0);
        assert_eq!(c.len(), 1, "{c:?}");
        let p = pts[c[0].sharp];
        let off = libm::hypot(f64::from(p[0]), f64::from(p[1] + 1200)) - 1200.0;
        assert!(off.abs() < 2.0, "the corner's place {p:?} lies off the arc by {off}");
    }
}
