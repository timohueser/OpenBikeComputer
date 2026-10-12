//! The collection of a target: after a switch, delete the files that only the releases that the
//! target no longer serves name, and the files that no release names. A key of an older publisher,
//! a pointer and `release.json` always stay.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::Serialize;

use crate::release::{self, Release, POINTERS};
use crate::target::{Listed, Target};

/// How long the files of a release stay after it stops serving, in seconds: longer than a client
/// needs to fetch the files that a pointer it just read names. 10 minutes, chosen by the owner;
/// change it here.
pub const GRACE: u64 = 600;

/// The folders where a collection deletes. Every other key stays.
const COLLECTED: [&str; 3] = ["objects/", "releases/", "sources/"];

/// What a collection did.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Collected {
    /// The releases whose files and manifest it deleted.
    pub releases: Vec<String>,
    /// The releases that `release.json` does not name, which stay for now.
    pub deferred: Vec<String>,
    /// The keys that it deleted, also of files that no release names, and their size.
    pub keys: u64,
    pub bytes: u64,
    /// Why it stopped before the end: a pointer that is not of the serving release, a manifest that
    /// it cannot read, or an error. `null` when it finished.
    pub stopped: Option<String>,
}

impl Collected {
    /// The collection in one line.
    pub fn text(&self) -> String {
        if let (Some(stopped), 0) = (&self.stopped, self.keys) {
            return format!("collected nothing: {stopped}");
        }
        let mut text = format!("collected {} releases, {} keys, {} bytes", self.releases.len(), self.keys, self.bytes);
        if !self.deferred.is_empty() {
            text += &format!("; {} releases stay until a later collection", self.deferred.len());
        }
        if let Some(stopped) = &self.stopped {
            text += &format!("; then it stopped: {stopped}");
        }
        text
    }
}

/// Collect `target` at the time `now`. A release that `release.json` does not name stopped serving
/// at or before `served_since`, when the release that served before the switch began to serve; or
/// at an unknown time when it is `None`. The first collection [`GRACE`] seconds or more after that
/// time deletes it. A file or a manifest that changed within the grace stays: another update can be
/// about to switch to it.
pub fn collect(target: &Target, served_since: Option<&str>, now: u64) -> Collected {
    let mut collected = Collected::default();
    if let Err(e) = pass(target, served_since, now, &mut collected) {
        collected.stopped = Some(e);
    }
    collected
}

fn pass(target: &Target, served_since: Option<&str>, now: u64, collected: &mut Collected) -> Result<(), String> {
    let Some((serving, _)) = release::serving(target)? else { return Ok(()) };
    let current = Release::read(target, &serving.serving.id)?;
    if let Some(key) = release::drift(target, &current)?.into_iter().next() {
        return Err(format!("{key} is not of the serving release"));
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
    let mut dropped = Vec::new();
    for (id, manifest) in listed.iter().filter_map(|(key, listed)| Some((manifest_id(key)?, listed))) {
        if named.contains(id) {
            continue;
        }
        let release = Release::read(target, id).map_err(|e| format!("the files of a release are not known: {e}"))?;
        if stopped && old(manifest.changed) {
            dropped.push(manifest);
        } else {
            collected.deferred.push(id.into());
            kept.push(release);
        }
    }
    let keep: BTreeSet<String> = kept.iter().flat_map(Release::files).map(|file| file.key).collect();
    // The files first and the manifests last: a collection that stops resumes from the manifests.
    let files = listed.values().filter(|listed| {
        manifest_id(&listed.key).is_none()
            && !keep.contains(&listed.key)
            && old(listed.changed)
            && (collectable(&listed.key) || temporary(&listed.key))
    });
    for listed in files.chain(dropped) {
        target.remove(&listed.key)?;
        (collected.keys, collected.bytes) = (collected.keys + 1, collected.bytes + listed.size);
        if let Some(id) = manifest_id(&listed.key) {
            collected.releases.push(id.into());
        }
    }
    Ok(())
}

/// The id of the manifest `releases/<id>.json`.
fn manifest_id(key: &str) -> Option<&str> {
    let id = key.strip_prefix("releases/")?.strip_suffix(".json")?;
    release::is_sha256(id).then_some(id)
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
