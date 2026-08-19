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
// The Recalculating banner's framebuffer clip (#1146 P2) — the band `App::reroute_banner_rows`
// reports, expressed in the same `Rectangle` vocabulary `Dirty::region` already uses.
use embedded_graphics::prelude::{Point, Size};
use embedded_graphics::primitives::Rectangle;
// `SettingsStore` (the load/save trait) is the ride loop's seam over the RRAM store; the `ble`
// build's store lives inside `object_store` (which imports it itself).
use obc_app::App;
use obc_ports::{InputClock, RideClock, Sensors, SettingsStore, TrackSink};
// The instance-owned sensor hub's control handle + GPS power enum (#808): the ride loop sets the
// rate/power latches the `sensors::sensor_task` awaits. Real-sensor build only.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use obc_platform::sensor_hub::GpsPower;
// The hub consumer handle threaded from `main` (the ZST `*Source` drains + presence + the event
// wake) — present on every build that uses the hub (real-sensor GPS, or debug-uart HR/power/cadence
// injection); absent only on the pure `synth` build.
#[cfg(not(all(not(feature = "debug-uart"), feature = "synth")))]
use obc_platform::sensor_hub::SensorConsumer;
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use obc_platform::sensor_hub::SensorControl;
// The `synth`-build stand-in GPS: walks a slow square loop so a saved ride is a non-degenerate
// ride object (the default streams the real SAM-M10Q; `debug-uart` a recorded host ride).
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
use obc_platform::SynthLocation;
// The map render's framebuffer adapter (the status screen builds its own inside `ble.rs`) + the
// battery stand-in until the nPM1300 PMIC gauge is read.
use obc_display::ls021::{FRAME_H, FRAME_W};
use obc_display::FbDevice64;
#[cfg(has_nav)]
use obc_formats::io::{ByteSink, Error as ByteError};
#[cfg(has_nav)]
use obc_formats::obcr::HEADER_FULL_LEN;
use obc_platform::StubFuelGauge;
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
/// Watchdog period: 24 s of 32768 Hz LFCLK ticks (the issue's 16–30 s band). The value lives in
/// `obc-dfu` since DR1 (#729): it is a boot-chain handoff contract — the bootloader must build
/// the byte-identical WDT config to adopt this dog across a DFU install and to pre-start the one
/// a trial boot runs under (see the contract note on the constant).
pub(crate) const WDT_TIMEOUT_TICKS: u32 = obc_dfu::WDT_TIMEOUT_TICKS;
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

/// Inputs that make the resident weather snapshot materially different. Keeping this named makes
/// the retry/cache rule below readable and avoids hiding its four independent invalidation axes
/// inside a nested tuple type at the use site.
type WeatherSampleKey = (crate::flat_store::FlatWeather, Option<(i32, i32)>, Option<(Option<usize>, u32, u32)>, i64);
/// Fixed-position weather has no clock input. This impossible minute bucket keeps the compact
/// `i64` cache-key field (an `Option<i64>` costs another eight resident bytes on Thumb). It cannot
/// collide with the projected key: dividing any cast `i64` wall time by 60 is strictly greater
/// than `i64::MIN`.
const WEATHER_TIME_INDEPENDENT: i64 = i64::MIN;

/// Synthetic-walk advance cadence (ms) on the `synth` build: the stand-in GPS publishes no `Signal`,
/// so the event-driven loop has no sensor event to wake on and falls back to this timer to step the
/// square-loop walk. The walk position is time-based, so a slower tick just lowers the demo frame rate.
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
const SYNTH_TICK_MS: u64 = 250;

/// The single sensor/host wake the event-driven map loop selects on — one `await` that covers the
/// whole sensor set so the loop sleeps until a datapoint actually arrives. Three builds:
/// - default (real sensors): the hub's unified `wait_event` datapoint edge (fix / baro / temp / GPS
///   time / heading) — exactly one wake per published sample, zero I²C at the frame rate;
/// - `debug-uart`: the host-streamed datapoint edge from the VCOM debug link;
/// - `synth`: no event source, so a coarse timer steps the synthetic walk.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
async fn wait_sensor_event(consumer: SensorConsumer<'static>) {
    consumer.wait_event().await
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

#[cfg(feature = "ble")]
fn weather_refresh_in_flight() -> bool {
    crate::ble::weather_refresh_in_flight()
}

#[cfg(not(feature = "ble"))]
const fn weather_refresh_in_flight() -> bool {
    false
}

/// The loop's third select arm: a sensor/host datapoint, or a flat-store movement on either link —
/// a route/trip commit/delete wakes the loop so the live-catalog rescan (#450) lands now, not at
/// the next timer/sensor wake (a parked device otherwise dozes up to the ~12 s watchdog-feed cap).
async fn wait_host_or_sensor_event(
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))] consumer: SensorConsumer<'static>,
) {
    embassy_futures::select::select(
        wait_sensor_event(
            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
            consumer,
        ),
        embassy_futures::select::select(
            crate::flat_store::wait_catalog_commit(),
            crate::object_store::wait_store_changed(),
        ),
    )
    .await;
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

/// Scan the card's `/tracks` into the app's Rides menu (epic #447 P7 / #454), carrying each ride's
/// durable object id + its synced flag (from the `/tracks` synced-set sidecar). Called at boot and on
/// every store-changed edge (a finished ride, an on-device or phone-side ride delete). Its own
/// `#[inline(never)]` frame so the ride catalog is popped on return, never resident under the deep
/// render path.
#[inline(never)]
pub(crate) fn load_rides(storage: &mut sd::Storage, app: &mut App) {
    let mut catalog = heapless::Vec::new();
    storage.scan_rides_into(&mut catalog);
    app.set_rides(&catalog, storage.ride_ids());
    // Feed the **full** compact ride-retention inventory (finding #876-2): every synced ride, not
    // just the newest-32 the menu shows, so the auto-delete sweep + eager stamp reach older synced
    // rides. Independent of the display catalog above; one extra synced-set read.
    app.set_ride_retention_inventory(&storage.ride_retention_inventory());
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
    // The `LoadRideTrack` derived fill level, answered off the pure predicate (#812): nothing is
    // consumed, so a missed pass re-asks and the cue clears the moment `set_ride_profile` lands.
    let Some(id) = app.ride_track_request() else { return };
    let profile = storage.as_mut().and_then(|s| s.ride_profile_by_id(id));
    if profile.is_none() {
        defmt::warn!("ride profile: fill for id {=u16} failed — the detail's band stays empty", id);
    }
    app.set_ride_profile(profile);
    // The track-shape preview (#678 rework 3) rides the same drain: a second forward stream of
    // the `RD{id}.ORD` into the ≤ 64-point resident (a 512 B copy + the ~448 B block buffer in
    // this same popped frame — small next to the profile builder's column scratch above).
    let preview = storage.as_mut().map(|s| s.ride_preview_by_id(id)).unwrap_or_default();
    app.set_ride_preview(&preview);
}

/// The router's **resident** half (epic #116 R4 + EL7, #1068) — everything the planner needs that
/// is *not* an arm of the scratch arena.
///
/// Since #1146 P2 that is one field. The A* table, the graph-tile cache and the resumable planner's
/// slot moved into [`arena::NavArm`](crate::arena::NavArm), claimed for the span of a search; the
/// terrain stayed here because it is read at **fix** cadence during a ride
/// (`App::sample_terrain`, EL8) — while the map plane is rendering and no search is running — so it
/// is state, not scratch, and folding it into an arm would have handed the render arm's `memset` the
/// altimeter's tile cache.
#[cfg(has_nav)]
pub(crate) struct NavResident {
    /// The map's terrain, or the null source: the emit phase samples it per point, and the ride
    /// loop's altimeter fuse samples it per fix. `&'static mut` because a `TerrainElevation` carries
    /// its ~2.1 KB tile cache inline and must never be copied into a plan frame (#419/#501).
    pub(crate) elev: &'static mut dyn obc_route::ElevationSource,
}
/// The `ble` build's stand-in: the router isn't in the combined image — its statics would push the
/// 256 KB DK's stack region below the measured deep-render peak (see build.rs's `has_nav` note).
/// The ride loop still drains create-route requests and answers the generic failure tier, so the POI
/// confirm never hangs. The 512 KB LM20 deletes this arm.
#[cfg(not(has_nav))]
pub(crate) struct NavResident;

/// One plan step's view of everything the planner touches: the scratch arena's nav arm, borrowed
/// from the guard the ride loop holds for the whole search, plus the resident terrain beside it.
///
/// A **view**, not an owner — rebuilt per call from `(&mut NavGuard, &mut NavResident)`, so the arm
/// and the terrain are borrowed only for the length of one synchronous planner step and no reference
/// into the arena is ever live across an `.await`.
#[cfg(has_nav)]
struct NavBuffers<'a> {
    /// The guard itself rather than the arm behind it: the planner slot is a `MaybeUninit` the
    /// claim deliberately leaves unwritten, and the guard is what carries "a plan has been written
    /// into it" (`NavGuard::plan_parts` / `planner_ref`). Reaching past it to `&mut NavArm` would
    /// put that fact back in this loop's bookkeeping, where nothing checks it.
    guard: &'a mut crate::arena::NavGuard,
    elev: &'a mut dyn obc_route::ElevationSource,
}

/// One in-flight plan's **board-side bookkeeping** (#499): the open reserved-file handle and the
/// per-phase wall-time accumulators the RTT line reports. Loop-local (small); the ~9.5 KB planner
/// itself sits in the [`NavBuffers`] `.bss` slot this struct guards.
#[cfg(has_nav)]
struct NavRun {
    allocation: Option<obc_storage::flat::Allocation>,
    io: NavIo,
    cancel_requested: bool,
    io_started: Instant,
    /// Wall time when the request was drained — the RTT line's user-perceived `total_ms`.
    t0: Instant,
    /// Per-phase step time (µs), attributed by the planner's phase **before** each step:
    /// `[snap, search, emit]`.
    phase_us: [u64; 3],
    /// Store-task time spent flushing bounded output stages before the final checksum/commit.
    write_us: u64,
    /// Physical-card reads issued inside planner steps, split by the same phase. Unlike the cache
    /// counters this sees sector-splitting and alignment bounces at the block-device boundary.
    #[cfg(feature = "sd-bench")]
    read_perf: [crate::card_io::ReadPerf; 3],
}

#[cfg(has_nav)]
enum NavIo {
    NeedAllocate,
    Allocating(crate::flat_store::Ticket),
    Ready,
    Staged(NavStep),
    Flushing(crate::flat_store::Ticket, obc_route::Step),
    NeedFinish(obc_route::Step),
    Finishing { ticket: crate::flat_store::Ticket, outcome: obc_route::Step, publishing: bool },
    NeedPublishCompensation(obc_storage::flat::ObjectId),
    CompensatingPublish(crate::flat_store::Ticket, obc_storage::flat::ObjectId),
}

/// Maximum bytes a computed OBCR can emit: 128-byte header, 256 full 1,530-byte chunk bodies,
/// and 256 44-byte index records. The flat store rounds this reservation to one extent and trims
/// it to the actual streamed length at commit.
#[cfg(has_nav)]
const NAV_ROUTE_RESERVE: u64 = 128 + 256 * (1_530 + 44);

#[cfg(has_nav)]
static NAV_STORE_REPLY: crate::flat_store::Reply = embassy_sync::signal::Signal::new();

#[cfg(has_nav)]
struct NavStageSink<'a> {
    stage: &'a mut [u8; crate::arena::NAV_OUTPUT_STAGE_BYTES],
    appended: usize,
    patch_len: usize,
}

#[cfg(has_nav)]
impl ByteSink for NavStageSink<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<(), ByteError> {
        let start = HEADER_FULL_LEN + self.appended;
        let end = start.checked_add(bytes.len()).ok_or(ByteError::TooLarge)?;
        let out = self.stage.get_mut(start..end).ok_or(ByteError::TooLarge)?;
        out.copy_from_slice(bytes);
        self.appended += bytes.len();
        Ok(())
    }

    fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), ByteError> {
        if offset != 0 || bytes.len() > HEADER_FULL_LEN {
            return Err(ByteError::BadOffset);
        }
        self.stage[..bytes.len()].copy_from_slice(bytes);
        self.patch_len = bytes.len();
        Ok(())
    }
}

#[cfg(has_nav)]
#[derive(Clone, Copy)]
struct NavStep {
    outcome: obc_route::Step,
    appended: usize,
    patch_len: usize,
}

#[cfg(has_nav)]
fn start_nav_flush(
    writer: crate::flat_store::Writer,
    guard: &mut crate::arena::NavGuard,
    allocation: obc_storage::flat::Allocation,
    step: NavStep,
) -> Result<crate::flat_store::Ticket, NavStep> {
    let base = guard.output.as_ptr();
    // SAFETY: the arena is static storage; its guard stays held while this ticket is live, and the
    // ride loop will not run another planner step until the storage task has answered.
    let bytes = unsafe { core::slice::from_raw_parts(base.add(HEADER_FULL_LEN), step.appended) };
    let header = unsafe { core::slice::from_raw_parts(base, step.patch_len) };
    let request = crate::flat_store::Request::WriteComputedRoute { allocation, bytes, header };
    writer.try_call(request, &NAV_STORE_REPLY).map_err(|_| step)
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
    nav.guard.begin_plan(obc_route::NavPlanner::new(req.from, req.to, req.name(), profile_idx));
    // One diagnostic line per plan start (#501 fault dossiers): the three addresses pin the memory
    // map without needing the ELF at hand. Since #1146 P2 they are offsets **inside the scratch
    // arena's nav arm** rather than three separate `.bss` statics — so the line now also says which
    // arm the block is serving, which is the first thing to check if a plan ever comes back wrong.
    let (planner, scratch, tiles) = nav.guard.arm_addrs();
    defmt::debug!(
        "nav plan: start planner=0x{=usize:08x} scratch=0x{=usize:08x} tiles=0x{=usize:08x}",
        planner,
        scratch,
        tiles
    );
}

/// Take everything a fresh route search needs, in the order that leaves nothing half-held: the
/// transfer gate's **search arm** first (a cable transfer streaming into the same store must win —
/// the arena's `nav ⊥ usb` rule), then the scratch arena's **nav arm** against the app's own
/// quiesced-map proof.
///
/// `Err(why)` = the caller must answer the failure tier now rather than arm a plan; every refusal
/// path has already given back whatever it took, so no spinner can hang behind a half-claim. A
/// request arriving while a plan is still in flight is *not* a refusal: we already hold both, and
/// the drain overwrites the planner slot for the new plan exactly as it did before.
#[cfg(has_nav)]
fn nav_take_arena(app: &App, guard: &mut Option<crate::arena::NavGuard>) -> Result<(), &'static str> {
    if guard.is_some() {
        return Ok(());
    }
    if !crate::link::TRANSFER_ACTIVE.begin_search() {
        return Err("a cable transfer holds the store");
    }
    let Some(quiesced) = app.nav_arena_precondition() else {
        // Unreachable by construction: draining a plan command is what engages the Recalculating
        // freeze, so by the time we are here the map plane is already quiet over a map base — and
        // menu planning has no map base to quiet. Loud in debug, handled in release.
        crate::link::TRANSFER_ACTIVE.end_search();
        debug_assert!(false, "a plan drained with the map plane still drawing — the freeze did not engage");
        return Err("the map plane is not quiesced");
    };
    match crate::arena::claim_nav(quiesced) {
        Ok(g) => {
            *guard = Some(g);
            Ok(())
        }
        Err(_) => {
            crate::link::TRANSFER_ACTIVE.end_search();
            Err("the scratch arena is busy")
        }
    }
}

/// The fixed slot index (0 HR · 1 Power · 2 Cadence) a scanned sensor's kind maps to (SE7, #714) —
/// used to tag a board scan hit for the app seam, which speaks slot indices, not `obc_ble` kinds.
#[cfg(feature = "ble")]
fn sensor_kind_slot(kind: obc_ble::SensorKind) -> u8 {
    match kind {
        obc_ble::SensorKind::HeartRate => 0,
        obc_ble::SensorKind::Power => 1,
        obc_ble::SensorKind::Cadence => 2,
    }
}

/// Distil the central manager's per-quantity sensor status into
/// the app-vocabulary [`SensorStatus`](obc_app::SensorStatus) the Sensors screen renders (SE7, #714):
/// `NotSet` when nothing is saved, else the connection phase, carrying battery + the freshest-value
/// tick.
#[cfg(feature = "ble")]
fn sensor_status_of(q: usize) -> obc_app::SensorStatus {
    use crate::ble::SensorSlotState;
    let s = crate::ble::sensor_slot_status(q);
    let phase = if !s.saved {
        obc_app::SensorPhase::NotSet
    } else {
        match s.state {
            SensorSlotState::Connected => obc_app::SensorPhase::Connected,
            SensorSlotState::Connecting => obc_app::SensorPhase::Connecting,
            SensorSlotState::Idle => obc_app::SensorPhase::Searching,
        }
    };
    obc_app::SensorStatus { phase, battery: s.battery, last_value_ms: s.last_value_ms }
}

/// Run **one bounded planner step** at the ride loop's shallow per-pass depth (#270/#419: the
/// step's frame carries only the cheap boot-parsed-tables `Reader` view, the bounded staging sink,
/// and the planner's own shallow call tree — the emitter lives in the planner's
/// `.bss` slot, not this frame; the deepest transient is the one emitter-sized move when the
/// emit phase constructs it). Everything long-running (render, input, the watchdog feed) runs
/// normally **between** steps — that is the whole point of #499.
///
/// The reader is `Reader::new` over the flat map object. Emitted OBCR bytes are staged in the arena
/// and flushed through the flat store's sole writer between steps, so no card write occurs inside
/// the synchronous planner call and the arena remains the only route-output scratch owner.
#[cfg(has_nav)]
#[inline(never)]
fn nav_step(
    map_src: &dyn obc_formats::io::ByteSource,
    map_tables: &MapTables,
    map_cache: &MapCache,
    nav: &mut NavBuffers<'_>,
) -> NavStep {
    let reader = Reader::new(map_src, map_tables, map_cache);
    // Only called while a `NavRun` is active, and a run is only created after the drain wrote the
    // planner — but the guard is what *knows* that, so ask it rather than assert it. An unwritten
    // slot answers the generic failure tier instead of stepping a planner nobody built.
    let Some((planner, scratch, tiles, output)) = nav.guard.plan_parts() else {
        debug_assert!(false, "a plan step with no planner written — the run outlived (or preceded) its drain");
        return NavStep { outcome: obc_route::Step::Failed(obc_route::NavError::NoPath), appended: 0, patch_len: 0 };
    };
    let mut sink = NavStageSink { stage: output, appended: 0, patch_len: 0 };
    let outcome = planner.step(&reader, scratch, tiles, &mut *nav.elev, &mut sink);
    NavStep { outcome, appended: sink.appended, patch_len: sink.patch_len }
}

/// Finish a **completed** plan: hash and publish or cancel the flat-store reservation, rescan +
/// re-feed the id-carrying catalog on success (sequential with — never nested under — the step frames, the
/// #496 de-nesting kept), emit the one `nav route:` RTT line with the per-phase breakdown
/// (issue #499's DoD), and answer the app — the `NavPlanned` event activates the route and swaps
/// the planning screen for the computed-route overview (or the failure card).
///
/// The RTT line (grep `nav route:`): outcome; route length; `total_ms` = wall time from the
/// request drain (it spans every pass the plan was spread over); `snap/search/emit_ms` = step
/// time attributed to the planner's phase before each step (emit includes the finishing header
/// patch); `write_ms` = flat-store write/hash/commit time; `rescan_ms` = the catalog rescan; `source_reads` =
/// logical graph-chunk plus index-window fills; `settles`; and the stackmeter high-water,
/// force-rescanned here — sentinel evidence is permanent, so it still reads the in-step peak.
/// With `sd-bench`, a second line reports the planner steps' physical card commands and time.
#[cfg(has_nav)]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn nav_finish(
    app: &mut App,
    nav: &mut NavBuffers<'_>,
    run: NavRun,
    result: Result<(u64, u32), obc_route::NavError>,
    now: u32,
) {
    use obc_route::NavError;
    let write_us = run.write_us;
    let rescan_us = 0;
    let cache = nav.guard.tiles.stats();
    // The ε rung the plan ended on (N8): 13/10 for a plain success or a fast no-path, 2/1 or 3/1 if
    // the ε-escalation ladder retried on exhaustion. `settles` is cumulative across the rungs. Both
    // read through the guard's checked accessor — a finish with no planner written reports zeroes
    // rather than an uninitialized read.
    let (settles, eps_num, eps_den) = nav.guard.planner_ref().map_or((0, 0, 0), |p| {
        let (n, d) = p.epsilon_used();
        (p.settles(), n, d)
    });
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
        "nav route: {=str} len={=u32} total_ms={=u64} snap_ms={=u64} search_ms={=u64} emit_ms={=u64} write_ms={=u64} rescan_ms={=u64} source_reads={=u32} graph_reads={=u32} index_reads={=u32} settles={=u32} eps={=u32}/{=u32} stack_hw={=usize}/{=usize}",
        outcome_str,
        len,
        run.t0.elapsed().as_millis(),
        run.phase_us[0] / 1000,
        run.phase_us[1] / 1000,
        run.phase_us[2] / 1000,
        write_us / 1000,
        rescan_us / 1000,
        cache.source_reads(),
        cache.misses,
        cache.index_misses,
        settles,
        eps_num,
        eps_den,
        hw,
        stackmeter::total()
    );
    #[cfg(feature = "sd-bench")]
    {
        let mut total = crate::card_io::ReadPerf::ZERO;
        for phase in run.read_perf {
            total.add_assign(phase);
        }
        defmt::info!(
            "nav SD bench: total_us={=u32} commands={=u32} blocks={=u32} single={=u32} multi={=u32} snap_us={=u32} snap_cmds={=u32} search_us={=u32} search_cmds={=u32} emit_us={=u32} emit_cmds={=u32}",
            total.us,
            total.commands,
            total.blocks,
            total.single_commands,
            total.multi_commands,
            run.read_perf[0].us,
            run.read_perf[0].commands,
            run.read_perf[1].us,
            run.read_perf[1].commands,
            run.read_perf[2].us,
            run.read_perf[2].commands
        );
    }
    app.apply_event(obc_app::HostEvent::NavPlanned(result.map(|(id, _)| id)));
}

/// The [`Gesture`](obc_app::Gesture) variant's name for the drained-input `defmt` breadcrumb
/// (issue #755 field forensics). Lives board-side because `obc-app` stays defmt-free
/// (host-agnostic); `Step`'s count is logged separately at the call site. Ungated — the
/// breadcrumb logs in every build variant.
fn gesture_name(g: obc_app::Gesture) -> &'static str {
    match g {
        obc_app::Gesture::Step(_) => "Step",
        obc_app::Gesture::Press => "Press",
        obc_app::Gesture::Hold => "Hold",
        obc_app::Gesture::Back => "Back",
        obc_app::Gesture::BackHold => "BackHold",
    }
}

/// One pass's sync-render output, carried from the **store phase** (render, under the guard) to
/// the **present phase** (push, guard-free) — the #809 split. `None` = no frame rendered this
/// pass, so nothing to present. `needs_map` picks the RTT log line's shape (map vs. UI frame).
struct RenderedFrame {
    needs_map: bool,
    stats: obc_render::RenderStats,
    render_us: u64,
}

/// Everything this pass's [`App::drain_host_commands`](obc_app::App::drain_host_commands) produced,
/// popped off the caller-owned [`HostMailbox`](obc_app::HostMailbox) **synchronously** into
/// board-local storage before the pass's first `.await` (FAR-19, #812). The mailbox itself — a
/// ~600 B `Deque<HostCommand>` — is a stack temporary that must never enter the ride-loop task
/// future (it would re-inflate the #808 poll frame it took care to shrink); only this small struct
/// of ids/bools/small-enums (~30 B — deliberately **no** `NavRequest`, whose 44 B would dominate:
/// the planner slot is written from it synchronously at the drain instead — see `plan_armed`) is
/// what survives across the pass's awaits (the bulge push, the DFU install, the store lock) to
/// each command's original consumption site.
///
/// The two **derived fill levels** (`LoadRideTrack` / `RefreshNavPreview`) are deliberately *not*
/// staged: they are answered at their fill sites off the pure predicates
/// [`App::ride_track_request`](obc_app::App::ride_track_request) /
/// [`App::nav_preview_missing`](obc_app::App::nav_preview_missing), because a `nav_finish` **this
/// pass** — which runs *after* the drain — can create a fresh `RefreshNavPreview` need that a
/// drain-time snapshot would miss. The mailbox re-emits and coalesces them each drain; the pop
/// loop discards them (the same shape `obc-host-core::HostLoop` uses).
#[derive(Default)]
struct HostPass {
    /// `RescanStore` — re-scan the object store and re-feed the catalogs. BLE-only: the
    /// store-changed edge is raised solely by the BLE plane's commit/delete path.
    rescan: bool,
    /// `CancelRoutePlan` — abort the in-flight route plan.
    cancel_plan: bool,
    /// `DeleteRoute { id }` — the durable id (index-resolved at drain).
    delete_route: Option<u64>,
    /// `DeleteTrip { id }` — cascade-delete the trip and its member routes.
    delete_trip: Option<u64>,
    /// `DeleteRide { id }` — the durable ride id (index-resolved at drain).
    delete_ride: Option<u16>,
    /// `StampRideSynced { id, utc }` — write ride `id`'s `synced_at` to the synced sidecar
    /// (auto-expiry epic #638, S3), the sweep's legacy-ride countdown start. Sidecar write, applied
    /// directly against the still-FAT ride journal.
    stamp_ride: Option<(u16, u32)>,
    /// `FinishTrack(action)` — close the open ride log (Save / Discard).
    finish: Option<obc_app::TrackAction>,
    /// `PlanRoute` was drained — the router should begin a plan this pass. The 44-byte `NavRequest`
    /// itself is **not** staged across the pass's awaits (it would dominate this struct and re-inflate
    /// the task future, #808/#812): the planner's `.bss` slot is written from it synchronously at the
    /// drain (`nav_begin` needs no store lock), and only this flag rides into the store phase, where
    /// `nav_route_begin` opens the reserved file and arms the run. The `ble` image (no router) answers
    /// the failure tier at the drain instead, so it never sets this.
    #[cfg(has_nav)]
    plan_armed: bool,
    /// `Dfu(action)` — most-recent-wins: the `dfu-install` debug post overwrites the drained value
    /// *after* the drain (behaviour-identical to the old slot's most-recent-wins overwrite).
    dfu: Option<obc_app::DfuAction>,
    /// `ForgetBond` — clear the phone bond. BLE-only (there is no bond store without the radio).
    #[cfg(feature = "ble")]
    forget: bool,
    /// `PersistSettings { revision }` — the revision to persist and later acknowledge.
    persist: Option<u16>,
    /// `ScanCardFree` — run the FAT free-cluster scan.
    card_scan: bool,
}

/// The GPS power state the ride wants: deep-sleep when not tracking, full-power fixes while riding, or
/// the M10's low-power tracking when the `power_saver` toggle is on. Recomputed each frame in
/// [`run_app`] and pushed to the sensor task (via [`SensorControl::set_power`]) only on a change.
/// Real-sensor build only — the `synth` / `debug-uart` feeds have no power-managed receiver.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
fn desired_gps_power(app: &App) -> GpsPower {
    if app.activity.is_tracking() {
        if app.settings().power_saver {
            GpsPower::LowPower
        } else {
            GpsPower::Active
        }
    } else {
        GpsPower::Sleep
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
    // The SD card + RRAM settings behind one async mutex (#193, #270). The loop takes it in two
    // short scopes per pass — the store phase (reconcile + sources + the sync render) and the
    // post-present tail (trial confirm + deferred save) — and **never holds it across the present
    // await** (#809), so the BLE object plane reaches the card during the FLPR scan and between
    // passes alike. Replaces the by-value `Storage`/settings store this fn used to own.
    shared: &SharedStoreMutex,
    map_tables: &MapTables,
    map_cache: &MapCache,
    // The flat map source and store are mandatory: a non-flat card never reaches this loop.
    flat_map: &'static dyn obc_formats::io::ByteSource,
    flat: &'static obc_storage::flat::FlatStore<crate::flat_store::FlatCard>,
    route_cache: &RouteCache,
    // The router's resident half (epic #116 R4 + EL7): the map's terrain, threaded from `main`
    // (never a local — the #270/#419 discipline). Its A* table, tile cache and planner slot are the
    // scratch arena's nav arm now (#1146 P2), claimed per search below. On `not(has_nav)` (the `ble`
    // build) this is the unit stand-in — the router isn't in that image.
    #[cfg(has_nav)] nav: NavResident,
    #[cfg(not(has_nav))] nav: NavResident,
    led: &mut Output<'static>,
    // The hardware watchdog's feed handle (#349), `None` only if the boot-time `try_new` found the
    // dog already running with a foreign config. Fed once per pass below, gated on the input
    // plane's heartbeat.
    mut wdt: Option<wdt::WatchdogHandle>,
    // The sensor hub's consumer handle (#808): the `*Source` drains + presence + the event wake.
    // Threaded from `main`'s `static SensorHub` on every build that uses the hub — the real-sensor
    // GPS sources, or the `debug-uart`/`ble` HR/power/cadence sources — so ownership is visible in
    // composition, not reached through a global. Absent only on the pure `synth` build.
    #[cfg(not(all(not(feature = "debug-uart"), feature = "synth")))] consumer: SensorConsumer<'static>,
    // The hub's control handle (#808): the GPS rate + power latches the sensor task awaits. Only the
    // real-sensor build drives a power-managed receiver.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))] control: SensorControl<'static>,
    // The OBCM bbox centre (lon, lat) — only the `SynthLocation` stand-in needs it (the host feed and
    // the real GPS both stream absolute positions). So it's threaded only on the `synth` build.
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))] cam_center: (i32, i32),
) -> ! {
    // Native renderer colour → identity `Rgb565`; `FbDevice64` quantizes to RGB222 on store.
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

    // Sensor sources — three builds, one `Sensors` either way (the app can't tell which):
    // - `debug-uart`: the host-streamed GPS / altimeter / compass, parsed by the VCOM tasks into
    //   obc-platform's debug-link signals; these ZST handles just `try_take` on the ~1 Hz contract.
    // - default (real sensors, #218): the SAM-M10Q + BMP581 task publishes through the hub;
    //   these consumer sources drain its mailboxes. Absolute positions, so no camera re-centre below.
    // - `synth`: the `SynthLocation` square loop (walked from a boot-relative `start`), no baro.
    #[cfg(feature = "debug-uart")]
    let (mut debug_loc, mut debug_alt, mut debug_compass) = (
        obc_platform::debug_link::DebugLocation,
        obc_platform::debug_link::DebugAltimeter,
        obc_platform::debug_link::DebugCompass,
    );
    // The hub sources (`consumer.location()` etc.) are *not* bound here: they're stateless
    // one-pointer drains, so each `app.tick` site below constructs them as call-expression
    // temporaries. `tick` is synchronous, so the temporaries never live across an `await` — binding
    // them for the loop's lifetime would park one hub pointer per source in this task's future
    // (measured in #808: ~40 B of `__embassy_main` arena) for no behavioral difference.
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
    let mut synth = SynthLocation::new(cam_center.0, cam_center.1, Instant::now());
    // Battery: a fixed 75 % stand-in until the nPM1300 PMIC fuel gauge is wired in. Polled in `Sensors`
    // like any other sensor.
    let mut fuel = StubFuelGauge::new(75);
    // WX7's fixed one-tile cache is the board's resident half of weather streaming. The snapshot
    // is host-owned by design (~0.8 KiB) and refreshed only when the selected bundle or sample
    // position changes; neither object is rebuilt per rendered frame.
    let mut weather_cache = obc_weather::WeatherCache::new();
    let mut weather_snapshot: Option<obc_app::WeatherSnapshot> = None;
    let mut weather_sample_key: Option<WeatherSampleKey> = None;
    let mut weather_bundle = crate::flat_store::active_weather(flat).ok().flatten();
    crate::flat_store::reconcile_weather(flat, weather_bundle);

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
    // bounded step per pass. Guards the planner slot's initialization.
    #[cfg(has_nav)]
    let mut nav_run: Option<NavRun> = None;
    // The scratch arena's **nav arm**, held for the whole search (#1146 P2) — many passes, by
    // design: the A* table, the tile cache and the planner all have to survive from one bounded step
    // to the next. This loop is the arena's sole owner-switcher, and the Recalculating freeze is what
    // keeps render claims away while it is held. Taken at the plan drain, given back on the answer
    // and on every cancel/abort path.
    #[cfg(has_nav)]
    let mut nav_guard: Option<crate::arena::NavGuard> = None;
    // The active route's resident chunk-index slot. A bare `RouteIndex` + validity flag, NOT an
    // `Option<RouteIndex>` built by value: the slot is ~12.3 KB and permanently part of this frame
    // either way, but a by-value build (`RouteIndex::read`'s return) also transits the stack at
    // the pass's deepest point — which is what overflowed the 44 KB main stack on the post-upload
    // rescan (STKOF HardFault, 2026-07-12). `build_route_index_into` fills it in place.
    let mut route_index: RouteIndex = RouteIndex::empty();
    let mut route_index_valid = false;
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
    // Terrain samples taken this boot (EL8), purely to throttle the `altfuse:` RTT line to one per
    // 64 fixes. Not state the app reads — the estimator's own counters live on `Activity`.
    #[cfg(has_nav)]
    let mut elev_fixes: u32 = 0;
    // The DFU trial confirm (epic #615 S4, #619) is anchored at "first frame presented AND SD
    // mounted" — precisely the first successful `render_present` below (main mounts the card and
    // faults out *before* this loop can run, so storage being live is already implied here; a
    // boot that can't reach a presented frame never confirms, and S3's rollback fires next boot).
    let mut trial_confirm_pending = true;
    // DR6 (#734): the scan's validated `StagedRef`, parked between the `DfuAction::Scan` that
    // produced it and the confirm's `DfuAction::Install`, so the arm reuses that full read + CRC
    // pass instead of redoing it. `Copy`, ~850 B; it lives in this loop task's future storage, off
    // `arm_update`'s sync stack (see the stack note in `dfu.rs`). A failed re-scan clears it; a stale
    // ref is safe — the bootloader re-verifies post-reboot regardless. `None` ⇒ `run_install` falls
    // back to a fresh scan (an Install with no preceding Scan, e.g. the `dfu-install` debug path).
    let mut cached_staged: Option<obc_dfu::StagedRef> = None;

    // SE7 (#714): the saved-sensor addresses last pushed to the central manager, so the per-pass
    // reconcile below drives a save/forget only on an actual change (the `set_radio_enabled` shape,
    // fired once per change — never re-signalled, so a steady state never interrupts a live link).
    // Starts empty → the first pass seeds the manager from the persisted `Settings.saved_sensors`.
    #[cfg(feature = "ble")]
    let mut pushed_sensors: [Option<([u8; 6], bool)>; obc_app::SENSOR_SLOTS] = [None; obc_app::SENSOR_SLOTS];
    // SE7 (#714): the next-re-arm deadline (loop-millis) for the discovery scan while the scan list is
    // up — `0` = not scanning (rings `request_scan` on the rising edge, then re-arms just under the
    // board's ~10 s window), so the scan stays live without pulsing the manager's work edge every pass.
    #[cfg(feature = "ble")]
    let mut sensor_scan_rearm_ms: u32 = 0;

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
    control.set_rate(prev_interval);

    // Drive the GPS power state: the sensor task acquires one boot fix regardless, then honours this —
    // Sleep while idle, Active/LowPower once a ride starts. Pushed once at boot, then again whenever
    // tracking or the `power_saver` toggle changes.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    let mut prev_power = desired_gps_power(app);
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    control.set_power(prev_power);

    // Whether the map-transfer card (issue #927) was **observed** on the stack last pass — the latch
    // that turns "the card is gone" into "the rider dismissed it". See the reconcile below.
    #[cfg(feature = "ble")]
    let mut map_card_shown = false;
    // Map-upload pacing (#889, the WDT-reset episode): while bytes are landing, the card is the
    // only thing on glass and every repaint is ~85 ms of render+push stolen from the SD write
    // path — so `Receiving` progress is fed to the app at most once per this interval, and the
    // loop's own timer is clamped up to it. Wakes still happen (gestures, sensors, the WDT feed
    // cap), they just find an unchanged card and repaint nothing. Terminal states bypass the
    // throttle: `Installed`/`Failed` must land on glass the pass they happen.
    const MAP_XFER_PACE_MS: u32 = 2_000;
    let mut map_uploading = false;
    let mut map_xfer_fed_ms: u32 = 0;

    loop {
        let now = Instant::now().as_millis() as u32;
        let hw = stackmeter::used(now);
        if hw > stack_hw {
            stack_hw = hw;
            // Surface the peak in the diagnostics blob for the A9 soak rig (#277) — the ride loop owns the
            // stackmeter, so on a `ble` build it publishes the mark into the BLE state the blob reads.
            #[cfg(feature = "ble")]
            crate::link::publish_stack_high_water(hw);
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
                defmt::info!("input: {=str} dropped (stack changed mid-hold)", gesture_name(g));
                continue;
            }
            // Field forensics (#755): every drained gesture, with the screen it lands on — the
            // RTT record that discriminates "the press never happened" (input-plane dead window)
            // from "the press landed on the wrong screen/row" (e.g. a press-nudged step turned
            // a 2-row confirm to Cancel first). Human-rate events; always on, a handful of bytes.
            if let obc_app::Gesture::Step(n) = g {
                defmt::info!("input: Step {=i32} on {=str}", n, app.top_screen().name());
            } else {
                defmt::info!("input: {=str} on {=str}", gesture_name(g), app.top_screen().name());
            }
            // Weather's manual refresh is an **entry edge**, not a periodic poll and not a draw
            // side effect.  WX8 already owns the request scheduler/radio plumbing; what was
            // missing was the one board seam that tells it the rider actually opened the WX11
            // dashboard.  Remember whether this gesture started on a weather surface so Back
            // from Hourly/Rain map does not manufacture another urgent request.
            #[cfg(feature = "ble")]
            let was_on_weather = matches!(
                app.top_screen(),
                obc_app::Screen::Weather(_) | obc_app::Screen::WeatherHourly(_) | obc_app::Screen::WeatherRainMap(_)
            );
            app.apply_gesture(g);
            #[cfg(feature = "ble")]
            if !was_on_weather && matches!(app.top_screen(), obc_app::Screen::Weather(_)) {
                crate::ble::request_weather_now();
                defmt::info!("weather: dashboard opened — urgent phone fetch requested");
            }
            holds_cancelled |= app.take_hold_cancel();
        }
        if holds_cancelled {
            display.cancel_holds();
        }
        app.advance_animations(InputClock(now));

        // ── Sensor presence → warning (issue #504), real-sensor build, once ──
        // The sensor task publishes its boot I²C probe result a moment after boot; map any chip that
        // didn't answer to a dismissable warning card. `try_take` yields once, so this fires a single
        // pass; `Warning(NONE)` (all present) is a no-op.
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        if let Some(p) = consumer.take_presence() {
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
            app.apply_event(obc_app::HostEvent::Warning(w));
        }

        // ── BLE → app seam (epic #447), POSTING half: everything that *posts* an app command or
        // event runs before the typed drain below, so the drain sees it this pass ──
        // Feed the link snapshot (connected + passkey) — a couple of atomic reads distilled to the
        // app's own `BleStatus`; `set_ble_status` compares against the last and dirties nothing on the
        // steady state. Then drain the object-store movement edge and apply a `StoreChanged` event per
        // commit/delete (the same edge that notifies the phone). Both `ble`-only: the map build has no
        // radio and no `object_store`, so the app simply stays disconnected there.
        for _ in 0..crate::flat_store::take_catalog_commits() {
            app.apply_event(obc_app::HostEvent::StoreChanged);
        }
        // The FAT ride repository remains until FS8. Its save/delete/sync edge shares the same app
        // event so the Rides menu still refreshes promptly while route/trip catalogs are flat-only.
        for _ in 0..crate::object_store::take_store_changed() {
            app.apply_event(obc_app::HostEvent::StoreChanged);
        }

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
            crate::link::set_recording(app.activity.is_tracking());
            if crate::object_store::dfu_install_pending() && app.open_remote_dfu_check() {
                let _ = crate::object_store::take_dfu_install_ble();
            }
            // BLE setClock (auto-expiry epic #638 S2, #642): a validated `(utc, offset)` from the phone
            // is waiting to stamp the wall clock. `stamp_clock_ble` sets + persists the offset and marks
            // the clock trusted `Ble`; posting from *this* half means the `PersistSettings` it arms is
            // caught by the typed drain below this same pass, so its save + `DEVICE_SETTINGS_CHANGED`
            // land promptly — a Config read soon after this setClock serves the fresh offset (#456). The
            // home clock jumps as soon as the loop renders (the post_ble_clock wake got us here).
            if let Some((utc, offset_min)) = crate::object_store::take_ble_clock() {
                app.stamp_clock_ble(utc, offset_min);
            }
            // The settings→radio switch (#455): push the persisted Bluetooth toggle across the
            // plane boundary — one atomic swap; the radio plane wakes only on a change (off = stop
            // advertising + drop the link; on = the normal lifecycle). Fire-and-forget by design:
            // this loop never blocks on the radio winding down, so no wake source here can go dead
            // with the radio off (#438's lesson).
            crate::ble::set_radio_enabled(app.settings().ble_enabled);
            // The weather due plane's inputs (WX8, #1193): the app-side half of the §11.4 request
            // context — ride state, fresh fix + its UTC, bearing/speed, active route id, trusted
            // "now". One small `Cell` store per pass; the scheduler task wakes only on the edges
            // it keys on (ride state, route), never at the fix cadence.
            crate::ble::set_weather_inputs(app.weather_snapshot());
        }

        // ── Map-transfer card (issue #927): the on-glass half of a write that runs for minutes ──
        // The USB data plane publishes the transfer state into `link`'s atomics; this is the one task
        // allowed to touch the `App`, so it reads them once per pass and reconciles the card. Fed an
        // unchanged state the reconcile repaints nothing.
        //
        // Dismissal has to be observed rather than signalled: the terminal card pops itself on a
        // press, and without this the next pass would simply push it back. `card_shown` latches the
        // *observed* card (never the intent, so a push deferred mid-hold isn't mistaken for a
        // dismissal), and a card that was up and no longer is means the rider closed it — so the
        // published state is cleared instead of re-fed.
        #[cfg(feature = "ble")]
        {
            if map_card_shown && !app.map_transfer_card_up() {
                crate::link::clear_map_transfer();
                map_card_shown = false;
                map_uploading = false;
            } else {
                let state = crate::link::map_transfer_state();
                let receiving = state.is_some_and(|s| s.is_receiving());
                // The throttle (see MAP_XFER_PACE_MS): a Receiving→Receiving pass inside the pace
                // window skips the feed, so a sensor-paced wake doesn't turn a progress tick into
                // a full repaint. Any transition — into, out of, or the first Receiving — feeds.
                if !(receiving && map_uploading && now.wrapping_sub(map_xfer_fed_ms) < MAP_XFER_PACE_MS) {
                    app.set_map_transfer(state);
                    map_card_shown = app.map_transfer_card_up();
                    map_xfer_fed_ms = now;
                }
                map_uploading = receiving && map_card_shown;
            }
        }

        // ── Typed host-command drain (FAR-19, #812): the pass's single `drain_host_commands`, run
        // unconditionally here so it captures every gesture-posted command (applied above) plus the
        // BLE posting half's `open_remote_dfu_check` Scan and store-changed edge — and precedes
        // *every* consumer (the BLE consuming half's Forget below, the DFU match, and the whole
        // store phase). The mailbox is a stack temporary scoped to this block and dropped at its
        // close, before the pass's first `.await`, so the ~600 B `Deque` never enters the task
        // future (only the small `HostPass` survives across the awaits). The two derived fill levels
        // are popped and discarded — answered at their fill sites off pure predicates (see
        // `HostPass`). ──
        let mut host_pass = HostPass::default();
        {
            let mut mailbox: obc_app::HostMailbox = obc_app::HostMailbox::new();
            let _ = app.drain_host_commands(&mut mailbox);
            while let Some(cmd) = mailbox.pop() {
                match cmd {
                    obc_app::HostCommand::RescanStore { .. } => host_pass.rescan = true,
                    obc_app::HostCommand::CancelRoutePlan => host_pass.cancel_plan = true,
                    obc_app::HostCommand::DeleteRoute { id } => host_pass.delete_route = Some(id),
                    obc_app::HostCommand::DeleteTrip { id } => host_pass.delete_trip = Some(id),
                    obc_app::HostCommand::DeleteRide { id } => host_pass.delete_ride = u16::try_from(id).ok(),
                    // Flat route retention deliberately stays inert until #1398 supplies its
                    // ObjectId-keyed metadata kind; there is no FAT sidecar to stamp after FS7.
                    obc_app::HostCommand::StampRouteUsed { .. } => {}
                    obc_app::HostCommand::StampRideSynced { id, utc } => {
                        host_pass.stamp_ride = u16::try_from(id).ok().map(|id| (id, utc));
                    }
                    obc_app::HostCommand::FinishTrack(action) => host_pass.finish = Some(action),
                    obc_app::HostCommand::PlanRoute(_req) => {
                        // Write the planner slot from the request **now** (synchronously, no store
                        // lock needed) so the 44-byte `NavRequest` never rides into the store phase
                        // across the pass's awaits (#808/#812) — only the flag does; the store phase
                        // opens the reserved file and arms the run. The `ble` image ships without the
                        // router, so it answers the failure tier here instead of arming.
                        //
                        // Since #1146 P2 the slot lives in the scratch arena, so the search must
                        // *take* the arena first — and the gate's search arm before it, because a
                        // cable transfer streaming into the same store outranks a reroute. Either
                        // refusal answers the app immediately (the polite failure path): a plan whose
                        // spinner never resolves would now also hold the Recalculating freeze, i.e. a
                        // map that never redraws again.
                        #[cfg(has_nav)]
                        match nav_take_arena(app, &mut nav_guard) {
                            Ok(()) => {
                                let mut bufs = NavBuffers {
                                    guard: nav_guard.as_mut().expect("nav_take_arena left the guard held"),
                                    elev: &mut *nav.elev,
                                };
                                nav_begin(&mut bufs, &_req, app.settings().bike_profile_idx);
                                host_pass.plan_armed = true;
                            }
                            Err(why) => {
                                defmt::warn!("nav: cannot start a plan ({=str}) — answering the failure tier", why);
                                app.apply_event(obc_app::HostEvent::NavPlanned(Err(obc_route::NavError::NoPath)));
                            }
                        }
                        #[cfg(not(has_nav))]
                        {
                            defmt::warn!(
                                "nav: router not built into the ble image (256K DK) — answering the failure tier"
                            );
                            app.apply_event(obc_app::HostEvent::NavPlanned(Err(obc_route::NavError::NoPath)));
                        }
                    }
                    // ── The detour family (#882) — answered, not swallowed ──
                    // These three used to fall into the `_ => {}` arm below, and that was a real bug
                    // rather than a gap: the Detour chooser is reachable from the ride menu, so a
                    // rider who confirmed one got a spinner that ran until they pressed Back. Since
                    // #1146 P2 it is worse than a stuck spinner — draining a plan command engages the
                    // Recalculating freeze, so an unanswered detour would freeze the map for the rest
                    // of the ride.
                    //
                    // The board has no detour half yet, and this is not the PR to invent one: #882's
                    // flow holds the planned detour's OBCR **in RAM** from `DetourPlanned` until the
                    // rider commits, then stream-splices `original[0..anchor] + detour +
                    // original[rejoin..]` into a derived route. The host does both with a `Vec`; the
                    // board has one flat-store route reservation and no heap, so the
                    // splice has nowhere to read the detour from while it writes. That is a storage
                    // design, with its own on-glass acceptance — a separate issue.
                    //
                    // So: answer the typed failure the moment the request drains, exactly as the
                    // router-less image answers `PlanRoute`. `on_detour_planned` swaps the spinner for
                    // the "Try a farther rejoin." card and releases the freeze in the same pass.
                    obc_app::HostCommand::PlanDetour(_) => {
                        defmt::warn!("nav: detour planning has no board half yet (#882) — answering the failure tier");
                        app.apply_event(obc_app::HostEvent::DetourPlanned(Err(obc_route::NavError::NoPath)));
                    }
                    // Unreachable while the plan above always fails (no preview screen ⇒ nothing to
                    // commit), and wired anyway: the day the plan half lands, a commit that fell into
                    // `_ => {}` would strand the preview's "Applying..." exactly as the spinner was
                    // stranded. The old route is untouched either way.
                    obc_app::HostCommand::CommitDetour => {
                        defmt::warn!("nav: detour commit has no board half yet (#882) — answering the failure tier");
                        app.apply_event(obc_app::HostEvent::DetourCommitted(Err(obc_route::NavError::NoPath)));
                    }
                    // Back on the detour planning or preview screen. Nothing board-side is in flight
                    // to cancel (the plan is answered at its drain), and the app has already dropped
                    // its preview and released the freeze — but the arm exists so the command is
                    // *consumed* here rather than silently, which is the whole lesson of the three
                    // above.
                    obc_app::HostCommand::CancelDetour => {}
                    obc_app::HostCommand::Dfu(action) => host_pass.dfu = Some(action),
                    #[cfg(feature = "ble")]
                    obc_app::HostCommand::ForgetBond => host_pass.forget = true,
                    obc_app::HostCommand::PersistSettings { revision } => host_pass.persist = Some(revision),
                    obc_app::HostCommand::ScanCardFree => host_pass.card_scan = true,
                    // The derived fill levels (`LoadRideTrack` / `RefreshNavPreview`) are answered at
                    // their fill sites off pure predicates, not staged; in the non-ble image
                    // `RescanStore`/`ForgetBond` are never posted and their fields don't exist, so
                    // they fall here too.
                    _ => {}
                }
            }
        }

        // ── BLE → app seam, CONSUMING half: acts on the drained `HostPass` + the sensor seam ──
        #[cfg(feature = "ble")]
        {
            // Drain the Bluetooth screen's Forget-phone hold and ring the bond clear (clear the RRAM
            // bond slot + drop the bonded connection).
            if host_pass.forget {
                crate::ble::request_forget_bond();
            }

            // ── The BLE sensor seam (epic #707, SE7) ──
            // Scan mode: while the Sensors screen's scan list is up, keep a discovery scan running and
            // feed the hits back; clear the app list when it closes. `request_scan` **must not** be
            // rung every pass — it pulses the manager's `WORK_EDGE`, which the manager's own scan
            // window selects on, so a per-pass ring would collapse the ~10 s window (and re-clear the
            // hit snapshot) every ~40 ms. Instead ring it once on the rising edge, then re-arm every
            // ~9 s (just under the board's 10 s window) so a lingering scan stays live without thrash.
            // This block runs **before** the saved-sensor reconcile below: the falling edge's
            // `cancel_scan` must clear a stale latched request *before* the reconcile's save request
            // wakes the manager, or the manager could slip in between and run the stale scan anyway.
            if app.sensor_scan_active() {
                // `now >= rearm` in wrapping-monotonic terms (the signed diff handles the ~49-day u32
                // wrap); `0` is the "not scanning yet" sentinel that fires on the rising edge.
                let due = sensor_scan_rearm_ms == 0 || now.wrapping_sub(sensor_scan_rearm_ms) as i32 >= 0;
                if due {
                    crate::ble::request_scan();
                    sensor_scan_rearm_ms = now.wrapping_add(9_000).max(1); // never 0 (the "off" sentinel)
                }
                let mut hits: heapless::Vec<obc_app::SensorScanHit, { obc_app::sensors::SCAN_HITS_MAX }> =
                    heapless::Vec::new();
                crate::ble::sensor_scan_hits(|found| {
                    for h in found {
                        let _ = hits.push(obc_app::SensorScanHit::new(
                            sensor_kind_slot(h.kind),
                            h.random as u8,
                            h.addr,
                            h.name.as_str(),
                            h.rssi,
                        ));
                    }
                });
                app.set_sensor_scan_hits(&hits);
            } else {
                // Falling edge: the scan list closed (a pick or a Back). Cancel discovery — the
                // re-arm may have left a *stale* scan request latched, which would outrank the
                // fresh save at the manager's loop top and hold the connect hostage for a full
                // 10 s window (epic #744, SR4); the cancel also ends a still-running window early
                // so a picked sensor connects now, not when the window expires.
                if sensor_scan_rearm_ms != 0 {
                    crate::ble::cancel_scan();
                }
                sensor_scan_rearm_ms = 0; // reset so the next entry rings on its rising edge
                app.set_sensor_scan_hits(&[]);
            }
            // Saved-sensor reconcile: the persisted `Settings.saved_sensors` is the source of truth
            // (the SE6 `SEED` hook is gone). Diff each slot against what was last pushed and drive the
            // change through SE6's save/forget latches — fired once per change (seed at boot from
            // all-`None`, a screen pair/forget, a factory reset clearing a slot). The board-side latch
            // pulse (`WORK_EDGE`) makes the manager connect/drop at once.
            for (q, slot) in app.settings().saved_sensors.iter().enumerate() {
                let want = slot.present.then_some((slot.addr, slot.addr_kind != 0));
                if want != pushed_sensors[q] {
                    match want {
                        Some((addr, random)) => crate::ble::request_save_sensor(q, addr, random),
                        None => crate::ble::request_forget_sensor(q),
                    }
                    pushed_sensors[q] = want;
                }
            }
            // Push the per-slot status snapshot (the Sensors screen's row status lines).
            let sensor_status = [sensor_status_of(0), sensor_status_of(1), sensor_status_of(2)];
            app.set_sensor_status(&sensor_status);
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

        // ── DFU install / scan requests (epic #615 S4/S5), acted on BEFORE the store phase ──
        // The `dfu-install` debug command reaches the *same execution path* the S5 update screen's
        // drained `Dfu` command does; staging it straight into `host_pass.dfu` here — **after** the
        // typed drain above — is behaviour-identical to the old most-recent-wins slot overwrite (it
        // supersedes any `Scan` the drain placed there). This block sits outside the store phase
        // (#809) so the "Installing update" card's present runs guard-free: the store is locked
        // briefly for the go/no-go checks, released across the card's render + present, then
        // re-locked for the arm itself — which then holds it exclusively across its whole SD→flash
        // stream, deliberately (a BLE `UPDATE.BIN` write must not interleave with the arm).
        #[cfg(feature = "debug-uart")]
        if obc_platform::debug_link::take_dfu_install() {
            host_pass.dfu = Some(obc_app::DfuAction::Install);
        }
        // Two phases share the drained `Dfu` command (epic #615 S5, #620): the S5 UI posts `Scan`
        // first (read-only validation → answer the app, which shows the confirm screen), then
        // `Install` from the confirm; the `dfu-install` debug command posts `Install` directly (no
        // confirm).
        match host_pass.dfu {
            Some(obc_app::DfuAction::Install) => {
                // The irreversible arm-and-reboot. Guards mirror what the System menu greys out:
                // never mid-recording (the arm ends in a reboot — a live ride would be lost) and
                // never over an unconverted ride save (`pending_save` is RAM state; rebooting drops
                // it and the next fresh ride would truncate the unconverted TRACK.OBT). On success
                // `run_install` never returns (it resets into the bootloader); on any non-reboot
                // outcome — a refusal here or an arm failure inside `run_install` — we get the typed
                // reason and land the error card (issue #755) so the confirm's "Preparing update..."
                // spinner can't strand the rider. The `D`-line breadcrumbs name the guard that
                // refused (the field-debugging motivation).
                let refusal = {
                    // First short store guard: just the go/no-go checks.
                    let store_guard = shared.lock().await;
                    if app.activity.is_tracking() {
                        crate::dfu::status("refused (is_tracking): a ride is recording -- finish it first");
                        Some(obc_app::DfuInstallError::Recording)
                    } else if store_guard.storage.as_ref().is_some_and(sd::Storage::has_pending_save) {
                        crate::dfu::status(
                            "refused (has_pending_save): a ride save is pending -- try again in a moment",
                        );
                        Some(obc_app::DfuInstallError::PendingSave)
                    } else if store_guard.storage.is_none() {
                        crate::dfu::status("refused (no_card): no SD card");
                        Some(obc_app::DfuInstallError::NoCard)
                    } else {
                        None
                    }
                };
                let outcome = if refusal.is_some() {
                    refusal
                } else {
                    // The guards passed — the arm is happening. Swap the confirm's "Preparing
                    // update..." spinner for the static "Installing update" card and put that
                    // frame on glass NOW, fully awaited: the arm ends in a warm reset into the
                    // bootloader, which never paints — it only parks the panel pins and keeps
                    // the COM wave alternating (`obc-boot/src/com.rs`) — so the MIP panel holds
                    // THIS frame for the whole snapshot + flash. Presented full-frame (no clip,
                    // no bulge exclusion: a live hold mid-confirm is gone after the reset anyway)
                    // and with the store guard released (#809 — the card render reads no storage).
                    // A failed present (stalled FLPR) deliberately doesn't guard the arm — the
                    // install matters more than the frame, and the arm failure path repaints
                    // normally via the `DfuInstallFailed` event.
                    app.apply_event(obc_app::HostEvent::DfuInstallBegan);
                    app.set_render_clip(None);
                    // `DfuInstalling` is a chrome card: it draws no map, so it needs no render
                    // scratch and never goes near the arena (#1146 P2).
                    display.render_frame(|f: &mut crate::ls021_flpr::Frame64| {
                        let mut fbdev = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
                        app.render_map_timed(
                            None,
                            &mut fbdev,
                            None,
                            None,
                            FRAME_W as f32,
                            FRAME_H as f32,
                            color_fn,
                            &InstantClock,
                        )
                    });
                    let _ = display.present_frame(None).await;
                    // Re-take the store for the arm (held exclusively across the whole install
                    // stream). The check→present→arm window this opens is benign: recording /
                    // pending-save state is ride-loop-owned (neither can appear meanwhile), a
                    // yanked card re-refuses below, and a BLE `UPDATE.BIN` rewrite in the window
                    // is the DR6 stale-ref case — the bootloader re-verifies the staged image
                    // after the reset regardless.
                    let mut store_guard = shared.lock().await;
                    let SharedStore { storage, settings: settings_store } = &mut *store_guard;
                    match storage.as_mut() {
                        // DR6 (#734): hand the confirm's carried scan ref to the arm (consumed
                        // either way). Absent ⇒ `run_install` re-scans (the `dfu-install` path).
                        Some(s) => crate::dfu::run_install(s, settings_store, &mut wdt, cached_staged.take()).await,
                        _ => {
                            crate::dfu::status("refused (no_card): no SD card");
                            Some(obc_app::DfuInstallError::NoCard)
                        }
                    }
                };
                if let Some(reason) = outcome {
                    app.apply_event(obc_app::HostEvent::DfuInstallFailed(reason));
                }
            }
            Some(obc_app::DfuAction::Scan) => {
                // The UI's read-only "Checking card..." step: validate `UPDATE.BIN` and answer the
                // app (the wait screen swaps to the confirm or an error card). No card ⇒ report the
                // update file as missing. The scan touches nothing, so no ride-state guard is needed
                // (the menu greys the row mid-ride anyway). One short store guard of its own.
                let result = {
                    let mut store_guard = shared.lock().await;
                    let SharedStore { storage, settings: settings_store } = &mut *store_guard;
                    match storage.as_mut() {
                        Some(s) => crate::dfu::run_scan(s, settings_store, &mut wdt),
                        None => Err(obc_app::DfuScanError::NotFound),
                    }
                };
                // DR6 (#734): park the validated ref for the confirm's Install; answer the app with
                // just the report. A failed scan clears any prior ref (the card may have changed).
                let report = match result {
                    Ok((report, staged)) => {
                        cached_staged = Some(staged);
                        Ok(report)
                    }
                    Err(e) => {
                        cached_staged = None;
                        Err(e)
                    }
                };
                app.apply_event(obc_app::HostEvent::DfuScanned(report));
            }
            None => {}
        }

        // ═══ Store phase (#809): ONE lexical block owns the store guard ═══
        // The settings save, the card reconcile, the per-frame route/track/map sources, and the
        // map *render* that reads them all run under this guard; the block's close is the guard's
        // death — **before** the present phase below, so a BLE object operation waits behind at
        // most the render, never the ~44 ms FLPR scan on top of it (#270 → #809). The phase is
        // render runs while the reader/source borrows of the open SD handles are live, which is
        // exactly what keeps an upload/delete from invalidating a reader mid-render. A route plan
        // may await bounded calls to the independent flat-store writer in this block; neither that
        // task nor the flat map reader borrows this legacy/settings mutex. Destructured into the two
        // names the body uses (`storage`, `settings_store`).
        let (rendered, dirty_map, hold_p, store_held_us) = {
            let mut store_guard = shared.lock().await;
            let t_store = Instant::now();
            let SharedStore { storage, settings: settings_store } = &mut *store_guard;

            // ── Live catalogs, on the store-changed edge only ──
            // Rebuild flat route/trip identities after any catalog commit and remap the app's held
            // indices by durable ObjectId. Dropping the active flat hold first ensures a replacement
            // at the same id is decoded from its new revision. The legacy edge also covers FAT rides,
            // which are rescanned below until their flat slice lands.
            if host_pass.rescan {
                // Drop the held revision before rebuilding identity/index state. A replace at
                // the same ObjectId must reopen the new revision, not keep rendering the hold.
                crate::flat_store::reconcile_route(flat, None);
                crate::flat_store::load_routes(flat, app);
                crate::flat_store::load_trips(flat, app);
                if let Ok(next) = crate::flat_store::active_weather(flat) {
                    if next != weather_bundle {
                        weather_bundle = next;
                        weather_sample_key = None;
                    }
                }
                crate::flat_store::reconcile_weather(flat, weather_bundle);
                if let Some(s) = storage.as_mut() {
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
            // `ObjectStore` lives behind the BLE task's `RefCell`, so post the id to that plane. It
            // owns the coherent catalog revision, notification, and rescan path.
            if let Some(id) = host_pass.delete_route {
                // A full channel DROPS the id (not observed backpressure — the app's dispatch
                // bookkeeping already ran): warn, and rely on the app's retain-until-rescan
                // candidate to re-dispatch it after the bounded backoff (finding #876-3).
                if !crate::flat_store::request_route_delete(id) {
                    defmt::warn!(
                        "ride: route-delete channel full — id {} dropped; the app's retained candidate retries",
                        id
                    );
                }
            }

            // ── On-device trip cascade delete (epic #526, TR3/TR4), from the folder long-press confirm ──
            // The Route menu's long-press → confirm recorded the trip's durable object id; the cascade
            // deletes the trip AND every member route (locked: post-trip cleanup). Same seam shape as the
            // route delete above: `ble` builds post to the BLE plane (`request_trip_cascade` →
            // `ObjectStore::delete_trip_cascade`, so both store revisions + both `storeChanged` edges
            // move coherently and the rescan returns on the STORE_CHANGED edge).
            if let Some(id) = host_pass.delete_trip {
                if !crate::flat_store::request_trip_cascade(id) {
                    defmt::warn!("ride: trip-delete queue full — object {=u64} retained for retry", id);
                }
            }

            // The System settings screen's card-free scan (T8 item 6): a drained on-entry request runs
            // one bounded FAT free-cluster read off the card and answers through the `CardScanned`
            // event (or a `None` → the screen keeps `--` when there's no card / no FSInfo free count).
            if host_pass.card_scan {
                app.apply_event(obc_app::HostEvent::CardScanned {
                    free_bytes: storage.as_ref().and_then(|s| s.card_free_bytes()),
                });
            }

            // ── On-device ride delete (epic #447, P7 / #454), on the Rides-menu hold-to-delete edge ──
            // The same seam as the route delete, in the ride namespace: the app resolves the highlighted
            // ride's durable object id. On `ble`, post it to the BLE plane (it owns the `ObjectStore`
            // `RefCell`) so the delete goes through the store — revision bump + `storeChanged`, coherent
            // with a phone-initiated delete; the rescan returns on the resulting store-changed edge above.
            // The greying while recording already keeps the delete legal (no open TRACK.OBT / pending
            // save collides).
            if let Some(id) = host_pass.delete_ride {
                // Same contract as the route delete above: a full channel drops the id — warn and
                // rely on the app's retained candidate to re-dispatch after the backoff.
                #[cfg(feature = "ble")]
                if !crate::object_store::request_ride_delete(id) {
                    defmt::warn!(
                        "ride: ride-delete channel full — id {} dropped; the app's retained candidate retries",
                        id
                    );
                }
            }

            // ── Auto-expiry sidecar stamps (epic #638, S3), on the sweep / activation edge ──
            // `last_used` (routes) / `synced_at` (rides) are **device-local** sidecar writes — no
            // store revision, no phone `storeChanged` — so unlike a delete they're applied directly
            // here under the store lock in both builds (the ride loop already holds the card this
            // phase), rather than routed through the BLE plane's `ObjectStore`. The app already
            // mirrored the value into its resident meta, so no re-feed is needed.
            if let Some((id, utc)) = host_pass.stamp_ride {
                if let Some(s) = storage.as_mut() {
                    s.stamp_ride_synced_at(id, utc);
                }
            }

            // ── The Ride detail's track-profile fill (epic #678 T2 / #680), on the detail-entry edge ──
            // An open detail wants its recorded track profiled for the elevation band: stream the
            // `RD{id}.ORD` once into the app's resident buffer, in this pass, under the store lock —
            // sequential with (never under) the render below, and a no-op on every other pass.
            fill_ride_profile(storage, app);

            // ── The resumable route planner (#499), one bounded step per pass ──
            // A drained create-route request allocates unpublished flat-store space, (re)writes the `.bss` planner
            // slot, and arms a `NavRun`; each subsequent pass runs **one** `nav_step` at this shallow
            // depth and then continues the normal pass (render, input, the pass-top watchdog feed) —
            // the UI stays live while the route computes. A drained cancel (Back on the planning
            // screen) aborts: cancel the reservation, answer nothing. On a terminal step,
            // `nav_finish` hashes/publishes (or cancels) it, rescans the catalog (sequential,
            // never nested — the #496 de-nesting), emits the per-phase RTT line, and answers the app;
            // the positional state is then forced to re-derive, exactly like the store-changed rescan
            // above (the plan publishes a new object and may change the active geometry source).
            //
            // `not(has_nav)` (the `ble` build, whose image ships without the router — see build.rs):
            // the request is still drained and answered with the generic failure tier ("Couldn't find
            // a route."), so the POI confirm never hangs. The LM20 deletes that arm.
            let nav_cancel = host_pass.cancel_plan;
            #[cfg(has_nav)]
            {
                // Whether this pass ended the search — the one place the arena's nav arm and the
                // gate's search arm are given back. A flag rather than an inline release because the
                // guard is borrowed by the step view below and must die first.
                let mut search_ended = false;
                if host_pass.plan_armed {
                    // The planner slot was already written from the request at the pass-top drain
                    // (`nav_begin`, no store lock) — which is also where the arena's nav arm was
                    // claimed; here we allocate the output object and arm the run against it. A
                    // request while a plan is somehow still in flight replaces it (can't happen
                    // through the UI — the planning screen blocks a second confirm — but stay safe;
                    // the drain already overwrote the slot for the new plan and kept the same guard).
                    if nav_run.is_some() {
                        // The UI cannot issue a second plan while its planning screen is active.
                        // Keep the impossible case fail-closed instead of lending one reply slot to
                        // two live tickets after the drain has already replaced the planner.
                        debug_assert!(false, "a second route plan arrived while one was active");
                        if let Some(run) = nav_run.as_mut() {
                            run.cancel_requested = true;
                        }
                    } else {
                        nav_run = Some(NavRun {
                            allocation: None,
                            io: NavIo::NeedAllocate,
                            cancel_requested: false,
                            io_started: Instant::now(),
                            t0: Instant::now(),
                            phase_us: [0; 3],
                            write_us: 0,
                            #[cfg(feature = "sd-bench")]
                            read_perf: [crate::card_io::ReadPerf::ZERO; 3],
                        });
                    }
                }
                // `&& !plan_armed` because the canonical drain order puts a cancel *before* the plan
                // behind it: a pass carrying both is cancelling the **old** plan — which the arm above
                // already closed — while arming a fresh one whose file and arena guard must survive.
                // Without the guard the cancel would close the new run's file and hand back the arena
                // the new search is about to step on.
                if nav_cancel && !host_pass.plan_armed {
                    if let Some(run) = nav_run.as_mut() {
                        run.cancel_requested = true;
                    } else {
                        search_ended = true;
                    }
                }
                if let (Some(mut run), Some(writer), Some(guard)) =
                    (nav_run.take(), crate::flat_store::writer(), nav_guard.as_mut())
                {
                    let mut finished = None;
                    let mut cancelled = false;
                    match run.io {
                        NavIo::NeedAllocate => {
                            if run.cancel_requested {
                                cancelled = true;
                            } else if let Ok(ticket) = writer.try_call(
                                crate::flat_store::Request::Allocate { bytes: NAV_ROUTE_RESERVE },
                                &NAV_STORE_REPLY,
                            ) {
                                run.io_started = Instant::now();
                                run.io = NavIo::Allocating(ticket);
                            }
                        }
                        NavIo::Allocating(ticket) => {
                            if let Some(answer) = writer.try_result(ticket, &NAV_STORE_REPLY) {
                                run.write_us += run.io_started.elapsed().as_micros();
                                match answer {
                                    Ok(crate::flat_store::Outcome::Allocated(allocation)) => {
                                        run.allocation = Some(allocation);
                                        run.io = if run.cancel_requested {
                                            NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath))
                                        } else {
                                            NavIo::Ready
                                        };
                                    }
                                    _ => finished = Some(Err(obc_route::NavError::NoPath)),
                                }
                            }
                        }
                        NavIo::Ready => {
                            if run.cancel_requested {
                                run.io = NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath));
                            } else {
                                let map_src = flat_map;
                                let mut bufs = NavBuffers { guard, elev: &mut *nav.elev };
                                // The step's view over the arena arm + the resident terrain, alive only for
                                // the length of these synchronous calls.
                                // The run is active ⇒ the slot was written for this plan; a `None` here
                                // would mean the bookkeeping and the arm disagree, and the step below
                                // answers the failure tier for the same reason.
                                let phase = bufs.guard.planner_ref().map_or(obc_route::NavPhase::Snap, |p| p.phase());
                                let phase_idx = match phase {
                                    obc_route::NavPhase::Snap => 0,
                                    obc_route::NavPhase::Search => 1,
                                    obc_route::NavPhase::Emit | obc_route::NavPhase::Done => 2,
                                };
                                #[cfg(feature = "sd-bench")]
                                let reads_before = crate::card_io::read_perf_snapshot();
                                let ts = Instant::now();
                                let step = nav_step(map_src, map_tables, map_cache, &mut bufs);
                                let us = ts.elapsed().as_micros();
                                run.phase_us[phase_idx] += us;
                                #[cfg(feature = "sd-bench")]
                                run.read_perf[phase_idx]
                                    .add_assign(crate::card_io::read_perf_snapshot().since(reads_before));
                                run.io = if step.appended == 0 && step.patch_len == 0 {
                                    if matches!(step.outcome, obc_route::Step::Running) {
                                        NavIo::Ready
                                    } else {
                                        NavIo::NeedFinish(step.outcome)
                                    }
                                } else {
                                    NavIo::Staged(step)
                                };
                            }
                        }
                        NavIo::Staged(step) => {
                            if run.cancel_requested {
                                run.io = NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath));
                            } else if let Some(allocation) = run.allocation {
                                match start_nav_flush(writer, guard, allocation, step) {
                                    Ok(ticket) => {
                                        run.io_started = Instant::now();
                                        run.io = NavIo::Flushing(ticket, step.outcome);
                                    }
                                    Err(step) => run.io = NavIo::Staged(step),
                                }
                            }
                        }
                        NavIo::Flushing(ticket, outcome) => {
                            if let Some(answer) = writer.try_result(ticket, &NAV_STORE_REPLY) {
                                run.write_us += run.io_started.elapsed().as_micros();
                                match answer {
                                    Ok(crate::flat_store::Outcome::Wrote(allocation)) => {
                                        run.allocation = Some(allocation);
                                        run.io = if run.cancel_requested || !matches!(outcome, obc_route::Step::Running)
                                        {
                                            NavIo::NeedFinish(outcome)
                                        } else {
                                            NavIo::Ready
                                        };
                                    }
                                    _ => {
                                        run.io =
                                            NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath));
                                    }
                                }
                            }
                        }
                        NavIo::NeedFinish(outcome) => {
                            if let Some(allocation) = run.allocation {
                                let mut final_outcome = outcome;
                                let mut publishing =
                                    !run.cancel_requested && matches!(outcome, obc_route::Step::Done(_));
                                let request = if publishing {
                                    let name_len = usize::from(guard.output[6]).min(48);
                                    let name = core::str::from_utf8(&guard.output[64..64 + name_len])
                                        .ok()
                                        .and_then(obc_storage::flat::DisplayName::new);
                                    match name {
                                        Some(name) => {
                                            crate::flat_store::Request::PublishComputedRoute { allocation, name }
                                        }
                                        None => {
                                            publishing = false;
                                            final_outcome = obc_route::Step::Failed(obc_route::NavError::NoPath);
                                            crate::flat_store::Request::Cancel { allocation }
                                        }
                                    }
                                } else {
                                    crate::flat_store::Request::Cancel { allocation }
                                };
                                if let Ok(ticket) = writer.try_call(request, &NAV_STORE_REPLY) {
                                    run.io_started = Instant::now();
                                    run.io = NavIo::Finishing { ticket, outcome: final_outcome, publishing };
                                }
                            } else {
                                finished = Some(Err(obc_route::NavError::NoPath));
                            }
                        }
                        NavIo::Finishing { ticket, outcome, publishing } => {
                            if let Some(answer) = writer.try_result(ticket, &NAV_STORE_REPLY) {
                                run.write_us += run.io_started.elapsed().as_micros();
                                if publishing {
                                    match answer {
                                        Ok(crate::flat_store::Outcome::Published(id)) => {
                                            run.allocation = None;
                                            match obc_app::host::nav_publish_disposition(run.cancel_requested, id.0) {
                                                obc_app::host::NavPublishDisposition::Activate(id) => {
                                                    let len = match outcome {
                                                        obc_route::Step::Done(stats) => stats.total_distance_m,
                                                        _ => 0,
                                                    };
                                                    crate::flat_store::load_routes(flat, app);
                                                    let _ = crate::flat_store::reconcile_route(flat, Some(id));
                                                    finished = Some(Ok((id, len)));
                                                }
                                                obc_app::host::NavPublishDisposition::Compensate(id) => {
                                                    run.io =
                                                        NavIo::NeedPublishCompensation(obc_storage::flat::ObjectId(id));
                                                }
                                            }
                                        }
                                        _ => {
                                            run.cancel_requested = false;
                                            run.io =
                                                NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath));
                                        }
                                    }
                                } else if run.cancel_requested {
                                    cancelled = true;
                                } else {
                                    finished = Some(Err(match outcome {
                                        obc_route::Step::Failed(error) => error,
                                        _ => obc_route::NavError::NoPath,
                                    }));
                                }
                            }
                        }
                        NavIo::NeedPublishCompensation(id) => {
                            let request = crate::flat_store::Request::RemoveComputedRoute {
                                id,
                                revision: obc_storage::flat::Revision(1),
                            };
                            if let Ok(ticket) = writer.try_call(request, &NAV_STORE_REPLY) {
                                run.io_started = Instant::now();
                                run.io = NavIo::CompensatingPublish(ticket, id);
                            }
                        }
                        NavIo::CompensatingPublish(ticket, id) => {
                            if let Some(answer) = writer.try_result(ticket, &NAV_STORE_REPLY) {
                                run.write_us += run.io_started.elapsed().as_micros();
                                if matches!(answer, Ok(crate::flat_store::Outcome::Done)) {
                                    cancelled = true;
                                } else {
                                    // Do not acknowledge cancellation while its published route is
                                    // still visible. Retry the exact-revision removal on a later pass.
                                    defmt::warn!(
                                        "nav route: cancellation compensation for object {=u64} failed — retrying",
                                        id.0
                                    );
                                    run.io = NavIo::NeedPublishCompensation(id);
                                }
                            }
                        }
                    }
                    if cancelled {
                        defmt::info!("nav route: cancelled after {=u64} ms", run.t0.elapsed().as_millis());
                        search_ended = true;
                    } else if let Some(result) = finished {
                        let mut bufs = NavBuffers { guard, elev: &mut *nav.elev };
                        nav_finish(app, &mut bufs, run, result, now);
                        search_ended = true;
                        prev_active = None;
                        index_route = None;
                    } else {
                        nav_run = Some(run);
                    }
                } else if nav_run.is_some() {
                    app.apply_event(obc_app::HostEvent::NavPlanned(Err(obc_route::NavError::NoPath)));
                    nav_run = None;
                    search_ended = true;
                }
                if search_ended {
                    // Drop the guard (releasing the arena so the next frame can render the map
                    // again), then the gate's search arm — in that order, because a transfer that
                    // arms the instant the search arm opens must find the arena free.
                    nav_guard = None;
                    crate::link::TRANSFER_ACTIVE.end_search();
                }
            }
            #[cfg(not(has_nav))]
            {
                let _ = nav_cancel; // no plan can be in flight — the cancel is inert here
                let _: &NavResident = &nav; // the unit stand-in — nothing to plan with
                                            // A `PlanRoute` was already answered with the failure tier at the pass-top drain (the
                                            // `ble` image ships no router), so nothing to do here.
            }

            // Settings coherence, phone → device (#456): a BLE Config write persisted units + name to
            // RRAM but the live `App` copy never learned. Reload the BLE-owned fields into it *before*
            // the change-detection save below, so (a) the UI re-captions same-session and (b) the app's
            // `==`-diff save can't clobber the phone's write with its own stale copy. Only units + name
            // are BLE-writable, so the merge is narrow (`adopt_ble_fields`) — a device-only edit pending
            // this frame is untouched. Board-crate flag, drained once per BLE write; a no-op otherwise.
            #[cfg(feature = "ble")]
            if crate::object_store::take_ble_config_written() {
                // Merge only the BLE-owned fields; `merge_ble_settings` preserves any pending device-edit
                // save (its revision is untouched) so neither the phone's write nor the rider's edit is
                // lost (#456 + #810).
                app.merge_ble_settings(&settings_store.load().unwrap_or_default());
            }

            // Persist settings the moment an edited value leaves the settings subtree: one in-place
            // 16-byte RRAM line, skipped when nothing is owed. The write is acknowledged back to the app
            // by revision (#810) — a durable write clears the dirty state; a failed one keeps it retryable
            // (the app re-arms a bounded backoff) and surfaces the advisory warning card.
            if let Some(revision) = host_pass.persist {
                match settings_store.save(app.settings()) {
                    Ok(()) => {
                        app.apply_event(obc_app::HostEvent::SettingsPersisted { revision });
                        // Settings coherence, device → phone (#456): the RRAM blob just moved, so the BLE
                        // config-read cache is stale — flag it so the BLE plane refreshes from RRAM before
                        // its next Config read / advertised-name read. One relaxed store.
                        #[cfg(feature = "ble")]
                        crate::object_store::mark_device_settings_changed();
                        // Push a changed GPS fix interval to the sensor task → it re-VALSETs the M10's rate.
                        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
                        if app.settings().fix_interval_s != prev_interval {
                            prev_interval = app.settings().fix_interval_s;
                            control.set_rate(prev_interval);
                        }
                    }
                    Err(error) => app.apply_event(obc_app::HostEvent::SettingsPersistFailed { revision, error }),
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
                    control.set_power(power);
                }
            }

            // A pending debug `Z` camera-scale command (render benchmark): pin the map to an exact
            // meters-per-pixel and force one redraw, so a host zoom sweep gets exactly one fresh,
            // stage-timed frame per setting instead of stepping the selection's 1.2× steps.
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
                // `NavPlanning` normally mirrors this, but test the host's actual ownership too:
                // Back may already have popped the screen while its cancellation is still queued.
                // A debug N in that window must not become a replacement plan that overwrites the
                // resident planner before the old run has released its allocation.
                if nav_run.is_some() || !app.debug_start_nav(from, to, "Bench") {
                    defmt::warn!("nav plan: ignored repeated debug N while a plan is active");
                }
            }

            let active = app.active_route_index();
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
            // state re-walk. The `FinishTrack` one-shot was drained into `host_pass.finish` at the pass
            // top; reading it here is equivalent to the old peek-then-take (a drained action is consumed
            // exactly once, this pass, only when the reconcile runs).
            let session = app.activity.session();
            if active != prev_active || session != prev_session || host_pass.finish.is_some() {
                let action = host_pass.finish;
                let mut name: heapless::String<64> = heapless::String::new();
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    let _ = name.push_str(&r.name);
                }
                // A Save also writes the durable ride object: snapshot the app's ride totals + wall-clock
                // anchor in the same frame, so the header matches the log's last points.
                let stats = (action == Some(obc_app::TrackAction::Save)).then(|| app.ride_stats());
                let active_id = active.and_then(|i| app.route_ids().get(i).copied());
                crate::flat_store::reconcile_route(flat, active_id);
                // `storage` is `Option` (a card-less `ble` combined build still serves BLE, map idle); the
                // map build always has `Some`. No card ⇒ nothing to reconcile.
                if let Some(s) = storage.as_mut() {
                    s.reconcile_track(action, session, &name, stats.as_ref(), settings_store);
                }
                prev_active = active;
                prev_session = session;
            }

            // Cache the active route's chunk index across frames: rebuild it (the header + full chunk-meta
            // walk off SD) only when the route changes, or retry if a prior build failed on a flaky link.
            // Not gated on rendering — the matcher in `tick` needs the index on every fresh fix.
            if index_route != active {
                route_index_valid = false;
                match active {
                    Some(_) => {
                        // In place into the resident slot — see its declaration; a by-value build here
                        // is the stack-overflow footgun.
                        let id = active.and_then(|i| app.route_ids().get(i).copied());
                        let source = crate::flat_store::reconcile_route(flat, id);
                        if source.is_some_and(|source| route_index.read_into(source).is_ok()) {
                            route_index_valid = true;
                            index_route = active; // cached — no more rebuilds until the route changes
                        } else {
                            // Transient SD glitch: leave the key mismatched so every frame retries, hiding
                            // the route this frame rather than the whole ride.
                            index_route = None;
                            defmt::warn!("flat: route index read failed — retrying next frame");
                        }
                    }
                    None => {
                        index_route = None;
                    }
                }
            }
            // This frame's route reader = the cached index + a fresh geometry source (both cheap, no I/O —
            // the source just wraps the open handle). Geometry streams lazily where it's read: the matcher
            // on a fresh fix, the renderer on a redraw frame.
            let id = active.and_then(|i| app.route_ids().get(i).copied());
            let route_src = crate::flat_store::reconcile_route(flat, id);
            let route = match (route_index_valid.then_some(&route_index), route_src) {
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
                    altimeter: Some(&mut debug_alt),
                    compass: Some(&mut debug_compass),
                    track: track_dyn,
                    fuel: Some(&mut fuel),
                    // Host-injected `H`/`P`/`R` land in the shared hub mailboxes; on a
                    // `ble` + `debug-uart` build a real strap feeds the same ones (last-writer-wins).
                    hr: Some(&mut consumer.hr()),
                    power: Some(&mut consumer.power()),
                    cadence: Some(&mut consumer.cadence()),
                    // No thermometer on this build, and the host feed streams no GPS time yet.
                    ..Sensors::new(&mut debug_loc)
                },
                route.as_ref(),
            );
            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
            app.tick(
                RideClock(now),
                Sensors {
                    altimeter: Some(&mut consumer.altimeter()),
                    temperature: Some(&mut consumer.temperature()),
                    clock: Some(&mut consumer.clock()), // SAM-M10Q UTC → the wall clock (always stamps; #641)
                    compass: Some(&mut consumer.compass()), // ICM-20948 / AK09916 heading while stopped
                    track: track_dyn,
                    fuel: Some(&mut fuel),
                    // On a `ble` build the central manager (SE6) feeds the shared hub
                    // mailboxes; without `ble` there is no radio, so no sensor source — the
                    // `Sensors::new` base already leaves those three `None`.
                    #[cfg(feature = "ble")]
                    hr: Some(&mut consumer.hr()),
                    #[cfg(feature = "ble")]
                    power: Some(&mut consumer.power()),
                    #[cfg(feature = "ble")]
                    cadence: Some(&mut consumer.cadence()),
                    ..Sensors::new(&mut consumer.location())
                },
                route.as_ref(),
            );
            #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
            app.tick(
                RideClock(now),
                // The synthetic loop has no sensors at all — not even a clock source.
                Sensors { track: track_dyn, fuel: Some(&mut fuel), ..Sensors::new(&mut synth) },
                route.as_ref(),
            );

            // The **map-referenced altimeter** (elevation epic #1068, EL8): one terrain sample per
            // fresh fix, feeding the offset estimator that turns the BMP581's weather-drifting
            // relative altitude into a trustworthy absolute one on the Elevation tile. The request
            // is a one-shot armed by `tick`, so this is at most one 512 B tile read per fix — and
            // usually none at all, since consecutive fixes sit in the same tile and the four-slot
            // cache holds it. `nav.elev` is the very same `.bss` source the emit path samples;
            // nothing is borrowed across a planner step, so the shared `&'static mut` is fine here.
            #[cfg(has_nav)]
            if app.sample_terrain(&mut *nav.elev) {
                // The `altfuse:` RTT line (grep it) — the board half of the simulator's Altimeter
                // panel, and the inspection hook #529 was waiting on: `p_ref` is the sea-level-
                // reduced pressure with the ride's own climbing already subtracted out, so its
                // *trend* is weather and nothing else. Throttled to one line per 64 fixes so a long
                // ride doesn't flood the transport.
                elev_fixes = elev_fixes.wrapping_add(1);
                if elev_fixes.is_multiple_of(64) {
                    let a = app.activity.altitude();
                    let baro = app.activity.baro_elevation_m().unwrap_or(f32::NAN);
                    defmt::debug!(
                        "altfuse: raw={=f32} m offset={=f32} m fused={=f32} m p_ref={=f32} hPa acc={=u32} gated={=u32} reseeds={=u16}",
                        baro,
                        a.offset_m().unwrap_or(f32::NAN),
                        a.fused_m(baro).unwrap_or(f32::NAN),
                        a.reference_pressure_hpa(baro).unwrap_or(f32::NAN),
                        a.accepted(),
                        a.gated(),
                        a.reseeds()
                    );
                }
            }

            // Stream the selected OBCW into the host-owned resident snapshot. Keying on the fully
            // validated flat revision plus the live fix keeps this off ordinary redraws while still
            // resampling at movement cadence; a replacement changes the key even if an OBCW producer
            // reuses its generation. With no fix the hourly half remains useful and every rain
            // sample is honestly no-data (the companion likewise refuses to build a *new* local
            // bundle without a device position).
            let weather_pos = app.has_live_fix(now).then(|| app.state.user_fix.map(|fix| (fix.lat, fix.lon))).flatten();
            let weather_projection = app.ride_projection();
            let weather_projection_key = weather_projection
                .map(|projection| (app.active_route_index(), projection.progress_m, projection.speed_cms));
            // A route projection moves with time even while the GPS coordinate/progress is
            // unchanged, so it gets a minute bucket matching the dashboard's timer resolution.
            // Fixed-position sampling has no time input at all: giving it a minute key would
            // reread the same hourly block and rain cells forever while the device is parked (and
            // before the first fix), with byte-identical output.
            let weather_projection_minute =
                weather_projection.map_or(WEATHER_TIME_INDEPENDENT, |_| app.wall_unix_now() as i64 / 60);
            let next_weather_key =
                weather_bundle.map(|bundle| (bundle, weather_pos, weather_projection_key, weather_projection_minute));
            if next_weather_key != weather_sample_key {
                let next_snapshot = weather_bundle.and_then(|bundle| {
                    let source = crate::flat_store::reconcile_weather(flat, Some(bundle))?;
                    let reader = bundle.validated.reader(source).ok()?;
                    let projection = route.as_ref().zip(weather_projection);
                    obc_app::WeatherSnapshot::sample_along(&reader, &mut weather_cache, weather_pos, projection).ok()
                });
                // A transient SD read must retry on the next pass, not pin a failed sample until
                // the next minute bucket. No active candidate is a settled `None` and may key.
                weather_sample_key =
                    if next_weather_key.is_none() || next_snapshot.is_some() { next_weather_key } else { None };
                if next_snapshot != weather_snapshot {
                    weather_snapshot = next_snapshot;
                    if let Some(snapshot) = weather_snapshot.as_ref() {
                        let wall_now = app.wall_unix_now() as i64;
                        let floor = snapshot.rain_zoom_floor(app.state.cam_lat).unwrap_or(0.0);
                        app.set_rain_view(snapshot.steps_ahead(wall_now), floor);
                        app.weather_alert_tick(Some(snapshot));
                    } else {
                        app.set_rain_view(0, 0.0);
                    }
                    app.weather_feed_changed();
                }
            }

            // Feed the high-priority plane's Select hold-progress to the map render so the in-screen
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

            // ── The Recalculating freeze (#1146 P2) ──
            // A planner run is live over a map base: the map plane holds still until it answers, and
            // the reflective panel keeps the last frame on glass for free. Two things follow, and
            // both are load-bearing. The map redraw is **skipped, not queued** — latched into
            // `pending_map_redraw` so nothing is lost and the catch-up lands the pass the freeze
            // lifts (`App::note_plan_ended` dirties the map for exactly that). And the *overlay*
            // still paints: `dirty.overlay` carries the freeze's edge, and the banner is what turns
            // a frozen screen from "the device wedged" into "it is recalculating".
            //
            // That edge is the **engaged level's**, minted inside `App::take_dirty` — not the plan
            // start's. The two differ exactly where it matters: a plan drained under the opaque
            // planning spinner freezes nothing (chrome base, and that frame renders normally), and
            // the pass that puts a map base back under the still-running search is a screen change
            // with no plan edge in it at all. Keyed on the plan's edge, this branch would find
            // `dirty.overlay` already spent on the chrome frame and paint nothing for the rest of
            // the search — a stale screen, no explanation, and input going to the base underneath.
            // Whatever the frame under the banner happens to be (the last map, or the spinner the
            // rider just left), the banner is what says the device is working; the full repaint when
            // the freeze lifts restores the rest.
            //
            // This is also what makes the arena's `render ⊥ nav` rule hold in practice rather than
            // only at the gate: no map render is attempted while the nav arm is out, so the claim
            // below is never refused on the ordinary path.
            let frozen = app.reroute_freeze_active();
            if frozen && dirty.map {
                pending_map_redraw = true;
                dirty.map = false;
                dirty.region = None;
            }
            // Build this pass's frame — **render only**, still under the guard: the map render reads
            // the `reader`/`route` built just below, whose borrows of the open SD handles live here.
            // The push to glass happens after the store phase closes, guard-free (#809).
            let rendered: Option<RenderedFrame> = if frozen {
                // The banner rides the overlay plane, which on this board means: draw it straight
                // into the resident framebuffer and let the self-diffing present push the handful of
                // rows it changed. It deliberately does **not** go through the FLPR's
                // `present_overlay` composite path the hold bulge uses — that path's scratch is
                // bounded at 16 columns (`MAX_OVERLAY_COLS`), because the bulge is a 16 px strip at
                // the right edge; a 240-px-wide banner band would need a ~26 KB transient on the
                // overlay frame's stack, on the crate where transients overflow it. Painting into the
                // frame is free instead, and safe because the map render that would otherwise own
                // those pixels is precisely what is not running — the full repaint when the freeze
                // lifts restores them.
                match app.reroute_banner_rows(FRAME_H as f32).filter(|_| dirty.overlay) {
                    Some((y0, rows)) => {
                        let (stats, render_us) = display.render_frame(|f: &mut crate::ls021_flpr::Frame64| {
                            let mut fbdev = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
                            // Clip the framebuffer to the banner's own band — belt and braces over
                            // the drawing itself, so a future overlay item cannot quietly repaint
                            // map pixels the freeze is preserving.
                            fbdev.set_clip(Rectangle::new(
                                Point::new(0, y0 as i32),
                                Size::new(FRAME_W as u32, rows as u32),
                            ));
                            app.render_overlay(&mut fbdev, FRAME_W as f32, FRAME_H as f32, color_fn);
                            obc_render::RenderStats::default()
                        });
                        Some(RenderedFrame { needs_map: false, stats, render_us })
                    }
                    // Mid-freeze with no edge: nothing changed on either plane, so nothing to push.
                    None => None,
                }
            } else if dirty.map {
                // The map pipeline runs **only when the base screen needs the streamed `Reader`** — the
                // Map view, and the POI list on the frame it takes its one-shot snapshot (#425, its
                // query runs in the draw path off `rx.reader`). On a menu / Statistics / Home redraw, or
                // a POI list already showing its frozen snapshot, it's skipped entirely — no SD
                // style-table parse, no `Reader` build (so no stack spike), no map render — that screen
                // draws just its own chrome. Such a frame costs only its own draw + the push.
                let needs_map = app.base_needs_reader();
                // The flat map source is resolved once at boot and skipped on chrome-only frames,
                // keeping menu redraws free of map I/O.
                let reader = needs_map.then(|| Reader::new(flat_map, map_tables, map_cache));
                if needs_map && reader.is_none() {
                    pending_map_redraw = true;
                    defmt::warn!(
                        "map: reader build failed this frame (flaky SD?) — kept frame, retrying redraw next frame"
                    );
                    None
                } else {
                    // ── The scratch arena's render arm (#1146 P2) ──
                    // A base that **draws the map** claims it for the render span and gives it back
                    // at the end of this block — the arena's whole render ⊥ nav / render ⊥ usb
                    // enforcement, since a live search or a live transfer is literally the holder.
                    // A chrome base claims nothing and renders with **no scratch at all**: only the
                    // Map screen's draw touches it, and the app's render entry point takes it as an
                    // `Option`. That's what keeps those frames drawing *while* another arm is out —
                    // the nav-planning spinner is the only sign of life during a menu plan, and the
                    // map-transfer card the only explanation for a saturated SD bus.
                    let draws_map = app.base_draws_map();
                    let mut render_guard = if draws_map { crate::arena::claim_render().ok() } else { None };
                    // Unreachable on the ordinary path (the freeze above skips map frames during a
                    // search, and a transfer puts its card over the map), so a refusal is a gating
                    // bug — already reported loudly by `arena::claim_render`. Degrade the way every
                    // other transient render failure does: keep the frame on glass, retry next pass.
                    if draws_map && render_guard.is_none() {
                        pending_map_redraw = true;
                        defmt::warn!(
                            "map: the scratch arena is held by {} — skipping this map redraw, retrying next frame",
                            defmt::Debug2Format(&crate::arena::owner())
                        );
                        None
                    } else {
                        // Render the whole frame into the resident RGB222 plane — the display boundary,
                        // behind `MapDisplay::render_frame`; the present below (after the guard is gone)
                        // scans it out, going *around* a live bulge's rows so the composite paints them.
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
                        // Construct the rain lease only for the WX11 rain-map base. The dashboard
                        // and hourly screens still receive the resident snapshot, but pay zero SD
                        // header/frame/tile reads during draw; the ordinary Map never receives a
                        // lease and therefore cannot be tinted by weather accidentally.
                        let weather_source = if app.base_wants_rain() {
                            weather_bundle.and_then(|bundle| crate::flat_store::reconcile_weather(flat, Some(bundle)))
                        } else {
                            None
                        };
                        let weather_reader =
                            weather_source.and_then(|source| weather_bundle?.validated.reader(source).ok());
                        let weather_bind_failed =
                            app.base_wants_rain() && weather_bundle.is_some() && weather_reader.is_none();
                        if weather_bind_failed {
                            // A transient header read is not evidence of a dry map. Keep the last
                            // complete glass and retry rather than flashing a misleading rain-free
                            // frame. A valid reader with no current frame remains a truthful None.
                            pending_map_redraw = true;
                            drop(render_guard);
                            defmt::warn!("weather: active bundle bind failed — kept frame, retrying redraw next frame");
                            None
                        } else {
                            let wall_now = app.wall_unix_now() as i64;
                            let rain_step = app.state.rain_step;
                            let mut rain_adapter = weather_reader.as_ref().and_then(|reader| {
                                obc_app::RainOverlayAdapter::at_step(reader, &mut weather_cache, wall_now, rain_step)
                            });
                            let weather_snapshot_ref = weather_snapshot.as_ref();
                            let weather_refreshing = weather_refresh_in_flight();
                            #[cfg(feature = "sd-bench")]
                            let read_before = crate::card_io::read_perf_snapshot();
                            let (stats, render_us) = display.render_frame(|f: &mut crate::ls021_flpr::Frame64| {
                                let mut fbdev = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
                                if let Some(r) = clip {
                                    fbdev.set_clip(r);
                                }
                                // One scene, because there is one map file: the `Reader` is both the
                                // geometry source and the POI/hours/nav one. The volume-set arm that
                                // used to sit beside this — `render_scene_map_rain_timed` with a
                                // `MountedSet` as the scene and the core `Reader` for everything else
                                // — is gone with the set mount (FS7.5-c2, #1420).
                                app.render_map_rain_timed(
                                    render_guard.as_deref_mut(),
                                    &mut fbdev,
                                    reader.as_ref(),
                                    route.as_ref(),
                                    rain_adapter
                                        .as_mut()
                                        .map(|adapter| adapter as &mut dyn obc_render::RainOverlaySource),
                                    obc_app::WeatherFeed {
                                        snapshot: weather_snapshot_ref,
                                        refreshing: weather_refreshing,
                                    },
                                    FRAME_W as f32,
                                    FRAME_H as f32,
                                    color_fn,
                                    &InstantClock,
                                )
                            });
                            #[cfg(feature = "sd-bench")]
                            if needs_map {
                                let reads = crate::card_io::read_perf_snapshot().since(read_before);
                                defmt::info!(
                                "map SD bench: {=u32} us | logical {=u32} read(s) / {=u32} B | physical {=u32} command(s) / {=u32} block(s) ({=u32} single + {=u32} multi)",
                                reads.us,
                                stats.map_sd_reads,
                                stats.map_bytes_read,
                                reads.commands,
                                reads.blocks,
                                reads.single_commands,
                                reads.multi_commands
                                );
                            }
                            // The guard (when a map base took one) dies here, at the end of the render
                            // span — before the present's await, never across it (#677).
                            drop(render_guard);
                            Some(RenderedFrame { needs_map, stats, render_us })
                        }
                    }
                }
            } else {
                None
            };

            // ═══ The store phase ends HERE: the tuple is the block's value and `store_guard` dies at
            // the closing brace — every reader/source/track borrow of the card ended above, and the
            // present await below *cannot* hold the guard, by construction. ═══
            (rendered, dirty.map, hold_p, t_store.elapsed().as_micros())
        };

        // ═══ Present phase (#809): guard-free — the FLPR scans the frame (~44 ms full-frame)
        // with the store released, so a BLE object operation interleaves with the scan instead
        // of queueing behind it. `presented_ok` anchors the DFU trial confirm in the tail. ═══
        let mut presented_ok = false;
        if let Some(rf) = rendered {
            let (ok, push_us) = display.present_frame(overlay_span).await;
            presented_ok = ok;

            // Snapshot this frame's render stats for the host telemetry line — the same numbers as
            // the RTT `map frame` log. The nRF reader isn't `TimedSource`-wrapped, so the SD/cache
            // I/O folds into `collect_us` (`read_us` stays 0); the bulge composites on its own
            // overlay push, so `overlay_us` stays 0.
            #[cfg(feature = "debug-uart")]
            {
                let mpp_milli = (app.state.viewport(FRAME_W as f32, FRAME_H as f32).meters_per_pixel() * 1000.0) as u32;
                last_telem = obc_platform::debug_link::Telemetry {
                    frame_us: rf.render_us as u32,
                    lod: rf.stats.lod as u8,
                    feat_drawn: rf.stats.features_drawn as u32,
                    feat_tried: rf.stats.features_tried as u32,
                    feat_dropped: rf.stats.features_dropped as u32,
                    chunks: rf.stats.chunks_visited as u32,
                    cache_hits: rf.stats.map_chunk_hits,
                    cache_misses: rf.stats.map_chunk_misses,
                    sd_reads: rf.stats.map_sd_reads,
                    bytes_read: rf.stats.map_bytes_read,
                    collect_us: rf.stats.collect_us,
                    read_us: 0,
                    sort_us: rf.stats.sort_us,
                    draw_us: rf.stats.draw_us,
                    overlay_us: 0,
                    mpp_milli,
                };
            }

            // A transport fault (`present` → false, e.g. a stalled FLPR) latches a retry like the
            // reader-build failure rather than faulting.
            if !ok {
                pending_map_redraw = true;
            }

            // A map frame carries the map render stats; a non-map (menu / Statistics / Home) frame
            // is just a screen redraw + push, so log it as such — no meaningless lod/feat/chunks.
            if rf.needs_map {
                defmt::info!(
                    "map frame: render {=u64} us + push {=u64} us | lod {=usize} | feat {=usize}/{=usize} | chunks {=usize} | map-cache {=u32} hit / {=u32} miss",
                    rf.render_us,
                    push_us,
                    rf.stats.lod,
                    rf.stats.features_drawn,
                    rf.stats.features_tried,
                    rf.stats.chunks_visited,
                    rf.stats.map_chunk_hits,
                    rf.stats.map_chunk_misses
                );
            } else {
                // A menu / Statistics / Home redraw: just its own chrome + the (now self-diffed)
                // push, so the partial-push win shows as a small `push` next to the full `render`.
                defmt::info!(
                    "ui frame: render {=u64} us + push {=u64} us (screen redraw, no map)",
                    rf.render_us,
                    push_us
                );
            }
        }

        // The hold bulge already pushed at the top of this pass (bulge-first, see above). But if a
        // screen present just landed, its `exclude` skipped the bulge rows — they still show the *old*
        // frame under the bulge. Re-composite them over the fresh fb now (a ~12 ms partial push, only
        // on the rare pass where a redraw and a live bulge coincide) so the band never lags the screen.
        if dirty_map && overlay_span.is_some() {
            display.present_bulge(overlay_span, false).await;
        }

        // ═══ Store tail (#809): a second short guard for the store work that must FOLLOW the
        // present — the trial confirm is anchored on a frame having reached glass, and the
        // deferred ride save must grind against an already-presented screen, not delay it. ═══
        let (save_pending, tail_held_us) = {
            let mut store_guard = shared.lock().await;
            let t_tail = Instant::now();
            let SharedStore { storage, settings: settings_store } = &mut *store_guard;

            // ── DFU trial confirm (epic #615 S4, #619), once, at the health anchor ──
            // A frame just landed on glass and the SD mounted at boot: if this boot is a
            // trial (`Trial { installed, .. }` on the boot-state page), write
            // `Idle { installed }` — the whole confirm — and hand the app the one-time
            // "updated to vX" fact for S5's toast. A failed first present retries the
            // anchor on a later pass; an unconfirmed trial rolls back next boot by design.
            if trial_confirm_pending && presented_ok {
                trial_confirm_pending = false;
                if let Some(installed) = crate::dfu::confirm_trial(settings_store) {
                    app.apply_event(obc_app::HostEvent::UpdateConfirmed(obc_app::dfu::clamp(
                        installed.fw_version_str(),
                    )));
                }
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
            (storage.as_ref().is_some_and(|s| s.has_pending_save()), t_tail.elapsed().as_micros())
        };

        // #809 instrumentation — debug level, outside the timed render/push spans: the pass's two
        // guard holds, proving on RTT that neither contains the present (compare with `map frame`'s
        // `push`; before this split the single hold contained render *and* push). `tail` spikes on
        // the rare deferred-save pass — that stretch is the store-contention floor stage 2 would
        // attack, so this line is also its measurement.
        defmt::debug!(
            "store guard: phase {=u64} us + tail {=u64} us (present ran guard-free)",
            store_held_us,
            tail_held_us
        );

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
        // A map upload owns the device (#889): the card is the only thing on glass, so don't let
        // an animation flag or a short app deadline wake the loop faster than the progress pace —
        // every avoided repaint is ~85 ms handed back to the SD write path. Gestures and sensor
        // events still wake the loop early; the feed throttle above makes those wakes repaint
        // nothing. Well under the WDT feed cap, which is the ceiling this must never approach —
        // the un-yielding upload loop starving this feed is exactly what reset the device on
        // glass (2026-07-30).
        let ms = if map_uploading { ms.max(MAP_XFER_PACE_MS) } else { ms };
        let _ = select5(
            GESTURES.ready_to_receive(),
            INPUT_WAKE.wait(),
            // A sensor/host datapoint, or (`ble` builds) a store movement — an upload/delete
            // rescans the catalog now, not at the next timer wake (#450).
            wait_host_or_sensor_event(
                #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
                consumer,
            ),
            // A BLE link edge — connect/disconnect *and* the pairing passkey — so the passkey card
            // wakes the loop from warm sleep (epic #447, P2). `pending()` on a map build.
            wait_ble_edge(),
            Timer::after_millis(ms as u64),
        )
        .await;
    }
}
