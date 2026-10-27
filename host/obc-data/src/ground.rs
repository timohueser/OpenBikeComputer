//! The ground sources of TrailView, `data/trailview/ground.json`, as `specs/trailview-tiles.md`
//! defines them. `obc data` reads only which source each ground layer reads, and which of those a
//! capture fetches.

use std::path::Path;

use serde::Deserialize;

/// The protocols whose files a capture writes: `tools/trailview/ground.py --source ID --fetch`.
const CAPTURED: [&str; 3] = ["arcgis-image", "arcgis-features", "wfs"];

#[derive(Debug, Deserialize)]
pub struct Ground {
    pub material: Layer,
    pub lithology: Layer,
}

#[derive(Debug, Deserialize)]
pub struct Layer {
    pub sources: Vec<GroundSource>,
}

/// One step of a layer's chain: the id of its row in `data/sources.toml` and how a step reads it.
#[derive(Debug, Deserialize)]
pub struct GroundSource {
    pub source: String,
    #[serde(default)]
    pub protocol: Option<String>,
}

impl Ground {
    /// The ground sources of the repository at `root`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("data/trailview/ground.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Whether a capture of `ground.py` fetches the source `id`.
    pub fn captured(&self, id: &str) -> bool {
        self.material.sources.iter().chain(&self.lithology.sources).any(|step| {
            step.source == id && step.protocol.as_deref().is_some_and(|protocol| CAPTURED.contains(&protocol))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_national_maps_and_corine_are_captured_and_the_downloads_are_not() {
        let ground = Ground::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").as_path()).unwrap();
        for id in ["corine-land-cover", "land-cover-ie", "land-cover-gb-sct", "geology-es"] {
            assert!(ground.captured(id), "{id}");
        }
        for id in ["egdi-geology", "esa-worldcover", "osm-planet", "sentinel-2-mosaics"] {
            assert!(!ground.captured(id), "{id}");
        }
    }
}
