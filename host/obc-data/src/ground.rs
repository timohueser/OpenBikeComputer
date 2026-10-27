//! The ground sources of TrailView, `data/trailview/ground.json`, as `specs/trailview-tiles.md`
//! defines them. `obc data` reads which source each ground layer reads, and the keys that decide
//! what a capture of a source fetches.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

/// The keys of an entry that change what its capture fetches; `legend` too where the capture
/// filters by it.
const CAPTURE_KEYS: [&str; 7] = ["protocol", "layer", "field", "sort", "crs", "px_m", "zoom"];

#[derive(Debug, Deserialize)]
pub struct Ground {
    pub material: Layer,
    pub lithology: Layer,
}

#[derive(Debug, Deserialize)]
pub struct Layer {
    pub sources: Vec<GroundSource>,
}

/// One step of a layer's chain: the id of its row in `data/sources.toml` and the other keys of its
/// entry.
#[derive(Debug, Deserialize)]
pub struct GroundSource {
    pub source: String,
    #[serde(flatten)]
    pub keys: BTreeMap<String, Value>,
}

impl Ground {
    /// The ground sources of the repository at `root`.
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("data/trailview/ground.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The first entry of the source `id`.
    pub fn entry(&self, id: &str) -> Option<&GroundSource> {
        self.material.sources.iter().chain(&self.lithology.sources).find(|step| step.source == id)
    }
}

impl GroundSource {
    /// The first 12 hex digits of the SHA-256 of the keys that decide what a capture fetches. A
    /// capture request names it, so a stored capture with other keys is not read again.
    pub fn capture_digest(&self) -> String {
        let filtered = self.keys.get("protocol").and_then(Value::as_str) == Some("arcgis-features");
        let keys: BTreeMap<&str, &Value> = self
            .keys
            .iter()
            .filter(|(key, _)| CAPTURE_KEYS.contains(&key.as_str()) || (filtered && key.as_str() == "legend"))
            .map(|(key, value)| (key.as_str(), value))
            .collect();
        let text = serde_json::to_string(&keys).expect("JSON values serialize");
        crate::store::sha256_hex(text.as_bytes())[..12].to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capture_digest_follows_what_the_capture_fetches_and_not_how_the_layer_combines() {
        let entry = |json: &str| serde_json::from_str::<GroundSource>(json).unwrap().capture_digest();
        let base = entry(r#"{"source": "x", "protocol": "arcgis-features", "field": "F", "combine": "authority"}"#);
        assert_eq!(base, entry(r#"{"source": "x", "protocol": "arcgis-features", "field": "F", "combine": "refine"}"#));
        assert_ne!(
            base,
            entry(r#"{"source": "x", "protocol": "arcgis-features", "field": "G", "combine": "authority"}"#)
        );
        assert_ne!(base, entry(r#"{"source": "x", "protocol": "arcgis-features", "field": "F", "zoom": 11}"#));
        let image = |legend: &str| {
            entry(&format!(
                r#"{{"source": "x", "protocol": "arcgis-image", "legend": {{"classes": [["1", "{legend}"]]}}}}"#
            ))
        };
        assert_eq!(image("heath"), image("bog"), "the legend of a map image does not filter its capture");
    }
}
