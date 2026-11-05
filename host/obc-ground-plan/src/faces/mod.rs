//! The faces of ways and junctions in one cell and level: integer strokes of the ways, a round per
//! pair of neighbouring arms at each graph point (a tangent circle and its fan), point regions that
//! relabel their arms up to straight seams, band layers as wider strokes, one integer slice of the
//! cell by all their outlines, and a label per patch by key. Every point of the cell lies in exactly
//! one face.

pub mod check;
mod cover;
mod geom;
mod outline;
mod platform;
mod points;
pub mod rules;
mod slice;
mod stroke;
#[cfg(test)]
mod tests;
mod types;

use std::collections::BTreeMap;

pub use rules::{input_m, Mouth, Pair, PlanRules, Rank, Rules};
pub use types::*;

use cover::Cover;
use geom::{cell_line_points, inner_point, with_cell_lines, P};
use points::{Arm, Part, Rounded};
use slice::{on_cell_line, slice, snap_round, Loop, Map, NONE};
use stroke::{pieces_along, union, Rings};

use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS, FRAME_BITS};
use crate::graph::{Family, Level, Station, WayGraph};
use crate::sections::WaySection;
use crate::CellInput;

/// A way of the level: all its samples in cell units, with its half-width and band offsets.
pub(crate) struct WayLine<'a> {
    pub way: u32,
    pub sec: WaySection<'a>,
    pub rail: bool,
    pub p: Vec<P>,
    /// The carriageway half-width in units.
    pub h: Vec<f64>,
    /// The outer offsets in units of the band layers: paved shoulder, shoulder, verge, slope.
    pub bands: Vec<[f64; 4]>,
    /// The global key of the span of each sample.
    pub key: Vec<Glob>,
    /// Per sample, the sides of the segment from it on with a sidewalk: 1 left, 2 right.
    pub walk: Vec<u8>,
    pub round: [bool; 2],
    /// The box of its carriageway: its samples' box grown by its widest half-width.
    pub bbox: [i32; 4],
    /// The boxes of the carriageway of each `CHUNK` segments in order.
    pub chunks: Vec<[i32; 4]>,
    /// The index in `p` of each of the way's graph samples: a taper adds vertices between them.
    pub index: Vec<usize>,
}

/// Segments per box of `WayLine::chunks`: a test near one point reads only the segments of a long way
/// there.
const CHUNK: usize = 16;

impl WayLine<'_> {
    /// The segments (i, i + 1) whose carriageway can come within `pad` units of q.
    pub fn near(&self, q: [f64; 2], pad: f64) -> impl Iterator<Item = usize> + '_ {
        let far = move |b: &[i32; 4]| {
            q[0] < f64::from(b[0]) - pad
                || q[0] > f64::from(b[2]) + pad
                || q[1] < f64::from(b[1]) - pad
                || q[1] > f64::from(b[3]) + pad
        };
        let n = self.p.len().saturating_sub(1);
        let chunks = if far(&self.bbox) { &self.chunks[..0] } else { &self.chunks[..] };
        (chunks.iter().enumerate())
            .filter(move |(_, b)| !far(b))
            .flat_map(move |(k, _)| k * CHUNK..((k + 1) * CHUNK).min(n))
    }

    /// The offsets of the layers at sample i, inner to outer: carriageway, paved shoulder,
    /// shoulder, verge, slope.
    pub fn offsets(&self, i: usize) -> [f64; 5] {
        let b = self.bands[i];
        [self.h[i], b[0], b[1], b[2], b[3]]
    }
}

/// A global name that two cells give alike: a span's owner tile and feature with its start vertex,
/// or a point's position.
type Glob = [i64; 5];

/// The window of a cell: its square grown by `m` metres at its edge nearer the pole, in cell units.
pub fn window(cell: CellKey, m: f64) -> [i32; 4] {
    let o = cell.origin();
    let pole = if o.y < 1 << (FRAME_BITS - 1) { o.y } else { o.y + CELL_UNITS };
    let w = (m / metres_per_unit(pole)).floor() as i32;
    let c = CELL_UNITS as i32;
    [-w, -w, c + w, c + w]
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Label {
    owner: Owner,
    surface: Surface,
    rule: HeightRule,
    material: Option<u32>,
    crossing: Option<u32>,
    rank: u8,
    reach_m: f32,
}

const TERRAIN: Label = Label {
    owner: Owner::Terrain,
    surface: Surface::Terrain,
    rule: HeightRule::Terrain,
    material: None,
    crossing: None,
    rank: u8::MAX,
    reach_m: 0.0,
};

/// A candidate: rings with the NonZero rule, a label, and the key by which it wins: rank, class,
/// kind (a point before a way), then its global name (a way's at the point asked). A point's region
/// (`relabel`: the parts of its arms' lines in it, each with its seams) gives no surface: it gives its
/// label to a patch of one of its ways that lies past none of that way's seams. A point's fans give
/// a patch past one of the seams they reach across (`beyond`) the label of the face on its other side.
/// A round at a corner of the paved outline (`fill`) takes the label at its point inside the paved
/// area.
struct Cand {
    rings: Rings,
    label: Label,
    key: (u8, u8, u8),
    glob: Option<Glob>,
    line: usize,
    relabel: Option<Vec<Part>>,
    beyond: Vec<([P; 2], Label)>,
    fill: Option<[i64; 2]>,
}

struct Ctx<'a> {
    graph: &'a WayGraph,
    rules: &'a Rules,
    origin: GlobalPoint,
}

impl Ctx<'_> {
    fn units(&self, m: f64, p: P) -> f64 {
        m / metres_per_unit(self.origin.y + i64::from(p[1]))
    }

    fn global(&self, p: P) -> [i64; 2] {
        [self.origin.x + i64::from(p[0]), self.origin.y + i64::from(p[1])]
    }
}

fn in_level(graph: &WayGraph, way: u32, level: i8) -> bool {
    let w = &graph.ways[way as usize];
    if w.family == Family::Water {
        return false;
    }
    match level {
        0 => w.level == Level::Ground,
        k => w.level == Level::Deck && w.deck == Some(k as u32 - 1),
    }
}

fn station_key(way: u32, dir: i8, st: &Station) -> (u32, i8, u32, u64) {
    (way, dir, st.span, st.u.to_bits())
}

/// The lines of the level's ways whose centre line meets the box `bx`.
fn lines<'a>(ctx: &Ctx<'a>, input: &CellInput, level: i8, bx: [i32; 4]) -> (Vec<WayLine<'a>>, BTreeMap<u32, usize>) {
    let g = ctx.graph;
    // The round ends: free ends. A way ends square at a junction or a joint, whose face fills the
    // gaps between its arms; and at a level change.
    let mut ends: BTreeMap<(u32, i8, u32, u64), bool> = BTreeMap::new();
    for e in &g.ends {
        ends.insert(station_key(e.arm.way, e.arm.dir, &e.arm.at), true);
    }
    let mut out = Vec::new();
    let mut of_way = BTreeMap::new();
    for way in &g.ways {
        if !in_level(g, way.id, level) {
            continue;
        }
        let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
        if !samples.windows(2).any(|s| geom::meets(bx, s[0].pos, s[1].pos)) {
            continue;
        }
        let part = &g.parts[way.part as usize];
        let feature = &input.features[part.feature.0 as usize];
        let sec = ctx.rules.sections.way(&feature.kind, &feature.tags, false);
        let sh = sec.shoulder_m;
        let mut line = WayLine {
            way: way.id,
            rail: way.family == Family::Rail,
            p: Vec::with_capacity(samples.len()),
            h: Vec::with_capacity(samples.len()),
            bands: Vec::with_capacity(samples.len()),
            key: Vec::with_capacity(samples.len()),
            walk: samples.iter().map(|s| s.walk).collect(),
            round: [
                ends.get(&station_key(way.id, 1, &way.from)).copied().unwrap_or(false),
                ends.get(&station_key(way.id, -1, &way.to)).copied().unwrap_or(false),
            ],
            bbox: [i32::MAX, i32::MAX, i32::MIN, i32::MIN],
            chunks: Vec::new(),
            index: (0..samples.len()).collect(),
            sec,
        };
        for s in samples {
            let u = |m: f64| ctx.units(m, s.pos);
            let h = sec.half_m + s.widen_m;
            line.p.push(s.pos);
            line.h.push(u(h));
            let paved = if sec.shoulder_paved { sh } else { 0.0 };
            let verge = h + sh + sec.verge_m;
            line.bands.push([u(h + paved), u(h + sh), u(verge), u(verge + sec.reach_m)]);
            let span = s.span.min(part.spans.len() as u32 - 1) as usize;
            let o = part.spans[span].owner;
            let v = part.vertices[span].pos;
            line.key.push([i64::from(o[0]), i64::from(o[1]), i64::from(o[2]), v[0], v[1]]);
        }
        of_way.insert(way.id, out.len());
        out.push(line);
    }
    taper(ctx, &mut out, &of_way);
    let grow =
        |b: [i32; 4], p: P, h: i32| [b[0].min(p[0] - h), b[1].min(p[1] - h), b[2].max(p[0] + h), b[3].max(p[1] + h)];
    for line in &mut out {
        let h = libm::ceil(line.h.iter().copied().fold(0.0, f64::max)) as i32;
        for &p in &line.p {
            line.bbox = grow(line.bbox, p, h);
        }
        let n = line.p.len();
        line.chunks = (0..n.saturating_sub(1).div_ceil(CHUNK))
            .map(|k| {
                let ids = k * CHUNK..((k + 1) * CHUNK + 1).min(n);
                let h = libm::ceil(line.h[ids.clone()].iter().copied().fold(0.0, f64::max)) as i32;
                ids.fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, i| grow(b, line.p[i], h))
            })
            .collect();
    }
    (out, of_way)
}

/// A taper of a line's layers from a joint: from the vertex `at` along `dir`, each layer's offset
/// starts at the other arm's offset of that layer, `to`.
struct Taper {
    at: usize,
    dir: i8,
    to: [f64; 5],
}

/// At a joint, each layer edge (carriageway, paved shoulder, shoulder, verge, slope) of the arm that
/// is wider there runs straight into the other arm's edge of that layer: its offset is at most the
/// other's plus a rate times the distance from the joint. So a band that the other arm lacks tapers
/// out with the carriageway. The rate is `taper_rate`, or more where the layer's own offset within
/// `extent_m` of the joint (it widens in a bend) would end the taper past `extent_m`: so the taper
/// meets the own edge within `extent_m`, with no step. A vertex is added where a taper meets the own
/// offset, so the edge turns there and at the joint only. A junction's arms keep their widths.
fn taper(ctx: &Ctx, lines: &mut [WayLine], of_way: &BTreeMap<u32, usize>) {
    let g = ctx.graph;
    let mut tapers: BTreeMap<usize, Vec<Taper>> = BTreeMap::new();
    for (pos, arms) in g.joints.iter().map(|j| (j.pos, &j.arms)) {
        let (Some(&la), Some(&lb)) = (of_way.get(&arms[0].way), of_way.get(&arms[1].way)) else { continue };
        if arms[0].level != arms[1].level || la == lb {
            continue;
        }
        let at = [sample_at(g, &lines[la], &arms[0].at, pos), sample_at(g, &lines[lb], &arms[1].at, pos)];
        let o = [lines[la].offsets(at[0]), lines[lb].offsets(at[1])];
        for (me, l) in [(0, la), (1, lb)] {
            if (0..5).any(|m| o[me][m] - o[1 - me][m] >= 1.0) {
                tapers.entry(l).or_default().push(Taper { at: at[me], dir: arms[me].dir, to: o[1 - me] });
            }
        }
    }
    for (l, list) in tapers {
        let line = &mut lines[l];
        let n = line.p.len();
        let extent = ctx.units(ctx.rules.plan.extent_m, line.p[list[0].at]);
        let own: Vec<[f64; 5]> = (0..n).map(|i| line.offsets(i)).collect();
        // The distance of each vertex from each taper's joint along the line, to the first past
        // `extent_m`; each taper's rate per layer.
        let mut along = vec![vec![f64::INFINITY; n]; list.len()];
        let mut rate = vec![[ctx.rules.plan.taper_rate; 5]; list.len()];
        for (k, t) in list.iter().enumerate() {
            along[k][t.at] = 0.0;
            let mut widest = own[t.at];
            let mut i = t.at;
            while let Some(j) = i.checked_add_signed(isize::from(t.dir)).filter(|&j| j < n) {
                let (a, b) = (line.p[i], line.p[j]);
                along[k][j] = along[k][i] + libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
                widest = std::array::from_fn(|m| widest[m].max(own[j][m]));
                if along[k][j] >= extent {
                    break;
                }
                i = j;
            }
            for m in 0..5 {
                rate[k][m] = rate[k][m].max((widest[m] - t.to[m]) / extent);
            }
        }
        // The offsets at distances `s` from the joints: each layer at most its tapers, and no layer
        // inside the one below it.
        let narrowed = |o: [f64; 5], s: &[f64]| {
            let mut r = o;
            for (k, t) in list.iter().enumerate().filter(|&(k, _)| s[k].is_finite()) {
                for m in 0..5 {
                    r[m] = r[m].min(t.to[m] + rate[k][m] * s[k]);
                }
            }
            for m in 1..5 {
                r[m] = r[m].max(r[m - 1]);
            }
            r
        };
        // The vertices: position, offsets, key, walk.
        let mut rows: Vec<(P, [f64; 5], Glob, u8)> = Vec::with_capacity(n);
        let mut index = Vec::with_capacity(n);
        for i in 0..n {
            index.push(rows.len());
            let s: Vec<f64> = (0..list.len()).map(|k| along[k][i]).collect();
            rows.push((line.p[i], narrowed(own[i], &s), line.key[i], line.walk[i]));
            if i + 1 == n {
                break;
            }
            // Where a taper meets a layer's own offset inside the segment to i + 1: a vertex there.
            let mut cuts: Vec<f64> = Vec::new();
            for (k, t) in list.iter().enumerate() {
                let (s0, s1) = (along[k][i], along[k][i + 1]);
                if !(s0.is_finite() && s1.is_finite()) {
                    continue;
                }
                for m in 0..5 {
                    let (o0, o1) = (own[i][m], own[i + 1][m]);
                    let f = |x: f64| t.to[m] + rate[k][m] * (s0 + x * (s1 - s0)) - (o0 + x * (o1 - o0));
                    let (f0, f1) = (f(0.0), f(1.0));
                    if (f0 < 0.0) != (f1 < 0.0) && f0 != 0.0 && f1 != 0.0 {
                        cuts.push(f0 / (f0 - f1));
                    }
                }
            }
            cuts.sort_by(f64::total_cmp);
            let mut last = line.p[i];
            for x in cuts {
                let (a, b) = (line.p[i], line.p[i + 1]);
                let d = |c: usize| libm::round(x * f64::from(b[c] - a[c])) as i32;
                let q = [a[0] + d(0), a[1] + d(1)];
                // As GP2 keeps no two samples closer than half its finest step: the strokes join
                // segments of that length or more cleanly. The edge then misses the taper's end by at
                // most the rate times that.
                let gap = ctx.units(crate::graph::MIN_STEP_M / 2.0, q);
                let far = |r: P| libm::hypot(f64::from(q[0] - r[0]), f64::from(q[1] - r[1])) >= gap;
                if !far(last) || !far(b) {
                    continue;
                }
                last = q;
                let o: [f64; 5] = std::array::from_fn(|m| own[i][m] + x * (own[i + 1][m] - own[i][m]));
                let s: Vec<f64> = (0..list.len()).map(|k| along[k][i] + x * (along[k][i + 1] - along[k][i])).collect();
                rows.push((q, narrowed(o, &s), line.key[i], line.walk[i]));
            }
        }
        line.p = rows.iter().map(|r| r.0).collect();
        line.h = rows.iter().map(|r| r.1[0]).collect();
        line.bands = rows.iter().map(|r| [r.1[1], r.1[2], r.1[3], r.1[4]]).collect();
        line.key = rows.iter().map(|r| r.2).collect();
        line.walk = rows.iter().map(|r| r.3).collect();
        line.index = index;
    }
}

/// The sample of a line at a graph station, else the one nearest `pos`.
fn sample_at(g: &WayGraph, line: &WayLine, st: &Station, pos: P) -> usize {
    let way = &g.ways[line.way as usize];
    let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
    let exact = samples.iter().position(|s| {
        (s.span == st.span && (s.u - st.u).abs() < 1e-9) || (s.span == st.span + 1 && st.u >= 1.0 && s.u <= 0.0)
    });
    exact.map(|k| line.index[k]).unwrap_or_else(|| {
        let d = |p: P| (i64::from(p[0] - pos[0])).pow(2) + (i64::from(p[1] - pos[1])).pow(2);
        (0..line.p.len()).min_by_key(|&i| d(line.p[i])).unwrap_or(0)
    })
}

/// A graph point: its position, the arms of the level, the round radius in metres, and its face's
/// label and key.
struct Point {
    pos: P,
    arms: Vec<Arm>,
    /// Neighbouring arms (as indices into `arms`), the first turning counter-clockwise to the second
    /// through their sector, with the radius of their corner in metres.
    pairs: Vec<(usize, usize, f64)>,
    /// A junction or a joint: its arms belong to its face up to their seams.
    seams: bool,
    label: Label,
    key: (u8, u8, u8),
    glob: Glob,
}

fn way_label(rules: &Rules, line: &WayLine) -> (Label, u8) {
    let rank = rules.plan.rank(if line.rail { Rank::RailBed } else { Rank::Carriageway });
    let label = Label {
        owner: Owner::Way(line.way),
        surface: if line.rail { Surface::RailBed } else { Surface::Carriageway },
        rule: HeightRule::Owner,
        material: Some(line.way),
        crossing: None,
        rank,
        reach_m: line.sec.reach_m as f32,
    };
    (label, rank)
}

/// The samples where a line's stroke turns tighter than the inner radius `r[i]` (units) on its inner
/// edge: the edge at the half-width on the side the line turns to, with a concave corner at each
/// sample that turns that way. Corners with no turn the other way between them form one corner where
/// their rounds of radius r would overlap or where they lie less than `support` units apart along the
/// edge, as the kink check groups them. A corner's length along the edge is the centre line's less
/// the half-width's share of each turn: an edge that folds over a turn tighter than the half-width
/// has none. A corner whose length (at least `support`) over its summed turn is under r gets a round
/// at its sharpest sample.
fn turns(line: &WayLine, r: &[f64], support: f64) -> Vec<usize> {
    let n = line.p.len();
    let mut s = vec![0.0; n];
    for i in 1..n {
        let (a, b) = (line.p[i - 1], line.p[i]);
        let (dx, dy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
        s[i] = s[i - 1] + (dx * dx + dy * dy).sqrt();
    }
    // The signed turn at each sample, toward the left normal positive.
    let turn: Vec<f64> = (0..n)
        .map(|i| {
            let (Some(u), Some(v)) = (
                i.checked_sub(1).and_then(|h| stroke::unit(line.p[h], line.p[i])),
                line.p.get(i + 1).and_then(|&q| stroke::unit(line.p[i], q)),
            ) else {
                return 0.0;
            };
            libm::atan2(u[0] * v[1] - u[1] * v[0], u[0] * v[0] + u[1] * v[1])
        })
        .collect();
    let mut out = Vec::new();
    for side in [1.0, -1.0] {
        let t = |k: usize| (side * turn[k]).max(0.0);
        // The edge a corner takes from each side, and the reach of its round along the edge.
        let cut = |k: usize| line.h[k] * libm::tan(t(k) / 2.0);
        let reach = |k: usize| r[k] * libm::tan(t(k) / 2.0);
        let mut i = 1;
        while i + 1 < n {
            if t(i) == 0.0 {
                i += 1;
                continue;
            }
            let (mut sum, mut length, mut last, mut sharpest) = (t(i), 0.0, i, i);
            while last + 2 < n && t(last + 1) > 0.0 {
                let j = last + 1;
                let gap = s[j] - s[last] - cut(last) - cut(j);
                if gap >= support.max(reach(last) + reach(j)) {
                    break;
                }
                (sum, length, last) = (sum + t(j), length + gap, j);
                if t(j) > t(sharpest) {
                    sharpest = j;
                }
            }
            if length.max(support) / sum < r[sharpest] {
                out.push(sharpest);
            }
            i = last + 1;
        }
    }
    out.sort_unstable();
    out
}

/// Whether an arm's first segment has a sidewalk on its right, seen from the point, else on its left.
fn sidewalk(line: &WayLine, arm: Arm, right: bool) -> bool {
    let seg = if arm.dir > 0 { Some(arm.at) } else { arm.at.checked_sub(1) };
    let bit = if (arm.dir > 0) == right { 2 } else { 1 };
    seg.is_some_and(|s| s + 1 < line.walk.len() && line.walk[s] & bit != 0)
}

/// The junctions and joints whose faces reach the window.
fn graph_points(ctx: &Ctx, lines: &[WayLine], of_way: &BTreeMap<u32, usize>, bx: [i32; 4]) -> Vec<Point> {
    let g = ctx.graph;
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    let mut out = Vec::new();
    let class = |l: usize| lines[l].sec.rank as u8;
    for (id, j) in g.junctions.iter().enumerate() {
        let arms: Vec<(&crate::graph::Arm, usize)> =
            j.arms.iter().filter_map(|a| of_way.get(&a.way).map(|&l| (a, l))).collect();
        if arms.len() < 2 || !near(j.pos) {
            continue;
        }
        // The material: the arm of the best class, a road or path before a rail, a host before a guest.
        let rail = arms.iter().all(|(_, l)| lines[*l].rail);
        let best = arms.iter().min_by_key(|(a, l)| (lines[*l].rail != rail, class(*l), !a.host, a.way)).unwrap();
        let (mut label, rank) = way_label(ctx.rules, &lines[best.1]);
        label.owner = Owner::Junction(id as u32);
        label.rule = HeightRule::Blend;
        let pts: Vec<Arm> = arms
            .iter()
            .map(|(a, l)| Arm { line: *l, at: sample_at(g, &lines[*l], &a.at, j.pos), dir: a.dir })
            .collect();
        // Each two neighbouring arms, the first turning counter-clockwise to the second through their
        // sector, so that the sector lies on the first's right and the second's left. Their corner
        // takes the lesser radius of their classes, the sidewalk one where either has a sidewalk there.
        let pairs = (0..pts.len())
            .map(|k| {
                let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                let walk = sidewalk(&lines[a.line], a, true) || sidewalk(&lines[b.line], b, false);
                let radius = |l: usize| {
                    let sec = &lines[l].sec;
                    if walk {
                        sec.corner_radius_walk_m
                    } else {
                        sec.corner_radius_m
                    }
                };
                (k, (k + 1) % pts.len(), radius(a.line).min(radius(b.line)))
            })
            .collect();
        let gp = ctx.global(j.pos);
        out.push(Point {
            pos: j.pos,
            arms: pts,
            pairs,
            seams: true,
            label,
            key: (rank, class(best.1), 0),
            glob: [gp[0], gp[1], 0, 0, 0],
        });
    }
    for j in &g.joints {
        let (Some(&la), Some(&lb)) = (of_way.get(&j.arms[0].way), of_way.get(&j.arms[1].way)) else { continue };
        if !near(j.pos) || j.arms[0].level != j.arms[1].level {
            continue;
        }
        let at = [sample_at(g, &lines[la], &j.arms[0].at, j.pos), sample_at(g, &lines[lb], &j.arms[1].at, j.pos)];
        // The face takes the label of the lesser key at the joint.
        let ka = (class(la), lines[la].key[at[0]]);
        let kb = (class(lb), lines[lb].key[at[1]]);
        let owner = if ka <= kb { la } else { lb };
        let (label, rank) = way_label(ctx.rules, &lines[owner]);
        let radius = lines[la].sec.inner_radius_m.min(lines[lb].sec.inner_radius_m);
        let gp = ctx.global(j.pos);
        out.push(Point {
            pos: j.pos,
            arms: vec![
                Arm { line: la, at: at[0], dir: j.arms[0].dir },
                Arm { line: lb, at: at[1], dir: j.arms[1].dir },
            ],
            pairs: vec![(0, 1, radius), (1, 0, radius)],
            seams: true,
            label,
            key: (rank, class(owner), 1),
            glob: [gp[0], gp[1], 0, 0, 0],
        });
    }
    out
}

/// The tight turns of single ways whose faces reach the window, except at a sample on a part of a
/// junction's or joint's region (`taken`, per line): that point's rounds shape it.
fn turn_points(ctx: &Ctx, lines: &[WayLine], taken: &[Vec<(f64, f64)>], bx: [i32; 4]) -> Vec<Point> {
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    let mut out = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        let r_m = line.sec.inner_radius_m;
        if r_m <= 0.0 || line.rail {
            continue;
        }
        let r: Vec<f64> = line.p.iter().map(|&p| ctx.units(r_m, p)).collect();
        for i in turns(line, &r, ctx.units(check::SUPPORT_M, line.p[0])) {
            if !near(line.p[i]) || taken[li].iter().any(|&(lo, hi)| (lo..=hi).contains(&(i as f64))) {
                continue;
            }
            let (label, rank) = way_label(ctx.rules, line);
            out.push(Point {
                pos: line.p[i],
                arms: vec![Arm { line: li, at: i, dir: 1 }, Arm { line: li, at: i, dir: -1 }],
                pairs: vec![(0, 1, r_m)],
                seams: false,
                label,
                key: (rank, line.sec.rank as u8, 2),
                glob: {
                    let gp = ctx.global(line.p[i]);
                    [gp[0], gp[1], 0, 0, 0]
                },
            });
        }
    }
    out
}

/// A vertex less than 2 units off a cell line moves onto it: the overlay's 1-unit snap then cannot
/// bend the cell line toward a vertex beside it.
fn snap(p: P) -> P {
    let c = CELL_UNITS as i32;
    let snap = |v: i32| {
        let m = v.rem_euclid(c);
        if m != 0 && m < 2 {
            v - m
        } else if m != 0 && c - m < 2 {
            v + (c - m)
        } else {
            v
        }
    };
    [snap(p[0]), snap(p[1])]
}

/// Each ring's vertices snapped to the cell lines, with an exact vertex where it crosses one.
fn prepare(rings: &Rings) -> Rings {
    rings
        .iter()
        .map(|r| {
            let mut s: Vec<P> = r.iter().map(|&p| snap(p)).collect();
            s.dedup();
            if s.len() > 1 && s[0] == s[s.len() - 1] {
                s.pop();
            }
            with_cell_lines(&s)
        })
        .filter(|r| r.len() >= 3)
        .collect()
}

/// Crossings of a cell line closer than this many units along it are one point.
const MERGE: i32 = 4;

/// The crossings of the window's lines by candidate edges and seams, closer than `MERGE` units along
/// a line, move to one point: a cell corner in the run, else its least. Two edges that cross each other
/// beside a cell line then cross on it, so no cell holds a sliver of their wedge that the next cell
/// sees as a whole patch.
fn merge_crossings(win: [i32; 4], rings: &mut [&mut Rings], seams: &mut Vec<[P; 2]>) {
    let c = CELL_UNITS as i32;
    for axis in 0..2 {
        for line in [win[axis], win[axis + 2]] {
            let on = |p: &P| p[axis] == line;
            let mut along: Vec<i32> = (rings.iter().flat_map(|r| r.iter().flatten()))
                .chain(seams.iter().flatten())
                .filter(|p| on(p))
                .map(|p| p[1 - axis])
                .collect();
            along.sort_unstable();
            along.dedup();
            let mut to: BTreeMap<i32, i32> = BTreeMap::new();
            let mut start = 0;
            for k in 1..=along.len() {
                if k < along.len() && along[k] - along[k - 1] < MERGE {
                    continue;
                }
                let run = &along[start..k];
                if run.len() > 1 {
                    let target = run.iter().copied().find(|v| v.rem_euclid(c) == 0).unwrap_or(run[0]);
                    to.extend(run.iter().map(|&v| (v, target)));
                }
                start = k;
            }
            if to.is_empty() {
                continue;
            }
            let shift = |p: &mut P| {
                if on(p) {
                    if let Some(&v) = to.get(&p[1 - axis]) {
                        p[1 - axis] = v;
                    }
                }
            };
            for ring in rings.iter_mut().flat_map(|r| r.iter_mut()) {
                ring.iter_mut().for_each(shift);
                ring.dedup();
                while ring.len() > 1 && ring.first() == ring.last() {
                    ring.pop();
                }
            }
            seams.iter_mut().flatten().for_each(shift);
        }
    }
    for r in rings.iter_mut() {
        r.retain(|ring| ring.len() >= 3);
    }
    seams.retain(|s| s[0] != s[1]);
}

/// The lines of a cell's level whose faces reach its window, as `build` reads them.
pub(crate) fn level_lines<'a>(
    graph: &'a WayGraph,
    input: &CellInput,
    rules: &'a Rules,
    cell: CellKey,
    level: i8,
) -> Vec<WayLine<'a>> {
    let ctx = Ctx { graph, rules, origin: cell.origin() };
    lines(&ctx, input, level, window(cell, input_m(&rules.plan, &rules.sections))).0
}

/// The face plan of one cell and level. `slopes = false` leaves out the slope layer (decks).
pub fn build(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, level: i8, slopes: bool) -> FacePlan {
    let ctx = Ctx { graph, rules, origin: cell.origin() };
    let plan = &rules.plan;
    let win = window(cell, 0.0);
    let bx = window(cell, input_m(plan, &rules.sections));
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    // The carriageways reach farther than the points: the outline rounds read them. The ways reach
    // farther still: the off-line test of the points' rounds reads them.
    let lx = window(cell, rules::lines_m(plan, &rules.sections));
    let (lines, of_way) = lines(&ctx, input, level, window(cell, rules::ways_m(plan, &rules.sections)));
    // The points within `bx` round their arms; the points past it only end their runs.
    let all = graph_points(&ctx, &lines, &of_way, window(cell, rules::points_m(plan, &rules.sections)));
    let (mut points, outer): (Vec<Point>, Vec<Point>) = all.into_iter().partition(|pt| near(pt.pos));
    let mut stats = FaceStats::default();
    stats.junctions = points.iter().filter(|pt| matches!(pt.label.owner, Owner::Junction(_))).count() as u32;
    stats.joints = points.len() as u32 - stats.junctions;
    // A junction's or joint's arms end at the next junction or joint on their way, a turn's arms at
    // the next point. A turn on a part of a junction's or joint's region is none.
    let stops = |points: &[Point]| {
        let mut stops: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
        for a in points.iter().chain(&outer).flat_map(|pt| &pt.arms) {
            stops[a.line].push(a.at);
        }
        for s in &mut stops {
            s.sort_unstable();
            s.dedup();
        }
        stops
    };
    let round = |pt: &Point, stops: &[Vec<usize>]| {
        let u = |m: f64| ctx.units(m, pt.pos);
        // A round may cut into its arms' strokes as far as their chords leave the curve.
        let sag = |k: usize| lines[pt.arms[k].line].sec.sag_plan_m;
        let pairs: Vec<(usize, usize, f64, f64)> =
            pt.pairs.iter().map(|&(a, b, r)| (a, b, u(r), u(sag(a).max(sag(b))))).collect();
        points::round(&lines, stops, pt.pos, &pt.arms, &pairs, u(plan.extent_m), pt.seams)
    };
    let jj = stops(&points);
    let mut rounded: Vec<Rounded> = points.iter().map(|pt| round(pt, &jj)).collect();
    // Two junctions or joints whose seams on the link between them would overlap share one seam.
    let mut by_arm: BTreeMap<(usize, usize, i8), (usize, usize)> = BTreeMap::new();
    for (i, pt) in points.iter().enumerate() {
        for (k, a) in pt.arms.iter().enumerate() {
            by_arm.insert((a.line, a.at, a.dir), (i, k));
        }
    }
    // Per point and arm: the seam it shares and the point across it.
    let mut shared: Vec<Vec<Option<(points::Seam, usize)>>> =
        points.iter().map(|pt| vec![None; pt.arms.len()]).collect();
    for (&(l, at, dir), &(pa, ka)) in &by_arm {
        let Some(&b) = jj[l].iter().find(|&&s| s > at) else { continue };
        let Some(&(pb, kb)) = by_arm.get(&(l, b, -1)).filter(|_| dir == 1) else { continue };
        let seam = ctx.units(plan.seam_m, lines[l].p[at]);
        let far = [rounded[pa].far[ka], rounded[pb].far[kb]];
        if let Some(sm) = points::link_seam(&lines[l], at, b, far, seam) {
            shared[pa][ka] = Some((sm, pb));
            shared[pb][kb] = Some((sm, pa));
        }
    }
    for ((pt, r), shared) in points.iter().zip(&mut rounded).zip(&shared) {
        let seams: Vec<Option<points::Seam>> = shared.iter().map(|s| s.map(|(sm, _)| sm)).collect();
        points::region(pt.pos, &pt.arms, r, ctx.units(plan.seam_m, pt.pos), &seams);
    }
    let mut taken: Vec<Vec<(f64, f64)>> = vec![Vec::new(); lines.len()];
    for (pt, r) in points.iter().zip(&rounded) {
        for a in &pt.arms {
            taken[a.line].push((a.at as f64, a.at as f64));
        }
        for part in &r.spans {
            taken[part.line].push((part.lo, part.hi));
        }
    }
    // The cuts of a line's square end at a junction or joint: the corners of all its arms' layers.
    let mut end_cuts: BTreeMap<(usize, usize), Vec<f64>> = BTreeMap::new();
    for pt in &points {
        let cuts = points::cuts(&lines, &pt.arms);
        for a in pt.arms.iter().filter(|a| a.at == 0 || a.at + 1 == lines[a.line].p.len()) {
            end_cuts.insert((a.line, a.at), cuts.clone());
        }
    }
    let first_turn = points.len();
    points.extend(turn_points(&ctx, &lines, &taken, bx));
    stats.turns = (points.len() - first_turn) as u32;
    let all = stops(&points);
    rounded.extend(points[first_turn..].iter().map(|pt| round(pt, &all)));
    // Past a seam, the fans that reach across it belong to the face on its other side: the fans of a
    // point past the seam it shares, to the point across it; the fans of the next turn on the line,
    // to the seam's point. Each cut grows to split those fans, and the slice cuts them along it.
    let mut beyond: Vec<Vec<([P; 2], Label)>> = vec![Vec::new(); points.len()];
    let mut seam_points: BTreeMap<(usize, [P; 2]), Vec<usize>> = BTreeMap::new();
    for i in 0..first_turn {
        for (k, sh) in shared[i].iter().enumerate() {
            let Some(cut) = rounded[i].seams[k] else { continue };
            let other = sh.map(|(_, j)| j);
            if other.is_some_and(|j| j < i) {
                continue;
            }
            let l = points[i].arms[k].line;
            // The next turn past the seam, before any other point on the line.
            let end = rounded[i].spans.iter().find_map(|p| match p.cuts {
                [Some(c), _] if p.line == l && c == cut => Some((p.lo, -1.0)),
                [_, Some(c)] if p.line == l && c == cut => Some((p.hi, 1.0)),
                _ => None,
            });
            let next = end.filter(|_| other.is_none()).and_then(|(x, dir)| {
                let ahead = |at: usize| (at as f64 - x) * dir;
                let stop = jj[l].iter().map(|&s| ahead(s)).filter(|&d| d > 0.0).fold(f64::INFINITY, f64::min);
                (first_turn..points.len())
                    .filter(|&t| points[t].arms[0].line == l && ahead(points[t].arms[0].at) > 0.0)
                    .filter(|&t| ahead(points[t].arms[0].at) < stop)
                    .min_by(|&a, &b| ahead(points[a].arms[0].at).total_cmp(&ahead(points[b].arms[0].at)))
            });
            let far: Vec<usize> = other.into_iter().chain(next).collect();
            let grown = {
                let own = other.map(|_| (rounded[i].fans.as_slice(), 1.0));
                let fans: Vec<(&[Vec<P>], f64)> =
                    own.into_iter().chain(far.iter().map(|&j| (rounded[j].fans.as_slice(), -1.0))).collect();
                points::grow(cut, &fans)
            };
            for &j in std::iter::once(&i).chain(&far) {
                points::replace_cut(&mut rounded[j], cut, grown);
            }
            if let Some(j) = other {
                beyond[i].push((grown, points[j].label));
            }
            for &j in &far {
                beyond[j].push(([grown[1], grown[0]], points[i].label));
            }
            let key = (l, [snap(grown[0].min(grown[1])), snap(grown[0].max(grown[1]))]);
            seam_points.insert(key, std::iter::once(i).chain(far).collect());
        }
    }
    let mut cands: Vec<Cand> = Vec::new();
    // The ways' carriageways and the band layers, on the run of samples near the window.
    let mut layers: [Vec<Vec<P>>; 4] = Default::default();
    // The carriageways on the runs that only the outline rounds read.
    let mut outer_paved: Vec<Vec<P>> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        // The samples of the segments that meet a box.
        let run = |bx: [i32; 4]| {
            let meets = |i: &usize| geom::meets(bx, line.p[*i], line.p[i + 1]);
            let n = line.p.len() - 1;
            Some(((0..n).find(meets)?, (0..n).rfind(meets)? + 2))
        };
        // The cuts at the first and the last sample of layer k (0 the carriageway).
        let cut = |i: usize, k: usize| end_cuts.get(&(li, i)).cloned().unwrap_or_else(|| line.offsets(i)[..k].to_vec());
        let carriageway = |(first, last): (usize, usize)| {
            let ends = [line.round[0] && first == 0, line.round[1] && last == line.p.len()];
            let mut own = Vec::new();
            let (p, h) = (&line.p[first..last], &line.h[first..last]);
            pieces_along(p, h, &[], [&cut(first, 0), &cut(last - 1, 0)], ends, false, &mut own);
            own
        };
        let outer = run(lx);
        let Some((first, last)) = run(bx) else {
            outer_paved.extend(outer.map(|r| prepare(&union(&carriageway(r), [0, 0]))).unwrap_or_default());
            continue;
        };
        if outer != Some((first, last)) {
            outer_paved.extend(outer.map(|r| prepare(&union(&carriageway(r), [0, 0]))).unwrap_or_default());
        }
        let ends = [line.round[0] && first == 0, line.round[1] && last == line.p.len()];
        let p = &line.p[first..last];
        let own = carriageway((first, last));
        let (label, rank) = way_label(rules, line);
        cands.push(Cand {
            rings: union(&own, [0, 0]),
            label,
            key: (rank, line.sec.rank as u8, 3),
            glob: None,
            line: li,
            relabel: None,
            beyond: Vec::new(),
            fill: None,
        });
        for (k, layer) in layers.iter_mut().enumerate() {
            if k == 3 && !slopes {
                continue;
            }
            let inner = |i: usize| if k == 0 { line.h[i] } else { line.bands[i][k - 1] };
            let d: Vec<f64> = (first..last).map(|i| line.bands[i][k]).collect();
            if (first..last).all(|i| line.bands[i][k] <= inner(i)) {
                continue;
            }
            let cuts = [&cut(first, k + 1)[..], &cut(last - 1, k + 1)[..]];
            pieces_along(p, &d, &[], cuts, ends, false, layer);
        }
    }
    stats.ways = lines.len() as u32;
    if level == 0 {
        cands.extend(platform::candidates(&ctx, input, bx, &mut stats));
    }
    // The points' fans and regions, and the band fans.
    let mut fan_of: Vec<Option<usize>> = vec![None; points.len()];
    for (i, (pt, r)) in points.iter().zip(&rounded).enumerate() {
        let glob = Some(pt.glob);
        if !r.fans.is_empty() {
            fan_of[i] = Some(cands.len());
            cands.push(Cand {
                rings: union(&r.fans, pt.pos),
                label: pt.label,
                // A round paves land only: its fan loses to every carriageway and rail bed, so it
                // never lies over another way, and its arms' carriageways reach the point's face
                // through its region.
                key: (plan.rank(Rank::RailBed), pt.key.1, pt.key.2),
                glob,
                line: 0,
                relabel: None,
                beyond: std::mem::take(&mut beyond[i]),
                fill: None,
            });
        }
        if !r.region.is_empty() {
            let rings = union(&r.region, pt.pos);
            let spans = Some(r.spans.clone());
            let beyond = Vec::new();
            cands.push(Cand { rings, label: pt.label, key: pt.key, glob, line: 0, relabel: spans, beyond, fill: None });
        }
        for (k, layer) in layers.iter_mut().enumerate() {
            if k < 3 || slopes {
                layer.extend(r.rings[k].iter().cloned());
            }
        }
    }
    let band_labels = [
        (Rank::ShoulderPaved, Surface::Shoulder, HeightRule::Owner),
        (Rank::Shoulder, Surface::Shoulder, HeightRule::Owner),
        (Rank::Verge, Surface::Verge, HeightRule::Slope),
        (Rank::Slope, Surface::Slope, HeightRule::Slope),
    ];
    for (k, layer) in layers.iter().enumerate() {
        if layer.is_empty() {
            continue;
        }
        let (rank, surface, rule) = band_labels[k];
        let rank = plan.rank(rank);
        let label = Label { owner: Owner::Terrain, surface, rule, material: None, crossing: None, rank, reach_m: 0.0 };
        cands.push(Cand {
            rings: union(layer, [0, 0]),
            label,
            key: (rank, 0, 0),
            glob: Some([0; 5]),
            line: 0,
            relabel: None,
            beyond: Vec::new(),
            fill: None,
        });
    }
    for c in &mut cands {
        c.rings = prepare(&c.rings);
    }
    // The rounds at the corners of the paved outline that no point's round shapes. Their key loses
    // to every carriageway and rail bed, so a round paves land only. `lines_m` holds their reach.
    let mut paved: Vec<Vec<P>> = (cands.iter())
        .filter(|c| c.relabel.is_none() && matches!(c.label.surface, Surface::Carriageway | Surface::RailBed))
        .flat_map(|c| c.rings.iter().cloned())
        .collect();
    paved.extend(outer_paved);
    let circles: Vec<(P, f64)> = rounded.iter().flat_map(|r| r.circles.iter().map(|c| (c.centre, c.radius))).collect();
    let r_max = rules::inner_max_m(&rules.sections);
    let beyond = |p: P| {
        let d = (win[0] - p[0]).max(p[0] - win[2]).max(win[1] - p[1]).max(p[1] - win[3]).max(0);
        f64::from(d) * metres_per_unit(ctx.origin.y + i64::from(p[1]))
    };
    let units = |m: f64, p: P| ctx.units(m, p);
    let fills = outline::rounds(&union(&paved, [0, 0]), &lines, &circles, &beyond, &units, plan.extent_m / 2.0, r_max);
    let fill_key = (plan.rank(Rank::RailBed), u8::MAX, u8::MAX);
    for (k, &(rank, surface, rule)) in band_labels.iter().enumerate() {
        let rings: Vec<Vec<P>> = fills.iter().flat_map(|f| f.bands[k].iter().cloned()).collect();
        if rings.is_empty() || (k == 3 && !slopes) {
            continue;
        }
        let rank = plan.rank(rank);
        let label = Label { owner: Owner::Terrain, surface, rule, material: None, crossing: None, rank, reach_m: 0.0 };
        let (rings, glob, beyond) = (prepare(&rings), Some([0; 5]), Vec::new());
        cands.push(Cand { rings, label, key: (rank, 0, 0), glob, line: 0, relabel: None, beyond, fill: None });
    }
    for f in &fills {
        let gc = ctx.global(f.circle.centre);
        cands.push(Cand {
            rings: prepare(&vec![f.ring.clone()]),
            label: TERRAIN,
            key: fill_key,
            glob: Some([gc[0], gc[1], 0, 0, 0]),
            line: 0,
            relabel: None,
            beyond: Vec::new(),
            fill: Some(f.inside),
        });
    }
    // Each seam cuts its line's carriageway and the fans that reach across it.
    let mut seams: Vec<(usize, [P; 2], Vec<usize>)> =
        seam_points.into_iter().map(|((l, s), pts)| (l, s, pts.iter().filter_map(|&p| fan_of[p]).collect())).collect();
    // A seam off the window cuts nothing of it.
    seams.retain(|&(_, [a, b], _)| geom::meets(win, a, b));
    let seams = seams_on_carriageways(&mut cands, &seams);
    // A seam meets the cell lines where the carriageway's ring does.
    let seams: Vec<[P; 2]> = seams
        .iter()
        .flat_map(|&[a, b]| {
            let mut pts = vec![a];
            cell_line_points(a, b, &mut pts);
            pts.push(b);
            pts.dedup();
            pts.windows(2).map(|w| [w[0], w[1]]).collect::<Vec<_>>()
        })
        .collect();
    let mut seams = seams;
    merge_crossings(win, &mut cands.iter_mut().map(|c| &mut c.rings).collect::<Vec<_>>(), &mut seams);
    cands.retain(|c| !c.rings.is_empty());
    stats.candidates = cands.len() as u32;
    let strings: Vec<Vec<P>> = cands.iter().flat_map(|c| c.rings.iter().cloned()).collect();
    let (patches, segments) = slice(win, &strings, &seams);
    stats.patches = patches.len() as u32;
    stats.slice_segments = segments as u32;
    let rings: Vec<&[Vec<P>]> = cands.iter().map(|c| c.rings.as_slice()).collect();
    let cover = Cover::new(win, &rings);
    for k in 0..cands.len() {
        if let Some(q) = cands[k].fill {
            let at: Vec<u32> = cover.at(q).into_iter().filter(|&c| cands[c as usize].fill.is_none()).collect();
            cands[k].label = decide(&cands, &lines, at, q);
        }
    }
    let labels: Vec<Label> = patches.iter().map(|patch| label(&cands, &cover, &lines, patch)).collect();
    let mut plan = output(cell, level, Map::new(snap_round(&patches)), &labels, stats);
    let mpu = |p: P| metres_per_unit(ctx.origin.y + i64::from(p[1]));
    plan.corners = (points.iter().zip(&rounded))
        .flat_map(|(pt, r)| r.circles.iter().map(|c| CornerArc::new(pt.label.owner, c, mpu(c.centre))))
        .collect();
    let fill_label = |f: &outline::Fill| cands.iter().find(|c| c.fill == Some(f.inside)).map(|c| c.label);
    for f in &fills {
        if let Some(l) = fill_label(f).filter(|l| l.surface != Surface::Terrain) {
            plan.corners.push(CornerArc::new(l.owner, &f.circle, mpu(f.circle.centre)));
        }
    }
    plan
}

/// The global name of a way near a point (doubled): the key of its sample nearest the point.
fn way_glob(line: &WayLine, q: [i64; 2]) -> Glob {
    let d = |p: P| (2 * i64::from(p[0]) - q[0]).pow(2) + (2 * i64::from(p[1]) - q[1]).pow(2);
    let i = (0..line.p.len()).min_by_key(|&i| (d(line.p[i]), i)).unwrap_or(0);
    line.key[i]
}

/// A crossing of a seam's line by a ring: its fraction along the seam, the candidate (as an index
/// into the seam's list), ring and segment, its place along the segment in millionths, and +1 where
/// the line enters the candidate, −1 where it leaves.
type Crossing = (f64, usize, usize, usize, i64, i32);

/// Each seam as a cut of the paved outline it splits: its way's carriageway and the fans that reach
/// across it, from edge to edge of their union about the seam's middle. Each crossing of a ring on
/// the way becomes a vertex of that ring, so the cut ends on an edge of the slice: past it a cut
/// would only split one face and leave a slit, and an edge that a cut crosses between its vertices
/// would bend by a unit there. A seam along an edge of the carriageway (a square end of the way) is
/// that edge already. A seam whose union runs past an end keeps that end.
fn seams_on_carriageways(cands: &mut [Cand], seams: &[(usize, [P; 2], Vec<usize>)]) -> Vec<[P; 2]> {
    // Per (candidate, ring, segment): the points to insert, by their place along the segment.
    let mut inserts: BTreeMap<(usize, usize, usize), Vec<(i64, P)>> = BTreeMap::new();
    let mut out = Vec::with_capacity(seams.len());
    for (l, [a, b], fans) in seams {
        let (a, b) = (*a, *b);
        let Some(ci) = cands.iter().position(|c| c.relabel.is_none() && c.glob.is_none() && c.line == *l) else {
            out.push([a, b]);
            continue;
        };
        let (fa, fb) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
        let r = [fb[0] - fa[0], fb[1] - fa[1]];
        let len = (r[0] * r[0] + r[1] * r[1]).sqrt();
        if len == 0.0 {
            continue;
        }
        let off = |x: [f64; 2]| ((x[0] - fa[0]) * r[1] - (x[1] - fa[1]) * r[0]).abs() / len;
        let along = |x: [f64; 2]| ((x[0] - fa[0]) * r[0] + (x[1] - fa[1]) * r[1]) / (len * len);
        let on_edge = cands[ci].rings.iter().any(|ring| {
            (0..ring.len()).any(|si| {
                let (p, q) = (ring[si], ring[(si + 1) % ring.len()]);
                let (fp, fq) = ([f64::from(p[0]), f64::from(p[1])], [f64::from(q[0]), f64::from(q[1])]);
                off(fp) <= 1.0 && off(fq) <= 1.0 && along(fp).max(along(fq)) > 0.0 && along(fp).min(along(fq)) < 1.0
            })
        });
        if on_edge {
            continue;
        }
        // The crossings of the whole line, so the windings start at zero; a vertex on the line lies
        // on its left.
        let set: Vec<usize> = std::iter::once(ci).chain(fans.iter().copied()).collect();
        let left = |p: P| geom::cross(a, b, p) >= 0;
        let mut xs: Vec<Crossing> = Vec::new();
        for (slot, &c) in set.iter().enumerate() {
            for (ri, ring) in cands[c].rings.iter().enumerate() {
                for (si, &p) in ring.iter().enumerate() {
                    let q = ring[(si + 1) % ring.len()];
                    if left(p) == left(q) {
                        continue;
                    }
                    let d = [f64::from(q[0] - p[0]), f64::from(q[1] - p[1])];
                    let den = r[0] * d[1] - r[1] * d[0];
                    let (x, y) = (f64::from(p[0]) - fa[0], f64::from(p[1]) - fa[1]);
                    let (t, u) = ((x * d[1] - y * d[0]) / den, (x * r[1] - y * r[0]) / den);
                    // Rings turn counter-clockwise: the line enters where it crosses an edge from
                    // the edge's right.
                    let sign = if den < 0.0 { 1 } else { -1 };
                    xs.push((t, slot, ri, si, geom::round(u.clamp(0.0, 1.0) * 1e6), sign));
                }
            }
        }
        xs.sort_by(|x, y| x.0.total_cmp(&y.0).then((x.1, x.2, x.3).cmp(&(y.1, y.2, y.3))));
        // Crossings within half a unit along the line are one: per group, its crossings and the
        // count of candidates that cover the line after it.
        let mut groups: Vec<(usize, usize, usize)> = Vec::new();
        let mut wind = vec![0i32; set.len()];
        let mut start = 0;
        for k in 1..=xs.len() {
            if k < xs.len() && (xs[k].0 - xs[start].0) * len < 0.5 {
                continue;
            }
            for x in &xs[start..k] {
                wind[x.1] += x.5;
            }
            groups.push((start, k, wind.iter().filter(|&&w| w != 0).count()));
            start = k;
        }
        let Some(mid) = groups.iter().rposition(|g| xs[g.0].0 <= 0.5).filter(|&g| groups[g].2 > 0) else {
            out.push([a, b]);
            continue;
        };
        let mut lo = mid;
        while lo > 0 && groups[lo - 1].2 > 0 {
            lo -= 1;
        }
        let hi = (mid + 1..groups.len()).find(|&g| groups[g].2 == 0);
        // The seam runs through one vertex per group of crossings in the union, each a vertex of
        // every ring of its group: rings that run together across the seam (two fans' sides on one
        // centre line) then meet it at one point, not a unit apart.
        let mut pts = Vec::new();
        if xs[groups[lo].0].0 < 0.0 {
            pts.push(a);
        }
        for &(i, j, _) in &groups[lo..=hi.unwrap_or(groups.len() - 1)] {
            let t = xs[i].0;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            // The crossing point; within 2 units of an end of its segment it is that end: a point
            // inserted there could fold the ring back.
            let (_, slot, ri, si, _, _) = xs[i];
            let ring = &cands[set[slot]].rings[ri];
            let (p, q) = (ring[si], ring[(si + 1) % ring.len()]);
            let mut at = [geom::round(fa[0] + t * r[0]) as i32, geom::round(fa[1] + t * r[1]) as i32];
            let d2 = |x: P, y: P| (i64::from(x[0] - y[0])).pow(2) + (i64::from(x[1] - y[1])).pow(2);
            if d2(at, p) <= 4 {
                at = p;
            } else if d2(at, q) <= 4 {
                at = q;
            }
            pts.push(at);
            for x in &xs[i..j] {
                inserts.entry((set[x.1], x.2, x.3)).or_default().push((x.4, at));
            }
        }
        if hi.is_none_or(|g| xs[groups[g].0].0 > 1.0) {
            pts.push(b);
        }
        pts.dedup();
        out.extend(pts.windows(2).map(|w| [w[0], w[1]]));
    }
    // From the last segment of a ring back, so the earlier indices stay valid.
    for ((ci, ri, si), mut pts) in inserts.into_iter().rev() {
        pts.sort_unstable();
        let ring = &mut cands[ci].rings[ri];
        let (p, q) = (ring[si], ring[(si + 1) % ring.len()]);
        let mut add: Vec<P> = pts.into_iter().map(|(_, x)| x).filter(|&x| x != p && x != q).collect();
        add.dedup();
        ring.splice(si + 1..si + 1, add);
    }
    out.retain(|s| s[0] != s[1]);
    out
}

/// Whether a doubled point lies past a seam: on the far side of its cut line, exactly as the slice
/// holds it, and within its length. The seam is an edge of the slice, so every patch lies on one
/// side of it.
fn past(cut: &[P; 2], q: [i64; 2]) -> bool {
    let (a, b) = (snap(cut[0]), snap(cut[1]));
    let (a, b) = ([2 * i128::from(a[0]), 2 * i128::from(a[1])], [2 * i128::from(b[0]), 2 * i128::from(b[1])]);
    let (u, v) = ([b[0] - a[0], b[1] - a[1]], [i128::from(q[0]) - a[0], i128::from(q[1]) - a[1]]);
    let along = u[0] * v[0] + u[1] * v[1];
    u[0] * v[1] - u[1] * v[0] < 0 && (0..=u[0] * u[0] + u[1] * u[1]).contains(&along)
}

/// Whether a doubled point lies past one of a part's seams.
fn past_seam(part: &Part, q: [i64; 2]) -> bool {
    part.cuts.iter().flatten().any(|cut| past(cut, q))
}

/// The label of a patch: the least key among the candidates that cover its inner point, else
/// terrain; a carriageway over a rail bed is a crossing.
fn label(cands: &[Cand], cover: &Cover, lines: &[WayLine], patch: &[Vec<P>]) -> Label {
    let at = match inner_point(patch) {
        Some(q) => cover.at(q),
        None => {
            let mut all: Vec<u32> =
                patch[0].iter().flat_map(|p| cover.at([2 * i64::from(p[0]), 2 * i64::from(p[1])])).collect();
            all.sort_unstable();
            all.dedup();
            all
        }
    };
    // A patch with no inner point reads its seams at the mean of its vertices: a vertex of it can
    // lie on a seam.
    let q = inner_point(patch).unwrap_or_else(|| {
        let n = patch[0].len() as i64;
        let sum = patch[0].iter().fold([0i64; 2], |s, p| [s[0] + i64::from(p[0]), s[1] + i64::from(p[1])]);
        [(2 * sum[0]).div_euclid(n), (2 * sum[1]).div_euclid(n)]
    });
    let label = decide(cands, lines, at.clone(), q);
    if label.surface != Surface::Crossing {
        return label;
    }
    // A carriageway over a rail bed is a crossing where the rail runs through it: its centre line
    // enters the patch, or a carriageway covers the centre line's point nearest the patch (a rail in
    // a street). Else the two ways only run side by side, and the bed wins.
    let bed = (at.iter().map(|&c| &cands[c as usize]))
        .filter(|c| c.label.surface == Surface::RailBed && matches!(c.label.owner, Owner::Way(_)))
        .min_by_key(|c| c.key);
    let Some(bed) = bed else { return label };
    let rail = &lines[bed.line].p;
    let ring = &patch[0];
    let (lo, hi) = ring.iter().fold(([i32::MAX; 2], [i32::MIN; 2]), |(lo, hi), p| {
        ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
    });
    let meets = |a: P, b: P| {
        a[0].max(b[0]) >= lo[0] && a[0].min(b[0]) <= hi[0] && a[1].max(b[1]) >= lo[1] && a[1].min(b[1]) <= hi[1]
    };
    let crosses = |a: P, b: P, c: P, d: P| {
        let s = |x: i128| x.signum();
        s(geom::cross(a, b, c)) * s(geom::cross(a, b, d)) < 0 && s(geom::cross(c, d, a)) * s(geom::cross(c, d, b)) < 0
    };
    let enters = rail.windows(2).filter(|w| meets(w[0], w[1])).any(|w| {
        geom::winding(patch, [2 * i64::from(w[0][0]), 2 * i64::from(w[0][1])]) != 0
            || (0..ring.len()).any(|i| crosses(w[0], w[1], ring[i], ring[(i + 1) % ring.len()]))
    });
    let nearest = rail.windows(2).map(|w| geom::nearest_on(w[0], w[1], q)).min_by_key(|&(d, _)| d).map(|(_, c)| c);
    let street = nearest.is_some_and(|c| {
        cover.at(c).iter().any(|&k| {
            let k = &cands[k as usize];
            k.relabel.is_none() && k.label.surface == Surface::Carriageway
        })
    });
    if enters || street {
        label
    } else {
        bed.label
    }
}

/// The label at a doubled point q that the candidates `at` cover.
fn decide(cands: &[Cand], lines: &[WayLine], mut at: Vec<u32>, q: [i64; 2]) -> Label {
    let regions: Vec<u32> = at.iter().copied().filter(|&c| cands[c as usize].relabel.is_some()).collect();
    at.retain(|&c| cands[c as usize].relabel.is_none());
    let Some(&first) = at.iter().min_by_key(|&&c| cands[c as usize].key) else { return TERRAIN };
    let best = cands[first as usize].key;
    at.retain(|&c| cands[c as usize].key == best || cands[c as usize].label.surface == Surface::RailBed);
    let glob = |c: &Cand| c.glob.unwrap_or_else(|| way_glob(&lines[c.line], q));
    let win = (at.iter().map(|&c| &cands[c as usize])).filter(|c| c.key == best).min_by_key(|c| glob(c)).unwrap();
    // A round at a corner of the paved outline keeps the label decided at its corner, whole.
    if win.fill.is_some() {
        return win.label;
    }
    let mut label = win.label;
    let across = win.beyond.iter().find(|(cut, _)| past(cut, q));
    if let Some(&(_, other)) = across {
        label = other;
    }
    // A patch of a way of the point in its region takes the point's label, unless it lies past one
    // of the way's seams.
    if let (Owner::Way(w), None) = (label.owner, across) {
        let of = (regions.iter().map(|&c| &cands[c as usize]))
            .filter(|c| {
                let mut parts = c.relabel.iter().flatten().filter(|part| lines[part.line].way == w).peekable();
                parts.peek().is_some() && parts.all(|part| !past_seam(part, q))
            })
            .min_by_key(|c| (c.key, glob(c)));
        if let Some(region) = of {
            label = region.label;
        }
    }
    if label.surface == Surface::Carriageway {
        if let Some(rail) = at.iter().map(|&c| &cands[c as usize]).find(|c| c.label.surface == Surface::RailBed) {
            label.surface = Surface::Crossing;
            label.crossing = rail.label.material;
        }
    }
    label
}

/// The faces from the labelled patches: patches of one label that share an edge, not across a cell
/// line, form a face.
fn output(cell: CellKey, level: i8, map: Map, labels: &[Label], mut stats: FaceStats) -> FacePlan {
    let mut ids: Vec<Label> = Vec::new();
    let key: Vec<u32> = labels
        .iter()
        .map(|l| match ids.iter().position(|x| x == l) {
            Some(i) => i as u32,
            None => {
                ids.push(*l);
                ids.len() as u32 - 1
            }
        })
        .collect();
    let groups = map.groups(&key, false);
    // One face per outer loop of a group; a hole belongs to the least outer loop of its group that
    // holds it. A group splits where the precision collapses a part of it to a point or a line.
    let mut by_group: BTreeMap<u32, (Vec<Loop>, Vec<Loop>)> = BTreeMap::new();
    for lp in map.loops(&groups) {
        let entry = by_group.entry(lp.0).or_default();
        if lp.2 > 0 {
            entry.0.push(lp);
        } else {
            entry.1.push(lp);
        }
    }
    let points = |lp: &Loop| -> Vec<P> { lp.1.iter().map(|&v| map.point(v)).collect() };
    let mut shapes: Vec<(u32, Loop, Vec<Loop>)> = Vec::new();
    for (g, (outers, holes)) in by_group {
        let first = shapes.len();
        shapes.extend(outers.into_iter().map(|o| (g, o, Vec::new())));
        for h in holes {
            let (a, b) = (map.point(h.1[0]), map.point(h.1[1]));
            let mid = [i64::from(a[0]) + i64::from(b[0]), i64::from(a[1]) + i64::from(b[1])];
            let home = (first..shapes.len())
                .filter(|&k| shapes.len() == first + 1 || geom::winding(&[points(&shapes[k].1)], mid) != 0)
                .min_by_key(|&k| shapes[k].1 .2);
            if let Some(k) = home {
                shapes[k].2.push(h);
            }
        }
    }
    let mut used: Vec<u32> =
        shapes.iter().flat_map(|(_, o, hs)| o.1.iter().chain(hs.iter().flat_map(|h| &h.1)).copied()).collect();
    used.sort_unstable();
    used.dedup();
    let index = |v: u32| used.binary_search(&v).unwrap() as u32;
    let c = CELL_UNITS as i32;
    let mut faces: Vec<(Vec<u32>, Face)> = shapes
        .into_iter()
        .map(|(g, outer, holes)| {
            let l = labels[g as usize];
            let ring = |lp: &[u32]| {
                let mut r: Vec<u32> = lp.iter().map(|&v| index(v)).collect();
                let start = (0..r.len()).min_by_key(|&i| r[i]).unwrap_or(0);
                r.rotate_left(start);
                r
            };
            let mut rings = vec![ring(&outer.1)];
            rings.extend(holes.iter().map(|h| ring(&h.1)));
            let inside =
                outer.1.iter().map(|&v| map.point(v)).all(|p| (0..=c).contains(&p[0]) && (0..=c).contains(&p[1]));
            let halves: Vec<u32> = outer.3.iter().chain(holes.iter().flat_map(|h| &h.3)).copied().collect();
            let face = Face {
                id: 0,
                owner: l.owner,
                surface: l.surface,
                level,
                rule: l.rule,
                material: l.material,
                crossing: l.crossing,
                rank: l.rank,
                inside,
                reach_m: l.reach_m,
                rings,
            };
            (halves, face)
        })
        .collect();
    faces.sort_by_key(|(_, f)| (f.surface, f.owner, f.rings[0].first().copied()));
    let mut face_of = vec![OUTSIDE; map.half.len()];
    for (id, (halves, f)) in faces.iter_mut().enumerate() {
        f.id = id as u32;
        for &h in halves.iter() {
            face_of[h as usize] = id as u32;
        }
    }
    let faces: Vec<Face> = faces.into_iter().map(|(_, f)| f).collect();
    let mut edges = Vec::new();
    for (h, e) in map.half.iter().enumerate() {
        if !map.bounds(&groups, h as u32) || face_of[h] == OUTSIDE {
            continue;
        }
        let (a, b) = (index(e.a), index(e.b));
        let right = if e.twin == NONE { OUTSIDE } else { face_of[e.twin as usize] };
        if right != OUTSIDE && a > b {
            continue;
        }
        let left = face_of[h];
        let (a, b, left, right) = if a < b { (a, b, left, right) } else { (b, a, right, left) };
        let kind =
            if on_cell_line(map.point(e.a), map.point(e.b)) { EdgeKind::CellBorder } else { EdgeKind::Continuous };
        edges.push(Edge { a, b, left, right, kind, flags: 0 });
    }
    edges.sort_by_key(|e| (e.a, e.b));
    stats.faces = faces.len() as u32;
    stats.vertices = used.len() as u32;
    stats.edges = edges.len() as u32;
    FacePlan {
        cell,
        level,
        vertices: used.iter().map(|&v| map.point(v)).collect(),
        faces,
        edges,
        corners: Vec::new(),
        stats,
    }
}
