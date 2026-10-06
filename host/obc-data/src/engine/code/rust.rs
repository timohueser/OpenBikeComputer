//! The resolved normal and build closure of declared producer crates.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

type Selected = (BTreeSet<PathBuf>, BTreeMap<String, String>);

#[derive(Deserialize)]
pub(super) struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
    #[serde(skip)]
    checksums: BTreeMap<(String, String, String), String>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    version: String,
    edition: String,
    source: Option<String>,
    manifest_path: PathBuf,
    #[serde(flatten)]
    settings: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<Dependency>,
    features: Vec<String>,
}

#[derive(Deserialize)]
struct Dependency {
    pkg: String,
    dep_kinds: Vec<Kind>,
}

#[derive(Deserialize)]
struct Kind {
    kind: Option<String>,
}

#[derive(Deserialize)]
struct Lock {
    package: Vec<Locked>,
}

#[derive(Deserialize)]
struct Locked {
    name: String,
    version: String,
    source: Option<String>,
    checksum: Option<String>,
}

impl Metadata {
    pub fn load(root: &Path) -> Result<Self, String> {
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let host = Command::new(rustc).arg("-vV").output().map_err(|error| format!("rustc: {error}"))?;
        if !host.status.success() {
            return Err(format!("rustc: {}", String::from_utf8_lossy(&host.stderr).trim()));
        }
        let host = String::from_utf8_lossy(&host.stdout);
        let host = host.lines().find_map(|line| line.strip_prefix("host: ")).ok_or("rustc names no host target")?;
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args(["metadata", "--format-version", "1", "--locked", "--offline", "--filter-platform", host])
            .current_dir(root)
            .output()
            .map_err(|error| format!("cargo metadata: {error}"))?;
        if !output.status.success() {
            return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        let mut metadata: Self =
            serde_json::from_slice(&output.stdout).map_err(|error| format!("cargo metadata: {error}"))?;
        let lock = std::fs::read_to_string(root.join("Cargo.lock")).map_err(|error| format!("Cargo.lock: {error}"))?;
        let lock: Lock = toml::from_str(&lock).map_err(|error| format!("Cargo.lock: {error}"))?;
        metadata.checksums = lock
            .package
            .into_iter()
            .filter_map(|package| Some(((package.name, package.version, package.source?), package.checksum?)))
            .collect();
        Ok(metadata)
    }

    pub fn selected(&self, root: &Path, crates: &[String], include_engine: bool) -> Result<Selected, String> {
        let packages: HashMap<_, _> = self.packages.iter().map(|package| (package.id.as_str(), package)).collect();
        let nodes: HashMap<_, _> = self.resolve.nodes.iter().map(|node| (node.id.as_str(), node)).collect();
        let mut pending = Vec::new();
        for name in crates {
            let package = self
                .packages
                .iter()
                .find(|package| &package.name == name && package.source.is_none())
                .ok_or_else(|| format!("the workspace has no crate `{name}`"))?;
            pending.push(package.id.as_str());
        }
        let (mut seen, mut dirs, mut identities) = (BTreeSet::new(), BTreeSet::new(), BTreeMap::new());
        while let Some(id) = pending.pop() {
            let package = packages.get(id).ok_or_else(|| format!("cargo has no resolved package `{id}`"))?;
            // Engine plumbing is not producer code. Selected content settings have their own projection.
            if (!include_engine && package.name == env!("CARGO_PKG_NAME")) || !seen.insert(id) {
                continue;
            }
            let node = nodes.get(id).ok_or_else(|| format!("cargo has no resolved node `{id}`"))?;
            let mut features = node.features.clone();
            features.sort();
            features.dedup();
            let checksum = match &package.source {
                Some(source) if source.starts_with("registry+") => Some(
                    self.checksums
                        .get(&(package.name.clone(), package.version.clone(), source.clone()))
                        .ok_or_else(|| format!("Cargo.lock has no checksum for `{id}`"))?
                        .as_str(),
                ),
                Some(_) => None,
                None => {
                    let dir = package
                        .manifest_path
                        .parent()
                        .unwrap()
                        .canonicalize()
                        .map_err(|error| format!("{}: {error}", package.manifest_path.display()))?;
                    if !dir.starts_with(root) {
                        return Err(format!("{} is outside {}", dir.display(), root.display()));
                    }
                    dirs.insert(dir);
                    None
                }
            };
            // Cargo resolves workspace-inherited package fields before exposing them here.
            let settings: BTreeMap<_, _> =
                ["authors", "description", "homepage", "repository", "license", "rust_version", "links"]
                    .into_iter()
                    .filter_map(|name| package.settings.get(name).map(|value| (name, value)))
                    .collect();
            let identity = serde_json::json!({ "name": package.name, "version": package.version, "edition": package.edition,
                "source": package.source, "checksum": checksum, "features": features, "settings": settings });
            let bytes = serde_json::to_vec(&super::super::sorted(identity)).map_err(|error| error.to_string())?;
            let key =
                format!("cargo/{}@{}#{}", package.name, package.version, package.source.as_deref().unwrap_or("path"));
            identities.insert(key, crate::store::sha256_hex(&bytes));
            pending.extend(
                node.deps
                    .iter()
                    .filter(|dep| dep.dep_kinds.iter().any(|kind| kind.kind.as_deref() != Some("dev")))
                    .map(|dep| dep.pkg.as_str()),
            );
        }
        Ok((dirs, identities))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    #[test]
    fn resolved_normal_and_build_packages_bind_identity_without_dev_or_unrelated_nodes() {
        let scratch = Scratch::new("resolved-code");
        let root = scratch.0.canonicalize().unwrap();
        let dir = root.join("producer");
        std::fs::create_dir_all(&dir).unwrap();
        let package = |id: &str, source: Option<&str>| {
            serde_json::json!({
                "id": id, "name": id, "version": "1.0.0", "edition": "2021", "source": source,
                "manifest_path": dir.join("Cargo.toml")
            })
        };
        let dep = |name: &str, kind: Option<&str>| serde_json::json!({"pkg": name, "dep_kinds": [{"kind": kind}]});
        let node = |name: &str, deps: Vec<serde_json::Value>| {
            serde_json::json!({
                "id": name, "deps": deps, "features": ["default"]
            })
        };
        let graph = serde_json::json!({
            "packages": [package("producer", None), package("normal", Some("registry+test")),
                package("build", Some("registry+test")), package("dev", Some("registry+test")),
                package("unrelated", Some("registry+test"))],
            "resolve": {"nodes": [node("producer", vec![dep("normal", None), dep("build", Some("build")), dep("dev", Some("dev"))]),
                node("normal", Vec::new()), node("build", Vec::new()), node("dev", Vec::new()), node("unrelated", Vec::new())]}
        });
        let mut metadata: Metadata = serde_json::from_value(graph).unwrap();
        for name in ["normal", "build", "dev", "unrelated"] {
            metadata.checksums.insert((name.into(), "1.0.0".into(), "registry+test".into()), name.into());
        }
        let names = vec!["producer".into()];
        let before = metadata.selected(&root, &names, false).unwrap();
        assert_eq!(before.0, BTreeSet::from([dir]));
        assert_eq!(before.1.len(), 3);
        for name in ["dev", "unrelated"] {
            metadata.checksums.insert((name.into(), "1.0.0".into(), "registry+test".into()), "changed".into());
        }
        assert_eq!(metadata.selected(&root, &names, false).unwrap(), before);
        for name in ["normal", "build"] {
            let key = (name.into(), "1.0.0".into(), "registry+test".into());
            let old = metadata.checksums.insert(key.clone(), "changed".into()).unwrap();
            assert_ne!(metadata.selected(&root, &names, false).unwrap().1, before.1, "{name}");
            metadata.checksums.insert(key, old);
        }
        metadata.packages[0].settings.insert("license".into(), serde_json::json!("MIT"));
        assert_ne!(
            metadata.selected(&root, &names, false).unwrap().1,
            before.1,
            "resolved inherited package settings bind code"
        );
        metadata.packages[0].settings.clear();
        metadata.packages[1].source = Some("git+test#first".into());
        let git = metadata.selected(&root, &names, false).unwrap().1;
        metadata.packages[1].source = Some("git+test#second".into());
        assert_ne!(metadata.selected(&root, &names, false).unwrap().1, git, "resolved git revisions bind bytes");
        metadata.packages[1].source = Some("registry+test".into());
        metadata.resolve.nodes[1].features.push("format-affecting".into());
        assert_ne!(
            metadata.selected(&root, &names, false).unwrap().1,
            before.1,
            "resolved build features are real inputs"
        );
    }
}
