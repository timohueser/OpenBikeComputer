use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use trip_router::build;
use trip_router::model::{Graph, Point, Profile, NO_ELEVATION};
use trip_router::partition::{Partition, WeightedGraph};
use trip_router::{elevation, osm, storage};

const USAGE: &str = "import OUTGRAPH west south east north DEMDIR_OR_- PBF...\nbenchmark GRAPH OUTDIR PROFILE startlat startlon endlat endlon [max_cell_nodes] [--ch-only]";

fn number(value: &str) -> Result<f64, String> {
    value.parse::<f64>().map_err(|e| e.to_string()).and_then(|v| {
        if v.is_finite() {
            Ok(v)
        } else {
            Err("Coordinates must be finite".into())
        }
    })
}

fn coordinate(lat: &str, lon: &str) -> Result<Point, String> {
    let (lat, lon) = (number(lat)?, number(lon)?);
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return Err("Coordinates outside geographic range".into());
    }
    Ok(Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION })
}

fn import(args: &[String]) -> Result<Value, String> {
    if args.len() < 7 {
        return Err(USAGE.into());
    }
    let now = Instant::now();
    let bounds = [number(&args[1])?, number(&args[2])?, number(&args[3])?, number(&args[4])?];
    let paths: Vec<_> = args[6..].iter().map(PathBuf::from).collect();
    eprintln!("Importing {} PBF files", paths.len());
    let mut graph = osm::import(&paths, bounds, None)?;
    if args[5] != "-" {
        eprintln!("Applying elevation data");
        elevation::apply_dem(&mut graph, Path::new(&args[5]))?;
    }
    let bytes = postcard::to_allocvec(&graph).map_err(|e| e.to_string())?;
    fs::write(&args[0], &bytes).map_err(|e| e.to_string())?;
    Ok(json!({"command":"import", "points":graph.points.len(), "directed_roads":graph.roads.len(),
        "raw_graph_bytes":bytes.len(), "elapsed_ms":now.elapsed().as_secs_f64()*1000.0,
        "warnings":graph.warnings, "bounds":bounds}))
}

fn snap(graph: &WeightedGraph, point: Point, arriving: bool) -> Result<(u32, f64), String> {
    let mut eligible = vec![false; graph.coords.len()];
    for (from, edges) in graph.adjacency.iter().enumerate() {
        if arriving {
            for &(to, _) in edges {
                eligible[to as usize] = true;
            }
        } else {
            eligible[from] = !edges.is_empty();
        }
    }
    graph
        .coords
        .iter()
        .enumerate()
        .filter(|(id, _)| eligible[*id])
        .map(|(id, &p)| (id as u32, p.distance(point)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .ok_or_else(|| "No eligible endpoint state".into())
}

fn verify(graph: &WeightedGraph, nodes: &[u32], start: u32, end: u32, cost: u64, oracle: u64) -> Result<(), String> {
    if nodes.first() != Some(&start) || nodes.last() != Some(&end) {
        return Err("Reconstructed path has wrong endpoints".into());
    }
    if cost != oracle || build::path_cost(graph, nodes)? != oracle {
        return Err(format!("Reconstructed path or query differs from oracle cost {oracle}"));
    }
    Ok(())
}

struct GeometryPages {
    graph_path: PathBuf,
    directory: PathBuf,
    sizes: BTreeMap<u32, usize>,
}

impl GeometryPages {
    fn route(&mut self, nodes: &[u32]) -> Result<Value, String> {
        let now = Instant::now();
        let graph: Graph =
            postcard::from_bytes(&fs::read(&self.graph_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        // The seed state describes the incoming road; its traversal is not part of the query.
        let roads = &nodes[1..];
        let pages: BTreeSet<_> = roads.iter().map(|id| id / storage::ROADS_PER_PAGE).collect();
        fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        for &page in &pages {
            if self.sizes.contains_key(&page) {
                continue;
            }
            let first = page as usize * storage::ROADS_PER_PAGE as usize;
            let end = (first + storage::ROADS_PER_PAGE as usize).min(graph.roads.len());
            let bytes = storage::encode(&(first as u32, &graph.roads[first..end]))?;
            fs::write(self.directory.join(format!("{page}.bin")), &bytes).map_err(|e| e.to_string())?;
            self.sizes.insert(page, bytes.len());
        }
        let bytes: usize = pages.iter().map(|p| self.sizes[p]).sum();
        let sum =
            |f: fn(&trip_router::model::Road) -> u64| roads.iter().map(|id| f(&graph.roads[*id as usize])).sum::<u64>();
        Ok(json!({"compressed_bytes":bytes, "pages":pages.len(), "directed_roads":roads.len(),
            "distance_m":sum(|r| r.length_m as u64), "ascent_m":sum(|r| r.ascent_m as u64),
            "unknown_elevation_m":sum(|r| if r.shape.iter().any(|p| p.elevation == NO_ELEVATION) {r.length_m as u64} else {0}),
            "page_creation_or_reuse_ms":now.elapsed().as_secs_f64()*1000.0}))
    }
}

fn benchmark(args: &[String]) -> Result<Value, String> {
    if !(7..=9).contains(&args.len()) {
        return Err(USAGE.into());
    }
    let mut cell_nodes = 1024;
    let mut ch_only = false;
    for arg in &args[7..] {
        if arg == "--ch-only" {
            ch_only = true;
        } else {
            cell_nodes = arg.parse::<usize>().map_err(|e| e.to_string())?;
        }
    }
    if cell_nodes == 0 {
        return Err("Cell size must be positive".into());
    }
    let profile = Profile::presets()
        .into_iter()
        .find(|p| p.name == args[2])
        .ok_or_else(|| format!("Unknown preset: {}", args[2]))?;
    let requested_start = coordinate(&args[3], &args[4])?;
    let requested_end = coordinate(&args[5], &args[6])?;
    let load_started = Instant::now();
    let graph: Graph =
        postcard::from_bytes(&fs::read(&args[0]).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let source_load_ms = load_started.elapsed().as_secs_f64() * 1000.0;
    let weighted = build::state_graph(&graph, &profile)?;
    let warnings = graph.warnings.clone();
    drop(graph);
    let (start, start_distance) = snap(&weighted, requested_start, false)?;
    let (end, end_distance) = snap(&weighted, requested_end, true)?;
    eprintln!("Exact reference query on {} states", weighted.coords.len());
    let now = Instant::now();
    let oracle = build::dijkstra(&weighted, start, end).ok_or("No path between snapped endpoint states")?;
    let oracle_ms = now.elapsed().as_secs_f64() * 1000.0;
    let directory = Path::new(&args[1]);
    let mut geometry = GeometryPages {
        graph_path: PathBuf::from(&args[0]),
        directory: directory.join("geometry"),
        sizes: BTreeMap::new(),
    };
    let mut endpoint_bytes = None;
    eprintln!("Preparing and querying contraction hierarchy");
    let ch_result = (|| -> Result<Value, String> {
        let directory = directory.join("ch");
        let prepared = build::prepare(&weighted, &directory)?;
        let query =
            build::query(&prepared, &weighted, &directory, start, end)?.ok_or("CH failed to find oracle route")?;
        verify(&weighted, &query.path_nodes, start, end, query.cost, oracle)?;
        endpoint_bytes = Some(query.endpoint_bytes);
        let detail = geometry.route(&query.path_nodes)?;
        Ok(
            json!({"status":"ok", "seeds":{"start_rank":prepared.ranks[start as usize],"start_road":start,"end_rank":prepared.ranks[end as usize],"end_road":end}, "preparation":prepared.stats, "query_with_local_disk_read_decode":query,
            "geometry":detail, "complete_selected_payload_bytes":query.summary_bytes + query.endpoint_bytes + detail["compressed_bytes"].as_u64().unwrap() as usize,
            "oracle_cost_and_witness_verified":true}),
        )
    })();
    let ch = ch_result.unwrap_or_else(|error| json!({"status":"error", "error":error}));
    let partition = if ch_only {
        json!({"status":"skipped", "reason":"--ch-only"})
    } else {
        eprintln!("Preparing and querying one-level coordinate partition overlay");
        let result = (|| -> Result<Value, String> {
            let now = Instant::now();
            let overlay = Partition::build(&weighted, cell_nodes, 1024)?;
            let preparation_ms = now.elapsed().as_secs_f64() * 1000.0;
            let now = Instant::now();
            let route = overlay.query(start, end)?.ok_or("Partition failed to find oracle route")?;
            let resident_query_ms = now.elapsed().as_secs_f64() * 1000.0;
            verify(&weighted, &route.path_nodes, start, end, route.cost, oracle)?;
            let stats = overlay.stats().clone();
            drop(overlay);
            let detail = geometry.route(&route.path_nodes)?;
            let transfer = &route.transfer;
            let graph_bytes = transfer.directory_bytes + transfer.summary_bytes + transfer.detail_bytes;
            Ok(json!({"status":"ok", "preparation_ms":preparation_ms, "preparation":stats,
                "resident_query_and_reconstruction_ms":resident_query_ms, "cost":route.cost, "settled_nodes":route.settled_nodes,
                "selected_payload":{"directory_bytes":transfer.directory_bytes,"summary_bytes":transfer.summary_bytes,
                    "detail_bytes":transfer.detail_bytes,"summary_cells":transfer.summary_cells.len(),"detail_cells":transfer.detail_cells.len()},
                "common_ch_endpoint_payload_bytes":endpoint_bytes, "geometry":detail,
                "complete_selected_payload_bytes":endpoint_bytes.map(|n| n + graph_bytes + detail["compressed_bytes"].as_u64().unwrap() as usize),
                "oracle_cost_and_witness_verified":true}))
        })();
        result.unwrap_or_else(|error| json!({"status":"error", "error":error}))
    };
    Ok(json!({"command":"benchmark", "profile":profile.name,
        "source_graph_load_ms":source_load_ms, "states":weighted.coords.len(),
        "transitions":weighted.adjacency.iter().map(Vec::len).sum::<usize>(),
        "snapping":{"start_distance_m":start_distance,"end_distance_m":end_distance,"start_state":start,"end_state":end},
        "reference":{"cost":oracle,"elapsed_ms":oracle_ms}, "ch":ch,"partition":partition,"warnings":warnings,
        "limitations":["Regional input bounds can exclude better routes outside the graph",
            "Endpoints are snapped in resident data to fixed incoming-road states, not a production spatial lookup",
            "CH query reads and decodes local files; partition query uses resident structures",
            "Payload counts exclude HTTP overhead, basemap data and alternative routes",
            "Geometry pages include all road attributes and elevation samples for chunks of 128 road IDs",
            "Reported owned storage excludes source graph, query heaps, allocator overhead and process RSS",
            "No network, browser or iPhone timing is measured"]}))
}

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("import") => import(&args[1..]),
        Some("benchmark") => benchmark(&args[1..]),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            println!("{}", json!({"status":"error", "error":error}));
            std::process::exit(1);
        }
    }
}
