//! A release: what a target serves, in one manifest. It names the settings of the target, every
//! output, the client pointers, and the lists of the files of each output, of each fetch and of the
//! files that a client finds by name. Each list is an object of its own, so the manifest grows with
//! the steps and not with the files. Its id is the SHA-256 of its bytes, which give the SHA-256 of
//! each list. `release.json` of the target names the release that it serves and one rollback: a
//! switch writes it first, so it is the commit point.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::engine::{digest, sorted, Client, Input, InputRecord, LayerFile, Receipt, Step};
use crate::settings::Settings;
use crate::store::{hash_file, sha256_hex, Store};
use crate::target::{Condition, Tag, Target};

/// The key that names the release that a target serves, and its rollback.
pub const SERVING: &str = "release.json";

/// The key that names, as a [`Role`], the release whose planner services load before a switch to it.
pub const NEXT: &str = "planner/next.json";

/// The keys that clients read first, sorted. Every release names both. Clients that an older
/// publisher served read them too, so nothing deletes them.
pub const POINTERS: [&str; 2] = ["cell-catalog/catalog.json", "planner/catalog.json"];

/// The format of [`Release`] and [`Serving`], which a follower says at `/planner-api/version`. A change
/// that a follower of the format before cannot read raises it; so does a stricter [`Release::check`]
/// or a new word of a refresh, which the shape of the manifest does not show. A follower reads its
/// own format and the one before, so a deploy of the services keeps the release that prod serves:
/// `tests/manifests/` holds a manifest of each.
pub const FORMAT: u32 = 2;

/// Two releases are equal when their manifests are.
#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub settings: Settings,
    /// Sorted by `step`, each step once.
    pub outputs: Vec<OutputEntry>,
    /// What a client reads at each key of [`POINTERS`]: a client file of an output. Sorted by key.
    pub pointers: Vec<File>,
    /// The client files of outputs that a client finds by name, at `releases/<folder>/<path>`:
    /// [`File`]s sorted by key. The folder is the digest of its files, so other files make another
    /// folder.
    pub named: List,
    /// Each fetch that an output read, sorted by source, params and version.
    pub sources: Vec<FetchedEntry>,
    /// The machine that made the release.
    pub machine: String,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub time: String,
    /// The bytes of the lists that the target does not hold, by SHA-256: of a manifest of format 1,
    /// which holds its lists inline, and of a release before its publish.
    #[serde(skip)]
    held: BTreeMap<String, Vec<u8>>,
    /// The id that the release was read under; `None` before a publish.
    #[serde(skip)]
    read_as: Option<String>,
}

impl PartialEq for Release {
    fn eq(&self, other: &Release) -> bool {
        self.canonical() == other.canonical()
    }
}

impl Eq for Release {}

/// A list of a release: an object of the target, compact JSON with the keys of each object in byte
/// order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct List {
    pub size: u64,
    pub sha256: String,
}

impl List {
    pub fn key(&self) -> String {
        format!("objects/{}", self.sha256)
    }
}

/// A key of a target and its bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub key: String,
    pub size: u64,
    pub sha256: String,
}

/// An output in the manifest: an [`Output`] whose files and inputs are in its list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputEntry {
    pub layer: String,
    pub step: String,
    pub format: u32,
    pub revision: u32,
    pub key: String,
    /// `{"files", "inputs"}`, as in [`Output`].
    pub list: List,
    pub options: Value,
    pub digest: String,
    pub snapshots: BTreeMap<String, SnapshotRead>,
    pub client: Client,
    pub cost: Cost,
}

/// A fetch in the manifest: a [`Fetched`] whose files are in its list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchedEntry {
    pub source: String,
    pub version: String,
    /// The sorted `NAME=VALUE`s of the fetch.
    pub params: Vec<(String, String)>,
    /// The [`FetchedFile`]s, sorted by name.
    pub list: List,
}

/// The files of one fetch that outputs of a release read. A manifest of format 1 of an older
/// publisher has more fields, such as `copy`: a reader ignores them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Fetched {
    pub source: String,
    pub version: String,
    /// The sorted `NAME=VALUE`s of the fetch.
    pub params: Vec<(String, String)>,
    /// Sorted by name.
    pub files: Vec<FetchedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
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

/// The files of one step: its receipt, what it read of each source, and what a client reads.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
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

/// The list of an output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputList {
    files: Vec<LayerFile>,
    inputs: Vec<InputRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRead {
    pub version: String,
    /// The `NAME=VALUE` of the fetch, sorted.
    pub params: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
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

/// A manifest of format 1, which holds its lists inline.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inline {
    settings: Settings,
    outputs: Vec<Output>,
    pointers: Vec<File>,
    named: Vec<File>,
    sources: Vec<Fetched>,
    machine: String,
    time: String,
}

/// The lists of a release, read and checked: what [`Release::load`] gives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// Each output with its files and inputs, sorted by step.
    pub outputs: Vec<Output>,
    /// Each fetch with the files that the outputs read, sorted by source, params and version.
    pub sources: Vec<Fetched>,
    /// The files that a client finds by name, sorted by key.
    pub named: Vec<File>,
    /// The lists that the target holds: none of a manifest that holds its lists.
    pub lists: Vec<File>,
}

impl Files {
    /// The SHA-256 and the size of each client file of an output: the target holds its bytes at
    /// `objects/<sha256>`.
    pub fn objects(&self) -> BTreeMap<&str, u64> {
        self.outputs.iter().flat_map(Output::client_files).map(|file| (file.sha256.as_str(), file.size)).collect()
    }

    /// The keys of the release but its manifest: its lists, the objects of the client files, then
    /// the named files.
    pub fn files(&self) -> Vec<File> {
        let objects = self.objects().into_iter().map(|(sha256, size)| File {
            key: format!("objects/{sha256}"),
            size,
            sha256: sha256.into(),
        });
        self.lists.iter().cloned().chain(objects).chain(self.named.iter().cloned()).collect()
    }

    /// The checks of `release` that need its lists.
    pub fn check(&self, release: &Release) -> Result<(), String> {
        let mut files = self.outputs.iter().flat_map(|output| &output.files);
        if let Some(file) = files.find(|file| !is_sha256(&file.sha256)) {
            return Err(format!("the output file {} has no SHA-256", file.path));
        }
        if self.named.windows(2).any(|pair| pair[0].key >= pair[1].key) {
            return Err("the named files are not sorted by key, or a key is there twice".into());
        }
        let served = self.objects();
        for files in [&release.pointers, &self.named] {
            if let Some(file) = files.iter().find(|file| served.get(file.sha256.as_str()) != Some(&file.size)) {
                return Err(format!("{} is no client file of an output", file.key));
            }
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
        Ok(())
    }
}

/// Compact JSON with the keys of each object in byte order.
fn canonical(value: &impl Serialize) -> Vec<u8> {
    let value = serde_json::to_value(value).expect("a release serializes");
    serde_json::to_vec(&sorted(value)).expect("JSON values serialize")
}

impl Release {
    /// The release of `files`, unchecked. It holds the bytes of its lists until a publish gives them
    /// to the target.
    pub fn new(settings: Settings, pointers: Vec<File>, files: &Files, machine: String, time: String) -> Release {
        let mut held = BTreeMap::new();
        let mut list = |value: Value| {
            let bytes = canonical(&value);
            let list = List { size: bytes.len() as u64, sha256: sha256_hex(&bytes) };
            held.insert(list.sha256.clone(), bytes);
            list
        };
        let outputs = files.outputs.iter().map(|output| OutputEntry {
            layer: output.layer.clone(),
            step: output.step.clone(),
            format: output.format,
            revision: output.revision,
            key: output.key.clone(),
            list: list(json!({"files": output.files, "inputs": output.inputs})),
            options: output.options.clone(),
            digest: output.digest.clone(),
            snapshots: output.snapshots.clone(),
            client: output.client.clone(),
            cost: output.cost,
        });
        let outputs = outputs.collect();
        let sources = files.sources.iter().map(|fetched| FetchedEntry {
            source: fetched.source.clone(),
            version: fetched.version.clone(),
            params: fetched.params.clone(),
            list: list(json!(fetched.files)),
        });
        let sources = sources.collect();
        let named = list(json!(files.named));
        Release { settings, outputs, pointers, named, sources, machine, time, held, read_as: None }
    }

    /// The manifest of this format as compact JSON with the keys of each object in byte order.
    pub fn canonical(&self) -> Vec<u8> {
        canonical(self)
    }

    /// The id that the release was read under, also of a manifest of format 1; of a release built in
    /// memory, the SHA-256 of [`Release::canonical`], which a publish writes. A change of the public
    /// fields of a read release does not change it.
    pub fn id(&self) -> String {
        self.read_as.clone().unwrap_or_else(|| sha256_hex(&self.canonical()))
    }

    /// The checks of the manifest alone.
    pub fn check(&self) -> Result<(), String> {
        if self.outputs.windows(2).any(|pair| pair[0].step >= pair[1].step) {
            return Err("the outputs are not sorted by step, or a step is there twice".into());
        }
        if self.pointers.windows(2).any(|pair| pair[0].key >= pair[1].key) {
            return Err("the pointers are not sorted by key, or a key is there twice".into());
        }
        if !self.pointers.iter().map(|pointer| pointer.key.as_str()).eq(POINTERS) {
            return Err(format!("a release names the pointers {}, and no other", POINTERS.join(" and ")));
        }
        let fetch = |fetched: &FetchedEntry| (fetched.source.clone(), fetched.params.clone(), fetched.version.clone());
        if self.sources.windows(2).any(|pair| fetch(&pair[0]) >= fetch(&pair[1])) {
            return Err("the sources are not sorted by source, params and version, or a fetch is there twice".into());
        }
        if let Some(list) = self.lists().into_iter().find(|list| !is_sha256(&list.sha256)) {
            return Err(format!("the list {} has no SHA-256", list.key));
        }
        Ok(())
    }

    /// Each list as a key of the target, sorted by key, each once.
    pub fn lists(&self) -> Vec<File> {
        let outputs = self.outputs.iter().map(|output| &output.list);
        let lists = outputs.chain(self.sources.iter().map(|fetched| &fetched.list)).chain([&self.named]);
        let unique: BTreeMap<&str, u64> = lists.map(|list| (list.sha256.as_str(), list.size)).collect();
        let file = |(sha256, size): (&str, u64)| File { key: format!("objects/{sha256}"), size, sha256: sha256.into() };
        unique.into_iter().map(file).collect()
    }

    /// The lists of the release, each from `read` unless the manifest holds it, checked.
    pub fn load(&self, read: impl Fn(&List) -> Result<Vec<u8>, String>) -> Result<Files, String> {
        let mut outputs = Vec::with_capacity(self.outputs.len());
        for entry in &self.outputs {
            let OutputList { files, inputs } = self.list(&entry.list, &read)?;
            outputs.push(Output {
                layer: entry.layer.clone(),
                step: entry.step.clone(),
                format: entry.format,
                revision: entry.revision,
                key: entry.key.clone(),
                inputs,
                options: entry.options.clone(),
                digest: entry.digest.clone(),
                files,
                snapshots: entry.snapshots.clone(),
                client: entry.client.clone(),
                cost: entry.cost,
            });
        }
        let mut sources = Vec::with_capacity(self.sources.len());
        for entry in &self.sources {
            let (source, version, params) = (entry.source.clone(), entry.version.clone(), entry.params.clone());
            sources.push(Fetched { source, version, params, files: self.list(&entry.list, &read)? });
        }
        let named = self.named_files(&read)?;
        let lists = self.lists().into_iter().filter(|list| !self.held.contains_key(&list.sha256)).collect();
        let files = Files { outputs, sources, named, lists };
        files.check(self)?;
        Ok(files)
    }

    /// The files that a client finds by name, from `read` unless the manifest holds their list: the
    /// one list that a follower reads.
    pub fn named_files(&self, read: impl Fn(&List) -> Result<Vec<u8>, String>) -> Result<Vec<File>, String> {
        self.list(&self.named, &read)
    }

    fn list<T: DeserializeOwned>(
        &self,
        list: &List,
        read: &impl Fn(&List) -> Result<Vec<u8>, String>,
    ) -> Result<T, String> {
        let read_bytes;
        let bytes = match self.held.get(&list.sha256) {
            Some(bytes) => bytes,
            None => {
                read_bytes = read(list)?;
                &read_bytes
            }
        };
        if (bytes.len() as u64, sha256_hex(bytes)) != (list.size, list.sha256.clone()) {
            return Err(format!("{} does not hold the list that the manifest names", list.key()));
        }
        serde_json::from_slice(bytes).map_err(|e| format!("{}: {e}", list.key()))
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

    /// Whether `other` has the same settings, outputs, pointers, named files and fetches: what a
    /// target serves, whoever made it, when, and what its builds cost. An update keeps the entry of
    /// each file of a fetch that the serving release read, so where and when a machine retrieved it
    /// changes no list.
    pub fn serves_as(&self, other: &Release) -> bool {
        let same = |a: &OutputEntry, b: &OutputEntry| OutputEntry { cost: a.cost, ..b.clone() } == *a;
        (&self.settings, &self.pointers, &self.named, &self.sources)
            == (&other.settings, &other.pointers, &other.named, &other.sources)
            && self.outputs.len() == other.outputs.len()
            && self.outputs.iter().zip(&other.outputs).all(|(a, b)| same(a, b))
    }

    /// The release `id` of `target`.
    pub fn read(target: &Target, id: &str) -> Result<Release, String> {
        let key = manifest_key(id);
        let bytes = target.get(&key)?.ok_or_else(|| format!("the target has no {key}"))?;
        Release::parse(id, &bytes)
    }

    /// The release `id` from the bytes of its manifest, of this format or of format 1. It checks the
    /// manifest; [`Release::load`] checks the lists.
    pub fn parse(id: &str, bytes: &[u8]) -> Result<Release, String> {
        let key = manifest_key(id);
        if sha256_hex(bytes) != id {
            return Err(format!("{key}: its SHA-256 is not its id"));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(|e| format!("{key}: {e}"))?;
        let mut release = if value["named"].is_array() {
            let inline: Inline = serde_json::from_value(value).map_err(|e| format!("{key}: {e}"))?;
            let files =
                Files { outputs: inline.outputs, sources: inline.sources, named: inline.named, lists: Vec::new() };
            Release::new(inline.settings, inline.pointers, &files, inline.machine, inline.time)
        } else {
            serde_json::from_value(value).map_err(|e| format!("{key}: {e}"))?
        };
        release.check().map_err(|e| format!("{key}: {e}"))?;
        release.read_as = Some(id.into());
        Ok(release)
    }
}

/// The reader of the lists in `store`, which takes a list that it lacks from its target: a list is
/// immutable, so the store caches it.
pub fn in_store(store: &Store) -> impl Fn(&List) -> Result<Vec<u8>, String> + '_ {
    |list| {
        let path = store.ensure(&list.sha256)?;
        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

pub fn manifest_key(id: &str) -> String {
    format!("releases/{id}.json")
}

pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Give `target` each key of `release` that it lacks, from the store, and give the id. The store
/// gets the lists that the release holds, and gives only those keys: each must have its SHA-256
/// before the target changes. The manifest goes last.
pub fn publish(target: &Target, store: &Store, release: &Release) -> Result<String, String> {
    release.check()?;
    for bytes in release.held.values() {
        store.put_object(bytes)?;
    }
    let files = release.load(in_store(store))?;
    let bytes = release.canonical();
    let id = sha256_hex(&bytes);
    let files: BTreeMap<String, File> =
        release.lists().into_iter().chain(files.files()).map(|file| (file.key.clone(), file)).collect();
    let mut sizes: Vec<(String, u64)> = files.values().map(|file| (file.key.clone(), file.size)).collect();
    sizes.push((manifest_key(&id), bytes.len() as u64));
    let missing: BTreeSet<String> = target.missing(&sizes)?.into_iter().collect();
    let mut upload = Vec::new();
    for file in files.into_values().filter(|file| missing.contains(&file.key)) {
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
    // The switch names only a release whose keys the target holds, each with its size.
    if let Some(key) = target.missing(&sizes)?.first() {
        return Err(format!("{key} is not on the target after the upload; nothing switched"));
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
    let since = crate::date::timestamp(target.now()?);
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

    #[test]
    fn the_format_pins_the_shape_of_the_manifest() {
        // The shape without the doc comments, which schemars writes as descriptions, with sorted keys:
        // serde_json keeps the order of insertion when a crate of the build turns on `preserve_order`.
        fn shape(value: &Value) -> String {
            match value {
                Value::Object(map) => {
                    let mut keys: Vec<&String> = map.keys().filter(|k| *k != "description").collect();
                    keys.sort();
                    let fields: Vec<String> =
                        keys.iter().map(|k| format!("{}:{}", Value::from(k.as_str()), shape(&map[*k]))).collect();
                    format!("{{{}}}", fields.join(","))
                }
                Value::Array(list) => {
                    // `required` follows the order of the fields, which the bytes of a manifest do not.
                    let mut items: Vec<String> = list.iter().map(shape).collect();
                    if list.iter().all(Value::is_string) {
                        items.sort();
                    }
                    format!("[{}]", items.join(","))
                }
                other => other.to_string(),
            }
        }
        let schemas = serde_json::json!([
            schemars::schema_for!(Release),
            schemars::schema_for!(Serving),
            schemars::schema_for!(Output),
            schemars::schema_for!(Vec<FetchedFile>),
        ]);
        let digest = sha256_hex(shape(&schemas).as_bytes());
        assert_eq!(
            (FORMAT, &digest[..16]),
            (2, "0da8ea7f1f56f35e"),
            "The manifest or a list changed its shape. When a follower of this FORMAT cannot read a manifest of the \
             new code, raise FORMAT and add a manifest of it to tests/manifests/. Then pin the new digest here."
        );
    }

    #[test]
    fn the_follower_reads_the_manifests_of_its_format_and_the_one_before() {
        let manifests: [(u32, &[u8]); 2] = [
            (1, include_bytes!("../tests/manifests/format-1.json")),
            (2, include_bytes!("../tests/manifests/format-2.json")),
        ];
        let formats: Vec<u32> = manifests.iter().map(|(format, _)| *format).collect();
        assert_eq!(
            formats,
            (FORMAT.saturating_sub(1).max(1)..=FORMAT).collect::<Vec<_>>(),
            "a manifest of each format"
        );
        for (format, bytes) in manifests {
            Release::parse(&sha256_hex(bytes), bytes).unwrap_or_else(|e| panic!("format {format}: {e}"));
        }
        // A manifest of format 1 holds its lists: they load without a read, and the target holds none.
        let inline = Release::parse(&sha256_hex(manifests[0].1), manifests[0].1).unwrap();
        assert_eq!(inline.id(), sha256_hex(manifests[0].1), "the id that it was read under");
        let files = inline.load(|list| Err(format!("read {}", list.key()))).unwrap();
        assert_eq!((files.outputs.len(), files.named.len(), files.lists.len()), (3, 2, 0));
        assert!(files.files().iter().all(|file| !inline.lists().contains(file)));
        assert_eq!(inline.canonical(), manifests[1].1, "format-2.json is format-1.json in format 2");
    }

    /// The files of a release of one output with the file `bytes`, which read `source@version`.
    pub(crate) fn reading(bytes: &[u8], source: &str, version: &str) -> Files {
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
        Files { outputs: vec![output], ..Files::default() }
    }

    /// Write `release` to `target` as a publish does, without a store, and give its id.
    pub(crate) fn put(target: &Target, release: &Release) -> String {
        for bytes in release.held.values() {
            target.put(&format!("objects/{}", sha256_hex(bytes)), bytes, &Condition::Any).unwrap();
        }
        let id = release.id();
        target.put(&manifest_key(&id), &release.canonical(), &Condition::Any).unwrap();
        id
    }

    /// The pointers and the files of a release of the built pipeline: `test/join` gives the device
    /// pointer and `test/upper` the planner pointer; the files of `test/count` and `test/join` are
    /// named.
    fn parts(fixture: &Fixture) -> (Vec<File>, Files) {
        let steps = pipeline();
        let step = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        let built = fixture.build(&steps).unwrap();
        let mut outputs: Vec<Output> = built.iter().map(|b| Output::new(&b.receipt, step(&b.receipt.step))).collect();
        outputs.sort_by(|a, b| a.step.cmp(&b.step));
        let [count, joined, upper] = [0, 1, 2].map(|at| outputs[at].files[0].clone());
        let folder = digest([("count.txt", count.sha256.as_str()), ("joined.txt", joined.sha256.as_str())]);
        let file = |key: String, file: &LayerFile| File { key, size: file.size, sha256: file.sha256.clone() };
        let pointers = vec![file(POINTERS[0].into(), &joined), file(POINTERS[1].into(), &upper)];
        let named = vec![
            file(format!("releases/{folder}/count.txt"), &count),
            file(format!("releases/{folder}/joined.txt"), &joined),
        ];
        (pointers, Files { outputs, named, ..Files::default() })
    }

    /// The checked release of `files`.
    fn made(pointers: Vec<File>, files: &Files) -> Result<Release, String> {
        let release =
            Release::new(Settings::default(), pointers, files, "laptop".into(), "2026-10-12T00:00:00Z".into());
        release.check().and_then(|()| files.check(&release)).map(|()| release)
    }

    fn release(fixture: &Fixture) -> Release {
        let (pointers, files) = parts(fixture);
        made(pointers, &files).unwrap()
    }

    #[test]
    fn a_switch_writes_release_json_then_the_pointers_and_keeps_one_rollback() {
        let fixture = fixture("release-switch");
        let target = Target::Dir(fixture.scratch.0.join("target"));
        let first = release(&fixture);
        let id = publish(&target, &fixture.store, &first).unwrap();
        let read = Release::read(&target, &id).unwrap();
        assert_eq!(read.canonical(), first.canonical());
        let reader = Store::at(fixture.scratch.0.join("reader")).reading(&target);
        let files = read.load(in_store(&reader)).unwrap();
        assert_eq!(files.lists, first.lists(), "the target holds each list");
        for file in files.files() {
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
        let count = files.outputs[0].files[0].clone();
        let saved = std::fs::read(fixture.store.object(&count.sha256)).unwrap();
        for file in files.files() {
            let _ = std::fs::remove_file(fixture.store.object(&file.sha256));
        }
        let mut pointers = first.pointers.clone();
        pointers[1] = File { key: POINTERS[1].into(), size: count.size, sha256: count.sha256.clone() };
        let second = Release::new(Settings::default(), pointers, &files, "vps".into(), read.time.clone());
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
    fn a_manifest_names_its_lists_and_its_id_proves_them() {
        let fixture = fixture("release-lists");
        let target = Target::Dir(fixture.scratch.0.join("target"));
        let (pointers, files) = parts(&fixture);
        let release = made(pointers.clone(), &files).unwrap();
        let manifest: Value = serde_json::from_slice(&release.canonical()).unwrap();
        assert!(manifest["outputs"][0].get("files").is_none() && manifest["outputs"][0]["list"]["sha256"].is_string());
        assert_eq!(release.lists().len(), 4, "one list of each output, and one of the named files");
        let id = publish(&target, &fixture.store, &release).unwrap();
        let read = Release::read(&target, &id).unwrap();
        let reader = |name: &str| Store::at(fixture.scratch.0.join(name)).reading(&target);
        assert_eq!(read.named_files(in_store(&reader("named"))).unwrap(), files.named);
        assert_eq!(read.load(in_store(&reader("all"))).unwrap(), Files { lists: release.lists(), ..files.clone() });

        let mut other = files.clone();
        other.outputs[0].inputs[0].files.push("other.txt".into());
        let changed = made(pointers, &other).unwrap();
        assert_ne!(changed.id(), release.id(), "a change of a list changes the id");
        assert_eq!(changed.outputs.iter().filter(|output| !release.outputs.contains(output)).count(), 1);

        // A list on the target with other bytes does not load.
        let list = read.outputs[0].list.key();
        target.put(&list, b"[]", &Condition::Any).unwrap();
        assert!(read.load(in_store(&reader("other"))).unwrap_err().contains("holds bytes with the SHA-256"));
    }

    #[test]
    fn a_manifest_keeps_its_pointers_and_named_files_in_their_places() {
        let fixture = fixture("release-check");
        let (pointers, valid) = parts(&fixture);
        made(pointers.clone(), &valid).unwrap();
        let upper = valid.outputs[2].files[0].clone();
        let refused = |edit: &dyn Fn(&mut Vec<File>, &mut Files)| {
            let (mut pointers, mut files) = (pointers.clone(), valid.clone());
            edit(&mut pointers, &mut files);
            made(pointers, &files).unwrap_err()
        };
        assert!(refused(&|_, f| f.outputs.swap(0, 1)).contains("sorted by step"));
        assert!(refused(&|_, f| f.outputs[0].files[0].sha256 = "x".into()).contains("no SHA-256"));
        assert!(refused(&|p, _| p[0].key = "objects/a".into()).contains("no other"));
        assert!(refused(&|p, _| drop(p.pop())).contains("no other"), "a release names both pointers");
        assert!(refused(&|p, _| p[0].sha256 = "a".repeat(64)).contains("no client file of an output"));
        let private = |_: &mut Vec<File>, f: &mut Files| f.outputs[1].client = Client::None;
        assert!(refused(&private).contains("no client file of an output"), "a target serves only client files");
        assert!(refused(&|_, f| f.named[0].key = f.named[0].key.replace("releases/", "named/")).contains("folder"));
        let other = |_: &mut Vec<File>, f: &mut Files| {
            (f.named[1].sha256, f.named[1].size) = (upper.sha256.clone(), upper.size);
        };
        assert!(refused(&other).contains("not the digest"), "a named file changes the folder");
        assert!(refused(&|_, f| drop(f.named.pop())).contains("not the digest"));

        let release = made(pointers.clone(), &valid).unwrap();
        let mut rebuilt = valid.clone();
        rebuilt.outputs[0].cost.wall_ms += 1;
        let rebuilt = Release::new(Settings::default(), pointers.clone(), &rebuilt, "desktop".into(), "now".into());
        assert!(rebuilt.serves_as(&release), "a rebuild of the same bytes serves as the release");
        let mut other = valid.clone();
        other.outputs[0].key = "other".into();
        assert!(!made(pointers, &other).unwrap().serves_as(&release));
    }

    /// A fetch of the file `land.zip`, retrieved at `retrieved`.
    fn land(retrieved: &str) -> Fetched {
        let file = FetchedFile {
            name: "land.zip".into(),
            url: String::new(),
            size: 1,
            sha256: "a".repeat(64),
            retrieved: retrieved.into(),
        };
        Fetched { source: "land".into(), version: "1".into(), params: Vec::new(), files: vec![file] }
    }

    #[test]
    fn a_manifest_of_format_1_that_lists_copies_of_its_sources_reads_and_names_no_copy() {
        let fixture = fixture("release-copies");
        let (pointers, files) = parts(&fixture);
        let files = Files { sources: vec![land("2026-10-01T00:00:00Z")], ..files };
        let mut older = serde_json::json!({
            "settings": Settings::default(),
            "outputs": files.outputs,
            "pointers": pointers,
            "named": files.named,
            "sources": files.sources,
            "machine": "laptop",
            "time": "2026-10-12T00:00:00Z",
        });
        let fetched = older["sources"][0].as_object_mut().unwrap();
        fetched.insert("copy".into(), true.into());
        fetched.insert("licence".into(), "ODbL-1.0".into());
        let bytes = serde_json::to_vec(&older).unwrap();
        let read = Release::parse(&sha256_hex(&bytes), &bytes).unwrap();
        assert_eq!(read, made(pointers, &files).unwrap());
        let loaded = read.load(|list| Err(format!("read {}", list.key()))).unwrap();
        assert_eq!(loaded, files);
        assert!(loaded.files().iter().all(|file| !file.key.starts_with("sources/")));
    }
}
