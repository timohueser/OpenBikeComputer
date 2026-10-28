//! The GLO-30 bias of the national models of a version of `terrain`: `glo30_bias_m` of each model
//! in `index.json`. A model keeps the bias of the version before. A model without one gets the
//! median of model − GLO-30 over the bias samples (`obc_dem::planner::samples`) of the mean tiles
//! of the version whose best model it is, when there are enough of them
//! (`obc_dem::planner::MIN_SAMPLES`). A version without a version before, such as the first of a
//! format, measures every model.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

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

/// A tile with a height of a block that the run made, as the mark `<bi>-<bj>.tiles` of the block
/// names it: its sources best first, and its bias samples.
#[derive(serde::Deserialize)]
pub(crate) struct Made {
    pub sources: Vec<String>,
    samples: String,
}

/// The bias of each best model of a tile of the version that `base`, the index of the version
/// before, does not state. `made` are the tiles of the blocks that the run made; the other tiles are
/// those of `previous`, outside those blocks. A sample at sea in `glo30` does not count.
pub(crate) fn measure(
    job: &mut Job,
    (previous, base): (Option<&Manifest>, Option<&Index>),
    made: &BTreeMap<String, Made>,
    glo30: Glo30,
) -> Result<BTreeMap<String, f64>, String> {
    let stated = |key: &str| base.and_then(|index| index.sources.get(key)?.get("glo30_bias_m")).is_some();
    let mut best: BTreeMap<&str, (&str, Option<&Made>)> = BTreeMap::new();
    for (tile, sources) in base.iter().flat_map(|index| &index.contributors) {
        best.extend(sources.first().map(|key| (tile.as_str(), (key.as_str(), None))));
    }
    for (tile, made) in made {
        best.extend(made.sources.first().map(|key| (tile.as_str(), (key.as_str(), Some(made)))));
    }
    let mut points: BTreeMap<String, Vec<Sample>> = BTreeMap::new();
    for (tile, (key, made)) in best.into_iter().filter(|(_, (key, _))| !stated(key)) {
        let (ti, tj) = tile.split_once('/').and_then(|(i, j)| Some((i.parse().ok()?, j.parse().ok()?))).ok_or(tile)?;
        let found = match (made, previous) {
            (Some(made), _) => parse(&made.samples, ti, tj).map_err(|e| format!("the samples of tile {tile}: {e}"))?,
            (None, Some(previous)) => {
                earlier(&job.earlier_file(previous, &crate::terrain_reference::block_file("mean/", ti, tj))?, ti, tj)?
            }
            (None, None) => unreachable!("a tile that the run did not make is of the version before"),
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

/// The samples of tile `(ti, tj)` in the mean block at `path` of the version before.
fn earlier(path: &Path, ti: u32, tj: u32) -> Result<Vec<Sample>, String> {
    let read = ReferenceTile::open_in_block(path, ti, tj, Pool::Mean)?;
    Ok(samples(ti, tj, |r, c| read.height(r, c).map(f64::from)))
}

/// The samples of a tile: the height of each sample in decimetres, row by row from the south, or
/// `-` where it has none.
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

    /// A tile of the version before is a window of its mean block: tile 4812/4222 is the south row
    /// and the third column of block 1203/1055. A pixel holds its index in the block modulo 10^6,
    /// so a flip or a shift of the window reads other values at the samples.
    #[test]
    fn the_samples_of_a_tile_of_the_version_before_are_those_of_its_window_of_the_mean_block() {
        use tiff::encoder::{colortype::GrayI32, TiffEncoder};
        use tiff::tags::Tag;

        let side = 4 * TILE_PIXELS;
        let value = |row: usize, col: usize| (row * side + col) as i32 % 1_000_000;
        let mut pixels: Vec<i32> = (0..side * side).map(|k| value(k / side, k % side)).collect();
        // The sample of row 0 and column 1 of the tile, from the south and the west, has no height.
        let (row0, col0) = (3 * TILE_PIXELS + TILE_PIXELS - 1, 2 * TILE_PIXELS);
        pixels[(row0 - SAMPLE) * side + col0 + SAMPLE + SAMPLES] = i32::MIN;
        let (step, tile) = (1i64 << STEP_LOG2, 1i64 << TILE_LOG2);
        let degrees = |udeg: i64| udeg as f64 / 1e6;
        let temp = crate::device::tests::temp("bias-block");
        fs::create_dir_all(&temp.0).unwrap();
        let path = temp.0.join("block.tif");
        let mut encoder = TiffEncoder::new(fs::File::create(&path).unwrap()).unwrap();
        let mut image = encoder.new_image::<GrayI32>(side as u32, side as u32).unwrap();
        let (north, west) = (i64::from(GRID_ORIGIN) + 4816 * tile, i64::from(GRID_ORIGIN) + 4220 * tile);
        let scale = [degrees(step), degrees(step), 0.0];
        image.encoder().write_tag(Tag::ModelPixelScaleTag, &scale[..]).unwrap();
        let tie = [0.0, 0.0, 0.0, degrees(west), degrees(north), 0.0];
        image.encoder().write_tag(Tag::ModelTiepointTag, &tie[..]).unwrap();
        image.encoder().write_tag(Tag::GdalNodata, "-2147483648").unwrap();
        image.write_data(&pixels).unwrap();

        let found = earlier(&path, 4812, 4222).unwrap();
        let side_samples = TILE_PIXELS / SAMPLES;
        assert_eq!(found.len(), side_samples * side_samples - 1);
        let centre = |id: i64, pixel: usize| i64::from(GRID_ORIGIN) + id * tile + pixel as i64 * step + step / 2;
        let at =
            |r: usize, c: usize| found.iter().find(|s| (s.0, s.1) == (centre(4812, r), centre(4222, c))).map(|s| s.2);
        let metres = |r: usize, c: usize| f64::from(value(row0 - r, col0 + c) as f32 / 10.0);
        assert_eq!(at(SAMPLE, SAMPLE), Some(metres(SAMPLE, SAMPLE)), "the south-west sample");
        assert_eq!(at(SAMPLE, SAMPLE + SAMPLES), None);
        assert_eq!(at(SAMPLE + SAMPLES, SAMPLE), Some(metres(SAMPLE + SAMPLES, SAMPLE)), "north of it");
        assert!(earlier(&path, 4816, 4222).is_err(), "the block of another tile refuses");
    }
}
