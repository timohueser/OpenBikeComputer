//! Fixture packages wrap ordinary region builds and keep their own source selections.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::engine::release::Release;
use crate::engine::{Input, LayerFile};
use crate::env::Env;
use crate::product::{Product, Steps, Unplanned};
use crate::regions::{Area, Regions};
use crate::store::{hash_file, sha256_hex, Store};

mod records;
pub use records::{saved, Saved};

pub type Assemble = fn(&Release, &Store, &Path) -> Result<(), String>;
pub type Recipes = fn(&Env, &Regions, &Store, &Inputs) -> Result<Steps, Unplanned>;

/// The producer worker supplies its existing map product and checked device assembler.
pub struct FixtureCollection {
    pub maps: &'static dyn Product,
    pub recipes: Recipes,
    pub assemble: Assemble,
}

/// Exact imported inputs of one package, separate from producer receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inputs {
    pub osm: CapturedInput,
    pub osm_sha256: String,
    pub content: BTreeMap<String, CapturedInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapturedInput {
    pub source: String,
    pub version: String,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub catalog: LayerFile,
    /// The exact package declarations and canonical regions used for this review.
    pub configuration: String,
    pub destination: String,
    pub packages: BTreeMap<String, PackagePlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PackagePlan {
    pub region: String,
    pub bootstrap: LayerFile,
    pub sources: BTreeMap<String, String>,
    pub inputs: Option<Inputs>,
    pub work: Option<crate::engine::plan::Plan>,
    pub blocked: Vec<String>,
}

impl Plan {
    pub fn check(&self) -> Result<(), String> {
        if self.catalog.path != "fixtures/catalog.toml" || self.packages.is_empty() {
            return Err("fixture apply needs a reviewed catalog and package selection".into());
        }
        digest(&self.catalog.sha256)?;
        digest(&self.configuration)?;
        if self.destination.is_empty() {
            return Err("fixture apply has no isolated destination".into());
        }
        for (id, package) in &self.packages {
            if !crate::is_kebab(id) || !crate::is_kebab(&package.region) {
                return Err("fixture apply has an invalid package or region".into());
            }
            if !package.blocked.is_empty() || package.inputs.is_none() || package.work.is_none() {
                return Err(format!("fixture {id} requires preparation and a complete review"));
            }
            digest(&package.bootstrap.sha256)?;
            let inputs = package.inputs.as_ref().expect("checked above");
            digest(&inputs.osm_sha256)?;
            for input in std::iter::once(&inputs.osm).chain(inputs.content.values()) {
                input.check()?;
            }
        }
        Ok(())
    }

    pub fn catalog_unchanged(&self, root: &Path) -> Result<(), String> {
        if hash_file(&root.join("fixtures/catalog.toml"))? != (self.catalog.sha256.clone(), self.catalog.size) {
            return Err("fixture catalog changed; review a new plan before its update".into());
        }
        Ok(())
    }
}

impl CapturedInput {
    pub fn check(&self) -> Result<(), String> {
        if !crate::is_kebab(&self.source)
            || self.version.is_empty()
            || self.version.contains('/')
            || self.version.chars().any(char::is_control)
        {
            return Err("fixture captured input has no normalized source and version".into());
        }
        let mut selected = std::collections::BTreeSet::new();
        for file in &self.files {
            relative(file)?;
            if !selected.insert(file) {
                return Err("fixture captured input names a file twice".into());
            }
        }
        Ok(())
    }

    pub fn input(&self) -> Input {
        Input::Snapshot {
            source: self.source.clone(),
            version: self.version.clone(),
            params: Vec::new(),
            files: self.files.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    pub packages: BTreeMap<String, Package>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub region: String,
    pub map: String,
    pub osm: String,
    /// The original build record supplies bootstrap inputs, never producer receipts.
    pub bootstrap: String,
    /// Explicit raw source selections; historical baked terrain is not a raw DEM version.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Archive destination to a registered captured source.
    #[serde(default)]
    pub assets: BTreeMap<String, String>,
}

impl Collection {
    pub fn load(root: &Path, regions: &Regions) -> Result<Self, String> {
        let path = root.join("data/env/fixtures.toml");
        let collection: Self = toml::from_str(&std::fs::read_to_string(&path).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if collection.packages.is_empty() {
            return Err("fixtures names no packages".into());
        }
        for (id, package) in &collection.packages {
            if !crate::is_kebab(id) {
                return Err(format!("fixture package {id:?} is not a normalized name"));
            }
            match regions.get(&package.region).map(|region| &region.area) {
                Some(Area::Box { .. }) => {}
                _ => return Err(format!("fixture {id} needs a canonical box region")),
            }
            relative(&package.map)?;
            relative(&package.osm)?;
            relative(&package.bootstrap)?;
            for (source, version) in &package.sources {
                if !crate::is_kebab(source) || version.is_empty() {
                    return Err(format!("fixture {id} has an invalid raw source selection"));
                }
            }
            for (destination, source) in &package.assets {
                relative(destination)?;
                if destination == &package.map || !crate::is_kebab(source) {
                    return Err(format!("fixture {id} has a conflicting map path or invalid asset source"));
                }
            }
        }
        Ok(collection)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Bootstrap {
    pub bounds_lon_lat: [f64; 4],
    pub source_packages: BTreeMap<String, String>,
    pub source_pbf: Option<Captured>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Captured {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

impl Package {
    pub fn bootstrap(&self, root: &Path, regions: &Regions) -> Result<(LayerFile, Bootstrap), String> {
        relative(&self.bootstrap)?;
        let path = root.join(&self.bootstrap);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let bootstrap: Bootstrap = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let region = regions.get(&self.region).ok_or("fixture region is missing")?;
        let Area::Box { bbox } = &region.area else { return Err("fixture region is not a box".into()) };
        if bootstrap.bounds_lon_lat != [bbox.west, bbox.south, bbox.east, bbox.north] {
            return Err(format!("{}: bootstrap bounds differ from region {}", self.bootstrap, self.region));
        }
        if let Some(pbf) = &bootstrap.source_pbf {
            relative(&pbf.path)?;
            digest(&pbf.sha256)?;
            if pbf.path != self.osm {
                return Err("fixture OSM selection differs from its bootstrap record".into());
            }
        }
        for (id, hash) in &bootstrap.source_packages {
            if !crate::is_kebab(id) {
                return Err(format!("bootstrap input package {id:?} is not normalized"));
            }
            digest(hash)?;
        }
        Ok((
            LayerFile { path: self.bootstrap.clone(), size: bytes.len() as u64, sha256: sha256_hex(&bytes) },
            bootstrap,
        ))
    }
}

impl Captured {
    /// A transformed capture keeps its stored digest; its upstream digest is not interchangeable.
    pub fn check(&self, path: &Path) -> Result<(), String> {
        let (digest, bytes) = hash_file(path)?;
        if digest != self.sha256 || bytes != self.bytes {
            return Err(format!("{}: exact fixture input {} is missing or changed", path.display(), self.sha256));
        }
        Ok(())
    }
}

pub fn relative(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(format!("fixture path {path:?} is not a relative file path"));
    }
    Ok(())
}

pub(crate) fn digest(hash: &str) -> Result<(), String> {
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)) {
        return Err("fixture input has no exact SHA-256".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    #[test]
    fn package_bootstraps_match_the_canonical_regions_and_original_stored_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let regions = Regions::load(&root).unwrap();
        let collection = Collection::load(&root, &regions).unwrap();
        assert_eq!(collection.packages.len(), 6);
        for package in collection.packages.values() {
            let (record, bootstrap) = package.bootstrap(&root, &regions).unwrap();
            assert_eq!(hash_file(&root.join(&record.path)).unwrap(), (record.sha256, record.size));
            assert!(!bootstrap.source_packages.is_empty() || package.region == "freiburg");
        }
        let (_, west_cork) = collection.packages["sim-assistant-west-cork"].bootstrap(&root, &regions).unwrap();
        assert_eq!(
            west_cork.source_pbf.unwrap().sha256,
            "1c4a40f4887b23854e618ad3c5e38777e034941014b042046dafe494896876d5"
        );
        let temporary = Scratch::new("fixture-bootstrap-stored");
        let path = temporary.0.join("crop.osm.pbf");
        std::fs::write(&path, b"stored cropped bytes").unwrap();
        let (sha256, bytes) = hash_file(&path).unwrap();
        let capture = Captured { path: "crop.osm.pbf".into(), bytes, sha256 };
        capture.check(&path).unwrap();
        assert!(Captured { sha256: sha256_hex(b"full upstream bytes"), ..capture }.check(&path).is_err());
    }
}
