use crate::{overlays, Overlays};
use route_engine::{package::digest, Error, Result};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::Mutex,
};

pub enum OverlaySource {
    Region(Overlays),
    Grid(Grid),
}
pub struct Grid {
    source: String,
    identity: String,
    coverage: [f64; 4],
    files: Vec<(PathBuf, [f64; 4])>,
    cache: Mutex<VecDeque<(usize, Overlays)>>,
}
fn invalid(error: impl std::fmt::Display) -> Error {
    Error::InvalidData(error.to_string())
}
impl OverlaySource {
    pub fn open(root: &Path) -> Result<Self> {
        if !root.join("blocks.json").exists() {
            let bytes = std::fs::read(root.join("manifest.json")).map_err(invalid)?;
            return Ok(Self::Region(Overlays::open(root, &digest(&bytes))?));
        }
        let bytes = std::fs::read(root.join("blocks.json")).map_err(invalid)?;
        let manifest: route_engine::blocks::Manifest = serde_json::from_slice(&bytes).map_err(invalid)?;
        if manifest.format != 2 || !route_engine::table::valid_digest(&manifest.source) {
            return Err(invalid("Invalid overlay routing selection"));
        }
        let mut files = Vec::new();
        let cells: Vec<String> =
            serde_json::from_slice(&std::fs::read(root.join("layers.json")).map_err(invalid)?).map_err(invalid)?;
        for cell in cells {
            let parts: Vec<_> = cell.split('-').collect();
            if parts.len() != 3
                || parts[0] != "9"
                || parts[1..].iter().any(|p| !p.parse::<u32>().is_ok_and(|n| n < 512))
            {
                return Err(invalid("Invalid overlay cell"));
            }
            let path = root.join("layers").join(format!("{cell}.sqlite"));
            if !path.is_file() {
                return Err(Error::MissingRegion(format!("Missing overlay cell {cell}")));
            }
            let overlay = Overlays::open_file(&path, &manifest.source)?;
            files.push((path, overlay.coverage()));
        }
        if files.is_empty() {
            return Err(Error::MissingRegion("No overlay cells are installed".into()));
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Self::Grid(Grid {
            source: manifest.source,
            identity: digest(&bytes),
            coverage: manifest.data.bounds,
            files,
            cache: Mutex::new(VecDeque::new()),
        }))
    }
    pub fn query(&self, params: &HashMap<String, String>, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>> {
        let Self::Grid(grid) = self else {
            let Self::Region(region) = self else { unreachable!() };
            return region.query(params, cancelled);
        };
        let (bounds, _, _, _) = overlays::query_window(params)?;
        let mut features = BTreeMap::new();
        let mut routes = serde_json::Map::new();
        let mut points = 0;
        let mut cache = grid.cache.lock().map_err(|_| Error::Limit)?;
        for (index, (path, coverage)) in grid.files.iter().enumerate() {
            if coverage[0] > bounds[2] || coverage[2] < bounds[0] || coverage[1] > bounds[3] || coverage[3] < bounds[1]
            {
                continue;
            }
            if cancelled() {
                return Err(Error::Cancelled);
            }
            let overlay = if let Some(at) = cache.iter().position(|(id, _)| *id == index) {
                cache.remove(at).unwrap().1
            } else {
                Overlays::open_file(path, &grid.source)?
            };
            let result = overlay.query(params, cancelled);
            cache.push_back((index, overlay));
            while cache.len() > 8 {
                cache.pop_front();
            }
            let mut reply: Value = serde_json::from_slice(&result?).map_err(invalid)?;
            let rows = reply["features"].as_array_mut().ok_or_else(|| invalid("Invalid overlay collection"))?;
            for feature in rows.drain(..) {
                let id = feature["id"].as_i64().ok_or_else(|| invalid("Invalid overlay feature identity"))?;
                if let std::collections::btree_map::Entry::Vacant(entry) = features.entry(id) {
                    points += feature["geometry"]["coordinates"]
                        .as_array()
                        .ok_or_else(|| invalid("Invalid overlay geometry"))?
                        .len();
                    if points > 200_000 {
                        return Err(Error::InvalidRequest(
                            "Zoom in to show route networks and access restrictions.".into(),
                        ));
                    }
                    entry.insert(feature);
                }
            }
            if let Some(entries) = reply["routes"].as_object_mut() {
                routes.extend(std::mem::take(entries));
            }
        }
        serde_json::to_vec(&json!({"type":"FeatureCollection", "features":features.into_values().collect::<Vec<_>>(),
            "routes":routes,"package":grid.identity,"coverage":grid.coverage}))
        .map_err(invalid)
    }
}
