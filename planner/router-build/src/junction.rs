//! The rules of a junction that the costs of each cycling profile and the route bench share: the
//! heading at a junction, the through road, the signal groups and the cost of a turn. Each number
//! comes from the prototype records of the route-quality tracker.
use crate::{base::Topology, source::Tags, Graph, Roads};
use planner_router::{geometry::METRES_PER_UDEG, model::Point};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// A turn off the through road costs this times (1 − cosine of the change of heading), so a route
/// leaves its road at a right angle where that saves about 100 m. 150 m gained little more.
pub const TURN_M: f64 = 100.0;
/// A signalled junction costs about one stop. 120 m made the routes in a town 3 % longer.
pub const SIGNAL_M: f64 = 60.0;
/// Signal nodes that roads of at most this length in total join are one junction. No group was
/// over 200 m across with it, against six with 60 m.
pub const GROUP_M: u32 = 40;
/// The length of road before and after a junction that gives the heading there.
pub const HEADING_M: f64 = 15.0;

/// The cycle routes and the signal groups of a graph, for all profiles. It holds no table of the
/// size of all roads or all junctions.
pub struct Junctions {
    /// (way row, relation ID) for each way that a cycle route lists as its own member, sorted.
    routes: Vec<(u32, i64)>,
    /// (junction, group) for each junction of a signal group, sorted. A group is named by its
    /// first signal node.
    groups: Vec<(u32, u32)>,
}

impl Junctions {
    pub fn new(graph: &Graph) -> Self {
        let ways = &graph.osm.ways;
        let relations = graph.osm.relations.iter().filter(|relation| signed(&relation.tags, &["bicycle"]));
        // A way of a route inside a superroute belongs to that route only.
        let mut routes: Vec<(u32, i64)> = relations
            .flat_map(|relation| {
                let id = relation.id;
                relation.members().filter_map(move |(member, _)| Some((ways.index(member.way()?)? as u32, id)))
            })
            .collect();
        routes.sort_unstable();
        routes.dedup();
        Junctions { routes, groups: groups(graph) }
    }

    /// The bytes of the tables.
    pub fn bytes(&self) -> u64 {
        (size_of_val(self.routes.as_slice()) + size_of_val(self.groups.as_slice())) as u64
    }

    fn group(&self, junction: u32) -> Option<u32> {
        let at = self.groups.binary_search_by_key(&junction, |&(junction, _)| junction).ok()?;
        Some(self.groups[at].1)
    }

    /// Whether `road` arrives at a junction of a signal group and starts outside that group.
    pub fn enters(&self, graph: &Graph, road: u32) -> bool {
        let roads = &graph.roads;
        self.group(roads.to(road as usize)).is_some_and(|group| self.group(roads.from(road as usize)) != Some(group))
    }

    fn routes(&self, row: u32) -> &[(u32, i64)] {
        &self.routes[self.routes.partition_point(|&(r, _)| r < row)..self.routes.partition_point(|&(r, _)| r <= row)]
    }

    /// Whether `after` continues the road of `before`: it is not the road back, and the two are
    /// roads of one way, or their ways have the same name or share a cycle route.
    pub fn through(&self, graph: &Graph, before: u32, after: u32) -> bool {
        let roads = &graph.roads;
        let (a, b) = (roads.way_row(before as usize), roads.way_row(after as usize));
        if back(roads, before, after) {
            return false;
        }
        if a == b {
            return true;
        }
        let (wa, wb) = (graph.osm.ways.row(a as usize), graph.osm.ways.row(b as usize));
        let name = wa.tags.get("name").filter(|name| !name.is_empty());
        let routed = wa.facts.ranks != [0, 0] && wb.facts.ranks != [0, 0];
        name.is_some() && name == wb.tags.get("name")
            || routed && self.routes(a).iter().any(|&(_, id)| self.routes(b).iter().any(|&(_, other)| other == id))
    }

    /// The cost in metres of the turn from road `before` onto road `after` for a cycling profile,
    /// where `onward` roads depart where `before` arrives (`onward`).
    pub fn cost(&self, graph: &Graph, before: u32, after: u32, onward: usize) -> u64 {
        let signal = if self.enters(graph, before) { SIGNAL_M } else { 0.0 };
        let choice = onward >= 2 && !self.through(graph, before, after);
        let turn = if choice { TURN_M * (1.0 - cosine(graph, before, after)) } else { 0.0 };
        (signal + turn).round() as u64
    }
}

/// Whether `tags` are those of a signed route whose `route` is one of `kinds`: `type=route` or
/// `superroute`, not proposed.
pub fn signed(tags: &Tags, kinds: &[&str]) -> bool {
    tags.get("route").is_some_and(|route| kinds.contains(&route))
        && matches!(tags.get("type"), Some("route" | "superroute"))
        && !tags.contains("state", "proposed")
}

/// Whether `after` is the road back from `before`: the other direction of the same piece of a way.
pub fn back(roads: &Roads, before: u32, after: u32) -> bool {
    let (a, b) = (before as usize, after as usize);
    roads.way_row(a) == roads.way_row(b) && roads.from(a) == roads.to(b) && roads.to(a) == roads.from(b)
}

/// The roads of each mode that depart where `before` arrives, without the road back.
pub fn onward(graph: &Graph, topology: &Topology, before: u32) -> usize {
    let (first, head) = (topology.first.values(), topology.head.values());
    let arcs = first[before as usize] as usize..first[before as usize + 1] as usize;
    head[arcs].iter().filter(|&&after| !back(&graph.roads, before, after)).count()
}

/// The cosine of the change of heading from the last `HEADING_M` of road `before` to the first
/// `HEADING_M` of road `after`; 1 where a heading has no length.
pub fn cosine(graph: &Graph, before: u32, after: u32) -> f64 {
    let roads = &graph.roads;
    let (arrival, departure) = (roads.shape(before as usize), roads.shape(after as usize));
    let (end, start) = (arrival[arrival.len() - 1], departure[0]);
    let behind = arrival.iter().rev().skip(1).find(|point| point.distance(end) >= HEADING_M).unwrap_or(&arrival[0]);
    let ahead = departure.iter().skip(1).find(|point| point.distance(start) >= HEADING_M);
    // East and north from `a` to `b`, in microdegrees of latitude.
    let vector = |a: Point, b: Point| {
        (f64::from(b.lon - a.lon) * (f64::from(a.lat) * 1e-6).to_radians().cos(), f64::from(b.lat - a.lat))
    };
    let (u, v) = (vector(*behind, end), vector(start, *ahead.unwrap_or(&departure[departure.len() - 1])));
    let lengths = u.0.hypot(u.1) * v.0.hypot(v.1);
    if lengths == 0.0 {
        return 1.0;
    }
    ((u.0 * v.0 + u.1 * v.1) / lengths).clamp(-1.0, 1.0)
}

/// (junction, group) for each junction of a signal group, sorted. Two signal nodes are in one group
/// where roads of at most `GROUP_M` in total join them, in any direction, and so are the junctions
/// on those roads. The joins chain, so a row of signal nodes is one group.
fn groups(graph: &Graph) -> Vec<(u32, u32)> {
    let (roads, points) = (&graph.roads, graph.points.values());
    let signals: Vec<u32> =
        (0..points.len() as u32).filter(|&node| graph.signals.values()[node as usize] != 0).collect();
    if signals.is_empty() {
        return Vec::new();
    }
    // Squares of at least `GROUP_M` on a side at each signal node: a point within `GROUP_M` of a
    // signal node lies in its square or next to it.
    let tall = (f64::from(GROUP_M) / METRES_PER_UDEG).ceil() as i32;
    let poleward = signals.iter().map(|&node| points[node as usize].lat.unsigned_abs()).max().unwrap_or(0);
    let wide = (f64::from(tall) / (f64::from(poleward) * 1e-6).to_radians().cos()).ceil() as i32;
    let square = |point: Point| [point.lat.div_euclid(tall), point.lon.div_euclid(wide)];
    let mut squares = HashMap::<[i32; 2], Vec<u32>>::new();
    for &node in &signals {
        squares.entry(square(points[node as usize])).or_default().push(node);
    }
    let near = |point: Point| {
        let [lat, lon] = square(point);
        let keys = (lat - 1..=lat + 1).flat_map(|lat| (lon - 1..=lon + 1).map(move |lon| [lat, lon]));
        keys.filter_map(|key| squares.get(&key))
            .flatten()
            .any(|&node| points[node as usize].distance(point) <= f64::from(GROUP_M))
    };
    // The roads that a join can take, as (from, to, metres) in both directions.
    let length = |road: usize| roads.length_m.values()[road];
    let mut edges: Vec<(u32, u32, u32)> = (0..roads.len())
        .into_par_iter()
        .filter(|&road| length(road) <= GROUP_M && near(points[roads.from(road) as usize]))
        .flat_map_iter(|road| {
            let (from, to) = (roads.from(road), roads.to(road));
            [(from, to, length(road)), (to, from, length(road))]
        })
        .collect();
    edges.sort_unstable();
    edges.dedup();
    // (junction, metres, signal index) for each junction within `GROUP_M` of a signal node.
    let mut reached: Vec<(u32, u32, u32)> = (0..signals.len())
        .into_par_iter()
        .flat_map_iter(|i| reach(&edges, signals[i]).into_iter().map(move |(node, metres)| (node, metres, i as u32)))
        .collect();
    reached.sort_unstable();
    let index = |node: u32| signals.binary_search(&node).ok().map(|i| i as u32);
    // Each set has its smallest signal index as its root.
    let mut parent: Vec<u32> = (0..signals.len() as u32).collect();
    fn root(parent: &mut [u32], mut i: u32) -> u32 {
        while parent[i as usize] != i {
            parent[i as usize] = parent[parent[i as usize] as usize];
            i = parent[i as usize];
        }
        i
    }
    for &(node, _, i) in &reached {
        if let Some(j) = index(node) {
            let (a, b) = (root(&mut parent, i), root(&mut parent, j));
            parent[a.max(b) as usize] = a.min(b);
        }
    }
    let mut groups = Vec::new();
    for run in reached.chunk_by(|a, b| a.0 == b.0) {
        // A junction that is no signal node is in a group where a join of its two nearest signal
        // nodes, which differ, passes it.
        let member = match index(run[0].0) {
            Some(i) => Some(i),
            None => (run.len() >= 2 && run[0].1 + run[1].1 <= GROUP_M).then_some(run[0].2),
        };
        if let Some(i) = member {
            groups.push((run[0].0, signals[root(&mut parent, i) as usize]));
        }
    }
    groups
}

/// The junctions that `edges` join to `start` within `GROUP_M` in total, with the metres of the
/// shortest join.
fn reach(edges: &[(u32, u32, u32)], start: u32) -> Vec<(u32, u32)> {
    let mut best = vec![(start, 0)];
    let mut queue = BinaryHeap::from([Reverse((0, start))]);
    while let Some(Reverse((metres, node))) = queue.pop() {
        if best.iter().any(|&(known, shorter)| known == node && shorter < metres) {
            continue;
        }
        let first = edges.partition_point(|edge| edge.0 < node);
        for &(_, next, length) in edges[first..].iter().take_while(|edge| edge.0 == node) {
            let through = metres + length;
            if through > GROUP_M {
                continue;
            }
            match best.iter_mut().find(|(known, _)| *known == next) {
                Some((_, shorter)) if *shorter <= through => continue,
                Some((_, longer)) => *longer = through,
                None => best.push((next, through)),
            }
            queue.push(Reverse((through, next)));
        }
    }
    best
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::source::{Builder, Id, Relations, Tags, WayFacts, Ways};
    use planner_router::model::{Road, BIKE, FOOT, NO_ELEVATION, PUSH};

    /// A way ID, the junctions of the shape of a piece of the way, and the tags of the way.
    pub(crate) type Piece<'a> = (i64, &'a [u32], &'a [(&'a str, &'a str)]);

    /// A graph with a road in each direction of each piece, from its first to its last junction.
    /// Junction `i` lies at `points[i]`, in microdegrees of latitude and longitude. The tags of the
    /// first piece of a way count.
    pub(crate) fn graph(points: &[[i32; 2]], pieces: &[Piece]) -> Graph {
        let points: Vec<_> = points.iter().map(|&[lat, lon]| Point { lat, lon, elevation: NO_ELEVATION }).collect();
        let (mut ways, mut roads) = (Builder::<Ways>::default(), Vec::new());
        for &(way, nodes, tags) in pieces {
            let ids: Vec<i64> = nodes.iter().map(|&node| node.into()).collect();
            ways.push(way, &ids, &Tags::from_pairs(tags.iter().copied()));
            let shape: Vec<_> = nodes.iter().map(|&node| points[node as usize]).collect();
            let length_m = shape.windows(2).map(|p| p[0].distance(p[1])).sum::<f64>().round() as u32;
            let back: Vec<_> = shape.iter().rev().copied().collect();
            let (first, last) = (nodes[0], nodes[nodes.len() - 1]);
            for (reversed, from, to, shape) in [(false, first, last, shape), (true, last, first, back)] {
                let (class, access, difficulty) = (1, BIKE | FOOT | PUSH, 255);
                roads.push(Road {
                    from,
                    to,
                    way,
                    reversed,
                    length_m,
                    class,
                    access,
                    difficulty,
                    shape,
                    ..Road::default()
                });
            }
        }
        let mut graph = Graph {
            node_ids: (0..points.len() as i64).collect(),
            node_access: crate::column::Column::filled(points.len(), BIKE | FOOT | PUSH),
            points: points.into_iter().collect(),
            roads: roads.into_iter().collect(),
            ..Graph::default()
        };
        graph.osm.ways = ways.finish().0;
        graph.link_ways();
        graph
    }

    /// The road from junction `from` to junction `to`.
    pub(crate) fn road(graph: &Graph, from: u32, to: u32) -> u32 {
        let roads = &graph.roads;
        (0..roads.len()).find(|&road| (roads.from(road), roads.to(road)) == (from, to)).unwrap() as u32
    }

    /// The cost of the turn over the junctions `nodes`.
    fn turn(graph: &Graph, [a, b, c]: [u32; 3]) -> u64 {
        let topology = Topology::new(&graph.roads, graph.points.len()).unwrap();
        let (before, after) = (road(graph, a, b), road(graph, b, c));
        Junctions::new(graph).cost(graph, before, after, onward(graph, &topology, before))
    }

    /// Junction 1 joins 0 in the west, 2 in the east and 3 in the north, about 111 m away.
    const CROSS: [[i32; 2]; 4] = [[0, 0], [0, 1000], [0, 2000], [1000, 1000]];
    const STREET: &[(&str, &str)] = &[("highway", "residential")];

    #[test]
    fn a_turn_costs_where_the_route_leaves_its_road_at_a_choice() {
        let mut ways: Vec<Piece> = vec![(1, &[0, 1], STREET), (2, &[1, 2], STREET), (3, &[1, 3], STREET)];
        assert_eq!(turn(&graph(&CROSS, &ways), [0, 1, 3]), 100);
        assert_eq!(turn(&graph(&CROSS, &ways), [0, 1, 0]), 200);
        // The next road of the same way, with no name.
        ways[2].0 = 1;
        assert_eq!(turn(&graph(&CROSS, &ways), [0, 1, 3]), 0);
        let named: &[(&str, &str)] = &[("highway", "residential"), ("name", "Main")];
        ways[0] = (1, &[0, 1], named);
        ways[2] = (3, &[1, 3], named);
        assert_eq!(turn(&graph(&CROSS, &ways), [0, 1, 3]), 0);
        // A bend between two ways with no side road.
        let bend = graph(&CROSS, &[(1, &[0, 1], STREET), (3, &[1, 3], STREET)]);
        assert_eq!(turn(&bend, [0, 1, 3]), 0);
    }

    #[test]
    fn a_turn_along_a_shared_cycle_route_costs_nothing() {
        let mut graph = graph(&CROSS, &[(1, &[0, 1], STREET), (2, &[1, 2], STREET), (3, &[1, 3], STREET)]);
        let route = Tags::from_pairs([("type", "route"), ("route", "bicycle")]);
        let mut relations = Builder::<Relations>::default();
        relations.push(10, &route, [1, 3].map(|way| (Id::Way(way), "")));
        graph.osm.relations = relations.finish().0;
        let ranks = |way: i64| if way == 2 { [0, 0] } else { [1, 1] };
        let facts = graph.osm.ways.iter().map(|way| WayFacts { ranks: ranks(way.id), ..way.facts }).collect();
        graph.osm.ways.set_facts(facts);
        assert_eq!(turn(&graph, [0, 1, 3]), 0);
        assert_eq!(turn(&graph, [2, 1, 3]), 100);
    }

    #[test]
    fn signal_nodes_20_m_apart_are_one_junction_that_a_route_pays_once() {
        // A street from 0 to 4 with the signal nodes 1 and 3, each 10 m from junction 2, where a
        // side street from 5 arrives. The streets have one name, so a turn costs nothing.
        let points = [[0, 0], [0, 1000], [0, 1090], [0, 1180], [0, 2180], [-1000, 1090]];
        let main: &[(&str, &str)] = &[("highway", "residential"), ("name", "Main")];
        let pieces: Vec<Piece> =
            vec![(1, &[0, 1], main), (2, &[1, 2], main), (3, &[2, 3], main), (4, &[3, 4], main), (5, &[5, 2], main)];
        let mut graph = graph(&points, &pieces);
        graph.signals = [0, 1, 0, 1, 0, 0].into_iter().collect();
        let junctions = Junctions::new(&graph);
        assert_eq!([0, 1, 2, 3, 4].map(|junction| junctions.group(junction)), [None, Some(1), Some(1), Some(1), None]);
        let topology = Topology::new(&graph.roads, graph.points.len()).unwrap();
        let paid = |nodes: &[u32]| -> u64 {
            let roads: Vec<u32> = nodes.windows(2).map(|ends| road(&graph, ends[0], ends[1])).collect();
            roads
                .windows(2)
                .map(|pair| junctions.cost(&graph, pair[0], pair[1], onward(&graph, &topology, pair[0])))
                .sum()
        };
        assert_eq!(paid(&[0, 1, 2, 3, 4]), 60);
        assert_eq!(paid(&[5, 2, 3, 4]), 60);
    }
}
