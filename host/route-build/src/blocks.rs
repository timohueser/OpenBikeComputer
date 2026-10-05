//! Publication selects original pages. It does not change cost or geometry encodings.
use route_engine::{
    blocks::Manifest,
    directory::{Directory, Writer},
    model::Road,
    package::{digest, Package, Source},
    table::{self, Table},
    Error,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;
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
pub fn roads(input: &Package<impl Source>, bounds: [f64; 4]) -> Result<Vec<u32>> {
    let mut candidates = BTreeSet::new();
    let cell = |v: f64| ((v * 1e6).floor() as i32).div_euclid(route_engine::package::CELL);
    for lat in cell(bounds[1])..=cell(bounds[3]) {
        for lon in cell(bounds[0])..=cell(bounds[2]) {
            candidates.extend(input.spatial_roads((lat, lon))?);
        }
    }
    let mut selected = Vec::new();
    for id in candidates {
        if intersects(&input.road(id)?, bounds) {
            selected.push(id);
        }
    }
    Ok(selected)
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
    let geometry: BTreeSet<_> = roads.iter().map(|id| id / route_engine::package::ROADS_PER_PAGE).collect();
    retain(&mut data.geometry, &pages(geometry.iter().copied()), &mut objects)?;
    for page in geometry {
        objects.insert(input.key(&data.geometry, page)?);
    }
    objects.extend(data.costs.blocks.iter().cloned());
    if let Some(index) = &mut data.closures {
        objects.insert(index.sets.clone());
        retain(&mut index.roads.values, &road_pages, &mut objects)?;
    }
    let allowed = pages(roads.iter().map(|id| id / 64));
    for metric in data.metrics.values_mut() {
        retain(&mut metric.allowed, &allowed, &mut objects)?;
        retain(&mut metric.costs, &road_pages, &mut objects)?;
        retain(&mut metric.weights.road_costs.values, &road_pages, &mut objects)?;
        retain(&mut metric.weights.turns.values, &arc_pages, &mut objects)?;
    }
    if let Some(index) = &mut data.landmarks {
        let junctions: Vec<u32> =
            route_engine::landmarks::read(input, &index.mapping, roads.iter().copied(), index.junctions - 1)?;
        let junction_pages = pages(junctions.into_iter());
        retain(&mut index.mapping, &road_pages, &mut objects)?;
        for column in index.profiles.values_mut().flat_map(|columns| &mut columns.tables) {
            retain(column, &junction_pages, &mut objects)?;
        }
    }
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

fn pack(source: &Directory, root: &Path, keys: &[String]) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let id = digest(keys.join("").as_bytes());
    let directory = root.join("packs").join(&id);
    std::fs::create_dir_all(&directory)?;
    let mut writer = Writer::create(&directory)?;
    for key in keys {
        writer.write(&source.read(key)?)?;
    }
    writer.finish()?;
    Ok(id)
}

/// Write the routing blocks of the grid `cells` (an id and its bounds, inside the coverage of
/// the package `source`) to `root`: shared page packs, a manifest per cell, `blocks.json` and
/// `catalog.json`.
pub fn publish(
    source: &Path,
    cells: &[(String, [f64; 4])],
    root: &Path,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    if cells.is_empty() {
        return Err("The grid has no cells".into());
    }
    let input = Directory::open(source)?;
    let bounds = input.manifest().bounds;
    let full_roads: Vec<_> = (0..input.manifest().roads).collect();
    let mut full = prepare(&input, bounds, &full_roads)?;
    drop(full_roads);
    let mut selections: Vec<(String, Selection, [f64; 4])> = Vec::new();
    for (id, cell) in cells {
        if id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            || selections.iter().any(|(old, _, _)| old == id)
        {
            return Err("Invalid or duplicate grid cell identity".into());
        }
        let cell = *cell;
        if cell.iter().any(|v| !v.is_finite())
            || cell[0] >= cell[2]
            || cell[1] >= cell[3]
            || cell[0] < bounds[0]
            || cell[1] < bounds[1]
            || cell[2] > bounds[2]
            || cell[3] > bounds[3]
        {
            return Err("Grid cell is outside source coverage".into());
        }
        eprintln!("Selecting routing cell {}", id);
        let roads = roads(&input, cell)?;
        if !roads.is_empty() {
            let mut geometry = cell;
            for &id in &roads {
                for point in input.road(id)?.shape {
                    let lon = point.lon as f64 * 1e-6;
                    let lat = point.lat as f64 * 1e-6;
                    geometry[0] = geometry[0].min(lon);
                    geometry[1] = geometry[1].min(lat);
                    geometry[2] = geometry[2].max(lon);
                    geometry[3] = geometry[3].max(lat);
                }
            }
            selections.push((id.clone(), prepare(&input, cell, &roads)?, geometry));
        }
    }
    // Each page has one owner pack, shared by exactly the cells that need it.
    let mut consumers: BTreeMap<String, Vec<usize>> =
        full.objects.iter().map(|key| (key.clone(), Vec::new())).collect();
    for (cell, (_, selection, _)) in selections.iter().enumerate() {
        for key in &selection.objects {
            consumers.entry(key.clone()).or_default().push(cell);
        }
    }
    let mut groups = BTreeMap::<Vec<usize>, Vec<String>>::new();
    for (key, cells) in consumers {
        groups.entry(cells).or_default().push(key);
    }
    let source = Directory::source(source)?;
    let mut catalog = Vec::new();
    for (cells, mut keys) in groups {
        source.order_for_verify(&mut keys)?;
        let mut pending = Vec::new();
        let mut bytes = 0;
        for key in keys {
            let length = source.read(&key)?.len();
            if !pending.is_empty() && bytes + length > 16 * 1024 * 1024 {
                let id = pack(&source, root, &pending)?;
                for &cell in &cells {
                    selections[cell].1.manifest.archives.push(id.clone());
                }
                full.manifest.archives.push(id);
                pending.clear();
                bytes = 0;
            }
            pending.push(key);
            bytes += length;
        }
        if !pending.is_empty() {
            let id = pack(&source, root, &pending)?;
            for &cell in &cells {
                selections[cell].1.manifest.archives.push(id.clone());
            }
            full.manifest.archives.push(id);
        }
    }
    std::fs::create_dir_all(root.join("cells"))?;
    for (id, mut selection, geometry) in selections {
        selection.manifest.archives.sort();
        let filename = format!("cells/{id}.json");
        std::fs::write(root.join(&filename), serde_json::to_vec(&selection.manifest)?)?;
        catalog.push(serde_json::json!({"id":id, "bounds":selection.manifest.data.bounds,
            "manifest":filename, "adjacency":selection.adjacency, "geometry_bounds":geometry}));
    }
    full.manifest.archives.sort();
    std::fs::write(root.join("blocks.json"), serde_json::to_vec(&full.manifest)?)?;
    std::fs::write(
        root.join("catalog.json"),
        serde_json::to_vec(&crate::sort_keys(serde_json::json!({"format":2,
        "source":input.identity(), "cells":catalog})))?,
    )?;
    eprintln!("Published {} packs in {:.2}s", full.manifest.archives.len(), started.elapsed().as_secs_f64());
    Ok(())
}
