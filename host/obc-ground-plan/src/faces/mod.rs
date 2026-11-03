//! The faces of ways and junctions in one cell and level: candidates from the way graph, one integer
//! slice of the window by all their outlines, a label per patch by key, the planar map, the minimum
//! width and the slope bands. Every point of the window lies in exactly one face.

mod bands;
mod cands;
pub mod check;
mod corners;
mod geom;
mod junctions;
mod rails;
pub mod probe_offset;
pub mod rules;
mod slice;

mod slopes;
#[cfg(test)]
mod tests;
mod types;
mod width;

use std::collections::{BTreeMap, BTreeSet};

pub use bands::layers;
pub use rules::{Mouth, Pair, PlanRules, Rank, Rules};
pub use types::*;

use cands::{Cands, Label, TERRAIN};
use geom::{inner_point, P};
use slice::{on_cell_line, slice, Map, NONE};
use width::Thin;

use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS, FRAME_BITS};
use crate::graph::{Family, Level, WayGraph};
use crate::CellInput;

/// At most this many passes of the pinch rule.
const PINCH_PASSES: usize = 3;
fn pass_count() -> usize {
    std::env::var("GP3_PASSES").ok().and_then(|v| v.parse().ok()).unwrap_or(PINCH_PASSES)
}
/// Ord values of inserted samples compare within this.
const ORD_EPS: f64 = 1e-12;

/// What every rule reads.
pub(crate) struct Ctx<'a> {
    pub graph: &'a WayGraph,
    pub input: &'a CellInput,
    pub rules: &'a Rules,
    pub cell: CellKey,
    pub origin: GlobalPoint,
    pub level: i8,
    pub window: [i32; 4],
}

impl Ctx<'_> {
    /// Whether a way is on the plan's level: the ground ways of roads, paths and rails for level 0, the
    /// ways of deck k − 1 for level k.
    pub fn in_level(&self, way: u32) -> bool {
        let w = &self.graph.ways[way as usize];
        if w.family == Family::Water {
            return false;
        }
        match self.level {
            0 => w.level == Level::Ground,
            k => w.level == Level::Deck && w.deck == Some(k as u32 - 1),
        }
    }
}

/// The window of a cell: its square grown by `window_m` at its edge nearer the pole, in cell units.
pub fn window(cell: CellKey, window_m: f64) -> [i32; 4] {
    let o = cell.origin();
    let pole = if o.y < 1 << (FRAME_BITS - 1) { o.y } else { o.y + CELL_UNITS };
    let w = (window_m / metres_per_unit(pole)).floor() as i32;
    let c = CELL_UNITS as i32;
    [-w, -w, c + w, c + w]
}

/// The four cell lines across the window.
fn cell_lines(window: [i32; 4]) -> Vec<[P; 2]> {
    let [x0, y0, x1, y1] = window;
    let c = CELL_UNITS as i32;
    vec![[[0, y0], [0, y1]], [[c, y0], [c, y1]], [[x0, 0], [x1, 0]], [[x0, c], [x1, c]]]
}

/// One slice with its labels: the base label of each patch from the candidates, then each pass of thin
/// pieces, then the slope region.
struct Sliced {
    map: Map,
    labels: Vec<u32>,
}

fn sliced(
    cands: &mut Cands,
    window: [i32; 4],
    extra: &[Vec<P>],
    passes: &[Vec<Thin>],
    slope: Option<(&[Vec<P>], u32)>,
    stats: &mut FaceStats,
) -> Sliced {
    let mut strings: Vec<Vec<P>> = cands.strings().cloned().collect();
    strings.extend(extra.iter().cloned());
    let t0 = std::time::Instant::now();
    let (patches, segments) = slice(window, &strings, &cell_lines(window));
    tick("slice", t0);
    stats.slices += 1;
    stats.slice_segments = stats.slice_segments.max(segments as u32);
    stats.patches = patches.len() as u32;
    let mut labels = Vec::with_capacity(patches.len());
    let inner: Vec<Option<[i64; 2]>> = patches.iter().map(|p| inner_point(p)).collect();
    for (patch, q) in patches.iter().zip(&inner) {
        labels.push(cands.label(*q, &patch[0]));
    }
    tick("labels", t0);
    for pass in passes {
        for (i, patch) in patches.iter().enumerate() {
            if let Some(to) = width::apply(pass, labels[i], inner[i], &patch[0]) {
                labels[i] = to;
            }
        }
    }
    if let Some((rings, label)) = slope {
        for (i, q) in inner.iter().enumerate() {
            let q = q.unwrap_or([2 * i64::from(patches[i][0][0][0]), 2 * i64::from(patches[i][0][0][1])]);
            if labels[i] == TERRAIN && geom::winding(rings, q) != 0 {
                labels[i] = label;
            }
        }
    }
    let map = Map::new(patches);
    tick("map", t0);
    Sliced { map, labels }
}

/// The face plan of one cell and level. `slopes = false` leaves out the slope bands (decks).
pub fn build_faces(
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    cell: CellKey,
    level: i8,
    slopes: bool,
) -> FacePlan {
    let window = window(cell, std::env::var("GP3_WINDOW").ok().and_then(|v| v.parse().ok()).unwrap_or(rules.plan.window_m));
    let ctx = Ctx { graph, input, rules, cell, origin: cell.origin(), level, window };
    let mut stats = FaceStats::default();
    let mut secs = bands::sections(&ctx);
    if std::env::var("GP3_OFFSET").is_ok() {
        let t0 = std::time::Instant::now();
        let mut cands = Cands::new(window);
        let probe = probe_offset::candidates(&ctx, &secs, &mut cands);
        tick("offset candidates", t0);
        stats.candidates = cands.cands.len() as u32;
        stats.pieces = cands.pieces.len() as u32;
        let t1 = std::time::Instant::now();
        let mut strings: Vec<Vec<P>> = cands.strings().cloned().collect();
        strings.retain(|r| r.len() >= 3);
        let (patches, segments) = slice(window, &strings, &cell_lines(window));
        tick("offset slice", t1);
        stats.slice_segments = segments as u32;
        stats.patches = patches.len() as u32;
        let t2 = std::time::Instant::now();
        let labels: Vec<u32> = patches
            .iter()
            .map(|p| {
                let q = inner_point(p).unwrap_or([2 * i64::from(p[0][0][0]), 2 * i64::from(p[0][0][1])]);
                probe_offset::label(&ctx, &mut cands, &probe, q)
            })
            .collect();
        tick("offset labels", t2);
        let t3 = std::time::Instant::now();
        let s = Sliced { map: Map::new(patches), labels };
        tick("offset map", t3);
        let t4 = std::time::Instant::now();
        let (thin, left) = width::pinches(&ctx, &s.map, &s.labels, &cands.labels, None);
        stats.pinches_left = left + thin.len() as u32;
        stats.thin_pieces.extend(thin.iter().map(|t| t.hull.clone()));
        tick("offset pinch search", t4);
        let t5 = std::time::Instant::now();
        let plan = output(&ctx, s, &cands, Vec::new(), stats);
        tick(&format!("offset output: segments {} patches {} faces {}", plan.stats.slice_segments, plan.stats.patches, plan.stats.faces), t5);
        return plan;
    }
    junctions::taper(&ctx, &mut secs);
    let mut cands = Cands::new(window);
    let mut lc = vec![[None; 5]; graph.ways.len()];
    let t0 = std::time::Instant::now();
    let edits = junctions::build(&ctx, &secs, &mut cands, &mut lc, &mut stats);
    tick("points", t0);
    for foot in &edits.feet {
        let Some(w) = secs[foot.way as usize].as_mut() else { continue };
        let at = w.s.partition_point(|s| s.ord < foot.s.ord);
        let taken = |i: usize| w.s.get(i).is_some_and(|s| s.g == foot.s.g || (s.ord - foot.s.ord).abs() < ORD_EPS);
        if !taken(at) && !(at > 0 && taken(at - 1)) {
            w.s.insert(at, foot.s.clone());
        }
    }
    for (cand, key, c, seams, arc) in &edits.fills {
        let (Some(wa), Some(wb)) = (&secs[seams[0].way as usize], &secs[seams[1].way as usize]) else { continue };
        let mut ring = vec![*c];
        ring.extend(seams[0].points(wa));
        ring.extend(arc.iter().copied());
        ring.extend(seams[1].points(wb).into_iter().rev());
        cands.ring(*cand, *key, ring);
    }
    for w in secs.iter().flatten() {
        let layer = |side: usize, k: usize| if side == bands::LEFT { 2 + k } else { 2 - k };
        let cut = |l: usize, a: f64, b: f64| {
            edits.cuts.iter().any(|&(way, side, k, lo, hi)| {
                way == w.way && layer(side, k) == l && a.min(b) >= lo - ORD_EPS && a.max(b) <= hi + ORD_EPS
            })
        };
        let take = |a: f64, b: f64| {
            (edits.takes.iter())
                .find(|t| t.0 == w.way && a.min(b) >= t.1 - ORD_EPS && a.max(b) <= t.2 + ORD_EPS)
                .map(|t| (t.3, t.4))
        };
        bands::bands(&ctx, &mut cands, w, &mut lc[w.way as usize], &cut, &take);
    }
    if level == 0 {
        rails::platforms(&ctx, &mut cands, &mut stats);
    }
    tick("bands", t0);
    stats.candidates = cands.cands.len() as u32;
    stats.pieces = cands.pieces.len() as u32;
    // Stage 2a: the slice, then the pinch passes.
    let mut passes: Vec<Vec<Thin>> = Vec::new();
    let mut extra: Vec<Vec<P>> = Vec::new();
    let mut s = sliced(&mut cands, window, &extra, &passes, None, &mut stats);
    for pass in 0..=pass_count() {
        let t1 = std::time::Instant::now();
        let (thin, left) = width::pinches(&ctx, &s.map, &s.labels, &cands.labels, None);
        tick(&format!("pinches {} {left}", thin.len()), t1);
        if thin.is_empty() || pass == pass_count() {
            stats.pinches_left = left + if pass == pass_count() { thin.len() as u32 } else { 0 };
            break;
        }
        count_thin(&thin, &cands, &mut stats);
        extra.extend(thin.iter().map(|t| t.hull.clone()));
        passes.push(thin);
        s = sliced(&mut cands, window, &extra, &passes, None, &mut stats);
    }
    // Stage 2b: the slope bands of the outer edges.
    if slopes {
        let region = slopes::region(&ctx, &s.map, &s.labels, &cands.labels);
        let label = cands
            .labels
            .id(slopes::label(rules), cands::Key { rank: rules.plan.rank(Rank::Slope), ..cands::Key::TERRAIN });
        extra.extend(region.iter().cloned());
        s = sliced(&mut cands, window, &extra, &passes, Some((&region, label)), &mut stats);
        for pass in 0..=pass_count() {
            let ground = |l: u32| matches!(cands.labels.list[l as usize].surface, Surface::Slope | Surface::Terrain);
            let (thin, _) = width::pinches(&ctx, &s.map, &s.labels, &cands.labels, Some(&ground));
            if thin.is_empty() || pass == pass_count() {
                break;
            }
            count_thin(&thin, &cands, &mut stats);
            extra.extend(thin.iter().map(|t| t.hull.clone()));
            passes.push(thin);
            s = sliced(&mut cands, window, &extra, &passes, Some((&region, label)), &mut stats);
        }
        let (thin, left) = width::pinches(&ctx, &s.map, &s.labels, &cands.labels, None);
        stats.pinches_left = left + thin.len() as u32;
        if std::env::var("GP3_DEBUG").is_ok() {
            for t in thin.iter().take(14) {
                eprintln!(
                    "LEFT {:?}/{:?} -> {:?}/{:?} {:?}",
                    cands.labels.list[t.from as usize].surface,
                    cands.labels.list[t.from as usize].owner,
                    cands.labels.list[t.to as usize].surface,
                    cands.labels.list[t.to as usize].owner,
                    &t.hull[..t.hull.len().min(4)]
                );
            }
            eprintln!("left {left}, thin {}, stats {:?}", thin.len(), stats.thin);
        }
    }
    output(&ctx, s, &cands, edits.arcs, stats)
}

pub(crate) fn tick(what: &str, t0: std::time::Instant) {
    if std::env::var("GP3_DEBUG").is_ok() {
        eprintln!("{what}: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    }
}

fn count_thin(thin: &[Thin], cands: &Cands, stats: &mut FaceStats) {
    for t in thin {
        stats.thin_pieces.push(t.hull.clone());
        let (a, b) = (cands.labels.list[t.from as usize].surface, cands.labels.list[t.to as usize].surface);
        *stats.thin.entry((a, b)).or_default() += 1;
    }
}

/// The faces of the final map, its vertices, edges with kinds and flags, and the corner arcs.
fn output(ctx: &Ctx, s: Sliced, cands: &Cands, mut arcs: Vec<CornerArc>, mut stats: FaceStats) -> FacePlan {
    let map = &s.map;
    let groups = map.groups(&s.labels, false);
    let loops = map.loops(&groups);
    // Faces by group: outer loop, then holes.
    let mut by_group: BTreeMap<u32, (Vec<u32>, Vec<Vec<u32>>)> = BTreeMap::new();
    for (g, lp, area) in loops {
        let entry = by_group.entry(g).or_default();
        if area > 0 && entry.0.is_empty() {
            entry.0 = lp;
        } else {
            entry.1.push(lp);
        }
    }
    let mut used: BTreeSet<u32> = BTreeSet::new();
    for (outer, holes) in by_group.values() {
        used.extend(outer.iter().chain(holes.iter().flatten()));
    }
    let verts: Vec<u32> = used.into_iter().collect();
    // Map vertices are already sorted by (y, x).
    let index = |v: u32| verts.binary_search(&v).unwrap() as u32;
    let rel = Relations::new(ctx);
    let c = CELL_UNITS as i32;
    let mut faces: Vec<(u32, Face)> = by_group
        .into_iter()
        .map(|(g, (outer, holes))| {
            let label: Label = cands.labels.list[s.labels[g as usize] as usize];
            let ring = |lp: &[u32]| {
                let mut r: Vec<u32> = lp.iter().map(|&v| index(v)).collect();
                let start = (0..r.len()).min_by_key(|&i| r[i]).unwrap_or(0);
                r.rotate_left(start);
                r
            };
            let mut rings = vec![ring(&outer)];
            rings.extend(holes.iter().map(|h| ring(h)));
            let pts: Vec<P> = outer.iter().map(|&v| map.point(v)).collect();
            let inside = pts.iter().all(|p| (0..=c).contains(&p[0]) && (0..=c).contains(&p[1]));
            let face = Face {
                id: 0,
                owner: label.owner,
                surface: label.surface,
                level: ctx.level,
                rule: label.rule,
                material: label.material,
                crossing: label.crossing,
                rank: label.rank,
                inside,
                reach_m: label.reach_m,
                rings,
            };
            (g, face)
        })
        .collect();
    faces.sort_by_key(|(_, f)| (f.surface, f.owner, f.rings[0].first().copied()));
    let mut face_of: BTreeMap<u32, u32> = BTreeMap::new();
    for (id, (g, f)) in faces.iter_mut().enumerate() {
        f.id = id as u32;
        face_of.insert(*g, id as u32);
    }
    let faces: Vec<Face> = faces.into_iter().map(|(_, f)| f).collect();
    let mut edges = Vec::new();
    for (h, e) in map.half.iter().enumerate() {
        if !map.bounds(&groups, h as u32) {
            continue;
        }
        let other = map.across(&groups, h as u32);
        let (a, b) = (index(e.a), index(e.b));
        if other != NONE && a > b {
            continue;
        }
        let left = face_of[&groups[e.patch as usize]];
        let right = if other == NONE { OUTSIDE } else { face_of[&other] };
        let (a, b, left, right) = if a < b { (a, b, left, right) } else { (b, a, right, left) };
        let (pa, pb) = (map.point(e.a), map.point(e.b));
        let kind = if on_cell_line(pa, pb) { EdgeKind::CellBorder } else { EdgeKind::Continuous };
        let flags = if left == OUTSIDE || right == OUTSIDE {
            0
        } else {
            rel.flags(&faces[left as usize], &faces[right as usize])
        };
        edges.push(Edge { a, b, left, right, kind, flags });
    }
    edges.sort_by_key(|e| (e.a, e.b));
    arcs.sort_by(|x, y| (x.owner, x.centre).cmp(&(y.owner, y.centre)));
    stats.faces = faces.len() as u32;
    stats.vertices = verts.len() as u32;
    stats.edges = edges.len() as u32;
    FacePlan {
        cell: ctx.cell,
        level: ctx.level,
        vertices: verts.iter().map(|&v| map.point(v)).collect(),
        faces,
        edges,
        corners: arcs,
        stats,
    }
}

/// Which owners a junction, a joint or a bend relates, for the edge flags.
struct Relations {
    /// The points (junction ids, joint ids + offset) of each way.
    points: Vec<BTreeSet<u32>>,
    hosts: Vec<BTreeSet<u32>>,
}

impl Relations {
    fn new(ctx: &Ctx) -> Relations {
        let g = ctx.graph;
        let mut points = vec![BTreeSet::new(); g.ways.len()];
        let mut hosts = vec![BTreeSet::new(); g.junctions.len()];
        for (i, j) in g.junctions.iter().enumerate() {
            for a in &j.arms {
                points[a.way as usize].insert(i as u32);
                if a.host {
                    hosts[i].insert(a.way);
                }
            }
        }
        let base = g.junctions.len() as u32;
        for (i, j) in g.joints.iter().enumerate() {
            for a in &j.arms {
                points[a.way as usize].insert(base + i as u32);
            }
        }
        Relations { points, hosts }
    }

    fn related(&self, a: Owner, b: Owner) -> bool {
        let share = |x: u32, y: u32| x == y || !self.points[x as usize].is_disjoint(&self.points[y as usize]);
        let arm = |w: u32, j: u32| self.points[w as usize].contains(&j);
        match (a, b) {
            _ if a == b => true,
            (Owner::Way(x), Owner::Way(y)) => share(x, y),
            (Owner::Way(w), Owner::Junction(j)) | (Owner::Junction(j), Owner::Way(w)) => arm(w, j),
            (Owner::Way(w), Owner::Bend(x, _)) | (Owner::Bend(x, _), Owner::Way(w)) => share(w, x),
            (Owner::Junction(j), Owner::Junction(k)) => self.points.iter().any(|p| p.contains(&j) && p.contains(&k)),
            (Owner::Bend(x, _), Owner::Bend(y, _)) => share(x, y),
            (Owner::Junction(j), Owner::Bend(w, _)) | (Owner::Bend(w, _), Owner::Junction(j)) => arm(w, j),
            _ => false,
        }
    }

    fn flags(&self, a: &Face, b: &Face) -> u8 {
        let mut flags = 0;
        let blend = |f: &Face| f.rule == HeightRule::Blend;
        if blend(a) && blend(b) {
            flags |= BLEND_JOIN;
        }
        let seam = |x: &Face, y: &Face| match (x.owner, y.owner) {
            (Owner::Junction(j), Owner::Way(h)) => {
                x.rule == HeightRule::Blend
                    && matches!(x.surface, Surface::Carriageway | Surface::Crossing)
                    && self.hosts[j as usize].contains(&h)
                    && matches!(y.surface, Surface::Carriageway | Surface::Crossing | Surface::Shoulder)
            }
            _ => false,
        };
        if seam(a, b) || seam(b, a) {
            flags |= SEAM;
        }
        let ground = |f: &Face| matches!(f.surface, Surface::Slope | Surface::Terrain);
        if !ground(a) && !ground(b) && !(blend(a) && blend(b)) && !self.related(a.owner, b.owner) {
            flags |= UNRELATED;
        }
        flags
    }
}
