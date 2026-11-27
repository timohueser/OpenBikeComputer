//! The graph of a region with the costs of one profile, and the two searches of the route bench.
use crate::base::{group, Topology};
use crate::column::Column;
use crate::junction::{self, Junctions};
use crate::source::Tags;
use crate::{Costs, Graph};
use planner_router::model::Profile;
use planner_router::shape::P;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

pub struct Net<'a> {
    pub graph: &'a Graph,
    pub profile: &'a Profile,
    pub junctions: &'a Junctions,
    topology: &'a Topology,
    totals: &'a [u32],
    turns: &'a [u32],
    snap: &'a [u64],
    /// The roads that arrive at each junction (`base::group`).
    arrivals: (Column<u32>, Column<u32>),
    /// The road ends at each junction; the two directions of a piece of a way are one end.
    pub ends: Vec<u8>,
    /// The signed routes of the profile that list each way row.
    pub routes: HashMap<u32, Vec<i64>>,
    /// For each road its length as its cost, and for each turn 0, or `u32::MAX` where the profile
    /// may not take it.
    lengths: (Vec<u32>, Vec<u32>),
}

/// Whether `tags` are those of a signed route of the activity of `profile`: a hiking route for a
/// walking profile, else a cycle route.
pub fn signed(profile: &Profile, tags: &Tags) -> bool {
    junction::signed(tags, if profile.walking { &["hiking", "foot"] } else { &["bicycle"] })
}

impl<'a> Net<'a> {
    /// `costs` holds the costs of `graph` on `topology`, and those of its profile `index`;
    /// `junctions` are those of `graph`.
    pub fn new(
        graph: &'a Graph,
        topology: &'a Topology,
        junctions: &'a Junctions,
        costs: &'a Costs,
        index: usize,
    ) -> Self {
        let (roads, profile) = (&graph.roads, &costs.profiles[index]);
        let (totals, turns) = (costs.roads[index].totals.values(), costs.roads[index].turns.values());
        let lengths = roads.length_m.values().iter().zip(totals);
        let lengths = lengths.map(|(&length, &total)| if total == u32::MAX { u32::MAX } else { length }).collect();
        let legal = turns.iter().map(|&turn| if turn == u32::MAX { u32::MAX } else { 0 }).collect();
        let mut pieces: Vec<_> = (0..roads.len())
            .flat_map(|road| {
                let piece = (roads.way_row(road), roads.length_m.values()[road]);
                [(roads.from(road), piece, roads.to(road)), (roads.to(road), piece, roads.from(road))]
            })
            .collect();
        pieces.sort_unstable();
        pieces.dedup();
        let mut ends = vec![0u8; graph.points.len()];
        for (junction, ..) in pieces {
            ends[junction as usize] = ends[junction as usize].saturating_add(1);
        }
        let mut routes = HashMap::<u32, Vec<i64>>::new();
        for relation in graph.osm.relations.iter().filter(|relation| signed(profile, &relation.tags)) {
            for row in relation.members().filter_map(|(member, _)| graph.osm.ways.index(member.way()?)) {
                routes.entry(row as u32).or_default().push(relation.id);
            }
        }
        Net {
            graph,
            profile,
            junctions,
            topology,
            totals,
            turns,
            snap: costs.snap[index].values(),
            arrivals: group(roads.to.values(), graph.points.len()),
            ends,
            routes,
            lengths: (lengths, legal),
        }
    }

    pub fn from(&self, road: u32) -> u32 {
        self.graph.roads.from(road as usize)
    }

    pub fn to(&self, road: u32) -> u32 {
        self.graph.roads.to(road as usize)
    }

    pub fn length(&self, road: u32) -> f64 {
        f64::from(self.graph.roads.length_m.values()[road as usize])
    }

    /// Whether the profile may use the road.
    pub fn allowed(&self, road: u32) -> bool {
        self.totals[road as usize] != u32::MAX
    }

    /// Whether the road has the snap bit of the profile.
    pub fn snappable(&self, road: u32) -> bool {
        self.snap[road as usize / 64] & (1 << (road % 64)) != 0
    }

    /// Whether the profile may turn from road `before` onto road `after`.
    pub fn permits(&self, before: u32, after: u32) -> bool {
        let (first, head) = (self.topology.first.values(), self.topology.head.values());
        let arcs = first[before as usize] as usize..first[before as usize + 1] as usize;
        head[arcs.clone()].binary_search(&after).is_ok_and(|arc| self.turns[arcs.start + arc] != u32::MAX)
    }

    /// The roads that arrive at `junction`.
    pub fn arriving(&self, junction: u32) -> &[u32] {
        let start = self.arrivals.0.values();
        &self.arrivals.1.values()[start[junction as usize] as usize..start[junction as usize + 1] as usize]
    }

    /// The line of `roads`, in microdegrees of longitude and latitude.
    pub fn line(&self, roads: &[u32]) -> Vec<P> {
        let mut line = Vec::new();
        for &road in roads {
            // A road starts at the last point of the road before it.
            let shape = &self.graph.roads.shape(road as usize)[usize::from(!line.is_empty())..];
            line.extend(shape.iter().map(|point| [point.lon, point.lat]));
        }
        line
    }

    /// The roads of the cheapest route from junction `from` to junction `to`, or `None` where
    /// there is none. The search runs from each road that arrives at `from` to each road that
    /// arrives at `to`; the arrival road at `from` is not part of the route. The production
    /// router snaps a point to a place on a road, so its route can differ at the two ends.
    pub fn route(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        self.search(self.totals, self.turns, from, to)
    }

    /// `route` with the length as the cost, on the roads and turns that the profile may take.
    pub fn shortest(&self, from: u32, to: u32) -> Option<Vec<u32>> {
        self.search(&self.lengths.0, &self.lengths.1, from, to)
    }

    /// An exact search over the roads: a road costs its entry of `totals` and the turn onto it its
    /// entry of `turns`, and `u32::MAX` closes either.
    fn search(&self, totals: &[u32], turns: &[u32], from: u32, to: u32) -> Option<Vec<u32>> {
        let (first, head) = (self.topology.first.values(), self.topology.head.values());
        let mut cost = vec![u64::MAX; totals.len()];
        let mut before = vec![u32::MAX; totals.len()];
        let mut heap = BinaryHeap::new();
        for &road in self.arriving(from).iter().filter(|&&road| self.allowed(road)) {
            cost[road as usize] = 0;
            heap.push(Reverse((0u64, road)));
        }
        while let Some(Reverse((here, road))) = heap.pop() {
            if here > cost[road as usize] {
                continue;
            }
            if self.to(road) == to {
                let mut roads = vec![road];
                while before[*roads.last()? as usize] != u32::MAX {
                    roads.push(before[*roads.last()? as usize]);
                }
                roads.pop();
                roads.reverse();
                return Some(roads);
            }
            let arcs = first[road as usize] as usize..first[road as usize + 1] as usize;
            for (arc, &next) in arcs.clone().zip(&head[arcs]) {
                let (turn, total) = (turns[arc], totals[next as usize]);
                if turn == u32::MAX || total == u32::MAX {
                    continue;
                }
                let there = here + u64::from(turn) + u64::from(total);
                if there < cost[next as usize] {
                    cost[next as usize] = there;
                    before[next as usize] = road;
                    heap.push(Reverse((there, next)));
                }
            }
        }
        None
    }
}
