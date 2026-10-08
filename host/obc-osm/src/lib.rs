//! Native OSM source preparation, independent of styled map packing.

use std::path::{Path, PathBuf};
use std::process::Command;

use obc_formats::grid::cell_square;
use serde::{Deserialize, Serialize};

pub mod step;

/// All shipped bands divide this source leaf size exactly.
pub const SOURCE_LEAF_LOG2: u32 = 23;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LeafId {
    pub i: i64,
    pub j: i64,
}

impl LeafId {
    pub fn square(self) -> (i64, i64, i64, i64) {
        cell_square(SOURCE_LEAF_LOG2, self.i, self.j)
    }

    /// The source square and its reference-complete extraction halo.
    pub fn extract_bbox(self) -> [f64; 4] {
        extract_bbox(self.square())
    }
}

/// Clamp the one-microdegree extraction halo to actual geography.
pub fn extract_bbox((w, s, e, n): (i64, i64, i64, i64)) -> [f64; 4] {
    [
        ((w - 1).max(-180_000_000) as f64) / 1e6,
        ((s - 1).max(-90_000_000) as f64) / 1e6,
        ((e + 1).min(180_000_000) as f64) / 1e6,
        ((n + 1).min(90_000_000) as f64) / 1e6,
    ]
}

#[derive(Debug, Clone)]
pub struct ExtractRequest {
    pub output: String,
    pub bbox: [f64; 4],
}

/// Injectable because CI must prove the hierarchy without downloading or
/// requiring Osmium. Production uses [`OsmiumRunner`].
pub trait ShardRunner: Sync {
    fn check(&self) -> Result<(), String>;
    fn split(&self, input: &Path, output_dir: &Path, requests: &[ExtractRequest]) -> Result<(), String>;
}

pub struct OsmiumRunner {
    binary: PathBuf,
}

impl Default for OsmiumRunner {
    fn default() -> Self {
        Self { binary: std::env::var_os("OBC_OSMIUM").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("osmium")) }
    }
}

impl ShardRunner for OsmiumRunner {
    fn check(&self) -> Result<(), String> {
        let out = Command::new(&self.binary)
            .arg("--version")
            .output()
            .map_err(|e| format!("{} is required for OSM preparation: {e}", self.binary.display()))?;
        if !out.status.success() {
            return Err(format!("{} --version failed with {}", self.binary.display(), out.status));
        }
        Ok(())
    }

    fn split(&self, input: &Path, output_dir: &Path, requests: &[ExtractRequest]) -> Result<(), String> {
        #[derive(Serialize)]
        struct Config<'a> {
            extracts: Vec<ConfigExtract<'a>>,
        }
        #[derive(Serialize)]
        struct ConfigExtract<'a> {
            output: &'a str,
            bbox: [f64; 4],
        }
        std::fs::create_dir_all(output_dir).map_err(|e| format!("{}: {e}", output_dir.display()))?;
        let config =
            Config { extracts: requests.iter().map(|r| ConfigExtract { output: &r.output, bbox: r.bbox }).collect() };
        let config_path = output_dir.join("extracts.json");
        let mut text = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        text.push('\n');
        obc_data::store::write_atomic(&config_path, text.as_bytes())?;
        // `-F pbf`: a planet from the store is an object without a file extension.
        let status = Command::new(&self.binary)
            .args(["extract", "-F", "pbf", "--config"])
            .arg(&config_path)
            .args(["--directory"])
            .arg(output_dir)
            .args(["--strategy", "smart", "--set-bounds", "--overwrite", "--verbose"])
            .arg(input)
            .status()
            .map_err(|e| format!("run {} extract: {e}", self.binary.display()))?;
        if !status.success() {
            return Err(format!("{} extract failed with {status}", self.binary.display()));
        }
        for request in requests {
            let path = output_dir.join(&request.output);
            if !path.is_file() {
                return Err(format!("{} did not produce {}", self.binary.display(), path.display()));
            }
        }
        Ok(())
    }
}

impl OsmiumRunner {
    /// The canonical executable binding. Discovery checks its local version.
    pub fn binding(&self) -> Result<obc_data::engine::Library, String> {
        let binary = if self.binary.components().count() > 1 {
            self.binary.clone()
        } else {
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .map(|directory| directory.join(&self.binary))
                .find(|path| path.is_file())
                .ok_or("prepare Osmium, or set OBC_OSMIUM to its executable")?
        };
        let binary = binary.canonicalize().map_err(|e| e.to_string())?;
        let (sha256, _) = obc_data::store::hash_file(&binary)?;
        let output = Command::new(&binary).arg("--version").output().map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err("prepared Osmium --version failed".into());
        }
        Ok(obc_data::engine::Library {
            version: Some(String::from_utf8(output.stdout).map_err(|e| e.to_string())?),
            name: "osmium".into(),
            path: binary,
            sha256,
        })
    }

    /// Use the exact canonical executable selected by the checked request.
    pub fn bound(libraries: &[obc_data::engine::Library]) -> Result<(Self, &obc_data::engine::Library), String> {
        let providers: Vec<_> = libraries.iter().filter(|provider| provider.name == "osmium").collect();
        let [provider] = providers[..] else { return Err("the request needs one named Osmium binding".into()) };
        let runner = Self { binary: provider.path.clone() };
        runner.check_binding(provider)?;
        Ok((runner, provider))
    }

    pub fn check_binding(&self, provider: &obc_data::engine::Library) -> Result<(), String> {
        if !provider.path.is_absolute()
            || provider.path.canonicalize().map_err(|e| format!("prepared Osmium: {e}"))? != provider.path
            || obc_data::store::hash_file(&provider.path)?.0 != provider.sha256
        {
            return Err("prepared Osmium changed; prepare a new plan".into());
        }
        Ok(())
    }

    /// The first line of `osmium --version`.
    pub fn version(&self) -> Result<String, String> {
        let out = Command::new(&self.binary).arg("--version").output();
        let out = out.map_err(|e| format!("{} --version: {e}", self.binary.display()))?;
        if !out.status.success() {
            return Err(format!("{} --version failed with {}", self.binary.display(), out.status));
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
    }

    #[test]
    fn declared_osmium_uses_exact_provider_and_refuses_persistent_replacement() {
        let dir = std::env::temp_dir().join(format!("obc-osmium-binding-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let binary = dir.join("selected-osmium");
        std::fs::write(&binary, "original executable bytes").unwrap();
        let provider = obc_data::engine::Library {
            version: None,
            name: "osmium".into(),
            path: binary.clone(),
            sha256: obc_data::store::hash_file(&binary).unwrap().0,
        };
        assert!(OsmiumRunner::bound(&[]).err().unwrap().contains("one named"));
        let providers = [provider];
        let (runner, binding) = OsmiumRunner::bound(&providers).unwrap();
        assert_eq!(runner.binary, binary, "the callback uses the declared path instead of PATH/OBC_OSMIUM");
        std::fs::write(&binary, "changed executable bytes").unwrap();
        assert!(runner.check_binding(binding).unwrap_err().contains("changed"));
        assert!(OsmiumRunner::bound(&providers).err().unwrap().contains("changed"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn installed_osmium_accepts_the_generated_binary_split_config() {
        let runner = OsmiumRunner::default();
        if runner.check().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("obc-osmium-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let requests = [
            ExtractRequest { output: "teningen.osm.pbf".into(), bbox: [7.0, 47.0, 8.5, 49.0] },
            ExtractRequest { output: "empty.osm.pbf".into(), bbox: [20.0, 20.0, 21.0, 21.0] },
        ];
        runner.split(&repo("builder/tests/corpus/data/tiny.osm.pbf"), &dir, &requests).unwrap();
        assert!(requests.iter().all(|request| dir.join(&request.output).is_file()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn installed_osmium_keeps_identical_leaf_bytes_across_replication_headers() {
        let runner = OsmiumRunner::default();
        if runner.check().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("obc-osmium-replication-headers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = repo("builder/tests/corpus/data/tiny.osm.pbf");
        let make = |name: &str, sequence: &str, timestamp: &str| {
            let path = dir.join(name);
            let status = Command::new(&runner.binary)
                .arg("cat")
                .arg(&source)
                .arg("-o")
                .arg(&path)
                .arg("--overwrite")
                .arg(format!("--output-header=osmosis_replication_sequence_number={sequence}"))
                .arg(format!("--output-header=osmosis_replication_timestamp={timestamp}"))
                .arg("--output-header=osmosis_replication_base_url=https://planet.openstreetmap.org/replication/hour/")
                .status()
                .unwrap();
            assert!(status.success());
            path
        };
        let first = make("first.osm.pbf", "10", "2026-08-01T10:00:00Z");
        let second = make("second.osm.pbf", "11", "2026-08-01T11:00:00Z");
        let request = [ExtractRequest { output: "leaf.osm.pbf".into(), bbox: [7.0, 47.0, 8.5, 49.0] }];
        let first_out = dir.join("first");
        let second_out = dir.join("second");
        runner.split(&first, &first_out, &request).unwrap();
        runner.split(&second, &second_out, &request).unwrap();
        assert_eq!(
            obc_data::store::hash_file(&first_out.join("leaf.osm.pbf")).unwrap(),
            obc_data::store::hash_file(&second_out.join("leaf.osm.pbf")).unwrap(),
            "replication-only source header changes must not invalidate every geographic leaf"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
