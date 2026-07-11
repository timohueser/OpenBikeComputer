//! The map/ride thread-mode plane (in every build) — split out of `main.rs` (issue #351).
//!
//! [`run_app`], the shared backend-agnostic ride loop, plus its loop-only helpers: the sensor-wake
//! select arm, the GPS power policy, the watchdog cadence, the per-frame render clock, and the
//! route-catalog scan. `main` still owns bring-up + the resident statics and awaits [`run_app`]
//! as its tail future (single call site — see the `#[inline(always)]` note on the fn).

use core::sync::atomic::Ordering;

// The event-driven loop's wake select: `select5` over gesture / hold-wake / sensor / BLE link-edge /
// deadline (the BLE arm is `pending()` on a map build — see `wait_ble_edge`).
use embassy_futures::select::select5;
use embassy_nrf::gpio::Output;
use embassy_nrf::wdt;
use embassy_time::{Instant, Timer};
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
// `SettingsStore` (the load/save trait) is the ride loop's seam over the RRAM store; the `ble`
// build's store lives inside `object_store` (which imports it itself).
use obc_app::SettingsStore;
use obc_app::{App, InputClock, RideClock, Sensors, TrackSink};
// The real-sensor `Signal` sources: the `GpsLocation`/`BaroAltimeter`/`SensorTemp` ZSTs the ride
// loop polls, fed by `sensors::sensor_task`. Real-sensor build only.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use obc_platform::sensor_link;
// The `synth`-build stand-in GPS: walks a slow square loop so a saved ride is a non-degenerate
// ride object (the default streams the real SAM-M10Q; `debug-uart` a recorded host ride).
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
use obc_platform::SynthLocation;
// The map render's framebuffer adapter (the status screen builds its own inside `ble.rs`) + the
// battery stand-in until the nPM1300 PMIC gauge is read.
use obc_platform::{DisplayDriver, FbDevice64, StubFuelGauge, FRAME_H, FRAME_W};
use obc_reader::{MapCache, MapTables, Reader};
// The ride loop's route types: the decoded-route-geometry cache, the resident per-route chunk
// index, and the streamed route reader the matcher + map render share.
use obc_route::{RouteCache, RouteIndex, RouteReader};

use crate::input_plane::{GESTURES, INPUT_HB_MS, INPUT_WAKE, LOOP_MS};
use crate::map_plane::MapDisplay;
use crate::{sd, stackmeter, SharedStore, SharedStoreMutex};

// ── Hardware watchdog (#349): the last-resort net under a wedged plane. The ride loop feeds it,
// gated on the input plane's heartbeat, so **either** plane wedging trips the dog — not just
// thread mode staying alive. Deliberately generous: it must never fire on a slow frame or a deep
// SD reconcile, only on a genuine wedge. ──
/// Watchdog period: 24 s of 32768 Hz LFCLK ticks (the issue's 16–30 s band).
pub(crate) const WDT_TIMEOUT_TICKS: u32 = 24 * 32768;
/// Cap (ms) on the ride loop's event-driven sleep, ~WDT/2 — an otherwise-idle device still wakes
/// to feed the dog. One extra wake per ~12 s is negligible next to [`IDLE_REPOLL_MS`].
const WDT_FEED_CAP_MS: u32 = 12_000;
/// How stale [`INPUT_HB_MS`] may be before the ride loop **withholds** the feed. The idle input
/// plane legitimately sleeps [`IDLE_REPOLL_MS`] (30 s) between stamps, so the window is 2× that
/// plus margin — no false trip on a parked device; a wedged input plane trips the dog within
/// roughly this window + the WDT period (~90 s worst case, fine for a last resort). A stamp
/// slightly *newer* than the loop's own `now` (the planes race on `Instant::now()`) counts as
/// fresh, not as a wrapped ~u32::MAX staleness.
const INPUT_HB_STALE_MS: u32 = 65_000;

/// Synthetic-walk advance cadence (ms) on the `synth` build: the stand-in GPS publishes no `Signal`,
/// so the event-driven loop has no sensor event to wake on and falls back to this timer to step the
/// square-loop walk. The walk position is time-based, so a slower tick just lowers the demo frame rate.
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
const SYNTH_TICK_MS: u64 = 250;

/// The single sensor/host wake the event-driven map loop selects on — one `await` that covers the
/// whole sensor set so the loop sleeps until a datapoint actually arrives. Three builds:
/// - default (real sensors): the unified [`sensor_link::wait_event`] datapoint edge (fix / baro /
///   temp / GPS time / heading) — exactly one wake per published sample, zero I²C at the frame rate;
/// - `debug-uart`: the host-streamed datapoint edge from the VCOM debug link;
/// - `synth`: no event source, so a coarse timer steps the synthetic walk.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
async fn wait_sensor_event() {
    sensor_link::wait_event().await
}
#[cfg(feature = "debug-uart")]
async fn wait_sensor_event() {
    obc_platform::debug_link::wait_event().await
}
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
async fn wait_sensor_event() {
    Timer::after_millis(SYNTH_TICK_MS).await
}

/// The BLE link-edge wake the event-driven map loop selects on (epic #447, P2): a link change — a
/// connect/disconnect, or the pairing `PassKeyDisplay` — must pull the loop out of warm sleep so it
/// feeds the fresh status into `set_ble_status` and renders the passkey card on glass. Reuses the
/// BLE side's existing `publish` edge (`STATUS_EDGE`) via [`ble::wait_status_change`]; it invents no
/// new wake path. On a **map build** (no radio) there's no link, so this never fires — a bare
/// `pending()` — keeping the loop's select shape identical across builds.
#[cfg(feature = "ble")]
async fn wait_ble_edge() {
    crate::ble::wait_status_change().await
}
#[cfg(not(feature = "ble"))]
async fn wait_ble_edge() {
    core::future::pending::<()>().await
}

/// The loop's third select arm: a sensor/host datapoint, **or** (`ble` builds) a store movement —
/// a BLE route commit/delete wakes the loop so the live-catalog rescan (#450) lands now, not at
/// the next timer/sensor wake (a parked device otherwise dozes up to the ~12 s watchdog-feed cap).
#[cfg(feature = "ble")]
async fn wait_host_or_sensor_event() {
    embassy_futures::select::select(wait_sensor_event(), crate::object_store::wait_store_changed()).await;
}
#[cfg(not(feature = "ble"))]
async fn wait_host_or_sensor_event() {
    wait_sensor_event().await
}

/// A `no_std` [`Clock`](obc_render::Clock) over embassy's monotonic `Instant`, in microseconds — the
/// time base for the map render's per-stage timing (collect / sort / draw) the VCOM telemetry
/// carries. The same monotonic clock the loop's frame `Instant` reads, so the stages reconcile.
struct InstantClock;
impl obc_render::Clock for InstantClock {
    fn now_us(&self) -> u64 {
        Instant::now().as_micros()
    }
}

/// Scan the card's `/routes` catalog into the app's Route menu, carrying each entry's durable
/// object id so the app can remap held indices by identity across rescans (#450). Called at boot
/// (from `main`) and again on every store-changed edge (the live rescan below — same machinery).
/// Deliberately its **own `#[inline(never)]` frame**: the ~5 KB [`Catalog`](obc_app::Catalog)
/// (`Vec<RouteSummary, MAX_ROUTES>`, 64 × ~84 B) lives here and is popped on return, so it never
/// sits resident *beneath* the long-lived [`run_app`] ride loop — where 5 KB would steal from the
/// deep route-load render path's stack and overflow the 256 KB part. The mid-session call runs
/// sequentially with (never under) that deep render path, so it adds nothing to the pass's peak.
#[inline(never)]
pub(crate) fn load_routes(storage: &mut sd::Storage, app: &mut App) {
    let catalog = storage.scan_routes();
    app.set_routes_with_ids(&catalog, storage.route_ids());
}

/// Scan the card's `/tracks` into the app's Rides menu (epic #447 P7 / #454), carrying each ride's
/// durable object id + its synced flag (from the `/tracks` synced-set sidecar). Called at boot and on
/// every store-changed edge (a finished ride, an on-device or phone-side ride delete). Its own
/// `#[inline(never)]` frame for the same stack reason as [`load_routes`]: the ride catalog is popped
/// on return, never resident under the deep render path.
#[inline(never)]
pub(crate) fn load_rides(storage: &mut sd::Storage, app: &mut App) {
    let catalog = storage.scan_rides();
    app.set_rides(&catalog, storage.ride_ids());
}

/// Fill an open Ride detail's pending **track-profile request** (epic #678 T2 / #680): drain the
/// ride's durable id, stream its `RD{id}.ORD` once (chunked SD reads — no whole-track buffer),
/// and answer `App::set_ride_profile` — a stream failure (or no card) answers `None`, so a dead
/// file isn't ground against every pass and the band just keeps its loading note. A no-op on the
/// dominant pass (no detail open / already answered).
///
/// Its **own `#[inline(never)]` frame**, the `load_routes`/`load_rides` stack discipline: the
/// builder's column scratch + the returned profile (a few KB on the nrf-mem build) live here and
/// are popped on return — never resident in [`run_app`]'s poll frame under the deep render path
/// (the fill runs sequentially with, never beneath, the render).
#[inline(never)]
fn fill_ride_profile(storage: &mut Option<sd::Storage>, app: &mut App) {
    let Some(id) = app.take_ride_track_request() else { return };
    let profile = storage.as_mut().and_then(|s| s.ride_profile_by_id(id));
    if profile.is_none() {
        defmt::warn!("ride profile: fill for id {=u16} failed — the detail's band stays empty", id);
    }
    app.set_ride_profile(profile);
}

/// The on-device router's caller-owned buffers (epic #116, R4): the fixed A* table + graph-tile
/// cache, `.bss` statics built in `main` and bundled so [`run_app`]'s signature and call sites
/// stay identical across the `has_nav` gate.
#[cfg(has_nav)]
pub(crate) struct NavBuffers {
    pub(crate) scratch: &'static mut obc_route::NavScratch,
    pub(crate) tiles: &'static mut obc_reader::NavTileCache,
    /// The resumable planner's `.bss` slot (#499): ~9.5 KB (it owns the OBCR emitter across
    /// steps), (re)written per request and only read while [`NavRun`] says a plan is active —
    /// which is what makes the `assume_init` accesses in the step path sound.
    pub(crate) planner: &'static mut core::mem::MaybeUninit<obc_route::NavPlanner>,
}
/// The `ble` build's stand-in: the router isn't in the combined image — its ~14.3 KB of statics
/// would push the 256 KB DK's stack region below the measured deep-render peak (see build.rs's
/// `has_nav` note). The ride loop still drains create-route requests and answers the generic
/// failure tier, so the POI confirm never hangs. The 512 KB LM20 deletes this arm.
#[cfg(not(has_nav))]
pub(crate) struct NavBuffers;

/// One in-flight plan's **board-side bookkeeping** (#499): the open reserved-file handle and the
/// per-phase wall-time accumulators the RTT line reports. Loop-local (small); the ~9.5 KB planner
/// itself sits in the [`NavBuffers`] `.bss` slot this struct guards.
#[cfg(has_nav)]
struct NavRun {
    file: embedded_sdmmc::RawFile,
    /// Wall time when the request was drained — the RTT line's user-perceived `total_ms`.
    t0: Instant,
    /// Per-phase step time (µs), attributed by the planner's phase **before** each step:
    /// `[snap, search, emit]`.
    phase_us: [u64; 3],
}

/// Construct + write a fresh request's planner into its `.bss` slot, in this immediately-popped
/// frame — the #419/#501 stack discipline: `NavPlanner::new` materializes a ~9 kB temporary, and
/// inlined into the ride loop that slot landed in the **main task's poll frame**, allocated at
/// entry of every poll (measured 25.3 kB poll body; stacked under the 26.5 kB pre-flattening
/// `nav_step` frame it overflowed the 50.6 kB stack region at ~60.5 kB on glass — the #501
/// HardFault's true cause). The one plan-start defmt line lives here with it.
#[cfg(has_nav)]
#[inline(never)]
fn nav_begin(nav: &mut NavBuffers, req: &obc_app::NavRequest, profile_idx: u8) {
    // The rider's bike-type setting (N5 §8.6); an out-of-range index falls back to profile 0 in the router.
    nav.planner.write(obc_route::NavPlanner::new(req.from, req.to, req.name(), profile_idx));
    // One diagnostic line per plan start (#501 fault dossiers): the three nav statics' addresses
    // pin the memory map without needing the ELF at hand.
    defmt::debug!(
        "nav plan: start planner=0x{=usize:08x} scratch=0x{=usize:08x} tiles=0x{=usize:08x}",
        nav.planner.as_ptr() as usize,
        core::ptr::from_ref(&*nav.scratch) as usize,
        core::ptr::from_ref(&*nav.tiles) as usize
    );
}

/// Run **one bounded planner step** at the ride loop's shallow per-pass depth (#270/#419: the
/// step's frame carries only the cheap boot-parsed-tables `Reader` view, the SD sink over the
/// held-open file, and the planner's own shallow call tree — the emitter lives in the planner's
/// `.bss` slot, not this frame; the deepest transient is the one emitter-sized move when the
/// emit phase constructs it). Everything long-running (render, input, the watchdog feed) runs
/// normally **between** steps — that is the whole point of #499.
#[cfg(has_nav)]
#[inline(never)]
fn nav_step(
    storage: &sd::Storage,
    map_tables: &MapTables,
    map_cache: &MapCache,
    nav: &mut NavBuffers,
    file: embedded_sdmmc::RawFile,
) -> obc_route::Step {
    let Some(map_src) = storage.map_source() else {
        return obc_route::Step::Failed(obc_route::NavError::NoPath);
    };
    let reader = Reader::new(&map_src, map_tables, map_cache);
    let mut sink = storage.nav_sink(file);
    // SAFETY: only called while a `NavRun` is active, and the drain arm wrote the planner slot
    // before creating that run (see `NavBuffers::planner`).
    let planner = unsafe { nav.planner.assume_init_mut() };
    planner.step(&reader, &mut *nav.scratch, &mut *nav.tiles, &mut sink)
}

/// Finish a **completed** plan: flush/patch or delete the reserved file, rescan + re-feed the
/// id-carrying catalog on success (sequential with — never nested under — the step frames, the
/// #496 de-nesting kept), emit the one `nav route:` RTT line with the per-phase breakdown
/// (issue #499's DoD), and answer the app — `notify_nav_result` activates the route and swaps
/// the planning screen for the computed-route overview (or the failure card).
///
/// The RTT line (grep `nav route:`): outcome; route length; `total_ms` = wall time from the
/// request drain (it spans every pass the plan was spread over); `snap/search/emit_ms` = step
/// time attributed to the planner's phase before each step (emit includes the finishing header
/// patch); `write_ms` = the file flush/close; `rescan_ms` = the catalog rescan; `sd_reads` =
/// the tile-cache misses (each is one chunk read); `settles`; and the stackmeter high-water,
/// force-rescanned here — sentinel evidence is permanent, so it still reads the in-step peak.
#[cfg(has_nav)]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn nav_finish(
    storage: &mut sd::Storage,
    app: &mut App,
    nav: &mut NavBuffers,
    run: NavRun,
    outcome: obc_route::Step,
    now: u32,
) {
    use obc_route::NavError;
    let planned = match outcome {
        obc_route::Step::Done(stats) => Ok(stats.total_distance_m),
        obc_route::Step::Failed(e) => Err(e),
        obc_route::Step::Running => Err(NavError::NoPath), // unreachable: callers pass terminals
    };
    let tw = Instant::now();
    storage.nav_route_finish(run.file, planned.is_ok());
    let write_us = tw.elapsed().as_micros();
    let tr = Instant::now();
    let result: Result<(u16, u32), NavError> = planned.and_then(|len| {
        load_routes(storage, app);
        let id = storage.nav_route_id().ok_or(NavError::NoPath)?;
        Ok((id, len))
    });
    let rescan_us = tr.elapsed().as_micros();
    let cache = nav.tiles.stats();
    // SAFETY: the run guarded the slot; the planner is initialized for this plan.
    let settles = unsafe { nav.planner.assume_init_ref() }.settles();
    // The ε rung the plan ended on (N8): 13/10 for a plain success or a fast no-path, 2/1 or 3/1 if
    // the ε-escalation ladder retried on exhaustion. `settles` above is cumulative across the rungs.
    let (eps_num, eps_den) = unsafe { nav.planner.assume_init_ref() }.epsilon_used();
    let hw = stackmeter::rescan(now);
    // `exhausted` is the range tier ("Too far to route here" on glass — no distance cap);
    // `no-path` the generic tier.
    let outcome_str = match &result {
        Ok(_) => "ok",
        Err(NavError::NoPath) => "no-path",
        Err(NavError::Exhausted) => "exhausted",
    };
    let len = result.map(|(_, len)| len).unwrap_or(0);
    defmt::info!(
        "nav route: {=str} len={=u32} total_ms={=u64} snap_ms={=u64} search_ms={=u64} emit_ms={=u64} write_ms={=u64} rescan_ms={=u64} sd_reads={=u32} settles={=u32} eps={=u32}/{=u32} stack_hw={=usize}/{=usize}",
        outcome_str,
        len,
        run.t0.elapsed().as_millis(),
        run.phase_us[0] / 1000,
        run.phase_us[1] / 1000,
        run.phase_us[2] / 1000,
        write_us / 1000,
        rescan_us / 1000,
        cache.misses,
        settles,
        eps_num,
        eps_den,
        hw,
        stackmeter::total()
    );
    app.notify_nav_result(result.map(|(id, _)| id));
}

/// The GPS power state the ride wants: deep-sleep when not tracking, full-power fixes while riding, or
/// the M10's low-power tracking when the `power_saver` toggle is on. Recomputed each frame in
/// [`run_app`] and pushed to the sensor task (via [`sensor_link::set_power`]) only on a change.
/// Real-sensor build only — the `synth` / `debug-uart` feeds have no power-managed receiver.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
fn desired_gps_power(app: &App) -> sensor_link::GpsPower {
    if app.activity.is_tracking() {
        if app.settings().power_saver {
            sensor_link::GpsPower::LowPower
        } else {
            sensor_link::GpsPower::Active
        }
    } else {
        sensor_link::GpsPower::Sleep
    }
}

/// The shared map plane + ride loop, driving present through [`MapDisplay`] so it carries **no backend
/// `#[cfg]`**. Each tick: drain the gestures the input plane recognised, advance the visible screens'
/// timed content, reconcile the card to the app's intent (open the selected route's geometry; begin /
/// finalise the ride log), feed the sensors → `tick` (integrate the fix, map-match, log the
/// track point), then re-render the map only on `dirty.map` and present it. A static screen does zero
/// map renders. LED0 keeps a ~1 Hz heartbeat. Never returns.
///
/// A finished ride persists as the durable ride object `RD{id}.ORD` only — the device writes no
/// GPX (the phone owns human-format export after sync). The conversion is **deferred**: Finish
/// stashes it and the loop runs it once the confirm pop has left the glass (see
/// `Storage::run_pending_save`), so the save's blocking SD stretch never freezes the hold
/// animation.
///
/// The remaining `#[cfg]`s here are the orthogonal `debug-uart` *feature* (a host sensor feed +
/// telemetry vs. the `SynthLocation` stand-in), not the display backend — that is wholly behind
/// `MapDisplay`.
#[allow(clippy::too_many_arguments)]
// `#[inline(always)]`: this is a single-call-site `-> !` future. Inlining folds it (and the present
// methods above) back into `main`'s frame — recovering the ~5 KB of stack the bare extraction cost
// (the deep route-load render then overran the 256 KB part's stack).
#[inline(always)]
pub(crate) async fn run_app(
    mut display: MapDisplay,
    app: &mut App,
    // The SD card + RRAM settings behind one async mutex (#193, #270). The loop locks it once per
    // pass and holds the guard across the render, then releases it before the event-wait so a future
    // BLE object plane can reach the card between passes. Replaces the by-value `Storage`/settings
    // store this fn used to own.
    shared: &SharedStoreMutex,
    map_tables: &MapTables,
    map_cache: &MapCache,
    route_cache: &RouteCache,
    // The router's fixed A* table + graph-tile cache (epic #116, R4): `.bss` statics threaded
    // from `main` (never locals — the #270/#419 discipline), reset per plan inside `plan_route`.
    // On `not(has_nav)` (the `ble` build) it's the unit stand-in — the router isn't in that image.
    #[cfg(has_nav)] mut nav: NavBuffers,
    #[cfg(not(has_nav))] nav: NavBuffers,
    led: &mut Output<'static>,
    // The hardware watchdog's feed handle (#349), `None` only if the boot-time `try_new` found the
    // dog already running with a foreign config. Fed once per pass below, gated on the input
    // plane's heartbeat.
    mut wdt: Option<wdt::WatchdogHandle>,
    // The OBCM bbox centre (lon, lat) — only the `SynthLocation` stand-in needs it (the host feed and
    // the real GPS both stream absolute positions). So it's threaded only on the `synth` build.
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))] cam_center: (i32, i32),
) -> ! {
    // Native renderer colour → identity `Rgb565`; `FbDevice64` quantizes to RGB222 on store.
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

    // Sensor sources — three builds, one `Sensors` either way (the app can't tell which):
    // - `debug-uart`: the host-streamed GPS / altimeter / compass, parsed by the VCOM tasks into
    //   obc-platform's debug-link signals; these ZST handles just `try_take` on the ~1 Hz contract.
    // - default (real sensors, #218): the SAM-M10Q + BMP581 task publishes through `sensor_link`;
    //   these ZSTs drain its `Signal`s. Absolute positions, so no camera re-centre below.
    // - `synth`: the `SynthLocation` square loop (walked from a boot-relative `start`), no baro.
    #[cfg(feature = "debug-uart")]
    let (mut debug_loc, mut debug_alt, mut debug_compass) = (
        obc_platform::debug_link::DebugLocation,
        obc_platform::debug_link::DebugAltimeter,
        obc_platform::debug_link::DebugCompass,
    );
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    let (mut gps, mut baro, mut temp, mut gps_clock, mut mag_compass) = (
        sensor_link::GpsLocation,
        sensor_link::BaroAltimeter,
        sensor_link::SensorTemp,
        sensor_link::GpsClock,
        sensor_link::MagCompass,
    );
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
    let mut synth = SynthLocation::new(cam_center.0, cam_center.1, Instant::now());
    // Battery: a fixed 75 % stand-in until the nPM1300 PMIC fuel gauge is wired in. Polled in `Sensors`
    // like any other sensor.
    let mut fuel = StubFuelGauge::new(75);

    // Per-frame ride-loop state:
    // - `prev_route` re-centres SynthLocation onto a freshly-loaded route's start (`synth` build only);
    // - `prev_active`/`prev_session` gate the SD reconcile on actual change;
    // - `route_index`/`index_route` cache the active route's chunk index, rebuilt only on a route change;
    // - `pending_map_redraw` re-arms a redraw a transient SD glitch couldn't service;
    // - `last_telem*` throttle the host telemetry (debug-uart only).
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
    let mut prev_route: Option<usize> = None;
    let mut prev_active: Option<usize> = None;
    let mut prev_session: Option<u32> = None;
    // The in-flight route plan's bookkeeping (#499): `Some` while a plan is being stepped, one
    // bounded step per pass. Guards the `.bss` planner slot's initialization.
    #[cfg(has_nav)]
    let mut nav_run: Option<NavRun> = None;
    let mut route_index: Option<RouteIndex> = None;
    let mut index_route: Option<usize> = None;
    let mut pending_map_redraw = false;
    #[cfg(feature = "debug-uart")]
    let mut last_telem_ms: u32 = 0;
    #[cfg(feature = "debug-uart")]
    let mut last_telem = obc_platform::debug_link::Telemetry::default();
    // Stack-guard bookkeeping: log only when a new deepest reach is seen, so a future change that pushes
    // the deep render path closer to the 256 KB-DK's ~36 KB stack ceiling shows up immediately.
    let mut stack_hw = 0usize;
    let mut last_led = 0u32;
    // Previous frame's hold-progress, so a hold that retracts on a non-map screen (released early, or
    // just completed) gets one trailing redraw to clear its on-screen bar — the falling edge the
    // charging redraw below would otherwise miss now that a cancelled long-press emits no gesture.
    let mut prev_hold_p = 0.0f32;
    // The DFU trial confirm (epic #615 S4, #619) is anchored at "first frame presented AND SD
    // mounted" — precisely the first successful `render_present` below (main mounts the card and
    // faults out *before* this loop can run, so storage being live is already implied here; a
    // boot that can't reach a presented frame never confirms, and S3's rollback fires next boot).
    let mut trial_confirm_pending = true;

    // Settings: seed the app from the persistent RRAM store at boot (a blank/corrupt page decodes to
    // `None` → defaults), then persist on any change the settings screens make. One brief lock,
    // released at once — the loop re-locks the shared store each pass.
    app.set_settings({
        let mut store = shared.lock().await;
        store.settings.load().unwrap_or_default()
    });

    // The DFU boot-outcome reconcile: boot-state page + the armer's breadcrumb → the one-time
    // post-update verdict card ("UPDATE FAILED" / the accepted-trial toast). A `Trial` boot is
    // left alone — the health-anchor confirm below owns that verdict. Same brief-lock idiom.
    {
        let mut store = shared.lock().await;
        crate::dfu::reconcile_boot_outcome(app, &mut store.settings);
    }

    // Align the GPS to the persisted fix interval: push it to the sensor task once at boot (the task
    // boots at a 1 s default), then again whenever the Power screen edits it. `prev_interval` gates the
    // re-VALSET so an unrelated settings change (units, clock) doesn't reconfigure the M10.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    let mut prev_interval = app.settings().fix_interval_s;
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    sensor_link::set_rate(prev_interval);

    // Drive the GPS power state: the sensor task acquires one boot fix regardless, then honours this —
    // Sleep while idle, Active/LowPower once a ride starts. Pushed once at boot, then again whenever
    // tracking or the `power_saver` toggle changes.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    let mut prev_power = desired_gps_power(app);
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    sensor_link::set_power(prev_power);

    loop {
        let now = Instant::now().as_millis() as u32;
        let hw = stackmeter::used(now);
        if hw > stack_hw {
            stack_hw = hw;
            // Surface the peak in the diagnostics blob for the A9 soak rig (#277) — the ride loop owns the
            // stackmeter, so on a `ble` build it publishes the mark into the BLE state the blob reads.
            #[cfg(feature = "ble")]
            crate::ble::publish_stack_high_water(hw);
            defmt::info!("stack high-water {=usize} / {=usize} B (new peak)", hw, stackmeter::total());
        }

        // ── #349 fault tolerance, once per pass ──
        // The FLPR degraded for good (MAX_CONSEC_RELAUNCHES relaunches failed) → drop to the
        // heartbeat idle. This loop **keeps feeding the watchdog**: degraded is a deliberate
        // terminal state, not a wedge — an unfed dog here would just boot-loop the device against
        // a dead FLPR. COM + the input plane keep running (the glass holds its last image,
        // DC-bias-safe); only a power-cycle retries the panel.
        if display.degraded() {
            defmt::error!("display degraded — heartbeat idle (ride loop stopped; power-cycle to retry)");
            loop {
                led.toggle();
                if let Some(h) = wdt.as_mut() {
                    h.pet();
                }
                Timer::after_millis(500).await;
            }
        }
        // Feed the watchdog, gated on the input plane's heartbeat: this pass proves thread mode
        // alive, the stamp proves the P3 recognizer alive — either plane wedging stops the feed
        // and the dog resets the device within its period.
        if let Some(h) = wdt.as_mut() {
            // The input plane stamps from its own `Instant::now()`, which can be a hair newer
            // than this loop's `now` — the subtraction then wraps to ~u32::MAX. A wrapped
            // (top-half) age means the heartbeat is *ahead* of us, i.e. maximally fresh.
            let age = now.wrapping_sub(INPUT_HB_MS.load(Ordering::Relaxed));
            if age <= INPUT_HB_STALE_MS || age > u32::MAX / 2 {
                h.pet();
            } else {
                defmt::error!("WDT: input-plane heartbeat {=u32} ms stale — withholding the feed", age);
            }
        }

        // Feed the input plane's live hold-progress before anything below consults it: every
        // hold-deferral rule this pass runs (`hold_charging` — the upload popups' delivery and
        // auto-close, the passkey card's open/close) must read *this* pass's charge state. A loop
        // woken from warm sleep by INPUT_WAKE otherwise saw the previous pass's seconds-stale 0.0
        // and could land or close a host-pushed screen mid-charge.
        app.set_hold_progress(display.hold_progress());

        // Apply the high-priority plane's recognised gestures, in order, then advance animations.
        // The screen transition lands a frame after the overlay already confirmed the press.
        // A gesture that changed the screen stack invalidates any hold charging at that moment —
        // it was aimed at the *old* top (e.g. a popup's "Save & new"), and completing it onto the
        // new one can be destructive (the Route menu's hold-to-delete footer, issue #480): drop
        // any `Hold`/`BackHold` queued behind the transition and cancel the input plane's
        // in-flight recognition.
        let mut holds_cancelled = false;
        while let Ok(g) = GESTURES.try_receive() {
            if holds_cancelled && matches!(g, obc_app::Gesture::Hold | obc_app::Gesture::BackHold) {
                continue;
            }
            app.apply_gesture(g);
            holds_cancelled |= app.take_hold_cancel();
        }
        if holds_cancelled {
            display.cancel_holds();
        }
        app.advance_animations(InputClock(now));

        // ── Sensor presence → warning (issue #504), real-sensor build, once ──
        // The sensor task publishes its boot I²C probe result a moment after boot; map any chip that
        // didn't answer to a dismissable warning card. `try_take` yields once, so this fires a single
        // pass; `notify_warning(NONE)` (all present) is a no-op.
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        if let Some(p) = sensor_link::take_presence() {
            let mut w = obc_app::WarningFlags::NONE;
            if !p.gps {
                w |= obc_app::WarningFlags::NO_GPS;
            }
            if !p.altimeter {
                w |= obc_app::WarningFlags::NO_ALTIMETER;
            }
            if !p.compass {
                w |= obc_app::WarningFlags::NO_COMPASS;
            }
            app.notify_warning(w);
        }

        // ── BLE → app seam (epic #447), once per pass, cheap ──
        // Feed the link snapshot (connected + passkey) — a couple of atomic reads distilled to the
        // app's own `BleStatus`; `set_ble_status` compares against the last and dirties nothing on the
        // steady state. Then drain the object-store movement edge and ring `notify_store_changed` per
        // commit/delete (the same edge that notifies the phone). Both `ble`-only: the map build has no
        // radio and no `object_store`, so the app simply stays disconnected there.
        #[cfg(feature = "ble")]
        {
            app.set_ble_status(crate::ble::app_ble_status());
            // Mirror the ride-recording state to the BLE plane's `installFw` busy-gate (S6, #621), and
            // drain a BLE-initiated install request into the on-glass flow: `open_remote_dfu_check`
            // pushes the "Checking card..." wait and posts `DfuAction::Scan` — the System menu's press
            // arriving over the air, NEVER `DfuAction::Install` (spec §4.4: the phone can request, only
            // the rider installs; direct Install stays the physical debug link's + the confirm screen's).
            // The atomic is consumed only when the flow actually opened — a `false` is a *deferral*
            // (passkey card up, a DFU screen already on the stack, a hold charging, recording), so the
            // request stays pending, retries next pass, and keeps the BLE edge's `dfu_install_pending()`
            // busy-gate accurate while it waits. The Scan posted here is drained by the DFU match below
            // in this same pass, so the wait card swaps to the confirm/error promptly.
            crate::ble::set_recording(app.activity.is_tracking());
            if crate::object_store::dfu_install_pending() && app.open_remote_dfu_check() {
                let _ = crate::object_store::take_dfu_install_ble();
            }
            for _ in 0..crate::object_store::take_store_changed() {
                app.notify_store_changed();
            }
            // The settings→radio switch (#455): push the persisted Bluetooth toggle across the
            // plane boundary — one atomic swap; the radio plane wakes only on a change (off = stop
            // advertising + drop the link; on = the normal lifecycle). Fire-and-forget by design:
            // this loop never blocks on the radio winding down, so no wake source here can go dead
            // with the radio off (#438's lesson). Then drain the Bluetooth screen's Forget-phone
            // hold and ring the bond clear the same way.
            crate::ble::set_radio_enabled(app.settings().ble_enabled);
            if app.take_ble_forget() {
                crate::ble::request_forget_bond();
            }
        }

        // This frame's hold-bulge state, sampled once: the live row span (the present goes around it)
        // and the dirty edge the map plane owns the bulge re-push off of.
        //
        // The bulge pushes **first in the pass**, before the store lock, the SD reconcile, and any
        // screen redraw (#348 follow-up, widened here): a fired hold usually navigates — and a
        // fired *Finish* triggers the ride save — so with the bulge later in the pass its confirm
        // pop queued behind the new screen's render (~40–300 ms) or, worse, the whole SD save,
        // and the 220 ms pop expired unseen ("sometimes it just snaps"). Bulge-first, the pop's
        // attack lands on glass within ~10 ms of the fire — composited over the *old* fb for that
        // one frame, which is correct: that is what is on glass until the present below.
        let (overlay_dirty, overlay_span) = display.poll_overlay();
        display.present_bulge(overlay_span, overlay_dirty).await;

        // Lock the shared store for the rest of this pass: the settings save just below, the card
        // reconcile, the per-frame route/track/map sources, and the render that reads them all run
        // under one guard — held across the render `.await`, then dropped before the event-wait at the
        // loop tail so a future BLE object plane reaches the card between passes (#270). Destructured
        // into the two names the body already uses (`storage`, `settings_store`).
        let mut store_guard = shared.lock().await;
        let SharedStore { storage, settings: settings_store } = &mut *store_guard;

        // ── Live route catalog (#450), on the store-changed edge only ──
        // A BLE commit/delete moved `/routes` under the app: re-run the boot scan (same
        // `load_routes` machinery — its ~5 KB catalog lives in its own popped frame, sequential
        // with, never under, the deep render path) and re-feed `set_routes_with_ids`, which
        // remaps every held catalog index by durable object id — so the route being navigated
        // (or highlighted, or pending in a swap prompt) can never silently shift. Then force the
        // reconcile below to re-derive everything positional: the filename table was rebuilt (the
        // open handle's index may now name a different file) and a replace-upload may have
        // swapped the bytes under the open geometry handle — close it and let the reconcile
        // reopen + re-index off the fresh scan.
        #[cfg(feature = "ble")]
        if app.take_store_changed() > 0 {
            if let Some(s) = storage.as_mut() {
                // Close the active route's geometry handle BEFORE the scan: embedded-sdmmc
                // refuses a second open of an open file (`FileAlreadyOpen`, even ReadOnly), so a
                // scan that met the still-open geometry silently omitted it — and the identity
                // remap then unloaded the navigated route and it vanished from the Route menu
                // until the next rescan (the #480 vanishing-routes bug). The forced reconcile
                // below reopens it off the fresh tables.
                s.reconcile_route(None);
                load_routes(s, app);
                // The same edge covers rides (a phone-side ride delete, or a ride download that just
                // flipped a synced flag): re-scan `/tracks` and re-feed the Rides menu, which remaps
                // its highlight by id (#454). Cheap when nothing ride-related moved.
                load_rides(s, app);
            }
            prev_active = None; // force reconcile_route/track to re-run against the new indexing
            index_route = None; // and the chunk index to rebuild off the freshly-opened file
        }

        // ── On-device route delete (epic #447, P6), on the hold-to-delete edge only ──
        // The Route menu's guarded hold recorded a delete request; the app resolves it to the route's
        // durable object id. Route it to storage **through `ObjectStore`** (never raw SD) so the
        // catalog, revision, digest, and phone `storeChanged` notify all move together, exactly as a
        // phone-initiated delete does — then the store-changed edge (next pass) brings the live
        // rescan + P3 remap around, so `active_route` and the menu highlight follow by identity.
        //
        // `ble` builds: the `ObjectStore` lives behind the BLE task's `RefCell`, unreachable from
        // here (they're separate thread-mode tasks; there is no shared handle). Post the id to the
        // BLE plane (`request_route_delete`), which owns that `RefCell` and drains it whether the
        // phone is connected or the device is parked advertising — that keeps the whole delete on the
        // one `ObjectStore`, revision-and-notify coherent, and RefCell-legal (the ride loop never
        // borrows the store). The rescan comes back on the resulting `STORE_CHANGED` edge above.
        if app.has_route_delete() {
            if let Some(_id) = app.take_route_delete() {
                #[cfg(feature = "ble")]
                crate::object_store::request_route_delete(_id);
                // Map-only build: no `ObjectStore`/radio, so delete the file directly and re-scan the
                // catalog locally (the same `load_routes` machinery the store-changed edge runs).
                // Geometry handle closed FIRST, for the same two reasons as the store-changed
                // rescan above: an open file can't be deleted, and a scan that meets it silently
                // drops it from the catalog (#480).
                #[cfg(not(feature = "ble"))]
                if let Some(s) = storage.as_mut() {
                    s.reconcile_route(None);
                    if s.delete_route_by_id(_id) {
                        load_routes(s, app);
                    }
                    prev_active = None; // force the reconcile below to re-derive off the new indexing
                    index_route = None;
                }
            }
        }

        // The System settings screen's card-free scan (T8 item 6): a drained on-entry request runs
        // one bounded FAT free-cluster read off the card and answers through `set_card_free` (or a
        // `None` → the screen keeps `--` when there's no card / no FSInfo free count).
        if app.take_card_scan_request() {
            app.set_card_free(storage.as_ref().and_then(|s| s.card_free_bytes()));
        }

        // ── On-device ride delete (epic #447, P7 / #454), on the Rides-menu hold-to-delete edge ──
        // The same seam as the route delete, in the ride namespace: the app resolves the highlighted
        // ride's durable object id. On `ble`, post it to the BLE plane (it owns the `ObjectStore`
        // `RefCell`) so the delete goes through the store — revision bump + `storeChanged`, coherent
        // with a phone-initiated delete; the rescan returns on the resulting store-changed edge above.
        // Map-only: delete the `RD{id}.ORD` directly (retiring its synced-set flag) and re-feed the
        // Rides menu locally. The greying while recording already keeps the delete legal (no open
        // TRACK.OBT / pending save collides).
        if app.has_ride_delete() {
            if let Some(_id) = app.take_ride_delete() {
                #[cfg(feature = "ble")]
                crate::object_store::request_ride_delete(_id);
                #[cfg(not(feature = "ble"))]
                if let Some(s) = storage.as_mut() {
                    if s.delete_ride_by_id(_id) {
                        load_rides(s, app);
                    }
                }
            }
        }

        // ── The Ride detail's track-profile fill (epic #678 T2 / #680), on the detail-entry edge ──
        // An open detail wants its recorded track profiled for the elevation band: stream the
        // `RD{id}.ORD` once into the app's resident buffer, in this pass, under the store lock —
        // sequential with (never under) the render below, and a no-op on every other pass.
        fill_ride_profile(storage, app);

        // ── The resumable route planner (#499), one bounded step per pass ──
        // A drained create-route request opens the reserved file, (re)writes the `.bss` planner
        // slot, and arms a `NavRun`; each subsequent pass runs **one** `nav_step` at this shallow
        // depth and then continues the normal pass (render, input, the pass-top watchdog feed) —
        // the UI stays live while the route computes. A drained cancel (Back on the planning
        // screen) aborts: close + delete the partial file, answer nothing. On a terminal step,
        // `nav_finish` flushes/patches (or deletes) the file, rescans the catalog (sequential,
        // never nested — the #496 de-nesting), emits the per-phase RTT line, and answers the app;
        // the positional state is then forced to re-derive, exactly like the store-changed rescan
        // above (the plan closed the active geometry handle and may have rewritten bytes under
        // the reserved name).
        //
        // `not(has_nav)` (the `ble` build, whose image ships without the router — see build.rs):
        // the request is still drained and answered with the generic failure tier ("Couldn't find
        // a route."), so the POI confirm never hangs. The LM20 deletes that arm.
        let nav_cancel = app.take_nav_cancel();
        #[cfg(has_nav)]
        {
            if let Some(req) = app.take_nav_request() {
                // A request while a plan is somehow still in flight replaces it (can't happen
                // through the UI — the planning screen blocks a second confirm — but stay safe).
                if let (Some(run), Some(s)) = (nav_run.take(), storage.as_mut()) {
                    s.nav_route_finish(run.file, false);
                }
                match storage.as_mut().and_then(|s| s.nav_route_begin()) {
                    Some(file) => {
                        nav_begin(&mut nav, &req, app.settings().bike_profile_idx);
                        nav_run = Some(NavRun { file, t0: Instant::now(), phase_us: [0; 3] });
                    }
                    // No card / no dir: nothing to route against — the generic failure tier.
                    None => app.notify_nav_result(Err(obc_route::NavError::NoPath)),
                }
            }
            if nav_cancel {
                if let Some(run) = nav_run.take() {
                    if let Some(s) = storage.as_mut() {
                        s.nav_route_finish(run.file, false); // close + delete the partial file
                    }
                    defmt::info!("nav route: cancelled after {=u64} ms", run.t0.elapsed().as_millis());
                    // No notify: the planning screen is already gone (Back popped it).
                }
            }
            if let Some(run) = nav_run.as_mut() {
                if let Some(s) = storage.as_mut() {
                    // SAFETY: the run is active ⇒ the slot was written for this plan.
                    let phase = unsafe { nav.planner.assume_init_ref() }.phase();
                    let ts = Instant::now();
                    let step = nav_step(s, map_tables, map_cache, &mut nav, run.file);
                    let us = ts.elapsed().as_micros();
                    match phase {
                        obc_route::NavPhase::Snap => run.phase_us[0] += us,
                        obc_route::NavPhase::Search => run.phase_us[1] += us,
                        obc_route::NavPhase::Emit | obc_route::NavPhase::Done => run.phase_us[2] += us,
                    }
                    if !matches!(step, obc_route::Step::Running) {
                        let run = nav_run.take().expect("just borrowed it");
                        nav_finish(s, app, &mut nav, run, step, now);
                        prev_active = None; // reopen geometry / rebuild the index off the fresh scan
                        index_route = None;
                    }
                }
            }
        }
        #[cfg(not(has_nav))]
        {
            let _ = nav_cancel; // no plan can be in flight — the cancel is inert here
            if let Some(req) = app.take_nav_request() {
                let _: &NavBuffers = &nav; // the unit stand-in — nothing to plan with
                let _ = &req;
                defmt::warn!("nav: router not built into the ble image (256K DK) — answering the failure tier");
                app.notify_nav_result(Err(obc_route::NavError::NoPath));
            }
        }

        // The upload-arrived event (#451), drained strictly **after** the rescan above so the id
        // resolves against the fresh catalog. `notify_route_uploaded` raises the right popup
        // (idle prompt / mid-ride swap / active-replaced info card) and — on a replace of the
        // navigated route — drops the app's stale matcher progress + elevation profile, while the
        // rescan block already closed the open geometry handle and the reconcile below reopens +
        // re-indexes it off the new bytes: the full forced-adoption chain, one pass.
        #[cfg(feature = "ble")]
        if let Some((id, replaced)) = crate::object_store::take_route_uploaded() {
            // Build the route's mini elevation band from the just-committed OBCR (#682) — one scoped
            // stream at commit time — so the idle received card can draw it (the swap / active-
            // replace variants ignore it). A missing store or a no-elevation route yields `None`.
            let elevation = storage.as_mut().and_then(|s| s.route_elevation_sparkline(id));
            app.notify_route_uploaded(id, replaced, elevation);
        }

        // Settings coherence, phone → device (#456): a BLE Config write persisted units + name to
        // RRAM but the live `App` copy never learned. Reload the BLE-owned fields into it *before*
        // the change-detection save below, so (a) the UI re-captions same-session and (b) the app's
        // `==`-diff save can't clobber the phone's write with its own stale copy. Only units + name
        // are BLE-writable, so the merge is narrow (`adopt_ble_fields`) — a device-only edit pending
        // this frame is untouched. Board-crate flag, drained once per BLE write; a no-op otherwise.
        #[cfg(feature = "ble")]
        if crate::object_store::take_ble_config_written() {
            let mut settings = *app.settings();
            settings.adopt_ble_fields(&settings_store.load().unwrap_or_default());
            app.set_settings(settings);
        }

        // Persist settings the moment a settings screen changes one: one in-place 16-byte RRAM line,
        // skipped when nothing changed.
        if app.take_settings_dirty() {
            settings_store.save(app.settings());
            // Settings coherence, device → phone (#456): the RRAM blob just moved, so the BLE
            // config-read cache is stale — flag it so the BLE plane refreshes from RRAM before its
            // next Config read / advertised-name read. One relaxed store; a no-op on non-BLE builds.
            #[cfg(feature = "ble")]
            crate::object_store::mark_device_settings_changed();
            // Push a changed GPS fix interval to the sensor task → it re-VALSETs the M10's rate.
            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
            if app.settings().fix_interval_s != prev_interval {
                prev_interval = app.settings().fix_interval_s;
                sensor_link::set_rate(prev_interval);
            }
        }

        // Reconcile the GPS power state to the ride: Sleep when not tracking, Active (or LowPower with
        // `power_saver`) while riding. Recomputed every frame off the tracking + settings state, pushed
        // to the sensor task only on a change.
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        {
            let power = desired_gps_power(app);
            if power != prev_power {
                prev_power = power;
                sensor_link::set_power(power);
            }
        }

        // A pending debug `Z` camera-scale command (render benchmark): pin the map to an exact
        // meters-per-pixel and force one redraw, so a host zoom sweep gets exactly one fresh,
        // stage-timed frame per setting instead of stepping the encoder's 1.2× detents.
        #[cfg(feature = "debug-uart")]
        if let Some(mpp) = obc_platform::debug_link::take_zoom() {
            app.set_map_mpp(mpp);
        }

        // A pending debug `N` route-plan trigger (#500 perf bench): start a plan between two fixed
        // coords exactly as the POI confirm would (request + planning screen), so a host over VCOM
        // can drive the resumable router repeatably and read the `nav route:` RTT breakdown — no
        // POI-browser navigation needed. `has_nav` only (the router isn't in the `ble` image).
        #[cfg(all(feature = "debug-uart", has_nav))]
        if let Some((from, to)) = obc_platform::debug_link::take_nav() {
            app.debug_start_nav(from, to, "Bench");
        }

        // ── DFU install request (epic #615 S4, #619), the create-route drain shape ──
        // The `dfu-install` debug command posts the *same app-level request* the S5 update screen
        // will post; this drain is the single execution path either way, running with exclusive
        // storage access under the pass's store lock. Guards mirror what S5's menu entry will
        // grey out: never mid-recording (the arm ends in a reboot — a live ride would be lost)
        // and never over an unconverted ride save (`pending_save` is RAM state; rebooting drops
        // it and the next fresh ride would truncate the unconverted TRACK.OBT). On success
        // `run_install` never returns (it resets into the bootloader); on failure it has already
        // streamed the typed error and the loop just keeps riding.
        #[cfg(feature = "debug-uart")]
        if obc_platform::debug_link::take_dfu_install() {
            app.request_dfu_install();
        }
        // Two phases share this one-shot slot (epic #615 S5, #620): the S5 UI posts `Scan` first
        // (read-only validation → answer the app, which shows the confirm screen), then `Install`
        // from the confirm; the `dfu-install` debug command posts `Install` directly (no confirm).
        match app.take_dfu_request() {
            Some(obc_app::DfuAction::Install) => {
                // The irreversible arm-and-reboot. Guards mirror what the System menu greys out:
                // never mid-recording (the arm ends in a reboot — a live ride would be lost) and
                // never over an unconverted ride save (`pending_save` is RAM state; rebooting drops
                // it and the next fresh ride would truncate the unconverted TRACK.OBT). On success
                // `run_install` never returns (it resets into the bootloader); on failure it has
                // already streamed the typed error and the loop just keeps riding.
                if app.activity.is_tracking() {
                    crate::dfu::status("refused: a ride is recording -- finish it first");
                } else if storage.as_ref().is_some_and(sd::Storage::has_pending_save) {
                    crate::dfu::status("refused: a ride save is pending -- try again in a moment");
                } else if let Some(s) = storage.as_mut() {
                    crate::dfu::run_install(s, settings_store, &mut wdt).await;
                } else {
                    crate::dfu::status("refused: no SD card");
                }
            }
            Some(obc_app::DfuAction::Scan) => {
                // The UI's read-only "Checking card..." step: validate `UPDATE.BIN` and answer the
                // app (the wait screen swaps to the confirm or an error card). No card ⇒ report the
                // update file as missing. The scan touches nothing, so no ride-state guard is needed
                // (the menu greys the row mid-ride anyway).
                let result = match storage.as_mut() {
                    Some(s) => crate::dfu::run_scan(s, settings_store, &mut wdt),
                    None => Err(obc_app::DfuScanError::NotFound),
                };
                app.notify_dfu_scan_result(result);
            }
            None => {}
        }

        let active = app.activity.active_route;
        // Re-centre the synthetic GPS onto a freshly-loaded route's start so Follow doesn't yank the
        // camera off it (`synth` build only — the host feed and the real GPS stream absolute positions).
        #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
        if active != prev_route {
            if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                synth.recenter(r.start_lon, r.start_lat);
            }
            prev_route = active;
        }

        // Reconcile the card to the app's intent: open/close the active route's geometry and the ride
        // log (begin on load, close + stash the deferred save on Finish), reading the save name from
        // the active route.
        // Gated on the same edges `reconcile_*` test internally (a route swap, a session change, or a
        // pending track action) so the dominant static frame does no per-tick `String<64>` copy or
        // state re-walk. `has_track_action` is a non-consuming peek; `take_track_action` stays inside,
        // so the one-shot is drained only when processed.
        let session = app.activity.session;
        if active != prev_active || session != prev_session || app.activity.has_track_action() {
            let action = app.activity.take_track_action();
            let mut name: heapless::String<64> = heapless::String::new();
            if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                let _ = name.push_str(&r.name);
            }
            // A Save also writes the durable ride object: snapshot the app's ride totals + wall-clock
            // anchor in the same frame, so the header matches the log's last points.
            let stats = (action == Some(obc_app::TrackAction::Save)).then(|| app.ride_stats());
            // `storage` is `Option` (a card-less `ble` combined build still serves BLE, map idle); the
            // map build always has `Some`. No card ⇒ nothing to reconcile.
            if let Some(s) = storage.as_mut() {
                s.reconcile_route(active);
                s.reconcile_track(action, session, &name, stats.as_ref(), settings_store);
            }
            prev_active = active;
            prev_session = session;
        }

        // Cache the active route's chunk index across frames: rebuild it (the header + full chunk-meta
        // walk off SD) only when the route changes, or retry if a prior build failed on a flaky link.
        // Not gated on rendering — the matcher in `tick` needs the index on every fresh fix.
        if index_route != active {
            route_cache.clear(); // a route switch: drop stale slots (the cache keys by chunk index only)
            match active {
                Some(_) => match storage.as_ref().and_then(|s| s.build_route_index()) {
                    Some(idx) => {
                        route_index = Some(idx);
                        index_route = active; // cached — no more rebuilds until the route changes
                    }
                    None => {
                        // Transient SD glitch: leave the key mismatched so every frame retries, hiding
                        // the route this frame rather than the whole ride.
                        route_index = None;
                        index_route = None;
                        defmt::warn!("SD: route index read failed (flaky link?) — retrying next frame");
                    }
                },
                None => {
                    route_index = None;
                    index_route = None;
                }
            }
        }
        // This frame's route reader = the cached index + a fresh geometry source (both cheap, no I/O —
        // the source just wraps the open handle). Geometry streams lazily where it's read: the matcher
        // on a fresh fix, the renderer on a redraw frame.
        let route_src = storage.as_ref().and_then(|s| s.route_source());
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(src)) => Some(RouteReader::new_cached(idx, src, route_cache)),
            _ => None,
        };
        // The Route overview's shape preview (#685 §4; widened to stored routes by #678 rework 3's
        // track/elevation pager): a computed plan's `nav_finish` above answered the app and forced
        // this pass's index rebuild, and a stored route's overview entry pointed `active_route` at
        // it (same rebuild) — either way the previewed route's reader exists right here. Decimate
        // its polyline (≤ 64 points, one chunk walk through the resident cache) and hand the copy
        // over. `nav_preview_missing` is false once fed (and on every non-overview frame), so this
        // runs once per overview entry / plan, not per pass.
        if app.nav_preview_missing() {
            if let Some(r) = route.as_ref() {
                let pts = r.preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>();
                app.set_nav_preview(&pts);
            }
        }
        // The ride-log sink, built every tick (it only wraps the open log handle, no I/O), so a fresh
        // fix is written to the `.obt` log the moment it arrives, at the fix rate.
        let mut tsink = storage.as_ref().and_then(|s| s.track_sink());
        let track_dyn = tsink.as_mut().map(|t| t as &mut dyn TrackSink);

        // Feed the sensors → integrate the fix → map-match to the route → log the track point. Three
        // builds: the VCOM-streamed GPS + altimeter + compass (`debug-uart`); the real SAM-M10Q +
        // BMP581 GPS + altimeter + temperature, coherent per fix (default); or the SynthLocation square
        // loop, no other sensors (`synth`). `track_dyn` is consumed either way.
        #[cfg(feature = "debug-uart")]
        app.tick(
            RideClock(now),
            Sensors {
                loc: &mut debug_loc,
                altimeter: Some(&mut debug_alt),
                temperature: None,
                clock: None, // the host feed streams no GPS time yet
                compass: Some(&mut debug_compass),
                track: track_dyn,
                fuel: Some(&mut fuel),
            },
            route.as_ref(),
        );
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        app.tick(
            RideClock(now),
            Sensors {
                loc: &mut gps,
                altimeter: Some(&mut baro),
                temperature: Some(&mut temp),
                clock: Some(&mut gps_clock), // SAM-M10Q UTC → the wall clock when "Set from GPS" is on
                compass: Some(&mut mag_compass), // ICM-20948 / AK09916 heading while stopped
                track: track_dyn,
                fuel: Some(&mut fuel),
            },
            route.as_ref(),
        );
        #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
        app.tick(
            RideClock(now),
            Sensors {
                loc: &mut synth,
                altimeter: None,
                temperature: None,
                clock: None, // the synthetic loop has no clock source
                compass: None,
                track: track_dyn,
                fuel: Some(&mut fuel),
            },
            route.as_ref(),
        );

        // Feed the high-priority plane's encoder hold-progress to the map render so the in-screen
        // confirm fills (the factory-Reset bar) track the hold — `App`'s own input plane isn't
        // driven here, so the render would otherwise read 0 and the bar would never fill.
        let hold_p = display.hold_progress();
        app.set_hold_progress(hold_p);

        // Drain the per-frame dirty signal now that input + tick have run, and fold back a redraw a
        // previous frame couldn't service on a transient reader-build failure. Every board-level
        // demand folded in below is full-frame, so each also drops a region-scoped tick's clip
        // (`dirty.region`) — the region only survives when the ticks were the sole dirt.
        let mut dirty = app.take_dirty();
        if pending_map_redraw {
            dirty.map = true;
            dirty.region = None;
        }
        pending_map_redraw = false;
        // A FLPR relaunch landed since the last pass (#349): the fresh core has no frame history
        // and the diff store was reset — schedule the full repaint even if nothing else is dirty.
        if display.take_relaunch_repaint() {
            dirty.map = true;
            dirty.region = None;
        }
        // While a hold *charges* on a cheap (non-map) screen — the factory-Reset prompt, the
        // hold-to-delete bar — redraw it each frame so its bar tracks the live progress, **and** once
        // more on the frame the hold drops back to 0 (the falling edge), so an early release clears the
        // bar instead of leaving it stuck mid-fill. A pure hold-charge (and a *cancelled* one) emits no
        // gesture, so nothing else dirties the map. Gated on `!base_draws_map` so the expensive map view
        // is never re-rendered for a hold (there the overlay bulge is the live feedback), and on
        // `top_wants_hold_fill` so a hold charging where no fill would draw — the menus, an un-armed
        // Reset, the Fields Add row — repaints nothing.
        if (hold_p > 0.0 || prev_hold_p > 0.0) && !app.base_draws_map() && app.top_wants_hold_fill() {
            dirty.map = true;
            dirty.region = None;
        }
        prev_hold_p = hold_p;

        // While a hold is *charging* on the **map view**, defer expensive map redraws instead of
        // rendering them: a 150–300 ms map frame between two bulge pushes is exactly the mid-charge
        // freeze that made the bulge jerky while riding (a 1 Hz fix redraw deferred ≤500 ms is
        // invisible; the latched frame lands on the pass after the hold resolves). Only the map
        // base is deferred — cheap screens redraw per-frame anyway for their in-screen hold fills.
        // Once the hold *fires*, charging drops to 0, so a navigation's redraw is never held up.
        if dirty.map && app.base_draws_map() && display.hold_charging() {
            pending_map_redraw = true;
            dirty.map = false;
        }

        if dirty.map {
            // The map pipeline runs **only when the base screen needs the streamed `Reader`** — the
            // Map view, and the POI list on the frame it takes its one-shot snapshot (#425, its
            // query runs in the draw path off `rx.reader`). On a menu / Statistics / Home redraw, or
            // a POI list already showing its frozen snapshot, it's skipped entirely — no SD
            // style-table parse, no `Reader` build (so no stack spike), no map render — that screen
            // draws just its own chrome. Such a frame costs only its own draw + the push.
            let needs_map = app.base_needs_reader();
            // Build the streamed `Reader` **only** on a map frame, `None` otherwise. A *cheap* borrow of
            // the boot-parsed `MapTables` + a fresh `src` + the session-long `MapCache` — no style-table
            // SD read, no parse, no stack spike (what kept this deep path inside the 256 KB stack). The
            // only per-frame failure left is the source handle being momentarily unavailable (a flaky SD
            // link); skip the redraw, keep the last frame, latch a retry.
            let map_src = if needs_map { storage.as_ref().and_then(|s| s.map_source()) } else { None };
            let reader = map_src.as_ref().map(|s| Reader::new(s, map_tables, map_cache));
            if needs_map && reader.is_none() {
                pending_map_redraw = true;
                defmt::warn!(
                    "map: reader build failed this frame (flaky SD?) — kept frame, retrying redraw next frame"
                );
            } else {
                // Render the whole frame into the resident RGB222 plane, then present it — the
                // display boundary, behind `MapDisplay::render_present` (the FLPR scans it, going
                // *around* a live bulge's rows so the composite below paints them).
                // `render_map_timed` threads `InstantClock` so the stats
                // carry the collect/sort/draw timings; the hold bulge is **not** composited here — it
                // rides `present_bulge` on its own plane.
                //
                // A surviving `dirty.region` (the nav spinner's needle disc — only ever on a non-map
                // chrome frame) clips the render at both layers (#500 follow-up): the app's Canvas
                // rejects whole primitives whose bounds miss the region (the glyph/scanline machinery
                // a pixel clip can't skip), and the framebuffer discards any straddler's out-of-region
                // pixel writes — so a spinner frame costs the disc instead of the whole chrome, and
                // the row-diffed push scales down with it.
                let clip = if needs_map { None } else { dirty.region };
                app.set_render_clip(clip);
                let render = |d: &mut dyn DisplayDriver| {
                    let mut fbdev = FbDevice64::new(d.fb_mut(), FRAME_W as u32, FRAME_H as u32);
                    if let Some(r) = clip {
                        fbdev.set_clip(r);
                    }
                    app.render_map_timed(
                        &mut fbdev,
                        reader.as_ref(),
                        route.as_ref(),
                        FRAME_W as f32,
                        FRAME_H as f32,
                        color_fn,
                        &InstantClock,
                    )
                };
                let fp = display.render_present(overlay_span, render).await;

                // Snapshot this frame's render stats for the host telemetry line — the same numbers as
                // the RTT `map frame` log. The nRF reader isn't `TimedSource`-wrapped, so the SD/cache
                // I/O folds into `collect_us` (`read_us` stays 0); the bulge composites on its own
                // overlay push, so `overlay_us` stays 0.
                #[cfg(feature = "debug-uart")]
                {
                    let mpp_milli =
                        (app.state.viewport(FRAME_W as f32, FRAME_H as f32).meters_per_pixel() * 1000.0) as u32;
                    last_telem = obc_platform::debug_link::Telemetry {
                        frame_us: fp.render_us as u32,
                        lod: fp.stats.lod as u8,
                        feat_drawn: fp.stats.features_drawn as u32,
                        feat_tried: fp.stats.features_tried as u32,
                        feat_dropped: fp.stats.features_dropped as u32,
                        chunks: fp.stats.chunks_visited as u32,
                        cache_hits: fp.stats.map_chunk_hits,
                        cache_misses: fp.stats.map_chunk_misses,
                        sd_reads: fp.stats.map_sd_reads,
                        bytes_read: fp.stats.map_bytes_read,
                        collect_us: fp.stats.collect_us,
                        read_us: 0,
                        sort_us: fp.stats.sort_us,
                        draw_us: fp.stats.draw_us,
                        overlay_us: 0,
                        mpp_milli,
                    };
                }

                // A transport fault (`present` → false, e.g. a stalled FLPR) latches a retry like the
                // reader-build failure rather than faulting.
                if !fp.ok {
                    pending_map_redraw = true;
                }

                // ── DFU trial confirm (epic #615 S4, #619), once, at the health anchor ──
                // A frame just landed on glass and the SD mounted at boot: if this boot is a
                // trial (`Trial { installed, .. }` on the boot-state page), write
                // `Idle { installed }` — the whole confirm — and hand the app the one-time
                // "updated to vX" fact for S5's toast. A failed first present retries the
                // anchor on a later pass; an unconfirmed trial rolls back next boot by design.
                if trial_confirm_pending && fp.ok {
                    trial_confirm_pending = false;
                    if let Some(installed) = crate::dfu::confirm_trial(settings_store) {
                        app.notify_update_confirmed(installed.fw_version_str());
                    }
                }
                // A map frame carries the map render stats; a non-map (menu / Statistics / Home) frame
                // is just a screen redraw + push, so log it as such — no meaningless lod/feat/chunks.
                if needs_map {
                    defmt::info!(
                        "map frame: render {=u64} us + push {=u64} us | lod {=usize} | feat {=usize}/{=usize} | chunks {=usize} | map-cache {=u32} hit / {=u32} miss",
                        fp.render_us,
                        fp.push_us,
                        fp.stats.lod,
                        fp.stats.features_drawn,
                        fp.stats.features_tried,
                        fp.stats.chunks_visited,
                        fp.stats.map_chunk_hits,
                        fp.stats.map_chunk_misses
                    );
                } else {
                    // A menu / Statistics / Home redraw: just its own chrome + the (now self-diffed)
                    // push, so the partial-push win shows as a small `push` next to the full `render`.
                    defmt::info!(
                        "ui frame: render {=u64} us + push {=u64} us (screen redraw, no map)",
                        fp.render_us,
                        fp.push_us
                    );
                }
            }
        }

        // The hold bulge already pushed at the top of this pass (bulge-first, see above). But if a
        // screen present just landed, its `exclude` skipped the bulge rows — they still show the *old*
        // frame under the bulge. Re-composite them over the fresh fb now (a ~12 ms partial push, only
        // on the rare pass where a redraw and a live bulge coincide) so the band never lags the screen.
        if dirty.map && overlay_span.is_some() {
            display.present_bulge(overlay_span, false).await;
        }

        // A deferred ride save (Finish stashed it — see `Storage::run_pending_save`): run it only
        // once the hold bulge is fully quiet, i.e. the confirm pop and its trailing clear have
        // played out. The ORD conversion is the one long blocking SD stretch left in this loop, so
        // it grinds against a static, already-presented screen instead of freezing the animation.
        // `animating` below keeps the loop's short cadence while a save is still pending.
        if overlay_span.is_none() && !overlay_dirty {
            if let Some(s) = storage.as_mut() {
                s.run_pending_save(settings_store);
            }
        }
        // A ride object landed this pass (the deferred Finish above, or a back-to-back flush inside
        // `begin_track`/`reconcile_track` earlier in the pass) — raise the store edge so a fresh
        // `RD{id}.ORD` is visible *now*, not after a reboot (the boot scan used to be the only
        // reader). `ble`: post the saved-ride edge to the BLE plane, which owns the `ObjectStore`
        // — it re-scans its catalog + bumps the revision (phone `storeChanged(ride)` + digest), and
        // the resulting `STORE_CHANGED` edge re-feeds the Rides menu next pass: one edge, every
        // consumer, exactly like an upload or delete. Map-only: re-feed the Rides menu directly
        // (`load_rides` is its own popped frame — see its stack note — called sequentially here,
        // never under the deep render path).
        if storage.as_mut().is_some_and(sd::Storage::take_ride_saved) {
            #[cfg(feature = "ble")]
            crate::object_store::note_ride_saved();
            #[cfg(not(feature = "ble"))]
            if let Some(s) = storage.as_mut() {
                load_rides(s, app);
            }
        }
        let save_pending = storage.as_ref().is_some_and(|s| s.has_pending_save());

        // All card + settings work for this pass is done — release the shared store before the tail
        // (telemetry, LED, and the event-wait `.await`) so a future BLE object plane isn't held off
        // across the sleep (#270). `storage`/`settings_store` are unused past here.
        drop(store_guard);

        // Publish render-stats telemetry host-ward at ~2 Hz: throttled here (not in the TX task) so the
        // link never floods and the device never stalls on it.
        #[cfg(feature = "debug-uart")]
        if now.wrapping_sub(last_telem_ms) >= 500 {
            last_telem_ms = now;
            obc_platform::debug_link::set_telemetry(last_telem);
        }

        if now.wrapping_sub(last_led) >= 500 {
            led.toggle();
            last_led = now;
        }

        // ===================== Event-driven sleep =====================
        // Instead of a fixed ~8 ms tick, block until the next *real* wake: a recognised gesture
        // (`GESTURES` non-empty — a non-consuming `ready_to_receive`, so the drain at the loop top still
        // gets it), a hold starting to charge (`INPUT_WAKE` — a press emits no gesture, so without this
        // arm the loop slept through the whole charge on a quiet screen and the bulge's first frame on
        // glass was the confirm pop), a fresh sensor/host datapoint (`wait_sensor_event`), a BLE link
        // edge (`wait_ble_edge` — connect/disconnect *and* the pairing passkey, so the passkey card
        // wakes the loop from warm sleep), or the soonest screen animation deadline the app reports.
        // The body's reconciles are all edge-gated,
        // so running them only on a wake is correct — a parked Home screen wakes ~once a minute (the
        // clock minute-tick) instead of 125×/s, and an idle device with the GPS asleep wakes only on a
        // button or that minute tick.
        // While something is **actively animating** — a live hold bulge (`overlay_*`, incl. its retract),
        // a charging hold on either button (`charging`), a redraw a flaky SD glitch couldn't service
        // (`pending_map_redraw`), or a deferred ride save (`save_pending`) — keep the short cadence so
        // it stays fluid; otherwise arm the app's single next-wake deadline, or sleep indefinitely
        // until input/sensor.
        let charging = hold_p > 0.0 || display.hold_charging();
        #[cfg(has_nav)]
        let planning = nav_run.is_some();
        #[cfg(not(has_nav))]
        let planning = false;
        let animating =
            charging || planning || pending_map_redraw || overlay_dirty || overlay_span.is_some() || save_pending;
        let next_ms = if animating { Some(LOOP_MS as u32) } else { app.ms_until_next_wake(now) };
        // debug-uart host build: keep a ~2 Hz floor so streamed telemetry / `Z` zoom commands stay
        // responsive even on an otherwise-quiet screen (well under the WDT feed cap).
        #[cfg(feature = "debug-uart")]
        let ms = next_ms.unwrap_or(WDT_FEED_CAP_MS).min(500);
        // The indefinite sleep is capped at ~WDT/2 (#349) so an otherwise-idle device still wakes
        // to feed the watchdog — the `None` (sleep-until-input/sensor) arm becomes a long timer.
        #[cfg(not(feature = "debug-uart"))]
        let ms = next_ms.unwrap_or(WDT_FEED_CAP_MS).min(WDT_FEED_CAP_MS);
        let _ = select5(
            GESTURES.ready_to_receive(),
            INPUT_WAKE.wait(),
            // A sensor/host datapoint, or (`ble` builds) a store movement — an upload/delete
            // rescans the catalog now, not at the next timer wake (#450).
            wait_host_or_sensor_event(),
            // A BLE link edge — connect/disconnect *and* the pairing passkey — so the passkey card
            // wakes the loop from warm sleep (epic #447, P2). `pending()` on a map build.
            wait_ble_edge(),
            Timer::after_millis(ms as u64),
        )
        .await;
    }
}
