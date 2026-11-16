//! The faces of ways and junctions in one cell and level: integer strokes of the ways, a round per
//! pair of neighbouring arms at each graph point (a tangent circle and its fan), point regions that
//! relabel their arms up to straight seams, band layers as wider strokes, one integer slice of the
//! cell by all their outlines, and a label per patch by key. Every point of the cell lies in exactly
//! one face.

pub mod check;
mod cover;
pub mod culverts;
pub mod deck;
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

use i_overlay::core::overlay_rule::OverlayRule;

pub use deck::{DeckWay, Portal, Storeys};
pub use rules::{input_m, Mouth, Pair, PlanRules, Rank, Rules};
pub use types::*;

use cover::Cover;
use geom::{grid_points, inner_point, with_grid_lines, P};
use points::{Arm, Circle, Part, Rounded};
use slice::{on_cell_line, slice, snap_round, Loop, Map, NONE};
use stroke::{pieces_along, union, Rings};

use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS, FRAME_BITS};
use crate::graph::{Family, Level, Station, WayGraph};
use crate::sections::{WaySection, HALF_MAX_M};
use crate::CellInput;

/// A way of the level: all its samples in cell units, with its half-width and band offsets.
pub(crate) struct WayLine<'a> {
    pub way: u32,
    pub sec: WaySection<'a>,
    pub rail: bool,
    pub p: Vec<P>,
    /// The carriageway half-width in units.
    pub h: Vec<f64>,
    /// The outer offsets in units of the band layers (`LAYERS`).
    pub bands: Vec<[f64; LAYERS]>,
    /// The global key of the span of each sample.
    pub key: Vec<Glob>,
    /// Per sample, the sides of the segment from it on with a sidewalk: 1 left, 2 right.
    pub walk: Vec<u8>,
    pub round: [bool; 2],
    /// Whether each end is a deck end: a ground way's end at a joint or junction with a deck arm, or with the bores a
    /// bore arm (a portal, `portal`).
    pub deck: [bool; 2],
    pub portal: [bool; 2],
    /// At a portal (a joint of a ground way and a bore), the direction of the end segment, the same line for both
    /// ways: their two end tangents meaned (`portal_dirs`), so the two strokes end square on one end line.
    pub end_dir: [Option<[f64; 2]>; 2],
    /// At a portal, the offsets that every stroke which ends on the portal line passes (the layers of the ground way
    /// that ends there, and the bore's wall): the two ways and the cover share the line's vertices.
    pub end_cuts: [Option<Vec<f64>>; 2],
    /// The bores (ways) that this ground way enters: at its own portal end, or through the ways that go on from it at
    /// joints, within `open_max_m` along them (`entering`).
    pub leads: Vec<u32>,
    /// The box of its carriageway: its samples' box grown by its widest half-width.
    pub bbox: [i32; 4],
    /// The boxes of the carriageway of each `CHUNK` segments in order.
    pub chunks: Vec<[i32; 4]>,
    /// The index in `p` of each of the way's graph samples: a taper adds vertices between them.
    pub index: Vec<usize>,
    /// The points in `p` where the line splits into legs (`legs::cuts`), and each cut's span
    /// (`legs::spans`).
    pub cuts: Vec<usize>,
    pub spans: Vec<(usize, usize)>,
    /// The height of each graph sample (metres), in the order of `index`.
    pub z: Vec<f64>,
}

/// The band layers about a carriageway, inner to outer: paved shoulder, shoulder, verge, bank, slope.
/// Read a layer by its name, never by its number. A way's bank and water's shoulders and verge are 0 wide.
pub(crate) const LAYERS: usize = 5;
/// The verge, the outer edge of a line's core.
pub(crate) const VERGE: usize = 2;
/// The slope, the outermost layer.
pub(crate) const SLOPE: usize = LAYERS - 1;

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

    /// The offsets of the layers at sample i, inner to outer: carriageway, then `LAYERS`.
    pub fn offsets(&self, i: usize) -> [f64; LAYERS + 1] {
        std::array::from_fn(|k| if k == 0 { self.h[i] } else { self.bands[i][k - 1] })
    }

    /// The directions of the segments of the run of samples `first..last` (`stroke::pieces_along`): a portal's end
    /// segment takes the portal's direction, every other its chord's.
    pub fn dirs(&self, first: usize, last: usize) -> Vec<Option<[f64; 2]>> {
        // No portal end: every segment its chord's (`stroke::pieces_along` reads none as that).
        if !BORES || self.end_dir == [None, None] {
            return Vec::new();
        }
        let mut d = vec![None; (last - first).saturating_sub(1)];
        if let (Some(e), 0, true) = (self.end_dir[0], first, !d.is_empty()) {
            d[0] = Some(e);
        }
        if let (Some(e), Some(x)) = (self.end_dir[1], d.last_mut().filter(|_| last == self.p.len())) {
            *x = Some(e);
        }
        d
    }

    /// The half-width of the core (carriageway, shoulder and verge) at each sample.
    pub fn core(&self) -> Vec<f64> {
        self.bands.iter().map(|b| b[VERGE]).collect()
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

/// Whether the storeys over the ground are built (the feature `storeys`): a build without them links none of their
/// code.
const STOREYS: bool = cfg!(feature = "storeys");
/// Whether the storey under the ground is built (the feature `bores`).
const BORES: bool = crate::graph::BORES;

/// A way of storey 0. Water lies on the ground or in a tunnel, never on a deck.
fn in_level(graph: &WayGraph, way: u32) -> bool {
    graph.ways[way as usize].level == Level::Ground
}

/// The ways of a level's lines: storey 0's (`in_level`), or those of another storey (`deck::Decks`).
fn keep<'a>(graph: &'a WayGraph, decks: Option<&'a deck::Decks>, level: i8) -> impl Fn(u32) -> bool + 'a {
    move |w| match decks {
        Some(d) => d.storey[w as usize] == level,
        None => in_level(graph, w),
    }
}

/// A deck way: a ceiling line of storey 0 (`ceiling_lines`).
fn on_deck(graph: &WayGraph, way: u32) -> bool {
    let w = &graph.ways[way as usize];
    w.family != Family::Water && w.level == Level::Deck
}

/// A bore way (with the bores): a BORE stretch of a tunnel way that is not water, in the graph of the storeys. Its
/// floor bound is a ceiling line with the sign turned.
#[inline]
pub(crate) fn in_bore(graph: &WayGraph, way: u32) -> bool {
    let w = &graph.ways[way as usize];
    BORES && graph.levelled && w.family != Family::Water && w.level == Level::Tunnel
}

/// Whether the graph has a bore: a cell with none builds as a build with no bores does.
pub(crate) fn any_bore(graph: &WayGraph) -> bool {
    BORES && (0..graph.ways.len() as u32).any(|w| in_bore(graph, w))
}

/// An arm on a structure: a deck, or a bore.
#[inline]
fn raised(graph: &WayGraph, a: &crate::graph::Arm) -> bool {
    a.level == Level::Deck || in_bore(graph, a.way)
}

/// Each arm of a portal (a joint of a ground way and a bore way) by its key, with the end direction of both and the
/// other arm: the mean of the two ways' tangents at their end samples, both turned into the bore. Both storeys read it
/// from the same samples.
fn portal_dirs(g: &WayGraph) -> Vec<(ArmKey, [f64; 2], crate::graph::Arm)> {
    let mut out = Vec::new();
    for j in g.joints.iter().filter(|_| BORES) {
        let [a, b] = &j.arms;
        let (ground, bore) = match (in_bore(g, a.way), in_bore(g, b.way)) {
            (false, true) => (a, b),
            (true, false) => (b, a),
            _ => continue,
        };
        if g.ways[ground.way as usize].level != Level::Ground || g.ways[ground.way as usize].family == Family::Water {
            continue;
        }
        // An arm's tangent away from the node: its end sample's, along its part times its direction.
        let away = |arm: &crate::graph::Arm| {
            let r = &g.ways[arm.way as usize].samples;
            let t = g.samples[if arm.dir > 0 { r.start } else { r.end - 1 } as usize].tangent;
            [f64::from(arm.dir) * t[0], f64::from(arm.dir) * t[1]]
        };
        let (tg, tb) = (away(ground), away(bore));
        let m = [tb[0] - tg[0], tb[1] - tg[1]];
        let len = libm::hypot(m[0], m[1]);
        if len > 1e-9 {
            for arm in [a, b] {
                let other = if std::ptr::eq(arm, a) { b } else { a };
                out.push((station_key(arm.way, arm.dir, &arm.at), [m[0] / len, m[1] / len], other.clone()));
            }
        }
    }
    out
}

/// The ground ways that enter each bore, as (ground way, bore way): the way that ends on the portal line and the ways
/// that go on from it at joints of two ground ways, as far as `reach_m` along them.
fn entering(g: &WayGraph, dirs: &[(ArmKey, [f64; 2], crate::graph::Arm)], reach_m: f64) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for (key, _, other) in dirs.iter().filter(|x| !in_bore(g, x.0 .0) && in_bore(g, x.2.way)) {
        // The way and the direction of its arm at the end where the walk came in. Each way is taken once, so the walk
        // ends.
        let (mut way, mut dir, mut left) = (key.0, key.1, reach_m);
        loop {
            if out.contains(&(way, other.way)) {
                break;
            }
            out.push((way, other.way));
            let r = &g.ways[way as usize].samples;
            left -= (g.samples[r.end as usize - 1].along_m - g.samples[r.start as usize].along_m).abs();
            if left <= 0.0 {
                break;
            }
            // The joint at its other end, with a ground way that is no water.
            let next = g.joints.iter().find_map(|j| {
                let k = j.arms.iter().position(|a| a.way == way && a.dir == -dir)?;
                let o = &j.arms[1 - k];
                let w = &g.ways[o.way as usize];
                (w.level == Level::Ground && w.family != Family::Water).then_some((o.way, o.dir))
            });
            let Some((w, d)) = next else { break };
            (way, dir) = (w, d);
        }
    }
    out
}

/// An arm's key: its way, its direction and its station.
type ArmKey = (u32, i8, u32, u64);

fn station_key(way: u32, dir: i8, st: &Station) -> ArmKey {
    (way, dir, st.span, st.u.to_bits())
}

/// The lines of the ways that `keep` keeps whose centre line meets the box `bx`: the ways, then the water, so
/// the way lines are a prefix that holds no water.
fn lines<'a>(
    ctx: &Ctx<'a>,
    input: &CellInput,
    keep: &dyn Fn(u32) -> bool,
    bx: [i32; 4],
) -> (Vec<WayLine<'a>>, BTreeMap<u32, usize>) {
    lines_with(ctx, input, keep, bx, None)
}

/// `lines`, with storey 0's lines of the same box (`ground`) where the caller has them: a bore's portal end reads the
/// ground way that ends there.
fn lines_with<'a>(
    ctx: &Ctx<'a>,
    input: &CellInput,
    keep: &dyn Fn(u32) -> bool,
    bx: [i32; 4],
    ground: Option<&[WayLine]>,
) -> (Vec<WayLine<'a>>, BTreeMap<u32, usize>) {
    let g = ctx.graph;
    // The round ends: free ends. A way ends square at a junction or a joint, whose face fills the
    // gaps between its arms; and at a level change.
    let mut ends: BTreeMap<(u32, i8, u32, u64), bool> = BTreeMap::new();
    for e in &g.ends {
        ends.insert(station_key(e.arm.way, e.arm.dir, &e.arm.at), true);
    }
    // The deck ends: the ground arms of a joint or junction with an arm on a structure; a portal where it is a bore.
    let mut decks = std::collections::BTreeSet::new();
    let mut portals = std::collections::BTreeSet::new();
    for arms in g.junctions.iter().map(|j| &j.arms[..]).chain(g.joints.iter().map(|j| &j.arms[..])) {
        if arms.iter().any(|a| raised(g, a)) {
            let ground = arms.iter().filter(|a| a.level == Level::Ground).map(|a| station_key(a.way, a.dir, &a.at));
            if arms.iter().any(|a| in_bore(g, a.way)) {
                portals.extend(ground.clone());
            }
            decks.extend(ground);
        }
    }
    let dirs = if BORES { portal_dirs(g) } else { Vec::new() };
    let lead = if BORES { entering(g, &dirs, ctx.rules.structures.open_max_m) } else { Vec::new() };
    let mut out = Vec::new();
    let mut of_way = BTreeMap::new();
    let (ways, water): (Vec<_>, Vec<_>) = g.ways.iter().partition(|w| w.family != Family::Water);
    for way in ways.into_iter().chain(water) {
        if !keep(way.id) {
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
        // The bank band ends at the bank top: where the bank's cut reaches its rise on level ground.
        let bank = heights::cone_reach(sec.cut, sec.rise_m, &ctx.rules.plan.heights);
        let ends_at = [station_key(way.id, 1, &way.from), station_key(way.id, -1, &way.to)];
        // A portal's end segment takes the portal's direction, turned along the line.
        let end_dir = |k: usize| {
            if !BORES {
                return None;
            }
            let d = dirs.iter().find(|x| x.0 == ends_at[k])?.1;
            let (a, b) = if k == 0 {
                (samples[0].pos, samples[1].pos)
            } else {
                (samples[samples.len() - 2].pos, samples[samples.len() - 1].pos)
            };
            let along = f64::from(b[0] - a[0]) * d[0] + f64::from(b[1] - a[1]) * d[1];
            Some(if along < 0.0 { [-d[0], -d[1]] } else { d })
        };
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
            deck: ends_at.map(|k| decks.contains(&k)),
            portal: ends_at.map(|k| BORES && portals.contains(&k)),
            end_dir: [end_dir(0), end_dir(1)],
            end_cuts: [None, None],
            leads: if BORES { lead.iter().filter(|x| x.0 == way.id).map(|x| x.1).collect() } else { Vec::new() },
            bbox: [i32::MAX, i32::MAX, i32::MIN, i32::MIN],
            chunks: Vec::new(),
            index: (0..samples.len()).collect(),
            cuts: Vec::new(),
            spans: Vec::new(),
            z: samples.iter().map(|s| s.z_m).collect(),
            sec,
        };
        for s in samples {
            let u = |m: f64| ctx.units(m, s.pos);
            let h = sec.half_m + s.widen_m;
            line.p.push(s.pos);
            line.h.push(u(h));
            let paved = if sec.shoulder_paved { sh } else { 0.0 };
            let verge = h + sh + sec.verge_m;
            line.bands.push([u(h + paved), u(h + sh), u(verge), u(verge + bank), u(verge + sec.reach_m)]);
            let span = s.span.min(part.spans.len() as u32 - 1) as usize;
            let o = part.spans[span].owner;
            let v = part.vertices[span].pos;
            line.key.push([i64::from(o[0]), i64::from(o[1]), i64::from(o[2]), v[0], v[1]]);
        }
        // Its legs, on its graph samples: two parts beside each other whose slope bands meet at heights
        // that do not agree. A way of a class without a wall between its legs stays one line.
        if sec.leg_wall {
            let feather = sec.reach_m.max(ctx.rules.plan.heights.feather_min_m);
            let ok = heights::step_limit(sec.return_grade, feather, &ctx.rules.plan.heights) / 2.0;
            let band: Vec<f64> = line.bands.iter().map(|b| b[SLOPE]).collect();
            line.spans = legs::spans(&line.p, &line.core(), &band, &line.z, ok);
            line.cuts = line.spans.iter().map(|&(a, b)| (a + b) / 2).collect();
        }
        of_way.insert(way.id, out.len());
        out.push(line);
    }
    taper(ctx, &mut out, &of_way);
    // A bore's end on a portal takes the offsets of the ground way that ends there as storey 0 builds that way (its
    // own tapers included): one computation, so both strokes end on the same corners.
    if BORES && out.iter().any(|l| l.end_dir.iter().any(Option::is_some) && in_bore(g, l.way)) {
        let built;
        let ground = match ground {
            Some(lines) => lines,
            None => {
                built = lines(ctx, input, &|w| in_level(g, w), bx).0;
                &built[..]
            }
        };
        for line in out.iter_mut().filter(|l| in_bore(g, l.way)) {
            let way = &g.ways[line.way as usize];
            for (k, key) in [station_key(way.id, 1, &way.from), station_key(way.id, -1, &way.to)].iter().enumerate() {
                let Some((_, _, other)) = dirs.iter().find(|x| x.0 == *key) else { continue };
                let Some(gl) = ground.iter().position(|l| l.way == other.way) else { continue };
                let gi = if other.dir > 0 { 0 } else { ground[gl].p.len() - 1 };
                let o = ground[gl].offsets(gi);
                let at = if k == 0 { 0 } else { line.p.len() - 1 };
                line.p[at] = ground[gl].p[gi];
                line.h[at] = o[0];
                line.bands[at] = std::array::from_fn(|m| o[m + 1]);
                line.end_cuts[k] = ground[gl].end_cuts[usize::from(gi != 0)].clone();
            }
        }
    }
    // A ground way's portal end passes its own layers' corners and the bore's wall corner.
    for line in out.iter_mut().filter(|l| BORES && !in_bore(g, l.way)) {
        for k in 0..2 {
            if line.end_dir[k].is_some() {
                let at = if k == 0 { 0 } else { line.p.len() - 1 };
                let mut cuts = line.offsets(at).to_vec();
                cuts.push(line.bands[at][VERGE] + ctx.units(deck::WALL_M, line.p[at]));
                cuts.sort_by(f64::total_cmp);
                cuts.dedup();
                line.end_cuts[k] = Some(cuts);
            }
        }
    }
    for line in &mut out {
        line.cuts = line.cuts.iter().map(|&k| line.index[k]).collect();
        line.spans = line.spans.iter().map(|&(a, b)| (line.index[a], line.index[b])).collect();
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
    to: [f64; LAYERS + 1],
}

/// At a joint, each layer edge (carriageway, then `LAYERS`) of the arm that
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
            if (0..=LAYERS).any(|m| o[me][m] - o[1 - me][m] >= 1.0) {
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
        let o: Vec<[f64; LAYERS + 1]> = arms.iter().zip(&at).map(|((_, l), &i)| lines[*l].offsets(i)).collect();
        for (k, (a, l)) in arms.iter().enumerate().filter(|(_, (a, _))| !a.host) {
            let to: [f64; LAYERS + 1] =
                std::array::from_fn(|m| (0..o.len()).filter(|&i| i != k).map(|i| o[i][m]).fold(0.0, f64::max));
            if (0..=LAYERS).any(|m| o[k][m] - to[m] >= 1.0) {
                tapers.entry(*l).or_default().push(Taper { at: at[k], dir: a.dir, to });
            }
        }
    }
    for (l, list) in tapers {
        let line = &mut lines[l];
        let n = line.p.len();
        let extent = ctx.units(ctx.rules.plan.extent_m, line.p[list[0].at]);
        let own: Vec<[f64; LAYERS + 1]> = (0..n).map(|i| line.offsets(i)).collect();
        // The distance of each vertex from each taper's joint along the line, to the first past
        // `extent_m`; each taper's rate per layer.
        let mut along = vec![vec![f64::INFINITY; n]; list.len()];
        let mut rate = vec![[ctx.rules.plan.taper_rate; LAYERS + 1]; list.len()];
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
            for m in 0..=LAYERS {
                rate[k][m] = rate[k][m].max((widest[m] - t.to[m]) / extent);
            }
        }
        // The offsets at distances `s` from the joints: each layer at most its tapers, and no layer
        // inside the one below it.
        let narrowed = |o: [f64; LAYERS + 1], s: &[f64]| {
            let mut r = o;
            for (k, t) in list.iter().enumerate().filter(|&(k, _)| s[k].is_finite()) {
                for m in 0..=LAYERS {
                    r[m] = r[m].min(t.to[m] + rate[k][m] * s[k]);
                }
            }
            for m in 1..=LAYERS {
                r[m] = r[m].max(r[m - 1]);
            }
            r
        };
        // The vertices: position, offsets, key, walk.
        let mut rows: Vec<(P, [f64; LAYERS + 1], Glob, u8)> = Vec::with_capacity(n);
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
                for m in 0..=LAYERS {
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
                let o: [f64; LAYERS + 1] = std::array::from_fn(|m| own[i][m] + x * (own[i + 1][m] - own[i][m]));
                let s: Vec<f64> = (0..list.len()).map(|k| along[k][i] + x * (along[k][i + 1] - along[k][i])).collect();
                rows.push((q, narrowed(o, &s), line.key[i], line.walk[i]));
            }
        }
        line.p = rows.iter().map(|r| r.0).collect();
        line.h = rows.iter().map(|r| r.1[0]).collect();
        line.bands = rows.iter().map(|r| std::array::from_fn(|k| r.1[k + 1])).collect();
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

/// The key of a way's carriageway or rail bed candidate.
fn way_key(rules: &Rules, line: &WayLine) -> (u8, u8, u8) {
    (way_label(rules, line, 0).1, line.sec.rank as u8, 3)
}

fn way_label(rules: &Rules, line: &WayLine, leg: u8) -> (Label, u8) {
    let (rank, surface) = match (line.sec.water, line.rail) {
        (true, _) => (Rank::WaterLine, Surface::Water),
        (false, true) => (Rank::RailBed, Surface::RailBed),
        (false, false) => (Rank::Carriageway, Surface::Carriageway),
    };
    let rank = rules.plan.rank(rank);
    let label = Label {
        owner: Owner::Way(line.way),
        surface,
        rule: HeightRule::Owner,
        material: Some(line.way),
        crossing: None,
        rank,
        reach_m: line.sec.reach_m as f32,
        legs: [leg, 0],
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

/// The junctions and joints whose faces reach the window. A junction with an arm that goes on at another level
/// (`elsewhere`) is no point of this level: each of its arms here ends there, open (`deck::Decks`), as at a joint.
fn graph_points(
    ctx: &Ctx,
    lines: &[WayLine],
    of_way: &BTreeMap<u32, usize>,
    elsewhere: &dyn Fn(u32) -> bool,
    bx: [i32; 4],
) -> Vec<Point> {
    let g = ctx.graph;
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    let mut out = Vec::new();
    let class = |l: usize| lines[l].sec.rank as u8;
    for (id, j) in g.junctions.iter().enumerate() {
        let arms: Vec<(&crate::graph::Arm, usize)> =
            j.arms.iter().filter_map(|a| of_way.get(&a.way).map(|&l| (a, l))).collect();
        if arms.len() < 2 || !near(j.pos) || j.arms.iter().any(|a| elsewhere(a.way)) {
            continue;
        }
        // The material: the arm of the best class, a road or path before a rail, a host before a guest.
        let rail = arms.iter().all(|(_, l)| lines[*l].rail);
        let best = arms.iter().min_by_key(|(a, l)| (lines[*l].rail != rail, class(*l), !a.host, a.way)).unwrap();
        let at = sample_at(g, &lines[best.1], &best.0.at, j.pos);
        let (mut label, rank) = way_label(ctx.rules, &lines[best.1], legs::leg(&lines[best.1].cuts, at, best.0.dir));
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

/// Each ring's vertices snapped to the cell lines, with a vertex where it crosses one.
fn prepare(rings: &Rings) -> Rings {
    rings
        .iter()
        .map(|r| {
            let mut s: Vec<P> = r.iter().map(|&p| snap(p)).collect();
            s.dedup();
            if s.len() > 1 && s[0] == s[s.len() - 1] {
                s.pop();
            }
            with_grid_lines(&s, CELL_UNITS)
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

/// The box within which `candidates` reads a reach of `m` metres past the cell: with `records`,
/// `records::MARGIN_M` and a unit more.
fn grown(cell: CellKey, m: f64, records: bool) -> [i32; 4] {
    let (margin, unit) = if records { (records::MARGIN_M, 1) } else { (0.0, 0) };
    let w = window(cell, m + margin);
    [w[0] - unit, w[1] - unit, w[2] + unit, w[3] + unit]
}

/// The way lines of a cell's records: the prefix of the lines of `candidates` with `records`, on the
/// ground level, that holds no water.
pub(crate) fn record_lines<'a>(
    graph: &'a WayGraph,
    input: &CellInput,
    rules: &'a Rules,
    cell: CellKey,
) -> Vec<WayLine<'a>> {
    let ctx = Ctx { graph, rules, origin: cell.origin() };
    let mut lines =
        lines(&ctx, input, &|w| in_level(graph, w), grown(cell, rules::ways_m(&rules.plan, &rules.sections), true)).0;
    lines.retain(|l| !l.sec.water);
    lines
}

/// The ceiling lines of a cell's records: its deck ways whose ceiling reaches a bin of the cell (at most
/// `records::ceiling_end_reach` from the centre line), each one leg, with the half-width of the deck's stroke: its
/// paved layers' and `edge_band_m`. With the bores also its bore ways, whose floor bound holds the ground over the
/// bore's stroke: its core, its wall and the cover's margin.
pub(crate) fn ceiling_lines<'a>(
    graph: &'a WayGraph,
    input: &CellInput,
    rules: &'a Rules,
    cell: CellKey,
) -> Vec<WayLine<'a>> {
    let ctx = Ctx { graph, rules, origin: cell.origin() };
    let (deck_m, bore_m) = (rules.structures.edge_band_m, deck::WALL_M + deck::COVER_SIDE_M);
    // A cell whose graph has no bore reads its decks as a build with no bores does.
    let half = HALF_MAX_M + if BORES && any_bore(graph) { deck_m.max(bore_m) } else { deck_m };
    let m = records::ceiling_end_reach(half, rules.plan.heights.ceiling_release_m) + records::HEIGHT_PAD_M;
    let mut out = lines(&ctx, input, &|w| on_deck(graph, w) || in_bore(graph, w), grown(cell, m, true)).0;
    for line in &mut out {
        line.cuts.clear();
        line.spans.clear();
        let band = if in_bore(graph, line.way) { bore_m } else { deck_m };
        for i in 0..line.p.len() {
            line.h[i] = line.bands[i][VERGE] + ctx.units(band, line.p[i]);
        }
    }
    out
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
    let decks = (STOREYS && level != 0).then(|| deck::Decks::of(graph, input));
    let mut out =
        lines(&ctx, input, &keep(graph, decks.as_ref(), level), window(cell, input_m(&rules.plan, &rules.sections))).0;
    if STOREYS && decks.is_some() {
        storey_bands(&ctx, &mut out, level);
    }
    out
}

/// The layers of a storey's lines: a deck's stroke over the ground, a bore's floor under it.
fn storey_bands(ctx: &Ctx, lines: &mut [WayLine], level: i8) {
    for line in lines {
        if level < 0 {
            deck::bore_bands(line);
        } else {
            deck::deck_bands(line, &|p| ctx.units(ctx.rules.structures.edge_band_m, p));
        }
    }
}

/// The candidates of a cell and level, the one source of its shapes: the overlay slices and labels
/// them into faces, and the records evaluate them per point by the same `decide`.
struct Candidates<'a> {
    lines: Vec<WayLine<'a>>,
    /// The way lines: a prefix of `lines`, which the far level reads.
    ways: usize,
    cands: Vec<Cand>,
    seams: Vec<[P; 2]>,
    win: [i32; 4],
    cover: Cover,
    corners: Vec<CornerArc>,
    mouths: Vec<CulvertMouth>,
    /// The end lines on which storey 0 and a deck or a bore meet: the edge kind of a face edge on one (`DeckEdge`,
    /// `Portal`), the end point, the unit direction into the line and the half-width of its stroke there (units).
    ends: Vec<(EdgeKind, [f64; 5])>,
    /// With the bores, the two marks of each portal, by the bore's way: on storey 0 the core strokes of the ground ways
    /// that enter it (`WayLine::leads`), on the storey under it the bore's own stroke. A floor's edge between the two
    /// is an opening.
    marks: Vec<(u32, Rings)>,
    /// With the bores, on storey 0, the floors in front of a portal line: the bores of a way that enters them, that
    /// way's carriageway label, and the part of its core stroke under a way on top of it.
    wedges: Vec<(Vec<u32>, Label, Rings)>,
    /// With the bores, on storey 0, per lower way (a bore, or a way that enters one) the lines with a stretch on top of
    /// it and the strokes of those stretches.
    lid_of: Vec<(u32, usize, Rings)>,
    stats: FaceStats,
}

/// The face plan of one cell and level. `slopes = false` leaves out storey 0's slope layer; a storey over the
/// ground holds its decks' edge band there.
pub fn build(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, level: i8, slopes: bool) -> FacePlan {
    let c =
        candidates(graph, input, rules, cell, level, if slopes || level != 0 { LAYERS } else { SLOPE }, false, None);
    Sliced::new(&c).cell(&c, cell, level)
}

/// The slice's window: the cell grown by one tile. Every neighbour's slice cuts on the tile's sides
/// too, so near the cell both cut alike, and the window's frame lies far from the cell.
const GROW: i32 = slice::TILE as i32;

/// One slice of the candidates' window by all their outlines, a label per patch.
struct Sliced {
    patches: Vec<Vec<Vec<P>>>,
    labels: Vec<Label>,
    /// With the bores (`joint`), per patch: the label of the storey under the ground (terrain where no bore lies under
    /// it), and its floor's id (one per label of the storey under the ground or of a floor in front of a portal line,
    /// `NO_UNDER` for none).
    under: Vec<Label>,
    under_id: Vec<u32>,
    /// Per patch, the label of the floor in front of a portal line that holds it, else terrain.
    wedge: Vec<Label>,
    /// Per patch, the bores (ways) whose entering ways' strokes hold it, and those whose own stroke or floor in front of
    /// a portal line holds it.
    front: Vec<Vec<u32>>,
    back: Vec<Vec<u32>>,
    stats: FaceStats,
}

/// No bore under a patch or a face.
pub const NO_UNDER: u32 = u32::MAX;

/// Per patch of a map: its floor's id and the bores of the strokes that hold it (`Sliced::joint`).
#[derive(Clone, Copy)]
struct Floors<'a> {
    under: &'a [u32],
    front: &'a [Vec<u32>],
    back: &'a [Vec<u32>],
}

impl Sliced {
    fn new(c: &Candidates) -> Sliced {
        let mut stats = c.stats.clone();
        let strings: Vec<Vec<P>> = c.cands.iter().flat_map(|c| c.rings.iter().cloned()).collect();
        let (patches, segments) = slice(c.win, &strings, &c.seams);
        stats.slice_segments = segments as u32;
        let labels = patches.iter().map(|patch| label(&c.cands, &c.cover, &c.lines, patch, false)).collect();
        let (under, wedge) = (Vec::new(), Vec::new());
        Sliced { patches, labels, under, under_id: Vec::new(), wedge, front: Vec::new(), back: Vec::new(), stats }
    }

    /// The one map: one slice of storey 0's candidates and those of the storey under it (`u`), each patch with a label
    /// of each. Over a floor, storey 0 is the cover or a paved layer of a way on top of that floor's way; every other
    /// face yields. No cover lies on an entering way's stroke where no floor lies under it.
    fn joint(c: &Candidates, u: &Candidates) -> Sliced {
        let mut stats = c.stats.clone();
        let strings: Vec<Vec<P>> = (c.cands.iter().chain(&u.cands).flat_map(|c| c.rings.iter().cloned()))
            .chain(c.marks.iter().chain(&u.marks).flat_map(|m| m.1.iter().cloned()))
            .chain(c.wedges.iter().flat_map(|w| w.2.iter().cloned()))
            .collect();
        let seams: Vec<[P; 2]> = c.seams.iter().chain(&u.seams).copied().collect();
        let (patches, segments) = slice(c.win, &strings, &seams);
        stats.slice_segments = segments as u32;
        let n = patches.len();
        let (mut labels, mut under, mut inner) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        for patch in &patches {
            labels.push(label(&c.cands, &c.cover, &c.lines, patch, false));
            under.push(label(&u.cands, &u.cover, &u.lines, patch, false));
            inner.push(inner_point(patch));
        }
        // The box of some rings, doubled as an inner point is.
        let boxed = |rings: &Rings| {
            rings.iter().flatten().fold([i64::MAX, i64::MAX, i64::MIN, i64::MIN], |b, p| {
                let (x, y) = (2 * i64::from(p[0]), 2 * i64::from(p[1]));
                [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
            })
        };
        let holds = |rings: &Rings, b: &[i64; 4], q: [i64; 2]| {
            q[0] >= b[0] && q[0] <= b[2] && q[1] >= b[1] && q[1] <= b[3] && geom::winding(rings, q) != 0
        };
        // Per patch, the bores of the marks that hold its inner point (storey 0's: front; the storey under it's: back),
        // and the floor in front of a portal line that holds a patch where no bore's floor lies under it.
        let marks: Vec<&(u32, Rings)> = c.marks.iter().chain(&u.marks).collect();
        let mark_boxes: Vec<[i64; 4]> = marks.iter().map(|m| boxed(&m.1)).collect();
        let wedge_boxes: Vec<[i64; 4]> = c.wedges.iter().map(|w| boxed(&w.2)).collect();
        let (mut front, mut back) = (vec![Vec::new(); n], vec![Vec::new(); n]);
        let (mut wedge, mut under_id, mut ids) = (vec![TERRAIN; n], vec![NO_UNDER; n], Vec::new());
        for k in 0..n {
            if let Some(q) = inner[k] {
                for (i, m) in marks.iter().enumerate() {
                    if holds(&m.1, &mark_boxes[i], q) {
                        if i < c.marks.len() { &mut front[k] } else { &mut back[k] }.push(m.0);
                    }
                }
                for (w, b) in c.wedges.iter().zip(&wedge_boxes) {
                    if under[k] == TERRAIN && holds(&w.2, b, q) {
                        wedge[k] = w.1;
                        back[k].extend(&w.0);
                        break;
                    }
                }
            }
            // A floor's id: one per label of the storey under the ground or of a floor in front of a portal line.
            let l = if under[k] == TERRAIN { wedge[k] } else { under[k] };
            if l != TERRAIN {
                under_id[k] = match ids.iter().position(|x| *x == l) {
                    Some(i) => i as u32,
                    None => {
                        ids.push(l);
                        ids.len() as u32 - 1
                    }
                };
            }
        }
        // The cover's label, also where the window holds no cover candidate.
        let cover = Label { surface: Surface::Cover, rule: HeightRule::Slope, rank: 0, ..TERRAIN };
        for k in 0..patches.len() {
            if under_id[k] == NO_UNDER {
                if labels[k].surface == Surface::Cover && !front[k].is_empty() {
                    let keep = |x: &Cand| x.label.surface != Surface::Cover;
                    labels[k] = label_but(&c.cands, &c.cover, &c.lines, &patches[k], false, Some(&keep));
                    back[k].clear();
                }
            } else if labels[k].surface != Surface::Cover {
                // The lines that are a lid here: the patch lies in the stroke of their stretches on top of a way of
                // the floor's.
                let ok: Vec<usize> = (c.lid_of.iter())
                    .filter(|x| back[k].contains(&x.0) && inner[k].is_some_and(|q| geom::winding(&x.2, q) != 0))
                    .map(|x| x.1)
                    .collect();
                let keep = |x: &Cand| {
                    x.relabel.is_some()
                        || (x.glob.is_none() && ok.contains(&x.line))
                        || x.label.material.is_some_and(|m| ok.iter().any(|&l| c.lines[l].way == m))
                };
                let l = label_but(&c.cands, &c.cover, &c.lines, &patches[k], false, Some(&keep));
                labels[k] = if l == TERRAIN { cover } else { l };
            }
        }
        Sliced { patches, labels, under, under_id, wedge, front, back, stats }
    }

    /// The patches of the cell's square.
    fn own(&self, cell: CellKey) -> Vec<usize> {
        let sq = window(cell, 0.0);
        let inside = |p: &&P| p[0] >= sq[0] && p[0] <= sq[2] && p[1] >= sq[1] && p[1] <= sq[3];
        (0..self.patches.len()).filter(|&k| self.patches[k].iter().flatten().all(|p| inside(&p))).collect()
    }

    /// The faces of the cell: its own patches. The snap rounding keeps off the cell lines, so they
    /// round as in a slice of the cell alone.
    fn cell(&self, c: &Candidates, cell: CellKey, level: i8) -> FacePlan {
        self.cells(c, cell, level).0
    }

    /// `cell`, and with the bores (`joint`) from the same map the faces of the storey under the ground and of the
    /// floors in front of the portal lines, each face with its floor (`Face::under`).
    fn cells(&self, c: &Candidates, cell: CellKey, level: i8) -> (FacePlan, Option<(FacePlan, FacePlan)>) {
        let own = self.own(cell);
        let patches: Vec<Vec<Vec<P>>> = own.iter().map(|&k| self.patches[k].clone()).collect();
        let map = Map::new(snap_round(&patches));
        let labels: Vec<Label> = own.iter().map(|&k| self.labels[k]).collect();
        let mut stats = self.stats.clone();
        stats.patches = labels.len() as u32;
        // With the bores, each own patch's floor and marks.
        let joint = BORES && !self.under_id.is_empty();
        let pick = |v: &[Vec<u32>]| -> Vec<Vec<u32>> { own.iter().map(|&k| v[k].clone()).collect() };
        let (under, front, back): (Vec<u32>, _, _) = if joint {
            (own.iter().map(|&k| self.under_id[k]).collect(), pick(&self.front), pick(&self.back))
        } else {
            Default::default()
        };
        let floors = joint.then_some(Floors { under: &under, front: &front, back: &back });
        let storeys = joint.then(|| {
            let of = |v: &[Label]| -> Vec<Label> { own.iter().map(|&k| v[k]).collect() };
            let floors = Some(Floors { under: &under, front: &[], back: &[] });
            let plan = |level: i8, labels: &[Label]| output(cell, level, &map, labels, floors, stats.clone());
            (plan(-1, &of(&self.under)), plan(0, &of(&self.wedge)))
        });
        let mut plan = output(cell, level, &map, &labels, floors, stats);
        plan.corners = c.corners.clone();
        plan.mouths = c.mouths.clone();
        let ends = &c.ends;
        // A deck's end passes the rounded corners of every layer of its stroke.
        let tol = if level != 0 { DECK_END_UNITS } else { END_UNITS };
        let on = |v: P, (_, e): &(EdgeKind, [f64; 5])| {
            let r = [f64::from(v[0]) - e[0], f64::from(v[1]) - e[1]];
            (r[0] * e[2] + r[1] * e[3]).abs() <= tol && (r[1] * e[2] - r[0] * e[3]).abs() <= e[4] + tol
        };
        for e in plan.edges.iter_mut().filter(|e| e.kind == EdgeKind::Continuous) {
            let (a, b) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
            // Storey 0's portal edges come from the one map (`output`), by the labels of its patches.
            if let Some((kind, _)) = ends.iter().find(|end| on(a, end) && on(b, end)) {
                if *kind != EdgeKind::Portal || level != 0 {
                    e.kind = *kind;
                }
            }
        }
        (plan, storeys)
    }

    /// The planar map of the whole window, for its faces by any labels.
    fn map(&self) -> Map {
        Map::new(snap_round(&self.patches))
    }

    /// The faces of a map of patches with a label per patch.
    fn plan(&self, cell: CellKey, level: i8, map: &Map, labels: &[Label]) -> FacePlan {
        let mut stats = self.stats.clone();
        stats.patches = labels.len() as u32;
        output(cell, level, map, labels, None, stats)
    }
}

/// A face edge lies on a line's end line where its ends lie within this many units of it: the slice rounds
/// the stroke's whole-unit corners.
const END_UNITS: f64 = 2.0;
/// The same for a deck's end on its end line, whose corners are those of each layer of its stroke, each rounded on its
/// own: they lie a few units apart.
const DECK_END_UNITS: f64 = 4.0;

/// A line's deck end lines (`Portal` at a bore, else `DeckEdge`): the end point, the unit direction into the line (a
/// portal's own, `WayLine::end_dir`) and the half-width of its core (units).
fn deck_ends(line: &WayLine) -> Vec<(EdgeKind, [f64; 5])> {
    let n = line.p.len();
    let end = |k: usize, at: usize, next: usize| {
        let (p, q) = (line.p[at], line.p[next]);
        let d = [f64::from(q[0] - p[0]), f64::from(q[1] - p[1])];
        let len = libm::hypot(d[0], d[1]).max(1e-9);
        let d = line.end_dir[k].map_or([d[0] / len, d[1] / len], |e| if k == 0 { e } else { [-e[0], -e[1]] });
        let kind = if BORES && line.portal[k] { EdgeKind::Portal } else { EdgeKind::DeckEdge };
        (kind, [f64::from(p[0]), f64::from(p[1]), d[0], d[1], line.bands[at][VERGE]])
    };
    let mut out = Vec::new();
    if n >= 2 && line.deck[0] {
        out.push(end(0, 0, 1));
    }
    if n >= 2 && line.deck[1] {
        out.push(end(1, n - 1, n - 2));
    }
    out
}

/// Whether a candidate is a band layer. The cover over a bore is none: it decides the opening at every distance.
fn band(c: &Cand) -> bool {
    c.relabel.is_none()
        && c.fill.is_none()
        && c.glob.is_some()
        && c.label.owner == Owner::Terrain
        && !matches!(c.label.surface, Surface::Zone | Surface::Cover)
}

/// Whether the far level leaves a label out: water (whose bank is a band layer) and a culvert's zone, until the
/// far records hold water.
fn wet_or_zone(l: &Label) -> bool {
    matches!(l.surface, Surface::Water | Surface::Zone)
}

/// Whether the far level leaves a candidate out: a band layer, water or a zone.
fn near_only(c: &Cand) -> bool {
    band(c) || wet_or_zone(&c.label)
}

impl Candidates<'_> {
    /// The labels of the far level, per patch of the slice: no band layers and no water. Each round at a
    /// corner of the paved outline decides its label again without them. They win a point only where
    /// no way does, so only the patches of their labels change.
    fn far(&mut self, s: &Sliced) -> Vec<Label> {
        for k in 0..self.cands.len() {
            let Some(q) = self.cands[k].fill else { continue };
            let at = (self.cover.at(q).into_iter())
                .filter(|&c| self.cands[c as usize].fill.is_none() && !near_only(&self.cands[c as usize]))
                .collect();
            self.cands[k].label = decide(&self.cands, &self.lines, at, q);
        }
        let banded = |l: &Label| (l.owner == Owner::Terrain && l.surface != Surface::Terrain) || wet_or_zone(l);
        (s.patches.iter().zip(&s.labels))
            .map(|(p, l)| if banded(l) { label(&self.cands, &self.cover, &self.lines, p, true) } else { *l })
            .collect()
    }
}

/// The candidates of one cell and level with the band layers below `bands` (`LAYERS` all, `SLOPE`
/// without the slope layer), and the labels of the outline rounds decided. With `records`, every reach grows by
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
    ground: Option<&[WayLine]>,
) -> Candidates<'a> {
    let ctx = Ctx { graph, rules, origin: cell.origin() };
    let plan = &rules.plan;
    let grow = if records { GROW } else { 0 };
    let reach = |m: f64| grown(cell, m, records);
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
    // A storey over the ground holds its deck ways, each a deck's stroke; the storey under it its bores' floors.
    let decks = (STOREYS && level != 0).then(|| deck::Decks::of(graph, input));
    let (mut lines, of_way) = lines_with(
        &ctx,
        input,
        &keep(graph, decks.as_ref(), level),
        reach(rules::ways_m(plan, &rules.sections)),
        ground,
    );
    if decks.is_some() {
        storey_bands(&ctx, &mut lines, level);
    }
    // The points within `bx` round their arms; the points past it only end their runs.
    let elsewhere = |w: u32| decks.as_ref().is_some_and(|d| d.goes_on(graph, w, level));
    let all = graph_points(&ctx, &lines, &of_way, &elsewhere, reach(rules::points_m(plan, &rules.sections)));
    let (mut points, outer): (Vec<Point>, Vec<Point>) = all.into_iter().partition(|pt| near(pt.pos));
    let mut stats = FaceStats::default();
    let junction = |pt: &&Point| matches!(pt.label.owner, Owner::Junction(_));
    stats.junctions = points.iter().filter(|pt| own(pt.pos)).filter(junction).count() as u32;
    stats.joints = points.iter().filter(|pt| own(pt.pos)).count() as u32 - stats.junctions;
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
    // The cover over the bores, the lids (the stretches on top of a bore or of a way that enters one) and the lines
    // with such a stretch.
    let Lid { cover: cover_cands, entering: lids, tops, of_lower: lid_of } = if BORES && level == 0 && any_bore(graph) {
        bore_cover(&ctx, input, &lines, reach(rules::ways_m(plan, &rules.sections)), bx)
    } else {
        Lid::default()
    };
    let mut cands: Vec<Cand> = Vec::new();
    let mut mouths = Vec::new();
    // The ways' carriageways and the band layers, on the run of samples near the window. A way with
    // legs keeps its own layers of the owner rule (its shoulders): `split_legs` gives each piece in a
    // leg's near zone that leg's line as its host, as for the carriageway.
    let mut layers: [Vec<Vec<P>>; LAYERS] = Default::default();
    let mut own_layers: BTreeMap<usize, [Vec<Vec<P>>; LAYERS]> = BTreeMap::new();
    // The carriageways on the runs that only the outline rounds read.
    let mut outer_paved: Vec<Vec<P>> = Vec::new();
    // The marks of the portals by the bore's way: on storey 0 the core strokes of the ground ways that enter it, on the
    // storey under the ground the bore's own stroke.
    let mut mark_pieces: BTreeMap<u32, Vec<Vec<P>>> = BTreeMap::new();
    // Per ground way that enters a bore: its line, its bores and its core stroke.
    let mut entering: Vec<(usize, Vec<u32>, Vec<Vec<P>>)> = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        // The samples of the segments that meet a box.
        let run = |bx: [i32; 4]| {
            let meets = |i: &usize| geom::meets(bx, line.p[*i], line.p[i + 1]);
            let n = line.p.len() - 1;
            Some(((0..n).find(meets)?, (0..n).rfind(meets)? + 2))
        };
        // The cuts at the first and the last sample of layer k (0 the carriageway).
        let cut = |i: usize, k: usize| {
            let portal = if BORES && i == 0 { line.end_cuts[0].clone() } else { None }.or_else(|| {
                if BORES && i + 1 == line.p.len() {
                    line.end_cuts[1].clone()
                } else {
                    None
                }
            });
            portal.or_else(|| end_cuts.get(&(li, i)).cloned()).unwrap_or_else(|| line.offsets(i)[..k].to_vec())
        };
        let carriageway = |(first, last): (usize, usize)| {
            let ends = [line.round[0] && first == 0, line.round[1] && last == line.p.len()];
            let mut own = Vec::new();
            let (p, h) = (&line.p[first..last], &line.h[first..last]);
            pieces_along(p, h, &line.dirs(first, last), [&cut(first, 0), &cut(last - 1, 0)], ends, false, &mut own);
            own
        };
        // The paved outline holds no water.
        let outer = if line.sec.water { None } else { run(lx) };
        let Some((first, last)) = run(bx) else {
            outer_paved.extend(outer.map(|r| prepare(&union(&carriageway(r), [0, 0]))).unwrap_or_default());
            continue;
        };
        if outer != Some((first, last)) {
            outer_paved.extend(outer.map(|r| prepare(&union(&carriageway(r), [0, 0]))).unwrap_or_default());
        }
        let ends = [line.round[0] && first == 0, line.round[1] && last == line.p.len()];
        let p = &line.p[first..last];
        if BORES && (level == 0 || level == -1) && graph.levelled {
            let bore = in_bore(graph, line.way);
            let of: Vec<u32> = match (level, bore) {
                (0, false) => line.leads.clone(),
                (-1, true) => vec![line.way],
                _ => Vec::new(),
            };
            if !of.is_empty() {
                let layer = if bore { SLOPE } else { VERGE };
                // An opening lies on a bore's end line: the bore's own mark is the stroke of its end segment at each
                // portal in the run, so a way that passes over the bore elsewhere opens nothing there.
                let n = line.p.len();
                let runs: Vec<(usize, usize)> = if bore {
                    let at =
                        [(first == 0 && line.end_dir[0].is_some(), 0), (last == n && line.end_dir[1].is_some(), n - 2)];
                    at.iter().filter(|x| x.0).map(|x| (x.1, x.1 + 2)).collect()
                } else {
                    vec![(first, last)]
                };
                let mut pieces = Vec::new();
                for (a, b) in runs {
                    let d: Vec<f64> = (a..b).map(|i| line.bands[i][layer]).collect();
                    let cuts = [&cut(a, layer + 1)[..], &cut(b - 1, layer + 1)[..]];
                    let ends = [line.round[0] && a == 0, line.round[1] && b == n];
                    pieces_along(&line.p[a..b], &d, &line.dirs(a, b), cuts, ends, false, &mut pieces);
                }
                for &w in &of {
                    mark_pieces.entry(w).or_default().extend(pieces.iter().cloned());
                }
                if !bore {
                    entering.push((li, of, pieces));
                }
            }
        }
        let own = carriageway((first, last));
        cands.push(Cand {
            rings: union(&own, [0, 0]),
            label: way_label(rules, line, legs::leg(&line.cuts, first, 1)).0,
            key: way_key(rules, line),
            glob: None,
            line: li,
            relabel: None,
            beyond: Vec::new(),
            fill: None,
            discs: Vec::new(),
        });
        // A deck's band layers are its own: the slab's edge band names its way.
        let mut own = (!line.cuts.is_empty() || decks.is_some()).then(|| own_layers.entry(li).or_default());
        // So are the paved layers of a way on its segments on top of a lower way: their heights are that way's, not
        // those of the way under it, and over a floor they keep their place as a lid (`Sliced::joint`). They are those
        // segments' pieces of the band layers' own stroke, so the two meet on its mitre lines.
        let top = tops.get(li).filter(|t| BORES && own.is_none() && !t.is_empty());
        let mut named: [Vec<Vec<P>>; LAYERS] = Default::default();
        // At a deck end the slope takes the round end of a free end, so its bank runs round the end: the
        // paved layers end square on the end line. A portal is no free end: the ground goes on over the bore, so
        // the bank ends square on the portal line too.
        let free = |k: usize| line.deck[k] && !(BORES && line.portal[k]);
        let deck = [free(0) && first == 0, free(1) && last == line.p.len()];
        for k in 0..bands {
            let inner = |i: usize| if k == 0 { line.h[i] } else { line.bands[i][k - 1] };
            let d: Vec<f64> = (first..last).map(|i| line.bands[i][k]).collect();
            if (first..last).all(|i| line.bands[i][k] <= inner(i)) {
                continue;
            }
            let cuts = [&cut(first, k + 1)[..], &cut(last - 1, k + 1)[..]];
            let layer = match own.as_deref_mut() {
                Some(own) if decks.is_some() || BAND_LABELS[k].2 == HeightRule::Owner => &mut own[k],
                _ => &mut layers[k],
            };
            let ends = if k == SLOPE { [ends[0] || deck[0], ends[1] || deck[1]] } else { ends };
            match top.filter(|_| BAND_LABELS[k].2 == HeightRule::Owner) {
                Some(t) => {
                    let (mut pieces, mut of) = (Vec::new(), Vec::new());
                    let dirs = line.dirs(first, last);
                    stroke::pieces_along_of(p, &d, &dirs, cuts, ends, false, &mut pieces, Some(&mut of));
                    for (ring, [a, b]) in pieces.into_iter().zip(of) {
                        let on_top = (first + a..=first + b).all(|i| t[i]);
                        if on_top { &mut named[k] } else { &mut *layer }.push(ring);
                    }
                }
                None => pieces_along(p, &d, &line.dirs(first, last), cuts, ends, false, layer),
            }
        }
        if top.is_some() {
            let own = own_layers.entry(li).or_default();
            for (k, layer) in named.into_iter().enumerate() {
                own[k].extend(layer);
            }
        }
    }
    stats.ways = lines.len() as u32;
    if level == 0 {
        cands.extend(platform::candidates(&ctx, input, bx, &mut stats));
        // A culvert's zone lies within `REACH_M` of its crossing.
        let (zones, found) = culverts::candidates(&ctx, &lines, reach(culverts::REACH_M));
        cands.extend(zones);
        mouths = found;
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
            });
        }
        if !r.region.is_empty() {
            let rings = union(&r.region, pt.pos);
            let spans = Some(r.spans.clone());
            let beyond = Vec::new();
            let (key, discs) = (pt.key, Vec::new());
            cands.push(Cand { rings, label: pt.label, key, glob, line: 0, relabel: spans, beyond, fill: None, discs });
        }
        // A point's rings belong to the one way with legs among its arms, if there is one; on a deck, to the way
        // of its label.
        let mut legged: Vec<usize> = pt.arms.iter().map(|a| a.line).filter(|&l| !lines[l].cuts.is_empty()).collect();
        legged.sort_unstable();
        legged.dedup();
        let deck_owner = pt.label.material.and_then(|m| of_way.get(&m).copied()).filter(|_| STOREYS && decks.is_some());
        for k in 0..bands {
            let layer = match (deck_owner, &legged[..]) {
                (Some(l), _) => &mut own_layers.entry(l).or_default()[k],
                (None, [l]) if BAND_LABELS[k].2 == HeightRule::Owner => &mut own_layers.entry(*l).or_default()[k],
                _ => &mut layers[k],
            };
            layer.extend(r.rings[k].iter().cloned());
        }
    }
    // A way's own band layers win over the others where they overlap: the way's shoulder is its own.
    let owned = own_layers.iter().flat_map(|(&l, layers)| layers.iter().enumerate().map(move |(k, r)| (Some(l), k, r)));
    for (line, k, layer) in layers.iter().enumerate().map(|(k, r)| (None, k, r)).chain(owned) {
        if layer.is_empty() {
            continue;
        }
        let (rank, surface, rule) = band_label(k, STOREYS && decks.is_some());
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
        });
    }
    // Where a way on top lies over the last metres of a way that enters a bore, that way's face is a floor under the
    // way on top, as behind the portal line: its paved layers lose the place, and the place is a floor of its own
    // label (`wedges`).
    let mut wedges: Vec<(Vec<u32>, Label, Rings)> = Vec::new();
    if level == 0 && BORES {
        cands.extend(cover_cands);
        for (li, of, pieces) in &entering {
            let Some((_, lids)) = lids.iter().find(|x| x.0 == lines[*li].way) else { continue };
            let rings = stroke::boolean(&union(pieces, [0, 0]), lids, OverlayRule::Intersect, [0, 0]);
            if rings.is_empty() {
                continue;
            }
            let way = lines[*li].way;
            for c in cands.iter_mut().filter(|c| {
                let carriageway = c.glob.is_none() && c.line == *li && matches!(c.label.owner, Owner::Way(_));
                let layer = c.label.owner == Owner::Terrain
                    && c.label.material == Some(way)
                    && c.label.rule == HeightRule::Owner;
                c.relabel.is_none() && c.fill.is_none() && (carriageway || layer)
            }) {
                c.rings = stroke::boolean(&c.rings, lids, OverlayRule::Difference, [0, 0]);
            }
            // With the way itself: its lids are the lids of the wedge (`Sliced::joint`).
            let of: Vec<u32> = of.iter().copied().chain([way]).collect();
            wedges.push((of, way_label(rules, &lines[*li], 0).0, rings));
        }
    }
    for c in &mut cands {
        c.rings = prepare(&c.rings);
    }
    let mut marks: Vec<(u32, Rings)> =
        mark_pieces.into_iter().map(|(w, pieces)| (w, prepare(&union(&pieces, [0, 0])))).collect();
    for w in &mut wedges {
        w.2 = prepare(&w.2);
    }
    let pieces = split_legs(&ctx, &lines, &of_way, &mut cands);
    // The rounds at the corners of the paved outline that no point's round shapes. Their key loses
    // to every carriageway and rail bed, so a round paves land only. `lines_m` holds their reach.
    let mut paved: Vec<Vec<P>> = (cands.iter())
        .filter(|c| c.relabel.is_none() && matches!(c.label.surface, Surface::Carriageway | Surface::RailBed))
        .flat_map(|c| c.rings.iter().cloned())
        .collect();
    paved.extend(outer_paved);
    let circles: Vec<(P, f64)> = rounded.iter().flat_map(|r| r.circles.iter().map(|c| (c.centre, c.radius))).collect();
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
    for (k, &(rank, surface, rule)) in BAND_LABELS.iter().enumerate() {
        let rings: Vec<Vec<P>> = fills.iter().flat_map(|f| f.bands[k].iter().cloned()).collect();
        // A deck's band layers are its ways' own: a round's shared band would name no way.
        if rings.is_empty() || k >= bands || (STOREYS && decks.is_some()) {
            continue;
        }
        let rank = plan.rank(rank);
        let label = Label { rank, surface, rule, ..TERRAIN };
        let (rings, glob, beyond) = (prepare(&rings), Some([0; 5]), Vec::new());
        let (key, discs) = ((rank, 1, 0), Vec::new());
        cands.push(Cand { rings, label, key, glob, line: 0, relabel: None, beyond, fill: None, discs });
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
        });
    }
    // Each seam cuts its line's carriageway and the fans that reach across it.
    let mut seams: Vec<(usize, [P; 2], Vec<usize>)> = (seam_points.into_iter())
        .map(|((l, s), pts)| (l, s, pts.iter().filter_map(|&p| fan_of[p]).flat_map(|c| pieces[c].clone()).collect()))
        .collect();
    // A seam off the window cuts nothing of it.
    seams.retain(|&(_, [a, b], _)| geom::meets(win, a, b));
    let seams = seams_on_carriageways(&mut cands, &seams);
    // The seams and rings split at grid lines of `step`, as `grid_points` puts their points.
    let split = |seams: &[[P; 2]], step: i64| -> Vec<[P; 2]> {
        (seams.iter())
            .flat_map(|&[a, b]| {
                let mut pts = vec![a];
                grid_points(a, b, step, &mut pts);
                pts.push(b);
                pts.dedup();
                pts.windows(2).map(|w| [w[0], w[1]]).collect::<Vec<_>>()
            })
            .collect()
    };
    // A seam meets the cell lines where the carriageway's ring does.
    let mut seams = split(&seams, CELL_UNITS);
    let square = window(cell, 0.0);
    merge_crossings(
        square,
        &mut (cands.iter_mut().map(|c| &mut c.rings))
            .chain(marks.iter_mut().map(|m| &mut m.1))
            .chain(wedges.iter_mut().map(|w| &mut w.2))
            .collect::<Vec<_>>(),
        &mut seams,
    );
    cands.retain(|c| !c.rings.is_empty());
    // The last change of the rings and seams: a vertex at each tile line they cross. No tile of the slice
    // then rounds a crossing of its own side, which the overlay would snap onto a tile corner that the
    // next cell holds without the ring, and the cover and the labels read the edges that the slice cuts.
    for c in &mut cands {
        c.rings = c.rings.iter().map(|r| with_grid_lines(r, slice::TILE)).collect();
    }
    for m in &mut marks {
        m.1 = m.1.iter().map(|r| with_grid_lines(r, slice::TILE)).collect();
    }
    for w in &mut wedges {
        w.2 = w.2.iter().map(|r| with_grid_lines(r, slice::TILE)).collect();
    }
    let seams = split(&seams, slice::TILE);
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
        .collect();
    let fill_label = |f: &outline::Fill| cands.iter().find(|c| c.fill == Some(f.inside)).map(|c| c.label);
    for f in &fills {
        if let Some(l) = fill_label(f).filter(|l| l.surface != Surface::Terrain) {
            corners.push(CornerArc::new(l.owner, &f.circle, mpu(f.circle.centre)));
        }
    }
    let ways = lines.iter().take_while(|l| !l.sec.water).count();
    let ends: Vec<(EdgeKind, [f64; 5])> = match &decks {
        Some(d) => lines.iter().flat_map(|l| deck::open_ends(l, d.open[l.way as usize])).collect(),
        None => lines.iter().flat_map(deck_ends).collect(),
    };
    Candidates { lines, ways, cands, seams, win, cover, corners, mouths, ends, marks, wedges, lid_of, stats }
}

/// The samples of a line whose segments meet a box: the run `first..last`.
fn run_in(line: &WayLine, bx: [i32; 4]) -> Option<(usize, usize)> {
    let meets = |i: &usize| geom::meets(bx, line.p[*i], line.p[i + 1]);
    let n = line.p.len() - 1;
    Some(((0..n).find(meets)?, (0..n).rfind(meets)? + 2))
}

/// The lid over the bores near a cell (`bore_cover`).
#[derive(Default)]
struct Lid {
    /// The cover: one candidate of storey 0 over each bore's stroke (its core and its wall) from end line to end line,
    /// less its lids. Its key wins every face: over a floor, only a way on top keeps its own surface (`Sliced::joint`).
    /// Its height is the ground's (the slope rule) over the floor bound (`heights::floor`).
    cover: Vec<Cand>,
    /// Per way that enters a bore, the strokes of the stretches on top of it.
    entering: Vec<(u32, Rings)>,
    /// Per line of storey 0 and segment, whether it lies on top of a lower way (empty for a line with none).
    tops: Vec<Vec<bool>>,
    /// Per lower way (a bore, or a way that enters one), the lines with a stretch on top of it and their strokes.
    of_lower: Vec<(u32, usize, Rings)>,
}

/// The lid over the bores near the cell (with the bores), from storey 0's lines `ground`.
fn bore_cover(ctx: &Ctx, input: &CellInput, ground: &[WayLine], ways_box: [i32; 4], bx: [i32; 4]) -> Lid {
    let g = ctx.graph;
    let (bores, _) = lines_with(ctx, input, &|w| in_bore(g, w), ways_box, Some(ground));
    // Per bore, its stroke: its core and its wall.
    let mut strokes: Vec<Vec<Vec<P>>> = vec![Vec::new(); bores.len()];
    for (line, rings) in bores.iter().zip(&mut strokes) {
        let Some((first, last)) = run_in(line, bx) else { continue };
        let own = |i: usize| line.offsets(i)[..=VERGE + 1].to_vec();
        let c0 = if first == 0 { line.end_cuts[0].clone() } else { None }.unwrap_or_else(|| own(first));
        let c1 = if last == line.p.len() { line.end_cuts[1].clone() } else { None }.unwrap_or_else(|| own(last - 1));
        let d: Vec<f64> = (first..last).map(|i| line.bands[i][VERGE] + ctx.units(deck::WALL_M, line.p[i])).collect();
        pieces_along(&line.p[first..last], &d, &line.dirs(first, last), [&c0, &c1], [false; 2], false, rings);
    }
    if strokes.iter().all(Vec::is_empty) {
        return Lid::default();
    }
    // A way lies on top of a lower way where its surface lies at least the lower way's least room over the lower way's
    // floor at every place where their strokes meet: the heights decide, not a crossing of the centre lines. A way
    // beside the bore or level with it (the other tube's approach) is none: it ends at the side of the opening.
    // The lower ways' centre lines with their heights (the graph's samples), their reach (core and wall, units) and
    // their least room (the least bore and the roof's least plate, metres).
    type Poly = Vec<([f64; 2], f64)>;
    let poly = |w: u32| -> Poly {
        let r = &g.ways[w as usize].samples;
        g.samples[r.start as usize..r.end as usize]
            .iter()
            .map(|s| ([f64::from(s.pos[0]), f64::from(s.pos[1])], s.z_m))
            .collect()
    };
    // The lower ways: the bores, and the ground ways that enter one (a way on top of a bore can reach over the last
    // metres of the way that enters it: there it is that way's lid too).
    let enters = |l: &&WayLine| !l.leads.is_empty() && !l.sec.water;
    let under: Vec<(Poly, f64, f64, u32, [f64; 4])> = (bores.iter().map(|b| (b, ctx.units(deck::WALL_M, b.p[0]))))
        .chain(ground.iter().filter(enters).map(|a| (a, 0.0)))
        .map(|(b, wall)| {
            let core = b.bands.iter().map(|x| x[VERGE]).fold(0.0, f64::max) + wall;
            let l = poly(b.way);
            // The box of its centre line.
            let bx = l.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |x, (p, _)| {
                [x[0].min(p[0]), x[1].min(p[1]), x[2].max(p[0]), x[3].max(p[1])]
            });
            (l, core, b.sec.bore_least_m + b.sec.roof_least_m, b.way, bx)
        })
        .collect();
    // The height of a bore's floor at the foot of q on its line, and the distance to it (units).
    let foot = |q: [f64; 2], l: &Poly| -> Option<(f64, f64)> {
        l.windows(2)
            .map(|w| {
                let d = [w[1].0[0] - w[0].0[0], w[1].0[1] - w[0].0[1]];
                let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-9);
                let t = (((q[0] - w[0].0[0]) * d[0] + (q[1] - w[0].0[1]) * d[1]) / l2).clamp(0.0, 1.0);
                (libm::hypot(q[0] - w[0].0[0] - t * d[0], q[1] - w[0].0[1] - t * d[1]), w[0].1 + t * (w[1].1 - w[0].1))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
    };
    // A stretch of a ground way between two of its samples lies on top where, at each of its points about 2 m apart,
    // its surface lies at least the least room over the floor of every bore whose stroke its own stroke meets there;
    // a stretch that meets no bore is not asked.
    // Per lower way, the core strokes of the stretches on top of it: its lids.
    let mut cores: Vec<Vec<Vec<P>>> = vec![Vec::new(); under.len()];
    let mut tops: Vec<Vec<bool>> = vec![Vec::new(); ground.len()];
    // Per lower way and line with a stretch on top of it, the strokes of those stretches.
    let mut lid_of: Vec<(u32, usize, Rings)> = Vec::new();
    for (li, line) in ground.iter().enumerate() {
        let Some((first, last)) = run_in(line, bx) else { continue };
        let core_a = line.bands.iter().map(|b| b[VERGE]).fold(0.0, f64::max);
        let way = &g.ways[line.way as usize];
        let samples = &g.samples[way.samples.start as usize..way.samples.end as usize];
        // The height at vertex i of the line: of its graph sample, or between the two about a taper's vertex.
        let z = |i: usize| -> f64 {
            let k = line.index.partition_point(|&x| x <= i).saturating_sub(1);
            if line.index[k] == i || k + 1 >= samples.len() {
                return samples[k.min(samples.len() - 1)].z_m;
            }
            let (a, c) = (line.index[k], line.index[k + 1]);
            samples[k].z_m + (i - a) as f64 / (c - a) as f64 * (samples[k + 1].z_m - samples[k].z_m)
        };
        // Whether stretch i lies on top of lower way u: at each of its points whose stroke meets u's, at least u's
        // least room over u's floor; and it meets u.
        let on_top = |i: usize, u: &(Poly, f64, f64, u32, [f64; 4])| -> bool {
            let (l, core_b, need, way, bx) = u;
            if *way == line.way {
                return false;
            }
            let (a, c) = (line.p[i], line.p[i + 1]);
            let (a, c) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(c[0]), f64::from(c[1])]);
            let r = core_a + core_b;
            // The stretch's box off the lower way's box, grown by their reach: they do not meet.
            if a[0].max(c[0]) < bx[0] - r
                || a[0].min(c[0]) > bx[2] + r
                || a[1].max(c[1]) < bx[1] - r
                || a[1].min(c[1]) > bx[3] + r
            {
                return false;
            }
            let n = (libm::hypot(c[0] - a[0], c[1] - a[1]) / 1200.0).ceil().max(1.0) as usize;
            let mut meets = false;
            for k in 0..=n {
                let t = k as f64 / n as f64;
                let (q, zq) = ([a[0] + t * (c[0] - a[0]), a[1] + t * (c[1] - a[1])], z(i) + t * (z(i + 1) - z(i)));
                if let Some((_, zb)) = foot(q, l).filter(|x| x.0 <= r) {
                    if zq - zb < *need {
                        return false;
                    }
                    meets = true;
                }
            }
            meets
        };
        let ends = [line.round[0] && first == 0, line.round[1] && last == line.p.len()];
        for (u, cores) in under.iter().zip(&mut cores) {
            let top: Vec<bool> = (first..last - 1).map(|i| on_top(i, u)).collect();
            if top.contains(&true) {
                let t = &mut tops[li];
                t.resize(line.p.len() - 1, false);
                for (i, &x) in top.iter().enumerate() {
                    t[first + i] |= x;
                }
            }
            let before = cores.len();
            let mut i = 0;
            while i < top.len() {
                if !top[i] {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j < top.len() && top[j] {
                    j += 1;
                }
                let (lo, hi) = (first + i, first + j + 1);
                // A lid is the way's own surface: its carriageway and its shoulders (the layers of the owner rule). Its
                // verge is ground.
                let d: Vec<f64> = (lo..hi).map(|k| line.bands[k][1]).collect();
                let round = [ends[0] && lo == 0, ends[1] && hi == line.p.len()];
                pieces_along(&line.p[lo..hi], &d, &line.dirs(lo, hi), [&[], &[]], round, false, cores);
                i = j;
            }
            if cores.len() > before {
                lid_of.push((u.3, li, union(&cores[before..], [0, 0])));
            }
        }
    }
    // The cover: each bore's stroke less its own lids. The lids of the ways that enter a bore, by way.
    let lids: Vec<Rings> = cores.iter().map(|c| union(c, [0, 0])).collect();
    let mut rings = Vec::new();
    for (stroke, lid) in strokes.iter().zip(&lids).filter(|x| !x.0.is_empty()) {
        rings.extend(stroke::boolean(&union(stroke, [0, 0]), lid, OverlayRule::Difference, [0, 0]));
    }
    let rings = union(&rings, [0, 0]);
    let lids: Vec<(u32, Rings)> =
        (under.iter().zip(lids)).skip(bores.len()).filter(|x| !x.1.is_empty()).map(|(u, lid)| (u.3, lid)).collect();
    let label = Label { surface: Surface::Cover, rule: HeightRule::Slope, rank: 0, ..TERRAIN };
    let cover = vec![Cand {
        rings,
        label,
        key: (0, 0, 0),
        glob: Some([i64::MIN; 5]),
        line: 0,
        relabel: None,
        beyond: Vec::new(),
        fill: None,
        discs: Vec::new(),
    }];
    Lid { cover, entering: lids, tops, of_lower: lid_of }
}

/// Splits every candidate that gives the label of a way with legs (`legs`), or that is one of its own
/// band layers: the medial lines between its legs cut it, each piece goes to its nearest leg, whose line
/// is its host, and each leg's part is the union of its pieces, which share their vertices. Where two
/// legs overlap, the nearest one owns the ground, and the step between them stands on the medial line
/// (blended where they agree). A candidate keeps its place with its first leg's part; the others go to
/// the end. Returns per candidate the places of its parts.
fn split_legs(ctx: &Ctx, lines: &[WayLine], of_way: &BTreeMap<u32, usize>, cands: &mut Vec<Cand>) -> Vec<Vec<usize>> {
    let mut parts: Vec<Vec<usize>> = (0..cands.len()).map(|c| vec![c]).collect();
    // Per line, its legs, the medial lines between them, the near zone of each leg (`Legs::runs`), and
    // the medial lines with the zones' edges.
    type Cuts = (legs::Legs, Vec<[P; 2]>, Vec<Rings>, Vec<[P; 2]>);
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
        let (legs, medial, zones, edges) = legs_of[l].get_or_insert_with(|| {
            let legs = legs::Legs::new(&line.p, &line.cuts, &|o| ctx.units(line.leg_reach_m(), o));
            let medial = legs.medial(&|o| ctx.units(LEG_STEP_M, o));
            // A leg's near zone: its stretches' strokes, as wide as its reach, with round ends, so the
            // zones of two legs leave no gap about their cut.
            let mut strokes = vec![Vec::new(); line.cuts.len() + 1];
            for (k, run, r) in legs.runs() {
                stroke::pieces_along(&run, &vec![r; run.len()], &[], [&[], &[]], [true, true], false, &mut strokes[k]);
            }
            let zones: Vec<Rings> = strokes.iter().map(|s| prepare(&union(s, anchor))).collect();
            let mut edges = medial.clone();
            for r in zones.iter().flatten() {
                edges.extend((0..r.len()).map(|i| [r[i], r[(i + 1) % r.len()]]));
            }
            (legs, medial, zones, edges)
        });
        // A band also splits at the edge of the near zone: its pieces in the zone take their nearest
        // leg's line as their host, and any piece outside stays plain, so its label does not depend on a
        // cut that another cell may not see.
        let band = cands[c].label.owner == Owner::Terrain;
        let label = cands[c].label;
        let mut by_label: Vec<(Label, Rings)> = Vec::new();
        for piece in stroke::cut(&cands[c].rings, if band { edges } else { medial }, anchor) {
            let q = inner_point(&piece).unwrap_or_else(|| {
                let n = piece[0].len() as i64;
                let sum = piece[0].iter().fold([0i64; 2], |s, p| [s[0] + i64::from(p[0]), s[1] + i64::from(p[1])]);
                [(2 * sum[0]).div_euclid(n), (2 * sum[1]).div_euclid(n)]
            });
            let leg = legs.nearest([q[0] as f64 / 2.0, q[1] as f64 / 2.0]);
            let part = match band && zones.iter().all(|z| geom::winding(z, q) == 0) {
                true => Label { material: None, ..label },
                false => Label { legs: [leg, label.legs[1]], ..label },
            };
            match by_label.iter_mut().find(|(l, _)| *l == part) {
                Some((_, rings)) => rings.extend(piece),
                None => by_label.push((part, piece)),
            }
        }
        if by_label.len() < 2 {
            cands[c].label = by_label.first().map_or(label, |s| s.0);
            continue;
        }
        let split: Vec<(Label, Rings)> =
            by_label.into_iter().map(|(l, rings)| (l, prepare(&union(&rings, anchor)))).collect();
        for (k, (part_label, rings)) in split.into_iter().enumerate() {
            let o = &cands[c];
            let part = Cand {
                rings,
                label: part_label,
                key: o.key,
                glob: o.glob,
                line: o.line,
                relabel: None,
                beyond: o.beyond.clone(),
                fill: o.fill,
                discs: o.discs.clone(),
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

/// The label of each band layer (`LAYERS`). A way with legs keeps its layers of the owner rule as its
/// own, and the nearest leg hosts each of their pieces (`split_legs`).
const BAND_LABELS: [(Rank, Surface, HeightRule); LAYERS] = [
    (Rank::ShoulderPaved, Surface::Shoulder, HeightRule::Owner),
    (Rank::Shoulder, Surface::Shoulder, HeightRule::Owner),
    (Rank::Verge, Surface::Verge, HeightRule::Slope),
    (Rank::Bank, Surface::Bank, HeightRule::Slope),
    (Rank::Slope, Surface::Slope, HeightRule::Slope),
];

/// The label of band layer k; on a deck, the slope's place holds the deck's edge band, of the sheet's lowest rank,
/// at its way's height.
fn band_label(k: usize, deck: bool) -> (Rank, Surface, HeightRule) {
    if deck && k == SLOPE {
        (Rank::Slope, Surface::DeckEdge, HeightRule::Owner)
    } else {
        BAND_LABELS[k]
    }
}

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
    label_but(cands, cover, lines, patch, far, None)
}

/// `label` of the candidates that `keep` keeps (`Sliced::joint`).
fn label_but(
    cands: &[Cand],
    cover: &Cover,
    lines: &[WayLine],
    patch: &[Vec<P>],
    far: bool,
    keep: Option<&dyn Fn(&Cand) -> bool>,
) -> Label {
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
        at.retain(|&c| !near_only(&cands[c as usize]));
    }
    if let (true, Some(keep)) = (BORES, keep) {
        at.retain(|&c| keep(&cands[c as usize]));
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

/// Whether a label's material way is line water.
fn of_water(lines: &[WayLine], l: &Label) -> bool {
    l.material.is_some_and(|m| lines.iter().any(|x| x.way == m && x.sec.water))
}

/// The label at a doubled point q that the candidates `at` cover. Where a zone covers q, a candidate of line
/// water does not count: the water ends at the zone.
fn decide(cands: &[Cand], lines: &[WayLine], mut at: Vec<u32>, q: [i64; 2]) -> Label {
    if at.iter().any(|&c| cands[c as usize].label.surface == Surface::Zone) {
        at.retain(|&c| !of_water(lines, &cands[c as usize].label));
    }
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
    // of the way's seams. A zone keeps itself.
    if let (Owner::Way(w), None, false) = (label.owner, across, label.surface == Surface::Zone) {
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
/// line, form a face. With the bores (`floors`), patches of one label over two floors are two faces, and an opening
/// edge is a `Portal` with the entering way's face on the left where `FRONT_LEFT` says so.
fn output(
    cell: CellKey,
    level: i8,
    map: &Map,
    labels: &[Label],
    floors: Option<Floors>,
    mut stats: FaceStats,
) -> FacePlan {
    let floors = floors.filter(|_| BORES);
    let under_of = |k: usize| floors.map_or(NO_UNDER, |f| f.under[k]);
    let mut ids: Vec<(Label, u32)> = Vec::new();
    let key: Vec<u32> = labels
        .iter()
        .enumerate()
        .map(|(k, l)| match ids.iter().position(|x| x.0 == *l && x.1 == under_of(k)) {
            Some(i) => i as u32,
            None => {
                ids.push((*l, under_of(k)));
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
            let under = under_of(g as usize);
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
                under,
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
        let mut kind =
            if on_cell_line(map.point(e.a), map.point(e.b)) { EdgeKind::CellBorder } else { EdgeKind::Continuous };
        // An opening edge: a floor lies under the patch on one side, and on the other side lies the stroke of a way that
        // enters that floor's bore, with no floor under it. It reads labels of patches, no distance.
        let mut flags = 0;
        if let (Some(Floors { under: u, front, back }), true, true) =
            (floors.filter(|f| !f.front.is_empty()), e.twin != NONE, kind == EdgeKind::Continuous)
        {
            let (p, q) = (e.patch as usize, map.half[e.twin as usize].patch as usize);
            let meets = |f: usize, b: usize| {
                u[f] == NO_UNDER && u[b] != NO_UNDER && front[f].iter().any(|w| back[b].contains(w))
            };
            let (front_here, front_there) = (meets(p, q), meets(q, p));
            if front_here || front_there {
                kind = EdgeKind::Portal;
                // The half-edge's own patch lies on its left before the swap below.
                let unswapped = index(e.a) < index(e.b);
                if front_here == unswapped {
                    flags |= FRONT_LEFT;
                }
            }
        }
        edges.push(Edge { a, b, left, right, kind, flags });
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
        mouths: Vec::new(),
        stats,
    }
}
