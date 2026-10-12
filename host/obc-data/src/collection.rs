//! The collection of a target: after a switch, delete the files that only the releases that the
//! target no longer serves name, and the files that no release names. A key of an older publisher,
//! a pointer and `release.json` always stay.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;

use crate::release::{self, manifest_key, Release, POINTERS};
use crate::target::{Listed, Target};

/// How long the files of a release stay after it stops serving, in seconds: a client that read an
/// old pointer finishes its downloads. Chosen, not measured: a client downloads a region in minutes.
pub const GRACE: u64 = 600;

/// The folders where a collection deletes. Every other key stays.
const COLLECTED: [&str; 3] = ["objects/", "releases/", "sources/"];

/// What a collection did.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Collected {
    /// The releases whose manifests and files it deleted.
    pub releases: Vec<String>,
    /// The releases that `release.json` does not name, which stay until the grace ends.
    pub deferred: Vec<String>,
    /// The number of keys that it deleted, also of files that no release names.
    pub keys: u64,
    pub bytes: u64,
    /// Why it stopped, such as a pointer that is not of the serving release; `null` when it finished.
    pub skipped: Option<String>,
}

impl Collected {
    /// The collection in one line.
    pub fn text(&self) -> String {
        if let Some(skipped) = &self.skipped {
            return format!("collected nothing: {skipped}");
        }
        let mut text = format!("collected {} releases, {} keys, {} bytes", self.releases.len(), self.keys, self.bytes);
        if !self.deferred.is_empty() {
            text += &format!("; {} releases stay until {GRACE} s after they stopped serving", self.deferred.len());
        }
        text
    }
}

/// Collect `target` at the time `now`. A release that `release.json` does not name stopped serving
/// at or before `served_since`, when the release that served before the switch began to serve; or
/// at an unknown time when it is `None`. It goes when the grace after that time ended. A file or a
/// manifest that changed within the grace stays: another update can be about to switch to it.
pub fn collect(target: &Target, served_since: Option<&str>, now: u64) -> Result<Collected, String> {
    let mut collected = Collected::default();
    let Some((serving, _)) = release::serving(target)? else { return Ok(collected) };
    let current = Release::read(target, &serving.serving.id)?;
    if let Some(key) = release::drift(target, &current)?.into_iter().next() {
        collected.skipped = Some(format!("{key} is not of the serving release"));
        return Ok(collected);
    }
    let mut kept = vec![current];
    if let Some(rollback) = &serving.rollback {
        kept.push(Release::read(target, &rollback.id)?);
    }
    let listed: BTreeMap<String, Listed> =
        target.list()?.into_iter().map(|listed| (listed.key.clone(), listed)).collect();
    let old = |changed: u64| changed.saturating_add(GRACE) <= now;
    let stopped = served_since.and_then(crate::date::seconds).is_some_and(old);
    let named: BTreeSet<&str> =
        std::iter::once(&serving.serving).chain(&serving.rollback).map(|role| role.id.as_str()).collect();
    for (id, manifest) in listed.iter().filter_map(|(key, listed)| Some((manifest_id(key)?, listed))) {
        if named.contains(id) {
            continue;
        }
        match Release::read(target, id) {
            Ok(_) if stopped && old(manifest.changed) => collected.releases.push(id.into()),
            Ok(release) => {
                collected.deferred.push(id.into());
                kept.push(release);
            }
            Err(e) => {
                collected.skipped = Some(format!("the files of a release are not known: {e}"));
                return Ok(collected);
            }
        }
    }
    let mut keep: BTreeSet<String> = kept.iter().flat_map(|release| release.files()).map(|file| file.key).collect();
    keep.extend(kept.iter().map(|release| manifest_key(&release.id())));
    // The files first and the manifests last: a collection that stops resumes from the manifests.
    let files = listed.values().filter(|listed| {
        !keep.contains(&listed.key)
            && manifest_id(&listed.key).is_none()
            && old(listed.changed)
            && (collectable(&listed.key) || temporary(&listed.key))
    });
    let manifests = collected.releases.iter().map(|id| &listed[&manifest_key(id)]);
    for listed in files.chain(manifests).collect::<Vec<_>>() {
        assert!(collectable(&listed.key) || temporary(&listed.key), "a collection deletes only in {COLLECTED:?}");
        target.remove(&listed.key)?;
        (collected.keys, collected.bytes) = (collected.keys + 1, collected.bytes + listed.size);
    }
    Ok(collected)
}

/// The id of the manifest `releases/<id>.json`.
fn manifest_id(key: &str) -> Option<&str> {
    let id = key.strip_prefix("releases/")?.strip_suffix(".json")?;
    (id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))).then_some(id)
}

/// Whether a collection may delete `key`: a key in one of [`COLLECTED`].
fn collectable(key: &str) -> bool {
    COLLECTED.iter().any(|folder| key.starts_with(folder))
}

/// Whether `key` is a part file of an upload or a temporary file of a write that stopped: in the
/// root of the target, or a sibling of a pointer that names the pointer.
fn temporary(key: &str) -> bool {
    let (folder, name) = key.rsplit_once('/').unwrap_or(("", key));
    let hidden = name.starts_with('.') && (name.ends_with(".part") || name.ends_with(".tmp"));
    let beside = |pointer: &str| {
        let (pointer_folder, file) = pointer.rsplit_once('/').unwrap_or(("", pointer));
        pointer_folder == folder && name.starts_with(&format!(".{file}."))
    };
    hidden && (folder.is_empty() || POINTERS.iter().any(|pointer| beside(pointer)))
}
