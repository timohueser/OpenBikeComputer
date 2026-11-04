//! The faces of ways and junctions in one cell and level: integer strokes of the ways, one local
//! closing per graph point, band layers as wider strokes, one integer slice of the cell by all their
//! outlines, and a label per patch by key. Every point of the cell lies in exactly one face.

pub mod check;
mod cover;
mod geom;
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
use geom::{inner_point, with_cell_lines, P};
use points::{Arm, Rounded};
use slice::{on_cell_line, slice, Map, NONE};
use stroke::{pieces, union, Rings};

use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS, FRAME_BITS};
use crate::graph::{Family, Level, Station, WayGraph};
use crate::sections::WaySection;
use crate::CellInput;

/// The stages of `build_faces_with`, in order, for a caller's clock.
pub const STAGES: [&str; 6] = ["lines", "points", "candidates", "slice", "labels", "output"];

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
    pub round: [bool; 2],
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
/// (`relabel`: per arm, its line and the part in the region as fractional sample indices) gives no
/// surface: it gives its label to a patch of an arm's carriageway whose inner point projects on that
/// part of the line, not on another part of the same way.
struct Cand {
    rings: Rings,
    label: Label,
    key: (u8, u8, u8),
    glob: Option<Glob>,
    line: usize,
    relabel: Option<Vec<(usize, f64, f64)>>,
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

/// The lines of the level's ways that come within `reach` units of the window.
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
        let near = |p: &[i32; 2]| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
        if samples.len() < 2 || !samples.iter().any(|s| near(&s.pos)) {
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
            round: [
                ends.get(&station_key(way.id, 1, &way.from)).copied().unwrap_or(false),
                ends.get(&station_key(way.id, -1, &way.to)).copied().unwrap_or(false),
            ],
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
    (out, of_way)
}

/// The sample of a line at a graph station, else the one nearest `pos`.
fn sample_at(g: &WayGraph, line: &WayLine, st: &Station, pos: P) -> usize {
    let way = &g.ways[line.way as usize];
    let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
    let exact = samples.iter().position(|s| {
        (s.span == st.span && (s.u - st.u).abs() < 1e-9) || (s.span == st.span + 1 && st.u >= 1.0 && s.u <= 0.0)
    });
    exact.unwrap_or_else(|| {
        let d = |p: P| (i64::from(p[0] - pos[0])).pow(2) + (i64::from(p[1] - pos[1])).pow(2);
        (0..line.p.len()).min_by_key(|&i| d(line.p[i])).unwrap_or(0)
    })
}

/// A graph point: its position, the arms of the level, the closing radius in metres, and its face's
/// label and key.
struct Point {
    pos: P,
    arms: Vec<Arm>,
    /// Neighbouring arms (as indices into `arms`) with the radius of their corner in metres.
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

/// The turn of a line over each sample's reach `w[i]` units: the angle between the chords at
/// stations s − w and s + w. A turn point is a sample whose turn passes `min_rad` and is the greatest
/// within its reach (then the greater chord turn at the sample, then the lesser station).
fn turns(line: &WayLine, w: &[f64], min_rad: f64) -> Vec<usize> {
    let n = line.p.len();
    let mut s = vec![0.0; n];
    for i in 1..n {
        let (a, b) = (line.p[i - 1], line.p[i]);
        let (dx, dy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
        s[i] = s[i - 1] + (dx * dx + dy * dy).sqrt();
    }
    let dir = |seg: usize| {
        let (a, b) = (line.p[seg], line.p[seg + 1]);
        [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])]
    };
    let angle = |u: [f64; 2], v: [f64; 2]| libm::atan2(u[0] * v[1] - u[1] * v[0], u[0] * v[0] + u[1] * v[1]).abs();
    let seg_at = |x: f64| s[1..n].partition_point(|&t| t < x).min(n - 2);
    let turn: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            if i == 0 || i + 1 == n {
                return (0.0, 0.0);
            }
            (angle(dir(seg_at(s[i] - w[i])), dir(seg_at(s[i] + w[i]))), angle(dir(i - 1), dir(i)))
        })
        .collect();
    (1..n.saturating_sub(1))
        .filter(|&i| {
            let (lo, hi) = (s.partition_point(|&t| t < s[i] - w[i]), s.partition_point(|&t| t <= s[i] + w[i]));
            turn[i].0 > min_rad
                && (lo..hi).filter(|&j| j != i).all(|j| {
                    let (a, b) = (turn[j], turn[i]);
                    a.0 < b.0 || (a.0 == b.0 && (a.1 < b.1 || (a.1 == b.1 && j > i)))
                })
        })
        .collect()
}

/// The graph points whose faces reach the window: junctions, joints and the tight turns of single
/// ways. Also the stations of each line's points.
fn graph_points(ctx: &Ctx, lines: &[WayLine], of_way: &BTreeMap<u32, usize>, bx: [i32; 4]) -> (Vec<Point>, FaceStats) {
    let (g, plan) = (ctx.graph, &ctx.rules.plan);
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    let mut out = Vec::new();
    let mut stats = FaceStats::default();
    let mut taken: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
    let class = |l: usize| lines[l].sec.rank as u8;
    for (id, j) in g.junctions.iter().enumerate() {
        let arms: Vec<(&crate::graph::Arm, usize)> =
            j.arms.iter().filter_map(|a| of_way.get(&a.way).map(|&l| (a, l))).collect();
        if arms.len() < 2 || !near(j.pos) {
            continue;
        }
        // The pair rule of each two neighbouring arms whose sector is under straight_deg.
        let mut pairs = Vec::new();
        let straight = plan.straight_deg.to_radians();
        for k in 0..arms.len() {
            let (a, b) = (arms[k], arms[(k + 1) % arms.len()]);
            let mut sector = b.0.angle - a.0.angle;
            if k + 1 == arms.len() {
                sector += 2.0 * std::f64::consts::PI;
            }
            if sector >= straight {
                continue;
            }
            let (ga, gb) = (lines[a.1].sec.group, lines[b.1].sec.group);
            let pair = match (a.0.host, b.0.host) {
                (true, false) => plan.pair(ga, gb),
                (false, true) => plan.pair(gb, ga),
                _ if class(a.1) <= class(b.1) => plan.pair(ga, gb),
                _ => plan.pair(gb, ga),
            };
            pairs.push((k, (k + 1) % arms.len(), pair.radius_m));
        }
        // The material: the arm of the best class, a road or path before a rail.
        let rail = arms.iter().all(|(_, l)| lines[*l].rail);
        let best = arms.iter().min_by_key(|(a, l)| (lines[*l].rail != rail, class(*l), a.way)).unwrap();
        let (mut label, rank) = way_label(ctx.rules, &lines[best.1]);
        label.owner = Owner::Junction(id as u32);
        label.rule = HeightRule::Blend;
        let pts: Vec<Arm> = arms
            .iter()
            .map(|(a, l)| {
                let at = sample_at(g, &lines[*l], &a.at, j.pos);
                taken[*l].push(at);
                Arm { line: *l, at, dir: a.dir }
            })
            .collect();
        let gp = ctx.global(j.pos);
        stats.junctions += 1;
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
        taken[la].push(at[0]);
        taken[lb].push(at[1]);
        // The face takes the label of the lesser key at the joint.
        let ka = (class(la), lines[la].key[at[0]]);
        let kb = (class(lb), lines[lb].key[at[1]]);
        let owner = if ka <= kb { la } else { lb };
        let (label, rank) = way_label(ctx.rules, &lines[owner]);
        let radius = lines[la].sec.inner_radius_m.min(lines[lb].sec.inner_radius_m);
        let gp = ctx.global(j.pos);
        stats.joints += 1;
        out.push(Point {
            pos: j.pos,
            arms: vec![
                Arm { line: la, at: at[0], dir: j.arms[0].dir },
                Arm { line: lb, at: at[1], dir: j.arms[1].dir },
            ],
            pairs: vec![(0, 1, radius)],
            seams: true,
            label,
            key: (rank, class(owner), 1),
            glob: [gp[0], gp[1], 0, 0, 0],
        });
    }
    for (li, line) in lines.iter().enumerate() {
        let r_m = line.sec.inner_radius_m;
        if r_m <= 0.0 || line.rail {
            continue;
        }
        let w: Vec<f64> = (0..line.p.len()).map(|i| line.h[i] + ctx.units(r_m, line.p[i])).collect();
        for i in turns(line, &w, plan.turn_deg.to_radians()) {
            if !near(line.p[i]) || taken[li].contains(&i) {
                continue;
            }
            let (label, rank) = way_label(ctx.rules, line);
            stats.turns += 1;
            out.push(Point {
                pos: line.p[i],
                arms: vec![Arm { line: li, at: i, dir: 1 }, Arm { line: li, at: i, dir: -1 }],
                pairs: vec![(0, 1, r_m)],
                seams: false,
                label,
                key: (rank, class(li), 2),
                glob: {
                    let gp = ctx.global(line.p[i]);
                    [gp[0], gp[1], 0, 0, 0]
                },
            });
        }
    }
    (out, stats)
}

/// A vertex less than 2 units off a cell line moves onto it; then each ring gets an exact vertex where
/// it crosses a cell line.
fn prepare(rings: &Rings) -> Rings {
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
    rings
        .iter()
        .map(|r| {
            let mut s: Vec<P> = r.iter().map(|p| [snap(p[0]), snap(p[1])]).collect();
            s.dedup();
            if s.len() > 1 && s[0] == s[s.len() - 1] {
                s.pop();
            }
            with_cell_lines(&s)
        })
        .filter(|r| r.len() >= 3)
        .collect()
}

/// The face plan of one cell and level. `slopes = false` leaves out the slope layer (decks).
pub fn build_faces(
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    cell: CellKey,
    level: i8,
    slopes: bool,
) -> FacePlan {
    build_faces_with(graph, input, rules, cell, level, slopes, &mut |_| {})
}

/// `build_faces`, calling `tick` with each stage of `STAGES` as it ends.
pub fn build_faces_with(
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    cell: CellKey,
    level: i8,
    slopes: bool,
    tick: &mut dyn FnMut(&'static str),
) -> FacePlan {
    let ctx = Ctx { graph, rules, origin: cell.origin() };
    let plan = &rules.plan;
    let win = window(cell, 0.0);
    let bx = window(cell, input_m(plan, &rules.sections));
    let (lines, of_way) = lines(&ctx, input, level, bx);
    tick("lines");
    let (points, mut stats) = graph_points(&ctx, &lines, &of_way, bx);
    // Each point's arms end at the next point on their way.
    let mut stops: Vec<Vec<usize>> = vec![Vec::new(); lines.len()];
    for pt in &points {
        for a in &pt.arms {
            stops[a.line].push(a.at);
        }
    }
    for s in &mut stops {
        s.sort_unstable();
        s.dedup();
    }
    let rounded: Vec<Rounded> = points
        .iter()
        .map(|pt| {
            let u = |m: f64| ctx.units(m, pt.pos);
            let pairs: Vec<(usize, usize, f64)> = pt.pairs.iter().map(|&(a, b, r)| (a, b, u(r))).collect();
            points::round(&lines, &stops, pt.pos, &pt.arms, &pairs, u(plan.extent_m), u(plan.seam_m), pt.seams)
        })
        .collect();
    tick("points");
    let mut cands: Vec<Cand> = Vec::new();
    // The ways' carriageways and the band layers, on the run of samples near the window.
    let mut layers: [Vec<Vec<P>>; 4] = Default::default();
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    for (li, line) in lines.iter().enumerate() {
        let first = line.p.iter().position(|&p| near(p)).unwrap_or(0).saturating_sub(1);
        let last = (line.p.iter().rposition(|&p| near(p)).unwrap_or(0) + 2).min(line.p.len());
        let ends = [line.round[0] && first == 0, line.round[1] && last == line.p.len()];
        let p = &line.p[first..last];
        let mut own = Vec::new();
        pieces(p, &line.h[first..last], ends, false, &mut own);
        let (label, rank) = way_label(rules, line);
        cands.push(Cand {
            rings: union(&own, [0, 0]),
            label,
            key: (rank, line.sec.rank as u8, 3),
            glob: None,
            line: li,
            relabel: None,
        });
        for k in 0..4 {
            if k == 3 && !slopes {
                continue;
            }
            let inner = |i: usize| if k == 0 { line.h[i] } else { line.bands[i][k - 1] };
            let d: Vec<f64> = (first..last).map(|i| line.bands[i][k]).collect();
            if (first..last).all(|i| line.bands[i][k] <= inner(i)) {
                continue;
            }
            pieces(p, &d, ends, false, &mut layers[k]);
        }
    }
    stats.ways = lines.len() as u32;
    // The points' fans and regions, and the band fans.
    for (pt, r) in points.iter().zip(&rounded) {
        let glob = Some(pt.glob);
        if !r.fans.is_empty() {
            cands.push(Cand {
                rings: union(&r.fans, pt.pos),
                label: pt.label,
                key: pt.key,
                glob,
                line: 0,
                relabel: None,
            });
        }
        if !r.region.is_empty() {
            let rings = union(&r.region, pt.pos);
            let spans = Some(r.spans.clone());
            cands.push(Cand { rings, label: pt.label, key: pt.key, glob, line: 0, relabel: spans });
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
        });
    }
    for c in &mut cands {
        c.rings = prepare(&c.rings);
    }
    cands.retain(|c| !c.rings.is_empty());
    stats.candidates = cands.len() as u32;
    tick("candidates");
    let strings: Vec<Vec<P>> = cands.iter().flat_map(|c| c.rings.iter().cloned()).collect();
    let seams: Vec<[P; 2]> = rounded.iter().flat_map(|r| r.seams.iter().copied()).collect();
    let (patches, segments) = slice(win, &strings, &seams);
    stats.patches = patches.len() as u32;
    stats.slice_segments = segments as u32;
    tick("slice");
    let rings: Vec<&[Vec<P>]> = cands.iter().map(|c| c.rings.as_slice()).collect();
    let cover = Cover::new(win, &rings);
    let labels: Vec<Label> = patches.iter().map(|patch| label(&cands, &cover, &lines, patch)).collect();
    tick("labels");
    let mut plan = output(cell, level, Map::new(patches), &labels, stats);
    let mpu = |p: P| metres_per_unit(ctx.origin.y + i64::from(p[1]));
    plan.corners = (points.iter().zip(&rounded))
        .flat_map(|(pt, r)| {
            r.circles.iter().map(|&(c, ru)| CornerArc { owner: pt.label.owner, centre: c, radius_m: ru * mpu(c) })
        })
        .collect();
    tick("output");
    plan
}

/// The global name of a way near a point (doubled): the key of its sample nearest the point.
fn way_glob(line: &WayLine, q: [i64; 2]) -> Glob {
    let d = |p: P| (2 * i64::from(p[0]) - q[0]).pow(2) + (2 * i64::from(p[1]) - q[1]).pow(2);
    let i = (0..line.p.len()).min_by_key(|&i| (d(line.p[i]), i)).unwrap_or(0);
    line.key[i]
}

/// The fractional sample index of the point of a line nearest a doubled point.
fn along(line: &WayLine, q: [i64; 2]) -> f64 {
    let q = [q[0] as f64 / 2.0, q[1] as f64 / 2.0];
    let mut best = (f64::INFINITY, 0.0);
    for (i, w) in line.p.windows(2).enumerate() {
        let (a, b) = ([f64::from(w[0][0]), f64::from(w[0][1])], [f64::from(w[1][0]), f64::from(w[1][1])]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 0.0 { (((q[0] - a[0]) * dx + (q[1] - a[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let (x, y) = (a[0] + t * dx - q[0], a[1] + t * dy - q[1]);
        let d = x * x + y * y;
        if d < best.0 {
            best = (d, i as f64 + t);
        }
    }
    best.1
}

/// The label of a patch: the least key among the candidates that cover its inner point, else
/// terrain; a carriageway over a rail bed is a crossing.
fn label(cands: &[Cand], cover: &Cover, lines: &[WayLine], patch: &[Vec<P>]) -> Label {
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
    let q = inner_point(patch).unwrap_or([2 * i64::from(patch[0][0][0]), 2 * i64::from(patch[0][0][1])]);
    let regions: Vec<u32> = at.iter().copied().filter(|&c| cands[c as usize].relabel.is_some()).collect();
    at.retain(|&c| cands[c as usize].relabel.is_none());
    let Some(&first) = at.iter().min_by_key(|&&c| cands[c as usize].key) else { return TERRAIN };
    let best = cands[first as usize].key;
    at.retain(|&c| cands[c as usize].key == best || cands[c as usize].label.surface == Surface::RailBed);
    let glob = |c: &Cand| c.glob.unwrap_or_else(|| way_glob(&lines[c.line], q));
    let win = (at.iter().map(|&c| &cands[c as usize])).filter(|c| c.key == best).min_by_key(|c| glob(c)).unwrap();
    let mut label = win.label;
    if matches!(label.owner, Owner::Way(_)) && !regions.is_empty() {
        let x = along(&lines[win.line], q);
        let of = (regions.iter().map(|&c| &cands[c as usize]))
            .filter(|c| c.relabel.iter().flatten().any(|&(l, lo, hi)| l == win.line && (lo..=hi).contains(&x)))
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
    let loops = map.loops(&groups);
    let mut by_group: BTreeMap<u32, (Vec<u32>, Vec<Vec<u32>>)> = BTreeMap::new();
    for (g, lp, area) in loops {
        let entry = by_group.entry(g).or_default();
        if area > 0 && entry.0.is_empty() {
            entry.0 = lp;
        } else {
            entry.1.push(lp);
        }
    }
    let mut used: Vec<u32> = by_group.values().flat_map(|(o, h)| o.iter().chain(h.iter().flatten()).copied()).collect();
    used.sort_unstable();
    used.dedup();
    let index = |v: u32| used.binary_search(&v).unwrap() as u32;
    let c = CELL_UNITS as i32;
    let mut faces: Vec<(u32, Face)> = by_group
        .into_iter()
        .map(|(g, (outer, holes))| {
            let l = labels[g as usize];
            let ring = |lp: &[u32]| {
                let mut r: Vec<u32> = lp.iter().map(|&v| index(v)).collect();
                let start = (0..r.len()).min_by_key(|&i| r[i]).unwrap_or(0);
                r.rotate_left(start);
                r
            };
            let mut rings = vec![ring(&outer)];
            rings.extend(holes.iter().map(|h| ring(h)));
            let inside =
                outer.iter().map(|&v| map.point(v)).all(|p| (0..=c).contains(&p[0]) && (0..=c).contains(&p[1]));
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
