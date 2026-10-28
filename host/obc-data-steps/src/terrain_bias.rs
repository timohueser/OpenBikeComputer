//! The GLO-30 bias of the national models of a version of `terrain`: `glo30_bias_m` of each model
//! in `index.json`. A model has the bias of the first version that measured it. A model without one
//! has the median of model − GLO-30 over the bias samples (`obc_dem::planner::samples`) of the mean
//! tiles of the version whose best model it is.

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

/// The bias of each best model of a tile of the version that `base`, the index of the version
/// before, does not state. `made` are the sources of each tile that the run made, best first; the
/// mark `<ti>-<tj>.samples` of such a tile holds its samples. The other tiles are those of
/// `previous`.
pub(crate) fn measure(
    job: &mut Job,
    previous: Option<&Manifest>,
    base: Option<&Index>,
    made: &BTreeMap<String, Vec<String>>,
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
    let glo30 = glo30(job, &points)?;
    obc_dem::planner::biases(&points, &glo30)
}

/// The samples of the mark of tile `(ti, tj)`: the height of each sample in decimetres, row by row
/// from the south, or `-` where it has none.
fn marked(job: &Job, ti: u32, tj: u32) -> Result<Vec<Sample>, String> {
    let path = job.state.join("tiles").join(format!("{ti:04}-{tj:04}.samples"));
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let values: Vec<Option<f64>> = text
        .split_whitespace()
        .map(|value| (value != "-").then(|| value.parse::<i64>().map(|dm| dm as f64 / 10.0)).transpose())
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let side = TILE_PIXELS / SAMPLES;
    if values.len() != side * side {
        return Err(format!("{}: {} samples, not {}", path.display(), values.len(), side * side));
    }
    Ok(samples(ti, tj, |r, c| values[r / SAMPLES * side + c / SAMPLES]))
}

/// The GLO-30 squares of `points` that are land, fetched.
fn glo30(job: &mut Job, points: &BTreeMap<String, Vec<Sample>>) -> Result<Vec<(TileId, PathBuf)>, String> {
    let square = |&(lat, lon, _): &Sample| TileId {
        lat: lat.div_euclid(1_000_000) as i32,
        lon: lon.div_euclid(1_000_000) as i32,
    };
    let squares: BTreeSet<TileId> = points.values().flatten().map(square).collect();
    if squares.is_empty() {
        return Ok(Vec::new());
    }
    let (_, list) = job.fetch(&Wanted { source: TILE_LIST.into(), version: None, params: Vec::new() })?;
    let [list] = list.values().collect::<Vec<_>>()[..] else { return Err(format!("{TILE_LIST} is not one file")) };
    let text = fs::read_to_string(list).map_err(|e| format!("{}: {e}", list.display()))?;
    let land: HashSet<&str> = text.lines().map(str::trim).collect();
    let squares: Vec<TileId> = squares.into_iter().filter(|tile| land.contains(tile.stem().as_str())).collect();
    if squares.is_empty() {
        return Ok(Vec::new());
    }
    let params = squares.iter().map(|tile| ("tile".to_string(), tile.stem())).collect();
    let (_, files) = job.fetch(&Wanted { source: GLO30.into(), version: None, params })?;
    let by_name: BTreeMap<&str, &PathBuf> =
        files.iter().map(|(name, path)| (name.rsplit('/').next().unwrap_or(name), path)).collect();
    let found =
        squares.into_iter().filter_map(|tile| Some((tile, by_name.get(tile.file_name().as_str())?.to_path_buf())));
    Ok(found.collect())
}
