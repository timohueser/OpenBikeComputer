use super::measures::measure;
use super::net::Net;
use super::pairs::compare;
use super::panel::{sample, Class, Pair, Spec};
use super::signed::signed_share;
use crate::base::Topology;
use crate::source::{Builder, Data, Id, NodeRef, Relations, Tags, Ways};
use crate::{Costs, Graph};
use planner_router::model::{Point, Road, Surface, BIKE, FOOT, PUSH};
use serde_json::{json, Value};

/// A grid of streets 600 m apart, 10 columns by 3 rows, with the costs of `touring`. Node and
/// junction `10 × row + column` are the same number. The streets of row 0 have the name `Main`.
/// The regional cycle route 1 follows row 1. The local cycle route 2 runs from node 0 over row 2
/// to node 9.
struct Region {
    graph: Graph,
    topology: Topology,
    costs: Costs,
}

impl Region {
    fn net(&self) -> Net<'_> {
        Net::new(&self.graph, &self.topology, &self.costs, 0)
    }

    /// The roads of the route over the junctions `nodes`.
    fn roads(&self, nodes: &[u32]) -> Vec<u32> {
        let roads = &self.graph.roads;
        let road = |ends: &[u32]| (0..roads.len()).find(|&road| [roads.from(road), roads.to(road)] == ends);
        nodes.windows(2).map(|ends| road(ends).unwrap() as u32).collect()
    }
}

fn way(a: u32, b: u32) -> i64 {
    i64::from(100 * a.min(b) + a.max(b))
}

fn region() -> Region {
    let mut graph = Graph::default();
    for node in 0..30 {
        let point = Point { lat: 100_000 + node / 10 * 5_400, lon: 100_000 + node % 10 * 5_400, elevation: 0.0 };
        graph.points.push(point);
        graph.node_ids.push(node.into());
        graph.node_access.push(BIKE | FOOT | PUSH);
    }
    let (mut ways, mut roads) = (Builder::<Ways>::default(), Vec::new());
    for a in 0..30 {
        for b in [a + 1, a + 10].into_iter().filter(|&b| b < 30 && (b == a + 10 || b % 10 != 0)) {
            let name = (b < 10).then_some(("name", "Main"));
            let tags = Tags::from_pairs([("highway", "residential")].into_iter().chain(name));
            ways.push(way(a, b), &[a.into(), b.into()], &tags);
            for (from, to) in [(a, b), (b, a)] {
                let shape = vec![graph.points.values()[from as usize], graph.points.values()[to as usize]];
                roads.push(Road {
                    from,
                    to,
                    way: way(a, b),
                    reversed: from > to,
                    length_m: shape[0].distance(shape[1]).round() as u32,
                    surface: Surface::Paved,
                    class: 1,
                    access: BIKE | FOOT | PUSH,
                    difficulty: 255,
                    shape,
                    ..Road::default()
                });
            }
        }
    }
    graph.roads = roads.into_iter().collect();
    let mut relations = Builder::<Relations>::default();
    let mut route = |id, network, nodes: Vec<u32>| {
        let tags = Tags::from_pairs([("type", "route"), ("route", "bicycle"), ("network", network)]);
        relations.push(id, &tags, nodes.windows(2).map(|ends| (Id::Way(way(ends[0], ends[1])), "")));
    };
    route(1, "rcn", (10..20).collect());
    route(2, "lcn", [0, 10].into_iter().chain(20..30).chain([19, 9]).collect());
    let mut ways = ways.finish().0;
    let node = |id: i64| {
        let point = graph.points.values()[id as usize];
        NodeRef { lat: point.lat, lon: point.lon, flags: NodeRef::PRESENT, ..NodeRef::missing(false) }
    };
    ways.set_points(ways.references().iter().map(|&id| node(id)).collect());
    graph.osm = Data { ways, relations: relations.finish().0, ..Data::default() };
    graph.link_ways();
    let topology = Topology::new(&graph.roads, graph.points.len()).unwrap();
    let costs = crate::costs(&graph, &topology, &crate::profiles(&["touring"]).unwrap()).unwrap();
    Region { graph, topology, costs }
}

#[test]
fn a_route_counts_its_turns_and_its_leave_and_rejoin_events() {
    let region = region();
    // The bend at node 0 is no turn: the node has two road ends. The route leaves `Main` at node 1
    // and is on it again after 1.8 km.
    let measures = measure(&region.net(), &region.roads(&[10, 0, 1, 11, 12, 2, 3]));
    assert_eq!((measures.turns, measures.events), (4, 1));
    let straight = measure(&region.net(), &region.roads(&[0, 1, 2, 3]));
    assert_eq!((straight.turns, straight.events), (0, 0));
}

#[test]
fn the_signed_share_tells_a_route_that_the_router_follows_from_one_that_it_leaves() {
    let signed = signed_share(&region().net(), 10_000.0).unwrap();
    let [_, _, followed, left, _] = signed.levels;
    assert!(followed.measured_m > 5_000.0);
    assert_eq!((followed.near_m, followed.same_road_m), (followed.measured_m, followed.measured_m));
    // The router takes row 0, so only the two ends of route 2 are near its line.
    assert!(left.measured_m > 7_000.0 && left.near_m < 0.02 * left.measured_m, "{left:?}");
    assert_eq!(left.same_road_m, 0.0);
    assert_eq!((signed.legs, signed.legs_with_no_split, signed.splits, signed.not_longer), (2, 1, 1, 0));
}

#[test]
fn a_sample_gives_the_same_pairs_of_its_classes_again() {
    const CLASSES: &[Class] = &[(1.0, 2.0, 4), (4.0, 6.0, 4)];
    let region = region();
    let spec = Spec { classes: CLASSES, inside: None };
    let pairs = sample(&region.net(), &spec).unwrap();
    assert_eq!(pairs, sample(&region.net(), &spec).unwrap());
    let points = region.graph.points.values();
    let fits = |pair: &Pair, class: usize| {
        let km = points[pair.from as usize].distance(points[pair.to as usize]) / 1000.0;
        km >= CLASSES[class].0 && km <= CLASSES[class].1
    };
    assert!(pairs.len() == 8 && pairs.iter().enumerate().all(|(i, pair)| fits(pair, i / 4)), "{pairs:?}");
}

#[test]
fn compare_keeps_the_pairs_with_two_different_routes() {
    let route = |lat: f64| json!({"line": [[lat, 7.0], [lat, 7.02]]});
    let run = |routes: [Value; 3]| json!({"pairs": [1, 2, 3], "routes": {"touring": routes}});
    // Pair 1 has two routes 100 m apart, pair 2 the same route twice, pair 3 no route in a run.
    let runs = [run([route(48.0), route(48.0), Value::Null]), run([route(48.0009), route(48.0), route(48.0)])];
    let (data, missing) = compare([&runs[0], &runs[1]], ["a", "b"]).unwrap();
    let kept: Vec<&Value> = data["sets"][0]["pairs"].as_array().unwrap().iter().map(|pair| &pair["id"]).collect();
    assert_eq!((kept, missing), (vec![&json!(1)], 1));
}
