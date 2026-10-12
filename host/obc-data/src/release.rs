//! A release: what a target serves, in one manifest. It names the settings of the target, every
//! output with its files, the client pointers and the files that a client finds by name. Its id is
//! the SHA-256 of its bytes. `release.json` of the target names the release that it serves and one
//! rollback: a switch writes it first, so it is the commit point.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{digest, sorted, Client, Input, InputRecord, LayerFile, Receipt, Step};
use crate::settings::Settings;
use crate::store::{hash_file, sha256_hex, FileRecord, Requested, Store};
use crate::target::{Condition, Tag, Target};

/// The key that names the release that a target serves, and its rollback.
pub const SERVING: &str = "release.json";

/// The keys that clients read first, sorted. Every release names both. Clients that an older
/// publisher served read them too, so nothing deletes them.
pub const POINTERS: [&str; 2] = ["cell-catalog/catalog.json", "planner/catalog.json"];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub settings: Settings,
    /// Sorted by `step`, each step once.
    pub outputs: Vec<Output>,
    /// What a client reads at each key of [`POINTERS`]: a client file of an output. Sorted by key.
    pub pointers: Vec<File>,
    /// Client files of outputs that a client finds by name, at `releases/<folder>/<path>`. The
    /// folder is the digest of its files, so other files make another folder. Sorted by key.
    pub named: Vec<File>,
    /// The files of each fetch that an output read, sorted by source, params and version.
    pub sources: Vec<Fetched>,
    /// The machine that made the release.
    pub machine: String,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub time: String,
}

/// A key of a target and its bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub key: String,
    pub size: u64,
    pub sha256: String,
}

/// The files of one fetch that outputs of a release read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fetched {
    pub source: String,
    pub version: String,
    /// The sorted `NAME=VALUE`s of the fetch.
    pub params: Vec<(String, String)>,
    /// Sorted by name.
    pub files: Vec<FetchedFile>,
    /// Whether the target holds a copy of each file at `sources/<source>/<version>/<name>`: when
    /// the licence lets us give the bytes to others.
    pub copy: bool,
    /// The licence and the credit of the source in `data/sources.toml`, for a copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FetchedFile {
    pub name: String,
    /// Where upstream gives it.
    pub url: String,
    pub size: u64,
    pub sha256: String,
    /// When a machine retrieved it first, `YYYY-MM-DDTHH:MM:SSZ`.
    pub retrieved: String,
}

impl FetchedFile {
    /// Whether `other` has the same name and bytes. Where and when a machine retrieved it do not
    /// count.
    pub fn same(&self, other: &FetchedFile) -> bool {
        (&self.name, self.size, &self.sha256) == (&other.name, other.size, &other.sha256)
    }
}

impl Fetched {
    /// The key of the copy of `file` on a target.
    pub fn key(&self, file: &FetchedFile) -> String {
        copy_key(&self.source, &self.version, &file.name)
    }

    /// The files that `names` names, or all of them when it names none.
    pub fn named<'a>(&'a self, names: &'a [String]) -> impl Iterator<Item = &'a FetchedFile> {
        self.files.iter().filter(move |file| names.is_empty() || names.contains(&file.name))
    }

    /// Give `store` the files of the fetch that `names` names, or all of them when it names none,
    /// from their copies on `target`, with their records. A record keeps the time when a machine
    /// retrieved the file first, so an old version never becomes the newest of its source.
    pub fn restore(&self, target: &Target, store: &Store, names: &[String]) -> Result<(), String> {
        for file in self.named(names).filter(|file| !store.object(&file.sha256).is_file()) {
            let downloaded = target.download(&self.key(file), &file.sha256, store)?;
            downloaded.ok_or_else(|| format!("the target has no {}", self.key(file)))?;
        }
        let records: Vec<FileRecord> = self
            .named(names)
            .map(|file| FileRecord {
                name: file.name.clone(),
                url: file.url.clone(),
                size: file.size,
                sha256: file.sha256.clone(),
                retrieved: file.retrieved.clone(),
            })
            .collect();
        crate::fetch::record(store, &self.source, &self.version, &records)?;
        if !self.params.is_empty() {
            let files = self.files.iter().map(|file| file.name.clone()).collect();
            store.put_requested(
                &self.source,
                &Requested { version: self.version.clone(), params: self.params.clone(), files },
            )?;
        }
        Ok(())
    }
}

/// The files of one step: its receipt, what it read of each source, and what a client reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Output {
    pub layer: String,
    pub step: String,
    pub format: u32,
    pub revision: u32,
    pub key: String,
    pub inputs: Vec<InputRecord>,
    pub options: Value,
    pub digest: String,
    /// Every file of the step, also those that no client reads.
    pub files: Vec<LayerFile>,
    /// Source id to the fetch that the step read: a receipt holds digests, not versions.
    pub snapshots: BTreeMap<String, SnapshotRead>,
    /// The files that a target serves.
    pub client: Client,
    /// What the build of the receipt cost, on the machine that built it.
    pub cost: Cost,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRead {
    pub version: String,
    /// The `NAME=VALUE` of the fetch, sorted.
    pub params: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub wall_ms: u64,
    pub peak_rss_bytes: Option<u64>,
    pub bytes_out: u64,
}

/// `release.json`: the release that a target serves, and the one that a rollback switches to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Serving {
    pub serving: Role,
    pub rollback: Option<Role>,
}

/// A release and the time it took its role, `YYYY-MM-DDTHH:MM:SSZ`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub id: String,
    pub since: String,
}

impl Output {
    /// The output that `receipt` records, built by `step`.
    pub fn new(receipt: &Receipt, step: &Step) -> Self {
        let snapshots = step.inputs.iter().filter_map(|input| match input {
            Input::Snapshot { source, version, params, .. } => {
                let params = crate::store::sorted(params);
                Some((source.clone(), SnapshotRead { version: version.clone(), params }))
            }
            Input::Step { .. } => None,
        });
        Output {
            layer: receipt.layer.clone(),
            step: receipt.step.clone(),
            format: receipt.format,
            revision: receipt.revision,
            key: receipt.key.clone(),
            inputs: receipt.inputs.clone(),
            options: receipt.options.clone(),
            digest: receipt.digest.clone(),
            files: receipt.files.clone(),
            snapshots: snapshots.collect(),
            client: step.client.sorted(),
            cost: Cost {
                wall_ms: receipt.wall_ms,
                peak_rss_bytes: receipt.peak_rss_bytes,
                bytes_out: receipt.bytes_out,
            },
        }
    }

    pub fn client_files(&self) -> impl Iterator<Item = &LayerFile> {
        self.files.iter().filter(|file| self.client.includes(&file.path))
    }
}

impl Release {
    /// The manifest as compact JSON with the keys of each object in byte order.
    pub fn canonical(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("a release serializes");
        serde_json::to_vec(&sorted(value)).expect("JSON values serialize")
    }

    pub fn id(&self) -> String {
        sha256_hex(&self.canonical())
    }

    /// The SHA-256 and the size of each client file of an output: the target holds its bytes at
    /// `objects/<sha256>`.
    pub fn objects(&self) -> BTreeMap<&str, u64> {
        self.outputs.iter().flat_map(Output::client_files).map(|file| (file.sha256.as_str(), file.size)).collect()
    }

    /// The keys of the release but its manifest: the objects of the client files, the named files,
    /// then the copies of the source files.
    pub fn files(&self) -> Vec<File> {
        let objects = self.objects().into_iter().map(|(sha256, size)| File {
            key: format!("objects/{sha256}"),
            size,
            sha256: sha256.into(),
        });
        let mut copies = BTreeMap::new();
        for fetched in self.sources.iter().filter(|fetched| fetched.copy) {
            for file in &fetched.files {
                let key = fetched.key(file);
                copies.insert(key.clone(), File { key, size: file.size, sha256: file.sha256.clone() });
            }
        }
        objects.chain(self.named.iter().cloned()).chain(copies.into_values()).collect()
    }

    pub fn check(&self) -> Result<(), String> {
        if self.outputs.windows(2).any(|pair| pair[0].step >= pair[1].step) {
            return Err("the outputs are not sorted by step, or a step is there twice".into());
        }
        let mut files = self.outputs.iter().flat_map(|output| &output.files);
        if let Some(file) = files.find(|file| !is_sha256(&file.sha256)) {
            return Err(format!("the output file {} has no SHA-256", file.path));
        }
        let served = self.objects();
        for files in [&self.pointers, &self.named] {
            if files.windows(2).any(|pair| pair[0].key >= pair[1].key) {
                return Err("the pointers or the named files are not sorted by key, or a key is there twice".into());
            }
            if let Some(file) = files.iter().find(|file| served.get(file.sha256.as_str()) != Some(&file.size)) {
                return Err(format!("{} is no client file of an output", file.key));
            }
        }
        if !self.pointers.iter().map(|pointer| pointer.key.as_str()).eq(POINTERS) {
            return Err(format!("a release names the pointers {}, and no other", POINTERS.join(" and ")));
        }
        let mut folders: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        for file in &self.named {
            let place = file.key.strip_prefix("releases/").and_then(|rest| rest.split_once('/'));
            let Some((folder, path)) = place.filter(|(folder, _)| is_sha256(folder)) else {
                return Err(format!("the named file {} is not in a folder `releases/<sha256>/`", file.key));
            };
            folders.entry(folder).or_default().push((path, file.sha256.as_str()));
        }
        if let Some((folder, _)) = folders.into_iter().find(|(folder, files)| digest(files.iter().copied()) != *folder)
        {
            return Err(format!("releases/{folder}/ is not the digest of its files"));
        }
        let fetch = |fetched: &Fetched| (fetched.source.clone(), fetched.params.clone(), fetched.version.clone());
        if self.sources.windows(2).any(|pair| fetch(&pair[0]) >= fetch(&pair[1])) {
            return Err("the sources are not sorted by source, params and version, or a fetch is there twice".into());
        }
        for fetched in &self.sources {
            if fetched.files.windows(2).any(|pair| pair[0].name >= pair[1].name) {
                return Err(format!("the files of {}@{} are not sorted by name", fetched.source, fetched.version));
            }
            if let Some(file) = fetched.files.iter().find(|file| !is_sha256(&file.sha256)) {
                return Err(format!("the file {} of {}@{} has no SHA-256", file.name, fetched.source, fetched.version));
            }
        }
        for file in self.files() {
            crate::r2::check_key(&file.key)?;
        }
        // Two fetches of one version can name one file: its copy has one SHA-256.
        let mut copies = BTreeMap::new();
        for fetched in self.sources.iter().filter(|fetched| fetched.copy) {
            for file in &fetched.files {
                let key = fetched.key(file);
                if copies.insert(key.clone(), &file.sha256).is_some_and(|other| other != &file.sha256) {
                    return Err(format!("{key} is the copy of two files"));
                }
            }
        }
        Ok(())
    }

    /// The version of each fetch that an output read. One update reads one version of a fetch.
    pub fn versions(&self) -> crate::product::Versions {
        let reads = self.outputs.iter().flat_map(|output| &output.snapshots);
        let mut versions = crate::product::Versions::new();
        for (source, read) in reads {
            let version = versions.entry((source.clone(), read.params.clone())).or_default();
            *version = version.clone().max(read.version.clone());
        }
        versions
    }

    /// Whether `other` has the same settings, outputs, pointers and named files: what a target
    /// serves, whoever made it, when, and what its builds cost.
    pub fn serves_as(&self, other: &Release) -> bool {
        let same = |a: &Output, b: &Output| Output { cost: a.cost, ..b.clone() } == *a;
        // Whether the target copies a source, its credit, and where and when a machine retrieved a
        // file change no file of the release.
        let read = |a: &Fetched, b: &Fetched| {
            (&a.source, &a.version, &a.params) == (&b.source, &b.version, &b.params)
                && a.files.len() == b.files.len()
                && a.files.iter().zip(&b.files).all(|(a, b)| a.same(b))
        };
        (&self.settings, &self.pointers, &self.named) == (&other.settings, &other.pointers, &other.named)
            && self.sources.len() == other.sources.len()
            && self.sources.iter().zip(&other.sources).all(|(a, b)| read(a, b))
            && self.outputs.len() == other.outputs.len()
            && self.outputs.iter().zip(&other.outputs).all(|(a, b)| same(a, b))
    }

    /// The release `id` of `target`.
    pub fn read(target: &Target, id: &str) -> Result<Release, String> {
        let key = manifest_key(id);
        let bytes = target.get(&key)?.ok_or_else(|| format!("the target has no {key}"))?;
        Release::parse(id, &bytes)
    }

    /// The release `id` from the bytes of its manifest.
    pub fn parse(id: &str, bytes: &[u8]) -> Result<Release, String> {
        let key = manifest_key(id);
        if sha256_hex(bytes) != id {
            return Err(format!("{key}: its SHA-256 is not its id"));
        }
        let release: Release = serde_json::from_slice(bytes).map_err(|e| format!("{key}: {e}"))?;
        release.check().map_err(|e| format!("{key}: {e}"))?;
        Ok(release)
    }
}

pub fn manifest_key(id: &str) -> String {
    format!("releases/{id}.json")
}

/// The key of the copy of the file `name` of `source` at `version`.
pub fn copy_key(source: &str, version: &str, name: &str) -> String {
    format!("sources/{source}/{version}/{name}")
}

pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Give `target` each key of `release` that it lacks, from the store, and give the id. The store
/// gives only those keys: each must have its SHA-256 before the target changes. The manifest goes
/// last.
pub fn publish(target: &Target, store: &Store, release: &Release) -> Result<String, String> {
    release.check()?;
    let bytes = release.canonical();
    let id = sha256_hex(&bytes);
    let files = release.files();
    let mut sizes: Vec<(String, u64)> = files.iter().map(|file| (file.key.clone(), file.size)).collect();
    sizes.push((manifest_key(&id), bytes.len() as u64));
    let missing: BTreeSet<String> = target.missing(&sizes)?.into_iter().collect();
    let mut upload = Vec::new();
    for file in files.into_iter().filter(|file| missing.contains(&file.key)) {
        let path = store.object(&file.sha256);
        if hash_file(&path)? != (file.sha256.clone(), file.size) {
            return Err(format!("{} holds other bytes than {}; nothing changed", path.display(), file.key));
        }
        upload.push((path, file.key));
    }
    target.upload(&upload)?;
    if missing.contains(&manifest_key(&id)) {
        // Another writer of the same manifest writes the same bytes.
        target.put(&manifest_key(&id), &bytes, &Condition::Absent)?;
    }
    Ok(id)
}

/// What `release.json` of `target` says, and its tag; `None` before the first switch.
pub fn serving(target: &Target) -> Result<Option<(Serving, Tag)>, String> {
    let Some((bytes, tag)) = target.get_tagged(SERVING)? else { return Ok(None) };
    let serving = serde_json::from_slice(&bytes).map_err(|e| format!("{SERVING}: {e}"))?;
    Ok(Some((serving, tag)))
}

/// Make the published release `id` the one that `target` serves, and the one that `base` says it
/// serves its rollback. `false` when `release.json` is no longer `base`: nothing changed. The
/// pointers change after `release.json`, and before it a switch to another release needs each
/// pointer to hold the bytes of the serving release or of `id`: so each pointer holds the bytes of
/// the serving release or of the rollback. A switch to the serving release writes its pointers
/// again.
pub fn switch(target: &Target, base: Option<&(Serving, Tag)>, id: &str) -> Result<bool, String> {
    let release = Release::read(target, id)?;
    let since = crate::date::timestamp(crate::date::now());
    let next = match base {
        // A pointer that drifted served other bytes until now: the grace of the collection starts now.
        Some((now, _)) if now.serving.id == id => {
            Serving { serving: Role { id: id.into(), since: since.clone() }, rollback: now.rollback.clone() }
        }
        Some((now, _)) => {
            let serving = Release::read(target, &now.serving.id)?;
            for (now, next) in serving.pointers.iter().zip(&release.pointers) {
                let held = target.get(&now.key)?.map(|bytes| sha256_hex(&bytes));
                if held.as_ref().is_some_and(|held| *held != now.sha256 && *held != next.sha256) {
                    return Err(format!(
                        "{} is of neither the serving release nor {id}; switch to the serving release again first",
                        now.key
                    ));
                }
            }
            Serving {
                serving: Role { id: id.into(), since: since.clone() },
                rollback: Some(Role { id: now.serving.id.clone(), since }),
            }
        }
        None => Serving { serving: Role { id: id.into(), since }, rollback: None },
    };
    let condition = base.map_or(Condition::Absent, |(_, tag)| Condition::Match(tag.clone()));
    let bytes = serde_json::to_vec_pretty(&next).map_err(|e| e.to_string())?;
    if target.put(SERVING, &bytes, &condition)?.is_none() {
        return Ok(false);
    }
    for pointer in &release.pointers {
        let key = format!("objects/{}", pointer.sha256);
        let bytes = target.get(&key)?.filter(|bytes| sha256_hex(bytes) == pointer.sha256);
        let bytes =
            bytes.ok_or_else(|| format!("{key} is missing or holds other bytes; {} is unchanged", pointer.key))?;
        target.put(&pointer.key, &bytes, &Condition::Any)?;
    }
    Ok(true)
}

/// The pointer keys whose bytes on `target` are not those that `release` names.
pub fn drift(target: &Target, release: &Release) -> Result<Vec<String>, String> {
    let mut drift = Vec::new();
    for pointer in &release.pointers {
        if target.get(&pointer.key)?.is_none_or(|bytes| sha256_hex(&bytes) != pointer.sha256) {
            drift.push(pointer.key.clone());
        }
    }
    Ok(drift)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline, Fixture};

    /// A release of one output with the file `bytes`, which read `source@version`.
    pub(crate) fn reading(bytes: &[u8], source: &str, version: &str) -> Release {
        let file = LayerFile { path: "out".into(), size: bytes.len() as u64, sha256: sha256_hex(bytes) };
        let read = InputRecord {
            kind: crate::engine::InputKind::Snapshot,
            name: source.into(),
            digest: digest([]),
            files: Vec::new(),
        };
        let output = Output {
            layer: "test".into(),
            step: "test/one".into(),
            format: 1,
            revision: 1,
            key: "key".into(),
            inputs: vec![read],
            options: serde_json::json!({}),
            digest: String::new(),
            files: vec![file],
            snapshots: [(source.into(), SnapshotRead { version: version.into(), params: Vec::new() })].into(),
            client: Client::All,
            cost: Cost { wall_ms: 0, peak_rss_bytes: None, bytes_out: bytes.len() as u64 },
        };
        Release {
            settings: Settings::default(),
            outputs: vec![output],
            pointers: Vec::new(),
            named: Vec::new(),
            sources: Vec::new(),
            machine: "test".into(),
            time: "2026-10-12T00:00:00Z".into(),
        }
    }

    /// The release of the built pipeline: `test/join` gives the device pointer and `test/upper` the
    /// planner pointer; the files of `test/count` and `test/join` are named.
    fn release(fixture: &Fixture) -> Release {
        let steps = pipeline();
        let step = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        let built = fixture.build(&steps).unwrap();
        let mut outputs: Vec<Output> = built.iter().map(|b| Output::new(&b.receipt, step(&b.receipt.step))).collect();
        outputs.sort_by(|a, b| a.step.cmp(&b.step));
        let [count, joined, upper] = [0, 1, 2].map(|at| outputs[at].files[0].clone());
        let folder = digest([("count.txt", count.sha256.as_str()), ("joined.txt", joined.sha256.as_str())]);
        let file = |key: String, file: &LayerFile| File { key, size: file.size, sha256: file.sha256.clone() };
        Release {
            settings: Settings::default(),
            pointers: vec![file(POINTERS[0].into(), &joined), file(POINTERS[1].into(), &upper)],
            named: vec![
                file(format!("releases/{folder}/count.txt"), &count),
                file(format!("releases/{folder}/joined.txt"), &joined),
            ],
            sources: Vec::new(),
            outputs,
            machine: "laptop".into(),
            time: "2026-10-12T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_switch_writes_release_json_then_the_pointers_and_keeps_one_rollback() {
        let fixture = fixture("release-switch");
        let target = Target::Dir(fixture.scratch.0.join("target"));
        let first = release(&fixture);
        let id = publish(&target, &fixture.store, &first).unwrap();
        assert_eq!(Release::read(&target, &id).unwrap(), first);
        for file in first.files() {
            let bytes = target.get(&file.key).unwrap().unwrap();
            assert_eq!((sha256_hex(&bytes), bytes.len() as u64), (file.sha256.clone(), file.size), "{}", file.key);
        }
        assert_eq!(drift(&target, &first).unwrap(), POINTERS);
        assert!(switch(&target, None, &id).unwrap());
        let base = serving(&target).unwrap().unwrap();
        assert_eq!((base.0.serving.id.as_str(), &base.0.rollback), (id.as_str(), &None));
        assert!(drift(&target, &first).unwrap().is_empty());
        assert!(switch(&target, Some(&base), &"b".repeat(64)).unwrap_err().contains("has no releases/"));

        // The target has every file: a second release that reads none of the store publishes.
        let count = first.outputs[0].files[0].clone();
        let saved = std::fs::read(fixture.store.object(&count.sha256)).unwrap();
        for file in first.files() {
            let _ = std::fs::remove_file(fixture.store.object(&file.sha256));
        }
        let mut second = Release { machine: "vps".into(), ..first.clone() };
        second.pointers[1] = File { key: POINTERS[1].into(), size: count.size, sha256: count.sha256.clone() };
        let next = publish(&target, &fixture.store, &second).unwrap();
        assert!(!switch(&target, None, &next).unwrap(), "release.json is no longer absent");
        let old = (base.0.clone(), Tag("0".repeat(64)));
        assert!(!switch(&target, Some(&old), &next).unwrap(), "release.json has another tag");
        assert_eq!(serving(&target).unwrap().unwrap(), base, "a refused switch changes nothing");

        // `release.json` changes first: without the object of a pointer, the pointer stays.
        let object = fixture.scratch.0.join("target").join(format!("objects/{}", count.sha256));
        std::fs::remove_file(&object).unwrap();
        assert!(switch(&target, Some(&base), &next).unwrap_err().contains("is unchanged"));
        let now = serving(&target).unwrap().unwrap();
        assert_eq!((&now.0.serving.id, now.0.rollback.as_ref().map(|role| &role.id)), (&next, Some(&id)));
        assert_eq!(drift(&target, &second).unwrap(), [POINTERS[1]]);

        // A rollback after the interrupted switch needs no object of the release that it leaves.
        assert!(switch(&target, Some(&now), &id).unwrap(), "a rollback");
        let back = serving(&target).unwrap().unwrap();
        assert_eq!((&back.0.serving.id, back.0.rollback.as_ref().map(|role| &role.id)), (&id, Some(&next)));
        assert!(drift(&target, &first).unwrap().is_empty());

        target.put(POINTERS[0], b"another publisher", &Condition::Any).unwrap();
        let refused = switch(&target, Some(&back), &next).unwrap_err();
        assert!(refused.contains("of neither the serving release nor"), "{refused}");
        let mut before = back.0.clone();
        before.serving.since = "2026-01-01T00:00:00Z".into();
        target.put(SERVING, &serde_json::to_vec_pretty(&before).unwrap(), &Condition::Any).unwrap();
        assert!(switch(&target, serving(&target).unwrap().as_ref(), &id).unwrap(), "a switch to the serving release");
        assert!(drift(&target, &first).unwrap().is_empty(), "repairs the pointer");
        let repaired = serving(&target).unwrap().unwrap().0;
        assert_eq!((&repaired.serving.id, &repaired.rollback), (&before.serving.id, &before.rollback));
        assert_ne!(repaired.serving.since, before.serving.since, "the old pointer served until now");

        std::fs::write(fixture.scratch.0.join("count"), saved).unwrap();
        target.upload(&[(fixture.scratch.0.join("count"), format!("objects/{}", count.sha256))]).unwrap();
        assert!(switch(&target, Some(&serving(&target).unwrap().unwrap()), &next).unwrap());
        assert!(drift(&target, &second).unwrap().is_empty());

        let forged = "a".repeat(64);
        target.put(&manifest_key(&forged), &second.canonical(), &Condition::Absent).unwrap();
        assert!(Release::read(&target, &forged).unwrap_err().contains("not its id"));
    }

    #[test]
    fn a_manifest_keeps_its_pointers_and_named_files_in_their_places() {
        let fixture = fixture("release-check");
        let valid = release(&fixture);
        valid.check().unwrap();
        let upper = valid.outputs[2].files[0].clone();
        let refused = |edit: &dyn Fn(&mut Release)| {
            let mut release = valid.clone();
            edit(&mut release);
            release.check().unwrap_err()
        };
        assert!(refused(&|r| r.outputs.swap(0, 1)).contains("sorted by step"));
        assert!(refused(&|r| r.outputs[0].files[0].sha256 = "x".into()).contains("no SHA-256"));
        assert!(refused(&|r| r.pointers[0].key = "objects/a".into()).contains("no other"));
        assert!(refused(&|r| drop(r.pointers.pop())).contains("no other"), "a release names both pointers");
        assert!(refused(&|r| r.pointers[0].sha256 = "a".repeat(64)).contains("no client file of an output"));
        let private = |r: &mut Release| r.outputs[1].client = Client::None;
        assert!(refused(&private).contains("no client file of an output"), "a target serves only client files");
        assert!(refused(&|r| r.named[0].key = r.named[0].key.replace("releases/", "named/")).contains("folder"));
        let other = |r: &mut Release| (r.named[1].sha256, r.named[1].size) = (upper.sha256.clone(), upper.size);
        assert!(refused(&other).contains("not the digest"), "a named file changes the folder");
        assert!(refused(&|r| drop(r.named.pop())).contains("not the digest"));
        let fetch = |params: &str, sha256: &str| Fetched {
            source: "land".into(),
            version: "1".into(),
            params: vec![("area".into(), params.into())],
            files: vec![FetchedFile {
                name: "land.zip".into(),
                url: String::new(),
                size: 1,
                sha256: sha256.into(),
                retrieved: String::new(),
            }],
            copy: true,
            licence: None,
            attribution: None,
        };
        let copies = |r: &mut Release| r.sources = vec![fetch("a", &"a".repeat(64)), fetch("b", &"b".repeat(64))];
        assert!(refused(&copies).contains("copy of two files"));

        let mut rebuilt = Release { machine: "desktop".into(), ..valid.clone() };
        rebuilt.outputs[0].cost.wall_ms += 1;
        assert!(rebuilt.serves_as(&valid), "a rebuild of the same bytes serves as the release");
        rebuilt.sources = vec![Fetched { copy: false, ..fetch("a", &"a".repeat(64)) }];
        let copied = Release { sources: vec![fetch("a", &"a".repeat(64))], ..valid.clone() };
        assert!(rebuilt.serves_as(&copied), "a copy alone changes no file of the release");
        rebuilt.outputs[0].key = "other".into();
        assert!(!rebuilt.serves_as(&valid));
    }
}
