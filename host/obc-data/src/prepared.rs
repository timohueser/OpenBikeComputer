//! Prepared sources: a long job makes one version of a source in its home in the archive,
//! `prepared/<id>/<version>/` of one root of `OBC_DATA_ARCHIVE`, its manifest last. The archive is
//! the only copy. A bake reads a version as it reads any other source: the files that a step names,
//! each with the SHA-256 of the manifest. The manifest is the identity of a version: a step reads
//! `<version>/<manifest sha256>`, because a test root can have other files under one version name.
//! `specs/obc-data.md`, "Prepared sources", gives the layout.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::runs::Run;
use crate::engine::LayerFile;
use crate::fetch::http::Http;
use crate::product::{Inputs, RequestKey, Wanted};
use crate::sources::{FetchKind, Refresh, Source};
use crate::store::{hash_file, sha256_hex, FileRecord, Snapshot, Store};
use crate::target::{Condition, Target};

/// The folder of an archive root that holds the home of each prepared source.
const PREPARED: &str = "prepared";
/// The start of the error of a fetch whose version no root holds.
const ABSENT: &str = "no root of OBC_DATA_ARCHIVE has ";
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
    /// Plan the inputs with metadata requests only; never download an archive here.
    fn plan(
        &self,
        context: &Planning,
        source: &Source,
        scope: Option<&Scope>,
        languages: &[String],
    ) -> Result<Plan, String>;
    /// Validate and order the article languages of this version.
    fn languages(&self, requested: &[String]) -> Result<Vec<String>, String> {
        if requested.is_empty() {
            Ok(Vec::new())
        } else {
            Err("this source has no article languages".into())
        }
    }
    /// Make the files of the version of `job`: write each into `job.work`, then give it to
    /// [`Job::finish`]. A run that resumes calls it again; [`Job::done`] names what is done.
    fn prepare(&self, job: &mut Job) -> Result<(), String>;
}

/// What a prepare reads and makes, as its review shows it.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Plan {
    /// The sources that it reads: the fetches, and a source that a program of the producer reads,
    /// each with the size of its files when the producer knows it, such as a file on this machine.
    pub inputs: Vec<Input>,
    /// The files of the version, at most; `None` when the producer cannot tell before the run.
    pub files: Option<u64>,
}

/// Services for input discovery.
pub struct Planning<'a> {
    pub store: &'a Store,
    pub http: &'a Http,
    /// The rows of `data/sources.toml`.
    pub sources: &'a [Source],
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    pub wanted: Wanted,
    pub bytes: Option<u64>,
    /// A large archive read in place, outside the content-addressed object store.
    pub archive: Option<Archive>,
}

impl From<(Wanted, Option<u64>)> for Input {
    fn from((wanted, bytes): (Wanted, Option<u64>)) -> Self {
        Self { wanted, bytes, archive: None }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Archive {
    pub key: String,
    pub path: PathBuf,
    /// Absent for an archive supplied locally.
    pub url: Option<String>,
    /// The remote object identity; a mutable endpoint must supply a strong ETag.
    pub etag: Option<String>,
}

/// The producer of the source `id`.
pub(crate) fn producer<'a>(prepared: &[&'a dyn Prepared], id: &str) -> Option<&'a dyn Prepared> {
    prepared.iter().copied().find(|producer| producer.name() == id)
}

/// The home of the source `id` in the archive root `root`, as a target: its lock, its versions and
/// their files.
pub fn folder(root: &Path, id: &str) -> Target {
    Target::Dir(path(root, id))
}

/// The path of the home of `id` in the archive root `root`.
pub(crate) fn path(root: &Path, id: &str) -> PathBuf {
    root.join(PREPARED).join(id)
}

/// The root of the home of `id`: the first of `roots` that has `prepared/<id>/`.
pub(crate) fn home<'a>(roots: &'a [PathBuf], id: &str) -> Option<&'a PathBuf> {
    roots.iter().find(|root| path(root, id).is_dir())
}

/// The root that a prepare or a copy of `id` writes: its home, else the first root with
/// [`crate::store::RESERVE`] free. Refused without a root, and while a root does not exist.
pub(crate) fn place(roots: &[PathBuf], id: &str) -> Result<PathBuf, String> {
    if roots.is_empty() {
        return Err("OBC_DATA_ARCHIVE names no root, and a prepared version lives only in the archive".into());
    }
    crate::datasets::mounted(roots)?;
    if let Some(root) = home(roots, id) {
        return Ok(root.clone());
    }
    let roomy = roots.iter().find(|root| crate::store::check_free(root, 0).is_ok());
    roomy.cloned().ok_or_else(|| "no root of OBC_DATA_ARCHIVE has 4 GiB free".into())
}

/// Whether a fetch error is of a version that no root of the archive holds.
pub(crate) fn absent_from_archive(error: &str) -> bool {
    error.starts_with(ABSENT)
}

/// The part of the world that a version covers, as the settings of a target name a region.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub region: String,
    /// The TOML text of the region and of the members of a union, by id.
    pub definitions: BTreeMap<String, String>,
}

/// `prepared/<id>/<version>/manifest.json`: compact JSON with the keys of each object in byte order.
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
    /// The files that an earlier version of the same source holds, by path: the version keeps them
    /// under `<that version>/<path>`, not under its own folder.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reused: BTreeMap<String, String>,
    /// `null`: every place that the source covers.
    pub scope: Option<Scope>,
    /// The scopes of the earlier versions that this version extends, oldest first: it keeps their
    /// files outside its own scope.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<Option<Scope>>,
    /// Article languages included in this version; empty for a source without articles. A manifest
    /// of a version made before the field has none.
    #[serde(default)]
    pub languages: Vec<String>,
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
    /// The peak of the process of the run, or, when that is more, the peak of the largest program
    /// that it ran times the most programs that ran at once; `null` when the system does not
    /// report it.
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

/// An entry of `versions.json`, which lists the complete versions oldest first.
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
    /// The region of its scope; absent for a version of every place that the source covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl Manifest {
    pub fn canonical(&self) -> Vec<u8> {
        let value = serde_json::to_value(self).expect("a manifest serializes");
        serde_json::to_vec(&crate::engine::sorted(value)).expect("JSON values serialize")
    }

    pub(crate) fn parse(id: &str, version: &str, bytes: &[u8]) -> Result<Manifest, String> {
        let manifest: Manifest =
            serde_json::from_slice(bytes).map_err(|e| format!("{id}/{version}/{MANIFEST}: {e}"))?;
        if (manifest.source.as_str(), manifest.version.as_str()) != (id, version) {
            return Err(format!("{id}/{version}/{MANIFEST} is the manifest of another version"));
        }
        Ok(manifest)
    }
}

/// The complete versions of the source `id` whose folder is `folder`.
pub(crate) fn versions(folder: &Target, id: &str) -> Result<Vec<Version>, String> {
    let Some(bytes) = folder.get(VERSIONS)? else { return Ok(Vec::new()) };
    serde_json::from_slice(&bytes).map_err(|e| format!("{id}/{VERSIONS}: {e}"))
}

/// The manifest of `id@version` in `folder`; `None` while the version is not complete.
pub(crate) fn manifest(folder: &Target, id: &str, version: &str) -> Result<Option<Manifest>, String> {
    let bytes = folder.get(&format!("{version}/{MANIFEST}"))?;
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
        versions.sort_by_key(|entry| order(&entry.version));
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

/// A version as text shows it: a prepared version as `<version> (<first 12 hex digits of its
/// manifest SHA-256>)`. JSON gives the whole version.
pub fn show(version: &str) -> String {
    match unpin(version) {
        Some((name, sha256)) => format!("{name} ({})", &sha256[..12]),
        None => version.into(),
    }
}

/// The name and the manifest SHA-256 of the version `pinned` that a step reads.
pub(crate) fn unpin(pinned: &str) -> Option<(&str, &str)> {
    pinned.split_once('/').filter(|(name, sha256)| order(name).is_some() && crate::release::is_sha256(sha256))
}

/// The place of the version `name` in time, `None` when it is not the name of a version. The first
/// version of a UTC day is `YYYY-MM-DD`; each later one of that day adds `.2`, `.3` and so on.
pub(crate) fn order(name: &str) -> Option<(i64, u32)> {
    let (day, ordinal) = match name.split_once('.') {
        None => (name, 1),
        Some((day, ordinal)) => (day, ordinal.parse().ok().filter(|n: &u32| *n >= 2 && n.to_string() == ordinal)?),
    };
    Some((crate::date::parse(day)?, ordinal))
}

/// The day of the version `name`, `YYYY-MM-DD`.
pub fn day(name: &str) -> &str {
    name.split_once('.').map_or(name, |(day, _)| day)
}

/// The version that a new run on the UTC day `today` makes: the first name of that day after each
/// version of that day that `listed` names, without a manifest in `folder`.
pub(crate) fn next(folder: &Target, id: &str, today: &str, listed: &[Version]) -> Result<String, String> {
    let date = crate::date::parse(today).ok_or_else(|| format!("{today} is not a date"))?;
    let after = listed.iter().filter_map(|entry| order(&entry.version)).filter(|(of, _)| *of == date);
    let mut ordinal = after.map(|(_, ordinal)| ordinal + 1).max().unwrap_or(1);
    loop {
        let name = if ordinal == 1 { today.to_string() } else { format!("{today}.{ordinal}") };
        if manifest(folder, id, &name)?.is_none() {
            return Ok(name);
        }
        ordinal += 1;
    }
}

/// Make `manifest` the manifest of its version in `folder`, after a check that the folder has each
/// of its files with its size, then [`list`] the version. A manifest that an earlier run of the
/// version wrote stays when it names the same files.
pub fn complete(folder: &Target, manifest: &Manifest) -> Result<Version, String> {
    let (id, version) = (&manifest.source, &manifest.version);
    let key = format!("{version}/{MANIFEST}");
    if let Some(missing) = absent_kept(folder, version, &manifest.files, &manifest.reused)?.first() {
        return Err(format!("{id}/{missing} is not in the archive; no manifest is written"));
    }
    if folder.put(&key, &manifest.canonical(), &Condition::Absent)?.is_none() {
        let kept = self::manifest(folder, id, version)?.ok_or_else(|| format!("{id}/{key} came and went"))?;
        if kept.files != manifest.files {
            return Err(format!("{id}/{key} names other files than this run made"));
        }
    }
    list(folder, id, version)
}

/// The keys `<version>/<path>` of `files` that the source folder `folder` lacks or holds with
/// another size.
pub(crate) fn absent(folder: &Target, version: &str, files: &[LayerFile]) -> Result<Vec<String>, String> {
    absent_kept(folder, version, files, &BTreeMap::new())
}

/// [`absent`], where `reused` names the earlier version that holds a file.
fn absent_kept(
    folder: &Target,
    version: &str,
    files: &[LayerFile],
    reused: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let origin = |path: &str| reused.get(path).map_or(version, String::as_str);
    let sizes: Vec<(String, u64)> =
        files.iter().map(|file| (format!("{}/{}", origin(&file.path), file.path), file.size)).collect();
    folder.missing(&sizes)
}

/// Add the complete version `id@version` of `folder` to its `versions.json`, from its manifest.
pub(crate) fn list(folder: &Target, id: &str, version: &str) -> Result<Version, String> {
    let key = format!("{version}/{MANIFEST}");
    let bytes = folder.get(&key)?.ok_or_else(|| format!("{id}/{key} is gone"))?;
    let manifest = Manifest::parse(id, version, &bytes)?;
    let entry = Version {
        version: version.into(),
        manifest_sha256: sha256_hex(&bytes),
        format: manifest.format,
        bytes: manifest.files.iter().map(|file| file.size).sum(),
        time: manifest.time,
        region: manifest.scope.map(|scope| scope.region),
    };
    edit_versions(folder, |versions| {
        if !versions.iter().any(|listed| listed.version == entry.version) {
            versions.push(entry.clone());
        }
    })?;
    Ok(entry)
}

/// What a resume needs on this machine: `STORE/prepared/<id>/<version>/` holds the selection of the
/// run, its journal, `work/` and `state/`.
pub(crate) fn job_dir(store: &Store, id: &str, version: &str) -> PathBuf {
    store.root().join(PREPARED).join(id).join(version)
}

/// The version of `id` that a run of this machine started and did not complete; the newest one
/// when there are more.
pub(crate) fn started(store: &Store, id: &str) -> Result<Option<String>, String> {
    let dir = store.root().join(PREPARED).join(id);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let names = entries.filter_map(|entry| entry.ok()?.file_name().into_string().ok());
    Ok(names.filter(|name| order(name).is_some()).max_by_key(|name| order(name)))
}

/// A line of the journal of a run.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Entry {
    /// A file that is in the home.
    File(LayerFile),
    /// A file of the earlier version `version` that this version keeps.
    Reused { file: LayerFile, version: String },
    /// The files of a source version that the prepare read, name to SHA-256.
    Input { source: String, version: String, files: BTreeMap<String, String> },
}

/// What a producer works with: one version of one source in its home.
pub struct Job<'a> {
    pub id: String,
    pub version: String,
    pub scope: Option<Scope>,
    pub languages: Vec<String>,
    pub plan: Plan,
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
    /// The files of `files` that an earlier version holds, with that version.
    reused: BTreeMap<String, String>,
    extends: Vec<Option<Scope>>,
    inputs: BTreeMap<(String, String), BTreeMap<String, String>>,
    /// The files that this run moved into the home.
    pub(crate) uploaded: u64,
    /// The programs that run now, and the most that ran at once.
    programs: (Arc<AtomicU64>, AtomicU64),
}

/// What [`Job::open`] needs besides the run.
pub struct Opening<'a> {
    pub root: &'a Path,
    pub store: &'a Store,
    pub http: &'a Http,
    pub sources: &'a [Source],
    /// The archive root of the home of the source.
    pub home: &'a Path,
}

impl<'a> Job<'a> {
    /// The job of `id@version` with `scope`, with what earlier runs of it did. A run of the version
    /// for another scope or language selection refuses.
    pub fn open(
        opening: Opening<'a>,
        (id, version): (&str, &str),
        scope: Option<Scope>,
        languages: &[String],
        run: &'a mut Run,
    ) -> Result<Job<'a>, String> {
        let Opening { root, store, http, sources, home } = opening;
        let source = sources.iter().find(|source| source.id == id);
        let source = source.ok_or_else(|| format!("no source `{id}` in data/sources.toml"))?;
        let dir = job_dir(store, id, version);
        for folder in ["work", "state"] {
            fs::create_dir_all(dir.join(folder)).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let selection_file = dir.join("selection.json");
        let bytes = serde_json::to_vec(&(&scope, languages)).expect("a selection serializes");
        match fs::read(&selection_file) {
            Ok(held) if held == bytes => {}
            Ok(_) => {
                return Err(format!(
                    "a run of {id}@{version} for another region or language selection stopped; resume it with its selection, or delete {}",
                    dir.display()
                ))
            }
            Err(_) => crate::store::write_atomic(&selection_file, &bytes)?,
        }
        let mut job = Job {
            id: id.into(),
            version: version.into(),
            scope,
            languages: languages.to_vec(),
            plan: Plan::default(),
            source,
            work: dir.join("work"),
            state: dir.join("state"),
            root,
            store,
            http,
            sources,
            folder: folder(home, id),
            run,
            journal: dir.join("journal.jsonl"),
            files: BTreeMap::new(),
            reused: BTreeMap::new(),
            extends: Vec::new(),
            inputs: BTreeMap::new(),
            uploaded: 0,
            programs: Default::default(),
        };
        let text = fs::read_to_string(&job.journal).unwrap_or_default();
        // A run that stopped in a write leaves a part of its last line.
        for entry in text.lines().filter_map(|line| serde_json::from_str::<Entry>(line).ok()) {
            job.apply(entry);
        }
        // The cleanup of a newer version can delete the keys of the journal: the run makes them again.
        let files: Vec<LayerFile> = job.files.values().cloned().collect();
        for key in absent_kept(&job.folder, version, &files, &job.reused)? {
            let path = key.split_once('/').map_or(key.as_str(), |(_, path)| path).to_string();
            job.files.remove(&path);
            job.reused.remove(&path);
        }
        Ok(job)
    }

    pub fn planning(&self) -> Planning<'_> {
        Planning { store: self.store, http: self.http, sources: self.sources }
    }

    fn apply(&mut self, entry: Entry) {
        match entry {
            Entry::File(file) => {
                self.reused.remove(&file.path);
                self.files.insert(file.path.clone(), file);
            }
            Entry::Reused { file, version } => {
                self.reused.insert(file.path.clone(), version);
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

    /// Whether `path` is in the home: a resume does not make it again.
    pub fn done(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    /// The files that are in the home, by path.
    pub fn files(&self) -> impl Iterator<Item = &LayerFile> {
        self.files.values()
    }

    /// Show `steps`, the pieces that the programs of the producer make, such as the tiles of a
    /// version, as the steps of a layer of the run named after the source, with their progress.
    pub fn plan_steps(&mut self, steps: &[String]) -> Result<(), String> {
        self.run.plan_work(&self.id, steps)
    }

    /// Record that the program of the step `step` of [`Job::plan_steps`] started.
    pub fn step_started(&mut self, step: &str) -> Result<(), String> {
        self.run.record(&crate::engine::runs::Event::StepStarted { step: step.into() })
    }

    /// Record that the step `step` ended: what its program used, and the bytes of its files.
    pub fn step_finished(&mut self, step: &str, usage: &crate::engine::Usage, bytes_out: u64) -> Result<(), String> {
        let receipt = crate::engine::Receipt {
            layer: self.id.clone(),
            step: step.into(),
            format: 0,
            revision: 0,
            key: String::new(),
            inputs: Vec::new(),
            options: serde_json::Value::Null,
            digest: String::new(),
            files: Vec::new(),
            built: crate::date::timestamp(crate::date::now()),
            wall_ms: usage.wall_ms,
            cpu_ms: usage.cpu_ms,
            peak_rss_bytes: usage.peak_rss_bytes,
            own_process: true,
            bytes_in: 0,
            bytes_out,
            metrics: BTreeMap::new(),
        };
        let event = crate::engine::runs::Event::StepFinished {
            step: step.into(),
            reused: false,
            unchanged: false,
            receipt: Box::new(receipt),
        };
        self.run.record(&event)
    }

    /// Record that the step `step` failed.
    pub fn step_failed(&mut self, step: &str, error: &str) -> Result<(), String> {
        self.run.record(&crate::engine::runs::Event::StepFailed { step: step.into(), error: error.into() })
    }

    /// The rows of `data/sources.toml`.
    pub fn sources(&self) -> &'a [Source] {
        self.sources
    }

    /// The files that an earlier version holds, with that version.
    pub fn reused(&self) -> &BTreeMap<String, String> {
        &self.reused
    }

    /// The scopes of the earlier versions that this version extends.
    pub fn extends(&self) -> &[Option<Scope>] {
        &self.extends
    }

    /// The newest complete version of the source in the home before this one, in `format`: the
    /// version that this one extends, so its manifest names the scopes of that version.
    pub fn extend(&mut self, format: u32) -> Result<Option<Manifest>, String> {
        let listed: Vec<Version> = match self.folder.get(VERSIONS)? {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}/{VERSIONS}: {e}", self.id))?,
            None => Vec::new(),
        };
        let before =
            listed.iter().filter(|entry| entry.format == format && order(&entry.version) < order(&self.version));
        let Some(entry) = before.max_by_key(|entry| order(&entry.version)) else { return Ok(None) };
        let key = format!("{}/{MANIFEST}", entry.version);
        let bytes = self.folder.get(&key)?.ok_or_else(|| format!("{}/{key} is gone", self.id))?;
        let manifest = Manifest::parse(&self.id, &entry.version, &bytes)?;
        self.extends = manifest.extends.iter().cloned().chain([manifest.scope.clone()]).collect();
        Ok(Some(manifest))
    }

    /// The file `path` of the earlier version `manifest`, in the store.
    pub fn earlier_file(&self, manifest: &Manifest, path: &str) -> Result<PathBuf, String> {
        let file = manifest.files.iter().find(|file| file.path == path);
        let file = file.ok_or_else(|| format!("{}@{} has no file {path}", self.id, manifest.version))?;
        let origin = manifest.reused.get(path).unwrap_or(&manifest.version);
        take(&self.folder, (&self.id, &format!("{origin}/{path}")), &file.sha256, self.store)
    }

    /// Keep the file `path` of the earlier version `manifest` in this version, without an upload.
    pub fn reuse(&mut self, manifest: &Manifest, path: &str) -> Result<(), String> {
        if self.files.contains_key(path) {
            return Ok(());
        }
        let file = manifest.files.iter().find(|file| file.path == path);
        let file = file.ok_or_else(|| format!("{}@{} has no file {path}", self.id, manifest.version))?.clone();
        let version = manifest.reused.get(path).unwrap_or(&manifest.version).clone();
        self.record(Entry::Reused { file, version })
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

    /// Run `command`, a program of the producer, in the repository root, and wait for it. Its
    /// error names the program, the arguments before the first option, and its last error line.
    pub fn program(&self, command: &mut Command) -> Result<(), String> {
        self.start(command, None)?.wait()
    }

    /// Start `command` as [`Job::program`] does, and give it without a wait. With `label`, each line
    /// of its output goes to standard error after `label`, and its error starts with `label`.
    pub fn start(&self, command: &mut Command, label: Option<&str>) -> Result<Program, String> {
        self.run.check_stop(self.store)?;
        // The program stops when this process does, so no orphan writes into a later run.
        command.current_dir(self.root).env("OBC_PARENT_PID", std::process::id().to_string());
        let program = Path::new(command.get_program()).file_name().unwrap_or_default().to_string_lossy();
        let args = command.get_args().map(|arg| arg.to_string_lossy()).take_while(|arg| !arg.starts_with("--"));
        let name = std::iter::once(program).chain(args).collect::<Vec<_>>().join(" ");
        if label.is_some() {
            command.stdout(std::process::Stdio::piped());
        }
        let mut child =
            command.stderr(std::process::Stdio::piped()).spawn().map_err(|e| format!("cannot start `{name}`: {e}"))?;
        // The copy of the output ends with the program, which `Program::wait` waits for.
        let _ = crate::engine::copy_lines(child.stdout.take(), label.unwrap_or_default());
        let errors = Some(crate::engine::copy_lines(child.stderr.take(), label.unwrap_or_default()));
        let (running, most) = &self.programs;
        most.fetch_max(running.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
        let (label, running) = (label.map(String::from), Arc::clone(running));
        Ok(Program { child, name, label, errors, running, started: Instant::now(), ended: None })
    }

    /// The peak memory of the run: of this process, or of the most programs that ran at once.
    pub(crate) fn peak_rss_bytes(&self) -> Option<u64> {
        crate::engine::peak_rss(self.programs.1.load(Ordering::Relaxed).max(1))
    }

    /// Move the files `paths` of `work` into the home, record each, and delete them from `work`.
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
                return Err(format!("{path} has other bytes than the {} bytes that a run moved", done.size));
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

/// A program of a producer that [`Job::start`] started. A drop before [`Program::wait`] sends it
/// SIGTERM and waits for its end, so it writes nothing after the producer returns.
pub struct Program {
    child: std::process::Child,
    name: String,
    label: Option<String>,
    /// `None` once the wait gave the status.
    errors: Option<std::sync::mpsc::Receiver<String>>,
    /// The programs of the job that run.
    running: Arc<AtomicU64>,
    started: Instant,
    /// The end that [`Program::failed`] saw: the status, CPU and peak memory, and the wall time.
    ended: Option<(std::process::ExitStatus, Option<u64>, Option<u64>, u64)>,
}

impl Program {
    /// Wait for the program; its error is that of [`Job::program`].
    pub fn wait(self) -> Result<(), String> {
        self.wait_usage().map(|_| ())
    }

    /// [`Program::wait`], and the time, CPU and peak memory of the program.
    pub fn wait_usage(mut self) -> Result<crate::engine::Usage, String> {
        let errors = self.errors.take().expect("a program is waited for once");
        let waited = self.reap();
        let label = |e: String| match &self.label {
            Some(label) => format!("{label}: {e}"),
            None => e,
        };
        let (status, cpu_ms, peak_rss_bytes, wall_ms) = waited.map_err(label)?;
        crate::engine::ended(&self.name, status, errors).map_err(label)?;
        Ok(crate::engine::Usage { wall_ms, cpu_ms, peak_rss_bytes })
    }

    fn reap(&mut self) -> Result<(std::process::ExitStatus, Option<u64>, Option<u64>, u64), String> {
        if let Some(ended) = self.ended {
            return Ok(ended);
        }
        #[cfg(unix)]
        let (status, cpu_ms, peak) = crate::engine::wait_pid(self.child.id(), &AtomicBool::new(false))?;
        #[cfg(not(unix))]
        let (status, cpu_ms, peak) =
            (self.child.wait().map_err(|e| format!("wait for `{}`: {e}", self.name))?, None, None);
        Ok((status, cpu_ms, peak, self.started.elapsed().as_millis() as u64))
    }

    /// Whether the program ended and failed. The end keeps what the program used and when it ended.
    pub fn failed(&mut self) -> bool {
        if self.ended.is_none() {
            #[cfg(unix)]
            let end = crate::engine::try_wait_pid(self.child.id()).ok().flatten();
            #[cfg(not(unix))]
            let end = self.child.try_wait().ok().flatten().map(|status| (status, None, None));
            let wall = self.started.elapsed().as_millis() as u64;
            self.ended = end.map(|(status, cpu_ms, peak)| (status, cpu_ms, peak, wall));
        }
        self.ended.is_some_and(|(status, ..)| !status.success())
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        self.running.fetch_sub(1, Ordering::Relaxed);
        if self.errors.is_none() {
            return;
        }
        // SIGTERM, not SIGKILL, so that the program can remove what it wrote. `try_wait` reaps a
        // program that ended, whose pid can then be another's: it gets no signal.
        if let Ok(None) = self.child.try_wait() {
            #[cfg(unix)]
            // SAFETY: kill sends a signal. The child is not reaped, so its pid is still its own.
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
            }
            #[cfg(not(unix))]
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Delete what this machine keeps of the run of `id@version`: the home has it all.
pub(crate) fn forget(store: &Store, id: &str, version: &str) -> Result<(), String> {
    let dir = job_dir(store, id, version);
    fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// The files of `id@version` that the journal of its run on this machine names, sorted by path: a
/// resume does not move them again.
pub(crate) fn journaled(store: &Store, id: &str, version: &str) -> Vec<LayerFile> {
    let journal = job_dir(store, id, version).join("journal.jsonl");
    let text = fs::read_to_string(journal).unwrap_or_default();
    let mut files = BTreeMap::new();
    for entry in text.lines().filter_map(|line| serde_json::from_str::<Entry>(line).ok()) {
        if let Entry::File(file) = entry {
            files.insert(file.path.clone(), file);
        }
    }
    files.into_values().collect()
}

/// Give `inputs` the version of each prepared source of `read`, the fetches that the steps read, in
/// the format of its producer: the serving version of a `manual` source, else the newest version
/// of its home in the archive roots `roots`. A source without one is due, and the listing blocks it.
pub(crate) fn resolve(
    roots: &[PathBuf],
    (sources, read, producers): (&[Source], &BTreeSet<RequestKey>, &[&dyn Prepared]),
    inputs: &mut Inputs,
) -> Result<(), String> {
    let prepared = sources.iter().filter(|source| source.fetch.kind == FetchKind::Prepared);
    for source in prepared.filter(|source| read.iter().any(|(id, _)| *id == source.id)) {
        let key = (source.id.clone(), Vec::new());
        let serving = inputs.serving.get(&key).cloned();
        let format = producer(producers, &source.id).map(|producer| producer.format());
        let fits = |entry: &Version| format.is_none_or(|format| entry.format == format);
        let listed = match home(roots, &source.id) {
            Some(root) => versions(&folder(root, &source.id), &source.id)?,
            // A bake from the store needs no archive.
            None => {
                if serving.is_some() {
                    eprintln!(
                        "obc data: no root of OBC_DATA_ARCHIVE has {PREPARED}/{}/: the target keeps the version that it serves",
                        source.id
                    );
                }
                Vec::new()
            }
        };
        // A serving version that the home does not list stays, as on a machine without the archive.
        let serving = serving.filter(|serving| listed.iter().find(|entry| pinned(entry) == *serving).is_none_or(fits));
        let newest = listed.iter().filter(|entry| fits(entry)).max_by_key(|entry| order(&entry.version)).map(pinned);
        let place = |pinned: &String| unpin(pinned).and_then(|(version, _)| order(version));
        let manual = source.refresh == Refresh::Manual && inputs.serving.contains_key(&key);
        let chosen = match (newest, serving) {
            // A `manual` source keeps its serving version, unless that is of another format.
            (_, serving) if manual => serving,
            (Some(newest), Some(serving)) if place(&serving) > place(&newest) => Some(serving),
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

/// Give `store` a copy, never a link, of the file `key` of the home `folder` of `id`, which must
/// have the SHA-256 `sha256`, and give its object.
fn take(folder: &Target, (id, key): (&str, &str), sha256: &str, store: &Store) -> Result<PathBuf, String> {
    let object = store.object(sha256);
    if object.is_file() {
        return Ok(object);
    }
    let Target::Dir(dir) = folder else { return Err(format!("{id} has no home in the archive")) };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let part =
        store.partial(&format!("{sha256}.{}.{}.prepared", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let parent = part.parent().expect("a partial file has a folder");
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    fs::copy(dir.join(key), &part).map_err(|e| format!("{id}/{key}: {e}"))?;
    let (held, _) = hash_file(&part)?;
    if held != sha256 {
        let _ = fs::remove_file(&part);
        return Err(format!("{id}/{key} holds bytes with the SHA-256 {held}, not {sha256}"));
    }
    store.insert(&part, sha256)
}

/// Give `store` the manifest of the version of `request`, `<version>/<manifest sha256>`, and the
/// files that `names` names, from the first of the archive roots `roots` whose home of the source
/// has a manifest of the version with that SHA-256. Each file must have the SHA-256 that the
/// manifest gives.
pub(crate) fn fetch(
    roots: &[PathBuf],
    store: &Store,
    request: &crate::fetch::Request,
    names: &[String],
) -> Result<Snapshot, String> {
    let id = &request.source.id;
    let Some(pinned) = request.version.as_deref() else {
        return Err(format!("`{id}` is prepared: name its version, `{id}@<version>/<manifest sha256>`"));
    };
    let (version, sha256) =
        unpin(pinned).ok_or_else(|| format!("`{pinned}` is not a version of `{id}`: <version>/<manifest sha256>"))?;
    let key = format!("{version}/{MANIFEST}");
    let mut found = None;
    for root in roots {
        let folder = folder(root, id);
        if let Some(bytes) = folder.get(&key)?.filter(|bytes| sha256_hex(bytes) == sha256) {
            found = Some((folder, bytes));
            break;
        }
    }
    let (folder, bytes) = found.ok_or_else(|| format!("{ABSENT}{id}@{}", show(pinned)))?;
    let manifest = Manifest::parse(id, version, &bytes)?;
    let retrieved = crate::date::timestamp(crate::date::now());
    let record = |name: &str, origin: &str, size: u64, sha256: String| FileRecord {
        name: name.into(),
        url: format!("{PREPARED}/{id}/{origin}/{name}"),
        size,
        sha256,
        retrieved: retrieved.clone(),
        archive: None,
    };
    if !store.object(sha256).is_file() {
        let part = store.partial(&format!("{sha256}.{}.manifest", std::process::id()));
        crate::store::write_atomic(&part, &bytes)?;
        store.insert(&part, sha256)?;
    }
    let mut records = vec![record(MANIFEST, version, bytes.len() as u64, sha256.into())];
    for name in names.iter().filter(|name| *name != MANIFEST) {
        let file = manifest.files.iter().find(|file| &file.path == name);
        let file = file.ok_or_else(|| format!("{id}@{version} has no file {name}"))?;
        let origin = manifest.reused.get(name).map_or(version, String::as_str);
        take(&folder, (id, &format!("{origin}/{name}")), &file.sha256, store)?;
        records.push(record(name, origin, file.size, file.sha256.clone()));
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

/// The keys of `folder`, the home of a source, of the versions before `version` that have no
/// manifest: a stopped run left them, and no run of this machine resumes them.
pub(crate) fn incomplete(folder: &Target, version: &str) -> Result<Vec<String>, String> {
    let listed = folder.list()?;
    let keys: BTreeSet<&str> = listed.iter().map(|listed| listed.key.as_str()).collect();
    let older = |key: &str| {
        let Some((name, _)) = key.split_once('/') else { return false };
        let complete = keys.contains(format!("{name}/{MANIFEST}").as_str());
        order(name).is_some() && order(name) < order(version) && !complete
    };
    Ok(listed.iter().filter(|listed| older(&listed.key)).map(|listed| listed.key.clone()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_of_one_day_follow_their_ordinal_and_a_plain_date_stays_a_version() {
        let names = ["2026-10-28", "2026-10-28.2", "2026-10-28.10", "2026-10-29"];
        let places: Vec<_> = names.iter().map(|name| order(name).unwrap()).collect();
        assert!(places.windows(2).all(|pair| pair[0] < pair[1]), "{places:?}");
        for name in ["2026-10-28.1", "2026-10-28.02", "2026-10-28.+2", "2026-10-28.", "2026-02-30"] {
            assert_eq!(order(name), None, "{name}");
        }
        let sha256 = "0".repeat(64);
        assert_eq!(unpin(&format!("2026-10-28/{sha256}")), Some(("2026-10-28", sha256.as_str())));
        assert_eq!(unpin(&format!("2026-10-28.2/{sha256}")), Some(("2026-10-28.2", sha256.as_str())));
    }
}
