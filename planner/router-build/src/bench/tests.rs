use super::measures::measure;
use super::net::Net;
use super::pairs::{compare, overlay};
use super::panel::{panel, sample, Class, Pair, Spec};
use super::signed::signed_share;
use crate::base::Topology;
use crate::junction::Junctions;
use crate::source::{Builder, Data, Id, NodeRef, Relations, Tags, Ways};
use crate::{Costs, Graph};
use planner_router::model::{Point, Road, Surface, BIKE, FOOT, PUSH};
use serde_json::{json, Value};

/// A grid of streets 600 m apart, 10 columns by 4 rows, with the costs of `touring`. Node and
/// junction `10 × row + column` are the same number. The streets of row 0 have the name `Main`.
/// The regional cycle route 1 follows row 1. The local cycle route 2 runs from node 0 over row 2
/// to node 9. The national cycle route 3 follows row 3: there the street from node 33 to node 32
/// is a one-way for cycling, and `touring` may not use the street between the nodes 34 and 35.
/// Node 27 to node 37 is a cycleway, and node 28 to node 38 a path with `bicycle=designated`.
struct Region {
    graph: Graph,
    topology: Topology,
    junctions: Junctions,
    costs: Costs,
}

impl Region {
    fn new(graph: Graph) -> Self {
        let topology = Topology::new(&graph.roads, graph.points.len()).unwrap();
        let costs = crate::costs(&graph, &topology, &crate::profiles(&["touring"]).unwrap()).unwrap();
        Region { junctions: Junctions::new(&graph), graph, topology, costs }
    }

    fn net(&self) -> Net<'_> {
        Net::new(&self.graph, &self.topology, &self.junctions, &self.costs, 0)
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
    for node in 0..40 {
        let point = Point { lat: 100_000 + node / 10 * 5_400, lon: 100_000 + node % 10 * 5_400, elevation: 0.0 };
        graph.points.push(point);
        graph.node_ids.push(node.into());
        graph.node_access.push(BIKE | FOOT | PUSH);
    }
    let (mut ways, mut roads) = (Builder::<Ways>::default(), Vec::new());
    for a in 0..40 {
        for b in [a + 1, a + 10].into_iter().filter(|&b| b < 40 && (b == a + 10 || b % 10 != 0)) {
            let name = (b < 10).then_some(("name", "Main"));
            let (highway, class) = match (a, b) {
                (27, 37) => ("cycleway", 0),
                (28, 38) => ("path", 4),
                _ => ("residential", 1),
            };
            let designated = (class == 4).then_some(("bicycle", "designated"));
            let tags = Tags::from_pairs([("highway", highway)].into_iter().chain(name).chain(designated));
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
                    class,
                    access: if (from, to) == (32, 33) { FOOT | PUSH } else { BIKE | FOOT | PUSH },
                    difficulty: if (a, b) == (34, 35) { 3 } else { 255 },
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
    route(3, "ncn", (30..40).collect());
    let mut ways = ways.finish().0;
    let node = |id: i64| {
        let point = graph.points.values()[id as usize];
        NodeRef { lat: point.lat, lon: point.lon, flags: NodeRef::PRESENT, ..NodeRef::missing(false) }
    };
    ways.set_points(ways.references().iter().map(|&id| node(id)).collect());
    graph.osm = Data { ways, relations: relations.finish().0, ..Data::default() };
    graph.link_ways();
    Region::new(graph)
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
fn a_route_through_a_group_of_two_signal_nodes_enters_one_signalled_junction() {
    // A street over the junctions 0 to 3, with the signal nodes 1 and 2 20 m apart.
    let points = [[0, 0], [0, 1000], [0, 1180], [0, 2180]];
    let street: &[(&str, &str)] = &[("highway", "residential")];
    let mut graph =
        crate::junction::tests::graph(&points, &[(1, &[0, 1], street), (2, &[1, 2], street), (3, &[2, 3], street)]);
    graph.signals = [0, 1, 1, 0].into_iter().collect();
    let region = Region::new(graph);
    assert_eq!(measure(&region.net(), &region.roads(&[0, 1, 2, 3])).signalled, 1);
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
    // Route 3 adds two legs with no split.
    assert_eq!((signed.legs, signed.legs_with_no_split, signed.splits, signed.not_longer), (4, 3, 1, 0));
}

#[test]
fn a_signed_line_is_cut_where_the_profile_may_not_ride_and_a_short_piece_is_not_measured() {
    // The one-way and the closed street cut route 3 into the pieces 30 to 32, 33 to 34 and 35 to
    // 39. The piece of 600 m is under 1 km.
    let signed = signed_share(&region().net(), 10_000.0).unwrap();
    assert_eq!(signed.levels[1].measured_m, 3_600.0);
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
fn a_panel_counts_a_pair_with_no_route() {
    let pairs = [Pair { from: 0, to: 9 }, Pair { from: 5, to: 5 }];
    let panel = panel(&region().net(), &pairs).unwrap();
    assert_eq!((panel.pairs, panel.routes, panel.total.length_m), (2, 1, 5_400.0));
    // A panel with no route is a result too: the run still writes its numbers.
    let none = super::panel::panel(&region().net(), &pairs[1..]).unwrap();
    assert!((none.pairs, none.routes) == (1, 0) && none.detour[0].is_nan());
}

#[test]
fn the_overlay_has_the_cycle_paths_and_the_signed_ways_and_no_ordinary_street() {
    let region = region();
    let overlay = overlay(&region.net(), &[Pair { from: 0, to: 39 }]);
    let place = |node: usize| {
        let point = region.graph.points.values()[node];
        [f64::from(point.lat) / 1e6, f64::from(point.lon) / 1e6]
    };
    let has = |kind: &str, a, b| overlay[kind].as_array().unwrap().contains(&json!([place(a), place(b)]));
    assert!(has("cycle path", 27, 37) && has("cycle path", 28, 38) && has("signed route", 10, 11));
    // Node 0 to node 1 is a street of no kind.
    assert!(!["cycle path", "cycle lane or track", "signed route"].iter().any(|kind| has(kind, 0, 1)));
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
