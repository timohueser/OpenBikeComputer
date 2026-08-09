//! The simulated companion: the §11 weather request/upload lifecycle, driven by the **real**
//! firmware scheduler.
//!
//! The simulator does not decide when weather is due. `obc_ble::DueScheduler` does — the same
//! type the board runs, fed the same facts: the app's own request context (ride state, fix,
//! bearing, route id, bundle identity), the rider's configured refresh interval, whether a store
//! is present, and how old the held bundle is. When it raises a request the companion does what
//! the phone does — reads the context, "disconnects", fetches over HTTP, "reconnects", uploads —
//! and the upload is classified by `obc_ble::classify_upload`, exactly as the board classifies it.
//!
//! What this deliberately does **not** do: inject weather into the UI. A bundle only becomes
//! visible by passing OBCW validation and winning the A/B generation comparison, so a simulator
//! run exercises the same accept/reject decisions the device makes on the glass.

use obc_ble::{
    classify_upload, BundleFacts, BundleIdentity, DueScheduler, Raise, UploadDisposition, WeatherRefresh,
    WeatherRequestContext, REASON_NO_BUNDLE, REASON_RETRY, REASON_SCHEDULED, REASON_URGENT, VALID_BEARING,
    VALID_BUNDLE, VALID_POSITION, VALID_ROUTE, VALID_SPEED,
};

use crate::weather_live::LiveWeather;
use crate::weather_store::SimWeather;

/// What the last lifecycle pass did, for the dev panel. Outside the emulated device pixels.
#[derive(Debug, Clone, Default)]
pub struct CompanionState {
    pub pending_request_id: Option<u32>,
    pub last_reason: u16,
    pub next_wake_s: Option<u64>,
    pub raises: u32,
    pub commits: u32,
    pub rejected: u32,
    pub last_disposition: Option<&'static str>,
}

impl CompanionState {
    /// The raised request's reason bits, spelled out — the panel shows why the device is asking.
    pub fn reason_text(&self) -> String {
        let mut parts = Vec::new();
        for (bit, name) in [
            (REASON_SCHEDULED, "scheduled"),
            (REASON_URGENT, "urgent"),
            (REASON_RETRY, "retry"),
            (REASON_NO_BUNDLE, "no-bundle"),
        ] {
            if self.last_reason & bit != 0 {
                parts.push(name);
            }
        }
        if parts.is_empty() {
            "resting".into()
        } else {
            parts.join("+")
        }
    }
}

/// The host half of the lifecycle.
pub struct SimCompanion {
    scheduler: DueScheduler,
    pub state: CompanionState,
    /// Rising-edge detector for "the rider opened Weather" — the board's `request_weather_now()`
    /// with no board to call it.
    on_weather_screen: bool,
}

impl Default for SimCompanion {
    fn default() -> Self {
        Self::new()
    }
}

impl SimCompanion {
    pub fn new() -> Self {
        Self { scheduler: DueScheduler::new(), state: CompanionState::default(), on_weather_screen: false }
    }

    /// One pass of the whole lifecycle. Returns fresh bundle bytes when an upload committed.
    ///
    /// `store` is the currently held bundle (for the §11.4 bundle-identity fields and the
    /// scheduler's age arithmetic); `None` means the device holds nothing, which the scheduler
    /// turns into `REASON_NO_BUNDLE` and an immediate request.
    pub fn poll(
        &mut self,
        app: &obc_app::App,
        store: Option<&SimWeather>,
        live: &mut LiveWeather,
        now: i64,
    ) -> Option<Vec<u8>> {
        // Urgent-on-open: the board arms this from the Weather screen's open. The simulator
        // watches the same event through the screen stack rather than growing a simulator-only
        // hook inside obc-app.
        let on_weather = app.top_screen().name().starts_with("Weather");
        if on_weather && !self.on_weather_screen {
            self.scheduler.open_weather();
        }
        self.on_weather_screen = on_weather;

        let snapshot = app.weather_snapshot();
        let refresh = refresh_of(app);
        let held = store.and_then(identity_of);
        let facts = match held {
            Some(identity) => {
                BundleFacts { held: true, age_s: u64::try_from((now - identity.generated_at).max(0)).ok() }
            }
            None => BundleFacts::NONE,
        };
        let now_s = now.max(0) as u64;
        // The store is always "ready" in the simulator — the host filesystem is the card. The
        // no-card case is reachable through the ordinary boot-fault controls.
        let raise = self.scheduler.poll(now_s, refresh, snapshot.ride_active, true, facts);
        self.state.pending_request_id = self.scheduler.pending_request_id();
        self.state.next_wake_s = self.scheduler.next_wake_s(refresh, snapshot.ride_active, true);
        let raise = raise?;
        self.state.raises += 1;
        self.state.last_reason = raise.reason;

        // 1-2. The phone reads the request context and disconnects. Building it for real is the
        //      point: the context is what tells the companion *where* and *what for*.
        let context = build_context(&snapshot, refresh, &raise, held);
        let position = if context.has(VALID_POSITION) {
            (context.lat_udeg, context.lon_udeg)
        } else {
            // No trusted fix: the phone falls back to its own last known position. The simulator's
            // stand-in is the camera, which is what the rider is looking at.
            (app.state.cam_lat, app.state.cam_lon)
        };

        // 3. BLE is off while HTTP runs. Nothing here touches a radio, which is exactly the
        //    property the budget depends on.
        let bytes = live.fetch(position, now, context.request_id)?;

        // 4. Reconnect and upload. The disposition is the *firmware's* verdict, not ours.
        let incoming = identity_from_bytes(&bytes)?;
        let disposition = classify_upload(incoming, held);
        self.state.last_disposition = Some(match disposition {
            UploadDisposition::Commit => "commit",
            UploadDisposition::DuplicateIgnored => "duplicate ignored",
            UploadDisposition::StaleIgnored => "stale ignored",
        });
        // §11: *any* accepted upload finishes the request — including one the store then refuses
        // as not-newer. Pacing follows acceptance, not novelty, or a device holding a current
        // bundle would retry forever.
        self.scheduler.commit_succeeded(now_s);
        self.state.pending_request_id = None;
        match disposition {
            UploadDisposition::Commit => {
                self.state.commits += 1;
                Some(bytes)
            }
            _ => {
                self.state.rejected += 1;
                None
            }
        }
    }
}

fn refresh_of(app: &obc_app::App) -> WeatherRefresh {
    WeatherRefresh::from_u8(app.settings().weather_refresh as u8).unwrap_or(WeatherRefresh::DEFAULT)
}

fn identity_of(store: &SimWeather) -> Option<BundleIdentity> {
    identity_from_bytes(store.bytes())
}

fn identity_from_bytes(bytes: &[u8]) -> Option<BundleIdentity> {
    let source = obc_formats::io::SliceSource(bytes);
    let reader = obc_weather::WeatherReader::open(&source).ok()?;
    let header = reader.header();
    Some(BundleIdentity { generation: header.generation, generated_at: header.generated_at })
}

/// The §11.4 context fill, flags-not-sentinels: a field is present only when it is *true*, and
/// the validity bit is what says so. This mirrors the board's `build_context`.
fn build_context(
    snapshot: &obc_app::ble::WeatherSnapshot,
    refresh: WeatherRefresh,
    raise: &Raise,
    bundle: Option<BundleIdentity>,
) -> WeatherRequestContext {
    let mut context = WeatherRequestContext {
        request_id: raise.request_id,
        reason: raise.reason,
        refresh_raw: refresh.as_u8(),
        ..WeatherRequestContext::EMPTY
    };
    if let Some(fix) = snapshot.position {
        context.validity |= VALID_POSITION;
        context.lat_udeg = fix.lat_udeg;
        context.lon_udeg = fix.lon_udeg;
        context.fix_utc = fix.fix_utc;
    }
    if let Some(bearing) = snapshot.bearing_deg {
        context.validity |= VALID_BEARING;
        context.bearing_deg = bearing;
    }
    if let Some(speed) = snapshot.speed_deci_ms {
        context.validity |= VALID_SPEED;
        context.speed_deci_ms = speed;
    }
    if let Some(route) = snapshot.route_id {
        context.validity |= VALID_ROUTE;
        context.route_id = route;
    }
    if let Some(identity) = bundle {
        context.validity |= VALID_BUNDLE;
        context.bundle_generation = identity.generation;
        context.bundle_generated_at = identity.generated_at;
    }
    context
}
