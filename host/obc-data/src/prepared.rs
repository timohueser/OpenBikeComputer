//! Prepared sources: a long job publishes one version of a source to a target, under
//! `sources/<id>/<version>/`, its manifest last. A bake reads a version as it reads any other
//! source: the files that a step names, each with the SHA-256 of the manifest.
//! `specs/obc-data.md`, "Prepared sources", gives the layout.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    /// The fetches that a prepare of `id` reads, for its review.
    fn inputs(&self, id: &str, scope: Option<&Scope>) -> Result<Vec<Wanted>, String>;
    /// Make the files of the version of `job`: write each into `job.work`, then give it to
    /// [`Job::finish`]. A run that resumes calls it again; [`Job::done`] names what is done.
    fn prepare(&self, job: &mut Job) -> Result<(), String>;
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    pub version: String,
    pub manifest_sha256: String,
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

/// Make `manifest` the manifest of its version on `target`, then list the version in
/// `versions.json`. A manifest that an earlier run of the version wrote stays when it names the same
/// files.
pub(crate) fn complete(target: &Target, manifest: &Manifest) -> Result<Version, String> {
    let (id, version) = (&manifest.source, &manifest.version);
    let (folder, key) = (folder(target, id), format!("{version}/{MANIFEST}"));
    let mut bytes = manifest.canonical();
    if folder.put(&key, &bytes, &Condition::Absent)?.is_none() {
        bytes = folder.get(&key)?.ok_or_else(|| format!("sources/{id}/{key} came and went"))?;
        if Manifest::parse(id, version, &bytes)?.files != manifest.files {
            return Err(format!("sources/{id}/{key} names other files than this run made"));
        }
    }
    let entry = Version {
        version: version.clone(),
        manifest_sha256: sha256_hex(&bytes),
        bytes: manifest.files.iter().map(|file| file.size).sum(),
        time: manifest.time.clone(),
    };
    edit_versions(&folder, |versions| {
        if !versions.iter().any(|listed| listed.version == entry.version) {
            versions.push(entry.clone());
        }
    })?;
    Ok(entry)
}

/// What a resume needs on this machine: `STORE/prepared/<target>/<id>/<version>/` holds the scope
/// of the run, its journal and `work/`.
fn job_dir(store: &Store, target: &str, id: &str, version: &str) -> PathBuf {
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
    /// Where the producer writes a file before [`Job::finish`]. It stays when the run stops.
    pub work: PathBuf,
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
pub(crate) struct Opening<'a> {
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
    pub(crate) fn open(
        opening: Opening<'a>,
        (id, version): (&str, &str),
        scope: Option<Scope>,
        run: &'a mut Run,
    ) -> Result<Job<'a>, String> {
        let Opening { root, store, http, sources, target: (name, target) } = opening;
        let dir = job_dir(store, name, id, version);
        fs::create_dir_all(dir.join("work")).map_err(|e| format!("{}: {e}", dir.display()))?;
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
            work: dir.join("work"),
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
    pub(crate) fn files(&self) -> impl Iterator<Item = &LayerFile> {
        self.files.values()
    }

    /// Fetch `wanted` into the store, and give the object of each file by name. The manifest names
    /// the version that it read.
    pub fn fetch(&mut self, wanted: &Wanted) -> Result<BTreeMap<String, PathBuf>, String> {
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
        self.record(Entry::Input { source: snapshot.source.clone(), version: snapshot.version.clone(), files })?;
        Ok(snapshot.files.into_iter().map(|file| (file.name, self.store.object(&file.sha256))).collect())
    }

    /// Upload the file `path` of `work` to the target, record it, and delete it from `work`.
    pub fn finish(&mut self, path: &str) -> Result<(), String> {
        self.run.check_stop(self.store)?;
        let relative = path.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
        if !relative || path == MANIFEST || path.contains('\\') {
            return Err(format!("{path} is not the path of a file of a version"));
        }
        let file = self.work.join(path);
        let (sha256, size) = hash_file(&file)?;
        if let Some(done) = self.files.get(path).filter(|done| done.sha256 != sha256) {
            return Err(format!("{path} has other bytes than the {} bytes that a run uploaded", done.size));
        }
        let key = format!("{}/{path}", self.version);
        self.folder.upload(&[(file.clone(), key.clone())])?;
        // A key is immutable: one with another size is refused here.
        if !self.folder.missing(&[(key.clone(), size)])?.is_empty() {
            return Err(format!("sources/{}/{key} is not on the target after the upload", self.id));
        }
        if !self.done(path) {
            self.record(Entry::File(LayerFile { path: path.into(), size, sha256 }))?;
            self.uploaded += 1;
        }
        fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))
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

/// The number and the size of the files of `id@version` that the journal of its run on this machine
/// names: a resume does not upload them again.
pub(crate) fn resumed(store: &Store, target: &str, id: &str, version: &str) -> (u64, u64) {
    let journal = job_dir(store, target, id, version).join("journal.jsonl");
    let text = fs::read_to_string(journal).unwrap_or_default();
    let mut files = BTreeMap::new();
    for entry in text.lines().filter_map(|line| serde_json::from_str::<Entry>(line).ok()) {
        if let Entry::File(file) = entry {
            files.insert(file.path, file.size);
        }
    }
    (files.len() as u64, files.values().sum())
}

/// Give `inputs` the version of each prepared source of `read`, the fetches that the steps of the
/// target `name` read: the serving version of a `manual` source, else the newest version that the
/// target or [`PROD`] lists. A source without one is due, and its fetch says how to prepare it.
pub(crate) fn resolve(
    root: &Path,
    store: &Store,
    (name, target): (&str, &Target),
    (sources, read): (&[Source], &BTreeSet<RequestKey>),
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
        let mut known: Vec<String> = serving.iter().cloned().collect();
        if source.refresh != Refresh::Manual || serving.is_none() {
            known.extend(versions(target, &source.id)?.into_iter().map(|listed| listed.version));
            match prod.as_ref().map(|prod| versions(prod, &source.id)) {
                Some(Ok(listed)) => known.extend(listed.into_iter().map(|listed| listed.version)),
                // A bake from the store needs no network.
                Some(Err(e)) => {
                    eprintln!("obc data: `{PROD}` cannot be read ({e}); `{name}` reads the versions it knows")
                }
                None => {}
            }
        }
        match known.into_iter().max() {
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

/// Give `store` the manifest of the version of `request` and the files that `names` names, from the
/// first target of `data/targets.toml` that has the complete version, [`PROD`] first. Each file
/// must have the SHA-256 that the manifest gives. No target needs a credential.
pub(crate) fn fetch(
    root: &Path,
    store: &Store,
    request: &crate::fetch::Request,
    names: &[String],
) -> Result<Snapshot, String> {
    let id = &request.source.id;
    let version = request.version.as_ref().ok_or_else(|| {
        format!("no target lists a version of `{id}`. Fix: prepare one with `obc data source prepare {id}`")
    })?;
    let mut targets: Vec<(String, Target)> = crate::target::load(root, store)?.into_iter().collect();
    targets.sort_by_key(|(name, _)| name.as_str() != PROD);
    let key = format!("{version}/{MANIFEST}");
    let mut found = None;
    for (_, target) in targets {
        let folder = folder(&target, id);
        if let Some(bytes) = folder.get_public(&key)? {
            found = Some((folder, bytes));
            break;
        }
    }
    let (folder, bytes) = found.ok_or_else(|| format!("no target of data/targets.toml has {id}@{version}"))?;
    let manifest = Manifest::parse(id, version, &bytes)?;
    let retrieved = crate::date::timestamp(crate::date::now());
    let record = |name: &str, size: u64, sha256: String| FileRecord {
        name: name.into(),
        url: format!("sources/{id}/{version}/{name}"),
        size,
        sha256,
        retrieved: retrieved.clone(),
    };
    let sha256 = sha256_hex(&bytes);
    if !store.object(&sha256).is_file() {
        let part = store.partial(&format!("{sha256}.{}.manifest", std::process::id()));
        crate::store::write_atomic(&part, &bytes)?;
        store.insert(&part, &sha256)?;
    }
    let mut records = vec![record(MANIFEST, bytes.len() as u64, sha256)];
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
    crate::fetch::record(store, id, version, &records)?;
    Ok(Snapshot { source: id.clone(), version: version.clone(), files: records })
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
        let read: BTreeSet<&str> =
            reads.filter(|(source, _)| *source == id).map(|(_, read)| read.version.as_str()).collect();
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
