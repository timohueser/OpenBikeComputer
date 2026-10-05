//! The code of a step: the files it declares, and the files of the crates it declares with their
//! path dependencies. The code hash is their digest.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::Code;
use crate::store::hash_file;

/// The code files: path relative to `root`, with `/`, to SHA-256.
pub fn files(root: &Path, code: &Code) -> Result<BTreeMap<String, String>, String> {
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
    if let Some(path) = code.paths.iter().find(|path| !files.iter().any(|file| file.starts_with(root.join(path)))) {
        return Err(format!("{path} is ignored by git"));
    }
    let mut pending: Vec<(PathBuf, &Path)> = Vec::new();
    for dir in &crates {
        let sources = files.iter().filter(|file| file.starts_with(dir) && is_rust(file));
        pending.extend(sources.map(|source| (source.clone(), dir.as_path())));
    }
    while let Some((source, dir)) = pending.pop() {
        for file in included(&source, dir)? {
            if files.insert(file.clone()) && is_rust(&file) {
                pending.push((file, dir));
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
    Ok(hashes)
}

pub fn hash(files: &BTreeMap<String, String>) -> String {
    super::digest(files.iter().map(|(path, sha256)| (path.as_str(), sha256.as_str())))
}

fn is_rust(file: &Path) -> bool {
    file.extension() == Some("rs".as_ref())
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Err(format!("obc data runs in a git checkout, and {} is not one", root.display()));
        }
        return Err(format!("git ls-files: {}", stderr.trim()));
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

/// The files that the Rust file `source` names: the argument of `include_str!`, `include_bytes!`
/// or `include!`, and the module file of `#[path = "…"]`.
fn included(source: &Path, manifest_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let text = fs::read_to_string(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let source_dir = source.parent().unwrap_or(source);
    let mut paths = Vec::new();
    for call in ["include_str!(", "include_bytes!(", "include!("] {
        for (start, _) in text.match_indices(call) {
            paths.extend(include_path(&text[start + call.len()..], source_dir, manifest_dir));
        }
    }
    for (start, _) in text.match_indices("#[path") {
        let rest = text[start + "#[path".len()..].trim_start().strip_prefix('=');
        paths.extend(rest.and_then(|rest| literal(rest.trim_start())).map(|(path, _)| source_dir.join(path)));
    }
    // A path that does not exist is not compiled in, for example one in a comment.
    Ok(paths.into_iter().filter_map(|path| path.canonicalize().ok()).filter(|path| path.is_file()).collect())
}

/// The path of an include argument: a string literal, or a `concat!` of string literals that
/// may start with `env!("CARGO_MANIFEST_DIR")`.
fn include_path(argument: &str, source_dir: &Path, manifest_dir: &Path) -> Option<PathBuf> {
    let argument = argument.trim_start();
    let Some(mut rest) = argument.strip_prefix("concat!(") else {
        return literal(argument).map(|(path, _)| source_dir.join(path));
    };
    let mut base = source_dir;
    if let Some(after) = rest.trim_start().strip_prefix("env!(\"CARGO_MANIFEST_DIR\")") {
        base = manifest_dir;
        rest = after.trim_start().strip_prefix(',')?;
    }
    let mut path = String::new();
    while let Some((part, after)) = literal(rest.trim_start()) {
        path.push_str(part);
        rest = after.trim_start().strip_prefix(',').unwrap_or(after);
    }
    rest.trim_start().starts_with(')').then(|| base.join(path.trim_start_matches('/')))
}

/// The text of the string literal at the start of `text`, normal or raw, and the rest.
fn literal(text: &str) -> Option<(&str, &str)> {
    if let Some(rest) = text.strip_prefix('"') {
        return rest.split_once('"');
    }
    let rest = text.strip_prefix('r')?;
    let hashes = &rest[..rest.len() - rest.trim_start_matches('#').len()];
    rest[hashes.len()..].strip_prefix('"')?.split_once(&format!("\"{hashes}"))
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

/// The directories of `crates` and of their normal and build path dependencies, but `obc-data`,
/// from `cargo metadata` of the workspace at `root`.
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
        let package = packages.get(&next).ok_or_else(|| {
            format!("the path dependency {} is not in the workspace; declare its files instead", next.display())
        })?;
        // The engine only selects and passes the inputs of a step: what it selects reaches the
        // key through the input digests, so its own code is no code of a step.
        if package.name == env!("CARGO_PKG_NAME") || !dirs.insert(next.clone()) {
            continue;
        }
        for dependency in &package.dependencies {
            if let Some(path) = dependency.path.as_deref().filter(|_| dependency.kind.as_deref() != Some("dev")) {
                pending.push(path.canonicalize().map_err(|e| format!("{}: {e}", path.display()))?);
            }
        }
    }
    Ok(dirs)
}
