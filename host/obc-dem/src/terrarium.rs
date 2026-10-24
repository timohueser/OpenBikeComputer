//! The terrain of the planner maps and of TrailView: lossless Terrarium WebP tiles in MBTiles, of
//! zooms 0 to 12, and of zoom 13 where the reference archive holds the ground. A pixel is the
//! height at its centre in quarter metres: whole metres in R and G, the quarters in B.
//!
//! One sweep from north to south fills the tiles of every zoom, so each GLO-30 square decodes
//! once and at most two rows of squares are decoded at a time. The tiles of one row fill on every
//! core. The archive takes the finished tiles zoom by zoom from a spool file, in the order of the
//! zoom's rows and columns, so its bytes do not depend on the sweep.

use std::f64::consts::PI;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::ops::RangeInclusive;
use std::os::unix::fs::FileExt;
use std::path::Path;

use image::{codecs::webp::WebPEncoder, ExtendedColorType};
use rayon::prelude::*;
use rusqlite::{params, Connection};

use crate::planner::{squares, Recent, Terrain};

const SIZE: u32 = 512;
/// The last zoom. Its tiles exist only where the archive holds the ground, south of
/// [`FINE_NORTH`]: there zoom 12 is already 10 m or finer.
const FINE: u32 = 13;
const FINE_NORTH: f64 = 58.4;

fn latitude(y: f64, n: f64) -> f64 {
    ((PI * (1.0 - 2.0 * y / n)).sinh().atan()).to_degrees()
}

/// One zoom of the sweep: the row of tiles that it fills.
struct Level {
    z: u32,
    west: u32,
    y: u32,
    south: u32,
    /// The next pixel row of row `y` to fill.
    py: u32,
    /// The latitude of each pixel row of row `y`, in microdegrees.
    lats: Vec<i32>,
    /// The RGBA pixels of each tile of row `y`, west to east, and whether a pixel has a height.
    tiles: Vec<(Vec<u8>, bool)>,
    /// Whether each tile of row `y` is filled: at [`FINE`], only where the archive holds the ground.
    wanted: Vec<bool>,
    /// The spool offset and length of each finished tile, with its column and MBTiles row.
    spooled: Vec<(u32, u32, u64, usize)>,
}

impl Level {
    fn new(z: u32, [w, s, e, n]: [f64; 4], terrain: &Terrain) -> Self {
        let count = f64::from(1u32 << z);
        let row = |lat: f64| (1.0 - (lat.to_radians().tan() + 1.0 / lat.to_radians().cos()).ln() / PI) / 2.0 * count;
        let west = ((w + 180.0) / 360.0 * count).floor() as u32;
        let east = (((e + 180.0) / 360.0 * count).ceil() as u32).min(count as u32);
        let (y, south) = (row(n).floor() as u32, (row(s).ceil() as u32).min(count as u32));
        let tiles = vec![(Vec::new(), false); (east - west) as usize];
        let wanted = Vec::new();
        let mut level = Self { z, west, y, south, py: 0, lats: Vec::new(), tiles, wanted, spooled: Vec::new() };
        level.start_row(terrain);
        level
    }

    fn start_row(&mut self, terrain: &Terrain) {
        let count = f64::from(1u32 << self.z);
        self.lats = (0..SIZE)
            .map(|py| {
                (latitude(f64::from(self.y) + (f64::from(py) + 0.5) / f64::from(SIZE), count) * 1e6).round() as i32
            })
            .collect();
        let (north, south) = (latitude(f64::from(self.y), count), latitude(f64::from(self.y + 1), count));
        let fine = (north + south).abs() / 2.0 < FINE_NORTH;
        self.wanted = (0..self.tiles.len() as u32)
            .map(|i| {
                let lon = |x: u32| f64::from(x) / count * 360.0 - 180.0;
                let x = self.west + i;
                self.z < FINE || fine && terrain.national_in([lon(x), south, lon(x + 1), north])
            })
            .collect();
        for (rgba, known) in &mut self.tiles {
            *rgba = vec![0; (SIZE * SIZE * 4) as usize];
            *known = false;
        }
    }

    /// The latitude of the next pixel row, or `None` when the zoom is complete.
    fn next(&self) -> Option<i32> {
        (self.y < self.south).then(|| self.lats[self.py as usize])
    }

    /// Fill the pixel rows of row `y` whose heights the squares of the latitudes `decoded` answer.
    /// When the row is complete, spool its tiles with a height and start the next.
    fn fill(&mut self, terrain: &Terrain, decoded: &RangeInclusive<i32>, spool: &File) -> Result<(), String> {
        let end = (self.py..SIZE).find(|&py| squares(self.lats[py as usize]).start() < decoded.start()).unwrap_or(SIZE);
        let (count, rows, z, west) = (f64::from(1u32 << self.z), self.py..end, self.z, self.west);
        let (lats, wanted) = (&self.lats, &self.wanted);
        self.tiles.par_iter_mut().enumerate().try_for_each(|(i, (rgba, known))| {
            if !wanted[i] {
                return Ok(());
            }
            let x = f64::from(west + i as u32);
            let mut recent = Recent::default();
            for py in rows.clone() {
                let lat = lats[py as usize];
                for px in 0..SIZE {
                    let lon = ((x + (f64::from(px) + 0.5) / f64::from(SIZE)) / count * 360.0 - 180.0) * 1e6;
                    let height = terrain.height(lat, lon.round() as i32, &mut recent)?;
                    let at = ((py * SIZE + px) * 4) as usize;
                    rgba[at..at + 4].copy_from_slice(&match height {
                        Some(metres) => terrarium(metres),
                        None => [128, 0, 0, 0],
                    });
                    *known |= height.is_some();
                }
            }
            Ok::<_, String>(())
        })?;
        self.py = end;
        if end < SIZE {
            return Ok(());
        }
        let encoded = self.tiles.par_iter().map(|(rgba, known)| {
            if !known {
                return Ok(None);
            }
            let mut bytes = Vec::new();
            let encoder = WebPEncoder::new_lossless(&mut bytes);
            encoder.encode(rgba, SIZE, SIZE, ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
            Ok(Some(bytes))
        });
        let encoded = encoded.collect::<Result<Vec<_>, String>>()?;
        let mut at = spool.metadata().map_err(|e| e.to_string())?.len();
        for (i, bytes) in encoded.into_iter().enumerate() {
            if let Some(bytes) = bytes {
                (&*spool).write_all(&bytes).map_err(|e| e.to_string())?;
                self.spooled.push((west + i as u32, (1u32 << z) - self.y - 1, at, bytes.len()));
                at += bytes.len() as u64;
            }
        }
        self.y += 1;
        self.py = 0;
        match self.y < self.south {
            true => self.start_row(terrain),
            false => self.tiles.clear(),
        }
        Ok(())
    }
}

/// The opaque Terrarium pixel of a height: R × 256 + G + B / 256 − 32768 metres, rounded to the
/// nearest quarter metre. B is the quarters × 64, so the decoded height is that quarter exactly.
/// Four values of B keep the tiles small.
pub fn terrarium(metres: f64) -> [u8; 4] {
    let quarters = (metres * 4.0).round() as i64;
    let rg = (quarters.div_euclid(4) + 32768).clamp(0, 65535) as u32;
    [(rg >> 8) as u8, rg as u8, (quarters.rem_euclid(4) * 64) as u8, 255]
}

/// Write the tiles of `bounds` (west, south, east, north in degrees) to the new file `output`.
/// The metadata `bounds` is the text `bounds_text`.
pub fn write(terrain: &mut Terrain, bounds: [f64; 4], bounds_text: &str, output: &Path) -> Result<(), String> {
    let [w, s, e, n] = bounds;
    if !bounds.iter().all(|v| v.is_finite()) || w < -180.0 || e > 180.0 || s < -85.0 || n > 85.0 || w >= e || s >= n {
        return Err("Invalid terrain bounds".into());
    }
    if output.exists() {
        return Err("Output exists; choose a fresh path".into());
    }
    // Unlinked at once, so the spool goes away with the process.
    let path = output.with_extension("spool");
    let spool = OpenOptions::new().read(true).append(true).create_new(true).open(&path);
    let spool = spool.map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;

    let mut levels: Vec<Level> = (0..=FINE).map(|z| Level::new(z, bounds, terrain)).collect();
    // The northmost pending pixel row decides the squares: no pending row needs one north of them.
    let mut last = None;
    while let Some(level) = levels.iter_mut().filter(|level| level.next().is_some()).max_by_key(|level| level.next()) {
        let decoded = squares(level.next().expect("pending"));
        if last.as_ref() != Some(&decoded) {
            eprintln!("Terrain at latitude {}", decoded.end());
            terrain.load(decoded.clone())?;
            last = Some(decoded.clone());
        }
        level.fill(terrain, &decoded, &spool)?;
    }

    let sql = |e: rusqlite::Error| e.to_string();
    let db = Connection::open(output).map_err(sql)?;
    db.execute_batch(
        "PRAGMA journal_mode=OFF; CREATE TABLE metadata(name TEXT, value TEXT);
        CREATE TABLE tiles(zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB,
        PRIMARY KEY(zoom_level,tile_column,tile_row)); BEGIN;",
    )
    .map_err(sql)?;
    for level in &levels {
        for &(x, row, at, length) in &level.spooled {
            let mut bytes = vec![0; length];
            spool.read_exact_at(&mut bytes, at).map_err(|e| e.to_string())?;
            db.execute("INSERT INTO tiles VALUES (?1,?2,?3,?4)", params![level.z, x, row, bytes]).map_err(sql)?;
        }
        db.execute_batch("COMMIT; BEGIN;").map_err(sql)?;
        eprintln!("Terrain zoom {} complete", level.z);
    }
    let identities = serde_json::to_string(&terrain.identities).expect("strings serialize");
    let maxzoom = levels.iter().rev().find(|level| !level.spooled.is_empty()).map_or(12, |level| level.z.max(12));
    for (key, value) in [
        ("name", "OpenBikeComputer terrain".into()),
        ("format", "webp".into()),
        ("type", "baselayer".into()),
        ("minzoom", "0".into()),
        ("maxzoom", maxzoom.to_string()),
        ("bounds", bounds_text.into()),
        ("attribution", terrain.attribution().join("; ")),
        ("source_sha256", identities),
    ] {
        db.execute("INSERT INTO metadata VALUES (?1,?2)", params![key, value]).map_err(sql)?;
    }
    db.execute_batch("COMMIT;").map_err(sql)
}

#[cfg(test)]
mod tests {
    use super::terrarium;

    /// A height decodes as MapLibre reads Terrarium to its nearest quarter metre, exactly: B takes
    /// four values, one per quarter.
    #[test]
    fn a_height_comes_back_as_its_quarter_metre() {
        let mut blues = std::collections::BTreeSet::new();
        for step in -2_000_i64..=36_000 {
            for offset in [-0.1249, 0.0, 0.1249] {
                let metres = step as f64 / 4.0 + offset;
                let [r, g, b, a] = terrarium(metres);
                let decoded = f64::from(r) * 256.0 + f64::from(g) + f64::from(b) / 256.0 - 32768.0;
                assert!(decoded == step as f64 / 4.0 && a == 255, "{metres} m");
                blues.insert(b);
            }
        }
        assert_eq!(blues.into_iter().collect::<Vec<_>>(), [0, 64, 128, 192]);
    }
}
