//! The terrain steps of `obc data`: the published terrain cells of one leaf, from the GLO-30 tiles
//! of its request and the reference archive of the leaf, which `obc-bake terrain --reference`
//! writes the same; and the terrain of the planner maps.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use obc_data::engine::Request;
use serde_json::Value;

use crate::bake::{published_cell, write_cell_file};
use crate::geotiff::{DemMosaic, DemTile};
use crate::reference::ReferenceArchive;

/// The source whose tiles the step reads.
pub const GLO30: &str = "copernicus-glo-30";

/// The options name the pairing, `posting_log2` and `cell_log2`, and the cells, as `[ci, cj]`.
/// The layer input, when the request has one, is the reference archive of the leaf below
/// `reference/`. The layer is `terrain/<i>/<j>.obcd` for each cell with a height,
/// `metadata/empty.json`: the ids of the cells without one, so a leaf at sea is a layer too, and,
/// when a cell reads a national model, `metadata/credits.json`: the credit of each such model.
pub fn terrain(request: &Request) -> Result<(), String> {
    let options = &request.options;
    let log2 = |name: &str| {
        options[name]
            .as_u64()
            .and_then(|value| u8::try_from(value).ok())
            .ok_or(format!("option `{name}` is not a log2"))
    };
    let (posting_log2, cell_log2) = (log2("posting_log2")?, log2("cell_log2")?);
    let index = |value: &Value| value.as_u64().and_then(|value| u32::try_from(value).ok());
    let cells = options["cells"].as_array().ok_or("option `cells` is not a list")?.iter().map(|cell| match cell {
        Value::Array(pair) if pair.len() == 2 => Some((index(&pair[0])?, index(&pair[1])?)),
        _ => None,
    });
    let cells = cells.collect::<Option<Vec<_>>>().ok_or("a cell is not [ci, cj]")?;

    // A leaf at sea reads no tile.
    let mut mosaic = DemMosaic::default();
    for tile in request.snapshots.get(GLO30).into_iter().flat_map(|tiles| tiles.values()) {
        mosaic.push(DemTile::open(tile)?);
    }
    let reference = match request.layers.values().collect::<Vec<_>>()[..] {
        [] => None,
        [files] => {
            let view = request.output.with_file_name("view");
            obc_data::engine::view(files, &view)?;
            Some(ReferenceArchive::open(&view.join("reference"))?)
        }
        _ => return Err("the request reads more than one layer: a leaf has one reference archive".into()),
    };

    let dir = request.output.join("terrain");
    let width = obc_elevation::grid::id_width(cell_log2);
    let (mut empty, mut used) = (Vec::new(), BTreeSet::new());
    for (ci, cj) in cells {
        let (block, lift) = published_cell(&mosaic, ci, cj, posting_log2, cell_log2, reference.as_ref())?;
        used.extend(lift.map.iter().flat_map(|map| map.sources().iter().cloned()));
        match block {
            Some(block) => {
                let path = dir.join(format!("{ci:0width$}/{cj:0width$}.obcd"));
                write_cell_file(&path, posting_log2, cell_log2, ci, cj, &block)?;
            }
            None => empty.push(format!("{cell_log2}/{ci:0width$}/{cj:0width$}")),
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let metadata = request.output.join("metadata");
    std::fs::create_dir_all(&metadata).map_err(|e| e.to_string())?;
    let write = |name: &str, value: Value| {
        let path = metadata.join(name);
        std::fs::write(&path, value.to_string()).map_err(|e| format!("{}: {e}", path.display()))
    };
    write("empty.json", empty.into())?;
    let credits = reference.iter().flat_map(|archive| archive.credits()).filter(|credit| used.contains(&credit.key));
    let credits: Vec<Value> = credits
        .map(|credit| {
            let crate::reference::SourceCredit { key, product, attribution, licence } = credit;
            serde_json::json!({"key": key, "product": product, "attribution": attribution, "licence": licence})
        })
        .collect();
    match credits.is_empty() {
        true => Ok(()),
        false => write("credits.json", credits.into()),
    }
}

/// The terrain of the planner maps. The option `bounds` is `[west, south, east, north]` in
/// degrees. The layer is `terrain.mbtiles`: the bytes that `planner-dem` writes from the same
/// GLO-30 tiles without a reference archive.
#[cfg(feature = "terrarium")]
pub fn planner_terrain(request: &Request) -> Result<(), String> {
    let bounds = bounds(&request.options)?;
    let mut terrain = crate::planner::Terrain::open(&glo30(request, bounds), None, bounds)?;
    let text = bounds.map(|value| value.to_string()).join(",");
    crate::terrarium::write(&mut terrain, bounds, &text, &request.output.join("terrain.mbtiles"))
}

/// The option `bounds`: `[west, south, east, north]` in degrees.
pub fn bounds(options: &Value) -> Result<[f64; 4], String> {
    let values = options["bounds"].as_array().map(|values| values.iter().filter_map(Value::as_f64).collect::<Vec<_>>());
    values
        .and_then(|values| values.try_into().ok())
        .ok_or_else(|| "option `bounds` is not [west, south, east, north]".into())
}

/// The GLO-30 files of the request, in the order of [`crate::planner::tiles`] of `bounds`.
pub fn glo30(request: &Request, bounds: [f64; 4]) -> Vec<PathBuf> {
    let files = request.snapshots.get(GLO30).into_iter().flatten();
    let files: BTreeMap<&str, &PathBuf> =
        files.map(|(name, path)| (name.rsplit('/').next().unwrap_or(name), path)).collect();
    let tiles = crate::planner::tiles(bounds);
    tiles.iter().filter_map(|tile| files.get(tile.file_name().as_str()).map(|path| path.to_path_buf())).collect()
}
