//! The TrailView fixture as cell inputs: its zoom-13 feature tiles (gzip) and its Terrarium WebP
//! terrain tiles. The fixture tests, the debug tool and the census of `obc-trailview-features` share it.

// Each of its users reads a part of it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use obc_ground_plan::frame::{from_lon_lat, CellKey};
use obc_ground_plan::terrain::{pages_from_terrarium, PageKey, StoredPage, Terrain};
use obc_ground_plan::tile::{DecodeError, TileCache};
use obc_ground_plan::transport::{FeatureTile, Request};
use obc_ground_plan::CellInput;

/// The workers of `Fixture::map`.
const THREADS: usize = 4;

/// The decoded pages of a terrain tile z/x/y; None where the fixture lacks the tile.
type Pages = Option<[Option<StoredPage>; 4]>;

pub struct Fixture {
    dir: PathBuf,
    /// West, south, east, north in degrees.
    pub bounds: [f64; 4],
    pub cache: TileCache,
    terrain: BTreeMap<(u32, u32, u32), Pages>,
}

/// The fixture of `builder/web`.
pub fn default_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../builder/web/tests/browser/trailview/fixture")
}

/// The bytes after gzip of feature tile 13/column/row, or None where the fixture lacks it.
pub fn feature_tile(dir: &Path, column: u32, row: u32) -> Option<Vec<u8>> {
    let gzip = std::fs::read(dir.join(format!("features/13/{column}/{row}.bin"))).ok()?;
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(&gzip[..]).read_to_end(&mut bytes).expect("a fixture tile is gzip");
    Some(bytes)
}

impl Fixture {
    pub fn open(dir: &Path) -> Fixture {
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("config.json")).expect("the fixture's config.json"))
                .unwrap();
        let bounds = std::array::from_fn(|i| config["bounds"][i].as_f64().expect("bounds"));
        Fixture { dir: dir.to_path_buf(), bounds, cache: TileCache::new(16), terrain: BTreeMap::new() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn tile(&self, column: u32, row: u32) -> Option<Vec<u8>> {
        feature_tile(&self.dir, column, row)
    }

    /// The pages of `keys` that the fixture holds. Each WebP decodes once.
    pub fn pages(&mut self, keys: &[PageKey]) -> Vec<StoredPage> {
        keys.iter()
            .filter_map(|&key| {
                let (z, x, y, quadrant) = key.tile();
                let path = self.dir.join(format!("terrain/{z}/{x}/{y}.webp"));
                let pages = self.terrain.entry((z, x, y)).or_insert_with(|| {
                    let bytes = std::fs::read(path).ok()?;
                    let rgba = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP)
                        .expect("a fixture terrain tile is WebP")
                        .to_rgba8();
                    Some(pages_from_terrarium(rgba.as_raw(), rgba.width() as usize, z, x, y).expect("512 px"))
                });
                pages.as_ref()?[quadrant].clone()
            })
            .collect()
    }

    pub fn try_cell(&mut self, key: CellKey) -> Result<CellInput, DecodeError> {
        let pages = self.pages(&Terrain::wanted(key));
        let dir = self.dir.clone();
        CellInput::new(key, &mut self.cache, |column, row| feature_tile(&dir, column, row), pages)
    }

    /// The request of a cell: each of its feature tiles that the fixture holds, and its pages.
    pub fn request(&mut self, key: CellKey) -> Request {
        let tiles = (key.feature_tiles().into_iter())
            .filter_map(|(column, row)| Some(FeatureTile { column, row, bytes: self.tile(column, row)? }))
            .collect();
        Request { key, tiles, pages: self.pages(&Terrain::wanted(key)) }
    }

    pub fn cell(&mut self, key: CellKey) -> CellInput {
        self.try_cell(key).expect("the fixture's tiles decode")
    }

    /// `f` of each cell and its input on `THREADS` workers, in the order of `cells`. The workers take
    /// the next cell as they finish one; the inputs read one at a time.
    pub fn map<T: Send>(&mut self, cells: &[CellKey], f: impl Fn(CellKey, CellInput) -> T + Sync) -> Vec<T> {
        let (next, fixture) = (AtomicUsize::new(0), Mutex::new(self));
        let mut done: Vec<(usize, T)> = std::thread::scope(|s| {
            let worker = || {
                let mut done = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&key) = cells.get(i) else { break done };
                    let input = fixture.lock().unwrap().cell(key);
                    done.push((i, f(key, input)));
                }
            };
            let workers: Vec<_> = (0..THREADS).map(|_| s.spawn(worker)).collect();
            workers.into_iter().flat_map(|w| w.join().unwrap()).collect()
        });
        done.sort_by_key(|d| d.0);
        done.into_iter().map(|d| d.1).collect()
    }

    /// The cells whose squares lie inside the bounds.
    pub fn cells(&self) -> Vec<CellKey> {
        let [west, south, east, north] = self.bounds;
        let (a, b) = (CellKey::of(from_lon_lat(west, north)), CellKey::of(from_lon_lat(east, south)));
        (a.y + 1..b.y).flat_map(|y| (a.x + 1..b.x).map(move |x| CellKey { x, y })).collect()
    }
}

/// Longitude and latitude of a global point, for tables only: no byte of a plan depends on it.
#[allow(clippy::disallowed_methods)]
pub fn lon_lat(x: f64, y: f64) -> [f64; 2] {
    let square = (1u64 << obc_ground_plan::frame::FRAME_BITS) as f64;
    let lat = (core::f64::consts::PI * (1.0 - 2.0 * y / square)).sinh().atan();
    [x / square * 360.0 - 180.0, lat.to_degrees()]
}

/// The levels that a cell owns, for the census tables: each crossing with a ladder, each run of a
/// way with a state (from its middle sample's cell), each end line; places in degrees, classes as
/// rows of `roads.json`.
pub fn levels_detail(g: &obc_ground_plan::graph::WayGraph) -> serde_json::Value {
    use serde_json::json;
    let o = g.cell.origin();
    let at = |p: [i32; 2]| [o.x as f64 + f64::from(p[0]), o.y as f64 + f64::from(p[1])];
    let owned = |p: [f64; 2]| g.cell.contains(obc_ground_plan::frame::GlobalPoint { x: p[0] as i64, y: p[1] as i64 });
    let way = |w: u32| {
        let way = &g.ways[w as usize];
        json!([format!("{:?}", way.family), way.class, format!("{:?}", way.level), w])
    };
    let crossings: Vec<_> = (g.crossings.iter())
        .filter(|c| c.owned)
        .filter_map(|c| {
            let l = c.ladder.as_ref()?;
            let p = at(c.pos);
            Some(json!({"at": lon_lat(p[0], p[1]), "kind": format!("{:?}", c.kind), "upper": way(l.upper),
                "lower": way(l.lower), "room_m": (l.room_m * 100.0).round() / 100.0, "full_m": l.full_m, "least_m": l.least_m,
                "step": format!("{:?}", l.step), "ends": l.ends.map(way)}))
        })
        .collect();
    let mut runs = Vec::new();
    for r in &g.levels.runs {
        let w = &g.ways[r.way as usize];
        let inside: Vec<[f64; 2]> = (g.samples[w.samples.start as usize..w.samples.end as usize].iter())
            .filter(|s| (s.span, s.u) >= (r.from.span, r.from.u) && (s.span, s.u) <= (r.to.span, r.to.u))
            .map(|s| at(s.pos))
            .collect();
        let Some(mid) = inside.get(inside.len() / 2) else { continue };
        if owned(*mid) {
            let storey = r.sheet.map_or(0, |s| g.levels.sheets[s as usize].storey);
            let ends = [inside[0], inside[inside.len() - 1]].map(|p| lon_lat(p[0], p[1]));
            runs.push(json!({"way": way(r.way), "state": format!("{:?}", r.state), "storey": storey, "from": ends[0], "to": ends[1]}));
        }
    }
    let lines: Vec<_> = (g.levels.end_lines.iter())
        .filter_map(|l| {
            let (a, b) = (at(l.ends[0]), at(l.ends[1]));
            let c = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
            owned(c).then(|| json!({"at": lon_lat(c[0], c[1]), "kind": format!("{:?}", l.kind), "ways": l.ways.iter().map(|w| w.0).collect::<Vec<_>>()}))
        })
        .collect();
    json!({"crossings": crossings, "runs": runs, "end_lines": lines})
}
