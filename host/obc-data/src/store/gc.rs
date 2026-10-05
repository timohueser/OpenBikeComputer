//! `obc data gc store`: delete the objects and the snapshot records that no environment, pin or
//! fixture reaches. Receipts and import records stay: they are history.

use std::collections::{BTreeSet, HashMap, HashSet};
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
    /// `(source, version)` from `[pins]` of every `data/env/*.toml`.
    pub pins: BTreeSet<(String, String)>,
    /// Each SHA-256 that a pin, a fixture or a planner region recipe names.
    pub sha256s: BTreeSet<String>,
}

impl Roots {
    /// The pins of every environment file, and the SHA-256 values in the fixture catalog, the
    /// fixture build records and the planner region recipes.
    pub fn from_repo(root: &Path) -> Result<Self, String> {
        let mut roots = Roots::default();
        for path in files(&root.join("data/env"), &["toml"])? {
            let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let table: toml::Table = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            for (source, version) in table.get("pins").and_then(|pins| pins.as_table()).into_iter().flatten() {
                let version =
                    version.as_str().ok_or_else(|| format!("{}: pin {source} is not text", path.display()))?;
                roots.sha256s.extend(sha256s(version));
                roots.pins.insert((source.clone(), version.to_string()));
            }
        }
        let mut pinned = vec![root.join("fixtures/catalog.toml")];
        pinned.extend(files(&root.join("fixtures/sources"), &["json", "toml"])?);
        pinned.extend(files(&root.join("tools/planner-regions"), &["json"])?);
        for path in pinned {
            match fs::read_to_string(&path) {
                Ok(text) => roots.sha256s.extend(sha256s(&text)),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Ok(roots)
    }
}

/// What `gc store` deletes, or deleted, and what stays.
#[derive(Debug, Default, Serialize, JsonSchema)]
#[schemars(rename = "GcPlan")]
pub struct Plan {
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

/// What a collection deletes. A snapshot record is reached when a pin names it, or when it is the
/// newest record of its source or of a request. An object is reached when a pin, a fixture, a planner recipe or an
/// import record names it, or a reached record or layer has it. A layer is reached when each input
/// is: a snapshot input whose digest is of all the files, or of one file, of a reached record of
/// its source, and a layer input whose digest is of a reached layer.
pub fn plan(store: &Store, roots: &Roots) -> Result<Plan, String> {
    let mut reached: HashSet<String> = roots.sha256s.iter().cloned().collect();
    for path in files(&store.root().join("imports"), &["jsonl"])? {
        reached.extend(sha256s(&fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?));
    }
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
        if !roots.pins.contains(&key) && !kept.contains(&key) && retrieved(snapshot) < newest[source.as_str()] {
            plan.snapshots.push(format!("{source}@{version}"));
            continue;
        }
        let files = || snapshot.files.iter().map(|file| (file.name.as_str(), file.sha256.as_str()));
        snapshot_digests.insert((source.clone(), engine::digest(files())));
        snapshot_digests.extend(files().map(|file| (source.clone(), engine::digest([file]))));
        reached.extend(snapshot.files.iter().map(|file| file.sha256.clone()));
    }
    let mut receipts = Vec::new();
    for key in names(&store.root().join("layers"), "json")? {
        receipts.extend(store.layer(&key)?);
    }
    let mut layer_digests = HashSet::new();
    loop {
        let before = layer_digests.len();
        for receipt in &receipts {
            let inputs_reached = receipt.inputs.iter().all(|input| match input.kind {
                InputKind::Snapshot => snapshot_digests.contains(&(input.name.clone(), input.digest.clone())),
                InputKind::Layer => layer_digests.contains(&input.digest),
            });
            if inputs_reached && layer_digests.insert(receipt.digest.clone()) {
                reached.extend(receipt.files.iter().map(|file| file.sha256.clone()));
            }
        }
        if layer_digests.len() == before {
            break;
        }
    }
    for prefix in names(&store.root().join("objects"), "")? {
        for sha256 in names(&store.root().join("objects").join(prefix), "")? {
            let size = fs::metadata(store.object(&sha256)).map_err(|e| format!("object {sha256}: {e}"))?.len();
            if reached.contains(&sha256) {
                plan.keep_objects += 1;
                plan.keep_bytes += size;
            } else {
                plan.remove_bytes += size;
                plan.objects.push((sha256, size));
            }
        }
    }
    plan.objects.sort();
    plan.snapshots.sort();
    Ok(plan)
}

/// Delete what nothing reaches, and say what. `None`, and nothing deleted, while a fetch, a build or
/// an import holds the store.
pub fn apply(store: &Store, roots: &Roots) -> Result<Option<Plan>, String> {
    let Some(_alone) = store.try_alone()? else {
        return Ok(None);
    };
    let plan = plan(store, roots)?;
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
        let sha256 = object(store, bytes);
        let digest = engine::digest([("out", sha256.as_str())]);
        let files = vec![LayerFile { path: "out".into(), size: bytes.len() as u64, sha256 }];
        let receipt = Receipt {
            step: key.into(),
            key: key.into(),
            inputs,
            options: serde_json::json!({}),
            code: String::new(),
            command: None,
            outputs: vec!["out".into()],
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
        InputRecord { kind, name: name.into(), digest }
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
        assert_eq!(
            plan.keep_objects, 11,
            "pinned files, the newest of each source and request, the digest pin, the fixture, an import and three layers"
        );

        let using = store.using().unwrap();
        assert!(apply(&store, &roots).unwrap().is_none(), "a running fetch stops a collection");
        drop(using);
        assert_eq!(apply(&store, &roots).unwrap().unwrap().objects, plan.objects, "it deletes what the plan names");
        assert!(store.snapshot("land", "2026-08-01").unwrap().is_none());
        assert!(store.object(&sha256_hex(b"land b")).is_file(), "a file of the pinned record stays");
        assert!(!store.object(&sha256_hex(b"stale")).exists());
        assert!(store.layer("stale").unwrap().is_some(), "a receipt is history and stays");
        let again = super::plan(&store, &roots).unwrap();
        assert!(again.snapshots.is_empty() && again.objects.is_empty());
    }
}
