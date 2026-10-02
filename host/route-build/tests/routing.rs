use route_engine::{
    model::{Graph, Pace, Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION, PUSH},
    package::{digest, Package, Source},
    snap::{Candidate, Policy},
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
    let profiles = Profile::presets();
    package_with_profiles(graph, &profiles[..5])
}
fn package_with_profiles(graph: &Graph, profiles: &[Profile]) -> (Memory, Vec<u8>) {
    let graph = with_sources(graph.clone());
    let mut objects = HashMap::new();
    let manifest = route_build::prepare(&graph, "test".into(), [-1.0, -1.0, 1.0, 1.0], profiles, vec![], |bytes| {
        let key = digest(bytes);
        objects.insert(key.clone(), bytes.to_vec());
        Ok(key)
    })
    .unwrap();
    (Memory(Arc::new(objects)), serde_json::to_vec(&manifest).unwrap())
}

#[test]
fn profile_selection_keeps_closed_routes_and_removes_unused_objects() {
    use route_engine::directory::{Directory, Writer};
    let root = std::env::temp_dir().join(format!("route-select-test-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let input = root.join("input");
    std::fs::create_dir(&input).unwrap();
    let (source, manifest) = package_with_profiles(&fixture(), &Profile::presets());
    let mut writer = Writer::create(&input).unwrap();
    for bytes in source.0.values() {
        writer.write(bytes).unwrap();
    }
    writer.finish().unwrap();
    std::fs::write(input.join("manifest.json"), &manifest).unwrap();
    let output = root.join("selected");
    let command = |profiles: &str| {
        std::process::Command::new(env!("CARGO_BIN_EXE_route-select"))
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .arg("--profiles")
            .arg(profiles)
            .arg("--runtime")
            .output()
            .unwrap()
    };
    assert!(!command("unknown").status.success());
    assert!(!output.exists());
    assert!(command("touring,touring/less-climbing").status.success());
    let selected = Directory::open(&output).unwrap();
    selected.verify().unwrap();
    assert_eq!(selected.manifest().metrics.len(), 2);
    assert!(selected.manifest().osm.tables().all(|table| table.len == 0 && table.blocks.is_empty()));
    let original = Package::open(source.clone(), &manifest).unwrap();
    assert_eq!(selected.manifest().source_sha256, original.manifest().source_sha256);
    assert_eq!(selected.manifest().geometry.blocks, original.manifest().geometry.blocks);
    assert_eq!(selected.manifest().geometry.len, original.manifest().geometry.len);
    assert_eq!(std::fs::read(input.join("manifest.json")).unwrap(), manifest);
    let index = std::fs::read(output.join("pages.idx")).unwrap();
    assert_eq!(u64::from_le_bytes(index[8..16].try_into().unwrap()) as usize, selected.objects().unwrap().len());
    assert!(
        std::fs::metadata(output.join("pages.bin")).unwrap().len()
            < std::fs::metadata(input.join("pages.bin")).unwrap().len()
    );
    assert!(selected.metric("road").is_err());
    let mut before = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    let mut after = Router::new(selected, 768 * 1024 * 1024);
    for profile in ["touring", "touring/less-climbing"] {
        let request = Request {
            points: vec![[0.001, 0.0], [0.029, 0.02]],
            profile: profile.into(),
            pace: Pace::default(),
            alternatives: false,
            alternatives_only: false,
            turnarounds: vec![],
        };
        let expected = before.route(&request, &Control::default()).unwrap();
        let actual = after.route(&request, &Control::default()).unwrap();
        assert_eq!(actual.cost, expected.cost);
        assert_eq!(actual.geometry, expected.geometry);
    }
    assert!(!command("touring").status.success());
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
    let forbidden_foot = (0..roads.len())
        .flat_map(|a| {
            let roads = &roads;
            (0..roads.len()).filter_map(move |b| {
                (roads[a].to == roads[b].from && (a + b) % 11 == 0).then_some((a as u32, b as u32))
            })
        })
        .collect();
    Graph { points, roads, forbidden, forbidden_foot, warnings: vec![], ..Graph::default() }
}
fn with_sources(mut graph: Graph) -> Graph {
    // Reordering or clipping roads does not change their source way tags.
    if !graph.osm.ways.is_empty() {
        return graph;
    }
    graph.node_ids = (0..graph.points.len() as i64).collect();
    graph.node_access = vec![BIKE | FOOT; graph.points.len()];
    graph.osm = Default::default();
    for road in &mut graph.roads {
        road.way = road.from.min(road.to) as i64 * graph.points.len() as i64 + road.from.max(road.to) as i64;
        road.reversed = road.from > road.to;
        let highway = ["cycleway", "residential", "primary", "track", "path", "steps", ""][road.class as usize];
        let surface = ["", "asphalt", "compacted", "gravel", "dirt", "sand"][road.surface as usize];
        graph.osm.ways.insert(
            road.way,
            route_engine::osm::Way {
                id: road.way,
                nodes: vec![road.from as i64, road.to as i64],
                tags: [("highway".into(), highway.into()), ("surface".into(), surface.into())].into_iter().collect(),
            },
        );
    }
    graph
}
// Independent arrival-road Dijkstra. The target is a partial transition, not a compact state.
fn oracle(graph: &Graph, profile: &Profile, from: &Candidate, to: &Candidate) -> Option<u64> {
    let graph = with_sources(graph.clone());
    let costing = route_build::cost::Costing::new(&graph, profile).unwrap();
    let (a, b) = (from.position, to.position);
    let prefix = costing.roads[a.road as usize].as_ref().unwrap().prefix(a.fraction).unwrap();
    let suffix = costing.roads[b.road as usize].as_ref().unwrap().prefix(b.fraction).unwrap();
    let mut best = if a.road == b.road && a.fraction <= b.fraction { Some(suffix - prefix) } else { None };
    let mut dist = vec![u64::MAX; graph.roads.len()];
    dist[a.road as usize] = costing.roads[a.road as usize].as_ref().unwrap().total() - prefix;
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
            let Some(weight) = costing.roads[next].as_ref() else {
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
    let order = route_build::layout::spatial_order(&mut graph).unwrap();
    for (new, &old) in order.iter().enumerate() {
        assert_eq!(
            serde_json::to_value(&graph.roads[new]).unwrap(),
            serde_json::to_value(&original.roads[old as usize]).unwrap()
        );
    }
    for (actual, expected) in
        [(&graph.forbidden, &original.forbidden), (&graph.forbidden_foot, &original.forbidden_foot)]
    {
        let mut restored: Vec<_> = actual.iter().map(|&(a, b)| (order[a as usize], order[b as usize])).collect();
        restored.sort_unstable();
        assert_eq!(&restored, expected);
    }
    let profiles = Profile::presets();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    let coords: Vec<_> = graph.roads.iter().step_by(3).flat_map(|r| [coordinate(r, 0.2), coordinate(r, 0.8)]).collect();
    for profile in &profiles {
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
                                &profile.name,
                                Policy::default(),
                            )
                            .unwrap(),
                    );
                }
                let expected = snaps[0]
                    .retained
                    .iter()
                    .flat_map(|a| snaps[1].retained.iter().filter_map(|b| oracle(&graph, profile, a, b)))
                    .min();
                let actual = router.route(
                    &Request {
                        points: vec![from, to],
                        profile: profile.name.clone(),
                        pace: Pace::default(),
                        alternatives: false,
                        alternatives_only: false,
                        turnarounds: vec![],
                    },
                    &Control::default(),
                );
                match (expected, actual) {
                    (Some(cost), Ok(route)) => {
                        assert_eq!(cost, route.cost, "{from:?} -> {to:?}");
                        for pair in route.legs[0].roads.windows(2) {
                            assert_eq!(graph.roads[pair[0].road as usize].to, graph.roads[pair[1].road as usize].from);
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
fn bounding_box_repreparation_preserves_whole_roads_and_matches_independent_search() {
    let graph = with_sources(fixture());
    let profiles = Profile::presets();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let mut input = Package::open(source, &manifest).unwrap();
    assert!(route_build::extract::prepare(&mut input, "invalid".into(), [-2., -1., 0., 0.], true, |_| unreachable!())
        .is_err());
    let mut objects = HashMap::new();
    let bounds = [0.005, -0.001, 0.025, 0.021];
    let manifest = route_build::extract::prepare(&mut input, "box".into(), bounds, true, |bytes| {
        let key = digest(bytes);
        objects.insert(key.clone(), bytes.to_vec());
        Ok(key)
    })
    .unwrap();
    assert_eq!(manifest.metrics.len(), profiles.len());
    assert_eq!(manifest.bounds, bounds);
    assert_eq!(serde_json::to_value(&manifest.osm).unwrap(), serde_json::to_value(&input.manifest().osm).unwrap());
    let retained: Vec<_> =
        graph.roads.iter().enumerate().filter(|(_, r)| r.from % 4 != 0 || r.to % 4 != 0).map(|(id, _)| id).collect();
    assert!(retained.len() < graph.roads.len());
    assert_eq!(manifest.roads as usize, retained.len());
    let mut package = Package::open(Memory(Arc::new(objects)), &serde_json::to_vec(&manifest).unwrap()).unwrap();
    package.verify().unwrap();
    let roads: Vec<_> = (0..manifest.roads).map(|id| package.road(id).unwrap()).collect();
    let remap: HashMap<_, _> = roads
        .iter()
        .enumerate()
        .map(|(new, road)| {
            let old = retained
                .iter()
                .copied()
                .find(|&old| (graph.roads[old].from, graph.roads[old].to) == (road.from, road.to))
                .unwrap();
            (old as u32, new as u32)
        })
        .collect();
    let values = |package: &Package<Memory>, table: &route_engine::table::Table| -> Vec<u32> {
        table
            .blocks
            .iter()
            .flat_map(|key| {
                let deltas: Vec<i64> = package.read(key).unwrap();
                let mut value = 0;
                deltas.into_iter().map(move |delta| {
                    value += delta;
                    value as u32
                })
            })
            .collect()
    };
    let original = input.manifest().landmarks.as_ref().unwrap();
    let projected = manifest.landmarks.as_ref().unwrap();
    let before = values(&input, &original.mapping);
    let after = values(&package, &projected.mapping);
    assert_eq!(projected.scale, original.scale);
    for (profile, columns) in &original.profiles {
        for (i, column) in columns.iter().enumerate() {
            let original_values = values(&input, column);
            let projected_values = values(&package, &projected.profiles[profile][i]);
            for (&old, &new) in &remap {
                assert_eq!(
                    original_values[before[old as usize] as usize],
                    projected_values[after[new as usize] as usize]
                );
            }
        }
    }
    let remap_turns = |turns: &[(u32, u32)]| {
        let mut mapped: Vec<_> = turns.iter().filter_map(|(a, b)| Some((*remap.get(a)?, *remap.get(b)?))).collect();
        mapped.sort_unstable();
        mapped
    };
    let selected = Graph {
        roads,
        forbidden: remap_turns(&graph.forbidden),
        forbidden_foot: remap_turns(&graph.forbidden_foot),
        ..graph.clone()
    };
    assert!(selected.roads.iter().flat_map(|r| &r.shape).any(|p| p.lon == 0));
    let mut router = Router::new(package, 768 * 1024 * 1024);
    let coordinates = [[0.0075, 0.0], [0.015, 0.01], [0.0225, 0.02]];
    for profile in &profiles {
        for &from in &coordinates {
            for &to in &coordinates {
                let snaps: Vec<_> = [from, to]
                    .iter()
                    .map(|&[lon, lat]| {
                        router
                            .snap(
                                Point {
                                    lon: (lon * 1e6_f64).round() as i32,
                                    lat: (lat * 1e6_f64).round() as i32,
                                    elevation: NO_ELEVATION,
                                },
                                &profile.name,
                                Policy::default(),
                            )
                            .unwrap()
                    })
                    .collect();
                let expected = snaps[0]
                    .retained
                    .iter()
                    .flat_map(|a| snaps[1].retained.iter().filter_map(|b| oracle(&selected, profile, a, b)))
                    .min();
                let actual = router.route(
                    &Request {
                        points: vec![from, to],
                        profile: profile.name.clone(),
                        pace: Pace::default(),
                        alternatives: false,
                        alternatives_only: false,
                        turnarounds: vec![],
                    },
                    &Control::default(),
                );
                match (expected, actual) {
                    (Some(cost), Ok(route)) => assert_eq!(cost, route.cost, "{} {from:?} -> {to:?}", profile.name),
                    (None, Err(Error::NoPath)) => {}
                    (expected, actual) => panic!("expected {expected:?}, got {actual:?}"),
                }
            }
        }
    }
    let mut foot_only = graph.clone();
    for &id in &retained {
        foot_only.roads[id].access = FOOT;
    }
    let (source, bytes) = package_with_profiles(&foot_only, &profiles);
    let mut input = Package::open(source, &bytes).unwrap();
    let mut objects = HashMap::new();
    let manifest =
        route_build::extract::prepare(&mut input, "foot".into(), [0.014, -0.0001, 0.016, 0.0001], false, |bytes| {
            let key = digest(bytes);
            objects.insert(key.clone(), bytes.to_vec());
            Ok(key)
        })
        .unwrap();
    assert_eq!(manifest.metrics.len(), profiles.len());
    let package = Package::open(Memory(Arc::new(objects)), &serde_json::to_vec(&manifest).unwrap()).unwrap();
    for id in 0..manifest.roads {
        assert!(!package.allowed("road", id).unwrap());
        assert!(package.allowed("hiking", id).unwrap());
    }
}

#[test]
fn lazy_endpoint_costs_preserve_every_profile_and_partial_offset() {
    let mut graph = fixture();
    for (id, road) in graph.roads.iter_mut().enumerate() {
        let a = road.shape[0];
        let b = road.shape[1];
        road.shape = (0..=6)
            .map(|i| Point {
                lon: a.lon + (b.lon - a.lon) * i / 6 + if i == 3 { 10 } else { 0 },
                lat: a.lat + (b.lat - a.lat) * i / 6 + if i == 3 { 10 } else { 0 },
                elevation: if i == 4 && id % 3 == 0 { NO_ELEVATION } else { [0., 3., 2., 7., 9., 9., 0.][i as usize] },
            })
            .collect();
    }
    let graph = with_sources(graph);
    let profiles = Profile::presets();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let mut package = Package::open(source, &manifest).unwrap();
    for profile in &profiles {
        let costing = route_build::cost::Costing::new(&graph, profile).unwrap();
        for (id, expected) in costing.roads.iter().enumerate() {
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
fn query_does_not_read_source_tag_pages_but_installation_verifies_them() {
    let graph = fixture();
    let (source, manifest) = package(&graph);
    let package = Package::open(source.clone(), &manifest).unwrap();
    package.verify().unwrap();
    let mut objects = (*source.0).clone();
    for key in package.manifest().osm.tables().flat_map(|t| &t.blocks) {
        objects.remove(key);
    }
    let incomplete = Package::open(Memory(Arc::new(objects)), &manifest).unwrap();
    assert!(matches!(incomplete.verify(), Err(Error::MissingRegion(_))));
    let mut router = Router::new(incomplete, 768 * 1024 * 1024);
    let request = Request {
        points: vec![coordinate(&graph.roads[0], 0.2), coordinate(&graph.roads[0], 0.8)],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
    };
    assert!(router.route(&request, &Control::default()).is_ok());
}

#[test]
fn via_direction_pace_and_failure_states_are_explicit() {
    let graph = fixture();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source.clone(), &manifest).unwrap(), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![
            coordinate(&graph.roads[0], 0.2),
            coordinate(&graph.roads[7], 0.5),
            coordinate(&graph.roads[15], 0.8),
        ],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
    };
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.attachments.len(), 3);
    assert_eq!(route.surfaces.len(), route.geometry.len() - 1);
    assert_eq!(route.pushing.len(), route.surfaces.len());
    for leg in &route.legs {
        assert_eq!(route.surfaces[leg.from_index], graph.roads[leg.roads[0].road as usize].surface);
        assert_eq!(route.surfaces[leg.to_index - 1], graph.roads[leg.roads.last().unwrap().road as usize].surface);
    }
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
    let key = router.package().key(&router.package().manifest().geometry, 0).unwrap();
    broken.get_mut(&key).unwrap()[0] ^= 1;
    let mut package = Package::open(Memory(Arc::new(broken)), &manifest).unwrap();
    assert!(matches!(package.road(0), Err(Error::InvalidData(_))));
}

#[test]
fn a_shape_keeps_direction_and_only_an_explicit_visit_can_reverse() {
    let mut graph = fixture();
    graph.roads.retain(|r| (r.from == 0 && r.to == 1) || (r.from == 1 && r.to == 0));
    graph.forbidden.clear();
    graph.forbidden_foot.clear();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![
            coordinate(&graph.roads[0], 0.2),
            coordinate(&graph.roads[0], 0.8),
            coordinate(&graph.roads[0], 0.2),
        ],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
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
fn terrain_reuses_nearby_tiles_without_changing_road_identity_or_heights() {
    let mut graph = fixture();
    graph.roads.truncate(3);
    let identities: Vec<_> = graph.roads.iter().map(|r| (r.way, r.from, r.to)).collect();
    for (road, lon) in graph.roads.iter_mut().zip([0, 1_000_000, 1000]) {
        road.shape = [lon, lon + 10].map(|lon| Point { lat: 0, lon, elevation: NO_ELEVATION }).to_vec();
    }
    let mut loaded = None;
    let mut loads = 0;
    route_build::terrain::apply(&mut graph, |point| {
        let tile = point.lon.div_euclid(100_000);
        if loaded != Some(tile) {
            loaded = Some(tile);
            loads += 1;
        }
        Ok(Some(100.0 + point.lon.rem_euclid(1000) as f64 / 10.0))
    })
    .unwrap();
    assert_eq!(loads, 2);
    assert_eq!(graph.roads.iter().map(|r| (r.way, r.from, r.to)).collect::<Vec<_>>(), identities);
    for road in &graph.roads {
        assert_eq!(road.shape.iter().map(|p| p.elevation).collect::<Vec<_>>(), [100.0, 101.0]);
        assert_eq!((road.ascent_m, road.descent_m), (1, 0));
    }
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
fn closed_packages_route_across_multiple_checked_blocks() {
    let template = fixture().roads[0].clone();
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
    let graph =
        Graph { points, roads, forbidden: vec![], forbidden_foot: vec![], warnings: vec![], ..Graph::default() };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    assert!(router.package().manifest().graph.first.blocks.len() > 1);
    let request = Request {
        points: vec![[0.00002, 0.0], [0.21988, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: true,
        alternatives_only: false,
        turnarounds: vec![],
    };
    let result = router.routes(&request, &Control::default()).unwrap();
    assert_eq!(result.routes.len(), 1, "A single corridor does not need invented alternatives");
    let route = &result.routes[0];
    assert_eq!(
        route.cost,
        oracle(&graph, &Profile::presets()[0], &route.attachments[0], &route.attachments[1]).unwrap()
    );
    assert_eq!(route.legs[0].roads.len(), 2199);
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
    let graph =
        Graph { points, roads, forbidden: vec![], forbidden_foot: vec![], warnings: vec![], ..Graph::default() };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    let request = Request {
        points: vec![[0.0, 0.0], [0.06, 0.0]],
        profile: "touring".into(),
        pace: Pace::default(),
        alternatives: true,
        alternatives_only: false,
        turnarounds: vec![],
    };
    let routes = router.routes(&request, &Control::default()).unwrap().routes;
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[1].reason, "corridor");
    assert_eq!(routes[1].legs.len(), 1);
    assert_eq!(routes[1].attachments.len(), 2);
    assert_ne!(routes[0].id, routes[1].id);
    assert_eq!(routes[0].totals.distance_m, routes[1].totals.distance_m);
    let only = Request { alternatives: false, alternatives_only: true, ..request };
    let alternatives = router.routes(&only, &Control::default()).unwrap().routes;
    assert_eq!(alternatives.iter().map(|r| &r.id).collect::<Vec<_>>(), [&routes[1].id]);
}

#[test]
fn partial_cost_localizes_climbing_and_telescopes_across_shape_points() {
    let mut road = fixture().roads[0].clone();
    let mut midpoint = road.shape[0];
    midpoint.lon = (road.shape[0].lon + road.shape[1].lon) / 2;
    midpoint.elevation = 100.0;
    road.shape[0].elevation = 0.0;
    road.shape[1].elevation = 100.0;
    road.shape.insert(1, midpoint);
    road.ascent_m = 100;
    let profile = Profile::presets().remove(0);
    let mut graph = fixture();
    graph.roads = vec![road];
    let graph = with_sources(graph);
    let costing = route_build::cost::Costing::new(&graph, &profile).unwrap();
    let curve = costing.roads[0].as_ref().unwrap();
    let full = curve.total();
    assert!(curve.prefix(0.5).unwrap() > full / 2);
    let costs: Vec<_> = [0.0, 0.1, 0.5, 0.9, 1.0].iter().map(|&f| curve.prefix(f).unwrap()).collect();
    assert!(costs.is_sorted());
    assert_eq!(costs.windows(2).map(|c| c[1] - c[0]).sum::<u64>(), full);
}

#[test]
fn geometry_cache_retains_a_snap_working_set_across_many_small_pages() {
    use route_engine::package::Manifest;
    use std::cell::Cell;
    struct Counted<'a>(Memory, &'a Cell<usize>);
    impl Source for Counted<'_> {
        fn read(&self, key: &str) -> route_engine::Result<Vec<u8>> {
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
        let road = Road { way: page, ..graph.roads[0].clone() };
        let bytes = route_engine::geometry::encode(&vec![road; 128]).unwrap();
        let key = digest(&bytes);
        geometry.push(key.clone());
        objects.insert(key, bytes);
    }
    let mut write = |bytes: &[u8]| {
        let key = digest(bytes);
        objects.insert(key.clone(), bytes.to_vec());
        Ok(key)
    };
    manifest.geometry = route_engine::table::Table::write(&geometry, &mut write).unwrap();
    manifest.graph = route_engine::base::write_topology(manifest.roads, &[], &mut write).unwrap();
    for metric in manifest.metrics.values_mut() {
        metric.weights = route_engine::base::write_weights(&vec![1; manifest.roads as usize], &[], &mut write).unwrap();
        metric.allowed = route_engine::table::Table::write(&vec![u64::MAX; 40], &mut write).unwrap();
        metric.costs = route_engine::table::Table::write(&vec![1u32; manifest.roads as usize], &mut write).unwrap();
    }
    let reads = Cell::new(0);
    let mut package =
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
    let mut graph = fixture();
    graph.points = [(0, 0), (0, 10_000), (100, 4_000), (100, 6_000)]
        .map(|(lat, lon)| Point { lat, lon, elevation: NO_ELEVATION })
        .to_vec();
    let template = graph.roads[0].clone();
    graph.roads = [(0, 1, 0), (1, 0, 0), (2, 3, 0), (3, 2, 0), (0, 2, 1), (2, 0, 1)]
        .map(|(from, to, difficulty)| {
            let shape = vec![graph.points[from as usize], graph.points[to as usize]];
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
        .to_vec();
    graph.forbidden.clear();
    graph.forbidden_foot.clear();
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![[0.0, 0.0], [0.005, 0.0002], [0.01, 0.0]],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![1],
    };
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.attachments[1].projected.lat, 0);
    assert!(route.attachments[1].snap_distance_m < 25.0);
    assert_eq!(route.attachments[0].snap_distance_m, 0.0);
    assert_eq!(route.attachments[2].snap_distance_m, 0.0);
    request.profile = "mtb".into();
    let mtb = router.route(&request, &Control::default()).unwrap();
    assert_eq!(mtb.attachments[1].projected.lat, 100);
    request.profile = "road".into();
    // An explicit point on the disconnected driveway must not jump to a different road.
    request.points[1][1] = 0.0001;
    assert!(matches!(router.route(&request, &Control::default()), Err(Error::NoPath)));
}

#[test]
fn route_goals_preserve_the_bikes_surface_suitability() {
    let mut graph = fixture();
    graph.points =
        [(0, 0), (0, 10_000), (15_000, 5_000)].map(|(lat, lon)| Point { lat, lon, elevation: NO_ELEVATION }).to_vec();
    let template = graph.roads[0].clone();
    graph.roads = [(0, 1), (1, 0), (0, 2), (2, 0), (2, 1), (1, 2)]
        .map(|(from, to)| {
            let shape = vec![graph.points[from as usize], graph.points[to as usize]];
            Road {
                from,
                to,
                surface: if from + to == 1 { Surface::Gravel } else { Surface::Paved },
                length_m: shape[0].distance(shape[1]).round() as u32,
                shape,
                ..template.clone()
            }
        })
        .to_vec();
    graph.forbidden.clear();
    graph.forbidden_foot.clear();
    let profiles: Vec<_> = Profile::presets()
        .into_iter()
        .filter(|p| ["road", "road/shorter", "road/smoother", "gravel/shorter"].contains(&p.name.as_str()))
        .collect();
    let (source, manifest) = package_with_profiles(&graph, &profiles);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    let mut request = Request {
        points: vec![[0.0, 0.0], [0.01, 0.0]],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
    };
    for profile in ["road", "road/shorter", "road/smoother"] {
        request.profile = profile.into();
        let route = router.route(&request, &Control::default()).unwrap();
        assert_eq!(route.totals.surface_m[Surface::Gravel as usize], 0, "{profile}");
        assert!(route.totals.surface_m[Surface::Paved as usize] > 3000, "{profile}");
    }
    request.profile = "gravel/shorter".into();
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.totals.surface_m[Surface::Paved as usize], 0);
    assert!(route.totals.distance_m < 1200);
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
                uncertain_access: false,
                structure: false,
                access: if i == 1 {
                    route_engine::osm::access(
                        |key| match key {
                            "highway" => Some("footway"),
                            "bicycle" => Some("no"),
                            _ => None,
                        },
                        FOOT | PUSH,
                        "forward",
                    )
                } else {
                    BIKE | FOOT | PUSH
                },
                shape: vec![points[from as usize], points[to as usize]],
            });
        }
    }
    let mut graph = Graph { points, roads, ..Graph::default() };
    let request = Request {
        points: vec![[0.0001, 0.0], [0.0029, 0.0]],
        profile: "road".into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds: vec![],
    };
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    let route = router.route(&request, &Control::default()).unwrap();
    assert_eq!(route.totals.pushing_m, 111);
    assert_eq!(route.pushing, vec![false, true, false]);
    for road in &mut graph.roads {
        road.access &= !PUSH;
    }
    let (source, manifest) = package(&graph);
    let mut router = Router::new(Package::open(source, &manifest).unwrap(), 768 * 1024 * 1024);
    assert!(router.route(&request, &Control::default()).is_err());
}

#[test]
fn shared_pages_preserve_routes_costs_and_guidance_for_every_profile() {
    use route_engine::{
        data::RoutingData,
        search::{Query, Seed, Workspace},
    };
    let (source, bytes) = package_with_profiles(&fixture(), &Profile::presets());
    let mut original = Package::open(source.clone(), &bytes).unwrap();
    let n = original.manifest().roads;
    let bounds = original.manifest().bounds;
    let make = |ids: &[u32]| {
        let selection = route_build::blocks::prepare(&original, bounds, ids).unwrap();
        let objects = selection.objects.iter().map(|key| (key.clone(), source.read(key).unwrap())).collect();
        let selected = route_engine::blocks::Package::open(
            Memory(Arc::new(objects)),
            &serde_json::to_vec(&selection.manifest).unwrap(),
        )
        .unwrap();
        (selection, selected)
    };
    let (full, full_package) = make(&(0..n).collect::<Vec<_>>());
    let selected: Vec<_> = (0..n).filter(|&i| i < n / 3 || i >= 2 * n / 3).collect();
    let (_, mut partial) = make(&selected);
    let mut full_router = Router::new(full_package, 768 * 1024 * 1024);
    let mut original_router = Router::new(original.fork(), 768 * 1024 * 1024);
    let names: Vec<_> = original.manifest().metrics.keys().cloned().collect();
    let mut saw_forbidden = false;
    for name in names {
        for points in [vec![[0.001, 0.0], [0.029, 0.02]], vec![[0.029, 0.02], [0.001, 0.0]]] {
            let request = Request {
                points,
                profile: name.clone(),
                pace: Pace::default(),
                alternatives: false,
                alternatives_only: false,
                turnarounds: vec![],
            };
            let before = original_router.route(&request, &Control::default()).unwrap();
            let after = full_router.route(&request, &Control::default()).unwrap();
            assert_eq!(after.cost, before.cost);
            assert_eq!(after.geometry, before.geometry);
            assert_eq!(after.elapsed, before.elapsed);
        }
        let (graph, costs) = original.base(&name).unwrap();
        let (joined, joined_costs) = partial.base(&name).unwrap();
        let mut masked_roads: Vec<u64> = (0..n).map(|i| costs.roads.get(i as usize)).collect();
        for id in 0..n {
            if partial.local_id(id).is_none() {
                masked_roads[id as usize] = u64::MAX;
            }
        }
        let masked = route_engine::base::Costs {
            roads: route_engine::base::Numbers::U64(masked_roads),
            turns: costs.turns.clone(),
        };
        let mut expected_work = Workspace::default();
        let mut actual_work = Workspace::default();
        for from in 0..joined.nodes() as u32 {
            let global = partial.source_id(from).unwrap();
            assert_eq!(partial.local_id(global), Some(from));
            assert_eq!(
                serde_json::to_value(partial.road(from).unwrap()).unwrap(),
                serde_json::to_value(original.road(global).unwrap()).unwrap()
            );
            let expected: Vec<_> = (graph.first[global as usize]..graph.first[global as usize + 1])
                .filter_map(|arc| {
                    partial.local_id(graph.head[arc as usize]).map(|to| (to, costs.arc(&graph, arc as usize)))
                })
                .collect();
            let actual: Vec<_> = (joined.first[from as usize]..joined.first[from as usize + 1])
                .map(|arc| (joined.head[arc as usize], joined_costs.arc(&joined, arc as usize)))
                .collect();
            saw_forbidden |= actual.iter().any(|(_, c)| *c == u64::MAX);
            assert_eq!(actual, expected);
            for to in 0..joined.nodes() as u32 {
                let starts = [Seed { node: from, cost: 7, road: from, choice: 0 }];
                let ends = [Seed { node: to, cost: 11, road: to, choice: 0 }];
                let source_starts = [Seed { node: global, ..starts[0] }];
                let source_ends = [Seed { node: partial.source_id(to).unwrap(), ..ends[0] }];
                let query = Query {
                    starts: &starts,
                    ends: &ends,
                    ceiling: u64::MAX,
                    max_labels: usize::MAX,
                    max_roads: usize::MAX,
                    heap_bytes: 1_000_000,
                    cancelled: &|| false,
                };
                let expected = expected_work
                    .run(&graph, &masked, Query { starts: &source_starts, ends: &source_ends, ..query })
                    .unwrap();
                let guidance = partial.landmarks(&name, &starts, &ends).unwrap().unwrap();
                let source_guidance =
                    RoutingData::landmarks(&original, &name, &source_starts, &source_ends).unwrap().unwrap();
                for local in 0..joined.nodes() as u32 {
                    assert_eq!(guidance.get(local), source_guidance.get(partial.source_id(local).unwrap()));
                }
                let mut potential = |road| Ok(guidance.get(road));
                let actual =
                    actual_work.run_with_potential(&joined, &joined_costs, query, Some(&mut potential)).unwrap();
                assert_eq!(actual.as_ref().map(|r| r.cost), expected.as_ref().map(|r| r.cost));
            }
        }
    }
    assert!(saw_forbidden);
    let mut incomplete = full.manifest.clone();
    incomplete.data.graph.head.retain(&Default::default()).unwrap();
    let mut missing =
        route_engine::blocks::Package::open(source.clone(), &serde_json::to_vec(&incomplete).unwrap()).unwrap();
    assert!(matches!(missing.base("touring"), Err(Error::MissingRegion(_))));
    incomplete = full.manifest.clone();
    incomplete.roads.push(incomplete.roads[0]);
    assert!(route_engine::blocks::Package::open(source, &serde_json::to_vec(&incomplete).unwrap()).is_err());
    let (_, cached) = partial.base("touring").unwrap();
    partial.set_memory_budget(partial.routing_bytes("touring").unwrap());
    let (_, again) = partial.base("touring").unwrap();
    assert!(Arc::ptr_eq(&cached, &again));
    partial.set_memory_budget(1);
    assert!(matches!(partial.base("touring"), Err(Error::Limit)));
}

#[test]
fn sparse_tables_keep_source_page_numbers_and_last_page_length() {
    use route_engine::table::{Cache, Table, ENTRIES};
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
