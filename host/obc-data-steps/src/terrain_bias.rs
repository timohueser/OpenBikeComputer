//! The GLO-30 bias of the national models of a version of `terrain`: `glo30_bias_m` of each model
//! in `index.json`. A model keeps the bias of the version before. A model without one gets the
//! median of model − GLO-30 over the bias samples (`obc_dem::planner::samples`) of the mean tiles
//! of the version whose best model it is, when there are enough of them
//! (`obc_dem::planner::MIN_SAMPLES`). A version without a version before, such as the first of a
//! format, measures every model.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::PathBuf;

use obc_data::prepared::{Job, Manifest};
use obc_data::product::Wanted;
use obc_dem::fetch::TileId;
use obc_dem::planner::{samples, Sample, SAMPLES};
use obc_dem::reference::{Pool, ReferenceTile, TILE_PIXELS};
use obc_dem::step::GLO30;

use crate::terrain_reference::Index;

/// The tile list of GLO-30: a square without a tile is sea.
const TILE_LIST: &str = "copernicus-glo-30-tiles";

/// The GLO-30 file of each square of the set that is land.
pub(crate) type Glo30 = fn(&mut Job, &BTreeSet<TileId>) -> Result<BTreeMap<TileId, PathBuf>, String>;

/// The bias of each best model of a tile of the version that `base`, the index of the version
/// before, does not state. `made` are the sources of each tile that the run made, best first; the
/// mark `<ti>-<tj>.samples` of such a tile holds its samples. The other tiles are those of
/// `previous`. A sample at sea in `glo30` does not count.
pub(crate) fn measure(
    job: &mut Job,
    (previous, base): (Option<&Manifest>, Option<&Index>),
    made: &BTreeMap<String, Vec<String>>,
    glo30: Glo30,
) -> Result<BTreeMap<String, f64>, String> {
    let stated = |key: &str| base.and_then(|index| index.sources.get(key)?.get("glo30_bias_m")).is_some();
    let mut best: BTreeMap<&str, &str> = BTreeMap::new();
    best.extend(base.into_iter().flat_map(|index| &index.tiles).map(|(tile, key)| (tile.as_str(), key.as_str())));
    best.extend(made.iter().filter_map(|(tile, keys)| Some((tile.as_str(), keys.first()?.as_str()))));
    let mut points: BTreeMap<String, Vec<Sample>> = BTreeMap::new();
    for (tile, key) in best.into_iter().filter(|(_, key)| !stated(key)) {
        let (ti, tj) = tile.split_once('/').and_then(|(i, j)| Some((i.parse().ok()?, j.parse().ok()?))).ok_or(tile)?;
        let found = match (made.contains_key(tile), previous) {
            (true, _) => marked(job, ti, tj)?,
            (false, Some(previous)) => {
                let path = job.earlier_file(previous, &format!("mean/16/{tile}.tif"))?;
                let read = ReferenceTile::open(&path, ti, tj, Pool::Mean)?;
                samples(ti, tj, |r, c| read.height(r, c).map(f64::from))
            }
            (false, None) => unreachable!("a tile that the run did not make is of the version before"),
        };
        points.entry(key.to_string()).or_default().extend(found);
    }
    if points.is_empty() {
        return Ok(BTreeMap::new());
    }
    let squares: BTreeSet<TileId> = points.values().flatten().map(square).collect();
    let files = glo30(job, &squares)?;
    for (key, values) in &mut points {
        let before = values.len();
        values.retain(|sample| files.contains_key(&square(sample)));
        if values.len() < before {
            eprintln!("GLO-30 bias of `{key}`: {} samples at sea in GLO-30 do not count", before - values.len());
        }
    }
    obc_dem::planner::biases(&points, &files.into_iter().collect::<Vec<_>>())
}

fn square(&(lat, lon, _): &Sample) -> TileId {
    TileId { lat: lat.div_euclid(1_000_000) as i32, lon: lon.div_euclid(1_000_000) as i32 }
}

/// The samples of the mark of tile `(ti, tj)`: the height of each sample in decimetres, row by row
/// from the south, or `-` where it has none.
fn marked(job: &Job, ti: u32, tj: u32) -> Result<Vec<Sample>, String> {
    let path = job.state.join("tiles").join(format!("{ti:04}-{tj:04}.samples"));
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text, ti, tj).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse(text: &str, ti: u32, tj: u32) -> Result<Vec<Sample>, String> {
    let values: Vec<Option<f64>> = text
        .split_whitespace()
        .map(|value| (value != "-").then(|| value.parse::<i64>().map(|dm| dm as f64 / 10.0)).transpose())
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let side = TILE_PIXELS / SAMPLES;
    if values.len() != side * side {
        return Err(format!("{} samples, not {}", values.len(), side * side));
    }
    Ok(samples(ti, tj, |r, c| values[r / SAMPLES * side + c / SAMPLES]))
}

/// The GLO-30 files of the squares of `squares` that `copernicus-glo-30-tiles` names. A square that
/// it names and the fetch does not give is an error.
pub(crate) fn fetch(job: &mut Job, squares: &BTreeSet<TileId>) -> Result<BTreeMap<TileId, PathBuf>, String> {
    let (_, list) = job.fetch(&Wanted { source: TILE_LIST.into(), version: None, params: Vec::new() })?;
    let [list] = list.values().collect::<Vec<_>>()[..] else { return Err(format!("{TILE_LIST} is not one file")) };
    let text = fs::read_to_string(list).map_err(|e| format!("{}: {e}", list.display()))?;
    let land: HashSet<&str> = text.lines().map(str::trim).collect();
    let land: Vec<TileId> = squares.iter().copied().filter(|tile| land.contains(tile.stem().as_str())).collect();
    if land.is_empty() {
        return Ok(BTreeMap::new());
    }
    let params = land.iter().map(|tile| ("tile".to_string(), tile.stem())).collect();
    let (_, files) = job.fetch(&Wanted { source: GLO30.into(), version: None, params })?;
    let by_name: BTreeMap<&str, &PathBuf> =
        files.iter().map(|(name, path)| (name.rsplit('/').next().unwrap_or(name), path)).collect();
    let found = land.into_iter().map(|tile| match by_name.get(tile.file_name().as_str()) {
        Some(path) => Ok((tile, path.to_path_buf())),
        None => Err(format!("the fetch of {GLO30} gives no file of the land square {}", tile.stem())),
    });
    found.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_dem::planner::SAMPLE;
    use obc_dem::reference::{STEP_LOG2, TILE_LOG2};
    use obc_formats::obct::GRID_ORIGIN;

    /// The mark lists the samples row by row from the south and west to east. Each value is unique,
    /// so a flip or a transposition fails.
    #[test]
    fn the_samples_of_a_mark_are_row_by_row_from_the_south() {
        let side = TILE_PIXELS / SAMPLES;
        let mut tokens: Vec<String> = (0..side * side).map(|k| (10 * k).to_string()).collect();
        tokens[1] = "-".into();
        let (ti, tj) = (4809, 4222);
        let parsed = parse(&tokens.join(" "), ti, tj).unwrap();
        assert_eq!(parsed.len(), side * side - 1);
        let (step, tile) = (1i64 << STEP_LOG2, 1i64 << TILE_LOG2);
        let centre =
            |id: u32, pixel: usize| i64::from(GRID_ORIGIN) + i64::from(id) * tile + pixel as i64 * step + step / 2;
        let at = |row: usize, col: usize| (centre(ti, SAMPLE + SAMPLES * row), centre(tj, SAMPLE + SAMPLES * col));
        let find = |(lat, lon): (i64, i64)| parsed.iter().find(|s| (s.0, s.1) == (lat, lon)).map(|s| s.2);
        assert_eq!(find(at(0, 0)), Some(0.0), "the first value is the south-west sample");
        assert_eq!(find(at(0, 1)), None, "the second is east of it");
        assert_eq!(find(at(0, 2)), Some(2.0));
        assert_eq!(find(at(1, 0)), Some(side as f64), "the next line is north");
        assert!(parse("1 2 3", ti, tj).is_err());
    }
}
