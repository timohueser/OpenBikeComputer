//! The map/ride thread-mode plane (`has_map` builds only) — split out of `main.rs` (issue #351).
//!
//! [`run_app`], the shared backend-agnostic ride loop, plus its loop-only helpers: the sensor-wake
//! select arm, the GPS power policy, the watchdog cadence, the per-frame render clock, and the
//! route-catalog scan. `main` still owns bring-up + the resident statics and awaits [`run_app`]
//! as its tail future (single call site — see the `#[inline(always)]` note on the fn).

use core::sync::atomic::Ordering;

// The event-driven loop's wake select: `select3` over gesture / sensor / deadline.
use embassy_futures::select::select3;
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
// `.gpx` (the default streams the real SAM-M10Q; `debug-uart` a recorded host ride).
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
use obc_platform::SynthLocation;
// The map render's framebuffer adapter (the status screen builds its own inside `ble.rs`) + the
// battery stand-in until the nPM1300 PMIC gauge is read.
use obc_platform::{FbDevice64, StubFuelGauge};
use obc_reader::{MapCache, MapTables, Reader};
// The ride loop's route types: the decoded-route-geometry cache, the resident per-route chunk
// index, and the streamed route reader the matcher + map render share.
use obc_route::{RouteCache, RouteIndex, RouteReader};

use crate::display::{DisplayDriver, FRAME_H, FRAME_W};
use crate::planes::{MapDisplay, GESTURES, INPUT_HB_MS, LOOP_MS};
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

/// A `no_std` [`Clock`](obc_render::Clock) over embassy's monotonic `Instant`, in microseconds — the
/// time base for the map render's per-stage timing (collect / sort / draw) the VCOM telemetry
/// carries. The same monotonic clock the loop's frame `Instant` reads, so the stages reconcile.
struct InstantClock;
impl obc_render::Clock for InstantClock {
    fn now_us(&self) -> u64 {
        Instant::now().as_micros()
    }
}

/// Scan the card's `/routes/*.obcr` catalog into the app's Route menu. Deliberately its **own
/// `#[inline(never)]` frame**: the ~5 KB [`Catalog`](obc_app::Catalog) (`Vec<RouteSummary,
/// MAX_ROUTES>`, 64 × ~84 B) lives here and is popped on return, so it never sits on `main`'s frame
/// *beneath* the long-lived [`run_app`] ride loop — where a resident 5 KB catalog would steal from the
/// deep route-load render path's stack and overflow the 256 KB part.
#[inline(never)]
pub(crate) fn load_routes(storage: &mut sd::Storage, app: &mut App) {
    let catalog = storage.scan_routes();
    app.set_routes(&catalog);
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
/// finalise-to-GPX the ride log), feed the sensors → `tick` (integrate the fix, map-match, log the
/// track point), then re-render the map only on `dirty.map` and present it. A static screen does zero
/// map renders. LED0 keeps a ~1 Hz heartbeat. Never returns.
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

    // Settings: seed the app from the persistent RRAM store at boot (a blank/corrupt page decodes to
    // `None` → defaults), then persist on any change the settings screens make. One brief lock,
    // released at once — the loop re-locks the shared store each pass.
    app.set_settings({
        let mut store = shared.lock().await;
        store.settings.load().unwrap_or_default()
    });

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

        // Apply the high-priority plane's recognised gestures, in order, then advance animations.
        // The screen transition lands a frame after the overlay already confirmed the press.
        while let Ok(g) = GESTURES.try_receive() {
            app.apply_gesture(g);
        }
        app.advance_animations(InputClock(now));

        // Lock the shared store for the rest of this pass: the settings save just below, the card
        // reconcile, the per-frame route/track/map sources, and the render that reads them all run
        // under one guard — held across the render `.await`, then dropped before the event-wait at the
        // loop tail so a future BLE object plane reaches the card between passes (#270). Destructured
        // into the two names the body already uses (`storage`, `settings_store`).
        let mut store_guard = shared.lock().await;
        let SharedStore { storage, settings: settings_store } = &mut *store_guard;

        // Persist settings the moment a settings screen changes one: one in-place 16-byte RRAM line,
        // skipped when nothing changed.
        if app.take_settings_dirty() {
            settings_store.save(app.settings());
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
        // log (begin on load, finalise-to-GPX on Finish), reading the save name from the active route.
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
            storage.reconcile_route(active);
            storage.reconcile_track(action, session, &name, stats.as_ref());
            prev_active = active;
            prev_session = session;
        }

        // Cache the active route's chunk index across frames: rebuild it (the header + full chunk-meta
        // walk off SD) only when the route changes, or retry if a prior build failed on a flaky link.
        // Not gated on rendering — the matcher in `tick` needs the index on every fresh fix.
        if index_route != active {
            route_cache.clear(); // a route switch: drop stale slots (the cache keys by chunk index only)
            match active {
                Some(_) => match storage.build_route_index() {
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
        let route_src = storage.route_source();
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(src)) => Some(RouteReader::new_cached(idx, src, route_cache)),
            _ => None,
        };
        // The ride-log sink, built every tick (it only wraps the open log handle, no I/O), so a fresh
        // fix is written to the `.gpx` the moment it arrives, at the fix rate.
        let mut tsink = storage.track_sink();
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
        // previous frame couldn't service on a transient reader-build failure.
        let mut dirty = app.take_dirty();
        dirty.map |= pending_map_redraw;
        pending_map_redraw = false;
        // A FLPR relaunch landed since the last pass (#349): the fresh core has no frame history
        // and the diff store was reset — schedule the full repaint even if nothing else is dirty.
        dirty.map |= display.take_relaunch_repaint();
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
        }
        prev_hold_p = hold_p;

        // This frame's hold-bulge state, sampled once: the live row span on both backends (the present
        // goes around it); the dirty edge only on the FLPR (whose map plane owns the bulge re-push —
        // on ST7789 the input/overlay plane consumes that edge itself).
        let (overlay_dirty, overlay_span) = display.poll_overlay();

        // The hold bulge pushes **before** any screen redraw this pass (#348 follow-up): a fired hold
        // usually navigates, so with the bulge last its confirm pop's first frame queued behind the
        // new screen's render + present — ~40 ms on a menu, 150–300 ms on the map view, where the
        // whole 220 ms pop expired unseen (the "sometimes it just snaps" inconsistency). Bulge-first,
        // the pop's attack lands on glass within ~10 ms of the fire, holds at pop depth while the new
        // screen renders (composited over the *old* fb for that one frame — correct: that is what is
        // on glass until the present below), and eases out on the following passes. ST7789: no-op.
        display.present_bulge(overlay_span, overlay_dirty).await;

        if dirty.map {
            // The map pipeline runs **only when the base screen actually draws the map** (the Map
            // view). On a menu / Statistics / Home redraw it's skipped entirely — no SD style-table
            // parse, no `Reader` build (so no stack spike), no map render — that screen draws just its
            // own chrome. A non-map frame costs only its own draw + the push.
            let needs_map = app.base_draws_map();
            // Build the streamed `Reader` **only** on a map frame, `None` otherwise. A *cheap* borrow of
            // the boot-parsed `MapTables` + a fresh `src` + the session-long `MapCache` — no style-table
            // SD read, no parse, no stack spike (what kept this deep path inside the 256 KB stack). The
            // only per-frame failure left is the source handle being momentarily unavailable (a flaky SD
            // link); skip the redraw, keep the last frame, latch a retry.
            let map_src = if needs_map { storage.map_source() } else { None };
            let reader = map_src.as_ref().map(|s| Reader::new(s, map_tables, map_cache));
            if needs_map && reader.is_none() {
                pending_map_redraw = true;
                defmt::warn!(
                    "map: reader build failed this frame (flaky SD?) — kept frame, retrying redraw next frame"
                );
            } else {
                // Render the whole frame into the resident RGB222 plane, then present it — the single
                // per-backend boundary, behind `MapDisplay::render_present` (ST7789 bands the whole
                // frame under its bus lock; the FLPR scans it, going *around* a live bulge's rows so the
                // composite below paints them). `render_map_timed` threads `InstantClock` so the stats
                // carry the collect/sort/draw timings; the hold bulge is **not** composited here — it
                // rides `present_bulge` on its own plane.
                let render = |d: &mut dyn DisplayDriver| {
                    let mut fbdev = FbDevice64::new(d.fb_mut(), FRAME_W as u32, FRAME_H as u32);
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
        // gets it), a fresh sensor/host datapoint (`wait_sensor_event`), or the soonest screen animation
        // deadline the app reports. The body's reconciles are all edge-gated, so running them only on a
        // wake is correct — a parked Home screen wakes ~once a minute (the clock minute-tick) instead of
        // 125×/s, and an idle device with the GPS asleep wakes only on a button or that minute tick.
        // While something is **actively animating** — a live hold bulge (`overlay_*`, incl. its retract),
        // a charging in-screen hold (`hold_p`), or a redraw a flaky SD glitch couldn't service
        // (`pending_map_redraw`) — keep the short cadence so it stays fluid; otherwise arm the app's
        // single next-wake deadline, or sleep indefinitely until input/sensor.
        let animating = hold_p > 0.0 || pending_map_redraw || overlay_dirty || overlay_span.is_some();
        let next_ms = if animating { Some(LOOP_MS as u32) } else { app.ms_until_next_wake(now) };
        // debug-uart host build: keep a ~2 Hz floor so streamed telemetry / `Z` zoom commands stay
        // responsive even on an otherwise-quiet screen (well under the WDT feed cap).
        #[cfg(feature = "debug-uart")]
        let ms = next_ms.unwrap_or(WDT_FEED_CAP_MS).min(500);
        // The indefinite sleep is capped at ~WDT/2 (#349) so an otherwise-idle device still wakes
        // to feed the watchdog — the `None` (sleep-until-input/sensor) arm becomes a long timer.
        #[cfg(not(feature = "debug-uart"))]
        let ms = next_ms.unwrap_or(WDT_FEED_CAP_MS).min(WDT_FEED_CAP_MS);
        let _ = select3(GESTURES.ready_to_receive(), wait_sensor_event(), Timer::after_millis(ms as u64)).await;
    }
}
