//! The host weather client: what the phone does, in Rust, for the simulator.
//!
//! ```text
//!   wx/v2/manifest.json ──►  plan            (expiry + bbox ÷ shard grid; nothing selectable)
//!        │                     │
//!        │                     ▼
//!        │             OBCG corridor reads   (header + covering pages + needed tiles, Range)
//!        │                     │
//!        ▼                     ▼
//!   MET hourly  ────────►  OBCW bundle       (shared obc-formats encoder; device-readable)
//! ```
//!
//! Two boundaries are load-bearing and are enforced here, not by convention:
//!
//! - **The service never receives a coordinate.** Every OBC request is a key-addressed `GET` or
//!   Range read of a static object. The corridor *derives* from the rider's position, but the
//!   position itself only ever leaves this process in a MET query — the single third party the
//!   epic allows to see one.
//! - **Nothing chooses.** There is one dataset on one lattice at one cadence, so "which objects
//!   cover me" is four divisions ([`manifest_v2::Grid::shards_for`]) and not a policy. WXR5 #1244
//!   deleted the client half of that — the tier ladder, bbox containment, expired-product
//!   shadowing and the lattice-nesting refusal — and WXR7 #1246 deleted the producer half and the
//!   spec text, so none of it exists anywhere any more.
//!
//! This is a second implementation of the contract the iOS companion implements; the phone stays
//! the reference. Where the two could drift, this crate's tests pin the shared vectors both read —
//! including `specs/vectors/wx-manifest-v2.json`, the manifest both parsers must answer identically.

pub mod bundle;
pub mod corridor;
pub mod http;
pub mod manifest_v2;
pub mod met;

use std::collections::BTreeMap;

use bundle::{FrameInput, Lattice, Scene};
use corridor::{Corridor, ShardRead, CLOCK_SKEW_TOLERANCE_S, HORIZON_S, MAX_OBSERVATION_AGE_S};
use http::{Http, Request, MANIFEST_CAP};
use manifest_v2::{Manifest, PlanOutcome};

/// The production service origin.
pub const DEFAULT_SERVICE_URL: &str = "https://wx.openbikecomputer.com";

/// Why there is no rain map. Every variant is a *stated* reason: the UI never has to guess, and
/// **none of them may be rendered as "dry"** — an absent map is not an absence of rain. The
/// difference is the whole reason [`manifest_v2::PlanOutcome`] is four-valued rather than an empty
/// list of objects to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRainMap {
    /// The corridor is off the lattice, or is not a window this client will interpret.
    OutOfDomain,
    /// On the lattice, but no source reaches it in any frame — the polar band. The objects exist
    /// and are entirely intensity 15, so there is nothing there to fetch and nothing to show.
    Uncovered,
    /// The published generation is past its own `stale_after` and nothing fresher replaced it.
    /// **No weather**, which is not no rain.
    Expired,
    /// The manifest itself could not be had.
    ServiceUnavailable,
    /// Every frame this generation publishes is outside the window the rain map answers — too old
    /// to be a current observation, or further ahead than two hours. Nothing failed; the data on
    /// offer is about a different time. Distinct from [`NoRainMap::FramesUnavailable`] because
    /// wearing that label would have said "failed" about a service that answered perfectly.
    OutsideWindow,
    /// Every shard of every frame failed to fetch or verify. A present object that 404s, comes back
    /// short or fails its CRC is an **error**, never an absence of rain.
    FramesUnavailable,
}

impl std::fmt::Display for NoRainMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoRainMap::OutOfDomain => write!(f, "this position is off the weather lattice"),
            NoRainMap::Uncovered => write!(f, "no source covers this position"),
            NoRainMap::Expired => write!(f, "the published weather expired and nothing replaced it"),
            NoRainMap::ServiceUnavailable => write!(f, "the weather service is unreachable"),
            NoRainMap::OutsideWindow => write!(f, "every published frame is outside the two-hour window"),
            NoRainMap::FramesUnavailable => write!(f, "every frame failed to fetch or verify"),
        }
    }
}

/// Evidence about one fetch. Counters only — never control flow, and never rendered as weather.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// What the **coordinate-free** half of the job cost: the manifest plus the corridor Range
    /// reads, this fetch only. MET is deliberately not in here — it is a different party seeing a
    /// different thing, and folding its 200 KB document into "service bytes" would make the number
    /// mean nothing (it was ~96 % of the figure before this was split).
    pub service_requests: u32,
    pub service_bytes: u64,
    /// What MET cost, this fetch — the one request that carries the rider's coordinate.
    pub met_requests: u32,
    pub met_bytes: u64,
    /// Shard crops answered from the in-process cache, costing no request at all.
    pub cached_frames: u32,
    /// Frames the manifest parser refused. One bad entry costs its own frame, never the document.
    pub skipped_manifest_frames: usize,
    pub clock_skew_suspected: bool,
    /// Shards that failed to fetch or verify. One is a hole in one frame, never a failed job.
    pub failed_frames: u32,
    /// Shards the manifest measured as dry, so no request was made. Evidence that "no rain" was
    /// *measured*, which is the difference between a dry map and a missing one.
    pub dry_shards: u32,
    /// Shards whose manifest entry says a radar painted them, rather than model fill. Per shard
    /// because that is where the fact is true — and it stays a counter: OBCW carries one quality
    /// flag per *frame*, so no per-shard bit may set it (see `read_plan`).
    pub observed_shards: u32,
    pub dropped_oversize_frames: u32,
    /// The generation this bundle was built from, for the dev panel. Not on the device.
    pub generation: Option<String>,
    pub attribution: Vec<String>,
    /// Why there is no rain map, when there isn't one.
    pub no_rain_map: Option<NoRainMap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The hourly forecast could not be had and no cached one exists. Without hourly there is no
    /// bundle at all — the rain map is the optional half, not the other way round.
    Hourly(met::MetError),
    Build(bundle::BuildError),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Hourly(error) => write!(f, "hourly: {error}"),
            FetchError::Build(error) => write!(f, "bundle: {error}"),
        }
    }
}

/// One assembled OBCW bundle plus the evidence behind it.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub bytes: Vec<u8>,
    pub diagnostics: Diagnostics,
}

/// The stateful client: it holds the manifest cache (whose window the manifest itself states, per
/// OBCG §10) and the MET cache (whose `Expires` is absolute), because the throttle rules *are*
/// those caches.
pub struct WeatherClient {
    origin: String,
    met: met::MetClient,
    manifest_cache: Option<CachedManifest>,
    /// Cropped shards, keyed on `(object key, window)`. Frame objects are immutable by the
    /// publishing contract, so a hit is served without a request of any kind.
    frames: corridor::FrameCache,
    generation: u32,
}

#[derive(Debug, Clone)]
struct CachedManifest {
    manifest: Manifest,
    etag: Option<String>,
    fetched_at: i64,
}

impl WeatherClient {
    pub fn new(origin: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            met: met::MetClient::new(),
            manifest_cache: None,
            frames: corridor::FrameCache::default(),
            generation: 0,
        }
    }

    /// Point the MET half at a stand-in endpoint (fixtures, a local capture).
    pub fn with_met_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.met = met::MetClient::new().with_endpoint(endpoint);
        self
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The manifest, honoring the window **the document states** (`freshness.manifest_max_age_s`)
    /// and revalidating with the stored ETag past it. The client holds no duration of its own: a
    /// cadence change is a baker deploy. An unreachable service with a cached manifest is not an
    /// outage; a cold cache is.
    pub fn manifest<H: Http>(&mut self, http: &mut H, now: i64) -> Result<Manifest, manifest_v2::ManifestError> {
        // The backwards-clock guard matters: without it a clock that jumped back would pin a
        // stale manifest forever instead of refetching.
        if let Some(cached) = &self.manifest_cache {
            if now >= cached.fetched_at && !cached.manifest.freshness.manifest_is_stale(cached.fetched_at, now) {
                return Ok(cached.manifest.clone());
            }
        }
        let url = corridor::join(&self.origin, manifest_v2::MANIFEST_KEY);
        let request = Request {
            url,
            range: None,
            if_none_match: self.manifest_cache.as_ref().and_then(|cached| cached.etag.clone()),
            if_modified_since: None,
        };
        let response = match http.perform(&request, MANIFEST_CAP) {
            Ok(response) => response,
            Err(error) => {
                return match &self.manifest_cache {
                    Some(cached) => Ok(cached.manifest.clone()),
                    None => Err(manifest_v2::ManifestError::Malformed(error.to_string())),
                }
            }
        };
        if response.is_not_modified() {
            if let Some(cached) = self.manifest_cache.as_mut() {
                cached.fetched_at = now;
                return Ok(cached.manifest.clone());
            }
        }
        if !response.is_success() {
            return match &self.manifest_cache {
                Some(cached) => Ok(cached.manifest.clone()),
                None => Err(manifest_v2::ManifestError::Malformed(format!("status {}", response.status))),
            };
        }
        let parsed = manifest_v2::parse(&response.body)?;
        self.manifest_cache = Some(CachedManifest { manifest: parsed.clone(), etag: response.etag, fetched_at: now });
        Ok(parsed)
    }

    /// The whole job: manifest → plan → corridor reads → MET → OBCW.
    ///
    /// The rain half degrades on its own. A service outage, an expired generation and a corridor off
    /// the lattice all produce a truthful *hourly-only* bundle with the reason recorded — never a
    /// fabricated map and never a dry claim.
    pub fn fetch<H: Http>(
        &mut self,
        http: &mut H,
        corridor: &Corridor,
        now: i64,
        request_id: u32,
    ) -> Result<Bundle, FetchError> {
        let mut diagnostics = Diagnostics::default();
        let (before_requests, before_bytes) = (http.requests(), http.bytes());
        let cache_hits_before = self.frames.hits;

        let scene = match self.manifest(http, now) {
            Ok(manifest) => {
                diagnostics.skipped_manifest_frames = manifest.skipped_frames;
                diagnostics.clock_skew_suspected = manifest.generated_at - now > CLOCK_SKEW_TOLERANCE_S;
                diagnostics.generation = Some(manifest.generation.clone());
                diagnostics.attribution.extend(manifest.attribution.iter().map(|attribution| attribution.text.clone()));
                let plan = manifest.plan(&corridor.bounds, now);
                match plan.outcome {
                    // Read the outcome first and the vectors second: outside `Covered` both are
                    // empty and *mean nothing*, and rendering that as a dry map is the failure the
                    // whole epic exists to make impossible.
                    PlanOutcome::Covered => {
                        let (frames, outside_window) =
                            self.read_plan(http, &manifest, &plan, corridor, now, &mut diagnostics);
                        // Nothing failed and nothing is left: every frame this generation publishes
                        // is outside the window the rain map answers. That is a different sentence
                        // from "the objects would not come", so it gets a different reason.
                        if frames.is_empty() && diagnostics.failed_frames == 0 && outside_window > 0 {
                            diagnostics.no_rain_map = Some(NoRainMap::OutsideWindow);
                        }
                        (!frames.is_empty()).then(|| (Lattice::from(&manifest.grid), frames))
                    }
                    PlanOutcome::OutOfDomain => {
                        diagnostics.no_rain_map = Some(NoRainMap::OutOfDomain);
                        None
                    }
                    PlanOutcome::Uncovered => {
                        diagnostics.no_rain_map = Some(NoRainMap::Uncovered);
                        None
                    }
                    PlanOutcome::Expired => {
                        diagnostics.no_rain_map = Some(NoRainMap::Expired);
                        None
                    }
                }
            }
            Err(_) => {
                diagnostics.no_rain_map = Some(NoRainMap::ServiceUnavailable);
                None
            }
        };

        // Everything above this line was addressed to the OBC service and carried no coordinate.
        // The counters are closed *here*, before MET, so "service cost" keeps meaning exactly
        // that — the manifest and the corridor's Range reads, nothing else.
        diagnostics.service_requests = http.requests().saturating_sub(before_requests);
        diagnostics.service_bytes = http.bytes().saturating_sub(before_bytes);
        diagnostics.cached_frames = self.frames.hits.saturating_sub(cache_hits_before);
        let (before_met_requests, before_met_bytes) = (http.requests(), http.bytes());

        // MET is the only request that carries the rider's coordinate — by design, and only ever
        // rounded to four decimals.
        let hourly = self.met.hourly(http, corridor.lat_udeg, corridor.lon_udeg, now).map_err(FetchError::Hourly)?;
        diagnostics.met_requests = http.requests().saturating_sub(before_met_requests);
        diagnostics.met_bytes = http.bytes().saturating_sub(before_met_bytes);
        diagnostics.attribution.push(met::ATTRIBUTION_TEXT.to_string());

        self.generation = self.generation.wrapping_add(1).max(1);
        let (bytes, report) = bundle::build(
            self.generation,
            request_id,
            now,
            (corridor.lat_udeg, corridor.lon_udeg),
            &corridor.bounds,
            scene.as_ref().map(|(lattice, frames)| Scene { lattice: *lattice, frames }),
            &hourly,
        )
        .map_err(FetchError::Build)?;
        diagnostics.dropped_oversize_frames = report.dropped_oversize;
        if report.frames == 0 && diagnostics.no_rain_map.is_none() {
            diagnostics.no_rain_map = Some(NoRainMap::FramesUnavailable);
        }
        Ok(Bundle { bytes, diagnostics })
    }

    /// Turn a plan into frames: fetch what exists, paint what is dry, count what failed.
    ///
    /// A frame is kept as long as *something* is known about it — a dry shard is knowledge, and a
    /// failed shard is a **hole in its frame**, not the loss of the frame. Dropping the frame would
    /// throw away the eight shards that did arrive to punish the one that did not; the hole is
    /// no-data, which is distinguishable from dry at every layer below, so keeping it cannot make
    /// an outage look rain-free. Only a frame where every present shard failed and nothing was dry
    /// disappears, and losing every frame that way is [`NoRainMap::FramesUnavailable`].
    fn read_plan<H: Http>(
        &mut self,
        http: &mut H,
        manifest: &Manifest,
        plan: &manifest_v2::Plan,
        corridor: &Corridor,
        now: i64,
        diagnostics: &mut Diagnostics,
    ) -> (Vec<FrameInput>, u32) {
        let origin = self.origin.clone();
        let mut frames: BTreeMap<u32, FrameInput> = BTreeMap::new();
        let mut outside_window = 0u32;
        // Frames outside the usable window are not fetched: two hours ahead is the question the
        // rain map answers, and an observation older than six hours would be a lie told with a
        // true timestamp. Both are properties of the timeline.
        let usable = |offset_min: u32, outside: &mut u32| -> Option<i64> {
            let frame = manifest.frame(offset_min)?;
            let inside = frame.valid_at <= now + HORIZON_S && frame.valid_at >= now - MAX_OBSERVATION_AGE_S;
            if !inside {
                *outside += 1;
            }
            inside.then_some(frame.valid_at)
        };
        // The quality flag follows the frame's **temporal nature**, not its content and not the
        // per-shard `observed` bits. An OBCW frame carries one flag for a mosaic that is radar over
        // the rider and model fill across the seam, so no content rule can be true of all of it —
        // and a content rule made an all-dry frame's flag depend on whether the baker happened to
        // publish an object, which is how the two clients came to disagree about the commonest scene
        // there is. So: the frame at offset 0 whose validity is within the dataset's own
        // `max_source_skew_s` of now is the **analysis** and says observed; every forward frame is a
        // forecast and says so. An all-dry radar scan is still an observation; an all-dry forecast
        // frame is still a forecast. The per-shard bits stay in the diagnostics, where they are true.
        //
        // **The temporal test is necessary and not sufficient, since WXR9** (#1251/#1278 m6). It
        // used to be the whole rule, and the baker could not then publish an f0 that was neither an
        // observation nor a real model step. It can now: in a region with only the hourly floor and
        // a cycle anchored off the hour — three quarters of them — `derive::uniform_frames` inserts
        // a **morphed** f0, correctly published `FLAG_FORECAST` with every shard's manifest
        // `observed` bit clear, and this rule showed it to the rider as an observation.
        //
        // So the manifest's bits get a veto, and only a veto: a frame may claim observed if the
        // temporal test passes **and** at least one of the published shards under it says observed.
        // That is deliberately not "all of them" — a corridor that is radar over the rider and model
        // across a seam is exactly the case the paragraph above exists to keep saying observed, and
        // an AND would flip it. And a frame with *no* published shards is the all-dry scene, whose
        // bits do not exist because a dry shard is not published; it keeps the temporal answer,
        // which is the whole point of not having a content rule. The veto can only ever clear the
        // flag, never set one, so nothing that was a forecast becomes an observation here.
        let skew = manifest.cadence.max_source_skew_s.max(0);
        let observed_frame = |offset_min: u32, valid_at: i64| offset_min == 0 && (now - valid_at).abs() <= skew;
        // `(published shards, observed among them)` per frame, for the veto.
        let mut provenance: BTreeMap<u32, (u32, u32)> = BTreeMap::new();

        for read in &plan.fetch {
            let Some(valid_at) = usable(read.offset_min, &mut outside_window) else { continue };
            let Some(geometry) = manifest.grid.shard_geometry(read.shard) else { continue };
            if read.observed {
                diagnostics.observed_shards += 1;
            }
            let counts = provenance.entry(read.offset_min).or_insert((0, 0));
            counts.0 += 1;
            counts.1 += u32::from(read.observed);
            let shard = ShardRead {
                key: read.key.clone(),
                geometry,
                bytes: read.bytes,
                object_crc32: read.object_crc32,
                valid_at,
                observed: read.observed,
            };
            let entry = frames.entry(read.offset_min).or_insert(FrameInput {
                valid_at,
                observed: observed_frame(read.offset_min, valid_at),
                ..FrameInput::default()
            });
            match corridor::crop_frame_cached(http, &origin, &shard, &corridor.bounds, &mut self.frames) {
                Ok(crop) => entry.crops.push(crop),
                // One bad shard is one hole in one frame, not a failed job.
                Err(_) => diagnostics.failed_frames += 1,
            }
        }
        for (offset_min, shard) in &plan.dry {
            let Some(valid_at) = usable(*offset_min, &mut outside_window) else { continue };
            let Some(bounds) = manifest.grid.shard_geometry(*shard).map(|geometry| geometry.bounds()) else {
                continue;
            };
            diagnostics.dry_shards += 1;
            frames
                .entry(*offset_min)
                .or_insert(FrameInput {
                    valid_at,
                    observed: observed_frame(*offset_min, valid_at),
                    ..FrameInput::default()
                })
                .dry
                .push(bounds);
        }
        // The manifest's veto, applied once every contributing shard is known (see `observed_frame`).
        for (offset_min, frame) in frames.iter_mut() {
            if let Some((published, observed)) = provenance.get(offset_min) {
                if *published > 0 && *observed == 0 {
                    frame.observed = false;
                }
            }
        }
        // A frame every one of whose shards failed is not a frame: it would be an all-no-data image
        // claiming a timestamp. It goes, and its absence is counted by `failed_frames` above.
        frames.retain(|_, frame| !frame.crops.is_empty() || !frame.dry.is_empty());
        (frames.into_values().collect(), outside_window)
    }
}
