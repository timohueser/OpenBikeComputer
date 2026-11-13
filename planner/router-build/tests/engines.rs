//! The data and the search of format 10 (`Region`) against those of format 9 (`Selection`) of the
//! same bake.

use planner_router::cells::Directory;
use planner_router::compare::{self, Outcome};
use planner_router::data::{RoutingData, Selection};
use planner_router::host::{self, Part};
use planner_router::install;
use planner_router::model::{Point, Profile, Road, Surface, BIKE, FOOT, PUSH};
use planner_router::package::{digest, Endpoint, Package, Source};
use planner_router::region::Region;
use planner_router::snap::{self, Candidates, Policy, REACH_M};
use planner_router::{Control, Error, Request, Router};
use planner_router_build::cells::{write, Info, Input};
use planner_router_build::source::{Builder, Tags, Ways};
use planner_router_build::{base, Graph, Turn};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::Arc;

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

const SIDE: u32 = 20;

/// A town of 20 × 20 junctions about 350 m apart, each moved a little: two-way and one-way roads
/// with gaps and diagonals, three surfaces, smooth heights, forbidden turns for each mode, some
/// roads for walkers only, two closures, and one paved two-way road of about 10.5 km between two
/// far junctions.
fn town() -> Graph {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let height = |lat: i32, lon: i32| 500.0 + 120.0 * ((lat as f32) * 2e-5).sin() * ((lon as f32) * 3e-5).cos();
    let points: Vec<Point> = (0..SIDE * SIDE)
        .map(|i| {
            let lat = 46_500_000 + (i / SIDE) as i32 * 3_150 + (xorshift(&mut state) % 1_200) as i32 - 600;
            let lon = 7_500_000 + (i % SIDE) as i32 * 4_600 + (xorshift(&mut state) % 1_800) as i32 - 900;
            Point { lat, lon, elevation: height(lat, lon) }
        })
        .collect();
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for i in 0..SIDE * SIDE {
        let (row, column) = (i / SIDE, i % SIDE);
        if column + 1 < SIDE && !xorshift(&mut state).is_multiple_of(13) {
            pairs.push((i, i + 1));
        }
        if row + 1 < SIDE && !xorshift(&mut state).is_multiple_of(13) {
            pairs.push((i, i + SIDE));
        }
        if row + 1 < SIDE && column + 1 < SIDE && xorshift(&mut state).is_multiple_of(7) {
            pairs.push((i, i + SIDE + 1));
        }
    }
    let long = pairs.len();
    pairs.push((0, SIDE - 1));
    let (mut roads, mut ways) = (Vec::new(), Builder::<Ways>::default());
    for (k, &(a, b)) in pairs.iter().enumerate() {
        let way = 1000 + k as i64;
        let random = xorshift(&mut state);
        let (highway, surface, tag) = match random % 3 {
            _ if k == long => ("residential", Surface::Paved, "asphalt"),
            0 => ("residential", Surface::Paved, "asphalt"),
            1 => ("track", Surface::Gravel, "gravel"),
            _ => ("path", Surface::Dirt, "dirt"),
        };
        let mut tags = vec![("highway", highway), ("surface", tag)];
        match k {
            7 => tags.push(("access", "permit")),
            23 => tags.push(("bicycle:conditional", "no @ (Nov-Mar)")),
            _ => {}
        }
        ways.push(way, &[i64::from(a), i64::from(b)], &Tags::from_pairs(tags));
        let one_way = k != long && random.is_multiple_of(5);
        let access = if k != long && random.is_multiple_of(11) { FOOT } else { BIKE | FOOT | PUSH };
        let (pa, pb) = (points[a as usize], points[b as usize]);
        let middle = if k == long {
            // About 4 km south of the first row, so the road is longer than 10 km.
            Point { lat: pa.lat - 36_000, lon: (pa.lon + pb.lon) / 2, elevation: 0.0 }
        } else {
            let lat = (pa.lat + pb.lat) / 2 + (xorshift(&mut state) % 400) as i32 - 200;
            let lon = (pa.lon + pb.lon) / 2 + (xorshift(&mut state) % 400) as i32 - 200;
            Point { lat, lon, elevation: 0.0 }
        };
        let middle = Point { elevation: height(middle.lat, middle.lon), ..middle };
        let directions: &[(u32, u32)] = if one_way { &[(a, b)] } else { &[(a, b), (b, a)] };
        for &(from, to) in directions {
            let shape = if from == a { vec![pa, middle, pb] } else { vec![pb, middle, pa] };
            let length = shape.windows(2).map(|p| p[0].distance(p[1])).sum::<f64>().round() as u32;
            roads.push(Road {
                from,
                to,
                way,
                reversed: from > to,
                length_m: length,
                surface,
                class: [1, 3, 4][(random % 3) as usize],
                access,
                difficulty: 255,
                hiking_difficulty: None,
                shape,
                ..Road::default()
            });
        }
    }
    assert!(roads.iter().any(|road| road.length_m > 10_400), "the long road");
    let mut forbidden = Vec::new();
    let mut forbidden_foot = Vec::new();
    for (a, before) in roads.iter().enumerate() {
        for (b, after) in roads.iter().enumerate() {
            if before.to == after.from && a != b {
                match xorshift(&mut state) % 17 {
                    0 => forbidden.push(Turn { from: a as u32, to: b as u32 }),
                    1 => forbidden_foot.push(Turn { from: a as u32, to: b as u32 }),
                    _ => {}
                }
            }
        }
    }
    let junctions = points.len();
    let mut graph = Graph {
        points: points.into_iter().collect(),
        node_ids: (0..junctions as i64).collect(),
        node_access: std::iter::repeat_n(BIKE | FOOT | PUSH, junctions).collect(),
        signals: std::iter::repeat_n(0, junctions).collect(),
        node_doubts: std::iter::repeat_n(0, junctions).collect(),
        node_closures: std::iter::repeat_n(u32::MAX, junctions).collect(),
        roads: roads.into_iter().collect(),
        forbidden: forbidden.into_iter().collect(),
        forbidden_foot: forbidden_foot.into_iter().collect(),
        ..Graph::default()
    };
    planner_router_build::layout::order(&mut graph, &[8, 32, 128], None).unwrap();
    graph.osm.ways = ways.finish().0;
    graph.link_ways();
    graph
}

#[derive(Clone)]
struct Objects(Arc<HashMap<String, Vec<u8>>>);

impl Source for Objects {
    fn read(&self, key: &str) -> planner_router::Result<Vec<u8>> {
        self.0.get(key).cloned().ok_or_else(|| Error::MissingRegion(key.into()))
    }
}

/// A folder of the test that `Drop` removes.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The two formats of one bake of `town()`: format 9 in memory, and format 10 as objects that
/// `install` reads.
struct Bake {
    graph: Graph,
    old: Selection<Objects>,
    scratch: Scratch,
    directory: Vec<u8>,
    info: Vec<u8>,
}

fn bake(name: &str) -> Bake {
    let graph = town();
    let profiles = Profile::presets();
    assert_eq!(profiles.len(), 10);
    let mut objects = HashMap::new();
    let bounds = [7.4, 46.4, 7.7, 46.6];
    let manifest =
        planner_router_build::prepare(&graph, "town".into(), bounds, &profiles, vec![], "test".into(), |b| {
            let key = digest(b);
            objects.insert(key.clone(), b.to_vec());
            Ok(key)
        })
        .unwrap();
    let manifest = serde_json::to_vec(&manifest).unwrap();
    let old = Selection::whole(Package::open(Objects(Arc::new(objects)), &manifest).unwrap());

    let topology = base::Topology::new(&graph.roads, graph.points.len()).unwrap();
    let costs = planner_router_build::costs(&graph, &topology, &profiles).unwrap();
    let (first, head) = (topology.first.values(), topology.head.values());
    let tables = planner_router_build::tables::build(&graph.cells, first, head, &costs.columns(), 16).unwrap();
    let scratch = Scratch(std::env::temp_dir().join(format!("obc-engines-{name}-{}", std::process::id())));
    let _ = fs::remove_dir_all(&scratch.0);
    let info = Info { region: "town".into(), bounds, source_sha256: vec![], attribution: "test".into() };
    let input = Input {
        graph: &graph,
        topology: &topology,
        costs: &costs,
        tables: &tables,
        sizes: &[8, 32, 128],
        download_level: 2,
        info,
        level: 3,
    };
    write(&input, &scratch.0.join("objects")).unwrap();
    let read = |file: &str| fs::read(scratch.0.join("objects").join(file)).unwrap();
    let (directory, info) = (read("directory.bin"), read("package.json"));
    Bake { graph, old, scratch, directory, info }
}

impl Bake {
    /// The folder of an install of the download cells `cells`, or all.
    fn install(&self, cells: Option<&[u32]>) -> PathBuf {
        let target = self.scratch.0.join(format!("install-{}", cells.map_or(0, |c| c.len())));
        let objects = self.scratch.0.join("objects");
        let fetch = |name: &str| File::open(objects.join(name));
        install::install(&target, (&self.directory, &self.info), (cells, None), 2, fetch).unwrap();
        target
    }

    /// The data of an install of the download cells `cells`, or all, through memory maps.
    fn region(&self, cells: Option<&[u32]>) -> Region<host::Maps> {
        planner_router::open::cells(&self.install(cells), false).unwrap()
    }

    /// The data of the install of all cells, held in memory.
    fn memory(&self) -> Region<host::Memory> {
        let target = self.install(None);
        let (directory, plan) = install::installed(&target).unwrap();
        let mut memory = host::Memory::default();
        for item in &plan.files {
            let bytes = fs::read(target.join(install::CELLS).join(&item.name)).unwrap();
            memory.insert(Part { record: item.record, part: item.part }, bytes);
        }
        let marker = fs::read(target.join(install::MARKER)).unwrap();
        Region::open(memory, directory, self.info.clone(), &marker).unwrap()
    }
}

fn departures(endpoint: &Endpoint) -> Vec<(u32, u64)> {
    let mut list: Vec<_> = endpoint.departures.iter().map(|d| (d.state, d.penalty)).collect();
    list.sort_unstable();
    list
}

/// The road, the fraction bits, the projected point and the distance bits of each candidate.
type Snapped = (bool, Vec<(u32, u64, i32, i32, u64)>);

fn candidates(found: &Candidates) -> Snapped {
    let list = found.retained.iter().map(|c| {
        let p = c.position;
        (p.road, p.fraction.to_bits(), c.projected.lat, c.projected.lon, c.snap_distance_m.to_bits())
    });
    (found.truncated, list.collect())
}

/// The middle and the last point of each 5th road, and 50 points off the roads.
fn snap_points(graph: &Graph) -> Vec<Point> {
    let mut points: Vec<Point> = (0..graph.roads.len())
        .step_by(5)
        .flat_map(|id| {
            let shape = graph.roads.shape(id);
            [shape[shape.len() / 2], shape[shape.len() - 1]]
        })
        .collect();
    let long = (0..graph.roads.len()).find(|&id| graph.roads.attributes(id).length_m > 10_000).unwrap();
    points.extend([graph.roads.shape(long)[1], graph.roads.shape(long)[0]]);
    let mut state = 77u64;
    points.extend((0..50).map(|_| Point {
        lat: 46_498_000 + (xorshift(&mut state) % 64_000) as i32,
        lon: 7_498_000 + (xorshift(&mut state) % 92_000) as i32,
        elevation: 0.0,
    }));
    points
}

#[test]
fn the_data_of_format_10_is_the_data_of_format_9() {
    let bake = bake("same");
    let (old, new) = (&bake.old, bake.region(None));
    assert_eq!((old.region(), old.bounds()), (new.region(), new.bounds()));
    assert_eq!(old.profiles(), new.profiles());
    let roads = bake.graph.roads.len() as u32;
    let (mut turns, mut closures) = (0, 0);
    for profile in old.profiles() {
        for road in 0..roads {
            let (a, b) = (old.endpoint(profile, road).unwrap(), new.endpoint(profile, road).unwrap());
            assert_eq!(a.cost, b.cost, "the cost of road {road} for {profile}");
            assert_eq!(departures(&a), departures(&b), "the turns into road {road} for {profile}");
            turns += a.departures.len();
        }
    }
    assert!(turns > 0);
    let fields = |road: &Road| {
        let shape: Vec<_> = road.shape.iter().map(|p| (p.lat, p.lon, p.elevation.to_bits())).collect();
        (shape, road.surface, road.access, road.difficulty, road.hiking_difficulty, road.way, road.length_m)
    };
    for road in 0..roads {
        assert_eq!(old.with_road(road, fields).unwrap(), new.with_road(road, fields).unwrap(), "road {road}");
        for mode in [BIKE, FOOT, PUSH] {
            let found = old.closing(road, mode).unwrap();
            closures += usize::from(found.is_some());
            assert_eq!(found, new.closing(road, mode).unwrap(), "the closures of road {road}");
        }
    }
    assert!(closures > 0, "the town has closures");
    // The parts in memory give the same roads, and each span that the data announces fits its part.
    let memory = bake.memory();
    for road in 0..roads {
        assert_eq!(memory.with_road(road, fields).unwrap(), new.with_road(road, fields).unwrap());
    }
    memory.announce(&(0..roads).collect::<Vec<_>>());
    let profiles = old.profiles();
    let mut kinds = [0; 2];
    for (k, &point) in snap_points(&bake.graph).iter().enumerate() {
        let profile = profiles[k % profiles.len()];
        for radius_m in [250.0, REACH_M, 4_000.0] {
            let policy = Policy { radius_m, ..Policy::default() };
            let a = candidates(&old.snap(point, profile, policy).unwrap());
            assert_eq!(a, candidates(&new.snap(point, profile, policy).unwrap()), "{point:?} {radius_m} m {profile}");
            kinds[usize::from(a.0)] += 1;
        }
    }
    assert!(kinds[0] > 0 && kinds[1] > 0, "snaps with and without truncation: {kinds:?}");
    assert!(matches!(new.profile("unknown"), Err(Error::InvalidRequest(_))));
}

#[test]
fn a_missing_cell_is_a_missing_region_and_gives_no_turn() {
    let bake = bake("missing");
    let directory = Directory::new(&bake.directory).unwrap();
    let downloads = directory.downloads();
    let ids: Vec<u32> = (0..downloads / 2).map(|record| directory.cell(record).unwrap().id).collect();
    let (full, half) = (bake.region(None), bake.region(Some(&ids)));
    let installed = |road: u32| directory.download_of_road(road).unwrap().unwrap() < downloads / 2;
    let profile = "touring";
    for radius_m in [Policy::default().radius_m, REACH_M] {
        let policy = Policy { radius_m, ..Policy::default() };
        let (mut missing, mut found) = (0, 0);
        for point in snap_points(&bake.graph) {
            // A download cell whose box meets the window and that the install lacks.
            let window = snap::window(point, radius_m).unwrap();
            let lacks = directory.meeting(window).unwrap().iter().any(|&record| record >= downloads / 2);
            let all = full.snap(point, profile, policy).unwrap();
            match half.snap(point, profile, policy) {
                Err(Error::MissingRegion(_)) if lacks => missing += 1,
                Ok(some) if !lacks => {
                    assert!(some.retained.iter().all(|c| installed(c.position.road)));
                    if all.retained.iter().all(|c| installed(c.position.road)) {
                        assert_eq!(candidates(&some), candidates(&all), "{point:?} {radius_m} m");
                    }
                    found += 1;
                }
                other => panic!(
                    "{point:?} {radius_m} m: {:?} where a cell is missing: {lacks}",
                    other.map(|c| c.retained.len())
                ),
            }
        }
        assert!(missing > 0 && found > 0, "{radius_m} m: {missing} missing, {found} found");
    }
    let mut cut = 0;
    for road in (0..bake.graph.roads.len() as u32).filter(|&road| installed(road)) {
        let (a, b) = (full.endpoint(profile, road).unwrap(), half.endpoint(profile, road).unwrap());
        // The roads that enter a road all arrive at its departure junction, in one download cell.
        if a.departures.iter().all(|d| installed(d.state)) {
            assert_eq!(departures(&a), departures(&b));
        } else {
            assert!(b.departures.is_empty(), "road {road}");
            cut += 1;
        }
    }
    assert!(cut > 0, "a road enters the install from a missing cell");
    let outside = (0..bake.graph.roads.len() as u32).find(|&road| !installed(road)).unwrap();
    assert!(matches!(half.with_road(outside, |_| ()), Err(Error::MissingRegion(_))));
}

#[test]
fn a_route_costs_what_its_pieces_and_turns_cost() {
    let bake = bake("cost");
    let graph = &bake.graph;
    let mut router = Router::new(bake.old.fork(), usize::MAX);
    let coordinate = |p: Point| [p.lon as f64 * 1e-6, p.lat as f64 * 1e-6];
    let junction = |k: u32| coordinate(graph.points.values()[k as usize]);
    let middle = |road: usize| coordinate(graph.roads.shape(road)[1]);
    let check = |router: &mut Router<Selection<Objects>>, request: &Request| {
        let Ok(route) = router.route(request, &Control::default()) else { return None };
        assert_eq!(router.cost(&route, &request.profile).unwrap(), Some(route.cost), "{request:?}");
        Some(route)
    };
    let mut routes = 0;
    let points = graph.points.len() as u32;
    for (k, profile) in Profile::presets().iter().enumerate() {
        for step in 0..6u32 {
            let (a, b, c) = (k as u32 * 37 + step * 11, k as u32 * 53 + step * 97 + 140, step * 131 + 7);
            let request = |points: Vec<[f64; 2]>| -> Request {
                serde_json::from_value(serde_json::json!({"points": points, "profile": profile.name})).unwrap()
            };
            let via = middle((c * 7) as usize % graph.roads.len());
            let shapes = [
                vec![junction(a % points), junction(b % points)],
                vec![middle(a as usize % graph.roads.len()), via, junction(b % points)],
                vec![junction(c % points), junction(a % points), middle(b as usize % graph.roads.len())],
            ];
            routes += shapes.into_iter().filter_map(|points| check(&mut router, &request(points))).count();
        }
    }
    assert!(routes > 100, "{routes} routes");
    // A leg from the end of one road to the start of the road after it: no piece, one turn. And a
    // leg whose two points are one place of one road: no turn.
    let selection = &bake.old;
    let (mut turn, mut place) = (false, false);
    for k in 0..points {
        let found = selection.snap(graph.points.values()[k as usize], "touring", Policy::default()).unwrap();
        let ends = found.retained.iter().filter(|c| c.position.fraction == 1.0);
        let starts = || found.retained.iter().filter(|c| c.position.fraction == 0.0);
        let Some((from, to)) = ends.flat_map(|from| starts().map(move |to| (from, to))).find(|(from, to)| {
            let entry = selection.endpoint("touring", to.position.road).unwrap();
            entry.departures.iter().any(|d| d.state == from.position.road)
        }) else {
            continue;
        };
        let mut request: Request = serde_json::from_value(serde_json::json!({
            "points": [junction(k), junction(k)], "profile": "touring",
            "start_position": from.position.id(), "end_position": to.position.id(),
        }))
        .unwrap();
        let route = check(&mut router, &request).unwrap();
        let leg = &route.legs[0];
        assert!(leg.pieces.is_empty() && leg.start.road != leg.end.road, "{leg:?}");
        turn = true;
        request.points = vec![middle(k as usize), middle(k as usize)];
        (request.start_position, request.end_position) = (None, None);
        let route = check(&mut router, &request).unwrap();
        assert_eq!(route.cost, 0);
        place = true;
        break;
    }
    assert!(turn && place);
}

#[test]
fn the_engine_on_format_10_gives_the_costs_and_candidates_of_format_9() {
    let bake = bake("search");
    let mut old = Router::new(bake.old.fork(), usize::MAX);
    let mut new = Router::new(bake.region(None), usize::MAX);
    let (mut requests, mut ties, mut routed) = (0, 0, 0);
    for profile in Profile::presets() {
        for request in compare::requests(&old, &profile.name, 40, 17, 10_000.0) {
            let (outcome, _) =
                compare::check(&mut old, &mut new, &request).unwrap_or_else(|e| panic!("{request:?}: {e}"));
            ties += usize::from(outcome == Outcome::Tie);
            routed += usize::from(old.routes(&request, &Control::default()).is_ok());
            requests += 1;
        }
    }
    assert_eq!(requests, 400);
    assert!(routed > 300, "{routed} of {requests} requests give a route");
    assert!(ties * 20 <= requests, "{ties} ties in {requests} requests");
}

#[test]
fn an_install_of_some_cells_routes_on_its_roads() {
    let bake = bake("some");
    let directory = Directory::new(&bake.directory).unwrap();
    let half = directory.downloads() / 2;
    let ids: Vec<u32> = (0..half).map(|record| directory.cell(record).unwrap().id).collect();
    let installed = |road: u32| directory.download_of_road(road).unwrap().unwrap() < half;
    let on_installed = |route: &planner_router::Route| route.pieces().all(|piece| installed(piece.road));
    let mut full = Router::new(bake.region(None), usize::MAX);
    let mut some = Router::new(bake.region(Some(&ids)), usize::MAX);
    // Points whose snap windows meet only installed cells.
    let points: Vec<[f64; 2]> = snap_points(&bake.graph)
        .into_iter()
        .filter(|&point| directory.meeting(snap::window(point, REACH_M).unwrap()).unwrap().iter().all(|&r| r < half))
        .map(|p| [p.lon as f64 * 1e-6, p.lat as f64 * 1e-6])
        .collect();
    let (mut same, mut dearer) = (0, 0);
    for pair in points.windows(2).step_by(3) {
        let request: Request =
            serde_json::from_value(serde_json::json!({"points": pair, "profile": "touring"})).unwrap();
        let all = full.route(&request, &Control::default());
        match (some.route(&request, &Control::default()), all) {
            (Ok(route), Ok(all)) => {
                assert!(on_installed(&route), "{request:?}");
                if on_installed(&all) {
                    assert_eq!(route.cost, all.cost, "{request:?}");
                    same += 1;
                } else {
                    assert!(route.cost >= all.cost, "{request:?}");
                    dearer += 1;
                }
            }
            (Err(Error::NoPath), Ok(all)) => assert!(!on_installed(&all), "{request:?}"),
            (Err(Error::NoPath), Err(Error::NoPath)) => {}
            (route, all) => panic!("{request:?}: {:?} against {:?}", route.map(|r| r.cost), all.map(|r| r.cost)),
        }
    }
    assert!(same > 0, "{same} routes as on the full install, {dearer} dearer");
}

#[test]
fn only_the_chosen_legs_are_unpacked() {
    let bake = bake("unpack");
    let points = bake.graph.points.values();
    let junction = |k: usize| [points[k].lon as f64 * 1e-6, points[k].lat as f64 * 1e-6];
    let mut router = Router::new(bake.region(None), usize::MAX);
    // The via point is a junction, so each road at it is a candidate and the first leg runs one
    // search for each.
    let request: Request = serde_json::from_value(serde_json::json!({
        "points": [junction(0), junction(points.len() / 2), junction(points.len() - 1)], "profile": "touring",
    }))
    .unwrap();
    router.package().take_stats();
    let route = router.route(&request, &Control::default()).unwrap();
    let stats = router.package().take_stats();
    assert!(stats.searches > 3, "{stats:?}");
    assert_eq!(stats.legs, 2, "{stats:?}");
    assert!(route.legs.iter().all(|leg| leg.pieces.len() > 1));
    // Each piece is a road of its leg, and a leg has at most one road before its first piece.
    let pieces = route.pieces().count();
    assert!((pieces..=pieces + 2).contains(&stats.roads), "{stats:?} for {pieces} pieces");
}
