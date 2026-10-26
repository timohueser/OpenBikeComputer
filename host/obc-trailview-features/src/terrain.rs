//! The master terrain: Terrarium WebP tiles of 512 px, R × 256 + G + B / 256 − 32768 metres,
//! unknown unless opaque. A point reads the deepest zoom that knows its four pixels. The heights
//! are those of `tools/trailview/profiles.py` to the bit: the same f32 arithmetic in the same order.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::pmtiles::Archive;
use crate::Terrain;

const TILE: usize = 512;
/// Decoded tiles of 1 MB that the threads share. A tile that leaves the cache costs one more decode
/// when a job reads it again: the road solver reads its samples in the order of the tiles.
const CACHED: usize = 256;

type Key = (u8, u32, u32);
/// The heights of a tile, row by row; None where the source has no tile.
type Heights = Option<Arc<Vec<f32>>>;

pub struct Tiles {
    /// The archives of the grid by name (`{z}-{x}-{y}`, or `13/{z}-{x}-{y}` for zoom 13), opened on use.
    files: HashMap<String, PathBuf>,
    open: Mutex<HashMap<String, Arc<Archive>>>,
    top: u8,
    cache: Mutex<Cache>,
}

#[derive(Default)]
struct Cache {
    /// Each tile with the clock of its last use.
    tiles: HashMap<Key, (Arc<Vec<f32>>, u64)>,
    /// The tiles that the source lacks, apart: they take no memory to speak of, and a point past
    /// the terrain asks for one at each zoom.
    missing: HashSet<Key>,
    clock: u64,
}

impl Tiles {
    /// The terrain of the grid of `planner/terrain` ({path: file}): a tile is in the archive of its
    /// zoom-11 tile, a tile of zoom 13 in the archive of `13/`, and a tile of a lower zoom in its own.
    pub fn grid(files: &BTreeMap<String, PathBuf>) -> Result<Self, String> {
        let index = files.get("index.json").ok_or("the grid has no index.json")?;
        let text = std::fs::read(index).map_err(|e| format!("{}: {e}", index.display()))?;
        let index: serde_json::Value =
            serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", index.display()))?;
        let entries = index["files"].as_object().ok_or("index.json has no files")?;
        let mut archives = HashMap::new();
        for (name, entry) in entries {
            let Some(part) = name.strip_prefix("maps/tiles/terrain/").and_then(|n| n.strip_suffix(".pmtiles")) else {
                continue;
            };
            let sha = entry["sha256"].as_str().ok_or_else(|| format!("{name} has no sha256"))?;
            let file =
                files.get(&format!("objects/{sha}")).ok_or_else(|| format!("the grid lacks the object of {name}"))?;
            archives.insert(part.to_string(), file.clone());
        }
        let top = if archives.keys().any(|part| part.starts_with("13/")) { 13 } else { 12 };
        Ok(Self { files: archives, open: Mutex::default(), top, cache: Mutex::default() })
    }

    /// TEMPORARY until job.rs goes: there is no MBTiles terrain.
    pub fn mbtiles(path: &std::path::Path) -> Result<Self, String> {
        Err(format!("{}: the terrain is a grid", path.display()))
    }

    /// The encoded tile, None where the grid has none.
    fn read(&self, (z, x, y): Key) -> Result<Option<Vec<u8>>, String> {
        let level = z.min(11);
        let shift = z - level;
        let part = format!("{level}-{}-{}", x >> shift, y >> shift);
        let part = if z == 13 { format!("13/{part}") } else { part };
        let archive = {
            let mut open = self.open.lock().unwrap();
            match open.get(&part) {
                Some(archive) => archive.clone(),
                None => {
                    let Some(path) = self.files.get(&part) else { return Ok(None) };
                    let archive = Archive::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                    if archive.tile_compression != 1 {
                        return Err(format!(
                            "{}: tile compression {}, not none",
                            path.display(),
                            archive.tile_compression
                        ));
                    }
                    let archive = Arc::new(archive);
                    open.insert(part, archive.clone());
                    archive
                }
            }
        };
        archive.get(z, x, y)
    }

    /// The heights of a tile, decoded outside the lock of the cache.
    fn tile(&self, key: Key) -> Heights {
        {
            let mut cache = self.cache.lock().unwrap();
            cache.clock += 1;
            let clock = cache.clock;
            if let Some((heights, used)) = cache.tiles.get_mut(&key) {
                *used = clock;
                return Some(heights.clone());
            }
            if cache.missing.contains(&key) {
                return None;
            }
        }
        let data = self.read(key).unwrap_or_else(|e| panic!("terrain tile {key:?}: {e}"));
        let mut cache = self.cache.lock().unwrap();
        let Some(data) = data else {
            cache.missing.insert(key);
            return None;
        };
        drop(cache);
        let heights = Arc::new(decode(&data).unwrap_or_else(|e| panic!("terrain tile {key:?}: {e}")));
        let mut cache = self.cache.lock().unwrap();
        cache.clock += 1;
        let clock = cache.clock;
        cache.tiles.insert(key, (heights.clone(), clock));
        if cache.tiles.len() > CACHED {
            let oldest = cache.tiles.iter().min_by_key(|(_, (_, used))| *used).map(|(key, _)| *key);
            cache.tiles.remove(&oldest.unwrap());
        }
        Some(heights)
    }
}

impl Terrain for Tiles {
    fn top(&self) -> u32 {
        self.top.into()
    }

    fn heights(&self, points: &[[f64; 2]]) -> Vec<f64> {
        // The last tiles that this call read, so that points sorted by tile take the lock of the
        // cache about once per tile.
        let mut recent: Vec<(Key, Heights)> = Vec::with_capacity(4);
        let mut pixel = |z: u8, i: i64, j: i64| -> f32 {
            let key = (z, (i / TILE as i64) as u32, (j / TILE as i64) as u32);
            let heights = match recent.iter().position(|(k, _)| *k == key) {
                Some(at) => recent[at].1.clone(),
                None => {
                    let heights = self.tile(key);
                    if recent.len() == 4 {
                        recent.remove(0);
                    }
                    recent.push((key, heights.clone()));
                    heights
                }
            };
            match &heights {
                Some(h) => h[(j as usize % TILE) * TILE + i as usize % TILE],
                None => f32::NAN,
            }
        };
        points
            .iter()
            .map(|&[x, y]| {
                for z in (0..=self.top).rev() {
                    let n = ((1i64 << z) * TILE as i64) as f64;
                    let (u, v) = (x * n - 0.5, y * n - 0.5);
                    let (fi, fj) = (u.floor(), v.floor());
                    let (fu, fv) = ((u - fi) as f32, (v - fj) as f32);
                    let (i, j) = (fi as i64, fj as i64);
                    let last = n as i64 - 1;
                    let (i0, i1, j0, j1) =
                        (i.clamp(0, last), (i + 1).clamp(0, last), j.clamp(0, last), (j + 1).clamp(0, last));
                    let h = (pixel(z, i0, j0) * (1.0 - fu) + pixel(z, i1, j0) * fu) * (1.0 - fv)
                        + (pixel(z, i0, j1) * (1.0 - fu) + pixel(z, i1, j1) * fu) * fv;
                    if h.is_finite() {
                        return h as f64;
                    }
                }
                f64::NAN
            })
            .collect()
    }
}

/// The heights of a Terrarium WebP tile, NaN where it is not opaque.
fn decode(data: &[u8]) -> Result<Vec<f32>, String> {
    let image = image::load_from_memory_with_format(data, image::ImageFormat::WebP).map_err(|e| e.to_string())?;
    let rgba = image.to_rgba8();
    if rgba.dimensions() != (TILE as u32, TILE as u32) {
        return Err(format!("the tile is {:?} pixels, not {TILE}", rgba.dimensions()));
    }
    Ok(rgba
        .pixels()
        .map(|p| {
            let [r, g, b, a] = p.0.map(f32::from);
            if a < 255.0 {
                f32::NAN
            } else {
                r * 256.0 + g + b / 256.0 - 32768.0
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pmtiles::{self, tile_id, Header};
    use std::io::Write;

    /// A tile of heights h(i, j) metres at pixel (i, j), transparent where h is None.
    fn webp(h: impl Fn(usize, usize) -> Option<f64>) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(TILE * TILE * 4);
        for j in 0..TILE {
            for i in 0..TILE {
                let code = ((h(i, j).unwrap_or(0.0) + 32768.0) * 256.0).round() as u32;
                rgba.extend([
                    (code >> 16) as u8,
                    (code >> 8) as u8,
                    code as u8,
                    if h(i, j).is_some() { 255 } else { 0 },
                ]);
            }
        }
        let mut out = Vec::new();
        image::codecs::webp::WebPEncoder::new_lossless(&mut out)
            .encode(&rgba, TILE as u32, TILE as u32, image::ExtendedColorType::Rgba8)
            .unwrap();
        out
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("obc-trailview-terrain-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn varints(out: &mut Vec<u8>, values: impl IntoIterator<Item = u64>) {
        for mut value in values {
            while value >= 0x80 {
                out.push(value as u8 | 0x80);
                value >>= 7;
            }
            out.push(value as u8);
        }
    }

    /// A gzip directory of (tile id, offset, length, run length).
    fn directory(entries: &[(u64, u64, u64, u64)]) -> Vec<u8> {
        let mut raw = Vec::new();
        varints(&mut raw, [entries.len() as u64]);
        let ids: Vec<u64> = entries.iter().map(|e| e.0).collect();
        varints(&mut raw, ids.iter().enumerate().map(|(k, id)| id - if k > 0 { ids[k - 1] } else { 0 }));
        varints(&mut raw, entries.iter().map(|e| e.3));
        varints(&mut raw, entries.iter().map(|e| e.2));
        varints(&mut raw, entries.iter().map(|e| e.1 + 1));
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&raw).unwrap();
        gzip.finish().unwrap()
    }

    #[test]
    fn a_grid_tile_of_zoom_12_reads_from_its_zoom_11_archive_through_a_leaf_directory() {
        let dir = scratch("grid");
        let (z, x, y) = (12u8, 2131u32, 1439u32);
        let tile = webp(|i, j| Some(100.0 + i as f64 + j as f64 / 4.0));
        // A run of two tiles of one content, in a leaf directory that the root points to.
        let leaf = directory(&[(tile_id(z, x, y) - 1, 0, tile.len() as u64, 2)]);
        let root = directory(&[(tile_id(z, x, y) - 1, 0, leaf.len() as u64, 0)]);
        let mut header = [0u8; 127];
        header[..7].copy_from_slice(b"PMTiles");
        header[7] = 3;
        let words = [
            127,
            root.len() as u64,
            0,
            0,
            127 + root.len() as u64,
            leaf.len() as u64,
            127 + (root.len() + leaf.len()) as u64,
        ];
        for (k, word) in words.into_iter().enumerate() {
            header[8 + 8 * k..16 + 8 * k].copy_from_slice(&word.to_le_bytes());
        }
        (header[97], header[98], header[99]) = (2, 1, 4);
        std::fs::write(dir.join("archive"), [&header[..], &root, &leaf, &tile].concat()).unwrap();
        let index = r#"{"files": {"maps/tiles/terrain/11-1065-719.pmtiles": {"sha256": "ab"}, "maps/other.json": {"sha256": "cd"}}}"#;
        std::fs::write(dir.join("index.json"), index).unwrap();
        let files =
            BTreeMap::from([("index.json".into(), dir.join("index.json")), ("objects/ab".into(), dir.join("archive"))]);
        let terrain = Tiles::grid(&files).unwrap();
        assert_eq!(terrain.top, 12);
        let n = (1u64 << z) as f64 * TILE as f64;
        let at = |i: f64, j: f64| [(x as f64 * TILE as f64 + i + 0.5) / n, (y as f64 * TILE as f64 + j + 0.5) / n];
        assert_eq!(terrain.heights(&[at(10.0, 20.0), at(10.5, 20.0)]), [115.0, 115.5]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_point_reads_the_deepest_zoom_that_knows_its_four_pixels() {
        let dir = scratch("fallback");
        let header = |zoom| Header {
            tile_type: 4,
            tile_compression: 1,
            bounds_e7: [0; 4],
            center_zoom: zoom,
            center_e7: [0; 2],
        };
        // Zoom 1 knows only its tile (0, 0), in the north-west, and not its pixel (200, 300).
        let low = webp(|i, j| ((i, j) != (200, 300)).then_some(10.0 + i as f64 / 2.0));
        pmtiles::write(&dir.join("zero"), &[(tile_id(0, 0, 0), webp(|_, _| Some(100.0)))], &header(0), "{}").unwrap();
        pmtiles::write(&dir.join("one"), &[(tile_id(1, 0, 0), low)], &header(1), "{}").unwrap();
        let index = r#"{"files": {"maps/tiles/terrain/0-0-0.pmtiles": {"sha256": "zero"}, "maps/tiles/terrain/1-0-0.pmtiles": {"sha256": "one"}}}"#;
        std::fs::write(dir.join("index.json"), index).unwrap();
        let files = BTreeMap::from([
            ("index.json".into(), dir.join("index.json")),
            ("objects/zero".into(), dir.join("zero")),
            ("objects/one".into(), dir.join("one")),
        ]);
        let terrain = Tiles::grid(&files).unwrap();
        let n = 2.0 * TILE as f64;
        let at = |i: f64, j: f64| [(i + 0.5) / n, (j + 0.5) / n];
        let heights = terrain.heights(&[
            at(4.25, 7.0),
            // The left edge reads its column twice.
            at(-0.5, 7.0),
            at(199.5, 300.0),
            // The east neighbour tile at zoom 1 is unknown: the pixel pair across the edge falls back.
            at(511.5, 7.0),
            at(700.0, 7.0),
        ]);
        assert_eq!(heights, [12.125, 10.0, 100.0, 100.0, 100.0]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
