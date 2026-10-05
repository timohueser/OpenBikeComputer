//! The engine: it builds each layer from snapshots, other layers and options, and reuses a layer
//! whose key the store has. `specs/obc-data.md` describes steps, keys and receipts.

mod code;
mod process;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::date;
use crate::store::{hash_file, sha256_hex, Store};

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
}

pub enum Input {
    Snapshot {
        source: String,
        version: String,
    },
    /// The layer of another step.
    Layer(String),
}

/// The code that makes a layer. When in doubt, declare more: too much costs a rebuild, too little
/// gives stale data.
#[derive(Default)]
pub struct Code {
    /// Files and directories, relative to the repository root.
    pub paths: Vec<String>,
    /// Workspace crates; each brings its path dependencies.
    pub crates: Vec<String>,
}

pub enum Run {
    /// A function in this process. It gets no network client.
    Rust(fn(&Request) -> Result<(), String>),
    /// A program and its arguments, started offline in the repository root with the request as
    /// JSON on standard input.
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
    pub options: Value,
    /// An empty directory: the layer is the files the step writes in it.
    pub output: PathBuf,
    /// Where the step may write a JSON object of metrics.
    pub metrics: PathBuf,
}

/// The record of one layer: what made it, its files, and what building it cost.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub step: String,
    pub key: String,
    pub inputs: Vec<InputRecord>,
    pub options: Value,
    pub code: String,
    pub command: Option<Vec<String>>,
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputRecord {
    pub kind: InputKind,
    /// The source id or the layer name.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    // In the byte order of the names: a key sorts its inputs by kind.
    Layer,
    Snapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LayerFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

pub struct Built {
    pub receipt: Receipt,
    pub reused: bool,
}

/// Build `steps` in dependency order, and reuse each layer whose key the store has. `root` is the
/// repository root: code paths are relative to it, and a command starts in it.
pub fn build(store: &Store, root: &Path, steps: &[Step]) -> Result<Vec<Built>, String> {
    let mut done: HashMap<&str, Receipt> = HashMap::new();
    let mut built = Vec::new();
    for step in order(steps)? {
        let result = build_step(store, root, step, &done).map_err(|e| format!("step `{}`: {e}", step.name))?;
        done.insert(&step.name, result.receipt.clone());
        built.push(result);
    }
    Ok(built)
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
    });
    sha256_hex(&serde_json::to_vec(&sorted(spec)).expect("JSON values serialize"))
}

/// `value` with the keys of every object in byte order, whatever map serde_json was built with.
fn sorted(value: Value) -> Value {
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
        if let Some(name) = step.layers().find(|name| !names.contains(name)) {
            return Err(format!("step `{}` reads the layer `{name}`, which no step makes", step.name));
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
    fn layers(&self) -> impl Iterator<Item = &str> {
        self.inputs.iter().filter_map(|input| match input {
            Input::Layer(name) => Some(name.as_str()),
            Input::Snapshot { .. } => None,
        })
    }
}

fn build_step(store: &Store, root: &Path, step: &Step, done: &HashMap<&str, Receipt>) -> Result<Built, String> {
    if !step.options.is_object() {
        return Err("the options are not a JSON object".into());
    }
    let mut request = Request {
        step: step.name.clone(),
        snapshots: BTreeMap::new(),
        layers: BTreeMap::new(),
        options: step.options.clone(),
        output: PathBuf::new(),
        metrics: PathBuf::new(),
    };
    let mut inputs = Vec::new();
    let mut bytes_in = 0;
    for input in &step.inputs {
        let (kind, name, version, files): (_, _, _, Vec<(String, String, u64)>) = match input {
            Input::Snapshot { source, version } => {
                let snapshot = store
                    .snapshot(source, version)?
                    .ok_or_else(|| format!("the store has no snapshot {source}@{version}; fetch it first"))?;
                let files = snapshot.files.into_iter().map(|file| (file.name, file.sha256, file.size)).collect();
                (InputKind::Snapshot, source, Some(version.clone()), files)
            }
            Input::Layer(name) => {
                let files = done[name.as_str()].files.iter();
                (
                    InputKind::Layer,
                    name,
                    None,
                    files.map(|file| (file.path.clone(), file.sha256.clone(), file.size)).collect(),
                )
            }
        };
        let digest = digest(files.iter().map(|(name, sha256, _)| (name.as_str(), sha256.as_str())));
        let record = InputRecord { kind, name: name.clone(), version, digest };
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

    let mut receipt = Receipt {
        step: step.name.clone(),
        key: String::new(),
        inputs,
        options: step.options.clone(),
        code: code::hash(root, &step.code)?,
        command: match &step.run {
            Run::Rust(_) => None,
            Run::Command(argv) => Some(argv.clone()),
        },
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

    let _lock = store.lock(&format!("layer-{}", receipt.key))?;
    if let Some(stored) = store.layer(&receipt.key)? {
        if stored.files.iter().all(|file| store.object(&file.sha256).is_file()) {
            return Ok(Built { receipt: stored, reused: true });
        }
    }

    let work = store.partial(&format!("layer-{}", receipt.key));
    request.output = work.join("output");
    request.metrics = work.join("metrics.json");
    remove_dir(&work)?;
    fs::create_dir_all(&request.output).map_err(|e| format!("{}: {e}", request.output.display()))?;
    let usage = match &step.run {
        Run::Rust(function) => process::in_process(|| function(&request)),
        Run::Command(argv) => process::run(root, argv, &request),
    }?;
    receipt.files = collect(store, &request.output, &step.outputs)?;
    receipt.metrics = match fs::read_to_string(&request.metrics) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("the metrics are not a JSON object: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(e) => return Err(format!("{}: {e}", request.metrics.display())),
    };
    remove_dir(&work)?;
    receipt.digest = digest(receipt.files.iter().map(|file| (file.path.as_str(), file.sha256.as_str())));
    receipt.bytes_out = receipt.files.iter().map(|file| file.size).sum();
    receipt.built = date::timestamp(date::now());
    receipt.wall_ms = usage.wall_ms;
    receipt.cpu_ms = usage.cpu_ms;
    receipt.peak_rss_bytes = usage.peak_rss_bytes;
    store.put_layer(&receipt)?;
    Ok(Built { receipt, reused: false })
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
    let covers = |declared: &str, path: &str| {
        path == declared || path.strip_prefix(declared).is_some_and(|rest| rest.starts_with('/'))
    };
    if let Some(missing) = declared.iter().find(|declared| !paths.iter().any(|path| covers(declared, path))) {
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
mod tests {
    use super::*;
    use crate::store::tests::Scratch;
    use crate::store::{write_atomic, FileRecord, Snapshot};
    use serde_json::json;

    const JOIN: &str = "import json, os, sys
request = json.load(sys.stdin)
upper = open(request['layers']['test/upper']['upper.txt']).read()
tail = open(request['snapshots']['tail']['tail.txt']).read()
open(os.path.join(request['output'], 'joined.txt'), 'w').write(upper + tail)
json.dump({'characters': len(upper + tail)}, open(request['metrics'], 'w'))
";

    struct Fixture {
        scratch: Scratch,
        store: Store,
    }

    impl Fixture {
        fn root(&self) -> PathBuf {
            self.scratch.0.join("repository")
        }

        fn build(&self, steps: &[Step]) -> Result<Vec<Built>, String> {
            build(&self.store, &self.root(), steps)
        }
    }

    /// A store with the snapshots `head@1` and `tail@1`, and a repository with `join.py`. `None`
    /// when this machine cannot run a command step offline.
    fn fixture(name: &str) -> Option<Fixture> {
        if !process::offline_allowed() {
            assert!(std::env::var_os("CI").is_none(), "CI must allow the user namespaces a command step runs in");
            eprintln!("skipped: this machine forbids the user namespaces a command step runs in");
            return None;
        }
        let scratch = Scratch::new(name);
        let store = Store::at(scratch.0.join("store"));
        for (source, bytes) in [("head", b"head\n"), ("tail", b"tail\n")] {
            let name = format!("{source}.txt");
            let file = store.partial(&name);
            write_atomic(&file, bytes).unwrap();
            let sha256 = sha256_hex(bytes);
            store.insert(&file, &sha256).unwrap();
            let url = format!("https://example.org/{name}");
            let retrieved = "2026-10-05T00:00:00Z".into();
            let files = vec![FileRecord { name, url, size: bytes.len() as u64, sha256, retrieved }];
            store.put_snapshot(&Snapshot { source: source.into(), version: "1".into(), files }).unwrap();
        }
        let fixture = Fixture { scratch, store };
        fs::create_dir_all(fixture.root()).unwrap();
        fs::write(fixture.root().join("join.py"), JOIN).unwrap();
        Some(fixture)
    }

    fn upper(request: &Request) -> Result<(), String> {
        let text = fs::read_to_string(&request.snapshots["head"]["head.txt"]).map_err(|e| e.to_string())?;
        fs::write(request.output.join("upper.txt"), text.to_uppercase()).map_err(|e| e.to_string())
    }

    fn count(request: &Request) -> Result<(), String> {
        let text = fs::read_to_string(&request.layers["test/join"]["joined.txt"]).map_err(|e| e.to_string())?;
        fs::create_dir(request.output.join("count")).map_err(|e| e.to_string())?;
        fs::write(request.output.join("count/lines.txt"), text.lines().count().to_string()).map_err(|e| e.to_string())
    }

    /// Two fetched sources and three steps, the second a Python command. The last step is listed
    /// first: the engine orders them.
    fn pipeline() -> Vec<Step> {
        let step = |name: &str, inputs, code, outputs: &str, run| Step {
            name: name.into(),
            inputs,
            options: json!({}),
            code,
            outputs: vec![outputs.into()],
            run,
        };
        let snapshot = |source: &str| Input::Snapshot { source: source.into(), version: "1".into() };
        let join = Code { paths: vec!["join.py".into()], crates: Vec::new() };
        let python = Run::Command(vec!["python3".into(), "join.py".into()]);
        vec![
            step("test/count", vec![Input::Layer("test/join".into())], Code::default(), "count", Run::Rust(count)),
            step("test/join", vec![Input::Layer("test/upper".into()), snapshot("tail")], join, "joined.txt", python),
            step("test/upper", vec![snapshot("head")], Code::default(), "upper.txt", Run::Rust(upper)),
        ]
    }

    fn summary(built: &[Built]) -> Vec<(&str, bool)> {
        built.iter().map(|built| (built.receipt.step.as_str(), built.reused)).collect()
    }

    fn keys(built: &[Built]) -> Vec<String> {
        built.iter().map(|built| built.receipt.key.clone()).collect()
    }

    #[test]
    fn a_pipeline_builds_then_reuses_and_records_receipts() {
        let Some(fixture) = fixture("engine-pipeline") else { return };
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

        let second = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&second), [("test/upper", true), ("test/join", true), ("test/count", true)]);
        let receipts = |built: &[Built]| built.iter().map(|built| built.receipt.clone()).collect::<Vec<_>>();
        assert_eq!(receipts(&second), receipts(&first));
    }

    #[test]
    fn a_code_change_rebuilds_its_layer_and_stops_where_the_bytes_are_the_same() {
        let Some(fixture) = fixture("engine-code") else { return };
        let first = keys(&fixture.build(&pipeline()).unwrap());

        fs::write(fixture.root().join("join.py"), format!("# The same bytes.\n{JOIN}")).unwrap();
        let built = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&built), [("test/upper", true), ("test/join", false), ("test/count", true)]);
        let second = keys(&built);
        assert_eq!((first[0] == second[0], first[1] == second[1], first[2] == second[2]), (true, false, true));

        fs::write(fixture.root().join("join.py"), JOIN.replace("upper + tail", "tail + upper")).unwrap();
        let built = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&built), [("test/upper", true), ("test/join", false), ("test/count", false)]);
        let third = keys(&built);
        assert_eq!((second[0] == third[0], second[1] == third[1], second[2] == third[2]), (true, false, false));
    }

    #[test]
    fn a_crate_brings_its_path_dependencies_and_cargo_lock_only_when_declared() {
        let scratch = Scratch::new("engine-crates");
        let write = |path: &str, text: &str| {
            let path = scratch.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write("Cargo.toml", "[workspace]\nmembers = [\"app\", \"lib\", \"check\"]\nresolver = \"2\"\n");
        let app = "[dependencies]\nlib = { path = \"../lib\" }\n[dev-dependencies]\ncheck = { path = \"../check\" }\n";
        for (name, dependencies) in [("app", app), ("lib", ""), ("check", "")] {
            let package = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
            write(&format!("{name}/Cargo.toml"), &(package + dependencies));
            write(&format!("{name}/src/lib.rs"), "");
        }
        let hash = |paths: &[&str]| {
            let code = Code { paths: paths.iter().map(|path| path.to_string()).collect(), crates: vec!["app".into()] };
            code::hash(&scratch.0, &code).unwrap()
        };
        let before = hash(&[]);
        write("check/src/lib.rs", "pub fn helper() {}\n");
        write("Cargo.lock", "version = 4\n");
        assert_eq!(hash(&[]), before, "a dev-dependency and an undeclared Cargo.lock are not code");
        write("lib/src/lib.rs", "pub fn step() {}\n");
        let changed = hash(&[]);
        assert_ne!(changed, before);
        assert_ne!(hash(&["Cargo.lock"]), changed);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_command_step_cannot_open_a_connection() {
        let Some(fixture) = fixture("engine-offline") else { return };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let connect = "import json, socket, sys\n\
                       port = json.load(sys.stdin)['options']['port']\n\
                       socket.create_connection(('127.0.0.1', port), timeout=5)\n";
        fs::write(fixture.root().join("connect.py"), connect).unwrap();
        let step = Step {
            name: "test/online".into(),
            inputs: Vec::new(),
            options: json!({"port": listener.local_addr().unwrap().port()}),
            code: Code { paths: vec!["connect.py".into()], crates: Vec::new() },
            outputs: Vec::new(),
            run: Run::Command(vec!["python3".into(), "connect.py".into()]),
        };
        let error = fixture.build(&[step]).err().expect("the connection fails, and so the step");
        assert!(error.contains("`python3 connect.py` failed"), "{error}");
    }
}
