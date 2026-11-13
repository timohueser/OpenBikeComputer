//! The levels of the way graph as the feature tiles carry them: the bake runs `levels` on each cell and writes the
//! tags of each way from what the cells decide (`found`, `merge`); a cell reads its runs and end lines back (`read`).
//!
//! A place on a way is its metres along its stroke (`Sample::along_m`): the same in every cell, as the spans are.

use std::collections::BTreeMap;

use super::curve::{point, s_at, u_at};
use super::parts::{classify, Rule};
use super::points::{exact, key, norm};
use super::{CrossingKind, EndKind, EndLine, Family, Level, Run, State, Station, Step, WayGraph};
use crate::frame::{metres_per_unit_at, GlobalPoint};
use crate::sections::Sections;
use crate::tile::Tags;
use crate::CellInput;

/// The states along a way: `S (m K S)*`, a state, then per end line its metres along the stroke, its kind and the
/// state past it, in rising metres.
pub const LEVELS: &str = "levels";
/// The storey of a deck way over another deck, 2 or more.
pub const STOREY: &str = "storey";
/// The depth of a deck way's slab on ladder step 2, metres.
pub const SLAB: &str = "slab";
/// Two metres along that a cell and the bake read on one station are this near.
const SAME_M: f64 = 1e-6;
/// A chain of decks over decks is short; a loop (a spiral ramp) stops growing here.
const CHAIN_ROUNDS: usize = 8;

const STATES: [(State, char); 4] = [(State::Deck, 'D'), (State::Bore, 'B'), (State::Open, 'O'), (State::Hidden, 'H')];
const KINDS: [(EndKind, char); 3] = [(EndKind::A, 'A'), (EndKind::Portal, 'P'), (EndKind::Closed, 'C')];

/// What a cell decides of one feature: the runs and end lines of its ways inside the cell's square, in metres along
/// its stroke; the storey of its deck; the least depth of its slab on ladder step 2.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub runs: Vec<(f64, f64, State)>,
    pub lines: Vec<(f64, EndKind)>,
    pub storey: i8,
    pub slab: Option<f64>,
}

/// What one cell decides: per feature, by its zoom-13 tile and its position there, what `Found` holds; and each deck
/// group (`WayGraph::decks`) whose ways belong to two features or more, as their keys. One deck group has one storey.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub found: BTreeMap<[u32; 3], Found>,
    pub groups: Vec<Vec<[u32; 3]>>,
}

/// The metres along the stroke at a station.
fn along(g: &WayGraph, st: &Station) -> f64 {
    let part = &g.parts[st.part as usize];
    let span = &part.spans[st.span as usize];
    span.along0_m + if part.backward { -1.0 } else { 1.0 } * s_at(span, st.u)
}

/// Whether every part of the feature of a way has an alignment: else its metres along start again in each tile.
fn aligned(g: &WayGraph, input: &CellInput, w: u32) -> bool {
    let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
    f.parts.iter().all(|p| p.plan.is_some())
}

fn deck_way(g: &WayGraph, w: u32) -> bool {
    let x = &g.ways[w as usize];
    x.level == Level::Deck && x.family != Family::Water
}

/// The storey of each way after `levels`: 0 off a deck; 1 for a deck group (`WayGraph::decks`), and one more than
/// each group under it at a crossing whose ladder gives a deck (full or thin). A deck is a DECK run of a bridge way,
/// or a synthetic deck whole.
pub(super) fn storeys(g: &WayGraph) -> Vec<i8> {
    let mut decked: BTreeMap<u32, Vec<(Station, Station)>> = BTreeMap::new();
    for r in g.levels.runs.iter().filter(|r| r.state == State::Deck && deck_way(g, r.way)) {
        decked.entry(r.way).or_default().push((r.from, r.to));
    }
    let tagged: Vec<u32> = g.levels.runs.iter().map(|r| r.way).collect();
    for w in (0..g.ways.len() as u32).filter(|&w| deck_way(g, w) && !tagged.contains(&w)) {
        decked.insert(w, vec![(g.ways[w as usize].from, g.ways[w as usize].to)]);
    }
    let inside = |w: u32, st: &Station| {
        decked.get(&w).is_some_and(|ps| ps.iter().any(|(a, b)| key(a) <= key(st) && key(st) <= key(b)))
    };
    let over: Vec<(usize, usize)> = (g.crossings.iter())
        .filter(|c| c.kind == CrossingKind::Deck)
        .filter_map(|c| {
            let l = c.ladder.as_ref().filter(|l| matches!(l.step, Step::Full | Step::Thin))?;
            let (up, low) = (g.ways[l.upper as usize].deck?, g.ways[l.lower as usize].deck?);
            (up != low && inside(l.upper, &c.over) && inside(l.lower, &c.under)).then_some((low as usize, up as usize))
        })
        .collect();
    let mut group = vec![1i8; g.decks.len()];
    for _ in 0..CHAIN_ROUNDS {
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
    (0..g.ways.len() as u32)
        .map(|w| match (decked.contains_key(&w), g.ways[w as usize].deck) {
            (true, Some(d)) => group[d as usize],
            (true, None) => 1,
            (false, _) => 0,
        })
        .collect()
}

/// Whether way w lies inside the cell's square: a way lies on one side of each cell line, and the middle of its first
/// chord tells which.
pub fn inside(g: &WayGraph, w: u32) -> bool {
    let r = &g.ways[w as usize].samples;
    if r.is_empty() {
        return false;
    }
    let (a, b) = (g.samples[r.start as usize].pos, g.samples[(r.start as usize + 1).min(r.end as usize - 1)].pos);
    let (o, mid) = (g.cell.origin(), |k: usize| (i64::from(a[k]) + i64::from(b[k])) / 2);
    g.cell.contains(GlobalPoint { x: o.x + mid(0), y: o.y + mid(1) })
}

/// What a cell's graph after `levels` decides of each feature that a way inside the cell's square has, and the deck
/// groups that join features.
pub fn found(g: &WayGraph, input: &CellInput, sections: &Sections) -> Cell {
    let feature = |w: u32| {
        let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
        [f.parts[0].tile.0, f.parts[0].tile.1, f.index]
    };
    let mut groups = Vec::new();
    for d in &g.decks {
        let mut keys: Vec<[u32; 3]> = d.ways.iter().filter(|&&w| aligned(g, input, w)).map(|&w| feature(w)).collect();
        keys.sort_unstable();
        keys.dedup();
        if keys.len() > 1 {
            groups.push(keys);
        }
    }
    let storey = storeys(g);
    let mut out: BTreeMap<[u32; 3], Found> = BTreeMap::new();
    for w in (0..g.ways.len() as u32).filter(|&w| inside(g, w) && aligned(g, input, w)) {
        let runs: Vec<&Run> = g.levels.runs.iter().filter(|r| r.way == w).collect();
        let lines: Vec<&EndLine> = g.levels.end_lines.iter().filter(|l| l.way == w).collect();
        let thin = (g.crossings.iter().filter_map(|c| c.ladder.as_ref()))
            .filter(|l| l.upper == w && l.step == Step::Thin && deck_way(g, w))
            .map(|l| {
                let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
                let least = sections.way(&f.kind, &f.tags, false).deck_least_m;
                (l.room_m - l.least_m + least).max(least)
            })
            .reduce(f64::min);
        if runs.is_empty() && storey[w as usize] < 2 && thin.is_none() {
            continue;
        }
        let found = out.entry(feature(w)).or_default();
        found.runs.extend(runs.iter().map(|r| (along(g, &r.from), along(g, &r.to), r.state)));
        found.lines.extend(lines.iter().map(|l| (along(g, &l.at), l.kind)));
        found.storey = found.storey.max(storey[w as usize]);
        found.slab = match (found.slab, thin) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    Cell { found: out, groups }
}

/// The kind of the end line between two states.
pub fn kind_between(a: State, b: State) -> EndKind {
    match (a, b) {
        (State::Hidden, _) | (_, State::Hidden) => EndKind::Closed,
        (State::Deck, _) | (_, State::Deck) => EndKind::A,
        _ => EndKind::Portal,
    }
}

/// The tags of each feature id from what the cells found of it, in the order of the cells, and from the deck groups
/// that join ids: the features of one deck group, joined over all cells, take the highest storey among them.
pub fn merge(found: &BTreeMap<u32, Vec<Found>>, groups: &[Vec<u32>]) -> BTreeMap<u32, Tags> {
    let mut ids: Vec<u32> = found.keys().copied().chain(groups.iter().flatten().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let at = |id: u32| ids.binary_search(&id).unwrap();
    let mut root: Vec<usize> = (0..ids.len()).collect();
    fn find(root: &mut [usize], mut i: usize) -> usize {
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for g in groups {
        for pair in g.windows(2) {
            let (a, b) = (find(&mut root, at(pair[0])), find(&mut root, at(pair[1])));
            root[a.max(b)] = a.min(b);
        }
    }
    let mut storey = vec![0i8; ids.len()];
    for (&id, fs) in found {
        let r = find(&mut root, at(id));
        storey[r] = fs.iter().map(|f| f.storey).fold(storey[r], i8::max);
    }
    let mut out = BTreeMap::new();
    for (k, &id) in ids.iter().enumerate() {
        let r = find(&mut root, k);
        let t = tags(found.get(&id).map_or(&[], Vec::as_slice), storey[r]);
        if !t.is_empty() {
            out.insert(id, t);
        }
    }
    out
}

/// The tags of one feature from what each cell found of it, with the storey of its deck group. The runs join in
/// rising metres; a change of state that no cell gives a line keeps the line of its two states. Lines at one metre
/// come in the order of their kinds (A, portal, closed).
fn tags(found: &[Found], storey: i8) -> Tags {
    let (mut runs, mut lines, mut slab) = (Vec::new(), Vec::new(), None::<f64>);
    for f in found {
        runs.extend(f.runs.iter().map(|&(a, b, s)| (a.min(b), a.max(b), s)));
        lines.extend(f.lines.iter().copied());
        slab = f.slab.map_or(slab, |s| Some(slab.map_or(s, |x: f64| x.min(s))));
    }
    runs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
    for k in 1..runs.len() {
        if runs[k].2 != runs[k - 1].2 && !lines.iter().any(|l| (l.0 - runs[k].0).abs() <= SAME_M) {
            lines.push((runs[k].0, kind_between(runs[k - 1].2, runs[k].2)));
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    lines.dedup_by(|a, b| (a.0 - b.0).abs() <= SAME_M && a.1 == b.1);
    let mut out = Tags::new();
    if let Some(first) = runs.first() {
        // The state past each line: of the run that starts there, else of the run that holds it.
        let past = |m: f64| {
            let k = runs.partition_point(|r| r.0 <= m + SAME_M);
            runs[k.saturating_sub(1)].2
        };
        let letter = |s: State| STATES.iter().find(|x| x.0 == s).unwrap().1;
        let mut text = letter(first.2).to_string();
        for &(m, kind) in &lines {
            let k = KINDS.iter().find(|x| x.0 == kind).unwrap().1;
            text.push_str(&format!(" {m} {k} {}", letter(past(m))));
        }
        out.insert(LEVELS.into(), text);
    }
    if storey > 1 {
        out.insert(STOREY.into(), storey.to_string());
    }
    if let Some(s) = slab {
        out.insert(SLAB.into(), s.to_string());
    }
    out
}

/// An end line of `levels`: its metres along, its kind and the state past it.
type Line = (f64, EndKind, State);

/// A feature's `levels`: its first state and its lines.
fn parse(text: &str) -> Option<(State, Vec<Line>)> {
    let state = |t: &str| STATES.iter().find(|x| t.len() == 1 && t.starts_with(x.1)).map(|x| x.0);
    let kind = |t: &str| KINDS.iter().find(|x| t.len() == 1 && t.starts_with(x.1)).map(|x| x.0);
    let mut t = text.split(' ');
    let first = state(t.next()?)?;
    let mut lines = Vec::new();
    while let Some(m) = t.next() {
        lines.push((m.parse().ok()?, kind(t.next()?)?, state(t.next()?)?));
    }
    Some((first, lines))
}

/// The end line of way w at a station: across its stroke, and its band where `structure` is a deck or a bore.
pub fn end_line(
    g: &WayGraph,
    input: &CellInput,
    sections: &Sections,
    edge_band_m: f64,
    (w, st, kind, structure): (u32, Station, EndKind, State),
) -> EndLine {
    let part = &g.parts[st.part as usize];
    let sp = point(part, st.span as usize, st.u);
    let p = exact(&g.parts, &st);
    let half = match classify(&input.features[part.feature.0 as usize], sections).map(|c| c.3) {
        Some(Rule::Way(s, _)) => s.half_m + sp.widen_m,
        Some(Rule::Water(r)) => r.half_m,
        None => 0.0,
    };
    let band = if matches!(structure, State::Deck | State::Bore) { edge_band_m } else { 0.0 };
    let h = (half + band) / metres_per_unit_at(p[1]);
    let (n, o) = ([-sp.tangent[1], sp.tangent[0]], g.cell.origin());
    let local = |s: f64| {
        let q = [p[0] + s * n[0] * h, p[1] + s * n[1] * h];
        [(libm::round(q[0]) - o.x as f64) as i32, (libm::round(q[1]) - o.y as f64) as i32]
    };
    EndLine { kind, way: w, at: st, ends: [local(1.0), local(-1.0)] }
}

/// The runs and end lines of every way whose feature carries `levels` (`WayGraph::levels`, as `levels` gives them).
pub fn read(g: &mut WayGraph, input: &CellInput, sections: &Sections, edge_band_m: f64) {
    let mut levels = super::Levels::default();
    for w in 0..g.ways.len() as u32 {
        let way = &g.ways[w as usize];
        let tags = &input.features[g.parts[way.part as usize].feature.0 as usize].tags;
        let Some((first, lines)) = tags.get(LEVELS).filter(|_| aligned(g, input, w)).and_then(|t| parse(t)) else {
            continue;
        };
        let state_at = |m: f64| lines.iter().take_while(|l| l.0 <= m).last().map_or(first, |l| l.2);
        let (a0, a1) = (along(g, &way.from), along(g, &way.to));
        let at = |m: f64| -> Station {
            if (m - a0).abs() <= SAME_M {
                return way.from;
            }
            if (m - a1).abs() <= SAME_M {
                return way.to;
            }
            let part = &g.parts[way.part as usize];
            let sign = if part.backward { -1.0 } else { 1.0 };
            let span = (way.from.span..=way.to.span)
                .find(|&k| {
                    let s = sign * (m - part.spans[k as usize].along0_m);
                    (0.0..=part.spans[k as usize].len_m).contains(&s)
                })
                .unwrap_or(way.to.span);
            let s = sign * (m - part.spans[span as usize].along0_m);
            norm(part, Station { part: way.part, span, u: u_at(&part.spans[span as usize], s) })
        };
        let (lo, hi) = (a0.min(a1) - SAME_M, a0.max(a1) + SAME_M);
        // The lines in rising metres, so in the way's order where its metres rise along it.
        let mut here: Vec<(Station, EndKind, f64)> =
            lines.iter().filter(|l| (lo..=hi).contains(&l.0)).map(|l| (at(l.0), l.1, l.0)).collect();
        if a1 < a0 {
            here.reverse();
        }
        // The runs: split at each line inside the way, each the state at its middle, equal neighbours joined.
        let mut cuts: Vec<(Station, f64)> = vec![(way.from, a0)];
        cuts.extend(here.iter().filter(|h| key(&h.0) > key(&way.from) && key(&h.0) < key(&way.to)).map(|h| (h.0, h.2)));
        cuts.push((way.to, a1));
        let mut runs: Vec<Run> = Vec::new();
        for c in cuts.windows(2) {
            let state = state_at((c[0].1 + c[1].1) / 2.0);
            match runs.last_mut() {
                Some(r) if r.state == state => r.to = c[1].0,
                _ => runs.push(Run { way: w, from: c[0].0, to: c[1].0, state }),
            }
        }
        for (st, kind, m) in here {
            // The structure side: the lower of the states about the line, as `levels` gives the band.
            let structure = state_at(m - SAME_M * 2.0).min(state_at(m + SAME_M * 2.0));
            levels.end_lines.push(end_line(g, input, sections, edge_band_m, (w, st, kind, structure)));
        }
        levels.runs.extend(runs);
    }
    g.levels = levels;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A viaduct of two features joined on its deck, and a third feature joined to the second: only the first crosses
    /// over another deck, far from the joints, so no cell sees that crossing and the others at once. Every feature of
    /// the deck group takes the upper storey; a lone deck keeps its own.
    #[test]
    fn the_features_of_one_deck_group_take_one_storey_over_all_cells() {
        let deck = |storey| Found { runs: vec![(0.0, 50.0, State::Deck)], lines: Vec::new(), storey, slab: None };
        // Feature 1 in the cell of the crossing and in a cell past it; 3 and 5 in other cells; 7 only in a group.
        let found = BTreeMap::from([(1, vec![deck(2), deck(1)]), (3, vec![deck(1)]), (5, vec![deck(1)])]);
        let tags = merge(&found, &[vec![1, 3], vec![3, 7]]);
        let storey = |id: u32| tags.get(&id).and_then(|t| t.get(STOREY)).map(String::as_str);
        assert_eq!([storey(1), storey(3), storey(7)], [Some("2"); 3]);
        assert_eq!(storey(5), None, "a deck on storey 1 has no tag");
        assert_eq!(tags[&7].get(LEVELS), None, "a feature that no cell decides has only the storey");
    }
}
