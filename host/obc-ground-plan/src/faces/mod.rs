//! The faces of ways and junctions in one cell and level: integer strokes of the ways, a round per
//! pair of neighbouring arms at each graph point (a tangent circle and its fan), point regions that
//! relabel their arms up to straight seams, band layers as wider strokes, one integer slice of the
//! cell by all their outlines, and a label per patch by key. Every point of the cell lies in exactly
//! one face.

pub mod check;
mod cover;
mod geom;
pub mod heights;
pub mod legs;
mod outline;
mod platform;
mod points;
pub mod records;
pub mod rules;
mod slice;
mod stroke;
#[cfg(test)]
pub(crate) mod tests;
mod types;

use std::collections::BTreeMap;

pub use rules::{input_m, Mouth, Pair, PlanRules, Rank, Rules};
pub use types::*;

use cover::Cover;
use geom::{cell_line_points, inner_point, with_cell_lines, P};
use points::{Arm, Circle, Part, Rounded};
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
    /// The points in `p` where the line splits into legs (`legs::cuts`).
    pub cuts: Vec<usize>,
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

    /// The half-width of the core (carriageway, shoulder and verge) at each sample.
    pub fn core(&self) -> Vec<f64> {
        self.bands.iter().map(|b| b[2]).collect()
    }

    /// Twice the widest core of the way's section, its carriageway widened in full at a bend
    /// (metres): the reach of its legs (`legs::Legs`).
    pub fn leg_reach_m(&self) -> f64 {
        let s = &self.sec;
        2.0 * (s.half_m + s.widen_m + s.shoulder_m + s.verge_m)
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
    /// The legs of `material` and `crossing` (`legs`).
    legs: [u8; 2],
}

impl Label {
    fn of(f: &Face) -> Label {
        let (owner, surface, rule, material, crossing, rank) =
            (f.owner, f.surface, f.rule, f.material, f.crossing, f.rank);
        Label { owner, surface, rule, material, crossing, rank, reach_m: f.reach_m, legs: f.legs }
    }
}

const TERRAIN: Label = Label {
    owner: Owner::Terrain,
    surface: Surface::Terrain,
    rule: HeightRule::Terrain,
    material: None,
    crossing: None,
    rank: u8::MAX,
    reach_m: 0.0,
    legs: [0, 0],
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
    /// The circles whose discs the records leave out of the candidate, so that the records' arcs are
    /// true circles where the overlay's are chords.
    discs: Vec<Circle>,
    /// The stop arms' ways whose own candidates do not count where this candidate lies: it paves their
    /// junction's paved area.
    stops: Vec<u32>,
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
            cuts: Vec::new(),
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
        // Its legs, on its graph samples: two parts conflict where heights would not blend them. A
        // way of a class without a wall between its legs stays one line.
        if sec.leg_wall {
            let z: Vec<f64> = samples.iter().map(|s| s.z_m).collect();
            let feather = sec.reach_m.max(ctx.rules.plan.heights.feather_min_m);
            let dz = heights::step_limit(sec.return_grade, feather, &ctx.rules.plan.heights);
            line.cuts = legs::cuts(&line.p, &line.core(), &z, dz);
        }
        of_way.insert(way.id, out.len());
        out.push(line);
    }
    taper(ctx, &mut out, &of_way);
    for line in &mut out {
        line.cuts = line.cuts.iter().map(|&k| line.index[k]).collect();
    }
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
/// offset, so the edge turns there and at the joint only. At a junction, a guest tapers so into the
/// largest offset of each layer of the other arms; the hosts keep their widths.
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
    // A guest's square end at the point would stand out of a narrower host's far edge.
    for j in &g.junctions {
        let arms: Vec<(&crate::graph::Arm, usize)> =
            j.arms.iter().filter_map(|a| of_way.get(&a.way).map(|&l| (a, l))).collect();
        if arms.len() < j.arms.len() || arms.iter().any(|(a, _)| a.level != arms[0].0.level) {
            continue;
        }
        let at: Vec<usize> = arms.iter().map(|(a, l)| sample_at(g, &lines[*l], &a.at, j.pos)).collect();
        let o: Vec<[f64; 5]> = arms.iter().zip(&at).map(|((_, l), &i)| lines[*l].offsets(i)).collect();
        for (k, (a, l)) in arms.iter().enumerate().filter(|(_, (a, _))| !a.host) {
            let to: [f64; 5] =
                std::array::from_fn(|m| (0..o.len()).filter(|&i| i != k).map(|i| o[i][m]).fold(0.0, f64::max));
            if (0..5).any(|m| o[k][m] - to[m] >= 1.0) {
                tapers.entry(*l).or_default().push(Taper { at: at[k], dir: a.dir, to });
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
    /// The stop arms of a junction, with their corner radius in metres and their host's line where
    /// the pair blends (else `usize::MAX`): guests whose pair with the host has the mouth `stop`. They
    /// are no arms of its rounds and its region: each ends on the junction's paved edge, with its own
    /// rounds there in its own label.
    guests: Vec<(Arm, f64, usize)>,
    label: Label,
    key: (u8, u8, u8),
    glob: Glob,
}

fn way_label(rules: &Rules, line: &WayLine, leg: u8) -> (Label, u8) {
    let rank = rules.plan.rank(if line.rail { Rank::RailBed } else { Rank::Carriageway });
    let label = Label {
        owner: Owner::Way(line.way),
        surface: if line.rail { Surface::RailBed } else { Surface::Carriageway },
        rule: HeightRule::Owner,
        material: Some(line.way),
        crossing: None,
        rank,
        reach_m: line.sec.reach_m as f32,
        legs: [leg, 0],
    };
    (label, rank)
}

/// The label of a stop arm's mouth (its rounds and its carriageway up to their seam): a corner fill in
/// the arm's material, which the viewer paints whole, with no edge of its own.
fn mouth_label(rules: &Rules, line: &WayLine, arm: Arm) -> Label {
    let (label, _) = way_label(rules, line, legs::leg(&line.cuts, arm.at, arm.dir));
    Label { surface: Surface::CornerFill, ..label }
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
        // A guest stops where its pair with the host of the best class has the mouth `stop`.
        let host = arms.iter().filter(|(a, _)| a.host).min_by_key(|(a, l)| (class(*l), a.way)).map(|x| x.1);
        let pair = |l: usize, h: usize| ctx.rules.plan.pair(lines[h].sec.group, lines[l].sec.group);
        let stops =
            |(a, l): &(&crate::graph::Arm, usize)| !a.host && host.is_some_and(|h| pair(*l, h).mouth == Mouth::Stop);
        let (arms, stopped): (Vec<_>, Vec<_>) = arms.into_iter().partition(|x| !stops(x));
        // The material: the arm of the best class, a road or path before a rail, a host before a guest.
        let rail = arms.iter().all(|(_, l)| lines[*l].rail);
        let best = arms.iter().min_by_key(|(a, l)| (lines[*l].rail != rail, class(*l), !a.host, a.way)).unwrap();
        let at = sample_at(g, &lines[best.1], &best.0.at, j.pos);
        let (mut label, rank) = way_label(ctx.rules, &lines[best.1], legs::leg(&lines[best.1].cuts, at, best.0.dir));
        label.owner = Owner::Junction(id as u32);
        label.rule = HeightRule::Blend;
        let arm = |(a, l): &(&crate::graph::Arm, usize)| Arm {
            line: *l,
            at: sample_at(g, &lines[*l], &a.at, j.pos),
            dir: a.dir,
        };
        let pts: Vec<Arm> = arms.iter().map(arm).collect();
        let guests = (stopped.iter())
            .map(|x| {
                let radius = lines[x.1].sec.corner_radius_m.min(lines[best.1].sec.corner_radius_m);
                let relate = host.filter(|&h| pair(x.1, h).blend).unwrap_or(usize::MAX);
                (arm(x), radius, relate)
            })
            .collect();
        // Each two neighbouring arms, the first turning counter-clockwise to the second through their
        // sector, so that the sector lies on the first's right and the second's left. Their corner
        // takes the lesser radius of their classes, the sidewalk one where either has a sidewalk there.
        let pairs = (0..pts.len())
            .filter(|_| pts.len() > 1)
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
            guests,
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
        let (owner, k) = if ka <= kb { (la, 0) } else { (lb, 1) };
        let (label, rank) = way_label(ctx.rules, &lines[owner], legs::leg(&lines[owner].cuts, at[k], j.arms[k].dir));
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
            guests: Vec::new(),
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
            let (label, rank) = way_label(ctx.rules, line, legs::leg(&line.cuts, i, -1));
            out.push(Point {
                pos: line.p[i],
                arms: vec![Arm { line: li, at: i, dir: 1 }, Arm { line: li, at: i, dir: -1 }],
                pairs: vec![(0, 1, r_m)],
                seams: false,
                guests: Vec::new(),
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

/// The crossings of the cell's lines by candidate edges and seams, closer than `MERGE` units along
/// a line, move to one point: a cell corner in the run, else its least. Two edges that cross each other
/// beside a cell line then cross on it, so no cell holds a sliver of their wedge that the next cell
/// sees as a whole patch.
fn merge_crossings(square: [i32; 4], rings: &mut [&mut Rings], seams: &mut Vec<[P; 2]>) {
    let c = CELL_UNITS as i32;
    for axis in 0..2 {
        for line in [square[axis], square[axis + 2]] {
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

/// The lines of a cell's level whose faces reach its window: within `input_m`.
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

/// The candidates of a cell and level, the one source of its shapes: the overlay slices and labels
/// them into faces, and the records evaluate them per point by the same `decide`.
struct Candidates<'a> {
    lines: Vec<WayLine<'a>>,
    cands: Vec<Cand>,
    seams: Vec<[P; 2]>,
    win: [i32; 4],
    cover: Cover,
    corners: Vec<CornerArc>,
    stats: FaceStats,
}

/// The face plan of one cell and level. `slopes = false` leaves out the slope layer (decks).
pub fn build(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, level: i8, slopes: bool) -> FacePlan {
    let c = candidates(graph, input, rules, cell, level, if slopes { 4 } else { 3 }, false);
    Sliced::new(&c).cell(&c, cell, level)
}

/// The slice's window: the cell grown by one tile. Every neighbour's slice cuts on the tile's sides
/// too, so near the cell both cut alike, and the window's frame lies far from the cell.
const GROW: i32 = slice::TILE as i32;

/// One slice of the candidates' window by all their outlines, a label per patch.
struct Sliced {
    patches: Vec<Vec<Vec<P>>>,
    labels: Vec<Label>,
    stats: FaceStats,
}

impl Sliced {
    fn new(c: &Candidates) -> Sliced {
        let mut stats = c.stats.clone();
        let strings: Vec<Vec<P>> = c.cands.iter().flat_map(|c| c.rings.iter().cloned()).collect();
        let (patches, segments) = slice(c.win, &strings, &c.seams);
        stats.slice_segments = segments as u32;
        let labels = patches.iter().map(|patch| label(&c.cands, &c.cover, &c.lines, patch, false)).collect();
        Sliced { patches, labels, stats }
    }

    /// The faces of the cell: its own patches. The snap rounding keeps off the cell lines, so they
    /// round as in a slice of the cell alone.
    fn cell(&self, c: &Candidates, cell: CellKey, level: i8) -> FacePlan {
        let sq = window(cell, 0.0);
        let inside = |p: &&P| p[0] >= sq[0] && p[0] <= sq[2] && p[1] >= sq[1] && p[1] <= sq[3];
        let own: Vec<usize> =
            (0..self.patches.len()).filter(|&k| self.patches[k].iter().flatten().all(|p| inside(&p))).collect();
        let patches: Vec<Vec<Vec<P>>> = own.iter().map(|&k| self.patches[k].clone()).collect();
        let labels: Vec<Label> = own.iter().map(|&k| self.labels[k]).collect();
        let mut plan = self.plan(cell, level, &Map::new(snap_round(&patches)), &labels);
        plan.corners = c.corners.clone();
        plan
    }

    /// The planar map of the whole window, for its faces by any labels.
    fn map(&self) -> Map {
        Map::new(snap_round(&self.patches))
    }

    /// The faces of a map of patches with a label per patch.
    fn plan(&self, cell: CellKey, level: i8, map: &Map, labels: &[Label]) -> FacePlan {
        let mut stats = self.stats.clone();
        stats.patches = labels.len() as u32;
        output(cell, level, map, labels, stats)
    }
}

/// Whether a candidate is a band layer.
fn band(c: &Cand) -> bool {
    c.relabel.is_none() && c.fill.is_none() && c.glob.is_some() && c.label.owner == Owner::Terrain
}

impl Candidates<'_> {
    /// The labels of the far level, per patch of the slice: no band layers. Each round at a corner of
    /// the paved outline decides its label again without them. The band layers win a point only
    /// where no other candidate does, so only the patches of a band layer's label change.
    fn far(&mut self, s: &Sliced) -> Vec<Label> {
        for k in 0..self.cands.len() {
            let Some(q) = self.cands[k].fill else { continue };
            let at = (self.cover.at(q).into_iter())
                .filter(|&c| self.cands[c as usize].fill.is_none() && !band(&self.cands[c as usize]))
                .collect();
            self.cands[k].label = decide(&self.cands, &self.lines, at, q);
        }
        let banded = |l: &Label| l.owner == Owner::Terrain && l.surface != Surface::Terrain;
        (s.patches.iter().zip(&s.labels))
            .map(|(p, l)| if banded(l) { label(&self.cands, &self.cover, &self.lines, p, true) } else { *l })
            .collect()
    }
}

/// The candidates of one cell and level with the band layers below `bands` (4 all, 3 without the
/// slope layer), and the labels of the outline rounds decided. With `records`, every reach grows by
/// `records::MARGIN_M` and a unit, so the candidates are the neighbours' own that far past the cell,
/// and their window is the cell grown by `GROW` units; else the window is the cell. Both give the
/// cell the same faces. The crossings merge on the cell's own lines.
fn candidates<'a>(
    graph: &'a WayGraph,
    input: &CellInput,
    rules: &'a Rules,
    cell: CellKey,
    level: i8,
    bands: usize,
    records: bool,
) -> Candidates<'a> {
    let ctx = Ctx { graph, rules, origin: cell.origin() };
    let plan = &rules.plan;
    let (margin, unit, grow) = if records { (records::MARGIN_M, 1, GROW) } else { (0.0, 0, 0) };
    let reach = |m: f64| {
        let w = window(cell, m + margin);
        [w[0] - unit, w[1] - unit, w[2] + unit, w[3] + unit]
    };
    let win = {
        let w = window(cell, 0.0);
        [w[0] - grow, w[1] - grow, w[2] + grow, w[3] + grow]
    };
    let bx = reach(input_m(plan, &rules.sections));
    // The counters and the corners are the cell's own: its points within its input reach.
    let own = {
        let o = window(cell, input_m(plan, &rules.sections));
        move |p: P| p[0] >= o[0] && p[0] <= o[2] && p[1] >= o[1] && p[1] <= o[3]
    };
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    // The carriageways reach farther than the points: the outline rounds read them. The ways reach
    // farther still: the off-line test of the points' rounds reads them.
    let lx = reach(rules::lines_m(plan, &rules.sections));
    let (lines, of_way) = lines(&ctx, input, level, reach(rules::ways_m(plan, &rules.sections)));
    // The points within `bx` round their arms; the points past it only end their runs.
    let all = graph_points(&ctx, &lines, &of_way, reach(rules::points_m(plan, &rules.sections)));
    let (mut points, outer): (Vec<Point>, Vec<Point>) = all.into_iter().partition(|pt| near(pt.pos));
    let mut stats = FaceStats::default();
    let junction = |pt: &&Point| matches!(pt.label.owner, Owner::Junction(_));
    stats.junctions = points.iter().filter(|pt| own(pt.pos)).filter(junction).count() as u32;
    stats.joints = points.iter().filter(|pt| own(pt.pos)).count() as u32 - stats.junctions;
    // A junction's or joint's arms end at the next junction or joint on their way, a turn's arms at
    // the next point. A turn on a part of a junction's or joint's region is none.
    let stops = |points: &[Point]| {
        let mut stops: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
        for a in points.iter().chain(&outer).flat_map(|pt| pt.arms.iter().chain(pt.guests.iter().map(|g| &g.0))) {
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
        for a in pt.arms.iter().chain(pt.guests.iter().map(|g| &g.0)) {
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
    stats.turns = points[first_turn..].iter().filter(|pt| own(pt.pos)).count() as u32;
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
    // The stop mouths of the junctions: their paved areas, with the turns of their arms' ways within
    // `extent_m`, and their stop arms' own rounds.
    let turns_at = |i: usize| -> Vec<usize> {
        let (pt, reach) = (&points[i], ctx.units(plan.extent_m, points[i].pos));
        let near = |t: &Point| libm::hypot(f64::from(t.pos[0] - pt.pos[0]), f64::from(t.pos[1] - pt.pos[1])) <= reach;
        (first_turn..points.len())
            .filter(|&t| pt.arms.iter().any(|a| a.line == points[t].arms[0].line) && near(&points[t]))
            .collect()
    };
    let mouths: Vec<(usize, Vec<usize>, points::Mouths)> = (0..first_turn)
        .filter(|&i| !points[i].guests.is_empty())
        .map(|i| {
            let (pt, turns) = (&points[i], turns_at(i));
            let u = |m: f64| ctx.units(m, pt.pos);
            let guests: Vec<points::Guest> = (pt.guests.iter())
                .map(|&(arm, r, relate)| points::Guest {
                    arm,
                    radius: u(r),
                    tol: u(lines[arm.line].sec.sag_plan_m),
                    relate,
                })
                .collect();
            let near: Vec<&Rounded> = turns.iter().map(|&t| &rounded[t]).collect();
            let m = points::mouths(
                &lines,
                &jj,
                pt.pos,
                &pt.arms,
                &rounded[i],
                &near,
                &guests,
                u(plan.extent_m),
                u(plan.seam_m),
            );
            (i, turns, m)
        })
        .collect();
    let mut cands: Vec<Cand> = Vec::new();
    // The ways' carriageways and the band layers, on the run of samples near the window. A way with
    // legs keeps its own shoulder layers (`OWN_BANDS`): `split_legs` gives each piece its nearest
    // leg's line, the host of its heights.
    let mut layers: [Vec<Vec<P>>; 4] = Default::default();
    let mut own_layers: BTreeMap<usize, [Vec<Vec<P>>; 4]> = BTreeMap::new();
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
        let (label, rank) = way_label(rules, line, legs::leg(&line.cuts, first, 1));
        cands.push(Cand {
            rings: union(&own, [0, 0]),
            label,
            key: (rank, line.sec.rank as u8, 3),
            glob: None,
            line: li,
            relabel: None,
            beyond: Vec::new(),
            fill: None,
            discs: Vec::new(),
            stops: Vec::new(),
        });
        let mut own = (!line.cuts.is_empty()).then(|| own_layers.entry(li).or_default());
        for k in 0..bands {
            let inner = |i: usize| if k == 0 { line.h[i] } else { line.bands[i][k - 1] };
            let d: Vec<f64> = (first..last).map(|i| line.bands[i][k]).collect();
            if (first..last).all(|i| line.bands[i][k] <= inner(i)) {
                continue;
            }
            let cuts = [&cut(first, k + 1)[..], &cut(last - 1, k + 1)[..]];
            let layer = match own.as_deref_mut() {
                Some(own) if k < OWN_BANDS => &mut own[k],
                _ => &mut layers[k],
            };
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
                discs: r.circles.iter().filter(|c| c.radius > 0.0).cloned().collect(),
                stops: Vec::new(),
            });
        }
        if !r.region.is_empty() {
            let rings = union(&r.region, pt.pos);
            let spans = Some(r.spans.clone());
            let beyond = Vec::new();
            let (key, discs) = (pt.key, Vec::new());
            let stops = Vec::new();
            cands.push(Cand {
                rings,
                label: pt.label,
                key,
                glob,
                line: 0,
                relabel: spans,
                beyond,
                fill: None,
                discs,
                stops,
            });
        }
        let one = pt.arms.iter().all(|a| a.line == pt.arms[0].line && !lines[a.line].cuts.is_empty());
        let mut own = one.then(|| own_layers.entry(pt.arms[0].line).or_default());
        for k in 0..bands {
            let layer = match own.as_deref_mut() {
                Some(own) if k < OWN_BANDS => &mut own[k],
                _ => &mut layers[k],
            };
            layer.extend(r.rings[k].iter().cloned());
        }
    }
    // A stop arm's rounds, in the arm's own material.
    let mut stop_circles: Vec<(P, Owner, &points::Circle)> = Vec::new();
    // A stop round's band rings join the band layers, as a point's round's do; its paved shoulder
    // rings, which reach to the stop arm's centre line, are a candidate of their own with the paved
    // shoulder's label but no stop list (below), so they do not hide the stop arm.
    let mut stop_paved: Vec<Vec<P>> = Vec::new();
    for (k, ring) in mouths.iter().flat_map(|m| &m.2.rings).filter(|r| r.0 < bands) {
        if *k == 0 { &mut stop_paved } else { &mut layers[*k] }.push(ring.clone());
    }
    for (i, _, m) in &mouths {
        let pt = &points[*i];
        for (gi, circle, fan) in &m.rounds {
            let (arm, line) = (pt.guests[*gi].0, &lines[pt.guests[*gi].0.line]);
            stop_circles.push((pt.pos, Owner::Way(line.way), circle));
            cands.push(Cand {
                rings: union(std::slice::from_ref(fan), pt.pos),
                label: mouth_label(rules, line, arm),
                key: (plan.rank(Rank::RailBed), line.sec.rank as u8, 0),
                glob: Some(line.key[arm.at]),
                line: 0,
                relabel: None,
                beyond: Vec::new(),
                fill: None,
                discs: vec![circle.clone()],
                stops: Vec::new(),
            });
        }
        for (gi, rings, part, _) in &m.regions {
            let (arm, line) = (pt.guests[*gi].0, &lines[pt.guests[*gi].0.line]);
            let (label, rank) = (mouth_label(rules, line, arm), plan.rank(Rank::Carriageway));
            cands.push(Cand {
                rings: union(rings, pt.pos),
                label,
                key: (rank, line.sec.rank as u8, 0),
                glob: Some(line.key[arm.at]),
                line: 0,
                relabel: Some(vec![*part]),
                beyond: Vec::new(),
                fill: None,
                discs: Vec::new(),
                stops: Vec::new(),
            });
        }
    }
    let band_labels = [
        (Rank::ShoulderPaved, Surface::Shoulder, HeightRule::Owner),
        (Rank::Shoulder, Surface::Shoulder, HeightRule::Owner),
        (Rank::Verge, Surface::Verge, HeightRule::Slope),
        (Rank::Slope, Surface::Slope, HeightRule::Slope),
    ];
    // A way's own band layers win over the others where they overlap: the way's shoulder is its own.
    let owned = own_layers.iter().flat_map(|(&l, layers)| layers.iter().enumerate().map(move |(k, r)| (Some(l), k, r)));
    let mut band_of: [Option<usize>; 4] = [None; 4];
    let plain = layers.iter().enumerate().map(|(k, r)| (None, k, r)).chain(std::iter::once((None, 0, &stop_paved)));
    for (line, k, layer) in plain.chain(owned) {
        if layer.is_empty() {
            continue;
        }
        if line.is_none() {
            band_of[k] = band_of[k].or(Some(cands.len()));
        }
        let (rank, surface, rule) = band_labels[k];
        let rank = plan.rank(rank);
        let material = line.map(|l| lines[l].way);
        let label = Label { rank, surface, rule, material, ..TERRAIN };
        cands.push(Cand {
            rings: union(layer, [0, 0]),
            label,
            key: (rank, u8::from(line.is_none()), 0),
            glob: Some([0; 5]),
            line: line.unwrap_or(0),
            relabel: None,
            beyond: Vec::new(),
            fill: None,
            discs: Vec::new(),
            stops: Vec::new(),
        });
    }
    // Where a junction's paved area lies (its arms' carriageways, its fans, the paved shoulders), its
    // stop arms' own candidates do not count: they end on its paved edge.
    // Every junction that the lines meet tells, so two cells decide alike past the window.
    let near_fans = |i: usize| match mouths.iter().find(|m| m.0 == i) {
        Some((_, turns, _)) => std::iter::once(i).chain(turns.iter().copied()).filter_map(|t| fan_of[t]).collect(),
        None => Vec::new(),
    };
    let all_points = points[..first_turn].iter().enumerate().map(|(i, pt)| (Some(i), pt));
    for (i, pt) in all_points.chain(outer.iter().map(|pt| (None, pt))).filter(|(_, pt)| !pt.guests.is_empty()) {
        let ways: Vec<u32> = pt.guests.iter().map(|g| lines[g.0.line].way).collect();
        let carriageways = (pt.arms.iter())
            .filter_map(|a| cands.iter().position(|c| c.glob.is_none() && c.relabel.is_none() && c.line == a.line));
        let fans: Vec<usize> = i.map_or_else(Vec::new, near_fans);
        let paving: Vec<usize> = carriageways.chain(fans).chain(band_of[0]).collect();
        for c in paving {
            cands[c].stops.extend(&ways);
        }
    }
    for c in &mut cands {
        c.stops.sort_unstable();
        c.stops.dedup();
        c.rings = prepare(&c.rings);
    }
    let pieces = split_legs(&ctx, &lines, &of_way, &mut cands);
    // The rounds at the corners of the paved outline that no point's round shapes. Their key loses
    // to every carriageway and rail bed, so a round paves land only. `lines_m` holds their reach.
    let mut paved: Vec<Vec<P>> = (cands.iter())
        .filter(|c| {
            c.relabel.is_none()
                && matches!(c.label.surface, Surface::Carriageway | Surface::RailBed | Surface::CornerFill)
        })
        .flat_map(|c| c.rings.iter().cloned())
        .collect();
    paved.extend(outer_paved);
    let circles: Vec<(P, f64)> = (rounded.iter().flat_map(|r| &r.circles))
        .chain(stop_circles.iter().map(|s| s.2))
        .map(|c| (c.centre, c.radius))
        .collect();
    let r_max = rules::inner_max_m(&rules.sections);
    let near_win = reach(0.0);
    let beyond = |p: P| {
        let w = near_win;
        let d = (w[0] - p[0]).max(p[0] - w[2]).max(w[1] - p[1]).max(p[1] - w[3]).max(0);
        f64::from(d) * metres_per_unit(ctx.origin.y + i64::from(p[1]))
    };
    let units = |m: f64, p: P| ctx.units(m, p);
    let fills = outline::rounds(&union(&paved, [0, 0]), &lines, &circles, &beyond, &units, plan.extent_m / 2.0, r_max);
    let fill_key = (plan.rank(Rank::RailBed), u8::MAX, u8::MAX);
    for (k, &(rank, surface, rule)) in band_labels.iter().enumerate() {
        let rings: Vec<Vec<P>> = fills.iter().flat_map(|f| f.bands[k].iter().cloned()).collect();
        if rings.is_empty() || k >= bands {
            continue;
        }
        let rank = plan.rank(rank);
        let label = Label { rank, surface, rule, ..TERRAIN };
        let (rings, glob, beyond) = (prepare(&rings), Some([0; 5]), Vec::new());
        let (key, discs) = ((rank, 1, 0), Vec::new());
        let stops = Vec::new();
        cands.push(Cand { rings, label, key, glob, line: 0, relabel: None, beyond, fill: None, discs, stops });
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
            discs: vec![f.circle.clone()],
            stops: Vec::new(),
        });
    }
    // A stop arm's mouth seam cuts its carriageway only: its rounds' fans end at their feet, before it.
    for (i, _, m) in &mouths {
        for (gi, _, _, cut) in &m.regions {
            let key = (points[*i].guests[*gi].0.line, [snap(cut[0].min(cut[1])), snap(cut[0].max(cut[1]))]);
            seam_points.entry(key).or_default();
        }
    }
    // Each seam cuts its line's carriageway and the fans that reach across it.
    let mut seams: Vec<(usize, [P; 2], Vec<usize>)> = (seam_points.into_iter())
        .map(|((l, s), pts)| (l, s, pts.iter().filter_map(|&p| fan_of[p]).flat_map(|c| pieces[c].clone()).collect()))
        .collect();
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
    let square = window(cell, 0.0);
    merge_crossings(square, &mut cands.iter_mut().map(|c| &mut c.rings).collect::<Vec<_>>(), &mut seams);
    cands.retain(|c| !c.rings.is_empty());
    stats.candidates = cands.len() as u32;
    let rings: Vec<&[Vec<P>]> = cands.iter().map(|c| c.rings.as_slice()).collect();
    let cover = Cover::new(win, &rings);
    for k in 0..cands.len() {
        if let Some(q) = cands[k].fill {
            let at: Vec<u32> = cover.at(q).into_iter().filter(|&c| cands[c as usize].fill.is_none()).collect();
            cands[k].label = decide(&cands, &lines, at, q);
        }
    }
    let mpu = |p: P| metres_per_unit(ctx.origin.y + i64::from(p[1]));
    let mut corners: Vec<CornerArc> = (points.iter().zip(&rounded))
        .filter(|(pt, _)| own(pt.pos))
        .flat_map(|(pt, r)| r.circles.iter().map(|c| CornerArc::new(pt.label.owner, c, mpu(c.centre))))
        .chain(stop_circles.iter().filter(|s| own(s.0)).map(|&(_, o, c)| CornerArc::new(o, c, mpu(c.centre))))
        .collect();
    let fill_label = |f: &outline::Fill| cands.iter().find(|c| c.fill == Some(f.inside)).map(|c| c.label);
    for f in &fills {
        if let Some(l) = fill_label(f).filter(|l| l.surface != Surface::Terrain) {
            corners.push(CornerArc::new(l.owner, &f.circle, mpu(f.circle.centre)));
        }
    }
    Candidates { lines, cands, seams, win, cover, corners, stats }
}

/// Splits every candidate that gives the label of a way with legs (`legs`), or that is one of its own
/// band layers: the medial lines between
/// its legs cut it, each piece goes to its nearest leg, and each leg's part is the union of its pieces,
/// which share their vertices. A candidate keeps its place with its first leg's part; the others go to
/// the end. Returns per candidate the places of its parts.
fn split_legs(ctx: &Ctx, lines: &[WayLine], of_way: &BTreeMap<u32, usize>, cands: &mut Vec<Cand>) -> Vec<Vec<usize>> {
    let mut parts: Vec<Vec<usize>> = (0..cands.len()).map(|c| vec![c]).collect();
    // Per line, its legs, the medial lines between them, its near zone (`Legs::runs`), and the medial
    // lines with the zone's edges.
    type Cuts = (legs::Legs, Vec<[P; 2]>, Rings, Vec<[P; 2]>);
    let mut legs_of: Vec<Option<Cuts>> = (0..lines.len()).map(|_| None).collect();
    for c in 0..parts.len() {
        let w = match cands[c].label {
            Label { owner: Owner::Way(w), .. } | Label { owner: Owner::Terrain, material: Some(w), .. } => w,
            _ => continue,
        };
        let Some(&l) = of_way.get(&w).filter(|&&l| !lines[l].cuts.is_empty()) else { continue };
        if cands[c].relabel.is_some() {
            continue;
        }
        let line = &lines[l];
        // The origin of the integer cuts and unions only: they do not depend on it.
        let anchor = line.p[line.cuts[0]];
        let (legs, medial, zone, edges) = legs_of[l].get_or_insert_with(|| {
            let legs = legs::Legs::new(&line.p, &line.cuts, &|o| ctx.units(line.leg_reach_m(), o));
            let medial = legs.medial(&|o| ctx.units(LEG_STEP_M, o));
            // The near zone: each stretch's stroke, as wide as its reach, with round ends, so the
            // strokes of two legs leave no gap about their cut.
            let mut strokes = Vec::new();
            for (run, r) in legs.runs() {
                stroke::pieces_along(&run, &vec![r; run.len()], &[], [&[], &[]], [true, true], false, &mut strokes);
            }
            let zone = prepare(&union(&strokes, anchor));
            let mut edges = medial.clone();
            edges.extend(zone.iter().flat_map(|r| (0..r.len()).map(move |i| [r[i], r[(i + 1) % r.len()]])));
            (legs, medial, zone, edges)
        });
        // A band also splits at the edge of the near zone: only its pieces in the zone name their leg's
        // line.
        let band = cands[c].label.owner == Owner::Terrain;
        let mut by_leg: Vec<Rings> = vec![Vec::new(); 2 * (line.cuts.len() + 1)];
        for piece in stroke::cut(&cands[c].rings, if band { edges } else { medial }, anchor) {
            let q = inner_point(&piece).unwrap_or_else(|| {
                let n = piece[0].len() as i64;
                let sum = piece[0].iter().fold([0i64; 2], |s, p| [s[0] + i64::from(p[0]), s[1] + i64::from(p[1])]);
                [(2 * sum[0]).div_euclid(n), (2 * sum[1]).div_euclid(n)]
            });
            let near = !band || geom::winding(zone, q) != 0;
            let leg = legs.nearest([q[0] as f64 / 2.0, q[1] as f64 / 2.0]);
            by_leg[2 * usize::from(leg) + usize::from(near)].extend(piece);
        }
        let split: Vec<(u8, bool, Rings)> = (by_leg.into_iter().enumerate())
            .filter(|(_, r)| !r.is_empty())
            .map(|(k, r)| ((k / 2) as u8, k % 2 == 1, r))
            .collect();
        if split.len() < 2 {
            cands[c].label = split.first().map_or(cands[c].label, |s| leg_label(cands[c].label, s.0, s.1));
            continue;
        }
        let split: Vec<(u8, bool, Rings)> =
            split.into_iter().map(|(leg, near, rings)| (leg, near, prepare(&union(&rings, anchor)))).collect();
        let label = cands[c].label;
        for (k, (leg, near, rings)) in split.into_iter().enumerate() {
            let o = &cands[c];
            let part = Cand {
                rings,
                label: leg_label(label, leg, near),
                key: o.key,
                glob: o.glob,
                line: o.line,
                relabel: None,
                beyond: o.beyond.clone(),
                fill: o.fill,
                discs: o.discs.clone(),
                stops: o.stops.clone(),
            };
            if k == 0 {
                cands[c] = part;
            } else {
                parts[c].push(cands.len());
                cands.push(part);
            }
        }
    }
    parts
}

/// The label of a way's candidate on leg `leg`. A piece of a band outside the near zone (`near`
/// false) is a plain band, as in a cell that holds no cut of the way.
fn leg_label(mut label: Label, leg: u8, near: bool) -> Label {
    if !near {
        return Label { material: None, ..label };
    }
    label.legs[0] = leg;
    label
}

/// The band layers that a way with legs keeps as its own: the shoulders, whose heights take a host.
const OWN_BANDS: usize = 2;

/// The step of a medial line between two legs along a leg.
const LEG_STEP_M: f64 = 0.5;

/// The global name of a way's leg near a point (doubled): the key of its sample nearest the point.
fn way_glob(line: &WayLine, leg: u8, q: [i64; 2]) -> Glob {
    let (lo, hi) = legs::range(&line.cuts, line.p.len(), leg);
    let d = |p: P| (2 * i64::from(p[0]) - q[0]).pow(2) + (2 * i64::from(p[1]) - q[1]).pow(2);
    let i = (lo..=hi).min_by_key(|&i| (d(line.p[i]), i)).unwrap_or(lo);
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
        // The way's carriageway: one candidate per leg.
        let ways: Vec<usize> = (0..cands.len())
            .filter(|&c| cands[c].relabel.is_none() && cands[c].glob.is_none() && cands[c].line == *l)
            .collect();
        if ways.is_empty() {
            out.push([a, b]);
            continue;
        }
        let (fa, fb) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
        let r = [fb[0] - fa[0], fb[1] - fa[1]];
        let len = (r[0] * r[0] + r[1] * r[1]).sqrt();
        if len == 0.0 {
            continue;
        }
        let off = |x: [f64; 2]| ((x[0] - fa[0]) * r[1] - (x[1] - fa[1]) * r[0]).abs() / len;
        let along = |x: [f64; 2]| ((x[0] - fa[0]) * r[0] + (x[1] - fa[1]) * r[1]) / (len * len);
        let on_edge = ways.iter().flat_map(|&ci| &cands[ci].rings).any(|ring| {
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
        let set: Vec<usize> = ways.iter().chain(fans).copied().collect();
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
/// The label of a patch; `far` leaves out the band layers.
fn label(cands: &[Cand], cover: &Cover, lines: &[WayLine], patch: &[Vec<P>], far: bool) -> Label {
    let mut at = match inner_point(patch) {
        Some(q) => cover.at(q),
        None => {
            let mut all: Vec<u32> =
                patch[0].iter().flat_map(|p| cover.at([2 * i64::from(p[0]), 2 * i64::from(p[1])])).collect();
            all.sort_unstable();
            all.dedup();
            all
        }
    };
    if far {
        at.retain(|&c| !band(&cands[c as usize]));
    }
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
    // Where a junction's paved area lies, its stop arms' own candidates do not count.
    let stopped: Vec<u32> = at.iter().flat_map(|&c| cands[c as usize].stops.iter().copied()).collect();
    at.retain(|&c| !matches!(cands[c as usize].label.owner, Owner::Way(w) if stopped.contains(&w)));
    let regions: Vec<u32> = at.iter().copied().filter(|&c| cands[c as usize].relabel.is_some()).collect();
    at.retain(|&c| cands[c as usize].relabel.is_none());
    let Some(&first) = at.iter().min_by_key(|&&c| cands[c as usize].key) else { return TERRAIN };
    let best = cands[first as usize].key;
    at.retain(|&c| cands[c as usize].key == best || cands[c as usize].label.surface == Surface::RailBed);
    let glob = |c: &Cand| c.glob.unwrap_or_else(|| way_glob(&lines[c.line], c.label.legs[0], q));
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
            label.legs[1] = rail.label.legs[0];
        }
    }
    label
}

/// The faces from the labelled patches: patches of one label that share an edge, not across a cell
/// line, form a face.
fn output(cell: CellKey, level: i8, map: &Map, labels: &[Label], mut stats: FaceStats) -> FacePlan {
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
                legs: l.legs,
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
