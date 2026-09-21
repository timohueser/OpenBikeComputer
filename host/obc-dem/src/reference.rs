//! The reference archive: max-pooled bare-earth height on the OBCT lattice, as `int16` GeoTIFF
//! tiles that a bake streams one at a time.
//!
//! A country-sized reference does not fit in memory, so the archive is addressed rather than
//! loaded. Its lattice is the OBCT lattice at `2^6` µdeg (≈ 7.1 × 4.9 m at 47 °N) and its tiles are
//! `2^16` µdeg squares of 1024 × 1024 pixels, so **a tile id is arithmetic on a coordinate** and a
//! bake asks for the hundred tiles one cell's rule reads and nothing else.
//!
//! Two properties of the format are what make a 7 m archive usable as the §9 reference:
//!
//! * **A pixel is a maximum, not a sample.** Ingest keeps the largest source height whose pixel
//!   centre falls inside the pixel's square, so a rock tower one source pixel wide survives the
//!   pooling. `−32768` means no source pixel reached the square, and the contract permits no other
//!   nodata.
//! * **The transform is the lattice, exactly.** There is no reprojection and no resampling here:
//!   [`ReferenceTile`] holds every tile to the transform its id implies, to `1e-9`°, and refuses the
//!   file by name otherwise. A tile that survives that check needs no geometry reasoning downstream
//!   — a pixel centre is one integer expression.
//!
//! `index.json` says which tiles the archive has and who contributed to each of them. A mirror of
//! one box carries the whole index and only its own tiles, so a tile the index names and the disk
//! does not hold is **absent**, not an error; a tile that is on disk but broken is an error.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use obc_formats::obct::{GRID_ORIGIN, WORLD_SIDE};
use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

use crate::geotiff::geo_keys;

/// Archive lattice step as `log2(µdeg)`: `2^6` µdeg on both axes.
pub const STEP_LOG2: u8 = 6;
/// Archive tile side as `log2(µdeg)`: `2^16` µdeg, which is 1024 pixels.
pub const TILE_LOG2: u8 = 16;
/// Pixels along one tile edge.
pub const TILE_PIXELS: usize = 1 << (TILE_LOG2 - STEP_LOG2);
/// "No source pixel reached this square" — the archive's one nodata value.
pub const NO_PIXEL: i16 = i16::MIN;

const STEP: i64 = 1 << STEP_LOG2;
const TILE_UDEG: i64 = 1 << TILE_LOG2;
const ORIGIN: i64 = GRID_ORIGIN as i64;
/// Tiles along one axis of the world box, so a tile id is bounded before it becomes a path.
const TILES_PER_AXIS: i64 = WORLD_SIDE as i64 >> TILE_LOG2;
/// Transform tolerance in degrees, as the tile contract states it. A microdegree is `1e-6`, so this
/// admits floating-point noise and nothing an operator could have meant.
const TRANSFORM_EPS: f64 = 1e-9;

/// A half-open µdeg box: the archive pixels one bake reads.
///
/// Half-open because it is a union of half-posting node cells, which partition the plane the same
/// way the archive's own pixels do — a pixel centre on the box's high edge belongs to the next node,
/// and so to the next bake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub lat_lo: i64,
    pub lat_hi: i64,
    pub lon_lo: i64,
    pub lon_hi: i64,
}

impl Window {
    /// The tiles this window intersects, tile-major (`ti` then `tj`) so a bake's pass over them is
    /// one fixed order. Tiles outside the world box are not named: the lattice does not wrap, so a
    /// window that reaches past the antimeridian or a pole simply reads fewer tiles.
    pub fn tiles(&self) -> impl Iterator<Item = (u32, u32)> + Clone {
        let rows = tile_span(self.lat_lo, self.lat_hi);
        let cols = tile_span(self.lon_lo, self.lon_hi);
        rows.flat_map(move |ti| cols.clone().map(move |tj| (ti, tj)))
    }
}

/// The tile indices a half-open µdeg span touches, clipped to the world box.
fn tile_span(lo: i64, hi: i64) -> std::ops::Range<u32> {
    if hi <= lo {
        return 0..0;
    }
    let first = (lo - ORIGIN).div_euclid(TILE_UDEG).max(0);
    let last = (hi - 1 - ORIGIN).div_euclid(TILE_UDEG).min(TILES_PER_AXIS - 1);
    first as u32..(last + 1).max(first) as u32
}

/// One archive tile, decoded: 1024 × 1024 whole metres with `r` counting **north**.
///
/// The GeoTIFF stores rows north-up; the flip happens here, once, exactly as `DemTile` does it, so
/// no index downstream of this module has to decide which way is north.
pub struct ReferenceTile {
    ti: u32,
    tj: u32,
    pixels: Vec<i16>,
}

impl std::fmt::Debug for ReferenceTile {
    /// The heights are two million numbers, so a tile prints as what it is: an id.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ReferenceTile({}/{})", self.ti, self.tj)
    }
}

impl ReferenceTile {
    /// The height at pixel `(r, c)`, or `None` where no source pixel reached it.
    pub fn pixel(&self, r: usize, c: usize) -> Option<i16> {
        let value = *self.pixels.get(r * TILE_PIXELS + c)?;
        (value != NO_PIXEL).then_some(value)
    }

    /// The south-west corner of pixel `(0, 0)`, in µdeg — this tile's place on the lattice.
    pub fn origin_udeg(&self) -> (i64, i64) {
        (ORIGIN + i64::from(self.ti) * TILE_UDEG, ORIGIN + i64::from(self.tj) * TILE_UDEG)
    }

    /// Every pixel of this tile that has a height and whose **centre** lies in `window`, as
    /// `(lat, lon, metres)` in µdeg, row-major with `r` counting north.
    pub fn centres_in(&self, window: Window, mut visit: impl FnMut(i64, i64, i16)) {
        let (lat0, lon0) = self.origin_udeg();
        let rows = centre_span(window.lat_lo, window.lat_hi, lat0);
        let cols = centre_span(window.lon_lo, window.lon_hi, lon0);
        for r in rows {
            let lat = lat0 + r as i64 * STEP + STEP / 2;
            let row = &self.pixels[r * TILE_PIXELS..(r + 1) * TILE_PIXELS];
            for c in cols.clone() {
                let height = row[c];
                if height != NO_PIXEL {
                    visit(lat, lon0 + c as i64 * STEP + STEP / 2, height);
                }
            }
        }
    }

    /// Decode and validate the tile at `path`, which must be the tile the id names.
    fn open(path: &Path, ti: u32, tj: u32) -> Result<ReferenceTile, String> {
        let name = || path.display().to_string();
        let file = File::open(path).map_err(|e| format!("{}: {e}", name()))?;
        // The `tiff` crate's default limits allow a 256 MiB decoding buffer; a tile is
        // 1024 × 1024 × 2 B = 2 MiB, so the archive never approaches them.
        let mut dec =
            Decoder::new(BufReader::new(file)).map_err(|e| format!("{}: not a readable TIFF ({e})", name()))?;

        let (width, height) = dec.dimensions().map_err(|e| format!("{}: no dimensions ({e})", name()))?;
        if (width as usize, height as usize) != (TILE_PIXELS, TILE_PIXELS) {
            return Err(format!("{}: {width}×{height} — an archive tile is {TILE_PIXELS}²", name()));
        }
        if let Ok(bands) = dec.get_tag_u32(Tag::SamplesPerPixel) {
            if bands != 1 {
                return Err(format!("{}: {bands} bands — an archive tile has one", name()));
            }
        }
        expect_transform(&mut dec, ti, tj, &name)?;
        if let Ok(text) = dec.get_tag_ascii_string(Tag::GdalNodata) {
            let declared = text.trim().trim_end_matches('\0').parse::<f64>().ok();
            if declared != Some(f64::from(NO_PIXEL)) {
                return Err(format!("{}: nodata {text:?} — the archive's nodata is {NO_PIXEL}", name()));
            }
        }

        let north_up = match dec.read_image().map_err(|e| format!("{}: decode failed ({e})", name()))? {
            DecodingResult::I16(values) => values,
            _ => return Err(format!("{}: not int16 — the archive stores whole metres as int16", name())),
        };
        if north_up.len() != TILE_PIXELS * TILE_PIXELS {
            return Err(format!("{}: decoded {} samples for a {TILE_PIXELS}² raster", name(), north_up.len()));
        }
        let mut pixels = vec![0i16; north_up.len()];
        for r in 0..TILE_PIXELS {
            let src = &north_up[(TILE_PIXELS - 1 - r) * TILE_PIXELS..(TILE_PIXELS - r) * TILE_PIXELS];
            pixels[r * TILE_PIXELS..(r + 1) * TILE_PIXELS].copy_from_slice(src);
        }
        Ok(ReferenceTile { ti, tj, pixels })
    }
}

/// The pixel indices of a tile whose centres lie in a half-open µdeg span, given the tile's own
/// origin on that axis.
fn centre_span(lo: i64, hi: i64, origin: i64) -> std::ops::Range<usize> {
    let index = |bound: i64| ceil_div(bound - origin - STEP / 2, STEP).clamp(0, TILE_PIXELS as i64) as usize;
    let (first, last) = (index(lo), index(hi));
    first..last.max(first)
}

fn ceil_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b) + i64::from(a.rem_euclid(b) != 0)
}

/// Hold a tile's georeferencing to the lattice its id implies, and refuse the file by name.
///
/// Every part of it is checked, because each one moves ground: a wrong scale stretches the tile, a
/// wrong tie point slides it, `PixelIsPoint` shifts it half a pixel, and a projected raster is not
/// on this lattice at all.
fn expect_transform<R: std::io::Read + std::io::Seek>(
    dec: &mut Decoder<R>,
    ti: u32,
    tj: u32,
    name: &impl Fn() -> String,
) -> Result<(), String> {
    let tie = dec
        .get_tag_f64_vec(Tag::ModelTiepointTag)
        .map_err(|_| format!("{}: no ModelTiepointTag — not a georeferenced raster", name()))?;
    let scale = dec
        .get_tag_f64_vec(Tag::ModelPixelScaleTag)
        .map_err(|_| format!("{}: no ModelPixelScaleTag — not a north-up lattice raster", name()))?;
    if tie.len() < 6 || scale.len() < 2 {
        return Err(format!("{}: short ModelTiepointTag/ModelPixelScaleTag", name()));
    }
    if tie[0] != 0.0 || tie[1] != 0.0 {
        return Err(format!("{}: ModelTiepointTag is not anchored on raster (0, 0)", name()));
    }
    // A `PixelIsArea` tie point names the raster's north-west **corner**, which for tile (ti, tj) is
    // the lattice line the id puts it on.
    let step = STEP as f64 / 1e6;
    let want_lon = (ORIGIN + i64::from(tj) * TILE_UDEG) as f64 / 1e6;
    let want_lat = (ORIGIN + (i64::from(ti) + 1) * TILE_UDEG) as f64 / 1e6;
    for (what, actual, want) in
        [("pixel scale (lon)", scale[0], step), ("pixel scale (lat)", scale[1], step), ("origin lon", tie[3], want_lon)]
    {
        if (actual - want).abs() > TRANSFORM_EPS {
            return Err(format!("{}: {what} {actual} — tile {ti}/{tj} is {want} on the lattice", name()));
        }
    }
    if (tie[4] - want_lat).abs() > TRANSFORM_EPS {
        return Err(format!("{}: origin lat {} — tile {ti}/{tj} is {want_lat} on the lattice", name(), tie[4]));
    }

    let keys = geo_keys(dec);
    let key = |code: u16| keys.iter().find(|(k, _)| *k == code).map(|(_, v)| *v);
    match key(1024) {
        Some(2) | None => {}
        Some(other) => return Err(format!("{}: GTModelTypeGeoKey {other} — the lattice is geographic", name())),
    }
    match key(2048) {
        Some(4326) | None => {}
        Some(other) => return Err(format!("{}: GeographicTypeGeoKey {other} — the lattice is WGS 84", name())),
    }
    match key(1025) {
        Some(1) | None => Ok(()),
        Some(other) => Err(format!("{}: GTRasterTypeGeoKey {other} — an archive pixel is an area (1)", name())),
    }
}

/// An archive root, or a mirror of one: its `index.json` read, its tiles left on disk.
pub struct ReferenceArchive {
    root: PathBuf,
    /// Every source with a surviving pixel in a tile, best first — `index.json`'s `contributors`,
    /// which is what attribution reads.
    contributors: BTreeMap<(u32, u32), Vec<String>>,
}

impl ReferenceArchive {
    /// Read an archive's index. The tiles stay on disk until [`tile`](Self::tile) asks for one.
    pub fn open(root: &Path) -> Result<ReferenceArchive, String> {
        let path = root.join("index.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let index: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: not readable JSON ({e})", path.display()))?;
        let name = || path.display().to_string();

        for (field, want) in
            [("schema", 1u64), ("step_log2", u64::from(STEP_LOG2)), ("tile_log2", u64::from(TILE_LOG2))]
        {
            match index.get(field).and_then(serde_json::Value::as_u64) {
                Some(value) if value == want => {}
                Some(value) => return Err(format!("{}: {field} {value} — this reader reads {want}", name())),
                None => return Err(format!("{}: no {field} — not a reference archive index", name())),
            }
        }
        let tiles = index
            .get("tiles")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| format!("{}: no `tiles` map", name()))?;
        // `contributors` lists every source with a surviving pixel in a tile; `tiles` names only the
        // best-priority one. An index written before `contributors` existed has just the one, and a
        // one-source tile is the common case anyway.
        let listed = index.get("contributors").and_then(serde_json::Value::as_object);

        let mut contributors = BTreeMap::new();
        for (id, best) in tiles {
            let key = tile_id(id).ok_or_else(|| format!("{}: `{id}` is not a tile id", name()))?;
            let sources = match listed.and_then(|map| map.get(id)).and_then(serde_json::Value::as_array) {
                Some(array) => array.iter().filter_map(|v| v.as_str()).map(str::to_string).collect(),
                None => best.as_str().map(str::to_string).into_iter().collect(),
            };
            contributors.insert(key, sources);
        }
        Ok(ReferenceArchive { root: root.to_path_buf(), contributors })
    }

    /// Tiles the index names.
    pub fn len(&self) -> usize {
        self.contributors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contributors.is_empty()
    }

    /// Decode tile `(ti, tj)`, or `None` when this archive does not hold it.
    ///
    /// A tile the index names but the disk does not hold is `None`: that is what a mirror of one box
    /// looks like, since `mirror` copies the whole index and the tiles of its box alone. A tile that
    /// is on disk and does not hold the contract is an error naming the file.
    pub fn tile(&self, ti: u32, tj: u32) -> Result<Option<ReferenceTile>, String> {
        if !self.contributors.contains_key(&(ti, tj)) {
            return Ok(None);
        }
        let path = self.root.join(format!("{TILE_LOG2}/{ti:04}/{tj:04}.tif"));
        if !path.is_file() {
            return Ok(None);
        }
        ReferenceTile::open(&path, ti, tj).map(Some)
    }

    /// Every source that contributed a pixel to a tile this window reads, sorted — the attribution
    /// a container baked over this window must carry.
    pub fn sources_for(&self, window: Window) -> Vec<&str> {
        let mut keys = BTreeSet::new();
        for id in window.tiles() {
            for source in self.contributors.get(&id).into_iter().flatten() {
                keys.insert(source.as_str());
            }
        }
        keys.into_iter().collect()
    }
}

/// `"<ti>/<tj>"` as a tile id, or `None` when it names no square of the world box.
fn tile_id(text: &str) -> Option<(u32, u32)> {
    let (ti, tj) = text.split_once('/')?;
    let index = |part: &str| part.parse::<u32>().ok().filter(|&t| i64::from(t) < TILES_PER_AXIS);
    Some((index(ti)?, index(tj)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile id is arithmetic on a coordinate, and the Engelberg archive's own ids are the pin: the
    /// tile holding 46.75 N 8.30 E is `4809/4222`, and its transform is what the reader demands.
    #[test]
    fn a_tile_id_is_the_coordinate_shifted_onto_the_lattice() {
        let lat = 46_750_000i64;
        let lon = 8_300_000i64;
        assert_eq!(((lat - ORIGIN) >> TILE_LOG2, (lon - ORIGIN) >> TILE_LOG2), (4809, 4222));
        // …and the window of a µdeg box holds exactly the tiles that box touches.
        let one = Window { lat_lo: lat, lat_hi: lat + 1, lon_lo: lon, lon_hi: lon + 1 };
        assert_eq!(one.tiles().collect::<Vec<_>>(), vec![(4809, 4222)]);

        // A box that ends exactly on a tile line does not reach the next tile: the box is half-open.
        let line = ORIGIN + 4810 * TILE_UDEG;
        let upto = Window { lat_lo: line - 1, lat_hi: line, lon_lo: lon, lon_hi: lon + 1 };
        assert_eq!(upto.tiles().collect::<Vec<_>>(), vec![(4809, 4222)]);
        let across = Window { lat_lo: line - 1, lat_hi: line + 1, lon_lo: lon, lon_hi: lon + 1 };
        assert_eq!(across.tiles().collect::<Vec<_>>(), vec![(4809, 4222), (4810, 4222)]);
    }

    /// The lattice does not wrap and it does not go past the world box, so neither does a window.
    /// This is what keeps a cell at the antimeridian from reading ground on the far side of the
    /// world, and a negative coordinate from naming a tile the archive cannot hold.
    #[test]
    fn a_window_outside_the_world_box_names_no_tile() {
        let past = ORIGIN + TILES_PER_AXIS * TILE_UDEG;
        assert_eq!(Window { lat_lo: past, lat_hi: past + 1, lon_lo: 0, lon_hi: 1 }.tiles().count(), 0);
        assert_eq!(Window { lat_lo: 0, lat_hi: 1, lon_lo: past, lon_hi: past + 1 }.tiles().count(), 0);
        // A window straddling the east edge keeps the tiles inside it and drops the rest.
        let edge = Window { lat_lo: 0, lat_hi: 1, lon_lo: past - 1, lon_hi: past + TILE_UDEG };
        assert_eq!(edge.tiles().collect::<Vec<_>>(), vec![(4096, (TILES_PER_AXIS - 1) as u32)]);
        // Far below the origin is not a tile either — µdeg there are negative on both axes.
        assert_eq!(Window { lat_lo: ORIGIN - 10, lat_hi: ORIGIN - 1, lon_lo: 0, lon_hi: 1 }.tiles().count(), 0);
        // …and a negative µdeg inside the world box is an ordinary tile.
        assert_eq!(tile_id("0/0"), Some((0, 0)));
        assert_eq!(tile_id(&format!("{TILES_PER_AXIS}/0")), None, "past the world box is not a tile id");
        assert_eq!(tile_id("16/4809/4222"), None);
        assert_eq!(tile_id("ch"), None);
    }

    /// Pixel centres sit half a step inside the tile's own edges, so the first and last pixel of a
    /// window are decided by a centre and never by a boundary.
    #[test]
    fn a_window_selects_the_pixels_whose_centres_it_holds() {
        let origin = ORIGIN + 4809 * TILE_UDEG;
        // The first centre is at origin + 32 µdeg: a box that stops there holds nothing.
        assert_eq!(centre_span(origin, origin + STEP / 2, origin), 0..0);
        assert_eq!(centre_span(origin, origin + STEP / 2 + 1, origin), 0..1);
        // Exactly one node cell of the v1 posting, centred on the tile's 8th pixel boundary.
        assert_eq!(centre_span(origin + 8 * STEP, origin + 16 * STEP, origin), 8..16);
        // A window wider than the tile is clipped to it, on both sides.
        assert_eq!(centre_span(origin - TILE_UDEG, origin + 2 * TILE_UDEG, origin), 0..TILE_PIXELS);
    }
}
