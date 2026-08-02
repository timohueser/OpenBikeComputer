//! `obc-dem fetch` — downloading the GLO-30 tiles a box needs, over HTTPS.
//!
//! Deliberately the thinnest thing that works, and deliberately **separate from `bake`**: a bake is
//! a pure function of a directory of tiles, so it must never reach for the network. Splitting them
//! is what lets a bake be re-run offline and byte-compared, and what keeps this module's failure
//! modes (a 404, a flaky link) out of the determinism contract.
//!
//! The AWS Open Data mirror of the Copernicus DEM publishes one object per 1° × 1° tile, named by
//! the tile's **south-west corner**. Ocean-only squares have no object at all, so a `404` is
//! coverage information rather than an error: the bake writes `NODATA` there, per the format's "a
//! hole is silence, never a guess" principle.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::BboxUdeg;

/// The AWS Open Data mirror of the Copernicus DEM GLO-30 instance, public HTTPS, no credentials.
pub const GLO30_BASE_URL: &str = "https://copernicus-dem-30m.s3.amazonaws.com";

/// Read size for the download loop — big enough that syscall overhead is irrelevant on a ~40 MB
/// body, small enough that a partial file is never far from the last byte that arrived.
const CHUNK: usize = 1 << 16;

/// Attempts per tile. An attempt restarts from zero rather than resuming: the mirror is not
/// guaranteed to honour a `Range` request, and a silently truncated DEM tile would bake a plausible
/// raster with a torn edge — far worse than a slow download.
const ATTEMPTS: usize = 3;

/// One GLO-30 tile, named by the integer degree of its south-west corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TileId {
    /// Latitude of the tile's south edge, degrees.
    pub lat: i32,
    /// Longitude of the tile's west edge, degrees.
    pub lon: i32,
}

impl TileId {
    /// The mirror's object stem, e.g. `Copernicus_DSM_COG_10_N46_00_E008_00_DEM`.
    pub fn stem(&self) -> String {
        let (ns, lat) = if self.lat < 0 { ('S', -self.lat) } else { ('N', self.lat) };
        let (ew, lon) = if self.lon < 0 { ('W', -self.lon) } else { ('E', self.lon) };
        format!("Copernicus_DSM_COG_10_{ns}{lat:02}_00_{ew}{lon:03}_00_DEM")
    }

    /// The full object URL on the mirror.
    pub fn url(&self) -> String {
        let stem = self.stem();
        format!("{GLO30_BASE_URL}/{stem}/{stem}.tif")
    }

    /// The local file name a fetch writes.
    pub fn file_name(&self) -> String {
        format!("{}.tif", self.stem())
    }
}

/// The tiles a box needs, in a fixed order.
///
/// Two edge rules, both consequences of GLO-30 being **`PixelIsPoint`** (see [`crate::geotiff`]):
///
/// - A tile named `N46` carries posts at latitudes `(46, 47]` — its own south edge post belongs to
///   `N45`. So the tile owning latitude `L` is `ceil(L) − 1`, not `floor(L)`.
/// - A tile named `E008` carries posts at longitudes `[8, 9)`, so longitude is plain `floor`.
///
/// The box is grown by [`EDGE_PAD_DEG`] first, because a lattice point on the very edge of the box
/// interpolates over source posts just outside it — and a missing neighbour tile would void that
/// sample instead of interpolating it.
pub fn tiles_for(bbox: BboxUdeg) -> Vec<TileId> {
    let pad = EDGE_PAD_DEG;
    let (min_lat, max_lat) = (f64::from(bbox.min_lat) / 1e6 - pad, f64::from(bbox.max_lat) / 1e6 + pad);
    let (min_lon, max_lon) = (f64::from(bbox.min_lon) / 1e6 - pad, f64::from(bbox.max_lon) / 1e6 + pad);
    let lat_lo = min_lat.ceil() as i32 - 1;
    let lat_hi = max_lat.ceil() as i32 - 1;
    let lon_lo = min_lon.floor() as i32;
    let lon_hi = max_lon.floor() as i32;
    let mut tiles = Vec::new();
    for lat in lat_lo..=lat_hi {
        for lon in lon_lo..=lon_hi {
            tiles.push(TileId { lat, lon });
        }
    }
    tiles
}

/// How far outside the requested box a fetch reaches, degrees. One GLO-30 post is 1/3600° ≈ 0.00028°
/// and a bilinear sample needs the post *beyond* the one it sits on, so a couple of thousandths of a
/// degree is several posts of slack — enough to never lose an edge sample, small enough that it
/// almost never pulls in a 40 MB tile the box did not really touch.
pub const EDGE_PAD_DEG: f64 = 0.002;

/// What one fetch did, per tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetched {
    /// The file was already on disk and was left alone.
    Cached,
    /// Downloaded, this many bytes.
    Downloaded(u64),
    /// The mirror has no object for this square — ocean, or outside the dataset. Not an error.
    Absent,
}

/// Download every tile `bbox` needs into `dir`, skipping ones already there.
///
/// `progress` is called once per tile with its id and outcome.
pub fn fetch_tiles(
    bbox: BboxUdeg,
    dir: &Path,
    mut progress: impl FnMut(TileId, &Fetched),
) -> Result<Vec<PathBuf>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut present = Vec::new();
    for tile in tiles_for(bbox) {
        let path = dir.join(tile.file_name());
        let outcome = if path.exists() { Fetched::Cached } else { download(&tile, &path)? };
        progress(tile, &outcome);
        if outcome != Fetched::Absent {
            present.push(path);
        }
    }
    if present.is_empty() {
        return Err("no GLO-30 tiles cover that box — the mirror has no object for any of them".to_string());
    }
    Ok(present)
}

/// Fetch one tile to `path`, via a `.part` file so an interrupted run never leaves a half tile
/// looking like a whole one.
fn download(tile: &TileId, path: &Path) -> Result<Fetched, String> {
    let url = tile.url();
    let part = path.with_extension("tif.part");
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        match try_once(&url, &part) {
            Ok(Some(len)) => {
                std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))?;
                return Ok(Fetched::Downloaded(len));
            }
            Ok(None) => {
                let _ = std::fs::remove_file(&part);
                return Ok(Fetched::Absent);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                last = e;
                if attempt < ATTEMPTS {
                    eprintln!("obc-dem: {url}: {last} (attempt {attempt}/{ATTEMPTS})");
                }
            }
        }
    }
    Err(format!("GET {url}: {last}"))
}

/// One attempt. `Ok(None)` is a `404` — the square has no tile, which is a fact about the world
/// rather than a failure.
fn try_once(url: &str, part: &Path) -> Result<Option<u64>, String> {
    let response = match ureq::get(url).call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(404)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut body = response.into_body().into_reader();
    let file = std::fs::File::create(part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut out = std::io::BufWriter::new(file);
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", part.display()))?;
        total += n as u64;
    }
    out.flush().map_err(|e| format!("{}: {e}", part.display()))?;
    if total == 0 {
        return Err("empty body".to_string());
    }
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tile_id_is_the_mirrors_object_name() {
        assert_eq!(TileId { lat: 46, lon: 8 }.stem(), "Copernicus_DSM_COG_10_N46_00_E008_00_DEM");
        assert_eq!(
            TileId { lat: 46, lon: 8 }.url(),
            "https://copernicus-dem-30m.s3.amazonaws.com/Copernicus_DSM_COG_10_N46_00_E008_00_DEM/Copernicus_DSM_COG_10_N46_00_E008_00_DEM.tif"
        );
        // Both hemispheres, and the asymmetric zero padding the mirror uses (2 for lat, 3 for lon).
        assert_eq!(TileId { lat: -34, lon: -59 }.stem(), "Copernicus_DSM_COG_10_S34_00_W059_00_DEM");
        assert_eq!(TileId { lat: 0, lon: 0 }.stem(), "Copernicus_DSM_COG_10_N00_00_E000_00_DEM");
    }

    /// The two ownership rules, which are not the same rule: latitude posts belong to the tile
    /// *below* their integer, longitude posts to the tile at it.
    #[test]
    fn tile_selection_follows_the_pixel_is_point_ownership() {
        // Grimsel: wholly inside N46E008 despite the box's own name suggesting nothing about it.
        let grimsel = BboxUdeg::parse("46.48261,8.15034,46.72070,8.46007").unwrap();
        assert_eq!(tiles_for(grimsel), vec![TileId { lat: 46, lon: 8 }]);

        // Teningen: wholly inside N48E007.
        let teningen = BboxUdeg::parse("48.119,7.798,48.141,7.830").unwrap();
        assert_eq!(tiles_for(teningen), vec![TileId { lat: 48, lon: 7 }]);

        // A box reaching an integer latitude needs the tile *below* it, because that tile owns the
        // post exactly on the degree line.
        let across_lat = BboxUdeg::parse("45.99,8.2,46.10,8.3").unwrap();
        assert_eq!(tiles_for(across_lat), vec![TileId { lat: 45, lon: 8 }, TileId { lat: 46, lon: 8 }]);

        // A box reaching an integer longitude needs the tile *at* it.
        let across_lon = BboxUdeg::parse("46.2,8.99,46.3,9.01").unwrap();
        assert_eq!(tiles_for(across_lon), vec![TileId { lat: 46, lon: 8 }, TileId { lat: 46, lon: 9 }]);
    }

    /// A box that stops a hair short of a tile edge still pulls the neighbour, because the last
    /// lattice point inside it interpolates over posts just outside it.
    #[test]
    fn the_edge_pad_reaches_the_neighbour_a_bilinear_sample_needs() {
        let just_short = BboxUdeg::parse("46.2,8.2,46.3,8.9995").unwrap();
        assert_eq!(tiles_for(just_short), vec![TileId { lat: 46, lon: 8 }, TileId { lat: 46, lon: 9 }]);
        let clear = BboxUdeg::parse("46.2,8.2,46.3,8.5").unwrap();
        assert_eq!(tiles_for(clear), vec![TileId { lat: 46, lon: 8 }]);
    }
}
