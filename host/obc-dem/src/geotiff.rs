//! Reading the source DEM: one GeoTIFF tile ([`DemTile`]) and the mosaic of them ([`DemMosaic`])
//! that a bake samples.
//!
//! Everything this module knows about the source is read from the file's own tags — the
//! geotransform, the raster convention, the void value. Nothing about Copernicus GLO-30 is
//! hard-coded, because the one property of that dataset that *would* have been worth hard-coding is
//! false: **tile shape varies with latitude**. A 1° × 1° tile is 3600 columns up to 50 °N, 1800
//! from 50 to 60 °N, and coarser again further north, while every tile is 3600 rows. A mosaic that
//! assumed one global post lattice would silently misplace every sample above 50 °N.
//!
//! ## The two conventions that decide where a sample *is*
//!
//! 1. **`RasterPixelIsPoint`.** GLO-30 sets `GTRasterTypeGeoKey = 2`: the tie point names the
//!    **centre of post (0, 0)**, not the corner of a pixel. So tile `N46E008` carries posts from
//!    lon 8.0 to 8.99972 and from lat 47.0 down to 46.00028, and the post at lon 9.0 belongs to
//!    `N46E009` — the global post lattice is seamless, with every post owned exactly once. The
//!    `RasterPixelIsArea` convention is handled too (the tie point then names a pixel *corner*, so
//!    post centres sit half a step in), because it is the GeoTIFF default and a re-projected source
//!    would use it.
//! 2. **Row order.** A GeoTIFF's row 0 is the **northernmost** scanline; OBCT rows advance latitude
//!    northward (`OBCT_Spec.md` §2). The flip happens **here, once, on ingest** — exactly as that
//!    section says a baker must — so every row index downstream of this module means "north is up".
//!
//! Sampling the mosaic is `f64` throughout and bilinear over the four surrounding source posts;
//! see [`DemMosaic::height`] for why that is a point sample and not an area aggregate.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use tiff::decoder::{Decoder, DecodingResult};
use tiff::tags::Tag;

/// `GTModelTypeGeoKey` — 2 is `ModelTypeGeographic`, the only projection this tool reads. A
/// projected DEM would need a reprojection this crate deliberately does not do.
const GEOKEY_MODEL_TYPE: u16 = 1024;
/// `GTRasterTypeGeoKey` — 1 `PixelIsArea`, 2 `PixelIsPoint`.
const GEOKEY_RASTER_TYPE: u16 = 1025;
/// `GeographicTypeGeoKey` — 4326 is WGS 84, which every OBC coordinate already is.
const GEOKEY_GEOGRAPHIC_TYPE: u16 = 2048;

/// The GeoTIFF raster convention (`GTRasterTypeGeoKey`): what the tie point names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RasterType {
    /// The tie point is a pixel **corner**; post centres sit half a step inside it.
    Area,
    /// The tie point is a post **centre** — the DEM convention, and what GLO-30 ships.
    Point,
}

/// One decoded DEM tile: a rectangular grid of posts on a regular lat/lon lattice.
///
/// Rows are stored **south-up** (row 0 is the southernmost), which is the one place the GeoTIFF's
/// north-up scanline order is undone.
#[derive(Debug)]
pub struct DemTile {
    /// Latitude of post row 0, degrees — the southernmost row after the ingest flip.
    /// (No source path is kept: every error `open` can raise is raised while the local
    /// `path` is still in scope, and nothing downstream names the file again.)
    south_lat_deg: f64,
    /// Longitude of post column 0, degrees.
    west_lon_deg: f64,
    /// Post spacing, degrees. Both are positive; the sign of the GeoTIFF's north-down row step is
    /// absorbed by the flip.
    step_lat_deg: f64,
    step_lon_deg: f64,
    rows: usize,
    cols: usize,
    /// Post heights, row-major, south-up. `f32` because that is what the source ships; the
    /// arithmetic that touches them is `f64`.
    data: Vec<f32>,
    /// The source's declared void value (`GDAL_NODATA`), if it declared one.
    nodata: Option<f64>,
}

impl DemTile {
    /// Decode a GeoTIFF DEM tile from `path`.
    ///
    /// Every rejection here is a property of the *file*, checked once, so [`DemMosaic::height`] can
    /// be free of them: a tile that survives this is a regular geographic post grid whose every
    /// sample position is one multiply away.
    pub fn open(path: &Path) -> Result<DemTile, String> {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut dec =
            Decoder::new(BufReader::new(file)).map_err(|e| format!("{}: not a readable TIFF ({e})", path.display()))?;
        let name = || path.display().to_string();

        let (width, height) = dec.dimensions().map_err(|e| format!("{}: no dimensions ({e})", name()))?;
        let (cols, rows) = (width as usize, height as usize);
        if cols < 2 || rows < 2 {
            return Err(format!("{}: {cols}×{rows} is too small to interpolate over", name()));
        }

        // --- the geotransform ------------------------------------------------------------------
        let tie = dec
            .get_tag_f64_vec(Tag::ModelTiepointTag)
            .map_err(|_| format!("{}: no ModelTiepointTag — not a georeferenced raster", name()))?;
        let scale = dec.get_tag_f64_vec(Tag::ModelPixelScaleTag).map_err(|_| {
            format!("{}: no ModelPixelScaleTag — a transformation-matrix GeoTIFF is not supported", name())
        })?;
        if tie.len() < 6 || scale.len() < 2 {
            return Err(format!("{}: short ModelTiepointTag/ModelPixelScaleTag", name()));
        }
        // A single tie point anchored on raster (0, 0) is the only form here: anything else means
        // the raster is not a plain north-up grid, and guessing at one would misplace every sample.
        if tie[0] != 0.0 || tie[1] != 0.0 {
            return Err(format!("{}: ModelTiepointTag is not anchored on raster (0, 0)", name()));
        }
        let (step_lon_deg, step_lat_deg) = (scale[0], scale[1]);
        if !(step_lon_deg.is_finite() && step_lat_deg.is_finite()) || step_lon_deg <= 0.0 || step_lat_deg <= 0.0 {
            return Err(format!("{}: pixel scale {scale:?} is not a positive north-up step", name()));
        }

        // --- the GeoKeys -----------------------------------------------------------------------
        let keys = geo_keys(&mut dec);
        match keys.iter().find(|(k, _)| *k == GEOKEY_MODEL_TYPE).map(|(_, v)| *v) {
            Some(2) | None => {}
            Some(other) => return Err(format!("{}: GTModelTypeGeoKey {other} — only geographic (2) is read", name())),
        }
        if let Some((_, code)) = keys.iter().find(|(k, _)| *k == GEOKEY_GEOGRAPHIC_TYPE) {
            if *code != 4326 {
                return Err(format!("{}: GeographicTypeGeoKey {code} — only WGS 84 (4326) is read", name()));
            }
        }
        let raster_type = match keys.iter().find(|(k, _)| *k == GEOKEY_RASTER_TYPE).map(|(_, v)| *v) {
            Some(2) => RasterType::Point,
            // GeoTIFF's own default when the key is absent.
            Some(1) | None => RasterType::Area,
            Some(other) => {
                return Err(format!("{}: GTRasterTypeGeoKey {other} is neither area (1) nor point (2)", name()))
            }
        };
        // With `PixelIsArea` the tie point is a pixel corner, so post centres sit half a step in.
        let (half_lon, half_lat) = match raster_type {
            RasterType::Point => (0.0, 0.0),
            RasterType::Area => (step_lon_deg / 2.0, step_lat_deg / 2.0),
        };
        let west_lon_deg = tie[3] + half_lon;
        let north_lat_deg = tie[4] - half_lat;

        // --- the void value --------------------------------------------------------------------
        let nodata = dec
            .get_tag_ascii_string(Tag::GdalNodata)
            .ok()
            .and_then(|s| s.trim().trim_end_matches('\0').parse::<f64>().ok());

        // --- the samples, flipped once (spec §2) -----------------------------------------------
        let image = dec.read_image().map_err(|e| format!("{}: decode failed ({e})", name()))?;
        let north_up: Vec<f32> = match image {
            DecodingResult::F32(v) => v,
            DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
            DecodingResult::I16(v) => v.into_iter().map(f32::from).collect(),
            DecodingResult::U16(v) => v.into_iter().map(f32::from).collect(),
            _ => return Err(format!("{}: unsupported sample type — a DEM must be float or 16-bit integer", name())),
        };
        if north_up.len() != rows * cols {
            return Err(format!("{}: decoded {} samples for a {cols}×{rows} raster", name(), north_up.len()));
        }
        let mut data = vec![0f32; rows * cols];
        for row in 0..rows {
            let src = &north_up[(rows - 1 - row) * cols..(rows - row) * cols];
            data[row * cols..(row + 1) * cols].copy_from_slice(src);
        }

        Ok(DemTile {
            south_lat_deg: north_lat_deg - step_lat_deg * (rows - 1) as f64,
            west_lon_deg,
            step_lat_deg,
            step_lon_deg,
            rows,
            cols,
            data,
            nodata,
        })
    }

    /// Posts along each axis, `(rows, cols)`.
    pub fn shape(&self) -> (usize, usize) {
        (self.rows, self.cols)
    }

    /// Post spacing in degrees, `(lat, lon)`.
    pub fn step_deg(&self) -> (f64, f64) {
        (self.step_lat_deg, self.step_lon_deg)
    }

    /// Position of post `(0, 0)` — the **south-west** post, after the ingest flip. Where this lands
    /// relative to the file's tie point is the whole `PixelIsPoint` / `PixelIsArea` question, so it
    /// is exposed rather than inferred.
    pub fn south_west_deg(&self) -> (f64, f64) {
        (self.south_lat_deg, self.west_lon_deg)
    }

    /// Latitude of post row `row`, extrapolating linearly outside the tile — which is what makes a
    /// cross-tile corner lookup a coordinate rather than an index-space guess.
    fn post_lat(&self, row: i64) -> f64 {
        self.south_lat_deg + self.step_lat_deg * row as f64
    }

    fn post_lon(&self, col: i64) -> f64 {
        self.west_lon_deg + self.step_lon_deg * col as f64
    }

    /// The half-open box of coordinates this tile answers for: its posts, grown by half a step on
    /// every side, so abutting tiles partition the plane with no gap and no overlap.
    fn owns(&self, lat_deg: f64, lon_deg: f64) -> bool {
        let (lo_lat, hi_lat) =
            (self.post_lat(0) - self.step_lat_deg / 2.0, self.post_lat(self.rows as i64 - 1) + self.step_lat_deg / 2.0);
        let (lo_lon, hi_lon) =
            (self.post_lon(0) - self.step_lon_deg / 2.0, self.post_lon(self.cols as i64 - 1) + self.step_lon_deg / 2.0);
        lat_deg >= lo_lat && lat_deg < hi_lat && lon_deg >= lo_lon && lon_deg < hi_lon
    }

    /// The height at post `(row, col)`, or `None` for a void — the declared `GDAL_NODATA`, a NaN, or
    /// a value outside any believable terrain range. **No inpainting**: a void stays a void all the
    /// way to the `NODATA` sample the bake writes.
    fn post(&self, row: usize, col: usize) -> Option<f64> {
        let v = f64::from(self.data[row * self.cols + col]);
        if !v.is_finite() || !(-12_000.0..=12_000.0).contains(&v) {
            return None;
        }
        match self.nodata {
            // The declared void is compared after the `f32 -> f64` widening on both sides, so a
            // `-32767` written as `f32` still matches a `-32767` parsed from the tag.
            Some(n) if v == f64::from(n as f32) => None,
            _ => Some(v),
        }
    }

    /// The sort key that fixes tile order in a [`DemMosaic`]. Bit patterns rather than the floats
    /// themselves, so the order is total and has no NaN clause.
    fn order_key(&self) -> (i64, i64) {
        (self.south_lat_deg.to_bits() as i64, self.west_lon_deg.to_bits() as i64)
    }
}

/// Index-space slack: a query within a billionth of a post spacing of a post **is** on that post.
///
/// It exists for one case, which is not hypothetical: a target coordinate that lands exactly on the
/// tile's outermost post arrives as `last ± 1 ULP` after the degrees round-trip through the
/// geotransform, and without the snap that post would be a hole on one machine and a value on
/// another. A billionth of a post is ~30 nm on the ground.
const POST_EPS: f64 = 1e-9;

/// Pull a post index that is a hair outside `0..=last` back onto the boundary. Anything further out
/// is left alone: that is a genuinely uncovered coordinate, and the caller must not answer it.
fn snap(v: f64, last: f64) -> f64 {
    if v < 0.0 && v > -POST_EPS {
        0.0
    } else if v > last && v < last + POST_EPS {
        last
    } else {
        v
    }
}

/// Every `(key, value)` in the GeoTIFF GeoKey directory whose value is inline (`location = 0`).
/// The two keys this tool reads are both of that kind; a key stored out-of-line points into
/// `GeoDoubleParams`/`GeoAsciiParams` and names a datum detail, not a geometry fact.
fn geo_keys<R: std::io::Read + std::io::Seek>(dec: &mut Decoder<R>) -> Vec<(u16, u16)> {
    let Ok(raw) = dec.get_tag_u32_vec(Tag::GeoKeyDirectoryTag) else {
        return Vec::new();
    };
    if raw.len() < 4 {
        return Vec::new();
    }
    let count = raw[3] as usize;
    (0..count)
        .filter_map(|k| {
            let at = 4 + k * 4;
            let entry = raw.get(at..at + 4)?;
            (entry[1] == 0).then_some((entry[0] as u16, entry[3] as u16))
        })
        .collect()
}

/// The source DEM as one surface: a set of tiles, sampled bilinearly.
///
/// Tiles are held in a fixed order and looked up by coordinate, never by name — so a mosaic of one
/// tile, of a whole country, or of two tiles at different latitudes with *different* post spacings
/// all sample the same way.
///
/// **Every tile is held decoded in memory** — ~52 MB per 1° GLO-30 tile. That is the right shape for
/// a bake of a map-sized box, and the wrong shape for a continent: a caller baking DACH must feed it
/// the tiles a region needs and drop them between regions, not the whole dataset at once.
#[derive(Debug, Default)]
pub struct DemMosaic {
    tiles: Vec<DemTile>,
    /// The tile that answered the previous query. A bake walks a cell sample by sample, so
    /// essentially every query lands in the same tile as the one before it, and without this the
    /// lookup is a linear scan of the mosaic **per sample** — invisible at two tiles, quadratic
    /// misery at six hundred.
    ///
    /// It cannot change an answer: tile coverage boxes are disjoint by construction (§`owns`), so
    /// at most one tile ever matches and checking a different one first only reorders the search.
    last_hit: std::cell::Cell<usize>,
    /// The same memo, for the corner resolver alone. `height` resolves an off-tile stencil corner
    /// through [`DemMosaic::nearest_post`], which is a *different* tile by definition — sharing one
    /// slot meant every seam-adjacent sample overwrote the memo with the neighbour and then the next
    /// sample overwrote it back, i.e. two linear scans per sample exactly along the seams, where the
    /// memo is worth the most.
    last_corner_hit: std::cell::Cell<usize>,
}

impl DemMosaic {
    /// Load every `.tif` / `.tiff` under `dir`, in sorted path order.
    ///
    /// Sorted because the whole crate is a determinism contract and a directory listing is not
    /// ordered: two runs must load the same tiles in the same order, or a coordinate on a tile
    /// boundary could be answered by a different tile on each run.
    pub fn open_dir(dir: &Path) -> Result<DemMosaic, String> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"))
                    == Some(true)
            })
            .collect();
        paths.sort();
        if paths.is_empty() {
            return Err(format!("{}: no .tif tiles found", dir.display()));
        }
        let mut mosaic = DemMosaic::default();
        for path in &paths {
            mosaic.push(DemTile::open(path)?);
        }
        Ok(mosaic)
    }

    /// Add a tile, keeping the mosaic in its canonical order.
    pub fn push(&mut self, tile: DemTile) {
        self.tiles.push(tile);
        self.tiles.sort_by_key(DemTile::order_key);
        self.last_hit.set(0); // the sort invalidated both memo indices
        self.last_corner_hit.set(0);
    }

    /// How many tiles the mosaic holds.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The tile that answers for `(lat, lon)`, or `None` outside coverage.
    fn tile_for(&self, lat_deg: f64, lon_deg: f64) -> Option<&DemTile> {
        self.tile_for_memo(lat_deg, lon_deg, &self.last_hit)
    }

    /// [`DemMosaic::tile_for`], against a caller-chosen memo slot. Which slot is used can only
    /// change how long the search takes, never its answer (coverage boxes are disjoint).
    fn tile_for_memo(&self, lat_deg: f64, lon_deg: f64, memo: &std::cell::Cell<usize>) -> Option<&DemTile> {
        if let Some(tile) = self.tiles.get(memo.get()) {
            if tile.owns(lat_deg, lon_deg) {
                return Some(tile);
            }
        }
        let (index, tile) = self.tiles.iter().enumerate().find(|(_, t)| t.owns(lat_deg, lon_deg))?;
        memo.set(index);
        Some(tile)
    }

    /// The nearest source post to `(lat, lon)`, or `None` outside coverage / over a void. Used only
    /// to reach a corner that lies in a *neighbouring* tile: asking by coordinate rather than by
    /// index is what lets two tiles with different post spacings meet without inventing a lattice
    /// that neither of them is on.
    fn nearest_post(&self, lat_deg: f64, lon_deg: f64) -> Option<f64> {
        let tile = self.tile_for_memo(lat_deg, lon_deg, &self.last_corner_hit)?;
        let row = ((lat_deg - tile.south_lat_deg) / tile.step_lat_deg).round();
        let col = ((lon_deg - tile.west_lon_deg) / tile.step_lon_deg).round();
        let row = (row as i64).clamp(0, tile.rows as i64 - 1) as usize;
        let col = (col as i64).clamp(0, tile.cols as i64 - 1) as usize;
        tile.post(row, col)
    }

    /// The source height at `(lat, lon)` in metres, bilinearly interpolated over the four
    /// surrounding posts — or `None` outside coverage or next to a void.
    ///
    /// **A point sample of the source surface, not an area aggregate** (epic #1068's design note).
    /// At the v1 posting the target lattice is ~1.5 × coarser than a 30 m source, and at that ratio
    /// the difference between bilinear and box-averaging is well below the source's own ~2–4 m
    /// vertical RMSE — while a point sample is exactly reproducible from four numbers, which
    /// matters far more here than a fractionally better estimate.
    ///
    /// **A void poisons the sample**, the same rule `OBCT_Spec.md` §5.4 applies on the read side and
    /// for the same reason: a void is typically water or radar shadow, exactly where a value
    /// interpolated from the surviving corners would be most confidently wrong.
    ///
    /// All-`f64`, and every operation is an IEEE-754 correctly rounded add/subtract/multiply/divide
    /// — Rust contracts none of them into an FMA and reads no rounding mode — so the result is a
    /// function of the inputs alone and not of the host. That is the whole determinism claim.
    ///
    /// **The interpolation is written in corner-and-slope form**, not as the four weighted corners
    /// `OBCT_Spec.md` §5.2 spells out. The two are the same expression algebraically; only the
    /// reader's *integer* evaluation of §5.2 is normative, and no `f64` expression reproduces that
    /// bit for bit anyway. What this form buys is the case that dominates the dataset by area:
    /// GLO-30 **flattens water**, so a lake or the sea is a large patch of one exactly-repeated
    /// value. In the weighted-corner form the four `f64` weights sum to `1 ± a few ULP`, so a
    /// flattened surface at, say, 1879.5 m interpolates to 1879.4999999999998 at some points and
    /// 1879.5 at others — and half away from zero then quantises one lake into two different metres.
    /// Here the three difference terms are exactly `0.0` over equal corners, so a flat surface stays
    /// flat, to the bit.
    pub fn height(&self, lat_deg: f64, lon_deg: f64) -> Option<f64> {
        let tile = self.tile_for(lat_deg, lon_deg)?;
        let (last_row_f, last_col_f) = (tile.rows as f64 - 1.0, tile.cols as f64 - 1.0);
        let y = snap((lat_deg - tile.south_lat_deg) / tile.step_lat_deg, last_row_f);
        let x = snap((lon_deg - tile.west_lon_deg) / tile.step_lon_deg, last_col_f);
        // The stencil's base post. Clamped one short of the last post when the query lands exactly
        // *on* it: there is no post beyond the last one, so the honest interpolant there is the
        // last interval evaluated at its far end (`f = 1`) rather than a stencil with a missing
        // corner. Past the last post the clamp does not apply — that is genuinely off this tile,
        // and the corner resolver below is what decides whether a neighbour covers it.
        let r0 = if y <= last_row_f { y.floor().min(last_row_f - 1.0) } else { y.floor() };
        let c0 = if x <= last_col_f { x.floor().min(last_col_f - 1.0) } else { x.floor() };
        let (fy, fx) = (y - r0, x - c0);
        let (r0, c0) = (r0 as i64, c0 as i64);

        let corner = |dr: i64, dc: i64| -> Option<f64> {
            let (row, col) = (r0 + dr, c0 + dc);
            if row >= 0 && col >= 0 && (row as usize) < tile.rows && (col as usize) < tile.cols {
                tile.post(row as usize, col as usize)
            } else {
                // The corner is off this tile's own grid — ask the mosaic where that post *is*.
                self.nearest_post(tile.post_lat(row), tile.post_lon(col))
            }
        };
        let (v00, v10, v01, v11) = (corner(0, 0)?, corner(1, 0)?, corner(0, 1)?, corner(1, 1)?);
        Some(v00 + (v10 - v00) * fy + (v01 - v00) * fx + (v11 - v01 - v10 + v00) * fy * fx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-tile mosaic of a constant surface, at a posting that shares no point with the target
    /// lattice — the shape of GLO-30's flattened water.
    fn flat_mosaic(value: f32) -> DemMosaic {
        let (rows, cols) = (8usize, 8usize);
        let mut mosaic = DemMosaic::default();
        mosaic.push(DemTile {
            south_lat_deg: 46.0,
            west_lon_deg: 8.0,
            step_lat_deg: 1.0 / 3600.0,
            step_lon_deg: 1.0 / 3600.0,
            rows,
            cols,
            data: vec![value; rows * cols],
            nodata: None,
        });
        mosaic
    }

    /// The artifact this form exists to remove: a surface that is one repeated value must
    /// interpolate to **exactly** that value everywhere, or a `.5` lake level quantises into two
    /// different metres across one body of water.
    #[test]
    fn a_flattened_surface_interpolates_to_exactly_its_own_value() {
        let mosaic = flat_mosaic(1879.5);
        for k in 0..64 {
            let t = f64::from(k) / 64.0;
            let (lat, lon) = (46.0 + t / 3600.0 * 6.0, 8.0 + t / 3600.0 * 6.0);
            assert_eq!(mosaic.height(lat, lon), Some(1879.5), "at ({lat}, {lon})");
        }
    }

    /// **The source is never extrapolated.** An interpolated height needs four real posts, so
    /// coverage ends exactly at the outermost post — not at the half-step skirt, which decides only
    /// *which tile* answers a coordinate. One microdegree past the last post is silence, and the
    /// coverage-edge clamp that softens that on the read side (`OBCT_Spec.md` §5.3 step 3) is the
    /// reader's rule about its own file, not a licence for the baker to invent source data.
    #[test]
    fn the_source_surface_stops_at_its_outermost_post() {
        let mosaic = flat_mosaic(100.0);
        let step = 1.0 / 3600.0;
        assert_eq!(mosaic.height(46.0, 8.0), Some(100.0), "the corner post itself");
        assert_eq!(mosaic.height(46.0 + step * 6.5, 8.0 + step * 6.5), Some(100.0), "well inside");
        assert_eq!(mosaic.height(46.0 - step * 0.01, 8.0), None, "a hair south of the first post");
        assert_eq!(mosaic.height(46.0, 8.0 - step * 0.01), None, "a hair west of it");
        // The far corner is the last post, 7 steps up; past it the stencil has no fourth corner.
        let far = step * 7.0;
        assert_eq!(mosaic.height(46.0 + far, 8.0 + far), Some(100.0));
        assert_eq!(mosaic.height(46.0 + far + step * 0.01, 8.0), None);
    }

    /// Two abutting tiles are one surface: a sample between the last post of one and the first post
    /// of the next interpolates across the join, because a corner outside the home tile is resolved
    /// by **coordinate** and lands in its neighbour.
    #[test]
    fn a_corner_in_the_next_tile_is_fetched_rather_than_clamped() {
        let step = 1.0 / 3600.0;
        let plane = |row: usize, col: usize, base: f64| (base + row as f64 + 10.0 * col as f64) as f32;
        let mut mosaic = DemMosaic::default();
        for (index, west) in [8.0f64, 8.0 + step * 4.0].into_iter().enumerate() {
            let (rows, cols) = (4usize, 4usize);
            let mut data = vec![0f32; rows * cols];
            for row in 0..rows {
                for col in 0..cols {
                    // One continuous plane across both tiles: the second starts 4 columns along.
                    data[row * cols + col] = plane(row, col + index * 4, 100.0);
                }
            }
            mosaic.push(DemTile {
                south_lat_deg: 46.0,
                west_lon_deg: west,
                step_lat_deg: step,
                step_lon_deg: step,
                rows,
                cols,
                data,
                nodata: None,
            });
        }
        // Halfway between the last post of tile 0 (col 3, value 130) and the first of tile 1
        // (col 4, value 140). A clamp would answer 130; the cross-tile fetch answers 135.
        let at = mosaic.height(46.0, 8.0 + step * 3.5).unwrap();
        assert!((at - 135.0).abs() < 1e-9, "expected 135 across the seam, got {at}");
    }
}
