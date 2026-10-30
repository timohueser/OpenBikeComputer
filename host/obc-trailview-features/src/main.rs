//! `trailview-features --osm OSM.pbf --terrain TERRAIN.mbtiles --bounds W,S,E,N --output OUT.pmtiles
//! [--sea water-polygons-split-3857.zip]`: the archive of a box without `obc data`, with the kind table
//! and the road classes of the checkout and the terrain of a `planner-dem` MBTiles. Prints the metrics.

use std::path::{Path, PathBuf};

use obc_trailview_features::bake::{bake, Options};
use obc_trailview_features::{kinds, step, terrain};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| -> Result<&str, String> {
        let at = args.iter().position(|a| a == name).ok_or(format!("{name} is missing"))?;
        args.get(at + 1).map(String::as_str).ok_or(format!("{name} takes a value"))
    };
    let usage = "usage: trailview-features --osm OSM.pbf --terrain TERRAIN.mbtiles --bounds W,S,E,N --output OUT.pmtiles [--sea ZIP]";
    let (osm, terrain, bounds, output) = match (arg("--osm"), arg("--terrain"), arg("--bounds"), arg("--output")) {
        (Ok(o), Ok(t), Ok(b), Ok(out)) => (o, t, b, PathBuf::from(out)),
        _ => return Err(usage.into()),
    };
    let bounds: Vec<f64> =
        bounds.split(',').map(|v| v.parse().map_err(|_| usage.to_string())).collect::<Result<_, _>>()?;
    let table = |text: &str| serde_json::from_str::<serde_json::Value>(text).map_err(|e| e.to_string());
    let (class_names, classes) =
        kinds::classes(&kinds::solver_rows(&table(include_str!("../../../data/trailview/roads.json"))?))?;
    let options = Options {
        kinds: kinds::validate(&table(include_str!("../../../data/trailview/kinds.json"))?)?,
        bounds: bounds.try_into().map_err(|_| usage.to_string())?,
        attribution: "© OpenStreetMap contributors".into(),
        classes,
        class_names,
        sea: args.iter().any(|a| a == "--sea").then(|| arg("--sea").map(PathBuf::from)).transpose()?,
    };
    let terrain = terrain::Tiles::mbtiles(Path::new(terrain))?;
    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let work = tempfile::Builder::new().prefix(".trailview-osm-").tempdir_in(parent).map_err(|e| e.to_string())?;
    let osm_link = work.path().join("input.osm.pbf");
    let osm = std::fs::canonicalize(osm).map_err(|e| format!("{osm}: {e}"))?;
    std::os::unix::fs::symlink(&osm, &osm_link).map_err(|e| e.to_string())?;
    let metrics = bake(&options, &osm_link, &terrain, &output, work.path(), step::free_memory(None))?;
    println!("{metrics:#}");
    Ok(())
}
