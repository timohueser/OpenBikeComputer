//! Declared files and the selected Rust, Python and source content that produce a layer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

mod python;
mod rust;

use super::Code;
use crate::store::hash_file;

/// File paths and named dependency fingerprints, in byte order, to SHA-256.
pub fn files(root: &Path, code: &Code) -> Result<BTreeMap<String, String>, String> {
    Context::default().files(root, code)
}

#[derive(Default)]
pub(super) struct Context {
    rust: HashMap<Option<String>, Arc<rust::Metadata>>,
    python: HashMap<Option<String>, python::Identity>,
    packages: HashMap<String, BTreeMap<String, String>>,
    include_engine: bool,
}

impl Context {
    pub fn refresh_python(&mut self) {
        self.python.clear();
        self.packages.clear();
    }

    pub fn files(&mut self, root: &Path, code: &Code) -> Result<BTreeMap<String, String>, String> {
        let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
        let mut pathspecs = Vec::new();
        for path in &code.paths {
            if !root.join(path).exists() {
                return Err(format!("the code path {path} does not exist in {}", root.display()));
            }
            pathspecs.push(PathBuf::from(path));
        }
        let (crates, dependencies) = if code.crates.is_empty() {
            (BTreeSet::new(), BTreeMap::new())
        } else {
            if self.rust.get(&code.target).map(|metadata| metadata.unchanged(&root)).transpose()? != Some(true) {
                self.rust.insert(code.target.clone(), Arc::new(rust::Metadata::load(&root, code.target.as_deref())?));
            }
            self.rust[&code.target].selected(&root, &code.crates, self.include_engine)?
        };
        for dir in &crates {
            let dir =
                dir.strip_prefix(&root).map_err(|_| format!("{} is outside {}", dir.display(), root.display()))?;
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
            let explicit = code.paths.iter().any(|path| file.starts_with(root.join(path)));
            let hash = if !explicit && crates.iter().any(|dir| file == dir.join("Cargo.toml")) {
                manifest_hash(&file)?
            } else {
                hash_file(&file)?.0
            };
            hashes.insert(relative.replace('\\', "/"), hash);
        }
        hashes.extend(dependencies);
        if let Some(target) = &code.target {
            hashes.insert("rust/target".into(), crate::store::sha256_hex(target.as_bytes()));
        }
        if let Some(runtime) = &code.python {
            if !self.python.contains_key(&runtime.group) {
                self.python.insert(runtime.group.clone(), python::identity(&root, runtime)?);
            }
            hashes.extend(self.python[&runtime.group].hashes.clone());
        }
        if let Some(group) = &code.python_packages {
            if !self.packages.contains_key(group) {
                self.packages.insert(group.clone(), python::packages(&root, Some(group))?);
            }
            hashes.extend(self.packages[group].clone());
        }
        if !code.sources.is_empty() {
            let registry = crate::sources::Registry::load(&root)?;
            for id in &code.sources {
                let source = registry
                    .sources
                    .iter()
                    .find(|source| &source.id == id)
                    .ok_or_else(|| format!("code names no source `{id}`"))?;
                hashes.insert(
                    format!("data/sources.toml#{id}"),
                    source_hash(source, &["refresh", "credential", "r2_copy", "redistribute", "hosts"])?,
                );
            }
        }
        Ok(hashes)
    }
}

/// Bind a declared Python command to the interpreter in its checked code identity.
pub(super) fn python_command(root: &Path, code: &Code, expected: &str, command: &mut Command) -> Result<(), String> {
    let mut context = Context::default();
    if hash(&context.files(root, code)?) != expected {
        return Err("Python step code or interpreter changed; plan again".into());
    }
    let runtime = code.python.as_ref().ok_or("the command declares no Python runtime")?;
    command.env("UV_PYTHON", &context.python[&runtime.group].executable);
    // A no-sync override can make uv retain an incompatible project interpreter.
    command.env("UV_NO_SYNC", "0");
    Ok(())
}

pub fn hash(files: &BTreeMap<String, String>) -> String {
    super::digest(files.iter().map(|(path, sha256)| (path.as_str(), sha256.as_str())))
}

/// The compiled producer closure, including the engine that derives and applies its plans.
pub fn compiled(root: &Path, crates: &[String]) -> Result<String, String> {
    let registry = crate::sources::Registry::load(root)?;
    let code = Code { crates: crates.to_vec(), ..Code::default() };
    let mut context = Context { include_engine: true, ..Context::default() };
    let mut files = context.files(root, &code)?;
    // Embedded credits use the content projection; controls are read from the checkout.
    files.remove("data/sources.toml");
    for source in &registry.sources {
        // Product preflight reads embedded credential descriptors as well as content settings.
        files.insert(
            format!("data/sources.toml#{}", source.id),
            source_hash(source, &["refresh", "r2_copy", "redistribute", "hosts"])?,
        );
    }
    for path in ["Cargo.toml", "rust-toolchain.toml", ".cargo/config.toml"] {
        if root.join(path).is_file() {
            files.insert(path.into(), hash_file(&root.join(path))?.0);
        }
    }
    Ok(hash(&files))
}

fn source_hash(source: &crate::sources::Source, excluded: &[&str]) -> Result<String, String> {
    let mut content = serde_json::to_value(source).map_err(|error| error.to_string())?;
    for field in excluded {
        content.as_object_mut().unwrap().remove(*field);
    }
    let bytes = serde_json::to_vec(&super::sorted(content)).map_err(|error| error.to_string())?;
    Ok(crate::store::sha256_hex(&bytes))
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

/// Only automatically selected producer manifests omit dev-only dependency declarations.
fn manifest_hash(path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut manifest: toml::Value = toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let table = manifest.as_table_mut().ok_or("Cargo.toml is not a table")?;
    table.remove("dev-dependencies");
    if let Some(targets) = table.get_mut("target").and_then(toml::Value::as_table_mut) {
        for target in targets.iter_mut().filter_map(|(_, target)| target.as_table_mut()) {
            target.remove("dev-dependencies");
        }
        targets.retain(|_, target| target.as_table().is_none_or(|table| !table.is_empty()));
        if targets.is_empty() {
            table.remove("target");
        }
    }
    let value = serde_json::to_value(manifest).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(&super::sorted(value)).map_err(|error| error.to_string())?;
    Ok(crate::store::sha256_hex(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, write};

    #[test]
    fn rust_resolution_is_reused_until_a_workspace_manifest_or_lock_changes() {
        let fixture = fixture("code-resolution-cache");
        let root = fixture.root();
        crate::engine::tests::repository(&root, &[("steps", ""), ("other", "")]);
        let code = Code { crates: vec!["steps".into()], ..Code::default() };
        let mut context = Context::default();
        let before = context.files(&root, &code).unwrap();
        let loaded = context.rust[&None].clone();
        assert_eq!(context.files(&root, &code).unwrap(), before);
        assert!(Arc::ptr_eq(&loaded, &context.rust[&None]));

        let other = root.join("other/Cargo.toml");
        write(&other, &fs::read_to_string(&other).unwrap().replace("2021", "2024"));
        assert_eq!(context.files(&root, &code).unwrap(), before);
        assert!(
            !Arc::ptr_eq(&loaded, &context.rust[&None]),
            "an unrelated local manifest can affect Cargo feature unification"
        );
        let loaded = context.rust[&None].clone();
        let lock = root.join("Cargo.lock");
        write(&lock, &(fs::read_to_string(&lock).unwrap() + "\n# lock comment\n"));
        assert_eq!(context.files(&root, &code).unwrap(), before);
        assert!(!Arc::ptr_eq(&loaded, &context.rust[&None]));
        let loaded = context.rust[&None].clone();
        let workspace = root.join("Cargo.toml");
        write(&workspace, &(fs::read_to_string(&workspace).unwrap() + "\n# workspace comment\n"));
        assert_eq!(context.files(&root, &code).unwrap(), before);
        assert!(!Arc::ptr_eq(&loaded, &context.rust[&None]));
        let source = root.join("steps/src/lib.rs");
        write(&source, "pub fn updated() {}\n");
        let loaded = context.rust[&None].clone();
        assert_ne!(context.files(&root, &code).unwrap(), before);
        assert!(Arc::ptr_eq(&loaded, &context.rust[&None]), "source edits rehash files without running Cargo");
    }

    #[test]
    fn source_content_identity_is_scoped_and_freshness_controls_do_not_change_it() {
        let fixture = fixture("code-content-source");
        let root = fixture.root();
        let sources = root.join("data/sources.toml");
        let source = "[[source]]\nid = \"land\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Land\"\nfetch = { kind = \"http\", url = \"https://example.org/land\" }\nversion = \"date\"\nrefresh = 7\nredistribute = true\n";
        let text = format!("{source}{}", source.replace("land", "other").replace("Land", "Other"));
        write(&sources, &text);
        let code = Code { sources: vec!["land".into()], ..Default::default() };
        let before = files(&root, &code).unwrap();
        let mut steps = crate::engine::tests::pipeline();
        steps[0].code.sources = code.sources.clone();
        fixture.build(&steps).unwrap();
        write(&sources, &text.replace("refresh = 7", "refresh = 30").replace("Other", "Changed"));
        assert_eq!(files(&root, &code).unwrap(), before, "unrelated sources and age policy do not change content");
        assert!(fixture.plan(&steps).unwrap().groups.is_empty(), "policy-only edits reuse the built layers");
        write(&sources, &text.replace("attribution = \"Land\"", "attribution = \"New credit\""));
        assert_ne!(files(&root, &code).unwrap(), before);
        assert!(
            !fixture.plan(&steps).unwrap().groups.is_empty(),
            "plan and code identity use the same content projection"
        );
        write(&sources, &text.replace("example.org/land", "example.org/new-land"));
        assert_ne!(files(&root, &code).unwrap(), before, "acquisition configuration is content identity");
        let missing = Code { sources: vec!["missing".into()], ..Default::default() };
        assert!(files(&root, &missing).unwrap_err().contains("no source"));
    }

    #[test]
    fn automatic_manifest_projection_excludes_dev_tables_and_preserves_normal_build_configuration() {
        let fixture = fixture("code-manifest");
        let path = fixture.root().join("steps/Cargo.toml");
        let original = std::fs::read_to_string(&path).unwrap();
        let before = manifest_hash(&path).unwrap();
        let explicit = Code { paths: vec!["steps/Cargo.toml".into()], ..Default::default() };
        let full_before = files(&fixture.root(), &explicit).unwrap();
        let dev = "\n[dev-dependencies]\ncheck = \"2\"\n[target.'cfg(unix)'.dev-dependencies]\nprobe = \"3\"\n";
        write(&path, &(original.clone() + dev));
        assert_eq!(manifest_hash(&path).unwrap(), before);
        assert_ne!(files(&fixture.root(), &explicit).unwrap(), full_before, "explicit paths keep full file identity");
        write(&path, &(original.clone() + "\n[target.'cfg(unix)'.build-dependencies]\nnormal = \"3\"\n"));
        assert_ne!(manifest_hash(&path).unwrap(), before);
        write(&path, &original.replace("2021", "2024"));
        assert_ne!(manifest_hash(&path).unwrap(), before);
    }
}
