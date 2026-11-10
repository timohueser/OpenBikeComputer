//! The request of a cell as the viewer collects it: from a fixture folder (`features/13/C/R.bin` gzip,
//! `terrain/Z/X/Y.webp`) or from the map packs of a local release of `obc data`. It only reads.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use obc_data::engine::Client;
use obc_data::release::{manifest_key, List, Release as Manifest, Serving, SERVING};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::terrain::{pages_from_terrarium, StoredPage, Terrain};
use obc_ground_plan::transport::{FeatureTile, Request};
use obc_trailview_features::pmtiles::Archive;
use serde_json::Value;

use crate::fixture::{self, Fixture};

/// Decoded terrain tiles that a release keeps: about 0.5 MB each.
const TERRAIN_TILES: usize = 64;

/// The pages of a stored terrain tile; None where the release lacks the tile.
type Pages = Option<[Option<StoredPage>; 4]>;

pub enum Source {
    Fixture(Box<Fixture>),
    Release(Box<Release>),
}

/// Where the cells come from, as the driver passes it to its workers.
#[derive(Clone, Debug)]
pub enum Place {
    Fixture(PathBuf),
    Release(PathBuf, Option<String>),
}

impl Place {
    pub fn args(&self) -> Vec<String> {
        match self {
            Place::Fixture(dir) => vec!["--fixture".into(), dir.display().to_string()],
            Place::Release(dir, id) => {
                let mut out = vec!["--release".into(), dir.display().to_string()];
                out.extend(id.iter().flat_map(|id| ["--id".into(), id.clone()]));
                out
            }
        }
    }
}

impl Source {
    pub fn open(place: &Place) -> Result<Source, String> {
        Ok(match place {
            Place::Fixture(dir) => Source::Fixture(Box::new(Fixture::open(dir))),
            Place::Release(dir, id) => Source::Release(Box::new(Release::open(dir, id.as_deref())?)),
        })
    }

    /// The request of a cell, or None where none of its feature tiles exists: the viewer builds no plan
    /// there.
    pub fn request(&mut self, key: CellKey) -> Result<Option<Request>, String> {
        let request = match self {
            Source::Fixture(fixture) => fixture.request(key),
            Source::Release(release) => release.request(key)?,
        };
        Ok((!request.tiles.is_empty()).then_some(request))
    }

    /// The cells to census: the fixture's `cells.json`, or every cell whose square lies inside the
    /// fixture's bounds and whose feature tiles all exist; every cell that meets a release's bounds.
    pub fn cells(&self) -> Result<Vec<CellKey>, String> {
        match self {
            Source::Fixture(fixture) => {
                let listed = fixture.dir().join("cells.json");
                if listed.is_file() {
                    let text = std::fs::read(&listed).map_err(|e| format!("{}: {e}", listed.display()))?;
                    let cells: Vec<Value> = serde_json::from_slice(&text).map_err(|e| e.to_string())?;
                    return cells.iter().map(|c| parse_cell(c["cell"].as_str().unwrap_or(""))).collect();
                }
                let dir = fixture.dir();
                Ok((fixture.cells().into_iter())
                    .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(dir, c, r).is_some()))
                    .collect())
            }
            Source::Release(release) => Ok(cells_meeting(release.bounds)),
        }
    }
}

/// `16/X/Y` or `X/Y`.
pub fn parse_cell(text: &str) -> Result<CellKey, String> {
    let parts: Vec<&str> = text.split('/').collect();
    let [x, y] = parts[parts.len().saturating_sub(2)..] else { return Err(format!("not a cell: {text:?}")) };
    let n = |v: &str| v.parse::<u32>().map_err(|_| format!("not a cell: {text:?}"));
    Ok(CellKey { x: n(x)?, y: n(y)? })
}

/// The zoom-16 cells that meet [west, south, east, north], by row, then column.
pub fn cells_meeting([west, south, east, north]: [f64; 4]) -> Vec<CellKey> {
    use obc_ground_plan::frame::from_lon_lat;
    let (a, b) = (CellKey::of(from_lon_lat(west, north)), CellKey::of(from_lon_lat(east, south)));
    (a.y..=b.y).flat_map(|y| (a.x..=b.x).map(move |x| CellKey { x, y })).collect()
}

/// The map packs of one kind of a release: `maps/tiles/<kind>/...pmtiles` to its object.
struct Packs {
    files: BTreeMap<String, PathBuf>,
    open: BTreeMap<String, Archive>,
}

impl Packs {
    /// The tile's bytes as its pack stores them: the deepest pack that exists whose square holds the
    /// tile, from the tile's zoom back to the base level of its zoom (`specs/planner-offline.md`).
    fn get(&mut self, prefix: &str, z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>, String> {
        let base = match z {
            0..=6 => 0,
            7..=8 => 7,
            _ => 9,
        };
        let Some(name) = (base..=z)
            .rev()
            .map(|level| format!("{prefix}{level}-{}-{}.pmtiles", x >> (z - level), y >> (z - level)))
            .find(|name| self.files.contains_key(name))
        else {
            return Ok(None);
        };
        if !self.open.contains_key(&name) {
            let archive = Archive::open(&self.files[&name]).map_err(|e| format!("{name}: {e}"))?;
            self.open.insert(name.clone(), archive);
        }
        self.open[&name].get(z as u8, x, y).map_err(|e| format!("{name}: {e}"))
    }
}

pub struct Release {
    features: Packs,
    terrain: Packs,
    /// West, south, east, north of the feature layer.
    pub bounds: [f64; 4],
    /// Decoded terrain tiles with the order of their decode.
    pages: BTreeMap<(u32, u32, u32), (u64, Pages)>,
    clock: u64,
}

impl Release {
    /// The release `id` of the target folder `dir`, or the one that it serves.
    pub fn open(dir: &Path, id: Option<&str>) -> Result<Release, String> {
        let read = |key: &str| std::fs::read(dir.join(key)).map_err(|e| format!("{}: {e}", dir.join(key).display()));
        let id = match id {
            Some(id) => id.to_string(),
            None => {
                let serving: Serving = serde_json::from_slice(&read(SERVING)?).map_err(|e| e.to_string())?;
                serving.serving.id
            }
        };
        let manifest = Manifest::parse(&id, &read(&manifest_key(&id))?)?;
        let files = manifest.load(|list: &List| read(&list.key()))?;
        // The published output of a layer: its files hold its pack index.
        let packs = |layer: &str| -> Result<(Packs, Value), String> {
            let output = (files.outputs.iter())
                .find(|o| {
                    o.layer == layer && matches!(&o.client, Client::Paths(p) if p.iter().any(|p| p == "index.json"))
                })
                .ok_or_else(|| format!("release {id} publishes no {layer}"))?;
            let index = output.files.iter().find(|f| f.path == "index.json").ok_or("no index.json")?;
            let index: Value =
                serde_json::from_slice(&read(&format!("objects/{}", index.sha256))?).map_err(|e| e.to_string())?;
            let files = (index["files"].as_object().ok_or("index.json has no files")?.iter())
                .filter(|(name, _)| name.ends_with(".pmtiles"))
                .map(|(name, entry)| {
                    let sha = entry["sha256"].as_str().ok_or_else(|| format!("{name} has no sha256"))?;
                    Ok((name.clone(), dir.join("objects").join(sha)))
                })
                .collect::<Result<_, String>>()?;
            Ok((Packs { files, open: BTreeMap::new() }, output.options.clone()))
        };
        let (features, options) = packs("planner/trailview-features")?;
        let (terrain, _) = packs("planner/terrain")?;
        let bounds: [f64; 4] = serde_json::from_value(options["bounds"].clone())
            .map_err(|_| "planner/trailview-features has no bounds".to_string())?;
        Ok(Release { features, terrain, bounds, pages: BTreeMap::new(), clock: 0 })
    }

    fn request(&mut self, key: CellKey) -> Result<Request, String> {
        let mut tiles = Vec::new();
        for (column, row) in key.feature_tiles() {
            let Some(body) = self.features.get("maps/tiles/trailview-features/", 13, column, row)? else { continue };
            // A body can be the gzip of the tile, as the viewer finds it.
            let bytes = if body.starts_with(&[0x1f, 0x8b]) {
                let mut plain = Vec::new();
                flate2::read::GzDecoder::new(&body[..]).read_to_end(&mut plain).map_err(|e| e.to_string())?;
                plain
            } else {
                body
            };
            tiles.push(FeatureTile { column, row, bytes });
        }
        let mut pages = Vec::new();
        for page in Terrain::wanted(key) {
            let (z, x, y, quadrant) = page.tile();
            if let Some(p) = self.tile_pages(z, x, y)?.and_then(|t| t[quadrant].clone()) {
                pages.push(p);
            }
        }
        Ok(Request { key, tiles, pages })
    }

    fn tile_pages(&mut self, z: u32, x: u32, y: u32) -> Result<Pages, String> {
        self.clock += 1;
        if let Some(entry) = self.pages.get_mut(&(z, x, y)) {
            entry.0 = self.clock;
            return Ok(entry.1.clone());
        }
        let prefix = if z == 13 { "maps/tiles/terrain/13/" } else { "maps/tiles/terrain/" };
        let decoded = match self.terrain.get(prefix, z, x, y)? {
            None => None,
            Some(webp) => {
                let rgba = image::load_from_memory_with_format(&webp, image::ImageFormat::WebP)
                    .map_err(|e| format!("terrain {z}/{x}/{y}: {e}"))?
                    .to_rgba8();
                Some(pages_from_terrarium(rgba.as_raw(), rgba.width() as usize, z, x, y).map_err(|e| e.to_string())?)
            }
        };
        if self.pages.len() >= TERRAIN_TILES {
            let oldest = self.pages.iter().min_by_key(|e| e.1 .0).map(|e| *e.0);
            self.pages.remove(&oldest.unwrap());
        }
        self.pages.insert((z, x, y), (self.clock, decoded.clone()));
        Ok(decoded)
    }
}
