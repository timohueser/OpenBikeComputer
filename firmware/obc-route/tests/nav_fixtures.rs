#![cfg(feature = "external-fixtures")]

mod common;
use common::nav::plan_p;
use common::{route_points, VecSink};
use obc_elevation::NullElevation;
use obc_formats::io::SliceSource;
use obc_map_scene::{cos_lat, delta_m, BBox};
use obc_reader::{MapCache, MapProfile, MapTables, NavNeighbor, NavTileCache, Reader};
use obc_route::nav::{plan_route, NavPlanner, NavScratch, Step};
use obc_route::BikeType;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Over the real packed grimsel map, the same endpoints planned under Road (profile 0) and MTB
/// (profile 2) give different polylines: the profile weights steer the search end to end. The raw
/// lengths differ by ~2.8 km, so the assert cannot pass on emit jitter.
///
/// The endpoints are pinned from a deterministic sweep of the map's own nav nodes, chosen inside
/// the canonical grimsel extract bbox (`8.15034,46.48261,8.46007,46.72070`). The header bbox is
/// always wider than the extract, so pinning against the extract bbox is what survives a re-pack.
/// A re-pack from a newer OSM snapshot can still move the graph enough to need a re-pin.
// Reads the fixture from disk, which Miri's default isolation forbids, so it is skipped there. The
// UB tripwire is the record decode over the synthetic fixtures, which stay in the Miri run.
#[cfg_attr(miri, ignore)]
#[test]
fn road_vs_mtb_diverge_over_grimsel() {
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let from = (8_169_610, 46_694_536);
    let to = (8_217_309, 46_706_261);

    let (road, obcr_road, _) = plan_p(&bytes, from, to, "Road", BikeType::Road);
    let (mtb, obcr_mtb, _) = plan_p(&bytes, from, to, "MTB", BikeType::Mtb);
    let road = road.expect("Road plans");
    let mtb = mtb.expect("MTB plans");

    let pts_road = route_points(&obcr_road);
    let pts_mtb = route_points(&obcr_mtb);
    assert_ne!(pts_road, pts_mtb, "Road vs MTB must pick different polylines here");
    assert_ne!(road.total_distance_m, mtb.total_distance_m, "the two profiles' picks differ in raw ground length too");
}

/// The Grimsel map's nav graph planned through the Grimsel terrain sidecar, the same file the
/// simulator mounts. Nothing synthetic: this is the number a rider would see on the Route overview.
// Reads the committed fixtures from disk, so Miri skips it.
#[cfg_attr(miri, ignore)]
#[test]
fn a_real_grimsel_plan_carries_the_pass_road_profile() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let dem = obc_fixtures::read("sim-grimsel", "grimsel.obcd");
    let terrain_src = SliceSource(&dem);
    let mut terrain = obc_elevation::TerrainElevation::<{ obc_elevation::DEFAULT_TILE_SLOTS }>::parse(&terrain_src)
        .expect("the baked terrain parses");

    // Innertkirchen → up the pass road (the profile-divergence fixture's endpoints).
    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let src = SliceSource(&map);
    let tables = MapTables::parse(&src).expect("grimsel parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<{ obc_route::NAV_MAX_NODES }>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let route = plan_route(&r, from, to, "Grimsel", BikeType::Road, &mut scratch, &mut tiles, &mut terrain, &mut sink)
        .expect("the pass road plans");

    // Alpine ground, never the 0 m a missing fill would leave, and a real but not absurd climb.
    assert!((500..=2_200).contains(&route.min_ele_m), "min {} m is not alpine ground", route.min_ele_m);
    assert!((500..=2_600).contains(&route.max_ele_m), "max {} m is not alpine ground", route.max_ele_m);
    assert!(route.max_ele_m > route.min_ele_m + 100, "a pass road is not flat ({route:?})");
    assert!((100..=3_000).contains(&route.total_ascent_m), "ascent {} m is implausible", route.total_ascent_m);
    let pts = route_points(&sink.buf);
    assert!(pts.iter().all(|p| p.ele > 0), "every stored point has a real height");
    let (hits, misses) = terrain.stats();
    assert!(hits > misses, "the 4-tile cache serves the walk ({hits} hit / {misses} miss)");
}

/// Round-trip parity, the property the shared dead-band exists for: write a planned route out as
/// GPX and re-import it through [`gpx_to_obcr`](obc_route::gpx_to_obcr). The re-imported climb must
/// agree with the header the planner wrote. Without the emit-time fill both sides are 0 and the
/// check is vacuous. This route lies wholly inside the terrain coverage, so every exported point
/// has a measured height and the export cannot turn an unknown span into a false sample.
#[cfg_attr(miri, ignore)] // reads the committed fixtures from disk — see the note above
#[test]
fn a_planned_route_exported_to_gpx_and_reimported_keeps_its_climb() {
    let map = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let dem = obc_fixtures::read("sim-grimsel", "grimsel.obcd");
    let terrain_src = SliceSource(&dem);
    let mut terrain =
        obc_elevation::TerrainElevation::<{ obc_elevation::DEFAULT_TILE_SLOTS }>::parse(&terrain_src).unwrap();

    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let src = SliceSource(&map);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut scratch = Box::new(NavScratch::<{ obc_route::NAV_MAX_NODES }>::new());
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let planned =
        plan_route(&r, from, to, "Grimsel", BikeType::Road, &mut scratch, &mut tiles, &mut terrain, &mut sink)
            .expect("plans");

    // The export: one `<trkpt>` per stored point, exactly the fields an exporter has to hand.
    let mut gpx = String::from("<gpx><trk><trkseg>");
    for p in route_points(&sink.buf) {
        assert!(p.ele > 0, "an exported point with no height would export a lie");
        gpx.push_str(&format!(
            "<trkpt lat=\"{:.6}\" lon=\"{:.6}\"><ele>{}</ele></trkpt>",
            p.lat as f64 / 1e6,
            p.lon as f64 / 1e6,
            p.ele
        ));
    }
    gpx.push_str("</trkseg></trk></gpx>");

    let mut back = VecSink::default();
    let reimported = obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Grimsel", &mut back).expect("re-imports");

    assert_eq!((reimported.min_ele_m, reimported.max_ele_m), (planned.min_ele_m, planned.max_ele_m));
    let (a, b) = (planned.total_ascent_m as i64, reimported.total_ascent_m as i64);
    assert!(
        (a - b).abs() * 20 <= a.max(1),
        "planned +{a} m vs re-imported +{b} m — the two dead-band integrations must agree within 5%"
    );
}

/// On every stock profile the registered Grimsel route is a connected walk of the nav graph from
/// the start node to the goal node, takes no edge class the profile bans, and costs at most the
/// ratio of its [`NAV_EPSILON_LADDER`](obc_route::nav::NAV_EPSILON_LADDER) rung times a plain
/// Dijkstra optimum over the same packed edges. That ratio is the router's own bound, so a change
/// that keeps routes correct passes and a wrong path, cost or ban fails.
#[cfg_attr(miri, ignore)]
#[test]
fn the_registered_grimsel_fixture_routes_within_the_epsilon_bound_on_every_profile() {
    let bytes = obc_fixtures::read("sim-grimsel", "grimsel.obcm");
    let (from, to) = ((8_169_610, 46_694_536), (8_217_309, 46_706_261));
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).expect("grimsel parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let graph = load_graph(&r);
    // Both endpoints are nav nodes, so the snap keeps them exact and the oracle runs node to node.
    let node_at = |c| *graph.iter().find(|(_, n)| n.coord == c).expect("the pinned endpoint is a nav node").0;
    let (start, goal) = (node_at(from), node_at(to));
    let mut tiles = NavTileCache::new();
    for bike in BikeType::ALL {
        let profile = r.nav_profiles().get(bike as usize).expect("grimsel registers every stock profile");
        let (eps, obcr) = plan_with_rung(&r, from, to, bike);
        let pts: Vec<_> = route_points(&obcr).iter().map(|p| (p.lon, p.lat)).collect();
        assert_eq!((pts[0], pts[pts.len() - 1]), (from, to), "{bike:?} starts and ends at the requested nodes");
        let mut on_path = HashSet::from([start]);
        let hops = trace(&r, &mut tiles, &graph, &pts, start, 0, goal, &mut on_path)
            .unwrap_or_else(|| panic!("{bike:?} route is not a connected walk of the nav graph"));
        let found: u32 = hops
            .iter()
            .map(|nb| {
                edge_cost(profile, nb)
                    .unwrap_or_else(|| panic!("{bike:?} takes edge {} of banned kind {:#04x}", nb.edge_id, nb.way_kind))
            })
            .sum();
        let optimum = dijkstra(&graph, profile, start, goal);
        // The router keeps `g` in a saturating u16, so its bound holds only below `u16::MAX`.
        assert!(optimum < u32::from(u16::MAX), "{bike:?} optimum {optimum} saturates the router's g");
        assert!(optimum <= found, "{bike:?} beat the oracle ({found} < {optimum}): the oracle is wrong");
        assert!(
            found * eps.1 <= optimum * eps.0,
            "{bike:?} cost {found} exceeds {}/{} of the optimum {optimum}",
            eps.0,
            eps.1
        );
    }
}

/// How far an edge vertex the emitter dropped may lie from the route chord. The decimator tests a
/// vertex only against its local chord, so the drift from the final chord can pass its 1 m on a
/// gentle curve. Closest-fit order keeps a parallel edge inside this band from winning.
const TRACE_TOL_M: f32 = 10.0;

struct GraphNode {
    coord: (i32, i32),
    neighbors: Vec<NavNeighbor>,
}

fn load_graph(r: &Reader) -> HashMap<u32, GraphNode> {
    let all = BBox { min_lon: i32::MIN, min_lat: i32::MIN, max_lon: i32::MAX, max_lat: i32::MAX };
    let mut chunk = vec![0; r.nav_directory().chunk_size];
    let mut graph = HashMap::new();
    r.for_each_nav_node(&all, &mut chunk, |n| {
        graph.insert(n.id, GraphNode { coord: (n.lon, n.lat), neighbors: n.neighbors().collect() });
    })
    .expect("the nav graph reads");
    graph
}

/// The router's edge weight: the profile-weighted ground length plus the directional ascent at the
/// climb weight. `None` for a banned class.
fn edge_cost(p: &MapProfile, nb: &NavNeighbor) -> Option<u32> {
    Some(((nb.cost_m * p.multiplier(nb.way_kind)?) >> 4) + u32::from(nb.ascent_m) * u32::from(p.climb_weight()))
}

fn dijkstra(graph: &HashMap<u32, GraphNode>, p: &MapProfile, start: u32, goal: u32) -> u32 {
    let mut dist = HashMap::from([(start, 0u32)]);
    let mut heap = BinaryHeap::from([Reverse((0u32, start))]);
    while let Some(Reverse((d, u))) = heap.pop() {
        if u == goal {
            return d;
        }
        if dist.get(&u).is_some_and(|&best| d > best) {
            continue;
        }
        for nb in graph.get(&u).map_or(&[][..], |n| &n.neighbors) {
            let Some(w) = edge_cost(p, nb) else { continue };
            if dist.get(&nb.id).is_none_or(|&best| d + w < best) {
                dist.insert(nb.id, d + w);
                heap.push(Reverse((d + w, nb.id)));
            }
        }
    }
    panic!("the goal is unreachable");
}

/// Plan to completion and report the epsilon rung the plan ended on.
fn plan_with_rung(r: &Reader, from: (i32, i32), to: (i32, i32), bike: BikeType) -> ((u32, u32), Vec<u8>) {
    let mut scratch = NavScratch::<{ obc_route::NAV_MAX_NODES }>::new();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    let mut planner = NavPlanner::new(from, to, "Grimsel", bike);
    loop {
        match planner.step(r, &mut scratch, &mut tiles, &mut NullElevation, &mut sink) {
            Step::Running => {}
            Step::Done(_) => return (planner.epsilon_used(), sink.buf),
            Step::Failed(e) => panic!("{bike:?} plans on grimsel, got {e:?}"),
        }
    }
}

/// The graph edges the polyline `pts` walks from `at` (route point `i`) to `goal`. Where
/// parallel edges both fit, the closer one is tried first; a dead end backtracks. A node is
/// visited once, as on any positive-cost optimal path.
#[allow(clippy::too_many_arguments)]
fn trace(
    r: &Reader,
    tiles: &mut NavTileCache,
    graph: &HashMap<u32, GraphNode>,
    pts: &[(i32, i32)],
    at: u32,
    i: usize,
    goal: u32,
    on_path: &mut HashSet<u32>,
) -> Option<Vec<NavNeighbor>> {
    if at == goal && i + 1 == pts.len() {
        return Some(Vec::new());
    }
    let node = graph.get(&at)?;
    let mut fits = Vec::new();
    for nb in node.neighbors.iter().filter(|nb| !on_path.contains(&nb.id)) {
        let mut edge = Vec::new();
        r.nav_edge_oriented(tiles, nb.edge_id, node.coord, |p| edge.push(p)).expect("an edge starts at its node");
        if let Some((next, drift)) = follow(pts, i, &edge) {
            fits.push((drift, next, *nb));
        }
    }
    fits.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, next, nb) in fits {
        on_path.insert(nb.id);
        if let Some(mut rest) = trace(r, tiles, graph, pts, nb.id, next, goal, on_path) {
            rest.insert(0, nb);
            return Some(rest);
        }
        on_path.remove(&nb.id);
    }
    None
}

/// The last route point `edge` passes and its farthest vertex from the route, if it runs along
/// `pts` from point `i`. With no terrain the emitter inserts no point, so every route point is an
/// exact edge vertex, and a vertex it dropped lies near the chord between the points around it.
fn follow(pts: &[(i32, i32)], mut i: usize, edge: &[(i32, i32)]) -> Option<(usize, f32)> {
    let mut drift = 0.0f32;
    for &q in &edge[1..] {
        if pts.get(i + 1) == Some(&q) {
            i += 1;
        } else {
            drift = drift.max(seg_dist_m(q, pts[i], *pts.get(i + 1)?));
        }
    }
    (drift <= TRACE_TOL_M).then_some((i, drift))
}

fn seg_dist_m(p: (i32, i32), a: (i32, i32), b: (i32, i32)) -> f32 {
    let cl = cos_lat(p.1);
    let ((bx, by), (px, py)) = (delta_m(a, b, cl), delta_m(a, p, cl));
    let len2 = bx * bx + by * by;
    let t = if len2 > 0.0 { ((px * bx + py * by) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (px - bx * t).hypot(py - by * t)
}
