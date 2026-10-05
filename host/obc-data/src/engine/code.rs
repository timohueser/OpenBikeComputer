//! The code hash of a step: the digest of the files it declares, and of the crates it declares
//! with their path dependencies.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::{digest, Code};
use crate::store::hash_file;

pub fn hash(root: &Path, code: &Code) -> Result<String, String> {
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    let mut files = BTreeSet::new();
    for path in &code.paths {
        let path = root.join(path);
        if !path.exists() {
            return Err(format!("the code path {} does not exist", path.display()));
        }
        add(&path, &mut files)?;
    }
    for dir in crate_dirs(&root, &code.crates)? {
        files.insert(dir.join("Cargo.toml"));
        for path in [dir.join("build.rs"), dir.join("src")] {
            if path.exists() {
                add(&path, &mut files)?;
            }
        }
    }
    let mut hashes = BTreeMap::new();
    for file in files {
        let relative =
            file.strip_prefix(&root).map_err(|_| format!("{} is outside {}", file.display(), root.display()))?;
        let relative = relative.to_str().ok_or_else(|| format!("{} is not UTF-8", relative.display()))?;
        hashes.insert(relative.replace('\\', "/"), hash_file(&file)?.0);
    }
    Ok(digest(hashes.iter().map(|(path, sha256)| (path.as_str(), sha256.as_str()))))
}

/// `path`, or every file below it but entries whose names start with `.` and `__pycache__`.
fn add(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    if !path.is_dir() {
        files.insert(path.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", path.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with('.') && name != "__pycache__" {
            add(&entry.path(), files)?;
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    manifest_path: PathBuf,
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    kind: Option<String>,
    path: Option<PathBuf>,
}

/// The directories of `crates` and of their normal and build path dependencies, from
/// `cargo metadata` of the workspace at `root`.
fn crate_dirs(root: &Path, crates: &[String]) -> Result<BTreeSet<PathBuf>, String> {
    if crates.is_empty() {
        return Ok(BTreeSet::new());
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--format-version", "1", "--no-deps", "--offline"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo metadata: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let metadata: Metadata = serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let dir = |manifest: &Path| -> Result<PathBuf, String> {
        let dir = manifest.parent().unwrap_or(manifest);
        dir.canonicalize().map_err(|e| format!("{}: {e}", dir.display()))
    };
    let mut packages = HashMap::new();
    for package in &metadata.packages {
        packages.insert(dir(&package.manifest_path)?, package);
    }
    let mut pending = Vec::new();
    for name in crates {
        let package = metadata.packages.iter().find(|package| &package.name == name);
        pending.push(dir(&package.ok_or_else(|| format!("the workspace has no crate `{name}`"))?.manifest_path)?);
    }
    let mut dirs = BTreeSet::new();
    while let Some(next) = pending.pop() {
        if !dirs.insert(next.clone()) {
            continue;
        }
        let package = packages.get(&next).ok_or_else(|| {
            format!("the path dependency {} is not in the workspace; declare its files instead", next.display())
        })?;
        for dependency in &package.dependencies {
            if let Some(path) = dependency.path.as_deref().filter(|_| dependency.kind.as_deref() != Some("dev")) {
                pending.push(path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?);
            }
        }
    }
    Ok(dirs)
}
