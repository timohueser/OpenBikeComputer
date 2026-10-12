//! A release: what a target serves, in one manifest. It names the settings of the target, every
//! output with its files, the client pointers and the files that a client finds by name. Its id is
//! the SHA-256 of its bytes. `release.json` of the target names the release that it serves and one
//! rollback: a switch writes it first, so it is the commit point.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine::release::SnapshotRead;
use crate::engine::{sorted, Client, Input, InputRecord, LayerFile, Receipt, Step};
use crate::settings::Settings;
use crate::store::{hash_file, sha256_hex, Store};
use crate::target::{Condition, Tag, Target};

/// The key that names the release that a target serves, and its rollback.
pub const SERVING: &str = "release.json";

/// Keys that a pointer cannot have: the keys of releases, objects and sources, and the control
/// files of a target.
const RESERVED: [&str; 3] = ["objects/", "releases/", "sources/"];
const CONTROL: [&str; 3] = [SERVING, "lock.json", crate::r2::REMOVAL_LOG];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub settings: Settings,
    /// Sorted by `step`, each step once.
    pub outputs: Vec<Output>,
    /// What a client reads at each pointer key: the bytes of `objects/<sha256>`. Sorted by key.
    pub pointers: Vec<File>,
    /// Files that a client finds by name, at `releases/<folder>/<path>`. The folder is the SHA-256
    /// of one of its files, which names the others. Sorted by key.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub wall_ms: u64,
    pub peak_rss_bytes: Option<u64>,
    pub bytes_out: u64,
}

/// `release.json`: the release that a target serves, and the one that a rollback switches to.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Serving {
    pub serving: Role,
    pub rollback: Option<Role>,
}

/// A release and the time it took its role, `YYYY-MM-DDTHH:MM:SSZ`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
            Input::Layer { .. } => None,
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
            client: step.client.clone(),
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

    /// Every key of the release: the objects of the client files and of the pointers, the named
    /// files, and last the manifest, which names the keys before it.
    pub fn files(&self, id: &str) -> Vec<File> {
        let client = self.outputs.iter().flat_map(Output::client_files).map(|file| (&file.sha256, file.size));
        let pointers = self.pointers.iter().map(|pointer| (&pointer.sha256, pointer.size));
        let objects: BTreeMap<&String, u64> = client.chain(pointers).collect();
        let objects = objects.into_iter().map(|(sha256, size)| File {
            key: format!("objects/{sha256}"),
            size,
            sha256: sha256.clone(),
        });
        let manifest = File { key: manifest_key(id), size: self.canonical().len() as u64, sha256: id.into() };
        objects.chain(self.named.iter().cloned()).chain([manifest]).collect()
    }

    pub fn check(&self) -> Result<(), String> {
        if self.outputs.windows(2).any(|pair| pair[0].step >= pair[1].step) {
            return Err("the outputs are not sorted by step, or a step is there twice".into());
        }
        for files in [&self.pointers, &self.named] {
            if files.windows(2).any(|pair| pair[0].key >= pair[1].key) {
                return Err("the pointers or the named files are not sorted by key, or a key is there twice".into());
            }
            if let Some(file) = files.iter().find(|file| !is_sha256(&file.sha256)) {
                return Err(format!("{} has no SHA-256", file.key));
            }
        }
        for pointer in &self.pointers {
            crate::r2::check_key(&pointer.key)?;
            if CONTROL.contains(&pointer.key.as_str()) || RESERVED.iter().any(|prefix| pointer.key.starts_with(prefix))
            {
                return Err(format!("the pointer {} is at a key that a target keeps for another use", pointer.key));
            }
        }
        let built: BTreeSet<(&str, u64)> = self
            .outputs
            .iter()
            .flat_map(|output| &output.files)
            .map(|file| (file.sha256.as_str(), file.size))
            .collect();
        let mut folders: BTreeMap<&str, bool> = BTreeMap::new();
        for file in &self.named {
            crate::r2::check_key(&file.key)?;
            let folder = file.key.strip_prefix("releases/").and_then(|rest| rest.split_once('/')).map(|(id, _)| id);
            let Some(folder) = folder.filter(|folder| is_sha256(folder)) else {
                return Err(format!("the named file {} is not in a folder `releases/<sha256>/`", file.key));
            };
            if !built.contains(&(file.sha256.as_str(), file.size)) {
                return Err(format!("the named file {} is no file of an output", file.key));
            }
            *folders.entry(folder).or_default() |= file.sha256 == folder;
        }
        if let Some((folder, _)) = folders.into_iter().find(|(_, named)| !named) {
            return Err(format!("releases/{folder}/ holds no file whose SHA-256 is {folder}"));
        }
        Ok(())
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

fn manifest_key(id: &str) -> String {
    format!("releases/{id}.json")
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Give `target` each key of `release` that it lacks, with the bytes of the store, and give the id.
/// The manifest goes last. Each file is checked against its SHA-256 before the target changes.
pub fn publish(target: &Target, store: &Store, release: &Release) -> Result<String, String> {
    release.check()?;
    let (id, bytes) = (release.id(), release.canonical());
    let mut files = release.files(&id);
    let manifest = files.pop().expect("the manifest is the last key");
    let mut missing = Vec::new();
    for file in files {
        if target.has(&file.key)? {
            continue;
        }
        let path = store.object(&file.sha256);
        if hash_file(&path)? != (file.sha256.clone(), file.size) {
            return Err(format!("{} holds other bytes than {}; nothing changed", path.display(), file.key));
        }
        missing.push((path, file.key));
    }
    target.upload(&missing)?;
    if target.put(&manifest.key, &bytes, &Condition::Absent)?.is_none()
        && target.get(&manifest.key)?.as_deref() != Some(bytes.as_slice())
    {
        return Err(format!("{} holds other bytes", manifest.key));
    }
    Ok(id)
}

/// What `release.json` of `target` says, and its tag; `None` before the first switch.
pub fn serving(target: &Target) -> Result<Option<(Serving, Tag)>, String> {
    let Some((bytes, tag)) = target.get_tagged(SERVING)? else { return Ok(None) };
    let serving = serde_json::from_slice(&bytes).map_err(|e| format!("{SERVING}: {e}"))?;
    Ok(Some((serving, tag)))
}

/// Make the published release `id` the one that `target` serves, and the release that `base` says
/// it serves its rollback. `false` when `release.json` is no longer `base`: nothing changed. The
/// pointers change after `release.json`, so a pointer is always of the release that serves or of
/// the rollback.
pub fn switch(target: &Target, base: Option<&(Serving, Tag)>, id: &str, release: &Release) -> Result<bool, String> {
    if !target.has(&manifest_key(id))? {
        return Err(format!("the target has no {}; publish the release first", manifest_key(id)));
    }
    let since = crate::date::timestamp(crate::date::now());
    let next = match base {
        Some((now, _)) if now.serving.id == id => now.clone(),
        Some((now, _)) => Serving {
            serving: Role { id: id.into(), since: since.clone() },
            rollback: Some(Role { id: now.serving.id.clone(), since }),
        },
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

/// The pointer keys of `release` whose bytes on `target` are not those of the release.
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
mod tests {
    use super::*;
    use crate::engine::tests::{fixture, pipeline, Fixture};
    use crate::store::write_atomic;

    /// The release of the built pipeline. The file of `test/join` is the pointer `test/catalog.json`
    /// and names the folder of the named files; `other/catalog.json` is an object of the store
    /// that no step made.
    fn release(fixture: &Fixture) -> Release {
        let steps = pipeline();
        let step = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        let built = fixture.build(&steps).unwrap();
        let mut outputs: Vec<Output> = built.iter().map(|b| Output::new(&b.receipt, step(&b.receipt.step))).collect();
        outputs.sort_by(|a, b| a.step.cmp(&b.step));
        let (count, joined) = (&outputs[0].files[0], &outputs[1].files[0]);
        let generated = b"{\"generated\": true}";
        let path = fixture.store.partial("generated");
        write_atomic(&path, generated).unwrap();
        fixture.store.insert(&path, &sha256_hex(generated)).unwrap();
        let file = |key: String, file: &LayerFile| File { key, size: file.size, sha256: file.sha256.clone() };
        let other =
            File { key: "other/catalog.json".into(), size: generated.len() as u64, sha256: sha256_hex(generated) };
        Release {
            settings: Settings::default(),
            pointers: vec![other, file("test/catalog.json".into(), joined)],
            named: vec![
                file(format!("releases/{}/count.txt", joined.sha256), count),
                file(format!("releases/{}/joined.txt", joined.sha256), joined),
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
        for file in first.files(&id) {
            let bytes = target.get(&file.key).unwrap().unwrap();
            assert_eq!((sha256_hex(&bytes), bytes.len() as u64), (file.sha256.clone(), file.size), "{}", file.key);
        }
        assert_eq!(drift(&target, &first).unwrap(), ["other/catalog.json", "test/catalog.json"]);
        assert!(switch(&target, None, &id, &first).unwrap());
        let base = serving(&target).unwrap().unwrap();
        assert_eq!((base.0.serving.id.as_str(), &base.0.rollback), (id.as_str(), &None));
        assert!(switch(&target, Some(&base), &"b".repeat(64), &first).unwrap_err().contains("publish"));
        assert!(drift(&target, &first).unwrap().is_empty());

        // The target has every file: a second release needs none of them in the store.
        for file in first.files(&id) {
            let _ = std::fs::remove_file(fixture.store.object(&file.sha256));
        }
        let second = Release { machine: "vps".into(), ..first.clone() };
        let next = publish(&target, &fixture.store, &second).unwrap();
        assert!(!switch(&target, None, &next, &second).unwrap(), "release.json is no longer absent");
        let old = (base.0.clone(), Tag("0".repeat(64)));
        assert!(!switch(&target, Some(&old), &next, &second).unwrap(), "release.json has another tag");
        assert_eq!(serving(&target).unwrap().unwrap(), base, "a refused switch changes nothing");
        assert!(switch(&target, Some(&base), &next, &second).unwrap());
        let now = serving(&target).unwrap().unwrap().0;
        assert_eq!((now.serving.id, now.rollback.map(|role| role.id)), (next, Some(id)));

        let forged = "a".repeat(64);
        target.put(&manifest_key(&forged), &second.canonical(), &Condition::Absent).unwrap();
        assert!(Release::read(&target, &forged).unwrap_err().contains("not its id"));
    }

    #[test]
    fn a_manifest_keeps_its_pointers_and_named_files_in_their_places() {
        let fixture = fixture("release-check");
        let valid = release(&fixture);
        valid.check().unwrap();
        let refused = |edit: &dyn Fn(&mut Release)| {
            let mut release = valid.clone();
            edit(&mut release);
            release.check().unwrap_err()
        };
        assert!(refused(&|r| r.outputs.swap(0, 1)).contains("sorted by step"));
        assert!(refused(&|r| r.pointers[0].key = "objects/a".into()).contains("another use"));
        assert!(refused(&|r| r.pointers[1].key = SERVING.into()).contains("another use"));
        assert!(refused(&|r| r.named[0].key = r.named[0].key.replace("releases/", "named/")).contains("folder"));
        assert!(refused(&|r| r.named[0].sha256 = "a".repeat(64)).contains("no file of an output"));
        assert!(refused(&|r| drop(r.named.pop())).contains("holds no file whose SHA-256"));
    }
}
