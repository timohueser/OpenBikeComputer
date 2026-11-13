//! The decks of the storeys over the ground: the ways that storey k ≥ 1 holds, the stretch of each between its
//! end lines (the DECK runs of `graph::levels`), and the stroke of a deck: its paved layers and the band about
//! them (`edge_band_m`, `Surface::DeckEdge`).

use std::collections::{BTreeMap, BTreeSet};

use super::geom::P;
use super::stroke::unit;
use super::{WayLine, LAYERS, SLOPE};
use crate::graph::{CrossingKind, EndKind, Family, Level, State, Station, Step, WayGraph};

/// How a piece of a deck way ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum End {
    /// At the way's own end: a joint or junction with the next deck way, or the end of a synthetic deck. `open`: a
    /// ground way goes on there.
    Way { open: bool },
    /// On an end line through `at` (frame units from the cell's origin). `open`: an A line, on which storey 0 meets the
    /// deck; a closed line is the deck's free end.
    Line { at: [f64; 2], open: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Piece {
    pub from: Station,
    pub to: Station,
    pub ends: [End; 2],
}

/// The decks of a graph after `graph::levels`.
pub(crate) struct Decks {
    /// Per way: its storey, 0 for a way on no deck.
    pub storey: Vec<i8>,
    pub pieces: BTreeMap<u32, Vec<Piece>>,
}

fn key(s: &Station) -> (u32, f64) {
    (s.span, s.u)
}

impl Decks {
    /// A DECK run of a bridge way is a piece; a synthetic deck (no runs) is one piece end to end. A storey is the
    /// storey of a deck group (`WayGraph::decks`, the deck ways that share a point): 1, and one more than each
    /// group under it at a crossing whose ladder gives a deck (full or thin).
    pub(crate) fn of(g: &WayGraph) -> Decks {
        let deck_way = |w: u32| {
            let x = &g.ways[w as usize];
            x.level == Level::Deck && x.family != Family::Water
        };
        // Whether a ground way and no other deck way meets the end of way w at station `st`, which w leaves along
        // `dir`: there storey 0 goes on from the deck.
        let ground_at = |w: u32, dir: i8, st: &Station| {
            let sets = g.junctions.iter().map(|j| &j.arms[..]).chain(g.joints.iter().map(|j| &j.arms[..]));
            sets.into_iter().any(|arms| {
                arms.iter().any(|a| a.way == w && a.dir == dir && key(&a.at) == key(st))
                    && arms.iter().any(|a| a.level == Level::Ground && g.ways[a.way as usize].family != Family::Water)
                    && !arms.iter().any(|a| a.way != w && deck_way(a.way))
            })
        };
        let mut pieces: BTreeMap<u32, Vec<Piece>> = BTreeMap::new();
        for r in g.levels.runs.iter().filter(|r| r.state == State::Deck && deck_way(r.way)) {
            let end = |st: &Station, dir: i8| {
                let line = (g.levels.end_lines.iter())
                    .find(|l| l.way == r.way && key(&l.at) == key(st) && l.kind != EndKind::Portal);
                // Past an A line the way goes on as a ground way (its stub); past a closed line it is not drawn.
                line.map_or(End::Way { open: ground_at(r.way, dir, st) }, |l| {
                    let at = [0, 1].map(|k| (f64::from(l.ends[0][k]) + f64::from(l.ends[1][k])) / 2.0);
                    End::Line { at, open: l.kind == EndKind::A }
                })
            };
            let ends = [end(&r.from, 1), end(&r.to, -1)];
            pieces.entry(r.way).or_default().push(Piece { from: r.from, to: r.to, ends });
        }
        let tagged: BTreeSet<u32> = g.levels.runs.iter().map(|r| r.way).collect();
        for w in (0..g.ways.len() as u32).filter(|&w| deck_way(w) && !tagged.contains(&w)) {
            let way = &g.ways[w as usize];
            let ends = [End::Way { open: ground_at(w, 1, &way.from) }, End::Way { open: ground_at(w, -1, &way.to) }];
            pieces.insert(w, vec![Piece { from: way.from, to: way.to, ends }]);
        }
        let inside = |w: u32, st: &Station| {
            pieces.get(&w).is_some_and(|ps| ps.iter().any(|p| key(&p.from) <= key(st) && key(st) <= key(&p.to)))
        };
        let over: Vec<(usize, usize)> = (g.crossings.iter())
            .filter(|c| c.kind == CrossingKind::Deck)
            .filter_map(|c| {
                let l = c.ladder.as_ref().filter(|l| matches!(l.step, Step::Full | Step::Thin))?;
                let (up, low) = (g.ways[l.upper as usize].deck?, g.ways[l.lower as usize].deck?);
                (up != low && inside(l.upper, &c.over) && inside(l.lower, &c.under))
                    .then_some((low as usize, up as usize))
            })
            .collect();
        let mut group = vec![1i8; g.decks.len()];
        // A chain of decks over decks is short; a loop (a spiral ramp) stops growing here.
        for _ in 0..8 {
            let mut changed = false;
            for &(low, up) in &over {
                if group[up] <= group[low] {
                    group[up] = group[low] + 1;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let storey = (0..g.ways.len())
            .map(|w| match (pieces.contains_key(&(w as u32)), g.ways[w].deck) {
                (true, Some(d)) => group[d as usize],
                (true, None) => 1,
                (false, _) => 0,
            })
            .collect();
        Decks { storey, pieces }
    }
}

/// A deck's stroke on a line of storey k ≥ 1: its carriageway, its paved shoulder and the band (`band`, units at
/// a vertex) in the slope's place; the shoulder, the verge and the bank are 0 wide. A deck is one leg and ends
/// square: a free end lies on an end line or at a joint.
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

/// Where a stretch ends on an end line: the point on the line's chord, the unit direction of that chord into the
/// stretch, whether storey 0 meets it there, and the half-width of the stroke there (units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EndAt {
    pub at: P,
    pub into: [f64; 2],
    pub open: bool,
    pub half: f64,
}

/// The stroke of one piece of a line: its points with the offsets of each layer (carriageway, then `LAYERS`), the
/// direction of each segment, the line's vertices strictly inside it (from, to), and its end lines.
pub(crate) struct Stretch {
    pub p: Vec<P>,
    pub o: Vec<[f64; LAYERS + 1]>,
    pub dirs: Vec<Option<[f64; 2]>>,
    pub inner: (usize, usize),
    pub lines: [Option<EndAt>; 2],
}

/// The segment of the line nearest q and the share of q's foot on it.
fn foot(line: &WayLine, q: [f64; 2]) -> (usize, f64) {
    let mut best = (f64::INFINITY, 0, 0.0);
    for j in 0..line.p.len() - 1 {
        let (a, b) = (line.p[j], line.p[j + 1]);
        let d = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])];
        let v = [q[0] - f64::from(a[0]), q[1] - f64::from(a[1])];
        let l2 = d[0] * d[0] + d[1] * d[1];
        let t = if l2 > 0.0 { ((v[0] * d[0] + v[1] * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let e = libm::hypot(v[0] - t * d[0], v[1] - t * d[1]);
        if e < best.0 {
            best = (e, j, t);
        }
    }
    (best.1, best.2)
}

/// The stretch of each piece of a line: from its end line (or the line's end) to its end line, every vertex of the
/// line between. An end on a line keeps the direction of the segment that holds it, so the stroke ends square on
/// that segment's chord; an end that falls on a vertex is that vertex, square to the segment beyond it.
pub(crate) fn stretches(line: &WayLine, pieces: &[Piece]) -> Vec<Stretch> {
    let n = line.p.len();
    let chord = |j: usize| unit(line.p[j], line.p[j + 1]);
    // The foot of an end line on the segment that holds it: (segment, share, point), the point a whole unit from
    // the segment's start, so two cells round it alike.
    let at = |j: usize, t: f64| {
        let (a, b) = (line.p[j], line.p[j + 1]);
        [0, 1].map(|k| a[k] + libm::round(t * f64::from(b[k] - a[k])) as i32)
    };
    let offsets_at = |j: usize, t: f64| -> [f64; LAYERS + 1] {
        let (a, b) = (line.offsets(j), line.offsets(j + 1));
        std::array::from_fn(|k| a[k] + t * (b[k] - a[k]))
    };
    let mut out = Vec::new();
    for piece in pieces {
        let start = match piece.ends[0] {
            End::Line { at: q, open } => {
                let (j, t) = foot(line, q);
                Some(if t >= 1.0 && j + 2 < n { (j + 1, 0.0, open) } else { (j, t, open) })
            }
            End::Way { .. } => None,
        };
        let end = match piece.ends[1] {
            End::Line { at: q, open } => {
                let (j, t) = foot(line, q);
                Some(if t <= 0.0 && j >= 1 { (j - 1, 1.0, open) } else { (j, t, open) })
            }
            End::Way { .. } => None,
        };
        // The line's vertices strictly inside the piece.
        let lo = start.map_or(0, |(j, t, _)| if t <= 0.0 { j } else { j + 1 });
        let hi = end.map_or(n - 1, |(j, t, _)| if t >= 1.0 { j + 1 } else { j });
        let mut s = Stretch { p: Vec::new(), o: Vec::new(), dirs: Vec::new(), inner: (lo, hi), lines: [None, None] };
        if let Some((j, t, open)) = start.filter(|&(_, t, _)| t > 0.0) {
            let o = offsets_at(j, t);
            s.lines[0] = Some(EndAt { at: at(j, t), into: chord(j).unwrap_or([1.0, 0.0]), open, half: o[SLOPE + 1] });
            s.p.push(at(j, t));
            s.o.push(o);
        }
        for i in lo..=hi {
            s.p.push(line.p[i]);
            s.o.push(line.offsets(i));
        }
        if let Some((j, t, open)) = end.filter(|&(_, t, _)| t < 1.0) {
            let o = offsets_at(j, t);
            let along = chord(j).unwrap_or([1.0, 0.0]);
            s.lines[1] = Some(EndAt { at: at(j, t), into: [-along[0], -along[1]], open, half: o[SLOPE + 1] });
            s.p.push(at(j, t));
            s.o.push(o);
        }
        // An end on a vertex: the end line is square to the segment beyond it.
        if let (None, Some((_, _, open))) = (s.lines[0], start) {
            let o = line.offsets(lo);
            let into = chord(lo.min(n - 2)).unwrap_or([1.0, 0.0]);
            s.lines[0] = Some(EndAt { at: line.p[lo], into, open, half: o[SLOPE + 1] });
        }
        if let (None, Some((_, _, open))) = (s.lines[1], end) {
            let o = line.offsets(hi);
            let along = chord(hi.max(1) - 1).unwrap_or([1.0, 0.0]);
            s.lines[1] = Some(EndAt { at: line.p[hi], into: [-along[0], -along[1]], open, half: o[SLOPE + 1] });
        }
        // A way's own end where storey 0 goes on: its end line is square to its end segment.
        if let (None, End::Way { open: true }) = (s.lines[0], piece.ends[0]) {
            let into = chord(0).unwrap_or([1.0, 0.0]);
            s.lines[0] = Some(EndAt { at: line.p[0], into, open: true, half: line.offsets(0)[SLOPE + 1] });
        }
        if let (None, End::Way { open: true }) = (s.lines[1], piece.ends[1]) {
            let along = chord(n - 2).unwrap_or([1.0, 0.0]);
            let half = line.offsets(n - 1)[SLOPE + 1];
            s.lines[1] = Some(EndAt { at: line.p[n - 1], into: [-along[0], -along[1]], open: true, half });
        }
        let m = s.p.len();
        s.dirs = vec![None; m.saturating_sub(1)];
        if let (Some((j, t, _)), true) = (start, m >= 2) {
            if t > 0.0 {
                s.dirs[0] = chord(j);
            }
        }
        if let (Some((j, t, _)), true) = (end, m >= 2) {
            if t < 1.0 {
                s.dirs[m - 2] = chord(j);
            }
        }
        // A foot that rounds onto the next vertex: one point, the segment beyond keeps its own direction.
        let mut k = 1;
        while k < s.p.len() {
            if s.p[k] == s.p[k - 1] {
                let drop = if k == 1 { 1 } else { k - 1 };
                s.p.remove(drop);
                s.o.remove(drop);
                s.dirs.remove(drop - 1);
            } else {
                k += 1;
            }
        }
        if s.p.len() >= 2 {
            out.push(s);
        }
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
    pub(crate) fn of(g: &WayGraph, input: &crate::CellInput, rules: &super::Rules, w: u32) -> DeckWay {
        let way = &g.ways[w as usize];
        let feature = &input.features[g.parts[way.part as usize].feature.0 as usize];
        let sec = rules.sections.way(&feature.kind, &feature.tags, false);
        let thin = (g.crossings.iter())
            .filter_map(|c| c.ladder.as_ref())
            .filter(|l| l.upper == w && l.step == Step::Thin)
            .map(|l| (l.room_m - l.least_m + sec.deck_least_m).max(sec.deck_least_m));
        let depth_m = thin.fold(g.deck_depth(input, w, &sec), f64::min);
        let paved = sec.half_m + if sec.shoulder_paved { sec.shoulder_m } else { 0.0 };
        let width = 2.0 * (paved + rules.structures.edge_band_m);
        DeckWay {
            depth_m,
            railing: sec.railing || width < rules.structures.railing_width_m,
            pier_m: [PIER_ALONG_M[usize::from(sec.path)], PIER_SHARE * 2.0 * paved],
        }
    }
}
