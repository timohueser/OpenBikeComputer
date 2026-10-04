use clap::Parser;
use route_build::blocks::{self, Selection};
use route_engine::{
    directory::{Directory, Writer},
    package::{digest, Source},
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Parser)]
#[command(about = "Publish a shared routing page pool for a grid of download cells")]
struct Args {
    source: PathBuf,
    output: PathBuf,
    /// JSON array of {id,bounds} cells in the source coverage.
    #[arg(long)]
    cells: PathBuf,
}

fn pack(source: &Directory, root: &Path, keys: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    let id = digest(keys.join("").as_bytes());
    let directory = root.join("packs").join(&id);
    fs::create_dir_all(&directory)?;
    let mut writer = Writer::create(&directory)?;
    for key in keys {
        writer.write(&source.read(key)?)?;
    }
    writer.finish()?;
    Ok(id)
}

fn publish(args: &Args, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let definitions: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(&args.cells)?)?;
    if definitions.is_empty() {
        return Err("The grid has no cells".into());
    }
    let input = Directory::open(&args.source)?;
    let bounds = input.manifest().bounds;
    let full_roads: Vec<_> = (0..input.manifest().roads).collect();
    let mut full = blocks::prepare(&input, bounds, &full_roads)?;
    drop(full_roads);
    let mut selections: Vec<(String, Selection, [f64; 4])> = Vec::new();
    for definition in definitions {
        let id = definition["id"].as_str().ok_or("Missing grid cell identity")?;
        if id.is_empty()
            || !id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            || selections.iter().any(|(old, _, _)| old == id)
        {
            return Err("Invalid or duplicate grid cell identity".into());
        }
        let cell: [f64; 4] = serde_json::from_value(definition["bounds"].clone())?;
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
        let roads = blocks::roads(&input, cell)?;
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
            selections.push((id.into(), blocks::prepare(&input, cell, &roads)?, geometry));
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
    let source = Directory::source(&args.source)?;
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
    fs::create_dir_all(root.join("cells"))?;
    for (id, mut selection, geometry) in selections {
        selection.manifest.archives.sort();
        let filename = format!("cells/{id}.json");
        fs::write(root.join(&filename), serde_json::to_vec(&selection.manifest)?)?;
        catalog.push(serde_json::json!({"id":id, "bounds":selection.manifest.data.bounds,
            "manifest":filename, "adjacency":selection.adjacency, "geometry_bounds":geometry}));
    }
    full.manifest.archives.sort();
    fs::write(root.join("blocks.json"), serde_json::to_vec(&full.manifest)?)?;
    fs::write(
        root.join("catalog.json"),
        serde_json::to_vec(&serde_json::json!({"format":2,
        "source":input.identity(), "cells":catalog}))?,
    )?;
    eprintln!("Published {} packs in {:.2}s", full.manifest.archives.len(), started.elapsed().as_secs_f64());
    Ok(())
}
fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.output.exists() {
        return Err("The routing publication already exists".into());
    }
    let parent = args.output.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".routing-publication-{}", std::process::id()));
    fs::create_dir(&temporary)?;
    let result = publish(&args, &temporary).and_then(|_| Ok(fs::rename(&temporary, &args.output)?));
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}
fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
