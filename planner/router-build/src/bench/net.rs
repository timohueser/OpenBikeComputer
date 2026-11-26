//! The graph of a region with the costs of one profile, and the two searches of the route bench.
use crate::base::{group, Topology};
use crate::column::Column;
use crate::source::Tags;
use crate::{Costs, Graph};
use planner_router::base::{Costs as Weights, Graph as States, Numbers};
use planner_router::model::Profile;
use planner_router::search::{Query, Seed, Workspace};
use planner_router::shape::P;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Net<'a> {
    pub graph: &'a Graph,
    pub profile: &'a Profile,
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
    states: &'a States,
    /// The costs of the bake, then the length of each road as its cost.
    weights: [Weights; 2],
}

/// Whether `tags` are those of a signed route of the activity of `profile`: a hiking route for a
/// walking profile, else a cycle route.
pub fn signed(profile: &Profile, tags: &Tags) -> bool {
    let routes: &[&str] = if profile.walking { &["hiking", "foot"] } else { &["bicycle"] };
    tags.get("route").is_some_and(|route| routes.contains(&route))
        && matches!(tags.get("type"), Some("route" | "superroute"))
        && !tags.contains("state", "proposed")
}

/// The state graph of the search, with the arcs of `first` and `head` by their end too. One graph
/// serves each profile.
pub fn states(first: &[u32], head: &[u32]) -> States {
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
    States {
        first: first.to_vec(),
        head: head.to_vec(),
        reverse_first,
        reverse_tail,
        reverse_offsets: Numbers::U32(offsets),
    }
}

impl<'a> Net<'a> {
    /// `costs` holds the costs of `graph` on `topology`, and those of its profile `index`. `states`
    /// is the state graph of `topology`.
    pub fn new(graph: &'a Graph, topology: &'a Topology, states: &'a States, costs: &'a Costs, index: usize) -> Self {
        let (roads, profile) = (&graph.roads, &costs.profiles[index]);
        let (totals, turns) = (costs.roads[index].totals.values(), costs.roads[index].turns.values());
        let lengths = roads.length_m.values().iter().zip(totals);
        let lengths = lengths.map(|(&length, &total)| if total == u32::MAX { u32::MAX } else { length }).collect();
        let legal = turns.iter().map(|&turn| if turn == u32::MAX { u32::MAX } else { 0 }).collect();
        let weights = |roads, turns| Weights { roads: Numbers::U32(roads), turns: Arc::new(Numbers::U32(turns)) };
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
            topology,
            totals,
            turns,
            snap: costs.snap[index].values(),
            arrivals: group(roads.to.values(), graph.points.len()),
            ends,
            routes,
            states,
            weights: [weights(totals.to_vec(), turns.to_vec()), weights(lengths, legal)],
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
    pub fn route(&self, from: u32, to: u32) -> Result<Option<Vec<u32>>, String> {
        self.search(&self.weights[0], from, to)
    }

    /// `route` with the length as the cost, on the roads and turns that the profile may take.
    pub fn shortest(&self, from: u32, to: u32) -> Result<Option<Vec<u32>>, String> {
        self.search(&self.weights[1], from, to)
    }

    fn search(&self, weights: &Weights, from: u32, to: u32) -> Result<Option<Vec<u32>>, String> {
        let seeds = |junction: u32| -> Vec<Seed> {
            let arriving = self.arriving(junction).iter().filter(|&&road| self.allowed(road));
            // A label keeps the choice of its seed in a byte, and 255 is no choice.
            arriving.take(255).enumerate().map(|(i, &road)| Seed { node: road, cost: 0, choice: i as u8 }).collect()
        };
        let (starts, ends) = (seeds(from), seeds(to));
        let query = Query {
            starts: &starts,
            ends: &ends,
            ceiling: u64::MAX,
            max_roads: usize::MAX,
            heap_bytes: 1 << 30,
            heap_limit: 1 << 30,
            cancelled: &|| false,
        };
        let found = Workspace::default().run(self.states, weights, query).map_err(|e| e.to_string())?;
        Ok(found.map(|found| found.roads[1..].to_vec()))
    }
}
