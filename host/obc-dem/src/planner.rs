//! The heights of the planner: its terrain tiles and the slopes of its routes. The reference
//! archive gives the height where it holds the ground, GLO-30 elsewhere, and the two meet in a
//! feathered seam.
//!
//! GLO-30 is a surface model: it stands on roofs and on the forest canopy. Its bias is the median
//! of archive − GLO-30 over the overlap, and GLO-30 takes it near the archive. The edge of the
//! archive is where a box blur of its coverage, [`FEATHER_M`] wide on each side, is one half. From
//! the edge, the weight of the archive rises to 1 at [`FEATHER_M`] inside, and the bias fades out
//! at [`FEATHER_M`] outside, so the height is continuous everywhere.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use obc_data::store::hash_file;
use rayon::prelude::*;

use crate::fetch::TileId;
use crate::geotiff::{DemMosaic, DemTile};
use crate::reference::{ReferenceArchive, ReferenceTile, TileLookup, Window, ORIGIN, STEP, TILE_PIXELS, TILE_UDEG};

/// The half-width of the seam in metres. A canopy step of 20 m between the archive and GLO-30
/// ramps over it at 2 %, which the relief does not show as an edge.
pub const FEATHER_M: f64 = 1000.0;
/// A coverage cell is 2^4 × 2^4 archive pixels: about 114 m × 78 m at 47° N.
const CELL_LOG2: u32 = 4;
const CELLS: usize = TILE_PIXELS >> CELL_LOG2;
const CELL_UDEG: f64 = (STEP << CELL_LOG2) as f64;
/// Metres per µdeg of latitude.
const M_PER_UDEG: f64 = 0.111_32;

/// The GLO-30 squares that `bounds` (west, south, east, north in degrees) touches, south to north,
/// then west to east: the order of [`Terrain::identities`].
pub fn tiles([west, south, east, north]: [f64; 4]) -> Vec<TileId> {
    let lats = south.floor() as i32..=north.floor() as i32;
    lats.flat_map(|lat| (west.floor() as i32..=east.floor() as i32).map(move |lon| TileId { lat, lon })).collect()
}

/// The GLO-30 files of `bounds` in `dir`: a square without a file is sea.
pub fn tiles_in(dir: &Path, bounds: [f64; 4]) -> Vec<(TileId, PathBuf)> {
    let files = tiles(bounds).into_iter().map(|tile| (tile, dir.join(tile.file_name())));
    files.filter(|(_, path)| path.is_file()).collect()
}

/// The GLO-30 squares, by latitude, whose posts a height at `lat` in microdegrees can read: its
/// bilinear corners lie within two posts, and a square holds the posts of its own degree.
pub fn squares(lat: i32) -> RangeInclusive<i32> {
    const SLACK: i32 = 10_000;
    (lat - SLACK).div_euclid(1_000_000)..=(lat + SLACK).div_euclid(1_000_000)
}

/// Archive tiles by id, the oldest first; `None` where the archive holds no tile.
type Tiles = VecDeque<((u32, u32), Option<Arc<ReferenceTile>>)>;

/// The reference tiles that the last heights of one caller read, without a lock.
#[derive(Default)]
pub struct Recent(Tiles);

/// Decoded archive tiles at most, 4 MiB each: three tile rows across the Alps.
const DECODED: usize = 256;
/// The tiles of one caller at most: the four around a pixel, and the next ones.
const RECENT: usize = 8;

pub struct Terrain {
    /// The GLO-30 files by the latitude of their square.
    glo30: BTreeMap<i32, Vec<PathBuf>>,
    /// The decoded squares of the latitudes in `loaded`.
    fallback: DemMosaic,
    loaded: BTreeSet<i32>,
    reference: Option<ReferenceArchive>,
    /// The decoded archive tiles. The sweep of the terrain tiles reads one band of latitude at
    /// every zoom, so a tile decodes once for the whole sweep.
    decoded: Mutex<Tiles>,
    /// The blurred coverage of each coverage cell, by archive tile; a tile that is not here has 0.
    seam: BTreeMap<(u32, u32), Vec<f32>>,
    /// The median of archive − GLO-30 over the overlap, in metres.
    bias: f64,
    /// The SHA-256 of each GLO-30 file, then of each reference tile that `bounds` reaches.
    pub identities: Vec<String>,
    used: Mutex<BTreeSet<String>>,
    /// The credit of GLO-30.
    credit: String,
}

impl Terrain {
    /// The GLO-30 files `glo30`, in the order of [`tiles`], with their `credit`, and the reference
    /// archive at `reference`, for heights in `bounds`. Each file is hashed and its header checked.
    /// With an archive, its tiles in `bounds` decode once for the seam and the bias, with the
    /// GLO-30 squares under them.
    pub fn open(
        glo30: &[(TileId, PathBuf)],
        credit: &str,
        reference: Option<&Path>,
        bounds: [f64; 4],
    ) -> Result<Self, String> {
        let hashes = glo30.par_iter().map(|(_, path)| {
            DemTile::check(path)?;
            hash_file(path).map(|(sha256, _)| sha256).map_err(|e| format!("{}: {e}", path.display()))
        });
        let mut identities = hashes.collect::<Result<Vec<_>, _>>()?;
        let mut files = BTreeMap::<_, Vec<_>>::new();
        for (tile, path) in glo30 {
            files.entry(tile.lat).or_default().push(path.clone());
        }
        let reference = reference.map(ReferenceArchive::open).transpose()?;
        let window = Window {
            lat_lo: (bounds[1] * 1e6) as i64,
            lat_hi: (bounds[3] * 1e6) as i64 + 1,
            lon_lo: (bounds[0] * 1e6) as i64,
            lon_hi: (bounds[2] * 1e6) as i64 + 1,
        };
        if let Some(archive) = &reference {
            identities.extend(archive.held_digests(window).into_iter().map(|(_, _, hash)| hash.to_owned()));
        }
        let mut terrain = Self {
            glo30: files,
            fallback: DemMosaic::default(),
            loaded: BTreeSet::new(),
            reference,
            decoded: Mutex::default(),
            seam: BTreeMap::new(),
            bias: 0.0,
            identities,
            used: Mutex::default(),
            credit: credit.into(),
        };
        terrain.measure_seam(window)?;
        Ok(terrain)
    }

    /// The coverage of each archive tile in `window`, blurred into `seam`, and the bias from the
    /// archive pixels whose row and column are 8 modulo 16. Tile rows go north to south, so each
    /// GLO-30 square decodes once.
    fn measure_seam(&mut self, window: Window) -> Result<(), String> {
        let Some(archive) = self.reference.take() else { return Ok(()) };
        let held: Vec<(u32, u32)> = archive.held_digests(window).into_iter().map(|(ti, tj, _)| (ti, tj)).collect();
        let (mut coverage, mut differences) = (BTreeMap::new(), Vec::new());
        let share = 1.0 / (1u32 << (2 * CELL_LOG2)) as f32;
        for &(ti, tj) in held.iter().rev() {
            let south = (ORIGIN + i64::from(ti) * TILE_UDEG) as i32;
            self.load(*squares(south).start()..=*squares(south + TILE_UDEG as i32).end())?;
            let TileLookup::Held { tile, .. } = archive.tile(ti, tj)? else { continue };
            let (lat0, lon0) = tile.origin_udeg();
            let mut cells = vec![0f32; CELLS * CELLS];
            for r in 0..TILE_PIXELS {
                for c in 0..TILE_PIXELS {
                    let Some(height) = tile.height(r, c) else { continue };
                    cells[(r >> CELL_LOG2) * CELLS + (c >> CELL_LOG2)] += share;
                    if r % 16 == 8 && c % 16 == 8 {
                        let (lat, lon) = (lat0 + r as i64 * STEP + STEP / 2, lon0 + c as i64 * STEP + STEP / 2);
                        if let Some(glo30) = self.fallback.height(lat as f64 * 1e-6, lon as f64 * 1e-6) {
                            differences.push(f64::from(height) - glo30);
                        }
                    }
                }
            }
            coverage.insert((ti, tj), cells);
        }
        if !differences.is_empty() {
            let middle = differences.len() / 2;
            self.bias = *differences.select_nth_unstable_by(middle, f64::total_cmp).1;
        }
        let near: BTreeSet<(u32, u32)> = held
            .iter()
            .flat_map(|&(ti, tj)| (ti - 1..=ti + 1).flat_map(move |i| (tj - 1..=tj + 1).map(move |j| (i, j))))
            .collect();
        for (ti, tj) in near {
            let blurred = blur(&coverage, ti, tj);
            if blurred.iter().any(|&value| value > 0.0) {
                self.seam.insert((ti, tj), blurred);
            }
        }
        self.reference = Some(archive);
        Ok(())
    }

    /// Decode the GLO-30 squares of the latitudes `lats` and drop every other square.
    pub fn load(&mut self, lats: RangeInclusive<i32>) -> Result<(), String> {
        let missing: Vec<_> = self
            .glo30
            .range(lats.clone())
            .filter(|(lat, _)| !self.loaded.contains(lat))
            .flat_map(|(_, paths)| paths)
            .collect();
        let decoded = missing.into_par_iter().map(|path| DemTile::open(path)).collect::<Result<Vec<_>, _>>()?;
        // A GLO-30 square holds the posts north of its south edge, up to and with the next degree.
        self.fallback.retain(|tile| lats.contains(&(tile.south_west_deg().0.floor() as i32)));
        decoded.into_iter().for_each(|tile| self.fallback.push(tile));
        self.loaded = self.glo30.range(lats).map(|(lat, _)| *lat).collect();
        Ok(())
    }

    /// The height in metres at `lat`, `lon` in microdegrees, or `None` where no source has one.
    /// GLO-30 answers from the squares that [`Terrain::load`] decoded, which must hold
    /// [`squares`] of `lat`.
    pub fn height(&self, lat: i32, lon: i32, recent: &mut Recent) -> Result<Option<f64>, String> {
        debug_assert!(
            squares(lat).all(|lat| self.loaded.contains(&lat) || !self.glo30.contains_key(&lat)),
            "the GLO-30 squares of {lat} µdeg are not loaded"
        );
        let glo30 = self.fallback.height(lat as f64 * 1e-6, lon as f64 * 1e-6);
        let Some(archive) = &self.reference else { return Ok(glo30) };
        let national = self.national(archive, lat, lon, recent)?;
        Ok(blend(national, glo30, self.blurred(lat, lon), self.bias))
    }

    /// Whether the archive holds the ground somewhere in `bounds` (west, south, east, north in
    /// degrees): a coverage cell there is inside the edge of the seam.
    pub fn national_in(&self, [west, south, east, north]: [f64; 4]) -> bool {
        let cell = |degrees: f64| ((degrees * 1e6 - ORIGIN as f64) / CELL_UDEG).floor() as i64;
        let (rows, cols) = (cell(south)..=cell(north), cell(west)..=cell(east));
        rows.flat_map(|i| cols.clone().map(move |j| (i, j))).any(|(i, j)| self.cell(i, j) > 0.5)
    }

    /// The blurred coverage of the coverage cell `(i, j)`, counted from the lattice origin.
    fn cell(&self, i: i64, j: i64) -> f64 {
        let size = CELLS as i64;
        let at = (i.rem_euclid(size) * size + j.rem_euclid(size)) as usize;
        let tile = (i.div_euclid(size) as u32, j.div_euclid(size) as u32);
        self.seam.get(&tile).map_or(0.0, |cells| f64::from(cells[at]))
    }

    /// The blurred coverage at a point, bilinear between the centres of the coverage cells.
    fn blurred(&self, lat: i32, lon: i32) -> f64 {
        let cell = |udeg: i32| (f64::from(udeg) - ORIGIN as f64) / CELL_UDEG - 0.5;
        let (y, x) = (cell(lat), cell(lon));
        let (i, j, fy, fx) = (y.floor() as i64, x.floor() as i64, y - y.floor(), x - x.floor());
        let row = |i| self.cell(i, j) * (1.0 - fx) + self.cell(i, j + 1) * fx;
        row(i) * (1.0 - fy) + row(i + 1) * fy
    }

    /// The archive height at a point, bilinear between the four pixel centres around it. A
    /// missing centre leaves the others their weights.
    fn national(
        &self,
        archive: &ReferenceArchive,
        lat: i32,
        lon: i32,
        recent: &mut Recent,
    ) -> Result<Option<f64>, String> {
        let pixel = |udeg: i32| (f64::from(udeg) - (ORIGIN + STEP / 2) as f64) / STEP as f64;
        let (y, x) = (pixel(lat), pixel(lon));
        let (i0, j0, fy, fx) = (y.floor() as i64, x.floor() as i64, y - y.floor(), x - x.floor());
        let (mut sum, mut weight, size) = (0.0, 0.0, TILE_PIXELS as i64);
        for (i, wy) in [(i0, 1.0 - fy), (i0 + 1, fy)] {
            for (j, wx) in [(j0, 1.0 - fx), (j0 + 1, fx)] {
                let key = ((i / size) as u32, (j / size) as u32);
                let Some(tile) = self.decoded(archive, key, recent)? else { continue };
                if let Some(height) = tile.height((i % size) as usize, (j % size) as usize) {
                    sum += wy * wx * f64::from(height);
                    weight += wy * wx;
                }
            }
        }
        Ok((weight > 0.0).then(|| sum / weight))
    }

    /// Archive tile `key`, from `recent`, else from the decoded tiles, else decoded.
    fn decoded<'r>(
        &self,
        archive: &ReferenceArchive,
        key: (u32, u32),
        recent: &'r mut Recent,
    ) -> Result<Option<&'r ReferenceTile>, String> {
        let cache = &mut recent.0;
        if cache.back().is_none_or(|(id, _)| *id != key) {
            let tile = match cache.iter().position(|(id, _)| *id == key) {
                Some(index) => cache.remove(index).expect("the position is in the cache").1,
                None => self.shared(archive, key)?,
            };
            cache.push_back((key, tile));
            if cache.len() > RECENT {
                cache.pop_front();
            }
        }
        Ok(cache.back().and_then(|(_, tile)| tile.as_deref()))
    }

    fn shared(&self, archive: &ReferenceArchive, key: (u32, u32)) -> Result<Option<Arc<ReferenceTile>>, String> {
        if let Some((_, tile)) = self.decoded.lock().unwrap().iter().find(|(id, _)| *id == key) {
            return Ok(tile.clone());
        }
        let tile = match archive.tile(key.0, key.1)? {
            TileLookup::Held { tile, sources } => {
                self.used.lock().unwrap().extend(sources.iter().cloned());
                Some(Arc::new(tile))
            }
            TileLookup::Absent | TileLookup::Unknown => None,
        };
        let mut decoded = self.decoded.lock().unwrap();
        decoded.push_back((key, tile.clone()));
        if decoded.len() > DECODED {
            decoded.pop_front();
        }
        Ok(tile)
    }

    /// The credits of the sources that gave a height so far.
    pub fn attribution(&self) -> Vec<String> {
        let mut credits = Vec::new();
        if let Some(archive) = &self.reference {
            let used = self.used.lock().unwrap();
            credits.extend(
                archive
                    .credits()
                    .iter()
                    .filter(|c| used.contains(&c.key))
                    .map(|c| format!("{} ({})", c.attribution, c.licence)),
            );
        }
        if !self.glo30.is_empty() {
            credits.push(self.credit.clone());
        }
        credits
    }
}

/// The height in the seam, from the archive height, the GLO-30 height, the blurred coverage of
/// the archive there and the bias of GLO-30.
pub fn blend(national: Option<f64>, glo30: Option<f64>, blurred: f64, bias: f64) -> Option<f64> {
    let weight = national.map_or(0.0, |_| (2.0 * blurred - 1.0).clamp(0.0, 1.0));
    let corrected = glo30.map(|height| height + bias * (2.0 * blurred).clamp(0.0, 1.0));
    match (national, corrected) {
        (Some(national), Some(corrected)) => Some(weight * national + (1.0 - weight) * corrected),
        (national, corrected) => national.or(corrected),
    }
}

/// The coverage cells of tile `(ti, tj)`, each the mean coverage of the cells within
/// [`FEATHER_M`] north, south, east and west of it, from the tile and its eight neighbours.
fn blur(coverage: &BTreeMap<(u32, u32), Vec<f32>>, ti: u32, tj: u32) -> Vec<f32> {
    const SIDE: usize = 3 * CELLS;
    let lat = (ORIGIN + i64::from(ti) * TILE_UDEG + TILE_UDEG / 2) as f64 * 1e-6;
    let rows = (FEATHER_M / (CELL_UDEG * M_PER_UDEG)).round() as usize;
    let cols = (FEATHER_M / (CELL_UDEG * M_PER_UDEG * lat.to_radians().cos())).round() as usize;
    assert!(rows < CELLS && cols < CELLS, "the feather is narrower than a tile");
    // The summed area of the 3 × 3 block, after one row and one column of zeros.
    let mut sums = vec![0f64; (SIDE + 1) * (SIDE + 1)];
    for r in 0..SIDE {
        for c in 0..SIDE {
            let tile = (ti + (r / CELLS) as u32 - 1, tj + (c / CELLS) as u32 - 1);
            let value = coverage.get(&tile).map_or(0.0, |cells| cells[(r % CELLS) * CELLS + c % CELLS]);
            sums[(r + 1) * (SIDE + 1) + c + 1] =
                f64::from(value) + sums[r * (SIDE + 1) + c + 1] + sums[(r + 1) * (SIDE + 1) + c]
                    - sums[r * (SIDE + 1) + c];
        }
    }
    let mut out = vec![0f32; CELLS * CELLS];
    for r in 0..CELLS {
        for c in 0..CELLS {
            let (r0, r1, c0, c1) = (CELLS + r - rows, CELLS + r + rows + 1, CELLS + c - cols, CELLS + c + cols + 1);
            let area = sums[r1 * (SIDE + 1) + c1] - sums[r0 * (SIDE + 1) + c1] - sums[r1 * (SIDE + 1) + c0]
                + sums[r0 * (SIDE + 1) + c0];
            out[r * CELLS + c] = (area / ((r1 - r0) * (c1 - c0)) as f64) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Across a straight edge, the seam meets both sources with no step: GLO-30 with its bias at
    /// the edge, and the archive alone `FEATHER_M` inside it.
    #[test]
    fn the_seam_feathers_the_archive_into_glo30_with_its_bias() {
        let (national, glo30, bias) = (Some(500.0), Some(503.0), -2.0);
        // The box blur of a straight edge is a ramp: 0 at the feather outside, ½ at the edge, 1 inside.
        let at = |metres: f64| blend(national, glo30, (0.5 + metres / (2.0 * FEATHER_M)).clamp(0.0, 1.0), bias);
        assert_eq!(at(-FEATHER_M), Some(503.0), "far outside, GLO-30 as it is");
        assert_eq!(at(-FEATHER_M / 2.0), Some(502.0), "the bias fades in");
        assert_eq!(at(0.0), Some(501.0), "at the edge, GLO-30 with its bias");
        assert_eq!(at(FEATHER_M / 2.0), Some(500.5));
        assert_eq!(at(FEATHER_M), Some(500.0), "inside, the archive alone");
        let heights: Vec<f64> = (-120..=120).map(|step| at(f64::from(step) * 10.0).unwrap()).collect();
        assert!(heights.windows(2).all(|pair| (pair[1] - pair[0]).abs() <= 0.03), "no step anywhere");
        // Where one source has no height, the other answers.
        assert_eq!(blend(None, glo30, 1.0, bias), Some(501.0));
        assert_eq!(blend(national, None, 0.0, bias), Some(500.0));
        assert_eq!(blend(None, None, 0.5, bias), None);
    }

    /// The blur of a tile of full coverage beside a tile of none is one half at their edge, 1
    /// inside and 0 outside.
    #[test]
    fn the_blurred_coverage_is_one_half_at_the_edge_of_the_archive() {
        let coverage = BTreeMap::from([((4810, 4224), vec![1f32; CELLS * CELLS])]);
        let (inside, outside) = (blur(&coverage, 4810, 4224), blur(&coverage, 4810, 4225));
        let row = CELLS / 2 * CELLS;
        let (last, first) = (inside[row + CELLS - 1], outside[row]);
        assert!((last + first - 1.0).abs() < 1e-6 && (0.5..0.6).contains(&last), "{last} {first}");
        assert_eq!((inside[row + CELLS / 2], outside[row + CELLS / 2]), (1.0, 0.0));
    }
}
