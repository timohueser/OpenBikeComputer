#[cfg(not(feature = "builder"))]
fn main() {
    eprintln!("Use --features builder");
    std::process::exit(1);
}

#[cfg(feature = "builder")]
fn main() {
    if let Err(error) = experiment::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(feature = "builder")]
mod experiment {
    use serde_json::{json, Value};
    use std::cmp::Reverse;
    use std::collections::{BTreeMap, BinaryHeap, HashSet};
    use std::time::Instant;
    use trip_router::build::{self, path_cost, state_graph_with_costs};
    use trip_router::model::{Graph, Point, Profile, Surface, NO_ELEVATION};
    use trip_router::partition::WeightedGraph;

    struct Tree {
        distance: Vec<u64>,
        parent: Vec<Option<u32>>,
    }

    fn tree(graph: &WeightedGraph, start: u32) -> Result<Tree, String> {
        let mut distance = vec![u64::MAX; graph.coords.len()];
        let mut parent = vec![None; graph.coords.len()];
        let mut heap = BinaryHeap::from([Reverse((0, start))]);
        distance[start as usize] = 0;
        while let Some(Reverse((cost, from))) = heap.pop() {
            if distance[from as usize] != cost {
                continue;
            }
            for &(to, weight) in &graph.adjacency[from as usize] {
                let candidate = cost.checked_add(weight).ok_or("Tree cost overflow")?;
                if candidate < distance[to as usize] {
                    distance[to as usize] = candidate;
                    parent[to as usize] = Some(from);
                    heap.push(Reverse((candidate, to)));
                }
            }
        }
        Ok(Tree { distance, parent })
    }

    fn path(tree: &Tree, start: u32, end: u32) -> Option<Vec<u32>> {
        if tree.distance[end as usize] == u64::MAX {
            return None;
        }
        let mut nodes = vec![end];
        while *nodes.last()? != start {
            nodes.push(tree.parent[*nodes.last()? as usize]?);
        }
        nodes.reverse();
        Some(nodes)
    }

    fn reverse(graph: &WeightedGraph) -> WeightedGraph {
        let mut adjacency = vec![Vec::new(); graph.coords.len()];
        for (from, edges) in graph.adjacency.iter().enumerate() {
            for &(to, cost) in edges {
                adjacency[to as usize].push((from as u32, cost));
            }
        }
        WeightedGraph { coords: graph.coords.clone(), adjacency }
    }

    fn nearest(graph: &WeightedGraph, target: Point) -> Result<(u32, f64), String> {
        graph
            .coords
            .iter()
            .enumerate()
            .filter(|(id, _)| !graph.adjacency[*id].is_empty())
            .map(|(id, p)| (id as u32, p.distance(target)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .ok_or_else(|| "No connected endpoint".into())
    }

    fn point(lat: f64, lon: f64) -> Point {
        Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION }
    }

    struct Candidate {
        name: String,
        nodes: Vec<u32>,
        touring_cost: u64,
        metrics: [u64; 4],
        unknown_elevation_m: u64,
        unknown_surface_m: u64,
        geometry: Vec<Point>,
    }

    fn candidate(name: String, nodes: Vec<u32>, graph: &Graph, base: &WeightedGraph) -> Result<Candidate, String> {
        let touring_cost = path_cost(base, &nodes)?;
        let mut result = Candidate {
            name,
            nodes,
            touring_cost,
            metrics: [0; 4],
            unknown_elevation_m: 0,
            unknown_surface_m: 0,
            geometry: Vec::new(),
        };
        for &id in result.nodes.iter().skip(1) {
            let road = &graph.roads[id as usize];
            result.metrics[0] += road.length_m as u64;
            result.metrics[1] += road.ascent_m as u64;
            if matches!(road.surface, Surface::Compacted | Surface::Gravel | Surface::Dirt | Surface::Rough) {
                result.metrics[2] += road.length_m as u64;
            }
            if road.class == 2 {
                result.metrics[3] += road.length_m as u64;
            }
            if road.surface == Surface::Unknown {
                result.unknown_surface_m += road.length_m as u64;
            }
            if road.shape.iter().any(|p| p.elevation == NO_ELEVATION) {
                result.unknown_elevation_m += road.length_m as u64;
            }
            result.geometry.extend_from_slice(&road.shape);
        }
        Ok(result)
    }

    fn has_loop(nodes: &[u32], graph: &Graph) -> bool {
        let mut seen = HashSet::new();
        nodes.iter().any(|&id| !seen.insert(graph.roads[id as usize].to))
    }

    fn locally_optimal(nodes: &[u32], via: usize, base: &WeightedGraph, budget: u64) -> Result<bool, String> {
        let (mut left, mut right, mut cost) = (via, via, 0);
        while left > 0 && cost < budget {
            cost += path_cost(base, &nodes[left - 1..=left])?;
            left -= 1;
        }
        cost = 0;
        while right + 1 < nodes.len() && cost < budget {
            cost += path_cost(base, &nodes[right..=right + 1])?;
            right += 1;
        }
        Ok(build::dijkstra(base, nodes[left], nodes[right]) == Some(path_cost(base, &nodes[left..=right])?))
    }

    fn project(point: Point, latitude: f64) -> [f64; 2] {
        [point.lon as f64 * 0.111195 * latitude.cos(), point.lat as f64 * 0.111195]
    }

    fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
        let d = [b[0] - a[0], b[1] - a[1]];
        let denominator = d[0] * d[0] + d[1] * d[1];
        let t = if denominator == 0.0 {
            0.0
        } else {
            (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / denominator).clamp(0.0, 1.0)
        };
        (p[0] - a[0] - t * d[0]).hypot(p[1] - a[1] - t * d[1])
    }

    fn separated_m(a: &Candidate, b: &Candidate) -> f64 {
        if a.geometry.len() < 2 || b.geometry.len() < 2 {
            return 0.0;
        }
        let latitude = (a.geometry[0].lat as f64 * 1e-6).to_radians();
        let other: Vec<_> = b.geometry.iter().map(|&p| project(p, latitude)).collect();
        let mut carry = 0.0;
        let mut separated = 0.0;
        for pair in a.geometry.windows(2) {
            let (start, end) = (project(pair[0], latitude), project(pair[1], latitude));
            let length = (end[0] - start[0]).hypot(end[1] - start[1]);
            let mut offset = 200.0 - carry;
            while offset <= length {
                let t = offset / length;
                let p = [start[0] + (end[0] - start[0]) * t, start[1] + (end[1] - start[1]) * t];
                if other.windows(2).all(|s| segment_distance(p, s[0], s[1]) > 250.0) {
                    separated += 200.0;
                }
                offset += 200.0;
            }
            carry = length - (offset - 200.0);
        }
        separated
    }

    fn tradeoff(a: &Candidate, b: &Candidate) -> bool {
        let minimum_improvement = [200, 100, 500, 500];
        let minimum_loss = [200, 50, 200, 200];
        let better = (0..4).any(|i| {
            b.metrics[i].saturating_sub(a.metrics[i]) >= minimum_improvement[i]
                && a.metrics[i] as f64 <= b.metrics[i] as f64 * 0.9
        });
        let worse = (0..4).any(|i| {
            a.metrics[i].saturating_sub(b.metrics[i]) >= minimum_loss[i]
                && a.metrics[i] as f64 >= b.metrics[i] as f64 * 1.05
        });
        better && worse
    }

    fn select(candidates: &[Candidate]) -> Vec<usize> {
        let mut selected = vec![0];
        for id in 1..candidates.len() {
            let candidate = &candidates[id];
            if candidate.touring_cost as f64 > candidates[0].touring_cost as f64 * 1.35 {
                continue;
            }
            let meaningful = tradeoff(candidate, &candidates[0]);
            if !meaningful && candidate.touring_cost as f64 > candidates[0].touring_cost as f64 * 1.05 {
                continue;
            }
            if selected.iter().all(|&other| {
                (meaningful && tradeoff(candidate, &candidates[other]))
                    || (separated_m(candidate, &candidates[other]) >= 1000.0
                        && separated_m(&candidates[other], candidate) >= 1000.0)
            }) {
                selected.push(id);
            }
        }
        selected
    }

    fn query(graph: &Graph, start_point: Point, end_point: Point) -> Result<Value, String> {
        let started = Instant::now();
        let profile = Profile::presets().remove(0);
        let costs: Vec<_> = graph.roads.iter().map(|road| profile.cost(road)).collect();
        let base = state_graph_with_costs(graph, false, &costs)?;
        let backward = reverse(&base);
        let (start, start_distance) = nearest(&base, start_point)?;
        let (end, end_distance) = nearest(&backward, end_point)?;
        let forward = tree(&base, start)?;
        let reverse = tree(&backward, end)?;
        let primary = path(&forward, start, end).ok_or("No primary route between snapped states")?;
        if path_cost(&base, &primary)? != forward.distance[end as usize] {
            return Err("Primary witness mismatch".into());
        }
        if has_loop(&primary, graph) {
            return Err("Fixed endpoint states require a loop in the primary route".into());
        }
        let mut candidates = vec![candidate("touring".into(), primary, graph, &base)?];
        let mut rejected = Vec::new();
        let mut seen: HashSet<Vec<u32>> = HashSet::from([candidates[0].nodes.clone()]);
        for variant in ["shorter", "less_climbing", "smoother"] {
            let variant_costs: Vec<_> = graph
                .roads
                .iter()
                .zip(&costs)
                .map(|(road, cost)| {
                    cost.map(|cost| match variant {
                        "shorter" => (road.length_m as u64).max(1),
                        "less_climbing" => cost + road.ascent_m as u64 * 20,
                        _ => {
                            cost + if matches!(
                                road.surface,
                                Surface::Compacted | Surface::Gravel | Surface::Dirt | Surface::Rough
                            ) {
                                road.length_m as u64 * 2
                            } else {
                                0
                            }
                        }
                    })
                })
                .collect();
            let weighted = state_graph_with_costs(graph, false, &variant_costs)?;
            let tree = tree(&weighted, start)?;
            let nodes = path(&tree, start, end).ok_or("Variant lost an accessible primary route")?;
            if path_cost(&weighted, &nodes)? != tree.distance[end as usize] {
                return Err("Variant witness mismatch".into());
            }
            if has_loop(&nodes, graph) || !seen.insert(nodes.clone()) {
                rejected.push(json!({"source":variant,"reason":"loop_or_duplicate"}));
            } else {
                candidates.push(candidate(variant.into(), nodes, graph, &base)?);
            }
        }
        // Each spatial probe tries at most 32 states, avoiding a single dead-end or wrong-heading snap.
        let a = project(start_point, (start_point.lat as f64 * 1e-6).to_radians());
        let b = project(end_point, (start_point.lat as f64 * 1e-6).to_radians());
        let length = (b[0] - a[0]).hypot(b[1] - a[1]).max(1.0);
        for fraction in [0.25, 0.5, 0.75] {
            for offset in [-8000.0, -2000.0, 2000.0, 8000.0] {
                let x = a[0] + (b[0] - a[0]) * fraction - (b[1] - a[1]) / length * offset;
                let y = a[1] + (b[1] - a[1]) * fraction + (b[0] - a[0]) / length * offset;
                let via_point =
                    point(y / 111195.0, x / (111195.0 * (start_point.lat as f64 * 1e-6).to_radians().cos()));
                let name = format!("via_{fraction}_{offset}");
                let mut nearby: Vec<_> = base
                    .coords
                    .iter()
                    .enumerate()
                    .filter_map(|(id, p)| {
                        let cost = forward.distance[id].checked_add(reverse.distance[id])?;
                        (cost as f64 <= candidates[0].touring_cost as f64 * 1.35)
                            .then_some((p.distance(via_point), id as u32))
                    })
                    .collect();
                if nearby.len() > 32 {
                    nearby.select_nth_unstable_by(32, |a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                    nearby.truncate(32);
                }
                nearby.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
                let mut failures = BTreeMap::<&str, usize>::new();
                for (_, via) in nearby {
                    let mut nodes = path(&forward, start, via).ok_or("Reachable via lost forward witness")?;
                    let mut tail = path(&reverse, end, via).ok_or("Reachable via lost reverse witness")?;
                    let via_index = nodes.len() - 1;
                    tail.reverse();
                    nodes.extend_from_slice(&tail[1..]);
                    let expected = forward.distance[via as usize]
                        .checked_add(reverse.distance[via as usize])
                        .ok_or("Via cost overflow")?;
                    if path_cost(&base, &nodes)? != expected {
                        return Err("Via witness mismatch".into());
                    }
                    let reason = if has_loop(&nodes, graph) {
                        Some("loop")
                    } else if !seen.insert(nodes.clone()) {
                        Some("duplicate")
                    } else if !locally_optimal(&nodes, via_index, &base, candidates[0].touring_cost / 20)? {
                        Some("local_detour")
                    } else {
                        None
                    };
                    if let Some(reason) = reason {
                        *failures.entry(reason).or_default() += 1;
                    } else {
                        candidates.push(candidate(name.clone(), nodes, graph, &base)?);
                        break;
                    }
                }
                rejected.push(json!({"source":name,"failed_attempts":failures}));
            }
        }
        let generation_ms = started.elapsed().as_secs_f64() * 1000.0;
        candidates[1..].sort_by_key(|c| c.touring_cost);
        let selected = select(&candidates);
        let selected_names: Vec<_> = selected.iter().map(|&i| candidates[i].name.as_str()).collect();
        let summaries: Vec<_> = candidates.iter().enumerate().map(|(index, c)| json!({
            "source":c.name,"selected":selected.contains(&index),"touring_cost":c.touring_cost,
            "distance_m":c.metrics[0],"ascent_m":c.metrics[1],"unpaved_m":c.metrics[2],"main_road_m":c.metrics[3],
            "unknown_elevation_m":c.unknown_elevation_m,"unknown_surface_m":c.unknown_surface_m,
            "separated_from_primary_m":if index==0 {0.0} else {separated_m(c,&candidates[0])},
            "directed_roads":c.nodes.len()-1,"witness_verified":true,
            "selection_reason":if index==0 {"primary"} else if selected.contains(&index) {
                if tradeoff(c,&candidates[0]) {"metric_tradeoff"} else {"distinct_near_equal"}
            } else {"insufficient_quality_or_difference"}
        })).collect();
        Ok(
            json!({"generation_resident_dijkstra_ms":generation_ms,"total_with_selection_ms":started.elapsed().as_secs_f64()*1000.0,
            "requested_coordinates":{"start":[start_point.lat as f64*1e-6,start_point.lon as f64*1e-6],"end":[end_point.lat as f64*1e-6,end_point.lon as f64*1e-6]},
            "snapping_m":{"start":start_distance,"end":end_distance},"selected":selected_names,
            "candidates":summaries,"rejected_during_generation":rejected}),
        )
    }

    pub fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.len() != 1 && args.len() != 5 {
            return Err("Usage: route_alternatives GRAPH [startlat startlon endlat endlon]".into());
        }
        let graph: Graph =
            postcard::from_bytes(&std::fs::read(&args[0]).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let pairs = if args.len() == 5 {
            let p: Vec<f64> = args[1..]
                .iter()
                .map(|s| s.parse().map_err(|e: std::num::ParseFloatError| e.to_string()))
                .collect::<Result<_, _>>()?;
            if p.iter().any(|p| !p.is_finite())
                || p[0].abs() > 90.0
                || p[2].abs() > 90.0
                || p[1].abs() > 180.0
                || p[3].abs() > 180.0
            {
                return Err("Invalid endpoint coordinates".into());
            }
            vec![(point(p[0], p[1]), point(p[2], p[3]))]
        } else {
            vec![(point(49.611, 6.13), point(49.955, 6.029)), (point(49.496, 5.98), point(49.811, 6.421))]
        };
        let mut results = Vec::new();
        for (start, end) in pairs {
            eprintln!("Generating finite alternatives on the resident real graph");
            results.push(query(&graph, start, end)?);
        }
        println!("{}",serde_json::to_string_pretty(&json!({"queries":results,"source_warnings":graph.warnings,
                "method":"Four finite exact cost queries plus twelve single-via probes of at most 32 states each",
                "frozen_cost_variants":{"touring":"Existing touring preset; identical access and turn rules for all variants",
                    "shorter":"max(1, road length metres)","less_climbing":"touring cost + 20 times ascent metres",
                    "smoother":"touring cost + 2 times known unpaved metres; unknown surface remains unknown"},
            "limitations":["Resident Dijkstra candidate-generation experiment; timings are not accelerated client performance",
                "No full Pareto set, global regret guarantee, or arbitrary hard-resource constraints",
                "Via routes pass a local window test; uniformly bounded stretch is not proved",
                "Fixed incoming-road endpoint states can constrain snapping",
                "Illustrative selection thresholds; geographic separation is sampled at 200 m with a 250 m corridor",
                "Metric tradeoffs use whole-route totals; small local metric changes can be missed",
                "No routing beyond imported graph bounds; unknown elevation cannot validate climbing alternatives"]})).unwrap());
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn one_corridor_does_not_force_an_alternative() {
            let points: Vec<_> = (0..4).map(|i| point(49.0 + i as f64 * 0.01, 6.0)).collect();
            let roads = (0..3)
                .map(|i| trip_router::model::Road {
                    from: i,
                    to: i + 1,
                    way: i as i64,
                    length_m: 1112,
                    ascent_m: 0,
                    descent_m: 0,
                    surface: Surface::Paved,
                    class: 0,
                    access: trip_router::model::BIKE,
                    difficulty: 0,
                    hiking_difficulty: None,
                    uncertain_access: false,
                    shape: vec![points[i as usize], points[i as usize + 1]],
                })
                .collect();
            let graph = Graph { points, roads, forbidden: vec![], forbidden_foot: vec![], warnings: vec![] };
            let result = query(&graph, graph.points[1], graph.points[3]).unwrap();
            assert_eq!(result["selected"], json!(["touring"]));
            assert_eq!(result["candidates"][0]["distance_m"], 2224);
            assert_eq!(result["candidates"][0]["witness_verified"], true);
        }

        #[test]
        fn same_corridor_can_offer_a_surface_tradeoff() {
            let primary = Candidate {
                name: "primary".into(),
                nodes: vec![],
                touring_cost: 10000,
                metrics: [10000, 100, 2000, 0],
                unknown_elevation_m: 0,
                unknown_surface_m: 0,
                geometry: vec![point(49.0, 6.0), point(49.1, 6.0)],
            };
            let other = Candidate {
                name: "paved".into(),
                nodes: vec![],
                touring_cost: 11000,
                metrics: [11000, 100, 0, 0],
                unknown_elevation_m: 0,
                unknown_surface_m: 0,
                geometry: primary.geometry.clone(),
            };
            assert_eq!(select(&[primary, other]), vec![0, 1]);
        }

        #[test]
        fn equal_cost_diamond_generates_a_distinct_via_route() {
            let points = vec![
                point(49.0, 5.99),
                point(49.0, 6.0),
                point(49.02, 6.05),
                point(48.98, 6.05),
                point(49.0, 6.1),
                point(49.0, 6.11),
            ];
            let roads = [(0, 1), (1, 2), (2, 4), (1, 3), (3, 4), (4, 5)]
                .into_iter()
                .enumerate()
                .map(|(id, (from, to))| trip_router::model::Road {
                    from,
                    to,
                    way: id as i64,
                    length_m: if id == 0 || id == 5 { 1000 } else { 5000 },
                    ascent_m: 0,
                    descent_m: 0,
                    surface: Surface::Paved,
                    class: 0,
                    access: trip_router::model::BIKE,
                    difficulty: 0,
                    hiking_difficulty: None,
                    uncertain_access: false,
                    shape: vec![points[from as usize], points[to as usize]],
                })
                .collect();
            let graph = Graph { points, roads, forbidden: vec![], forbidden_foot: vec![], warnings: vec![] };
            let result = query(&graph, graph.points[1], graph.points[5]).unwrap();
            assert_eq!(result["selected"].as_array().unwrap().len(), 2);
            assert!(result["candidates"].as_array().unwrap().iter().any(|candidate| candidate["selected"] == true
                && candidate["selection_reason"] == "distinct_near_equal"
                && candidate["touring_cost"] == result["candidates"][0]["touring_cost"]));
        }
    }
}
