//! Publication selects original pages. It does not change cost or geometry encodings.
use route_engine::{
    blocks::Manifest,
    package::{Package, Source},
    table::{self, Table},
    Error,
};
use std::collections::{BTreeMap, BTreeSet};
type Result<T> = std::result::Result<T, Error>;

pub struct Selection {
    pub manifest: Manifest,
    pub objects: BTreeSet<String>,
    /// Union these ranges to count outgoing arcs when cells share boundary roads.
    pub adjacency: Vec<[u32; 2]>,
}

pub fn ranges(ids: impl IntoIterator<Item = u32>) -> Vec<[u32; 2]> {
    let mut result: Vec<[u32; 2]> = Vec::new();
    for id in ids {
        if let Some(last) = result.last_mut().filter(|r| r[1] == id) {
            last[1] += 1;
        } else {
            result.push([id, id + 1]);
        }
    }
    result
}

/// Select whole roads which intersect the coverage, including crossing segments with no vertex inside it.
pub fn roads(input: &mut Package<impl Source>, bounds: [f64; 4]) -> Result<Vec<u32>> {
    let mut candidates = BTreeSet::new();
    let cell = |v: f64| ((v * 1e6).floor() as i32).div_euclid(route_engine::package::CELL);
    for lat in cell(bounds[1])..=cell(bounds[3]) {
        for lon in cell(bounds[0])..=cell(bounds[2]) {
            candidates.extend(input.spatial_roads((lat, lon))?);
        }
    }
    let mut selected = Vec::new();
    for id in candidates {
        if crate::extract::intersects(&input.road(id)?, bounds) {
            selected.push(id);
        }
    }
    Ok(selected)
}

pub fn prepare(input: &Package<impl Source>, bounds: [f64; 4], roads: &[u32]) -> Result<Selection> {
    let original = input.manifest();
    if roads.is_empty() || roads.last().is_some_and(|&r| r >= original.roads) || !roads.windows(2).all(|p| p[0] < p[1])
    {
        return Err(Error::InvalidData("Invalid road selection".into()));
    }
    let mut data = original.clone();
    data.bounds = bounds;
    let mut objects = BTreeSet::new();
    let retain = |table: &mut Table, pages: &BTreeSet<usize>, objects: &mut BTreeSet<String>| -> Result<()> {
        table.retain(pages)?;
        objects.extend(table.blocks.iter().cloned());
        Ok(())
    };
    let road_pages = pages(roads.iter().copied());
    let row_pages = pages(roads.iter().flat_map(|&r| [r, r + 1]));
    let mut cache = table::Cache::<u32>::default();
    let mut adjacency: Vec<[u32; 2]> = Vec::new();
    let mut arc_pages = BTreeSet::new();
    let mut arcs = 0u32;
    for &road in roads {
        let a =
            cache.block(input, &original.graph.first, road as usize / table::ENTRIES)?[road as usize % table::ENTRIES];
        let z = cache.block(input, &original.graph.first, (road as usize + 1) / table::ENTRIES)?
            [(road as usize + 1) % table::ENTRIES];
        if a > z || z > original.graph.head.len {
            return Err(Error::InvalidData("Invalid source adjacency".into()));
        }
        arcs = arcs.checked_add(z - a).ok_or(Error::Limit)?;
        if a < z {
            arc_pages.extend(a as usize / table::ENTRIES..=(z - 1) as usize / table::ENTRIES);
            if let Some(last) = adjacency.last_mut().filter(|r| r[1] == a) {
                last[1] = z;
            } else {
                adjacency.push([a, z]);
            }
        }
    }
    retain(&mut data.graph.first, &row_pages, &mut objects)?;
    retain(&mut data.graph.head, &arc_pages, &mut objects)?;
    for table in [&mut data.graph.reverse_first, &mut data.graph.reverse_tail, &mut data.graph.reverse_offsets.values] {
        table.retain(&BTreeSet::new())?;
    }
    let geometry: BTreeSet<_> = roads.iter().map(|id| id / route_engine::package::ROADS_PER_PAGE).collect();
    retain(&mut data.geometry, &pages(geometry.iter().copied()), &mut objects)?;
    for page in geometry {
        objects.insert(input.key(&data.geometry, page)?);
    }
    objects.extend(data.costs.blocks.iter().cloned());
    let allowed = pages(roads.iter().map(|id| id / 64));
    for metric in data.metrics.values_mut() {
        retain(&mut metric.allowed, &allowed, &mut objects)?;
        retain(&mut metric.costs, &road_pages, &mut objects)?;
        retain(&mut metric.weights.road_costs.values, &road_pages, &mut objects)?;
        retain(&mut metric.weights.turns.values, &arc_pages, &mut objects)?;
    }
    if let Some(index) = &mut data.landmarks {
        let junctions: Vec<u32> =
            route_engine::landmarks::read_selected(input, &index.mapping, roads.iter().copied(), index.junctions - 1)?;
        let junction_pages = pages(junctions.into_iter());
        retain(&mut index.mapping, &road_pages, &mut objects)?;
        for column in index.profiles.values_mut().flatten() {
            retain(column, &junction_pages, &mut objects)?;
        }
    }
    data.osm = Default::default();
    data.spatial.clear();
    let snap = snap(input, bounds)?;
    objects.extend(snap.values().cloned());
    Ok(Selection {
        manifest: Manifest {
            format: 2,
            source: input.identity().into(),
            data,
            roads: ranges(roads.iter().copied()),
            arcs,
            snap,
            archives: Vec::new(),
        },
        objects,
        adjacency,
    })
}

fn pages(indices: impl Iterator<Item = u32>) -> BTreeSet<usize> {
    indices.map(|id| id as usize / table::ENTRIES).collect()
}
fn snap(input: &Package<impl Source>, bounds: [f64; 4]) -> Result<BTreeMap<String, String>> {
    // Match the maximum 5 km snap radius at the most poleward point of the coverage.
    let latitude = bounds[1].abs().max(bounds[3].abs());
    let dy = 5000.0 / 110_000.0 + 0.000002;
    let dx = dy / latitude.to_radians().cos();
    let halo = [bounds[0] - dx, bounds[1] - dy, bounds[2] + dx, bounds[3] + dy];
    let mut result = BTreeMap::new();
    for (group, key) in &input.manifest().spatial {
        if !intersects_cell(group, 1.0, halo)? {
            continue;
        }
        let cells: BTreeMap<String, String> = input.read(key)?;
        for (cell, key) in cells {
            if intersects_cell(&cell, 0.01, halo)? {
                result.insert(cell, key);
            }
        }
    }
    Ok(result)
}
fn intersects_cell(key: &str, size: f64, bounds: [f64; 4]) -> Result<bool> {
    let invalid = || Error::InvalidData("Invalid snap cell coordinate".into());
    let (lat, lon) = key.split_once(',').ok_or_else(invalid)?;
    let y: i32 = lat.parse().map_err(|_| invalid())?;
    let x: i32 = lon.parse().map_err(|_| invalid())?;
    Ok(x as f64 * size <= bounds[2]
        && (x as f64 + 1.0) * size >= bounds[0]
        && y as f64 * size <= bounds[3]
        && (y as f64 + 1.0) * size >= bounds[1])
}
