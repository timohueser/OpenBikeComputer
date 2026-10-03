//! Extract an exact whole-road subgraph from immutable prepared arrival states.
use crate::{base, prepare_base};
use route_engine::{
    endpoints::Dictionary,
    model::{Graph, Road},
    package::{Manifest, Package, Source, CELL},
};
use std::collections::BTreeSet;

/// Bounds limit query endpoints. Roads that intersect them are retained in full.
/// Optional source OSM pages stay complete, including tags on excluded roads.
pub fn prepare<S: Source>(
    input: &mut Package<S>,
    region: String,
    bounds: [f64; 4],
    include_sources: bool,
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Manifest, String> {
    let started = std::time::Instant::now();
    eprintln!("Selecting roads");
    let source = input.manifest().clone();
    if bounds.iter().any(|v| !v.is_finite())
        || bounds[0] >= bounds[2]
        || bounds[1] >= bounds[3]
        || bounds[0] < source.bounds[0]
        || bounds[1] < source.bounds[1]
        || bounds[2] > source.bounds[2]
        || bounds[3] > source.bounds[3]
    {
        return Err("Extraction bounds must be nonempty and inside the source coverage".into());
    }
    let cell = |value: f64| {
        // Euclidean division also keeps western and southern cells aligned.
        ((value * 1e6).floor() as i32).div_euclid(CELL)
    };
    let mut candidates = BTreeSet::new();
    for lat in cell(bounds[1])..=cell(bounds[3]) {
        for lon in cell(bounds[0])..=cell(bounds[2]) {
            candidates.extend(input.spatial_roads((lat, lon)).map_err(|e| e.to_string())?);
        }
    }
    let mut ids = Vec::new();
    let mut graph = Graph { warnings: source.warnings.clone(), ..Graph::default() };
    for id in candidates {
        let road = input.road(id).map_err(|e| e.to_string())?;
        if intersects(&road, bounds) {
            ids.push(id);
            graph.roads.push(road);
        }
    }
    if ids.is_empty() {
        return Err("No roads intersect the requested bounds".into());
    }
    ids = crate::layout::spatial_order(&mut graph)?.into_iter().map(|old| ids[old as usize]).collect();
    let (mut manifest, edges) = prepare_base(&graph, region, bounds, source.source_sha256, &mut write)?;
    manifest.attribution = source.attribution;
    if include_sources {
        manifest.osm = source.osm;
        for table in manifest.osm.tables() {
            for key in table.blocks.iter().cloned().chain(input.keys(table).map_err(|e| e.to_string())?) {
                write(&input.bytes(&key).map_err(|e| e.to_string())?)?;
            }
        }
    }
    eprintln!("Prepared road graph in {:.1}s", started.elapsed().as_secs_f64());
    let mut dictionary = Dictionary::default();
    let inherited = route_engine::landmarks::project(input, &ids, &mut write)?;
    let junctions = if inherited.is_none() {
        Some(crate::landmarks::Junctions::new(graph.roads.iter().map(|r| (r.from, r.to)))?)
    } else {
        None
    };
    let mut landmarks = match inherited {
        Some(index) => index,
        None => junctions.as_ref().unwrap().index(&mut write)?,
    };
    for (name, metric) in &source.metrics {
        let profile_started = std::time::Instant::now();
        eprintln!("Preparing profile {name}");
        let endpoints = input.prepared_endpoints(name, &ids).map_err(|e| e.to_string())?;
        let mut road_costs = vec![u64::MAX; endpoints.len()];
        for (id, endpoint) in endpoints.iter().enumerate() {
            if let Some(parameters) = endpoint.cost {
                road_costs[id] = parameters.compile(&graph.roads[id], &metric.profile)?.total();
            }
        }
        let turns: Vec<_> = edges
            .iter()
            .map(|&(before, after)| {
                endpoints[after as usize]
                    .departures
                    .iter()
                    .find(|departure| departure.state == ids[before as usize])
                    .map_or(u64::MAX, |departure| departure.penalty)
            })
            .collect();
        if let Some(junctions) = &junctions {
            landmarks.profiles.insert(name.clone(), junctions.prepare(&road_costs, &mut write)?);
        }
        manifest.metrics.insert(
            name.clone(),
            base::metric(
                &metric.profile,
                endpoints.iter().map(|endpoint| endpoint.cost),
                &road_costs,
                &turns,
                &mut dictionary,
                &mut write,
            )?,
        );
        eprintln!("Prepared profile {name} in {:.1}s", profile_started.elapsed().as_secs_f64());
    }
    manifest.costs = route_engine::table::Table::write(&dictionary.into_values(), &mut write)?;
    manifest.landmarks = Some(landmarks);
    let closures = input.closures().map_err(|e| e.to_string())?.select(ids.iter().copied());
    manifest.closures = crate::write_closures(&closures, &mut write)?;
    Ok(manifest)
}

pub(crate) fn intersects(road: &Road, bounds: [f64; 4]) -> bool {
    road.shape.windows(2).any(|pair| {
        let a = [pair[0].lon as f64 * 1e-6, pair[0].lat as f64 * 1e-6];
        let b = [pair[1].lon as f64 * 1e-6, pair[1].lat as f64 * 1e-6];
        let (mut start, mut end) = (0.0f64, 1.0f64);
        for axis in 0..2 {
            let delta = b[axis] - a[axis];
            if delta == 0.0 {
                if a[axis] < bounds[axis] || a[axis] > bounds[axis + 2] {
                    return false;
                }
            } else {
                let low = (bounds[axis] - a[axis]) / delta;
                let high = (bounds[axis + 2] - a[axis]) / delta;
                start = start.max(low.min(high));
                end = end.min(low.max(high));
            }
        }
        start <= end
    })
}
