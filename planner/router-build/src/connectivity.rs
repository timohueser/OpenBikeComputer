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

use crate::base::{group, Topology};
use crate::column::Column;
use crate::Roads;

/// The shared topology, and the roads that arrive at each junction.
pub struct States<'a> {
    topology: &'a Topology,
    from: &'a [u32],
    /// The roads that arrive at junction `j` are `arrivals[arriving[j]..arriving[j + 1]]`, in road
    /// order.
    arriving: Column<u32>,
    arrivals: Column<u32>,
    /// The place of each road among the roads that depart from its junction: the arc from a road
    /// that arrives there to this road is `first[that road] + place`.
    place: Column<u32>,
}

impl<'a> States<'a> {
    /// `topology` is the topology of `roads`, whose junctions are below `points`.
    pub fn new(topology: &'a Topology, roads: &'a Roads, points: usize) -> Self {
        let (arriving, arrivals) = group(roads.to.values(), points);
        let (departing, departures) = group(roads.from.values(), points);
        let (departing, departures) = (departing.values(), departures.values());
        let mut place = Column::filled(roads.len(), 0);
        let places = place.values_mut();
        for junction in 0..points {
            let range = departing[junction] as usize..departing[junction + 1] as usize;
            for (place, &road) in (0u32..).zip(&departures[range]) {
                places[road as usize] = place;
            }
        }
        Self { topology, from: roads.from.values(), arriving, arrivals, place }
    }

    /// One snap bit per road: the road has a cost and lies in the metric's largest component or
    /// in one of at least `MINIMUM_COMPONENT` states. A transition counts where its turn is legal.
    pub fn snappable(&self, road_costs: &[u32], turns: &[u32]) -> Column<u64> {
        let roads = road_costs.len();
        let (first, head) = (self.topology.first.values(), self.topology.head.values());
        let (arriving, arrivals, place) = (self.arriving.values(), self.arrivals.values(), self.place.values());
        let open = |arc: usize, from: usize, to: usize| {
            turns[arc] != u32::MAX && road_costs[from] != u32::MAX && road_costs[to] != u32::MAX
        };
        // Kosaraju: finishing order on the forward graph, then components on the reverse graph.
        let mut seen = Column::filled(roads, false);
        let seen = seen.values_mut();
        let mut order = Column::default();
        let mut stack = Column::<(u32, u32)>::default();
        for root in 0..roads {
            if seen[root] || road_costs[root] == u32::MAX {
                continue;
            }
            seen[root] = true;
            stack.push((root as u32, first[root]));
            while let Some(&(node, arc)) = stack.values().last() {
                if arc == first[node as usize + 1] {
                    stack.pop();
                    order.push(node);
                    continue;
                }
                stack.values_mut().last_mut().unwrap().1 += 1;
                let to = head[arc as usize] as usize;
                if !seen[to] && open(arc as usize, node as usize, to) {
                    seen[to] = true;
                    stack.push((to as u32, first[to]));
                }
            }
        }
        seen.fill(false);
        let mut component = Column::filled(roads, u32::MAX);
        let component = component.values_mut();
        let mut sizes = Column::<u32>::default();
        let mut pending = Column::default();
        for &root in order.values().iter().rev() {
            if seen[root as usize] {
                continue;
            }
            let id = sizes.len() as u32;
            sizes.push(0);
            seen[root as usize] = true;
            pending.push(root);
            while let Some(node) = pending.pop() {
                component[node as usize] = id;
                sizes.values_mut()[id as usize] += 1;
                let junction = self.from[node as usize] as usize;
                for &from in &arrivals[arriving[junction] as usize..arriving[junction + 1] as usize] {
                    let arc = first[from as usize] + place[node as usize];
                    if !seen[from as usize] && open(arc as usize, from as usize, node as usize) {
                        seen[from as usize] = true;
                        pending.push(from);
                    }
                }
            }
        }
        let sizes = sizes.values();
        let largest = sizes.iter().copied().max().unwrap_or(0);
        let mut words = Column::filled(roads.div_ceil(64), 0u64);
        let words_mut = words.values_mut();
        for (road, &id) in component.iter().enumerate() {
            let size = if id == u32::MAX { 0 } else { sizes[id as usize] };
            if size != 0 && (size == largest || size as usize >= MINIMUM_COMPONENT) {
                words_mut[road / 64] |= 1 << (road % 64);
            }
        }
        words
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use planner_router::model::Road;

    /// The roads from junction to junction of `ends`, and their topology.
    fn graph(ends: &[(u32, u32)]) -> (Roads, Topology, usize) {
        let roads: Roads = ends.iter().map(|&(from, to)| Road { from, to, ..Road::default() }).collect();
        let points = ends.iter().map(|&(from, to)| from.max(to) as usize + 1).max().unwrap_or(0);
        let topology = Topology::new(&roads, points).unwrap();
        (roads, topology, points)
    }

    #[test]
    fn fragments_and_one_way_stubs_lose_their_snap_bit_but_the_largest_component_keeps_it() {
        // Roads 0 and 1 are a two-way pair and 2 and 3 close a cycle with 0. Road 4 is a one-way
        // stub that leaves it, 5 and 6 are a two-way pair that only a forbidden turn from 4 joins
        // to the rest, and 7 has no cost.
        let (roads, topology, points) = graph(&[(0, 1), (1, 0), (1, 2), (2, 0), (2, 3), (3, 4), (4, 3), (4, 0)]);
        let states = States::new(&topology, &roads, points);
        let costs = [1, 1, 1, 1, 1, 1, 1, u32::MAX];
        let mut turns = vec![0u32; topology.head.len()];
        let stub = topology.edges().position(|edge| edge == (4, 5)).unwrap();
        turns[stub] = u32::MAX;
        let words = states.snappable(&costs, &turns);
        let bits: Vec<bool> = (0..8).map(|road| words.values()[0] & (1 << road) != 0).collect();
        assert_eq!(bits, [true, true, true, true, false, false, false, false]);
        turns[stub] = 0;
        let costs = [1, 1, 1, 1, 1, 1, 1, 1];
        assert_eq!(states.snappable(&costs, &turns).values(), [0xff]);
    }

    #[test]
    fn an_island_of_the_minimum_size_keeps_its_snap_bit_beside_a_larger_mainland() {
        // Three one-way rings that nothing joins: the minimum size, twice that, and a pair.
        let mut ends = Vec::new();
        let mut start = 0u32;
        for size in [MINIMUM_COMPONENT as u32, 2 * MINIMUM_COMPONENT as u32, 2] {
            ends.extend((0..size).map(|i| (start + i, start + (i + 1) % size)));
            start += size;
        }
        let (roads, topology, points) = graph(&ends);
        let words =
            States::new(&topology, &roads, points).snappable(&vec![1; roads.len()], &vec![0; topology.head.len()]);
        let bit = |road: usize| words.values()[road / 64] & (1 << (road % 64)) != 0;
        assert!((0..3 * MINIMUM_COMPONENT).all(bit));
        assert!(!(3 * MINIMUM_COMPONENT..roads.len()).any(bit));
    }
}
