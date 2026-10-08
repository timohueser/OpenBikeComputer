//! Bake-time connectivity of each metric's road-state graph. A point snaps only to a road in a
//! large strongly connected component, so a leg without a path means its points are truly
//! disconnected: no retry with other roads could join them.

/// A road is snappable only in a strongly connected component of at least this many road
/// states, or in the metric's largest component. A smaller component is a fragment: a fenced
/// estate, a one-way that ends in a car park, a mapping gap. 100 states are about 50 two-way
/// segments, 1 to 2 km of road, below the smallest village network; GraphHopper drops subnetworks
/// below 200 edges. The largest component always stays, so a small test or island package keeps
/// its roads.
pub const MINIMUM_COMPONENT: usize = 100;

/// The shared state topology: arcs by source road, and the same arcs by destination road.
pub struct States {
    first: Vec<u32>,
    head: Vec<u32>,
    reverse_first: Vec<u32>,
    reverse_arcs: Vec<u32>,
}

impl States {
    /// `edges` are sorted by source road; an arc index is a position in `edges`.
    pub fn new(roads: usize, edges: &[(u32, u32)]) -> Self {
        let mut first = vec![0u32; roads + 1];
        let mut reverse_first = vec![0u32; roads + 1];
        for &(from, to) in edges {
            first[from as usize + 1] += 1;
            reverse_first[to as usize + 1] += 1;
        }
        for road in 0..roads {
            first[road + 1] += first[road];
            reverse_first[road + 1] += reverse_first[road];
        }
        let mut cursor = reverse_first.clone();
        let mut reverse_arcs = vec![0u32; edges.len()];
        for (arc, &(_, to)) in edges.iter().enumerate() {
            reverse_arcs[cursor[to as usize] as usize] = arc as u32;
            cursor[to as usize] += 1;
        }
        Self { first, head: edges.iter().map(|&(_, to)| to).collect(), reverse_first, reverse_arcs }
    }

    fn tail(&self, arc: u32) -> usize {
        self.first.partition_point(|&start| start <= arc) - 1
    }

    /// One snap bit per road: the road has a cost and lies in the metric's largest component or
    /// in one of at least `MINIMUM_COMPONENT` states. A transition counts where its turn is legal.
    pub fn snappable(&self, road_costs: &[u64], turns: &[u64]) -> Vec<u64> {
        let roads = road_costs.len();
        let open = |arc: usize, from: usize, to: usize| {
            turns[arc] != u64::MAX && road_costs[from] != u64::MAX && road_costs[to] != u64::MAX
        };
        // Kosaraju: finishing order on the forward graph, then components on the reverse graph.
        let mut seen = vec![false; roads];
        let mut order = Vec::with_capacity(roads);
        let mut stack: Vec<(u32, u32)> = Vec::new();
        for root in 0..roads {
            if seen[root] || road_costs[root] == u64::MAX {
                continue;
            }
            seen[root] = true;
            stack.push((root as u32, self.first[root]));
            while let Some(&(node, arc)) = stack.last() {
                if arc == self.first[node as usize + 1] {
                    stack.pop();
                    order.push(node);
                    continue;
                }
                stack.last_mut().unwrap().1 += 1;
                let to = self.head[arc as usize] as usize;
                if !seen[to] && open(arc as usize, node as usize, to) {
                    seen[to] = true;
                    stack.push((to as u32, self.first[to]));
                }
            }
        }
        seen.fill(false);
        let mut component = vec![u32::MAX; roads];
        let mut sizes: Vec<usize> = Vec::new();
        let mut pending = Vec::new();
        for root in order.into_iter().rev() {
            if seen[root as usize] {
                continue;
            }
            let id = sizes.len() as u32;
            sizes.push(0);
            seen[root as usize] = true;
            pending.push(root);
            while let Some(node) = pending.pop() {
                component[node as usize] = id;
                sizes[id as usize] += 1;
                for index in self.reverse_first[node as usize]..self.reverse_first[node as usize + 1] {
                    let arc = self.reverse_arcs[index as usize];
                    let from = self.tail(arc);
                    if !seen[from] && open(arc as usize, from, node as usize) {
                        seen[from] = true;
                        pending.push(from as u32);
                    }
                }
            }
        }
        let largest = sizes.iter().copied().max().unwrap_or(0);
        let mut words = vec![0u64; roads.div_ceil(64)];
        for (road, &id) in component.iter().enumerate() {
            if id != u32::MAX && (sizes[id as usize] == largest || sizes[id as usize] >= MINIMUM_COMPONENT) {
                words[road / 64] |= 1 << (road % 64);
            }
        }
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_and_one_way_stubs_lose_their_snap_bit_but_the_largest_component_keeps_it() {
        // Roads 0..4 form a two-way pair plus a cycle, 4 is a one-way stub leaving it, 5 and 6
        // are a two-way pair that only a forbidden turn joins to the rest, 7 has no cost.
        let edges = [(0, 1), (0, 4), (1, 0), (1, 2), (2, 3), (3, 1), (4, 5), (5, 6), (6, 5), (6, 7), (7, 0)];
        let states = States::new(8, &edges);
        let costs = [1, 1, 1, 1, 1, 1, 1, u64::MAX];
        let mut turns = vec![0u64; edges.len()];
        turns[6] = u64::MAX;
        let words = states.snappable(&costs, &turns);
        let bits: Vec<bool> = (0..8).map(|road| words[0] & (1 << road) != 0).collect();
        assert_eq!(bits, [true, true, true, true, false, false, false, false]);
        turns[6] = 0;
        let costs = [1, 1, 1, 1, 1, 1, 1, 1];
        assert_eq!(states.snappable(&costs, &turns), vec![0xff]);
    }

    #[test]
    fn an_island_of_the_minimum_size_keeps_its_snap_bit_beside_a_larger_mainland() {
        // Three cycles that nothing joins: the minimum size, twice that, and a pair.
        let mut edges = Vec::new();
        let mut start = 0u32;
        for size in [MINIMUM_COMPONENT as u32, 2 * MINIMUM_COMPONENT as u32, 2] {
            edges.extend((0..size).map(|i| (start + i, start + (i + 1) % size)));
            start += size;
        }
        edges.sort_unstable();
        let roads = start as usize;
        let words = States::new(roads, &edges).snappable(&vec![1; roads], &vec![0; edges.len()]);
        let bit = |road: usize| words[road / 64] & (1 << (road % 64)) != 0;
        assert!((0..3 * MINIMUM_COMPONENT).all(bit));
        assert!(!(3 * MINIMUM_COMPONENT..roads).any(bit));
    }
}
