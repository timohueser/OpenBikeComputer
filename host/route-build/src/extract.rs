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
    let mut dictionary = Dictionary::default();
    for (name, metric) in &source.metrics {
        eprintln!("Preparing profile {name}");
        let mut endpoints = Vec::with_capacity(ids.len());
        for &id in &ids {
            endpoints.push(input.prepared_endpoint(name, id).map_err(|e| e.to_string())?);
        }
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
    }
    manifest.costs = route_engine::table::Table::write(&dictionary.into_values(), &mut write)?;
    Ok(manifest)
}

fn intersects(road: &Road, bounds: [f64; 4]) -> bool {
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
