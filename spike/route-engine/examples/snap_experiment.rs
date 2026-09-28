#[cfg(not(feature = "builder"))]
fn main() {
    eprintln!("Run this example with --features builder");
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
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Instant;
    use trip_router::model::{Graph, Point, Profile, NO_ELEVATION};
    use trip_router::search::{Progress, Search};
    use trip_router::snap::{self, CorePath, Policy, SpatialIndex};
    use trip_router::storage::{self, Cache, Seed};

    struct Core<'a> {
        graph: &'a Graph,
        root: PathBuf,
        cache: Cache,
        ranks: HashMap<u32, u32>,
        calls: usize,
    }

    impl Core<'_> {
        fn seed(&mut self, road: u32) -> Result<Seed, String> {
            if !self.ranks.contains_key(&road) {
                let point = self.graph.points[self.graph.roads[road as usize].to as usize];
                let (lat, lon) = storage::snap_cell(point);
                let entries: Vec<(Point, u32, u32)> = storage::decode(
                    &fs::read(self.root.join(format!("ch/lookup/{lat}_{lon}.bin"))).map_err(|e| e.to_string())?,
                )?;
                for (_, id, rank) in entries {
                    self.ranks.insert(id, rank);
                }
            }
            Ok(Seed { node: *self.ranks.get(&road).ok_or("Missing lookup rank")?, road, cost: 0 })
        }

        fn query(&mut self, start: u32, end: u32) -> Result<Option<CorePath>, String> {
            let mut search = Search::new(&[self.seed(start)?], &[self.seed(end)?], 1_000_000);
            self.calls += 1;
            loop {
                match search.poll(&self.cache, 4096) {
                    Progress::Working { .. } => {}
                    Progress::NeedPages { pages } => {
                        for id in pages {
                            if self.cache.pages.contains_key(&id) {
                                return Err("Incomplete cached summary page".into());
                            }
                            let bytes = fs::read(self.root.join(format!("ch/{id}.bin"))).map_err(|e| e.to_string())?;
                            self.cache.insert_page(id, &bytes, 512 * 1024 * 1024)?;
                        }
                    }
                    Progress::Done { cost, roads, .. } => return Ok(Some(CorePath { cost, roads })),
                    Progress::NoPath => return Ok(None),
                    other => return Err(format!("Core query failed: {other:?}")),
                }
            }
        }
    }

    fn point_at(graph: &Graph, id: u32, fraction: f64) -> Result<Point, String> {
        let road = graph.roads.get(id as usize).ok_or("Request road outside graph")?;
        let length: f64 = road.shape.windows(2).map(|p| p[0].distance(p[1])).sum();
        let mut remaining = length * fraction;
        for pair in road.shape.windows(2) {
            let distance = pair[0].distance(pair[1]);
            if remaining <= distance && distance > 0.0 {
                let t = remaining / distance;
                return Ok(Point {
                    lat: (pair[0].lat as f64 + (pair[1].lat as f64 - pair[0].lat as f64) * t).round() as i32 + 3,
                    lon: (pair[0].lon as f64 + (pair[1].lon as f64 - pair[0].lon as f64) * t).round() as i32,
                    elevation: NO_ELEVATION,
                });
            }
            remaining -= distance;
        }
        Err("Request road has no geometry".into())
    }

    pub fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.len() != 4 {
            return Err("Usage: snap_experiment GRAPH PREPARED_ROOT SERVER_REQUESTS_JSON OUTPUT_JSON".into());
        }
        let now = Instant::now();
        let graph: Graph =
            postcard::from_bytes(&fs::read(&args[0]).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let load_ms = now.elapsed().as_secs_f64() * 1000.0;
        let requests: Vec<Value> =
            serde_json::from_slice(&fs::read(&args[2]).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let profile = Profile::presets().into_iter().find(|p| p.name == "touring").ok_or("Missing touring preset")?;
        let policy = Policy { radius_m: 50.0, ambiguity_m: 3.0, max_candidates: 4 };
        let mut cases = Vec::new();
        let mut points = Vec::new();
        for name in ["local-start", "short-north", "regional-central"] {
            let request =
                requests.iter().find(|r| r["name"] == name).ok_or_else(|| format!("Missing request {name}"))?;
            let id = |key: &str| -> Result<u32, String> {
                request[key]["road"]
                    .as_u64()
                    .ok_or("Missing road ID")?
                    .try_into()
                    .map_err(|_| "Road ID overflow".into())
            };
            let start = point_at(&graph, id("start")?, 0.37)?;
            let end = point_at(&graph, id("end")?, 0.63)?;
            points.extend([start, end]);
            cases.push((name, start, end));
        }
        let now = Instant::now();
        let index = SpatialIndex::around(&graph, &points, policy.radius_m)?;
        let index_ms = now.elapsed().as_secs_f64() * 1000.0;
        let mut core = Core {
            graph: &graph,
            root: Path::new(&args[1]).to_owned(),
            cache: Cache::default(),
            ranks: HashMap::new(),
            calls: 0,
        };
        let mut results = Vec::new();
        for (name, start, end) in cases {
            let now = Instant::now();
            let candidates = vec![
                index.candidates(&graph, &profile, start, policy)?,
                index.candidates(&graph, &profile, end, policy)?,
            ];
            let projection_ms = now.elapsed().as_secs_f64() * 1000.0;
            let now = Instant::now();
            let before = core.calls;
            let route = snap::route_via(&graph, &profile, &candidates, |a, b| core.query(a, b))?;
            let route_ms = now.elapsed().as_secs_f64() * 1000.0;
            let distance = route.as_ref().map(|r| {
                r.slices.iter().map(|s| graph.roads[s.road as usize].length_m as f64 * (s.to - s.from)).sum::<f64>()
            });
            eprintln!(
                "{name}: distance {distance:?} m, cost {:?}, {} CH calls, {route_ms:.1} ms",
                route.as_ref().map(|r| r.cost),
                core.calls - before
            );
            results.push(json!({"name":name,"coordinates":[start,end],"candidate_sets":candidates,"projection_ms":projection_ms,
                "attachment_and_core_elapsed_ms":route_ms,"ch_queries":core.calls-before,"distance_m":distance,"route":route}));
        }
        let output = json!({"profile":"touring","source_graph_load_ms":load_ms,"window_index_build_ms":index_ms,
            "decoded_summary_cache_bytes":core.cache.decoded_bytes,"cases":results,
            "limitations":["Source graph is resident. The window index and attachment incidence scan are prototype host operations.",
                "Summary cache is shared across cases. These are not cold-cache latency measurements.",
                "Each road contributes its nearest polyline projection. Only the declared radius, nearest-distance ambiguity band and candidate cap are searched.",
                "Route cost omits any off-road connector from requested coordinate to attachment. Snap distance and projected point are reported.",
                "Optimality is conditional on retained oriented attachments, regional coverage, fixed profile and exact CH callback.",
                "Integer prefix costs partition full prepared road costs; raw sample climb determines where climbing cost occurs."]});
        fs::write(&args[3], serde_json::to_vec_pretty(&output).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
