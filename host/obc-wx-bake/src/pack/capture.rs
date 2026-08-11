//! Capturing a real past event into a pack.
//!
//! The capture is a **recording proxy**, not a shopping list. [`RecordingUpstream`] sits between
//! the production adapters and the network: it rewrites each canonical URL into its archive URL
//! ([`super::archive`]), delegates, and records exactly what came back — bodies, byte ranges and
//! HEAD probes alike. So `upstream/` is by construction *precisely what the baker asked for*,
//! and there is no second list of members that could drift out of sync with the adapter's real
//! ingress. `service/` is then whatever [`run_cycle`] wrote — the production cycle over the pack's
//! own lattice ([`crate::pack::window::sub_lattice`]) — which is why a re-bake can be a
//! byte-comparison rather than a tolerance.
//!
//! Inside the recorder sits [`AsOf`], and it is not optional. An archive holds the *whole* day,
//! so run discovery replayed against one will happily select a run that had not been published
//! yet at the capture instant — the pack then ships a model baseline with an extra hour of
//! assimilation, and radar the device could not have had, and a nowcaster scored against its
//! `truth/` is measuring something the device will never see. `AsOf` makes any object whose own
//! key says it was not published yet a 404 for discovery, so the production `discover_latest` /
//! `select_run` fallback paths produce the honest answer with no adapter change at all. The
//! suppressed probes are recorded as members with a null length, so the fallback is visible in
//! `event.json` rather than merely claimed here. It guards the **service** half only: `truth/`
//! is by definition what happened afterwards.

use std::collections::BTreeMap;
use std::path::Path;

use crate::canonical::{emit_shard, run_cycle, CycleReport, CycleTimes, Lattice, Mosaic};
use crate::fetch::{FetchOutcome, Fetched, Upstream};
use crate::manifest_v2;
use crate::pack::archive;
use crate::pack::window::sub_lattice;
use crate::pack::{
    sha256, write_file, BakeParams, BboxUdeg, Event, Member, Retrieval, Role, ServiceObject, TruthFrame, TruthParams,
    FORMAT, SERVICE_DIR, TRUTH_DIR, UPSTREAM_DIR,
};
use crate::publish::DirStore;
use crate::source::{hrrr, mrms, Adapter, BakedSource};
use crate::timefmt;

/// The observed ladder the pack captures by default: +15 min through +2 h, the window a nowcast
/// is scored over.
pub const DEFAULT_TRUTH_OFFSETS_MIN: [u32; 8] = [15, 30, 45, 60, 75, 90, 105, 120];

/// The as-of clock: "would this object have existed yet?", asked of a **canonical** URL.
///
/// Deliberately not an [`Upstream`] of its own. The guard has to run on the canonical key, before
/// [`RecordingUpstream`] rewrites it into an archive URL that no longer states an observation
/// instant or a run hour — and it has to run *inside* the recorder, so a suppressed probe is
/// written into `event.json` as a member with a null length rather than vanishing. Both
/// requirements point at the same place, so it lives there.
#[derive(Debug, Clone, Copy)]
pub struct AsOf {
    at: i64,
}

impl AsOf {
    pub fn new(at: i64) -> Self {
        Self { at }
    }

    /// Was `url` still unpublished at the capture instant?
    pub fn not_yet_published(&self, url: &str) -> Result<bool, String> {
        Ok(archive::published_at(url)? > self.at)
    }
}

/// One thing the baker retrieved, before it is given a path in the pack.
#[derive(Debug, Clone)]
pub struct Captured {
    pub role: Role,
    pub url: String,
    pub archive_url: String,
    pub retrieval: Retrieval,
    pub bytes: Option<Vec<u8>>,
}

/// Records every retrieval the baker performs, rewriting canonical URLs to archive URLs on the
/// way out. The recorded `url` is always the canonical one — that is what a replay must serve.
pub struct RecordingUpstream<'a> {
    inner: &'a mut dyn Upstream,
    /// The as-of guard. Every request passes it before the URL is rewritten.
    as_of: AsOf,
    role: Role,
    captured: Vec<Captured>,
    /// Object lengths already learned from the baker's own HEAD probes, so recording a byte range
    /// costs no extra request.
    lengths: BTreeMap<String, u64>,
    suppressed: usize,
}

impl<'a> RecordingUpstream<'a> {
    pub fn new(inner: &'a mut dyn Upstream, as_of: AsOf) -> Self {
        Self { inner, as_of, role: Role::Service, captured: Vec::new(), lengths: BTreeMap::new(), suppressed: 0 }
    }

    /// Everything recorded from here on belongs to `role`.
    pub fn set_role(&mut self, role: Role) {
        self.role = role;
    }

    /// How many discovery probes were answered "not published yet" without reaching the network.
    /// A capture with zero of these has almost certainly been given an instant at which every
    /// candidate already existed — worth noticing, not worth failing.
    pub fn suppressed(&self) -> usize {
        self.suppressed
    }

    /// Does the as-of clock apply to what is being recorded right now?
    ///
    /// **Only to the service half.** `service/` must contain nothing the device could not have
    /// had at the capture instant — that is the whole of F1. `truth/` is the opposite by
    /// construction: it is what the radar saw *afterwards*, and a guard that refused it would
    /// refuse the pack's reason for existing. The two halves are already separate roles, so the
    /// distinction costs nothing and is visible in `event.json`.
    fn guarded(&self) -> bool {
        self.role == Role::Service
    }

    fn suppresses(&self, url: &str) -> Result<bool, String> {
        Ok(self.guarded() && self.as_of.not_yet_published(url)?)
    }

    pub fn into_captured(self) -> Vec<Captured> {
        self.captured
    }

    /// Record `capture` unless an identical retrieval is already recorded — the baker probes some
    /// objects more than once, and a pack lists each member exactly once.
    fn record(&mut self, capture: Captured) {
        if self.captured.iter().any(|existing| existing.url == capture.url && existing.retrieval == capture.retrieval) {
            return;
        }
        self.captured.push(capture);
    }
}

impl Upstream for RecordingUpstream<'_> {
    fn fetch(&mut self, url: &str, cap: u64, if_none_match: Option<&str>) -> Result<FetchOutcome, String> {
        // A body request for a not-yet-published object means the adapter and the guard disagree:
        // adapters only fetch what discovery already proved exists, so this is loud, not a 404.
        if self.suppresses(url)? {
            return Err(format!("{url} was not published at the capture instant — refusing to fetch it"));
        }
        let archive_url = archive::archive_url(url)?;
        let outcome = self.inner.fetch(&archive_url, cap, if_none_match)?;
        if let FetchOutcome::Body(Fetched { bytes, .. }) = &outcome {
            self.record(Captured {
                role: self.role,
                url: url.to_string(),
                archive_url,
                retrieval: Retrieval::Body,
                bytes: Some(bytes.clone()),
            });
        }
        Ok(outcome)
    }

    fn content_length(&mut self, url: &str) -> Result<Option<u64>, String> {
        let archive_url = archive::archive_url(url)?;
        // Discovery is all HEAD probes, so this is where the guard does its work: an object from
        // the future is simply not there, and the adapter's own fallback finds the honest answer.
        // The probe is still recorded, with a null length, so the fallback is visible in the pack.
        if self.suppresses(url)? {
            self.suppressed += 1;
            self.record(Captured {
                role: self.role,
                url: url.to_string(),
                archive_url,
                retrieval: Retrieval::Probe { object_length: None },
                bytes: None,
            });
            return Ok(None);
        }
        let length = self.inner.content_length(&archive_url)?;
        if let Some(length) = length {
            self.lengths.insert(archive_url.clone(), length);
        }
        self.record(Captured {
            role: self.role,
            url: url.to_string(),
            archive_url,
            retrieval: Retrieval::Probe { object_length: length },
            bytes: None,
        });
        Ok(length)
    }

    fn fetch_range(&mut self, url: &str, start: u64, end_inclusive: u64, cap: u64) -> Result<Fetched, String> {
        if self.suppresses(url)? {
            return Err(format!("{url} was not published at the capture instant — refusing to range-fetch it"));
        }
        let archive_url = archive::archive_url(url)?;
        // The whole object's length is what the baker's range arithmetic was bounded by, so it is
        // part of the member's provenance — a replay has to reproduce it. The baker HEADs the
        // object before it selects a range, so this is normally already known.
        let object_length = match self.lengths.get(&archive_url) {
            Some(length) => *length,
            None => {
                let length = self
                    .inner
                    .content_length(&archive_url)?
                    .ok_or_else(|| format!("{archive_url}: vanished between discovery and the range fetch"))?;
                self.lengths.insert(archive_url.clone(), length);
                length
            }
        };
        let fetched = self.inner.fetch_range(&archive_url, start, end_inclusive, cap)?;
        self.record(Captured {
            role: self.role,
            url: url.to_string(),
            archive_url,
            retrieval: Retrieval::Range { object_length, start, end_inclusive },
            bytes: Some(fetched.bytes.clone()),
        });
        Ok(fetched)
    }

    fn fetched_bytes(&self) -> u64 {
        self.inner.fetched_bytes()
    }
}

/// Everything a capture needs to know.
pub struct CaptureRequest {
    pub id: String,
    pub title: String,
    pub region: String,
    /// The wall clock injected into the cycle.
    pub now: i64,
    /// The ground the pack's lattice must cover. Required — see [`BakeParams::bbox_udeg`].
    pub bbox: BboxUdeg,
    /// The `regions.toml` id of the basemap this pack expects under it. Defaults to
    /// [`crate::pack::US_BASEMAP_REGION`], the one non-DACH region the bakery carries.
    pub basemap_region: String,
    pub truth_offsets_min: Vec<u32>,
    /// Check the truth ladder's raw upstream bodies into the pack too — **use it for any pack that
    /// ships.**
    ///
    /// Off by default only because a full-domain multi-cycle pack could not carry them. For a
    /// cropped one the arithmetic is not close: ~450 KB per observation against a `truth/` that
    /// would otherwise be eight baked artifacts whose sources live on one free mirror, so a later
    /// lattice or quantization change could only be absorbed by going back for them. With this on,
    /// [`crate::pack::rebake::verify_truth_rebake`] re-derives the ladder offline and
    /// byte-compares, exactly as `service/` already was.
    pub store_truth_upstream: bool,
}

pub struct CaptureReport {
    pub event: Event,
    pub cycle: CycleReport,
    pub upstream_bytes: u64,
    pub service_bytes: u64,
    pub truth_bytes: u64,
    /// Discovery probes the as-of guard answered "not published yet".
    pub suppressed: usize,
}

/// Capture one cycle of one event into `root`, which must not already hold a pack.
pub fn capture(root: &Path, request: &CaptureRequest, network: &mut dyn Upstream) -> Result<CaptureReport, String> {
    if root.join(super::EVENT_FILE).exists() {
        return Err(format!("{} already holds a pack — capture into a fresh directory", root.display()));
    }
    let service_root = root.join(SERVICE_DIR);
    let mut recorder = RecordingUpstream::new(network, AsOf::new(request.now));

    // --- the service tree: the real cycle, the real sources, the real publisher, on the pack's
    // own lattice.
    let lattice = sub_lattice(&request.bbox)?;
    let mrms_adapter = mrms::Mrms;
    let hrrr_adapter = hrrr::Hrrr;
    let adapters: Vec<&dyn Adapter> = vec![&mrms_adapter, &hrrr_adapter];
    let mut store = DirStore::new(&service_root);
    // One thread: a pack is one small shard, and a deterministic single-threaded bake keeps a
    // capture's cost off the machine it happens to run on.
    let cycle = run_cycle(&lattice, &adapters, &mut recorder, &mut store, request.now, 1, false)?;
    let coverage_udeg = pack_coverage(&lattice);

    // The observed ladder is anchored on the **last real observation**, not on the cycle's
    // quarter-hour reference time. A nowcast is scored against what the radar went on to see, so
    // the rung a rider would compare against is the observation the device actually held — and the
    // as-of guard makes that a few minutes behind the wall clock, which the quarter-hour anchor
    // would silently round away. Discovery is HEAD probes the cycle has already made, so this
    // costs no body bytes; `store_members` collapses the repeated probes into one member each.
    let anchor = mrms::discover_latest(&mut recorder, request.now)?
        .ok_or("no MRMS observation to anchor the observed ladder on")?;

    // --- the observed ladder, from the same anchor. Planned first, so a ladder that cannot exist
    // fails before a single byte moves.
    let ladder = plan_truth_ladder(anchor, &request.truth_offsets_min)?;
    recorder.set_role(Role::Truth);
    let mut truth_frames = Vec::with_capacity(ladder.len());
    for rung in &ladder {
        let bytes = bake_truth_frame(&mut recorder, &lattice, rung.valid_at)?;
        let path = format!("{TRUTH_DIR}/f{}.obcg", rung.offset_min);
        write_file(&root.join(TRUTH_DIR).join(format!("f{}.obcg", rung.offset_min)), &bytes)?;
        truth_frames.push(TruthFrame {
            requested_offset_min: rung.requested_offset_min,
            offset_min: rung.offset_min,
            valid_at: timefmt::rfc3339(rung.valid_at),
            path,
            bytes: bytes.len() as u64,
            sha256: sha256(&bytes),
        });
    }

    // --- freeze the raw bytes and write the provenance document.
    let suppressed = recorder.suppressed();
    let captured = recorder.into_captured();
    let (members, upstream_bytes) = store_members(root, &captured, request.store_truth_upstream)?;

    let service_tree = super::read_tree(&service_root)?;
    let service: Vec<ServiceObject> = service_tree
        .iter()
        .map(|(key, bytes)| ServiceObject { key: key.clone(), bytes: bytes.len() as u64, sha256: sha256(bytes) })
        .collect();
    let service_bytes = service.iter().map(|object| object.bytes).sum();
    let truth_bytes = truth_frames.iter().map(|frame| frame.bytes).sum();

    let window_end = truth_frames
        .iter()
        .map(|frame| frame.offset_min)
        .max()
        .map_or(anchor, |offset| anchor + i64::from(offset) * 60);
    let event = Event {
        format: FORMAT.to_string(),
        id: request.id.clone(),
        title: request.title.clone(),
        region: request.region.clone(),
        window_start: timefmt::rfc3339(anchor),
        window_end: timefmt::rfc3339(window_end),
        bake: BakeParams {
            sources: adapters.iter().map(|adapter| adapter.id().to_string()).collect(),
            now: timefmt::rfc3339(request.now),
            bbox_udeg: request.bbox,
        },
        coverage_udeg,
        basemap_region: request.basemap_region.clone(),
        truth: TruthParams {
            requested_offsets_min: request.truth_offsets_min.clone(),
            cadence_seconds: mrms::CADENCE_SECONDS,
        },
        members,
        manifest_key: manifest_v2::MANIFEST_KEY.to_string(),
        service,
        truth_frames,
    };
    event.write(root)?;
    Ok(CaptureReport { event, cycle, upstream_bytes, service_bytes, truth_bytes, suppressed })
}

/// One rung of the observed ladder: what was asked for, and the observation that answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruthRung {
    pub requested_offset_min: u32,
    pub offset_min: u32,
    pub valid_at: i64,
}

/// Resolve the whole requested ladder against the observation cadence, refusing one that cannot
/// exist.
///
/// Two requests closer together than the cadence snap onto the same instant, which would mean one
/// file written twice and a rung silently missing from the ladder — so it is an error, named with
/// both offsets. Pure, so the refusal is testable without a network.
pub fn plan_truth_ladder(anchor: i64, requested: &[u32]) -> Result<Vec<TruthRung>, String> {
    let mut ladder: Vec<TruthRung> = Vec::with_capacity(requested.len());
    for requested_offset_min in requested.iter().copied() {
        let (offset_min, valid_at) = snap_truth_offset(anchor, requested_offset_min);
        if let Some(clash) = ladder.iter().find(|rung| rung.offset_min == offset_min) {
            return Err(format!(
                "truth offsets +{} and +{requested_offset_min} min both resolve to +{offset_min} min — \
                 they are closer together than the {}-second observation cadence",
                clash.requested_offset_min,
                mrms::CADENCE_SECONDS
            ));
        }
        ladder.push(TruthRung { requested_offset_min, offset_min, valid_at });
    }
    Ok(ladder)
}

/// The observation instant a requested offset resolves to.
///
/// Upstream observations land on their own cadence — MRMS `PrecipRate` publishes every two
/// minutes, on even minutes — so a +15 min request has no object of its own. It is floored to the
/// newest observation at or before it, which is also what a verifier scoring a nowcast would use.
/// The returned offset is the real one, and it is what the frame's own OBCG header will say.
pub fn snap_truth_offset(anchor: i64, requested_offset_min: u32) -> (u32, i64) {
    let requested_at = anchor + i64::from(requested_offset_min) * 60;
    let valid_at = requested_at - requested_at.rem_euclid(mrms::CADENCE_SECONDS);
    let offset_min = u32::try_from((valid_at - anchor) / 60).unwrap_or(0);
    (offset_min, valid_at)
}

/// The ground a pack's lattice answers for: its one shard's own window.
pub fn pack_coverage(lattice: &Lattice) -> BboxUdeg {
    let geometry = lattice.geometry(lattice.shard(0).expect("a pack lattice is one shard"));
    BboxUdeg {
        south_udeg: i64::from(geometry.south_lat_udeg),
        west_udeg: i64::from(geometry.west_lon_udeg),
        north_udeg: geometry.north_lat_udeg(),
        east_udeg: geometry.east_lon_udeg(),
    }
}

/// One observed frame, baked through the same MRMS path and the same emitter the service tree's
/// own frames go through — a one-source mosaic on the pack's lattice.
///
/// **A truth frame is anchored on itself**: `reference_time == valid_at`, offset 0. It is a real
/// observation of a real instant, so that is the only stamping that satisfies `OBCG_Spec.md` §3.2,
/// which reserves `FLAG_OBSERVED` for offset 0 precisely because a frame ahead of its anchor is
/// about an instant nothing observed. Stamping the ladder's own offset into the header instead —
/// which is what this did until the flag rule landed — produced an object whose bytes said
/// "forecast valid at 19:02" over a genuine 19:02 radar field.
///
/// The ladder is not lost by that: `event.json`'s `truth_frames[]` carries `requested_offset_min`,
/// the snapped `offset_min` and `valid_at` for every rung, which is where a scorer reads it. The
/// rung is pack metadata about *when to compare*, not a claim the OBCG object makes about itself.
///
/// Public because [`crate::pack::rebake`] re-derives the truth ladder from the pack's own stored
/// bytes and byte-compares — the same promise `service/` carries, which only became keepable once
/// the truth ladder's raw observations were checked in.
pub fn bake_truth_frame(upstream: &mut dyn Upstream, lattice: &Lattice, valid_at: i64) -> Result<Vec<u8>, String> {
    let frame = mrms::bake_observation(upstream, valid_at)?;
    let source = BakedSource {
        id: mrms::ID,
        geometry: mrms::GEOMETRY,
        reference_time: valid_at,
        attribution: mrms::ATTRIBUTION,
        frames: vec![frame],
        // A truth frame is one observation emitted on its own instant: no derivation runs over it,
        // and giving it a motion history would invite one.
        motion_history: Vec::new(),
    };
    let mosaic = Mosaic::from_sources(vec![source])?;
    let object = emit_shard(lattice, &mosaic, CycleTimes { reference_time: valid_at }, 0, 0)?;
    Ok(object.bytes)
}

/// Give every captured body a path in `upstream/`, write the ones the pack stores, and turn the
/// lot into provenance records.
///
/// **Repeated HEAD probes collapse to one member.** A probe is a question with a yes/no answer, so
/// asking it twice — which happens whenever two things in a capture need the same discovery — is
/// one fact about the archive, not two. Bodies and byte ranges are never collapsed: two ranges of
/// one object are two different retrievals and both have to be replayable.
fn store_members(root: &Path, captured: &[Captured], store_truth_upstream: bool) -> Result<(Vec<Member>, u64), String> {
    let mut members = Vec::with_capacity(captured.len());
    let mut probed: BTreeMap<(Role, String), ()> = BTreeMap::new();
    let mut used: BTreeMap<String, ()> = BTreeMap::new();
    let mut stored_bytes = 0u64;
    for capture in captured {
        if matches!(capture.retrieval, Retrieval::Probe { .. })
            && probed.insert((capture.role, capture.url.clone()), ()).is_some()
        {
            continue;
        }
        let (licence, attribution_url) = archive::terms(&capture.url)?;
        let (path, length, digest, stored) = match &capture.bytes {
            None => (None, None, None, false),
            Some(bytes) => {
                let path = member_path(&capture.url, &capture.retrieval)?;
                if used.insert(path.clone(), ()).is_some() {
                    return Err(format!("two captured members collide on {path}"));
                }
                let stored = capture.role == Role::Service || store_truth_upstream;
                if stored {
                    write_file(&super::resolve(root, &path)?, bytes)?;
                    stored_bytes += bytes.len() as u64;
                }
                (Some(path), Some(bytes.len() as u64), Some(sha256(bytes)), stored)
            }
        };
        members.push(Member {
            role: capture.role,
            url: capture.url.clone(),
            archive_url: capture.archive_url.clone(),
            retrieval: capture.retrieval.clone(),
            path,
            length,
            sha256: digest,
            stored,
            licence: licence.to_string(),
            attribution_url: attribution_url.to_string(),
        });
    }
    Ok((members, stored_bytes))
}

/// `upstream/<source>/<object name>` for a body, plus an explicit `@start-end` for a range so a
/// member's file name states the byte window it is.
pub fn member_path(url: &str, retrieval: &Retrieval) -> Result<String, String> {
    let source = if url.starts_with(mrms::BUCKET) {
        "mrms"
    } else if url.starts_with(hrrr::BUCKET) {
        "hrrr"
    } else {
        return Err(format!("no pack directory for {url}"));
    };
    let name =
        url.rsplit('/').next().filter(|name| !name.is_empty()).ok_or_else(|| format!("no object name in {url}"))?;
    Ok(match retrieval {
        Retrieval::Range { start, end_inclusive, .. } => {
            format!("{UPSTREAM_DIR}/{source}/{name}@{start}-{end_inclusive}")
        }
        _ => format!("{UPSTREAM_DIR}/{source}/{name}"),
    })
}

/// Materialize every recorded-but-absent member from its archive URL, proving the sha256.
pub fn materialize(root: &Path, event: &mut Event, network: &mut dyn Upstream) -> Result<usize, String> {
    let mut restored = 0usize;
    for member in &mut event.members {
        if member.stored || !member.is_body_like() {
            continue;
        }
        let (Some(path), Some(expected), Some(length)) = (member.path.clone(), member.sha256.clone(), member.length)
        else {
            return Err(format!("{}: an unmaterialized member needs a path, length and sha256", member.url));
        };
        let bytes = match &member.retrieval {
            Retrieval::Range { start, end_inclusive, .. } => {
                network.fetch_range(&member.archive_url, *start, *end_inclusive, length + 1)?.bytes
            }
            _ => match network.fetch(&member.archive_url, length + 1, None)? {
                FetchOutcome::Body(fetched) => fetched.bytes,
                FetchOutcome::Unchanged => return Err(format!("{}: 304 without a validator", member.archive_url)),
            },
        };
        if bytes.len() as u64 != length || sha256(&bytes) != expected {
            return Err(format!(
                "{}: the archive now serves {} bytes / sha256 {} — the pack records {length} / {expected}",
                member.archive_url,
                bytes.len(),
                sha256(&bytes)
            ));
        }
        write_file(&super::resolve(root, &path)?, &bytes)?;
        member.stored = true;
        restored += 1;
    }
    event.write(root)?;
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A requested offset with no observation of its own is floored to the newest one at or
    /// before it, and the pack records both numbers.
    #[test]
    fn truth_offsets_are_floored_onto_the_observation_cadence() {
        let anchor = timefmt::parse_rfc3339("2020-08-10T18:52:00Z").unwrap();
        assert_eq!(snap_truth_offset(anchor, 30), (30, anchor + 30 * 60));
        assert_eq!(snap_truth_offset(anchor, 15), (14, anchor + 14 * 60));
        assert_eq!(snap_truth_offset(anchor, 45), (44, anchor + 44 * 60));
        assert_eq!(snap_truth_offset(anchor, 120), (120, anchor + 120 * 60));
        // An anchor that is itself on the cadence keeps every even request exact.
        for offset in [0, 2, 60, 118] {
            assert_eq!(snap_truth_offset(anchor, offset), (offset, anchor + i64::from(offset) * 60));
        }
    }

    /// A ladder whose requests are closer together than the cadence cannot exist, and saying so
    /// beats writing one file twice and shipping a ladder with a rung missing.
    #[test]
    fn a_truth_ladder_finer_than_the_observation_cadence_is_refused() {
        let anchor = timefmt::parse_rfc3339("2020-08-10T18:52:00Z").unwrap();
        // +14 and +15 fall in the same two-minute bucket, so both resolve to the +14 observation.
        let error = plan_truth_ladder(anchor, &[14, 15]).unwrap_err();
        assert!(error.contains("+14") && error.contains("+15"), "the error must name both offsets: {error}");
        assert!(error.contains("+14 min"), "and the instant they collide on: {error}");
        assert!(error.contains("120-second"), "and the cadence that caused it: {error}");
        // A repeated request is the same clash.
        assert!(plan_truth_ladder(anchor, &[30, 30]).is_err());
        // Adjacent requests that straddle the cadence are fine, and keep their own instants.
        let straddling = plan_truth_ladder(anchor, &[15, 16]).expect("+15 floors to +14, +16 is already on cadence");
        assert_eq!(straddling.iter().map(|rung| rung.offset_min).collect::<Vec<_>>(), vec![14, 16]);
        // The shipped ladder is fine, and every rung is distinct.
        let ladder = plan_truth_ladder(anchor, &DEFAULT_TRUTH_OFFSETS_MIN).expect("the default ladder resolves");
        assert_eq!(ladder.len(), DEFAULT_TRUTH_OFFSETS_MIN.len());
        let mut offsets: Vec<u32> = ladder.iter().map(|rung| rung.offset_min).collect();
        assert_eq!(offsets, vec![14, 30, 44, 60, 74, 90, 104, 120]);
        offsets.dedup();
        assert_eq!(offsets.len(), 8);
    }

    /// The as-of guard, on the two decisions the shipped pack turns on.
    #[test]
    fn the_as_of_clock_hides_what_had_not_been_published() {
        let at = timefmt::parse_rfc3339("2020-08-10T18:52:00Z").unwrap();
        let as_of = AsOf::new(at);
        // MRMS: 18:52 and 18:50 are still in the pipeline; 18:48 is out.
        assert!(as_of.not_yet_published(&mrms::object_url(at)).unwrap());
        assert!(as_of.not_yet_published(&mrms::object_url(at - 120)).unwrap());
        assert!(!as_of.not_yet_published(&mrms::object_url(at - 240)).unwrap());
        // HRRR: the 18Z subhourly set is not complete yet; the 17Z one is.
        let run_18 = timefmt::parse_rfc3339("2020-08-10T18:00:00Z").unwrap();
        assert!(as_of.not_yet_published(&hrrr::index_url(run_18, 4)).unwrap());
        assert!(!as_of.not_yet_published(&hrrr::index_url(run_18 - 3_600, 4)).unwrap());
    }

    /// The guard is for the service half only: `truth/` is what happened *afterwards*, so a
    /// recorder in the truth role must reach objects from beyond the capture instant.
    #[test]
    fn the_as_of_clock_never_hides_the_truth_ladder() {
        struct Present;
        impl Upstream for Present {
            fn fetch(&mut self, _url: &str, _cap: u64, _inm: Option<&str>) -> Result<FetchOutcome, String> {
                Ok(FetchOutcome::Body(Fetched { bytes: vec![7], etag: None, last_modified: None }))
            }
            fn content_length(&mut self, _url: &str) -> Result<Option<u64>, String> {
                Ok(Some(1))
            }
            fn fetch_range(&mut self, _url: &str, _s: u64, _e: u64, _cap: u64) -> Result<Fetched, String> {
                Ok(Fetched { bytes: vec![7], etag: None, last_modified: None })
            }
            fn fetched_bytes(&self) -> u64 {
                0
            }
        }
        let at = timefmt::parse_rfc3339("2020-08-10T18:52:00Z").unwrap();
        let future = mrms::object_url(at + 60 * 60);
        let mut network = Present;
        let mut recorder = RecordingUpstream::new(&mut network, AsOf::new(at));

        // Service role: an object from the future is simply not there, and the probe is recorded
        // with a null length so the fallback is visible in the pack.
        assert_eq!(recorder.content_length(&future).unwrap(), None);
        assert_eq!(recorder.suppressed(), 1);
        assert!(recorder.fetch(&future, 1_000, None).is_err(), "a body request past the guard must be loud");

        // Truth role: the same object is reachable, because that is the point of ground truth.
        recorder.set_role(Role::Truth);
        assert_eq!(recorder.content_length(&future).unwrap(), Some(1));
        assert!(recorder.fetch(&future, 1_000, None).is_ok());
        assert_eq!(recorder.suppressed(), 1, "the truth ladder suppresses nothing");

        let captured = recorder.into_captured();
        let suppressed_probe = captured
            .iter()
            .find(|capture| {
                capture.role == Role::Service && capture.retrieval == Retrieval::Probe { object_length: None }
            })
            .expect("the suppressed probe is recorded");
        assert_eq!(suppressed_probe.url, future);
        assert!(captured.iter().any(|capture| capture.role == Role::Truth && capture.retrieval == Retrieval::Body));
    }

    #[test]
    fn member_paths_name_the_object_and_the_byte_window() {
        let observation = mrms::object_url(timefmt::parse_rfc3339("2020-08-10T18:52:00Z").unwrap());
        assert_eq!(
            member_path(&observation, &Retrieval::Body).unwrap(),
            "upstream/mrms/MRMS_PrecipRate_00.00_20200810-185200.grib2.gz"
        );
        let run = timefmt::parse_rfc3339("2020-08-10T18:00:00Z").unwrap();
        assert_eq!(
            member_path(&hrrr::index_url(run, 1), &Retrieval::Body).unwrap(),
            "upstream/hrrr/hrrr.t18z.wrfsubhf01.grib2.idx"
        );
        assert_eq!(
            member_path(&hrrr::object_url(run, 1), &Retrieval::Range { object_length: 9, start: 1, end_inclusive: 4 })
                .unwrap(),
            "upstream/hrrr/hrrr.t18z.wrfsubhf01.grib2@1-4"
        );
    }
}
