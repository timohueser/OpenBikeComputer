//! The collection of `obc data clean`: delete the objects and the snapshot records that no live
//! release, saved Local adoption or fixture reaches. Receipts stay: they are history.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::Serialize;

use super::{read_record, read_records, sorted, Snapshot, Store};
use crate::engine::{self, InputKind};
use crate::live::Live;

/// What live and the repository keep.
#[derive(Debug, Default, Clone)]
pub struct Roots {
    /// `(source, version)` that a layer of a live release read, with the products.
    pub live: BTreeMap<(String, String), Vec<String>>,
    /// Exact source versions of saved Local data, including stopped experiments.
    pub local: BTreeMap<(String, String), Vec<String>>,
    /// Each SHA-256 that a live layer, a fixture or a planner region recipe names, with which of
    /// them.
    pub sha256s: BTreeMap<String, &'static str>,
}

impl Roots {
    /// The SHA-256 values in the fixture catalog, the fixture build records and the planner region
    /// recipes.
    pub fn from_repo(root: &Path) -> Result<Self, String> {
        let mut roots = Roots::default();
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

    /// Add the files and the snapshots of each layer of the live releases.
    pub fn add_live(&mut self, live: &Live) {
        for product in &live.products {
            let Some((_, release)) = &product.release else { continue };
            for layer in &release.layers {
                self.name(layer.files.iter().map(|file| file.sha256.clone()), "live release");
                for (source, read) in &layer.snapshots {
                    let products = self.live.entry((source.clone(), read.version.clone())).or_default();
                    if !products.contains(&product.product) {
                        products.push(product.product.clone());
                    }
                }
            }
        }
    }

    fn add_local(&mut self, store: &Store) -> Result<(), String> {
        for adoption in crate::local::saved(store)? {
            self.name(
                adoption.plan.layers.iter().flat_map(|layer| &layer.files).map(|file| file.sha256.clone()),
                "local release",
            );
            for name in adoption.inputs()? {
                let layer =
                    adoption.original.layers.iter().find(|layer| layer.step == name).expect("checked provenance");
                for (source, read) in &layer.snapshots {
                    let products = self.local.entry((source.clone(), read.version.clone())).or_default();
                    if !products.contains(&adoption.plan.product) {
                        products.push(adoption.plan.product.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// Record `why` for each SHA-256 that no other root named first.
    fn name(&mut self, sha256s: impl Iterator<Item = String>, why: &'static str) {
        sha256s.for_each(|sha256| {
            self.sha256s.entry(sha256).or_insert(why);
        });
    }
}

/// What `clean` deletes from the store, or deleted, and what stays.
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
    /// The size of `partial/`: unfinished downloads and the work of steps that stopped. A
    /// collection empties it.
    pub partial_bytes: u64,
}

/// A snapshot record, the layers of one step, or the objects that one kind of root names and no
/// kept record or layer has.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Kept {
    /// `source@version`, a step, or `N objects`.
    pub entry: String,
    /// The size of its files.
    pub bytes: u64,
    /// `live PRODUCT, …`, `newest of the source`, `newest of a request`, `inputs kept`,
    /// `live release`, `fixture` or `planner recipe`.
    pub because: Vec<String>,
}

/// What a collection deletes. A snapshot record is reached when a live layer read it, or when it is
/// the newest record of its source or of a request. An object is reached when a live layer, a
/// fixture or a planner recipe names it, or a reached record or layer has it. A layer is reached when each input
/// is: a snapshot input whose digest is of the files that it names of a reached record of its
/// source, and a layer input whose digest is of the files that it selects (all when it names
/// none) of a reached layer of its step.
pub fn plan(store: &Store, roots: &Roots) -> Result<Plan, String> {
    let mut roots = roots.clone();
    roots.add_local(store)?;
    let named = &roots.sha256s;
    // The objects of the kept records and layers.
    let mut reached = HashSet::new();
    let mut kept_snapshots = HashMap::<&str, Vec<&Snapshot>>::new();
    let mut plan = Plan::default();
    let mut snapshots = Vec::new();
    for (source, version) in records(store)? {
        snapshots.extend(store.snapshot(&source, &version)?);
    }
    // The newest record of a source is what a plan without a version reads, and what a source whose
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
        let requests = store.requests_of(&source)?;
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
        let live = roots.live.get(&key).map(|products| format!("live {}", products.join(", ")));
        let local = roots.local.get(&key).map(|products| format!("local {}", products.join(", ")));
        let newest_of_source = (retrieved(snapshot) >= newest[source.as_str()]).then(|| "newest of the source".into());
        let newest_of_request = kept.contains(&key).then(|| "newest of a request".into());
        let because: Vec<String> = [live, local, newest_of_source, newest_of_request].into_iter().flatten().collect();
        if because.is_empty() {
            plan.snapshots.push(format!("{source}@{version}"));
            continue;
        }
        let bytes = snapshot.files.iter().map(|file| file.size).sum();
        plan.kept.push(Kept { entry: format!("{source}@{version}"), bytes, because });
        kept_snapshots.entry(source).or_default().push(snapshot);
        reached.extend(snapshot.files.iter().map(|file| file.sha256.clone()));
    }
    // A receipt that this build cannot read can be of a newer build: its objects stay.
    let receipts: Vec<engine::Receipt> = read_records(&store.root().join("layers"), read_record)
        .map_err(|e| format!("{e}; a newer obc data may have written it, so nothing is collected"))?;
    let mut keys = HashSet::new();
    // The reached layers of each step.
    let mut layers = HashMap::<&str, Vec<&engine::Receipt>>::new();
    let mut steps = BTreeMap::<&str, u64>::new();
    loop {
        let before = keys.len();
        for receipt in &receipts {
            let inputs_reached = receipt.inputs.iter().all(|input| match input.kind {
                InputKind::Snapshot => kept_snapshots.get(input.name.as_str()).is_some_and(|snapshots| {
                    let names: HashSet<&str> = input.files.iter().map(String::as_str).collect();
                    snapshots.iter().any(|snapshot| {
                        let files = snapshot.files.iter().filter(|file| names.contains(file.name.as_str()));
                        engine::digest(files.map(|file| (file.name.as_str(), file.sha256.as_str()))) == input.digest
                    })
                }),
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
    plan.partial_bytes = size(&store.root().join("partial"))?;
    Ok(plan)
}

/// The size of the files below `path`, without following links.
fn size(path: &Path) -> Result<u64, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }
    let entries = fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))?;
    entries.map(|entry| size(&entry.map_err(|e| e.to_string())?.path())).sum()
}

/// Delete what nothing reaches, and say what. `None`, and nothing deleted, while a fetch or a build
/// holds the store, including the verification and publication phases of a run.
/// A plan that removes anything but `confirmed` deletes nothing.
pub fn apply(store: &Store, roots: &Roots, confirmed: &Plan) -> Result<Option<Plan>, String> {
    let Some(_alone) = store.try_alone()? else {
        return Ok(None);
    };
    let plan = plan(store, roots)?;
    if (&confirmed.snapshots, &confirmed.objects, confirmed.partial_bytes)
        != (&plan.snapshots, &plan.objects, plan.partial_bytes)
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
    // No fetch or step writes there while the collection holds the store alone.
    for name in names(&store.root().join("partial"), "")? {
        let path = store.partial(&name);
        match fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.is_dir() {
            true => fs::remove_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?,
            false => remove(&path)?,
        }
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

    fn input(kind: InputKind, name: &str, digest: String, files: &[&str]) -> InputRecord {
        InputRecord { kind, name: name.into(), digest, files: files.iter().map(|file| file.to_string()).collect() }
    }

    #[test]
    fn a_collection_keeps_the_newest_records_and_what_a_fixture_reaches() {
        let scratch = Scratch::new("gc");
        let (repo, store) = (scratch.0.join("repo"), Store::at(scratch.0.join("store")));
        let fixture = sha256_hex(b"fixture");
        fs::create_dir_all(repo.join("fixtures")).unwrap();
        fs::write(repo.join("fixtures/catalog.toml"), format!("[packages.a]\nsha256 = \"{fixture}\"\n")).unwrap();
        let roots = Roots::from_repo(&repo).unwrap();

        let new = [("a.zip", &b"land new"[..]), ("b.zip", b"land b"), ("c.zip", b"land c")];
        snapshot(&store, "land", "2026-09-01", "2026-09-01", &new);
        snapshot(&store, "land", "2026-08-01", "2026-08-01", &[("a.zip", b"land old"), ("b.zip", b"land b")]);
        snapshot(&store, "osm", "release/1", "2026-10-05", &[("planet.pbf", b"planet")]);
        snapshot(&store, "extract", "2026-08-01", "2026-08-01", &[("a.pbf", b"extract old")]);
        snapshot(&store, "extract", "2026-09-30", "2026-09-30", &[("a.pbf", b"extract new")]);
        // Area `b` was last fetched on an older day than area `a`.
        snapshot(&store, "extract", "2026-09-20", "2026-09-20", &[("b.pbf", b"extract b")]);
        for (version, area) in [("2026-09-30", "a"), ("2026-09-20", "b"), ("2026-08-01", "a")] {
            let params = vec![("area".to_string(), area.to_string())];
            let files = vec![format!("{area}.pbf")];
            store.put_requested("extract", &Requested { version: version.into(), params, files }).unwrap();
        }
        object(&store, b"fixture");
        let land = store.snapshot("land", "2026-09-01").unwrap().unwrap();
        let read = |names: &[&str]| {
            let files = land.files.iter().filter(|file| names.contains(&file.name.as_str()));
            input(
                InputKind::Snapshot,
                "land",
                engine::digest(files.map(|f| (f.name.as_str(), f.sha256.as_str()))),
                names,
            )
        };
        let cells = layer(&store, "cells", vec![read(&["a.zip", "b.zip", "c.zip"])], b"cells");
        layer(&store, "one", vec![read(&["a.zip"])], b"one");
        layer(&store, "two", vec![read(&["a.zip", "c.zip"])], b"two");
        layer(&store, "joined", vec![input(InputKind::Layer, "cells", cells, &[])], b"joined");
        let old = engine::digest([("a.zip", sha256_hex(b"land old").as_str())]);
        let stale = layer(&store, "stale", vec![input(InputKind::Snapshot, "land", old, &["a.zip"])], b"stale");
        layer(&store, "on-stale", vec![input(InputKind::Layer, "stale", stale, &[])], b"on stale");

        let plan = plan(&store, &roots).unwrap();
        assert_eq!(plan.snapshots, ["extract@2026-08-01", "land@2026-08-01"]);
        let mut removed: Vec<_> = [&b"land old"[..], b"extract old", b"stale", b"on stale"]
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
                "land@2026-09-01: newest of the source",
                "osm@release/1: newest of the source",
                "cells: inputs kept",
                "joined: inputs kept",
                "one: inputs kept",
                "two: inputs kept",
                "1 object: fixture",
            ]
        );
        assert_eq!(plan.keep_objects, 11, "the newest of each source and request, the fixture and four layers");

        let using = store.using().unwrap();
        assert!(apply(&store, &roots, &plan).unwrap().is_none(), "a running fetch stops a collection");
        drop(using);
        let older = Plan { objects: plan.objects[1..].to_vec(), ..plan.clone() };
        assert!(apply(&store, &roots, &older).is_err(), "a plan that is not the plan of now deletes nothing");
        assert!(store.snapshot("land", "2026-08-01").unwrap().is_some());
        let applied = apply(&store, &roots, &plan).unwrap().unwrap();
        assert_eq!(applied.objects, plan.objects, "it deletes what the plan names");
        assert!(store.snapshot("land", "2026-08-01").unwrap().is_none());
        assert!(store.object(&sha256_hex(b"land b")).is_file(), "a file of the newest record stays");
        assert!(!store.object(&sha256_hex(b"stale")).exists());
        assert!(store.layer("stale").unwrap().is_some(), "a receipt is history and stays");
        let again = super::plan(&store, &roots).unwrap();
        assert!(again.snapshots.is_empty() && again.objects.is_empty());
        write_atomic(&store.root().join("layers/newer.json"), b"{\"format\": 2}").unwrap();
        assert!(super::plan(&store, &roots).unwrap_err().contains("nothing is collected"));
    }

    #[test]
    fn a_layer_input_that_selects_files_reaches_the_layer_that_has_them() {
        let scratch = Scratch::new("gc-select");
        let (repo, store) = (scratch.0.join("repo"), Store::at(scratch.0.join("store")));
        fs::create_dir_all(&repo).unwrap();
        snapshot(&store, "osm", "1", "2026-10-05", &[("planet.pbf", b"planet")]);
        let planet = engine::digest([("planet.pbf", sha256_hex(b"planet").as_str())]);
        let leaves = [("osm/a.pbf", &b"leaf a"[..]), ("osm/b.pbf", b"leaf b")];
        layer_of(&store, "leaves", vec![input(InputKind::Snapshot, "osm", planet, &["planet.pbf"])], &leaves);
        let a = engine::digest([("osm/a.pbf", sha256_hex(b"leaf a").as_str())]);
        layer(&store, "cells", vec![input(InputKind::Layer, "leaves", a, &["osm/a.pbf"])], b"cells a");

        let plan = plan(&store, &Roots::from_repo(&repo).unwrap()).unwrap();
        assert!(plan.objects.is_empty(), "the layer that reads one leaf is reached: {:?}", plan.objects);
    }

    #[test]
    fn a_collection_keeps_the_layers_and_the_snapshots_of_the_live_releases() {
        use crate::live::{tests::release, Live, LiveProduct};
        let scratch = Scratch::new("gc-live");
        let store = Store::at(scratch.0.join("store"));
        snapshot(&store, "land", "2026-10-01", "2026-10-01", &[("land.zip", b"land live")]);
        snapshot(&store, "land", "2026-10-02", "2026-10-02", &[("land.zip", b"land new")]);
        // A layer file whose receipt the store does not have.
        object(&store, b"layer");
        let release = Some((String::new(), release(b"layer")));
        let product = LiveProduct {
            product: "test".into(),
            prefix: "test-catalog".into(),
            release,
            applied: None,
            observed: None,
            document: None,
        };
        let mut roots = Roots::default();
        roots.add_live(&Live { products: vec![product], ..Live::default() });

        let plan = plan(&store, &roots).unwrap();
        assert!(plan.snapshots.is_empty() && plan.objects.is_empty(), "{plan:?}");
        let kept: Vec<String> = plan.kept.iter().map(|k| format!("{}: {}", k.entry, k.because.join(" · "))).collect();
        assert_eq!(
            kept,
            ["land@2026-10-01: live test", "land@2026-10-02: newest of the source", "1 object: live release"]
        );
    }
}
