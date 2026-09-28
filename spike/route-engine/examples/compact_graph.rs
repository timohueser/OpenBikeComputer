//! Native representation experiment. Its endpoints are free junctions, not incoming road states.
#[cfg(not(feature = "builder"))]
fn main() {
    eprintln!("Enable the builder feature to run this experiment.");
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
    use serde_json::json;
    use std::fs;
    use std::time::Instant;
    use trip_router::compact::{full_reference, Compact};
    use trip_router::model::{Graph, Point, Profile};

    pub fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if !(6..=7).contains(&args.len()) {
            return Err("compact_graph GRAPH PROFILE START_LAT START_LON END_LAT END_LON [QUERY_REPEATS]".into());
        }
        let number = |i: usize| args[i].parse::<f64>().map_err(|e| e.to_string());
        let point = |lat, lon| -> Result<Point, String> {
            let lat = number(lat)?;
            let lon = number(lon)?;
            if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 {
                return Err("Invalid coordinates".into());
            }
            Ok(Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: 0 })
        };
        let start_point = point(2, 3)?;
        let end_point = point(4, 5)?;
        let repeats = args.get(6).map(|s| s.parse::<usize>()).transpose().map_err(|e| e.to_string())?.unwrap_or(20);
        if repeats == 0 {
            return Err("QUERY_REPEATS must be positive".into());
        }
        let profile = Profile::presets().into_iter().find(|p| p.name == args[1]).ok_or("Unknown profile")?;
        let begun = Instant::now();
        eprintln!("Reading graph");
        let bytes = fs::read(&args[0]).map_err(|e| e.to_string())?;
        let graph: Graph = postcard::from_bytes(&bytes).map_err(|e| e.to_string())?;
        drop(bytes);
        let load_ms = begun.elapsed().as_secs_f64() * 1000.0;
        eprintln!("Constructing selective arrival states");
        let now = Instant::now();
        let compact = Compact::build(&graph, &profile)?;
        let compact_ms = now.elapsed().as_secs_f64() * 1000.0;
        let snap = |point: Point, start: bool| {
            graph
                .points
                .iter()
                .enumerate()
                .filter(|(id, _)| {
                    if start {
                        !compact.starts(*id as u32).is_empty()
                    } else {
                        !compact.targets(*id as u32).is_empty()
                    }
                })
                .map(|(id, p)| (id as u32, p.distance(point)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .ok_or("No endpoint junction")
        };
        let (start, start_offset_m) = snap(start_point, true)?;
        let (end, end_offset_m) = snap(end_point, false)?;
        eprintln!("Full arrival-state oracle {start} -> {end}");
        let now = Instant::now();
        let reference = full_reference(&graph, &profile, start, end).ok_or("No reference route")?;
        let reference_ms = now.elapsed().as_secs_f64() * 1000.0;
        drop(graph);
        eprintln!("Source graph released; compact Dijkstra");
        let now = Instant::now();
        let route = compact.dijkstra(start, end)?.ok_or("No compact route")?;
        let compact_dijkstra_ms = now.elapsed().as_secs_f64() * 1000.0;
        if route.cost != reference.cost {
            return Err("Compact Dijkstra differs from full oracle".into());
        }
        eprintln!("Preparing compact CH");
        let now = Instant::now();
        let ch = compact.prepare_ch()?;
        let ch_preparation_ms = now.elapsed().as_secs_f64() * 1000.0;
        let ch_arcs = ch.get_num_in_edges() + ch.get_num_out_edges();
        let mut calculator = fast_paths::PathCalculator::new(ch.get_num_nodes());
        let mut query_ms = Vec::with_capacity(repeats);
        let mut route_road_count = 0;
        for _ in 0..repeats {
            let now = Instant::now();
            let route = compact.ch_route(&ch, &mut calculator, start, end)?.ok_or("No CH route")?;
            query_ms.push(now.elapsed().as_secs_f64() * 1000.0);
            if route.cost != reference.cost {
                return Err("Compact CH differs from full oracle".into());
            }
            route_road_count = route.roads.len();
        }
        let first_query_ms = query_ms[0];
        query_ms.sort_by(f64::total_cmp);
        println!("{}", serde_json::to_string_pretty(&json!({
        "input":args[0],"profile":profile.name,"endpoint_semantics":"unconstrained junctions; full first road charged",
        "start_junction":start,"end_junction":end,"start_offset_m":start_offset_m,"end_offset_m":end_offset_m,
        "load_ms":load_ms,"compact_build_ms":compact_ms,"representation":compact.stats(),
        "full_reference_ms":reference_ms,"compact_dijkstra_ms":compact_dijkstra_ms,"oracle_cost":reference.cost,
        "ch_preparation_ms":ch_preparation_ms,"ch_states":ch.get_num_nodes(),"ch_arcs":ch_arcs,
        "ch_minimum_array_bytes":ch.get_num_nodes() * 3 * std::mem::size_of::<usize>() + ch_arcs * 5 * std::mem::size_of::<usize>(),
        "query_first_ms":first_query_ms,"query_median_ms":query_ms[query_ms.len()/2],"query_max_ms":query_ms.last(),
        "query_repeats":repeats,"route_roads":route_road_count,"source_graph_released_before_preparation":true,
        "total_ms":begun.elapsed().as_secs_f64()*1000.0,"scope":"native representation experiment; no streaming package or alternative generation"
    })).unwrap());
        Ok(())
    }
}
