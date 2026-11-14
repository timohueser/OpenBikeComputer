//! The decks of the storeys over the ground: the ways that storey k ≥ 1 holds (the graph splits each bridge way at
//! the end lines that the tiles carry, `graph::level_tags::split`, so a deck is a whole way), their open ends, and the
//! stroke of a deck: its paved layers and the band about them (`edge_band_m`, `Surface::DeckEdge`).

use super::geom::P;
use super::{WayLine, SLOPE};
use crate::graph::{level_tags, Family, Level, WayGraph};
use crate::sections::Finish;
use crate::CellInput;

/// The number of a tag of the feature of way w.
fn tag(g: &WayGraph, input: &CellInput, w: u32, key: &str) -> Option<f64> {
    let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
    f.tags.get(key).and_then(|t| t.parse().ok())
}

/// The decks of a graph: per way its storey (0 for a way on no deck) and whether each of its ends is open.
pub(crate) struct Decks {
    pub storey: Vec<i8>,
    /// An open end meets a way that goes on past it on another storey: a way on the ground (storey 0 goes on), or a
    /// deck on another storey. The shell leaves it open; a free end is closed.
    pub open: Vec<[bool; 2]>,
}

impl Decks {
    /// A deck way's storey is its feature's (`level_tags::STOREY`, one more than each deck under it), 1 by default.
    pub(crate) fn of(g: &WayGraph, input: &CellInput) -> Decks {
        let deck_way = |w: u32| {
            let x = &g.ways[w as usize];
            x.level == Level::Deck && x.family != Family::Water
        };
        let storey: Vec<i8> = (0..g.ways.len() as u32)
            .map(|w| if deck_way(w) { tag(g, input, w, level_tags::STOREY).map_or(1, |s| s as i8) } else { 0 })
            .collect();
        let mut decks = Decks { storey, open: vec![[false; 2]; g.ways.len()] };
        let mut open = vec![[false; 2]; g.ways.len()];
        for arms in g.junctions.iter().map(|j| &j.arms[..]).chain(g.joints.iter().map(|j| &j.arms[..])) {
            for a in arms.iter().filter(|a| deck_way(a.way)) {
                if arms.iter().any(|b| decks.goes_on(g, b.way, decks.storey[a.way as usize])) {
                    open[a.way as usize][usize::from(a.dir < 0)] = true;
                }
            }
        }
        decks.open = open;
        decks
    }

    /// Whether way w goes on at another level than storey k: a way on the ground (but water), or a deck on another
    /// storey.
    pub(crate) fn goes_on(&self, g: &WayGraph, w: u32, k: i8) -> bool {
        let x = &g.ways[w as usize];
        x.family != Family::Water
            && (x.level == Level::Ground || x.level == Level::Deck && self.storey[w as usize] != k)
    }
}

/// A deck's stroke on a line of storey k ≥ 1: its carriageway, its paved shoulder and the band (`band`, units at
/// a vertex) in the slope's place; the shoulder, the verge and the bank are 0 wide. A deck is one leg and ends
/// square.
pub(crate) fn deck_bands(line: &mut WayLine, band: &dyn Fn(P) -> f64) {
    for i in 0..line.p.len() {
        let paved = line.bands[i][0];
        line.bands[i] = [paved, paved, paved, paved, paved + band(line.p[i])];
    }
    line.cuts.clear();
    line.spans.clear();
    line.round = [false; 2];
    line.deck = [false; 2];
}

/// The open end lines of a deck's line: the end point, the unit direction into the line and the half-width of its
/// stroke, its band included (units).
pub(crate) fn open_ends(line: &WayLine, open: [bool; 2]) -> Vec<[f64; 5]> {
    let n = line.p.len();
    let end = |at: usize, next: usize| {
        let (p, q) = (line.p[at], line.p[next]);
        let d = [f64::from(q[0] - p[0]), f64::from(q[1] - p[1])];
        let len = libm::hypot(d[0], d[1]).max(1e-9);
        [f64::from(p[0]), f64::from(p[1]), d[0] / len, d[1] / len, line.bands[at][SLOPE]]
    };
    let mut out = Vec::new();
    if n >= 2 && open[0] {
        out.push(end(0, 1));
    }
    if n >= 2 && open[1] {
        out.push(end(n - 1, n - 2));
    }
    out
}

/// A cell's storeys over the ground: their face plans (storey 1 first) and what their shells read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Storeys {
    pub plans: Vec<super::FacePlan>,
    /// Per deck way of the plans, by way: its slab, its edge feature and its piers.
    pub ways: Vec<(u32, DeckWay)>,
    /// The baked piers (`bridge-pier`) within `PIER_NEAR_M` of the cell, frame units from its origin.
    pub piers: Vec<[i32; 2]>,
    /// Storey 0's mesh points on its end lines (`EdgeKind::DeckEdge`, `mesh::points::edge_points`): a deck's end on
    /// such a line takes the same points.
    pub ground_ends: Vec<[i32; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckWay {
    /// Its storey over the ground.
    pub storey: i8,
    /// The slab under the deck's top: the tile's `deck` depth, on ladder step 2 down to the class's least.
    pub depth_m: f64,
    /// A railing on the free outline, else a parapet.
    pub railing: bool,
    /// A pier's length along the deck and its width across.
    pub pier_m: [f64; 2],
    /// What its slab and its edge feature are made of.
    pub build: Build,
    /// A top of planks.
    pub planks: bool,
}

/// What a deck's slab is made of: a road's on beams and a railway's are concrete, a hanging bridge's steel, any
/// other timber. Its railing is timber on timber, else steel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    Concrete = 0,
    Timber = 1,
    Steel = 2,
}

/// The `bridge:structure` of a bridge that hangs from cables, as `CABLES` of the viewer's `terrain/bins.ts`.
const CABLES: [&str; 3] = ["suspension", "simple-suspension", "cable-stayed"];

/// A pier's length along a road deck and a path deck, and its width as a share of the deck's paved width: the
/// crossing block's thickness table.
const PIER_ALONG_M: [f64; 2] = [1.6, 0.5];
const PIER_SHARE: f64 = 0.6;
/// A pier whose point lies this near the cell may reach into it.
pub const PIER_NEAR_M: f64 = 5.0;

impl Storeys {
    /// The deck way `w` of the plans.
    pub fn way(&self, w: u32) -> Option<&DeckWay> {
        self.ways.binary_search_by_key(&w, |x| x.0).ok().map(|k| &self.ways[k].1)
    }
}

impl DeckWay {
    /// Its build and its planks in a storey line's finish word (`records::BUILD_SHIFT`, `records::PLANKS`).
    pub(crate) fn finish_bits(&self) -> u32 {
        (self.build as u32) << super::records::BUILD_SHIFT | if self.planks { super::records::PLANKS } else { 0 }
    }

    pub(crate) fn of(g: &WayGraph, input: &CellInput, rules: &super::Rules, w: u32, decks: &Decks) -> DeckWay {
        let way = &g.ways[w as usize];
        let feature = &input.features[g.parts[way.part as usize].feature.0 as usize];
        let sec = rules.sections.way(&feature.kind, &feature.tags, false);
        let depth = g.deck_depth(input, w, &sec);
        let depth_m = tag(g, input, w, level_tags::SLAB).map_or(depth, |s| s.min(depth));
        let paved = sec.half_m + if sec.shoulder_paved { sec.shoulder_m } else { 0.0 };
        let width = 2.0 * (paved + rules.structures.edge_band_m);
        // The rule of `segment` in the viewer's structures/bridges.ts: a road on beams is a concrete bridge with
        // parapets; a covered bridge, one that hangs and a path's bridge have a railing, a railway's too. A top
        // that the viewer did not count as paved (`isPaved` of roads/decks.ts) is of planks on such a bridge.
        let text = |key: &str| feature.tags.get(key).map_or("", String::as_str);
        let (covered, cable) = (text("bridge") == "covered", CABLES.contains(&text("bridge:structure")));
        let rail = sec.group == "rail";
        let beam_road = !covered && !cable && !sec.path;
        let build = if cable {
            Build::Steel
        } else if rail || beam_road {
            Build::Concrete
        } else {
            Build::Timber
        };
        let paved_top = matches!(sec.finish, Some(Finish::Main | Finish::Road | Finish::Oneway));
        DeckWay {
            storey: decks.storey[w as usize],
            depth_m,
            railing: sec.railing || !beam_road || width < rules.structures.railing_width_m,
            pier_m: [PIER_ALONG_M[usize::from(sec.path)], PIER_SHARE * 2.0 * paved],
            build,
            planks: !beam_road && !rail && !paved_top,
        }
    }
}
