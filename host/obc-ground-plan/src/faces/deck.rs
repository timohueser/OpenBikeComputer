//! The decks of the storeys over the ground: the ways that storey k ≥ 1 holds (the graph splits each bridge way at
//! the end lines that the tiles carry, `graph::level_tags::split`, so a deck is a whole way), their open ends, and the
//! stroke of a deck: its paved layers and the band about them (`edge_band_m`, `Surface::DeckEdge`).

use super::geom::P;
use super::{WayLine, SLOPE};
use crate::graph::{level_tags, Family, Level, WayGraph};
use crate::CellInput;

/// The number of a tag of the feature of way w.
fn tag(g: &WayGraph, input: &CellInput, w: u32, key: &str) -> Option<f64> {
    let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
    f.tags.get(key).and_then(|t| t.parse().ok())
}

/// The decks of a graph: per way its storey (0 for a way on no deck), whether each of its ends is open, and its run.
pub(crate) struct Decks {
    pub storey: Vec<i8>,
    /// An open end meets a way that goes on past it on another storey: a way on the ground (storey 0 goes on), or a
    /// deck on another storey. The shell leaves it open; a free end is closed.
    pub open: Vec<[bool; 2]>,
    /// The deck ways that a junction or a joint joins on one storey are one run, named by its least way: where two
    /// runs meet side by side on a sheet, each has its own edge (`mesh::shell`).
    pub run: Vec<u32>,
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
        let goes_on = |w: u32, o: u32| {
            let x = &g.ways[o as usize];
            o != w
                && x.family != Family::Water
                && (x.level == Level::Ground || deck_way(o) && storey[o as usize] != storey[w as usize])
        };
        let mut open = vec![[false; 2]; g.ways.len()];
        let mut run: Vec<u32> = (0..g.ways.len() as u32).collect();
        fn root(run: &mut [u32], mut w: u32) -> u32 {
            while run[w as usize] != w {
                run[w as usize] = run[run[w as usize] as usize];
                w = run[w as usize];
            }
            w
        }
        for arms in g.junctions.iter().map(|j| &j.arms[..]).chain(g.joints.iter().map(|j| &j.arms[..])) {
            for a in arms.iter().filter(|a| deck_way(a.way)) {
                if arms.iter().any(|b| goes_on(a.way, b.way)) {
                    open[a.way as usize][usize::from(a.dir < 0)] = true;
                }
                for b in arms.iter().filter(|b| deck_way(b.way) && storey[b.way as usize] == storey[a.way as usize]) {
                    let (x, y) = (root(&mut run, a.way), root(&mut run, b.way));
                    run[x.max(y) as usize] = x.min(y);
                }
            }
        }
        let run = (0..g.ways.len() as u32).map(|w| root(&mut run, w)).collect();
        Decks { storey, open, run }
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
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckWay {
    /// Its storey over the ground, and its run (`Decks::run`).
    pub storey: i8,
    pub run: u32,
    /// The slab under the deck's top: the tile's `deck` depth, on ladder step 2 down to the class's least.
    pub depth_m: f64,
    /// A railing on the free outline, else a parapet.
    pub railing: bool,
    /// A pier's length along the deck and its width across.
    pub pier_m: [f64; 2],
}

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
    pub(crate) fn of(g: &WayGraph, input: &CellInput, rules: &super::Rules, w: u32, decks: &Decks) -> DeckWay {
        let way = &g.ways[w as usize];
        let feature = &input.features[g.parts[way.part as usize].feature.0 as usize];
        let sec = rules.sections.way(&feature.kind, &feature.tags, false);
        let depth = g.deck_depth(input, w, &sec);
        let depth_m = tag(g, input, w, level_tags::SLAB).map_or(depth, |s| s.min(depth));
        let paved = sec.half_m + if sec.shoulder_paved { sec.shoulder_m } else { 0.0 };
        let width = 2.0 * (paved + rules.structures.edge_band_m);
        DeckWay {
            storey: decks.storey[w as usize],
            run: decks.run[w as usize],
            depth_m,
            railing: sec.railing || width < rules.structures.railing_width_m,
            pier_m: [PIER_ALONG_M[usize::from(sec.path)], PIER_SHARE * 2.0 * paved],
        }
    }
}
