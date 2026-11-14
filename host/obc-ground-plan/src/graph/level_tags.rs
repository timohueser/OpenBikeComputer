//! The levels of the way graph as the feature tiles carry them: the bake runs `levels` on each cell and writes the
//! tags of each way from what the cells decide (`found`, `merge`); a cell's graph splits its ways at the end lines that
//! the tags give and takes the level of each stretch's state (`split`).
//!
//! A place on a way is its metres along its stroke (`Sample::along_m`): the same in every cell, as the spans are.

use std::collections::BTreeMap;

use super::curve::{insert, point, s_at, u_at};
use super::parts::{classify, Rule};
use super::points::{exact, key, norm, Piece, Plan};
use super::{CrossingKind, EndKind, EndLine, Family, Level, Part, Run, Sample, State, Station, Step, WayGraph};
use crate::faces::records::FLAT_TURN_DEG;
use crate::frame::{metres_per_unit_at, CellKey, GlobalPoint};
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
/// its stroke; the least depth of its slab on ladder step 2.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub runs: Vec<(f64, f64, State)>,
    pub lines: Vec<(f64, EndKind)>,
    pub slab: Option<f64>,
}

/// What one cell decides: per feature, by its zoom-13 tile and its position there, what `Found` holds; each deck group
/// (`WayGraph::decks`) whose ways belong to two features or more, as their keys; and the cell's own crossings of a
/// deck over a deck, as the keys of the lower and the upper feature. One deck group has one storey.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub found: BTreeMap<[u32; 3], Found>,
    pub groups: Vec<Vec<[u32; 3]>>,
    pub over: Vec<([u32; 3], [u32; 3])>,
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

/// The crossings of a deck over a deck that the cell owns, whose ladder gives a deck (full or thin), as (lower way,
/// upper way). A deck is a DECK run of a bridge way, or a synthetic deck whole.
fn over(g: &WayGraph) -> Vec<(u32, u32)> {
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
    (g.crossings.iter())
        .filter(|c| c.kind == CrossingKind::Deck && c.owned)
        .filter_map(|c| {
            let l = c.ladder.as_ref().filter(|l| matches!(l.step, Step::Full | Step::Thin))?;
            (inside(l.upper, &c.over) && inside(l.lower, &c.under)).then_some((l.lower, l.upper))
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
    let pairs = over(g).into_iter().filter(|&(l, u)| aligned(g, input, l) && aligned(g, input, u));
    let over = pairs.map(|(l, u)| (feature(l), feature(u))).collect();
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
        if runs.is_empty() && thin.is_none() {
            continue;
        }
        let found = out.entry(feature(w)).or_default();
        found.runs.extend(runs.iter().map(|r| (along(g, &r.from), along(g, &r.to), r.state)));
        found.lines.extend(lines.iter().map(|l| (along(g, &l.at), l.kind)));
        found.slab = match (found.slab, thin) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    Cell { found: out, groups, over }
}

/// The kind of the end line between two states.
pub fn kind_between(a: State, b: State) -> EndKind {
    match (a, b) {
        (State::Hidden, _) | (_, State::Hidden) => EndKind::Closed,
        (State::Deck, _) | (_, State::Deck) => EndKind::A,
        _ => EndKind::Portal,
    }
}

/// The tags of each feature id from what the cells found of it, in the order of the cells, from the deck groups that
/// join ids and from the crossings of a deck over a deck (lower id, upper id). The features of one deck group, joined
/// over all cells, have one storey: 1, and one more than each group under them.
pub fn merge(found: &BTreeMap<u32, Vec<Found>>, groups: &[Vec<u32>], over: &[(u32, u32)]) -> BTreeMap<u32, Tags> {
    let ends = over.iter().flat_map(|&(l, u)| [l, u]);
    let mut ids: Vec<u32> = found.keys().copied().chain(groups.iter().flatten().copied()).chain(ends).collect();
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
    // The chain on the joined groups; a crossing of a group over itself lifts nothing.
    let pairs: Vec<(usize, usize)> =
        over.iter().map(|&(l, u)| (find(&mut root, at(l)), find(&mut root, at(u)))).collect();
    let mut storey = vec![1i8; ids.len()];
    for _ in 0..CHAIN_ROUNDS {
        let mut changed = false;
        for &(low, up) in pairs.iter().filter(|p| p.0 != p.1) {
            if storey[up] <= storey[low] {
                storey[up] = storey[low] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
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

/// The pieces of a plan split at the end lines that the tiles carry (`LEVELS`), each with the level of its state: a
/// DECK stretch on its deck, an OPEN stretch on the ground (a bridge way's stub, or any way of the road solver), a
/// HIDDEN stretch not at all. A tunnel way keeps its level, and so does a synthetic deck. Each cut is a sample of its
/// part, so the stretches meet at a joint.
pub(crate) fn split(cell: CellKey, parts: &[Part], input: &CellInput, samples: &mut [Vec<Sample>], plan: Plan) -> Plan {
    let mut out = Vec::with_capacity(plan.pieces.len());
    for piece in &plan.pieces {
        let part = &parts[piece.part];
        let f = &input.features[part.feature.0 as usize];
        let tag = f.tags.get(LEVELS).filter(|_| f.parts.iter().all(|p| p.plan.is_some()));
        let levels = tag.and_then(|t| parse(t)).filter(|_| !piece.synthetic && part.level != Level::Tunnel);
        let Some((first, lines)) = levels else {
            out.push(piece.clone());
            continue;
        };
        let sign = if part.backward { -1.0 } else { 1.0 };
        let along = |st: &Station| {
            let span = &part.spans[st.span as usize];
            span.along0_m + sign * s_at(span, st.u)
        };
        let state_at = |m: f64| lines.iter().take_while(|l| l.0 <= m).last().map_or(first, |l| l.2);
        let (a0, a1) = (along(&piece.from), along(&piece.to));
        let (lo, hi) = (a0.min(a1) + SAME_M, a0.max(a1) - SAME_M);
        // The station at metres m along the piece: on the first of its spans whose metres hold m.
        let at = |m: f64| {
            let span = (piece.from.span..=piece.to.span)
                .find(|&k| {
                    (0.0..=part.spans[k as usize].len_m).contains(&(sign * (m - part.spans[k as usize].along0_m)))
                })
                .unwrap_or(piece.to.span);
            let s = sign * (m - part.spans[span as usize].along0_m);
            norm(part, Station { part: piece.part as u32, span, u: u_at(&part.spans[span as usize], s) })
        };
        let mut cuts: Vec<(Station, f64)> = vec![(piece.from, a0)];
        let inner = lines.iter().map(|l| l.0).filter(|&m| (lo..=hi).contains(&m));
        let mut inner: Vec<(Station, f64)> =
            inner.map(|m| (insert(part, &mut samples[piece.part], at(m), cell), m)).collect();
        if a1 < a0 {
            inner.reverse();
        }
        cuts.extend(inner.into_iter().filter(|c| key(&c.0) > key(&piece.from) && key(&c.0) < key(&piece.to)));
        cuts.push((piece.to, a1));
        for c in cuts.windows(2) {
            let level = match state_at((c[0].1 + c[1].1) / 2.0) {
                State::Deck => Level::Deck,
                State::Open => Level::Ground,
                State::Bore => part.level,
                State::Hidden => continue,
            };
            match out.last_mut() {
                Some(p) if p.part == piece.part && p.to == c[0].0 && p.level == level && p.link == piece.link => {
                    p.to = c[1].0
                }
                _ => out.push(Piece { from: c[0].0, to: c[1].0, level, ..piece.clone() }),
            }
        }
    }
    Plan { pieces: out, ..plan }
}

/// A junction with arms on the ground and on a deck lies on the ground: its arms meet at one height there, and the
/// ground's junction rounds its corners. Each deck arm's stretch from the junction up to its mouth, its first sample
/// where its stroke (its outer half-width and `band_m`) has left the strokes of the junction's ground arms that leave
/// across it, is a ground piece, which meets the rest of the deck at a joint as a stub meets its deck. A deck arm that
/// never leaves them is a ground piece as a whole. The mouth is a sample of the deck, so no new sample lies a few
/// centimetres from an old one.
/// Piece ends this near (frame units) are one node: a station's point is a curve point, not its vertex's exact one.
const NODE_UNITS: f64 = 4.0;

pub(crate) fn ground_junctions(
    parts: &[Part],
    input: &CellInput,
    sections: &Sections,
    band_m: f64,
    samples: &[Vec<Sample>],
    mut plan: Plan,
) -> Plan {
    // A mouth can land on another node of the deck (a junction of two decks near the ground's): it lies on the
    // ground then too. Each round only lowers deck to the ground at the samples of the decks, so the rounds end.
    loop {
        let next = ground_junctions_once(parts, input, sections, band_m, samples, plan.clone());
        if next.pieces == plan.pieces {
            return plan;
        }
        plan = next;
    }
}

/// A piece's arm at a node: the piece, its end (0, 1) or 2 where it runs through, the station, the direction away.
type NodeArm = (usize, usize, Station, [f64; 2]);

fn ground_junctions_once(
    parts: &[Part],
    input: &CellInput,
    sections: &Sections,
    band_m: f64,
    samples: &[Vec<Sample>],
    plan: Plan,
) -> Plan {
    let feature = |p: &Piece| &input.features[parts[p.part].feature.0 as usize];
    let half_m = |p: &Piece| sections.way(&feature(p).kind, &feature(p).tags, false).outer_m();
    let global = |part: &Part, st: &Station| {
        let sp = point(part, st.span as usize, st.u);
        let v = part.vertices[st.span as usize].pos;
        ([v[0] as f64 + sp.d[0], v[1] as f64 + sp.d[1]], sp.tangent)
    };
    let units = |m: f64, y: f64| m / metres_per_unit_at(y);
    // The nodes of the pieces on the ground or on a (real) deck: per node, each piece's arm there, an end (0 or 1) or
    // a vertex that it runs through (2), with its way's direction away from the node.
    let nodes = |plan: &Plan| {
        let mut out: BTreeMap<[i64; 2], Vec<NodeArm>> = BTreeMap::new();
        for (i, p) in plan.pieces.iter().enumerate() {
            let part = &parts[p.part];
            let deck = p.level == Level::Deck && !p.synthetic;
            if part.family == Family::Water || !(p.level == Level::Ground || deck) {
                continue;
            }
            let through = (p.from.span + 1..=p.to.span).map(|k| Station { span: k, u: 0.0, ..p.from });
            let arms = [(0, p.from), (1, p.to)].into_iter().chain(through.map(|st| (2, st)));
            for (e, st) in arms.filter(|(e, st)| *e < 2 || key(st) < key(&p.to)) {
                let (pos, t) = global(part, &st);
                let node = [(pos[0] / NODE_UNITS).round() as i64, (pos[1] / NODE_UNITS).round() as i64];
                let back = [-t[0], -t[1]];
                let list = out.entry(node).or_default();
                match e {
                    0 => list.push((i, 0, st, t)),
                    1 => list.push((i, 1, st, back)),
                    _ => list.extend([(i, 2, st, t), (i, 2, st, back)]),
                }
            }
        }
        out
    };
    let mixed = |arms: &[NodeArm], plan: &Plan| {
        arms.len() >= 3
            && arms.iter().any(|a| plan.pieces[a.0].level == Level::Ground)
            && arms.iter().any(|a| plan.pieces[a.0].level == Level::Deck)
    };
    // A deck that runs through such a node ends there.
    let mut cuts: Vec<(usize, Station)> = (nodes(&plan).values())
        .filter(|arms| mixed(arms, &plan))
        .flat_map(|arms| arms.iter().filter(|a| a.1 == 2 && plan.pieces[a.0].level == Level::Deck).map(|a| (a.0, a.2)))
        .collect();
    cuts.sort_by(|a, b| a.0.cmp(&b.0).then(key(&a.1).partial_cmp(&key(&b.1)).unwrap()));
    cuts.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    let plan = if cuts.is_empty() {
        plan
    } else {
        let mut pieces = Vec::with_capacity(plan.pieces.len() + cuts.len());
        for (i, p) in plan.pieces.iter().enumerate() {
            let mut from = p.from;
            for c in cuts.iter().filter(|c| c.0 == i) {
                pieces.push(Piece { from, to: c.1, ..p.clone() });
                from = c.1;
            }
            pieces.push(Piece { from, ..p.clone() });
        }
        Plan { pieces, ..plan }
    };
    let ends = nodes(&plan);
    // Per piece and end, the station of its mouth (none: the piece lies in the junction as a whole).
    let mut mouths: BTreeMap<(usize, usize), Option<Station>> = BTreeMap::new();
    for arms in ends.values().filter(|a| mixed(a, &plan)) {
        let ground: Vec<(usize, [f64; 2])> =
            arms.iter().filter(|a| plan.pieces[a.0].level == Level::Ground).map(|a| (a.0, a.3)).collect();
        // The ground arms' strokes: points along their lines, their half-widths (units) and their directions.
        let lines: Vec<(Vec<[f64; 2]>, f64, [f64; 2])> = (ground.iter().map(|&(g, dir)| (&plan.pieces[g], dir)))
            .map(|(p, dir)| {
                let part = &parts[p.part];
                let pts: Vec<[f64; 2]> = (p.from.span..=p.to.span)
                    .flat_map(|k| (0..=8).map(move |j| (k, f64::from(j) / 8.0)))
                    .filter(|&(k, u)| (k, u) >= (p.from.span, p.from.u) && (k, u) <= (p.to.span, p.to.u))
                    .map(|(k, u)| global(part, &Station { part: p.from.part, span: k, u }).0)
                    .collect();
                let y = pts.first().map_or(0.0, |q| q[1]);
                (pts, units(half_m(p), y), dir)
            })
            .collect();
        // A ground arm within `FLAT_TURN_DEG` of the deck arm runs beside it, as two ways that meet side by side: it
        // holds no mouth.
        let across = |a: [f64; 2], b: [f64; 2]| a[0] * b[0] + a[1] * b[1] < libm::cos(FLAT_TURN_DEG.to_radians());
        let near = |q: [f64; 2], d: [f64; 2]| {
            lines.iter().filter(|l| across(l.2, d)).any(|(pts, h, _)| {
                pts.windows(2).any(|w| {
                    let (a, b) = (w[0], w[1]);
                    let d = [b[0] - a[0], b[1] - a[1]];
                    let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-9);
                    let t = (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
                    libm::hypot(q[0] - a[0] - t * d[0], q[1] - a[1] - t * d[1]) < *h
                })
            })
        };
        for &(i, e, _, dir) in arms.iter().filter(|a| a.1 < 2 && plan.pieces[a.0].level == Level::Deck) {
            let p = &plan.pieces[i];
            let part = &parts[p.part];
            let k = |x: &Sample| (x.span, x.u);
            let mut inner: Vec<Station> = (samples[p.part].iter())
                .filter(|x| k(x) > (p.from.span, p.from.u) && k(x) < (p.to.span, p.to.u))
                .map(|x| Station { part: p.from.part, span: x.span, u: x.u })
                .collect();
            if e == 1 {
                inner.reverse();
            }
            let clear = |st: &Station| {
                let (c, t) = global(part, st);
                let h = units(half_m(p) + band_m, c[1]);
                let cross = [-1.0, -0.5, 0.0, 0.5, 1.0].map(|k| [c[0] - t[1] * h * k, c[1] + t[0] * h * k]);
                !cross.iter().any(|&q| near(q, dir))
            };
            mouths.insert((i, e), inner.into_iter().find(clear));
        }
    }
    if mouths.is_empty() {
        return plan;
    }
    let mut out = Vec::with_capacity(plan.pieces.len() + mouths.len());
    for (i, p) in plan.pieces.iter().enumerate() {
        let (a, b) = (mouths.get(&(i, 0)).copied(), mouths.get(&(i, 1)).copied());
        if a.is_none() && b.is_none() {
            out.push(p.clone());
            continue;
        }
        // The deck between the mouths, the junctions' stretches on the ground.
        let (lo, hi) = (a.unwrap_or(Some(p.from)), b.unwrap_or(Some(p.to)));
        match lo.zip(hi).filter(|(l, h)| key(l) < key(h)) {
            None => out.push(Piece { level: Level::Ground, ..p.clone() }),
            Some((l, h)) => {
                if l != p.from {
                    out.push(Piece { to: l, level: Level::Ground, ..p.clone() });
                }
                out.push(Piece { from: l, to: h, ..p.clone() });
                if h != p.to {
                    out.push(Piece { from: h, level: Level::Ground, ..p.clone() });
                }
            }
        }
    }
    Plan { pieces: out, ..plan }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A viaduct of two features joined on its deck, and a third feature joined to the second: only the first crosses
    /// over a deck L, and a deck U crosses over the second, each far from the other and from the joints, so no cell sees
    /// two of them at once. The viaduct takes storey 2 as a whole and U storey 3; a crossing of the viaduct over itself
    /// lifts nothing; L and a lone deck keep storey 1.
    #[test]
    fn the_storey_chain_runs_over_the_deck_groups_of_all_cells() {
        let deck = || Found { runs: vec![(0.0, 50.0, State::Deck)], lines: Vec::new(), slab: None };
        let found =
            BTreeMap::from([(1, vec![deck(), deck()]), (3, vec![deck()]), (5, vec![deck()]), (20, vec![deck()])]);
        let tags = merge(&found, &[vec![1, 3], vec![3, 7]], &[(10, 1), (3, 20), (1, 7)]);
        let storey = |id: u32| tags.get(&id).and_then(|t| t.get(STOREY)).map(String::as_str);
        assert_eq!([storey(1), storey(3), storey(7)], [Some("2"); 3]);
        assert_eq!(storey(20), Some("3"), "U over the viaduct");
        assert_eq!([storey(10), storey(5)], [None, None], "a deck on storey 1 has no tag");
        assert_eq!(tags[&7].get(LEVELS), None, "a feature that no cell decides has only the storey");
    }
}
