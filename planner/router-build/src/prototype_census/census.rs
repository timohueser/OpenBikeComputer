//! PROTOTYPE. The census: each signed cycle route as chains of directed roads, a search between
//! points about every `step_m` on each chain, and each split of the two road lists.
use super::terms::{self, Terms, NODE};
use crate::base::group;
use crate::catalog::assemble::{self, Member};
use crate::column::Column;
use crate::source::Id;
use crate::{osm, Graph};
use planner_router::base::{Costs, Graph as SearchGraph, Numbers};
use planner_router::model::{Point, Profile, BIKE};
use planner_router::search::{Query, Seed, Workspace};
use planner_router::shape::{self, Near, P};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// A chain shorter than this is not measured.
const MIN_CHAIN_M: f64 = 1_000.0;
/// The longest hole of a relation that `assemble::chain` joins, as in the route catalog.
const MAX_GAP_M: f64 = 500.0;
/// The length before and after a junction that gives the heading there.
const HEADING_M: f64 = 15.0;
const TURN_DEGREES: f64 = 45.0;

pub const LEVELS: [&str; 6] = ["", "other", "lcn", "rcn", "ncn", "icn"];

/// One side of a split, or of a leg.
#[derive(Clone, Default)]
pub struct Stretch {
    pub roads: Vec<u32>,
    pub length_m: f64,
    /// The sum of `terms`: the roads and the transitions before, between and after them.
    pub cost: f64,
    pub terms: Terms,
    /// The cost with `RoadTerms::full_bonus` and with `RoadTerms::unknown_paved` of each road.
    pub full_bonus: f64,
    pub unknown_paved: f64,
    pub turns: u32,
    pub signals: u32,
    /// Metres by `highway|surface|bicycle|tracktype|maxspeed`.
    pub signature: BTreeMap<String, f64>,
    pub ways: Vec<i64>,
}

pub struct Split {
    pub relation: i64,
    pub level: u8,
    /// The relations and legs that gave this same split.
    pub seen: usize,
    pub from: Point,
    pub to: Point,
    pub signed: Stretch,
    pub router: Stretch,
    /// The length of the signed stretch farther than 30 m from the router's line of the leg, and
    /// of the router's stretch from the signed line of the leg.
    pub signed_far_m: f64,
    pub router_far_m: f64,
    /// The split starts or ends at a point of the census, so the points may have cut it.
    pub at_point: bool,
}

#[derive(Default, Clone)]
pub struct Tally {
    pub relations: usize,
    pub line_m: f64,
    /// Line that has no directed road which `touring` may use, or whose chain is too short.
    pub lost_m: f64,
    /// Line that runs against a one-way for bicycles, where `touring` would push.
    pub oneway_m: f64,
    pub short_m: f64,
    pub measured_m: f64,
    pub near_m: f64,
    pub same_road_m: f64,
    pub legs: usize,
    pub identical_legs: usize,
    pub splits: usize,
    pub split_m: f64,
    pub signed_cost: f64,
    pub router_cost: f64,
}

impl Tally {
    fn add(&mut self, other: &Tally) {
        self.relations += other.relations;
        self.line_m += other.line_m;
        self.lost_m += other.lost_m;
        self.oneway_m += other.oneway_m;
        self.short_m += other.short_m;
        self.measured_m += other.measured_m;
        self.near_m += other.near_m;
        self.same_road_m += other.same_road_m;
        self.legs += other.legs;
        self.identical_legs += other.identical_legs;
        self.splits += other.splits;
        self.split_m += other.split_m;
        self.signed_cost += other.signed_cost;
        self.router_cost += other.router_cost;
    }
}

pub struct Route {
    pub id: i64,
    pub name: String,
    pub network: String,
    pub level: u8,
    pub tally: Tally,
}

#[derive(Default)]
pub struct Census {
    pub routes: Vec<Route>,
    pub levels: [Tally; 6],
    pub total: Tally,
    /// Each split once.
    pub splits: Vec<Split>,
    /// The signed metres of each tag signature in the measured legs.
    pub measured_signature: BTreeMap<String, f64>,
    /// The measured signed roads, and those in a split, each once.
    pub unique_measured_m: f64,
    pub unique_split_m: f64,
    /// By level, the relations with less main line than one step: their count, their line
    /// length, and those with a relation as a member.
    pub excluded: [(usize, f64, usize); 6],
    pub relations_seen: usize,
    pub relations_under_5km: usize,
    pub queries: usize,
    pub query_seconds: f64,
    pub optimality_violations: usize,
    pub pair_conflicts: usize,
    pub unmatched_runs: usize,
}

struct Net<'a> {
    graph: &'a Graph,
    profile: &'a Profile,
    first: &'a [u32],
    head: &'a [u32],
    totals: &'a [u32],
    turns: &'a [u32],
    arrivals: (Column<u32>, Column<u32>),
    departures: (Column<u32>, Column<u32>),
    search: SearchGraph,
    costs: Costs,
    workspace: Workspace,
    road_terms: HashMap<u32, terms::RoadTerms>,
}

fn reverse(first: &[u32], head: &[u32]) -> SearchGraph {
    let nodes = first.len() - 1;
    let mut reverse_first = vec![0u32; nodes + 1];
    for &to in head {
        reverse_first[to as usize + 1] += 1;
    }
    for node in 0..nodes {
        reverse_first[node + 1] += reverse_first[node];
    }
    let mut cursor = reverse_first.clone();
    let (mut reverse_tail, mut offsets) = (vec![0u32; head.len()], vec![0u32; head.len()]);
    for from in 0..nodes {
        for arc in first[from]..first[from + 1] {
            let at = &mut cursor[head[arc as usize] as usize];
            reverse_tail[*at as usize] = from as u32;
            offsets[*at as usize] = arc - first[from];
            *at += 1;
        }
    }
    SearchGraph {
        first: first.to_vec(),
        head: head.to_vec(),
        reverse_first,
        reverse_tail,
        reverse_offsets: Numbers::U32(offsets),
    }
}

fn at(group: &(Column<u32>, Column<u32>), junction: u32) -> &[u32] {
    let start = group.0.values();
    &group.1.values()[start[junction as usize] as usize..start[junction as usize + 1] as usize]
}

/// Metres east and north from `a` to `b`.
fn vector(a: Point, b: Point) -> (f64, f64) {
    let scale = (f64::from(a.lat) * 1e-6).to_radians().cos();
    (f64::from(b.lon - a.lon) * 0.111_32 * scale, f64::from(b.lat - a.lat) * 0.111_32)
}

impl Net<'_> {
    fn from(&self, road: u32) -> u32 {
        self.graph.roads.from(road as usize)
    }

    fn to(&self, road: u32) -> u32 {
        self.graph.roads.to(road as usize)
    }

    fn length(&self, road: u32) -> f64 {
        f64::from(self.graph.roads.length_m.values()[road as usize])
    }

    fn allowed(&self, road: u32) -> bool {
        self.totals[road as usize] != u32::MAX
    }

    /// The rider may not cycle on the road, but may in the other direction of its way.
    fn against_oneway(&self, road: u32) -> bool {
        let roads = &self.graph.roads;
        let access = self.graph.osm.ways.row(roads.way_row(road as usize) as usize).facts.access;
        let side = usize::from(roads.reversed(road as usize));
        access[side] & BIKE == 0 && access[1 - side] & BIKE != 0
    }

    /// The cost of the transition, or `None` where `touring` may not take it.
    fn turn(&self, before: u32, after: u32) -> Option<u64> {
        let arcs = self.first[before as usize] as usize..self.first[before as usize + 1] as usize;
        let arc = arcs.start + self.head[arcs.clone()].binary_search(&after).ok()?;
        (self.turns[arc] != u32::MAX).then_some(u64::from(self.turns[arc]))
    }

    /// The undirected road ends at a junction.
    fn degree(&self, junction: u32) -> usize {
        let roads = &self.graph.roads;
        let key = |road: u32, other: u32| (roads.way_row(road as usize), other, roads.length_m.values()[road as usize]);
        let mut ends: Vec<_> = at(&self.departures, junction).iter().map(|&road| key(road, self.to(road))).collect();
        ends.extend(at(&self.arrivals, junction).iter().map(|&road| key(road, self.from(road))));
        ends.sort_unstable();
        ends.dedup();
        ends.len()
    }

    /// The change of heading from the end of `before` to the start of `after`, in degrees.
    fn bend(&self, before: u32, after: u32) -> f64 {
        let (a, b) = (self.graph.roads.shape(before as usize), self.graph.roads.shape(after as usize));
        let end = a[a.len() - 1];
        let back = a.iter().rev().skip(1).find(|p| p.distance(end) >= HEADING_M).unwrap_or(&a[0]);
        let ahead = b.iter().skip(1).find(|p| p.distance(b[0]) >= HEADING_M).unwrap_or(&b[b.len() - 1]);
        let (u, v) = (vector(*back, end), vector(b[0], *ahead));
        let lengths = u.0.hypot(u.1) * v.0.hypot(v.1);
        if lengths == 0.0 {
            return 0.0;
        }
        ((u.0 * v.0 + u.1 * v.1) / lengths).clamp(-1.0, 1.0).acos().to_degrees()
    }

    fn is_turn(&self, before: u32, after: u32) -> bool {
        self.degree(self.to(before)) >= 3 && self.bend(before, after) > TURN_DEGREES
    }

    fn terms(&mut self, road: u32) -> Result<terms::RoadTerms, String> {
        if let Some(terms) = self.road_terms.get(&road) {
            return Ok(*terms);
        }
        let found = terms::road(self.graph, self.profile, road as usize)?.ok_or("a road of a path has no cost")?;
        if found.total != u64::from(self.totals[road as usize]) {
            return Err(format!(
                "road {road}: the terms give {}, the bake {}",
                found.total, self.totals[road as usize]
            ));
        }
        self.road_terms.insert(road, found);
        Ok(found)
    }

    fn signature(&self, road: u32) -> String {
        let way = self.graph.osm.ways.row(self.graph.roads.way_row(road as usize) as usize);
        ["highway", "surface", "bicycle", "tracktype", "maxspeed"].map(|key| way.tags.get(key).unwrap_or("-")).join("|")
    }

    fn line(&self, roads: &[u32]) -> Vec<P> {
        let mut line: Vec<P> = Vec::new();
        for &road in roads {
            line.extend(self.graph.roads.shape(road as usize).iter().map(|p| [p.lon, p.lat]));
            line.dedup();
        }
        line
    }

    /// `roads` after `before` and up to `after`. The turn at `before` counts only when `entry` says
    /// that both sides of the split come from that road.
    fn stretch(
        &mut self,
        before: Option<u32>,
        roads: &[u32],
        after: Option<u32>,
        entry: bool,
    ) -> Result<Stretch, String> {
        let mut stretch = Stretch { roads: roads.to_vec(), ..Stretch::default() };
        for (i, &road) in roads.iter().enumerate() {
            let costs = self.terms(road)?;
            terms::add(&mut stretch.terms, &costs.terms);
            stretch.full_bonus += costs.full_bonus;
            stretch.unknown_paved += costs.unknown_paved;
            let length = self.length(road);
            stretch.length_m += length;
            *stretch.signature.entry(self.signature(road)).or_default() += length;
            let way = self.graph.roads.way(road as usize);
            if stretch.ways.last() != Some(&way) {
                stretch.ways.push(way);
            }
            if i + 1 < roads.len() && self.graph.signals.values()[self.to(road) as usize] != 0 {
                stretch.signals += 1;
            }
        }
        let path: Vec<u32> = before.into_iter().chain(roads.iter().copied()).chain(after).collect();
        for (i, pair) in path.windows(2).enumerate() {
            let node = self.turn(pair[0], pair[1]).ok_or("a path takes a forbidden turn")? as f64;
            stretch.terms[NODE] += node;
            stretch.full_bonus += node;
            stretch.unknown_paved += node;
            if (i > 0 || before.is_none() || entry) && self.is_turn(pair[0], pair[1]) {
                stretch.turns += 1;
            }
        }
        stretch.cost = stretch.terms.iter().sum();
        Ok(stretch)
    }

    /// The cheapest road sequence from junction `from` to junction `to`, with the road that it
    /// arrives by at `from` first, and its cost.
    fn route(&mut self, from: u32, to: u32) -> Result<Option<(Vec<u32>, u64)>, String> {
        let seeds = |junction: u32| -> Vec<Seed> {
            let arriving = at(&self.arrivals, junction).iter().filter(|&&road| self.allowed(road));
            arriving.take(255).enumerate().map(|(i, &road)| Seed { node: road, cost: 0, choice: i as u8 }).collect()
        };
        let (starts, ends) = (seeds(from), seeds(to));
        let query = Query {
            starts: &starts,
            ends: &ends,
            ceiling: u64::MAX,
            max_roads: 1_000_000,
            heap_bytes: 1 << 30,
            heap_limit: 1 << 30,
            cancelled: &|| false,
        };
        let found = self.workspace.run(&self.search, &self.costs, query).map_err(|e| e.to_string())?;
        Ok(found.map(|found| (found.roads, found.cost)))
    }
}

/// The level of each relation in a cycle route: its own network or that of a route above it, as
/// `osm::cycle_ranks` gives it to the ways.
fn levels(graph: &Graph) -> HashMap<i64, u8> {
    let relations = &graph.osm.relations;
    let mut pending: Vec<(i64, u8)> = relations
        .iter()
        .filter(|r| {
            r.tags.get("route") == Some("bicycle") && matches!(r.tags.get("type"), Some("route" | "superroute"))
        })
        .map(|r| {
            let level = LEVELS.iter().position(|name| r.tags.get("network") == Some(*name)).filter(|l| *l >= 2);
            (r.id, level.unwrap_or(1) as u8)
        })
        .collect();
    let mut levels = HashMap::new();
    while let Some((id, level)) = pending.pop() {
        if levels.get(&id).is_some_and(|known| *known >= level) {
            continue;
        }
        levels.insert(id, level);
        let Some(relation) = relations.get(id).filter(|r| !r.tags.contains("state", "proposed")) else { continue };
        pending.extend(relation.members().filter_map(|(member, _)| match member {
            Id::Relation(child) => Some((child, level)),
            _ => None,
        }));
    }
    levels
}

/// The main line of a relation as runs of node IDs: its member ways inside the box, in the order
/// of `assemble::main_line`, joined where `assemble::chain` finds an order.
fn line_runs(graph: &Graph, relation: &crate::source::Relation, points: &mut HashMap<i64, P>) -> Vec<Vec<i64>> {
    let mut members = Vec::new();
    for (member, role) in relation.members() {
        let Some(way) = member.way().filter(|_| assemble::is_main(role)).and_then(|id| graph.osm.ways.get(id)) else {
            continue;
        };
        let roundabout = matches!(way.tags.get("junction"), Some("roundabout" | "circular"));
        let mut piece = Vec::new();
        let mut flush = |piece: &mut Vec<i64>| {
            if piece.len() >= 2 {
                members.push(Member { nodes: std::mem::take(piece), role: role.to_string(), roundabout });
            }
            piece.clear();
        };
        for (&node, point) in way.nodes.iter().zip(way.points) {
            if point.present() {
                points.insert(node, [point.lon, point.lat]);
                piece.push(node);
            } else {
                flush(&mut piece);
            }
        }
        flush(&mut piece);
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

/// The directed road of each pair of consecutive nodes of the ways `rows`.
fn pairs(graph: &Graph, vias: &[i64], rows: &HashSet<u32>, census: &mut Census) -> HashMap<(i64, i64), u32> {
    let roads = &graph.roads;
    let mut by_row = BTreeMap::<u32, Vec<u32>>::new();
    for road in 0..roads.len() {
        if rows.contains(&roads.way_row(road)) {
            by_row.entry(roads.way_row(road)).or_default().push(road as u32);
        }
    }
    let node = |junction: u32| graph.node_ids.values()[junction as usize];
    let mut pairs = HashMap::new();
    for (&row, candidates) in &by_row {
        let way = graph.osm.ways.row(row as usize);
        let mut used = vec![false; candidates.len()];
        for run in osm::split(&way, vias) {
            for reversed in [false, true] {
                if way.facts.access[usize::from(reversed)] == 0 {
                    continue;
                }
                let ends = if reversed { (run.last, run.first) } else { (run.first, run.last) };
                let length = run.length.round().max(1.0) as u32;
                let found = candidates.iter().enumerate().position(|(k, &road)| {
                    let road = road as usize;
                    !used[k]
                        && roads.reversed(road) == reversed
                        && node(roads.from(road)) == way.nodes[ends.0]
                        && node(roads.to(road)) == way.nodes[ends.1]
                        && roads.length_m.values()[road] == length
                });
                let Some(found) = found else {
                    census.unmatched_runs += 1;
                    continue;
                };
                used[found] = true;
                for i in run.first..run.last {
                    let key =
                        if reversed { (way.nodes[i + 1], way.nodes[i]) } else { (way.nodes[i], way.nodes[i + 1]) };
                    if pairs.insert(key, candidates[found]).is_some_and(|old| old != candidates[found]) {
                        census.pair_conflicts += 1;
                    }
                }
            }
        }
    }
    pairs
}

/// The legs of a chain: junction indices about every `step_m`. Junction `k` is where road `k`
/// starts; junction `len` is where the chain ends.
fn anchors(lengths: &[f64], step_m: f64) -> Vec<usize> {
    let mut along = vec![0.0];
    for length in lengths {
        along.push(along[along.len() - 1] + length);
    }
    let total = along[along.len() - 1];
    let legs = (total / step_m).round().max(1.0) as usize;
    let mut anchors = vec![0];
    for leg in 1..legs {
        let target = total * leg as f64 / legs as f64;
        let after = along.partition_point(|&m| m < target);
        let nearest = if after > 0 && target - along[after - 1] < along[after] - target { after - 1 } else { after };
        if nearest > anchors[anchors.len() - 1] && nearest < lengths.len() {
            anchors.push(nearest);
        }
    }
    anchors.push(lengths.len());
    anchors
}

pub fn run(
    graph: &Graph,
    profile: &Profile,
    (first, head): (&[u32], &[u32]),
    (totals, turns): (&[u32], &[u32]),
    vias: &[i64],
    (step_m, min_line_m): (f64, f64),
) -> Result<Census, String> {
    let mut census = Census::default();
    let points = graph.points.len();
    let mut net = Net {
        graph,
        profile,
        first,
        head,
        totals,
        turns,
        arrivals: group(graph.roads.to.values(), points),
        departures: group(graph.roads.from.values(), points),
        search: reverse(first, head),
        costs: Costs { roads: Numbers::U32(totals.to_vec()), turns: Arc::new(Numbers::U32(turns.to_vec())) },
        workspace: Workspace::default(),
        road_terms: HashMap::new(),
    };
    let levels = levels(graph);
    let mut node_points = HashMap::new();
    let mut lines = Vec::new();
    let mut rows = HashSet::new();
    for relation in graph.osm.relations.iter() {
        let tags = &relation.tags;
        if tags.get("route") != Some("bicycle")
            || !matches!(tags.get("type"), Some("route" | "superroute"))
            || tags.contains("state", "proposed")
        {
            continue;
        }
        census.relations_seen += 1;
        let runs = line_runs(graph, &relation, &mut node_points);
        let line_m: f64 = runs
            .iter()
            .map(|run| run.windows(2).map(|w| shape::distance(node_points[&w[0]], node_points[&w[1]])).sum::<f64>())
            .sum();
        if line_m < min_line_m {
            census.relations_under_5km += 1;
            let level = usize::from(levels.get(&relation.id).copied().unwrap_or(1));
            census.excluded[level].0 += 1;
            census.excluded[level].1 += line_m;
            census.excluded[level].2 +=
                usize::from(relation.members().any(|(member, _)| matches!(member, Id::Relation(_))));
            continue;
        }
        let ways = relation.members().filter_map(|(member, _)| graph.osm.ways.index(member.way()?));
        rows.extend(ways.map(|row| row as u32));
        lines.push((relation, runs, line_m));
    }
    let pairs = pairs(graph, vias, &rows, &mut census);
    let mut seen = HashMap::<(Vec<u32>, Vec<u32>), usize>::new();
    let (mut measured_roads, mut split_roads) = (HashSet::new(), HashSet::new());
    for (relation, runs, line_m) in lines {
        let level = levels.get(&relation.id).copied().unwrap_or(1);
        let name = [relation.tags.get("ref"), relation.tags.get("name")].into_iter().flatten().collect::<Vec<_>>();
        let mut tally = Tally { relations: 1, line_m, ..Tally::default() };
        // The chains: the roads of each run, cut where the line has no road that `touring` may use.
        let mut chains: Vec<Vec<u32>> = Vec::new();
        for run in &runs {
            let mut chain: Vec<u32> = Vec::new();
            for w in run.windows(2).filter(|w| w[0] != w[1]) {
                let road = pairs.get(&(w[0], w[1])).copied().filter(|&road| net.allowed(road));
                if road.is_some_and(|road| net.against_oneway(road)) {
                    tally.oneway_m += shape::distance(node_points[&w[0]], node_points[&w[1]]);
                    chains.push(std::mem::take(&mut chain));
                    continue;
                }
                let joins = |last: u32, road: u32| net.to(last) == net.from(road) && net.turn(last, road).is_some();
                match (road, chain.last().copied()) {
                    (Some(road), Some(last)) if road == last => {}
                    (Some(road), Some(last)) if joins(last, road) => chain.push(road),
                    (Some(road), _) => {
                        chains.push(std::mem::replace(&mut chain, vec![road]));
                    }
                    (None, _) => {
                        tally.lost_m += shape::distance(node_points[&w[0]], node_points[&w[1]]);
                        chains.push(std::mem::take(&mut chain));
                    }
                }
            }
            chains.push(chain);
        }
        for chain in chains.iter().filter(|chain| !chain.is_empty()) {
            let lengths: Vec<f64> = chain.iter().map(|&road| net.length(road)).collect();
            if lengths.iter().sum::<f64>() < MIN_CHAIN_M {
                tally.short_m += lengths.iter().sum::<f64>();
                continue;
            }
            for leg in anchors(&lengths, step_m).windows(2) {
                let (mut a, b) = (leg[0], leg[1]);
                // The road that the signed line arrives by at the first junction of the leg.
                let mut seed = a.checked_sub(1).map(|before| chain[before]);
                if seed.is_none() {
                    let arriving = at(&net.arrivals, net.from(chain[0])).iter().filter(|&&road| net.allowed(road));
                    seed =
                        arriving.filter_map(|&road| Some((net.turn(road, chain[0])?, road))).min().map(|found| found.1);
                    if seed.is_none() {
                        (seed, a) = (Some(chain[0]), 1);
                    }
                }
                let (seed, signed) = (seed.expect("a seed"), &chain[a..b]);
                let (from, to) = (net.to(seed), net.to(chain[b - 1]));
                if signed.is_empty() || from == to {
                    tally.short_m += lengths[leg[0]..b].iter().sum::<f64>();
                    continue;
                }
                let clock = Instant::now();
                let found = net.route(from, to)?;
                census.query_seconds += clock.elapsed().as_secs_f64();
                census.queries += 1;
                let Some((path, cost)) = found else { return Err(format!("relation {}: no path", relation.id)) };
                let (router_seed, router) = (path[0], &path[1..]);
                let signed_leg = net.stretch(Some(seed), signed, None, false)?;
                let router_leg = net.stretch(Some(router_seed), router, None, false)?;
                if (router_leg.cost - cost as f64).abs() > 0.5 {
                    return Err(format!(
                        "relation {}: the terms give {}, the search {cost}",
                        relation.id, router_leg.cost
                    ));
                }
                if router_leg.cost > signed_leg.cost + 0.5 {
                    census.optimality_violations += 1;
                }
                let (signed_line, router_line) = (net.line(signed), net.line(router));
                let (near_signed, near_router) = (Near::new(&signed_line), Near::new(&router_line));
                tally.legs += 1;
                tally.identical_legs += usize::from(signed == router);
                tally.measured_m += signed_leg.length_m;
                tally.near_m += signed_leg.length_m - shape::far(&signed_line, &near_router);
                tally.signed_cost += signed_leg.cost;
                tally.router_cost += router_leg.cost;
                for (signature, metres) in &signed_leg.signature {
                    *census.measured_signature.entry(signature.clone()).or_default() += metres;
                }
                measured_roads.extend(signed.iter().copied());
                // The splits: from the first road that differs to the next road that both take.
                let mut places = HashMap::<u32, Vec<usize>>::new();
                for (i, &road) in signed.iter().enumerate() {
                    places.entry(road).or_default().push(i);
                }
                let (mut i, mut j) = (0, 0);
                while i < signed.len() || j < router.len() {
                    if i < signed.len() && j < router.len() && signed[i] == router[j] {
                        tally.same_road_m += net.length(signed[i]);
                        (i, j) = (i + 1, j + 1);
                        continue;
                    }
                    let rejoin = (j..router.len()).find_map(|j2| {
                        let i2 = places.get(&router[j2])?.iter().copied().find(|&i2| i2 > i)?;
                        Some((i2, j2))
                    });
                    let (i2, j2) = rejoin.unwrap_or((signed.len(), router.len()));
                    let before = |k: usize, list: &[u32], seed: u32| if k > 0 { list[k - 1] } else { seed };
                    let after = signed.get(i2).copied();
                    let entry = i > 0 && j > 0;
                    let signed_side = net.stretch(Some(before(i, signed, seed)), &signed[i..i2], after, entry)?;
                    let router_side =
                        net.stretch(Some(before(j, router, router_seed)), &router[j..j2], after, entry)?;
                    tally.splits += 1;
                    tally.split_m += signed_side.length_m;
                    split_roads.extend(signed[i..i2].iter().copied());
                    let key = (signed[i..i2].to_vec(), router[j..j2].to_vec());
                    if let Some(&known) = seen.get(&key) {
                        census.splits[known].seen += 1;
                    } else {
                        seen.insert(key, census.splits.len());
                        let junction = |road: u32| graph.points.values()[road as usize];
                        census.splits.push(Split {
                            relation: relation.id,
                            level,
                            seen: 1,
                            from: junction(net.to(before(i, signed, seed))),
                            to: junction(if i2 > 0 { net.to(signed[i2 - 1]) } else { from }),
                            signed_far_m: shape::far(&net.line(&signed[i..i2]), &near_router),
                            router_far_m: shape::far(&net.line(&router[j..j2]), &near_signed),
                            at_point: i == 0 || i2 == signed.len(),
                            signed: signed_side,
                            router: router_side,
                        });
                    }
                    (i, j) = (i2, j2);
                }
            }
        }
        census.levels[usize::from(level)].add(&tally);
        census.total.add(&tally);
        census.routes.push(Route {
            id: relation.id,
            name: name.join(" "),
            network: relation.tags.get("network").unwrap_or("").to_string(),
            level,
            tally,
        });
    }
    census.unique_measured_m = measured_roads.iter().map(|&road| net.length(road)).sum();
    census.unique_split_m = split_roads.iter().map(|&road| net.length(road)).sum();
    Ok(census)
}
