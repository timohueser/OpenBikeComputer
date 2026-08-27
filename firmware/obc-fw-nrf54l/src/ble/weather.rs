//! The **weather due plane** (WX8, #1193): the board half of the intermittent weather lifecycle —
//! the loop that decides *when* a request is raised, fills the §11.4 request context with the real
//! rider/bundle facts, and arms the WX3 advertising hint.
//!
//! The decisions live in the host-tested [`obc_ble::DueScheduler`] (ride/urgent/retry/commit
//! matrix, pinned against a synthetic clock); this module is only the plumbing around it:
//!
//! - **Inputs** cross the plane boundary the same way every other App fact does: the ride loop
//!   distils an [`obc_app::ble::WeatherSnapshot`] once per pass ([`set_weather_inputs`], the reverse
//!   direction of `app_ble_status`), and the Config write path pokes [`note_settings_changed`].
//! - **Outputs** are exactly two: `server.set` on the Weather Request context attribute (so the
//!   next authenticated read serves this request), and [`super::state::arm_weather_request`] (the
//!   bounded advertising swap the lifecycle loop already honours — budget expiry and the
//!   served-read clear are its, per §11.3, not this module's).
//!
//! Nothing here is periodic: the task sleeps until the scheduler's own next instant or an event
//! edge, so an idle, not-riding device costs no wakeups at all.
//!
//! Weather bundles are flat-store kind 4 objects. The scheduler caches the validated active head,
//! wakes on every catalog movement, and satisfies a pending request only when the new head carries
//! that exact request id. Route/trip/map commits therefore wake the task harmlessly without ending
//! a weather retry ladder.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use obc_app::ble::WeatherSnapshot;
use obc_ble::{
    BundleFacts, DueScheduler, Raise, WeatherRefresh, WeatherRequestContext, VALID_BEARING, VALID_BUNDLE,
    VALID_POSITION, VALID_ROUTE, VALID_SPEED,
};

/// The deployed dataset advances on quarter-hour boundaries. A bundle built in one interval cannot
/// be superseded before the next boundary; two minutes cover the timer's randomized delay plus the
/// normal manifest publication lag. A conditional phone check handles anything later than that.
const SERVICE_CADENCE_S: i64 = 15 * 60;
const PUBLICATION_GRACE_S: i64 = 2 * 60;
/// Point-forecast locality chosen for the first on-device reuse policy. Rain coverage is much wider,
/// but moving farther than this can materially change MET's point answer.
const LOCATION_REUSE_RADIUS_M: f32 = 2_000.0;

use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

use super::gatt::Server;
use super::state;

/// The app-side context inputs, pushed by the ride loop once per pass (last-writer-wins — the
/// scheduler reads whatever is freshest when it wakes).
const EMPTY_SNAPSHOT: WeatherSnapshot = WeatherSnapshot {
    ride_active: false,
    position: None,
    bearing_deg: None,
    speed_deci_ms: None,
    route_id: None,
    now_utc: None,
};
static SNAPSHOT: BlockingMutex<CriticalSectionRawMutex, Cell<WeatherSnapshot>> =
    BlockingMutex::new(Cell::new(EMPTY_SNAPSHOT));

/// The scheduler task's wake edge — rung by every event below. Level + latest-state: a burst of
/// edges wakes the task once and it re-reads the current levels, so nothing here queues.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The rider opened Weather (WX11 wires the screen; the seam exists now so the screen PR is
/// UI-only) — an urgent request, honoured even outside a ride and with refresh `Off`.
static URGENT: AtomicBool = AtomicBool::new(false);
/// UI-facing level from dashboard entry/scheduler raise through commit or request lapse.
static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// The live request id, mirrored out of the task so the synchronous command handler can reject a
/// crossed `weatherUnchanged` acknowledgement before answering `ok`.
static PENDING_REQUEST_ID: AtomicU32 = AtomicU32::new(0);
static UNCHANGED: BlockingMutex<CriticalSectionRawMutex, Cell<Option<(u32, u16)>>> =
    BlockingMutex::new(Cell::new(None));

/// Push the app-side weather context snapshot across the plane boundary (ride loop, once per pass
/// — one small `Cell` store). Wakes the scheduler only on the edges it keys on (ride state, the
/// active route), never at the 1 Hz fix cadence.
pub fn set_weather_inputs(s: WeatherSnapshot) {
    let material_change = SNAPSHOT.lock(|c| {
        let prev = c.get();
        c.set(s);
        prev.ride_active != s.ride_active || prev.route_id != s.route_id
    });
    if material_change {
        WAKE.signal(());
    }
}

/// The rider opened Weather: raise an urgent request now (spec §11.4 reason bit 1). The board ride
/// loop calls this on the non-weather → Weather-dashboard transition; returning from one of the
/// dashboard's child surfaces does not re-arm it.
pub fn request_weather_now() {
    URGENT.store(true, Ordering::Relaxed);
    IN_FLIGHT.store(true, Ordering::Relaxed);
    WAKE.signal(());
}

/// Whether a fetch is running — the level the ride loop reports as
/// `ExternalFacts::note_weather_refreshing` once per pass, so both this plane's edges (a raise
/// here, a commit or lapse in the loop below) reach `WeatherDomain`, which owns the cue.
pub fn refreshing() -> bool {
    IN_FLIGHT.load(Ordering::Relaxed)
}

/// Accept a compact phone-side "both sources unchanged" acknowledgement. The command handler calls
/// this synchronously; the task owns the scheduler mutation and is woken through the ordinary edge.
pub(crate) fn note_unchanged(request_id: u32, retry_after_s: u16) -> bool {
    if request_id == 0 || PENDING_REQUEST_ID.load(Ordering::Relaxed) != request_id {
        return false;
    }
    UNCHANGED.lock(|slot| slot.set(Some((request_id, retry_after_s))));
    WAKE.signal(());
    true
}

/// The persisted `weather_refresh` setting moved (a Config write) — re-derive due state now.
pub(crate) fn note_settings_changed() {
    WAKE.signal(());
}

/// A flat-store commit/delete landed. The task revalidates the weather head and decides whether
/// it is the response to the pending request; this edge alone never claims success.
pub(crate) fn note_catalog_changed() {
    WAKE.signal(());
}

/// The due-scheduler loop. Joined into `ble::run`'s task set for the stack's whole life; every
/// local is small (the context is 52 bytes) so it adds no meaningful poll-frame weight (#677).
pub(crate) async fn run(server: &Server<'_>, store: &RefCell<ObjectStore>, _shared: &SharedStoreMutex) -> ! {
    let mut sched = DueScheduler::new();
    let flat = crate::flat_store::mounted();
    let mut seen_sequence = flat.map(|store| store.sequence());
    let mut bundle = flat.and_then(|store| crate::flat_store::active_weather(store).ok().flatten());
    // Whether the GATT attribute currently carries a live request (vs. the §11.4 resting value).
    let mut context_live = false;
    // The refresh byte the attribute currently serves — `None` until this task's first write, so
    // the first pass re-asserts the resting value even though `run` seeded one at boot (#1221 F2:
    // a Config write between seed and first pass must never leave a stale byte served).
    let mut served_refresh: Option<u8> = None;
    loop {
        let now_s = Instant::now().as_secs();
        let sequence = flat.map(|store| store.sequence());
        if sequence != seen_sequence {
            if let Some(flat) = flat {
                if let Ok(next) = crate::flat_store::active_weather(flat) {
                    if next != bundle {
                        bundle = next;
                        let pending = PENDING_REQUEST_ID.load(Ordering::Relaxed);
                        if pending != 0 && bundle.is_some_and(|weather| weather.header().request_id == pending) {
                            sched.commit_succeeded(now_s);
                            PENDING_REQUEST_ID.store(0, Ordering::Relaxed);
                            info!("ble: [weather] matching flat bundle committed — request satisfied");
                        }
                    }
                    seen_sequence = sequence;
                }
            }
        }
        if let Some((request_id, retry_after_s)) = UNCHANGED.lock(|slot| slot.take()) {
            if sched.unchanged_succeeded(request_id, now_s, retry_after_s) {
                IN_FLIGHT.store(false, Ordering::Relaxed);
                PENDING_REQUEST_ID.store(0, Ordering::Relaxed);
                info!("ble: [weather] sources unchanged — request satisfied without bundle upload");
            }
        }
        if URGENT.swap(false, Ordering::Relaxed) {
            sched.open_weather();
        }
        let snapshot = SNAPSHOT.lock(|c| c.get());
        // The persisted setting is obc-app's typed enum whose discriminant IS the §11.8 wire
        // byte (pinned), so the fallback is unreachable.
        let refresh_raw = store.borrow().settings().weather_refresh as u8;
        let refresh = WeatherRefresh::from_u8(refresh_raw).unwrap_or(WeatherRefresh::DEFAULT);
        // The active bundle's identity — the boot/commit-refreshed selection, no card I/O — and
        // whether storage exists at all (#1221 F5: no card ⇒ no requests, or the phone burns).
        let store_ready = flat.is_some_and(|store| store.mode().readable());
        let candidate = bundle.map(crate::flat_store::FlatWeather::candidate);
        let policy = bundle.map(crate::flat_store::FlatWeather::header);
        let (location_changed, source_current) = match (candidate, policy, snapshot.now_utc) {
            (Some(b), Some(policy), Some(now_utc)) => {
                let centre_lat = (policy.south_lat_udeg as i64 + policy.north_lat_udeg as i64) / 2;
                let centre_lon = (policy.west_lon_udeg as i64 + policy.east_lon_udeg as i64) / 2;
                let moved = snapshot.position.is_some_and(|fix| {
                    obc_map_scene::ground_dist_m((centre_lon as i32, centre_lat as i32), (fix.lon_udeg, fix.lat_udeg))
                        > LOCATION_REUSE_RADIUS_M
                });
                let boundary = b.generated_at.div_euclid(SERVICE_CADENCE_S) * SERVICE_CADENCE_S;
                let boundary_safe_at = boundary.saturating_add(PUBLICATION_GRACE_S);
                // A build inside the publication grace might still contain the preceding manifest,
                // so it is reusable only until the grace ends and then gets one phone-side probe.
                // A build after the grace has seen the current interval and is safe until the next.
                let next_probe = if b.generated_at < boundary_safe_at {
                    boundary_safe_at
                } else {
                    boundary_safe_at.saturating_add(SERVICE_CADENCE_S)
                };
                (moved, i64::from(now_utc) < next_probe)
            }
            // No fresh fix is not evidence that the rider moved. No trusted clock, however, cannot
            // prove the source is still inside its publication interval.
            (Some(_), _, None) => (false, false),
            _ => (false, false),
        };
        let facts = BundleFacts {
            held: bundle.is_some(),
            // Age only with a trusted clock; the scheduler treats unknown age conservatively.
            age_s: match (candidate, snapshot.now_utc) {
                (Some(b), Some(now_utc)) => Some((now_utc as i64 - b.generated_at).max(0) as u64),
                _ => None,
            },
            manual_reusable: candidate.is_some() && source_current && !location_changed,
            location_changed,
            hourly_only: policy.is_some_and(|facts| facts.frame_count == 0),
        };

        if let Some(raise) = sched.poll(now_s, refresh, snapshot.ride_active, store_ready, facts) {
            IN_FLIGHT.store(true, Ordering::Relaxed);
            PENDING_REQUEST_ID.store(raise.request_id, Ordering::Relaxed);
            let ctx = build_context(&snapshot, refresh_raw, raise, candidate);
            let _ = server.set(&server.weather_request.context, &ctx.encode());
            context_live = true;
            served_refresh = Some(refresh_raw);
            state::arm_weather_request(Duration::from_secs(obc_ble::WEATHER_REQUEST_WINDOW_S));
            info!(
                "ble: [weather] request {=u32} raised (reason {=u16:#06x}, validity {=u16:#06x})",
                raise.request_id, raise.reason, ctx.validity
            );
            if ctx.validity & VALID_POSITION == 0 {
                warn!("ble: [weather] request has no fresh GPS fix — companion cannot build a bundle");
            }
            continue; // re-derive the next wake against the fresh pending state
        }

        // No pending request → the attribute holds the resting "nothing is due" value, so a peer
        // that reads it out of turn learns nothing stale about the rider (§11.4). Re-written when
        // a request just ended (commit / lapse) **or** when the stored refresh byte moved (#1221
        // F2): §11.8's refresh byte reports the rider's own setting, and a resting value frozen at
        // an older one would misreport it — `note_settings_changed`'s wake lands here.
        if sched.pending_request_id().is_none() {
            IN_FLIGHT.store(false, Ordering::Relaxed);
            PENDING_REQUEST_ID.store(0, Ordering::Relaxed);
            // A request can lapse because its retry ladder ended or because its prerequisites
            // disappeared (ride stopped, refresh Off, card removed). The scheduler owns that
            // decision; mirror it to the radio immediately so a stale Weather Request UUID cannot
            // keep advertising a context that has already become resting.
            if context_live {
                state::clear_weather_request();
            }
            if context_live || served_refresh != Some(refresh_raw) {
                let _ =
                    server.set(&server.weather_request.context, &WeatherRequestContext::resting(refresh_raw).encode());
                context_live = false;
                served_refresh = Some(refresh_raw);
            }
        }

        match sched.next_wake_s(refresh, snapshot.ride_active, store_ready) {
            // A wake already in the past without a raise is a boundary case (levels moved under
            // us): yield a beat rather than spinning the executor.
            Some(at) if at <= now_s => Timer::after_millis(250).await,
            Some(at) => {
                let _ = select(WAKE.wait(), Timer::after(Duration::from_secs(at - now_s))).await;
            }
            None => WAKE.wait().await,
        }
    }
}

/// Fill the §11.4 context from the raise + the current app/bundle facts. Optional groups follow
/// the flags-not-sentinels rule: absent inputs leave their fields zero **and** their validity bit
/// clear — a device with no fix still raises a well-formed request for diagnostics/retry, but the
/// companion cannot fetch until the device supplies a position (there is intentionally no phone-
/// location fallback today).
fn build_context(
    s: &WeatherSnapshot,
    refresh_raw: u8,
    raise: Raise,
    bundle: Option<obc_weather::Candidate>,
) -> WeatherRequestContext {
    let mut ctx = WeatherRequestContext {
        refresh_raw,
        request_id: raise.request_id,
        reason: raise.reason,
        ..WeatherRequestContext::EMPTY
    };
    if let Some(p) = s.position {
        ctx.validity |= VALID_POSITION;
        ctx.lat_udeg = p.lat_udeg;
        ctx.lon_udeg = p.lon_udeg;
        ctx.fix_utc = p.fix_utc;
    }
    if let Some(bearing) = s.bearing_deg {
        ctx.validity |= VALID_BEARING;
        ctx.bearing_deg = bearing;
    }
    if let Some(speed) = s.speed_deci_ms {
        ctx.validity |= VALID_SPEED;
        ctx.speed_deci_ms = speed;
    }
    if let Some(route_id) = s.route_id {
        ctx.validity |= VALID_ROUTE;
        ctx.route_id = route_id;
    }
    if let Some(b) = bundle {
        ctx.validity |= VALID_BUNDLE;
        ctx.bundle_generation = b.generation;
        ctx.bundle_generated_at = b.generated_at;
        ctx.bundle_crc32 = b.bundle_crc32;
    }
    ctx
}
