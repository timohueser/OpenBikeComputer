use crate::queue::Queue;
use crate::{
    base::{Costs, Graph},
    Error, Result,
};

#[derive(Default)]
struct Labels {
    costs: Vec<u64>,
    parents: Vec<u32>,
    origins: Vec<u8>,
    dirty: Vec<u64>,
    count: usize,
}
impl Labels {
    fn reset(&mut self, nodes: usize) -> Result<()> {
        if self.costs.len() != nodes {
            self.costs.clear();
            self.parents.clear();
            self.origins.clear();
            self.costs.try_reserve_exact(nodes).map_err(|_| Error::Limit)?;
            self.parents.try_reserve_exact(nodes).map_err(|_| Error::Limit)?;
            self.origins.try_reserve_exact(nodes).map_err(|_| Error::Limit)?;
            self.origins.resize(nodes, u8::MAX);
            self.costs.resize(nodes, u64::MAX);
            self.parents.resize(nodes, u32::MAX);
            self.dirty = vec![0; nodes.div_ceil(512 * 64)];
        } else {
            for (word, bits) in self.dirty.iter_mut().enumerate() {
                while *bits != 0 {
                    let start = (word * 64 + bits.trailing_zeros() as usize) * 512;
                    self.costs[start..(start + 512).min(nodes)].fill(u64::MAX);
                    *bits &= *bits - 1;
                }
            }
        }
        self.count = 0;
        Ok(())
    }
    fn set(&mut self, node: u32, cost: u64, parent: u32, origin: u8) {
        let n = node as usize;
        if self.costs[n] == u64::MAX {
            self.count += 1;
            self.dirty[n / (512 * 64)] |= 1 << ((n / 512) % 64);
        }
        self.costs[n] = cost;
        self.parents[n] = parent;
        self.origins[n] = origin;
    }
}

#[derive(Default)]
pub struct Workspace {
    labels: [Labels; 2],
    heaps: [Queue; 2],
}

pub struct Seed {
    pub node: u32,
    pub cost: u64,
    pub road: u32,
    pub choice: u8,
}

#[derive(Clone, Copy)]
pub struct Query<'a> {
    pub starts: &'a [Seed],
    pub ends: &'a [Seed],
    pub ceiling: u64,
    pub max_labels: usize,
    pub max_roads: usize,
    pub heap_bytes: usize,
    pub cancelled: &'a dyn Fn() -> bool,
}

pub struct Found {
    pub cost: u64,
    pub roads: Vec<u32>,
    pub source: usize,
    pub target: usize,
}

impl Workspace {
    pub fn clear_heaps(&mut self) {
        for heap in &mut self.heaps {
            heap.clear();
            heap.shrink_to_fit();
        }
    }

    fn push(
        &mut self,
        side: usize,
        value: (u64, u8, u32),
        budget: usize,
        potential: &mut Option<&mut dyn FnMut(u32) -> Result<i128>>,
    ) -> Result<()> {
        let (cost, origin, node) = value;
        let p = match potential {
            Some(potential) => potential(node)?,
            None => 0,
        };
        let priority = if side == 0 { (cost as i128 * 2).checked_add(p) } else { (cost as i128 * 2).checked_sub(p) }
            .ok_or(Error::Limit)?;
        if priority == i128::MAX {
            return Err(Error::Limit);
        }
        let available = budget.saturating_sub(self.heaps[1 - side].bytes());
        self.heaps[side].push((priority, cost, origin, node), available)
    }

    pub fn run(&mut self, graph: &Graph, costs: &Costs, query: Query<'_>) -> Result<Option<Found>> {
        self.run_with_potential(graph, costs, query, None)
    }

    /// The potential must satisfy p(u) - p(v) <= 2 * weight(u, v) on every legal arc.
    pub fn run_with_potential(
        &mut self,
        graph: &Graph,
        costs: &Costs,
        query: Query<'_>,
        mut potential: Option<&mut dyn FnMut(u32) -> Result<i128>>,
    ) -> Result<Option<Found>> {
        let Query { starts, ends, ceiling, max_labels, max_roads, heap_bytes, cancelled } = query;
        if starts.is_empty() || ends.is_empty() {
            return Ok(None);
        }
        if self.heaps.iter().map(Queue::bytes).sum::<usize>() > heap_bytes {
            self.clear_heaps();
        }
        let nodes = graph.nodes();
        for side in 0..2 {
            self.labels[side].reset(nodes)?;
            self.heaps[side].clear();
        }
        for (side, seeds) in [starts, ends].iter().enumerate() {
            for seed in *seeds {
                if seed.node as usize >= nodes || seed.cost == u64::MAX {
                    return Err(Error::InvalidData("Invalid base search seed".into()));
                }
                if (seed.cost, seed.choice)
                    < (self.labels[side].costs[seed.node as usize], self.labels[side].origins[seed.node as usize])
                {
                    if self.labels[side].costs[seed.node as usize] == u64::MAX
                        && self.labels[0].count + self.labels[1].count >= max_labels
                    {
                        return Err(Error::Limit);
                    }
                    self.labels[side].set(seed.node, seed.cost, u32::MAX, seed.choice);
                    self.push(side, (seed.cost, seed.choice, seed.node), heap_bytes, &mut potential)?;
                }
            }
        }
        let mut best = ceiling;
        let mut meeting = None;
        let mut best_roots = (u8::MAX, u8::MAX);
        let mut work = 0usize;
        loop {
            if work.is_multiple_of(1024) && (cancelled)() {
                return Err(Error::Cancelled);
            }
            for side in 0..2 {
                let available = heap_bytes.saturating_sub(self.heaps[1 - side].bytes());
                while let Some((_, cost, origin, node)) = self.heaps[side].front(available)? {
                    if self.labels[side].costs[node as usize] == cost
                        && self.labels[side].origins[node as usize] == origin
                    {
                        break;
                    }
                    self.heaps[side].pop();
                }
            }
            let top: [i128; 2] = std::array::from_fn(|i| self.heaps[i].top().map_or(i128::MAX, |v| v.0));
            if top[0] == i128::MAX || top[1] == i128::MAX || top[0].saturating_add(top[1]) > best as i128 * 2 {
                break;
            }
            let side = usize::from(top[1] < top[0]);
            let (_, cost, origin, node) = self.heaps[side].pop().unwrap();
            work += 1;
            let opposite = self.labels[1 - side].costs[node as usize];
            if opposite != u64::MAX {
                let joined = cost.checked_add(opposite).ok_or(Error::Limit)?;
                if (joined, (self.labels[1].origins[node as usize], self.labels[0].origins[node as usize]))
                    < (best, best_roots)
                {
                    best = joined;
                    meeting = Some(node);
                    best_roots = (self.labels[1].origins[node as usize], self.labels[0].origins[node as usize]);
                }
            }
            let row = if side == 0 {
                graph.first[node as usize]..graph.first[node as usize + 1]
            } else {
                graph.reverse_first[node as usize]..graph.reverse_first[node as usize + 1]
            };
            for index in row {
                let index = index as usize;
                let arc = if side == 0 { index } else { graph.reverse_arc(index) };
                let to = if side == 0 { graph.head[index] } else { graph.reverse_tail[index] };
                let weight = costs.arc(graph, arc);
                if weight == u64::MAX {
                    continue;
                }
                let next = cost.checked_add(weight).filter(|&n| n != u64::MAX).ok_or(Error::Limit)?;
                if (next, origin) >= (self.labels[side].costs[to as usize], self.labels[side].origins[to as usize])
                    || next > best
                {
                    continue;
                }
                if self.labels[side].costs[to as usize] == u64::MAX
                    && self.labels[0].count + self.labels[1].count >= max_labels
                {
                    return Err(Error::Limit);
                }
                self.labels[side].set(to, next, arc as u32, origin);
                self.push(side, (next, origin, to), heap_bytes, &mut potential)?;
                let opposite = self.labels[1 - side].costs[to as usize];
                if opposite != u64::MAX {
                    let joined = next.checked_add(opposite).ok_or(Error::Limit)?;
                    if (joined, (self.labels[1].origins[to as usize], self.labels[0].origins[to as usize]))
                        < (best, best_roots)
                    {
                        best = joined;
                        meeting = Some(to);
                        best_roots = (self.labels[1].origins[to as usize], self.labels[0].origins[to as usize]);
                    }
                }
            }
        }
        let Some(meeting) = meeting else {
            return Ok(None);
        };
        let mut arcs = Vec::new();
        let mut node = meeting;
        while self.labels[0].parents[node as usize] != u32::MAX {
            let arc = self.labels[0].parents[node as usize];
            if arcs.len() >= max_roads {
                return Err(Error::Limit);
            }
            arcs.push(arc);
            node = (graph.first.partition_point(|&v| v <= arc) - 1) as u32;
        }
        let source = starts
            .iter()
            .position(|s| {
                s.node == node
                    && s.cost == self.labels[0].costs[node as usize]
                    && s.choice == self.labels[0].origins[node as usize]
            })
            .ok_or_else(|| Error::InvalidData("Missing source witness".into()))?;
        arcs.reverse();
        node = meeting;
        while self.labels[1].parents[node as usize] != u32::MAX {
            let arc = self.labels[1].parents[node as usize];
            if arcs.len() >= max_roads {
                return Err(Error::Limit);
            }
            arcs.push(arc);
            node = graph.head[arc as usize];
        }
        let target = ends
            .iter()
            .position(|s| {
                s.node == node
                    && s.cost == self.labels[1].costs[node as usize]
                    && s.choice == self.labels[1].origins[node as usize]
            })
            .ok_or_else(|| Error::InvalidData("Missing target witness".into()))?;
        let mut roads = Vec::with_capacity(arcs.len() + 1);
        roads.push(starts[source].road);
        let mut check = starts[source].cost.checked_add(ends[target].cost).ok_or(Error::Limit)?;
        for arc in arcs {
            check = check.checked_add(costs.arc(graph, arc as usize)).ok_or(Error::Limit)?;
            roads.push(graph.head[arc as usize]);
        }
        if check != best {
            return Err(Error::InvalidData("Base path witness cost differs".into()));
        }
        Ok(Some(Found { cost: best, roads, source, target }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::Numbers;

    fn graph(nodes: usize, edges: &[(u32, u32)]) -> Graph {
        let mut first = vec![0; nodes + 1];
        let mut head = Vec::new();
        let mut reverse_first = vec![0; nodes + 1];
        for &(from, to) in edges {
            first[from as usize + 1] += 1;
            head.push(to);
            reverse_first[to as usize + 1] += 1;
        }
        for n in 0..nodes {
            first[n + 1] += first[n];
            reverse_first[n + 1] += reverse_first[n];
        }
        let mut cursor = reverse_first.clone();
        let mut tail = vec![0; edges.len()];
        let mut offset = vec![0; edges.len()];
        for (a, &(from, to)) in edges.iter().enumerate() {
            let i = cursor[to as usize] as usize;
            tail[i] = from;
            offset[i] = a as u32 - first[from as usize];
            cursor[to as usize] += 1;
        }
        Graph { first, head, reverse_first, reverse_tail: tail, reverse_offsets: Numbers::U32(offset) }
    }

    #[test]
    fn workspace_reuse_clears_reached_labels_after_limits_and_dimension_changes() {
        let mut workspace = Workspace::default();
        for nodes in [32_769, 513, 2, 65_537] {
            let edges: Vec<_> = (1..nodes as u32).map(|to| (to - 1, to)).collect();
            let graph = graph(nodes, &edges);
            let costs = Costs { roads: Numbers::U8(vec![1; nodes]), turns: Numbers::U8(vec![0; edges.len()]) };
            for (from, to, limit) in [(0, nodes - 1, 2), (0, nodes - 1, usize::MAX), (nodes - 1, 0, usize::MAX)] {
                let starts = [Seed { node: from as u32, cost: 0, road: from as u32, choice: 0 }];
                let ends = [Seed { node: to as u32, cost: 0, road: to as u32, choice: 0 }];
                let result = workspace.run(
                    &graph,
                    &costs,
                    Query {
                        starts: &starts,
                        ends: &ends,
                        ceiling: u64::MAX,
                        max_labels: limit,
                        max_roads: nodes,
                        heap_bytes: 1024 * 1024,
                        cancelled: &|| false,
                    },
                );
                if limit == 2 {
                    assert!(matches!(result, Err(Error::Limit)));
                } else if from > to {
                    assert!(result.unwrap().is_none());
                } else {
                    let found = result.unwrap().unwrap();
                    assert_eq!(found.cost, (nodes - 1) as u64);
                    assert_eq!(found.roads, (0..nodes as u32).collect::<Vec<_>>());
                }
            }
        }
    }

    #[test]
    fn multi_seed_search_matches_independent_relaxation_and_exact_legal_witnesses() {
        let mut rng = 41u64;
        let mut random = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            rng >> 32
        };
        let mut workspace = Workspace::default();
        for trial in 0..32 {
            let nodes = 8;
            let mut edges = Vec::new();
            for from in 0..nodes {
                for to in 0..nodes {
                    if random() % 4 == 0 {
                        edges.push((from as u32, to as u32));
                    }
                }
            }
            let g = graph(nodes, &edges);
            let roads: Vec<_> =
                (0..nodes).map(|n| if n == 1 { u32::MAX as u64 + 7 } else { random() % 13 + 1 }).collect();
            let turns: Vec<_> =
                edges.iter().map(|_| if random() % 7 == 0 { u64::MAX } else { random() % 11 }).collect();
            let costs = Costs { roads: Numbers::U64(roads.clone()), turns: Numbers::U64(turns.clone()) };
            let columns: Vec<std::sync::Arc<Vec<u16>>> = (0..6)
                .map(|landmark| {
                    let mut distances = vec![u16::MAX; nodes];
                    distances[landmark] = 0;
                    for _ in 1..nodes {
                        let old = distances.clone();
                        for &(from, to) in &edges {
                            distances[from as usize] = distances[from as usize]
                                .min((roads[to as usize] / 8 + old[to as usize] as u64).min(u16::MAX as u64) as u16);
                        }
                    }
                    std::sync::Arc::new(distances)
                })
                .collect();
            for case in 0..16 {
                let starts = [
                    Seed { node: (case % nodes) as u32, cost: 7, road: (case % nodes) as u32, choice: 3 },
                    Seed { node: ((case + 2) % nodes) as u32, cost: 11, road: ((case + 2) % nodes) as u32, choice: 1 },
                ];
                let ends = [
                    Seed { node: ((case + trial) % nodes) as u32, cost: 17, road: 99, choice: 2 },
                    Seed { node: ((case + trial + 4) % nodes) as u32, cost: 0, road: 98, choice: 0 },
                ];
                let mut distances = vec![(u64::MAX, u8::MAX); nodes];
                for s in &starts {
                    distances[s.node as usize] = distances[s.node as usize].min((s.cost, s.choice));
                }
                for _ in 0..nodes - 1 {
                    let old = distances.clone();
                    for (a, &(from, to)) in edges.iter().enumerate() {
                        if old[from as usize].0 != u64::MAX && turns[a] != u64::MAX {
                            distances[to as usize] = distances[to as usize]
                                .min((old[from as usize].0 + roads[to as usize] + turns[a], old[from as usize].1));
                        }
                    }
                }
                let expected = ends
                    .iter()
                    .filter(|end| distances[end.node as usize].0 != u64::MAX)
                    .map(|end| (distances[end.node as usize].0 + end.cost, end.choice, distances[end.node as usize].1))
                    .min();
                let selected =
                    crate::landmarks::select(columns.len(), 8, &starts, &ends, |i, node| Ok(columns[i][node as usize]))
                        .unwrap();
                let prepared = crate::landmarks::Prepared {
                    mapping: std::sync::Arc::new((0..nodes as u32).collect()),
                    columns: selected
                        .into_iter()
                        .map(|(i, from, to)| (std::sync::Arc::clone(&columns[i]), from, to))
                        .collect(),
                    scale: 8,
                };
                for (arc, &(from, to)) in edges.iter().enumerate() {
                    let weight = costs.arc(&g, arc);
                    if weight != u64::MAX {
                        assert!(prepared.get(from) - prepared.get(to) <= 2 * weight as i128);
                    }
                }
                let mut heuristic = |node| Ok(prepared.get(node));
                for guided in [false, true] {
                    let result = workspace
                        .run_with_potential(
                            &g,
                            &costs,
                            Query {
                                starts: &starts,
                                ends: &ends,
                                ceiling: u64::MAX,
                                max_labels: 1000,
                                max_roads: 1000,
                                heap_bytes: 1024 * 1024,
                                cancelled: &|| false,
                            },
                            guided.then_some(&mut heuristic),
                        )
                        .unwrap();
                    assert_eq!(
                        result.as_ref().map(|r| (r.cost, ends[r.target].choice, starts[r.source].choice)),
                        expected
                    );
                    if let Some(result) = result {
                        assert_eq!(result.roads[0], starts[result.source].road);
                        assert_eq!(result.roads.last().copied(), Some(ends[result.target].node));
                        let mut total = starts[result.source].cost + ends[result.target].cost;
                        for pair in result.roads.windows(2) {
                            let arc = edges.binary_search(&(pair[0], pair[1])).unwrap();
                            assert_ne!(turns[arc], u64::MAX);
                            total += roads[pair[1] as usize] + turns[arc];
                        }
                        assert_eq!(total, result.cost);
                    }
                }
            }
        }
        let g = graph(2, &[(0, 1)]);
        let costs = Costs { roads: Numbers::U8(vec![1, 1]), turns: Numbers::U8(vec![0]) };
        let starts = [Seed { node: 0, cost: 0, road: 0, choice: 0 }];
        let ends = [Seed { node: 1, cost: 0, road: 1, choice: 0 }];
        assert!(matches!(
            workspace.run(
                &g,
                &costs,
                Query {
                    starts: &starts,
                    ends: &ends,
                    ceiling: u64::MAX,
                    max_labels: 1,
                    max_roads: 100,
                    heap_bytes: 1024,
                    cancelled: &|| false
                }
            ),
            Err(Error::Limit)
        ));
        assert!(matches!(
            workspace.run(
                &g,
                &costs,
                Query {
                    starts: &starts,
                    ends: &ends,
                    ceiling: u64::MAX,
                    max_labels: 100,
                    max_roads: 100,
                    heap_bytes: 1024,
                    cancelled: &|| true
                }
            ),
            Err(Error::Cancelled)
        ));
        workspace.clear_heaps();
        assert!(matches!(
            workspace.run(
                &g,
                &costs,
                Query {
                    starts: &starts,
                    ends: &ends,
                    ceiling: u64::MAX,
                    max_labels: 100,
                    max_roads: 100,
                    heap_bytes: 0,
                    cancelled: &|| false
                }
            ),
            Err(Error::Limit)
        ));
        let g = graph(1, &[]);
        let costs = Costs { roads: Numbers::U8(vec![1]), turns: Numbers::U8(vec![]) };
        let ends = [Seed { node: 0, cost: 3, road: 0, choice: 0 }, Seed { node: 0, cost: 1, road: 0, choice: 1 }];
        let result = workspace
            .run(
                &g,
                &costs,
                Query {
                    starts: &starts,
                    ends: &ends,
                    ceiling: u64::MAX,
                    max_labels: 2,
                    max_roads: 100,
                    heap_bytes: 1024,
                    cancelled: &|| false,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!((result.cost, result.target), (1, 1));
    }
}
