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
    let mut pathspecs = Vec::new();
    for path in &code.paths {
        if !root.join(path).exists() {
            return Err(format!("the code path {path} does not exist in {}", root.display()));
        }
        pathspecs.push(PathBuf::from(path));
    }
    let crates = crate_dirs(&root, &code.crates)?;
    for dir in &crates {
        let dir = dir.strip_prefix(&root).map_err(|_| format!("{} is outside {}", dir.display(), root.display()))?;
        pathspecs.extend(["Cargo.toml", "build.rs", "src"].map(|name| dir.join(name)));
    }
    let mut files = listed(&root, &pathspecs)?;
    for dir in &crates {
        let sources: Vec<_> = files
            .iter()
            .filter(|file| file.starts_with(dir) && file.extension() == Some("rs".as_ref()))
            .cloned()
            .collect();
        for source in sources {
            files.extend(included(&source, dir)?);
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

/// The files at or below `pathspecs` that git tracks or does not ignore.
fn listed(root: &Path, pathspecs: &[PathBuf]) -> Result<BTreeSet<PathBuf>, String> {
    if pathspecs.is_empty() {
        return Ok(BTreeSet::new());
    }
    let output = Command::new("git")
        .args(["--literal-pathspecs", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--"])
        .args(pathspecs)
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !output.status.success() {
        return Err(format!("git ls-files: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let paths = String::from_utf8(output.stdout).map_err(|_| "git ls-files: a path is not UTF-8")?;
    // A tracked file that the worktree deleted is listed too.
    Ok(paths
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| root.join(path))
        .filter(|path| path.is_file())
        .collect())
}

/// The files that `source` names with a string literal in `include_str!`, `include_bytes!` or
/// `include!`: relative to `source`, or after `concat!(env!("CARGO_MANIFEST_DIR"),`.
fn included(source: &Path, manifest_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let text = fs::read_to_string(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let mut files = Vec::new();
    for call in ["include_str!(", "include_bytes!(", "include!("] {
        for (start, _) in text.match_indices(call) {
            let rest = text[start + call.len()..].trim_start();
            let (base, rest) = match rest.strip_prefix("concat!(") {
                Some(rest) => {
                    let rest = rest.trim_start().strip_prefix("env!(\"CARGO_MANIFEST_DIR\")");
                    let Some(rest) = rest.and_then(|rest| rest.trim_start().strip_prefix(',')) else { continue };
                    (manifest_dir, rest.trim_start())
                }
                None => (source.parent().unwrap_or(source), rest),
            };
            let literal = rest.strip_prefix('"').and_then(|rest| rest.split_once('"')).map(|(literal, _)| literal);
            // A path that does not exist is not compiled in, for example one in a comment.
            if let Some(path) =
                literal.and_then(|literal| base.join(literal.trim_start_matches('/')).canonicalize().ok())
            {
                if path.is_file() {
                    files.push(path);
                }
            }
        }
    }
    Ok(files)
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
