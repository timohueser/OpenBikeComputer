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
use crate::sections::{Reach, Sections};
use crate::CellInput;

pub use crossings::end_on_chord;
pub(crate) use curve::{folds, water_tangents};
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
    /// The heights run linear along the arc between the vertices: the levels of water.
    pub linear_z: bool,
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
    pub fn counters(&self) -> [(&'static str, u32); 22] {
        [
            ("repeated", self.repeated),
            ("spikes", self.spikes),
            ("folded", self.folded),
            ("no_height", self.no_height),
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
    points::assemble(cell, area, parts, &part_rules, samples, plan, stats)
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
