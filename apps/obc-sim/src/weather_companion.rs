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
//! visible by passing OBCW validation and winning the generation comparison, so a simulator
//! run exercises the same accept/reject decisions the device makes on the glass.

use obc_ble::weather_request::{RequestContextBundle, RequestContextFacts, RequestContextFix};
use obc_ble::{
    classify_upload, BundleFacts, BundleIdentity, DueScheduler, UploadDisposition, WeatherRefresh,
    WeatherRequestContext, REASON_NO_BUNDLE, REASON_RETRY, REASON_SCHEDULED, REASON_URGENT, VALID_POSITION,
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
    /// A typed urgent request or scheduler request is pending.
    refreshing: bool,
    /// Whether the device has storage that could accept a bundle at all (`--no-card` clears it).
    /// §11.7's rule: no card ⇒ no requests, urgent included, because every upload would be
    /// answered `error` and the phone would burn its battery on the loop.
    store_ready: bool,
}

impl Default for SimCompanion {
    fn default() -> Self {
        Self::new(true)
    }
}

impl SimCompanion {
    pub fn new(store_ready: bool) -> Self {
        Self { scheduler: DueScheduler::new(), state: CompanionState::default(), refreshing: false, store_ready }
    }

    /// Queue the typed urgent request that the app asked the platform to raise.
    pub fn request_now(&mut self) {
        self.scheduler.open_weather();
        self.refreshing = true;
    }

    /// Whether typed urgent or cadence work is queued or pending.
    pub fn refreshing(&self) -> bool {
        self.refreshing
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
        // §11.8: the *raw* byte the rider's setting encodes, exactly as the board passes it. The
        // typed enum is only for the scheduler's own arithmetic; collapsing an unknown byte to the
        // default before it reaches the context would misreport the rider's cadence to the phone.
        let refresh_raw = app.settings().weather_refresh as u8;
        let fallback = (app.state.cam_lat, app.state.cam_lon);
        self.run(&app.weather_snapshot(), refresh_raw, store.and_then(held_of), fallback, live, now)
    }

    /// The lifecycle without the `App`: the scheduler's levels, the context fill, the fetch and
    /// the upload verdict. Split out so the §11.7 no-storage arm is testable on its own.
    pub fn run(
        &mut self,
        snapshot: &obc_app::ble::WeatherSnapshot,
        refresh_raw: u8,
        held: Option<RequestContextBundle>,
        fallback_position: (i32, i32),
        live: &mut LiveWeather,
        now: i64,
    ) -> Option<Vec<u8>> {
        let refresh = WeatherRefresh::from_u8(refresh_raw).unwrap_or(WeatherRefresh::DEFAULT);
        let facts = match held {
            Some(bundle) => {
                BundleFacts {
                    held: true,
                    age_s: u64::try_from((now - bundle.generated_at).max(0)).ok(),
                    // The simulator's compact bundle seam carries only the §11.4 identity, not
                    // validated window/frame metadata. Fail conservative and exercise the normal
                    // request path rather than inventing a local-reuse proof.
                    manual_reusable: false,
                    location_changed: false,
                    hourly_only: false,
                }
            }
            None => BundleFacts::NONE,
        };
        let now_s = now.max(0) as u64;
        let raise = self.scheduler.poll(now_s, refresh, snapshot.ride_active, self.store_ready, facts);
        self.state.pending_request_id = self.scheduler.pending_request_id();
        self.state.next_wake_s = self.scheduler.next_wake_s(refresh, snapshot.ride_active, self.store_ready);
        self.refreshing = self.state.pending_request_id.is_some();
        let raise = raise?;
        self.state.raises += 1;
        self.state.last_reason = raise.reason;

        // 1-2. The phone reads the request context and disconnects. Building it for real is the
        //      point: the context is what tells the companion *where* and *what for*.
        let context = WeatherRequestContext::raised(refresh_raw, raise, request_context_facts(snapshot, held));
        let position = if context.has(VALID_POSITION) {
            (context.lat_udeg, context.lon_udeg)
        } else {
            // No trusted fix: the phone falls back to its own last known position. The simulator's
            // stand-in is the camera, which is what the rider is looking at.
            fallback_position
        };
        // The corridor is a 90 km disc around wherever the device says the rider is. The bearing
        // and speed §11.4 carries are still read into the context — the device vouches for them and
        // the panel shows them — but nothing downstream projects a corridor from them any more:
        // under one uniform dataset a heading changes no answer, only the shape of the question.

        // 3. BLE is off while HTTP runs. Nothing here touches a radio, which is exactly the
        //    property the budget depends on.
        let bytes = live.fetch(position, now, context.request_id)?;

        // 4. Reconnect and upload. The disposition is the *firmware's* verdict, not ours.
        let incoming = held_from_bytes(&bytes)?;
        let disposition = classify_upload(bundle_identity(incoming), held.map(bundle_identity));
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
        self.refreshing = false;
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

fn held_of(store: &SimWeather) -> Option<RequestContextBundle> {
    let (generation, generated_at, crc32) = store.validated_identity();
    Some(RequestContextBundle { generation, generated_at, crc32 })
}

fn held_from_bytes(bytes: &[u8]) -> Option<RequestContextBundle> {
    let source = obc_formats::io::SliceSource(bytes);
    let reader = obc_weather::WeatherReader::open(&source).ok()?;
    let header = reader.header();
    Some(RequestContextBundle { generation: header.generation, generated_at: header.generated_at, crc32: header.crc32 })
}

fn request_context_facts(
    snapshot: &obc_app::ble::WeatherSnapshot,
    bundle: Option<RequestContextBundle>,
) -> RequestContextFacts {
    RequestContextFacts {
        fix: snapshot.position.map(|fix| RequestContextFix {
            lat_udeg: fix.lat_udeg,
            lon_udeg: fix.lon_udeg,
            fix_utc: fix.fix_utc,
        }),
        bearing_deg: snapshot.bearing_deg,
        speed_deci_ms: snapshot.speed_deci_ms,
        route_id: snapshot.route_id,
        bundle,
    }
}

fn bundle_identity(bundle: RequestContextBundle) -> BundleIdentity {
    BundleIdentity { generation: bundle.generation, generated_at: bundle.generated_at }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weather_live::{LiveConfig, LiveWeather};
    use obc_ble::VALID_BUNDLE;
    use obc_wx_client::http::FailureControls;

    /// A live client that can never reach a socket: every request fails at the transport, which is
    /// what makes "did the companion try at all?" observable in a unit test.
    fn offline_live() -> LiveWeather {
        LiveWeather::new(&LiveConfig {
            controls: FailureControls { offline: true, ..FailureControls::default() },
            ..LiveConfig::default()
        })
    }

    fn riding() -> obc_app::ble::WeatherSnapshot {
        obc_app::ble::WeatherSnapshot {
            ride_active: true,
            position: Some(obc_app::ble::WeatherFix { lat_udeg: 48_060_000, lon_udeg: 7_900_000, fix_utc: 1_800_000 }),
            ..Default::default()
        }
    }

    /// §11.7: a device with no storage raises **nothing** — not even the no-bundle request that
    /// the same levels raise immediately with a card. If it did, every upload would be answered
    /// `error` and the phone would spend its battery on the loop forever.
    #[test]
    fn no_card_means_no_request_and_no_http_at_all() {
        let mut companion = SimCompanion::new(false);
        let mut live = offline_live();
        let bytes = companion.run(&riding(), 2, None, (48_060_000, 7_900_000), &mut live, 1_800_000);
        assert!(bytes.is_none());
        assert_eq!(companion.state.raises, 0, "a card-less device must not raise a weather request");
        assert_eq!(companion.state.next_wake_s, None, "…and must not schedule one either");
        assert_eq!(live.total_requests(), 0, "no request may reach the wire");
    }

    /// The same levels *with* a card: the request is raised and the fetch is attempted. This is
    /// what makes the test above a statement about `store_ready` rather than about the fixture.
    #[test]
    fn with_a_card_the_same_levels_raise_and_fetch() {
        let mut companion = SimCompanion::new(true);
        let mut live = offline_live();
        let bytes = companion.run(&riding(), 2, None, (48_060_000, 7_900_000), &mut live, 1_800_000);
        assert!(bytes.is_none(), "the offline client cannot produce a bundle");
        assert_eq!(companion.state.raises, 1);
        assert!(live.total_requests() > 0, "the companion must actually go and fetch");
    }

    /// §11.4's last field and §11.8's refresh byte, both filled from the facts rather than from a
    /// default: the CRC identifies the bundle the device is holding, and the refresh byte is the
    /// rider's own setting carried verbatim — including a byte this build cannot name.
    #[test]
    fn the_context_carries_the_bundle_crc_and_the_raw_refresh_byte() {
        let held = RequestContextBundle { generation: 7, generated_at: 1_799_000, crc32: 0xDEAD_BEEF };
        let raise = obc_ble::Raise { request_id: 3, reason: REASON_SCHEDULED };
        let context = WeatherRequestContext::raised(9, raise, request_context_facts(&riding(), Some(held)));
        assert_eq!(context.refresh_raw, 9, "an unknown cadence byte rides through untouched (§11.8)");
        assert!(WeatherRefresh::from_u8(9).is_err(), "…and this build genuinely cannot name it");
        assert!(context.has(VALID_BUNDLE));
        assert_eq!(context.bundle_crc32, 0xDEAD_BEEF, "the held bundle's real CRC, not a zero");
        assert_eq!(context.bundle_generation, 7);
    }

    /// The corridor is the same 90 km disc whatever the device vouched for. A bearing and a speed
    /// used to stretch it into a cone, because containment made the shape decide which product
    /// answered; with one dataset there is nothing for a heading to change, and a rider who turns
    /// around must not get a different map.
    #[test]
    fn the_corridor_is_the_same_disc_whatever_the_context_vouches_for() {
        let mut moving = riding();
        moving.bearing_deg = Some(90);
        moving.speed_deci_ms = Some(80); // 8 m/s ≈ 29 km/h
        let mut companion = SimCompanion::new(true);
        let mut live = offline_live();
        companion.run(&moving, 2, None, (48_060_000, 7_900_000), &mut live, 1_800_000);
        let directed = live.report.corridor_km.expect("a corridor was asked about");

        let mut companion = SimCompanion::new(true);
        let mut live = offline_live();
        companion.run(&riding(), 2, None, (48_060_000, 7_900_000), &mut live, 1_800_000);
        let still = live.report.corridor_km.expect("a corridor was asked about");
        assert_eq!(directed, still, "a heading must not change the window");
        assert!((still.1 - 180.0).abs() < 1.0, "2 x 90 km north-south: {}", still.1);
    }
}
