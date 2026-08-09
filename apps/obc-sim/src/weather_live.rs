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
use obc_wx_client::select::Corridor;
use obc_wx_client::WeatherClient;

use crate::weather_store::SimWeather;

/// The `--weather live` knobs.
#[derive(Debug, Clone)]
pub struct LiveConfig {
    pub service: String,
    pub radius_km: f64,
    pub controls: FailureControls,
}

impl Default for LiveConfig {
    fn default() -> Self {
        Self {
            service: obc_wx_client::DEFAULT_SERVICE_URL.to_string(),
            // The phone's undirected disc: 10 km reach floor + the 5 km lateral margin. Small
            // enough that a corridor read is a handful of KB, wide enough to see a front arrive.
            radius_km: 15.0,
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
    pub requests: u32,
    pub service_bytes: u64,
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
    radius_m: f64,
    pub report: LiveReport,
    last_fetch: Option<i64>,
    last_position: Option<(i32, i32)>,
}

impl LiveWeather {
    pub fn new(config: &LiveConfig) -> Self {
        Self {
            client: WeatherClient::new(config.service.clone()),
            http: FaultyHttp::new(UreqHttp::new(), config.controls.clone()),
            radius_m: config.radius_km * 1_000.0,
            report: LiveReport::default(),
            last_fetch: None,
            last_position: None,
        }
    }

    /// One fetch. On failure the previous bundle stays in place — a rider keeps the weather they
    /// had, visibly aging, rather than losing the screen to an outage.
    pub fn fetch(&mut self, position: (i32, i32), now: i64, request_id: u32) -> Option<Vec<u8>> {
        let corridor = Corridor::around(position.0, position.1, self.radius_m);
        self.last_fetch = Some(now);
        self.last_position = Some(position);
        match self.client.fetch(&mut self.http, &corridor, now, request_id) {
            Ok(bundle) => {
                let diagnostics = &bundle.diagnostics;
                self.report = LiveReport {
                    fetched_at: now,
                    bundle_bytes: bundle.bytes.len(),
                    requests: self.http.requests(),
                    service_bytes: diagnostics.service_bytes,
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
                self.report.requests = self.http.requests();
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
    let store = live.fetch(position, now, 1).and_then(|bytes| SimWeather::from_bytes(bytes, now_override));
    if store.is_none() {
        eprintln!(
            "--weather live: no bundle ({})",
            live.report.error.as_deref().unwrap_or("the service answered but the bundle was unreadable")
        );
    }
    WeatherSource { store, live: Some(live), clock_anchor: Some(now) }
}
