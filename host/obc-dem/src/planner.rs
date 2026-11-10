//! The heights of the planner: its terrain tiles and the slopes of its routes. The reference
//! archive gives the height where it holds the ground, GLO-30 elsewhere, and the two meet in a
//! feathered seam.
//!
//! GLO-30 is a surface model: it stands on roofs and on the forest canopy. The terrain prepare
//! measures the bias of each national model once, the median of model − GLO-30 ([`biases`]), and
//! the index of the archive states it. Near the archive, GLO-30 takes the bias of the best model of
//! the tile, and outside the archive that of the nearest full cell. A coverage cell is full where
//! the archive has each of its pixels. The depth of a point is its distance into the
//! full cells, or minus its distance to them, less the diagonal of a cell: the cells next to the
//! edge, on both sides, have depth 0. The weight of the archive rises from 0 at depth 0 to 1 at
//! [`FEATHER_M`], and the bias fades out from depth 0 to −[`FEATHER_M`]. So the weight is 0
//! wherever the archive misses a pixel near a point, also at a concave edge such as a national
//! border, and the height is continuous everywhere. The seam reads the coverage of one ring of
//! tiles around the bounds ([`reach`]), so a height does not depend on the bounds of the bake.
//!
//! Where GLO-30 has weight, [`crate::shape`] corrects it at the coastlines and summits of OSM.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use obc_data::store::hash_file;
use rayon::prelude::*;

use crate::fetch::TileId;
use crate::geotiff::{DemMosaic, DemTile};
use crate::reference::{ReferenceArchive, ReferenceTile, TileLookup, Window, ORIGIN, STEP, TILE_PIXELS, TILE_UDEG};
use crate::shape::{Features, Shapes, READ_M};

/// A bias sample is each pixel of a tile whose row and column, counted north and east, are
/// [`SAMPLE`] modulo [`SAMPLES`]: 16 × 16 pixels a tile.
pub const SAMPLES: usize = 64;
pub const SAMPLE: usize = 32;
/// A bias sample: latitude and longitude in µdeg, and the height of the model in metres.
pub type Sample = (i64, i64, f64);
/// The fewest samples with GLO-30 that a bias is measured from: those of four full tiles, so that a
/// test version of one tile does not fix the bias of a model.
pub const MIN_SAMPLES: usize = 4 * (TILE_PIXELS / SAMPLES) * (TILE_PIXELS / SAMPLES);
/// How a version of `terrain` without the bias of a model gets it.
pub const BIAS_FIX: &str = "a prepare of a box of about four tiles inside the extent of the model extends the \
     version and measures it: obc data source prepare terrain --box W,S,E,N --dry-run";

/// The width of each ramp of the seam in metres. A canopy step of 20 m between the archive and
/// GLO-30 ramps over it at 2 %, which the relief does not show as an edge.
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

/// The archive pixels that the heights in `bounds` (west, south, east, north in degrees) read:
/// those of its tiles and of the ring of tiles around them.
pub fn reach([west, south, east, north]: [f64; 4]) -> Window {
    let udeg = |degrees: f64| (degrees * 1e6) as i64;
    Window {
        lat_lo: udeg(south) - TILE_UDEG,
        lat_hi: udeg(north) + 1 + TILE_UDEG,
        lon_lo: udeg(west) - TILE_UDEG,
        lon_hi: udeg(east) + 1 + TILE_UDEG,
    }
}

/// The GLO-30 files of `bounds` in `dir`: a square without a file is sea.
pub fn tiles_in(dir: &Path, bounds: [f64; 4]) -> Vec<(TileId, PathBuf)> {
    let files = tiles(bounds).into_iter().map(|tile| (tile, dir.join(tile.file_name())));
    files.filter(|(_, path)| path.is_file()).collect()
}

/// How far in µdeg of latitude a height reads GLO-30: a shaped height reads GLO-30 up to
/// [`READ_M`] away, and its bilinear corners lie within two posts of that.
const SLACK: i32 = 10_000;
const _: () = assert!(READ_M / M_PER_UDEG + 2.0 * 1e6 / 3600.0 < SLACK as f64);

/// The GLO-30 squares, by latitude, whose posts a height at `lat` in microdegrees can read. A
/// square holds the posts of its own degree.
pub fn squares(lat: i32) -> RangeInclusive<i32> {
    (lat - SLACK).div_euclid(1_000_000)..=(lat + SLACK).div_euclid(1_000_000)
}

/// The GLO-30 squares, by longitude, whose posts a height at `lat`, `lon` in microdegrees can
/// read. The slack grows with 1 / cos(lat), and the longitude posting of GLO-30 within 3 % of that.
fn columns(lat: i32, lon: i32) -> RangeInclusive<i32> {
    let slack = f64::from(SLACK) / (f64::from(lat) * 1e-6).to_radians().cos();
    let square = |udeg: f64| (udeg * 1e-6).floor() as i32;
    square(f64::from(lon) - slack)..=square(f64::from(lon) + slack)
}

/// Values by archive tile, the oldest first.
type Fifo<T> = VecDeque<((u32, u32), T)>;
/// Archive tiles by id; `None` where the archive holds no tile.
type Tiles = Fifo<Option<Arc<ReferenceTile>>>;
/// The depth and the bias of each coverage cell of an archive tile, row by row.
type Depths = Arc<[[f32; 2]]>;

/// The reference tiles and the depth tiles that the last heights of one caller read, without a
/// lock. A `Recent` serves one [`Terrain`]: the depths depend on its reach.
#[derive(Default)]
pub struct Recent {
    tiles: Tiles,
    depths: Fifo<Option<Depths>>,
}

/// The value of `key` from `cache`, else from `get`, kept as the newest of at most `most`.
fn recall<T: ?Sized>(
    cache: &mut Fifo<Option<Arc<T>>>,
    key: (u32, u32),
    most: usize,
    get: impl FnOnce() -> Result<Option<Arc<T>>, String>,
) -> Result<Option<&T>, String> {
    if cache.back().is_none_or(|(id, _)| *id != key) {
        let value = match cache.iter().position(|(id, _)| *id == key) {
            Some(index) => cache.remove(index).expect("the position is in the cache").1,
            None => get()?,
        };
        cache.push_back((key, value));
        if cache.len() > most {
            cache.pop_front();
        }
    }
    Ok(cache.back().and_then(|(_, value)| value.as_deref()))
}

/// The value of `key` in `fifo`.
fn find<T: Clone>(fifo: &Fifo<T>, key: (u32, u32)) -> Option<T> {
    fifo.iter().find(|(id, _)| *id == key).map(|(_, value)| value.clone())
}

/// The value of `key` in `fifo`, else `value`, kept as the newest of at most `most`. So of two
/// threads that compute one key, the first insert stays.
fn keep<T: Clone>(fifo: &mut Fifo<T>, key: (u32, u32), value: T, most: usize) -> T {
    if let Some(kept) = find(fifo, key) {
        return kept;
    }
    fifo.push_back((key, value.clone()));
    if fifo.len() > most {
        fifo.pop_front();
    }
    value
}

/// Decoded archive tiles at most, 4 MiB each: 1 GiB, about one and a half tile rows across the
/// Alps (11°, about 170 tiles a row). Over a wider archive, the sweep decodes some tiles again.
const DECODED: usize = 256;
/// The tiles of one caller at most: the four around a pixel, and the next ones.
const RECENT: usize = 8;
/// The depth tiles of one caller at most: the four around a point.
const RECENT_DEPTHS: usize = 4;
/// The coverages of the seam at most, about 0.5 KiB each: 2 MiB.
const COVERAGES: usize = 4096;
/// The depth tiles of the seam at most, 32 KiB each: 8 MiB.
const DEPTHS: usize = 256;
/// The GLO-30 squares that heights decode at most, 52 MB each: the nine around a square, and the
/// next ones.
const SQUARES: usize = 12;

/// A GLO-30 square: the latitude and the longitude of its south-west corner in degrees.
type Square = (i32, i32);

pub struct Terrain {
    /// The GLO-30 files by square.
    glo30: BTreeMap<Square, PathBuf>,
    /// The latitudes that [`Terrain::load`] decoded, at every longitude, their squares, and the
    /// mosaic of them.
    rows: Option<RangeInclusive<i32>>,
    loaded: BTreeMap<Square, Arc<DemTile>>,
    fallback: DemMosaic,
    /// The other squares that heights decoded, the oldest first: at most [`SQUARES`].
    squares: Mutex<VecDeque<(Square, Arc<DemTile>)>>,
    reference: Option<Arc<ReferenceArchive>>,
    /// The decoded archive tiles. The sweep of the terrain tiles reads one band of latitude at
    /// every zoom, so a tile decodes once for the whole sweep.
    decoded: Mutex<Tiles>,
    seam: Seam,
    /// The SHA-256 of each GLO-30 file, then of each reference tile of [`reach`].
    pub identities: Vec<String>,
    used: Mutex<BTreeSet<String>>,
    /// The credit of GLO-30.
    credit: String,
    /// The mapped features that correct GLO-30, and their credit.
    shapes: Shapes,
    shapes_credit: Option<String>,
}

impl Terrain {
    /// The GLO-30 files `glo30`, in the order of [`tiles`], with their `credit`, and the reference
    /// archive at `reference`, for heights in `bounds`. Each file is hashed and its header checked.
    /// With an archive, the seam reads only its index here, and decodes a tile at first use.
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
        let files = glo30.iter().map(|(tile, path)| ((tile.lat, tile.lon), path.clone())).collect();
        let reference = reference.map(ReferenceArchive::open).transpose()?.map(Arc::new);
        let seam = match &reference {
            Some(archive) => {
                let held = archive.held_digests(reach(bounds));
                identities.extend(held.iter().map(|(_, _, hash)| hash.to_string()));
                Seam::open(archive.clone(), held.into_iter().map(|(ti, tj, _)| (ti, tj)))?
            }
            None => Seam::default(),
        };
        Ok(Self {
            glo30: files,
            rows: None,
            loaded: BTreeMap::new(),
            fallback: DemMosaic::default(),
            squares: Mutex::default(),
            reference,
            decoded: Mutex::default(),
            seam,
            identities,
            used: Mutex::default(),
            credit: credit.into(),
            shapes: Shapes::default(),
            shapes_credit: None,
        })
    }

    /// The terrain with GLO-30 shaped by the coastlines and summits of OSM, whose credit is
    /// `credit`.
    pub fn shaped(self, (lines, peaks): Features, credit: &str) -> Self {
        let shapes = Shapes::new(lines, peaks);
        let shapes_credit = (!shapes.is_empty()).then(|| credit.to_string());
        Self { shapes, shapes_credit, ..self }
    }

    /// Decode the GLO-30 squares of the latitudes `lats`, at every longitude, and drop every other
    /// square that this decoded. It takes over the squares of `lats` that heights decoded.
    pub fn load(&mut self, lats: RangeInclusive<i32>) -> Result<(), String> {
        self.loaded.retain(|(lat, _), _| lats.contains(lat));
        let squares = self.squares.get_mut().unwrap();
        for (square, tile) in std::mem::take(squares) {
            if lats.contains(&square.0) {
                self.loaded.entry(square).or_insert(tile);
            } else {
                squares.push_back((square, tile));
            }
        }
        let band = self.glo30.range((*lats.start(), i32::MIN)..=(*lats.end(), i32::MAX));
        let missing: Vec<_> = band.filter(|(square, _)| !self.loaded.contains_key(square)).collect();
        let decoded = missing.into_par_iter().map(|(square, path)| Ok((*square, Arc::new(DemTile::open(path)?))));
        self.loaded.extend(decoded.collect::<Result<Vec<_>, String>>()?);
        self.fallback = DemMosaic::default();
        self.loaded.values().for_each(|tile| self.fallback.push(tile.clone()));
        self.rows = Some(lats);
        Ok(())
    }

    /// The height in metres at `lat`, `lon` in microdegrees, or `None` where no source has one.
    pub fn height(&self, lat: i32, lon: i32, recent: &mut Recent) -> Result<Option<f64>, String> {
        let shaped = |mosaic: &DemMosaic| {
            self.shapes.height(lat as f64 * 1e-6, lon as f64 * 1e-6, &|lat, lon| mosaic.height(lat, lon))
        };
        let band = squares(lat);
        let loaded = self.rows.as_ref().is_some_and(|rows| rows.contains(band.start()) && rows.contains(band.end()));
        let glo30 = || match loaded {
            true => Ok(shaped(&self.fallback)),
            false => self.glo30_around(lat, lon).map(|mosaic| shaped(&mosaic)),
        };
        let Some(archive) = &self.reference else { return glo30() };
        let national = self.national(archive, lat, lon, recent)?;
        let (depth, bias) = self.seam.at(lat, lon, recent)?;
        // Where the archive holds the ground alone, GLO-30 has no weight.
        let glo30 = if national.is_some() && depth >= FEATHER_M { None } else { glo30()? };
        Ok(blend(national, glo30, depth, bias))
    }

    /// The mosaic of the GLO-30 squares that a height at `lat`, `lon` in microdegrees can read.
    /// There it gives the heights of a mosaic of every square: a mosaic answers from the one tile
    /// that owns a point.
    fn glo30_around(&self, lat: i32, lon: i32) -> Result<DemMosaic, String> {
        let mut mosaic = DemMosaic::default();
        let lons = columns(lat, lon);
        for lat in squares(lat) {
            for (square, path) in self.glo30.range((lat, *lons.start())..=(lat, *lons.end())) {
                mosaic.push(match self.loaded.get(square) {
                    Some(tile) => tile.clone(),
                    None => self.decode(*square, path)?,
                });
            }
        }
        Ok(mosaic)
    }

    /// GLO-30 square `square`, from the squares that heights decoded, else decoded. It decodes
    /// under the lock, so that two threads do not decode one square twice.
    fn decode(&self, square: Square, path: &Path) -> Result<Arc<DemTile>, String> {
        let mut squares = self.squares.lock().unwrap();
        if let Some((_, tile)) = squares.iter().find(|(id, _)| *id == square) {
            return Ok(tile.clone());
        }
        let tile = Arc::new(DemTile::open(path)?);
        squares.push_back((square, tile.clone()));
        if squares.len() > SQUARES {
            squares.pop_front();
        }
        Ok(tile)
    }

    /// How many squares heights decoded and keep, besides those of [`Terrain::load`].
    pub fn decoded_squares(&self) -> usize {
        self.squares.lock().unwrap().len()
    }

    /// Whether the archive holds the ground somewhere in `bounds` (west, south, east, north in
    /// degrees): its weight is above 0 in a coverage cell there.
    pub fn national_in(&self, [west, south, east, north]: [f64; 4]) -> Result<bool, String> {
        let cell = |degrees: f64| ((degrees * 1e6 - ORIGIN as f64) / CELL_UDEG).floor() as i64;
        let mut recent = Recent::default();
        for i in cell(south)..=cell(north) {
            for j in cell(west)..=cell(east) {
                if self.seam.cell(i, j, &mut recent)?[0] > 0.0 {
                    return Ok(true);
                }
            }
        }
        Ok(false)
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
        recall(&mut recent.tiles, key, RECENT, || self.shared(archive, key))
    }

    fn shared(&self, archive: &ReferenceArchive, key: (u32, u32)) -> Result<Option<Arc<ReferenceTile>>, String> {
        if let Some(tile) = find(&self.decoded.lock().unwrap(), key) {
            return Ok(tile);
        }
        let tile = match archive.tile(key.0, key.1)? {
            TileLookup::Held { tile, sources } => {
                self.used.lock().unwrap().extend(sources.iter().cloned());
                Some(Arc::new(tile))
            }
            TileLookup::Absent | TileLookup::Unknown => None,
        };
        Ok(keep(&mut self.decoded.lock().unwrap(), key, tile, DECODED))
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
            credits.extend(self.shapes_credit.clone());
        }
        credits
    }
}

/// The height in the seam, from the archive height, the GLO-30 height, the depth of the point in
/// the archive and the bias of GLO-30.
pub fn blend(national: Option<f64>, glo30: Option<f64>, depth: f64, bias: f64) -> Option<f64> {
    let weight = national.map_or(0.0, |_| (depth / FEATHER_M).clamp(0.0, 1.0));
    let corrected = glo30.map(|height| height + bias * (1.0 + depth / FEATHER_M).clamp(0.0, 1.0));
    match (national, corrected) {
        (Some(national), Some(corrected)) => Some(weight * national + (1.0 - weight) * corrected),
        (national, corrected) => national.or(corrected),
    }
}

/// The bias of each national model: the median of model − GLO-30 in metres, to the centimetre,
/// over its samples, with GLO-30 from the squares of `glo30`, which must hold the square of each
/// sample. A sample without a GLO-30 height does not count. A model with fewer than
/// [`MIN_SAMPLES`] has no bias. Each thread decodes one square at a time.
pub fn biases(
    samples: &BTreeMap<String, Vec<Sample>>,
    glo30: &[(TileId, PathBuf)],
) -> Result<BTreeMap<String, f64>, String> {
    let mut by_square: BTreeMap<(i32, i32), Vec<(&str, Sample)>> = BTreeMap::new();
    for (key, points) in samples {
        for &(lat, lon, metres) in points {
            let square = (lat.div_euclid(1_000_000) as i32, lon.div_euclid(1_000_000) as i32);
            by_square.entry(square).or_default().push((key, (lat, lon, metres)));
        }
    }
    let files: BTreeMap<(i32, i32), &PathBuf> = glo30.iter().map(|(tile, path)| ((tile.lat, tile.lon), path)).collect();
    let squares = by_square.into_iter().map(|((lat, lon), points)| match files.get(&(lat, lon)) {
        Some(path) => Ok((*path, points)),
        None => Err(format!("no GLO-30 square {} for {} bias samples", TileId { lat, lon }.stem(), points.len())),
    });
    let squares = squares.collect::<Result<Vec<_>, String>>()?;
    let differences = squares
        .into_par_iter()
        .map(|(path, points)| {
            let mut mosaic = DemMosaic::default();
            mosaic.push(DemTile::open(path)?);
            let glo30 = |lat: i64, lon: i64| mosaic.height(lat as f64 * 1e-6, lon as f64 * 1e-6);
            Ok(points
                .into_iter()
                .filter_map(|(key, (lat, lon, metres))| Some((key, metres - glo30(lat, lon)?)))
                .collect())
        })
        .collect::<Result<Vec<Vec<_>>, String>>()?;
    let mut by_model: BTreeMap<&str, Vec<f64>> = samples.keys().map(|key| (key.as_str(), Vec::new())).collect();
    for (key, difference) in differences.into_iter().flatten() {
        by_model.get_mut(key).expect("a model of the samples").push(difference);
    }
    let mut measured = BTreeMap::new();
    for (key, mut values) in by_model {
        let n = values.len();
        if n < MIN_SAMPLES {
            eprintln!("GLO-30 bias of `{key}`: {n} samples with GLO-30, fewer than {MIN_SAMPLES}: no bias");
            continue;
        }
        let bias = (*values.select_nth_unstable_by(n / 2, f64::total_cmp).1 * 100.0).round() / 100.0;
        eprintln!("GLO-30 bias of `{key}`: {bias:.2} m from {n} samples");
        measured.insert(key.to_string(), bias);
    }
    Ok(measured)
}

/// The bias samples of archive tile `(ti, tj)`, from `height` of pixel `(r, c)`, `r` counting north.
pub fn samples(ti: u32, tj: u32, height: impl Fn(usize, usize) -> Option<f64>) -> Vec<Sample> {
    let (lat0, lon0) = (ORIGIN + i64::from(ti) * TILE_UDEG, ORIGIN + i64::from(tj) * TILE_UDEG);
    let pixels = || (SAMPLE..TILE_PIXELS).step_by(SAMPLES);
    let centre = |origin: i64, pixel: usize| origin + pixel as i64 * STEP + STEP / 2;
    let points = pixels().flat_map(|r| pixels().map(move |c| (r, c)));
    points.filter_map(|(r, c)| Some((centre(lat0, r), centre(lon0, c), height(r, c)?))).collect()
}

/// The coverage cells of an archive tile: one bit for each cell, row by row, set where the archive
/// has each of its pixels; and the bias of the best model of the tile.
#[derive(Clone)]
struct Coverage {
    full: [u64; CELLS],
    bias: f32,
}
const _: () = assert!(CELLS == u64::BITS as usize);

impl Coverage {
    fn is_full(&self, r: usize, c: usize) -> bool {
        self.full[r] >> c & 1 == 1
    }
}

/// The full cells of an archive tile, as [`Coverage::full`].
fn full_cells(tile: &ReferenceTile) -> [u64; CELLS] {
    let mut cells = vec![0u16; CELLS * CELLS];
    for r in 0..TILE_PIXELS {
        for c in 0..TILE_PIXELS {
            cells[(r >> CELL_LOG2) * CELLS + (c >> CELL_LOG2)] += u16::from(tile.height(r, c).is_some());
        }
    }
    let mut full = [0u64; CELLS];
    for (at, &count) in cells.iter().enumerate() {
        full[at / CELLS] |= u64::from(count == 1 << (2 * CELL_LOG2)) << (at % CELLS);
    }
    full
}

/// The full cells of a held archive tile, or `None` where the archive does not hold it.
type FullCells = Box<dyn Fn(u32, u32) -> Result<Option<[u64; CELLS]>, String> + Send + Sync>;

/// The depth and the bias of each coverage cell. The seam computes the depths of a tile at first
/// use, from the coverages of its 3 × 3 block, and keeps the last ones. A tile without depths is
/// [`FEATHER_M`] or more outside.
struct Seam {
    /// The bias of the best model of each held tile.
    held: BTreeMap<(u32, u32), f32>,
    full: FullCells,
    cache: Mutex<SeamCache>,
}

/// `None` is a tile without a coverage, or without depths.
#[derive(Default)]
struct SeamCache {
    coverages: Fifo<Option<Coverage>>,
    depths: Fifo<Option<Depths>>,
}

impl Default for Seam {
    fn default() -> Self {
        Self::new(BTreeMap::new(), |_, _| Ok(None))
    }
}

impl Seam {
    /// The seam of the archive tiles `tiles` that the archive holds. It reads only the index: it
    /// decodes no tile.
    fn open(archive: Arc<ReferenceArchive>, tiles: impl IntoIterator<Item = (u32, u32)>) -> Result<Self, String> {
        let (mut held, mut used) = (BTreeMap::new(), BTreeMap::new());
        for (ti, tj) in tiles {
            let Some((best, bias)) = archive.bias(ti, tj) else { continue };
            let bias = bias.ok_or_else(|| {
                format!(
                    "the archive index states no `glo30_bias_m` of `{best}`, the best model of tile {ti}/{tj}. \
                     Fix: {BIAS_FIX}"
                )
            })? as f32;
            held.insert((ti, tj), bias);
            used.insert(best.to_string(), bias);
        }
        for (key, bias) in used {
            eprintln!("GLO-30 bias {bias:.2} m next to `{key}`");
        }
        // Not through `Terrain::shared`, so a tile that only the seam reads adds no credit.
        Ok(Self::new(held, move |ti, tj| match archive.tile(ti, tj)? {
            TileLookup::Held { tile, .. } => Ok(Some(full_cells(&tile))),
            TileLookup::Absent | TileLookup::Unknown => Ok(None),
        }))
    }

    /// The seam of the tiles of `held`, with the bias of each, whose full cells `full` gives.
    fn new(
        held: BTreeMap<(u32, u32), f32>,
        full: impl Fn(u32, u32) -> Result<Option<[u64; CELLS]>, String> + Send + Sync + 'static,
    ) -> Self {
        Self { held, full: Box::new(full), cache: Mutex::default() }
    }

    /// The depth and the bias of the coverage cell `(i, j)`, counted from the lattice origin.
    fn cell(&self, i: i64, j: i64, recent: &mut Recent) -> Result<[f64; 2], String> {
        let size = CELLS as i64;
        let at = (i.rem_euclid(size) * size + j.rem_euclid(size)) as usize;
        let tile = (i.div_euclid(size) as u32, j.div_euclid(size) as u32);
        let depths = recall(&mut recent.depths, tile, RECENT_DEPTHS, || self.depths(tile))?;
        Ok(depths.map_or([-FEATHER_M, 0.0], |cells| cells[at].map(f64::from)))
    }

    /// The depths of tile `(ti, tj)`, or `None` where each is at or below −[`FEATHER_M`]. The lock
    /// covers only the look-ups and the inserts, so the parallel heights do not wait for a decode.
    fn depths(&self, (ti, tj): (u32, u32)) -> Result<Option<Depths>, String> {
        let around = |t: u32| (i64::from(t) - 1..=i64::from(t) + 1).filter_map(|t| u32::try_from(t).ok());
        let block: Vec<_> = around(ti).flat_map(|i| around(tj).map(move |j| (i, j))).collect();
        if !block.iter().any(|tile| self.held.contains_key(tile)) {
            return Ok(None);
        }
        if let Some(depths) = find(&self.cache.lock().unwrap().depths, (ti, tj)) {
            return Ok(depths);
        }
        let mut coverages = BTreeMap::new();
        for tile in block {
            if let Some(coverage) = self.coverage(tile)? {
                coverages.insert(tile, coverage);
            }
        }
        let cells = depths(|tile| coverages.get(&tile), ti, tj);
        let depths = cells.iter().any(|&[depth, _]| f64::from(depth) > -FEATHER_M).then(|| Depths::from(cells));
        Ok(keep(&mut self.cache.lock().unwrap().depths, (ti, tj), depths, DEPTHS))
    }

    /// The coverage of tile `tile`, or `None` where it is not held or has no best model.
    fn coverage(&self, tile: (u32, u32)) -> Result<Option<Coverage>, String> {
        let Some(&bias) = self.held.get(&tile) else { return Ok(None) };
        if let Some(coverage) = find(&self.cache.lock().unwrap().coverages, tile) {
            return Ok(coverage);
        }
        let coverage = (self.full)(tile.0, tile.1)?.map(|full| Coverage { full, bias });
        Ok(keep(&mut self.cache.lock().unwrap().coverages, tile, coverage, COVERAGES))
    }

    /// The depth and the bias at a point, bilinear between the centres of the coverage cells. Where
    /// the archive misses a pixel, the four cells around a point are full cells next to the edge or
    /// not full, so the depth is 0 or less.
    fn at(&self, lat: i32, lon: i32, recent: &mut Recent) -> Result<(f64, f64), String> {
        let cell = |udeg: i32| (f64::from(udeg) - ORIGIN as f64) / CELL_UDEG - 0.5;
        let (y, x) = (cell(lat), cell(lon));
        let (i, j, fy, fx) = (y.floor() as i64, x.floor() as i64, y - y.floor(), x - x.floor());
        let mix = |a: [f64; 2], b: [f64; 2], f: f64| [0, 1].map(|k| a[k] * (1.0 - f) + b[k] * f);
        let south = mix(self.cell(i, j, recent)?, self.cell(i, j + 1, recent)?, fx);
        let north = mix(self.cell(i + 1, j, recent)?, self.cell(i + 1, j + 1, recent)?, fx);
        let [depth, bias] = mix(south, north, fy);
        Ok((depth, bias))
    }
}

/// The depth and the bias of each coverage cell of tile `(ti, tj)`, from the tile and its eight
/// neighbours. The depth is the distance between cell centres to the nearest cell on the other side
/// of the edge, less the diagonal of a cell, at most [`FEATHER_M`]; negative outside the full cells.
/// A full cell has the bias of its tile, and a cell outside the bias of the nearest full cell.
fn depths<'c>(coverage: impl Fn((u32, u32)) -> Option<&'c Coverage>, ti: u32, tj: u32) -> Vec<[f32; 2]> {
    const SIDE: usize = 3 * CELLS;
    let lat = (ORIGIN + i64::from(ti) * TILE_UDEG + TILE_UDEG / 2) as f64 * 1e-6;
    let (height, width) = (CELL_UDEG * M_PER_UDEG, CELL_UDEG * M_PER_UDEG * lat.to_radians().cos());
    let diagonal = height.hypot(width);
    let reach = FEATHER_M + diagonal;
    let (rows, cols) = ((reach / height).ceil() as usize, (reach / width).ceil() as usize);
    assert!(rows < CELLS && cols < CELLS, "the seam is narrower than a tile");
    // The full cells of the 3 × 3 block of tiles.
    let tile = |r: usize, c: usize| (ti + (r / CELLS) as u32 - 1, tj + (c / CELLS) as u32 - 1);
    let mut block = vec![false; SIDE * SIDE];
    for (r, row) in block.chunks_mut(SIDE).enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = coverage(tile(r, c)).is_some_and(|cells| cells.is_full(r % CELLS, c % CELLS));
        }
    }
    let mut out = vec![[0f32; 2]; CELLS * CELLS];
    for r in CELLS..2 * CELLS {
        for c in CELLS..2 * CELLS {
            let inside = block[r * SIDE + c];
            let mut nearest = (reach, (ti, tj));
            for dr in r - rows..=r + rows {
                for dc in c - cols..=c + cols {
                    if block[dr * SIDE + dc] != inside {
                        let metres = ((dr as f64 - r as f64) * height).hypot((dc as f64 - c as f64) * width);
                        if metres < nearest.0 {
                            nearest = (metres, tile(dr, dc));
                        }
                    }
                }
            }
            let depth = (nearest.0 - diagonal).max(0.0);
            let bias = coverage(if inside { (ti, tj) } else { nearest.1 }).map_or(0.0, |cells| cells.bias);
            out[(r - CELLS) * CELLS + c - CELLS] = [if inside { depth } else { -depth } as f32, bias];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seam of the tiles of `coverages`, built as [`Seam::open`] builds it.
    fn seam(coverages: BTreeMap<(u32, u32), Coverage>) -> Seam {
        let held = coverages.iter().map(|(&tile, coverage)| (tile, coverage.bias)).collect();
        Seam::new(held, move |ti, tj| Ok(coverages.get(&(ti, tj)).map(|coverage| coverage.full)))
    }

    const FULL: Coverage = Coverage { full: [u64::MAX; CELLS], bias: -2.0 };

    /// Across the edge, the seam meets both sources with no step: GLO-30 with its bias at depth 0,
    /// and the archive alone at `FEATHER_M`.
    #[test]
    fn the_seam_feathers_the_archive_into_glo30_with_its_bias() {
        let (national, glo30, bias) = (Some(500.0), Some(503.0), -2.0);
        let at = |depth: f64| blend(national, glo30, depth, bias);
        assert_eq!(at(-FEATHER_M), Some(503.0), "far outside, GLO-30 as it is");
        assert_eq!(at(-FEATHER_M / 2.0), Some(502.0), "the bias fades in");
        assert_eq!(at(0.0), Some(501.0), "at the edge, GLO-30 with its bias");
        assert_eq!(at(FEATHER_M / 2.0), Some(500.5));
        assert_eq!(at(FEATHER_M), Some(500.0), "inside, the archive alone");
        // Where one source has no height, the other answers.
        assert_eq!(blend(None, glo30, FEATHER_M, bias), Some(501.0));
        assert_eq!(blend(national, None, -FEATHER_M, bias), Some(500.0));
        assert_eq!(blend(None, None, 0.0, bias), None);
    }

    /// Where the archive stops at a concave edge, as at a national border, its weight is 0 and the
    /// height has no step. The archive covers three tiles in an L; the fourth has no height.
    #[test]
    fn the_seam_has_no_step_where_the_archive_stops_at_a_concave_edge() {
        let tiles = [(4810, 4224), (4810, 4225), (4811, 4224)];
        let seam = seam(tiles.into_iter().map(|tile| (tile, FULL)).collect());
        let mut recent = Recent::default();
        let corner = |tile: u32| (ORIGIN + i64::from(tile) * TILE_UDEG) as i32;
        let (edge, east) = (corner(4811), corner(4225));
        // North across the edge, 300 m east of the inner corner, in steps of 2 µdeg (0.2 m).
        let lon = east + (300.0 / (M_PER_UDEG * 46.8f64.to_radians().cos())) as i32;
        let heights: Vec<f64> = (edge - 20_000..edge + 20_000)
            .step_by(2)
            .map(|lat| {
                let (depth, bias) = seam.at(lat, lon, &mut recent).unwrap();
                blend((lat < edge).then_some(500.0), Some(520.0), depth, bias).unwrap()
            })
            .collect();
        let step = heights.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f64::max);
        assert!(step < 0.01, "a step of {step} m at the edge");
        let (inside, bias) = seam.at(corner(4810) + 30_000, corner(4224) + 30_000, &mut recent).unwrap();
        assert_eq!(blend(Some(500.0), Some(520.0), inside, bias), Some(500.0), "the archive alone");
    }

    /// A held tile with no full cell and no full neighbour has its bias, but no depth above
    /// −`FEATHER_M`: it reads as a tile far outside.
    #[test]
    fn a_held_tile_without_depth_reads_as_outside() {
        let (ti, tj) = (4810, 4224);
        let seam = seam(BTreeMap::from([((ti, tj), Coverage { full: [0; CELLS], bias: -2.0 })]));
        let (mut recent, size) = (Recent::default(), CELLS as i64);
        for i in i64::from(ti) * size..(i64::from(ti) + 1) * size {
            for j in i64::from(tj) * size..(i64::from(tj) + 1) * size {
                assert_eq!(seam.cell(i, j, &mut recent).unwrap(), [-FEATHER_M, 0.0], "cell {i}/{j}");
            }
        }
    }

    #[test]
    fn the_seam_keeps_at_most_its_cache() {
        let tiles = (0..300).map(|k| ((4810, 4000 + k), FULL));
        let seam = seam(tiles.collect());
        let mut recent = Recent::default();
        for k in 0..300 {
            seam.cell(4810 * CELLS as i64, (4000 + k) * CELLS as i64, &mut recent).unwrap();
        }
        assert_eq!(seam.cache.lock().unwrap().depths.len(), DEPTHS);
        assert_eq!(recent.depths.len(), RECENT_DEPTHS);
        // The coverages use the same `keep`: it drops the oldest, and the first value of a key stays.
        let mut fifo = Fifo::new();
        for k in 0..3 {
            keep(&mut fifo, (0, k), k, 2);
        }
        assert_eq!(keep(&mut fifo, (0, 2), 9, 2), 2);
        assert_eq!(fifo, [((0, 1), 1), ((0, 2), 2)]);
    }
}
