//! Replay a pack's `upstream/` through the **real** baker and compare against its `service/`.
//!
//! This is the pack format's load-bearing property, and it is deliberately cheap to state: the
//! replay is a [`FixtureUpstream`] — the same offline seam the checked-in fixture cycles use — and
//! the bake is [`run_cycle`] with the production adapters. Nothing about packs leaks into the
//! bakery, so "the pack re-bakes byte-identically" really does mean "the baker still produces
//! these bytes from these upstream bytes".

use std::collections::BTreeMap;
use std::path::Path;

use crate::cycle::{run_cycle, CycleReport};
use crate::fetch::FixtureUpstream;
use crate::manifest;
use crate::pack::crop::CroppedAdapter;
use crate::pack::{archive, resolve, Event, Member, Retrieval, Role, SERVICE_DIR};
use crate::publish::DirStore;
use crate::source::{us::UsComposite, Adapter};

/// Load the pack's stored service members into an offline upstream, keyed by the **canonical**
/// URLs the baker asks for (never the archive URLs the bytes came from).
pub fn replay_upstream(root: &Path, event: &Event) -> Result<FixtureUpstream, String> {
    let mut upstream = FixtureUpstream::default();
    for member in event.service_members() {
        match &member.retrieval {
            Retrieval::Probe { object_length } => {
                if let Some(length) = object_length {
                    upstream.declare(member.url.clone(), *length);
                }
            }
            Retrieval::Body => {
                upstream.insert(member.url.clone(), member_bytes(root, member)?, None);
            }
            Retrieval::Range { object_length, start, .. } => {
                upstream.insert_range(member.url.clone(), *object_length, *start, member_bytes(root, member)?);
            }
        }
    }
    Ok(upstream)
}

fn member_bytes(root: &Path, member: &Member) -> Result<Vec<u8>, String> {
    let path = member.path.as_deref().ok_or_else(|| format!("{}: a body member needs a path", member.url))?;
    if !member.stored {
        return Err(format!(
            "{path} is recorded but not checked in — run `obc-wx-pack fetch` to materialize it from {}",
            member.archive_url
        ));
    }
    let bytes = std::fs::read(resolve(root, path)?).map_err(|error| format!("{path}: {error}"))?;
    let digest = crate::pack::sha256(&bytes);
    match member.sha256.as_deref() {
        Some(expected) if expected == digest => Ok(bytes),
        Some(expected) => Err(format!("{path}: sha256 {digest} != the recorded {expected}")),
        None => Err(format!("{path}: no recorded sha256")),
    }
}

/// Bake the pack's upstream into `destination`, exactly as the capture did.
pub fn bake_into(root: &Path, event: &Event, destination: &Path) -> Result<CycleReport, String> {
    if event.bake.adapter != crate::source::us::ID {
        return Err(format!(
            "pack adapter {:?} cannot be replayed yet (supported: {})",
            event.bake.adapter,
            archive::SUPPORTED_ADAPTERS.join(", ")
        ));
    }
    let now = manifest::parse_rfc3339(&event.bake.now)
        .ok_or_else(|| format!("event.json: bake.now {:?} is not RFC 3339", event.bake.now))?;
    let mut upstream = replay_upstream(root, event)?;
    let mut store = DirStore::new(destination);
    let base = UsComposite;
    let cropped;
    let adapter: &dyn Adapter = match event.bake.bbox_udeg {
        Some(bbox) => {
            cropped = CroppedAdapter::new(&base, bbox);
            &cropped
        }
        None => &base,
    };
    run_cycle(&[adapter], &mut upstream, &mut store, now, false)
}

/// The whole CI check: replay `upstream/`, and prove the result equals `service/` byte for byte
/// and key for key.
pub fn verify_rebake(root: &Path, event: &Event, scratch: &Path) -> Result<CycleReport, String> {
    // The comparison is over the *whole* destination tree, so it has to start empty — and an
    // empty destination must be earned, never taken: `--out` is a user-supplied path.
    if scratch.exists() {
        let existing = crate::pack::read_tree(scratch)?;
        if !existing.is_empty() {
            return Err(format!("{} is not empty — re-bake into a fresh directory", scratch.display()));
        }
        std::fs::remove_dir_all(scratch).map_err(|error| format!("{}: {error}", scratch.display()))?;
    }
    let report = bake_into(root, event, scratch)?;
    let rebaked = crate::pack::read_tree(scratch)?;
    let stored = crate::pack::read_tree(&root.join(SERVICE_DIR))?;
    compare(&stored, &rebaked)?;
    // …and `event.json`'s own object list must be that same tree, so the document can never drift
    // away from the bytes it describes.
    let listed: BTreeMap<&str, u64> = event.service.iter().map(|object| (object.key.as_str(), object.bytes)).collect();
    let actual: BTreeMap<&str, u64> = stored.iter().map(|(key, bytes)| (key.as_str(), bytes.len() as u64)).collect();
    if listed != actual {
        return Err("event.json's service object list disagrees with the service/ tree".into());
    }
    if !event.service.iter().any(|object| object.key == event.manifest_key) {
        return Err(format!("event.json's manifest_key {} is not among its service objects", event.manifest_key));
    }
    Ok(report)
}

fn compare(stored: &BTreeMap<String, Vec<u8>>, rebaked: &BTreeMap<String, Vec<u8>>) -> Result<(), String> {
    let missing: Vec<&String> = stored.keys().filter(|key| !rebaked.contains_key(*key)).collect();
    let extra: Vec<&String> = rebaked.keys().filter(|key| !stored.contains_key(*key)).collect();
    if !missing.is_empty() || !extra.is_empty() {
        return Err(format!("re-bake tree differs: missing {missing:?}, unexpected {extra:?}"));
    }
    for (key, bytes) in stored {
        let fresh = &rebaked[key];
        if fresh != bytes {
            return Err(format!(
                "{key} is not byte-identical on re-bake ({} stored bytes vs {} rebaked)",
                bytes.len(),
                fresh.len()
            ));
        }
    }
    Ok(())
}

/// Every member the pack records but has not checked in, with the archive URL that restores it.
pub fn unmaterialized(event: &Event) -> Vec<&Member> {
    event.members.iter().filter(|member| member.is_body_like() && !member.stored).collect()
}

/// Truth members exist for later scoring; nothing in CI decodes them.
pub fn truth_members(event: &Event) -> impl Iterator<Item = &Member> {
    event.members.iter().filter(|member| member.role == Role::Truth)
}
