//! Prepared sources: a long job publishes one version of a source to a target, under
//! `sources/<id>/<version>/`, its manifest last. A bake reads a version as it reads any other
//! source: the files that a step names, each with the SHA-256 of the manifest. The manifest is the
//! identity of a version: a step reads `<version>/<manifest sha256>`, because two targets can have
//! other files under one version name. `specs/obc-data.md`, "Prepared sources", gives the layout.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::runs::Run;
use crate::engine::LayerFile;
use crate::fetch::http::Http;
use crate::product::{Inputs, RequestKey, Wanted};
use crate::release::Release;
use crate::sources::{FetchKind, Refresh, Source};
use crate::store::{hash_file, sha256_hex, FileRecord, Snapshot, Store};
use crate::target::{Condition, Listed, Target};

/// The target whose versions every target knows. Another target also knows its own.
pub(crate) const PROD: &str = "prod";
/// The key of the complete versions of a prepared source, in the folder of the source.
pub(crate) const VERSIONS: &str = "versions.json";
/// The name of the manifest in the folder of a version.
pub const MANIFEST: &str = "manifest.json";

/// The producer of prepared sources. The `obc data` binary passes each producer to `cli::main`.
pub trait Prepared: Sync {
    /// Kebab-case: it prepares the source with this id, or each source `<name>/<key>`.
    fn name(&self) -> &'static str;
    /// The number that readers check.
    fn format(&self) -> u32;
    /// A person changes it in the change that improves the output.
    fn revision(&self) -> u32;
    /// What a prepare of `source` for `scope` reads and makes, for its review. It fetches nothing.
    fn plan(&self, source: &Source, scope: Option<&Scope>) -> Result<Plan, String>;
    /// Make the files of the version of `job`: write each into `job.work`, then give it to
    /// [`Job::finish`]. A run that resumes calls it again; [`Job::done`] names what is done.
    fn prepare(&self, job: &mut Job) -> Result<(), String>;
}

/// What a prepare reads and makes, as its review shows it.
pub struct Plan {
    /// The sources that it reads: the fetches, and a source that a program of the producer reads,
    /// each with the size of its files when the producer knows it, such as a file on this machine.
    pub inputs: Vec<(Wanted, Option<u64>)>,
    /// The files of the version, at most; `None` when the producer cannot tell before the run.
    pub files: Option<u64>,
}

/// The producer of the source `id`.
pub(crate) fn producer<'a>(prepared: &[&'a dyn Prepared], id: &str) -> Option<&'a dyn Prepared> {
    let family = id.split_once('/').map_or(id, |(name, _)| name);
    prepared.iter().copied().find(|producer| producer.name() == family)
}

/// The folder of the source `id` on `target`, as a target: its lock, its versions and their files.
pub(crate) fn folder(target: &Target, id: &str) -> Target {
    target.within(&format!("sources/{id}"))
}

/// The part of the world that a version covers, as the settings of a target name a region.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub region: String,
    /// The TOML text of the region and of the members of a union, by id.
    pub definitions: BTreeMap<String, String>,
}

/// `sources/<id>/<version>/manifest.json`: compact JSON with the keys of each object in byte order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub source: String,
    pub version: String,
    pub format: u32,
    pub revision: u32,
    /// Each source version that the prepare read, sorted by source and version.
    pub inputs: Vec<Read>,
    /// Sorted by path.
    pub files: Vec<LayerFile>,
    /// `null`: every place that the source covers.
    pub scope: Option<Scope>,
    /// The licence and the credit of the source in `data/sources.toml`.
    pub licence: Option<String>,
    pub attribution: Option<String>,
    /// The machine that made the version.
    pub machine: String,
    /// When the version was complete, `YYYY-MM-DDTHH:MM:SSZ`.
    pub time: String,
    /// What the last run of the version cost: the review of the next version estimates from it.
    pub cost: Cost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub wall_ms: u64,
    /// The peak of the process of the run, or of the largest program that it ran; `null` when the
    /// system does not report it.
    pub peak_rss_bytes: Option<u64>,
}

/// A source version that a prepare read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub source: String,
    pub version: String,
    /// The [digest](#keys) of the files that it read.
    pub digest: String,
}

/// An entry of `versions.json`, which lists the complete versions sorted by version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub version: String,
    pub manifest_sha256: String,
    /// The format of the manifest: a target reads only a version of the format of its producer.
    pub format: u32,
    /// The size of its files.
    pub bytes: u64,
    /// When it was complete, `YYYY-MM-DDTHH:MM:SSZ`.
    pub time: String,
}

impl Manifest {
    pub fn canonical(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("a manifest serializes");
        serde_json::to_vec(&crate::engine::sorted(value)).expect("JSON values serialize")
    }

    fn parse(id: &str, version: &str, bytes: &[u8]) -> Result<Manifest, String> {
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|e| format!("sources/{id}/{version}/{MANIFEST}: {e}"))?;
        if (manifest.source.as_str(), manifest.version.as_str()) != (id, version) {
            return Err(format!("sources/{id}/{version}/{MANIFEST} is the manifest of another version"));
        }
        Ok(manifest)
    }
}

/// The complete versions of `id` on `target`.
pub(crate) fn versions(target: &Target, id: &str) -> Result<Vec<Version>, String> {
    let Some(bytes) = folder(target, id).get(VERSIONS)? else { return Ok(Vec::new()) };
    serde_json::from_slice(&bytes).map_err(|e| format!("sources/{id}/{VERSIONS}: {e}"))
}

/// The manifest of `id@version` on `target`; `None` while the version is not complete.
pub(crate) fn manifest(target: &Target, id: &str, version: &str) -> Result<Option<Manifest>, String> {
    let bytes = folder(target, id).get(&format!("{version}/{MANIFEST}"))?;
    bytes.map(|bytes| Manifest::parse(id, version, &bytes)).transpose()
}

/// Edit `versions.json` of the source folder `folder` with conditional writes, read again after a
/// write that another run changed first.
fn edit_versions(folder: &Target, edit: impl Fn(&mut Vec<Version>)) -> Result<(), String> {
    for _ in 0..8 {
        let read = folder.get_tagged(VERSIONS)?;
        let mut versions: Vec<Version> = match &read {
            Some((bytes, _)) => serde_json::from_slice(bytes).map_err(|e| format!("{VERSIONS}: {e}"))?,
            None => Vec::new(),
        };
        let before = versions.clone();
        edit(&mut versions);
        versions.sort_by(|a, b| a.version.cmp(&b.version));
        if read.is_some() && versions == before {
            return Ok(());
        }
        let condition = read.map_or(Condition::Absent, |(_, tag)| Condition::Match(tag));
        let bytes = serde_json::to_vec_pretty(&versions).map_err(|e| e.to_string())?;
        if folder.put(VERSIONS, &bytes, &condition)?.is_some() {
            return Ok(());
        }
    }
    Err(format!("{VERSIONS} changed at each of 8 writes"))
}

/// The version that a step reads: the name of `listed` and the SHA-256 of its manifest.
fn pinned(listed: &Version) -> String {
    format!("{}/{}", listed.version, listed.manifest_sha256)
}

/// A version as text shows it: a prepared version as `<date> (<first 12 hex digits of its manifest
/// SHA-256>)`. JSON gives the whole version.
pub fn show(version: &str) -> String {
    match unpin(version) {
        Some((name, sha256)) => format!("{name} ({})", &sha256[..12]),
        None => version.into(),
    }
}

/// The name and the manifest SHA-256 of the version `pinned` that a step reads.
pub(crate) fn unpin(pinned: &str) -> Option<(&str, &str)> {
    pinned
        .split_once('/')
        .filter(|(name, sha256)| crate::date::parse(name).is_some() && crate::release::is_sha256(sha256))
}

/// Make `manifest` the manifest of its version on `target`, after a check that the target has each
/// of its files with its size, then [`list`] the version. A manifest that an earlier run of the
/// version wrote stays when it names the same files.
pub(crate) fn complete(target: &Target, manifest: &Manifest) -> Result<Version, String> {
    let (id, version) = (&manifest.source, &manifest.version);
    let (folder, key) = (folder(target, id), format!("{version}/{MANIFEST}"));
    if let Some(missing) = absent(&folder, version, &manifest.files)?.first() {
        return Err(format!("sources/{id}/{missing} is not on the target; no manifest is written"));
    }
    if folder.put(&key, &manifest.canonical(), &Condition::Absent)?.is_none() {
        let kept = self::manifest(target, id, version)?.ok_or_else(|| format!("sources/{id}/{key} came and went"))?;
        if kept.files != manifest.files {
            return Err(format!("sources/{id}/{key} names other files than this run made"));
        }
    }
    list(target, id, version)
}

/// The keys `<version>/<path>` of `files` that the source folder `folder` lacks or holds with
/// another size.
pub(crate) fn absent(folder: &Target, version: &str, files: &[LayerFile]) -> Result<Vec<String>, String> {
    let sizes: Vec<(String, u64)> = files.iter().map(|file| (format!("{version}/{}", file.path), file.size)).collect();
    folder.missing(&sizes)
}

/// Add the complete version `id@version` of `target` to its `versions.json`, from its manifest.
pub(crate) fn list(target: &Target, id: &str, version: &str) -> Result<Version, String> {
    let folder = folder(target, id);
    let key = format!("{version}/{MANIFEST}");
    let bytes = folder.get(&key)?.ok_or_else(|| format!("sources/{id}/{key} is gone"))?;
    let manifest = Manifest::parse(id, version, &bytes)?;
    let entry = Version {
        version: version.into(),
        manifest_sha256: sha256_hex(&bytes),
        format: manifest.format,
        bytes: manifest.files.iter().map(|file| file.size).sum(),
        time: manifest.time,
    };
    edit_versions(&folder, |versions| {
        if !versions.iter().any(|listed| listed.version == entry.version) {
            versions.push(entry.clone());
        }
    })?;
    Ok(entry)
}

/// What a resume needs on this machine: `STORE/prepared/<target>/<id>/<version>/` holds the scope
/// of the run, its journal, `work/` and `state/`.
pub(crate) fn job_dir(store: &Store, target: &str, id: &str, version: &str) -> PathBuf {
    store.root().join("prepared").join(target).join(id).join(version)
}

/// The version of `id` for `target` that a run of this machine started and did not complete; the
/// newest one when there are more.
pub(crate) fn started(store: &Store, target: &str, id: &str) -> Result<Option<String>, String> {
    let dir = store.root().join("prepared").join(target).join(id);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let names = entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok());
    Ok(names.filter(|name| crate::date::parse(name).is_some()).max())
}

/// A line of the journal of a run.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Entry {
    /// A file that is on the target.
    File(LayerFile),
    /// The files of a source version that the prepare read, name to SHA-256.
    Input { source: String, version: String, files: BTreeMap<String, String> },
}

/// What a producer works with: one version of one source for one target.
pub struct Job<'a> {
    pub id: String,
    pub version: String,
    pub scope: Option<Scope>,
    /// The row of the source in `data/sources.toml`.
    pub source: &'a Source,
    /// Where the producer writes a file before [`Job::finish`]. It stays when the run stops.
    pub work: PathBuf,
    /// What the producer keeps for the runs of the version that follow, such as what it learnt of
    /// a file. It stays until the version is complete.
    pub state: PathBuf,
    /// The repository root, where a producer starts a program.
    pub root: &'a Path,
    store: &'a Store,
    http: &'a Http,
    sources: &'a [Source],
    folder: Target,
    run: &'a mut Run,
    journal: PathBuf,
    files: BTreeMap<String, LayerFile>,
    inputs: BTreeMap<(String, String), BTreeMap<String, String>>,
    /// The files that this run uploaded.
    pub(crate) uploaded: u64,
}

/// What [`Job::open`] needs besides the run.
pub struct Opening<'a> {
    pub root: &'a Path,
    pub store: &'a Store,
    pub http: &'a Http,
    pub sources: &'a [Source],
    /// The target, with its name.
    pub target: (&'a str, &'a Target),
}

impl<'a> Job<'a> {
    /// The job of `id@version` with `scope`, with what earlier runs of it did. A run of the version
    /// for another scope refuses.
    pub fn open(
        opening: Opening<'a>,
        (id, version): (&str, &str),
        scope: Option<Scope>,
        run: &'a mut Run,
    ) -> Result<Job<'a>, String> {
        let Opening { root, store, http, sources, target: (name, target) } = opening;
        let source = sources.iter().find(|source| source.id == id);
        let source = source.ok_or_else(|| format!("no source `{id}` in data/sources.toml"))?;
        let dir = job_dir(store, name, id, version);
        for folder in ["work", "state"] {
            fs::create_dir_all(dir.join(folder)).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let scope_file = dir.join("scope.json");
        let bytes = serde_json::to_vec(&scope).expect("a scope serializes");
        match fs::read(&scope_file) {
            Ok(held) if held == bytes => {}
            Ok(_) => {
                return Err(format!(
                    "a run of {id}@{version} for another region stopped; resume it with its region, or delete {}",
                    dir.display()
                ))
            }
            Err(_) => crate::store::write_atomic(&scope_file, &bytes)?,
        }
        let mut job = Job {
            id: id.into(),
            version: version.into(),
            scope,
            source,
            work: dir.join("work"),
            state: dir.join("state"),
            root,
            store,
            http,
            sources,
            folder: folder(target, id),
            run,
            journal: dir.join("journal.jsonl"),
            files: BTreeMap::new(),
            inputs: BTreeMap::new(),
            uploaded: 0,
        };
        let text = fs::read_to_string(&job.journal).unwrap_or_default();
        // A run that stopped in a write leaves a part of its last line.
        for entry in text.lines().filter_map(|line| serde_json::from_str::<Entry>(line).ok()) {
            job.apply(entry);
        }
        // The cleanup of a newer version can delete the keys of the journal: the run makes them again.
        let files: Vec<LayerFile> = job.files.values().cloned().collect();
        for key in absent(&job.folder, version, &files)? {
            job.files.remove(&key[version.len() + 1..]);
        }
        Ok(job)
    }

    fn apply(&mut self, entry: Entry) {
        match entry {
            Entry::File(file) => {
                self.files.insert(file.path.clone(), file);
            }
            Entry::Input { source, version, files } => self.inputs.entry((source, version)).or_default().extend(files),
        }
    }

    fn record(&mut self, entry: Entry) -> Result<(), String> {
        let mut line = serde_json::to_vec(&entry).expect("an entry serializes");
        line.push(b'\n');
        let mut journal = fs::OpenOptions::new().create(true).append(true).open(&self.journal);
        let written = journal
            .as_mut()
            .map_err(|e| e.to_string())
            .and_then(|journal| journal.write_all(&line).and_then(|()| journal.sync_data()).map_err(|e| e.to_string()));
        written.map_err(|e| format!("{}: {e}", self.journal.display()))?;
        self.apply(entry);
        Ok(())
    }

    /// Whether `path` is on the target: a resume does not make it again.
    pub fn done(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// The files that are on the target, by path.
    pub fn files(&self) -> impl Iterator<Item = &LayerFile> {
        self.files.values()
    }

    /// Fetch `wanted` into the store, and give the version and the object of each file by name. The
    /// manifest names the version that it read.
    pub fn fetch(&mut self, wanted: &Wanted) -> Result<(String, BTreeMap<String, PathBuf>), String> {
        let source = self.sources.iter().find(|source| source.id == wanted.source);
        let source = source.ok_or_else(|| format!("no source `{}` in data/sources.toml", wanted.source))?;
        let request = crate::fetch::Request {
            refresh: false,
            source,
            version: wanted.version.clone(),
            params: wanted.params.clone(),
        };
        let snapshot = self.run.fetch_request(self.root, self.store, self.http, &request, &[])?;
        let files = snapshot.files.iter().map(|file| (file.name.clone(), file.sha256.clone())).collect();
        self.input(&snapshot.source, &snapshot.version, files)?;
        let objects = snapshot.files.into_iter().map(|file| (file.name, self.store.object(&file.sha256))).collect();
        Ok((snapshot.version, objects))
    }

    /// Record that the prepare read `files`, name to SHA-256, of `source@version` without a fetch:
    /// a program of the producer read them. The manifest names the version.
    pub fn input(&mut self, source: &str, version: &str, files: BTreeMap<String, String>) -> Result<(), String> {
        self.record(Entry::Input { source: source.into(), version: version.into(), files })
    }

    /// Stop when a signal or `obc data runs RUN --stop` stops the run, such as in a long wait.
    pub fn stopped(&self) -> Result<(), String> {
        self.run.check_stop(self.store)
    }

    /// Run `command`, a program of the producer, in the repository root, and wait for it.
    pub fn program(&self, command: &mut Command) -> Result<(), String> {
        self.run.check_stop(self.store)?;
        // The program stops when this process does, so no orphan writes into a later run.
        command.current_dir(self.root).env("OBC_PARENT_PID", std::process::id().to_string());
        let program = command.get_program().to_string_lossy().into_owned();
        let status = command.status().map_err(|e| format!("cannot start `{program}`: {e}"))?;
        status.success().then_some(()).ok_or_else(|| format!("`{program}` failed: {status}"))
    }

    /// Upload the files `paths` of `work` to the target in one call, record each, and delete them
    /// from `work`.
    pub fn finish(&mut self, paths: &[String]) -> Result<(), String> {
        self.run.check_stop(self.store)?;
        let mut files = Vec::new();
        for path in paths {
            let relative = path.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
            if !relative || path == MANIFEST || path.contains('\\') {
                return Err(format!("{path} is not the path of a file of a version"));
            }
            let file = self.work.join(path);
            let (sha256, size) = hash_file(&file)?;
            if let Some(done) = self.files.get(path).filter(|done| done.sha256 != sha256) {
                return Err(format!("{path} has other bytes than the {} bytes that a run uploaded", done.size));
            }
            files.push((file, LayerFile { path: path.clone(), size, sha256 }));
        }
        // No reader reads the version before its manifest: a key of a stopped run can hold other
        // bytes, and they go.
        let keys: Vec<(PathBuf, String)> =
            files.iter().map(|(file, done)| (file.clone(), format!("{}/{}", self.version, done.path))).collect();
        self.folder.replace(&keys)?;
        for (file, done) in files {
            if !self.done(&done.path) {
                self.record(Entry::File(done))?;
                self.uploaded += 1;
            }
            fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        Ok(())
    }

    /// The source versions that the prepare read, sorted.
    pub(crate) fn reads(&self) -> Vec<Read> {
        let reads = self.inputs.iter().map(|((source, version), files)| Read {
            source: source.clone(),
            version: version.clone(),
            digest: crate::engine::digest(files.iter().map(|(name, sha256)| (name.as_str(), sha256.as_str()))),
        });
        reads.collect()
    }
}

/// Delete what this machine keeps of the run of `id@version` for `target`: the target has it all.
pub(crate) fn forget(store: &Store, target: &str, id: &str, version: &str) -> Result<(), String> {
    let dir = job_dir(store, target, id, version);
    fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// The files of `id@version` that the journal of its run on this machine names, sorted by path: a
/// resume does not upload them again.
pub(crate) fn journaled(store: &Store, target: &str, id: &str, version: &str) -> Vec<LayerFile> {
    let journal = job_dir(store, target, id, version).join("journal.jsonl");
    let text = fs::read_to_string(journal).unwrap_or_default();
    let mut files = BTreeMap::new();
    for entry in text.lines().filter_map(|line| serde_json::from_str::<Entry>(line).ok()) {
        if let Entry::File(file) = entry {
            files.insert(file.path.clone(), file);
        }
    }
    files.into_values().collect()
}

/// Give `inputs` the version of each prepared source of `read`, the fetches that the steps of the
/// target `name` read, in the format of its producer: the serving version of a `manual` source,
/// else the newest version that the target lists, or, for a target that is not [`PROD`], that
/// `prod` lists. The entry of the target wins over the one of `prod` with the same name. A source
/// without one is due, and the listing blocks it.
pub(crate) fn resolve(
    root: &Path,
    store: &Store,
    (name, target): (&str, &Target),
    (sources, read, producers): (&[Source], &BTreeSet<RequestKey>, &[&dyn Prepared]),
    inputs: &mut Inputs,
) -> Result<(), String> {
    let prepared = sources.iter().filter(|source| source.fetch.kind == FetchKind::Prepared);
    let prepared: Vec<&Source> = prepared.filter(|source| read.iter().any(|(id, _)| *id == source.id)).collect();
    if prepared.is_empty() {
        return Ok(());
    }
    let prod = match name {
        PROD => None,
        _ => crate::target::load(root, store)?.remove(PROD),
    };
    for source in prepared {
        let key = (source.id.clone(), Vec::new());
        let serving = inputs.serving.get(&key).cloned();
        let format = producer(producers, &source.id).map(|producer| producer.format());
        let mut listed = versions(target, &source.id)?;
        match prod.as_ref().map(|prod| versions(prod, &source.id)) {
            Some(Ok(more)) => {
                let own: BTreeSet<String> = listed.iter().map(|entry| entry.version.clone()).collect();
                listed.extend(more.into_iter().filter(|entry| !own.contains(&entry.version)));
            }
            // A bake from the store needs no network.
            Some(Err(e)) => eprintln!("obc data: `{PROD}` cannot be read ({e}); `{name}` reads the versions it knows"),
            None => {}
        }
        let fits = |entry: &Version| format.is_none_or(|format| entry.format == format);
        // A serving version that no list names stays, as on a machine without network.
        let serving = serving.filter(|serving| listed.iter().find(|entry| pinned(entry) == *serving).is_none_or(fits));
        let newest = listed.iter().filter(|entry| fits(entry)).max_by(|a, b| a.version.cmp(&b.version)).map(pinned);
        let date = |pinned: &String| unpin(pinned).map(|(version, _)| version.to_string());
        let manual = source.refresh == Refresh::Manual && inputs.serving.contains_key(&key);
        let chosen = match (newest, serving) {
            // A `manual` source keeps its serving version, unless that is of another format.
            (_, serving) if manual => serving,
            (Some(newest), Some(serving)) if date(&serving) > date(&newest) => Some(serving),
            (newest, serving) => newest.or(serving),
        };
        match chosen {
            Some(version) => {
                inputs.resolved.insert(key, version);
            }
            None => {
                inputs.due.insert(key);
            }
        }
    }
    Ok(())
}

/// Give `store` the manifest of the version of `request`, `<version>/<manifest sha256>`, and the
/// files that `names` names, from the first target of `data/targets.toml` whose manifest of the
/// version has that SHA-256, [`PROD`] first. A target that cannot be read is skipped. Each file must
/// have the SHA-256 that the manifest gives. No target needs a credential.
pub(crate) fn fetch(
    root: &Path,
    store: &Store,
    request: &crate::fetch::Request,
    names: &[String],
) -> Result<Snapshot, String> {
    let id = &request.source.id;
    let Some(pinned) = request.version.as_deref() else {
        return Err(format!("`{id}` is prepared: name its version, `{id}@<date>/<manifest sha256>`"));
    };
    let (version, sha256) =
        unpin(pinned).ok_or_else(|| format!("`{pinned}` is not a version of `{id}`: <date>/<manifest sha256>"))?;
    let mut targets: Vec<(String, Target)> = crate::target::load(root, store)?.into_iter().collect();
    targets.sort_by_key(|(name, _)| name.as_str() != PROD);
    let key = format!("{version}/{MANIFEST}");
    let (mut found, mut unread) = (None, Vec::new());
    for (name, target) in targets {
        let folder = folder(&target, id);
        match folder.get_public(&key) {
            Ok(Some(bytes)) if sha256_hex(&bytes) == sha256 => {
                found = Some((folder, bytes));
                break;
            }
            Ok(_) => {}
            Err(e) => unread.push(format!("`{name}` ({e})")),
        }
    }
    let (folder, bytes) = found.ok_or_else(|| {
        let unread = match unread.is_empty() {
            true => String::new(),
            false => format!("; these targets cannot be read: {}", unread.join(", ")),
        };
        format!("no target of data/targets.toml has {id}@{pinned}{unread}")
    })?;
    let manifest = Manifest::parse(id, version, &bytes)?;
    let retrieved = crate::date::timestamp(crate::date::now());
    let record = |name: &str, size: u64, sha256: String| FileRecord {
        name: name.into(),
        url: format!("sources/{id}/{version}/{name}"),
        size,
        sha256,
        retrieved: retrieved.clone(),
    };
    if !store.object(sha256).is_file() {
        let part = store.partial(&format!("{sha256}.{}.manifest", std::process::id()));
        crate::store::write_atomic(&part, &bytes)?;
        store.insert(&part, sha256)?;
    }
    let mut records = vec![record(MANIFEST, bytes.len() as u64, sha256.into())];
    for name in names.iter().filter(|name| *name != MANIFEST) {
        let file = manifest.files.iter().find(|file| &file.path == name);
        let file = file.ok_or_else(|| format!("{id}@{version} has no file {name}"))?;
        if !store.object(&file.sha256).is_file() {
            let downloaded = folder.download(&format!("{version}/{name}"), &file.sha256, store);
            downloaded
                .map_err(|e| format!("sources/{id}/{e}"))?
                .ok_or_else(|| format!("the target has no sources/{id}/{version}/{name}"))?;
        }
        records.push(record(name, file.size, file.sha256.clone()));
    }
    crate::fetch::record(store, id, pinned, &records)?;
    Ok(Snapshot { source: id.clone(), version: pinned.into(), files: records })
}

/// Refuse a step that reads a prepared source without names: its files are not all in the store.
pub(crate) fn named(steps: &[crate::engine::Step], sources: &[Source]) -> Result<(), String> {
    let prepared = |id: &str| sources.iter().any(|source| source.id == id && source.fetch.kind == FetchKind::Prepared);
    for step in steps {
        for input in &step.inputs {
            if let crate::engine::Input::Snapshot { source, files, .. } = input {
                if files.is_empty() && prepared(source) {
                    return Err(format!("step `{}` reads the prepared source `{source}` and names no file", step.name));
                }
            }
        }
    }
    Ok(())
}

/// The keys of the versions of `id` on `target` before `version` that have no manifest: a stopped
/// run left them, and no run of this machine resumes them.
pub(crate) fn incomplete(target: &Target, id: &str, version: &str) -> Result<Vec<String>, String> {
    let listed = target.list_under(&format!("sources/{id}"))?;
    let prefix = format!("sources/{id}/");
    let keys: BTreeSet<&str> = listed.iter().map(|listed| listed.key.as_str()).collect();
    let older = |key: &str| {
        let Some((name, _)) = key[prefix.len()..].split_once('/') else { return false };
        let complete = keys.contains(format!("{prefix}{name}/{MANIFEST}").as_str());
        crate::date::parse(name).is_some() && name < version && !complete
    };
    Ok(listed.iter().filter(|listed| older(&listed.key)).map(|listed| listed.key.clone()).collect())
}

/// A complete version on a target that a collection deletes.
pub(crate) struct Obsolete<'a> {
    pub id: String,
    pub version: String,
    /// Its keys, the manifest last.
    pub keys: Vec<&'a Listed>,
}

/// The complete versions of prepared sources in `listed`, the keys of a target, that no release of
/// `kept` reads while one reads a newer version of the source. The newest version that no release
/// reads and a version without a manifest stay. A prepared source is a folder `sources/<id>/` with
/// `versions.json`.
pub(crate) fn obsolete<'a>(listed: &'a BTreeMap<String, Listed>, kept: &[Release]) -> Vec<Obsolete<'a>> {
    let ids = listed.keys().filter_map(|key| key.strip_prefix("sources/")?.strip_suffix(&format!("/{VERSIONS}")));
    let mut obsolete = Vec::new();
    for id in ids {
        let reads = kept.iter().flat_map(|release| &release.outputs).flat_map(|output| &output.snapshots);
        let read: BTreeSet<&str> = reads
            .filter(|(source, _)| *source == id)
            .map(|(_, read)| unpin(&read.version).map_or(read.version.as_str(), |(version, _)| version))
            .collect();
        let Some(newest) = read.last() else { continue };
        let prefix = format!("sources/{id}/");
        let mut versions: BTreeMap<&str, Vec<&Listed>> = BTreeMap::new();
        for (key, file) in listed.range(prefix.clone()..).take_while(|(key, _)| key.starts_with(&prefix)) {
            let Some((version, _)) = key[prefix.len()..].split_once('/') else { continue };
            if crate::date::parse(version).is_some() {
                versions.entry(version).or_default().push(file);
            }
        }
        for (version, mut keys) in versions {
            let manifest = format!("{prefix}{version}/{MANIFEST}");
            if version >= *newest || read.contains(version) || !keys.iter().any(|file| file.key == manifest) {
                continue;
            }
            keys.sort_by_key(|file| file.key == manifest);
            obsolete.push(Obsolete { id: id.into(), version: version.into(), keys });
        }
    }
    obsolete
}

/// Remove `versions` of `id` from `versions.json` on `target`, so no reader takes one.
pub(crate) fn unlist(target: &Target, id: &str, versions: &[&str]) -> Result<(), String> {
    edit_versions(&folder(target, id), |listed| listed.retain(|entry| !versions.contains(&entry.version.as_str())))
}
