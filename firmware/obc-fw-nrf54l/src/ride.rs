//! The map/ride thread-mode plane (in every build) — split out of `main.rs` (issue #351).
//!
//! [`run_app`], the shared backend-agnostic ride loop, plus its loop-only helpers: the sensor-wake
//! select arm, the GPS power policy, the watchdog cadence, the per-frame render clock, and the
//! route-catalog scan. `main` still owns bring-up + the resident statics and awaits [`run_app`]
//! as its tail future (single call site — see the `#[inline(always)]` note on the fn).

use core::sync::atomic::Ordering;

// The event-driven loop's wake select: `select5` over gesture / hold-wake / sensor / BLE link-edge /
// deadline.
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
use obc_ports::{InputClock, RideClock, Sensors, SettingsStore};
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

use crate::input_plane::{CHORDS, GESTURES, INPUT_HB_MS, INPUT_WAKE, LOOP_MS};
use crate::map_plane::MapDisplay;
use crate::{stackmeter, SharedStore, SharedStoreMutex};

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
            embassy_futures::select::select(
                crate::object_store::wait_store_changed(),
                crate::usb::wait_stage_request(),
            ),
        ),
    )
    .await;
}

/// Report successful protocol-v4 route/trip uploads as [`ExternalFacts`], and only after the catalog
/// snapshots containing their committed heads have been fed to `App`. This ordering is what lets a
/// same-id active route replacement invalidate geometry-derived state: the next pass's
/// `on_route_uploaded` resolves the identity against the snapshot that was just read.
///
/// The fact slots are *latest-wins*, so several uploads inside one catalog read collapse to the
/// newest of each kind — which is what the card should show anyway, and the identity remap is the
/// catalog read's own work, not the fact's.
fn note_catalog_uploads(app: &App, facts: &mut obc_app::device_core::ExternalFacts) {
    use obc_app::device_core::{RouteUpload, TripUpload};
    // More than one full catalog of distinct facts can only happen when rapid remove/create churn
    // leaves stale ids queued. If that exhausted the bounded handoff, conservatively refresh the
    // active route first: even if its exact replace fact was the evicted oldest entry, no geometry
    // derived from the displaced revision survives. Exact retained facts then land in commit order
    // and retain the final advisory card. The store-revision level cannot stand in for this — it
    // orders a *re-read*, and only a `replaced` upload drops the geometry-derived state.
    if crate::flat_store::take_catalog_upload_loss() {
        if let Some(id) = app.active_route_index().and_then(|i| app.route_ids().get(i).copied()) {
            facts.note_route_upload(RouteUpload { id, replaced: true, elevation: None });
        }
    }
    while let Some(upload) = crate::flat_store::take_catalog_upload() {
        match upload.kind() {
            crate::flat_store::CatalogUploadKind::Route => {
                facts.note_route_upload(RouteUpload { id: upload.id(), replaced: upload.replaced(), elevation: None })
            }
            crate::flat_store::CatalogUploadKind::Trip => {
                facts.note_trip_upload(TripUpload { id: upload.id(), replaced: upload.replaced() })
            }
        }
    }
}

/// **`CatalogEffect::ReadCatalog`'s whole body**: re-read the object store into the resident
/// catalogs and re-point the weather bundle at whatever the read found.
///
/// Returns whether the read was *complete*. A partial one — a transient listing or object I/O
/// failure — deliberately keeps the previous whole snapshot, so a flaky card shows a stale menu
/// rather than an empty one; the caller answers `Failed { Unreadable }` and owns the retry.
///
/// The upload facts are drained **after** the catalogs are re-fed, and only on a complete read: the
/// identity the next pass resolves has to be the one that was just read, which is what lets a
/// same-id route replacement reach `on_route_uploaded` and drop every piece of geometry-derived
/// state from the displaced revision before rendering resumes.
#[inline(never)]
fn read_catalogs(
    flat: &'static obc_storage::flat::FlatStore<crate::flat_store::FlatCard>,
    app: &mut App,
    facts: &mut obc_app::device_core::ExternalFacts,
    weather_bundle: &mut Option<crate::flat_store::FlatWeather>,
    weather_sample_key: &mut Option<WeatherSampleKey>,
) -> bool {
    // Drop the held revision before rebuilding identity/index state. A replace at the same ObjectId
    // must reopen the new revision, not keep rendering the hold.
    crate::flat_store::reconcile_route(flat, None);
    let routes_loaded = crate::flat_store::load_routes(flat, app);
    let trips_loaded = crate::flat_store::load_trips(flat, app);
    if let Ok(next) = crate::flat_store::active_weather(flat) {
        if next != *weather_bundle {
            *weather_bundle = next;
            *weather_sample_key = None;
        }
    }
    // The installed-data fact (#1437, #1549): the selected head's object id and revision, reported
    // as a level. This is what makes `WeatherDomain::installed()` non-`None` on a real device, and
    // a *move* of it is what records `RefreshResult::Installed`.
    if let Some(weather) = *weather_bundle {
        facts.note_weather_data(obc_app::device_core::WeatherData {
            data: obc_app::device_core::DataIdentity::new(weather.id.0),
            revision: obc_app::device_core::Revision::new(weather.revision.0),
        });
    }
    crate::flat_store::reconcile_weather(flat, *weather_bundle);
    let rides_loaded = crate::flat_store::load_rides(flat, app);
    if routes_loaded && trips_loaded {
        note_catalog_uploads(app, facts);
    }
    routes_loaded && trips_loaded && rides_loaded
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

/// Take the scratch arena's **nav arm** for a fresh route search, against the app's own
/// quiesced-map proof.
///
/// `Err(why)` = the caller must answer the failure tier now rather than arm a plan; a refused claim
/// took nothing, so no spinner can hang behind a half-claim. A request arriving while a plan is
/// still in flight is *not* a refusal: we already hold the arm, and the drain overwrites the
/// planner slot for the new plan exactly as it did before.
///
/// The arena's own owner is what enforces `nav ⊥ usb` — a cable transfer streaming into the same
/// store holds the block, so the claim is refused by ownership rather than by a second gate
/// tracking the same fact. The refusal *names* that holder, because "wait for the cable" is
/// something the rider can act on and "the scratch arena is busy" is not.
#[cfg(has_nav)]
fn nav_take_arena(app: &App, guard: &mut Option<crate::arena::NavGuard>) -> Result<(), &'static str> {
    use obc_app::{ArenaError, ArenaOwner};
    if guard.is_some() {
        return Ok(());
    }
    let Some(quiesced) = app.nav_arena_precondition() else {
        // Unreachable by construction: draining a plan command is what engages the Recalculating
        // freeze, so by the time we are here the map plane is already quiet over a map base — and
        // menu planning has no map base to quiet. Loud in debug, handled in release.
        debug_assert!(false, "a plan drained with the map plane still drawing — the freeze did not engage");
        return Err("the map plane is not quiesced");
    };
    match crate::arena::claim_nav(quiesced) {
        Ok(g) => {
            *guard = Some(g);
            Ok(())
        }
        Err(ArenaError::Busy(ArenaOwner::Usb)) => Err("a cable transfer holds the store"),
        Err(_) => Err("the scratch arena is busy"),
    }
}

/// The fixed slot index (0 HR · 1 Power · 2 Cadence) a scanned sensor's kind maps to (SE7, #714) —
/// used to tag a board scan hit for the app seam, which speaks slot indices, not `obc_ble` kinds.
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
/// #496 de-nesting kept) and emit the one `nav route:` RTT line with the per-phase breakdown
/// (issue #499's DoD). The *answer* is the caller's: it delivers a terminal
/// [`NavigatorOutcome`](obc_app::navigator::NavigatorOutcome) under the operation's token, and the
/// next pass activates the route and swaps the planning screen for the computed-route overview (or
/// the failure card).
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
fn nav_finish(nav: &mut NavBuffers<'_>, run: NavRun, result: Result<(u64, u32), obc_route::NavError>, now: u32) {
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

/// The drained chord's name, for the same field-forensics RTT record as [`gesture_name`].
fn chord_name(c: obc_app::Chord) -> &'static str {
    match c {
        obc_app::Chord::Quick => "Quick",
        obc_app::Chord::Context => "Context",
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

/// What this firmware image and this hardware implement **at all** — constant for a boot, and the
/// place the two things the board genuinely cannot do are said out loud rather than answered as
/// failures (`Capabilities`' own rule).
///
/// - `detour: false` — #882's flow holds the planned detour's OBCR in RAM until the rider commits
///   and then stream-splices it into a derived route. The host does that with a `Vec`; the board has
///   one flat-store reservation and no heap, so the splice has nowhere to read the detour from while
///   it writes. That is a storage design with its own acceptance — Gate 4 item 5, #1400.
/// - `retention_metadata: false` — FS7/FS8 removed the FAT sidecars and #1398 supplies the
///   ObjectId-keyed replacement. A route-use stamp is mirrored in the resident view and is never
///   durable, which is a stated capability now instead of a dropped command.
const BOARD_SUPPORT: obc_app::device_core::PlatformSupport = obc_app::device_core::PlatformSupport {
    detour: false,
    settings_persistence: true,
    dfu: true,
    weather: true,
    bonding: true,
    storage_space_report: true,
    retention_metadata: false,
};

/// The board's store identity for [`ExternalFacts::note_store_revision`]. One card mounted once at
/// boot, for the life of the boot: the identity half never moves and only the revision does. A
/// remount would be a different identity — and the board faults out rather than remounting, so there
/// is nothing here that could report a store it has unmounted.
const BOARD_STORE: obc_app::device_core::StoreIdentity = obc_app::device_core::StoreIdentity::new(1);

/// One in-flight `CatalogEffect::RemoveObject` on the flat store's **ticketed** writer path: the
/// storage task's answer slip, and the operation token that answer has to carry back.
///
/// The removal moved off the answerless `MENU_DELETES` channel with #1397 S6b, because a full queue
/// there *drops* the id — which the domain would read as an operation that never completes, leaving
/// its one catalog slot occupied for the rest of the boot. Here a full queue is simply a pass where
/// the effect was not taken, and a refused commit is a `Failed` the domain re-queues.
struct CatalogRemoval {
    ticket: crate::flat_store::Ticket,
    token: obc_app::device_core::OperationToken<obc_app::device_core::CatalogTag>,
    object: u64,
}

/// The reply slot the catalog removal's round trip uses. One slot per *concurrently live* call, and
/// the domain admits one catalog operation at a time, so this is exactly one.
static CATALOG_STORE_REPLY: crate::flat_store::Reply = embassy_sync::signal::Signal::new();

/// **The board's typed effect executor** — everything owed between two `App::run_pass` calls.
///
/// It is the [`RideRuntime`](https://github.com/timohueser/OpenBikeComputer/issues/1262) shape
/// #1262's amendment accepted, without the module: bounded effects are *staged* out of the plan and
/// executed in the physical phase that already owns them, and token-carrying outcomes are returned
/// on a later pass. The phase order is the board's — the guard split (#809), bulge-first (#348) and
/// the arena claims (#1146 P2) are physical facts this struct does not get a vote on; the pass order
/// is DeviceCore's.
///
/// What is **not** here is as deliberate: no `PassPlan` (it is destructured and dropped inside the
/// store phase — FAR-19's rule, restated for the typed protocol), no polyline (the derived reads are
/// served into a stack buffer immediately before the pass, so 512 B never becomes resident), and no
/// mailbox (the ~600 B `Deque` stays a synchronous stack temporary in the residual drain's block).
#[derive(Default)]
struct RideExec {
    /// What the executor finished, for the next pass's stage 1.
    outcomes: obc_app::device_core::OutcomeSlots,
    /// What moved underneath DeviceCore that nobody asked for, for the next pass's stage 2.
    facts: obc_app::device_core::ExternalFacts,
    /// The previous plan's derived needs, answered at the top of the next store phase.
    needs: obc_app::device_core::DerivedNeeds,
    /// The bounded effects the previous pass decided, each served in its own physical phase: the
    /// catalog / settings / storage-info / navigator ones at the top of the store phase, the DFU one
    /// in the guard-free block ahead of it (#809 — the card's present must not hold the store).
    effects: obc_app::device_core::EffectSlots,
    /// The in-flight removal, held across passes and polled without parking.
    catalog: Option<CatalogRemoval>,
    /// The operation the planner run is running under — the token every planner answer carries back.
    #[cfg(has_nav)]
    nav_token: Option<obc_app::device_core::OperationToken<obc_app::device_core::NavigatorTag>>,
    /// A `DfuEffect::ArmInstall` passed its go/no-go and is waiting for the "Installing update" card
    /// to reach glass — the count is how many frames it has waited. The arm runs in the store
    /// **tail**, after the present, and never returns on success, so the frame the MIP holds through
    /// the whole flash is a real presented frame rather than a hand-rolled render inside the effect.
    ///
    /// It waits because `CardScheduler::deliver_dfu` can **bounce** an install-began answer that has
    /// to *push* rather than replace a wait (the `dfu-install` debug arm, with no spinner up) when
    /// the stack is full; it re-queues, and arming meanwhile would freeze a frame showing something
    /// else onto the panel for the whole flash and the reboot. The wait is bounded by
    /// [`ARM_CARD_FRAMES`] rather than open-ended: the install matters more than the frame, which is
    /// the same stance the inline path took when a present failed.
    arm_pending: Option<u8>,
}

/// How many frames the arm waits for the "Installing update" card before going ahead without it.
/// A bounced push lands on the very next sweep in practice; this is that with room.
const ARM_CARD_FRAMES: u8 = 8;

impl RideExec {
    /// Whether the executor is holding something the next pass must see — an answer to consume, or
    /// an effect to serve, or a derived read it has been asked for.
    ///
    /// `residual_pending` is the **legacy mailbox's** half, which this struct cannot see and which
    /// now carries one thing: the rider's `ForgetBond`. It is posted by one pass and performed by
    /// the next pass's drain, and the guarded hold that posts it leaves a static screen — so
    /// without folding it in, that "next pass" is whenever the rider presses something else.
    /// `App::has_pending_residual_command` is narrow on purpose; see its docs for why the wider
    /// query would spin.
    ///
    /// The rider's ride **save** was the other half until #1398. It is a `RecorderEffect` in this
    /// pass's own plan now, so `effects` covers it and it needs no term of its own.
    ///
    /// The in-flight catalog removal is deliberately **not** here: it keeps the short animation
    /// cadence instead of an immediate re-pass, because it is a round trip to another task, and
    /// spinning at full speed against a commit that takes hundreds of milliseconds would starve the
    /// executor that has to answer it. The re-read a failed read owes is not here either — the
    /// domain holds it (#1541) and offers it once per pass, which is once per wake.
    ///
    /// Folds into the wake exactly as [`PassPlan::immediate`] does for a deferred connection: the
    /// work is already decided, and parking on it would leave it sitting until the next rider input.
    fn owed(&self, residual_pending: bool) -> bool {
        self.outcomes.has_pending() || self.effects.has_pending() || !self.needs.is_empty() || residual_pending
    }

    /// Whether a store round trip is outstanding — the removal ticket. A committed removal wakes the
    /// loop through `CATALOG_WAKE`, but an `existed: false` answer and a refused commit move no
    /// sequence and therefore raise no wake, so without this the answer would sit until something
    /// else happened to wake the loop.
    fn polling_store(&self) -> bool {
        self.catalog.is_some()
    }

    /// Hand one outcome to its domain's slot.
    ///
    /// The pass drains every slot at stage 1 unconditionally, so a full slot means two answers were
    /// produced for one domain inside a single frame. That cannot happen — each arm serves at most
    /// one effect — and a change that made it happen would otherwise lose the second answer.
    fn deliver<T>(slot: &mut obc_app::device_core::Slot<T>, outcome: T, domain: &str) {
        if slot.try_put(outcome).is_err() {
            defmt::error!("exec: {=str} answered twice in one frame — the second answer was lost", domain);
            debug_assert!(false, "one outcome per domain per frame");
        }
    }
}

/// The GPS power state the ride wants: deep-sleep when not tracking, full-power fixes while riding, or
/// the M10's low-power tracking when the `power_saver` toggle is on. Recomputed each frame in
/// [`run_app`] and pushed to the sensor task (via [`SensorControl::set_power`]) only on a change.
/// Real-sensor build only — the `synth` / `debug-uart` feeds have no power-managed receiver.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
fn desired_gps_power(app: &App) -> GpsPower {
    if app.recording() {
        if app.settings().power_saver {
            GpsPower::LowPower
        } else {
            GpsPower::Active
        }
    } else {
        GpsPower::Sleep
    }
}

/// Drive the panel at `level`, remembering it in `last` so the PWM is written only on a change.
///
/// Two callers, one rule: the boot seed applies the persisted level before the first frame, and the
/// loop applies the app's derived answer every pass. Both go through here so the change gate cannot
/// drift between them. Today's PWM port never refuses; a refusal would be a port that cannot reach
/// its hardware, which is worth one line per change and never one per frame.
fn apply_backlight(backlight: &mut crate::panel_power::PanelBacklight, last: &mut u8, level: u8) {
    if level == *last {
        return;
    }
    *last = level;
    if obc_ports::Backlight::apply(backlight, level).is_err() {
        defmt::warn!("backlight: the port refused level {=u8}", level);
    }
}

/// The shared map plane + ride loop, driving present through [`MapDisplay`] so it carries **no backend
/// `#[cfg]`**. Each tick: drain the gestures the input plane recognised, advance the visible screens'
/// timed content, reconcile the card to the app's intent (open the selected route's geometry; begin /
/// finalise the ride object), feed the sensors → `tick` (integrate the fix, map-match, record the
/// track point), then re-render the map only on `dirty.map` and present it. A static screen does zero
/// map renders. LED0 keeps a ~1 Hz heartbeat. Never returns.
///
/// A finished ride is the flat object's recorded 20-byte samples followed by one summary footer —
/// the device writes no GPX (the phone owns human-format export after sync). Finish journals that
/// bounded footer tail and clears `RECORDING` in one commit; it never rereads or converts the ride.
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
    // post-present tail (trial confirm) — and **never holds it across the present
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
    // The panel's brightness port (#1558), armed in `main` where the peripherals live and driven
    // from the per-pass apply below. By value: this loop is its only user for the life of the
    // device, and it never returns.
    mut backlight: crate::panel_power::PanelBacklight,
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
    // The resample counter reported as `ExternalFacts::note_weather_sample` — the weather screens'
    // repaint edge, which no stack-local render key can see (a resample changes the card under an
    // unchanged installed revision).
    let mut weather_sample = obc_app::device_core::Revision::ZERO;
    let mut weather_bundle = crate::flat_store::active_weather(flat).ok().flatten();
    crate::flat_store::reconcile_weather(flat, weather_bundle);
    // **The board's typed effect executor** (#1397 S6b) — the outcomes, facts and staged effects
    // that live between two `App::run_pass` calls. Built here, beside the other loop-lifetime state,
    // because it is exactly that: one per boot, owned by the one task that touches the `App`.
    let mut exec = RideExec::default();
    let mut ride_recorder = crate::flat_ride::Recorder::new(
        flat,
        crate::flat_store::writer().expect("the flat storage task is armed before the ride loop"),
        Instant::now().as_millis() as u32,
    );
    // A reset may have landed after the footer checkpoint but before the single clearing commit.
    // Service that terminal state before the first UI pass; it must not wait for a later route or
    // session edge, and it must never expose a footer-bearing object as resumable samples.
    ride_recorder.settle().await;
    let recovery_warning = ride_recorder.take_warning();
    if let Some(continuation) = ride_recorder.recovered_continuation() {
        let _ = app.offer_recovered_ride(continuation);
    } else if ride_recorder.recovery_faulted() {
        let _ = app.offer_damaged_ride();
    } else if recovery_warning {
        exec.facts.raise_warnings(obc_app::WarningFlags::REC_ERROR);
    }

    // Per-frame ride-loop state:
    // - `prev_route` re-centres SynthLocation onto a freshly-loaded route's start (`synth` build only);
    // - `prev_active` gates the SD route reconcile on actual change, `opened_session` the ride object;
    // - `route_index`/`index_route` cache the active route's chunk index, rebuilt only on a route change;
    // - `pending_map_redraw` re-arms a redraw a transient SD glitch couldn't service;
    // - `last_telem*` throttle the host telemetry (debug-uart only).
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
    let mut prev_route: Option<usize> = None;
    let mut prev_active: Option<usize> = None;
    // The ride session this executor has opened an object for. Never cleared by a close: a session
    // that has been served is served, whatever became of its object. See `RecorderMachine::object_owed`
    // for why this is an id rather than "is anything recording".
    let mut opened_session: Option<u32> = None;
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
    // A map upload's 64 KiB write-combining arm. Only this loop switches arena owners; the USB
    // task asks through a level+edge handshake and borrows the bytes synchronously in storage.
    let mut usb_stage_guard: Option<crate::arena::UsbGuard> = None;
    // The active route's resident chunk-index slot. A bare `RouteIndex` + validity flag, NOT an
    // `Option<RouteIndex>` built by value: the slot is ~12.3 KB and permanently part of this frame
    // either way, but a by-value build (`RouteIndex::read`'s return) also transits the stack at
    // the pass's deepest point — which is what overflowed the 44 KB main stack on the post-upload
    // rescan (STKOF HardFault, 2026-07-12). `build_route_index_into` fills it in place.
    let mut route_index: RouteIndex = RouteIndex::empty();
    let mut route_index_valid = false;
    let mut index_route: Option<usize> = None;
    let mut pending_map_redraw = false;
    let mut power_off = crate::panel_power::SystemOff;
    // The level last handed to the backlight, so the PWM is touched on a change rather than every
    // pass. `u8::MAX` is never a real level, so the boot apply below always reaches the hardware.
    let mut backlight_level = u8::MAX;
    // The panel-light capability, straight from the port that answers it (#1515 D2). `true` since
    // the board drives a real PWM (#1558), which is what puts the brightness control on the
    // drawer's root row.
    app.set_backlight_available(obc_ports::Backlight::available(&backlight));
    // The map plane is one resident RGB222 framebuffer that the present scans out of, so every
    // repaint here is a repaint *over the last frame*. That is what lets the app leave the frozen
    // base's rows alone while a drawer's sheet grows over them (#1559).
    app.set_resident_frame(true);
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
    let mut pushed_sensors: [Option<([u8; 6], bool)>; obc_app::SENSOR_SLOTS] = [None; obc_app::SENSOR_SLOTS];
    // SE7 (#714): the next-re-arm deadline (loop-millis) for the discovery scan while the scan list is
    // up — `0` = not scanning (rings `request_scan` on the rising edge, then re-arms just under the
    // board's ~10 s window), so the scan stays live without pulsing the manager's work edge every pass.
    let mut sensor_scan_rearm_ms: u32 = 0;

    // Settings: seed the app from the persistent RRAM store at boot (a blank/corrupt page decodes to
    // `None` → defaults), then persist on any change the settings screens make. One brief lock,
    // released at once — the loop re-locks the shared store each pass.
    app.set_settings({
        let mut store = shared.lock().await;
        store.settings.load().unwrap_or_default()
    });
    // …and the brightness in that seed reaches the panel **here**, before the first frame is drawn.
    // The per-pass apply at the end of the loop would otherwise leave the light at the factory level
    // `PanelBacklight::new` armed it with until the first render finished, so a rider who set a dim
    // panel would watch it start bright and then drop.
    apply_backlight(&mut backlight, &mut backlight_level, app.backlight_level());
    // The weather alert-mark record (#1542), seeded the same way. A seed that came out of a stored
    // v16 blob's frozen span arms the record's write, so the next pass rehomes the rider's anchors.
    {
        let mut store = shared.lock().await;
        let (marks, provenance) = store.settings.load_alert_marks();
        app.set_alert_marks(marks, provenance);
    }

    // The DFU boot-outcome reconcile: boot-state page + the armer's breadcrumb → the one-time
    // post-update verdict card ("UPDATE FAILED" / the accepted-trial toast). A `Trial` boot is
    // left alone — the health-anchor confirm below owns that verdict. Same brief-lock idiom.
    {
        let mut store = shared.lock().await;
        crate::dfu::reconcile_boot_outcome(&mut exec.facts, &mut store.settings);
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
    // The transfer level last reported to `CoreMode`, so the RTT line below logs its edge and not
    // one line per pass. `debug-uart` only — see the line.
    #[cfg(feature = "debug-uart")]
    let mut prev_transferring = false;

    loop {
        let now = Instant::now().as_millis() as u32;
        let hw = stackmeter::used(now);
        if hw > stack_hw {
            stack_hw = hw;
            // Surface the peak in the diagnostics blob for the A9 soak rig (#277) — the ride loop owns the
            // stackmeter, so it publishes the mark into the BLE state the blob reads.
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

        // ── The levels this frame reports, ahead of the pass that reads them (stage 2) ──
        //
        // The **store revision is not one of them here.** It is reported immediately before
        // `run_pass`, after this frame's store phase — see there for why.
        //
        // `CoreMode`'s transfer level, from the flat engine's own live transfer (#1397 S6b, closing
        // S5 open question 2). Every kind counts: a route, trip or weather upload holds the store
        // exactly as a map does, and none of those three raises the #927 progress card this level
        // used to be derived from.
        let transferring = crate::flat_store::transfer_active();
        exec.facts.note_transfer(if transferring {
            obc_app::device_core::TransferState::Active
        } else {
            obc_app::device_core::TransferState::Idle
        });
        // The level's own edge, on RTT. `debug-uart` only, exactly like the freeze banner's line:
        // the soak rig is its only reader and the shipping image should not carry the string. It is
        // the *only* witness a route/trip/weather upload now moves this level — none of those three
        // raises the #927 card the level used to be derived from, so without this line the closing
        // of S5 open question 2 is unobservable on glass.
        #[cfg(feature = "debug-uart")]
        if transferring != prev_transferring {
            prev_transferring = transferring;
            defmt::info!("xfer: transfer level {=str} (flat engine)", if transferring { "active" } else { "idle" });
        }

        // ── Sensor presence → warning (issue #504), real-sensor build, once ──
        // The sensor task publishes its boot I²C probe result a moment after boot; map any chip that
        // didn't answer to a dismissable warning card. `try_take` yields once, so this fires a single
        // pass; an empty flag set is a no-op.
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
            exec.facts.raise_warnings(w);
        }

        // ── BLE → app seam (epic #447), FEEDING half: everything that hands the app a value the
        // pass below reads. The half that *acts on* what the pass decided runs after it, inside the
        // store phase — that is the only reordering the typed cutover forced here. ──
        {
            // The link snapshot (connected + passkey) as a **level**: the pass compares it against
            // what it last saw and calls `set_ble_status` only on a change, so a steady state
            // dirties nothing.
            exec.facts.note_link(crate::ble::app_ble_status());
            // The weather due plane's IN_FLIGHT level (#1549). A level and not an operation's
            // answer: the `weather_refresh` cadence raises fetches nobody ordered, and the rider is
            // owed the UPDATING cue for those too. Read once per pass, so both edges reach the
            // domain — the plane sets it on a raise and clears it on a commit or a lapse.
            exec.facts.note_weather_refreshing(crate::ble::weather_refreshing());
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
            crate::link::set_recording(app.recording());
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
        //
        // This block is a **feed**, so it runs ahead of the pass that applies the gestures — which
        // means the press that pops the card is observed on the frame *after* it happens. Benign,
        // and it is the shape the latch was built for: it observes rather than being told, and a
        // terminal state is never re-fed once cleared.
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

            let wants_stage = crate::usb::stage_requested();
            if wants_stage && usb_stage_guard.is_none() {
                if let Some(ready) = app.usb_stage_precondition() {
                    if let Ok(guard) = crate::arena::claim_usb(ready) {
                        usb_stage_guard = Some(guard);
                        crate::usb::set_stage_granted(true);
                        defmt::info!("arena: 64 KiB USB write-combining arm granted");
                    }
                }
            } else if !wants_stage && usb_stage_guard.is_some() {
                usb_stage_guard = None;
                crate::usb::set_stage_granted(false);
                defmt::info!("arena: USB write-combining arm reclaimed");
            }
        }

        // ── BLE → app seam, FEEDING half (continued): the sensor snapshots the pass reads ──
        // The *requests* that keep discovery alive and reconcile the saved slots are the acting
        // half, and they moved after the pass with #1397 S6b: both key on screen and settings state
        // this frame's gestures produce, and gestures are applied inside the pass now.
        {
            // The scan list's hits, as a feed: the manager's snapshot into the app, so a wake for
            // any reason renders what discovery has found so far.
            if app.sensor_scan_active() {
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
                app.set_sensor_scan_hits(&[]);
            }
            // Push the per-slot status snapshot (the Sensors screen's row status lines).
            let sensor_status = [sensor_status_of(0), sensor_status_of(1), sensor_status_of(2)];
            app.set_sensor_status(&sensor_status);
        }

        // This frame's hold-bulge state, sampled once: the live row span the present goes around and
        // the bulge re-push is driven from.
        //
        // The bulge pushes **first in the pass**, before the store lock, the SD reconcile, and any
        // screen redraw (#348 follow-up, widened here): a fired hold usually navigates — and a
        // fired *Finish* triggers the ride save — so with the bulge later in the pass its confirm
        // pop queued behind the new screen's render (~40–300 ms) or, worse, the whole SD save,
        // and the 220 ms pop expired unseen ("sometimes it just snaps"). Bulge-first, the pop's
        // attack lands on glass within ~10 ms of the fire — composited over the *old* fb for that
        // one frame, which is correct: that is what is on glass until the present below.
        let overlay_span = display.poll_overlay();
        display.present_bulge(overlay_span).await;

        // ── The staged DFU effect (epic #615 S4/S5), served BEFORE the store phase ──
        //
        // `DfuEffect` is the one effect the board deliberately serves a pass *late*: the guard-free
        // block it belongs in is here, ahead of the store phase (#809 — the "Installing update"
        // card's present must not hold the store guard, or a BLE object operation queues behind a
        // ~44 ms FLPR scan). One pass of latency on an operation that ends in a reboot is not a
        // cost; moving this block under the guard would be. The outcome is delivered into the inbox
        // and consumed by *this* frame's pass, so the confirm/error card still lands in one frame.
        //
        // The `dfu-install` debug command reaches the same path by naming the intent to `DfuState`
        // rather than reaching for the executor — so it produces the same `ArmInstall` under the
        // same operation token the confirm screen's press does.
        #[cfg(feature = "debug-uart")]
        if obc_platform::debug_link::take_dfu_install() {
            app.debug_request_dfu_install();
        }
        if let Some(effect) = exec.effects.dfu.take() {
            use obc_app::dfu::{DfuEffect, DfuOutcome};
            match effect {
                DfuEffect::ArmInstall { token } => {
                    // The irreversible arm-and-reboot. Guards mirror what the System menu greys out:
                    // never mid-recording (the arm ends in a reboot — a live ride would be lost) and
                    // never while the flat recorder still owns an active `RECORDING` object. A
                    // refusal is a typed reason and lands the error card (issue #755) so the
                    // confirm's "Preparing update..." spinner can't strand the rider. The `D`-line
                    // breadcrumbs name the guard that refused (the field-debugging motivation).
                    let refusal = {
                        // One short store guard: just the go/no-go checks.
                        let store_guard = shared.lock().await;
                        if app.recording() || ride_recorder.is_recording() {
                            crate::dfu::status("refused (is_tracking): a ride is recording -- finish it first");
                            Some(obc_app::DfuInstallError::Recording)
                        } else if store_guard.storage.is_none() {
                            crate::dfu::status("refused (no_card): no SD card");
                            Some(obc_app::DfuInstallError::NoCard)
                        } else {
                            None
                        }
                    };
                    match refusal {
                        Some(error) => {
                            RideExec::deliver(&mut exec.outcomes.dfu, DfuOutcome::InstallFailed { token, error }, "dfu")
                        }
                        None => {
                            // The guards passed: answer the arm **now** so this frame's pass swaps
                            // the confirm's "Preparing update..." spinner for the static
                            // "Installing update" card, and let the ordinary render + present put
                            // that frame on glass. The arm itself runs in the store tail, after the
                            // present — the warm reset into the bootloader never paints (it only
                            // parks the panel pins and keeps the COM wave alternating,
                            // `obc-boot/src/com.rs`), so the MIP holds THAT frame for the whole
                            // snapshot + flash.
                            //
                            // This replaces a hand-rolled render/present pair inside the effect with
                            // the frame the loop already knows how to produce, and it is what makes
                            // the guard-free present a property of the phase rather than of this
                            // block remembering to release the lock.
                            RideExec::deliver(&mut exec.outcomes.dfu, DfuOutcome::InstallBegan { token }, "dfu");
                            exec.arm_pending = Some(0);
                        }
                    }
                }
                DfuEffect::Scan { token } => {
                    // The UI's read-only "Checking card..." step: validate `UPDATE.BIN` and answer
                    // the app (the wait screen swaps to the confirm or an error card). No card ⇒
                    // report the update file as missing. The scan touches nothing, so no ride-state
                    // guard is needed (the menu greys the row mid-ride anyway). One short store
                    // guard of its own.
                    let result = {
                        let mut store_guard = shared.lock().await;
                        let SharedStore { storage, settings: settings_store } = &mut *store_guard;
                        match storage.as_mut() {
                            Some(s) => crate::dfu::run_scan(s, settings_store, &mut wdt),
                            None => Err(obc_app::DfuScanError::NotFound),
                        }
                    };
                    // DR6 (#734): park the validated ref for the confirm's Install; answer the app
                    // with just the report. A failed scan clears any prior ref (the card may have
                    // changed).
                    let outcome = match result {
                        Ok((report, staged)) => {
                            cached_staged = Some(staged);
                            DfuOutcome::ScanFinished { token, report }
                        }
                        Err(error) => {
                            cached_staged = None;
                            DfuOutcome::ScanFailed { token, error }
                        }
                    };
                    RideExec::deliver(&mut exec.outcomes.dfu, outcome, "dfu");
                }
            }
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
        let (rendered, dirty_map, hold_p, next_wake_ms, immediate, store_held_us) = {
            let mut store_guard = shared.lock().await;
            let t_store = Instant::now();
            let SharedStore { storage, settings: settings_store } = &mut *store_guard;

            // ═══ The staged effects, each in the physical phase that already owns it ═══
            //
            // These are the bounded operations the **previous** pass decided. They run at the top of
            // the store phase because that is where the work they name has always lived: a catalog
            // re-read has to precede this frame's route source/index build (it closes and reopens the
            // held revision, so a reader built before it would outlive its source), and the settings
            // write needs the store this block holds.
            if let Some(effect) = exec.effects.catalog.take() {
                use obc_app::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
                match effect {
                    // The rescan block: rebuild the flat route/trip/ride identities and remap the
                    // app's held indices by durable ObjectId.
                    CatalogEffect::ReadCatalog { token } => {
                        let read =
                            read_catalogs(flat, app, &mut exec.facts, &mut weather_bundle, &mut weather_sample_key);
                        prev_active = None; // force reconcile_route/track to re-run against the new indexing
                        index_route = None; // and the chunk index to rebuild off the freshly-opened file

                        // A partial read is answered `Unreadable`, and the **domain** re-offers the read from there
                        // (#1541) — one per pass, which is one per wake.
                        let outcome = if read {
                            CatalogOutcome::CatalogRead { token }
                        } else {
                            CatalogOutcome::Failed { token, error: CatalogError::Unreadable }
                        };
                        RideExec::deliver(&mut exec.outcomes.catalog, outcome, "catalog");
                    }
                    // The rider's removal — a route, a ride, an expiry, or one step of a trip
                    // cascade — on the **answering** writer path. The effect is namespace-free (FS7
                    // numbers every object out of one id space), so the store resolves the head at
                    // that id and reports whether it was there. The cascade's member-then-folder
                    // order is `CatalogMachine`'s, and arrives here as one removal at a time
                    // (#1491), so this executor composes nothing.
                    //
                    // A full request queue is not an answer: the effect simply was not taken this
                    // pass, so the domain re-offers it.
                    CatalogEffect::RemoveObject { token, object } => {
                        match crate::flat_store::writer().ok_or(()).and_then(|w| {
                            w.try_call(
                                crate::flat_store::Request::RemoveObject { id: obc_storage::flat::ObjectId(object) },
                                &CATALOG_STORE_REPLY,
                            )
                        }) {
                            Ok(ticket) => exec.catalog = Some(CatalogRemoval { ticket, token, object }),
                            Err(()) => {
                                defmt::warn!(
                                    "flat: removal of object {=u64} could not be queued — the domain re-offers it",
                                    object
                                );
                                RideExec::deliver(
                                    &mut exec.outcomes.catalog,
                                    CatalogOutcome::Failed { token, error: CatalogError::RemoveFailed },
                                    "catalog",
                                );
                            }
                        }
                    }
                }
            }

            // The in-flight removal's answer, polled without parking. `existed: false` is a
            // **success** — the subject vanished before the commit and the goal state holds (#1433
            // §13) — while a refused or failed commit is a `Failed` the domain re-queues.
            if let Some(removal) = exec.catalog.take() {
                use obc_app::catalog_state::{CatalogError, CatalogOutcome};
                let answer =
                    crate::flat_store::writer().and_then(|w| w.try_result(removal.ticket, &CATALOG_STORE_REPLY));
                match answer {
                    None => exec.catalog = Some(removal),
                    Some(Ok(crate::flat_store::Outcome::Removed { existed })) => {
                        defmt::info!("catalog: object {=u64} removed (existed {=bool})", removal.object, existed);
                        RideExec::deliver(
                            &mut exec.outcomes.catalog,
                            CatalogOutcome::ObjectRemoved { token: removal.token, object: removal.object, existed },
                            "catalog",
                        )
                    }
                    Some(_) => {
                        defmt::warn!("catalog: object {=u64} removal failed — the domain re-queues it", removal.object);
                        RideExec::deliver(
                            &mut exec.outcomes.catalog,
                            CatalogOutcome::Failed { token: removal.token, error: CatalogError::RemoveFailed },
                            "catalog",
                        )
                    }
                }
            }

            // The System settings screen's card-free scan (T8 item 6): one bounded FAT free-cluster
            // read off the card. `None` — no mounted card, or no FSInfo free count — is a
            // measurement that produced no figure, and `StorageInfo` blanks the row back to `--`
            // rather than leaving a byte count from a card that may no longer be in the device.
            if let Some(effect) = exec.effects.storage_info.take() {
                use obc_app::device_core::{StorageInfoEffect, StorageInfoError, StorageInfoOutcome};
                let StorageInfoEffect::MeasureFreeSpace { token } = effect;
                let outcome = match storage.as_ref().and_then(|s| s.card_free_bytes()) {
                    Some(free_bytes) => StorageInfoOutcome::Measured { token, free_bytes },
                    None => StorageInfoOutcome::Failed { token, error: StorageInfoError::NotMounted },
                };
                RideExec::deliver(&mut exec.outcomes.storage_info, outcome, "storage");
            }

            // ── The staged recording operation (#1398) ──
            //
            // One bounded card operation, exactly like the catalog's above it, and it decides
            // nothing: Recorder chose the checkpoint cadence and what "closed" means, and this
            // reports whether the store did it. A failure is a **typed reason**, so the ride stays
            // open and Recorder re-offers the same operation rather than the rider losing it.
            if let Some(effect) = exec.effects.recorder.take() {
                use obc_app::recorder::{RecorderEffect, RecorderError, RecorderOutcome, RideClose};
                let outcome = match effect {
                    RecorderEffect::Checkpoint { token } => {
                        let stats = app.recorder.ride_stats();
                        let continuation = app.recorder.continuation();
                        match ride_recorder.checkpoint(now, &stats, continuation).await {
                            true => RecorderOutcome::Checkpointed { token },
                            false => RecorderOutcome::Failed { token, error: RecorderError::Write },
                        }
                    }
                    RecorderEffect::Finalize { token } => {
                        // The samples this ride staged and no append has taken yet go into the
                        // bounded tail **before** the footer: they belong to the ride being saved,
                        // and the totals the footer carries already count the distance and the
                        // moving time they cover. This is inside the close's own service, so it
                        // orders nothing against the append rank.
                        for point in app.recorder.staged() {
                            if !ride_recorder.append(*point) {
                                break; // the tail refused it; the footer is still the honest total
                            }
                        }
                        // The footer facts come from Recorder, which stamped its wall-clock anchor
                        // as it minted this close. The save name is not read at all: it was frozen
                        // when the ride opened.
                        let stats = app.recorder.ride_stats();
                        match ride_recorder.finalize(&stats).await {
                            RideClose::Committed(ride) => RecorderOutcome::Finalized { token, ride },
                            RideClose::Nothing => {
                                defmt::warn!("flat ride: finalize with no open object — the ride was never created");
                                RecorderOutcome::Discarded { token }
                            }
                            RideClose::Failed => RecorderOutcome::Failed { token, error: RecorderError::Write },
                        }
                    }
                    RecorderEffect::Discard { token } => match ride_recorder.discard().await {
                        true => RecorderOutcome::Discarded { token },
                        false => RecorderOutcome::Failed { token, error: RecorderError::Write },
                    },
                    // The staged samples into the bounded tail, in order, for as long as the
                    // recorder keeps taking them. A short write is answered honestly: Recorder keeps
                    // the tail staged and offers it again, so a refusal costs a delay rather than a
                    // hole in the ride log. Nothing written at all is a failure, which is what
                    // raises the recording warning.
                    RecorderEffect::Append { token, samples } => {
                        let staged = app.recorder.staged();
                        let want = (samples as usize).min(staged.len());
                        let mut written = 0u16;
                        while (written as usize) < want && ride_recorder.append(staged[written as usize]) {
                            written += 1;
                        }
                        match written {
                            0 if want > 0 => RecorderOutcome::Failed { token, error: RecorderError::Write },
                            _ => RecorderOutcome::Appended { token, samples: written },
                        }
                    }
                };
                if ride_recorder.take_warning() {
                    exec.facts.raise_warnings(obc_app::WarningFlags::REC_ERROR);
                }
                RideExec::deliver(&mut exec.outcomes.recorder, outcome, "recorder");
            }

            // The domains with no board executor at all. Each is answered rather than dropped, so a
            // domain that starts producing one cannot wedge behind an executor that ignored it —
            // and the loud line names the slice that owes it.
            if exec.effects.retention.take().is_some() {
                defmt::error!(
                    "retention: a sidecar write reached the board — PlatformSupport::retention_metadata is false"
                );
                debug_assert!(false, "no retention effect is produced without a metadata store");
            }
            if exec.effects.bond.take().is_some() {
                defmt::error!("bond: the removal is the residual ForgetBond command (#1398/#1400)");
                debug_assert!(false, "BondEffect has no producer");
            }
            // ── The Ride detail's track profile and the route overview's shape (#678 T2 / #680) ──
            // Answered below, immediately before the pass that consumes them — see the derived fill.

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
            // `NavigatorEffect` — the board paces its own search (#1400,
            // Gate 4/#1400): one `Acquire` arms the run, the block below runs **one** bounded step
            // per pass, and the answer is terminal. `Step` and `CommitRoute` are never produced.
            #[allow(unused_mut, unused_assignments)]
            let mut nav_cancel = false;
            if let Some(effect) = exec.effects.navigator.take() {
                use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlannerWork};
                match effect {
                    #[cfg(has_nav)]
                    NavigatorEffect::Acquire { token, work: PlannerWork::Route(request) } => {
                        // Since #1146 P2 the planner slot lives in the scratch arena, so the search
                        // must *take* the arena first — and a cable transfer streaming into the same
                        // store outranks a reroute, which `nav_take_arena` enforces by asking the
                        // arena who holds it. A refusal names the holder and answers the operation,
                        // so no spinner hangs behind a half-claim and the freeze comes off.
                        let refusal = match nav_take_arena(app, &mut nav_guard) {
                            Err(why) => Some(why),
                            // Impossible through the UI (the planning screen blocks a second
                            // confirm), and fail-closed rather than lending one reply slot to two
                            // live tickets: refuse the *new* operation so the rider gets the failure
                            // card instead of a spinner nothing will ever resolve.
                            Ok(()) if nav_run.is_some() => {
                                debug_assert!(false, "a second route plan arrived while one was active");
                                if let Some(run) = nav_run.as_mut() {
                                    run.cancel_requested = true;
                                }
                                Some("a plan is already running")
                            }
                            Ok(()) => {
                                let mut bufs = NavBuffers {
                                    guard: nav_guard.as_mut().expect("nav_take_arena left the guard held"),
                                    elev: &mut *nav.elev,
                                };
                                nav_begin(&mut bufs, &request, app.settings().bike_profile_idx);
                                exec.nav_token = Some(token);
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
                                None
                            }
                        };
                        if let Some(why) = refusal {
                            defmt::warn!("nav: cannot start a plan ({=str}) — refusing the operation", why);
                            RideExec::deliver(
                                &mut exec.outcomes.navigator,
                                NavigatorOutcome::Failed { token, error: NavigatorError::Workspace },
                                "navigator",
                            );
                        }
                    }
                    // The `ble` image ships without the router (the 256 KB DK's statics), so the
                    // workspace this operation asks for does not exist in it.
                    #[cfg(not(has_nav))]
                    NavigatorEffect::Acquire { token, work: PlannerWork::Route(_) } => {
                        defmt::warn!("nav: router not built into the ble image (256K DK) — refusing the operation");
                        RideExec::deliver(
                            &mut exec.outcomes.navigator,
                            NavigatorOutcome::Failed { token, error: NavigatorError::Workspace },
                            "navigator",
                        );
                    }
                    // `PlatformSupport::detour = false` on this board: #882's splice holds the
                    // planned detour's OBCR in RAM until the rider commits, and the board has one
                    // flat-store reservation and no heap to read it from while it writes. That is
                    // Gate 4 item 5 (#1400).
                    //
                    // **This arm is reachable today**, and says so rather than asserting otherwise:
                    // nothing consults `Capabilities::navigator` yet, so the ride menu's Detour row
                    // is still live and a rider pressing it produces this operation. It is refused
                    // — the capability is absent, not the path — and the refusal must not read as a
                    // fault: an unanswered one would wedge the Recalculating freeze for the rest of
                    // the ride, and a fabricated `NoPath` would tell the rider the device searched.
                    // A `warn`, not an `error`, because a rider pressing a row the firmware still
                    // offers is an expected condition; the alarm class is for effects nobody should
                    // ever have decided.
                    NavigatorEffect::Acquire { token, work: PlannerWork::Detour(_) }
                    | NavigatorEffect::CommitDetour { token } => {
                        defmt::warn!("nav: detour is not supported on this board — refusing the operation");
                        RideExec::deliver(
                            &mut exec.outcomes.navigator,
                            NavigatorOutcome::Failed { token, error: NavigatorError::Workspace },
                            "navigator",
                        );
                    }
                    // The rider walked away. The run itself needs a pass or two to hand its
                    // reservation back; the *operation* is over now, and Navigator released its
                    // search level when it minted this effect.
                    NavigatorEffect::Release { token } => {
                        nav_cancel = true;
                        RideExec::deliver(
                            &mut exec.outcomes.navigator,
                            NavigatorOutcome::Released { token },
                            "navigator",
                        );
                    }
                    NavigatorEffect::Step { .. } | NavigatorEffect::CommitRoute { .. } => {
                        defmt::error!("nav: the executor paces the search — stepped pacing is #1400's");
                        debug_assert!(false, "Step/CommitRoute have no producer yet");
                    }
                }
            }

            #[cfg(has_nav)]
            {
                // Whether this pass ended the search — the one place the arena's nav arm is given
                // back. A flag rather than an inline release because the guard is borrowed by the
                // step view below and must die first.
                let mut search_ended = false;
                // A `Release` and an `Acquire` can never arrive in one pass — the plan's navigator
                // slot holds exactly one effect, and Navigator offers releases before new work — so
                // the old `&& !plan_armed` guard against a cancel closing the *new* run's file is
                // now a property of the protocol rather than a rule this block has to remember.
                if nav_cancel {
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
                                use obc_app::host::{
                                    nav_compensation_disposition, NavCompensationDisposition as Disposition,
                                    NavCompensationStatus as Status,
                                };
                                let status = match answer {
                                    Ok(crate::flat_store::Outcome::Done) => Status::Removed,
                                    Err(obc_storage::flat::StoreError::NotFound) => Status::Absent,
                                    Err(obc_storage::flat::StoreError::Media | obc_storage::flat::StoreError::Busy) => {
                                        Status::Retry
                                    }
                                    // The exact id/revision makes every other refusal permanent or
                                    // an invariant violation. Most importantly, ReadOnly cannot be
                                    // repaired in-session. Publish reserved a second sequence, so
                                    // seeing it here means another writer consumed that last slot.
                                    _ => Status::Terminal,
                                };
                                match nav_compensation_disposition(status) {
                                    Disposition::Cancelled => cancelled = true,
                                    Disposition::Retry => {
                                        defmt::warn!(
                                            "nav route: cancellation compensation for object {=u64} hit transient media failure — retrying",
                                            id.0
                                        );
                                        run.io = NavIo::NeedPublishCompensation(id);
                                    }
                                    Disposition::CancelledAfterTerminalFailure => {
                                        // Never report route success, and never retain NavGuard/the
                                        // planner arena forever. This should be unreachable under
                                        // PublishComputedRoute's two-sequence admission invariant.
                                        defmt::error!(
                                            "nav route: terminal cancellation compensation failure for object {=u64}; releasing planner",
                                            id.0
                                        );
                                        cancelled = true;
                                    }
                                }
                            }
                        }
                    }
                    use obc_app::navigator::{NavigatorError, NavigatorOutcome};
                    if cancelled {
                        defmt::info!("nav route: cancelled after {=u64} ms", run.t0.elapsed().as_millis());
                        // The operation was already answered `Released` when the cancellation
                        // reached this executor; the run winding down owes nothing further.
                        exec.nav_token = None;
                        search_ended = true;
                    } else if let Some(result) = finished {
                        let mut bufs = NavBuffers { guard, elev: &mut *nav.elev };
                        nav_finish(&mut bufs, run, result, now);
                        // The terminal answer, under the operation the rider actually started. A run
                        // with no token is a cancelled one whose `Released` already landed — the
                        // domain would refuse a second answer anyway, so say nothing.
                        if let Some(token) = exec.nav_token.take() {
                            let outcome = match result {
                                Ok((route, _)) => NavigatorOutcome::PlanFinished { token, route },
                                Err(error) => NavigatorOutcome::Failed { token, error: NavigatorError::Plan(error) },
                            };
                            RideExec::deliver(&mut exec.outcomes.navigator, outcome, "navigator");
                        }
                        search_ended = true;
                        prev_active = None;
                        index_route = None;
                    } else {
                        nav_run = Some(run);
                    }
                } else if nav_run.is_some() {
                    // The one writer went away under a live run (a card that stopped answering).
                    use obc_app::navigator::{NavigatorError, NavigatorOutcome};
                    if let Some(token) = exec.nav_token.take() {
                        RideExec::deliver(
                            &mut exec.outcomes.navigator,
                            NavigatorOutcome::Failed { token, error: NavigatorError::Store },
                            "navigator",
                        );
                    }
                    nav_run = None;
                    search_ended = true;
                }
                if search_ended {
                    // Drop the guard, releasing the arena so the next frame can render the map
                    // again — and so a transfer waiting on it finds the block free. The app's own
                    // search level was released by the answer that set `search_ended`.
                    nav_guard = None;
                }
            }
            #[cfg(not(has_nav))]
            {
                let _ = nav_cancel; // no plan can be in flight — the release is inert here
                let _: &NavResident = &nav; // the unit stand-in — nothing to plan with
                                            // An `Acquire` was already refused above (the `ble` image ships no router).
            }

            // Settings coherence, phone → device (#456): a BLE Config write persisted units + name to
            // RRAM but the live `App` copy never learned. Reload the BLE-owned fields into it *before*
            // the change-detection save below, so (a) the UI re-captions same-session and (b) the app's
            // `==`-diff save can't clobber the phone's write with its own stale copy. Only units + name
            // are BLE-writable, so the merge is narrow (`adopt_ble_fields`) — a device-only edit pending
            // this frame is untouched. Board-crate flag, drained once per BLE write; a no-op otherwise.
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
            if let Some(effect) = exec.effects.settings.take() {
                use obc_app::settings::{SettingsEffect, SettingsOutcome};
                let outcome = match effect {
                    SettingsEffect::PersistRevision { token, revision } => match settings_store.save(app.settings()) {
                        Ok(()) => {
                            // Settings coherence, device → phone (#456): the RRAM blob just moved, so the BLE
                            // config-read cache is stale — flag it so the BLE plane refreshes from RRAM before
                            // its next Config read / advertised-name read. One relaxed store.
                            crate::object_store::mark_device_settings_changed();
                            // Push a changed GPS fix interval to the sensor task → it re-VALSETs the M10's rate.
                            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
                            if app.settings().fix_interval_s != prev_interval {
                                prev_interval = app.settings().fix_interval_s;
                                control.set_rate(prev_interval);
                            }
                            SettingsOutcome::Persisted { token, revision }
                        }
                        Err(error) => SettingsOutcome::PersistFailed { token, revision, error },
                    },
                    // The alert-mark record (#1542): its own 64-byte line, at alert-fire rate. No
                    // BLE cache to invalidate and no GPS rate to re-push — the phone never reads
                    // these bytes and no screen edits them.
                    SettingsEffect::PersistAlertMarks { token, revision } => {
                        match settings_store.save_alert_marks(app.alert_marks()) {
                            Ok(()) => SettingsOutcome::MarksPersisted { token, revision },
                            Err(error) => SettingsOutcome::MarksPersistFailed { token, revision, error },
                        }
                    }
                };
                RideExec::deliver(&mut exec.outcomes.settings, outcome, "settings");
            }
            // The weather refresh (#1549): **raise** a request with the due plane, and answer that
            // and nothing more. What comes back is reported as the installed-data fact when
            // `read_catalogs` next sees a new head; whether a fetch is running is the plane's own
            // level. Without a companion there is nothing to raise it with — but the capability
            // gate means this arm is only reached while one is connected, so a raise here always
            // lands and `Failed { LinkLost }` stays the shape a host with no radio answers with.
            if let Some(obc_app::weather::WeatherEffect::RequestRefresh { token }) = exec.effects.weather.take() {
                crate::ble::request_weather_now();
                defmt::info!("weather: the dashboard was opened — urgent phone fetch raised");
                RideExec::deliver(
                    &mut exec.outcomes.weather,
                    obc_app::weather::WeatherOutcome::Raised { token },
                    "weather",
                );
            }
            // Every effect this frame carried has now been offered a home. Anything left is a
            // domain with no board executor at all, and saying so loudly is what stops it becoming a
            // silent wedge.
            if exec.effects.has_pending() {
                defmt::error!("exec: an effect this board cannot serve was decided");
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

            // ── The residual legacy drain: one command, and the shared list says which ──
            //
            // `ForgetBond` is the class whose domain cannot validate an operation token, so it
            // cannot own an outcome (epic #1433 §4.3) — the one every typed executor still drains,
            // pinned by `obc_app::device_core::residual`. The ride close left with #1398: it is a
            // `RecorderEffect` served in the store phase and answered with a `RecorderOutcome`.
            //
            // **Asked for by name**, and that is load-bearing rather than tidy: the whole-order
            // A whole-order walk *pulls* from every domain it passes — it mints the operation
            // as it goes — so it would take an intent admitted since this frame's pass and hand back
            // a command this loop then declines to perform, leaving the domain holding an operation
            // nobody answers. Everything running between here and `run_pass` is exposed to that: the
            // debug link's route plan, the phone's remote update check, a BLE clock stamp arming a
            // settings write.
            //
            // The predicate below stays as belt and braces. Anything that still reaches it is a
            // class DeviceCore owns, and running it beside the effect that carries it would do the
            // work twice — so the board reports and skips rather than panicking mid-ride.
            //
            // The mailbox — a ~600 B `Deque<HostCommand>` — is a stack temporary scoped to this
            // block and dropped at its close, before the reconcile's `.await`, so it never enters
            // the ride-loop task future (it would re-inflate the #808 poll frame).
            {
                use obc_app::device_core::residual::residual;
                let mut mailbox: obc_app::HostMailbox = obc_app::HostMailbox::new();
                let _ = app.drain_residual_commands(&mut mailbox);
                while let Some(cmd) = mailbox.pop() {
                    if !residual(&cmd) {
                        defmt::error!(
                            "exec: {} came back on the legacy protocol — DeviceCore owns it now, so it is skipped",
                            defmt::Debug2Format(&cmd)
                        );
                        debug_assert!(false, "the residual is ForgetBond");
                        continue;
                    }
                    match cmd {
                        // The bond removal is confirmed by a link-status fact, never by a reply
                        // (#1400).
                        obc_app::HostCommand::ForgetBond => crate::ble::request_forget_bond(),
                    }
                }
            }

            // Point the card at the active route's geometry, and open a ride object for the session
            // Recorder decided on. Gated on the edges that can change either — a route swap, or a
            // session with no object yet — so the dominant static frame does no per-tick
            // `String<64>` copy.
            //
            // **The owed object is named by id, and on this loop that is load-bearing.** A close is
            // served at the top of this iteration but its verdict is applied by the pass at the
            // *end* of it, so right here `app.ride_session()` is still `Some(N)` while the object it
            // named is already gone. A gate that asked "is a session open and nothing recording"
            // could not tell that apart from "the start failed, retry", and would allocate a fresh
            // 32 MiB `RECORDING` object under the closing ride's identity — never closed, refusing
            // every later DFU install, and surfacing as a bogus recovered ride at the next boot.
            // `object_owed` compares the id the executor has already opened one for, so a served
            // close owes nothing and a failed start still retries. Closing is not here at all: it
            // is a `RecorderEffect`, served in the store phase above.
            let owed = app.recorder.object_owed(opened_session);
            if active != prev_active || owed.is_some() {
                let mut name: heapless::String<64> = heapless::String::new();
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    let _ = name.push_str(&r.name);
                }
                let active_id = active.and_then(|i| app.route_ids().get(i).copied());
                crate::flat_store::reconcile_route(flat, active_id);
                if let Some(id) = owed {
                    ride_recorder.open(flat, id, &name, now).await;
                    // A start the card refused leaves no object, so the id stays unclaimed and the
                    // next iteration retries it. A start that took claims it once and for all.
                    if ride_recorder.open_session() == Some(id) {
                        opened_session = Some(id);
                    }
                }
                if ride_recorder.take_warning() {
                    exec.facts.raise_warnings(obc_app::WarningFlags::REC_ERROR);
                }
                prev_active = active;
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
            // ═══ The pass, and everything that only exists for it ═══
            //
            // **One tight scope that ends before the next `.await`.** The 512 B polyline buffer, the
            // gesture batch and the `PassPlan` itself are stack temporaries here; a binding still
            // live across an await would instead become a permanent slot in this task's future
            // (#808/#1084 — `run_app` is `#[inline(always)]` into `__embassy_main`, whose task
            // storage is `.bss`, so "in the future" and "resident" are the same thing).
            let plan = {
                // ── The keyed derived reads (#1437), answered immediately before the pass ──
                //
                // Into that stack buffer, handed straight into `PassInputs::targets`: at
                // `NAV_PREVIEW_MAX` a polyline is 512 B and the board's resident headroom is two
                // orders of magnitude smaller, so no executor-owned copy may exist.
                //
                // **At most one read per pass.** The need is a level and re-emits, so a second want
                // lands a pass later — one bounded read per frame is the pacing this board already
                // had. Both reads are flat-store reads and take no guard.
                let mut derived_pts: heapless::Vec<(i32, i32), { obc_app::NAV_PREVIEW_MAX }> = heapless::Vec::new();
                let mut derived = obc_app::device_core::DerivedInputs::NONE;
                if let Some(key) = exec.needs.ride_track {
                    // The Ride detail's track profile + shape (#678 T2 / #680): stream the flat ride
                    // object once into the app's resident profile buffer and this frame's stack buffer.
                    let filled = crate::flat_store::fill_ride_track(flat, app, key.ride, &mut derived_pts);
                    // The ~5 KB profile is filled **in place**, which invalidates the view — so the
                    // `view` generation the answer carries has to be the one the need has *after*
                    // the fill, not before, or the domain would reject its own executor's answer.
                    //
                    // The **subject** is the opposite: it must stay the one that was actually read.
                    // `accept_ride_profile`'s only staleness guard is that the answer's key equals
                    // the need's, so minting the whole key from the *current* need would make that
                    // check vacuous here — and it is reachable, not theoretical: `read_catalogs`
                    // ran earlier in this same store phase and remaps the held indices by durable
                    // id, so a delete landing between the plan and this fill can move the viewed
                    // ride. Answering under the new ride's key would put ride A's profile and
                    // polyline on ride B's detail screen. A moved subject (or fresh bytes under it)
                    // is answered by *not answering*: the need re-emits and the next pass reads the
                    // ride the rider is actually looking at.
                    match app.derived_needs().ride_track {
                        Some(now) if now.ride == key.ride && now.source == key.source => {
                            derived.ride_track = Some(if filled {
                                obc_app::device_core::DerivedInput::filled(now)
                            } else {
                                obc_app::device_core::DerivedInput::failed(now)
                            });
                        }
                        _ => defmt::info!("derived: the ride-track subject moved under the read — re-asking next pass"),
                    }
                } else if let Some(key) = exec.needs.nav_preview {
                    // The Route overview's shape preview (#685 §4; widened to stored routes by #678
                    // rework 3's track/elevation pager). The previewed route is the active one, and its
                    // reader was built just above — a computed plan's finish forced this frame's index
                    // rebuild, and a stored route's overview entry pointed `active_route` at it.
                    //
                    // Answered either way: a failure *is* an answer (a dead file must cost one read, not
                    // one per pass), so an unreadable route settles with no shape instead of re-firing
                    // the level forever.
                    derived.nav_preview = Some(match route.as_ref() {
                        Some(r) => {
                            let _ =
                                derived_pts.extend_from_slice(&r.preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>());
                            obc_app::device_core::DerivedInput::filled(key)
                        }
                        None => obc_app::device_core::DerivedInput::failed(key),
                    });
                }
                let targets = if exec.needs.ride_track.is_some() {
                    obc_app::device_core::DerivedTargets { ride_preview: derived_pts.as_slice(), nav_preview: &[] }
                } else {
                    obc_app::device_core::DerivedTargets { ride_preview: &[], nav_preview: derived_pts.as_slice() }
                };

                // ── This frame's chords, above the screen stack ──
                // A drawer opens or closes before the gestures below are handed to whatever screen
                // it left on top. The constituents were swallowed by the recogniser, so nothing
                // here can also move the selection underneath.
                while let Ok(chord) = CHORDS.try_receive() {
                    let acted = app.apply_chord(chord);
                    defmt::info!(
                        "input: chord {=str} on {=str} ({=str})",
                        chord_name(chord),
                        app.top_screen().name(),
                        if acted { "opened/closed" } else { "refused" }
                    );
                }

                // ── This frame's gestures, as one batch ──
                // The high-priority plane recognised them; the pass applies them in order and owns
                // #480's rule that a `Hold`/`BackHold` queued behind a stack-changing gesture is dropped
                // rather than delivered to the screen that replaced its target. Collected here, with no
                // `.await` between the collect and the pass, so the batch is a stack temporary.
                let mut gestures: heapless::Vec<obc_app::Gesture, { crate::input_plane::GESTURE_QUEUE }> =
                    heapless::Vec::new();
                while let Ok(g) = GESTURES.try_receive() {
                    // Field forensics (#755): every drained gesture, with the screen it lands on — the
                    // RTT record that discriminates "the press never happened" (input-plane dead window)
                    // from "the press landed on the wrong screen/row". Human-rate events; always on.
                    if let obc_app::Gesture::Step(n) = g {
                        defmt::info!("input: Step {=i32} on {=str}", n, app.top_screen().name());
                    } else {
                        defmt::info!("input: {=str} on {=str}", gesture_name(g), app.top_screen().name());
                    }
                    if gestures.push(g).is_err() {
                        defmt::warn!("input: {=str} dropped — the frame's gesture batch is full", gesture_name(g));
                    }
                }
                // ═══ **One `App::run_pass` per frame** (#1433 §6, #1397 S6b) ═══
                //
                // Run here — in place of the `app.tick` this replaces — because this is the only point
                // where the sensors and the route reader are both live at once.
                // Fourteen stages in, one bounded `PassPlan` out. Three builds: the VCOM-streamed GPS +
                // altimeter + compass (`debug-uart`); the real SAM-M10Q + BMP581, coherent per fix
                // (default); or the SynthLocation square loop, no other sensors (`synth`).
                // ── The store's own level, reported *after* this frame's staged effects ──
                //
                // The store's monotonic sequence **is** the revision, so the board reports a level
                // rather than counting commit edges. Reported here, and not with the other levels
                // at the top of the frame, because the staged effects above are where a commit
                // happens: a ride finalized or a removal committed at the top of this iteration
                // moves the sequence, and a level sampled before them carries the pre-commit value.
                //
                // That ordering is the whole of "one saved ride is one catalog read". Both the
                // commit's `StoreRevision` and Recorder's `RideFinalized` then reach the **same**
                // pass, and both arm the one `refresh_owed` bit — which `CatalogState::next_effect`
                // spends when it *issues* the read, not when the read is answered. Sampling before
                // the effects split the two arms across consecutive passes, so the bit was armed,
                // spent, and armed again, and the domain read the store twice for one save.
                exec.facts.note_store_revision(obc_app::device_core::StoreRevision {
                    store: BOARD_STORE,
                    revision: obc_app::device_core::Revision::new(flat.sequence()),
                });
                let clock = obc_app::device_core::PassClock { ride: RideClock(now), ui: InputClock(now) };
                // The hub sources (`consumer.location()` etc.) are constructed as **call-expression
                // temporaries**, exactly as they were at the `app.tick` sites this replaces: they are
                // stateless one-pointer drains, and binding them for the loop's lifetime would park one
                // hub pointer per source in this task's future (~40 B of `__embassy_main` arena,
                // measured in #808) for no behavioural difference. That is why the three builds each
                // spell the whole call rather than sharing a `let sensors`.
                #[cfg(feature = "debug-uart")]
                let plan = app.run_pass(obc_app::device_core::PassInputs {
                    now: clock,
                    gestures: &gestures,
                    sensors: Sensors {
                        altimeter: Some(&mut debug_alt),
                        compass: Some(&mut debug_compass),
                        fuel: Some(&mut fuel),
                        // Host-injected `H`/`P`/`R` land in the shared hub mailboxes; on a
                        // `ble` + `debug-uart` build a real strap feeds the same ones (last-writer-wins).
                        hr: Some(&mut consumer.hr()),
                        power: Some(&mut consumer.power()),
                        cadence: Some(&mut consumer.cadence()),
                        // No thermometer on this build, and the host feed streams no GPS time yet.
                        ..Sensors::new(&mut debug_loc)
                    },
                    route: route.as_ref(),
                    weather: weather_snapshot.as_ref(),
                    support: BOARD_SUPPORT,
                    outcomes: &mut exec.outcomes,
                    facts: &mut exec.facts,
                    derived,
                    targets,
                });
                #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
                let plan = app.run_pass(obc_app::device_core::PassInputs {
                    now: clock,
                    gestures: &gestures,
                    sensors: Sensors {
                        altimeter: Some(&mut consumer.altimeter()),
                        temperature: Some(&mut consumer.temperature()),
                        clock: Some(&mut consumer.clock()), // SAM-M10Q UTC → the wall clock (always stamps; #641)
                        compass: Some(&mut consumer.compass()), // ICM-20948 / AK09916 heading while stopped
                        fuel: Some(&mut fuel),
                        // The central manager (SE6) feeds the shared hub mailboxes.
                        hr: Some(&mut consumer.hr()),
                        power: Some(&mut consumer.power()),
                        cadence: Some(&mut consumer.cadence()),
                        ..Sensors::new(&mut consumer.location())
                    },
                    route: route.as_ref(),
                    weather: weather_snapshot.as_ref(),
                    support: BOARD_SUPPORT,
                    outcomes: &mut exec.outcomes,
                    facts: &mut exec.facts,
                    derived,
                    targets,
                });
                #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
                let plan = app.run_pass(obc_app::device_core::PassInputs {
                    now: clock,
                    gestures: &gestures,
                    // The synthetic loop has no sensors at all — not even a clock source.
                    sensors: Sensors { fuel: Some(&mut fuel), ..Sensors::new(&mut synth) },
                    route: route.as_ref(),
                    weather: weather_snapshot.as_ref(),
                    support: BOARD_SUPPORT,
                    outcomes: &mut exec.outcomes,
                    facts: &mut exec.facts,
                    derived,
                    targets,
                });
                plan
            };

            // The hold-cancel latch is the board's input plane's, and `stage_input` deliberately
            // does not drain it: a gesture that changed the screen stack invalidates a hold charging
            // *right now* on the high-priority plane, whose recogniser only this side can cancel.
            if app.take_hold_cancel() {
                display.cancel_holds();
            }
            // **`PassPlan` never crosses an `.await`** (FAR-19, restated for the typed protocol): the
            // three fields the tail needs are copied out here and the plan is dropped inside the
            // store phase. Only the staged `EffectSlots` and the small executor state survive to the
            // present and sleep phases.
            let obc_app::device_core::PassPlan { render, next_wake_ms, derived_needs, sources, effects, immediate } =
                plan;
            exec.needs = derived_needs;
            debug_assert!(
                !exec.effects.has_pending(),
                "every staged effect is served in this frame's store phase before the next plan lands"
            );
            exec.effects = effects;

            // ── BLE → app seam, ACTING half: what the pass just decided, out to the radio plane ──
            // Both of these key on screen and settings state **this** frame's gestures produced, so
            // they read the app after the pass rather than before it.
            {
                // Scan mode: while the Sensors screen's scan list is up, keep a discovery scan
                // running. `request_scan` **must not** be rung every pass — it pulses the manager's
                // `WORK_EDGE`, which the manager's own scan window selects on, so a per-pass ring
                // would collapse the ~10 s window (and re-clear the hit snapshot) every ~40 ms.
                // Ring once on the rising edge, then re-arm every ~9 s.
                if app.sensor_scan_active() {
                    // `now >= rearm` in wrapping-monotonic terms (the signed diff handles the ~49-day
                    // u32 wrap); `0` is the "not scanning yet" sentinel that fires on the rising edge.
                    let due = sensor_scan_rearm_ms == 0 || now.wrapping_sub(sensor_scan_rearm_ms) as i32 >= 0;
                    if due {
                        crate::ble::request_scan();
                        sensor_scan_rearm_ms = now.wrapping_add(9_000).max(1); // never 0 (the "off" sentinel)
                    }
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
                }
                // Saved-sensor reconcile: the persisted `Settings.saved_sensors` is the source of
                // truth. Diff each slot against what was last pushed and drive the change through
                // SE6's save/forget latches — fired once per change (seed at boot from all-`None`, a
                // screen pair/forget, a factory reset clearing a slot).
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
            }

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
                    let a = app.recorder.altitude();
                    let baro = app.recorder.baro_elevation_m().unwrap_or(f32::NAN);
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
                    // The resample is a *fact* — a monotone level the domain holds, and the repaint
                    // edge a stack-local render key can otherwise never see. What the new sample
                    // means (the rain map's step range and zoom floor, and whether an alert is
                    // owed) is decided at stage 10 of the next pass, from this very borrow.
                    weather_sample = weather_sample.next();
                    exec.facts.note_weather_sample(weather_sample);
                }
            }

            // Feed the high-priority plane's Select hold-progress to the map render so the in-screen
            // confirm fills (the factory-Reset bar) track the hold — `App`'s own input plane isn't
            // driven here, so the render would otherwise read 0 and the bar would never fill.
            let hold_p = display.hold_progress();
            app.set_hold_progress(hold_p);

            // The pass's own render decision (`plan.render`) is this frame's dirty signal — the same
            // `take_dirty` level edge the loop used to drain itself, now taken by stage 14 and handed
            // over in the plan. What it replaces is the *input* to the four board redraw folds below,
            // not the folds: each demand here is physical (a latched redraw, a FLPR relaunch, a live
            // hold fill, the freeze) and stays the board's until S4 #1447 lands render keys. Every one
            // of them is full-frame, so each also drops a region-scoped clip (`dirty.region`) — the
            // region only survives when the pass's own ticks were the sole dirt.
            let mut dirty = render;
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
            // An install is armed and this is the frame the panel keeps: the warm reset into the
            // bootloader never paints, so whatever is on glass when the tail arms stays there for the
            // whole SD→flash stream and the reboot — and there is no later frame to fix it with. The
            // "Installing update" card landing does dirty the screen full-frame in practice, but on an
            // irreversible path that must be a property of *this* code rather than of the domain's
            // dirty behaviour, so the fullness is forced here beside the other board demands.
            if exec.arm_pending.is_some() {
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
            // lifts (Navigator dirties the map for exactly that). And the *overlay*
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
                        // Named apart from the `ui frame:` line every other non-map redraw shares:
                        // the menus, the station steps and the planning spinner all take this same
                        // branch, so a log reader (and the #1487 soak driver) cannot tell a banner
                        // repaint from a menu repaint without it. `debug-uart` only — the harness is
                        // its only reader and the shipping image should not carry the string.
                        #[cfg(feature = "debug-uart")]
                        defmt::info!("freeze: banner repaint rows {=u16}..{=u16}", y0, y0 + rows);
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
                // `plan.sources.map` — the pass's own answer to "the base screen draws the map", so
                // the reader this frame opens is the one the pass planned for rather than a second
                // derivation of the same predicate. Its sibling `sources.route` is consumed above:
                // it is `active_route.is_some()`, which is exactly what the index/reader build keys
                // on (`index_route != active`), only coarser — the board keeps the finer edge.
                let needs_map = sources.map;
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
                                    weather_snapshot_ref,
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
            (rendered, dirty.map, hold_p, next_wake_ms, immediate, t_store.elapsed().as_micros())
        };

        // ═══ Present phase (#809): guard-free — the FLPR scans the frame (~44 ms full-frame)
        // with the store released, so a BLE object operation interleaves with the scan instead
        // of queueing behind it. `presented_ok` anchors the DFU trial confirm in the tail. ═══
        let mut presented_ok = false;
        if let Some(rf) = rendered {
            // A frame that is about to be frozen on the panel by a warm reset is presented
            // **full-frame**: the arm's `exclude` would leave the bulge rows showing the previous
            // screen for the whole install, and a live hold mid-confirm is gone after the reset
            // anyway. Every other frame goes around a live bulge as usual.
            let exclude = if exec.arm_pending.is_some() { None } else { overlay_span };
            let (ok, push_us) = display.present_frame(exclude).await;
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

        // The panel's brightness follows whatever the app says it should be this frame — the quick
        // drawer's live preview while its editor is open, the committed setting otherwise. That
        // derived answer is why a cancelled edit needs no undo path here: the editor closes and the
        // next frame reads the committed row again.
        apply_backlight(&mut backlight, &mut backlight_level, app.backlight_level());

        // The rider completed the guarded power-off hold, and the frame that says so has just been
        // pushed. `power_off` does not return.
        if app.power_off_requested() && presented_ok {
            obc_ports::PowerOff::power_off(&mut power_off);
        }

        // The hold bulge already pushed at the top of this pass (bulge-first, see above). But if a
        // screen present just landed, its `exclude` skipped the bulge rows — they still show the *old*
        // frame under the bulge. Re-composite them over the fresh fb now (a ~12 ms partial push, only
        // on the rare pass where a redraw and a live bulge coincide) so the band never lags the screen.
        if dirty_map && overlay_span.is_some() {
            display.present_bulge(overlay_span).await;
        }

        // ═══ Store tail (#809): a second short guard for the store work that must FOLLOW the
        // present — the trial confirm is anchored on a frame having reached glass, and the
        // deferred ride save must grind against an already-presented screen, not delay it. ═══
        let tail_held_us = {
            let mut store_guard = shared.lock().await;
            let t_tail = Instant::now();
            let SharedStore { storage: _, settings: settings_store } = &mut *store_guard;

            // ── DFU trial confirm (epic #615 S4, #619), once, at the health anchor ──
            // A frame just landed on glass and the SD mounted at boot: if this boot is a
            // trial (`Trial { installed, .. }` on the boot-state page), write
            // `Idle { installed }` — the whole confirm — and hand the app the one-time
            // "updated to vX" fact for S5's toast. A failed first present retries the
            // anchor on a later pass; an unconfirmed trial rolls back next boot by design.
            if trial_confirm_pending && presented_ok {
                trial_confirm_pending = false;
                if let Some(installed) = crate::dfu::confirm_trial(settings_store) {
                    let confirmed =
                        obc_app::device_core::UpdateResult::Confirmed(obc_app::dfu::clamp(installed.fw_version_str()));
                    if exec.facts.note_update_result(confirmed).is_err() {
                        defmt::error!("dfu: the boot verdict slot was still full at the trial confirm");
                        debug_assert!(false, "one boot, one update verdict");
                    }
                }
            }

            // ── The staged install arm (epic #615 S4), once, AFTER the present ──
            // The "Installing update" card is on glass now: the warm reset into the bootloader never
            // paints — it only parks the panel pins and keeps the COM wave alternating
            // (`obc-boot/src/com.rs`) — so the MIP holds *this* frame for the whole snapshot + flash.
            // The arm holds the store exclusively across its whole SD→flash stream, deliberately (a
            // BLE `UPDATE.BIN` write must not interleave with it), which is why it belongs in the
            // tail rather than in the guard-free block that decided it.
            //
            // The check→present→arm window this opens is benign: recording / pending-save state is
            // ride-loop-owned (neither can appear meanwhile), a yanked card re-refuses here, and a
            // BLE `UPDATE.BIN` rewrite in the window is the DR6 stale-ref case — the bootloader
            // re-verifies the staged image after the reset regardless.
            // The card must actually be on the stack before the panel is handed to the reset: the
            // scheduler can bounce an install-began answer it has to *push* onto a full stack (the
            // debug arm, with no spinner to replace) and re-queue it, and arming meanwhile would
            // freeze the previous frame on for the whole flash. Bounded, because the install matters
            // more than the frame — the same stance the inline path took when a present failed.
            let arm_now = match exec.arm_pending {
                None => false,
                Some(_) if app.dfu_installing_card_up() => true,
                Some(waited) if waited < ARM_CARD_FRAMES => {
                    exec.arm_pending = Some(waited + 1);
                    false
                }
                Some(_) => {
                    defmt::warn!("dfu: arming without the installing card on glass — the panel keeps the frame it has");
                    true
                }
            };
            if arm_now {
                exec.arm_pending = None;
                let SharedStore { storage, settings: settings_store } = &mut *store_guard;
                // DR6 (#734): hand the confirm's carried scan ref to the arm (consumed either way).
                // Absent ⇒ `run_install` re-scans (the `dfu-install` debug path). On success this
                // never returns.
                let failed = match storage.as_mut() {
                    Some(s) => crate::dfu::run_install(s, settings_store, &mut wdt, cached_staged.take()).await,
                    None => {
                        crate::dfu::status("refused (no_card): no SD card");
                        Some(obc_app::DfuInstallError::NoCard)
                    }
                };
                if let Some(error) = failed {
                    // `InstallBegan` is the operation's **terminal** answer — `DfuState` invalidated
                    // its token when it accepted it — so an arm that then failed cannot ride the same
                    // operation, and inventing a second one would be an operation the rider never
                    // started. It is a *fact* instead, and the vocabulary already has the right one:
                    // the update did not start. The rider sees the update-failed card rather than an
                    // installing card that never becomes a reboot.
                    //
                    // What this costs, stated: the specific `DfuInstallError` tier is lost on this
                    // path (it survives on the guard refusals above, which answer the operation while
                    // it is still live). The exact reason is on RTT and in the `D`-line breadcrumbs.
                    defmt::error!("dfu: the arm failed after the card was presented: {}", defmt::Debug2Format(&error));
                    let verdict = obc_app::device_core::UpdateResult::Failed {
                        why: obc_app::DfuFailure::NotStarted,
                        staged: None,
                    };
                    if exec.facts.note_update_result(verdict).is_err() {
                        defmt::error!("dfu: a boot verdict was already pending when the arm failed");
                    }
                }
            }

            t_tail.elapsed().as_micros()
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
        // edge (`ble::wait_status_change` — connect/disconnect *and* the pairing passkey, so the
        // passkey card wakes the loop from warm sleep), or the soonest screen animation deadline the
        // app reports.
        // The body's reconciles are all edge-gated,
        // so running them only on a wake is correct — a parked Home screen wakes ~once a minute (the
        // clock minute-tick) instead of 125×/s, and an idle device with the GPS asleep wakes only on a
        // button or that minute tick.
        // While something is **actively animating** — a live hold bulge (`overlay_*`, incl. its retract),
        // a charging hold on either button (`charging`), a redraw a flaky SD glitch couldn't service
        // (`pending_map_redraw`) — keep the short cadence so
        // it stays fluid; otherwise arm the app's single next-wake deadline, or sleep indefinitely
        // until input/sensor.
        let charging = hold_p > 0.0 || display.hold_charging();
        // "A search is live" is the app's fact, never the board's run handle: `CoreMode` is set when
        // the plan command drains and cleared by the answer, which brackets `nav_run` on both sides.
        // It costs a hot loop rather than an exclusion if it is wrong, and it is the last place the
        // board derived this a second way.
        let planning = app.core_mode() == obc_app::device_core::ModeState::Searching;
        let animating = charging || planning || pending_map_redraw || display.overlay_owed() || overlay_span.is_some();
        // The pass's own deadline (`plan.next_wake_ms`), plus the reasons to come straight back: the
        // plan's `immediate` — a later-to-earlier connection is in flight, so work already decided
        // would otherwise sit until the next rider input — and the executor's own `owed`: an answer
        // to consume, an effect to serve, a derived read it was asked for, or a residual command in
        // the legacy mailbox. That last one is the rider's **forget-phone**, which nothing else here
        // can see, and which the guarded hold that posts it leaves on a static screen. The ride
        // save was the other half until #1398; it is an effect in this pass's plan now, so `owed`
        // covers it. An outstanding store round trip takes the short animation cadence instead,
        // because spinning at full speed against a commit that runs for hundreds of milliseconds
        // would starve the task answering it.
        let next_ms = if animating || exec.polling_store() {
            Some(LOOP_MS as u32)
        } else if immediate || exec.owed(app.has_pending_residual_command()) {
            Some(0)
        } else {
            next_wake_ms
        };
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
            // A sensor/host datapoint, or a store movement — an upload/delete rescans the catalog
            // now, not at the next timer wake (#450).
            wait_host_or_sensor_event(
                #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
                consumer,
            ),
            // A BLE link edge — connect/disconnect *and* the pairing passkey — so the passkey card
            // wakes the loop from warm sleep (epic #447, P2).
            crate::ble::wait_status_change(),
            Timer::after_millis(ms as u64),
        )
        .await;
    }
}
