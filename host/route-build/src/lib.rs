mod compact;
mod hierarchy;
#[cfg(feature = "obc-terrain")]
pub mod obc_terrain;
pub mod osm;
pub mod terrain;

use route_engine::{
    model::{Graph, Profile},
    package::{self, Manifest, Metric, CELL, ROADS_PER_PAGE},
    storage,
};
use std::collections::{BTreeMap, BTreeSet};

/// Writes a closed regional package. Every shortcut witness and geometry page is included.
/// The caller publishes the manifest only after all objects have been written.
pub fn prepare(
    graph: &Graph,
    region: String,
    bounds: [f64; 4],
    profiles: &[Profile],
    source_sha256: Vec<String>,
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Manifest, String> {
    let mut manifest = Manifest {
        format: package::FORMAT,
        region,
        bounds,
        source_sha256,
        attribution: "© OpenStreetMap contributors; ODbL 1.0".into(),
        warnings: graph.warnings.clone(),
        roads: u32::try_from(graph.roads.len()).map_err(|_| "Too many roads")?,
        geometry: Vec::new(),
        spatial: BTreeMap::new(),
        metrics: BTreeMap::new(),
    };
    let mut cells = BTreeMap::<(i32, i32), BTreeSet<u32>>::new();
    for (id, road) in graph.roads.iter().enumerate() {
        if road.shape.len() < 2 || road.class > 6 {
            return Err("Road lacks valid geometry or class".into());
        }
        for pair in road.shape.windows(2) {
            let (a, b) = (package::cell(pair[0]), package::cell(pair[1]));
            if (a.0.abs_diff(b.0) as u64 + 1) * (a.1.abs_diff(b.1) as u64 + 1) > 4096 {
                return Err(format!("Road segment spans more than 4096 spatial cells of {} microdegrees", CELL));
            }
            for lat in a.0.min(b.0)..=a.0.max(b.0) {
                for lon in a.1.min(b.1)..=a.1.max(b.1) {
                    cells.entry((lat, lon)).or_default().insert(id as u32);
                }
            }
        }
    }
    for roads in graph.roads.chunks(ROADS_PER_PAGE as usize) {
        manifest.geometry.push(write(&storage::encode(&roads)?)?);
    }
    for (cell, roads) in cells {
        manifest
            .spatial
            .insert(package::cell_key(cell), write(&storage::encode(&roads.into_iter().collect::<Vec<_>>())?)?);
    }
    for profile in profiles {
        if manifest.metrics.contains_key(&profile.name) {
            return Err("Duplicate metric identity".into());
        }
        let mut graph = compact::Compact::new(graph, profile)?;
        let (pages, ranks) = hierarchy::export(&graph, &mut write)?;
        for endpoint in &mut graph.endpoints {
            if endpoint.cost.is_some() {
                endpoint.arrival = ranks[endpoint.arrival as usize];
                for state in &mut endpoint.departures {
                    *state = ranks[*state as usize];
                }
            }
        }
        let mut endpoints = Vec::new();
        for page in graph.endpoints.chunks(ROADS_PER_PAGE as usize) {
            endpoints.push(write(&storage::encode(&page)?)?);
        }
        manifest.metrics.insert(
            profile.name.clone(),
            Metric { profile: profile.clone(), graph: pages, endpoints, states: ranks.len() as u32 },
        );
    }
    Ok(manifest)
}
