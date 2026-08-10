//! The host weather client: what the phone does, in Rust, for the simulator.
//!
//! ```text
//!   manifest.json  ──►  select a product      (containment + freshness + frames; never an id)
//!        │                    │
//!        │                    ▼
//!        │            OBCG corridor reads     (header + covering pages + needed tiles, Range)
//!        │                    │
//!        ▼                    ▼
//!   MET hourly  ────────►  OBCW bundle         (shared obc-formats encoder; device-readable)
//! ```
//!
//! Two boundaries are load-bearing and are enforced here, not by convention:
//!
//! - **The service never receives a coordinate.** Every OBC request is a key-addressed `GET` or
//!   Range read of a static object. The corridor *derives* from the rider's position, but the
//!   position itself only ever leaves this process in a MET query — the single third party the
//!   epic allows to see one.
//! - **Nothing branches on a product id.** Coverage, tier and freshness are manifest data, so a
//!   new region is a baker deploy and never a client release.
//!
//! This is a second implementation of the contract the iOS companion implements; the phone stays
//! the reference. Where the two could drift, this crate's tests pin the shared vectors both read.

pub mod bundle;
pub mod corridor;
pub mod http;
pub mod manifest;
pub mod met;
pub mod select;

use corridor::Crop;
use http::{Http, Request, MANIFEST_CAP};
use manifest::Manifest;
use select::{Corridor, NoRainMap};

/// The production service origin.
pub const DEFAULT_SERVICE_URL: &str = "https://wx.openbikecomputer.com";

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
    /// Frames answered from the in-process crop cache, costing no request at all.
    pub cached_frames: u32,
    pub skipped_manifest_products: usize,
    /// Covering products that were past their staleness deadline when we looked.
    pub expired_products: Vec<String>,
    pub clock_skew_suspected: bool,
    /// Frames that failed to fetch or verify. A frame is dropped, never faked.
    pub failed_frames: u32,
    pub dropped_incompatible_frames: u32,
    pub dropped_oversize_frames: u32,
    /// The chosen product's manifest id and tier, for the dev panel. Not on the device.
    pub product: Option<(String, u8)>,
    pub attribution: Vec<String>,
    /// Why there is no rain map, when there isn't one.
    pub no_rain_map: Option<String>,
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

/// The stateful client: it holds the manifest cache (60 s per OBCG §10, ETag-revalidated) and the
/// MET cache (whose `Expires` is absolute), because the throttle rules *are* those caches.
pub struct WeatherClient {
    origin: String,
    met: met::MetClient,
    manifest_cache: Option<CachedManifest>,
    /// Cropped frames, keyed on `(object key, window)`. Frame objects are immutable by the
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

    /// The manifest, honoring the 60 s window and revalidating with the stored ETag past it. An
    /// unreachable service with a cached manifest is not an outage; a cold cache is.
    pub fn manifest<H: Http>(&mut self, http: &mut H, now: i64) -> Result<Manifest, manifest::ManifestError> {
        // The backwards-clock guard matters: without it a clock that jumped back would pin a
        // stale manifest forever instead of refetching.
        if let Some(cached) = &self.manifest_cache {
            if now >= cached.fetched_at && now - cached.fetched_at < manifest::FRESHNESS_WINDOW_S {
                return Ok(cached.manifest.clone());
            }
        }
        let url = corridor::join(&self.origin, manifest::MANIFEST_KEY);
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
                    None => Err(manifest::ManifestError::Malformed(error.to_string())),
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
                None => Err(manifest::ManifestError::Malformed(format!("status {}", response.status))),
            };
        }
        let parsed = manifest::parse(&response.body)?;
        self.manifest_cache = Some(CachedManifest { manifest: parsed.clone(), etag: response.etag, fetched_at: now });
        Ok(parsed)
    }

    /// The whole job: manifest → selection → corridor reads → MET → OBCW.
    ///
    /// The rain half degrades on its own. A service outage, an expired product or an uncovered
    /// corridor all produce a truthful *hourly-only* bundle with the reason recorded — never a
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

        let crops = match self.manifest(http, now) {
            Ok(manifest) => {
                diagnostics.skipped_manifest_products = manifest.skipped_products;
                let (chosen, report) = select::select(&manifest, corridor, now);
                diagnostics.expired_products = report.expired;
                diagnostics.clock_skew_suspected = report.clock_skew_suspected;
                match chosen {
                    Ok(product) => {
                        diagnostics.product = Some((product.id.clone(), product.tier));
                        diagnostics.attribution.push(product.attribution.text.clone());
                        self.read_corridor(http, product, corridor, now, &mut diagnostics)
                    }
                    Err(reason) => {
                        diagnostics.no_rain_map = Some(reason.to_string());
                        Vec::new()
                    }
                }
            }
            Err(error) => {
                diagnostics.no_rain_map = Some(format!("{}: {error}", NoRainMap::ServiceUnavailable));
                Vec::new()
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
            &crops,
            &hourly,
        )
        .map_err(FetchError::Build)?;
        diagnostics.dropped_incompatible_frames = report.dropped_incompatible;
        diagnostics.dropped_oversize_frames = report.dropped_oversize;
        if report.frames == 0 && diagnostics.no_rain_map.is_none() {
            diagnostics.no_rain_map = Some(NoRainMap::FramesUnavailable.to_string());
        }
        Ok(Bundle { bytes, diagnostics })
    }

    fn read_corridor<H: Http>(
        &mut self,
        http: &mut H,
        product: &manifest::Product,
        corridor: &Corridor,
        now: i64,
        diagnostics: &mut Diagnostics,
    ) -> Vec<Crop> {
        let mut crops = Vec::new();
        let origin = self.origin.clone();
        for frame in select::usable_frames(product, now) {
            match corridor::crop_frame_cached(http, &origin, frame, &corridor.bounds, &mut self.frames) {
                Ok(crop) => crops.push(crop),
                // One bad frame is one missing timestamp, not a failed job. Only losing every
                // frame becomes "frames unavailable".
                Err(_) => diagnostics.failed_frames += 1,
            }
        }
        if crops.is_empty() {
            diagnostics.no_rain_map = Some(NoRainMap::FramesUnavailable.to_string());
        }
        crops
    }
}
