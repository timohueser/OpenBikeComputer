//! Shared runs, way pieces, and the points of a set of pieces: junctions with their hosts, joints
//! and ends at every position where pieces meet, and the final assembly of the graph.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use core::f64::consts::PI;

use super::curve::{geo, point};
use super::parts::Rule;
use super::{
    inside, Arm, Crossing, CrossingKind, Deck, End, Family, GraphStats, HostRule, Joint, Junction, Level, LinkKind,
    Network, Part, PointRef, Sample, Source, Station, Way, WayGraph, HEADING_M, ROOM_MAX_M, SIDE_BACK, SIDE_ON,
    STROKE_DEG,
};
use crate::frame::CellKey;
use crate::CellInput;

/// A stretch of one part with one level, before ids.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Piece {
    pub part: usize,
    pub from: Station,
    pub to: Station,
    pub level: Level,
    pub link: Option<LinkKind>,
    pub synthetic: bool,
}

/// A station on a piece that is a point of its own: an implicit junction or a snap foot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Anchor {
    pub pos: [i64; 2],
    pub at: Station,
    pub implicit: bool,
}

/// What the stages hand on: the pieces, the anchors, the crossings without a shared vertex, and the
/// carriageway links (their foot ends are guests).
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub pieces: Vec<Piece>,
    pub anchors: Vec<Anchor>,
    pub crossings: Vec<Crossing>,
}

/// The station of a vertex: (v, 0), or (n − 2, 1) for the last.
pub(crate) fn vertex_station(part: &Part, index: usize, v: usize) -> Station {
    let n = part.spans.len();
    let (span, u) = if v < n { (v, 0.0) } else { (n - 1, 1.0) };
    Station { part: index as u32, span: span as u32, u }
}

/// A station in its one form: the end of a span is the next span's u = 0, but at the last vertex.
pub(crate) fn norm(part: &Part, st: Station) -> Station {
    if st.u >= 1.0 && (st.span as usize) + 1 < part.spans.len() {
        Station { span: st.span + 1, u: 0.0, ..st }
    } else {
        st
    }
}

pub(crate) fn key(st: &Station) -> (u32, f64) {
    (st.span, st.u)
}

pub(crate) fn before(a: &Station, b: &Station) -> bool {
    key(a) < key(b)
}

/// Metres along a piece from its start to a station on it.
pub(crate) fn metres(parts: &[Part], piece: &Piece, st: &Station) -> f64 {
    let part = &parts[piece.part];
    let s = |st: &Station| super::s_at(&part.spans[st.span as usize], st.u);
    let whole: f64 = (piece.from.span..st.span).map(|k| part.spans[k as usize].len_m).sum();
    whole + s(st) - s(&piece.from)
}

/// The station `delta` metres along a piece from a station on it, at most to the piece's ends. It
/// reads only the spans between, so every cell gives one station.
pub(crate) fn advance(parts: &[Part], piece: &Piece, st: &Station, delta: f64) -> Station {
    let part = &parts[piece.part];
    let s_of = |st: &Station| super::s_at(&part.spans[st.span as usize], st.u);
    let (mut span, mut s, mut left) = (st.span, s_of(st), delta.abs());
    loop {
        let len = part.spans[span as usize].len_m;
        if delta >= 0.0 {
            if span == piece.to.span {
                s = (s + left).min(s_of(&piece.to));
                break;
            }
            if s + left <= len {
                s += left;
                break;
            }
            left -= len - s;
            (span, s) = (span + 1, 0.0);
        } else {
            if span == piece.from.span {
                s = (s - left).max(s_of(&piece.from));
                break;
            }
            if s - left >= 0.0 {
                s -= left;
                break;
            }
            left -= s;
            span -= 1;
            s = part.spans[span as usize].len_m;
        }
    }
    norm(part, Station { span, u: super::u_at(&part.spans[span as usize], s), ..*st })
}

/// The metres along a part between two stations on it, summed from the lower one.
pub(crate) fn between(parts: &[Part], a: &Station, b: &Station) -> f64 {
    let (a, b) = if before(b, a) { (b, a) } else { (a, b) };
    let part = &parts[a.part as usize];
    let s = |st: &Station| super::s_at(&part.spans[st.span as usize], st.u);
    if a.span == b.span {
        return s(b) - s(a);
    }
    let mut sum = part.spans[a.span as usize].len_m - s(a);
    for k in a.span + 1..b.span {
        sum += part.spans[k as usize].len_m;
    }
    sum + s(b)
}

/// The global position of a station, rounded as a sample is.
pub(crate) fn position(parts: &[Part], st: &Station) -> [i64; 2] {
    let part = &parts[st.part as usize];
    let span = st.span as usize;
    let g = geo(&part.vertices[span], &part.vertices[span + 1], part.spans[span].chord);
    super::curve::round(&g, g.eval(st.u)[0])
}

/// The global position of a station in frame units, unrounded.
pub(crate) fn exact(parts: &[Part], st: &Station) -> [f64; 2] {
    let part = &parts[st.part as usize];
    let p = point(part, st.span as usize, st.u);
    let p0 = part.vertices[st.span as usize].pos;
    [p0[0] as f64 + p.d[0], p0[1] as f64 + p.d[1]]
}

/// Pieces of every part: its runs of sampled spans that no shared run takes.
pub(crate) fn pieces(parts: &[Part], sampled: &[Vec<bool>], input: &CellInput, stats: &mut GraphStats) -> Plan {
    let lost = shared_runs(parts, sampled, input, stats);
    let mut plan = Plan::default();
    for (index, part) in parts.iter().enumerate() {
        let mut start = None;
        for span in 0..=part.spans.len() {
            let keep = span < part.spans.len() && sampled[index][span] && !lost[index][span];
            match (start, keep) {
                (None, true) => start = Some(span),
                (Some(a), false) => {
                    plan.pieces.push(Piece {
                        part: index,
                        from: vertex_station(part, index, a),
                        to: vertex_station(part, index, span),
                        level: part.level,
                        link: None,
                        synthetic: false,
                    });
                    start = None;
                }
                _ => {}
            }
        }
    }
    plan
}

/// The order of the parts that hold one segment, read from that segment alone: class, kind, tags,
/// then the data of its two vertices, the lower position first, as seen in that direction.
fn segment_order(part: &Part, k: usize, input: &CellInput) -> (u8, String, crate::tile::Tags, Vec<f64>) {
    let (a, b) = (&part.vertices[k], &part.vertices[k + 1]);
    let forward = a.pos <= b.pos;
    let sign = if forward { 1.0 } else { -1.0 };
    let swap = |bits: u8| if forward { bits } else { (bits & 1) << 1 | (bits >> 1) & 1 };
    let turn = |t: f64| if forward { t } else { (t + PI).rem_euclid(core::f64::consts::TAU) };
    let (lo, hi) = if forward { (a, b) } else { (b, a) };
    let mut data = Vec::with_capacity(15);
    for v in [lo, hi] {
        data.extend([turn(v.tangent), v.z_m, sign * v.grade, sign * v.bank, v.widen_m, f64::from(swap(v.side))]);
    }
    data.push(f64::from(swap(a.walk)));
    let f = &input.features[part.feature.0 as usize];
    (part.class, f.kind.clone(), f.tags.clone(), data)
}

/// Where parts of one network hold one segment, every part but the first in `segment_order` loses
/// that span: the decision reads only the segment, so every cell that holds it agrees. A run whose
/// order changes along it hands over at a vertex, a joint.
fn shared_runs(parts: &[Part], sampled: &[Vec<bool>], input: &CellInput, stats: &mut GraphStats) -> Vec<Vec<bool>> {
    let mut lost: Vec<Vec<bool>> = parts.iter().map(|p| vec![false; p.spans.len()]).collect();
    let mut segments: BTreeMap<[[i64; 2]; 2], Vec<(usize, usize)>> = BTreeMap::new();
    for (i, part) in parts.iter().enumerate() {
        for k in (0..part.spans.len()).filter(|&k| sampled[i][k]) {
            let (a, b) = (part.vertices[k].pos, part.vertices[k + 1].pos);
            segments.entry([a.min(b), a.max(b)]).or_default().push((i, k));
        }
    }
    for holders in segments.values().filter(|h| h.len() > 1) {
        let network = |&(i, _): &(usize, usize)| parts[i].family.network();
        for net in [Network::Way, Network::Rail, Network::Water] {
            let mine: Vec<(usize, usize)> = holders.iter().copied().filter(|h| network(h) == net).collect();
            if mine.len() < 2 {
                continue;
            }
            let orders: Vec<_> = mine.iter().map(|&(i, k)| segment_order(&parts[i], k, input)).collect();
            let best = (0..mine.len())
                .min_by(|&x, &y| {
                    let (p, q) = (&orders[x], &orders[y]);
                    (p.0, &p.1, &p.2).cmp(&(q.0, &q.1, &q.2)).then_with(|| {
                        p.3.iter().zip(&q.3).map(|(u, v)| u.total_cmp(v)).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
                    })
                })
                .unwrap();
            for (n, &(i, k)) in mine.iter().enumerate() {
                lost[i][k] |= n != best;
            }
        }
    }
    for spans in &lost {
        stats.shared_runs +=
            spans.windows(2).filter(|w| w[1] && !w[0]).count() as u32 + u32::from(spans.first() == Some(&true));
    }
    lost
}

/// An arm before ids: a piece, a station, the metres along the piece, a direction.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawArm {
    pub piece: usize,
    pub at: Station,
    pub m: f64,
    pub dir: i8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NodeKind {
    Junction,
    Joint,
    End,
}

/// A point of one group at one position.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub pos: [i64; 2],
    pub network: Network,
    pub kind: NodeKind,
    pub arms: Vec<RawArm>,
    pub implicit: bool,
}

/// A shared vertex of two networks with two or more arms each: the position, and each network with its arms.
pub(crate) type Shared = ([i64; 2], (Network, Vec<RawArm>), (Network, Vec<RawArm>));

/// The points of the pieces in the area, and the crossings at shared vertices.
pub(crate) struct Nodes {
    pub nodes: Vec<Node>,
    pub shared: Vec<Shared>,
    /// The pieces at each position where two or more occurrences meet.
    pub meets: BTreeMap<[i64; 2], Vec<usize>>,
}

pub(crate) fn nodes(parts: &[Part], plan: &Plan, area: [i64; 4], stats: Option<&mut GraphStats>) -> Nodes {
    // (position, piece, station, an arm back, an arm on, implicit) of each occurrence.
    let mut at: Vec<([i64; 2], usize, Station, bool, bool, bool)> = Vec::new();
    for (index, piece) in plan.pieces.iter().enumerate() {
        let part = &parts[piece.part];
        let mut add = |pos: [i64; 2], st: Station, implicit: bool| {
            at.push((pos, index, st, before(&piece.from, &st), before(&st, &piece.to), implicit));
        };
        add(position(parts, &piece.from), piece.from, false);
        for v in piece.from.span as usize + 1..=piece.to.span as usize {
            let st = vertex_station(part, piece.part, v);
            if before(&st, &piece.to) {
                add(part.vertices[v].pos, st, false);
            }
        }
        add(position(parts, &piece.to), piece.to, false);
        for a in plan.anchors.iter().filter(|a| a.at.part as usize == piece.part) {
            if before(&piece.from, &a.at) && before(&a.at, &piece.to) {
                add(a.pos, a.at, a.implicit);
            }
        }
    }
    // An anchor at a vertex is that vertex's occurrence.
    at.sort_by(|a, b| (a.0, a.1, key(&a.2)).partial_cmp(&(b.0, b.1, key(&b.2))).unwrap());
    at.dedup_by(|b, a| {
        let same = (a.0, a.1, key(&a.2)) == (b.0, b.1, key(&b.2));
        a.5 |= same && b.5;
        same
    });
    let mut out = Nodes { nodes: Vec::new(), shared: Vec::new(), meets: BTreeMap::new() };
    let mut end_on_other = 0;
    for occurrences in at.chunk_by(|a, b| a.0 == b.0) {
        let pos = occurrences[0].0;
        if occurrences.len() < 2 && occurrences.iter().all(|o| o.3 && o.4) {
            continue;
        }
        out.meets.insert(pos, occurrences.iter().map(|o| o.1).collect());
        if !inside(area, pos) {
            continue;
        }
        let implicit = occurrences.iter().any(|o| o.5);
        let mut groups: BTreeMap<Network, (Vec<RawArm>, usize)> = BTreeMap::new();
        for &(_, piece, st, back, on, _) in occurrences {
            let entry = groups.entry(parts[plan.pieces[piece].part].family.network()).or_default();
            entry.1 += 1;
            let m = metres(parts, &plan.pieces[piece], &st);
            let dirs = [(back, -1), (on, 1)];
            entry.0.extend(dirs.iter().filter(|d| d.0).map(|&(_, dir)| RawArm { piece, at: st, m, dir }));
        }
        let wide: Vec<Network> = groups.iter().filter(|(_, (arms, _))| arms.len() >= 2).map(|(&g, _)| g).collect();
        for (i, &g) in wide.iter().enumerate() {
            for &h in &wide[i + 1..] {
                out.shared.push((pos, (g, groups[&g].0.clone()), (h, groups[&h].0.clone())));
            }
        }
        for (group, (arms, count)) in groups {
            let kind = match arms.len() {
                0 => continue,
                1 => {
                    end_on_other += usize::from(!wide.is_empty());
                    NodeKind::End
                }
                2 if count == 1 => continue,
                2 => NodeKind::Joint,
                _ => NodeKind::Junction,
            };
            out.nodes.push(Node { pos, network: group, kind, arms, implicit });
        }
    }
    if let Some(stats) = stats {
        stats.end_on_other += end_on_other as u32;
    }
    out
}

/// The heading of an arm: the unit vector from the point to the curve `HEADING_M` along its piece,
/// and that curve point.
fn heading(parts: &[Part], piece: &Piece, arm: &RawArm, pos: [i64; 2]) -> ([f64; 2], [i64; 2]) {
    let st = advance(parts, piece, &arm.at, f64::from(arm.dir) * HEADING_M);
    let p = exact(parts, &st);
    let v = [p[0] - pos[0] as f64, p[1] - pos[1] as f64];
    let length = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if length > 0.0 {
        return ([v[0] / length, v[1] / length], position(parts, &st));
    }
    let t = point(&parts[arm.at.part as usize], arm.at.span as usize, arm.at.u).tangent;
    let dir = f64::from(arm.dir);
    ([dir * t[0], dir * t[1]], position(parts, &st))
}

/// The `side` bit of an arm at a vertex.
fn side_bit(parts: &[Part], arm: &RawArm) -> bool {
    let part = &parts[arm.at.part as usize];
    let v = if arm.at.u == 0.0 {
        arm.at.span as usize
    } else if arm.at.u == 1.0 {
        arm.at.span as usize + 1
    } else {
        return false;
    };
    part.vertices[v].side & if arm.dir < 0 { SIDE_BACK } else { SIDE_ON } != 0
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// The angle between two unit vectors.
fn angle_of(a: [f64; 2], b: [f64; 2]) -> f64 {
    libm::atan2((a[0] * b[1] - a[1] * b[0]).abs(), dot(a, b))
}

/// The hosts of a junction's arms, which are sorted. A link's foot is a guest and is not a candidate.
fn hosts(parts: &[Part], plan: &Plan, node: &Node, headings: &[[f64; 2]]) -> (Vec<bool>, HostRule) {
    let n = node.arms.len();
    let piece = |a: &RawArm| &plan.pieces[a.piece];
    let family = |a: &RawArm| parts[piece(a).part].family;
    let foot = |a: &RawArm| piece(a).link == Some(LinkKind::Carriageway) && a.dir < 0;
    let mut host = vec![false; n];
    let open: Vec<usize> = (0..n).filter(|&i| !foot(&node.arms[i])).collect();
    let roads: Vec<usize> = open.iter().copied().filter(|&i| family(&node.arms[i]) != Family::Path).collect();
    if !node.implicit && node.network != Network::Water {
        let unmarked: Vec<usize> = roads.iter().copied().filter(|&i| !side_bit(parts, &node.arms[i])).collect();
        if unmarked.len() == 2 {
            unmarked.iter().for_each(|&i| host[i] = true);
            return (host, HostRule::Side);
        }
    }
    let candidates = if roads.is_empty() { open } else { roads };
    if candidates.len() == 1 {
        host[candidates[0]] = true;
        return (host, HostRule::Fallback);
    }
    // The pair whose worse class is best, then the most nearly straight, then the better class: a
    // stub of a better class does not break a through pair.
    let rank = |i: usize| parts[piece(&node.arms[i]).part].class;
    let mut best: Option<((u8, f64, u8), usize, usize)> = None;
    for (x, &i) in candidates.iter().enumerate() {
        for &j in &candidates[x + 1..] {
            let k = (rank(i).max(rank(j)), dot(headings[i], headings[j]), rank(i).min(rank(j)));
            if best.as_ref().is_none_or(|(b, ..)| k.partial_cmp(b) == Some(Ordering::Less)) {
                best = Some((k, i, j));
            }
        }
    }
    if let Some((_, i, j)) = best {
        host[i] = true;
        host[j] = true;
    }
    (host, HostRule::Fallback)
}

/// The graph from the final pieces: ways, samples, points, crossings, decks and stats.
pub(crate) fn assemble(
    cell: CellKey,
    area: [i64; 4],
    parts: Vec<Part>,
    rules: &[Rule<'_>],
    samples: Vec<Vec<Sample>>,
    mut plan: Plan,
    mut stats: GraphStats,
) -> WayGraph {
    plan.pieces.sort_by(|a, b| (a.part, key(&a.from)).partial_cmp(&(b.part, key(&b.from))).unwrap());
    let mut out = WayGraph {
        cell,
        parts: Vec::new(),
        ways: Vec::new(),
        samples: Vec::new(),
        junctions: Vec::new(),
        joints: Vec::new(),
        ends: Vec::new(),
        crossings: Vec::new(),
        decks: Vec::new(),
        stats: GraphStats::default(),
    };
    for (id, piece) in plan.pieces.iter().enumerate() {
        let part = &parts[piece.part];
        let start = out.samples.len() as u32;
        let within = |s: &&Sample| {
            let k = (s.span, s.u);
            k >= key(&piece.from) && k <= key(&piece.to)
        };
        out.samples.extend(samples[piece.part].iter().filter(within).cloned());
        out.ways.push(Way {
            id: id as u32,
            part: piece.part as u32,
            from: piece.from,
            to: piece.to,
            family: part.family,
            class: part.class,
            level: piece.level,
            deck: None,
            link: piece.link,
            samples: start..out.samples.len() as u32,
        });
    }
    stats.samples = out.samples.len() as u32;
    let found = nodes(&parts, &plan, area, Some(&mut stats));
    let o = cell.origin();
    let local = |p: [i64; 2]| [(p[0] - o.x) as i32, (p[1] - o.y) as i32];
    let owned = |p: [i64; 2]| cell.contains(crate::frame::GlobalPoint { x: p[0], y: p[1] });
    // The points on each piece by metres, for `room_m` and `next`.
    let mut refs: Vec<PointRef> = Vec::new();
    let mut counts = [0u32; 3];
    for node in &found.nodes {
        let i = node.kind as usize;
        refs.push(match node.kind {
            NodeKind::Junction => PointRef::Junction(counts[i]),
            NodeKind::Joint => PointRef::Joint(counts[i]),
            NodeKind::End => PointRef::End(counts[i]),
        });
        counts[i] += 1;
    }
    let mut on_piece: Vec<Vec<(f64, PointRef, Station)>> = vec![Vec::new(); plan.pieces.len()];
    for (node, &r) in found.nodes.iter().zip(&refs) {
        for arm in &node.arms {
            if !on_piece[arm.piece].iter().any(|&(m, q, _)| q == r && m == arm.m) {
                on_piece[arm.piece].push((arm.m, r, arm.at));
            }
        }
    }
    for list in &mut on_piece {
        list.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    }
    for (node, &r) in found.nodes.iter().zip(&refs) {
        let mut arms: Vec<(Arm, [i64; 2])> = node
            .arms
            .iter()
            .map(|raw| {
                let piece = &plan.pieces[raw.piece];
                let (h, point) = heading(&parts, piece, raw, node.pos);
                let ahead = on_piece[raw.piece].iter().filter(|&&(m, q, _)| q != r || m != raw.m);
                let next = if raw.dir > 0 {
                    ahead.filter(|(m, ..)| *m > raw.m).min_by(|a, b| a.0.total_cmp(&b.0))
                } else {
                    ahead.filter(|(m, ..)| *m < raw.m).max_by(|a, b| a.0.total_cmp(&b.0))
                };
                let end = if raw.dir > 0 { piece.to } else { piece.from };
                let room = between(&parts, &raw.at, &next.map_or(end, |n| n.2)).min(ROOM_MAX_M);
                let arm = Arm {
                    way: raw.piece as u32,
                    at: raw.at,
                    dir: raw.dir,
                    heading: h,
                    angle: libm::atan2(h[1], h[0]),
                    host: false,
                    level: piece.level,
                    room_m: room,
                    next: next.map(|&(_, q, _)| q),
                };
                (arm, point)
            })
            .collect();
        let mut order: Vec<usize> = (0..arms.len()).collect();
        order.sort_by(|&a, &b| {
            let (x, y) = (&arms[a], &arms[b]);
            x.0.angle.total_cmp(&y.0.angle).then(x.1.cmp(&y.1)).then(x.0.dir.cmp(&y.0.dir))
        });
        let raws: Vec<RawArm> = order.iter().map(|&i| node.arms[i].clone()).collect();
        arms = order.into_iter().map(|i| arms[i].clone()).collect();
        let sorted = Node { arms: raws, ..node.clone() };
        let pos = local(node.pos);
        match node.kind {
            NodeKind::Junction => {
                let headings: Vec<[f64; 2]> = arms.iter().map(|a| a.0.heading).collect();
                let (host, host_rule) = hosts(&parts, &plan, &sorted, &headings);
                let arms = arms.into_iter().zip(host).map(|((arm, _), host)| Arm { host, ..arm }).collect();
                stats.implicit_junctions += u32::from(node.implicit);
                out.junctions.push(Junction {
                    pos,
                    network: node.network,
                    arms,
                    host_rule,
                    implicit: node.implicit,
                    owned: owned(node.pos),
                });
            }
            NodeKind::Joint => {
                let [a, b]: [Arm; 2] = [arms[0].0.clone(), arms[1].0.clone()];
                let tangent = |arm: &Arm| {
                    let t = point(&parts[arm.at.part as usize], arm.at.span as usize, arm.at.u).tangent;
                    [f64::from(arm.dir) * t[0], f64::from(arm.dir) * t[1]]
                };
                let (ta, tb) = (tangent(&a), tangent(&b));
                let stroke = angle_of([-ta[0], -ta[1]], tb).to_degrees() < STROKE_DEG;
                let turn_rad = PI - angle_of(a.heading, b.heading);
                out.joints.push(Joint {
                    pos,
                    network: node.network,
                    arms: [a, b],
                    turn_rad,
                    stroke,
                    owned: owned(node.pos),
                });
            }
            NodeKind::End => {
                let arm = arms.into_iter().next().unwrap().0;
                out.ends.push(End { pos, network: node.network, arm, owned: owned(node.pos) });
            }
        }
    }
    stats.junctions = out.junctions.len() as u32;
    stats.joints = out.joints.len() as u32;
    stats.ends = out.ends.len() as u32;
    // Crossings at shared vertices, then those of the stages after. Each network's arm is the first by
    // (rank, heading point, direction), which reads only the vertex's neighbourhood.
    for (pos, (ga, arms_a), (gb, arms_b)) in &found.shared {
        let pick = |arms: &[RawArm]| {
            arms.iter()
                .min_by_key(|a| {
                    let piece = &plan.pieces[a.piece];
                    (parts[piece.part].class, heading(&parts, piece, a, *pos).1, a.dir)
                })
                .unwrap()
                .clone()
        };
        let (a, b) = (pick(arms_a), pick(arms_b));
        let tunnel = arms_b.iter().any(|x| plan.pieces[x.piece].level == Level::Tunnel);
        let (kind, source) = match (ga, gb) {
            (Network::Way, Network::Rail) => (CrossingKind::LevelCrossing, Source::SharedVertex),
            (Network::Way, Network::Water) if tunnel => (CrossingKind::Culvert, Source::SharedVertex),
            (Network::Way, Network::Water) => (CrossingKind::Ford, Source::SharedVertex),
            _ => {
                let water = rules[plan.pieces[b.piece].part].water();
                if water.width_m <= water.culvert_width_m {
                    (CrossingKind::Culvert, Source::Rule)
                } else {
                    (CrossingKind::Stacked, Source::Fallback)
                }
            }
        };
        let ta = point(&parts[a.at.part as usize], a.at.span as usize, a.at.u).tangent;
        let tb = point(&parts[b.at.part as usize], b.at.span as usize, b.at.u).tangent;
        let angle = libm::atan2((ta[0] * tb[1] - ta[1] * tb[0]).abs(), dot(ta, tb).abs());
        plan.crossings.push(Crossing {
            pos: local(*pos),
            over: a.at,
            under: b.at,
            kind,
            source,
            angle_rad: angle,
            deck: None,
            owned: false,
        });
    }
    // Decks: deck pieces that share a point, and each synthetic stretch.
    let mut root: Vec<usize> = (0..plan.pieces.len()).collect();
    fn find(root: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    for node in &found.nodes {
        let decks: Vec<usize> = (node.arms.iter())
            .map(|a| a.piece)
            .filter(|&p| plan.pieces[p].level == Level::Deck && !plan.pieces[p].synthetic)
            .collect();
        for w in decks.windows(2) {
            let (a, b) = (find(&mut root, w[0]), find(&mut root, w[1]));
            root[a.max(b)] = a.min(b);
        }
    }
    let mut groups: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    for (i, piece) in plan.pieces.iter().enumerate().filter(|(_, p)| p.level == Level::Deck) {
        let r = if piece.synthetic { i } else { find(&mut root, i) };
        groups.entry(r).or_default().push(i as u32);
    }
    let lowest = |ways: &[u32]| {
        ways.iter()
            .flat_map(|&w| {
                let r = &out.ways[w as usize].samples;
                out.samples[r.start as usize..r.end as usize].iter().map(|s| s.pos)
            })
            .min()
    };
    let mut decks: Vec<Deck> =
        groups.into_values().map(|ways| Deck { synthetic: plan.pieces[ways[0] as usize].synthetic, ways }).collect();
    decks.sort_by_key(|d| lowest(&d.ways));
    for (id, deck) in decks.iter().enumerate() {
        for &w in &deck.ways {
            out.ways[w as usize].deck = Some(id as u32);
        }
    }
    out.decks = decks;
    let deck_at = |st: &Station| {
        out.ways
            .iter()
            .find(|w| w.part == st.part && w.level == Level::Deck && key(&w.from) <= key(st) && key(st) <= key(&w.to))
            .and_then(|w| w.deck)
    };
    for c in &mut plan.crossings {
        c.owned = owned([o.x + i64::from(c.pos[0]), o.y + i64::from(c.pos[1])]);
        if matches!(c.kind, CrossingKind::Deck | CrossingKind::Footbridge) {
            c.deck = deck_at(&c.over);
        }
        *stats.crossings.entry((c.kind, c.source)).or_default() += 1;
    }
    plan.crossings.sort_by_key(|c| (c.pos, c.kind, c.source));
    out.crossings = plan.crossings;
    out.parts = parts;
    out.stats = stats;
    out
}
