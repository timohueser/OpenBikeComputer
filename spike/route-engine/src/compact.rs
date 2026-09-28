//! Representation experiment: arrival states exist only at junctions with effective turn restrictions.
use crate::model::{Graph, Point, Profile};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::mem::size_of;
use std::ops::Range;

#[derive(Clone, Copy, Debug)]
pub struct WeightedArc {
    pub to: u32,
    pub road: u32,
    pub cost: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Route {
    pub cost: u64,
    pub roads: Vec<u32>,
}

#[derive(Serialize)]
pub struct Stats {
    pub junctions: usize,
    pub restricted_junctions: usize,
    pub states: usize,
    pub arcs: usize,
    pub permitted_roads: usize,
    pub full_expanded_states: usize,
    pub full_expanded_arcs: usize,
    /// Vec backing allocations; excludes allocator and HashMap bucket overhead.
    pub vector_capacity_bytes: usize,
    /// Existing WeightedGraph layout at exact lengths, before allocator spare capacity.
    pub full_expanded_minimum_vector_bytes: usize,
}

pub struct Compact {
    pub coords: Vec<Point>,
    pub outgoing: Vec<Vec<WeightedArc>>,
    junctions: Vec<Range<u32>>,
    /// A start has no incoming turn. These arcs can exist even when all arrivals forbid them.
    restricted_starts: HashMap<u32, Vec<WeightedArc>>,
    stats: Stats,
}

impl Compact {
    pub fn build(graph: &Graph, profile: &Profile) -> Result<Self, String> {
        profile.validate()?;
        let costs: Vec<_> = graph.roads.iter().map(|r| profile.cost(r).unwrap_or(u64::MAX)).collect();
        let n = graph.points.len();
        if n > u32::MAX as usize || graph.roads.len() > u32::MAX as usize {
            return Err("Graph exceeds u32 IDs".into());
        }
        let mut active = vec![false; n];
        let mut incoming = vec![0u32; n];
        for (road, &cost) in graph.roads.iter().zip(&costs) {
            if road.from as usize >= n || road.to as usize >= n {
                return Err("Road endpoint outside graph".into());
            }
            if cost == u64::MAX {
                continue;
            }
            active[road.from as usize] = true;
            active[road.to as usize] = true;
            incoming[road.to as usize] += 1;
        }
        let mut restricted = vec![false; n];
        let restrictions = if profile.walking { &graph.forbidden_foot } else { &graph.forbidden };
        if !restrictions.is_sorted() {
            return Err("Turn restrictions must be sorted".into());
        }
        for &(before, after) in restrictions {
            let before_road = graph.roads.get(before as usize).ok_or("Restriction road outside graph")?;
            let after_road = graph.roads.get(after as usize).ok_or("Restriction road outside graph")?;
            if before_road.to != after_road.from {
                return Err("Restriction roads do not meet".into());
            }
            if costs[before as usize] != u64::MAX && costs[after as usize] != u64::MAX {
                restricted[before_road.to as usize] = true;
            }
        }
        let mut junctions = Vec::with_capacity(n);
        let mut coords = Vec::new();
        for junction in 0..n {
            let first = u32::try_from(coords.len()).map_err(|_| "Too many compact states")?;
            let count = if restricted[junction] { incoming[junction] } else { active[junction] as u32 };
            let end = first.checked_add(count).ok_or("Too many compact states")?;
            junctions.push(first..end);
            coords.extend(std::iter::repeat_n(graph.points[junction], count as usize));
        }
        let mut next: Vec<_> = junctions.iter().map(|r| r.start).collect();
        let mut arrival = vec![u32::MAX; graph.roads.len()];
        for (id, road) in graph.roads.iter().enumerate() {
            if costs[id] == u64::MAX {
                continue;
            }
            let junction = road.to as usize;
            arrival[id] = next[junction];
            if restricted[junction] {
                next[junction] += 1;
            }
        }
        let mut outgoing = vec![Vec::new(); coords.len()];
        let mut restricted_starts = HashMap::<u32, Vec<WeightedArc>>::new();
        for (id, road) in graph.roads.iter().enumerate() {
            if costs[id] == u64::MAX {
                continue;
            }
            let arc = WeightedArc { to: arrival[id], road: id as u32, cost: costs[id] };
            if restricted[road.from as usize] {
                restricted_starts.entry(road.from).or_default().push(arc);
            } else {
                outgoing[junctions[road.from as usize].start as usize].push(arc);
            }
        }
        for (id, road) in graph.roads.iter().enumerate() {
            if costs[id] == u64::MAX || !restricted[road.to as usize] {
                continue;
            }
            for &arc in restricted_starts.get(&road.to).into_iter().flatten() {
                if graph.permits_turn(id as u32, arc.road, profile.walking) {
                    outgoing[arrival[id] as usize].push(arc);
                }
            }
        }
        let full_expanded_arcs = (0..n)
            .map(|junction| {
                let range = &junctions[junction];
                if range.is_empty() {
                    0
                } else if restricted[junction] {
                    range.clone().map(|s| outgoing[s as usize].len()).sum()
                } else {
                    incoming[junction] as usize * outgoing[range.start as usize].len()
                }
            })
            .sum();
        let vector_capacity_bytes = coords.capacity() * size_of::<Point>()
            + outgoing.capacity() * size_of::<Vec<WeightedArc>>()
            + outgoing.iter().map(|arcs| arcs.capacity() * size_of::<WeightedArc>()).sum::<usize>()
            + junctions.capacity() * size_of::<Range<u32>>()
            + restricted_starts.values().map(|arcs| arcs.capacity() * size_of::<WeightedArc>()).sum::<usize>();
        let stats = Stats {
            junctions: active.iter().filter(|&&active| active).count(),
            restricted_junctions: restricted.iter().filter(|&&value| value).count(),
            states: coords.len(),
            arcs: outgoing.iter().map(Vec::len).sum(),
            permitted_roads: costs.iter().filter(|&&c| c != u64::MAX).count(),
            full_expanded_states: graph.roads.len(),
            full_expanded_arcs,
            vector_capacity_bytes,
            full_expanded_minimum_vector_bytes: graph.roads.len() * (size_of::<Point>() + size_of::<Vec<(u32, u64)>>())
                + full_expanded_arcs * size_of::<(u32, u64)>(),
        };
        Ok(Self { coords, outgoing, junctions, restricted_starts, stats })
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn starts(&self, junction: u32) -> &[WeightedArc] {
        if let Some(arcs) = self.restricted_starts.get(&junction) {
            return arcs;
        }
        let Some(range) = self.junctions.get(junction as usize) else { return &[] };
        if range.is_empty() {
            &[]
        } else {
            &self.outgoing[range.start as usize]
        }
    }

    pub fn targets(&self, junction: u32) -> Range<u32> {
        self.junctions.get(junction as usize).cloned().unwrap_or(0..0)
    }

    pub fn dijkstra(&self, start: u32, end: u32) -> Result<Option<Route>, String> {
        if start as usize >= self.junctions.len() || end as usize >= self.junctions.len() {
            return Err("Invalid junction".into());
        }
        if start == end {
            return Ok(Some(Route { cost: 0, roads: vec![] }));
        }
        let targets = self.targets(end);
        let mut distances = vec![u64::MAX; self.coords.len()];
        let mut parents = vec![u32::MAX; self.coords.len()];
        let mut roads = vec![u32::MAX; self.coords.len()];
        let mut heap = BinaryHeap::new();
        for arc in self.starts(start) {
            if (arc.cost, arc.road) < (distances[arc.to as usize], roads[arc.to as usize]) {
                distances[arc.to as usize] = arc.cost;
                roads[arc.to as usize] = arc.road;
                heap.push(Reverse((arc.cost, arc.to)));
            }
        }
        while let Some(Reverse((cost, node))) = heap.pop() {
            if cost != distances[node as usize] {
                continue;
            }
            if targets.contains(&node) {
                let mut witness = vec![];
                let mut state = node;
                while state != u32::MAX {
                    witness.push(roads[state as usize]);
                    state = parents[state as usize];
                }
                witness.reverse();
                return Ok(Some(Route { cost, roads: witness }));
            }
            for arc in &self.outgoing[node as usize] {
                let next = cost.checked_add(arc.cost).filter(|&c| c != u64::MAX).ok_or("Cost overflow")?;
                if next < distances[arc.to as usize] {
                    distances[arc.to as usize] = next;
                    parents[arc.to as usize] = node;
                    roads[arc.to as usize] = arc.road;
                    heap.push(Reverse((next, arc.to)));
                }
            }
        }
        Ok(None)
    }

    pub fn prepare_ch(&self) -> Result<fast_paths::FastGraph, String> {
        let mut input = fast_paths::InputGraph::new();
        for (from, arcs) in self.outgoing.iter().enumerate() {
            for arc in arcs {
                let cost = usize::try_from(arc.cost).map_err(|_| "Cost exceeds native word size")?;
                if cost == usize::MAX || cost == 0 {
                    return Err("CH needs positive finite costs".into());
                }
                input.add_edge(from, arc.to as usize, cost);
            }
        }
        input.freeze();
        Ok(fast_paths::prepare(&input))
    }

    pub fn ch_route(
        &self,
        ch: &fast_paths::FastGraph,
        calculator: &mut fast_paths::PathCalculator,
        start: u32,
        end: u32,
    ) -> Result<Option<Route>, String> {
        if start as usize >= self.junctions.len() || end as usize >= self.junctions.len() {
            return Err("Invalid junction".into());
        }
        if start == end {
            return Ok(Some(Route { cost: 0, roads: vec![] }));
        }
        let targets = self.targets(end);
        let mut best = self
            .starts(start)
            .iter()
            .filter(|s| targets.contains(&s.to))
            .min_by_key(|s| (s.cost, s.road))
            .map(|s| Route { cost: s.cost, roads: vec![s.road] });
        // Fast Paths excludes trailing states with no incident arc. Such states only permit a direct seed route.
        let n = ch.get_num_nodes();
        let starts = self
            .starts(start)
            .iter()
            .filter(|s| (s.to as usize) < n)
            .map(|s| (s.to as usize, s.cost as usize))
            .collect::<Vec<_>>();
        let ends = targets.filter(|&s| (s as usize) < n).map(|s| (s as usize, 0)).collect::<Vec<_>>();
        if starts.is_empty() || ends.is_empty() {
            return Ok(best);
        }
        if let Some(path) = calculator.calc_path_multiple_sources_and_targets(ch, starts, ends) {
            let nodes = path.get_nodes();
            let seed = self
                .starts(start)
                .iter()
                .filter(|s| s.to as usize == nodes[0])
                .min_by_key(|s| (s.cost, s.road))
                .ok_or("Missing source witness")?;
            let mut route = Route { cost: seed.cost, roads: vec![seed.road] };
            for pair in nodes.windows(2) {
                let arc = self.outgoing[pair[0]]
                    .iter()
                    .filter(|a| a.to as usize == pair[1])
                    .min_by_key(|a| (a.cost, a.road))
                    .ok_or("Missing base arc witness")?;
                route.cost = route.cost.checked_add(arc.cost).ok_or("Witness cost overflow")?;
                route.roads.push(arc.road);
            }
            if route.cost != path.get_weight() as u64 {
                return Err("CH witness cost mismatch".into());
            }
            if best.as_ref().is_none_or(|b| (route.cost, &route.roads) < (b.cost, &b.roads)) {
                best = Some(route);
            }
        }
        Ok(best)
    }
}

/// Full arrival-road Dijkstra oracle with the same unconstrained junction endpoints.
pub fn full_reference(graph: &Graph, profile: &Profile, start: u32, end: u32) -> Option<Route> {
    if start == end {
        return Some(Route { cost: 0, roads: vec![] });
    }
    let costs: Vec<_> = graph.roads.iter().map(|r| profile.cost(r)).collect();
    let departures = graph.departures();
    let mut distances = vec![u64::MAX; graph.roads.len()];
    let mut parents = vec![u32::MAX; graph.roads.len()];
    let mut heap = BinaryHeap::new();
    for &road in &departures[start as usize] {
        if let Some(cost) = costs[road as usize] {
            distances[road as usize] = cost;
            heap.push(Reverse((cost, road)));
        }
    }
    while let Some(Reverse((cost, incoming))) = heap.pop() {
        if cost != distances[incoming as usize] {
            continue;
        }
        let junction = graph.roads[incoming as usize].to;
        if junction == end {
            let mut roads = vec![];
            let mut current = incoming;
            while current != u32::MAX {
                roads.push(current);
                current = parents[current as usize];
            }
            roads.reverse();
            return Some(Route { cost, roads });
        }
        for &outgoing in &departures[junction as usize] {
            if !graph.permits_turn(incoming, outgoing, profile.walking) {
                continue;
            }
            let Some(weight) = costs[outgoing as usize] else { continue };
            let next = cost.checked_add(weight)?;
            if next < distances[outgoing as usize] {
                distances[outgoing as usize] = next;
                parents[outgoing as usize] = incoming;
                heap.push(Reverse((next, outgoing)));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Road, Surface, BIKE, FOOT};

    fn fixture() -> Graph {
        let pairs = [
            (0, 1, 1),
            (0, 1, 1),
            (1, 2, 1),
            (2, 3, 1),
            (1, 4, 2),
            (4, 2, 2),
            (2, 5, 3),
            (5, 3, 3),
            (3, 0, 2),
            (2, 1, 1),
        ];
        Graph {
            points: (0..7).map(|i| Point { lat: 0, lon: i * 100, elevation: 0 }).collect(),
            roads: pairs
                .iter()
                .enumerate()
                .map(|(id, &(from, to, length_m))| Road {
                    from,
                    to,
                    way: id as i64,
                    length_m,
                    ascent_m: 0,
                    descent_m: 0,
                    surface: Surface::Paved,
                    class: 0,
                    access: BIKE | FOOT,
                    difficulty: 0,
                    hiking_difficulty: None,
                    uncertain_access: false,
                    shape: vec![],
                })
                .collect(),
            forbidden: vec![(0, 2), (2, 3)],
            forbidden_foot: vec![(1, 2), (5, 3)],
            warnings: vec![],
        }
    }

    fn assert_witness(graph: &Graph, profile: &Profile, start: u32, end: u32, route: &Route) {
        let mut node = start;
        let mut cost = 0;
        for &road in &route.roads {
            assert_eq!(graph.roads[road as usize].from, node);
            node = graph.roads[road as usize].to;
            cost += profile.cost(&graph.roads[road as usize]).unwrap();
        }
        for roads in route.roads.windows(2) {
            assert!(graph.permits_turn(roads[0], roads[1], profile.walking));
        }
        assert_eq!(node, end);
        assert_eq!(cost, route.cost);
    }

    #[test]
    fn compact_and_ch_match_arrival_state_oracle_at_every_junction() {
        let mut graph = fixture();
        for variant in 0..4 {
            if variant == 1 {
                graph.forbidden = vec![(0, 2), (1, 2), (2, 3), (9, 2)];
            }
            if variant == 2 {
                graph.roads[0].access = FOOT;
                graph.roads[5].access = BIKE;
            }
            if variant == 3 {
                graph.forbidden.clear();
                graph.forbidden_foot.clear();
            }
            for profile in Profile::presets().into_iter().filter(|p| p.name == "touring" || p.name == "hiking") {
                let compact = Compact::build(&graph, &profile).unwrap();
                let full = crate::build::state_graph(&graph, &profile).unwrap();
                assert_eq!(compact.stats().full_expanded_arcs, full.adjacency.iter().map(Vec::len).sum::<usize>());
                let ch = compact.prepare_ch().unwrap();
                let mut calculator = fast_paths::PathCalculator::new(ch.get_num_nodes());
                for start in 0..graph.points.len() as u32 {
                    for end in 0..graph.points.len() as u32 {
                        let expected = full_reference(&graph, &profile, start, end);
                        for route in [
                            compact.dijkstra(start, end).unwrap(),
                            compact.ch_route(&ch, &mut calculator, start, end).unwrap(),
                        ] {
                            assert_eq!(
                                route.as_ref().map(|r| r.cost),
                                expected.as_ref().map(|r| r.cost),
                                "variant={variant} profile={} start={start} end={end}",
                                profile.name
                            );
                            if let Some(route) = route {
                                assert_witness(&graph, &profile, start, end, &route);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn blocked_arrivals_do_not_block_starting_at_that_junction() {
        let mut graph = fixture();
        graph.points.truncate(3);
        graph.roads = vec![graph.roads[0].clone(), graph.roads[2].clone()];
        graph.forbidden = vec![(0, 1)];
        graph.forbidden_foot.clear();
        let profile = Profile::presets().remove(0);
        let compact = Compact::build(&graph, &profile).unwrap();
        let ch = compact.prepare_ch().unwrap();
        let mut calculator = fast_paths::PathCalculator::new(ch.get_num_nodes());
        assert_eq!(ch.get_num_nodes(), 2);
        assert_eq!(compact.ch_route(&ch, &mut calculator, 1, 2).unwrap().unwrap().roads, [1]);
        assert_eq!(compact.ch_route(&ch, &mut calculator, 0, 2).unwrap(), None);
        assert_eq!(compact.ch_route(&ch, &mut calculator, 0, 1).unwrap().unwrap().roads, [0]);
        assert!(compact.ch_route(&ch, &mut calculator, 1, 1).unwrap().unwrap().roads.is_empty());
    }
}
