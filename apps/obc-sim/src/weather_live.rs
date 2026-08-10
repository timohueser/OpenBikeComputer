//! `--weather live`: real weather, from the real service, through the production device path.
//!
//! The simulator does not learn a second way to draw rain. It fetches the **same bytes the phone
//! consumes** — `wx/v1/manifest.json` plus OBCG corridor Range reads plus MET hourly — assembles
//! the **same OBCW bundle** the phone would upload, and hands it to the same [`SimWeather`] the
//! deterministic fixtures use. Everything downstream of the bundle is untouched production code:
//! the WX7 reader, the A/B selector, the WX10 renderer, the WX11 screens.
//!
//! Two rules from the epic are visible here rather than assumed:
//!
//! - **The service never sees a coordinate.** The corridor derives from the rider's position, but
//!   the requests it turns into are key-addressed Range reads of immutable objects. MET is the one
//!   third party that receives the position, and only rounded to four decimals.
//! - **Live mode anchors on the real wall clock.** A fixture store anchors the app clock on its
//!   own first frame so previews are deterministic; doing that live would silently *hide staleness*
//!   — a baker that stopped an hour ago would render as a fresh nowcast. Live weather must age.

use obc_wx_client::http::{FailureControls, FaultyHttp, Http, UreqHttp};
use obc_wx_client::select::{Corridor, Fix};
use obc_wx_client::WeatherClient;

use crate::weather_store::SimWeather;

/// The `--weather live` knobs.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    pub service: String,
    /// `--weather-radius-km`: force an undirected disc of this radius. Absent (the default) means
    /// the corridor is **projected** from the fix the way the phone projects it — bearing and
    /// speed included — so the simulator and the phone can select different tiers only when they
    /// genuinely disagree, never because the simulator asked a rounder question.
    pub radius_km: Option<f64>,
    pub controls: FailureControls,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            service: obc_wx_client::DEFAULT_SERVICE_URL.to_string(),
            radius_km: None,
            controls: FailureControls::default(),
        }
    }
}

/// What the last live fetch did — the dev-panel evidence, deliberately **outside** the emulated
/// device pixels. The device itself never learns which product it is looking at.
#[derive(Debug, Clone, Default)]
pub struct LiveReport {
    pub fetched_at: i64,
    pub bundle_bytes: usize,
    /// This fetch's **coordinate-free** cost: the manifest plus the corridor Range reads. MET is
    /// counted separately below, because mixing them makes both numbers meaningless — MET's one
    /// document dwarfs a whole corridor's worth of Range reads.
    pub service_requests: u32,
    pub service_bytes: u64,
    pub met_requests: u32,
    pub met_bytes: u64,
    /// Frames this fetch answered from the crop cache — immutable objects it did not re-read.
    pub cached_frames: u32,
    /// Every request this process has made, all sources, since it started.
    pub total_requests: u32,
    /// The corridor this fetch asked about: `(width_km, height_km, directed)`.
    pub corridor_km: Option<(f64, f64, bool)>,
    pub product: Option<(String, u8)>,
    pub expired: Vec<String>,
    pub no_rain_map: Option<String>,
    pub attribution: Vec<String>,
    pub failed_frames: u32,
    pub dropped_incompatible_frames: u32,
    pub error: Option<String>,
}

/// The live source: a `WeatherClient` plus the fetch cadence and the last report.
pub struct LiveWeather {
    client: WeatherClient,
    http: FaultyHttp<UreqHttp>,
    /// `--weather-radius-km`, when the rider asked for a fixed disc instead of a projection.
    forced_radius_m: Option<f64>,
    pub report: LiveReport,
    last_fetch: Option<i64>,
    last_position: Option<(i32, i32)>,
}

impl LiveWeather {
    pub fn new(config: &LiveConfig) -> Self {
        Self {
            client: WeatherClient::new(config.service.clone()),
            http: FaultyHttp::new(UreqHttp::new(), config.controls.clone()),
            forced_radius_m: config.radius_km.map(|km| km * 1_000.0),
            report: LiveReport::default(),
            last_fetch: None,
            last_position: None,
        }
    }

    /// Every request this process has made — the counter the §11.7 "no card, no requests" check
    /// reads, and the panel's running total.
    pub fn total_requests(&self) -> u32 {
        self.http.requests()
    }

    /// The corridor for `fix`: the phone's projection, unless `--weather-radius-km` forced a disc.
    fn corridor(&self, fix: &Fix) -> Corridor {
        match self.forced_radius_m {
            Some(radius) => Corridor::around(fix.lat_udeg, fix.lon_udeg, radius),
            None => Corridor::projected(fix),
        }
    }

    /// One fetch. On failure the previous bundle stays in place — a rider keeps the weather they
    /// had, visibly aging, rather than losing the screen to an outage.
    pub fn fetch(&mut self, fix: &Fix, now: i64, request_id: u32) -> Option<Vec<u8>> {
        let corridor = self.corridor(fix);
        self.last_fetch = Some(now);
        self.last_position = Some((fix.lat_udeg, fix.lon_udeg));
        let corridor_km = Some((
            (corridor.bounds.east_udeg - corridor.bounds.west_udeg) as f64 / 1e6
                * 111.32
                * (f64::from(fix.lat_udeg) / 1e6).to_radians().cos().max(0.05),
            (corridor.bounds.north_udeg - corridor.bounds.south_udeg) as f64 / 1e6 * 111.32,
            !corridor.undirected,
        ));
        match self.client.fetch(&mut self.http, &corridor, now, request_id) {
            Ok(bundle) => {
                let diagnostics = &bundle.diagnostics;
                self.report = LiveReport {
                    fetched_at: now,
                    bundle_bytes: bundle.bytes.len(),
                    service_requests: diagnostics.service_requests,
                    service_bytes: diagnostics.service_bytes,
                    met_requests: diagnostics.met_requests,
                    met_bytes: diagnostics.met_bytes,
                    cached_frames: diagnostics.cached_frames,
                    total_requests: self.http.requests(),
                    corridor_km,
                    product: diagnostics.product.clone(),
                    expired: diagnostics.expired_products.clone(),
                    no_rain_map: diagnostics.no_rain_map.clone(),
                    attribution: diagnostics.attribution.clone(),
                    failed_frames: diagnostics.failed_frames,
                    dropped_incompatible_frames: diagnostics.dropped_incompatible_frames,
                    error: None,
                };
                Some(bundle.bytes)
            }
            Err(error) => {
                self.report.error = Some(error.to_string());
                self.report.total_requests = self.http.requests();
                self.report.corridor_km = corridor_km;
                None
            }
        }
    }
}

/// What `--weather` resolved to.
pub struct WeatherSource {
    pub store: Option<SimWeather>,
    /// Present only in live mode; drives the refresh cadence and the dev-panel report.
    pub live: Option<LiveWeather>,
    /// The instant the app clock anchors on when no `--clock` was passed.
    pub clock_anchor: Option<i64>,
}

/// Resolve `--weather`: `live` fetches from the service, everything else is the existing
/// fixture/demo path.
pub fn build(
    arg: &str,
    now_override: Option<i64>,
    map_bbox: (i32, i32, i32, i32),
    config: &LiveConfig,
    position: (i32, i32),
    store_ready: bool,
) -> WeatherSource {
    if arg != "live" {
        let store = SimWeather::from_arg(arg, now_override, map_bbox);
        let clock_anchor = store.as_ref().and_then(|store| store.effective_now());
        return WeatherSource { store, live: None, clock_anchor };
    }
    // Live weather is dated by the real clock, not by its own newest frame: a bundle whose
    // freshest observation is 40 minutes old must read WEATHER UPDATE NEEDED, and it only can if
    // "now" is genuinely now. `--weather-now` still wins, because that is the deterministic
    // stale-scenario tool.
    let now = now_override.unwrap_or_else(|| chrono::Utc::now().timestamp());
    let mut live = LiveWeather::new(config);
    // §11.7: a device with no storage raises no request, so the companion never fetches — and the
    // seed fetch is that same request. `--no-card` therefore issues no HTTP at all, which is the
    // observable form of the rule.
    if !store_ready {
        return WeatherSource { store: None, live: Some(live), clock_anchor: Some(now) };
    }
    // The seed fetch has no ride behind it yet: no bearing, no speed, no route — so this is the
    // undirected disc, exactly as a phone answering a device that vouches for neither.
    let fix = Fix { lat_udeg: position.0, lon_udeg: position.1, ..Fix::default() };
    let store = live.fetch(&fix, now, 1).and_then(|bytes| SimWeather::from_bytes(bytes, now_override));
    if store.is_none() {
        eprintln!(
            "--weather live: no bundle ({})",
            live.report.error.as_deref().unwrap_or("the service answered but the bundle was unreadable")
        );
    }
    WeatherSource { store, live: Some(live), clock_anchor: Some(now) }
}
