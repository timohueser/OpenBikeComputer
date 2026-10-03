mod base;
pub mod blocks;
pub mod cost;
pub mod extract;
pub mod landmarks;
pub mod layout;
#[cfg(feature = "obc-terrain")]
pub mod obc_terrain;
pub mod osm;
mod road_bike;
pub mod terrain;

use route_engine::{
    closures::Closures,
    endpoints::Dictionary,
    model::{Graph, Profile},
    package::{self, Manifest, CELL, ROADS_PER_PAGE},
    storage,
    table::Table,
};
use std::collections::{BTreeMap, BTreeSet};

/// Writes a closed regional package with all legal transitions and complete road geometry.
/// The caller publishes the manifest only after all objects have been written.
pub fn prepare(
    graph: &Graph,
    region: String,
    bounds: [f64; 4],
    profiles: &[Profile],
    source_sha256: Vec<String>,
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Manifest, String> {
    if graph.roads.iter().any(|road| road.from as usize >= graph.points.len() || road.to as usize >= graph.points.len())
    {
        return Err("Invalid road endpoints".into());
    }
    for restrictions in [&graph.forbidden, &graph.forbidden_foot] {
        if !restrictions.is_sorted() {
            return Err("Unsorted turn restrictions".into());
        }
        for &(before, after) in restrictions {
            let before = graph.roads.get(before as usize).ok_or("Unknown restriction road")?;
            let after = graph.roads.get(after as usize).ok_or("Unknown restriction road")?;
            if before.to != after.from {
                return Err("Restriction roads do not meet".into());
            }
        }
    }
    let (mut manifest, edges) = prepare_base(graph, region, bounds, source_sha256, &mut write)?;
    let mut dictionary = Dictionary::default();
    let junctions = landmarks::Junctions::new(graph.roads.iter().map(|r| (r.from, r.to)))?;
    let mut landmarks = junctions.index(&mut write)?;
    for profile in profiles {
        eprintln!("Preparing profile {}", profile.name);
        if manifest.metrics.contains_key(&profile.name) {
            return Err("Duplicate metric identity".into());
        }
        let costing = cost::Costing::new(graph, profile)?;
        let road_costs: Vec<_> =
            costing.roads.iter().map(|cost| cost.as_ref().map_or(u64::MAX, |cost| cost.total())).collect();
        landmarks.profiles.insert(profile.name.clone(), junctions.prepare(&road_costs, &mut write)?);
        let turns: Vec<_> =
            edges.iter().map(|&(before, after)| costing.transition(before, after).unwrap_or(u64::MAX)).collect();
        manifest.metrics.insert(
            profile.name.clone(),
            base::metric(
                profile,
                (0..graph.roads.len()).map(|road| costing.basis(road)),
                &road_costs,
                &turns,
                &mut dictionary,
                &mut write,
            )?,
        );
    }
    manifest.costs = Table::write(&dictionary.into_values(), &mut write)?;
    manifest.landmarks = Some(landmarks);
    let closures = Closures::build(graph.roads.iter().enumerate().map(|(id, road)| {
        let tags = graph.osm.ways.get(&road.way).map(|way| &way.tags);
        (
            id as u32,
            tags.map_or_else(Vec::new, |tags| {
                route_engine::osm::closures(tags.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            }),
        )
    }))?;
    manifest.closures = write_closures(&closures, &mut write)?;
    Ok(manifest)
}

/// Writes the table only when a road has a possible closure.
fn write_closures(
    closures: &Closures,
    write: &mut impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Option<String>, String> {
    if closures.roads.is_empty() {
        return Ok(None);
    }
    write(&storage::encode(closures)?).map(Some)
}

fn prepare_base(
    graph: &Graph,
    region: String,
    bounds: [f64; 4],
    source_sha256: Vec<String>,
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<(Manifest, Vec<(u32, u32)>), String> {
    let edges = base::edges(&graph.roads)?;
    let roads = u32::try_from(graph.roads.len()).map_err(|_| "Too many roads")?;
    let mut manifest = Manifest {
        landmarks: None,
        closures: None,
        format: package::FORMAT,
        region,
        bounds,
        source_sha256,
        attribution: "© OpenStreetMap contributors; ODbL 1.0".into(),
        warnings: graph.warnings.clone(),
        roads,
        graph: route_engine::base::write_topology(roads, &edges, &mut write)?,
        geometry: Table::default(),
        osm: package::OsmPages::default(),
        spatial: BTreeMap::new(),
        metrics: BTreeMap::new(),
        costs: Table::default(),
    };
    macro_rules! source_pages {
        ($field:ident) => {
            let keys = graph
                .osm
                .$field
                .values()
                .collect::<Vec<_>>()
                .chunks(128)
                .map(|page| write(&storage::encode(&page)?))
                .collect::<Result<Vec<_>, String>>()?;
            manifest.osm.$field = Table::write(&keys, &mut write)?;
        };
    }
    source_pages!(nodes);
    source_pages!(ways);
    source_pages!(relations);
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
    let geometry = graph
        .roads
        .chunks(ROADS_PER_PAGE as usize)
        .map(|roads| write(&route_engine::geometry::encode(roads)?))
        .collect::<Result<Vec<_>, String>>()?;
    manifest.geometry = Table::write(&geometry, &mut write)?;
    let mut groups = BTreeMap::<String, BTreeMap<String, String>>::new();
    for (cell, roads) in cells {
        let group = package::cell_key((cell.0.div_euclid(100), cell.1.div_euclid(100)));
        groups
            .entry(group)
            .or_default()
            .insert(package::cell_key(cell), write(&storage::encode(&roads.into_iter().collect::<Vec<_>>())?)?);
    }
    for (group, cells) in groups {
        manifest.spatial.insert(group, write(&storage::encode(&cells)?)?);
    }
    Ok((manifest, edges))
}
