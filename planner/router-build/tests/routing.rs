use planner_router::cost::RoadCost;
use planner_router::{
    answer::answer,
    data::{RoutingData, Selection},
    model::{Pace, Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION, PUSH},
    package::{digest, Package, Source},
    router::Response,
    snap::{Candidate, Policy, Position, REACH_M},
    Control, Error, Request, Route, Router,
};
use planner_router_build::cost::{way_bases, Costing};
use planner_router_build::source::{Builder, Tags, Ways};
use planner_router_build::{base::Dictionary, Graph, Turn};
use serde_json::{json, Value};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    sync::Arc,
};

#[derive(Clone)]
struct Memory(Arc<HashMap<String, Vec<u8>>>);
impl Source for Memory {
    fn read(&self, key: &str) -> planner_router::Result<Vec<u8>> {
        self.0.get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
    }
}
fn package(graph: &Graph) -> (Memory, Vec<u8>) {
    let profiles = Profile::presets();
    package_with_profiles(graph, &profiles[..5])
}
fn routing(source: Memory, manifest: &[u8]) -> Selection<Memory> {
    Selection::whole(Package::open(source, manifest).unwrap())
}
fn package_with_profiles(graph: &Graph, profiles: &[Profile]) -> (Memory, Vec<u8>) {
    let graph = with_sources(graph.clone());
    let mut objects = HashMap::new();
    let manifest = planner_router_build::prepare(
        &graph,
        "test".into(),
        [-1.0, -1.0, 1.0, 1.0],
        profiles,
        vec![],
        "test".into(),
        |bytes| {
            let key = digest(bytes);
            objects.insert(key.clone(), bytes.to_vec());
            Ok(key)
        },
    )
    .unwrap();
    (Memory(Arc::new(objects)), serde_json::to_vec(&manifest).unwrap())
}
/// One value for each edge of the route, from the runs of one edge channel of its answer.
fn edges(route: &Route, channel: &str) -> Value {
    let answer = answer(&Response { routes: vec![route.clone()] });
    let runs = answer["routes"][0]["edges"][channel].as_array().unwrap().clone();
    runs.iter().flat_map(|run| vec![run[0].clone(); run[1].as_u64().unwrap() as usize]).collect()
}
fn point([lon, lat]: [f64; 2]) -> Point {
    Point { lon: (lon * 1e6).round() as i32, lat: (lat * 1e6).round() as i32, elevation: NO_ELEVATION }
}
/// The route point where the route attaches request point `k`, and its distance from that point.
fn attachment(route: &Route, request: &Request, k: usize) -> (Point, f64) {
    let attached = route.points[if k == 0 { 0 } else { route.legs[k - 1].to_index }];
    (attached, attached.distance(point(request.points[k])))
}
/// The candidates a route request attaches a point to: within 250 m, or else within reach.
fn snaps(router: &Router<Selection<Memory>>, coordinate: [f64; 2], profile: &str) -> Vec<Candidate> {
    let package = router.package();
    let found = package.snap(point(coordinate), profile, Policy::default()).unwrap();
    if found.retained.is_empty() {
        return package
            .snap(point(coordinate), profile, Policy { radius_m: REACH_M, ..Policy::default() })
            .unwrap()
            .retained;
    }
    found.retained
}

fn fixture() -> Graph {
    let points: Vec<_> =
        (0..12).map(|i| Point { lat: i / 4 * 10_000, lon: i % 4 * 10_000, elevation: NO_ELEVATION }).collect();
    let mut roads = Vec::new();
    for from in 0..12 {
        for to in 0..12 {
            if (from as i32 - to as i32).abs() == 1 && from / 4 == to / 4
                || (from as i32 - to as i32).abs() == 4 && from % 2 == 0
            {
                roads.push(Road {
                    from,
                    to,
                    way: roads.len() as i64,
                    reversed: from > to,
                    length_m: points[from as usize].distance(points[to as usize]).round() as u32,
                    ascent_m: 0,
                    descent_m: 0,
                    surface: if from.min(to) % 2 == 0 { Surface::Gravel } else { Surface::Paved },
                    class: 1,
                    access: BIKE | FOOT,
                    difficulty: 0,
                    hiking_difficulty: None,
                    structure: false,
                    shape: vec![points[from as usize], points[to as usize]],
                });
            }
        }
    }
    let forbidden = (0..roads.len())
        .flat_map(|a| {
            let roads = &roads;
            (0..roads.len()).filter_map(move |b| {
                (roads[a].to == roads[b].from && (a + b) % 7 == 0).then_some(Turn { from: a as u32, to: b as u32 })
            })
        })
        .collect();
    let forbidden_foot = (0..roads.len())
        .flat_map(|a| {
            let roads = &roads;
            (0..roads.len()).filter_map(move |b| {
                (roads[a].to == roads[b].from && (a + b) % 11 == 0).then_some(Turn { from: a as u32, to: b as u32 })
            })
        })
        .collect();
    Graph {
        points: points.into_iter().collect(),
        roads: roads.into_iter().collect(),
        forbidden,
        forbidden_foot,
        warnings: vec![],
        ..Graph::default()
    }
}
/// The roads of `graph`, each with its own shape.
fn roads(graph: &Graph) -> Vec<Road> {
    graph.roads.iter().collect()
}
fn edit_roads(graph: &mut Graph, edit: impl FnOnce(&mut Vec<Road>)) {
    let mut roads = roads(graph);
    edit(&mut roads);
    graph.roads = roads.into_iter().collect();
}
fn with_sources(mut graph: Graph) -> Graph {
    // Reordering or clipping roads does not change their source way tags.
    if !graph.osm.ways.is_empty() {
        graph.link_ways();
        return graph;
    }
    graph.node_ids = (0..graph.points.len() as i64).collect();
    graph.node_access = std::iter::repeat_n(BIKE | FOOT, graph.points.len()).collect();
    let mut ways = Builder::<Ways>::default();
    let points = graph.points.len() as i64;
    edit_roads(&mut graph, |roads| {
        for road in roads {
            road.way = road.from.min(road.to) as i64 * points + road.from.max(road.to) as i64;
            road.reversed = road.from > road.to;
            let highway = ["cycleway", "residential", "primary", "track", "path", "steps", ""][road.class as usize];
            let surface = ["", "asphalt", "compacted", "gravel", "dirt", "sand"][road.surface as usize];
            let tags = Tags::from_pairs([("highway", highway), ("surface", surface)]);
            ways.push(road.way, &[road.from as i64, road.to as i64], &tags);
        }
    });
    graph.osm = planner_router_build::source::Data { ways: ways.finish().0, ..Default::default() };
    graph.link_ways();
    graph
}
fn costing<'a>(graph: &'a Graph, profile: &'a Profile, dictionary: &'a mut Dictionary) -> Costing<'a> {
    let bases = way_bases(&graph.osm, std::slice::from_ref(profile)).unwrap();
    Costing::new(graph, profile, &bases, dictionary).unwrap()
}
// Independent arrival-road Dijkstra. The target is a partial transition, not a compact state.
fn oracle(graph: &Graph, profile: &Profile, a: Position, b: Position) -> Option<u64> {
    let graph = with_sources(graph.clone());
    let mut dictionary = Dictionary::default();
    let costing = costing(&graph, profile, &mut dictionary);
    search(&costing, &curves(&graph, &costing, profile), a, b)
}
/// The cost curve of each road, compiled from the basis of `costing`, which keeps its total.
fn curves(graph: &Graph, costing: &Costing, profile: &Profile) -> Vec<Option<RoadCost>> {
    let curves: Vec<_> = graph
        .roads
        .iter()
        .enumerate()
        .map(|(id, road)| costing.basis(id as u32).map(|basis| basis.compile(&road, profile).unwrap()))
        .collect();
    let totals: Vec<_> = curves.iter().map(|curve| curve.as_ref().map_or(u32::MAX, |c| c.total() as u32)).collect();
    assert_eq!(costing.totals.values(), totals);
    curves
}
fn search(costing: &Costing, curves: &[Option<RoadCost>], a: Position, b: Position) -> Option<u64> {
    let prefix = curves[a.road as usize].as_ref().unwrap().prefix(a.fraction).unwrap();
    let suffix = curves[b.road as usize].as_ref().unwrap().prefix(b.fraction).unwrap();
    let mut best = if a.road == b.road && a.fraction <= b.fraction { Some(suffix - prefix) } else { None };
    let mut dist = vec![u64::MAX; curves.len()];
    dist[a.road as usize] = curves[a.road as usize].as_ref().unwrap().total() - prefix;
    let mut heap = BinaryHeap::from([Reverse((dist[a.road as usize], a.road))]);
    while let Some(Reverse((cost, road))) = heap.pop() {
        if cost != dist[road as usize] {
            continue;
        }
        for (next, distance) in dist.iter_mut().enumerate() {
            let Some(penalty) = costing.transition(road, next as u32) else {
                continue;
            };
            if next as u32 == b.road {
                best = Some(best.unwrap_or(u64::MAX).min(cost + suffix + penalty));
            }
            let Some(weight) = curves[next].as_ref() else {
                continue;
            };
            let weight = weight.total() + penalty;
            if cost + weight < *distance {
                *distance = cost + weight;
                heap.push(Reverse((cost + weight, next as u32)));
            }
        }
    }
    best
}
fn coordinate(road: &Road, fraction: f64) -> [f64; 2] {
    let (a, b) = (road.shape[0], road.shape[1]);
    [
        (a.lon as f64 + (b.lon - a.lon) as f64 * fraction) * 1e-6,
        (a.lat as f64 + (b.lat - a.lat) as f64 * fraction) * 1e-6,
    ]
}
#[test]
fn prepared_coordinate_routes_match_independent_arrival_road_search() {
    let original = with_sources(fixture());
    let mut graph = original.clone();
    let order = planner_router_build::layout::order(&mut graph, &[4, 8], None).unwrap();
    let order = order.values();
    // `with_sources` gives junction `i` the node ID `i`, so the node IDs give the new junction IDs.
    let mut rank = vec![0; graph.points.len()];
    for (new, &old) in graph.node_ids.values().iter().enumerate() {
        rank[old as usize] = new as u32;
    }
    let (reordered, original_roads) = (roads(&graph), roads(&original));
    for (new, &old) in order.iter().enumerate() {
        let mut road = original_roads[old as usize].clone();
        (road.from, road.to) = (rank[road.from as usize], rank[road.to as usize]);
        assert_eq!(serde_json::to_value(&reordered[new]).unwrap(), serde_json::to_value(&road).unwrap());
    }
    check_cells(&graph);
    for (actual, expected) in
        [(&graph.forbidden, &original.forbidden), (&graph.forbidden_foot, &original.forbidden_foot)]
    {
        let mut restored: Vec<_> = actual
            .values()
            .iter()
            .map(|&Turn { from, to }| Turn { from: order[from as usize], to: order[to as usize] })
            .collect();
        restored.sort_unstable();
        assert_eq!(restored, expected.values());
    }
    let profiles = Profile::presets();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let coords: Vec<_> = reordered.iter().step_by(3).flat_map(|r| [coordinate(r, 0.2), coordinate(r, 0.8)]).collect();
    for profile in &profiles {
        let mut dictionary = Dictionary::default();
        let costing = costing(&graph, profile, &mut dictionary);
        let curves = curves(&graph, &costing, profile);
        let snapped: Vec<_> = coords.iter().map(|&c| snaps(&router, c, &profile.name)).collect();
        for (&from, starts) in coords.iter().zip(&snapped) {
            for (&to, ends) in coords.iter().zip(&snapped) {
                let expected = starts
                    .iter()
                    .flat_map(|a| ends.iter().filter_map(|b| search(&costing, &curves, a.position, b.position)))
                    .min();
                let actual = router.route(
                    &Request {
                        points: vec![from, to],
                        profile: profile.name.clone(),
                        pace: Pace::default(),
                        alternatives: false,
                        alternatives_only: false,
                        turnarounds: vec![],
                        start_position: None,
                        end_position: None,
                    },
                    &Control::default(),
                );
                match (expected, actual) {
                    (Some(cost), Ok(route)) => {
                        assert_eq!(cost, route.cost, "{from:?} -> {to:?}");
                        for pair in route.legs[0].pieces.windows(2) {
                            assert_eq!(graph.roads.to(pair[0].road as usize), graph.roads.from(pair[1].road as usize));
                            assert!(graph.permits_turn(pair[0].road, pair[1].road, profile.walking));
                        }
                    }
                    (None, Err(Error::NoPath)) => {}
                    (expected, actual) => panic!("expected {expected:?}, got {actual:?}"),
                }
            }
        }
    }
}

#[test]
fn lazy_endpoint_costs_preserve_every_profile_and_partial_offset() {
    let mut graph = fixture();
    edit_roads(&mut graph, |roads| {
        for (id, road) in roads.iter_mut().enumerate() {
            let a = road.shape[0];
            let b = road.shape[1];
            road.shape = (0..=6)
                .map(|i| Point {
                    lon: a.lon + (b.lon - a.lon) * i / 6 + if i == 3 { 10 } else { 0 },
                    lat: a.lat + (b.lat - a.lat) * i / 6 + if i == 3 { 10 } else { 0 },
                    elevation: if i == 4 && id % 3 == 0 {
                        NO_ELEVATION
                    } else {
                        [0., 3., 2., 7., 9., 9., 0.][i as usize]
                    },
                })
                .collect();
        }
    });
    let graph = with_sources(graph);
    let profiles = Profile::presets();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let package = routing(source, &manifest);
    for profile in &profiles {
        let mut dictionary = Dictionary::default();
        let costing = costing(&graph, profile, &mut dictionary);
        for (id, expected) in curves(&graph, &costing, profile).iter().enumerate() {
            let actual = package.endpoint(&profile.name, id as u32).unwrap().cost;
            assert_eq!(actual, *expected, "{} road {id}", profile.name);
            if let Some(cost) = actual {
                let mut fractions: Vec<_> = (0..=100).map(|i| i as f64 / 100.0).collect();
                fractions.extend(cost.penalties.iter().map(|p| p.0));
                fractions.sort_by(f64::total_cmp);
                let prefixes: Vec<_> = fractions.iter().map(|&f| cost.prefix(f).unwrap()).collect();
                assert!(prefixes.is_sorted());
                assert_eq!(prefixes.windows(2).map(|p| p[1] - p[0]).sum::<u64>(), cost.total());
            }
        }
    }
}

#[test]
fn an_idle_service_drops_the_profiles_that_no_router_holds_before_it_answers_memory() {
    let graph = fixture();
    let (source, manifest) = package(&graph);
    // This handle keeps an unlimited budget, so the cache keeps each profile that it loads.
    let cache = routing(source, &manifest);
    let roads = roads(&graph);
    let request = Request {
        points: vec![coordinate(&roads[0], 0.2), coordinate(&roads[15], 0.8)],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let idle = cache.idle_bytes("touring").unwrap();
    // A router with `gravel` prepared holds it, so `touring` cannot have its idle room now.
    let held = cache.prepared("gravel").unwrap();
    let busy = cache.fork();
    let mut budget = busy.routing_bytes("touring").unwrap() - 1;
    assert!(budget >= idle);
    drop(busy);
    assert!(matches!(Router::new(cache.fork(), budget).route(&request, &Control::default()), Err(Error::Memory)));
    drop(held);
    // Without the holder, each budget gives the same answer at each try: a limit or the route.
    budget = idle;
    loop {
        let mut router = Router::new(cache.fork(), budget);
        router.package().prepared("touring").unwrap();
        cache.prepared("gravel").unwrap();
        match router.route(&request, &Control::default()) {
            Ok(_) => break,
            Err(Error::Limit) => budget += 64,
            Err(error) => panic!("{error:?} with a budget of {budget}"),
        }
    }
    assert!(budget > idle);
}

#[test]
fn via_direction_pace_and_failure_states_are_explicit() {
    let graph = fixture();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source.clone(), &manifest), 768 * 1024 * 1024);
    let roads = roads(&graph);
    let mut request = Request {
        points: vec![coordinate(&roads[0], 0.2), coordinate(&roads[7], 0.5), coordinate(&roads[15], 0.8)],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.legs.len(), 2);
    let surfaces = edges(&route, "surfaces");
    assert_eq!(surfaces.as_array().unwrap().len(), route.points.len() - 1);
    assert_eq!(edges(&route, "pushing").as_array().unwrap().len(), route.points.len() - 1);
    for leg in &route.legs {
        assert_eq!(surfaces[leg.from_index], json!(roads[leg.pieces[0].road as usize].surface));
        assert_eq!(surfaces[leg.to_index - 1], json!(roads[leg.pieces.last().unwrap().road as usize].surface));
    }
    for pair in route.legs.windows(2) {
        let (a, b) = (pair[0].pieces.last().unwrap(), pair[1].pieces.first().unwrap());
        assert_eq!(a.road, b.road);
        assert_eq!(a.to, b.from);
    }
    request.pace.personal_multiplier = 1.5;
    let slower = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.cost, slower.cost);
    assert_eq!(route.points, slower.points);
    assert!((route.totals().seconds * 1.5 - slower.totals().seconds).abs() < 1e-6);
    assert!(matches!(
        router.route(&request, &Control { cancelled: &|| true, ..Control::default() }),
        Err(Error::Cancelled)
    ));
    assert!(matches!(router.route(&request, &Control { max_queries: 0, ..Control::default() }), Err(Error::Limit)));
    request.points[0] = [10.0, 10.0];
    assert!(matches!(router.route(&request, &Control::default()), Err(Error::MissingRegion(_))));
    let mut broken = (*source.0).clone();
    let reader = router.package().package();
    let key = reader.key(&reader.manifest().geometry, 0).unwrap();
    broken.get_mut(&key).unwrap()[0] ^= 1;
    let package = Package::open(Memory(Arc::new(broken)), &manifest).unwrap();
    assert!(matches!(package.road(0), Err(Error::InvalidData(_))));
}

#[test]
fn a_point_without_a_road_nearby_attaches_to_the_nearest_road_within_reach() {
    // A short road that no other road joins, 333 m from the far point.
    let mut graph = fixture();
    let first = graph.points.len() as u32;
    graph.points.extend(&[14_000, 16_000].map(|lon| Point { lat: -3_000, lon, elevation: NO_ELEVATION }));
    for (from, to) in [(first, first + 1), (first + 1, first)] {
        let shape = vec![graph.points.values()[from as usize], graph.points.values()[to as usize]];
        let road = Road {
            from,
            to,
            reversed: from > to,
            length_m: shape[0].distance(shape[1]).round() as u32,
            shape,
            ..roads(&graph)[0].clone()
        };
        graph.roads.push(road, Default::default());
    }
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![[0.001, 0.0], [0.015, -0.006]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let route = router.route(&request, &Control::default()).unwrap();
    let (end, distance) = attachment(&route, &request, 1);
    assert!((Policy::default().radius_m..=REACH_M).contains(&distance));
    assert_eq!((end.lat, end.lon), (0, 15_000));
    request.points[1] = [0.015, -0.012];
    assert!(matches!(router.route(&request, &Control::default()), Err(Error::NoSnap(1))));
}

#[test]
fn a_shape_keeps_direction_and_only_an_explicit_visit_can_reverse() {
    let mut graph = fixture();
    edit_roads(&mut graph, |roads| roads.retain(|r| (r.from == 0 && r.to == 1) || (r.from == 1 && r.to == 0)));
    graph.forbidden = Default::default();
    graph.forbidden_foot = Default::default();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let road = &roads(&graph)[0];
    let mut request = Request {
        points: vec![coordinate(road, 0.2), coordinate(road, 0.8), coordinate(road, 0.2)],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let shaped = router.route(&request, &Control::default()).unwrap();
    request.turnarounds = vec![1];
    let visit = router.route(&request, &Control::default()).unwrap();
    assert!(shaped.totals().distance_m > visit.totals().distance_m + 400);
    assert_ne!(visit.legs[1].start.road, visit.legs[0].end.road);
    assert_eq!(visit.points.len(), visit.elapsed.len());
    assert!(visit.elapsed.is_sorted_by(|a, b| a <= b));
    assert!((visit.elapsed.last().unwrap() - visit.totals().seconds).abs() < 1e-6);
}

#[test]
fn a_window_pinned_to_its_neighbour_legs_repeats_the_whole_trip() {
    let graph = fixture();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let roads = roads(&graph);
    let whole = Request {
        points: [(0, 0.3), (7, 0.5), (15, 0.5), (20, 0.6), (3, 0.7), (11, 0.4)]
            .map(|(road, fraction)| coordinate(&roads[road], fraction))
            .to_vec(),
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let route = router.route(&whole, &Control::default()).unwrap();
    let leg = |route: &Route, k: usize| {
        let leg = &route.legs[k];
        (route.points[leg.from_index..=leg.to_index].to_vec(), leg.totals.distance_m, leg.start.id())
    };
    for k in 1..route.legs.len() {
        let window = Request {
            points: whole.points[k - 1..=k + 1].to_vec(),
            start_position: Some(route.legs[k - 1].start.id()),
            end_position: Some(route.legs[k].end.id()),
            ..whole.clone()
        };
        let pinned = router.route(&window, &Control::default()).unwrap();
        assert_eq!([leg(&pinned, 0), leg(&pinned, 1)], [leg(&route, k - 1), leg(&route, k)], "window at point {k}");
        assert_eq!(pinned.legs[1].end.id(), route.legs[k].end.id());
    }
    let foreign = Request { start_position: Some("0:0.5".into()), end_position: Some("x".into()), ..whole.clone() };
    assert_eq!(router.route(&foreign, &Control::default()).unwrap().points, route.points);
}

/// Checks the cells of an ordered graph against a direct scan: the cell of each road, its border
/// level, the entering roads first in each level-1 cell, and the leaving lists.
fn check_cells(graph: &Graph) {
    let cells = &graph.cells;
    let (level1, counts) = (cells.level1.values(), cells.counts.values());
    let levels = cells.levels();
    assert!(levels >= 2, "the fixture has two levels");
    // The level-`level` cell of each junction, from the first junctions of the cells.
    let cell = |level: usize, junction: u32| {
        let level1 = (0..level1.len() - 1).find(|&c| level1[c][0] <= junction && junction < level1[c + 1][0]).unwrap();
        if level == 1 {
            return level1;
        }
        let start: usize = counts[1..level - 1].iter().map(|&n| n as usize + 1).sum();
        let upper = &cells.upper.values()[start..=start + counts[level - 1] as usize];
        (0..counts[level - 1] as usize)
            .find(|&c| upper[c] as usize <= level1 && level1 < upper[c + 1] as usize)
            .unwrap()
    };
    let mut global = 0;
    for level in 1..=levels {
        for c in 0..counts[level - 1] as usize {
            let leaving: Vec<u32> = (0..graph.roads.len() as u32)
                .filter(|&road| {
                    let (from, to) = (graph.roads.from(road as usize), graph.roads.to(road as usize));
                    cell(level, from) == c && cell(level, to) != c
                })
                .collect();
            let first = cells.leaving_first.values();
            assert_eq!(&cells.leaving.values()[first[global] as usize..first[global + 1] as usize], leaving);
            global += 1;
        }
    }
    for road in 0..graph.roads.len() {
        let (from, to) = (graph.roads.from(road), graph.roads.to(road));
        let c = cell(1, to);
        assert!((level1[c][1] as usize..level1[c + 1][1] as usize).contains(&road), "road {road} in its cell");
        let border = (1..=levels).filter(|&level| cell(level, from) != cell(level, to)).max().unwrap_or(0);
        assert_eq!(cells.border.values()[road] as usize, border, "road {road}");
    }
    for c in 0..level1.len() - 1 {
        let border = &cells.border.values()[level1[c][1] as usize..level1[c + 1][1] as usize];
        assert!(border.is_sorted_by(|a, b| a >= b), "the entering roads come first in cell {c}");
    }
}

/// Orders the roads of `graph` in one cell with the heights of `height`, and returns the old ID of
/// each road. The tests drop roads, so the turn rules of `fixture` go.
fn order_with_terrain(graph: &mut Graph, mut height: impl FnMut(Point) -> Result<Option<f64>, String>) -> Vec<u32> {
    graph.forbidden = Default::default();
    graph.forbidden_foot = Default::default();
    let junctions = graph.points.len();
    graph.node_ids = (0..junctions as i64).collect();
    graph.node_access = std::iter::repeat_n(0, junctions).collect();
    graph.signals = std::iter::repeat_n(0, junctions).collect();
    graph.node_doubts = std::iter::repeat_n(0, junctions).collect();
    graph.node_closures = std::iter::repeat_n(u32::MAX, junctions).collect();
    planner_router_build::layout::order(graph, &[4096], Some(&mut height)).unwrap().values().to_vec()
}

#[test]
fn terrain_keeps_road_identity_and_heights() {
    let lons = [1_000_000, 0, 500_000, 20_000];
    let mut graph = fixture();
    edit_roads(&mut graph, |roads| {
        roads.truncate(lons.len());
        for (road, lon) in roads.iter_mut().zip(lons) {
            road.shape = [lon, lon + 10].map(|lon| Point { lat: 0, lon, elevation: NO_ELEVATION }).to_vec();
        }
    });
    let identities: Vec<_> = graph.roads.iter().map(|r| (r.way, r.from, r.to)).collect();
    let order = order_with_terrain(&mut graph, |point| {
        Ok(Some(100.0 + point.lon.div_euclid(1000) as f64 + point.lon.rem_euclid(1000) as f64 / 10.0))
    });
    for (road, &old) in graph.roads.iter().zip(&order) {
        let lon = lons[old as usize];
        assert_eq!((road.way, road.from, road.to), identities[old as usize]);
        let base = 100.0 + (lon / 1000) as f32;
        let points: Vec<_> = road.shape.iter().map(|p| (p.lon, p.elevation)).collect();
        assert_eq!(points, [(lon, base), (lon + 10, base + 1.0)]);
        assert_eq!((road.ascent_m, road.descent_m), (1, 0));
    }
}

#[test]
fn terrain_is_direction_independent_preserves_gaps_and_interpolates_structures() {
    let mut graph = fixture();
    edit_roads(&mut graph, |roads| roads.retain(|r| (r.from == 0 && r.to == 1) || (r.from == 1 && r.to == 0)));
    let height = |p: Point| Ok(if (4000..6000).contains(&p.lon) { None } else { Some(100.0 + p.lon as f64 / 100.0) });
    order_with_terrain(&mut graph, height);
    let [a, b] = &roads(&graph)[..] else { panic!("two roads") };
    assert_eq!(
        a.shape.iter().map(|p| p.elevation).collect::<Vec<_>>(),
        b.shape.iter().rev().map(|p| p.elevation).collect::<Vec<_>>()
    );
    assert_eq!(a.ascent_m, b.descent_m);
    assert!(a.shape.iter().any(|p| p.elevation == NO_ELEVATION));
    edit_roads(&mut graph, |roads| roads.iter_mut().for_each(|road| road.structure = true));
    order_with_terrain(&mut graph, height);
    let roads = roads(&graph);
    assert!(roads.iter().flat_map(|r| &r.shape).all(|p| p.elevation != NO_ELEVATION));
    for road in roads {
        assert_eq!((road.ascent_m, road.descent_m), if road.from == 0 { (100, 0) } else { (0, 100) });
    }
}

#[test]
fn closed_packages_route_across_multiple_checked_blocks() {
    let template = roads(&fixture())[0].clone();
    let points: Vec<_> = (0..2200).map(|i| Point { lon: i * 100, lat: 0, elevation: NO_ELEVATION }).collect();
    let mut roads = vec![];
    for i in 0..2199u32 {
        for (a, b) in [(i, i + 1), (i + 1, i)] {
            roads.push(Road {
                from: a,
                to: b,
                way: i as i64,
                shape: vec![points[a as usize], points[b as usize]],
                length_m: 11,
                ..template.clone()
            });
        }
    }
    let graph = Graph {
        points: points.iter().copied().collect(),
        roads: roads.into_iter().collect(),
        forbidden: Default::default(),
        forbidden_foot: Default::default(),
        warnings: vec![],
        ..Graph::default()
    };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    assert!(router.package().package().manifest().graph.first.blocks.len() > 1);
    let request = Request {
        points: vec![[0.00002, 0.0], [0.21988, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: true,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let result = router.routes(&request, &Control::default()).unwrap();
    assert_eq!(result.routes.len(), 1, "A single corridor does not need invented alternatives");
    let route = &result.routes[0];
    assert_eq!(route.cost, oracle(&graph, &Profile::presets()[0], route.legs[0].start, route.legs[0].end).unwrap());
    assert_eq!(route.legs[0].pieces.len(), 2199);
}

#[test]
fn a_leg_adds_the_sub_metre_climb_of_its_pieces_before_it_rounds() {
    let template = roads(&fixture())[0].clone();
    let points: Vec<_> = (0..11).map(|i| Point { lon: i * 100, lat: 0, elevation: i as f32 * 0.4 }).collect();
    let mut roads = vec![];
    for i in 0..10u32 {
        for (a, b) in [(i, i + 1), (i + 1, i)] {
            roads.push(Road {
                from: a,
                to: b,
                way: i as i64,
                shape: vec![points[a as usize], points[b as usize]],
                length_m: 11,
                ..template.clone()
            });
        }
    }
    let graph =
        Graph { points: points.iter().copied().collect(), roads: roads.into_iter().collect(), ..Graph::default() };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let request = Request {
        points: vec![[0.00002, 0.0], [0.00098, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.legs[0].pieces.len(), 10);
    // 3.84 m from rises of 0.4 m each, so every piece alone rounds to 0.
    assert_eq!((route.totals().ascent_m, route.totals().descent_m), (4, 0));
    let rise: f64 = route.points.windows(2).map(|p| (p[1].elevation as f64 - p[0].elevation as f64).max(0.0)).sum();
    assert!((rise - route.totals().ascent_m as f64).abs() <= 1.0);
}

#[test]
fn each_leg_smooths_its_own_heights() {
    let template = roads(&fixture())[0].clone();
    // Heights that alternate by ±0.5 m every 15 m, and climb at 2 % in the second half.
    let shape: Vec<_> = (0..=100)
        .map(|i| Point { lon: i * 135, lat: 0, elevation: (i % 2) as f32 - 0.5 + 0.3 * (i - 50).max(0) as f32 })
        .collect();
    let mut back = shape.clone();
    back.reverse();
    let roads = [(0, 1, shape.clone()), (1, 0, back)].map(|(from, to, shape)| Road {
        from,
        to,
        way: 0,
        length_m: 1_500,
        shape,
        ..template.clone()
    });
    let graph = Graph {
        points: [shape[0], shape[100]].into_iter().collect(),
        roads: roads.into_iter().collect(),
        ..Graph::default()
    };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 << 20);
    let mut route = |points: &[[f64; 2]]| {
        let request = Request {
            points: points.to_vec(),
            profile: "touring".into(),
            pace: Pace::default(),
            alternatives: false,
            alternatives_only: false,
            turnarounds: vec![],
            start_position: None,
            end_position: None,
        };
        router.route(&request, &Control::default()).unwrap()
    };
    let (start, middle, end) = ([0.0002, 0.0], [0.00681, 0.0], [0.0133, 0.0]);
    let whole = route(&[start, middle, end]);
    let raw: f32 = shape.windows(2).map(|p| (p[1].elevation - p[0].elevation).max(0.0)).sum();
    // The 2 % climb rises about 15 m; the noise adds most of the raw rises.
    assert!((14.0..raw / 2.0).contains(&(whole.totals().ascent_m as f32)));
    let mut seconds = 0.0;
    for (leg, ends) in whole.legs.iter().zip([[start, middle], [middle, end]]) {
        let alone = route(&ends).totals();
        assert_eq!((leg.totals.ascent_m, leg.totals.descent_m), (alone.ascent_m, alone.descent_m));
        assert!((leg.totals.seconds - alone.seconds).abs() < 1e-6);
        seconds += leg.totals.seconds;
        assert!((whole.elapsed[leg.to_index] - seconds).abs() < 1e-6);
    }
}

#[test]
fn alternatives_find_a_separate_corridor_without_an_out_and_back_probe() {
    let template = roads(&fixture())[0].clone();
    let points: Vec<_> = [(0, 0), (0, 9000), (60000, 9000), (60000, 0), (0, -9000), (60000, -9000)]
        .into_iter()
        .map(|(lon, lat)| Point { lon, lat, elevation: NO_ELEVATION })
        .collect();
    let mut roads = vec![];
    for (a, b) in [(0, 1), (1, 2), (2, 3), (0, 4), (4, 5), (5, 3)] {
        for (from, to) in [(a, b), (b, a)] {
            roads.push(Road {
                from: from as u32,
                to: to as u32,
                way: (a * 10 + b) as i64,
                shape: vec![points[from], points[to]],
                length_m: points[from].distance(points[to]).round() as u32,
                ..template.clone()
            });
        }
    }
    let graph = Graph {
        points: points.iter().copied().collect(),
        roads: roads.into_iter().collect(),
        forbidden: Default::default(),
        forbidden_foot: Default::default(),
        warnings: vec![],
        ..Graph::default()
    };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let request = Request {
        points: vec![[0.0, 0.0], [0.06, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: true,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let routes = router.routes(&request, &Control::default()).unwrap().routes;
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[1].reason, "corridor");
    assert_eq!(routes[1].legs.len(), 1);
    assert_ne!(routes[0].id, routes[1].id);
    assert_eq!(routes[0].totals().distance_m, routes[1].totals().distance_m);
    // One shaping point keeps the corridor.
    assert!(routes[0].via.is_none());
    let via = Request { points: vec![request.points[0], routes[1].via.unwrap(), request.points[1]], ..request.clone() };
    assert_eq!(router.route(&via, &Control::default()).unwrap().points, routes[1].points);
    let only = Request { alternatives: false, alternatives_only: true, ..request };
    let alternatives = router.routes(&only, &Control::default()).unwrap().routes;
    assert_eq!(alternatives.iter().map(|r| &r.id).collect::<Vec<_>>(), [&routes[1].id]);
}

#[test]
fn an_out_and_back_trip_gets_alternatives_within_one_query_budget() {
    let template = roads(&fixture())[0].clone();
    // A short cycleway crosses a hill; a longer, level cycleway goes through (4).
    let points: Vec<_> = [(-2_500, 0), (0, 0), (45_000, 0), (47_500, 0), (22_500, 22_000)]
        .into_iter()
        .map(|(lon, lat)| Point { lon, lat, elevation: 0.0 })
        .collect();
    let mut roads = vec![];
    for (a, b) in [(0, 1), (1, 2), (1, 4), (4, 2), (2, 3)] {
        for (from, to) in [(a, b), (b, a)] {
            let hill = a == 1 && b == 2;
            let mut shape = vec![points[from], points[to]];
            if hill {
                shape.insert(1, Point { lon: 22_500, lat: 0, elevation: 100.0 });
            }
            roads.push(Road {
                from: from as u32,
                to: to as u32,
                class: 0,
                surface: Surface::Paved,
                shape,
                length_m: points[from].distance(points[to]).round() as u32,
                ascent_m: if hill { 100 } else { 0 },
                descent_m: if hill { 100 } else { 0 },
                ..template.clone()
            });
        }
    }
    let graph = Graph {
        points: points.iter().copied().collect(),
        roads: roads.into_iter().collect(),
        forbidden: Default::default(),
        forbidden_foot: Default::default(),
        warnings: vec![],
        ..Graph::default()
    };
    let profiles: Vec<_> = Profile::presets().into_iter().filter(|p| p.name.starts_with("touring")).collect();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let request = Request {
        points: vec![[-0.0015, 0.0], [0.0465, 0.0], [-0.0015, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: true,
        alternatives_only: false,
        turnarounds: vec![1],
        start_position: None,
        end_position: None,
    };
    let routes = router.routes(&request, &Control::default()).unwrap().routes;
    assert_eq!(routes.iter().map(|r| r.reason).collect::<Vec<_>>(), ["primary", "less_climbing"]);
    assert_eq!(routes[1].profile, "touring/less-climbing");
    // The mean over 100 m lowers the summit between two 4 % slopes by 1 m.
    assert_eq!((routes[0].totals().ascent_m, routes[1].totals().ascent_m), (198, 0));
    // The primary route spends the whole query budget, so the alternatives stop and the answer keeps it.
    let alone = Request { alternatives: false, ..request.clone() };
    let needed = (1..)
        .find(|&queries| router.route(&alone, &Control { max_queries: queries, ..Control::default() }).is_ok())
        .unwrap();
    // A new router, because a router answers a repeated request from its last primary route.
    let mut fresh = Router::new(router.package().fork(), 768 * 1024 * 1024);
    let kept = fresh.routes(&request, &Control { max_queries: needed, ..Control::default() }).unwrap().routes;
    assert_eq!(kept.iter().map(|r| &r.id).collect::<Vec<_>>(), [&routes[0].id]);
}

#[test]
fn partial_cost_localizes_climbing_and_telescopes_across_shape_points() {
    let mut road = roads(&fixture())[0].clone();
    let mut midpoint = road.shape[0];
    midpoint.lon = (road.shape[0].lon + road.shape[1].lon) / 2;
    midpoint.elevation = 100.0;
    road.shape[0].elevation = 0.0;
    road.shape[1].elevation = 100.0;
    road.shape.insert(1, midpoint);
    road.ascent_m = 100;
    let profile = Profile::presets().remove(0);
    let mut graph = fixture();
    graph.roads = [road].into_iter().collect();
    let graph = with_sources(graph);
    let mut dictionary = Dictionary::default();
    let costing = costing(&graph, &profile, &mut dictionary);
    let curves = curves(&graph, &costing, &profile);
    let curve = curves[0].as_ref().unwrap();
    let full = curve.total();
    assert!(curve.prefix(0.5).unwrap() > full / 2);
    let costs: Vec<_> = [0.0, 0.1, 0.5, 0.9, 1.0].iter().map(|&f| curve.prefix(f).unwrap()).collect();
    assert!(costs.is_sorted());
    assert_eq!(costs.windows(2).map(|c| c[1] - c[0]).sum::<u64>(), full);
}

#[test]
fn geometry_cache_retains_a_snap_working_set_across_many_small_pages() {
    use planner_router::package::Manifest;
    use std::cell::Cell;
    struct Counted<'a>(Memory, &'a Cell<usize>);
    impl Source for Counted<'_> {
        fn read(&self, key: &str) -> planner_router::Result<Vec<u8>> {
            self.1.set(self.1.get() + 1);
            self.0.read(key)
        }
    }
    let graph = fixture();
    let (source, bytes) = package(&graph);
    let mut objects = (*source.0).clone();
    let mut manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
    manifest.roads = 20 * 128;
    manifest.landmarks = None;
    let mut geometry = Vec::new();
    for page in 0..20 {
        let road = Road { way: page, ..roads(&graph)[0].clone() };
        let bytes = planner_router::geometry::encode(&vec![road; 128]).unwrap();
        let key = digest(&bytes);
        geometry.push(key.clone());
        objects.insert(key, bytes);
    }
    let mut write = |bytes: &[u8]| {
        let key = digest(bytes);
        objects.insert(key.clone(), bytes.to_vec());
        Ok(key)
    };
    manifest.geometry = planner_router::table::Table::write(&geometry, &mut write).unwrap();
    manifest.graph =
        planner_router::base::write_topology(&vec![0; manifest.roads as usize + 1], &[], &mut write).unwrap();
    for metric in manifest.metrics.values_mut() {
        metric.weights = planner_router::base::write_weights(vec![1; manifest.roads as usize], [], &mut write).unwrap();
        metric.allowed = planner_router::table::Table::write(&vec![u64::MAX; 40], &mut write).unwrap();
        metric.costs = planner_router::table::Table::write(&vec![1u32; manifest.roads as usize], &mut write).unwrap();
    }
    let reads = Cell::new(0);
    let package =
        Package::open(Counted(Memory(Arc::new(objects)), &reads), &serde_json::to_vec(&manifest).unwrap()).unwrap();
    for _ in 0..2 {
        for page in 0..20 {
            assert_eq!(package.road(page * 128).unwrap().way, page as i64);
        }
    }
    assert_eq!(reads.get(), 21);
}

#[test]
fn disconnected_driveway_uses_a_nearby_connected_road_without_relaxing_the_profile() {
    // A road in two segments, and a driveway that only a grade 1 path joins to it.
    let mut graph = fixture();
    graph.points = [(0, 0), (0, 5_000), (0, 10_000), (100, 4_000), (100, 6_000)]
        .map(|(lat, lon)| Point { lat, lon, elevation: NO_ELEVATION })
        .into_iter()
        .collect();
    let template = roads(&graph)[0].clone();
    graph.roads = [(0, 1, 0), (1, 0, 0), (1, 2, 0), (2, 1, 0), (3, 4, 0), (4, 3, 0), (0, 3, 1), (3, 0, 1)]
        .map(|(from, to, difficulty)| {
            let shape = vec![graph.points.values()[from as usize], graph.points.values()[to as usize]];
            Road {
                from,
                to,
                difficulty,
                surface: Surface::Paved,
                length_m: shape[0].distance(shape[1]).round() as u32,
                shape,
                ..template.clone()
            }
        })
        .into_iter()
        .collect();
    graph.forbidden = Default::default();
    graph.forbidden_foot = Default::default();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![[0.0, 0.0], [0.005, 0.0002], [0.01, 0.0]],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![1],
        start_position: None,
        end_position: None,
    };
    let route = router.route(&request, &Control::default()).unwrap();
    let (driveway, distance) = attachment(&route, &request, 1);
    assert_eq!(driveway.lat, 0);
    assert!(distance < 25.0);
    assert_eq!(attachment(&route, &request, 0).1, 0.0);
    assert_eq!(attachment(&route, &request, 2).1, 0.0);
    request.profile = "mtb".into();
    let mtb = router.route(&request, &Control::default()).unwrap();
    assert_eq!(attachment(&mtb, &request, 1).0.lat, 100);
    // The driveway is a fragment of the road profile's graph, so a point on it is not a candidate
    // there: the first search already attaches it to the connected road.
    request.profile = "road".into();
    request.points[1][1] = 0.0001;
    let snapped = router.package().snap(
        Point { lon: 5_000, lat: 100, elevation: NO_ELEVATION },
        "road",
        Policy { radius_m: REACH_M, ..Policy::default() },
    );
    assert!(snapped.unwrap().retained.iter().all(|c| c.projected.lat == 0));
    let route = router.route(&request, &Control::default()).unwrap();
    let (driveway, distance) = attachment(&route, &request, 1);
    assert_eq!(driveway.lat, 0);
    assert!((10.0..12.0).contains(&distance));
}

#[test]
fn road_and_gravel_bikes_take_their_own_surfaces() {
    let mut graph = fixture();
    graph.points = [(0, 0), (0, 10_000), (15_000, 5_000)]
        .map(|(lat, lon)| Point { lat, lon, elevation: NO_ELEVATION })
        .into_iter()
        .collect();
    let template = roads(&graph)[0].clone();
    graph.roads = [(0, 1), (1, 0), (0, 2), (2, 0), (2, 1), (1, 2)]
        .map(|(from, to)| {
            let shape = vec![graph.points.values()[from as usize], graph.points.values()[to as usize]];
            Road {
                from,
                to,
                surface: if from + to == 1 { Surface::Gravel } else { Surface::Paved },
                length_m: shape[0].distance(shape[1]).round() as u32,
                shape,
                ..template.clone()
            }
        })
        .into_iter()
        .collect();
    graph.forbidden = Default::default();
    graph.forbidden_foot = Default::default();
    let profiles: Vec<_> =
        Profile::presets().into_iter().filter(|p| ["road", "gravel"].contains(&p.name.as_str())).collect();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![[0.0, 0.0], [0.01, 0.0]],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.totals().surface_m[Surface::Gravel as usize], 0);
    assert!(route.totals().surface_m[Surface::Paved as usize] > 3000);
    request.profile = "gravel".into();
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.totals().surface_m[Surface::Paved as usize], 0);
    assert!(route.totals().distance_m < 1200);
}

#[test]
fn riding_bans_allow_a_pushing_connection_unless_pushing_is_also_banned() {
    let points: Vec<_> = (0..4).map(|i| Point { lon: i * 1000, lat: 0, elevation: 0.0 }).collect();
    let mut roads = Vec::new();
    for i in 0..3 {
        for (from, to) in [(i, i + 1), (i + 1, i)] {
            roads.push(Road {
                from,
                to,
                way: i as i64,
                reversed: from > to,
                length_m: 111,
                ascent_m: 0,
                descent_m: 0,
                surface: Surface::Paved,
                class: 1,
                difficulty: 0,
                hiking_difficulty: None,
                structure: false,
                access: if i == 1 {
                    planner_router_build::source::access(
                        |key| match key {
                            "highway" => Some("footway"),
                            "bicycle" => Some("no"),
                            _ => None,
                        },
                        FOOT | PUSH,
                        "forward",
                        true,
                    )
                } else {
                    BIKE | FOOT | PUSH
                },
                shape: vec![points[from as usize], points[to as usize]],
            });
        }
    }
    let mut graph =
        Graph { points: points.iter().copied().collect(), roads: roads.into_iter().collect(), ..Graph::default() };
    let request = Request {
        points: vec![[0.0001, 0.0], [0.0029, 0.0]],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.totals().pushing_m, 111);
    assert_eq!(edges(&route, "pushing"), json!([false, true, false]));
    edit_roads(&mut graph, |roads| roads.iter_mut().for_each(|road| road.access &= !PUSH));
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    assert!(router.route(&request, &Control::default()).is_err());
}

#[test]
fn a_route_reports_its_possible_closures_only_for_the_mode_it_uses() {
    let points: Vec<_> = (0..4).map(|i| Point { lon: i * 1000, lat: 0, elevation: 0.0 }).collect();
    let roads = (0..3u32)
        .flat_map(|i| [(i, i + 1), (i + 1, i)])
        .map(|(from, to)| Road {
            from,
            to,
            way: from.min(to) as i64,
            reversed: from > to,
            length_m: 111,
            ascent_m: 0,
            descent_m: 0,
            surface: Surface::Paved,
            class: 1,
            access: BIKE | FOOT | PUSH,
            difficulty: if from.min(to) == 1 { 0 } else { 255 },
            hiking_difficulty: (from.min(to) == 1).then_some(2),
            structure: false,
            shape: vec![points[from as usize], points[to as usize]],
        })
        .collect();
    let node_ids = (0..4).collect();
    let node_access = std::iter::repeat_n(BIKE | FOOT | PUSH, 4).collect();
    let points = points.iter().copied().collect();
    let mut graph = Graph { points, roads, node_ids, node_access, ..Graph::default() };
    let mut sources = Builder::<Ways>::default();
    for way in 0..3 {
        let closures =
            [("access", "permit"), ("bicycle:conditional", "no @ (Nov-May)"), ("foot:conditional", "no @ (wet)")];
        let tags = std::iter::once(("highway", "secondary")).chain(if way == 1 { closures.to_vec() } else { vec![] });
        sources.push(way, &[way, way + 1], &Tags::from_pairs(tags));
    }
    graph.osm.ways = sources.finish().0;
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![[0.0001, 0.0], [0.0029, 0.0]],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    use planner_router::closures::{Closure, Kind};
    let closure = |kind, condition: &str| Closure { kind, condition: condition.into() };
    let permit = closure(Kind::Permit, "permit");
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(edges(&route, "closures"), json!([null, [permit, closure(Kind::Seasonal, "Nov-May")], null]));
    request.profile = "hiking".into();
    let route = router.route(&request, &Control::default()).unwrap();
    let walking = json!([permit, closure(Kind::Conditional, "wet")]);
    assert_eq!(edges(&route, "closures"), json!([null, walking, null]));
    // The grades travel with the road geometry, like the surface.
    assert_eq!(edges(&route, "sac_scale"), json!([null, 2, null]));
    assert_eq!(route.pieces().map(|p| p.mtb_scale).collect::<Vec<_>>(), [None, Some(0), None]);
}

#[test]
fn a_route_avoids_a_private_road_unless_a_shaping_point_is_on_it() {
    // A private road from A to B, 222 m, and an open way round it through C, 259 m.
    let at = |lon, lat| Point { lon, lat, elevation: 0.0 };
    let points = [at(-1000, 0), at(0, 0), at(2000, 0), at(3000, 0), at(1000, 600)];
    let ways = [(0usize, 1usize), (1, 2), (1, 4), (4, 2), (2, 3)];
    let roads = ways
        .iter()
        .enumerate()
        .flat_map(|(way, &(a, b))| [(way, a, b, false), (way, b, a, true)])
        .map(|(way, from, to, reversed)| Road {
            from: from as u32,
            to: to as u32,
            way: way as i64,
            reversed,
            length_m: points[from].distance(points[to]).round() as u32,
            ascent_m: 0,
            descent_m: 0,
            surface: Surface::Paved,
            class: 1,
            access: BIKE | FOOT | PUSH,
            difficulty: 255,
            hiking_difficulty: None,
            structure: false,
            shape: vec![points[from], points[to]],
        })
        .collect();
    let mut graph = Graph {
        points: points.iter().copied().collect(),
        roads,
        node_ids: (0..5).collect(),
        node_access: [BIKE | FOOT | PUSH; 5].into_iter().collect(),
        ..Graph::default()
    };
    let mut sources = Builder::<Ways>::default();
    for (way, &(a, b)) in ways.iter().enumerate() {
        let access = if way == 1 { "private" } else { "yes" };
        let tags = Tags::from_pairs([("highway", "service"), ("access", access)]);
        sources.push(way as i64, &[a as i64, b as i64], &tags);
    }
    graph.osm.ways = sources.finish().0;
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![[-0.0005, 0.0], [0.0025, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let private = json!([{ "kind": "private", "condition": "private" }]);
    let open = router.route(&request, &Control::default()).unwrap();
    assert!(!edges(&open, "closures").as_array().unwrap().contains(&private));
    request.points.insert(1, [0.001, 0.0]);
    let shaped = router.route(&request, &Control::default()).unwrap();
    assert!(edges(&shaped, "closures").as_array().unwrap().contains(&private));
    assert!(shaped.totals().distance_m < open.totals().distance_m);
}

#[test]
fn a_spur_that_ends_at_a_closed_gate_stays_snappable_through_its_u_turn() {
    // A junction with a road west, a road north and a 222 m spur east that ends at a gate.
    let at = |lon, lat| Point { lon, lat, elevation: 0.0 };
    let points = [at(-2000, 0), at(0, 0), at(2000, 0), at(0, 2000)];
    let ways = [(0usize, 1usize), (1, 2), (1, 3)];
    let roads = ways
        .iter()
        .enumerate()
        .flat_map(|(way, &(a, b))| [(way, a, b, false), (way, b, a, true)])
        .map(|(way, from, to, reversed)| Road {
            from: from as u32,
            to: to as u32,
            way: way as i64,
            reversed,
            length_m: points[from].distance(points[to]).round() as u32,
            ascent_m: 0,
            descent_m: 0,
            surface: Surface::Paved,
            class: 1,
            access: BIKE | FOOT | PUSH,
            difficulty: 255,
            hiking_difficulty: None,
            structure: false,
            shape: vec![points[from], points[to]],
        })
        .collect();
    let open = BIKE | FOOT | PUSH;
    let mut graph = Graph {
        points: points.iter().copied().collect(),
        roads,
        node_ids: (0..4).collect(),
        node_access: [open, open, 0, open].into_iter().collect(),
        ..Graph::default()
    };
    let mut sources = Builder::<Ways>::default();
    for (way, &(a, b)) in ways.iter().enumerate() {
        sources.push(way as i64, &[a as i64, b as i64], &Tags::from_pairs([("highway", "service")]));
    }
    graph.osm.ways = sources.finish().0;
    let (source, manifest) = package(&graph);
    let mut router = Router::new(routing(source, &manifest), 768 * 1024 * 1024);
    for id in 0..6 {
        assert!(router.package().package().allowed("touring", id).unwrap(), "road {id}");
    }
    // A point 55 m before the gate ends the route there instead of at the junction.
    let request = Request {
        points: vec![[-0.0015, 0.0], [0.0015, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
        start_position: None,
        end_position: None,
    };
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(attachment(&route, &request, 1).1, 0.0);
    let distance = route.totals().distance_m;
    assert!((330..=338).contains(&distance), "{distance}");
    let back = Request { points: vec![[0.0015, 0.0], [-0.0015, 0.0]], ..request };
    assert_eq!(attachment(&router.route(&back, &Control::default()).unwrap(), &back, 0).1, 0.0);
}

#[test]
fn shared_pages_preserve_routes_costs_and_guidance_for_every_profile() {
    use planner_router::search::{Query, Seed, Workspace};
    let (source, bytes) = package_with_profiles(&fixture(), &Profile::presets());
    let original = routing(source.clone(), &bytes);
    let n = original.package().manifest().roads;
    let bounds = original.package().manifest().bounds;
    let make = |ids: &[u32]| {
        let selection = planner_router_build::blocks::prepare(original.package(), bounds, ids).unwrap();
        let objects = selection.objects.iter().map(|key| (key.clone(), source.read(key).unwrap())).collect();
        let selected =
            Selection::open(Memory(Arc::new(objects)), &serde_json::to_vec(&selection.manifest).unwrap()).unwrap();
        (selection, selected)
    };
    let (full, full_package) = make(&(0..n).collect::<Vec<_>>());
    let selected: Vec<_> = (0..n).filter(|&i| i < n / 3 || i >= 2 * n / 3).collect();
    let (_, mut partial) = make(&selected);
    let mut full_router = Router::new(full_package, 768 * 1024 * 1024);
    let mut original_router = Router::new(original.fork(), 768 * 1024 * 1024);
    let names: Vec<_> = original.package().manifest().metrics.keys().cloned().collect();
    let mut saw_forbidden = false;
    for name in names {
        for points in [vec![[0.001, 0.0], [0.02, 0.011]], vec![[0.02, 0.011], [0.001, 0.0]]] {
            let request = Request {
                points,
                profile: name.clone(),
                pace: Pace::default(),
                alternatives: false,
                alternatives_only: false,
                turnarounds: vec![],
                start_position: None,
                end_position: None,
            };
            let before = original_router.route(&request, &Control::default()).unwrap();
            let after = full_router.route(&request, &Control::default()).unwrap();
            assert_eq!(after.cost, before.cost);
            assert_eq!(after.points, before.points);
            assert_eq!(after.elapsed, before.elapsed);
        }
        let whole = original.prepared(&name).unwrap();
        let (graph, costs) = (&whole.graph, &whole.costs);
        let part = partial.prepared(&name).unwrap();
        let (joined, joined_costs) = (&part.graph, &part.costs);
        let mut masked_roads: Vec<u64> = (0..n).map(|i| costs.roads.get(i as usize)).collect();
        for id in 0..n {
            if partial.local_id(id).is_none() {
                masked_roads[id as usize] = u64::MAX;
            }
        }
        let masked = planner_router::base::Costs {
            roads: planner_router::base::Numbers::U64(masked_roads),
            turns: costs.turns.clone(),
        };
        let mut expected_work = Workspace::default();
        let mut actual_work = Workspace::default();
        for from in 0..joined.nodes() as u32 {
            let global = partial.source_id(from).unwrap();
            assert_eq!(partial.local_id(global), Some(from));
            assert_eq!(
                partial.with_road(from, |road| serde_json::to_value(road).unwrap()).unwrap(),
                original.with_road(global, |road| serde_json::to_value(road).unwrap()).unwrap()
            );
            let expected: Vec<_> = (graph.first[global as usize]..graph.first[global as usize + 1])
                .filter_map(|arc| {
                    partial.local_id(graph.head[arc as usize]).map(|to| (to, costs.arc(graph, arc as usize)))
                })
                .collect();
            let actual: Vec<_> = (joined.first[from as usize]..joined.first[from as usize + 1])
                .map(|arc| (joined.head[arc as usize], joined_costs.arc(joined, arc as usize)))
                .collect();
            saw_forbidden |= actual.iter().any(|(_, c)| *c == u64::MAX);
            assert_eq!(actual, expected);
            for to in 0..joined.nodes() as u32 {
                let starts = [Seed { node: from, cost: 7, choice: 0 }];
                let ends = [Seed { node: to, cost: 11, choice: 0 }];
                let source_starts = [Seed { node: global, ..starts[0] }];
                let source_ends = [Seed { node: partial.source_id(to).unwrap(), ..ends[0] }];
                let query = Query {
                    starts: &starts,
                    ends: &ends,
                    ceiling: u64::MAX,
                    max_roads: usize::MAX,
                    heap_bytes: 1_000_000,
                    heap_limit: 1_000_000,
                    cancelled: &|| false,
                };
                let expected = expected_work
                    .run(graph, &masked, Query { starts: &source_starts, ends: &source_ends, ..query })
                    .unwrap();
                let guidance = part.guide.as_ref().unwrap().potential(&starts, &ends).unwrap();
                let source_guidance = whole.guide.as_ref().unwrap().potential(&source_starts, &source_ends).unwrap();
                for local in 0..joined.nodes() as u32 {
                    assert_eq!(guidance.get(local), source_guidance.get(partial.source_id(local).unwrap()));
                }
                let actual = actual_work.run_with_potential(joined, joined_costs, query, Some(&guidance)).unwrap();
                assert_eq!(actual.as_ref().map(|r| r.cost), expected.as_ref().map(|r| r.cost));
            }
        }
    }
    assert!(saw_forbidden);
    let mut incomplete = full.manifest.clone();
    incomplete.data.graph.head.retain(&Default::default()).unwrap();
    let missing = Selection::open(source.clone(), &serde_json::to_vec(&incomplete).unwrap()).unwrap();
    assert!(matches!(missing.prepared("touring"), Err(Error::MissingRegion(_))));
    incomplete = full.manifest.clone();
    incomplete.roads.push(incomplete.roads[0]);
    assert!(Selection::open(source, &serde_json::to_vec(&incomplete).unwrap()).is_err());
    let cached = partial.prepared("touring").unwrap();
    partial.set_memory_budget(partial.routing_bytes("touring").unwrap());
    assert!(Arc::ptr_eq(&cached, &partial.prepared("touring").unwrap()));
    partial.set_memory_budget(1);
    assert!(matches!(partial.prepared("hiking"), Err(Error::Limit)));
}

#[test]
fn sparse_tables_keep_source_page_numbers_and_last_page_length() {
    use planner_router::table::{Cache, Table, ENTRIES};
    let (source, manifest) = package(&fixture());
    let mut objects = (*source.0).clone();
    let values: Vec<u32> = (0..(ENTRIES * 3 + 17) as u32).collect();
    let mut table = Table::write(&values, &mut |bytes| {
        let key = digest(bytes);
        objects.insert(key.clone(), bytes.to_vec());
        Ok(key)
    })
    .unwrap();
    table.retain(&[1, 3].into()).unwrap();
    assert!(table.valid());
    let input = Package::open(Memory(Arc::new(objects)), &manifest).unwrap();
    let mut cache = Cache::<u32>::default();
    assert_eq!(&*cache.block(&input, &table, 1).unwrap(), &values[ENTRIES..ENTRIES * 2]);
    assert_eq!(&*cache.block(&input, &table, 3).unwrap(), &values[ENTRIES * 3..]);
    assert!(matches!(cache.block(&input, &table, 2), Err(Error::MissingRegion(_))));
    table.pages = Some(vec![3, 1]);
    assert!(!table.valid());
}
