//! `obc data gc store`: delete the objects and the snapshot records that no environment, pin or
//! fixture reaches. Receipts and import records stay: they are history.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use super::{read_records, sorted, Requested, Snapshot, Store};
use crate::engine::{self, InputKind};

/// What the repository pins.
#[derive(Debug, Default)]
pub struct Roots {
    /// `(source, version)` from `[pins]` of every `data/env/*.toml`, with the environments.
    pub pins: BTreeMap<(String, String), Vec<String>>,
    /// Each SHA-256 that a pin, a fixture or a planner region recipe names, with which of them.
    pub sha256s: BTreeMap<String, &'static str>,
}

impl Roots {
    /// The pins of every environment file, and the SHA-256 values in the fixture catalog, the
    /// fixture build records and the planner region recipes.
    pub fn from_repo(root: &Path) -> Result<Self, String> {
        let mut roots = Roots::default();
        for path in files(&root.join("data/env"), &["toml"])? {
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let table: toml::Table = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let env = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            for (source, version) in table.get("pins").and_then(|pins| pins.as_table()).into_iter().flatten() {
                let version =
                    version.as_str().ok_or_else(|| format!("{}: pin {source} is not text", path.display()))?;
                roots.name(sha256s(version), "pin");
                roots.pins.entry((source.clone(), version.to_string())).or_default().push(env.clone());
            }
        }
        let fixtures = files(&root.join("fixtures/sources"), &["json", "toml"])?;
        let recipes = files(&root.join("tools/planner-regions"), &["json"])?;
        let mut named = vec![(root.join("fixtures/catalog.toml"), "fixture")];
        named.extend(fixtures.into_iter().map(|path| (path, "fixture")));
        named.extend(recipes.into_iter().map(|path| (path, "planner recipe")));
        for (path, why) in named {
            match fs::read_to_string(&path) {
                Ok(text) => roots.name(sha256s(&text), why),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Ok(roots)
    }

    /// Record `why` for each SHA-256 that no other root named first.
    fn name(&mut self, sha256s: impl Iterator<Item = String>, why: &'static str) {
        sha256s.for_each(|sha256| {
            self.sha256s.entry(sha256).or_insert(why);
        });
    }
}

/// What `gc store` deletes, or deleted, and what stays.
#[derive(Debug, Default, Clone, Serialize, JsonSchema)]
#[schemars(rename = "GcPlan")]
pub struct Plan {
    /// What stays, and why.
    pub kept: Vec<Kept>,
    /// `source@version` of each snapshot record that nothing reaches.
    pub snapshots: Vec<String>,
    /// SHA-256 and size of each object that nothing reaches.
    pub objects: Vec<(String, u64)>,
    /// The size of `objects`.
    pub remove_bytes: u64,
    /// The objects that stay, and their size.
    pub keep_objects: u64,
    pub keep_bytes: u64,
}

/// A snapshot record, the layers of one step, or the objects that one kind of root names and no
/// kept record or layer has.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Kept {
    /// `source@version`, a step, or `N objects`.
    pub entry: String,
    /// The size of its files.
    pub bytes: u64,
    /// `pin of ENV, …`, `newest of the source`, `newest of a request`, `inputs kept`, `pin`,
    /// `fixture`, `planner recipe` or `import record`.
    pub because: Vec<String>,
}

/// What a collection deletes. A snapshot record is reached when a pin names it, or when it is the
/// newest record of its source or of a request. An object is reached when a pin, a fixture, a planner recipe or an
/// import record names it, or a reached record or layer has it. A layer is reached when each input
/// is: a snapshot input whose digest is of all the files, or of one file, of a reached record of
/// its source, and a layer input whose digest is of the files that it selects (all when it names
/// none) of a reached layer of its step.
pub fn plan(store: &Store, roots: &Roots) -> Result<Plan, String> {
    let mut named = roots.sha256s.clone();
    for path in files(&store.root().join("imports"), &["jsonl"])? {
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        sha256s(&text).for_each(|sha256| {
            named.entry(sha256).or_insert("import record");
        });
    }
    // The objects of the kept records and layers.
    let mut reached = HashSet::new();
    let mut snapshot_digests = HashSet::new();
    let mut plan = Plan::default();
    let mut snapshots = Vec::new();
    for (source, version) in records(store)? {
        snapshots.extend(store.snapshot(&source, &version)?);
    }
    // The newest record of a source is what a bake without a pin reads, and what a source whose
    // upstream serves only its newest file cannot give again.
    let retrieved = |snapshot: &Snapshot| snapshot.files.iter().map(|file| file.retrieved.clone()).max();
    let mut newest: HashMap<&str, Option<String>> = HashMap::new();
    for snapshot in &snapshots {
        let entry = newest.entry(&snapshot.source).or_default();
        *entry = (*entry).clone().max(retrieved(snapshot));
    }
    // The same for each request: a fetch of one Geofabrik area on an older day is the newest
    // file of that area.
    let mut kept: HashSet<(String, String)> = HashSet::new();
    for source in names(&store.root().join("requests"), "")? {
        let requests: Vec<Requested> = read_records(&store.root().join("requests").join(&source))?;
        // Params, then the latest retrieval and its version.
        let mut newest_of = HashMap::<_, (Option<String>, String)>::new();
        for request in requests {
            let Some(snapshot) = snapshots.iter().find(|s| s.source == source && s.version == request.version) else {
                continue;
            };
            let files = snapshot.files.iter().filter(|file| request.files.contains(&file.name));
            let day = files.map(|file| file.retrieved.clone()).max();
            let entry = newest_of.entry(sorted(&request.params)).or_insert((None, String::new()));
            if day > entry.0 {
                *entry = (day, request.version);
            }
        }
        kept.extend(newest_of.into_values().map(|(_, version)| (source.clone(), version)));
    }
    for snapshot in &snapshots {
        let (source, version) = (&snapshot.source, &snapshot.version);
        let key = (source.clone(), version.clone());
        let pinned = roots.pins.get(&key).map(|envs| format!("pin of {}", envs.join(", ")));
        let newest_of_source = (retrieved(snapshot) >= newest[source.as_str()]).then(|| "newest of the source".into());
        let newest_of_request = kept.contains(&key).then(|| "newest of a request".into());
        let because: Vec<String> = [pinned, newest_of_source, newest_of_request].into_iter().flatten().collect();
        if because.is_empty() {
            plan.snapshots.push(format!("{source}@{version}"));
            continue;
        }
        let bytes = snapshot.files.iter().map(|file| file.size).sum();
        plan.kept.push(Kept { entry: format!("{source}@{version}"), bytes, because });
        let files = || snapshot.files.iter().map(|file| (file.name.as_str(), file.sha256.as_str()));
        snapshot_digests.insert((source.clone(), engine::digest(files())));
        snapshot_digests.extend(files().map(|file| (source.clone(), engine::digest([file]))));
        reached.extend(snapshot.files.iter().map(|file| file.sha256.clone()));
    }
    let mut receipts = Vec::new();
    for key in names(&store.root().join("layers"), "json")? {
        receipts.extend(store.layer(&key)?);
    }
    let mut keys = HashSet::new();
    // The reached layers of each step.
    let mut layers = HashMap::<&str, Vec<&engine::Receipt>>::new();
    let mut steps = BTreeMap::<&str, u64>::new();
    loop {
        let before = keys.len();
        for receipt in &receipts {
            let inputs_reached = receipt.inputs.iter().all(|input| match input.kind {
                InputKind::Snapshot => snapshot_digests.contains(&(input.name.clone(), input.digest.clone())),
                InputKind::Layer => layers.get(input.name.as_str()).is_some_and(|layers| {
                    layers.iter().any(|layer| engine::layer_digest(&layer.files, &input.files) == input.digest)
                }),
            });
            if inputs_reached && keys.insert(receipt.key.as_str()) {
                layers.entry(&receipt.step).or_default().push(receipt);
                reached.extend(receipt.files.iter().map(|file| file.sha256.clone()));
                *steps.entry(&receipt.step).or_default() += receipt.files.iter().map(|file| file.size).sum::<u64>();
            }
        }
        if keys.len() == before {
            break;
        }
    }
    plan.kept.sort_by(|a, b| a.entry.cmp(&b.entry));
    let inputs_kept =
        |(step, bytes): (&str, u64)| Kept { entry: step.into(), bytes, because: vec!["inputs kept".into()] };
    plan.kept.extend(steps.into_iter().map(inputs_kept));
    // The number and size of the objects that only a root of each kind keeps.
    let mut only_named = BTreeMap::<&str, (u64, u64)>::new();
    for prefix in names(&store.root().join("objects"), "")? {
        for sha256 in names(&store.root().join("objects").join(prefix), "")? {
            let size = fs::metadata(store.object(&sha256)).map_err(|e| format!("object {sha256}: {e}"))?.len();
            let root = named.get(sha256.as_str()).filter(|_| !reached.contains(&sha256));
            if let Some(why) = root {
                let (objects, bytes) = only_named.entry(why).or_default();
                (*objects, *bytes) = (*objects + 1, *bytes + size);
            }
            if reached.contains(&sha256) || root.is_some() {
                plan.keep_objects += 1;
                plan.keep_bytes += size;
            } else {
                plan.remove_bytes += size;
                plan.objects.push((sha256, size));
            }
        }
    }
    for (why, (objects, bytes)) in only_named {
        plan.kept.push(Kept { entry: objects_text(objects), bytes, because: vec![why.into()] });
    }
    plan.objects.sort();
    plan.snapshots.sort();
    Ok(plan)
}

/// Delete what nothing reaches, and say what. `None`, and nothing deleted, while a fetch, a build or
/// an import holds the store. With `confirmed`, a plan that removes anything else deletes nothing.
pub fn apply(store: &Store, roots: &Roots, confirmed: Option<&Plan>) -> Result<Option<Plan>, String> {
    let Some(_alone) = store.try_alone()? else {
        return Ok(None);
    };
    let plan = plan(store, roots)?;
    if confirmed.is_some_and(|confirmed| (&confirmed.snapshots, &confirmed.objects) != (&plan.snapshots, &plan.objects))
    {
        return Err("the store changed after the plan; nothing was deleted".into());
    }
    for snapshot in &plan.snapshots {
        let (source, version) = snapshot.split_once('@').expect("source@version");
        let _lock = store.lock(&format!("snapshot-{source}@{version}"))?;
        remove(&store.snapshot_path(source, version))?;
    }
    for (sha256, _) in &plan.objects {
        remove(&store.object(sha256))?;
    }
    Ok(Some(plan))
}

/// `1 object`, `2 objects`.
pub fn objects_text(objects: u64) -> String {
    format!("{objects} object{}", if objects == 1 { "" } else { "s" })
}

fn remove(path: &Path) -> Result<(), String> {
    // Windows refuses to delete a read-only file.
    if let Ok(metadata) = fs::metadata(path) {
        let mut permissions = metadata.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() != ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
        _ => Ok(()),
    }
}

/// `(source, version)` of every snapshot record. A version may have several segments.
fn records(store: &Store) -> Result<Vec<(String, String)>, String> {
    let mut records = Vec::new();
    for source in names(&store.root().join("snapshots"), "")? {
        let dir = store.root().join("snapshots").join(&source);
        for path in files(&dir, &["json"])? {
            let relative = path.strip_prefix(&dir).expect("below dir").with_extension("");
            let version: Vec<_> = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect();
            records.push((source.clone(), version.join("/")));
        }
    }
    Ok(records)
}

/// The names in `dir` without the extension `extension` (all names when it is empty), or none
/// when `dir` does not exist. Hidden names, such as a temporary record, are skipped.
fn names(dir: &Path, extension: &str) -> Result<Vec<String>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = entry.map_err(|e| format!("{}: {e}", dir.display()))?.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        match extension {
            "" => names.push(name),
            extension => names.extend(name.strip_suffix(&format!(".{extension}")).map(str::to_string)),
        }
    }
    Ok(names)
}

/// The files below `dir` with one of `extensions`, or none when `dir` does not exist. Hidden
/// names are skipped.
fn files(dir: &Path, extensions: &[&str]) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = match fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{}: {e}", current.display())),
        };
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", current.display()))?;
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| extensions.iter().any(|e| ext == *e)) {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Every run of exactly 64 lowercase hex digits in `text`.
fn sha256s(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_ascii_hexdigit() || c.is_ascii_uppercase())
        .filter(|word| word.len() == 64)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{InputRecord, LayerFile, Receipt};
    use crate::store::tests::Scratch;
    use crate::store::{sha256_hex, write_atomic, FileRecord, Requested, Snapshot};

    fn object(store: &Store, bytes: &[u8]) -> String {
        let sha256 = sha256_hex(bytes);
        let part = store.partial("object");
        write_atomic(&part, bytes).unwrap();
        store.insert(&part, &sha256).unwrap();
        sha256
    }

    /// A record of `files`, retrieved on `day`.
    fn snapshot(store: &Store, source: &str, version: &str, day: &str, files: &[(&str, &[u8])]) {
        let files = files
            .iter()
            .map(|(name, bytes)| FileRecord {
                name: name.to_string(),
                url: format!("https://example.org/{name}"),
                size: bytes.len() as u64,
                sha256: object(store, bytes),
                retrieved: format!("{day}T00:00:00Z"),
            })
            .collect();
        store.put_snapshot(&Snapshot { source: source.into(), version: version.into(), files }).unwrap();
    }

    /// A layer with one file, `bytes`, that reads `inputs`; its digest.
    fn layer(store: &Store, key: &str, inputs: Vec<InputRecord>, bytes: &[u8]) -> String {
        layer_of(store, key, inputs, &[("out", bytes)])
    }

    /// A layer of `files`, `(path, bytes)`, that reads `inputs`; its digest.
    fn layer_of(store: &Store, key: &str, inputs: Vec<InputRecord>, files: &[(&str, &[u8])]) -> String {
        let files: Vec<LayerFile> = files
            .iter()
            .map(|(path, bytes)| LayerFile {
                path: path.to_string(),
                size: bytes.len() as u64,
                sha256: object(store, bytes),
            })
            .collect();
        let digest = engine::digest(files.iter().map(|file| (file.path.as_str(), file.sha256.as_str())));
        let receipt = Receipt {
            step: key.into(),
            key: key.into(),
            inputs,
            options: serde_json::json!({}),
            code: String::new(),
            command: None,
            outputs: files.iter().map(|file| file.path.clone()).collect(),
            digest: digest.clone(),
            files,
            built: "2026-10-05T00:00:00Z".into(),
            wall_ms: 0,
            cpu_ms: None,
            peak_rss_bytes: None,
            bytes_in: 0,
            bytes_out: 0,
            metrics: Default::default(),
        };
        store.put_layer(&receipt).unwrap();
        digest
    }

    fn input(kind: InputKind, name: &str, digest: String) -> InputRecord {
        InputRecord { kind, name: name.into(), digest, files: Vec::new() }
    }

    #[test]
    fn a_collection_keeps_what_a_pin_or_a_fixture_reaches() {
        let scratch = Scratch::new("gc");
        let (repo, store) = (scratch.0.join("repo"), Store::at(scratch.0.join("store")));
        let pinned_digest = sha256_hex(b"digest pin");
        let fixture = sha256_hex(b"fixture");
        let write = |path: &str, text: String| {
            fs::create_dir_all(repo.join(path).parent().unwrap()).unwrap();
            fs::write(repo.join(path), text).unwrap();
        };
        write("data/env/live.toml", format!("[pins]\nland = \"2026-09-01\"\nnatural = \"{pinned_digest}\"\n"));
        write("data/env/local.toml", "[pins]\nosm = \"release/1\"\n".into());
        write("fixtures/catalog.toml", format!("[packages.a]\nsha256 = \"{fixture}\"\n"));
        let roots = Roots::from_repo(&repo).unwrap();

        // The pinned record of `land` is older than the other one.
        snapshot(&store, "land", "2026-09-01", "2026-09-01", &[("a.zip", b"land new"), ("b.zip", b"land b")]);
        snapshot(&store, "land", "2026-08-01", "2026-08-01", &[("a.zip", b"land old"), ("b.zip", b"land b")]);
        snapshot(&store, "osm", "release/1", "2026-10-05", &[("planet.pbf", b"planet")]);
        // No pin names `extract`: its newest record stays.
        snapshot(&store, "extract", "2026-08-01", "2026-08-01", &[("a.pbf", b"extract old")]);
        snapshot(&store, "extract", "2026-09-30", "2026-09-30", &[("a.pbf", b"extract new")]);
        // Area `b` was last fetched on an older day than area `a`.
        snapshot(&store, "extract", "2026-09-20", "2026-09-20", &[("b.pbf", b"extract b")]);
        for (version, area) in [("2026-09-30", "a"), ("2026-09-20", "b"), ("2026-08-01", "a")] {
            let params = vec![("area".to_string(), area.to_string())];
            let files = vec![format!("{area}.pbf")];
            store.put_requested("extract", &Requested { version: version.into(), params, files }).unwrap();
        }
        object(&store, b"digest pin");
        object(&store, b"fixture");
        object(&store, b"imported, unused");
        object(&store, b"imported, kept");
        let line = format!(
            "{{\"dir\":\"/old\",\"path\":\"a\",\"size\":14,\"sha256\":\"{}\"}}\n",
            sha256_hex(b"imported, kept")
        );
        write_atomic(&store.root().join("imports/20261005T000000Z.jsonl"), line.as_bytes()).unwrap();
        let land = store.snapshot("land", "2026-09-01").unwrap().unwrap();
        let whole = engine::digest(land.files.iter().map(|f| (f.name.as_str(), f.sha256.as_str())));
        let one = engine::digest([("a.zip", land.files[0].sha256.as_str())]);
        let old = engine::digest([("a.zip", sha256_hex(b"land old").as_str())]);
        let cells = layer(&store, "cells", vec![input(InputKind::Snapshot, "land", whole)], b"cells");
        layer(&store, "one", vec![input(InputKind::Snapshot, "land", one)], b"one");
        layer(&store, "joined", vec![input(InputKind::Layer, "cells", cells)], b"joined");
        let stale = layer(&store, "stale", vec![input(InputKind::Snapshot, "land", old)], b"stale");
        layer(&store, "on-stale", vec![input(InputKind::Layer, "stale", stale)], b"on stale");

        let plan = plan(&store, &roots).unwrap();
        assert_eq!(plan.snapshots, ["extract@2026-08-01", "land@2026-08-01"]);
        let mut removed: Vec<_> = [&b"land old"[..], b"extract old", b"imported, unused", b"stale", b"on stale"]
            .iter()
            .map(|bytes| (sha256_hex(bytes), bytes.len() as u64))
            .collect();
        removed.sort();
        assert_eq!(plan.objects, removed);
        let kept: Vec<String> = plan.kept.iter().map(|k| format!("{}: {}", k.entry, k.because.join(" · "))).collect();
        assert_eq!(
            kept,
            [
                "extract@2026-09-20: newest of a request",
                "extract@2026-09-30: newest of the source · newest of a request",
                "land@2026-09-01: pin of live · newest of the source",
                "osm@release/1: pin of local · newest of the source",
                "cells: inputs kept",
                "joined: inputs kept",
                "one: inputs kept",
                "1 object: fixture",
                "1 object: import record",
                "1 object: pin",
            ]
        );
        assert_eq!(
            plan.keep_objects, 11,
            "pinned files, the newest of each source and request, the digest pin, the fixture, an import and three layers"
        );

        let using = store.using().unwrap();
        assert!(apply(&store, &roots, None).unwrap().is_none(), "a running fetch stops a collection");
        drop(using);
        let older = Plan { objects: plan.objects[1..].to_vec(), ..plan.clone() };
        assert!(apply(&store, &roots, Some(&older)).is_err(), "a plan that is not the plan of now deletes nothing");
        assert!(store.snapshot("land", "2026-08-01").unwrap().is_some());
        let applied = apply(&store, &roots, Some(&plan)).unwrap().unwrap();
        assert_eq!(applied.objects, plan.objects, "it deletes what the plan names");
        assert!(store.snapshot("land", "2026-08-01").unwrap().is_none());
        assert!(store.object(&sha256_hex(b"land b")).is_file(), "a file of the pinned record stays");
        assert!(!store.object(&sha256_hex(b"stale")).exists());
        assert!(store.layer("stale").unwrap().is_some(), "a receipt is history and stays");
        let again = super::plan(&store, &roots).unwrap();
        assert!(again.snapshots.is_empty() && again.objects.is_empty());
    }

    #[test]
    fn a_layer_input_that_selects_files_reaches_the_layer_that_has_them() {
        let scratch = Scratch::new("gc-select");
        let (repo, store) = (scratch.0.join("repo"), Store::at(scratch.0.join("store")));
        fs::create_dir_all(&repo).unwrap();
        snapshot(&store, "osm", "1", "2026-10-05", &[("planet.pbf", b"planet")]);
        let planet = engine::digest([("planet.pbf", sha256_hex(b"planet").as_str())]);
        let leaves = [("osm/a.pbf", &b"leaf a"[..]), ("osm/b.pbf", b"leaf b")];
        layer_of(&store, "leaves", vec![input(InputKind::Snapshot, "osm", planet)], &leaves);
        let a = engine::digest([("osm/a.pbf", sha256_hex(b"leaf a").as_str())]);
        let mut selects = input(InputKind::Layer, "leaves", a);
        selects.files = vec!["osm/a.pbf".into()];
        layer(&store, "cells", vec![selects], b"cells a");

        let plan = plan(&store, &Roots::from_repo(&repo).unwrap()).unwrap();
        assert!(plan.objects.is_empty(), "the layer that reads one leaf is reached: {:?}", plan.objects);
    }
}
