//! The checks of a cell's records: the label of sampled points against the face plan's, the far
//! level against the near one, and two cells on their shared line by global names: the labels and
//! the blend edges near each point.

use std::collections::BTreeMap;

use super::{blend_edges, find, label, BINS, BIN_UNITS, CAND, DISC, JUNCTION, LINE, MARGIN_M, PLATFORM, WAY_OWNER};
use crate::faces::check::owner_name;
use crate::faces::geom::winding;
use crate::faces::heights::HeightRules;
use crate::faces::{candidates, decide, Candidates, FacePlan, Label, Owner, Rules, Surface, LAYERS, SLOPE};
use crate::frame::{metres_per_unit, metres_per_unit_at, CellKey, CELL_UNITS};
use crate::graph::WayGraph;
use crate::CellInput;

/// A label as surface, owner kind, rank and rule; owner id; material way; crossing way. Terrain is
/// all zero.
pub type Name = [u32; 4];

const TERRAIN: Name = [0, 0, u32::MAX, u32::MAX];

/// Two cells' distances to a blend edge agree within this, in units. An edge that the two cells split
/// at different vertices past `MARGIN_M` snaps to a different path in each; each path stays in the
/// snap squares of the exact edge, which span up to 2 · PRECISION at the cell lines, so within
/// √2 · PRECISION of it, possibly on opposite sides.
const BLEND_TOL: f64 = 2.0 * std::f64::consts::SQRT_2 * crate::faces::slice::PRECISION as f64;

/// The arc steps of the overlay's polygons: 10°.
const STEP: f64 = std::f64::consts::PI / 18.0;

/// The name of label `id` of a block, its lines as ways.
pub fn records_name(words: &[u32], id: Option<u32>) -> Name {
    let Some(l) = id.map(|id| label(words, id)) else { return TERRAIN };
    if l[0] & 0xff == Surface::Terrain as u32 {
        return TERRAIN;
    }
    let way = |line: u32| if line == u32::MAX { u32::MAX } else { words[words[6] as usize + LINE * line as usize] };
    [l[0], l[1], way(l[2]), way(l[3])]
}

fn name(l: &Label) -> Name {
    if l.surface == Surface::Terrain {
        return TERRAIN;
    }
    let w = super::label_words(l);
    [w[0], w[1], w[2], w[3]]
}

fn face_name(plan: &FacePlan, face: Option<usize>) -> Name {
    face.map_or(TERRAIN, |k| name(&Label::of(&plan.faces[k])))
}

/// The overlay's decision at one point, as the overlay reads it at a patch's inner point.
pub struct Overlay<'a>(Candidates<'a>);

impl<'a> Overlay<'a> {
    pub fn new(graph: &'a WayGraph, input: &CellInput, rules: &'a Rules, cell: CellKey, slopes: bool) -> Self {
        Overlay(candidates(graph, input, rules, cell, 0, if slopes { LAYERS } else { SLOPE }, true))
    }

    /// The name of the overlay's label at q, a point on the half-unit grid.
    pub fn name(&self, q: [f64; 2]) -> Name {
        let d = [(2.0 * q[0]) as i64, (2.0 * q[1]) as i64];
        name(&decide(&self.0.cands, &self.0.lines, self.0.cover.at(d), d))
    }
}

/// The faces of a plan by bin, for the face at a point.
struct Index<'a> {
    plan: &'a FacePlan,
    rings: Vec<Vec<Vec<[i32; 2]>>>,
    bins: Vec<Vec<usize>>,
    /// The edges between two names by bin, grown by `grow` units.
    edges: Vec<Vec<usize>>,
    names: Vec<Name>,
}

fn bin_range(lo: i32, hi: i32, grow: i32) -> std::ops::RangeInclusive<i32> {
    ((lo - grow).div_euclid(BIN_UNITS)).max(0)..=((hi + grow).div_euclid(BIN_UNITS)).min(BINS - 1)
}

impl<'a> Index<'a> {
    fn new(plan: &'a FacePlan, grow: i32) -> Self {
        let n = (BINS * BINS) as usize;
        let rings: Vec<Vec<Vec<[i32; 2]>>> = (plan.faces.iter())
            .map(|f| f.rings.iter().map(|r| r.iter().map(|&v| plan.vertices[v as usize]).collect()).collect())
            .collect();
        let mut bins = vec![Vec::new(); n];
        for (k, r) in rings.iter().enumerate() {
            let (lo, hi) = r[0].iter().fold(([i32::MAX; 2], [i32::MIN; 2]), |(lo, hi), p| {
                ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
            });
            for row in bin_range(lo[1], hi[1], 0) {
                for col in bin_range(lo[0], hi[0], 0) {
                    bins[(row * BINS + col) as usize].push(k);
                }
            }
        }
        let names: Vec<Name> = (0..plan.faces.len()).map(|k| face_name(plan, Some(k))).collect();
        let mut edges = vec![Vec::new(); n];
        for (i, e) in plan.edges.iter().enumerate() {
            let name = |f: u32| names.get(f as usize).copied().unwrap_or(TERRAIN);
            if name(e.left) == name(e.right) {
                continue;
            }
            let (a, b) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
            for row in bin_range(a[1].min(b[1]), a[1].max(b[1]), grow) {
                for col in bin_range(a[0].min(b[0]), a[0].max(b[0]), grow) {
                    edges[(row * BINS + col) as usize].push(i);
                }
            }
        }
        Index { plan, rings, bins, edges, names }
    }

    fn bin(q: [f64; 2]) -> usize {
        let b = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).clamp(0, BINS - 1);
        (b(q[1]) * BINS + b(q[0])) as usize
    }

    /// The name of the face at q, a point on the half-unit grid off the vertices.
    fn name(&self, q: [f64; 2]) -> Name {
        let d = [(2.0 * q[0]) as i64, (2.0 * q[1]) as i64];
        let face = self.bins[Self::bin(q)].iter().copied().find(|&k| winding(&self.rings[k], d) != 0);
        face.map_or(TERRAIN, |k| self.names[k])
    }

    /// The distance in units from q to the nearest edge between two names, up to the index's growth.
    fn to_edge(&self, q: [f64; 2]) -> f64 {
        let p = |v: u32| self.plan.vertices[v as usize].map(f64::from);
        (self.edges[Self::bin(q)].iter())
            .map(|&i| seg_dist(p(self.plan.edges[i].a), p(self.plan.edges[i].b), q))
            .fold(f64::INFINITY, f64::min)
    }
}

fn seg_dist(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> f64 {
    let u = [b[0] - a[0], b[1] - a[1]];
    let t = (((q[0] - a[0]) * u[0] + (q[1] - a[1]) * u[1]) / (u[0] * u[0] + u[1] * u[1]).max(1e-12)).clamp(0.0, 1.0);
    libm::hypot(q[0] - a[0] - t * u[0], q[1] - a[1] - t * u[1])
}

/// Whether q lies inside a disc of the block's candidates, between the circle and its chords of
/// one arc step, rounded to units: where the records' true arc and the overlay's polygon differ.
fn on_arc(words: &[u32], q: [f64; 2]) -> bool {
    let extras = &words[words[8] as usize..];
    (0..words[1] as usize).any(|ci| {
        let k = &words[words[4] as usize + CAND * ci..][..CAND];
        (0..k[6] as usize).any(|d| {
            let e = &extras[k[5] as usize + DISC * d..];
            let r = f64::from(f32::from_bits(e[2]));
            let to = libm::hypot(q[0] - f64::from(e[0] as i32), q[1] - f64::from(e[1] as i32));
            to < r && to >= r * libm::cos(STEP / 2.0) - 1.0
        })
    })
}

/// A sampled point where the records' label differs from the face's.
#[derive(Clone, Debug)]
pub struct Miss {
    pub q: [f64; 2],
    pub face: Name,
    pub records: Name,
    /// The distance to the nearest edge between two names, in units.
    pub edge: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Agreement {
    pub points: u64,
    /// Misses within `edge_units` of an edge between two names.
    pub edge: u64,
    /// Misses between a disc's circle and its chords.
    pub arc: u64,
    /// Misses where `find` gives the overlay's decision at the point, and the overlay gives the
    /// patch the decision at its inner point: past a cut where the slice holds only a part of it,
    /// and the rail bed beside a road (the rule of `faces::label`). Of them, the rail beds.
    pub patch: u64,
    pub rail: u64,
    /// The other misses: faults of the records.
    pub area: Vec<Miss>,
}

/// The sample points of the bins with a candidate item: `n`² points of each on the half-unit grid.
/// A bin with none holds no edge of a candidate, so its points are terrain.
fn samples(words: &[u32], n: i32) -> impl Iterator<Item = [f64; 2]> + '_ {
    let at = move |i: i32, base: i32| f64::from(base * BIN_UNITS + (2 * i + 1) * BIN_UNITS / (2 * n)) + 0.5;
    (0..BINS * BINS)
        .filter(move |&b| words[words[9] as usize + super::HEADER * b as usize + 1] >> 16 > 0)
        .flat_map(move |b| (0..n * n).map(move |k| [at(k % n, b % BINS), at(k / n, b / BINS)]))
}

/// The records' labels against the plan's faces at n² points of every bin with a candidate: a miss
/// within `edge_units` of an edge between two names, between a round's circle and its chords, or
/// where `find` is the overlay's own decision at the point, is counted; any other is listed.
pub fn agreement(words: &[u32], plan: &FacePlan, overlay: &Overlay, n: i32, edge_units: f64) -> Agreement {
    let index = Index::new(plan, edge_units.ceil() as i32);
    let mut out = Agreement::default();
    for q in samples(words, n) {
        out.points += 1;
        let (face, records) = (index.name(q), records_name(words, find(words, q)));
        if face == records {
            continue;
        }
        let edge = index.to_edge(q);
        if edge < edge_units {
            out.edge += 1;
        } else if on_arc(words, q) {
            out.arc += 1;
        } else if overlay.name(q) == records {
            out.patch += 1;
            out.rail += u64::from(face[0] & 0xff == Surface::RailBed as u32);
        } else {
            out.area.push(Miss { q, face, records, edge });
        }
    }
    out
}

/// The far level against the near one at n² points of every bin: the points whose near label is no
/// band layer, and of them those whose far label differs.
pub fn far_agreement(near: &[u32], far: &[u32], n: i32) -> [u64; 2] {
    let band = [Surface::Shoulder, Surface::Verge, Surface::Slope].map(|s| s as u32);
    let words = |w: &[u32], id: Option<u32>| id.map(|id| label(w, id).to_vec());
    let mut count = [0u64; 2];
    for q in samples(near, n) {
        let a = words(near, find(near, q));
        if a.as_ref().is_some_and(|l| band.contains(&(l[0] & 0xff))) {
            continue;
        }
        count[0] += 1;
        count[1] += u64::from(words(far, find(far, q)) != a);
    }
    count
}

/// A cell for the border check: its records, its graph and its input's `feature_names`.
pub type Cell<'a> = (&'a [u32], &'a WayGraph, &'a [[u32; 3]]);

/// The global name of the label at q of a cell: two cells number ways apart.
fn global_name(cell: Cell, q: [f64; 2]) -> String {
    label_name(cell, find(cell.0, q), q)
}

/// The global name of label `id` of a cell, its ways named near q.
fn label_name(cell: Cell, id: Option<u32>, q: [f64; 2]) -> String {
    let (words, graph, features) = cell;
    let o = graph.cell.origin();
    let near = [o.x + q[0] as i64, o.y + q[1] as i64];
    let n = records_name(words, id);
    if n == TERRAIN {
        return "terrain".into();
    }
    let way = |w: u32| (w != u32::MAX).then(|| owner_name(graph, features, Owner::Way(w), near));
    let owner = match (n[0] >> 8) & 0xff {
        WAY_OWNER => Owner::Way(n[1]),
        JUNCTION => Owner::Junction(n[1]),
        PLATFORM => Owner::Platform(n[1]),
        _ => Owner::Terrain,
    };
    format!("{} {} {:?} {:?}", n[0] & !0xff00, owner_name(graph, features, owner, near), way(n[2]), way(n[3]))
}

/// The vertices of a block's candidate items on a cell line, as positions along it, and the count
/// of their edges along it: the line x = `at` (0 or the cell's size) for `vertical`, else y = `at`.
pub(crate) fn on_line(words: &[u32], vertical: bool, at: i32) -> (Vec<i32>, usize) {
    let (mut out, mut edges) = (Vec::new(), 0);
    let edge_bin = if at == 0 { 0 } else { BINS - 1 };
    for k in 0..BINS {
        let (col, row) = if vertical { (edge_bin, k) } else { (k, edge_bin) };
        let head = words[9] as usize + super::HEADER * (row * BINS + col) as usize;
        let mut p = words[head] as usize;
        for _ in 0..words[head + 1] >> 16 {
            let runs = words[p + 1];
            p += 2;
            for _ in 0..runs {
                let n = words[p] as usize;
                let mut last = false;
                for v in 0..n {
                    let (x, y) = (words[p + 1 + 2 * v] as i32, words[p + 2 + 2 * v] as i32);
                    let (on, along) = if vertical { (x, y) } else { (y, x) };
                    let here = on == at && (0..=CELL_UNITS as i32).contains(&along);
                    if here {
                        out.push(along);
                    }
                    edges += usize::from(here && last);
                    last = here;
                }
                p += 1 + 2 * n;
            }
        }
    }
    (out, edges)
}

/// Per pair of the blend face's label and the label beyond (global names), the least distance from q
/// to its blend edges that q's bin lists, if within `reach` units. Only these distances are the
/// neighbours' own: past `MARGIN_M` from the cell, an edge may end elsewhere. The distance takes q
/// from the ends first, so two cells measure alike.
fn blend_near(cell: Cell, q: [f64; 2], reach: f64) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    for e in blend_edges(cell.0, q) {
        let d = |p: [i32; 2]| [f64::from(p[0]) - q[0], f64::from(p[1]) - q[1]];
        let dist = seg_dist(d(e.a), d(e.b), [0.0, 0.0]);
        if dist <= reach {
            let name = format!("{} | {}", label_name(cell, Some(e.region), q), label_name(cell, e.other, q));
            let least = out.entry(name).or_insert(dist);
            *least = f64::min(*least, dist);
        }
    }
    out
}

/// Cell `b` east of `a` (`east`) or south of it, on their shared line by global names, at every
/// `step` units along it and at every vertex of either block on it and 1/1024 unit beside it: the
/// labels, and the blend edges within most of `MARGIN_M`, bit-equal below the heights' region fade.
/// The points read, those of them with a blend edge near, and the places where the two differ.
pub fn border(a: Cell, b: Cell, east: bool, step: i32) -> (u64, u64, Vec<(f64, String, String)>) {
    let c = CELL_UNITS as i32;
    let mut along: Vec<f64> = (0..=c / step).map(|k| f64::from(k * step)).collect();
    for v in on_line(a.0, east, c).0.into_iter().chain(on_line(b.0, east, 0).0) {
        along.extend([f64::from(v) - 1.0 / 1024.0, f64::from(v), f64::from(v) + 1.0 / 1024.0]);
    }
    along.retain(|&s| (0.0..=f64::from(c)).contains(&s));
    along.sort_by(f64::total_cmp);
    along.dedup();
    let at = |s: f64, on: f64| if east { [on, s] } else { [s, on] };
    // Each block lists the blend edges within `MARGIN_M` of a bin in its own units.
    let units = |cell: Cell| MARGIN_M / metres_per_unit(cell.1.cell.origin().y + CELL_UNITS / 2);
    let reach = 0.9 * units(a).min(units(b));
    let fade_m = HeightRules::of(a.0).region_fade_m;
    let (mut blend, mut differ) = (0, Vec::new());
    for &s in &along {
        let (p, q) = (at(s, f64::from(c)), at(s, 0.0));
        let (x, y) = (global_name(a, p), global_name(b, q));
        if x != y {
            differ.push((s, x, y));
        }
        // Both list each pair up to the tolerance past the reach, so a pair within the reach in one
        // cell is in the other's list too.
        let (x, y) = (blend_near(a, p, reach + BLEND_TOL), blend_near(b, q, reach + BLEND_TOL));
        blend += u64::from(!x.is_empty() || !y.is_empty());
        // The heights read a distance below the region fade: there both cells give the same bits.
        let fade = fade_m / metres_per_unit_at((a.1.cell.origin().y as f64) + p[1]);
        let alike = (x.keys().chain(y.keys())).all(|k| {
            let (d, e) = (x.get(k).copied().unwrap_or(f64::INFINITY), y.get(k).copied().unwrap_or(f64::INFINITY));
            d.min(e) > reach || if d.min(e) < fade { d == e } else { (d - e).abs() <= BLEND_TOL }
        });
        if !alike {
            differ.push((s, format!("blend {x:?}"), format!("blend {y:?}")));
        }
    }
    (along.len() as u64, blend, differ)
}

/// Two neighbouring cells' outline edges near their shared line, by global names: each edge of either
/// block within its host's listing reach (`outline_m`'s terms) of the other cell's square must be an
/// edge of the other block with the same words, its host line named by its class and its sample nearest
/// the edge (two cells number a way's legs from its own start). The edges compared, and the ones that
/// differ within `MARGIN_M` of the shared line and past it: only up to `MARGIN_M` past a cell are its
/// inputs the neighbours' own.
pub fn outline_border(a: (&[u32], CellKey), b: (&[u32], CellKey)) -> (u64, [Vec<String>; 2]) {
    type Edge = ([i64; 4], [u32; 5], [i64; 3]);
    let edges = |(w, key): (&[u32], CellKey)| -> Vec<Edge> {
        let o = key.origin();
        let line = |l: u32| &w[w[6] as usize + LINE * l as usize..][..LINE];
        super::outline_words(w)
            .as_chunks::<{ super::EDGE }>()
            .0
            .iter()
            .map(|e| {
                let (pa, pb) = super::edge_ends(e);
                let g = [pa[0], pa[1], pb[0], pb[1]].map(i64::from);
                let g = [g[0] + o.x, g[1] + o.y, g[2] + o.x, g[3] + o.y];
                let host = line(super::edge_line(e));
                let sample = |i: u32| &w[w[7] as usize + super::SAMPLE * (host[2] + i) as usize..][..2];
                let near = (0..host[3])
                    .map(|i| [i64::from(sample(i)[0] as i32) + o.x, i64::from(sample(i)[1] as i32) + o.y])
                    .min_by_key(|s| (s[0] - g[0]).pow(2) + (s[1] - g[1]).pow(2))
                    .unwrap_or([0, 0]);
                let name = [i64::from(host[7]), near[0], near[1]];
                (g, [e[3] >> 16, e[4], e[5], e[6], e[7]], name)
            })
            .collect()
    };
    let reach = |w: &[u32], l: u32| f64::from(f32::from_bits(w[w[6] as usize + LINE * l as usize + 6]));
    let (ea, eb) = (edges(a), edges(b));
    let mut differ = [Vec::new(), Vec::new()];
    let mut checked = 0;
    for ((from, from_edges), (to, to_edges)) in [((a, &ea), (b, &eb)), ((b, &eb), (a, &ea))] {
        let theirs: BTreeMap<[i64; 4], ([u32; 5], [i64; 3])> = to_edges.iter().map(|e| (e.0, (e.1, e.2))).collect();
        let o = to.1.origin();
        let sq = [o.x, o.y, o.x + CELL_UNITS, o.y + CELL_UNITS].map(|v| v as f64);
        let corners = [[sq[0], sq[1]], [sq[2], sq[1]], [sq[0], sq[3]], [sq[2], sq[3]]];
        let f = from.1.origin();
        let own = [f.x, f.y, f.x + CELL_UNITS, f.y + CELL_UNITS].map(|v| v as f64);
        let to_box = |v: [f64; 2]| {
            libm::hypot((sq[0] - v[0]).max(v[0] - sq[2]).max(0.0), (sq[1] - v[1]).max(v[1] - sq[3]).max(0.0))
        };
        for (k, e) in from_edges.iter().enumerate() {
            let (p, q) = ([e.0[0] as f64, e.0[1] as f64], [e.0[2] as f64, e.0[3] as f64]);
            let host = super::edge_line(&super::outline_words(from.0)[super::EDGE * k..]);
            // Well inside the other block's listing reach: less a margin for the scale over a cell.
            let listed = (reach(from.0, host) - 0.1) / metres_per_unit(e.0[1]);
            let d = if segment_meets_box(p, q, sq) {
                0.0
            } else {
                corners.iter().map(|&c| seg_dist(p, q, c)).fold(to_box(p).min(to_box(q)), f64::min)
            };
            if d >= listed {
                continue;
            }
            checked += 1;
            if theirs.get(&e.0) != Some(&(e.1, e.2)) {
                // Within `MARGIN_M` of the shared line: within it of both squares.
                let off =
                    |sq: [f64; 4], v: [f64; 2]| (sq[0] - v[0]).max(v[0] - sq[2]).max(sq[1] - v[1]).max(v[1] - sq[3]);
                let far = [p, q].iter().all(|&v| off(own, v).max(off(sq, v)) * metres_per_unit(e.0[1]) > MARGIN_M);
                differ[usize::from(far)].push(format!(
                    "{:?} host {:?} words {:?}: {:?}",
                    e.0,
                    e.2,
                    e.1,
                    theirs.get(&e.0)
                ));
            }
        }
    }
    (checked, differ)
}

/// Whether segment pq meets the box [x0, y0, x1, y1].
fn segment_meets_box(p: [f64; 2], q: [f64; 2], bx: [f64; 4]) -> bool {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let d = [q[0] - p[0], q[1] - p[1]];
    for (a, b) in [(-d[0], p[0] - bx[0]), (d[0], bx[2] - p[0]), (-d[1], p[1] - bx[1]), (d[1], bx[3] - p[1])] {
        if a == 0.0 {
            if b < 0.0 {
                return false;
            }
        } else if a < 0.0 {
            t0 = t0.max(b / a);
        } else {
            t1 = t1.min(b / a);
        }
    }
    t0 <= t1
}
