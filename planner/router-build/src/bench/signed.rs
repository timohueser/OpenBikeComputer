//! The share of signed route length that a profile follows.
use super::net::{signed, Net};
use crate::catalog::assemble::{self, Member};
use crate::source::Relation;
use crate::Graph;
use planner_router::model::BIKE;
use planner_router::shape::{self, Near, P};
use std::collections::{BTreeSet, HashMap, HashSet};

/// The least main line of a relation in the bounds.
const MIN_LINE_M: f64 = 5_000.0;
/// A piece of a line shorter than this is not measured.
const MIN_PIECE_M: f64 = 1_000.0;
/// The largest gap that `assemble::chain` joins, as in the route catalog.
const MAX_GAP_M: f64 = 500.0;

/// The network levels of the routes of a profile, the highest first. A relation with no such
/// `network` is `other`.
pub fn levels(walking: bool) -> [&'static str; 5] {
    if walking {
        ["iwn", "nwn", "rwn", "lwn", "other"]
    } else {
        ["icn", "ncn", "rcn", "lcn", "other"]
    }
}

/// The signed length of the legs, and what the router follows of it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Share {
    pub relations: usize,
    pub measured_m: f64,
    /// Within `shape::NEAR_M` of the router's line.
    pub near_m: f64,
    /// On a directed road that the router's route also takes.
    pub same_road_m: f64,
}

/// Relations overlap: a stretch in three relations counts three times in the shares and the legs.
#[derive(Debug, Default)]
pub struct Signed {
    pub total: Share,
    /// In the order of `levels`.
    pub levels: [Share; 5],
    pub legs: usize,
    pub legs_with_no_split: usize,
    /// A split runs from the first road that differs to the next road that both lines take. The
    /// same two stretches count once.
    pub splits: usize,
    /// The splits whose signed stretch is not longer than the router's stretch.
    pub not_longer: usize,
}

/// The main line of `relation` inside the bounds as runs of node IDs, and the place of each node
/// in `points`. The runs have the order of `assemble::chain`, or of the members where they do not
/// chain: unlike the route catalog, no fork is pruned and no side trip is placed.
fn line(graph: &Graph, relation: &Relation, points: &mut HashMap<i64, P>) -> Vec<Vec<i64>> {
    let mut members = Vec::new();
    for (member, role) in relation.members().filter(|(_, role)| assemble::is_main(role)) {
        let Some(way) = member.way().and_then(|id| graph.osm.ways.get(id)) else { continue };
        let roundabout = matches!(way.tags.get("junction"), Some("roundabout" | "circular"));
        let nodes: Vec<_> = way.nodes.iter().zip(way.points).collect();
        for inside in nodes.split(|(_, point)| !point.present()).filter(|inside| inside.len() >= 2) {
            points.extend(inside.iter().map(|&(&node, point)| (node, [point.lon, point.lat])));
            let nodes = inside.iter().map(|&(&node, _)| node).collect();
            members.push(Member { nodes, role: role.to_string(), roundabout });
        }
    }
    if members.is_empty() {
        return Vec::new();
    }
    let apart = |a: i64, b: i64| shape::distance(points[&a], points[&b]);
    let mut runs = assemble::main_line(&members, &apart);
    if runs.len() > 1 {
        runs = assemble::chain(&runs, &apart, MAX_GAP_M).unwrap_or(runs);
    }
    let mut joined: Vec<Vec<i64>> = Vec::new();
    for run in runs {
        match joined.last_mut() {
            Some(last) if last.last() == run.first() => last.extend(&run[1..]),
            _ => joined.push(run),
        }
    }
    joined
}

/// The directed road of each pair of consecutive nodes of the ways `rows`. A road covers the
/// nodes of its way between two nodes where a road of that way ends.
fn node_pairs(graph: &Graph, rows: &BTreeSet<u32>) -> HashMap<(i64, i64), u32> {
    let roads = &graph.roads;
    let node = |junction: u32| graph.node_ids.values()[junction as usize];
    let (mut by_ends, mut ends) = (HashMap::new(), HashSet::new());
    for road in (0..roads.len()).filter(|&road| rows.contains(&roads.way_row(road))) {
        let (row, from, to) = (roads.way_row(road), node(roads.from(road)), node(roads.to(road)));
        by_ends.insert((row, from, to, roads.reversed(road)), road as u32);
        ends.extend([(row, from), (row, to)]);
    }
    let mut pairs = HashMap::new();
    for &row in rows {
        let nodes = graph.osm.ways.row(row as usize).nodes;
        let mut start = 0;
        for end in (1..nodes.len()).filter(|&end| ends.contains(&(row, nodes[end]))) {
            for reversed in [false, true] {
                let (from, to) = if reversed { (end, start) } else { (start, end) };
                let Some(&road) = by_ends.get(&(row, nodes[from], nodes[to], reversed)) else { continue };
                for i in start..end {
                    pairs.insert(if reversed { (nodes[i + 1], nodes[i]) } else { (nodes[i], nodes[i + 1]) }, road);
                }
            }
            start = end;
        }
    }
    pairs
}

/// Whether a cycling profile may only push on the road, but may ride the other direction of its
/// way: a signed line there runs against a one-way.
fn against_oneway(net: &Net, road: u32) -> bool {
    let roads = &net.graph.roads;
    let access = net.graph.osm.ways.row(roads.way_row(road as usize) as usize).facts.access;
    let side = usize::from(roads.reversed(road as usize));
    !net.profile.walking && access[side] & BIKE == 0 && access[1 - side] & BIKE != 0
}

/// The pieces of the line `runs` as directed roads. A piece ends where the line has no road that
/// the profile may use, where it runs against a one-way, and where the profile may not turn onto
/// the next road.
fn pieces(net: &Net, runs: &[Vec<i64>], pairs: &HashMap<(i64, i64), u32>) -> Vec<Vec<u32>> {
    let mut pieces = vec![Vec::new()];
    for run in runs {
        for nodes in run.windows(2).filter(|nodes| nodes[0] != nodes[1]) {
            let road = pairs.get(&(nodes[0], nodes[1])).copied();
            let road = road.filter(|&road| net.allowed(road) && !against_oneway(net, road));
            let piece = pieces.last_mut().expect("a piece");
            match (road, piece.last().copied()) {
                (Some(road), Some(last)) if road == last => {}
                (Some(road), Some(last)) if net.to(last) == net.from(road) && net.permits(last, road) => {
                    piece.push(road);
                }
                (road, _) => pieces.push(road.into_iter().collect()),
            }
        }
        pieces.push(Vec::new());
    }
    pieces
}

/// The legs of a piece with the road lengths `lengths`, as road indices: a leg ends at the
/// junction nearest to each multiple of an even step of about `step_m`.
fn legs(lengths: &[f64], step_m: f64) -> Vec<usize> {
    let mut along = vec![0.0];
    for length in lengths {
        along.push(along[along.len() - 1] + length);
    }
    let total = along[along.len() - 1];
    let count = (total / step_m).round().max(1.0) as usize;
    let mut ends = vec![0];
    for leg in 1..count {
        let target = total * leg as f64 / count as f64;
        let after = along.partition_point(|&m| m < target);
        let nearest = if after > 0 && target - along[after - 1] < along[after] - target { after - 1 } else { after };
        if nearest > ends[ends.len() - 1] && nearest < lengths.len() {
            ends.push(nearest);
        }
    }
    ends.push(lengths.len());
    ends
}

/// Routes between points at junctions about every `step_m` on each signed route of the profile
/// with at least `MIN_LINE_M` of main line in the bounds, and compares each leg of the signed
/// line with the router's route.
pub fn signed_share(net: &Net, step_m: f64) -> Result<Signed, String> {
    let graph = net.graph;
    let (mut points, mut lines, mut rows) = (HashMap::new(), Vec::new(), BTreeSet::new());
    for relation in graph.osm.relations.iter().filter(|relation| signed(net.profile, &relation.tags)) {
        let runs = line(graph, &relation, &mut points);
        let steps = runs.iter().flat_map(|run| run.windows(2));
        if steps.map(|w| shape::distance(points[&w[0]], points[&w[1]])).sum::<f64>() < MIN_LINE_M {
            continue;
        }
        rows.extend(relation.members().filter_map(|(member, _)| Some(graph.osm.ways.index(member.way()?)? as u32)));
        lines.push((relation, runs));
    }
    let pairs = node_pairs(graph, &rows);
    let length = |roads: &[u32]| roads.iter().map(|&road| net.length(road)).sum::<f64>();
    let (mut result, mut seen) = (Signed::default(), HashSet::new());
    for (relation, runs) in lines {
        let mut share = Share { relations: 1, ..Share::default() };
        for piece in pieces(net, &runs, &pairs).iter().filter(|piece| length(piece) >= MIN_PIECE_M) {
            let lengths: Vec<f64> = piece.iter().map(|&road| net.length(road)).collect();
            for leg in legs(&lengths, step_m).windows(2) {
                let signed = &piece[leg[0]..leg[1]];
                let (from, to) = (net.from(signed[0]), net.to(signed[signed.len() - 1]));
                // A leg that comes back to its start, or whose ends have no route, is not measured.
                if from == to {
                    continue;
                }
                let Some(router) = net.route(from, to) else { continue };
                let (signed_line, router_line) = (net.line(signed), net.line(&router));
                share.measured_m += length(signed);
                share.near_m += length(signed) - shape::far(&signed_line, &Near::new(&router_line));
                result.legs += 1;
                result.legs_with_no_split += usize::from(signed == router);
                let taken: HashSet<u32> = router.iter().copied().collect();
                let same = signed.iter().filter(|road| taken.contains(road)).map(|&road| net.length(road));
                share.same_road_m += same.sum::<f64>();
                let (mut i, mut j) = (0, 0);
                while i < signed.len() || j < router.len() {
                    if i < signed.len() && j < router.len() && signed[i] == router[j] {
                        (i, j) = (i + 1, j + 1);
                        continue;
                    }
                    let rejoin = (j..router.len())
                        .find_map(|j| Some(((i + 1..signed.len()).find(|&i| signed[i] == router[j])?, j)));
                    let (i2, j2) = rejoin.unwrap_or((signed.len(), router.len()));
                    let (ours, theirs) = (&signed[i..i2], &router[j..j2]);
                    if seen.insert((ours.to_vec(), theirs.to_vec())) {
                        result.splits += 1;
                        result.not_longer += usize::from(!theirs.is_empty() && length(ours) <= length(theirs));
                    }
                    (i, j) = (i2, j2);
                }
            }
        }
        let network = relation.tags.get("network").unwrap_or("");
        let level = levels(net.profile.walking).iter().position(|level| *level == network).unwrap_or(4);
        for sum in [&mut result.total, &mut result.levels[level]] {
            sum.relations += share.relations;
            sum.measured_m += share.measured_m;
            sum.near_m += share.near_m;
            sum.same_road_m += share.same_road_m;
        }
    }
    Ok(result)
}
