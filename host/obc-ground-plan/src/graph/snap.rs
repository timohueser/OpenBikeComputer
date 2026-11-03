//! Stage 5: snapped ends. Two ends close to each other join by a link; an end close to a carriageway
//! edge at its height joins that way at its foot, by a link. A link that crosses another way goes.

use std::collections::{BTreeMap, BTreeSet};

use super::crossings::{cross, Grid};
use super::curve::{geo, insert, point, samples as span_samples, station_sample};
use super::parts::Rule;
use super::points::{metres, nodes, position, Anchor, NodeKind, Nodes, Piece, Plan};
use super::{
    Family, GraphStats, Level, LinkKind, Network, Part, Sample, Span, Station, Vertex, NEWTON_STEPS, SNAP_REACH_M,
};
use crate::frame::{metres_per_unit, CellKey};

/// The order of snap targets: the nearest edge, the better rank, the lower foot.
type Order = (f64, u8, [i64; 2]);

/// An end that may snap.
#[derive(Clone, Debug)]
struct End<'a> {
    pos: [i64; 2],
    piece: usize,
    st: Station,
    z: f64,
    rank: u8,
    level: Level,
    rule: Rule<'a>,
}

fn distance(a: [i64; 2], b: [i64; 2]) -> f64 {
    let d = [(b[0] - a[0]) as f64, (b[1] - a[1]) as f64];
    (d[0] * d[0] + d[1] * d[1]).sqrt()
}

/// The pieces that an end reaches along the graph within `budget` metres.
fn reached(nodes: &Nodes, on_piece: &[Vec<(f64, usize)>], piece: usize, m: f64, budget: f64) -> BTreeSet<usize> {
    let mut out = BTreeSet::from([piece]);
    let mut best: BTreeMap<usize, f64> = BTreeMap::new();
    let mut queue = vec![(piece, m, budget)];
    while let Some((piece, m, left)) = queue.pop() {
        for &(m2, node) in &on_piece[piece] {
            let rest = left - (m2 - m).abs();
            if rest < 0.0 || best.get(&node).is_some_and(|&b| b >= rest) {
                continue;
            }
            best.insert(node, rest);
            for arm in &nodes.nodes[node].arms {
                out.insert(arm.piece);
                queue.push((arm.piece, arm.m, rest));
            }
        }
    }
    out
}

/// The u on a span nearest to a point, from a first u: Newton steps on the squared distance.
fn foot(part: &Part, span: usize, mut u: f64, e: [i64; 2]) -> f64 {
    let g = geo(&part.vertices[span], &part.vertices[span + 1], part.spans[span].chord);
    let rel = [(e[0] - g.p0[0]) as f64, (e[1] - g.p0[1]) as f64];
    for _ in 0..NEWTON_STEPS {
        let [p, d, dd] = g.eval(u);
        let v = [p[0] - rel[0], p[1] - rel[1]];
        let f = v[0] * d[0] + v[1] * d[1];
        let df = d[0] * d[0] + d[1] * d[1] + v[0] * dd[0] + v[1] * dd[1];
        if df <= 0.0 {
            break;
        }
        u = (u - f / df).clamp(0.0, 1.0);
    }
    u
}

/// Whether a link from a to b crosses a sample segment of a piece other than `skip`.
fn crossed(grid: &Grid, a: [i64; 2], b: [i64; 2], skip: &[usize]) -> bool {
    grid.near(a, b, 0).iter().any(|&id| {
        let s = &grid.segs[id as usize];
        !skip.contains(&s.piece) && cross(a, b, s.a, s.b).is_some_and(|(t, _)| t > 0.0 && t < 1.0)
    })
}

/// A link part of one chord span from an end to a point, with the end's class and level.
#[allow(clippy::too_many_arguments)]
fn link<'a>(
    cell: CellKey,
    area: [i64; 4],
    parts: &mut Vec<Part>,
    rules: &mut Vec<Rule<'a>>,
    samples: &mut Vec<Vec<Sample>>,
    plan: &mut Plan,
    from: (&End<'a>, [i64; 2], f64),
    to: ([i64; 2], f64),
    kind: LinkKind,
) {
    let (e, a, za) = from;
    let (b, zb) = to;
    let source = &parts[e.st.part as usize];
    let along = station_sample(source, e.st.span as usize, e.st.u, cell).along_m;
    let tangent = libm::atan2((b[1] - a[1]) as f64, (b[0] - a[0]) as f64);
    let vertex =
        |pos: [i64; 2], z_m: f64| Vertex { pos, z_m, tangent, grade: 0.0, bank: 0.0, widen_m: 0.0, walk: 0, side: 0 };
    let (mut va, mut vb) = (vertex(a, za), vertex(b, zb));
    let span = Span::new(&va, &vb, true, along);
    let grade = (zb - za) / span.len_m;
    (va.grade, vb.grade) = (grade, grade);
    let part = Part {
        feature: source.feature,
        family: source.family,
        class: source.class,
        level: e.level,
        backward: false,
        vertices: vec![va, vb],
        spans: vec![span],
    };
    let index = parts.len();
    samples.push(span_samples(&part, &e.rule, cell, area).0);
    plan.pieces.push(Piece {
        part: index,
        from: Station { part: index as u32, span: 0, u: 0.0 },
        to: Station { part: index as u32, span: 0, u: 1.0 },
        level: e.level,
        link: Some(kind),
        synthetic: false,
    });
    parts.push(part);
    rules.push(e.rule);
}

pub(crate) fn snap<'a>(
    cell: CellKey,
    area: [i64; 4],
    parts: &mut Vec<Part>,
    rules: &mut Vec<Rule<'a>>,
    samples: &mut Vec<Vec<Sample>>,
    mut plan: Plan,
    stats: &mut GraphStats,
) -> Plan {
    let prelim = nodes(parts, &plan, area, None);
    let grid = Grid::new(cell, parts, samples, &plan.pieces);
    let mut on_piece: Vec<Vec<(f64, usize)>> = vec![Vec::new(); plan.pieces.len()];
    for (n, node) in prelim.nodes.iter().enumerate() {
        for arm in &node.arms {
            on_piece[arm.piece].push((arm.m, n));
        }
    }
    let ends: Vec<End> = (prelim.nodes.iter())
        .filter(|n| n.kind == NodeKind::End && n.network == Network::Way)
        .filter_map(|n| {
            let arm = &n.arms[0];
            let piece = &plan.pieces[arm.piece];
            let part = &parts[piece.part];
            (piece.level != Level::Tunnel && matches!(part.family, Family::Road | Family::Path)).then(|| End {
                pos: n.pos,
                piece: arm.piece,
                st: arm.at,
                z: point(part, arm.at.span as usize, arm.at.u).z_m,
                rank: part.class,
                level: piece.level,
                rule: rules[piece.part],
            })
        })
        .collect();
    let mpu = |p: [i64; 2]| metres_per_unit(p[1]);
    // End to end: each the other's nearest candidate.
    let nearest = |i: usize| {
        let e = &ends[i];
        (0..ends.len())
            .filter(|&j| j != i && ends[j].level == e.level)
            .map(|j| (distance(e.pos, ends[j].pos) * mpu(e.pos), ends[j].pos, j))
            .filter(|&(d, _, j)| {
                d <= e.rule.way().snap_m.min(ends[j].rule.way().snap_m)
                    && (e.z - ends[j].z).abs() <= e.rule.way().snap_height_m.min(ends[j].rule.way().snap_height_m)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(.., j)| j)
    };
    let mut done = vec![false; ends.len()];
    for i in 0..ends.len() {
        let Some(j) = nearest(i).filter(|&j| j > i && nearest(j) == Some(i)) else { continue };
        done[i] = true;
        done[j] = true;
        let (a, b) = if (ends[i].rank, ends[i].pos) <= (ends[j].rank, ends[j].pos) { (i, j) } else { (j, i) };
        if crossed(&grid, ends[a].pos, ends[b].pos, &[ends[a].piece, ends[b].piece]) {
            stats.link_crossed += 1;
            continue;
        }
        let (ea, eb) = (&ends[a], &ends[b]);
        link(cell, area, parts, rules, samples, &mut plan, (ea, ea.pos, ea.z), (eb.pos, eb.z), LinkKind::EndToEnd);
        stats.links_end += 1;
    }
    // To a carriageway: the nearest edge, then the better rank, then the lower foot.
    let pieces = plan.pieces.clone();
    for (_, e) in ends.iter().enumerate().filter(|(i, _)| !done[*i]) {
        let scale = mpu(e.pos);
        let reach = (SNAP_REACH_M / scale).ceil() as i64;
        let near = reached(&prelim, &on_piece, e.piece, metres(parts, &pieces[e.piece], &e.st), SNAP_REACH_M);
        // The nearest sample segment of each target piece.
        let mut by_piece: BTreeMap<usize, (f64, Station)> = BTreeMap::new();
        for id in grid.near(e.pos, e.pos, reach) {
            let s = &grid.segs[id as usize];
            let t = &pieces[s.piece];
            let tp = &parts[t.part];
            let level = t.level == e.level || (e.level == Level::Deck && t.level == Level::Ground);
            if near.contains(&s.piece)
                || !level
                || tp.class > e.rank
                || !matches!(tp.family, Family::Road | Family::Path)
            {
                continue;
            }
            let r = [(s.b[0] - s.a[0]) as f64, (s.b[1] - s.a[1]) as f64];
            let q = [(e.pos[0] - s.a[0]) as f64, (e.pos[1] - s.a[1]) as f64];
            let length2 = r[0] * r[0] + r[1] * r[1];
            let f = if length2 > 0.0 { ((q[0] * r[0] + q[1] * r[1]) / length2).clamp(0.0, 1.0) } else { 0.0 };
            let x = [q[0] - f * r[0], q[1] - f * r[1]];
            let d = (x[0] * x[0] + x[1] * x[1]).sqrt();
            if by_piece.get(&s.piece).is_none_or(|&(best, _)| d < best) {
                by_piece.insert(s.piece, (d, s.at(f)));
            }
        }
        // ((edge, rank, foot), target, station, height).
        let mut best: Option<(Order, usize, Station, f64)> = None;
        for (&target, &(_, st)) in &by_piece {
            let tp = &parts[pieces[target].part];
            let u = foot(tp, st.span as usize, st.u, e.pos);
            let st = Station { u, ..st };
            let p = point(tp, st.span as usize, u);
            let p0 = tp.vertices[st.span as usize].pos;
            let v = [(e.pos[0] - p0[0]) as f64 - p.d[0], (e.pos[1] - p0[1]) as f64 - p.d[1]];
            let dist = (v[0] * v[0] + v[1] * v[1]).sqrt() * scale;
            let half = rules[pieces[target].part].half_m();
            let edge = dist - (half + p.widen_m);
            if dist > SNAP_REACH_M {
                stats.snap_far += u32::from(edge <= e.rule.way().snap_m);
                continue;
            }
            let offset = ((v[0] * p.tangent[1] - v[1] * p.tangent[0]) * scale).clamp(-half, half);
            if edge > e.rule.way().snap_m || (e.z - (p.z_m + p.bank * offset)).abs() > e.rule.way().snap_height_m {
                continue;
            }
            let k = (edge, tp.class, position(parts, &st));
            if best.as_ref().is_none_or(|(b, ..)| k.partial_cmp(b) == Some(std::cmp::Ordering::Less)) {
                best = Some((k, target, st, p.z_m));
            }
        }
        let Some((_, target, st, z)) = best else { continue };
        let t = &pieces[target];
        let m = metres(parts, t, &st);
        // A point of the target within `snap_m` along it takes the link.
        let close = (on_piece[target].iter()).filter(|(m2, _)| (m2 - m).abs() <= e.rule.way().snap_m).min_by(|a, b| {
            (a.0 - m).abs().total_cmp(&(b.0 - m).abs()).then(prelim.nodes[a.1].pos.cmp(&prelim.nodes[b.1].pos))
        });
        let (to, z) = match close {
            Some(&(_, n)) => {
                let at = prelim.nodes[n].arms.iter().find(|a| a.piece == target).map_or(st, |a| a.at);
                (prelim.nodes[n].pos, point(&parts[at.part as usize], at.span as usize, at.u).z_m)
            }
            None => (position(parts, &st), z),
        };
        if crossed(&grid, e.pos, to, &[e.piece, target]) {
            stats.link_crossed += 1;
            continue;
        }
        let (to, z) = if close.is_none() {
            let part = t.part;
            let taken = insert(&parts[part], &mut samples[part], st, cell);
            let pos = position(parts, &taken);
            plan.anchors.push(Anchor { pos, at: taken, implicit: false });
            (pos, point(&parts[part], taken.span as usize, taken.u).z_m)
        } else {
            (to, z)
        };
        link(cell, area, parts, rules, samples, &mut plan, (e, e.pos, e.z), (to, z), LinkKind::Carriageway);
        stats.links_carriageway += 1;
    }
    plan
}
