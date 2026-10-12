//! A release: what a target serves, in one manifest. It names the settings of the target, every
//! output with its files, the client pointers and the files that a client finds by name. Its id is
//! the SHA-256 of its bytes. `release.json` of the target names the release that it serves and one
//! rollback: a switch writes it first, so it is the commit point.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::{digest, sorted, Client, Input, InputRecord, LayerFile, Receipt, Step};
use crate::settings::Settings;
use crate::store::{hash_file, sha256_hex, Store};
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

    /// The keys of the release but its manifest: the objects of the client files, then the named
    /// files.
    pub fn files(&self) -> Vec<File> {
        let objects = self.objects().into_iter().map(|(sha256, size)| File {
            key: format!("objects/{sha256}"),
            size,
            sha256: sha256.into(),
        });
        objects.chain(self.named.iter().cloned()).collect()
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
            crate::r2::check_key(&file.key)?;
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
        (&self.settings, &self.pointers, &self.named) == (&other.settings, &other.pointers, &other.named)
            && self.outputs.len() == other.outputs.len()
            && self.outputs.iter().zip(&other.outputs).all(|(a, b)| same(a, b))
    }

    /// The release `id` of `target`.
    pub fn read(target: &Target, id: &str) -> Result<Release, String> {
        let key = manifest_key(id);
        let bytes = target.get(&key)?.ok_or_else(|| format!("the target has no {key}"))?;
        if sha256_hex(&bytes) != id {
            return Err(format!("{key}: its SHA-256 is not its id"));
        }
        let release: Release = serde_json::from_slice(&bytes).map_err(|e| format!("{key}: {e}"))?;
        release.check().map_err(|e| format!("{key}: {e}"))?;
        Ok(release)
    }
}

pub fn manifest_key(id: &str) -> String {
    format!("releases/{id}.json")
}

fn is_sha256(text: &str) -> bool {
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
        Some((now, _)) if now.serving.id == id => now.clone(),
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
        assert!(switch(&target, Some(&back), &id).unwrap(), "a switch to the serving release repairs it");
        assert!(drift(&target, &first).unwrap().is_empty());
        assert_eq!(serving(&target).unwrap().unwrap().0, back.0);

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

        let mut rebuilt = Release { machine: "desktop".into(), ..valid.clone() };
        rebuilt.outputs[0].cost.wall_ms += 1;
        assert!(rebuilt.serves_as(&valid), "a rebuild of the same bytes serves as the release");
        rebuilt.outputs[0].key = "other".into();
        assert!(!rebuilt.serves_as(&valid));
    }
}
