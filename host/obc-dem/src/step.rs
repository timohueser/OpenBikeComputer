//! The terrain step of `obc data`: the published terrain cells of one leaf, from the GLO-30 tiles
//! of its request. `obc-bake terrain` writes the same `.obcd` bytes from the same tiles.

use obc_data::engine::Request;
use serde_json::Value;

use crate::bake::{published_cell, write_cell_file};
use crate::geotiff::{DemMosaic, DemTile};

/// The source whose tiles the step reads.
pub const GLO30: &str = "copernicus-glo-30";

/// The options name the pairing, `posting_log2` and `cell_log2`, and the cells, as `[ci, cj]`.
/// The layer is `terrain/<i>/<j>.obcd` for each cell with a height, and `terrain/empty.json`: the
/// ids of the cells without one, so a leaf at sea is a layer too.
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

    let tiles = request.snapshots.get(GLO30).ok_or(format!("the request has no snapshot {GLO30}"))?;
    let mut mosaic = DemMosaic::default();
    for tile in tiles.values() {
        mosaic.push(DemTile::open(tile)?);
    }

    let dir = request.output.join("terrain");
    let width = obc_elevation::grid::id_width(cell_log2);
    let mut empty = Vec::new();
    for (ci, cj) in cells {
        let (block, _) = published_cell(&mosaic, ci, cj, posting_log2, cell_log2, None)?;
        match block {
            Some(block) => {
                let path = dir.join(format!("{ci:0width$}/{cj:0width$}.obcd"));
                write_cell_file(&path, posting_log2, cell_log2, ci, cj, &block)?;
            }
            None => empty.push(format!("{cell_log2}/{ci:0width$}/{cj:0width$}")),
        }
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join("empty.json");
    std::fs::write(&path, serde_json::to_string(&empty).expect("strings serialize"))
        .map_err(|e| format!("{}: {e}", path.display()))
}
