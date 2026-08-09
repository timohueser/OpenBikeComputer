//! The **weather due plane** (WX8, #1193): the board half of the intermittent weather lifecycle —
//! the loop that decides *when* a request is raised, fills the §11.4 request context with the real
//! rider/bundle facts, and arms the WX3 advertising hint.
//!
//! The decisions live in the host-tested [`obc_ble::DueScheduler`] (ride/urgent/retry/commit
//! matrix, pinned against a synthetic clock); this module is only the plumbing around it:
//!
//! - **Inputs** cross the plane boundary the same way every other App fact does: the ride loop
//!   distils an [`obc_app::WeatherSnapshot`] once per pass ([`set_weather_inputs`], the reverse
//!   direction of `app_ble_status`), the Config write path pokes [`note_settings_changed`], and a
//!   committed bundle pokes [`note_commit`] from the store's finish path.
//! - **Outputs** are exactly two: `server.set` on the Weather Request context attribute (so the
//!   next authenticated read serves this request), and [`super::state::arm_weather_request`] (the
//!   bounded advertising swap the lifecycle loop already honours — budget expiry and the
//!   served-read clear are its, per §11.3, not this module's).
//!
//! Nothing here is periodic: the task sleeps until the scheduler's own next instant or an event
//! edge, so an idle, not-riding device costs no wakeups at all.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};

use defmt::info;
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use obc_app::WeatherSnapshot;
use obc_ble::{
    BundleFacts, DueScheduler, Raise, WeatherRefresh, WeatherRequestContext, VALID_BEARING, VALID_BUNDLE,
    VALID_POSITION, VALID_ROUTE, VALID_SPEED,
};

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

/// A weather bundle committed (the store's finish path) — the one thing that finishes a request.
static COMMITTED: AtomicBool = AtomicBool::new(false);

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

/// The rider opened Weather: raise an urgent request now (spec §11.4 reason bit 1). The WX11
/// Weather screen's open is the caller; nothing invokes it yet.
#[allow(dead_code)] // WX11 wires the Weather screen's open to this.
pub fn request_weather_now() {
    URGENT.store(true, Ordering::Relaxed);
    WAKE.signal(());
}

/// A weather bundle committed (WX7 store, via `ObjectStore::weather_finish`): finish the pending
/// request and re-anchor the schedule.
pub(crate) fn note_commit() {
    COMMITTED.store(true, Ordering::Relaxed);
    WAKE.signal(());
}

/// The persisted `weather_refresh` setting moved (a Config write) — re-derive due state now.
pub(crate) fn note_settings_changed() {
    WAKE.signal(());
}

/// The due-scheduler loop. Joined into `ble::run`'s task set for the stack's whole life; every
/// local is small (the context is 52 bytes) so it adds no meaningful poll-frame weight (#677).
pub(crate) async fn run(server: &Server<'_>, store: &RefCell<ObjectStore>, shared: &SharedStoreMutex) -> ! {
    let mut sched = DueScheduler::new();
    // Whether the GATT attribute currently carries a live request (vs. the §11.4 resting value).
    let mut context_live = false;
    loop {
        let now_s = Instant::now().as_secs();
        if COMMITTED.swap(false, Ordering::Relaxed) {
            sched.commit_succeeded(now_s);
            info!("ble: [weather] bundle committed — request satisfied, next interval anchored");
        }
        if URGENT.swap(false, Ordering::Relaxed) {
            sched.open_weather();
        }
        let snapshot = SNAPSHOT.lock(|c| c.get());
        // The persisted setting is always a validated §11.8 discriminant (writers go through the
        // strict write direction; the codec sanitises corruption), so the fallback is unreachable.
        let refresh_raw = store.borrow().settings().weather_refresh;
        let refresh = WeatherRefresh::from_u8(refresh_raw).unwrap_or(WeatherRefresh::DEFAULT);
        // The active bundle's identity — the boot/commit-refreshed selection, no card I/O.
        let bundle = {
            let guard = shared.lock().await;
            store.borrow().weather_active(&guard)
        };
        let facts = BundleFacts {
            held: bundle.is_some(),
            // Age only with a trusted clock; the scheduler treats unknown age conservatively.
            age_s: match (bundle, snapshot.now_utc) {
                (Some(b), Some(now_utc)) => Some((now_utc as i64 - b.generated_at).max(0) as u64),
                _ => None,
            },
        };

        if let Some(raise) = sched.poll(now_s, refresh, snapshot.ride_active, facts) {
            let ctx = build_context(&snapshot, refresh_raw, raise, bundle);
            let _ = server.set(&server.weather_request.context, &ctx.encode());
            context_live = true;
            state::arm_weather_request(Duration::from_secs(super::WEATHER_REQUEST_ADV_BUDGET_SECS));
            info!(
                "ble: [weather] request {=u32} raised (reason {=u16:#06x}, validity {=u16:#06x})",
                raise.request_id, raise.reason, ctx.validity
            );
            continue; // re-derive the next wake against the fresh pending state
        }

        // No pending request → restore the resting "nothing is due" value, so a peer that reads
        // the characteristic out of turn learns nothing stale about the rider (§11.4). Runs on a
        // commit and on an Off-ladder lapse.
        if context_live && sched.pending_request_id().is_none() {
            let mut resting = WeatherRequestContext::EMPTY;
            resting.refresh_raw = refresh_raw;
            let _ = server.set(&server.weather_request.context, &resting.encode());
            context_live = false;
        }

        match sched.next_wake_s(refresh, snapshot.ride_active, facts) {
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
/// clear — a device with no fix still raises a well-formed request (the phone fetches by its own
/// location).
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
