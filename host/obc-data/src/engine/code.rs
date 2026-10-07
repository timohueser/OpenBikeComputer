//! Declared files and the selected Rust, Python and source content that produce a layer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

mod build;
mod python;
mod rust;

pub(crate) use python::executable as python_executable;

use super::{Code, CodeIdentity, OwnerCode, Profile, ResolvedRust, Rust, SourceIdentity};
use crate::store::hash_file;

/// File paths and named dependency fingerprints, in byte order, to SHA-256.
pub fn files(root: &Path, code: &Code) -> Result<BTreeMap<String, String>, String> {
    identity(root, code).map(|identity| identity.files)
}

pub fn identity(root: &Path, code: &Code) -> Result<CodeIdentity, String> {
    Context::default().identity(root, code)
}

pub fn source_config(root: &Path, code: &Code, rust: Option<&ResolvedRust>) -> Result<SourceIdentity, String> {
    Context::default().source_config(root, code, rust)
}

pub fn owner_identity(root: &Path, owner: &OwnerCode) -> Result<CodeIdentity, String> {
    Context::default().owner_identity(root, owner)
}

pub fn owner_source_config(root: &Path, owner: &OwnerCode, rust: &ResolvedRust) -> Result<SourceIdentity, String> {
    Context::default().owner_source_config(root, owner, rust)
}

#[derive(Clone, Copy)]
enum Mode<'a> {
    Execution,
    Source(Option<&'a ResolvedRust>),
}

#[derive(Default)]
pub(crate) struct Context {
    rust: HashMap<Option<String>, Arc<rust::Metadata>>,
    python: HashMap<Option<String>, python::Identity>,
    packages: HashMap<String, BTreeMap<String, String>>,
    build: build::Context,
    native: Option<(PathBuf, String)>,
    include_engine: bool,
}

impl Context {
    pub fn refresh_python(&mut self) {
        self.python.clear();
        self.packages.clear();
    }

    pub fn files(&mut self, root: &Path, code: &Code) -> Result<BTreeMap<String, String>, String> {
        self.identity(root, code).map(|identity| identity.files)
    }

    pub fn identity(&mut self, root: &Path, code: &Code) -> Result<CodeIdentity, String> {
        self.resolve(root, code, Mode::Execution, None)
    }

    pub fn source_config(
        &mut self,
        root: &Path,
        code: &Code,
        rust: Option<&ResolvedRust>,
    ) -> Result<SourceIdentity, String> {
        let identity = self.resolve(root, code, Mode::Source(rust), None)?;
        Ok(SourceIdentity { files: identity.source_config, rust: identity.rust, git_inputs: identity.git_inputs })
    }

    pub fn owner_source_config(
        &mut self,
        root: &Path,
        owner: &OwnerCode,
        rust: &ResolvedRust,
    ) -> Result<SourceIdentity, String> {
        let mut code = owner.code.clone();
        code.crates.push(owner.crate_name.clone());
        let identity = self.resolve(root, &code, Mode::Source(Some(rust)), Some(&owner.crate_name))?;
        Ok(SourceIdentity { files: identity.source_config, rust: identity.rust, git_inputs: identity.git_inputs })
    }

    pub fn owner_identity(&mut self, root: &Path, owner: &OwnerCode) -> Result<CodeIdentity, String> {
        if owner.code.paths.is_empty() {
            return Err("a native owner must declare its source paths".into());
        }
        if !matches!(owner.code.rust, None | Some(Rust::Native { profile: Profile::Dev })) {
            return Err("native owner callbacks require the retained worker's native dev build".into());
        }
        let mut code = owner.code.clone();
        code.crates.push(owner.crate_name.clone());
        self.resolve(root, &code, Mode::Execution, Some(&owner.crate_name))
    }

    fn resolve(
        &mut self,
        root: &Path,
        code: &Code,
        mode: Mode<'_>,
        owner: Option<&str>,
    ) -> Result<CodeIdentity, String> {
        let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
        let native = match mode {
            Mode::Execution => self.build.preflight(&root, code)?,
            Mode::Source(_) => None,
        };
        let target = match mode {
            Mode::Execution => code.target.clone(),
            Mode::Source(rust) if !code.crates.is_empty() => {
                let rust = rust.ok_or("source/config resolution requires the recorded Rust target and profile")?;
                let declared = code.rust.clone().unwrap_or(Rust::Native { profile: Profile::Dev });
                if declared != rust.build || code.target.as_ref().is_some_and(|target| target != &rust.target) {
                    return Err("recorded Rust target/profile does not match the producer declaration".into());
                }
                Some(rust.target.clone())
            }
            Mode::Source(_) => None,
        };
        if let Some((_, identity)) = &native {
            let current = (root.clone(), identity.clone());
            if self.native.as_ref() != Some(&current) {
                self.rust.clear();
                self.native = Some(current);
            }
        }
        let mut pathspecs = Vec::new();
        for path in &code.paths {
            if !root.join(path).exists() {
                return Err(format!("the code path {path} does not exist in {}", root.display()));
            }
            pathspecs.push(PathBuf::from(path));
        }
        let (crates, dependencies, packages) = if code.crates.is_empty() {
            (BTreeSet::new(), BTreeMap::new(), rust::Packages::default())
        } else {
            if self.rust.get(&target).map(|metadata| metadata.unchanged(&root)).transpose()? != Some(true) {
                let selected_target = target.as_deref().or_else(|| native.as_ref().map(|(target, _)| target.as_str()));
                self.rust.insert(target.clone(), Arc::new(rust::Metadata::load(&root, selected_target)?));
            }
            self.rust[&target].selected(&root, &code.crates, self.include_engine || owner.is_some())?
        };
        for dir in &crates {
            let dir =
                dir.strip_prefix(&root).map_err(|_| format!("{} is outside {}", dir.display(), root.display()))?;
            pathspecs.extend(["Cargo.toml", "build.rs"].map(|name| dir.join(name)));
            let scoped = owner.is_some_and(|owner| {
                self.rust[&target].directory(owner).as_deref() == Some(&root.join(dir))
                    || self.rust[&target].directory(env!("CARGO_PKG_NAME")).as_deref() == Some(&root.join(dir))
            });
            if !scoped {
                pathspecs.push(dir.join("src"));
            }
        }
        let mut files = listed(&root, &pathspecs)?;
        let mut backend = BTreeSet::new();
        if owner.is_some() {
            if let Some(dir) = self.rust[&target].directory(env!("CARGO_PKG_NAME")) {
                backend = listed_paths(&root, &[dir.join("src")])?;
                backend
                    .retain(|path| path != &dir.join("src/cli/tui.rs") && !path.starts_with(dir.join("src/cli/tui")));
                files.extend(backend.iter().filter(|path| path.is_file()).cloned());
            }
        }
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
        let mut git_inputs: BTreeSet<_> = files.iter().map(|file| relative(&root, file)).collect::<Result<_, _>>()?;
        git_inputs.extend(backend.iter().map(|path| relative(&root, path)).collect::<Result<BTreeSet<_>, _>>()?);
        git_inputs.extend(pathspecs.iter().map(|path| path.to_string_lossy().replace('\\', "/")));
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
        if owner.is_some() && !code.paths.iter().any(|path| root.join("data/sources.toml").starts_with(root.join(path)))
        {
            // Embedded registry bytes are replaced by the owner's declared source projections.
            hashes.remove("data/sources.toml");
        }
        let mut source_config = hashes.clone();
        let rust = if !packages.names.is_empty() {
            let rust = match mode {
                Mode::Execution => {
                    let build = self.build.identity(&root, code, &packages)?;
                    source_config.extend(build.source_config);
                    hashes.extend(build.files);
                    build.rust
                }
                Mode::Source(Some(rust)) => {
                    let profile = match rust.build {
                        Rust::Native { profile } | Rust::Prepared { profile } => profile,
                    };
                    source_config.extend(build::source_config(&root, profile, &packages)?);
                    rust.clone()
                }
                Mode::Source(None) => unreachable!("a Rust closure requires a recorded target/profile"),
            };
            for path in ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config", ".cargo/config.toml"] {
                if root.join(path).is_file() {
                    git_inputs.insert(path.into());
                }
            }
            Some(rust)
        } else {
            None
        };
        for library in code.libraries.iter().filter(|_| matches!(mode, Mode::Execution)) {
            let name = format!("native/library/{}", library.name);
            if !crate::is_kebab(&library.name) || hashes.contains_key(&name) || !library.path.is_absolute() {
                return Err("native libraries need unique kebab-case names and absolute paths".into());
            }
            let hash = self
                .build
                .tool_hash(&library.path)
                .map_err(|error| format!("native library {}: {error}; start a fresh worker", library.name))?;
            if hash != library.sha256 {
                return Err(format!("native library {} changed; start a fresh worker", library.name));
            }
            hashes.insert(name, hash);
        }

        if let Some(runtime) = &code.python {
            match mode {
                Mode::Execution => {
                    if !self.python.contains_key(&runtime.group) {
                        self.python.insert(runtime.group.clone(), python::identity(&root, runtime)?);
                    }
                    source_config.extend(self.python[&runtime.group].packages.clone());
                    hashes.extend(self.python[&runtime.group].hashes.clone());
                }
                Mode::Source(_) => source_config.extend(python::packages(&root, runtime.group.as_deref())?),
            }
        }
        if let Some(group) = &code.python_packages {
            if !self.packages.contains_key(group) {
                self.packages.insert(group.clone(), python::packages(&root, Some(group))?);
            }
            source_config.extend(self.packages[group].clone());
            hashes.extend(self.packages[group].clone());
        }
        if code.python.is_some() || code.python_packages.is_some() {
            for path in ["pyproject.toml", "uv.lock", ".python-version"] {
                if root.join(path).is_file() {
                    git_inputs.insert(path.into());
                }
            }
        }
        if !code.sources.is_empty() {
            git_inputs.insert("data/sources.toml".into());
            let registry = crate::sources::Registry::load(&root)?;
            for id in &code.sources {
                let source = registry
                    .sources
                    .iter()
                    .find(|source| &source.id == id)
                    .ok_or_else(|| format!("code names no source `{id}`"))?;
                let key = format!("data/sources.toml#{id}");
                let hash = source_hash(source, &["refresh", "credential", "r2_copy", "redistribute", "hosts"])?;
                source_config.insert(key.clone(), hash.clone());
                hashes.insert(key, hash);
            }
        }
        Ok(CodeIdentity { files: hashes, source_config, rust, git_inputs })
    }
}

fn relative(root: &Path, file: &Path) -> Result<String, String> {
    file.strip_prefix(root)
        .map_err(|_| format!("{} is outside {}", file.display(), root.display()))?
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or_else(|| format!("{} is not UTF-8", file.display()))
}

pub(super) fn committed(root: &Path, inputs: &BTreeSet<String>) -> Result<(), String> {
    if inputs.is_empty() {
        return Ok(());
    }
    let output = Command::new("git")
        .args(["--literal-pathspecs", "status", "--porcelain=v1", "-z", "--untracked-files=all", "--"])
        .args(inputs)
        .current_dir(root)
        .output()
        .map_err(|error| format!("git status: {error}"))?;
    if !output.status.success() {
        return Err(format!("git status: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    if !output.stdout.is_empty() {
        return Err(
            "used producer code or build metadata is not committed; commit these inputs before applying Live".into()
        );
    }
    Ok(())
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
    for path in ["Cargo.toml", "rust-toolchain.toml"] {
        if root.join(path).is_file() {
            files.insert(path.into(), hash_file(&root.join(path))?.0);
        }
    }
    Ok(hash(&files))
}

pub(crate) fn source_hash(source: &crate::sources::Source, excluded: &[&str]) -> Result<String, String> {
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
    Ok(listed_paths(root, pathspecs)?.into_iter().filter(|path| path.is_file()).collect())
}

fn listed_paths(root: &Path, pathspecs: &[PathBuf]) -> Result<BTreeSet<PathBuf>, String> {
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
    Ok(paths.split('\0').filter(|path| !path.is_empty()).map(|path| root.join(path)).collect())
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
    fn owner_projection_excludes_ui_and_resolves_recorded_targets_without_execution_tools() {
        let fixture = fixture("owner-code-inputs");
        let root = fixture.root();
        crate::engine::tests::repository(&root, &[
            ("steps", "[dependencies]\nobc-data = { path = \"../obc-data\" }\n[target.'cfg(target_os = \"linux\")'.dependencies]\nlinux = { path = \"../linux\" }\n"),
            ("obc-data", ""),
            ("linux", ""),
        ]);
        write(&root.join("steps/src/lib.rs"), "mod plan; mod ui;\n");
        write(&root.join("steps/src/plan.rs"), "pub fn requests() {}\n");
        write(&root.join("steps/src/ui.rs"), "pub fn draw() {}\n");
        write(&root.join("obc-data/src/cli/tui.rs"), "pub fn screen() {}\n");
        write(&root.join("obc-data/src/regions/geofabrik.rs"), "pub fn parse() {}\n");
        write(&root.join("obc-data/src/store.rs"), "pub fn snapshot() {}\n");
        let owner = OwnerCode {
            crate_name: "steps".into(),
            code: Code { paths: vec!["steps/src/lib.rs".into(), "steps/src/plan.rs".into()], ..Default::default() },
        };
        let mut context = Context::default();
        let before = context.owner_identity(&root, &owner).unwrap();
        assert!(before.files.contains_key("rust/compiler"));
        assert!(!before.files.contains_key("steps/src/ui.rs"));
        assert!(!before.files.contains_key("obc-data/src/cli/tui.rs"));
        assert!(before.files.contains_key("obc-data/src/regions/geofabrik.rs"));
        assert!(before.files.contains_key("obc-data/src/store.rs"));
        assert!(before.git_inputs.contains("Cargo.lock"));
        assert!(before.git_inputs.contains("steps/Cargo.toml"));
        let git = |args: &[&str]| {
            assert!(Command::new("git").args(args).current_dir(&root).status().unwrap().success());
        };
        git(&["add", "."]);
        git(&["-c", "user.name=Fixture", "-c", "user.email=fixture@example.org", "commit", "-qm", "fixture"]);
        before.committed(&root).unwrap();
        write(&root.join("steps/src/ui.rs"), "pub fn new_screen() {}\n");
        write(&root.join("obc-data/src/cli/tui.rs"), "pub fn new_controls() {}\n");
        assert_eq!(context.owner_identity(&root, &owner).unwrap(), before);
        before.committed(&root).unwrap();
        for (path, original) in [
            ("obc-data/src/regions/geofabrik.rs", "pub fn parse() {}\n"),
            ("obc-data/src/store.rs", "pub fn snapshot() {}\n"),
        ] {
            write(&root.join(path), "pub fn different_selection() {}\n");
            let changed = context.owner_identity(&root, &owner).unwrap();
            assert_ne!(changed.source_config, before.source_config, "{path}");
            assert!(changed.committed(&root).unwrap_err().contains("not committed"));
            write(&root.join(path), original);
        }
        write(&root.join("steps/src/plan.rs"), "pub fn other_requests() {}\n");
        assert_ne!(context.owner_identity(&root, &owner).unwrap().files, before.files);
        assert!(before.committed(&root).unwrap_err().contains("not committed"));

        let recorded =
            ResolvedRust { target: "x86_64-unknown-linux-gnu".into(), build: Rust::Native { profile: Profile::Dev } };
        let witness = owner.source_config(&root, &recorded).unwrap();
        assert_eq!(witness.rust.as_ref(), Some(&recorded));
        assert!(witness.files.contains_key("linux/src/lib.rs"));
        assert!(!witness.files.contains_key("rust/compiler"));
        assert!(witness.files.contains_key("rust/profile"));
        let mut unavailable = owner.clone();
        unavailable.code.libraries.push(super::super::Library {
            name: "provider".into(),
            path: root.join("absent-provider"),
            sha256: "a".repeat(64),
        });
        assert_eq!(unavailable.source_config(&root, &recorded).unwrap(), witness);
        assert!(unavailable.identity(&root).unwrap_err().contains("native library provider"));
        let mut cross = owner.clone();
        cross.code.target = Some(recorded.target.clone());
        if before.rust.as_ref().unwrap().target != recorded.target {
            assert!(cross.identity(&root).unwrap_err().contains("native Rust code requires its compiler host"));
        }
        let layer = Code { crates: vec!["steps".into()], ..Default::default() };
        let full = context.identity(&root, &layer).unwrap();
        assert_eq!(
            full.files,
            context.files(&root, &layer).unwrap(),
            "per-layer keys retain the full execution identity"
        );
    }

    #[test]
    fn local_preparation_accepts_working_tree_code_and_live_apply_refuses_it() {
        let fixture = fixture("owner-commit-policy");
        let root = fixture.root();
        let owner = OwnerCode {
            crate_name: "steps".into(),
            code: Code { paths: vec!["steps/src/lib.rs".into()], ..Default::default() },
        };
        let mut run = crate::engine::runs::Run::create(&fixture.store, "prepare local").unwrap();
        run.check_owner(&root, &owner).unwrap();
        write(&root.join("steps/src/lib.rs"), "pub fn working_tree() {}\n");
        run.check_owner(&root, &owner).unwrap();
        run.require_committed_code();
        assert!(run.check_owner(&root, &owner).unwrap_err().contains("not committed"));
        run.finish(None).unwrap();
    }

    #[test]
    fn embedded_owner_registry_uses_selected_content_but_preserves_explicit_raw_inputs() {
        let fixture = fixture("owner-embedded-registry");
        let root = fixture.root();
        crate::engine::tests::repository(
            &root,
            &[("steps", "[dependencies]\nobc-data = { path = \"../obc-data\" }\n"), ("obc-data", "")],
        );
        write(
            &root.join("obc-data/src/sources.rs"),
            "pub const SOURCES: &str = include_str!(\"../../data/sources.toml\");\n",
        );
        let source = "[[source]]\nid = \"land\"\nkind = \"data\"\nlicence = \"CC0-1.0\"\nattribution = \"Land\"\nfetch = { kind = \"http\", url = \"https://example.org/land\" }\nversion = \"date\"\nrefresh = 7\nredistribute = true\n";
        let registry = format!("{source}{}", source.replace("land", "other").replace("Land", "Other"));
        let path = root.join("data/sources.toml");
        write(&path, &registry);
        let owner = OwnerCode {
            crate_name: "steps".into(),
            code: Code { paths: vec!["steps/src/lib.rs".into()], sources: vec!["land".into()], ..Default::default() },
        };
        let mut context = Context::default();
        let before = context.owner_identity(&root, &owner).unwrap();
        assert!(before.git_inputs.contains("data/sources.toml"));
        assert!(!before.files.contains_key("data/sources.toml"));
        assert!(before.files.contains_key("data/sources.toml#land"));
        let settings = |root: &Path| {
            let source = crate::sources::Registry::load(root)
                .unwrap()
                .sources
                .into_iter()
                .find(|source| source.id == "land")
                .unwrap();
            source_hash(&source, &["refresh"]).unwrap()
        };
        let access = settings(&root);
        write(&path, &registry.replace("refresh = 7", "refresh = 30").replace("Other", "New unrelated credit"));
        assert_eq!(context.owner_identity(&root, &owner).unwrap(), before);
        assert_eq!(settings(&root), access, "cadence and unrelated sources stay outside approval settings");
        write(&path, &registry.replace("redistribute = true", "redistribute = false"));
        assert_eq!(context.owner_identity(&root, &owner).unwrap(), before, "access controls do not change code bytes");
        assert_ne!(settings(&root), access, "effective publication access still requires a new approval");
        for change in [
            registry.replace("attribution = \"Land\"", "attribution = \"New credit\""),
            registry.replace("example.org/land", "example.org/new-land"),
        ] {
            write(&path, &change);
            let changed = context.owner_identity(&root, &owner).unwrap();
            assert_ne!(changed.files, before.files);
            assert_ne!(changed.source_config, before.source_config);
        }
        write(&path, &registry);
        let mut explicit = owner;
        explicit.code.paths.push("data/sources.toml".into());
        let raw = context.owner_identity(&root, &explicit).unwrap();
        assert!(raw.files.contains_key("data/sources.toml"));
        write(&path, &registry.replace("refresh = 7", "refresh = 30"));
        assert_ne!(context.owner_identity(&root, &explicit).unwrap().files, raw.files);
    }

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
