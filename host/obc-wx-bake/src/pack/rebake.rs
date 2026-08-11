//! Replay a pack's `upstream/` through the **real** baker and compare against its `service/`.
//!
//! This is the pack format's load-bearing property, and it is deliberately cheap to state: the
//! replay is a [`FixtureUpstream`] — the same offline seam the checked-in fixture cycles use — and
//! the bake is [`run_cycle`] with the production adapters over the pack's own lattice. Nothing
//! about packs leaks into the bakery, so "the pack re-bakes byte-identically" really does mean
//! "the baker still produces these bytes from these upstream bytes".

use std::collections::BTreeMap;
use std::path::Path;

use crate::canonical::{run_cycle, CycleReport};
use crate::fetch::FixtureUpstream;
use crate::pack::window::sub_lattice;
use crate::pack::{archive, resolve, Event, Member, Retrieval, Role, SERVICE_DIR};
use crate::publish::DirStore;
use crate::source::{hrrr, mrms, Adapter};
use crate::timefmt;

/// Load the pack's stored service members into an offline upstream, keyed by the **canonical**
/// URLs the baker asks for (never the archive URLs the bytes came from).
pub fn replay_upstream(root: &Path, event: &Event) -> Result<FixtureUpstream, String> {
    load(root, event.service_members())
}

/// The same, for the truth ladder's raw observations.
pub fn truth_upstream(root: &Path, event: &Event) -> Result<FixtureUpstream, String> {
    load(root, truth_members(event))
}

fn load<'a>(root: &Path, members: impl Iterator<Item = &'a Member>) -> Result<FixtureUpstream, String> {
    let mut upstream = FixtureUpstream::default();
    for member in members {
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
/// What a re-bake produced, plus the evidence that it was offline.
pub struct RebakeReport {
    pub cycle: CycleReport,
    /// Every URL the replay asked for, in order, as [`FixtureUpstream`] logs them (`HEAD <url>`
    /// for a probe, `<url>#start-end` for a range). Exposed so hermeticity can be *asserted*
    /// rather than asserted about: a request the pack does not carry a member for would mean the
    /// pack is incomplete, and the only reason it baked is something outside it.
    pub requests: Vec<String>,
}

pub fn bake_into(root: &Path, event: &Event, destination: &Path) -> Result<RebakeReport, String> {
    // A **set** comparison: `["hrrr","mrms"]` describes the same pack as `["mrms","hrrr"]`, and the
    // mosaic's own precedence comes from `source::MOSAIC_PRIORITY`, never from this list's order.
    let mut recorded: Vec<&str> = event.bake.sources.iter().map(String::as_str).collect();
    recorded.sort_unstable();
    let mut supported: Vec<&str> = archive::SUPPORTED_SOURCES.to_vec();
    supported.sort_unstable();
    if recorded != supported {
        return Err(format!(
            "pack sources {:?} cannot be replayed yet (supported: {})",
            event.bake.sources,
            archive::SUPPORTED_SOURCES.join(", ")
        ));
    }
    let now = timefmt::parse_rfc3339(&event.bake.now)
        .ok_or_else(|| format!("event.json: bake.now {:?} is not RFC 3339", event.bake.now))?;
    let lattice = sub_lattice(&event.bake.bbox_udeg)?;
    let mut upstream = replay_upstream(root, event)?;
    let mut store = DirStore::new(destination);
    let mrms_adapter = mrms::Mrms;
    let hrrr_adapter = hrrr::Hrrr;
    let adapters: Vec<&dyn Adapter> = vec![&mrms_adapter, &hrrr_adapter];
    let cycle = run_cycle(&lattice, &adapters, &mut upstream, &mut store, now, 1, false)?;
    Ok(RebakeReport { cycle, requests: upstream.requests })
}

/// The whole CI check: replay `upstream/`, and prove the result equals `service/` byte for byte
/// and key for key.
pub fn verify_rebake(root: &Path, event: &Event, scratch: &Path) -> Result<RebakeReport, String> {
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
    // Hermeticity, as a check rather than a claim: every request the replay made must be one the
    // pack carries a member for. `FixtureUpstream` has no network, so a request outside this set
    // could only have been satisfied by something the pack does not describe.
    let unaccounted: Vec<&String> = report.requests.iter().filter(|request| !accounted_for(event, request)).collect();
    if !unaccounted.is_empty() {
        return Err(format!("the re-bake asked for {unaccounted:?}, which no member of the pack accounts for"));
    }
    Ok(report)
}

/// Does some member of `event` describe the retrieval `request` names?
///
/// `FixtureUpstream` logs `HEAD <url>` for a probe, `<url>#start-end` for a range, and the bare
/// URL for a body — the same three shapes [`Retrieval`] distinguishes.
fn accounted_for(event: &Event, request: &str) -> bool {
    event.service_members().any(|member| match &member.retrieval {
        Retrieval::Probe { .. } => request == format!("HEAD {}", member.url),
        Retrieval::Body => request == member.url,
        Retrieval::Range { start, end_inclusive, .. } => request == format!("{}#{start}-{end_inclusive}", member.url),
    })
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

/// Re-derive every truth frame from the pack's own stored MRMS bytes and byte-compare.
///
/// `service/` has always been a pure re-run of checked-in bytes; `truth/` only became one when the
/// ladder's raw observations stopped being `stored: false`. That matters for the lattice and
/// quantization work ahead: a change there must fail here loudly, not leave eight stale baked
/// frames that can only be refreshed by going back to a single free mirror for 4.3 MB.
///
/// Returns the number of frames compared.
pub fn verify_truth_rebake(root: &Path, event: &Event) -> Result<usize, String> {
    if event.truth_frames.is_empty() {
        return Ok(0);
    }
    let anchor = timefmt::parse_rfc3339(&event.window_start)
        .ok_or_else(|| format!("event.json: window_start {:?} is not RFC 3339", event.window_start))?;
    let lattice = sub_lattice(&event.bake.bbox_udeg)?;
    let mut upstream = truth_upstream(root, event)?;
    for frame in &event.truth_frames {
        let valid_at = timefmt::parse_rfc3339(&frame.valid_at)
            .ok_or_else(|| format!("{}: valid_at {:?} is not RFC 3339", frame.path, frame.valid_at))?;
        // The rung lives in `event.json`, not in the object's header, so this is where it is
        // checked — the frame itself is anchored on its own instant (see `bake_truth_frame`).
        if valid_at - anchor != i64::from(frame.offset_min) * 60 {
            return Err(format!("{}: offset_min disagrees with valid_at - window_start", frame.path));
        }
        let baked = crate::pack::capture::bake_truth_frame(&mut upstream, &lattice, valid_at)?;
        let stored = std::fs::read(crate::pack::resolve(root, &frame.path)?)
            .map_err(|error| format!("{}: {error}", frame.path))?;
        if baked != stored {
            return Err(format!(
                "{} is not byte-identical on re-bake ({} stored bytes vs {} rebaked)",
                frame.path,
                stored.len(),
                baked.len()
            ));
        }
    }
    Ok(event.truth_frames.len())
}

/// Re-derive `service/` and `truth/` **in place** from the pack's own stored `upstream/`, and
/// rewrite the parts of `event.json` that describe them.
///
/// The counterpart of [`verify_rebake`], and it exists for the same reason that check does: the
/// baked halves of a pack are a pure function of bytes the pack already carries, so a deliberate
/// change to the lattice, the emitter or the quantization is absorbed by re-running the function
/// rather than by going back to a free mirror for the raw observations. It touches no network and
/// records no members — `upstream/` and the provenance in `members[]` are the pack's evidence and
/// this must never rewrite them.
pub fn regenerate(root: &Path, event: &mut Event) -> Result<(), String> {
    let service_root = root.join(SERVICE_DIR);
    for directory in [&service_root, &root.join(crate::pack::TRUTH_DIR)] {
        if directory.exists() {
            std::fs::remove_dir_all(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
        }
    }
    bake_into(root, event, &service_root)?;

    let lattice = sub_lattice(&event.bake.bbox_udeg)?;
    event.bake.sources = archive::SUPPORTED_SOURCES.iter().map(|id| (*id).to_string()).collect();
    event.manifest_key = crate::manifest_v2::MANIFEST_KEY.to_string();
    event.coverage_udeg = crate::pack::capture::pack_coverage(&lattice);
    event.service = crate::pack::read_tree(&service_root)?
        .iter()
        .map(|(key, bytes)| crate::pack::ServiceObject {
            key: key.clone(),
            bytes: bytes.len() as u64,
            sha256: crate::pack::sha256(bytes),
        })
        .collect();

    let mut upstream = truth_upstream(root, event)?;
    for frame in &mut event.truth_frames {
        let valid_at = timefmt::parse_rfc3339(&frame.valid_at)
            .ok_or_else(|| format!("{}: valid_at {:?} is not RFC 3339", frame.path, frame.valid_at))?;
        let bytes = crate::pack::capture::bake_truth_frame(&mut upstream, &lattice, valid_at)?;
        crate::pack::write_file(&crate::pack::resolve(root, &frame.path)?, &bytes)?;
        frame.bytes = bytes.len() as u64;
        frame.sha256 = crate::pack::sha256(&bytes);
    }
    event.write(root)
}

/// Every member the pack records but has not checked in, with the archive URL that restores it.
pub fn unmaterialized(event: &Event) -> Vec<&Member> {
    event.members.iter().filter(|member| member.is_body_like() && !member.stored).collect()
}

/// Truth members exist for later scoring; nothing in CI decodes them.
pub fn truth_members(event: &Event) -> impl Iterator<Item = &Member> {
    event.members.iter().filter(|member| member.role == Role::Truth)
}
