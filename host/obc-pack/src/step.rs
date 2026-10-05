//! The map-cell step of `obc data`: the cells of one band in one source leaf. Each cell has the
//! bytes that [`crate::cut::cut`] writes when it cuts the whole leaf, as the planet bake does.

use std::collections::BTreeMap;
use std::path::PathBuf;

use obc_data::engine::{view, Request};
use serde_json::Value;

use crate::config::{Config, ContourClass};
use crate::cut::{artifact_path, cut, CutOptions};
use crate::grid::{Band, BandTable, CellId};
use crate::progress::{CancelToken, Progress};

/// The schema that every cell is cut with.
const SCHEMA: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json"));
/// The source of the land polygons, which every cell reads.
pub const LAND: &str = "land-polygons";

/// Whether the cells of `band` read terrain: for the contours of its levels, or for the ascent of
/// its nav graph and the heights of its summits.
pub fn reads_terrain(band: &Band) -> Result<bool, String> {
    let config = Config::parse(SCHEMA)?;
    let classes = [ContourClass::Major, ContourClass::Index];
    let contours = classes.into_iter().filter_map(|class| config.contour_style(class)).map(|style| style.min_lod).min();
    let contours = config.contours.enabled && contours.is_some_and(|min| band.lods.iter().any(|&lod| lod >= min));
    Ok(contours || band.has_nav() || band.has_poi())
}

/// The options name the `band` of the recommended table, the source `leaf` as `[log2, i, j]`, and
/// the `cells` of the band to write, as `[i, j]`. The step reads the one `.osm.pbf` and every
/// `.obcd` of its layers, and the zip of `land-polygons`. The layer is
/// `cells/<band>/<i>/<j>.obcm` for each cell with content, and `cells/<band>/empty.json`: the ids
/// of the other cells.
pub fn cells(request: &Request) -> Result<(), String> {
    let options = &request.options;
    let band = options["band"].as_str().ok_or("option `band` is not a string")?;
    let bands = BandTable::recommended();
    let log2 = bands.band(band).ok_or(format!("no band `{band}`"))?.cell_log2;
    let numbers = |value: &Value| value.as_array()?.iter().map(Value::as_i64).collect::<Option<Vec<i64>>>();
    let leaf = match numbers(&options["leaf"]).as_deref() {
        Some(&[log2, i, j]) => u32::try_from(log2).ok().and_then(|log2| CellId::new(log2, i, j).ok()),
        _ => None,
    };
    let leaf = leaf.ok_or("option `leaf` is not [log2, i, j]")?;
    let cells = options["cells"].as_array().ok_or("option `cells` is not a list")?.iter().map(|cell| {
        match numbers(cell).as_deref() {
            Some(&[i, j]) => CellId::new(log2, i, j).ok(),
            _ => None,
        }
    });
    let cells = cells.collect::<Option<Vec<_>>>().ok_or("a cell is not [i, j] of the band")?;

    let files: BTreeMap<&String, &PathBuf> = request.layers.values().flatten().collect();
    let pbfs: Vec<String> = files
        .iter()
        .filter(|(path, _)| path.ends_with(".osm.pbf"))
        .map(|(_, object)| object.to_string_lossy().into_owned())
        .collect();
    if pbfs.len() != 1 {
        return Err(format!("the step reads {} .osm.pbf files, not one", pbfs.len()));
    }
    let land = request.snapshots.get(LAND).map(|files| files.values().collect::<Vec<_>>());
    let Some([land]) = land.as_deref() else {
        return Err(format!("the step reads no single file of `{LAND}`"));
    };
    let terrain: BTreeMap<String, PathBuf> = files
        .into_iter()
        .filter(|(path, _)| path.ends_with(".obcd"))
        .map(|(path, object)| (path.clone(), object.clone()))
        .collect();
    let terrain = match terrain.is_empty() {
        // A leaf at sea has no terrain cell.
        true => None,
        false => {
            let dir = request.output.with_file_name("view");
            view(&terrain, &dir)?;
            Some(dir)
        }
    };

    let opts = CutOptions {
        bands,
        select: cells,
        only_bands: vec![band.to_string()],
        land: Some(land.to_path_buf()),
        terrain,
        source_extent: Some(leaf.square()),
        ..CutOptions::default()
    };
    let config = Config::parse(SCHEMA)?;
    let progress = Progress::new(CancelToken::new(), |_, line| eprintln!("{line}"));
    let tree = request.output.with_file_name("cut");
    let summary = cut(&pbfs, &config, &tree, &opts, &progress).map_err(|e| e.to_string())?;

    let dir = request.output.join("cells").join(band);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut empty = Vec::new();
    for artifact in &summary.cells {
        if artifact.empty {
            empty.push(artifact.id.to_string());
            continue;
        }
        let path = request.output.join(&artifact.path);
        std::fs::create_dir_all(path.parent().expect("a cell path has a parent"))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::rename(artifact_path(&tree, artifact), &path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let path = dir.join("empty.json");
    std::fs::write(&path, serde_json::to_string(&empty).expect("strings serialize"))
        .map_err(|e| format!("{}: {e}", path.display()))
}
