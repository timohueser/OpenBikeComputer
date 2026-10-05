//! The terrain of the planner maps: lossless Terrarium WebP tiles of zooms 0 to 12 in MBTiles.

use std::f64::consts::PI;
use std::path::Path;

use image::{codecs::webp::WebPEncoder, ExtendedColorType};
use rusqlite::{params, Connection};

use crate::planner::Terrain;

const SIZE: u32 = 512;

fn latitude(y: f64, n: f64) -> f64 {
    ((PI * (1.0 - 2.0 * y / n)).sinh().atan()).to_degrees()
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
    let sql = |e: rusqlite::Error| e.to_string();
    let db = Connection::open(output).map_err(sql)?;
    db.execute_batch(
        "PRAGMA journal_mode=OFF; CREATE TABLE metadata(name TEXT, value TEXT);
        CREATE TABLE tiles(zoom_level INTEGER, tile_column INTEGER, tile_row INTEGER, tile_data BLOB,
        PRIMARY KEY(zoom_level,tile_column,tile_row)); BEGIN;",
    )
    .map_err(sql)?;
    let row =
        |lat: f64, count: f64| (1.0 - (lat.to_radians().tan() + 1.0 / lat.to_radians().cos()).ln() / PI) / 2.0 * count;
    for z in 0..=12 {
        let count = f64::from(1u32 << z);
        let west = ((w + 180.0) / 360.0 * count).floor() as u32;
        let east = (((e + 180.0) / 360.0 * count).ceil() as u32).min(count as u32);
        let north = row(n, count).floor() as u32;
        let south = (row(s, count).ceil() as u32).min(count as u32);
        for y in north..south {
            let latitudes: Vec<_> = (0..SIZE)
                .map(|py| {
                    (latitude(f64::from(y) + (f64::from(py) + 0.5) / f64::from(SIZE), count) * 1e6).round() as i32
                })
                .collect();
            for x in west..east {
                let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];
                let mut known = false;
                for (py, &lat) in latitudes.iter().enumerate() {
                    for px in 0..SIZE {
                        let lon =
                            ((f64::from(x) + (f64::from(px) + 0.5) / f64::from(SIZE)) / count * 360.0 - 180.0) * 1e6;
                        let height = terrain.height(lat, lon.round() as i32)?;
                        // Whole metres: relief and contours need no finer height, and lossless WebP compresses
                        // fewer distinct values better.
                        let value = height
                            .map(|h| ((h.round() + 32768.0) * 256.0).clamp(0.0, 16777215.0) as u32)
                            .unwrap_or(8388608);
                        let i = (py * SIZE as usize + px as usize) * 4;
                        rgba[i..i + 4].copy_from_slice(&[
                            (value >> 16) as u8,
                            (value >> 8) as u8,
                            value as u8,
                            if height.is_some() { 255 } else { 0 },
                        ]);
                        known |= height.is_some();
                    }
                }
                if known {
                    let mut bytes = Vec::new();
                    WebPEncoder::new_lossless(&mut bytes)
                        .encode(&rgba, SIZE, SIZE, ExtendedColorType::Rgba8)
                        .map_err(|e| e.to_string())?;
                    db.execute("INSERT INTO tiles VALUES (?1,?2,?3,?4)", params![z, x, (1u32 << z) - y - 1, bytes])
                        .map_err(sql)?;
                }
            }
        }
        db.execute_batch("COMMIT; BEGIN;").map_err(sql)?;
        eprintln!("Terrain zoom {z} complete");
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
