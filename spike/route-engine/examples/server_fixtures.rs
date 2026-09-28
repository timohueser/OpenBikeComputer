#[cfg(not(feature = "builder"))]
fn main() {
    eprintln!("Run this example with --features builder");
    std::process::exit(1);
}

#[cfg(feature = "builder")]
fn main() {
    if let Err(error) = fixtures::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(feature = "builder")]
mod fixtures {
    use serde::Serialize;
    use serde_json::Value;
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::fs;
    use std::path::Path;
    use trip_router::model::{Graph, Point, Profile};
    use trip_router::search::{Progress, Search};
    use trip_router::storage::{self, Cache, Seed, ROADS_PER_PAGE};

    const CACHE_BYTES: usize = 512 * 1024 * 1024;

    #[derive(Serialize)]
    struct Request {
        name: String,
        start: Seed,
        end: Seed,
        expect_cost: u64,
    }

    fn read(path: &Path) -> Result<Vec<u8>, String> {
        fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn field(value: &Value, key: &str) -> Result<u64, String> {
        value[key].as_u64().ok_or_else(|| format!("Missing integer {key} in baseline JSON"))
    }

    fn baseline_seed(value: &Value, side: &str) -> Result<Seed, String> {
        Ok(Seed {
            node: field(value, &format!("{side}_rank"))?.try_into().map_err(|_| "Rank exceeds u32")?,
            road: field(value, &format!("{side}_road"))?.try_into().map_err(|_| "Road ID exceeds u32")?,
            cost: 0,
        })
    }

    fn query(root: &Path, cache: &mut Cache, start: Seed, end: Seed) -> Result<(u64, Vec<u32>), String> {
        let mut search = Search::new(&[start], &[end], 1_000_000);
        loop {
            match search.poll(cache, 4096) {
                Progress::Working { .. } => {}
                Progress::NeedPages { pages } => {
                    for id in pages {
                        if cache.pages.contains_key(&id) {
                            return Err("Search requested an incomplete cached page".into());
                        }
                        cache.insert_page(id, &read(&root.join("ch").join(format!("{id}.bin")))?, CACHE_BYTES)?;
                    }
                }
                Progress::Done { cost, roads, .. } => return Ok((cost, roads)),
                other => return Err(format!("Fixture query did not finish: {other:?}")),
            }
        }
    }

    fn verify(
        graph: &Graph,
        profile: &Profile,
        start: Seed,
        end: Seed,
        cost: u64,
        roads: &[u32],
    ) -> Result<u64, String> {
        if roads.first() != Some(&start.road) || roads.last() != Some(&end.road) {
            return Err("Returned road states do not match fixture endpoints".into());
        }
        let mut summed = 0u64;
        let mut distance = 0u64;
        for pair in roads.windows(2) {
            let from = graph.roads.get(pair[0] as usize).ok_or("Unknown incoming road")?;
            let to = graph.roads.get(pair[1] as usize).ok_or("Unknown outgoing road")?;
            if from.to != to.from || !graph.permits_turn(pair[0], pair[1], profile.walking) {
                return Err("Returned route has a disconnected or forbidden transition".into());
            }
            let weight = profile.cost(to).ok_or("Returned route uses an excluded road")?;
            summed = summed.checked_add(weight).ok_or("Route cost overflow")?;
            distance = distance.checked_add(to.length_m as u64).ok_or("Route distance overflow")?;
        }
        if summed != cost {
            return Err(format!("Road cost {summed} differs from CH cost {cost}"));
        }
        Ok(distance)
    }

    fn seed(graph: &Graph, root: &Path, road: u32, ranks: &mut HashMap<u32, u32>) -> Result<Seed, String> {
        if !ranks.contains_key(&road) {
            let edge = graph.roads.get(road as usize).ok_or("Selected unknown road")?;
            let (lat, lon) = storage::snap_cell(graph.points[edge.to as usize]);
            let bytes = read(&root.join("ch/lookup").join(format!("{lat}_{lon}.bin")))?;
            let entries: Vec<(Point, u32, u32)> = storage::decode(&bytes)?;
            for (_, id, rank) in entries {
                ranks.insert(id, rank);
            }
        }
        Ok(Seed { node: *ranks.get(&road).ok_or("Road missing from its endpoint lookup page")?, road, cost: 0 })
    }

    fn geometry(graph: &Graph, root: &Path, roads: &[u32], written: &mut BTreeSet<u32>) -> Result<(), String> {
        for &id in roads.iter().skip(1) {
            let page = id / ROADS_PER_PAGE;
            if !written.insert(page) {
                continue;
            }
            let first = page as usize * ROADS_PER_PAGE as usize;
            let end = (first + ROADS_PER_PAGE as usize).min(graph.roads.len());
            let bytes = storage::encode(&(first as u32, &graph.roads[first..end]))?;
            let path = root.join("geometry").join(format!("{page}.bin"));
            fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }

    pub fn run() -> Result<(), String> {
        let args: Vec<_> = std::env::args().skip(1).collect();
        if args.len() != 4 {
            return Err(
                "Usage: server_fixtures GRAPH PREPARED_ROOT BASELINE_BENCHMARK_JSON OUTPUT_REQUESTS_JSON".into()
            );
        }
        let graph: Graph = postcard::from_bytes(&read(Path::new(&args[0]))?).map_err(|e| e.to_string())?;
        let baseline: Value = serde_json::from_slice(&read(Path::new(&args[2]))?).map_err(|e| e.to_string())?;
        if baseline["profile"] != "touring"
            || baseline["ch"]["status"] != "ok"
            || baseline["ch"]["oracle_cost_and_witness_verified"] != true
        {
            return Err("Expected a successful oracle-verified touring baseline".into());
        }
        let root = Path::new(&args[1]);
        let profile = Profile::presets().into_iter().find(|p| p.name == "touring").ok_or("Missing touring profile")?;
        let start = baseline_seed(&baseline["ch"]["seeds"], "start")?;
        let end = baseline_seed(&baseline["ch"]["seeds"], "end")?;
        let mut cache = Cache::default();
        let (cost, full) = query(root, &mut cache, start, end)?;
        if cost != field(&baseline["ch"]["query_with_local_disk_read_decode"], "cost")?
            || cost != field(&baseline["reference"], "cost")?
        {
            return Err("Prepared route differs from the verified baseline cost".into());
        }
        let total = verify(&graph, &profile, start, end, cost, &full)?;
        if full.len() < 2 || total == 0 {
            return Err("Baseline route is empty".into());
        }
        let mut distances = vec![0u64];
        let mut costs = vec![0u64];
        for &id in full.iter().skip(1) {
            let road = &graph.roads[id as usize];
            distances.push(distances.last().unwrap() + road.length_m as u64);
            costs.push(costs.last().unwrap() + profile.cost(road).ok_or("Excluded baseline road")?);
        }
        let mut selections = Vec::new();
        let mut selected = HashSet::new();
        for (name, percent, length) in [
            ("local-start", 2, 1_000),
            ("local-end", 86, 1_000),
            ("short-north", 12, 10_000),
            ("short-south", 58, 10_000),
            ("regional-north", 25, 50_000),
            ("regional-central", 42, 100_000),
            ("regional-south", 60, 200_000),
        ] {
            let from = distances.partition_point(|d| *d < total * percent / 100).min(full.len() - 2);
            let to = distances.partition_point(|d| *d < distances[from] + length).min(full.len() - 1).max(from + 1);
            if selected.insert((from, to)) {
                selections.push((name, from, to));
            }
        }
        selections.push(("full-route", 0, full.len() - 1));
        let mut ranks = HashMap::from([(start.road, start.node), (end.road, end.node)]);
        let mut requests = Vec::new();
        let mut written = BTreeSet::new();
        fs::create_dir_all(root.join("geometry")).map_err(|e| e.to_string())?;
        for (name, from, to) in selections {
            let start = seed(&graph, root, full[from], &mut ranks)?;
            let end = seed(&graph, root, full[to], &mut ranks)?;
            let (expect_cost, roads) = query(root, &mut cache, start, end)?;
            let distance = verify(&graph, &profile, start, end, expect_cost, &roads)?;
            if expect_cost != costs[to] - costs[from] {
                return Err(format!("{name}: CH cost differs from the optimal baseline subpath"));
            }
            geometry(&graph, root, &roads, &mut written)?;
            println!("touring {name}: {distance} m, cost {expect_cost}, {} traversed roads", roads.len() - 1);
            requests.push(Request { name: name.into(), start, end, expect_cost });
        }
        fs::write(&args[3], serde_json::to_vec_pretty(&requests).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        println!(
            "{} requests; {} geometry pages; {} decoded cache bytes",
            requests.len(),
            written.len(),
            cache.decoded_bytes
        );
        Ok(())
    }
}
