//! The way graph of one cell: every way and waterway as its curve with samples at global parameters,
//! split at the cell lines; the junctions with their hosts, the joints and the ends; snapped ends and
//! their links; a level per way piece; and every crossing with its type. `levels` adds the step of the
//! ladder of each crossing and the states and end lines of the bridge and tunnel ways.
//!
//! Every decision reads only data within its reach of the element it decides (the rows of
//! `reach_rows`), so two cells give the same graph where both hold complete data.

mod crossings;
mod curve;
mod levels;
mod parts;
mod points;
mod snap;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::frame::{metres_per_unit, CellKey, CELL_UNITS, FRAME_BITS, MARGIN_M};
use crate::sections::{leading_number, Reach, Sections, WaySection};
use crate::CellInput;

pub use crossings::end_on_chord;
pub(crate) use curve::{folds, water_spans, water_tangents};
pub use curve::{point, s_at, u_at, water_curves, water_grades, water_level, SpanPoint, WaterCurve};
pub use levels::{check_levels, levels, step, EndKind, EndLine, Ladder, Levels, Run, State, Step, Structures};

/// The coarsest sample step: the grid of level l is `BASE_STEP_M / 2^l` along a span.
pub const BASE_STEP_M: f64 = 16.0;
/// The finest step, level 7; a grid point closer than half of it to a break goes.
pub const MIN_STEP_M: f64 = 0.125;
const LEVELS: u32 = 7;
/// Arc knots per span: `s(u)` is linear between them.
pub const KNOTS: usize = 16;
/// An arm's heading points at the curve this far along: the bake's `LOOK_M`.
pub const HEADING_M: f64 = 8.0;
/// GP3's longest read along an arm: a return ramp of 4.5 m at the road grade 0.15.
pub const ROOM_MAX_M: f64 = 30.0;
/// A snap reads this far: `HALF_MAX_M` 15 m, `snap_m` and 4 m of widening.
pub const SNAP_REACH_M: f64 = 20.0;
/// The half-length of a synthetic deck: a deck over water up to 76 m wide at 90°.
pub const DECK_HALF_MAX_M: f64 = 40.0;
/// An unshared vertex where the chords turn by more than this is a spike.
pub const SPIKE_DEG: f64 = 175.0;
/// A joint whose tangents differ by less than this runs on in one stroke.
pub const STROKE_DEG: f64 = 0.5;
/// A synthetic deck reaches this far past the outer edge of what lies under it.
pub const DECK_SIDE_M: f64 = 1.0;
/// A synthetic deck over water also spans this much bank on each side.
pub const WATER_BANK_M: f64 = 1.0;
/// Bisection steps of a cell-line root: 2^-44 of a span after the 16 knots.
pub const ROOT_STEPS: u32 = 40;
/// The side of a bucket of the crossing and snap grid.
const BUCKET_M: f64 = 16.0;
/// A crossing's stations lie where the two curves cross nearest the chords' crossing, within this.
const PLACE_M: f64 = 16.0;
/// Newton steps of a snap's foot on the curve, after the projection on the samples.
const NEWTON_STEPS: u32 = 4;
/// `side` bits of a vertex: the way out back along the part, and on.
pub const SIDE_BACK: u8 = 1;
pub const SIDE_ON: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Road,
    Path,
    Rail,
    Water,
}

/// The families that join at a shared vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Network {
    Way,
    Rail,
    Water,
}

impl Family {
    pub fn network(self) -> Network {
        match self {
            Family::Road | Family::Path => Network::Way,
            Family::Rail => Network::Rail,
            Family::Water => Network::Water,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ground,
    Deck,
    Tunnel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkKind {
    EndToEnd,
    Carriageway,
}

/// The index of a feature in the cell's input; a link has the feature of its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FeatureRef(pub u32);

/// A place on a part's curve: a vertex v is (v, 0), the last vertex (n − 2, 1).
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct Station {
    pub part: u32,
    pub span: u32,
    pub u: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vertex {
    pub pos: [i64; 2],
    /// Water in a tunnel with no level has 0 here: no height.
    pub z_m: f64,
    /// Radians from +x to +y.
    pub tangent: f64,
    pub grade: f64,
    pub bank: f64,
    pub widen_m: f64,
    pub walk: u8,
    pub side: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub len_m: f64,
    /// Span metres at u = k / 16.
    pub knots: [f64; KNOTS + 1],
    /// The stroke metres at the start vertex in the tile that owns the span.
    pub along0_m: f64,
    /// The span is its chord with linear heights: no alignment, or a folded span.
    pub chord: bool,
    /// The tangent angles at its start and its end: its vertices', or for water `water_spans`'s.
    pub ends: [f64; 2],
    /// The length of the end tangents as a share of the chord: 1, or less where a water curve would
    /// leave its chord too far.
    pub handle: f64,
    /// The heights are the level of water along the arc between the vertices (`water_level`).
    pub water_z: bool,
    /// The zoom-13 tile (column, row) that owns the span, and the feature's position in that tile.
    pub owner: [u32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub feature: FeatureRef,
    pub family: Family,
    /// The rank: the row of `roads.json`, or of `waterways.json` for water.
    pub class: u8,
    /// The level of the feature. A way of the part can lie on another: read `Way::level`.
    pub level: Level,
    pub backward: bool,
    pub vertices: Vec<Vertex>,
    pub spans: Vec<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SampleKind {
    Vertex,
    CellLine,
    Grid,
    Station,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// Frame units from the cell's origin.
    pub pos: [i32; 2],
    pub span: u32,
    pub u: f64,
    pub s_m: f64,
    pub along_m: f64,
    pub z_m: f64,
    pub tangent: [f64; 2],
    pub grade: f64,
    pub bank: f64,
    pub widen_m: f64,
    pub walk: u8,
    pub kind: SampleKind,
}

/// A piece of a part with one level: its curve is the part's spans from `from` to `to`. Its `level`
/// is the one to read: a synthetic deck is a `Deck` way of a `Ground` part.
#[derive(Clone, Debug, PartialEq)]
pub struct Way {
    pub id: u32,
    pub part: u32,
    pub from: Station,
    pub to: Station,
    pub family: Family,
    pub class: u8,
    pub level: Level,
    pub deck: Option<u32>,
    pub link: Option<LinkKind>,
    pub samples: Range<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PointRef {
    Junction(u32),
    Joint(u32),
    End(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arm {
    pub way: u32,
    pub at: Station,
    /// +1 along the part, −1 back.
    pub dir: i8,
    pub heading: [f64; 2],
    /// `atan2` of the heading: x east, y south.
    pub angle: f64,
    pub host: bool,
    pub level: Level,
    pub room_m: f64,
    pub next: Option<PointRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostRule {
    Side,
    Fallback,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Junction {
    pub pos: [i32; 2],
    pub network: Network,
    /// Sorted by angle, then by the heading point, then by `dir`.
    pub arms: Vec<Arm>,
    pub host_rule: HostRule,
    /// Two ways that cross at one height with no shared vertex.
    pub implicit: bool,
    pub owned: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Joint {
    pub pos: [i32; 2],
    pub network: Network,
    pub arms: [Arm; 2],
    /// π less the angle between the headings: 0 straight on.
    pub turn_rad: f64,
    pub stroke: bool,
    pub owned: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct End {
    pub pos: [i32; 2],
    pub network: Network,
    pub arm: Arm,
    pub owned: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CrossingKind {
    Deck,
    Culvert,
    Ford,
    PipeInStones,
    SteppingStones,
    Footbridge,
    LevelCrossing,
    Stacked,
    /// A tunnel way under another way, from their tags.
    Bore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Tag,
    SharedVertex,
    Rule,
    Fallback,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Crossing {
    pub pos: [i32; 2],
    pub over: Station,
    pub under: Station,
    pub kind: CrossingKind,
    pub source: Source,
    /// From 0 to π/2.
    pub angle_rad: f64,
    pub deck: Option<u32>,
    pub owned: bool,
    /// The room and the step of the ladder, where one applies.
    pub ladder: Option<Ladder>,
}

/// Deck ways that share a point, or one synthetic stretch.
#[derive(Clone, Debug, PartialEq)]
pub struct Deck {
    pub ways: Vec<u32>,
    pub synthetic: bool,
}

/// What each stage removed, kept apart or decided. Counts over the work area.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphStats {
    pub repeated: u32,
    pub spikes: u32,
    pub folded: u32,
    pub no_height: u32,
    /// Water spans under the terrain by more than their `underground_m`.
    pub underground: u32,
    pub shared_runs: u32,
    pub end_on_other: u32,
    pub water_water: u32,
    pub grazing: u32,
    pub grazing_water: u32,
    pub buried: u32,
    pub deck_fallback: u32,
    pub deck_cut: u32,
    pub links_end: u32,
    pub links_carriageway: u32,
    pub link_crossed: u32,
    pub snap_far: u32,
    pub ends: u32,
    pub junctions: u32,
    pub joints: u32,
    pub implicit_junctions: u32,
    pub crossings: BTreeMap<(CrossingKind, Source), u32>,
    pub samples: u32,
}

impl GraphStats {
    /// Every counter but the crossings by type, by name.
    pub fn counters(&self) -> [(&'static str, u32); 23] {
        [
            ("repeated", self.repeated),
            ("spikes", self.spikes),
            ("folded", self.folded),
            ("no_height", self.no_height),
            ("underground", self.underground),
            ("shared_runs", self.shared_runs),
            ("end_on_other", self.end_on_other),
            ("water_water", self.water_water),
            ("grazing", self.grazing),
            ("grazing_water", self.grazing_water),
            ("buried", self.buried),
            ("deck_fallback", self.deck_fallback),
            ("deck_cut", self.deck_cut),
            ("links_end", self.links_end),
            ("links_carriageway", self.links_carriageway),
            ("link_crossed", self.link_crossed),
            ("snap_far", self.snap_far),
            ("ends", self.ends),
            ("junctions", self.junctions),
            ("joints", self.joints),
            ("implicit_junctions", self.implicit_junctions),
            ("crossings", self.crossings.values().sum()),
            ("samples", self.samples),
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WayGraph {
    pub cell: CellKey,
    pub parts: Vec<Part>,
    pub ways: Vec<Way>,
    pub samples: Vec<Sample>,
    pub junctions: Vec<Junction>,
    pub joints: Vec<Joint>,
    pub ends: Vec<End>,
    pub crossings: Vec<Crossing>,
    pub decks: Vec<Deck>,
    /// Empty from `build`: `levels` fills it.
    pub levels: Levels,
    pub stats: GraphStats,
}

/// A sample of a walk along a way: its place (cell units) and its way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkPoint {
    pub pos: [i32; 2],
    pub way: u32,
}

impl WayGraph {
    /// The way whose curve holds a station on the level `level`.
    pub fn way_at(&self, st: &Station, level: Level) -> Option<u32> {
        let key = |s: &Station| (s.span, s.u);
        (self.ways.iter())
            .find(|w| w.part == st.part && w.level == level && key(&w.from) <= key(st) && key(st) <= key(&w.to))
            .map(|w| w.id)
    }

    /// The sample of a way nearest a point: a crossing's point is a sample of both ways.
    pub fn sample_near(&self, way: u32, pos: [i32; 2]) -> u32 {
        let r = &self.ways[way as usize].samples;
        let d = |k: &u32| {
            let p = self.samples[*k as usize].pos;
            i64::from(p[0] - pos[0]).pow(2) + i64::from(p[1] - pos[1]).pow(2)
        };
        (r.start..r.end).min_by_key(d).unwrap_or(r.start)
    }

    /// The walks along a way's network from sample `at` of way `way` in direction `dir` (+1 along its part)
    /// for `max_m` metres, the last chord cut there: on through the joints, and with `branches` into each
    /// other arm of a junction, a walk of its own after the first. Without, a walk stops at a junction.
    /// Every walk starts at `at`.
    pub fn walk(&self, way: u32, at: u32, dir: i8, max_m: f64, branches: bool) -> Vec<Vec<WalkPoint>> {
        let mpu = |a: [i32; 2], b: [i32; 2]| metres_per_unit(self.cell.origin().y + i64::from(a[1] + b[1]) / 2);
        let point = |k: u32| WalkPoint { pos: self.samples[k as usize].pos, way: self.way_of_sample(k) };
        let mut out = Vec::new();
        let network = self.ways[way as usize].family.network();
        let mut todo = vec![(way, at, dir, 0.0, vec![point(at)])];
        while let Some((mut w, mut k, mut d, mut m, mut list)) = todo.pop() {
            loop {
                let r = &self.ways[w as usize].samples;
                let next = i64::from(k) + i64::from(d);
                if (i64::from(r.start)..i64::from(r.end)).contains(&next) {
                    let (a, b) = (self.samples[k as usize].pos, self.samples[next as usize].pos);
                    let len = libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])) * mpu(a, b);
                    if m + len >= max_m {
                        let f = if len > 0.0 { (max_m - m) / len } else { 0.0 };
                        let pos = [
                            a[0] + libm::round(f * f64::from(b[0] - a[0])) as i32,
                            a[1] + libm::round(f * f64::from(b[1] - a[1])) as i32,
                        ];
                        list.push(WalkPoint { pos, ..point(next as u32) });
                        break;
                    }
                    (k, m) = (next as u32, m + len);
                    list.push(point(k));
                    continue;
                }
                // The way's end: the arms there leave it back along its part.
                let end = |a: &Arm| a.way == w && a.dir == -d;
                let arms: Vec<&Arm> = match self.joints.iter().find(|j| j.network == network && j.arms.iter().any(end))
                {
                    Some(j) => j.arms.iter().filter(|a| !end(a)).collect(),
                    None if branches => (self.junctions.iter())
                        .find(|j| j.network == network && j.arms.iter().any(end))
                        .map(|j| j.arms.iter().filter(|a| !end(a)).collect())
                        .unwrap_or_default(),
                    None => Vec::new(),
                };
                let Some((first, rest)) = arms.split_first() else { break };
                let start = |a: &Arm| {
                    let r = &self.ways[a.way as usize].samples;
                    (a.way, if a.dir > 0 { r.start } else { r.end - 1 }, a.dir)
                };
                for a in rest {
                    let (w2, k2, d2) = start(a);
                    todo.push((w2, k2, d2, m, vec![point(k2)]));
                }
                (w, k, d) = start(first);
            }
            out.push(list);
        }
        out
    }

    /// The way of sample k.
    fn way_of_sample(&self, k: u32) -> u32 {
        self.ways.partition_point(|w| w.samples.end <= k) as u32
    }

    /// The deck depth of bridge way w: its tile's `deck` tag where the way lies on the deck of its own
    /// part, else the thinnest deck of its class (`section`).
    pub fn deck_depth(&self, input: &CellInput, w: u32, section: &WaySection) -> f64 {
        let way = &self.ways[w as usize];
        let part = &self.parts[way.part as usize];
        let tagged = way.level == Level::Deck && part.level == Level::Deck;
        let tag = input.features[part.feature.0 as usize].tags.get("deck");
        tag.and_then(|t| leading_number(t)).filter(|_| tagged).unwrap_or(section.deck_least_m)
    }
}

/// A broken invariant of `check`.
#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub check: &'static str,
    pub what: String,
}

/// The cell square grown by `MARGIN_M` at its edge nearer the pole: [x0, y0, x1, y1], closed.
pub fn work_area(cell: CellKey) -> [i64; 4] {
    let o = cell.origin();
    let pole = if o.y < 1 << (FRAME_BITS - 1) { o.y } else { o.y + CELL_UNITS };
    let m = (MARGIN_M / metres_per_unit(pole)).floor() as i64;
    [o.x - m, o.y - m, o.x + CELL_UNITS + m, o.y + CELL_UNITS + m]
}

fn inside([x0, y0, x1, y1]: [i64; 4], p: [i64; 2]) -> bool {
    (x0..=x1).contains(&p[0]) && (y0..=y1).contains(&p[1])
}

/// The graph of a cell from its input and the rule tables, in the order of the stages.
pub fn build(cell: CellKey, input: &CellInput, rules: &Sections) -> WayGraph {
    let area = work_area(cell);
    let mut stats = GraphStats::default();
    let (mut parts, mut part_rules) = parts::parts(input, rules, &mut stats);
    let (mut samples, sampled): (Vec<Vec<Sample>>, Vec<Vec<bool>>) =
        parts.iter().zip(&part_rules).map(|(part, rule)| curve::samples(part, rule, cell, area)).unzip();
    let plan = points::pieces(&parts, &sampled, input, &mut stats);
    let plan = crossings::crossings(cell, area, &parts, &part_rules, &mut samples, plan, &mut stats);
    let plan = snap::snap(cell, area, &mut parts, &mut part_rules, &mut samples, plan, &mut stats);
    let mut g = points::assemble(cell, area, parts, &part_rules, samples, plan, stats);
    open_culverts(&mut g, input, rules);
    g
}

/// The water of a short mapped culvert chain (tunnel water ways that meet end to end, up to its row's
/// `culvert_max_m` in all, as the classifier's, the bake's and develop's rule) under a way on the ground lies on the
/// ground, as the bake lays it: on one straight line over the chain between the levels of the open water at its two
/// ends, or at the one end's level. The faces hide it under a road or a railway only (`faces::culverts`), so the
/// stream runs up to the way's bank, and a path lies across it. A longer chain (a buried stream, `Source::Fallback`)
/// and one with no open water at an end stay in their tunnel.
fn open_culverts(g: &mut WayGraph, input: &CellInput, rules: &Sections) {
    let on_ground = |w: u32| g.ways[w as usize].family != Family::Water;
    let mut seeds: Vec<u32> = (g.crossings.iter())
        .filter(|c| c.kind == CrossingKind::Culvert && c.source != Source::Fallback)
        .filter(|c| g.way_at(&c.over, Level::Ground).is_some_and(on_ground))
        .filter_map(|c| g.way_at(&c.under, Level::Tunnel))
        .filter(|&w| g.ways[w as usize].family == Family::Water)
        .collect();
    seeds.sort_unstable();
    seeds.dedup();
    let mut done = BTreeSet::new();
    for w in seeds {
        if done.contains(&w) {
            continue;
        }
        let (chain, ends) = culvert_chain(g, w);
        done.extend(chain.iter().map(|&(x, _)| x));
        let way = &g.ways[w as usize];
        let f = &input.features[g.parts[way.part as usize].feature.0 as usize];
        // Each way's samples in the chain's order, with the metres along the chain at each and the way's direction.
        let mut at: Vec<(usize, f64, i8)> = Vec::new();
        let mut len = 0.0;
        for &(x, d) in &chain {
            let r = g.ways[x as usize].samples.clone();
            let mut k: Vec<usize> = (r.start as usize..r.end as usize).collect();
            if d < 0 {
                k.reverse();
            }
            let first = g.samples[k[0]].along_m;
            at.extend(k.iter().map(|&i| (i, len + (g.samples[i].along_m - first).abs(), d)));
            len = at.last().map_or(len, |a| a.1);
        }
        let (z0, z1) = match ends {
            [Some(x), Some(y)] => (x, y),
            [Some(z), None] | [None, Some(z)] => (z, z),
            [None, None] => continue,
        };
        if len > rules.water(&f.kind, &f.tags).culvert_max_m {
            continue;
        }
        let len = if len > 1e-6 { len } else { 1.0 };
        for (i, s, d) in at {
            g.samples[i].z_m = z0 + (s / len).clamp(0.0, 1.0) * (z1 - z0);
            g.samples[i].grade = f64::from(d) * (z1 - z0) / len;
        }
        for &(x, _) in &chain {
            g.ways[x as usize].level = Level::Ground;
        }
        let arms = (g.joints.iter_mut().flat_map(|j| j.arms.iter_mut()))
            .chain(g.junctions.iter_mut().flat_map(|j| j.arms.iter_mut()))
            .chain(g.ends.iter_mut().map(|e| &mut e.arm));
        for arm in arms.filter(|x| chain.iter().any(|c| c.0 == x.way)) {
            arm.level = Level::Ground;
        }
    }
}

/// The culvert chain through tunnel water way w: its tunnel water ways that meet end to end at joints, in order,
/// each with its direction (+1 along its part), and the level of the open water on the ground at each end, none
/// where the chain ends with none.
fn culvert_chain(g: &WayGraph, w: u32) -> (Vec<(u32, i8)>, [Option<f64>; 2]) {
    let tunnel = |x: u32| g.ways[x as usize].family == Family::Water && g.ways[x as usize].level == Level::Tunnel;
    // At the far end of way x walked in direction d: the next tunnel way and its direction, or the open level.
    let next = |x: u32, d: i8| -> Result<(u32, i8), Option<f64>> {
        let end = |a: &Arm| a.way == x && a.dir == -d;
        let joint = g.joints.iter().find(|j| j.network == Network::Water && j.arms.iter().any(end));
        let Some(j) = joint else { return Err(None) };
        let other = j.arms.iter().find(|a| !end(a)).ok_or(None)?;
        if tunnel(other.way) {
            return Ok((other.way, other.dir));
        }
        let open = g.ways[other.way as usize].level == Level::Ground;
        Err(open.then(|| g.samples[g.sample_near(other.way, j.pos) as usize].z_m))
    };
    let (mut ahead, mut behind, mut ends) = (vec![(w, 1i8)], Vec::new(), [None, None]);
    for (k, start) in [(1, 1i8), (0, -1)] {
        let (mut x, mut d) = (w, start);
        for _ in 0..64 {
            match next(x, d) {
                Ok((y, e)) if !ahead.iter().chain(&behind).any(|a: &(u32, i8)| a.0 == y) => {
                    (x, d) = (y, e);
                    if k == 1 {
                        ahead.push((y, e));
                    } else {
                        behind.push((y, -e));
                    }
                }
                Ok(_) => break,
                Err(z) => {
                    ends[k] = z;
                    break;
                }
            }
        }
    }
    behind.reverse();
    behind.extend(ahead);
    (behind, ends)
}

/// The rows of the graph in the reach table: (R_d, own R_e) of each decision.
pub fn reach_rows(culvert_max_m: f64) -> Vec<Reach> {
    let chain = |name: &str, terms: &[(&str, f64)]| Reach {
        name: name.into(),
        owner: "graph",
        chain: true,
        terms: terms.iter().map(|&(what, m)| (what.to_string(), m)).collect(),
    };
    let snap_foot = SNAP_REACH_M + SNAP_REACH_M;
    vec![
        chain("graph heading", &[("HEADING_M", HEADING_M)]),
        chain("graph room", &[("ROOM_MAX_M", ROOM_MAX_M), ("a snap foot there", snap_foot)]),
        chain("graph snap", &[("SNAP_REACH_M", SNAP_REACH_M), ("its link", SNAP_REACH_M)]),
        chain(
            "graph crossing",
            &[("HALF_MAX_M", crate::sections::HALF_MAX_M), ("PLACE_M", PLACE_M), ("HEADING_M", HEADING_M)],
        ),
        chain("graph synthetic deck", &[("DECK_HALF_MAX_M", DECK_HALF_MAX_M), ("its stretch", DECK_HALF_MAX_M)]),
        chain("graph culvert chain", &[("culvert_max_m", culvert_max_m)]),
        chain("graph shared run", &[("one shared span", 0.0), ("HEADING_M at its ends", HEADING_M)]),
        chain("graph spike", &[("a vertex in the grown cell and its two spans", 0.0)]),
    ]
}

/// The invariants G1 to G4 of a graph.
pub fn check(graph: &WayGraph) -> Vec<Fault> {
    let mut faults = Vec::new();
    let mut fault = |check: &'static str, what: String| faults.push(Fault { check, what });
    for way in &graph.ways {
        let samples = &graph.samples[way.samples.start as usize..way.samples.end as usize];
        for pair in samples.windows(2) {
            if (pair[0].span, pair[0].u) >= (pair[1].span, pair[1].u) {
                fault("G1", format!("way {}: samples out of order at span {} u {}", way.id, pair[1].span, pair[1].u));
            }
            if pair[0].pos == pair[1].pos {
                fault("G1", format!("way {}: two samples at {:?}", way.id, pair[0].pos));
            }
        }
        if samples.len() < 2 {
            fault("G1", format!("way {}: {} samples", way.id, samples.len()));
        }
        for s in samples {
            let values = [s.u, s.s_m, s.along_m, s.z_m, s.tangent[0], s.tangent[1], s.grade, s.bank, s.widen_m];
            if values.iter().any(|v| !v.is_finite()) {
                fault("G1", format!("way {}: a value that is not finite at {:?}", way.id, s.pos));
            }
        }
    }
    for j in &graph.junctions {
        let hosts = j.arms.iter().filter(|a| a.host).count();
        let group = j.arms.iter().all(|a| graph.ways[a.way as usize].family.network() == j.network);
        if j.arms.len() < 3 || !group || !(1..=2).contains(&hosts) {
            fault("G2", format!("junction at {:?}: {} arms, {hosts} hosts, one group {group}", j.pos, j.arms.len()));
        }
        if j.arms.windows(2).any(|a| a[0].angle > a[1].angle) {
            fault("G2", format!("junction at {:?}: arms not sorted", j.pos));
        }
    }
    let area = work_area(graph.cell);
    let o = graph.cell.origin();
    let global = |p: [i32; 2]| [o.x + i64::from(p[0]), o.y + i64::from(p[1])];
    // The arms at the ends of ways: (way, direction, station).
    let mut arms: BTreeSet<(u32, i8, u32, u64)> = BTreeSet::new();
    let mut at = |arm: &Arm| {
        arms.insert((arm.way, arm.dir, arm.at.span, arm.at.u.to_bits()));
    };
    graph.junctions.iter().flat_map(|j| &j.arms).for_each(&mut at);
    graph.joints.iter().flat_map(|j| &j.arms).for_each(&mut at);
    graph.ends.iter().map(|e| &e.arm).for_each(&mut at);
    for way in &graph.ways {
        let first = &graph.samples[way.samples.start as usize];
        let last = &graph.samples[way.samples.end as usize - 1];
        for (sample, dir, st) in [(first, 1, way.from), (last, -1, way.to)] {
            if inside(area, global(sample.pos)) && !arms.contains(&(way.id, dir, st.span, st.u.to_bits())) {
                fault("G3", format!("way {}: its end at {:?} is no arm of a point", way.id, sample.pos));
            }
        }
        if graph.decks.iter().any(|d| d.synthetic && d.ways.contains(&way.id)) {
            let inner =
                |s: &Station| (s.span, s.u) > (way.from.span, way.from.u) && (s.span, s.u) < (way.to.span, way.to.u);
            let holds = (graph.junctions.iter().flat_map(|j| &j.arms))
                .chain(graph.joints.iter().flat_map(|j| &j.arms))
                .chain(graph.ends.iter().map(|e| &e.arm))
                .any(|a| a.way == way.id && inner(&a.at));
            if holds {
                fault("G4", format!("synthetic deck way {} holds a point", way.id));
            }
        }
    }
    faults
}
