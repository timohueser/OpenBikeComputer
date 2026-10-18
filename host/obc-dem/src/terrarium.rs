//! The terrain of the planner maps: lossless Terrarium WebP tiles of zooms 0 to 12 in MBTiles.
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
    /// The spool offset and length of each finished tile, with its column and MBTiles row.
    spooled: Vec<(u32, u32, u64, usize)>,
}

impl Level {
    fn new(z: u32, [w, s, e, n]: [f64; 4]) -> Self {
        let count = f64::from(1u32 << z);
        let row = |lat: f64| (1.0 - (lat.to_radians().tan() + 1.0 / lat.to_radians().cos()).ln() / PI) / 2.0 * count;
        let west = ((w + 180.0) / 360.0 * count).floor() as u32;
        let east = (((e + 180.0) / 360.0 * count).ceil() as u32).min(count as u32);
        let (y, south) = (row(n).floor() as u32, (row(s).ceil() as u32).min(count as u32));
        let tiles = vec![(Vec::new(), false); (east - west) as usize];
        let mut level = Self { z, west, y, south, py: 0, lats: Vec::new(), tiles, spooled: Vec::new() };
        level.start_row();
        level
    }

    fn start_row(&mut self) {
        let count = f64::from(1u32 << self.z);
        self.lats = (0..SIZE)
            .map(|py| {
                (latitude(f64::from(self.y) + (f64::from(py) + 0.5) / f64::from(SIZE), count) * 1e6).round() as i32
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
        let lats = &self.lats;
        self.tiles.par_iter_mut().enumerate().try_for_each(|(i, (rgba, known))| {
            let x = f64::from(west + i as u32);
            let mut recent = Recent::default();
            for py in rows.clone() {
                let lat = lats[py as usize];
                for px in 0..SIZE {
                    let lon = ((x + (f64::from(px) + 0.5) / f64::from(SIZE)) / count * 360.0 - 180.0) * 1e6;
                    let height = terrain.height(lat, lon.round() as i32, &mut recent)?;
                    // Whole metres: relief and contours need no finer height, and lossless WebP
                    // compresses fewer distinct values better.
                    let value = height
                        .map(|h| ((h.round() + 32768.0) * 256.0).clamp(0.0, 16777215.0) as u32)
                        .unwrap_or(8388608);
                    let at = ((py * SIZE + px) * 4) as usize;
                    rgba[at..at + 4].copy_from_slice(&[
                        (value >> 16) as u8,
                        (value >> 8) as u8,
                        value as u8,
                        if height.is_some() { 255 } else { 0 },
                    ]);
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
            true => self.start_row(),
            false => self.tiles.clear(),
        }
        Ok(())
    }
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

    let mut levels: Vec<Level> = (0..=12).map(|z| Level::new(z, bounds)).collect();
    // The northmost pending pixel row decides the squares: no pending row needs one north of them.
    while let Some(level) = levels.iter_mut().filter(|level| level.next().is_some()).max_by_key(|level| level.next()) {
        let decoded = squares(level.next().expect("pending"));
        terrain.load(decoded.clone())?;
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
    for (key, value) in [
        ("name", "OpenBikeComputer terrain".into()),
        ("format", "webp".into()),
        ("type", "baselayer".into()),
        ("minzoom", "0".into()),
        ("maxzoom", "12".into()),
        ("bounds", bounds_text.into()),
        ("attribution", terrain.attribution().join("; ")),
        ("source_sha256", identities),
    ] {
        db.execute("INSERT INTO metadata VALUES (?1,?2)", params![key, value]).map_err(sql)?;
    }
    db.execute_batch("COMMIT;").map_err(sql)
}
