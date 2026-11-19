//! The levels of the way graph as the feature tiles carry them: the bake runs `levels` on each cell and writes the
//! tags of each way from what the cells decide (`found`, `merge`); a cell's graph splits its ways at the end lines that
//! the tags give and takes the level of each stretch's state (`split`).
//!
//! A place on a way is its metres along its stroke (`Sample::along_m`): the same in every cell, as the spans are.

use std::collections::BTreeMap;

use super::curve::{insert, point, s_at, u_at};
use super::levels::TunnelFound;
use super::parts::{classify, tunnel, Rule};
use super::points::{exact, key, norm, Piece, Plan};
use super::{
    CrossingKind, EndKind, EndLine, Family, Level, Part, Run, Sample, State, Station, Step, Structures, WayGraph,
};
use crate::faces::records::FLAT_TURN_DEG;
use crate::frame::{metres_per_unit_at, CellKey, GlobalPoint, CELL_UNITS};
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
/// Two features meet at a node where their ends lie this near, metres along.
const LINK_M: f64 = 0.5;

const STATES: [(State, char); 4] = [(State::Deck, 'D'), (State::Bore, 'B'), (State::Open, 'O'), (State::Hidden, 'H')];
const KINDS: [(EndKind, char); 3] = [(EndKind::A, 'A'), (EndKind::Portal, 'P'), (EndKind::Closed, 'C')];

/// What a cell decides of one feature: the runs and end lines of its ways inside the cell's square, in metres along
/// its stroke; the least depth of its slab on ladder step 2. Of a tunnel's ways (`TunnelFound`), what the merge
/// decides its states from: their extents, their stretches with room, with a way on top and over the body of a way
/// that they end at, and their tag ends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Found {
    pub runs: Vec<(f64, f64, State)>,
    pub lines: Vec<(f64, EndKind)>,
    pub slab: Option<f64>,
    pub tunnel: Vec<(f64, f64)>,
    pub room: Vec<(f64, f64)>,
    pub top: Vec<(f64, f64)>,
    pub stay: Vec<(f64, f64)>,
    /// Metres along and the way into the tunnel there: 1.0 to rising metres, -1.0 to falling.
    pub tag_ends: Vec<(f64, f64)>,
}

/// What one cell decides: per feature, by its zoom-13 tile and its position there, what `Found` holds; each deck group
/// (`WayGraph::decks`) whose ways belong to two features or more, as their keys; and the cell's own crossings of a
/// deck over a deck, as the keys of the lower and the upper feature. One deck group has one storey.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    pub found: BTreeMap<[u32; 3], Found>,
    pub groups: Vec<Vec<[u32; 3]>>,
    pub over: Vec<([u32; 3], [u32; 3])>,
    /// Two tunnel features whose ways meet at a node, each with its metres along there and the way into it (as a
    /// tag end).
    pub links: Vec<Link<[u32; 3]>>,
}

/// Two tunnel features (`F`) that meet at a node: per feature, its metres along there and the way into it.
pub type Link<F> = (F, f64, f64, F, f64, f64);

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

/// The stretches of way w inside the cell's square, metres along its stroke: each chord clipped to the square, so two
/// cells cut a chord at one point.
fn in_square(g: &WayGraph, w: u32) -> Vec<(f64, f64)> {
    let x = &g.ways[w as usize];
    let size = CELL_UNITS as f64;
    let at = |k: u32| {
        let s = &g.samples[k as usize];
        ([f64::from(s.pos[0]), f64::from(s.pos[1])], along(g, &Station { part: x.part, span: s.span, u: s.u }))
    };
    let mut out = Vec::new();
    for k in x.samples.start..x.samples.end.saturating_sub(1) {
        let ((a, ma), (b, mb)) = (at(k), at(k + 1));
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for i in 0..2 {
            let d = b[i] - a[i];
            if d == 0.0 {
                if a[i] < 0.0 || a[i] >= size {
                    t1 = -1.0;
                }
            } else {
                let (u, v) = ((0.0 - a[i]) / d, (size - a[i]) / d);
                (t0, t1) = (t0.max(u.min(v)), t1.min(u.max(v)));
            }
        }
        if t0 < t1 {
            let (p, q) = (ma + t0 * (mb - ma), ma + t1 * (mb - ma));
            out.push((p.min(q), p.max(q)));
        }
    }
    union(out)
}

/// The parts of stretches v inside stretches `by`.
fn clip(v: &[(f64, f64)], by: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let pairs = v.iter().flat_map(|&(a, b)| by.iter().map(move |&(c, d)| (a.max(c), b.min(d))));
    pairs.filter(|x| x.0 < x.1).collect()
}

/// Whether the end of way w (its start, or its end) lies inside the cell's square: the cell whose square holds a node
/// reads all the ways there.
fn holds_end(g: &WayGraph, w: u32, start: bool) -> bool {
    let r = &g.ways[w as usize].samples;
    if r.is_empty() {
        return false;
    }
    let a = g.samples[if start { r.start } else { r.end - 1 } as usize].pos;
    let o = g.cell.origin();
    g.cell.contains(GlobalPoint { x: o.x + i64::from(a[0]), y: o.y + i64::from(a[1]) })
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
    // The tunnels: what the merge decides their states from.
    let mut links = Vec::new();
    let span = |a: &Station, b: &Station| {
        let (x, y) = (along(g, a), along(g, b));
        (x.min(y), x.max(y))
    };
    // The way into way w from its start or its end, along the stroke.
    let into = |w: u32, start: bool| {
        let x = &g.ways[w as usize];
        let rising = if along(g, &x.to) >= along(g, &x.from) { 1.0 } else { -1.0 };
        if start {
            rising
        } else {
            -rising
        }
    };
    // The links at end e of a tunnel way to the other tunnel features that go on there.
    let link = |t: &TunnelFound, e: usize| {
        let end = if e == 0 { g.ways[t.way as usize].from } else { g.ways[t.way as usize].to };
        let next = t.next[e].iter().filter(|(o, _)| aligned(g, input, *o) && feature(*o) != feature(t.way));
        next.map(|&(o, at)| {
            let o_start = at == g.ways[o as usize].from;
            (feature(t.way), along(g, &end), into(t.way, e == 0), feature(o), along(g, &at), into(o, o_start))
        })
        .collect::<Vec<_>>()
    };
    // A cell finds the stretches of a tunnel way that lie in its square (a long way runs through many windows) and
    // the ends that lie there.
    for t in g.levels.tunnels.iter().filter(|t| aligned(g, input, t.way)) {
        let way = &g.ways[t.way as usize];
        let ends: Vec<usize> = (0..2).filter(|&e| holds_end(g, t.way, e == 0)).collect();
        let here = in_square(g, t.way);
        if here.is_empty() && !ends.iter().any(|&e| t.tag[e]) {
            links.extend(ends.iter().flat_map(|&e| link(t, e)));
            continue;
        }
        let found = out.entry(feature(t.way)).or_default();
        let clip = |v: &[(Station, Station)]| clip(&v.iter().map(|(a, b)| span(a, b)).collect::<Vec<_>>(), &here);
        found.tunnel.extend(here.iter().copied());
        found.room.extend(clip(&t.room));
        found.top.extend(clip(&t.top));
        found.stay.extend(clip(&t.stay));
        for &e in &ends {
            if t.tag[e] {
                found.tag_ends.push((along(g, if e == 0 { &way.from } else { &way.to }), into(t.way, e == 0)));
            }
            links.extend(link(t, e));
        }
    }
    Cell { found: out, groups, over, links }
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
pub fn merge(
    found: &BTreeMap<u32, Vec<Found>>,
    groups: &[Vec<u32>],
    over: &[(u32, u32)],
    links: &[Link<u32>],
    s: &Structures,
) -> BTreeMap<u32, Tags> {
    let tunnels = tunnel_states(found, links, s);
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
        let t = match tunnels.get(&id) {
            Some(f) => tags(std::slice::from_ref(f), storey[r]),
            None => tags(found.get(&id).map_or(&[], Vec::as_slice), storey[r]),
        };
        if !t.is_empty() {
            out.insert(id, t);
        }
    }
    out
}

/// Sorted stretches with those that overlap or touch joined.
fn union(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            Some(l) if a <= l.1 + SAME_M => l.1 = l.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn holds(v: &[(f64, f64)], m: f64) -> bool {
    v.iter().any(|&(a, b)| a <= m && m <= b)
}

/// A tunnel feature as the cells found it (`Found`), its stretches joined. Its parts can lie apart along one
/// alignment: each holds the extents of its ways, joined.
struct Tunnel {
    extents: Vec<(f64, f64)>,
    room: Vec<(f64, f64)>,
    top: Vec<(f64, f64)>,
    stay: Vec<(f64, f64)>,
    tag_ends: Vec<(f64, f64)>,
    /// The tunnel features that go on at its metres: (metres here, the other, metres there, the way into it).
    links: Vec<(f64, u32, f64, f64)>,
}

impl Tunnel {
    /// The end of the extent that holds metres m, going the way `dir`.
    fn end(&self, m: f64, dir: f64) -> f64 {
        let e = self.extents.iter().find(|e| e.0 - LINK_M <= m && m <= e.1 + LINK_M);
        e.map_or(m, |e| if dir > 0.0 { e.1 } else { e.0 })
    }
}

/// The tunnel features that the cells found, their stretches joined, with the features that go on at their metres.
fn tunnels(found: &BTreeMap<u32, Vec<Found>>, links: &[Link<u32>]) -> BTreeMap<u32, Tunnel> {
    let mut tunnels: BTreeMap<u32, Tunnel> = BTreeMap::new();
    for (&id, fs) in found {
        let all = |k: fn(&Found) -> &Vec<(f64, f64)>| union(fs.iter().flat_map(|f| k(f).iter().copied()).collect());
        let mut extents = all(|f| &f.tunnel);
        if extents.is_empty() {
            continue;
        }
        extents.dedup_by(|b, a| {
            let near = b.0 <= a.1 + LINK_M;
            a.1 = if near { a.1.max(b.1) } else { a.1 };
            near
        });
        let mut tag_ends: Vec<(f64, f64)> = fs.iter().flat_map(|f| f.tag_ends.iter().copied()).collect();
        tag_ends.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        tag_ends.dedup_by(|a, b| (a.0 - b.0).abs() <= SAME_M && a.1 == b.1);
        tunnels.insert(
            id,
            Tunnel {
                extents,
                room: all(|f| &f.room),
                top: all(|f| &f.top),
                stay: all(|f| &f.stay),
                tag_ends,
                links: Vec::new(),
            },
        );
    }
    for &(a, ma, da, b, mb, db) in links {
        for (x, mx, y, my, dy) in [(a, ma, b, mb, db), (b, mb, a, ma, da)] {
            if let (Some(t), true) = (tunnels.get_mut(&x), found.contains_key(&y)) {
                if !t.links.iter().any(|l| l.1 == y && (l.0 - mx).abs() <= LINK_M) {
                    t.links.push((mx, y, my, dy));
                }
            }
        }
    }
    tunnels
}

/// A chain of tunnel features, walked from a tag end: per feature in the order of the walk, its id and the metres along
/// its stroke where the chain enters and leaves it; per end (first, last), whether no tunnel way goes on there (a tag
/// end). An end that is not a tag end is a node where more than one tunnel feature goes on.
#[derive(Clone, Debug, PartialEq)]
pub struct Chain {
    pub ways: Vec<(u32, f64, f64)>,
    pub tag: [bool; 2],
}

/// The chains of the tunnel features that the cells found: from each tag end along the features, on through a node
/// where one feature goes on, as `tunnel_states` walks them. A chain with two tag ends comes once. A tag end where
/// another feature links (two cells that see the node apart) starts no chain: the walk from the other side holds it.
pub fn chains(found: &BTreeMap<u32, Vec<Found>>, links: &[Link<u32>]) -> Vec<Chain> {
    let tunnels = tunnels(found, links);
    let mut out = Vec::new();
    for (&id, t) in &tunnels {
        for &(m0, d0) in t.tag_ends.iter().filter(|e| !t.links.iter().any(|l| (l.0 - e.0).abs() <= LINK_M)) {
            let (mut f, mut m, mut dir) = (id, m0, d0);
            let mut chain = Chain { ways: Vec::new(), tag: [true, false] };
            for _ in 0..=tunnels.len() {
                let t = &tunnels[&f];
                let end = t.end(m, dir);
                chain.ways.push((f, m, end));
                let next: Vec<&(f64, u32, f64, f64)> = t.links.iter().filter(|l| (l.0 - end).abs() <= LINK_M).collect();
                let seen = |g: u32, mg: f64| chain.ways.iter().any(|w| w.0 == g && (w.1 - mg).abs() <= LINK_M);
                match next[..] {
                    [&(_, g, mg, dg)] if tunnels.contains_key(&g) && !seen(g, mg) => (f, m, dir) = (g, mg, dg),
                    [] => {
                        chain.tag[1] = t.tag_ends.iter().any(|&(mt, dt)| (mt - end).abs() <= LINK_M && dt == -dir);
                        break;
                    }
                    _ => break,
                }
            }
            let (first, last) = (chain.ways[0], chain.ways[chain.ways.len() - 1]);
            // Walked from both of its tag ends, a chain is kept from the end that comes first.
            if !chain.tag[1] || (first.0, first.1) < (last.0, last.2) {
                out.push(chain);
            }
        }
    }
    out
}

/// The states of the tunnel features, as found runs and tag-end lines: along each chain of tunnel features from each
/// tag end, open up to the first point with room (its portal line), at most `open_max_m`; a bore where it has room up
/// to `body_m` behind the portal line; hidden elsewhere. A way on top that crosses an open stretch hides it there (a
/// door); one whose stroke goes on over a stretch with room is that bore's lid. A stretch over the body of a way that
/// it ends at is hidden. A chain goes on at a feature's end only where one feature goes on.
fn tunnel_states(found: &BTreeMap<u32, Vec<Found>>, links: &[Link<u32>], s: &Structures) -> BTreeMap<u32, Found> {
    let tunnels = tunnels(found, links);
    let ids: Vec<u32> = tunnels.keys().copied().collect();
    let (mut open, mut body) = (BTreeMap::<u32, Vec<(f64, f64)>>::new(), BTreeMap::<u32, Vec<(f64, f64)>>::new());
    for &id in &ids {
        for &(m0, d0) in &tunnels[&id].tag_ends {
            // The chain from this tag end: (feature, metres where it enters, direction along the stroke).
            let (mut f, mut m, mut dir) = (id, m0, d0);
            let (mut walked, mut portal) = (0.0, None::<(u32, f64, f64)>);
            for _ in 0..CHAIN_ROUNDS * 4 {
                let t = &tunnels[&f];
                let end = t.end(m, dir);
                // The first room within this extent: past its end the chain goes on along the links.
                let first = if dir > 0.0 {
                    t.room.iter().find(|r| r.1 > m).map(|r| r.0.max(m))
                } else {
                    t.room.iter().rev().find(|r| r.0 < m).map(|r| r.1.min(m))
                };
                let reach = s.open_max_m - walked;
                match first.filter(|&p| (p - m) * dir <= (end - m) * dir && (p - m).abs() <= reach) {
                    Some(p) => {
                        open.entry(f).or_default().push((m.min(p), m.max(p)));
                        portal = Some((f, p, dir));
                        break;
                    }
                    None if (end - m).abs() >= reach => {
                        let x = m + dir * reach;
                        open.entry(f).or_default().push((m.min(x), m.max(x)));
                        break;
                    }
                    None => {
                        open.entry(f).or_default().push((m.min(end), m.max(end)));
                        walked += (end - m).abs();
                        let next: Vec<&(f64, u32, f64, f64)> =
                            t.links.iter().filter(|l| (l.0 - end).abs() <= LINK_M).collect();
                        let [&(_, g, mg, dg)] = next[..] else { break };
                        (f, m, dir) = (g, mg, dg);
                    }
                }
            }
            // The body: `body_m` behind the portal line, on along the chain.
            let Some((mut f, mut m, mut dir)) = portal else { continue };
            let mut left = s.body_m;
            for _ in 0..CHAIN_ROUNDS * 4 {
                let t = &tunnels[&f];
                let end = t.end(m, dir);
                let x = if (end - m).abs() >= left { m + dir * left } else { end };
                body.entry(f).or_default().push((m.min(x), m.max(x)));
                left -= (x - m).abs();
                let next: Vec<&(f64, u32, f64, f64)> = t.links.iter().filter(|l| (l.0 - end).abs() <= LINK_M).collect();
                match next[..] {
                    [&(_, g, mg, dg)] if left > 0.0 => (f, m, dir) = (g, mg, dg),
                    _ => break,
                }
            }
        }
    }
    let mut out = BTreeMap::new();
    for (&id, t) in &tunnels {
        let (open, body) = (open.remove(&id).unwrap_or_default(), body.remove(&id).unwrap_or_default());
        let meets = |(a, b): (f64, f64)| t.room.iter().any(|r| r.0 <= b + SAME_M && a <= r.1 + SAME_M);
        let doors: Vec<(f64, f64)> = t.top.iter().copied().filter(|&x| !meets(x)).collect();
        // Between two parts of the feature no way is drawn: open, so that no line comes of it.
        let state = |m: f64| {
            if !holds(&t.extents, m) {
                State::Open
            } else if holds(&t.stay, m) {
                State::Hidden
            } else if holds(&open, m) {
                if holds(&doors, m) {
                    State::Hidden
                } else {
                    State::Open
                }
            } else if holds(&body, m) && holds(&t.room, m) {
                State::Bore
            } else {
                State::Hidden
            }
        };
        let (lo, hi) = (t.extents[0].0, t.extents[t.extents.len() - 1].1);
        let mut cuts: Vec<f64> = [&open, &body, &t.room, &doors, &t.stay, &t.extents]
            .iter()
            .flat_map(|v| v.iter().flat_map(|&(a, b)| [a, b]))
            .filter(|&m| m > lo && m < hi)
            .chain([lo, hi])
            .collect();
        cuts.sort_by(f64::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() <= SAME_M);
        let mut f = Found::default();
        for w in cuts.windows(2) {
            let st = state((w[0] + w[1]) / 2.0);
            match f.runs.last_mut() {
                Some(r) if r.2 == st => r.1 = w[1],
                _ => f.runs.push((w[0], w[1], st)),
            }
        }
        // At a tag end, its state against the ground way beyond it.
        for &(m, dir) in &t.tag_ends {
            match state(m + dir * SAME_M * 10.0) {
                State::Bore => f.lines.push((m, EndKind::Portal)),
                State::Hidden => f.lines.push((m, EndKind::Closed)),
                _ => {}
            }
        }
        out.insert(id, f);
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

/// The state that the `levels` tag of the feature of tunnel way w gives at a station; none without the tag or an
/// alignment, and within a centimetre of one of its lines.
pub(crate) fn state_at(g: &WayGraph, input: &CellInput, w: u32, st: &Station) -> Option<State> {
    let f = &input.features[g.parts[g.ways[w as usize].part as usize].feature.0 as usize];
    let (first, lines) = parse(f.tags.get(LEVELS).filter(|_| aligned(g, input, w))?)?;
    let m = along(g, st);
    if lines.iter().any(|l| (l.0 - m).abs() < 0.01) {
        return None;
    }
    Some(lines.iter().take_while(|l| l.0 <= m).last().map_or(first, |l| l.2))
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
    let half = match classify(&input.features[part.feature.0 as usize], sections, true).map(|c| c.3) {
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
/// DECK stretch on its deck, a BORE stretch in its tunnel, an OPEN stretch on the ground (a bridge's or a tunnel's
/// stub, or any way of the road solver), a HIDDEN stretch not at all. A synthetic deck keeps its level, and without
/// the bores so does a tunnel way (`parts::tunnel`). `tunnels` splits the tunnel ways alone (before the crossings),
/// else the other ways. Each cut is a sample of its part, so the stretches meet at a joint.
pub(crate) fn split(
    cell: CellKey,
    parts: &[Part],
    input: &CellInput,
    samples: &mut [Vec<Sample>],
    plan: Plan,
    tunnels: bool,
) -> Plan {
    let mut out = Vec::with_capacity(plan.pieces.len());
    for piece in &plan.pieces {
        let part = &parts[piece.part];
        let f = &input.features[part.feature.0 as usize];
        let tag = f.tags.get(LEVELS).filter(|_| f.parts.iter().all(|p| p.plan.is_some()));
        let levels = tag.and_then(|t| parse(t)).filter(|_| !piece.synthetic && tunnel(f) == tunnels);
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
        let level = |m: f64| match state_at(m) {
            State::Deck => Some(Level::Deck),
            State::Open => Some(Level::Ground),
            State::Bore => Some(part.level),
            State::Hidden => None,
        };
        assemble(piece, &cuts, level, &mut out);
    }
    Plan { pieces: out, ..plan }
}

/// The stretches of a piece between its cuts (its `from`, the stations inside it in order, its `to`, each with a
/// measure that runs monotone along the piece), each with the level that `level` gives at the mean measure of its two
/// ends (None: not drawn), onto `out`: a stretch that goes on from the last piece at its level joins it.
pub(super) fn assemble(
    piece: &Piece,
    cuts: &[(Station, f64)],
    level: impl Fn(f64) -> Option<Level>,
    out: &mut Vec<Piece>,
) {
    for c in cuts.windows(2) {
        let Some(level) = level((c[0].1 + c[1].1) / 2.0) else { continue };
        match out.last_mut() {
            Some(p) if p.part == piece.part && p.to == c[0].0 && p.level == level && p.link == piece.link => {
                p.to = c[1].0
            }
            _ => out.push(Piece { from: c[0].0, to: c[1].0, level, ..piece.clone() }),
        }
    }
}

/// A junction with arms on the ground and on a structure (a deck, or with the bores a bore) lies on the ground: its
/// arms meet at one height there, and the ground's junction rounds its corners. Each structure arm's stretch from the
/// junction up to its mouth, its first sample where its stroke (its outer half-width and `band_m`) has left the strokes
/// of the junction's ground arms that leave across it, is a ground piece, which meets the rest of the structure at a
/// joint as a stub meets its deck. A structure arm that never leaves them is a ground piece as a whole. The mouth is a
/// sample of the structure, so no new sample lies a few centimetres from an old one.
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
    let raised = |p: &Piece| !p.synthetic && (p.level == Level::Deck || (super::BORES && p.level == Level::Tunnel));
    // The nodes of the pieces on the ground or on a (real) structure: per node, each piece's arm there, an end (0 or
    // 1) or a vertex that it runs through (2), with its way's direction away from the node.
    let nodes = |plan: &Plan| {
        // The arms, and per arm its node's key (y, then x, in `NODE_UNITS`) and its place in the arms, sorted.
        let mut arms: Vec<NodeArm> = Vec::new();
        let mut keys: Vec<(u64, u32)> = Vec::new();
        for (i, p) in plan.pieces.iter().enumerate() {
            let part = &parts[p.part];
            if part.family == Family::Water || !(p.level == Level::Ground || raised(p)) {
                continue;
            }
            let through = (p.from.span + 1..=p.to.span).map(|k| Station { span: k, u: 0.0, ..p.from });
            let ends = [(0, p.from), (1, p.to)].into_iter().chain(through.map(|st| (2, st)));
            for (e, st) in ends.filter(|(e, st)| *e < 2 || key(st) < key(&p.to)) {
                let (pos, t) = global(part, &st);
                let node = |v: f64| u64::from((v / NODE_UNITS).round() as i64 as u32);
                let back = [-t[0], -t[1]];
                let new: &[NodeArm] = match e {
                    0 => &[(i, 0, st, t)],
                    1 => &[(i, 1, st, back)],
                    _ => &[(i, 2, st, t), (i, 2, st, back)],
                };
                for a in new {
                    keys.push((node(pos[1]) << 32 | node(pos[0]), arms.len() as u32));
                    arms.push(*a);
                }
            }
        }
        keys.sort_unstable();
        keys.chunk_by(|a, b| a.0 == b.0)
            .map(|c| c.iter().map(|k| arms[k.1 as usize]).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    let mixed = |arms: &[NodeArm], plan: &Plan| {
        arms.len() >= 3
            && arms.iter().any(|a| plan.pieces[a.0].level == Level::Ground)
            && arms.iter().any(|a| raised(&plan.pieces[a.0]))
    };
    // A structure that runs through such a node ends there.
    let mut cuts: Vec<(usize, Station)> = Vec::new();
    for arms in nodes(&plan).iter().filter(|arms| mixed(arms, &plan)) {
        for a in arms.iter().filter(|a| a.1 == 2 && raised(&plan.pieces[a.0])) {
            if !cuts.contains(&(a.0, a.2)) {
                // In order along the piece: a few cuts.
                let at = cuts.iter().position(|c| (c.0, key(&c.1)) > (a.0, key(&a.2))).unwrap_or(cuts.len());
                cuts.insert(at, (a.0, a.2));
            }
        }
    }
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
    let mut mouths: Vec<((usize, usize), Option<Station>)> = Vec::new();
    for arms in ends.iter().filter(|a| mixed(a, &plan)) {
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
        for &(i, e, _, dir) in arms.iter().filter(|a| a.1 < 2 && raised(&plan.pieces[a.0])) {
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
            mouths.push(((i, e), inner.into_iter().find(clear)));
        }
    }
    if mouths.is_empty() {
        return plan;
    }
    let mut out = Vec::with_capacity(plan.pieces.len() + mouths.len());
    for (i, p) in plan.pieces.iter().enumerate() {
        let mouth = |e: usize| mouths.iter().find(|m| m.0 == (i, e)).map(|m| m.1);
        let (a, b) = (mouth(0), mouth(1));
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
        let deck = || Found { runs: vec![(0.0, 50.0, State::Deck)], ..Default::default() };
        let found =
            BTreeMap::from([(1, vec![deck(), deck()]), (3, vec![deck()]), (5, vec![deck()]), (20, vec![deck()])]);
        let tags = merge(
            &found,
            &[vec![1, 3], vec![3, 7]],
            &[(10, 1), (3, 20), (1, 7)],
            &[],
            &crate::faces::Rules::embedded().structures,
        );
        let storey = |id: u32| tags.get(&id).and_then(|t| t.get(STOREY)).map(String::as_str);
        assert_eq!([storey(1), storey(3), storey(7)], [Some("2"); 3]);
        assert_eq!(storey(20), Some("3"), "U over the viaduct");
        assert_eq!([storey(10), storey(5)], [None, None], "a deck on storey 1 has no tag");
        assert_eq!(tags[&7].get(LEVELS), None, "a feature that no cell decides has only the storey");
    }

    /// A tunnel feature in two parts along one alignment (0 to 100 m and 130 to 300 m) with another feature between
    /// them, linked at both of its ends: the chain from the tag end at 0 m follows the links at the first part's end,
    /// so the feature between is open up to the room at 140 m in the second part, not hidden.
    #[test]
    fn the_chain_follows_the_links_at_a_parts_end_before_the_room_of_the_next_part() {
        let tunnel = |extents: Vec<(f64, f64)>, room: Vec<(f64, f64)>, tag_ends: Vec<(f64, f64)>| Found {
            tunnel: extents,
            room,
            tag_ends,
            ..Default::default()
        };
        let found = BTreeMap::from([
            (
                1,
                vec![tunnel(vec![(0.0, 100.0), (130.0, 300.0)], vec![(140.0, 300.0)], vec![(0.0, 1.0), (300.0, -1.0)])],
            ),
            (2, vec![tunnel(vec![(0.0, 30.0)], Vec::new(), Vec::new())]),
        ]);
        let links = [(1, 100.0, -1.0, 2, 0.0, 1.0), (2, 30.0, -1.0, 1, 130.0, 1.0)];
        let tags = merge(&found, &[], &[], &links, &crate::faces::Rules::embedded().structures);
        let levels = |id: u32| tags.get(&id).and_then(|t| t.get(LEVELS)).cloned();
        assert_eq!(levels(2).as_deref(), Some("O"), "the feature between the parts");
        // The same walk gives the chain once, from the tag end that comes first, with both ends tag ends.
        let want = Chain { ways: vec![(1, 0.0, 100.0), (2, 0.0, 30.0), (1, 130.0, 300.0)], tag: [true, true] };
        assert_eq!(chains(&found, &links), [want]);
        // A third feature that goes on at the node of the second part's start forks there: three chains stop at it.
        let mut fork = found.clone();
        fork.insert(3, vec![tunnel(vec![(0.0, 20.0)], Vec::new(), vec![(20.0, -1.0)])]);
        let links = [links[0], links[1], (2, 30.0, -1.0, 3, 0.0, 1.0), (1, 130.0, 1.0, 3, 0.0, 1.0)];
        let ends: Vec<(u32, [bool; 2])> = chains(&fork, &links).iter().map(|c| (c.ways[0].0, c.tag)).collect();
        assert_eq!(ends, [(1, [true, false]), (1, [true, false]), (3, [true, false])]);
        // The cell of the second part's start sees no feature go on there, the cell of the feature between links them:
        // the chain still comes once.
        let mut apart = found.clone();
        apart.get_mut(&1).unwrap().push(tunnel(Vec::new(), Vec::new(), vec![(130.0, 1.0)]));
        assert_eq!(chains(&apart, &links[..2]).len(), 1);
    }
}
