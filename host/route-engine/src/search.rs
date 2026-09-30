use crate::{
    base::{Costs, Graph},
    Error, Result,
};
use std::{cmp::Reverse, collections::BinaryHeap, mem::size_of};

#[derive(Default)]
struct Labels {
    costs: Vec<u64>,
    parents: Vec<u32>,
    origins: Vec<u8>,
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
        } else {
            self.costs.fill(u64::MAX);
            self.parents.fill(u32::MAX);
            self.origins.fill(u8::MAX);
        }
        self.count = 0;
        Ok(())
    }
    fn set(&mut self, node: u32, cost: u64, parent: u32, origin: u8) {
        let n = node as usize;
        if self.costs[n] == u64::MAX {
            self.count += 1;
        }
        self.costs[n] = cost;
        self.parents[n] = parent;
        self.origins[n] = origin;
    }
}

#[derive(Default)]
pub struct Workspace {
    labels: [Labels; 2],
    heaps: [BinaryHeap<Reverse<(u64, u8, u32)>>; 2],
}

pub struct Seed {
    pub node: u32,
    pub cost: u64,
    pub road: u32,
    pub choice: u8,
}

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

    fn push(&mut self, side: usize, value: (u64, u8, u32), budget: usize) -> Result<()> {
        let heap = &self.heaps[side];
        if heap.len() == heap.capacity() {
            let wanted = heap.capacity().saturating_mul(2).max(4);
            // Reserve also accounts for a temporary old allocation during growth.
            let peak = wanted
                .saturating_add(self.heaps.iter().map(|h| h.capacity()).sum::<usize>())
                .saturating_mul(size_of::<Reverse<(u64, u8, u32)>>());
            if peak > budget {
                return Err(Error::Limit);
            }
            self.heaps[side].try_reserve_exact(wanted - heap.len()).map_err(|_| Error::Limit)?;
        }
        self.heaps[side].push(Reverse(value));
        Ok(())
    }

    pub fn run(&mut self, graph: &Graph, costs: &Costs, query: Query<'_>) -> Result<Option<Found>> {
        let Query { starts, ends, ceiling, max_labels, max_roads, heap_bytes, cancelled } = query;
        if starts.is_empty() || ends.is_empty() {
            return Ok(None);
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
                    self.push(side, (seed.cost, seed.choice, seed.node), heap_bytes)?;
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
                while let Some(&Reverse((cost, origin, node))) = self.heaps[side].peek() {
                    if self.labels[side].costs[node as usize] == cost
                        && self.labels[side].origins[node as usize] == origin
                    {
                        break;
                    }
                    self.heaps[side].pop();
                }
            }
            let top: [u64; 2] = std::array::from_fn(|i| self.heaps[i].peek().map_or(u64::MAX, |v| v.0 .0));
            if top[0] == u64::MAX || top[1] == u64::MAX || top[0].saturating_add(top[1]) > best {
                break;
            }
            let side = usize::from(top[1] < top[0]);
            let Reverse((cost, origin, node)) = self.heaps[side].pop().unwrap();
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
                self.push(side, (next, origin, to), heap_bytes)?;
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
                let result = workspace
                    .run(
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
                    )
                    .unwrap();
                assert_eq!(result.as_ref().map(|r| (r.cost, ends[r.target].choice, starts[r.source].choice)), expected);
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
