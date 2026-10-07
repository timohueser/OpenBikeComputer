//! The engine: it builds each layer from snapshots, other layers and options, and reuses a layer
//! whose key the store has. A plan says what a run would fetch and build, a run builds it, and
//! the state of each layer is computed when asked. `specs/obc-data.md` describes steps, keys,
//! receipts, plans, runs and states.

pub mod changes;
pub(crate) mod code;
pub mod plan;
mod process;
pub use code::{python_executable, runtime_rust, RuntimeRust};

pub mod release;
pub mod runs;
pub mod state;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::date;
use crate::store::{hash_file, sha256_hex, FileRecord, Snapshot, Store};

/// What a step reads, the code that makes its layer, and how it runs.
pub struct Step {
    /// The layer name: kebab-case segments joined by `/`.
    pub name: String,
    pub inputs: Vec<Input>,
    /// A JSON object.
    pub options: Value,
    pub code: Code,
    /// Paths in the output directory: a file, or a directory whose every file is part of the layer.
    pub outputs: Vec<String>,
    pub run: Run,
    /// The outputs that a client reads. Other files stay in the store.
    pub client: Client,
}

/// The published outputs of a layer: none, all, or selected declared paths (including directories).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    None,
    All,
    Paths(Vec<String>),
}

impl Client {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn includes(&self, path: &str) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Paths(paths) => paths.iter().any(|prefix| covers(prefix, path)),
        }
    }

    fn sorted(&self) -> Self {
        match self {
            Self::Paths(paths) => {
                let mut paths = paths.clone();
                paths.sort();
                paths.dedup();
                Self::Paths(paths)
            }
            other => other.clone(),
        }
    }
}

fn covers(prefix: &str, path: &str) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'))
}

pub enum Input {
    Snapshot {
        source: String,
        version: String,
        /// The `NAME=VALUE` of the fetch. The step reads the files that this fetch gives.
        params: Vec<(String, String)>,
        /// Without params: the names of the files the step reads, or none for every file.
        files: Vec<String>,
    },
    /// The layer of another step. `files` names the paths in the layer that the step reads, or
    /// none for every file.
    Layer { name: String, files: Vec<String> },
}

impl Input {
    /// Every file of the layer `name`.
    pub fn layer(name: impl Into<String>) -> Self {
        Input::Layer { name: name.into(), files: Vec::new() }
    }
}

/// The code that makes a layer. When in doubt, declare more: too much costs a rebuild, too little
/// gives stale data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Code {
    /// Files and directories, relative to the repository root.
    pub paths: Vec<String>,
    /// Workspace crates and their resolved normal and build dependencies.
    pub crates: Vec<String>,
    /// Resolve Rust dependencies for this target; None selects the producer host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// None binds the native dev build. Prepared builds bind their toolchain in step options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<Rust>,
    /// The content settings of these sources. Freshness and access controls are excluded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// The selected Python runtime and its locked package group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python: Option<Python>,
    /// A locked Python group packaged for another runtime, without selecting its interpreter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_packages: Option<String>,
    /// Native library or executable files bound by the provider before its code runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub libraries: Vec<Library>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Library {
    pub name: String,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rust {
    Native { profile: Profile },
    Prepared { profile: Profile },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Dev,
    Release,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Python {
    /// None selects the project's base packages, without default groups.
    pub group: Option<String>,
}

impl Code {
    /// File paths and named dependency fingerprints, in byte order, to SHA-256.
    pub fn files(&self, root: &Path) -> Result<BTreeMap<String, String>, String> {
        code::files(root, self)
    }

    /// Start tools with the same checked runtime policy as an engine step.
    pub fn command(&self, root: &Path, argv: &[String]) -> Result<std::process::Command, String> {
        let expected = code::hash(&self.files(root)?);
        process::command(root, argv, Some((self, &expected)))
    }

    /// Resolve source/config at a recorded target without selecting its execution tools.
    pub fn source_config(&self, root: &Path, rust: Option<&ResolvedRust>) -> Result<SourceIdentity, String> {
        code::source_config(root, self, rust)
    }

    /// Resolve content, execution and commitment inputs in one traversal.
    pub fn identity(&self, root: &Path) -> Result<CodeIdentity, String> {
        code::identity(root, self)
    }
}

/// Native acquisition or planning code, with scoped owner source and its actual dependencies.
#[derive(Debug, Clone)]
pub struct OwnerCode {
    pub crate_name: String,
    pub code: Code,
}

impl OwnerCode {
    pub fn identity(&self, root: &Path) -> Result<CodeIdentity, String> {
        code::owner_identity(root, self)
    }

    pub fn source_config(&self, root: &Path, rust: &ResolvedRust) -> Result<SourceIdentity, String> {
        code::owner_source_config(root, self, rust)
    }
}

/// Source compatibility is resolved at the recorded producer target and profile.
/// It permits consuming existing bytes; it does not prove that another host emits them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeIdentity {
    pub files: BTreeMap<String, String>,
    pub source_config: BTreeMap<String, String>,
    pub rust: Option<ResolvedRust>,
    /// Repository-relative physical inputs, before manifest or source projection.
    pub git_inputs: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceIdentity {
    pub files: BTreeMap<String, String>,
    pub rust: Option<ResolvedRust>,
    pub git_inputs: std::collections::BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedRust {
    pub target: String,
    pub build: Rust,
}

impl CodeIdentity {
    /// Commitment checks the selected physical inputs, including projected build metadata.
    pub fn committed(&self, root: &Path) -> Result<(), String> {
        code::committed(root, &self.git_inputs)
    }
}

impl SourceIdentity {
    pub fn committed(&self, root: &Path) -> Result<(), String> {
        code::committed(root, &self.git_inputs)
    }
}

pub enum Run {
    /// A function in this process. Its code must declare the crate of the function. One binary
    /// links every product, so Cargo unifies their features: a step crate enables every feature
    /// its bytes depend on itself, or makes its bytes independent of it (structs, or sorted keys
    /// for JSON objects).
    Rust(fn(&Request) -> Result<(), String>),
    /// A program and its arguments, started in the repository root with the request as JSON on
    /// standard input. No argument names a path outside the repository root.
    Command(Vec<String>),
}

/// What a step reads and where it writes.
#[derive(Debug, Serialize)]
pub struct Request {
    pub step: String,
    /// Source id, then file name, then object path.
    pub snapshots: BTreeMap<String, BTreeMap<String, PathBuf>>,
    /// Layer name, then path in the layer, then object path.
    pub layers: BTreeMap<String, BTreeMap<String, PathBuf>>,
    /// Receipt metadata for exactly the selected files of each layer input.
    pub layer_files: BTreeMap<String, Vec<LayerFile>>,
    pub options: Value,
    /// Exact native providers from the checked execution identity.
    pub libraries: Vec<Library>,
    /// An empty directory: the layer is the files the step writes in it.
    pub output: PathBuf,
    /// Where the step may write a JSON object of metrics.
    pub metrics: PathBuf,
}

/// The record of one layer: what made it, its files, and what building it cost.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub step: String,
    pub key: String,
    pub inputs: Vec<InputRecord>,
    pub options: Value,
    pub code: String,
    pub command: Option<Vec<String>>,
    /// The declared outputs, sorted.
    pub outputs: Vec<String>,
    /// The digest of `files`: what a step that reads this layer puts in its key.
    pub digest: String,
    pub files: Vec<LayerFile>,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub built: String,
    pub wall_ms: u64,
    pub cpu_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub metrics: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputRecord {
    pub kind: InputKind,
    /// The source id or the layer name.
    pub name: String,
    pub digest: String,
    /// Exact resolved snapshot file names, including an empty read. For a layer input, the
    /// selected paths, or none for every file. The digest names the bytes in the key.
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    // In the byte order of the names: a key sorts its inputs by kind.
    Layer,
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LayerFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug)]
pub struct Built {
    pub receipt: Receipt,
    pub reused: bool,
}

/// The SHA-256 of the lines that `sha256sum` writes for these files: `<sha256>  <name>`, sorted
/// by name.
pub fn digest<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut files: Vec<_> = files.into_iter().collect();
    files.sort();
    let text: String = files.iter().map(|(name, sha256)| format!("{sha256}  {name}\n")).collect();
    sha256_hex(text.as_bytes())
}

/// The key of a layer: the SHA-256 of its spec in canonical JSON.
pub fn key(receipt: &Receipt) -> String {
    let inputs: Vec<Value> = receipt
        .inputs
        .iter()
        .map(|input| serde_json::json!({"kind": input.kind, "name": input.name, "digest": input.digest}))
        .collect();
    let spec = serde_json::json!({
        "step": receipt.step,
        "command": receipt.command,
        "inputs": inputs,
        "options": receipt.options,
        "code": receipt.code,
        "outputs": receipt.outputs,
    });
    sha256_hex(&serde_json::to_vec(&sorted(spec)).expect("JSON values serialize"))
}

/// The recipe of a step with code hash `code`: its key without the digests of its inputs. A plan
/// holds it for each build, also for one whose key waits for a fetch or another build.
pub fn recipe(step: &Step, code: &str) -> String {
    let mut inputs: Vec<Value> = step
        .inputs
        .iter()
        .map(|input| match input {
            Input::Snapshot { source, version, params, files } => {
                let mut files = files.clone();
                files.sort();
                let params = crate::store::sorted(params);
                serde_json::json!({"kind": InputKind::Snapshot, "name": source, "version": version, "params": params, "files": files})
            }
            Input::Layer { name, files } => {
                let mut files = files.clone();
                files.sort();
                serde_json::json!({"kind": InputKind::Layer, "name": name, "files": files})
            }
        })
        .collect();
    inputs.sort_by_key(|input| input.to_string());
    let spec = serde_json::json!({
        "step": step.name,
        "command": step.command(),
        "inputs": inputs,
        "options": step.options,
        "code": code,
        "outputs": step.sorted_outputs(),
        "client": step.client.sorted(),
    });
    sha256_hex(&serde_json::to_vec(&sorted(spec)).expect("JSON values serialize"))
}

/// `value` with the keys of every object in byte order, whatever map serde_json was built with.
pub fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(entries.into_iter().map(|(key, value)| (key, sorted(value))).collect())
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        value => value,
    }
}

/// The steps with each one after the steps whose layers it reads.
fn order(steps: &[Step]) -> Result<Vec<&Step>, String> {
    let mut names = HashSet::new();
    for step in steps {
        if !step.name.split('/').all(crate::is_kebab) {
            return Err(format!("step `{}`: a layer name is kebab-case segments joined by `/`", step.name));
        }
        if !names.insert(step.name.as_str()) {
            return Err(format!("step `{}` is listed twice", step.name));
        }
    }
    for step in steps {
        let relative = |path: &String| {
            !path.contains(['\\', '\r', '\n'])
                && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
        };
        if let Some(path) = step.outputs.iter().find(|path| !relative(path)) {
            return Err(format!("step `{}`: output `{path}` is not a normalized relative path", step.name));
        }
        if let Client::Paths(paths) = &step.client {
            if paths.is_empty() {
                return Err(format!("step `{}`: client paths is empty; use none", step.name));
            }
            if let Some(path) = paths.iter().find(|path| !step.outputs.contains(path)) {
                return Err(format!("step `{}`: client path `{path}` is not a declared output", step.name));
            }
        }
        if let Some(name) = step.layers().find(|name| !names.contains(name)) {
            return Err(format!("step `{}` reads the layer `{name}`, which no step makes", step.name));
        }
        let both = |input: &Input| matches!(input, Input::Snapshot { params, files, .. } if !params.is_empty() && !files.is_empty());
        if step.inputs.iter().any(both) {
            return Err(format!(
                "step `{}`: a snapshot input selects its files by params or by name, not both",
                step.name
            ));
        }
    }
    let mut ordered = Vec::new();
    let mut placed = HashSet::new();
    while ordered.len() < steps.len() {
        let ready: Vec<&Step> = steps
            .iter()
            .filter(|step| !placed.contains(step.name.as_str()) && step.layers().all(|name| placed.contains(name)))
            .collect();
        if ready.is_empty() {
            return Err("the steps read each other's layers in a cycle".into());
        }
        placed.extend(ready.iter().map(|step| step.name.as_str()));
        ordered.extend(ready);
    }
    Ok(ordered)
}

impl Step {
    /// The names of the layers it reads.
    pub(crate) fn layers(&self) -> impl Iterator<Item = &str> {
        self.inputs.iter().filter_map(|input| match input {
            Input::Layer { name, .. } => Some(name.as_str()),
            Input::Snapshot { .. } => None,
        })
    }

    fn command(&self) -> Option<Vec<String>> {
        match &self.run {
            Run::Rust(_) => None,
            Run::Command(argv) => Some(argv.clone()),
        }
    }

    fn sorted_outputs(&self) -> Vec<String> {
        let mut outputs = self.outputs.clone();
        outputs.sort();
        outputs
    }
}

/// The code hash and the code files of each `Code`, computed once per value.
#[derive(Default)]
struct Codes<'a> {
    cached: HashMap<&'a Code, (String, CodeIdentity)>,
    context: code::Context,
}

impl<'a> Codes<'a> {
    fn get(&mut self, root: &Path, code: &'a Code) -> Result<&(String, CodeIdentity), String> {
        if !self.cached.contains_key(code) {
            let identity = self.context.identity(root, code)?;
            self.cached.insert(code, (code::hash(&identity.files), identity));
        }
        Ok(&self.cached[code])
    }
}

/// What the store has of a snapshot input.
enum Selection {
    /// The files that the input reads, each with its object.
    Present(Vec<FileRecord>),
    /// The names of the files that the store lacks. None when the store cannot name them: it has
    /// no record of the version, or no record of a fetch with the params.
    Lacks(Vec<String>),
}

fn selection(
    store: &Store,
    source: &str,
    version: &str,
    params: &[(String, String)],
    files: &[String],
) -> Result<Selection, String> {
    let names = match params {
        [] => files.to_vec(),
        params => match store.requested(source, version, params)? {
            // A fetch that gave no file: no names here must not select every file.
            Some(names) if names.is_empty() => return Ok(Selection::Present(Vec::new())),
            Some(names) => names,
            None => return Ok(Selection::Lacks(Vec::new())),
        },
    };
    let Some(snapshot) = store.snapshot(source, version)? else {
        return Ok(Selection::Lacks(names));
    };
    let (files, missing) = select(&snapshot, &names);
    let absent = files.iter().filter(|file| !store.object(&file.sha256).is_file()).map(|file| file.name.as_str());
    let lacks: BTreeSet<&str> = missing.into_iter().chain(absent).collect();
    if lacks.is_empty() {
        return Ok(Selection::Present(files.into_iter().cloned().collect()));
    }
    Ok(Selection::Lacks(lacks.into_iter().map(str::to_string).collect()))
}

/// The files of `snapshot` that `selected` names, or all of them when it names none, and the
/// selected names that the snapshot lacks.
fn select<'a>(snapshot: &'a Snapshot, selected: &'a [String]) -> (Vec<&'a FileRecord>, Vec<&'a str>) {
    let files = snapshot.files.iter().filter(|file| selected.is_empty() || selected.contains(&file.name)).collect();
    let missing = selected.iter().filter(|name| !snapshot.files.iter().any(|file| &file.name == *name));
    (files, missing.map(String::as_str).collect())
}

/// The object of each file that a snapshot input with these fields reads, by file name, or `None`
/// while the store lacks one. A product reads a file that its step list depends on this way.
pub fn snapshot_files(
    store: &Store,
    source: &str,
    version: &str,
    params: &[(String, String)],
    files: &[String],
) -> Result<Option<BTreeMap<String, PathBuf>>, String> {
    Ok(match selection(store, source, version, params, files)? {
        Selection::Present(files) => {
            Some(files.into_iter().map(|file| (file.name, store.object(&file.sha256))).collect())
        }
        Selection::Lacks(_) => None,
    })
}

/// The files of a layer that `selected` names, or all of them when it names none, and the selected
/// paths that the layer lacks.
fn select_layer<'a>(files: &'a [LayerFile], selected: &'a [String]) -> (Vec<&'a LayerFile>, Vec<&'a str>) {
    let chosen = files.iter().filter(|file| selected.is_empty() || selected.contains(&file.path)).collect();
    let missing = selected.iter().filter(|path| !files.iter().any(|file| &file.path == *path));
    (chosen, missing.map(String::as_str).collect())
}

/// The digest of the files of a layer that `selected` names: what a step that reads them keys.
pub(crate) fn layer_digest(files: &[LayerFile], selected: &[String]) -> String {
    digest(select_layer(files, selected).0.into_iter().map(|file| (file.path.as_str(), file.sha256.as_str())))
}

/// Link each file of `files` (a path such as `layer/a.pbf`, and its object) into the new directory
/// `dir`, for a tool that reads a directory. `dir` must not be in the output of the step; the
/// directory beside it, `request.output.with_file_name("view")`, goes when the step ends.
pub fn view(files: &BTreeMap<String, PathBuf>, dir: &Path) -> Result<(), String> {
    fs::create_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, object) in files {
        if name.split('/').any(|part| part.is_empty() || part == "." || part == "..") {
            return Err(format!("{name} is not a relative path"));
        }
        let link = dir.join(name);
        fs::create_dir_all(link.parent().expect("a joined path has a parent"))
            .map_err(|e| format!("{}: {e}", link.display()))?;
        symlink(object, &link).map_err(|e| format!("{}: {e}", link.display()))?;
    }
    Ok(())
}

/// A step whose layer is the one file of its snapshot or selected layer inputs, at the path of the option
/// `path`: a layer that other steps read, whatever gives its bytes. It only passes an input on, so,
/// like the rest of the engine, it is no code of a step. Its step declares the crate `obc-data`,
/// which adds no file.
pub fn pass(request: &Request) -> Result<(), String> {
    let path = request.options["path"].as_str().ok_or("option `path` is not a string")?;
    let files: Vec<&PathBuf> =
        request.snapshots.values().chain(request.layers.values()).flat_map(BTreeMap::values).collect();
    let [file] = files[..] else {
        return Err(format!("the step reads {} files, not one", files.len()));
    };
    let output = request.output.join(path);
    // The output is the same object as the input, so a link saves a copy.
    fs::hard_link(file, &output)
        .or_else(|_| fs::copy(file, &output).map(drop))
        .map_err(|e| format!("{}: {e}", output.display()))
}

#[cfg(unix)]
fn symlink(object: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(object, link)
}

#[cfg(not(unix))]
fn symlink(object: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(object, link)
}

/// The receipt of `step` before it runs, with its key, and its request. `layers` holds the
/// receipts of the layers it reads; `code` is its code hash.
fn prepare(
    store: &Store,
    step: &Step,
    layers: &HashMap<&str, release::Layer>,
    code: &str,
) -> Result<(Receipt, Request), String> {
    if !step.options.is_object() {
        return Err("the options are not a JSON object".into());
    }
    let mut request = Request {
        step: step.name.clone(),
        snapshots: BTreeMap::new(),
        layers: BTreeMap::new(),
        layer_files: BTreeMap::new(),
        options: step.options.clone(),
        libraries: step.code.libraries.clone(),
        output: PathBuf::new(),
        metrics: PathBuf::new(),
    };
    let mut inputs = Vec::new();
    let mut bytes_in = 0;
    for input in &step.inputs {
        let (kind, name, selected, files): (_, _, _, Vec<(String, String, u64)>) = match input {
            Input::Snapshot { source, version, params, files: selected } => {
                let files = match selection(store, source, version, params, selected)? {
                    Selection::Present(files) => files,
                    Selection::Lacks(names) => {
                        return Err(match names.first() {
                            Some(name) => format!("snapshot {source}@{version} has no file {name}; fetch it first"),
                            None => format!("the store has no snapshot {source}@{version}; fetch it first"),
                        })
                    }
                };
                let mut names: Vec<_> = files.iter().map(|file| file.name.clone()).collect();
                names.sort();
                let files = files.into_iter().map(|file| (file.name, file.sha256, file.size));
                (InputKind::Snapshot, source, names, files.collect())
            }
            Input::Layer { name, files: selected } => {
                let (files, missing) = select_layer(&layers[name.as_str()].files, selected);
                if let Some(path) = missing.first() {
                    return Err(format!("layer `{name}` has no file {path}"));
                }
                let files = files.into_iter().map(|file| (file.path.clone(), file.sha256.clone(), file.size));
                let mut selected = selected.clone();
                selected.sort();
                (InputKind::Layer, name, selected, files.collect())
            }
        };
        let digest = digest(files.iter().map(|(name, sha256, _)| (name.as_str(), sha256.as_str())));
        let record = InputRecord { kind, name: name.clone(), digest, files: selected };
        if kind == InputKind::Layer {
            let metadata = files
                .iter()
                .map(|(path, sha256, size)| LayerFile { path: path.clone(), sha256: sha256.clone(), size: *size })
                .collect();
            request.layer_files.insert(name.clone(), metadata);
        }
        let mut paths = BTreeMap::new();
        for (name, sha256, size) in files {
            let object = store.object(&sha256);
            if !object.is_file() {
                return Err(format!("{} `{}`: the object of {name} is missing", record.kind.label(), record.name));
            }
            if paths.insert(name.clone(), object).is_some() {
                return Err(format!("{} `{}` has two files named {name}", record.kind.label(), record.name));
            }
            bytes_in += size;
        }
        let map = match record.kind {
            InputKind::Snapshot => &mut request.snapshots,
            InputKind::Layer => &mut request.layers,
        };
        if map.insert(record.name.clone(), paths).is_some() {
            return Err(format!("{} `{}` is an input twice", record.kind.label(), record.name));
        }
        inputs.push(record);
    }
    inputs.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    match &step.run {
        Run::Rust(_) if step.code.crates.is_empty() => {
            return Err("a Rust step must declare the crate of its function in its code".into());
        }
        Run::Rust(_)
            if matches!(step.code.rust, Some(Rust::Prepared { .. } | Rust::Native { profile: Profile::Release })) =>
        {
            return Err(
                "a Rust function runs in the native dev worker; use a command for prepared or release code".into()
            );
        }
        Run::Command(argv) => {
            let outside = |arg: &&String| {
                Path::new(arg).is_absolute() || arg.contains("=/") || arg.split(['/', '=']).any(|part| part == "..")
            };
            if let Some(arg) = argv.iter().find(outside) {
                return Err(format!(
                    "the argument {arg} names a path outside the repository, which differs between machines"
                ));
            }
        }
        Run::Rust(_) => {}
    }
    let mut receipt = Receipt {
        step: step.name.clone(),
        key: String::new(),
        inputs,
        options: step.options.clone(),
        code: code.to_string(),
        command: step.command(),
        outputs: step.sorted_outputs(),
        digest: String::new(),
        files: Vec::new(),
        built: String::new(),
        wall_ms: 0,
        cpu_ms: None,
        peak_rss_bytes: None,
        bytes_in,
        bytes_out: 0,
        metrics: BTreeMap::new(),
    };
    receipt.key = key(&receipt);
    Ok((receipt, request))
}

/// The receipt of the layer with `key` when the store has it and all of its objects.
fn reusable(store: &Store, key: &str) -> Result<Option<Receipt>, String> {
    let stored = store.layer(key)?;
    Ok(stored.filter(|receipt| receipt.files.iter().all(|file| store.object(&file.sha256).is_file())))
}

/// Reuse the layer of the prepared key, or run the step and record its layer.
fn build_step(
    store: &Store,
    root: &Path,
    step: &Step,
    mut receipt: Receipt,
    mut request: Request,
    checks: &std::sync::Mutex<code::Context>,
) -> Result<Built, String> {
    let _lock = store.lock(&format!("layer-{}", receipt.key))?;
    check_code(checks, root, step, &receipt.code)?;
    if let Some(mut stored) = reusable(store, &receipt.key)? {
        stored.inputs = receipt.inputs.clone();
        return Ok(Built { receipt: stored, reused: true });
    }
    let work = store.partial(&format!("layer-{}", receipt.key));
    request.output = work.join("output");
    request.metrics = work.join("metrics.json");
    remove_dir(&work)?;
    let result = execute(store, root, step, &request, &mut receipt, checks);
    let removed = remove_dir(&work);
    result?;
    removed?;
    store.put_layer(&receipt)?;
    Ok(Built { receipt, reused: false })
}

/// Run the step, move its files into the objects, and record them, its metrics and its cost.
fn execute(
    store: &Store,
    root: &Path,
    step: &Step,
    request: &Request,
    receipt: &mut Receipt,
    checks: &std::sync::Mutex<code::Context>,
) -> Result<(), String> {
    fs::create_dir_all(&request.output).map_err(|e| format!("{}: {e}", request.output.display()))?;
    let usage = match &step.run {
        Run::Rust(function) => process::in_process(|| function(request)),
        Run::Command(argv) => process::run(root, argv, request, Some((&step.code, receipt.code.as_str()))),
    }?;
    check_code(checks, root, step, &receipt.code)?;
    receipt.files = collect(store, &request.output, &step.outputs)?;
    receipt.metrics = match fs::read_to_string(&request.metrics) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("the metrics are not a JSON object: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(format!("{}: {e}", request.metrics.display())),
    };
    receipt.digest = digest(receipt.files.iter().map(|file| (file.path.as_str(), file.sha256.as_str())));
    receipt.bytes_out = receipt.files.iter().map(|file| file.size).sum();
    receipt.built = date::timestamp(date::now());
    receipt.wall_ms = usage.wall_ms;
    receipt.cpu_ms = usage.cpu_ms;
    receipt.peak_rss_bytes = usage.peak_rss_bytes;
    Ok(())
}

fn check_code(
    checks: &std::sync::Mutex<code::Context>,
    root: &Path,
    step: &Step,
    expected: &str,
) -> Result<(), String> {
    let mut context = checks.lock().map_err(|_| "code checking failed")?;
    context.refresh_python();
    if code::hash(&context.files(root, &step.code)?) != expected {
        return Err(format!("the code of {} changed; plan again", step.name));
    }
    Ok(())
}

impl InputKind {
    fn label(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Layer => "layer",
        }
    }
}

/// Move the files of `output` into the objects. The step must write each declared output, and
/// nothing else.
fn collect(store: &Store, output: &Path, declared: &[String]) -> Result<Vec<LayerFile>, String> {
    let mut paths = Vec::new();
    walk(output, "", &mut paths)?;
    if let Some(missing) = declared
        .iter()
        .find(|declared| !output.join(declared).is_dir() && !paths.iter().any(|path| covers(declared, path)))
    {
        return Err(format!("it did not write its output {missing}"));
    }
    if let Some(extra) = paths.iter().find(|path| !declared.iter().any(|declared| covers(declared, path))) {
        return Err(format!("it wrote {extra}, which is not one of its outputs"));
    }
    let mut files = Vec::new();
    for path in paths {
        let file = output.join(&path);
        let (sha256, size) = hash_file(&file)?;
        store.insert(&file, &sha256)?;
        files.push(LayerFile { path, size, sha256 });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// The paths of the files below `dir`, joined by `/` after `prefix`.
fn walk(dir: &Path, prefix: &str, paths: &mut Vec<String>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = entry.file_name().into_string().map_err(|name| format!("{name:?} is not UTF-8"))?;
        let path = format!("{prefix}{name}");
        let kind = entry.file_type().map_err(|e| format!("{path}: {e}"))?;
        if kind.is_dir() {
            walk(&entry.path(), &format!("{path}/"), paths)?;
        } else if kind.is_file() {
            paths.push(path);
        } else {
            return Err(format!("it wrote {path}, which is not a file or a directory"));
        }
    }
    Ok(())
}

fn remove_dir(dir: &Path) -> Result<(), String> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", dir.display())),
        _ => Ok(()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::tests::Scratch;
    use crate::store::write_atomic;
    use serde_json::json;
    use std::process::Command;

    pub(crate) const JOIN: &str = "import json, os, sys
request = json.load(sys.stdin)
upper = open(request['layers']['test/upper']['upper.txt']).read()
tail = open(request['snapshots']['tail']['tail.txt']).read()
open(os.path.join(request['output'], 'joined.txt'), 'w').write(upper + tail)
json.dump({'characters': len(upper + tail)}, open(request['metrics'], 'w'))
";

    pub(crate) struct Fixture {
        pub(crate) scratch: Scratch,
        pub(crate) store: Store,
    }

    impl Fixture {
        pub(crate) fn root(&self) -> PathBuf {
            self.scratch.0.join("repository")
        }

        pub(crate) fn with_acquisition(&self) {
            let root = self.root();
            let source = crate::fetch::tests::source("https://example.org/file.bin", "date");
            for path in crate::fetch::owner_code(&source).code.paths {
                write(&root.join(path), "// fixture acquisition backend\n");
            }
            write(&root.join("host/obc-data/src/lib.rs"), "");
            write(
                &root.join("host/obc-data/Cargo.toml"),
                "[package]\nname = \"obc-data\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            );
            let manifest = root.join("Cargo.toml");
            let mut workspace: toml::Value = toml::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
            workspace["workspace"]["members"].as_array_mut().unwrap().push("host/obc-data".into());
            write(&manifest, &toml::to_string(&workspace).unwrap());
            let lock =
                Command::new("cargo").args(["generate-lockfile", "--offline"]).current_dir(&root).output().unwrap();
            assert!(lock.status.success(), "{}", String::from_utf8_lossy(&lock.stderr));
        }

        pub(crate) fn with_sources(&self, sources: &[crate::sources::Source]) {
            let root = self.root();
            for source in sources {
                for path in crate::fetch::owner_code(source).code.paths {
                    write(&root.join(path), "// fixture acquisition backend\n");
                }
            }
            let mut registry = sources.to_vec();
            for source in &mut registry {
                // Registry URLs stay HTTPS; requests use the existing loopback transport seam.
                if let Some(url) = &mut source.fetch.url {
                    *url = url.replacen("http://", "https://", 1);
                }
            }
            write(
                &root.join("data/sources.toml"),
                &toml::to_string(&std::collections::BTreeMap::from([("source", registry)])).unwrap(),
            );
        }

        pub(crate) fn plan(&self, steps: &[Step]) -> Result<plan::Plan, String> {
            plan::plan(&self.store, &self.root(), steps)
        }

        /// Plan `steps`, and run the plan.
        pub(crate) fn build(&self, steps: &[Step]) -> Result<Vec<Built>, String> {
            self.run(steps, &self.plan(steps)?, runs::Limits::machine())
        }

        pub(crate) fn run(
            &self,
            steps: &[Step],
            plan: &plan::Plan,
            limits: runs::Limits,
        ) -> Result<Vec<Built>, String> {
            let mut run = runs::Run::create(&self.store, "build test")?;
            let (root, http) = (self.root(), crate::fetch::http::Http::new());
            let context =
                runs::Context { store: &self.store, root: &root, sources: &[], http: &http, copies: None, limits };
            let built = run.build(&context, steps, plan);
            run.finish(built.as_ref().err().map(String::as_str))?;
            built
        }

        /// Add a file to the record of `source@1`.
        pub(crate) fn fetched(&self, source: &str, name: &str, bytes: &[u8]) {
            self.fetched_version(source, "1", name, bytes);
        }

        pub(crate) fn fetched_version(&self, source: &str, version: &str, name: &str, bytes: &[u8]) {
            let file = self.store.partial(name);
            write_atomic(&file, bytes).unwrap();
            let sha256 = sha256_hex(bytes);
            self.store.insert(&file, &sha256).unwrap();
            let url = format!("https://example.org/{name}");
            let retrieved = "2026-10-05T00:00:00Z".into();
            let mut snapshot = self.store.snapshot(source, version).unwrap().unwrap_or_else(|| Snapshot {
                source: source.into(),
                version: version.into(),
                files: Vec::new(),
            });
            snapshot.files.push(FileRecord { name: name.into(), url, size: bytes.len() as u64, sha256, retrieved });
            self.store.put_snapshot(&snapshot).unwrap();
        }
    }

    pub(crate) fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// A git repository with a workspace of `crates`; each `(name, dependencies)`.
    pub(crate) fn repository(root: &Path, crates: &[(&str, &str)]) {
        let members: Vec<String> = crates.iter().map(|(name, _)| format!("{name:?}")).collect();
        write(
            &root.join("Cargo.toml"),
            &format!("[workspace]\nmembers = [{}]\nresolver = \"2\"\n", members.join(", ")),
        );
        for (name, dependencies) in crates {
            let package = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
            write(&root.join(format!("{name}/Cargo.toml")), &(package + dependencies));
            write(&root.join(format!("{name}/src/lib.rs")), "");
        }
        assert!(Command::new("git").args(["init", "-q"]).current_dir(root).status().unwrap().success());
        let lock = Command::new("cargo").args(["generate-lockfile", "--offline"]).current_dir(root).output().unwrap();
        assert!(lock.status.success(), "{}", String::from_utf8_lossy(&lock.stderr));
    }

    /// A store with the snapshots `head@1` and `tail@1`, and a repository with `join.py` and the
    /// crate `steps`.
    pub(crate) fn fixture(name: &str) -> Fixture {
        let scratch = Scratch::new(name);
        let store = Store::at(scratch.0.join("store"));
        let fixture = Fixture { scratch, store };
        fixture.fetched("head", "head.txt", b"head\n");
        fixture.fetched("tail", "tail.txt", b"tail\n");
        repository(&fixture.root(), &[("steps", "")]);
        fs::write(fixture.root().join("join.py"), JOIN).unwrap();
        fixture
    }

    pub(crate) fn upper(request: &Request) -> Result<(), String> {
        let text = fs::read_to_string(&request.snapshots["head"]["head.txt"]).map_err(|e| e.to_string())?;
        fs::write(request.output.join("upper.txt"), text.to_uppercase()).map_err(|e| e.to_string())
    }

    fn count(request: &Request) -> Result<(), String> {
        let text = fs::read_to_string(&request.layers["test/join"]["joined.txt"]).map_err(|e| e.to_string())?;
        fs::create_dir(request.output.join("count")).map_err(|e| e.to_string())?;
        fs::write(request.output.join("count/lines.txt"), text.lines().count().to_string()).map_err(|e| e.to_string())
    }

    pub(crate) fn step(name: &str, inputs: Vec<Input>, code: Code, output: &str, run: Run) -> Step {
        Step {
            name: name.into(),
            inputs,
            options: json!({}),
            code,
            outputs: vec![output.into()],
            run,
            client: Client::All,
        }
    }

    pub(crate) fn packaged(client: Client) -> Step {
        fn run(request: &Request) -> Result<(), String> {
            for dir in ["published", "published-extra"] {
                fs::create_dir(request.output.join(dir)).map_err(|e| e.to_string())?;
            }
            for (path, bytes) in [
                ("published/a", "payload"),
                ("published/b", "payload"),
                ("published-extra/a", "other"),
                ("index.json", "metadata"),
            ] {
                fs::write(request.output.join(path), bytes).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Step {
            client,
            outputs: ["published", "published-extra", "index.json"].map(String::from).into(),
            ..step("test/package", Vec::new(), steps_crate(), "", Run::Rust(run))
        }
    }

    pub(crate) fn steps_crate() -> Code {
        Code { paths: Vec::new(), crates: vec!["steps".into()], ..Default::default() }
    }

    pub(crate) fn snapshot(source: &str, version: &str, files: &[&str]) -> Input {
        Input::Snapshot {
            source: source.into(),
            version: version.into(),
            params: Vec::new(),
            files: files.iter().map(|file| file.to_string()).collect(),
        }
    }

    /// Two fetched sources and three steps, the second a Python command. The last step is listed
    /// first: the engine orders them. `test/upper` selects `head.txt`; `test/join` reads every file
    /// of `tail@1`.
    pub(crate) fn pipeline() -> Vec<Step> {
        let join = Code { paths: vec!["join.py".into()], crates: Vec::new(), ..Default::default() };
        let python = Run::Command(vec!["python3".into(), "join.py".into()]);
        vec![
            step("test/count", vec![Input::layer("test/join")], steps_crate(), "count", Run::Rust(count)),
            step("test/join", vec![Input::layer("test/upper"), snapshot("tail", "1", &[])], join, "joined.txt", python),
            step(
                "test/upper",
                vec![snapshot("head", "1", &["head.txt"])],
                steps_crate(),
                "upper.txt",
                Run::Rust(upper),
            ),
        ]
    }

    pub(crate) fn summary(built: &[Built]) -> Vec<(&str, bool)> {
        built.iter().map(|built| (built.receipt.step.as_str(), built.reused)).collect()
    }

    fn code_hash(root: &Path, code: &Code) -> Result<String, String> {
        code::files(root, code).map(|files| code::hash(&files))
    }

    #[test]
    fn a_pipeline_builds_then_reuses_and_records_receipts() {
        let fixture = fixture("engine-pipeline");
        let first = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&first), [("test/upper", false), ("test/join", false), ("test/count", false)]);

        let join = &first[1].receipt;
        let joined = LayerFile { path: "joined.txt".into(), size: 10, sha256: sha256_hex(b"HEAD\ntail\n") };
        assert_eq!(join.files, [joined]);
        assert_eq!(join.digest, digest([("joined.txt", join.files[0].sha256.as_str())]));
        assert_eq!((join.bytes_in, join.bytes_out), (10, 10));
        assert_eq!(join.metrics["characters"], 10);
        assert_eq!(join.key, key(join));
        assert_eq!(fixture.store.layer(&join.key).unwrap().as_ref(), Some(join));
        assert!(fs::metadata(fixture.store.object(&join.files[0].sha256)).unwrap().permissions().readonly());
        if cfg!(unix) {
            assert!(join.cpu_ms.is_some(), "{join:?}");
            assert!(join.peak_rss_bytes.unwrap() > 1 << 20, "{join:?}");
        }
        assert_eq!(first[2].receipt.files[0].path, "count/lines.txt");

        assert_eq!(fixture.plan(&pipeline()).unwrap().groups, [], "the store has every layer");
    }

    #[test]
    fn an_empty_client_directory_retains_metadata_and_missing_outputs_still_fail() {
        let fixture = fixture("engine-empty-client");
        fn sea(request: &Request) -> Result<(), String> {
            fs::create_dir(request.output.join("terrain")).map_err(|e| e.to_string())?;
            write_atomic(&request.output.join("metadata/empty.json"), b"[\"13/10000/10000\"]")
        }
        let step = Step {
            name: "test/sea".into(),
            inputs: Vec::new(),
            options: json!({}),
            code: steps_crate(),
            outputs: vec!["terrain".into(), "metadata".into()],
            run: Run::Rust(sea),
            client: Client::Paths(vec!["terrain".into()]),
        };
        let built = fixture.build(&[step]).unwrap();
        assert_eq!(
            built[0].receipt.files.iter().map(|file| file.path.as_str()).collect::<Vec<_>>(),
            ["metadata/empty.json"]
        );
        let missing = Step {
            name: "test/missing".into(),
            inputs: Vec::new(),
            options: json!({}),
            code: steps_crate(),
            outputs: vec!["absent".into()],
            run: Run::Rust(sea),
            client: Client::All,
        };
        assert!(fixture.build(&[missing]).unwrap_err().contains("did not write its output absent"));
        let step = Step {
            name: "test/sea".into(),
            inputs: Vec::new(),
            options: json!({}),
            code: steps_crate(),
            outputs: vec!["terrain".into(), "metadata".into()],
            run: Run::Rust(sea),
            client: Client::Paths(vec!["terrain".into()]),
        };
        let release = release::release(&fixture.store, &fixture.root(), "test", "sea", &[], &[step]).unwrap().unwrap();
        assert!(release.objects().is_empty());
        assert_eq!(release.layers[0].files.len(), 1);
    }

    #[test]
    fn a_code_change_rebuilds_its_layer_and_stops_where_the_bytes_are_the_same() {
        let fixture = fixture("engine-code");
        let first = fixture.build(&pipeline()).unwrap();

        fs::write(fixture.root().join("join.py"), format!("# The same bytes.\n{JOIN}")).unwrap();
        let second = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&second), [("test/join", false), ("test/count", true)]);
        assert_ne!(second[0].receipt.key, first[1].receipt.key);
        assert_eq!(second[1].receipt, first[2].receipt);

        fs::write(fixture.root().join("join.py"), JOIN.replace("upper + tail", "tail + upper")).unwrap();
        let third = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&third), [("test/join", false), ("test/count", false)]);
        assert_ne!(third[1].receipt.key, second[1].receipt.key);
    }

    #[test]
    fn a_snapshot_input_keys_only_the_files_it_selects() {
        let fixture = fixture("engine-select");
        let built = fixture.build(&pipeline()).unwrap();
        let mut old = built[0].receipt.clone();
        let resolved = old.inputs[0].files.clone();
        assert!(!resolved.is_empty());
        old.inputs[0].files.clear();
        fixture.store.put_layer(&old).unwrap();
        let released =
            release::release(&fixture.store, &fixture.root(), "test", "monaco", &[], &pipeline()).unwrap().unwrap();
        let restored = released.layers.iter().find(|l| l.key == old.key).unwrap();
        assert_eq!(
            restored.inputs[0].files, resolved,
            "a freshly resolved digest-matching selection repairs provenance without rebuilding bytes"
        );

        fixture.fetched("head", "other.txt", b"other\n");
        assert_eq!(fixture.plan(&pipeline()).unwrap().groups, []);

        fixture.fetched("tail", "other.txt", b"other\n");
        let built = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&built), [("test/join", false), ("test/count", true)]);

        let mut steps = pipeline();
        steps[2].inputs = vec![snapshot("head", "1", &["x"])];
        let fetches = &fixture.plan(&steps).unwrap().groups[0].fetches;
        assert_eq!(fetches[0].files, ["x"], "a missing file is a fetch");
        let err = fixture.build(&steps).err().unwrap();
        assert_eq!(err, "fetch head@1: no source `head` in data/sources.toml");
    }

    #[test]
    fn a_layer_input_keys_and_passes_only_the_files_it_selects() {
        fn split(request: &Request) -> Result<(), String> {
            fs::create_dir(request.output.join("leaves")).map_err(|e| e.to_string())?;
            for (name, object) in &request.snapshots["head"] {
                fs::copy(object, request.output.join("leaves").join(name)).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        fn one(request: &Request) -> Result<(), String> {
            assert_eq!(request.layer_files.keys().collect::<Vec<_>>(), request.layers.keys().collect::<Vec<_>>());
            let files = &request.layer_files["test/leaves"];
            assert_eq!(
                files.iter().map(|file| &file.path).collect::<Vec<_>>(),
                request.layers["test/leaves"].keys().collect::<Vec<_>>()
            );
            assert_eq!((files[0].sha256.as_str(), files[0].size), (sha256_hex(b"head\n").as_str(), 5));
            let [object] = request.layers["test/leaves"].values().collect::<Vec<_>>()[..] else {
                return Err("the step reads more than one file".into());
            };
            fs::copy(object, request.output.join("one.txt")).map(drop).map_err(|e| e.to_string())
        }
        let steps = |file: &str| {
            vec![
                step("test/leaves", vec![snapshot("head", "1", &[])], steps_crate(), "leaves", Run::Rust(split)),
                step(
                    "test/one",
                    vec![Input::Layer { name: "test/leaves".into(), files: vec![file.into()] }],
                    steps_crate(),
                    "one.txt",
                    Run::Rust(one),
                ),
            ]
        };
        let fixture = fixture("engine-layer-files");
        fixture.build(&steps("leaves/head.txt")).unwrap();
        fixture.fetched("head", "other.txt", b"other\n");
        let built = fixture.build(&steps("leaves/head.txt")).unwrap();
        assert_eq!(summary(&built), [("test/leaves", false), ("test/one", true)]);

        let err = fixture.build(&steps("leaves/x.txt")).err().unwrap();
        assert!(err.contains("layer `test/leaves` has no file leaves/x.txt"), "{err}");
    }

    #[test]
    fn a_step_that_breaks_its_contract_fails_and_leaves_no_receipt_and_no_work() {
        fn nothing(_: &Request) -> Result<(), String> {
            Ok(())
        }
        fn extra(request: &Request) -> Result<(), String> {
            upper(request)?;
            fs::write(request.output.join("extra.txt"), "").map_err(|e| e.to_string())
        }
        #[cfg(unix)]
        fn link(request: &Request) -> Result<(), String> {
            std::os::unix::fs::symlink("/etc/hostname", request.output.join("upper.txt")).map_err(|e| e.to_string())
        }
        let fixture = fixture("engine-contract");
        let head = || vec![snapshot("head", "1", &[])];
        let mut cases = vec![
            (Run::Rust(nothing), steps_crate(), "did not write its output upper.txt"),
            (Run::Rust(extra), steps_crate(), "wrote extra.txt, which is not one of its outputs"),
            (Run::Rust(upper), Code::default(), "a Rust step must declare the crate"),
            (
                Run::Rust(upper),
                Code { rust: Some(Rust::Native { profile: Profile::Release }), ..steps_crate() },
                "native dev worker",
            ),
            (
                Run::Rust(upper),
                Code {
                    rust: Some(Rust::Prepared { profile: Profile::Release }),
                    target: Some("x86_64-unknown-linux-gnu".into()),
                    ..steps_crate()
                },
                "native dev worker",
            ),
            (Run::Command(vec!["/usr/bin/true".into()]), Code::default(), "argument /usr/bin/true names a path"),
            (Run::Command(vec!["x".into(), "--in=/tmp".into()]), Code::default(), "argument --in=/tmp names a path"),
            (Run::Command(vec!["x".into(), "a/../../b".into()]), Code::default(), "argument a/../../b names a path"),
        ];
        #[cfg(unix)]
        cases.push((Run::Rust(link), steps_crate(), "wrote upper.txt, which is not a file or a directory"));
        for (run, code, expected) in cases {
            let err = fixture.build(&[step("test/bad", head(), code, "upper.txt", run)]).err().unwrap();
            assert!(err.contains(expected), "{err}");
        }
        assert!(!fixture.store.root().join("layers").exists());
        let work = fs::read_dir(fixture.store.root().join("partial")).unwrap();
        let work: Vec<_> = work.map(|entry| entry.unwrap().file_name()).collect();
        assert!(work.iter().all(|name| !name.to_string_lossy().starts_with("layer-")), "{work:?}");
    }

    #[test]
    fn a_crate_binds_path_sources_and_included_files_without_dev_sources_or_lock_comments() {
        let scratch = Scratch::new("engine-crates");
        let app = "[dependencies]\nlib = { path = \"../lib\" }\n[dev-dependencies]\ncheck = { path = \"../check\" }\n";
        repository(&scratch.0, &[("app", app), ("lib", ""), ("check", "")]);
        let hash = |paths: &[&str]| {
            let code = Code {
                paths: paths.iter().map(|path| path.to_string()).collect(),
                crates: vec!["app".into()],
                ..Default::default()
            };
            code_hash(&scratch.0, &code).unwrap()
        };
        let before = hash(&[]);
        write(&scratch.0.join("check/src/lib.rs"), "pub fn helper() {}\n");
        let lock = fs::read_to_string(scratch.0.join("Cargo.lock")).unwrap();
        write(&scratch.0.join("Cargo.lock"), &(lock + "\n# A lock record comment.\n"));
        assert_eq!(hash(&[]), before, "dev sources and lock comments do not change the resolved producer identity");
        let lib =
            "pub const TABLE: &str = include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/../\", r\"table.txt\"));
#[path = \"../../gen/x.rs\"]
mod x;
";
        write(&scratch.0.join("lib/src/lib.rs"), lib);
        write(&scratch.0.join("gen/x.rs"), "const DEEP: &[u8] = include_bytes!(r#\"deep.bin\"#);\n");
        write(&scratch.0.join("table.txt"), "1\n");
        write(&scratch.0.join("gen/deep.bin"), "1\n");
        let changed = hash(&[]);
        assert_ne!(changed, before);
        write(&scratch.0.join("table.txt"), "2\n");
        assert_ne!(hash(&[]), changed, "an included file is code");
        let changed = hash(&[]);
        write(&scratch.0.join("gen/deep.bin"), "2\n");
        assert_ne!(hash(&[]), changed, "a file that a module file includes is code");
        assert_ne!(hash(&["Cargo.lock"]), hash(&[]));
    }

    #[test]
    fn rust_content_identity_follows_explicit_targets_for_normal_and_build_dependencies() {
        let scratch = Scratch::new("engine-target-code");
        let app = "[target.'cfg(target_os = \"linux\")'.dependencies]\nlinux-normal = { path = \"../linux-normal\" }\n[target.'cfg(target_os = \"linux\")'.build-dependencies]\nlinux-build = { path = \"../linux-build\" }\n[target.'cfg(target_os = \"macos\")'.dependencies]\nmac-normal = { path = \"../mac-normal\" }\n[target.'cfg(target_os = \"macos\")'.build-dependencies]\nmac-build = { path = \"../mac-build\" }\n";
        repository(
            &scratch.0,
            &[("app", app), ("linux-normal", ""), ("linux-build", ""), ("mac-normal", ""), ("mac-build", "")],
        );
        write(&scratch.0.join("app/build.rs"), "fn main() {}\n");
        let mut context = code::Context::default();
        let selected = |target: &str| Code {
            crates: vec!["app".into()],
            target: Some(target.into()),
            rust: Some(Rust::Prepared { profile: Profile::Release }),
            ..Default::default()
        };
        let linux = context.files(&scratch.0, &selected("x86_64-unknown-linux-gnu")).unwrap();
        let mac = context.files(&scratch.0, &selected("aarch64-apple-darwin")).unwrap();
        for kind in ["normal", "build"] {
            assert!(
                linux.contains_key(&format!("linux-{kind}/src/lib.rs")),
                "Linux {kind} is selected from any probe host"
            );
            assert!(!linux.contains_key(&format!("mac-{kind}/src/lib.rs")));
            assert!(mac.contains_key(&format!("mac-{kind}/src/lib.rs")));
            assert!(!mac.contains_key(&format!("linux-{kind}/src/lib.rs")));
        }
        assert_ne!(linux["rust/target"], mac["rust/target"]);
        write(&scratch.0.join("linux-build/src/lib.rs"), "pub fn changed() {}\n");
        assert_ne!(context.files(&scratch.0, &selected("x86_64-unknown-linux-gnu")).unwrap(), linux);
        assert_eq!(context.files(&scratch.0, &selected("aarch64-apple-darwin")).unwrap(), mac);
    }

    #[test]
    fn the_engine_is_no_code_of_a_step() {
        let scratch = Scratch::new("engine-not-code");
        repository(
            &scratch.0,
            &[("steps", "[dependencies]\nobc-data = { path = \"../obc-data\" }\n"), ("obc-data", "")],
        );
        let code = Code { paths: Vec::new(), crates: vec!["steps".into()], ..Default::default() };
        write(&scratch.0.join("data/sources.toml"), "source = []\n");
        let before = code_hash(&scratch.0, &code).unwrap();
        let compiled = code::compiled(&scratch.0, &code.crates).unwrap();
        write(&scratch.0.join("obc-data/src/lib.rs"), "pub fn select() {}\n");
        assert_eq!(code_hash(&scratch.0, &code).unwrap(), before);
        assert_ne!(code::compiled(&scratch.0, &code.crates).unwrap(), compiled, "the worker also binds its engine");
    }

    #[test]
    fn a_persistent_code_change_during_a_rust_or_command_step_leaves_no_output_object_or_receipt() {
        fn edit(request: &Request) -> Result<(), String> {
            fs::write(request.options["code"].as_str().unwrap(), "// changed during execution\n")
                .map_err(|e| e.to_string())?;
            fs::write(request.output.join("result.txt"), b"unaccepted output").map_err(|e| e.to_string())
        }
        for rust in [true, false] {
            let fixture = fixture(if rust { "rust-drift" } else { "command-drift" });
            let code = if rust {
                fixture.root().join("steps/src/lib.rs")
            } else {
                let code = fixture.root().join("edit.py");
                write(&code, "import json, pathlib, sys\nr=json.load(sys.stdin)\npathlib.Path(r['options']['code']).write_text('# changed during execution\\n')\n(pathlib.Path(r['output'])/'result.txt').write_bytes(b'unaccepted output')\n");
                code
            };
            let identity = if rust { steps_crate() } else { Code { paths: vec!["edit.py".into()], ..Code::default() } };
            let run = if rust { Run::Rust(edit) } else { Run::Command(vec!["python3".into(), "edit.py".into()]) };
            let mut step = step("test/drift", Vec::new(), identity, "result.txt", run);
            step.options = json!({"code":code});
            let error = fixture.build(&[step]).err().unwrap();
            assert!(error.contains("code of test/drift changed"), "{error}");
            assert!(!fixture.store.root().join("layers").exists());
            assert!(!fixture.store.object(&sha256_hex(b"unaccepted output")).exists());
        }
    }

    #[test]
    fn a_view_links_each_file_to_its_object() {
        let scratch = Scratch::new("engine-view");
        let object = scratch.0.join("object");
        write(&object, "tile");
        let files = BTreeMap::from([("tiles/a.pbf".to_string(), object.clone())]);
        view(&files, &scratch.0.join("view")).unwrap();
        assert_eq!(fs::read_link(scratch.0.join("view/tiles/a.pbf")).unwrap(), object);
        let outside = BTreeMap::from([("../a.pbf".to_string(), object)]);
        assert_eq!(view(&outside, &scratch.0.join("other")).unwrap_err(), "../a.pbf is not a relative path");
    }

    #[test]
    fn a_code_path_that_git_does_not_list_fails() {
        let scratch = Scratch::new("engine-ignored");
        let code = Code { paths: vec!["ignored.txt".into()], crates: Vec::new(), ..Default::default() };
        write(&scratch.0.join("ignored.txt"), "");
        let err = code_hash(&scratch.0, &code).unwrap_err();
        assert!(err.contains("obc data runs in a git checkout"), "{err}");
        repository(&scratch.0, &[]);
        write(&scratch.0.join(".gitignore"), "ignored.txt\n");
        assert_eq!(code_hash(&scratch.0, &code).unwrap_err(), "ignored.txt is ignored by git");
    }
}
