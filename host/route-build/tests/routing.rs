use route_engine::{
    model::{Graph, Pace, Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION},
    package::{digest, Package, Source},
    snap::{prefix_cost, Candidate, Policy},
    Control, Error, Request, Router,
};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
    sync::Arc,
};

#[derive(Clone)]
struct Memory(Arc<HashMap<String, Vec<u8>>>);
impl Source for Memory {
    fn read(&self, key: &str) -> route_engine::Result<Vec<u8>> {
        self.0.get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
    }
}
fn package(graph: &Graph) -> (Memory, Vec<u8>) {
    let mut objects = HashMap::new();
    let profiles = Profile::presets();
    let manifest =
        route_build::prepare(graph, "test".into(), [-1.0, -1.0, 1.0, 1.0], &profiles[..1], vec![], |bytes| {
            let key = digest(bytes);
            objects.insert(key.clone(), bytes.to_vec());
            Ok(key)
        })
        .unwrap();
    (Memory(Arc::new(objects)), serde_json::to_vec(&manifest).unwrap())
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
                    length_m: points[from as usize].distance(points[to as usize]).round() as u32,
                    ascent_m: 0,
                    descent_m: 0,
                    surface: if from % 2 == 0 { Surface::Gravel } else { Surface::Paved },
                    class: 1,
                    access: BIKE | FOOT,
                    difficulty: 0,
                    hiking_difficulty: None,
                    uncertain_access: false,
                    structure: false,
                    shape: vec![points[from as usize], points[to as usize]],
                });
            }
        }
    }
    let forbidden = (0..roads.len())
        .flat_map(|a| {
            let roads = &roads;
            (0..roads.len())
                .filter_map(move |b| (roads[a].to == roads[b].from && (a + b) % 7 == 0).then_some((a as u32, b as u32)))
        })
        .collect();
    Graph { points, roads, forbidden, forbidden_foot: vec![], warnings: vec![] }
}
// Independent arrival-road Dijkstra. The target is a partial transition, not a compact state.
fn oracle(graph: &Graph, profile: &Profile, from: &Candidate, to: &Candidate) -> Option<u64> {
    let (a, b) = (from.position, to.position);
    let prefix = prefix_cost(&graph.roads[a.road as usize], profile, a.fraction).unwrap();
    let suffix = prefix_cost(&graph.roads[b.road as usize], profile, b.fraction).unwrap();
    let mut best = if a.road == b.road && a.fraction <= b.fraction { Some(suffix - prefix) } else { None };
    let mut dist = vec![u64::MAX; graph.roads.len()];
    dist[a.road as usize] = profile.cost(&graph.roads[a.road as usize]).unwrap() - prefix;
    let mut heap = BinaryHeap::from([Reverse((dist[a.road as usize], a.road))]);
    while let Some(Reverse((cost, road))) = heap.pop() {
        if cost != dist[road as usize] {
            continue;
        }
        for (next, r) in graph.roads.iter().enumerate() {
            if graph.roads[road as usize].to != r.from || !graph.permits_turn(road, next as u32, profile.walking) {
                continue;
            }
            if next as u32 == b.road {
                best = Some(best.unwrap_or(u64::MAX).min(cost + suffix));
            }
            let Some(weight) = profile.cost(r) else {
                continue;
            };
            if cost + weight < dist[next] {
                dist[next] = cost + weight;
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
    let graph = fixture();
    let profile = Profile::presets().remove(0);
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 4 * 1024 * 1024);
    let coords: Vec<_> = graph.roads.iter().step_by(3).flat_map(|r| [coordinate(r, 0.2), coordinate(r, 0.8)]).collect();
    for &from in &coords {
        for &to in &coords {
            let mut snaps = Vec::new();
            for [lon, lat] in [from, to] {
                snaps.push(
                    router
                        .snap(
                            Point {
                                lon: (lon * 1e6).round() as i32,
                                lat: (lat * 1e6).round() as i32,
                                elevation: NO_ELEVATION,
                            },
                            "touring",
                            Policy::default(),
                        )
                        .unwrap(),
                );
            }
            let expected = snaps[0]
                .retained
                .iter()
                .flat_map(|a| snaps[1].retained.iter().filter_map(|b| oracle(&graph, &profile, a, b)))
                .min();
            let actual = router.route(
                &Request {
                    points: vec![from, to],
                    profile: "touring".into(),
                    pace: Pace::default(),
                    alternatives: false,
                    turnarounds: vec![],
                },
                &Control::default(),
            );
            match (expected, actual) {
                (Some(cost), Ok(route)) => {
                    assert_eq!(cost, route.cost, "{from:?} -> {to:?}");
                    for pair in route.legs[0].roads.windows(2) {
                        assert_eq!(graph.roads[pair[0].road as usize].to, graph.roads[pair[1].road as usize].from);
                        assert!(graph.permits_turn(pair[0].road, pair[1].road, false));
                    }
                }
                (None, Err(Error::NoPath)) => {}
                (expected, actual) => panic!("expected {expected:?}, got {actual:?}"),
            }
        }
    }
}
#[test]
fn via_direction_pace_and_failure_states_are_explicit() {
    let graph = fixture();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source.clone(), &manifest).unwrap(), 4 * 1024 * 1024);
    let mut request = Request {
        points: vec![
            coordinate(&graph.roads[0], 0.2),
            coordinate(&graph.roads[7], 0.5),
            coordinate(&graph.roads[15], 0.8),
        ],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        turnarounds: vec![],
    };
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.attachments.len(), 3);
    for pair in route.legs.windows(2) {
        let (a, b) = (pair[0].roads.last().unwrap(), pair[1].roads.first().unwrap());
        assert_eq!(a.road, b.road);
        assert_eq!(a.to, b.from);
    }
    request.pace.personal_multiplier = 1.5;
    let slower = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.cost, slower.cost);
    assert_eq!(route.geometry, slower.geometry);
    assert!((route.totals.seconds * 1.5 - slower.totals.seconds).abs() < 1e-6);
    assert!(matches!(
        router.route(&request, &Control { cancelled: &|| true, ..Control::default() }),
        Err(Error::Cancelled)
    ));
    assert!(matches!(router.route(&request, &Control { max_queries: 0, ..Control::default() }), Err(Error::Limit)));
    request.points[0] = [10.0, 10.0];
    assert!(matches!(router.route(&request, &Control::default()), Err(Error::MissingRegion(_))));
    let mut broken = (*source.0).clone();
    let key = router.package().manifest().geometry[0].clone();
    broken.get_mut(&key).unwrap()[0] ^= 1;
    let mut package = Package::open(Memory(Arc::new(broken)), &manifest).unwrap();
    assert!(matches!(package.road(0), Err(Error::InvalidData(_))));
}

#[test]
fn a_shape_keeps_direction_and_only_an_explicit_visit_can_reverse() {
    let mut graph = fixture();
    graph.roads.retain(|r| (r.from == 0 && r.to == 1) || (r.from == 1 && r.to == 0));
    graph.forbidden.clear();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 1024 * 1024);
    let mut request = Request {
        points: vec![
            coordinate(&graph.roads[0], 0.2),
            coordinate(&graph.roads[0], 0.8),
            coordinate(&graph.roads[0], 0.2),
        ],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        turnarounds: vec![],
    };
    let shaped = router.route(&request, &Control::default()).unwrap();
    request.turnarounds = vec![1];
    let visit = router.route(&request, &Control::default()).unwrap();
    assert!(shaped.totals.distance_m > visit.totals.distance_m + 400);
    assert_ne!(visit.legs[1].start_attachment.position.road, visit.attachments[1].position.road);
    assert_eq!(visit.geometry.len(), visit.elapsed.len());
    assert!(visit.elapsed.is_sorted_by(|a, b| a <= b));
    assert!((visit.elapsed.last().unwrap() - visit.totals.seconds).abs() < 1e-6);
}

#[test]
fn terrain_is_direction_independent_preserves_gaps_and_interpolates_structures() {
    let mut graph = fixture();
    graph.roads.retain(|r| (r.from == 0 && r.to == 1) || (r.from == 1 && r.to == 0));
    let height = |p: Point| Ok(if (4000..6000).contains(&p.lon) { None } else { Some(100.0 + p.lon as f64 / 100.0) });
    route_build::terrain::apply(&mut graph, height).unwrap();
    let a = &graph.roads[0];
    let b = &graph.roads[1];
    assert_eq!(
        a.shape.iter().map(|p| p.elevation).collect::<Vec<_>>(),
        b.shape.iter().rev().map(|p| p.elevation).collect::<Vec<_>>()
    );
    assert_eq!(a.ascent_m, b.descent_m);
    assert!(a.shape.iter().any(|p| p.elevation == NO_ELEVATION));
    for road in &mut graph.roads {
        road.structure = true;
    }
    route_build::terrain::apply(&mut graph, height).unwrap();
    assert!(graph.roads.iter().flat_map(|r| &r.shape).all(|p| p.elevation != NO_ELEVATION));
    assert_eq!(graph.roads[0].ascent_m, 100);
    assert_eq!(graph.roads[1].descent_m, 100);
}

#[test]
fn closed_packages_route_across_multiple_summary_pages() {
    let template = fixture().roads[0].clone();
    let points: Vec<_> = (0..400).map(|i| Point { lon: i * 1000, lat: 0, elevation: NO_ELEVATION }).collect();
    let mut roads = vec![];
    for i in 0..399u32 {
        for (a, b) in [(i, i + 1), (i + 1, i)] {
            roads.push(Road {
                from: a,
                to: b,
                way: i as i64,
                shape: vec![points[a as usize], points[b as usize]],
                length_m: 111,
                ..template.clone()
            });
        }
    }
    let graph = Graph { points, roads, forbidden: vec![], forbidden_foot: vec![], warnings: vec![] };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 64 * 1024);
    assert!(router.package().manifest().metrics["touring"].graph.len() > 1);
    let request = Request {
        points: vec![[0.0002, 0.0], [0.3988, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: true,
        turnarounds: vec![],
    };
    let result = router.routes(&request, &Control::default()).unwrap();
    assert_eq!(result.routes.len(), 1, "A single corridor does not need invented alternatives");
    let route = &result.routes[0];
    assert_eq!(
        route.cost,
        oracle(&graph, &Profile::presets()[0], &route.attachments[0], &route.attachments[1]).unwrap()
    );
    assert_eq!(route.legs[0].roads.len(), 399);
}

#[test]
fn alternatives_find_a_separate_corridor_without_an_out_and_back_probe() {
    let template = fixture().roads[0].clone();
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
    let graph = Graph { points, roads, forbidden: vec![], forbidden_foot: vec![], warnings: vec![] };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 1024 * 1024);
    let request = Request {
        points: vec![[0.0, 0.0], [0.06, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: true,
        turnarounds: vec![],
    };
    let routes = router.routes(&request, &Control::default()).unwrap().routes;
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[1].reason, "corridor");
    assert_eq!(routes[1].legs.len(), 1);
    assert_eq!(routes[1].attachments.len(), 2);
    assert_ne!(routes[0].id, routes[1].id);
    assert_eq!(routes[0].totals.distance_m, routes[1].totals.distance_m);
}

#[test]
fn partial_cost_localizes_climbing_and_telescopes_across_shape_points() {
    let mut road = fixture().roads[0].clone();
    let mut midpoint = road.shape[0];
    midpoint.lon = (road.shape[0].lon + road.shape[1].lon) / 2;
    midpoint.elevation = 100;
    road.shape[0].elevation = 0;
    road.shape[1].elevation = 100;
    road.shape.insert(1, midpoint);
    road.ascent_m = 100;
    let profile = Profile::presets().remove(0);
    let full = profile.cost(&road).unwrap();
    assert!(prefix_cost(&road, &profile, 0.5).unwrap() > full / 2);
    let costs: Vec<_> = [0.0, 0.1, 0.5, 0.9, 1.0].iter().map(|&f| prefix_cost(&road, &profile, f).unwrap()).collect();
    assert!(costs.is_sorted());
    assert_eq!(costs.windows(2).map(|c| c[1] - c[0]).sum::<u64>(), full);
}
