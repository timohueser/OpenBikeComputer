//! The map/ride thread-mode plane.
//!
//! [`run_app`], the shared backend-agnostic ride loop, and its loop-only helpers: the sensor-wake
//! select arm, the GPS power policy, the watchdog cadence, the per-frame render clock, and the
//! route-catalog scan. `main` owns bring-up and the resident statics, and awaits [`run_app`] as its
//! tail future.

use core::sync::atomic::Ordering;

use embassy_futures::select::select5;
use embassy_nrf::gpio::Output;
use embassy_nrf::wdt;
use embassy_time::{Instant, Timer};
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use embedded_graphics::prelude::{Point, Size};
use embedded_graphics::primitives::Rectangle;
use obc_app::App;
use obc_display::ls021::{FRAME_H, FRAME_W};
use obc_display::FbDevice64;
#[cfg(has_nav)]
use obc_formats::io::{ByteSink, Error as ByteError};
#[cfg(has_nav)]
use obc_formats::obcr::HEADER_FULL_LEN;
#[cfg(not(all(not(feature = "debug-uart"), feature = "synth")))]
use obc_platform::sensor_hub::SensorConsumer;
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use obc_platform::sensor_hub::SensorControl;
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use obc_platform::sensor_hub::SensorDemand;
use obc_platform::StubFuelGauge;
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
use obc_platform::SynthLocation;
use obc_ports::{InputClock, RideClock, Sensors, SettingsStore};
use obc_reader::{MapCache, MapTables, Reader};
use obc_route::{RouteCache, RouteIndex, RouteReader};

use crate::input_plane::{CHORDS, GESTURES, INPUT_HB_MS, INPUT_WAKE, LOOP_MS};
use crate::map_plane::MapDisplay;
use crate::{stackmeter, SharedSettings, SharedSettingsMutex};

/// Watchdog period: 24 s of 32768 Hz ticks. It lives in `obc-dfu` because the bootloader must
/// build the byte-identical WDT config to adopt this dog across a DFU install, and to pre-start the
/// one a trial boot runs under.
pub(crate) const WDT_TIMEOUT_TICKS: u32 = obc_dfu::WDT_TIMEOUT_TICKS;
/// Cap (ms) on the ride loop's event-driven sleep, about half the watchdog period, so an otherwise
/// idle device still wakes to feed the dog.
const WDT_FEED_CAP_MS: u32 = 12_000;
/// How stale [`INPUT_HB_MS`] may be before the ride loop withholds the feed. The idle input plane
/// sleeps 30 s between stamps, so this is twice that plus margin. A stamp slightly newer than the
/// loop's own `now` counts as fresh, not as a wrapped staleness.
const INPUT_HB_STALE_MS: u32 = 65_000;

/// Synthetic-walk advance cadence on the `synth` build. The stand-in GPS publishes no signal, so
/// the event-driven loop has no sensor event to wake on. The walk position is time-based.
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
const SYNTH_TICK_MS: u64 = 250;

/// The single sensor or host wake the event-driven map loop selects on, so the loop sleeps until a
/// datapoint arrives. With real sensors it is the hub's datapoint edge, with `debug-uart` the
/// host-streamed edge, and with `synth` a coarse timer that steps the synthetic walk.
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

/// The loop's third select arm: a sensor or host datapoint, or a flat-store movement on either
/// link, so a route or trip commit wakes the loop and the catalog rescan lands now.
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
                crate::link_control::wait_control_event(),
                crate::usb::wait_stage_request(),
            ),
        ),
    )
    .await;
}

/// Report successful protocol-v4 route and trip uploads as [`ExternalFacts`], and only after the
/// catalog snapshots that hold their committed heads have been fed to `App`. This ordering is what
/// lets a same-id active route replacement invalidate geometry-derived state.
///
/// The fact slots are latest-wins, so several uploads inside one catalog read collapse to the
/// newest of each kind.
fn note_catalog_uploads(app: &App, facts: &mut obc_app::device_core::ExternalFacts) {
    use obc_app::device_core::{RouteUpload, TripUpload};
    // More than one full catalog of distinct facts can only happen when rapid churn leaves stale ids
    // queued. If that exhausted the bounded handoff, refresh the active route first: no geometry
    // derived from the displaced revision survives. The store-revision level cannot stand in for
    // this, because it orders a re-read and only a `replaced` upload drops geometry-derived state.
    if crate::flat_store::take_catalog_upload_loss() {
        if let Some(id) = app.active_route_index().and_then(|i| app.route_ids().get(i).copied()) {
            facts.note_route_upload(RouteUpload { id, replaced: true, elevation: None });
        }
    }
    while let Some(upload) = crate::flat_store::take_catalog_upload() {
        match upload.kind() {
            obc_app::CatalogUploadKind::Route => {
                facts.note_route_upload(RouteUpload { id: upload.id(), replaced: upload.replaced(), elevation: None })
            }
            obc_app::CatalogUploadKind::Trip => {
                facts.note_trip_upload(TripUpload { id: upload.id(), replaced: upload.replaced() })
            }
        }
    }
}

#[inline(never)]
async fn read_catalogs(
    flat: &'static obc_storage::flat::FlatStore<crate::flat_store::FlatCard>,
    app: &mut App,
    facts: &mut obc_app::device_core::ExternalFacts,
) -> Result<obc_app::device_core::StoreRevision, obc_app::catalog_state::CatalogError> {
    use obc_app::catalog_state::CatalogError;
    app.begin_catalog_refresh();
    metadata_call(crate::flat_store::Request::ReconcileMetadata).await.map_err(catalog_metadata_error)?;
    let start = crate::flat_store::catalog_scope(flat);
    let routes_loaded = crate::flat_store::load_routes(flat, app);
    let trips_loaded = crate::flat_store::load_trips(flat, app);
    let rides_loaded = crate::flat_store::load_rides(flat, app);
    if routes_loaded && trips_loaded {
        note_catalog_uploads(app, facts);
    }
    if !routes_loaded || !trips_loaded || !rides_loaded {
        return Err(CatalogError::Unreadable);
    }
    crate::flat_store::load_metadata(flat, app).map_err(catalog_metadata_error)?;
    if start != crate::flat_store::catalog_scope(flat) {
        return Err(CatalogError::Stale);
    }
    Ok(start)
}

/// A `no_std` [`Clock`](obc_render::Clock) over embassy's monotonic `Instant`, in microseconds: the
/// time base for the map render's per-stage timing.
struct InstantClock;
impl obc_render::Clock for InstantClock {
    fn now_us(&self) -> u64 {
        Instant::now().as_micros()
    }
}

/// The router's resident half: everything the planner needs that is not an arm of the scratch
/// arena. The terrain stays here because `App::sample_terrain` reads it at fix cadence during a
/// ride, while the map plane renders and no search runs. It is state, not scratch.
#[cfg(has_nav)]
pub(crate) struct NavResident {
    /// The map's terrain, or the null source. It is a `&'static mut` because a `TerrainElevation`
    /// carries its tile cache inline and must never be copied into a plan frame.
    pub(crate) elev: &'static mut dyn obc_route::ElevationSource,
}
/// The stand-in for a build without the router. The ride loop still drains create-route requests
/// and answers the generic failure tier, so the confirm never hangs.
#[cfg(not(has_nav))]
pub(crate) struct NavResident;

/// One plan step's view of everything the planner touches: the scratch arena's nav arm, borrowed
/// from the guard the ride loop holds for the whole search, and the resident terrain beside it. It
/// is rebuilt per call, so no reference into the arena is ever live across an `.await`.
#[cfg(has_nav)]
struct NavBuffers<'a> {
    /// The guard itself, not the arm behind it: the planner slot is a `MaybeUninit` the claim
    /// leaves unwritten, and the guard is what carries "a plan has been written into it".
    guard: &'a mut crate::arena::NavGuard,
    elev: &'a mut dyn obc_route::ElevationSource,
}

/// One in-flight plan's board-side bookkeeping: the open reserved-file handle and the per-phase
/// wall-time accumulators the log line reports.
#[cfg(has_nav)]
struct NavRun {
    allocation: Option<obc_storage::flat::Allocation>,
    io: NavIo,
    cancel_requested: bool,
    io_started: Instant,
    /// Wall time when the request was drained, which is the log line's user-perceived `total_ms`.
    t0: Instant,
    /// Per-phase step time (µs), attributed by the planner's phase before each step:
    /// `[snap, search, emit]`.
    phase_us: [u64; 3],
    /// Store-task time spent flushing bounded output stages before the final checksum/commit.
    write_us: u64,
    /// Physical-card reads issued inside planner steps, by phase. Unlike the cache counters this
    /// sees sector splitting and alignment bounces at the block-device boundary.
    #[cfg(feature = "sd-bench")]
    read_perf: [crate::card_io::ReadPerf; 3],
}

#[cfg(has_nav)]
enum NavIo {
    NeedAllocate,
    Allocating(crate::flat_store::Ticket),
    Ready,
    StepRequested,
    ReadyCommit(obc_route::RouteStats),
    Complete,
    Published(obc_storage::flat::ObjectId),
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
const NAV_ROUTE_RESERVE: u64 = HEADER_FULL_LEN as u64
    + 256 * (255 * obc_formats::obcr::POINT_RECORD_LEN as u64 + obc_formats::obcr::CHUNK_META_LEN as u64);

#[cfg(has_nav)]
static NAV_STORE_REPLY: crate::flat_store::Reply = embassy_sync::signal::Signal::new();

#[cfg(has_nav)]
pub(crate) struct NavStageSink<'a> {
    pub(crate) stage: &'a mut [u8; crate::arena::NAV_OUTPUT_STAGE_BYTES],
    pub(crate) appended: usize,
    pub(crate) patch_len: usize,
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

/// Construct and write a fresh request's planner into its slot, in this immediately-popped frame.
/// `NavPlanner::new` materializes a ~9 KB temporary, and inlined into the ride loop that slot lands
/// in the main task's poll frame, which is allocated at the entry of every poll.
#[cfg(has_nav)]
#[inline(never)]
fn nav_begin(nav: &mut NavBuffers, req: &obc_app::NavRequest, profile_idx: u8) {
    // The rider's bike-type setting. An out-of-range index falls back to profile 0 in the router.
    nav.guard.begin_plan(obc_route::NavPlanner::new(req.from, req.to, req.name(), profile_idx));
    // One diagnostic line per plan start: the three addresses pin the memory map without the ELF at
    // hand. They are offsets inside the scratch arena's nav arm.
    let (planner, scratch, tiles) = nav.guard.arm_addrs();
    defmt::debug!(
        "nav plan: start planner=0x{=usize:08x} scratch=0x{=usize:08x} tiles=0x{=usize:08x}",
        planner,
        scratch,
        tiles
    );
}

/// Take the scratch arena's nav arm for a fresh route search, against the app's own quiesced-map
/// proof.
///
/// `Err(why)` means the caller must answer the failure tier now rather than arm a plan. A refused
/// claim took nothing, so no spinner can hang behind a half-claim. A request that arrives while a
/// plan is in flight is not a refusal: the arm is already held.
///
/// The arena's own owner is what enforces `nav ⊥ usb`. The refusal names the holder, because "wait
/// for the cable" is something the rider can act on.
#[cfg(has_nav)]
fn nav_take_arena(app: &App, guard: &mut Option<crate::arena::NavGuard>) -> Result<(), &'static str> {
    use obc_app::{ArenaError, ArenaOwner};
    if guard.is_some() {
        return Ok(());
    }
    let Some(quiesced) = app.nav_arena_precondition() else {
        // Unreachable by construction: draining a plan command is what engages the Recalculating
        // freeze, so the map plane is already quiet. Loud in debug, handled in release.
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

/// The fixed slot index (0 heart rate, 1 power, 2 cadence) a scanned sensor's kind maps to. The app
/// seam speaks slot indices, not `obc_ble` kinds.
fn sensor_kind_slot(kind: obc_ble::SensorKind) -> u8 {
    match kind {
        obc_ble::SensorKind::HeartRate => 0,
        obc_ble::SensorKind::Power => 1,
        obc_ble::SensorKind::Cadence => 2,
    }
}

/// Distil the central manager's per-quantity sensor status into the app-vocabulary
/// [`SensorStatus`](obc_app::SensorStatus) the Sensors screen renders.
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

/// Run one bounded planner step at the ride loop's shallow per-pass depth. The step's frame carries
/// only the `Reader` view, the bounded staging sink and the planner's own shallow call tree; the
/// emitter lives in the planner's slot. Everything long-running — the render, the input, the
/// watchdog feed — runs between steps.
///
/// Emitted OBCR bytes are staged in the arena and flushed through the flat store's sole writer
/// between steps, so no card write happens inside the synchronous planner call.
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
    // planner, but the guard is what knows that, so ask it rather than assert it. An unwritten slot
    // answers the generic failure tier instead of stepping a planner nobody built.
    let Some((planner, scratch, tiles, output)) = nav.guard.plan_parts() else {
        debug_assert!(false, "a plan step with no planner written — the run outlived (or preceded) its drain");
        return NavStep { outcome: obc_route::Step::Failed(obc_route::NavError::NoPath), appended: 0, patch_len: 0 };
    };
    let mut sink = NavStageSink { stage: output, appended: 0, patch_len: 0 };
    let outcome = planner.step(&reader, scratch, tiles, &mut *nav.elev, &mut sink);
    NavStep { outcome, appended: sink.appended, patch_len: sink.patch_len }
}

/// Finish a completed plan: hash and publish or cancel the flat-store reservation, and emit the one
/// `nav route:` log line with the per-phase breakdown. The answer itself is the caller's.
///
/// In that line, `total_ms` is wall time from the request drain, so it spans every pass the plan
/// was spread over, and `snap/search/emit_ms` is step time attributed to the planner's phase before
/// each step.
#[cfg(has_nav)]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn nav_finish(
    nav: &mut NavBuffers<'_>,
    run: &NavRun,
    result: Result<(u64, u32), obc_app::navigator::NavigatorError>,
    now: u32,
) {
    use obc_route::NavError;
    let write_us = run.write_us;
    let rescan_us = 0;
    let cache = nav.guard.tiles.stats();
    // The ε rung the plan ended on. `settles` is cumulative across the rungs. Both read through the
    // guard's checked accessor, so a finish with no planner written reports zeroes.
    let (settles, eps_num, eps_den) = nav.guard.planner_ref().map_or((0, 0, 0), |p| {
        let (n, d) = p.epsilon_used();
        (p.settles(), n, d)
    });
    let hw = stackmeter::rescan(now);
    // `exhausted` is the range tier; `no-path` the generic one.
    let outcome_str = match &result {
        Ok(_) => "ok",
        Err(obc_app::navigator::NavigatorError::Plan(NavError::NoPath)) => "no-path",
        Err(obc_app::navigator::NavigatorError::Plan(NavError::Exhausted)) => "exhausted",
        Err(obc_app::navigator::NavigatorError::Store) => "store",
        Err(obc_app::navigator::NavigatorError::SourceChanged) => "source-changed",
        Err(obc_app::navigator::NavigatorError::Workspace) => "workspace",
        Err(obc_app::navigator::NavigatorError::Movement) => "movement",
        Err(obc_app::navigator::NavigatorError::Unavailable) => "unavailable",
        Err(obc_app::navigator::NavigatorError::DurabilityUnknown) => "durability unknown",
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

/// The [`Gesture`](obc_app::Gesture) variant's name for the drained-input `defmt` breadcrumb. It
/// lives board-side because `obc-app` stays defmt-free.
fn gesture_name(g: obc_app::Gesture) -> &'static str {
    match g {
        obc_app::Gesture::Step(_) => "Step",
        obc_app::Gesture::Press => "Press",
        obc_app::Gesture::Hold => "Hold",
        obc_app::Gesture::Back => "Back",
        obc_app::Gesture::BackHold => "BackHold",
    }
}

fn chord_name(c: obc_app::Chord) -> &'static str {
    match c {
        obc_app::Chord::Quick => "Quick",
        obc_app::Chord::Assistant => "Assistant",
        obc_app::Chord::Context => "Context",
    }
}

/// One pass's sync-render output, carried from the store phase (render, under the guard) to the
/// present phase (push, guard-free). `None` means no frame was rendered this pass.
struct RenderedFrame {
    needs_map: bool,
    /// Whether this frame drew the sheet and nothing else. The base's draw was skipped, so its rows
    /// on the panel are the ones the sheet arrived over.
    sheet_only: bool,
    stats: obc_render::RenderStats,
    render_us: u64,
}

/// Platform features available for this boot. Detours use sealed temporary card storage and the
/// shared planner arena; metadata changes go through the card writer.
const BOARD_SUPPORT: obc_app::device_core::PlatformSupport = obc_app::device_core::PlatformSupport {
    detour: cfg!(has_nav),
    settings_persistence: true,
    dfu: true,

    bonding: true,
    storage_space_report: true,
};

/// One in-flight `CatalogEffect::RemoveObject` on the flat store's ticketed writer path: the
/// storage task's answer slip, and the operation token that answer has to carry back.
///
/// A full queue here is simply a pass where the effect was not taken, and a refused commit is a
/// `Failed` the domain re-queues.
struct CatalogRemoval {
    ticket: crate::flat_store::Ticket,
    token: obc_app::device_core::OperationToken<obc_app::device_core::CatalogTag>,
    object: u64,
}

/// The reply slot the catalog removal's round trip uses. One slot per *concurrently live* call, and
/// the domain admits one catalog operation at a time, so this is exactly one.
static CATALOG_STORE_REPLY: crate::flat_store::Reply = embassy_sync::signal::Signal::new();

/// The board's typed effect executor: everything owed between two `App::run_pass` calls. Bounded
/// effects are staged out of the plan and executed in the physical phase that already owns them,
/// and token-carrying outcomes are returned on a later pass. The phase order is the board's; the
/// pass order is DeviceCore's.
#[derive(Default)]
struct RideExec {
    outcomes: obc_app::device_core::OutcomeSlots,
    /// What moved underneath DeviceCore that nobody asked for, for the next pass's stage 2.
    facts: obc_app::device_core::ExternalFacts,
    /// The previous plan's derived needs, answered at the top of the next store phase.
    needs: obc_app::device_core::DerivedNeeds,
    effects: obc_app::device_core::EffectSlots,
    /// The in-flight removal, held across passes and polled without parking.
    catalog: Option<CatalogRemoval>,
    /// The operation the planner run is running under — the token every planner answer carries back.
    #[cfg(has_nav)]
    nav_token: Option<obc_app::device_core::OperationToken<obc_app::device_core::NavigatorTag>>,
    /// A `DfuEffect::ArmInstall` passed its go/no-go and is waiting for the "Installing update" card
    /// to reach glass; the count is how many frames it has waited. The arm runs in the store tail,
    /// after the present, and never returns on success, so the frame the panel holds through the
    /// whole flash is a real presented frame. It waits because the card's push can be bounced when
    /// the screen stack is full, and the wait is bounded by [`ARM_CARD_FRAMES`], because the install
    /// matters more than the frame.
    arm_pending: Option<u8>,
}

/// How many frames the arm waits for the "Installing update" card before going ahead without it.
/// A bounced push lands on the very next sweep in practice; this is that with room.
const ARM_CARD_FRAMES: u8 = 8;

impl RideExec {
    /// Whether the executor is holding something the next pass must see: an answer to consume, an
    /// effect to serve, or a derived read it has been asked for.
    ///
    /// The in-flight catalog removal is deliberately not here. It keeps the short animation cadence,
    /// because spinning at full speed against a commit that takes hundreds of milliseconds would
    /// starve the executor that has to answer it.
    fn owed(&self) -> bool {
        self.outcomes.has_pending() || self.effects.has_pending() || !self.needs.is_empty()
    }

    /// Whether a store round trip is outstanding. A committed removal wakes the loop, but an
    /// `existed: false` answer and a refused commit move no sequence and raise no wake.
    fn polling_store(&self) -> bool {
        self.catalog.is_some()
    }

    /// Hand one outcome to its domain's slot.
    ///
    /// The pass drains every slot at stage 1, so a full slot means two answers were produced for one
    /// domain inside a single frame. Each arm serves at most one effect, so that cannot happen.
    fn deliver<T>(slot: &mut obc_app::device_core::Slot<T>, outcome: T, domain: &str) {
        if slot.try_put(outcome).is_err() {
            defmt::error!("exec: {=str} answered twice in one frame — the second answer was lost", domain);
            debug_assert!(false, "one outcome per domain per frame");
        }
    }
}

#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
fn desired_sensor_power(app: &App) -> SensorDemand {
    SensorDemand::for_demand(
        app.recording(),
        app.settings().power_saver,
        app.peak_view_needs_position(),
        app.peak_view_is_base(),
    )
}

/// Drive the panel at `level`, remembering it in `last` so the PWM is written only on a change.
///
/// The boot seed and the per-pass apply both go through here, so the change gate cannot drift
/// between them.
fn apply_backlight(backlight: &mut crate::panel_power::PanelBacklight, last: &mut u8, level: u8) {
    if level == *last {
        return;
    }
    *last = level;
    if obc_ports::Backlight::apply(backlight, level).is_err() {
        defmt::warn!("backlight: the port refused level {=u8}", level);
    }
}

/// The shared map plane and ride loop. It drives present through [`MapDisplay`], so it carries no
/// backend `#[cfg]`. Each pass: drain the gestures the input plane recognised, advance the visible
/// screens' timed content, reconcile the card to the app's intent, feed the sensors into the pass
/// (integrate the fix, map-match, record the track point), then re-render the map only when it is
/// dirty and present it. A static screen does zero map renders. Never returns.
///
/// A finished ride is the flat object's recorded 20-byte samples followed by one summary footer.
/// The device writes no GPX: the phone owns export after sync.
#[allow(clippy::too_many_arguments)]
// `#[inline(always)]`: this is a single-call-site `-> !` future. Inlining folds it back into
// `main`'s frame, which recovers the stack the bare extraction costs.
#[inline(always)]
pub(crate) async fn run_app(
    mut display: MapDisplay,
    app: &mut App,
    // The card and the RRAM settings behind one async mutex. The loop takes it in two short scopes
    // per pass — the store phase and the post-present tail — and never holds it across the present
    // await, so the object planes reach the card during the panel scan.
    shared: &SharedSettingsMutex,
    map_tables: &MapTables,
    map_cache: &MapCache,
    // The flat map source and store are mandatory: a non-flat card never reaches this loop.
    flat_map: &'static dyn obc_formats::io::ByteSource,
    flat: &'static obc_storage::flat::FlatStore<crate::flat_store::FlatCard>,
    route_cache: &RouteCache,
    // The router's resident half: the map's terrain, threaded from `main` and never a local. Its A*
    // table, tile cache and planner slot are the scratch arena's nav arm, claimed per search below.
    #[cfg(has_nav)] nav: NavResident,
    #[cfg(not(has_nav))] nav: NavResident,
    led: &mut Output<'static>,
    // The panel's brightness port, armed in `main` where the peripherals live. By value: this loop
    // is its only user, and it never returns.
    mut backlight: crate::panel_power::PanelBacklight,
    // The watchdog feed handle, `None` only if the dog was already running with a foreign config.
    mut wdt: Option<wdt::WatchdogHandle>,
    // The sensor hub's consumer handle: the source drains, the presence flag and the event wake.
    // Absent only on the pure `synth` build.
    #[cfg(not(all(not(feature = "debug-uart"), feature = "synth")))] consumer: SensorConsumer<'static>,
    // The hub's control handle: the GPS rate and power latches the sensor task awaits.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))] control: SensorControl<'static>,
    // The map bbox centre. Only the `SynthLocation` stand-in needs it, because the host feed and the
    // real GPS both stream absolute positions.
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))] cam_center: (i32, i32),
) -> ! {
    // Native renderer colour → identity `Rgb565`; `FbDevice64` quantizes to RGB222 on store.
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

    // Sensor sources: three builds, and one `Sensors` either way, so the app cannot tell which.
    #[cfg(feature = "debug-uart")]
    let (mut debug_loc, mut debug_alt, mut debug_compass) = (
        obc_platform::debug_link::DebugLocation,
        obc_platform::debug_link::DebugAltimeter,
        obc_platform::debug_link::DebugCompass,
    );
    // The hub sources are not bound here. They are stateless one-pointer drains, so each pass site
    // builds them as call temporaries; a binding would park one hub pointer per source in this
    // task's future for no behavioural difference.
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
    let mut synth = SynthLocation::new(cam_center.0, cam_center.1, Instant::now());
    // Battery: a fixed stand-in until the PMIC fuel gauge is wired in.
    let mut fuel = StubFuelGauge::new(75);

    let mut exec = RideExec::default();
    let mut ride_recorder = crate::flat_ride::Recorder::new(
        flat,
        crate::flat_store::writer().expect("the flat storage task is armed before the ride loop"),
        Instant::now().as_millis() as u32,
    );
    // A reset may have landed after the footer checkpoint but before the single clearing commit.
    // Service that terminal state before the first UI pass. It must never expose a footer-bearing
    // object as resumable samples.
    ride_recorder.settle().await;
    let recovery_warning = ride_recorder.take_warning();
    if let Some(continuation) = ride_recorder.recovered_continuation() {
        let _ = app.offer_recovered_ride(continuation);
    } else if let Some(damage) = ride_recorder.recovery_damage() {
        // The classified damage decides what the card may offer: a readable catalog earns the one
        // guarded exact removal, an unreadable one earns none at all.
        let _ = app.offer_damaged_ride(damage);
    } else if recovery_warning {
        exec.facts.raise_warnings(obc_app::WarningFlags::REC_ERROR);
    }

    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
    let mut prev_route: Option<usize> = None;
    let mut prev_active: Option<usize> = None;
    // The ride session this executor has opened an object for. Never cleared by a close: a session
    // that has been served is served, whatever became of its object.
    let mut opened_session: Option<u32> = None;
    // The in-flight route plan's bookkeeping: `Some` while a plan is being stepped, one bounded step
    // per pass. It guards the planner slot's initialization.
    #[cfg(has_nav)]
    let mut nav_run: Option<NavRun> = None;
    #[cfg(has_nav)]
    let mut review_publication: Option<obc_formats::assistant::PayloadFingerprint> = None;
    #[cfg(has_nav)]
    let mut review_original: Option<obc_storage::flat::StoreSource<'static, crate::flat_store::FlatCard>> = None;
    #[cfg(has_nav)]
    let mut detour = crate::detour::Executor::new();
    #[cfg(has_nav)]
    let mut visit = crate::visit::Executor::new();
    // The scratch arena's nav arm, held for the whole search and so for many passes: the A* table,
    // the tile cache and the planner must all survive from one bounded step to the next. This loop
    // is the arena's sole owner-switcher. Taken at the plan drain, given back on the answer and on
    // every cancel path.
    #[cfg(has_nav)]
    let mut nav_guard: Option<crate::arena::NavGuard> = None;
    // A map upload's write-combining arm. Only this loop switches arena owners; the USB task asks
    // through a level and edge handshake.
    let mut usb_stage_guard: Option<crate::arena::UsbGuard> = None;
    // The active route's resident chunk-index slot: a bare `RouteIndex` and a validity flag, not an
    // `Option<RouteIndex>` built by value. The slot is about 12.3 KB either way, but a by-value
    // build also transits the stack at the pass's deepest point, which overflows it.
    let mut route_index: RouteIndex = RouteIndex::empty();
    let mut route_index_valid = false;
    let mut index_route: Option<usize> = None;
    let mut pending_map_redraw = false;
    let mut find_loading_painted = false;
    let mut power_off = crate::panel_power::SystemOff;
    // The level last handed to the backlight, so the PWM is touched on a change rather than every
    // pass. `u8::MAX` is never a real level, so the boot apply below always reaches the hardware.
    let mut backlight_level = u8::MAX;
    app.set_backlight_available(obc_ports::Backlight::available(&backlight));
    // The map plane is one resident framebuffer that the present scans out of, so every repaint is a
    // repaint over the last frame. That is what lets the app leave a frozen base's rows alone while
    // a drawer's sheet grows over them.
    app.set_resident_frame(true);
    #[cfg(feature = "debug-uart")]
    let mut last_telem_ms: u32 = 0;
    #[cfg(feature = "debug-uart")]
    let mut last_telem = obc_platform::debug_link::Telemetry::default();
    // Stack-guard bookkeeping: log only when a new deepest reach is seen.
    let mut stack_hw = 0usize;
    let mut last_led = 0u32;
    // The previous frame's hold progress, so a hold that retracts on a non-map screen gets one
    // trailing redraw to clear its bar. A cancelled long press emits no gesture.
    let mut prev_hold_p = 0.0f32;
    // Terrain samples taken this boot, only to throttle the `altfuse:` log line.
    #[cfg(has_nav)]
    let mut elev_fixes: u32 = 0;
    // The DFU trial confirm is anchored at "first frame presented and card mounted", which is the
    // first successful present below. A boot that cannot reach a presented frame never confirms, and
    // the rollback fires next boot.
    let mut trial_confirm_pending = true;
    // The scan's validated `StagedRef`, parked between the scan that produced it and the confirm's
    // install, so the arm reuses that read and CRC pass. A failed re-scan clears it, and a stale ref
    // is safe because the bootloader re-verifies after the reboot. `None` makes `run_install` fall
    // back to a fresh scan.
    let mut cached_staged: Option<obc_dfu::StagedRef> = None;

    // The saved-sensor addresses last pushed to the central manager, so the reconcile below drives a
    // save or forget only on an actual change and a steady state never interrupts a live link. It
    // starts empty, so the first pass seeds the manager from the persisted settings.
    let mut pushed_sensors: [Option<([u8; 6], bool)>; obc_app::SENSOR_SLOTS] = [None; obc_app::SENSOR_SLOTS];
    // The next re-arm deadline (loop ms) for the discovery scan while the scan list is up. `0` means
    // not scanning. It re-arms just under the manager's 10 s window, so the scan stays live without
    // pulsing the manager's work edge every pass.
    let mut sensor_scan_rearm_ms: u32 = 0;

    // Seed the app from the persistent RRAM store at boot; a blank or corrupt page decodes to the
    // defaults. One brief lock, released at once.
    app.set_settings({
        let mut store = shared.lock().await;
        store.settings.load().unwrap_or_default()
    });
    // The brightness in that seed reaches the panel here, before the first frame is drawn. The
    // per-pass apply at the end of the loop would otherwise leave the light at the level
    // `PanelBacklight::new` armed, so a rider who set a dim panel would watch it start bright.
    apply_backlight(&mut backlight, &mut backlight_level, app.backlight_level());

    // The DFU boot-outcome reconcile: the boot-state page and the armer's breadcrumb give the
    // one-time post-update verdict card. A trial boot is left alone; the confirm below owns it.
    {
        let mut store = shared.lock().await;
        crate::dfu::reconcile_boot_outcome(&mut exec.facts, &mut store.settings);
    }

    // Align the GPS to the persisted fix interval: push it to the sensor task once at boot, then
    // again whenever the Power screen edits it. `prev_interval` gates the re-VALSET, so an unrelated
    // settings change does not reconfigure the receiver.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    let mut prev_interval = app.settings().fix_interval_s;
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    control.set_rate(prev_interval);

    // Seed the receiver and compass demand; only changed levels are published after each pass.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    let mut prev_power = desired_sensor_power(app);
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    control.set_power(prev_power);

    // Whether the map-transfer card was observed on the stack last pass: the latch that turns "the
    // card is gone" into "the rider dismissed it".
    let mut map_card_shown = false;
    // Map-upload pacing: while bytes are landing the card is the only thing on glass, and every
    // repaint is about 85 ms of render and push stolen from the write path. Progress is fed to the
    // app at most once per this interval, and the loop's own timer is clamped up to it. Terminal
    // states bypass the throttle.
    const MAP_XFER_PACE_MS: u32 = 2_000;
    let mut map_uploading = false;
    let mut map_xfer_fed_ms: u32 = 0;
    // The transfer level last reported, so the log line below reports its edge and not every pass.
    #[cfg(feature = "debug-uart")]
    let mut prev_transferring = false;

    let mut peak_view = crate::peak_view::Runtime::new(app, flat_map, map_tables);

    loop {
        let now = Instant::now().as_millis() as u32;
        let hw = stackmeter::used(now);
        if hw > stack_hw {
            stack_hw = hw;
            // Surface the peak in the diagnostics blob: the ride loop owns the stackmeter, so it
            // publishes the mark into the state the blob reads.
            crate::link::publish_stack_high_water(hw);
            defmt::info!("stack high-water {=usize} / {=usize} B (new peak)", hw, stackmeter::total());
        }

        // The panel degraded for good, so drop to the heartbeat idle. This loop keeps feeding the
        // watchdog: degraded is a deliberate terminal state, not a wedge, and an unfed dog would just
        // boot-loop the device against a dead FLPR. Only a power cycle retries the panel.
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
        // alive and the stamp proves the recognizer alive, so either plane wedging stops the feed.
        if let Some(h) = wdt.as_mut() {
            // The input plane stamps from its own `Instant::now()`, which can be a hair newer than
            // this loop's `now`, and the subtraction then wraps. A wrapped age means the heartbeat is
            // ahead of us, so it is maximally fresh.
            let age = now.wrapping_sub(INPUT_HB_MS.load(Ordering::Relaxed));
            if age <= INPUT_HB_STALE_MS || age > u32::MAX / 2 {
                h.pet();
            } else {
                defmt::error!("WDT: input-plane heartbeat {=u32} ms stale — withholding the feed", age);
            }
        }

        // Feed the live hold progress before anything below consults it: every hold-deferral rule
        // this pass runs must read this pass's charge state. A loop woken from warm sleep otherwise
        // saw a seconds-stale 0.0 and could land or close a pushed screen mid-charge.
        let (select_p, back_p) = display.hold_progress();
        app.set_hold_progress(select_p, back_p);

        let transferring = crate::flat_store::transfer_active();
        exec.facts.note_transfer(if transferring {
            obc_app::device_core::TransferState::Active
        } else {
            obc_app::device_core::TransferState::Idle
        });
        #[cfg(feature = "debug-uart")]
        if transferring != prev_transferring {
            prev_transferring = transferring;
            defmt::info!("xfer: transfer level {=str} (flat engine)", if transferring { "active" } else { "idle" });
        }

        // The card transport gave up for this session. Raised every pass and deduplicated
        // downstream, so the card opens once and a dismissal is not re-nagged.
        if crate::flpr_mux::storage_latched() {
            exec.facts.raise_warnings(obc_app::WarningFlags::STORAGE_ERROR);
        }

        // The sensor task publishes once GPS responds or its startup deadline passes. Map chips that
        // are absent at that point to a dismissable warning; this is not a live-availability stream.
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

        // The BLE feeding half: everything that hands the app a value the pass below reads. The half
        // that acts on what the pass decided runs after it, inside the store phase.
        {
            // The link snapshot as a level: the pass compares it against what it last saw, so a
            // steady state dirties nothing.
            exec.facts.note_link(crate::ble::app_ble_status());

            // Mirror the ride-recording state to the BLE plane's install busy-gate, and drain a
            // BLE-initiated install request into the on-glass flow. The phone can request an install;
            // only the rider installs. The atomic is consumed only when the flow actually opened: a
            // `false` is a deferral, so the request stays pending and retries next pass.
            crate::link::set_recording(app.recording());
            if crate::link_control::dfu_install_pending() && app.open_remote_dfu_check() {
                let _ = crate::link_control::take_dfu_install_ble();
            }
            if let Some((utc, offset_min)) = crate::link_control::take_ble_clock() {
                app.stamp_clock_ble(utc, offset_min);
            }
            // Push the persisted Bluetooth toggle across the plane boundary. The radio plane wakes
            // only on a change. Fire and forget: this loop never blocks on the radio winding down.
            crate::ble::set_radio_enabled(app.settings().ble_enabled);
        }

        // The on-glass half of a write that runs for minutes. The USB data plane publishes the
        // transfer state into atomics, and this is the one task allowed to touch the `App`, so it
        // reads them once per pass and reconciles the card. An unchanged state repaints nothing.
        //
        // Dismissal has to be observed rather than signalled, because the terminal card pops itself
        // on a press and the next pass would push it back. The latch holds the observed card, so a
        // push deferred mid-hold is not mistaken for a dismissal.
        {
            if map_card_shown && !app.map_transfer_card_up() {
                crate::link::clear_map_transfer();
                map_card_shown = false;
                map_uploading = false;
            } else {
                let state = crate::link::map_transfer_state();
                let receiving = state.is_some_and(|s| s.is_receiving());
                // The throttle: a Receiving-to-Receiving pass inside the pace window skips the feed,
                // so a sensor-paced wake does not turn a progress tick into a full repaint.
                if !(receiving && map_uploading && now.wrapping_sub(map_xfer_fed_ms) < MAP_XFER_PACE_MS) {
                    app.set_map_transfer(state);
                    map_card_shown = app.map_transfer_card_up();
                    map_xfer_fed_ms = now;
                }
                map_uploading = receiving && map_card_shown;
            }

            peak_view.reconcile(app);

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

        // The sensor snapshots the pass reads. The requests that keep discovery alive are the acting
        // half and run after the pass, because both key on state this frame's gestures produce.
        {
            // The scan list's hits as a feed, so a wake for any reason renders what discovery has
            // found so far.
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
            let sensor_status = [sensor_status_of(0), sensor_status_of(1), sensor_status_of(2)];
            app.set_sensor_status(&sensor_status);
        }

        // This frame's hold-bulge state, sampled once.
        //
        // The bulge pushes first in the pass, before the store lock and any screen redraw: a fired
        // hold usually navigates, and a fired Finish triggers the ride save, so with the bulge later
        // its confirm pop queued behind the new screen's render or the whole save and the 220 ms pop
        // expired unseen. Bulge-first, the pop lands on glass within about 10 ms, composited over the
        // old framebuffer, which is what is on glass until the present below.
        let overlay_span = display.poll_overlay();
        display.present_bulge(overlay_span).await;

        // `DfuEffect` is the one effect the board serves a pass late. The guard-free block it
        // belongs in is here, ahead of the store phase, because the "Installing update" card's
        // present must not hold the store guard. One pass of latency on an operation that ends in a
        // reboot is not a cost. The outcome is consumed by this frame's pass, so the confirm or error
        // card still lands in one frame.
        #[cfg(feature = "debug-uart")]
        if obc_platform::debug_link::take_dfu_install() {
            app.debug_request_dfu_install();
        }
        // The recovery acceptance commands, drained in the same guard-free block and for the same
        // reason as `dfu-install`. All three dead-strip from the release image.
        #[cfg(feature = "debug-uart")]
        if let Some(kind) = obc_platform::debug_link::take_ride_damage() {
            ride_recorder.debug_fabricate_damage(flat, kind, now).await;
        }
        #[cfg(feature = "debug-uart")]
        if obc_platform::debug_link::take_ride_repair_fail() {
            ride_recorder.debug_arm_repair_failure();
        }
        #[cfg(feature = "debug-uart")]
        if obc_platform::debug_link::take_store_census() {
            crate::flat_store::debug_census(flat);
        }
        if let Some(effect) = exec.effects.dfu.take() {
            use obc_app::dfu::{DfuEffect, DfuOutcome};
            match effect {
                DfuEffect::ArmInstall { token } => {
                    // The irreversible arm and reboot. The guards mirror what the System menu greys
                    // out: never mid-recording, and never while the flat recorder still owns an
                    // active object. A refusal is a typed reason and lands the error card, so the
                    // spinner cannot strand the rider.
                    let refusal = if app.recording() || ride_recorder.is_recording() {
                        crate::dfu::status("refused (is_tracking): a ride is recording -- finish it first");
                        Some(obc_app::DfuInstallError::Recording)
                    } else if !flat.mode().writable() {
                        // The arm commits the rollback reserve, so a read-only store cannot arm.
                        crate::dfu::status("refused (no_card): the store is not writable");
                        Some(obc_app::DfuInstallError::NoCard)
                    } else {
                        None
                    };
                    match refusal {
                        Some(error) => {
                            RideExec::deliver(&mut exec.outcomes.dfu, DfuOutcome::InstallFailed { token, error }, "dfu")
                        }
                        None => {
                            // The guards passed: answer the arm now, so this frame's pass swaps the
                            // spinner for the static "Installing update" card and the ordinary render
                            // and present put that frame on glass. The arm itself runs in the store
                            // tail, after the present: the warm reset into the bootloader never
                            // paints, so the panel holds that frame for the whole snapshot and flash.
                            RideExec::deliver(&mut exec.outcomes.dfu, DfuOutcome::InstallBegan { token }, "dfu");
                            exec.arm_pending = Some(0);
                        }
                    }
                }
                DfuEffect::Scan { token } => {
                    // The read-only "Checking card..." step: validate the staged package and answer
                    // the app. The scan touches nothing, so it needs no ride-state guard.
                    let result = {
                        let mut store_guard = shared.lock().await;
                        let SharedSettings { settings: settings_store, .. } = &mut *store_guard;
                        crate::dfu::run_scan(flat, settings_store, &mut wdt)
                    };
                    // Park the validated ref for the confirm's install and answer the app with just
                    // the report. A failed scan clears any prior ref, because the catalog may have
                    // changed.
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

        // One lexical block owns the store guard. The settings save, the card reconcile, the
        // per-frame sources and the map render all run under it, and the block's close is the
        // guard's death — before the present phase below, so a BLE object operation waits behind at
        // most the render and never the panel scan on top of it. The render runs while the reader
        // borrows of the open handles are live, which is what keeps an upload or delete from
        // invalidating a reader mid-render.
        let (rendered, dirty_map, hold_p, next_wake_ms, immediate, store_held_us) = {
            let mut store_guard = shared.lock().await;
            let t_store = Instant::now();
            let SharedSettings { settings: settings_store } = &mut *store_guard;

            // The bounded operations the previous pass decided. They run at the top of the store
            // phase because that is where the work they name lives: a catalog re-read must precede
            // this frame's route source and index build, and the settings write needs the store this
            // block holds.
            flat.set_route_added_at(app.clock_trusted().then(|| app.wall_unix_now()));
            if crate::flat_store::take_route_storage_full() {
                app.offer_route_cleanup(crate::flat_store::catalog_scope(flat).store);
            }
            if let Some(effect) = exec.effects.catalog.take() {
                use obc_app::catalog_state::{CatalogEffect, CatalogError, CatalogOutcome};
                match effect {
                    CatalogEffect::RemoveReview { token, source } => {
                        let result = if source.store != flat.store_id().0 {
                            Err(obc_storage::flat::StoreError::NotFound)
                        } else if let Some(writer) = crate::flat_store::writer() {
                            writer
                                .call(
                                    crate::flat_store::Request::RemoveComputedRoute {
                                        id: obc_storage::flat::ObjectId(source.object),
                                        revision: obc_storage::flat::Revision(source.revision),
                                    },
                                    &CATALOG_STORE_REPLY,
                                )
                                .await
                        } else {
                            Err(obc_storage::flat::StoreError::ReadOnly)
                        };
                        let outcome = match result {
                            Ok(_) | Err(obc_storage::flat::StoreError::NotFound) => {
                                CatalogOutcome::ReviewRemoved { token, source }
                            }
                            Err(_) => CatalogOutcome::Failed { token, error: CatalogError::RemoveFailed },
                        };
                        RideExec::deliver(&mut exec.outcomes.catalog, outcome, "catalog");
                    }
                    CatalogEffect::CleanupRoute { token, before_utc, store } => {
                        let active = app
                            .active_route_index()
                            .and_then(|i| app.route_ids().get(i).copied())
                            .map(obc_storage::flat::ObjectId);
                        match crate::flat_store::writer().ok_or(()).and_then(|w| {
                            w.try_call(
                                crate::flat_store::Request::CleanupRoute { before_utc, store, active },
                                &CATALOG_STORE_REPLY,
                            )
                        }) {
                            Ok(ticket) => exec.catalog = Some(CatalogRemoval { ticket, token, object: 0 }),
                            Err(()) => RideExec::deliver(
                                &mut exec.outcomes.catalog,
                                CatalogOutcome::Failed { token, error: CatalogError::RemoveFailed },
                                "catalog",
                            ),
                        }
                    }
                    // Rebuild the flat route, trip and ride identities and remap the app's held
                    // indices by durable object id.
                    CatalogEffect::ReadCatalog { token } => {
                        let old_source = crate::flat_store::route_source_key();
                        let read = read_catalogs(flat, app, &mut exec.facts).await;
                        let active = app.active_route_index();
                        crate::flat_store::reconcile_route(flat, active.and_then(|i| app.route_ids().get(i).copied()));
                        if read.is_ok() && old_source.is_some() && crate::flat_store::route_source_key() == old_source {
                            prev_active = active;
                            if route_index_valid {
                                index_route = active;
                            }
                        } else {
                            prev_active = None;
                            index_route = None;
                            route_index_valid = false;
                        }

                        // A partial read is answered `Unreadable`, and the domain re-offers the read
                        // from there, once per pass.
                        let outcome = match read {
                            Ok(scope) => CatalogOutcome::CatalogRead { token, scope: Some(scope) },
                            Err(error) => CatalogOutcome::Failed { token, error },
                        };
                        RideExec::deliver(&mut exec.outcomes.catalog, outcome, "catalog");
                    }
                    // The rider's removal of a route, a ride, or one step of a trip cascade, on the
                    // answering writer path. The effect is namespace-free, so the store resolves the
                    // head at that id and reports whether it was there. A full request queue is not
                    // an answer: the effect was simply not taken this pass, so the domain re-offers
                    // it.
                    CatalogEffect::RemoveObject { token, object, kind } => {
                        match crate::flat_store::writer().ok_or(()).and_then(|w| {
                            w.try_call(
                                crate::flat_store::Request::RemoveObject {
                                    id: obc_storage::flat::ObjectId(object),
                                    kind,
                                },
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

            // The in-flight removal's answer, polled without parking. `existed: false` is a success,
            // because the subject vanished before the commit and the goal state holds, while a
            // refused or failed commit is a `Failed` the domain re-queues.
            if let Some(removal) = exec.catalog.take() {
                use obc_app::catalog_state::{CatalogError, CatalogOutcome};
                let answer =
                    crate::flat_store::writer().and_then(|w| w.try_result(removal.ticket, &CATALOG_STORE_REPLY));
                match answer {
                    None => exec.catalog = Some(removal),
                    Some(Ok(crate::flat_store::Outcome::CleanedRoute(object))) => {
                        let outcome = match object {
                            Some(object) => {
                                CatalogOutcome::ObjectRemoved { token: removal.token, object: object.0, existed: true }
                            }
                            None => CatalogOutcome::CleanupFinished { token: removal.token },
                        };
                        RideExec::deliver(&mut exec.outcomes.catalog, outcome, "catalog");
                    }
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

            // The System settings screen's free-space read comes from the mounted flat store's
            // resident bitmap. `None` keeps the existing unavailable answer for an unreadable card
            // or an impossible byte-count product, so the row blanks back to `--`.
            if let Some(effect) = exec.effects.storage_info.take() {
                use obc_app::device_core::{StorageInfoEffect, StorageInfoError, StorageInfoOutcome};
                let StorageInfoEffect::MeasureFreeSpace { token } = effect;
                let free = flat
                    .mode()
                    .readable()
                    .then(|| u64::from(flat.free_extents()).checked_mul(flat.extent_size()))
                    .flatten();
                let outcome = match free {
                    Some(free_bytes) => StorageInfoOutcome::Measured { token, free_bytes },
                    None => StorageInfoOutcome::Failed { token, error: StorageInfoError::NotMounted },
                };
                RideExec::deliver(&mut exec.outcomes.storage_info, outcome, "storage");
            }

            // Open the session before serving the previous pass's first samples.
            if let Some(outcome) =
                ride_recorder.execute(flat, app, &mut opened_session, exec.effects.recorder.take(), now).await
            {
                RideExec::deliver(&mut exec.outcomes.recorder, outcome, "recorder");
            }
            if ride_recorder.take_warning() {
                exec.facts.raise_warnings(obc_app::WarningFlags::REC_ERROR);
            }

            // The catalog and checkpoint calls share one physical reply slot.
            if exec.catalog.is_none() && exec.outcomes.metadata.is_empty() {
                if let Some(effect) = exec.effects.metadata.take() {
                    use obc_app::metadata::MetadataOutcome;
                    let token = effect.token();
                    #[cfg(has_nav)]
                    let resume = app.assistant_review_status() == obc_app::navigator::ReviewStatus::Saving
                        && app.assistant_preview().is_none();
                    #[cfg(has_nav)]
                    let resume_current = !resume
                        || app.assistant_checkpoint().is_some_and(|checkpoint| {
                            flat.with_source(
                                obc_storage::flat::ObjectId(checkpoint.route.object),
                                Some(obc_storage::flat::Revision(checkpoint.route.revision)),
                                |source| {
                                    // An imported route attributes no map, so no map can be wrong for it.
                                    obc_route::RouteObjectInfo::read(source).is_ok_and(|info| {
                                        info.attribution_map.is_none_or(|attribution| {
                                            attribution == crate::flat_store::planner_map_key(flat)
                                        })
                                    })
                                },
                            )
                            .unwrap_or(false)
                        });
                    #[cfg(has_nav)]
                    let clearing = app.assistant_checkpoint_payload(token).is_some_and(|change| change.next.is_none());
                    #[cfg(has_nav)]
                    let sources_current = clearing
                        || resume_current
                            && app.assistant_review_context().is_none_or(|context| {
                                crate::flat_store::planner_map_current()
                                    && context.map == crate::flat_store::planner_map_key(flat)
                                    && context.profile == app.settings().bike_profile_idx
                                    && context.original.is_none_or(|original| {
                                        visit.original_current()
                                            || review_original.as_ref().is_some_and(|held| {
                                                held.id().0 == original.object
                                                    && held.revision().0 == original.revision
                                                    && held.is_current()
                                            })
                                    })
                            });
                    #[cfg(not(has_nav))]
                    let sources_current = app.assistant_review_context().is_none();
                    let current_scope = crate::flat_store::catalog_scope(flat);
                    let clear_scope_moved =
                        app.assistant_checkpoint_payload(token).is_some_and(|change| change.next.is_none())
                            && effect.scope().is_some_and(|issued| {
                                app.assistant_store_matches(issued.store)
                                    && issued.store == current_scope.store
                                    && issued.revision != current_scope.revision
                            });
                    let result = if !sources_current {
                        Some(Err(obc_app::metadata::MetadataError::Stale))
                    } else if clear_scope_moved
                        || !effect.scope().is_some_and(|scope| app.assistant_store_matches(scope.store))
                        || !app.assistant_checkpoint_submission(token)
                    {
                        RideExec::deliver(
                            &mut exec.outcomes.metadata,
                            MetadataOutcome::Cancelled { token },
                            "metadata",
                        );
                        None
                    } else {
                        let request = effect
                            .scope()
                            .zip(app.assistant_checkpoint_payload(token))
                            .map(|(scope, change)| crate::flat_store::Request::WriteCheckpoint { scope, change });
                        Some(match request {
                            Some(request) => metadata_call(request).await,
                            None => Err(obc_app::metadata::MetadataError::Stale),
                        })
                    };
                    if let Some(result) = result {
                        let outcome = match result {
                            Ok(()) => MetadataOutcome::CheckpointWritten { token },
                            Err(error) => MetadataOutcome::Failed { token, error },
                        };
                        RideExec::deliver(&mut exec.outcomes.metadata, outcome, "metadata");
                    }
                }
            }
            if let Some(effect) = exec.effects.bond.take() {
                if let Err(error) = crate::ble::try_forget_bond(effect) {
                    RideExec::deliver(
                        &mut exec.outcomes.bond,
                        obc_app::ble::BondOutcome::Failed { token: effect.token(), error },
                        "bond",
                    );
                }
            }
            if let Some(outcome) = crate::ble::take_bond_outcome() {
                RideExec::deliver(&mut exec.outcomes.bond, outcome, "bond");
            }

            // Navigator requests each step and commit. Physical I/O may span several passes;
            // release is acknowledged only after the pending ticket and its cleanup settle.
            #[allow(unused_mut, unused_assignments)]
            let mut nav_cancel = false;
            #[cfg(has_nav)]
            if app.assistant_review_status() == obc_app::navigator::ReviewStatus::Accepted
                && nav_run.is_none()
                && review_publication.is_some_and(|source| {
                    app.assistant_checkpoint().is_some_and(|checkpoint| checkpoint.route == source)
                })
            {
                review_publication = None;
                if let Some(source) = review_original.take() {
                    flat.close(source.release());
                }
            }
            #[cfg(has_nav)]
            visit.accepted(app, flat);
            if let Some(effect) = exec.effects.navigator.take() {
                use obc_app::navigator::{NavigatorEffect, NavigatorError, NavigatorOutcome, PlannerWork};
                #[cfg(has_nav)]
                let source_error =
                    if matches!(effect, NavigatorEffect::Acquire { work: PlannerWork::AssistantRoute(_), .. })
                        && (app.assistant_visit_target().is_some()
                            || app
                                .assistant_review_context()
                                .is_some_and(|c| matches!(c.purpose, obc_app::navigator::ReviewPurpose::Easier(_))))
                    {
                        let id = app.active_route_index().and_then(|i| app.route_ids().get(i).copied());
                        let original = id.and_then(|id| crate::flat_store::route_fingerprint(flat, id));
                        let avoidance = id.is_some_and(|id| {
                            flat.with_source(obc_storage::flat::ObjectId(id), None, |source| {
                                obc_route::RouteObjectInfo::read(source).map(|info| info.unresolved_avoidance)
                            })
                            .ok()
                            .and_then(Result::ok)
                            .unwrap_or(true)
                        });
                        !app.bind_visit_sources(crate::flat_store::catalog_scope(flat), original, avoidance)
                    } else {
                        false
                    };
                #[cfg(not(has_nav))]
                let source_error = false;
                if source_error {
                    RideExec::deliver(
                        &mut exec.outcomes.navigator,
                        NavigatorOutcome::Failed { token: effect.token(), error: NavigatorError::SourceChanged },
                        "navigator",
                    );
                }
                #[cfg(has_nav)]
                let visit_effect = !source_error && visit.accepts(&effect, app);
                #[cfg(not(has_nav))]
                let visit_effect = false;
                #[cfg(has_nav)]
                if visit_effect {
                    if let Some(outcome) = visit.accept(effect, app, flat, &mut nav_guard) {
                        RideExec::deliver(&mut exec.outcomes.navigator, outcome, "navigator");
                    }
                }
                #[cfg(has_nav)]
                let detour_effect = !source_error && !visit_effect && detour.accepts(&effect);
                #[cfg(not(has_nav))]
                let detour_effect = false;
                #[cfg(has_nav)]
                if detour_effect {
                    if let Some(outcome) =
                        detour.accept(effect, app, flat, &mut nav_guard, app.settings().bike_profile_idx)
                    {
                        RideExec::deliver(&mut exec.outcomes.navigator, outcome, "navigator");
                    }
                }
                if !source_error && !visit_effect && !detour_effect {
                    match effect {
                        #[cfg(has_nav)]
                        NavigatorEffect::Acquire {
                            token,
                            work: PlannerWork::Route(request) | PlannerWork::AssistantRoute(request),
                        } => {
                            if !crate::flat_store::planner_map_current()
                                || app.assistant_review_context().is_some_and(|context| {
                                    context.map != crate::flat_store::planner_map_key(flat)
                                        || context.store.bytes() != flat.store_id().0
                                        || context.profile != app.settings().bike_profile_idx
                                        || !crate::assistant::original_allowed(
                                            flat,
                                            context,
                                            app.active_route_index()
                                                .and_then(|index| app.route_ids().get(index).copied()),
                                        )
                                })
                            {
                                RideExec::deliver(
                                    &mut exec.outcomes.navigator,
                                    NavigatorOutcome::Failed { token, error: NavigatorError::SourceChanged },
                                    "navigator",
                                );
                            } else {
                                // The planner slot lives in the scratch arena, so the search must
                                // take the arena first, and a cable transfer into the same store
                                // outranks a reroute. A refusal names the holder and answers the
                                // operation, so no spinner hangs behind a half-claim.
                                let refusal = match nav_take_arena(app, &mut nav_guard) {
                                    Err(why) => Some(why),
                                    // Impossible through the UI, and fail-closed rather than lending
                                    // one reply slot to two live tickets: refuse the new operation, so
                                    // the rider gets the failure card instead of a spinner.
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
                                        if let Some(context) = app.assistant_review_context() {
                                            if let Some((planner, ..)) = bufs.guard.plan_parts() {
                                                planner.set_attribution_map(context.map);
                                                planner.set_assistant_candidate();
                                            }
                                            if let Some(original) = context.original {
                                                crate::assistant::release_original(flat, &mut review_original, false);
                                                review_original = crate::flat_store::planner_original(
                                                    flat,
                                                    obc_storage::flat::ObjectId(original.object),
                                                )
                                                .ok();
                                            }
                                        }
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
                        }
                        // This image ships without the router, so the workspace this operation asks
                        // for does not exist in it.
                        #[cfg(not(has_nav))]
                        NavigatorEffect::Acquire {
                            token,
                            work: PlannerWork::Route(_) | PlannerWork::AssistantRoute(_),
                        } => {
                            defmt::warn!("nav: router not built into the ble image (256K DK) — refusing the operation");
                            RideExec::deliver(
                                &mut exec.outcomes.navigator,
                                NavigatorOutcome::Failed { token, error: NavigatorError::Workspace },
                                "navigator",
                            );
                        }
                        NavigatorEffect::Acquire {
                            token,
                            work: PlannerWork::Detour(_) | PlannerWork::RestoreReview(_),
                        }
                        | NavigatorEffect::CommitDetour { token } => {
                            defmt::warn!(
                                "nav: planner operation is not supported on this board — refusing the operation"
                            );
                            RideExec::deliver(
                                &mut exec.outcomes.navigator,
                                NavigatorOutcome::Failed { token, error: NavigatorError::Workspace },
                                "navigator",
                            );
                        }
                        NavigatorEffect::Release { token, retain_result, .. } => {
                            #[cfg(has_nav)]
                            if nav_run.is_none() && !retain_result {
                                if let Some(source) = review_publication.filter(|source| {
                                    !app.retains_find_review(obc_formats::obcr::RouteSourceKey {
                                        store: flat.store_id().0,
                                        object: source.object,
                                        revision: source.revision,
                                    })
                                }) {
                                    if let Some(writer) = crate::flat_store::writer() {
                                        let result = writer
                                            .call(
                                                crate::flat_store::Request::RemoveComputedRoute {
                                                    id: obc_storage::flat::ObjectId(source.object),
                                                    revision: obc_storage::flat::Revision(source.revision),
                                                },
                                                &NAV_STORE_REPLY,
                                            )
                                            .await;
                                        if result.is_err() {
                                            RideExec::deliver(
                                                &mut exec.outcomes.navigator,
                                                NavigatorOutcome::ReleaseUnresolved { token },
                                                "navigator",
                                            );
                                            continue;
                                        }
                                        crate::flat_store::load_routes(flat, app);
                                    } else {
                                        RideExec::deliver(
                                            &mut exec.outcomes.navigator,
                                            NavigatorOutcome::ReleaseUnresolved { token },
                                            "navigator",
                                        );
                                        continue;
                                    }
                                }
                                review_publication = None;
                                if let Some(source) = review_original.take() {
                                    flat.close(source.release());
                                }
                                nav_guard = None;
                                RideExec::deliver(
                                    &mut exec.outcomes.navigator,
                                    NavigatorOutcome::Released { token },
                                    "navigator",
                                );
                                continue;
                            }
                            nav_cancel = true;
                            #[cfg(has_nav)]
                            {
                                exec.nav_token = Some(token);
                                if let Some(run) = nav_run.as_mut() {
                                    if let NavIo::Published(id) = run.io {
                                        run.io = if retain_result
                                            || app.retains_find_review(obc_formats::obcr::RouteSourceKey {
                                                store: flat.store_id().0,
                                                object: id.0,
                                                revision: 1,
                                            }) {
                                            NavIo::Complete
                                        } else {
                                            NavIo::NeedPublishCompensation(id)
                                        };
                                    }
                                }
                            }
                            #[cfg(not(has_nav))]
                            let _ = retain_result;
                            #[cfg(not(has_nav))]
                            RideExec::deliver(
                                &mut exec.outcomes.navigator,
                                NavigatorOutcome::Released { token },
                                "navigator",
                            );
                        }
                        #[cfg(has_nav)]
                        NavigatorEffect::Step { token } | NavigatorEffect::CommitRoute { token } => {
                            let next = nav_run.as_ref().and_then(|run| match (&effect, &run.io) {
                                (NavigatorEffect::Step { .. }, NavIo::Ready) => Some(NavIo::StepRequested),
                                (NavigatorEffect::CommitRoute { .. }, NavIo::ReadyCommit(stats)) => {
                                    Some(NavIo::NeedFinish(obc_route::Step::Done(*stats)))
                                }
                                _ => None,
                            });
                            let error = if !crate::flat_store::planner_map_current()
                                || app.assistant_review_context().is_some_and(|context| {
                                    context.original.is_some()
                                        && !review_original.as_ref().is_some_and(|source| source.is_current())
                                }) {
                                Some(NavigatorError::SourceChanged)
                            } else if next.is_none() {
                                Some(NavigatorError::Workspace)
                            } else {
                                None
                            };
                            if let Some(error) = error {
                                RideExec::deliver(
                                    &mut exec.outcomes.navigator,
                                    NavigatorOutcome::Failed { token, error },
                                    "navigator",
                                );
                            } else if let Some(run) = nav_run.as_mut() {
                                exec.nav_token = Some(token);
                                run.io = next.unwrap();
                            }
                        }
                        #[cfg(not(has_nav))]
                        NavigatorEffect::Step { token } | NavigatorEffect::CommitRoute { token } => {
                            RideExec::deliver(
                                &mut exec.outcomes.navigator,
                                NavigatorOutcome::Failed { token, error: NavigatorError::Workspace },
                                "navigator",
                            );
                        }
                    }
                }
            }

            #[cfg(has_nav)]
            if let Some(writer) = crate::flat_store::writer() {
                if let Some(outcome) = visit.poll(
                    app,
                    flat,
                    writer,
                    &mut nav_guard,
                    flat_map,
                    map_tables,
                    map_cache,
                    &mut *nav.elev,
                    &NAV_STORE_REPLY,
                ) {
                    RideExec::deliver(&mut exec.outcomes.navigator, outcome, "navigator");
                }
                if let Some(outcome) = detour.poll(
                    app,
                    flat,
                    writer,
                    &mut nav_guard,
                    flat_map,
                    map_tables,
                    map_cache,
                    &mut *nav.elev,
                    &NAV_STORE_REPLY,
                ) {
                    RideExec::deliver(&mut exec.outcomes.navigator, outcome, "navigator");
                    prev_active = None;
                    index_route = None;
                }
            }

            #[cfg(has_nav)]
            {
                // Whether this pass ended the search: the one place the arena's nav arm is given
                // back. It is a flag rather than an inline release, because the guard is borrowed by
                // the step view below and must die first.
                let mut search_ended = false;
                // A `Release` and an `Acquire` can never arrive in one pass: the navigator slot holds
                // exactly one effect, and releases are offered before new work.
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
                    use obc_app::navigator::{NavigatorError, NavigatorOutcome, PlannerProgress};
                    let mut finished = None;
                    let mut finished_review = None;
                    let mut progressed = None;
                    let mut acquired = false;
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
                                        acquired = !run.cancel_requested;
                                        run.io = if run.cancel_requested {
                                            NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath))
                                        } else {
                                            NavIo::Ready
                                        };
                                    }
                                    _ if run.cancel_requested => cancelled = true,
                                    _ => finished = Some(Err(NavigatorError::Store)),
                                }
                            }
                        }
                        NavIo::Ready | NavIo::ReadyCommit(_) | NavIo::Complete | NavIo::Published(_) => {
                            if run.cancel_requested {
                                if run.allocation.is_some() {
                                    run.io = NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath));
                                } else {
                                    cancelled = true;
                                }
                            }
                        }
                        NavIo::StepRequested => {
                            if run.cancel_requested {
                                run.io = NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath));
                            } else {
                                let map_src = flat_map;
                                let mut bufs = NavBuffers { guard, elev: &mut *nav.elev };
                                // The step's view over the arena arm and the resident terrain, alive
                                // only for these synchronous calls. A `None` here would mean the
                                // bookkeeping and the arm disagree, and the step below answers the
                                // failure tier for the same reason.
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
                                    match step.outcome {
                                        obc_route::Step::Running => {
                                            progressed = Some(PlannerProgress::Searching);
                                            NavIo::Ready
                                        }
                                        obc_route::Step::Done(stats) => {
                                            progressed = Some(PlannerProgress::Reached);
                                            NavIo::ReadyCommit(stats)
                                        }
                                        obc_route::Step::Failed(error) => {
                                            finished = Some(Err(NavigatorError::Plan(error)));
                                            NavIo::Complete
                                        }
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
                                        run.io = if run.cancel_requested {
                                            NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath))
                                        } else {
                                            match outcome {
                                                obc_route::Step::Running => {
                                                    progressed = Some(PlannerProgress::Searching);
                                                    NavIo::Ready
                                                }
                                                obc_route::Step::Done(stats) => {
                                                    progressed = Some(PlannerProgress::Reached);
                                                    NavIo::ReadyCommit(stats)
                                                }
                                                obc_route::Step::Failed(error) => {
                                                    finished = Some(Err(NavigatorError::Plan(error)));
                                                    NavIo::Complete
                                                }
                                            }
                                        };
                                    }
                                    _ => {
                                        if run.cancel_requested {
                                            run.io =
                                                NavIo::NeedFinish(obc_route::Step::Failed(obc_route::NavError::NoPath));
                                        } else {
                                            finished = Some(Err(NavigatorError::Store));
                                        }
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
                                        Some(name) => crate::flat_store::Request::PublishComputedRoute {
                                            allocation,
                                            name,
                                            original: review_original
                                                .as_ref()
                                                .map(|source| (source.id(), source.revision())),
                                        },
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
                            } else if run.cancel_requested {
                                cancelled = true;
                            } else {
                                finished = Some(Err(NavigatorError::Store));
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
                                                    if let Some(context) = app.assistant_review_context() {
                                                        if let Some(source) =
                                                            crate::flat_store::route_fingerprint(flat, id)
                                                        {
                                                            review_publication = Some(source);
                                                            finished_review = flat
                                                                .with_source(
                                                                    obc_storage::flat::ObjectId(id),
                                                                    Some(obc_storage::flat::Revision(source.revision)),
                                                                    |bytes| {
                                                                        if let Some(target) =
                                                                            app.assistant_visit_target()
                                                                        {
                                                                            target
                                                                                .validate_destination(
                                                                                    bytes,
                                                                                    context.profile,
                                                                                )
                                                                                .map_err(|_| {
                                                                                    NavigatorError::Unavailable
                                                                                })?;
                                                                        }
                                                                        obc_app::navigator::ReviewedRoute::read(
                                                                            source, bytes, context,
                                                                        )
                                                                    },
                                                                )
                                                                .ok();
                                                        }
                                                    }
                                                    crate::flat_store::load_routes(flat, app);
                                                    if app.assistant_review_context().is_none() {
                                                        let _ = crate::flat_store::reconcile_route(flat, Some(id));
                                                    }
                                                    finished = Some(Ok((id, len)));
                                                }
                                                obc_app::host::NavPublishDisposition::Compensate(id) => {
                                                    run.io =
                                                        NavIo::NeedPublishCompensation(obc_storage::flat::ObjectId(id));
                                                }
                                            }
                                        }
                                        result => {
                                            if matches!(
                                                result,
                                                Err(obc_storage::flat::StoreError::Media
                                                    | obc_storage::flat::StoreError::ReadOnly)
                                            ) {
                                                finished = Some(Err(NavigatorError::DurabilityUnknown));
                                            } else if run.cancel_requested {
                                                run.io = NavIo::NeedFinish(obc_route::Step::Failed(
                                                    obc_route::NavError::NoPath,
                                                ));
                                            } else {
                                                finished = Some(Err(
                                                    if matches!(result, Err(obc_storage::flat::StoreError::NotFound)) {
                                                        NavigatorError::SourceChanged
                                                    } else {
                                                        NavigatorError::Store
                                                    },
                                                ));
                                            }
                                        }
                                    }
                                } else if run.cancel_requested {
                                    cancelled = true;
                                } else {
                                    run.allocation = None;
                                    finished = Some(Err(NavigatorError::Store));
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
                                    // Every other refusal is permanent or an invariant violation.
                                    // `ReadOnly` cannot be repaired in-session, and publish reserved a
                                    // second sequence, so seeing it here means another writer
                                    // consumed that last slot.
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
                                        // Never report route success, and never retain the guard or
                                        // the planner arena forever. This should be unreachable under
                                        // the publish path's two-sequence admission invariant.
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
                    if cancelled {
                        defmt::info!("nav route: released after {=u64} ms", run.t0.elapsed().as_millis());
                        search_ended = true;
                    } else {
                        if let Some(result) = finished {
                            let mut bufs = NavBuffers { guard, elev: &mut *nav.elev };
                            nav_finish(&mut bufs, &run, result, now);
                            run.io = match result {
                                Ok((id, _)) => NavIo::Published(obc_storage::flat::ObjectId(id)),
                                Err(_) => NavIo::Complete,
                            };
                            if let Some(token) = exec.nav_token.take() {
                                let outcome = match result {
                                    Ok(_) if app.assistant_review_context().is_some() => match finished_review {
                                        Some(Ok(preview)) => {
                                            guard.begin_sources();
                                            let shape = flat.with_source(
                                                obc_storage::flat::ObjectId(preview.source.object),
                                                Some(obc_storage::flat::Revision(preview.source.revision)),
                                                |source| crate::assistant::preview_shape(guard.sources().1, source),
                                            );
                                            match shape {
                                                Ok(Ok(shape)) => {
                                                    let outcome = app.assistant_preview_outcome(token, preview);
                                                    if matches!(outcome, NavigatorOutcome::ReviewReady { .. })
                                                        && !app.set_assistant_preview_shape(
                                                            token,
                                                            preview.source,
                                                            &shape,
                                                        )
                                                    {
                                                        NavigatorOutcome::Failed {
                                                            token,
                                                            error: NavigatorError::SourceChanged,
                                                        }
                                                    } else {
                                                        outcome
                                                    }
                                                }
                                                _ => NavigatorOutcome::Failed {
                                                    token,
                                                    error: NavigatorError::DurabilityUnknown,
                                                },
                                            }
                                        }
                                        Some(Err(error)) => NavigatorOutcome::Failed { token, error },
                                        None => {
                                            NavigatorOutcome::Failed { token, error: NavigatorError::DurabilityUnknown }
                                        }
                                    },
                                    Ok((route, _)) => NavigatorOutcome::PlanFinished { token, route },
                                    Err(NavigatorError::DurabilityUnknown) if run.cancel_requested => {
                                        search_ended = true;
                                        NavigatorOutcome::ReleaseUnresolved { token }
                                    }
                                    Err(error) => NavigatorOutcome::Failed { token, error },
                                };
                                RideExec::deliver(&mut exec.outcomes.navigator, outcome, "navigator");
                            }
                            if app.assistant_review_context().is_none() {
                                prev_active = None;
                                index_route = None;
                            }
                        } else if acquired || progressed.is_some() {
                            if let Some(token) = exec.nav_token.take() {
                                let outcome = match progressed {
                                    Some(progress) => NavigatorOutcome::Stepped { token, progress },
                                    None => NavigatorOutcome::Acquired { token },
                                };
                                RideExec::deliver(&mut exec.outcomes.navigator, outcome, "navigator");
                            }
                        }
                        if !search_ended {
                            nav_run = Some(run);
                        }
                    }
                }
                if search_ended {
                    if review_publication.is_some_and(|source| {
                        app.retains_find_review(obc_formats::obcr::RouteSourceKey {
                            store: flat.store_id().0,
                            object: source.object,
                            revision: source.revision,
                        })
                    }) && app.assistant_review_status() != obc_app::navigator::ReviewStatus::Preview
                    {
                        review_publication = None;
                    }
                    crate::assistant::release_original(
                        flat,
                        &mut review_original,
                        (app.assistant_preview().is_some()
                            && app.assistant_review_status() == obc_app::navigator::ReviewStatus::Preview)
                            || app.assistant_review_status() == obc_app::navigator::ReviewStatus::Unresolved,
                    );
                    // Acknowledge only after dropping the arena guard, so the navigator can unfreeze
                    // rendering or admit a replacement operation.
                    nav_guard = None;
                    if let Some(token) = exec.nav_token.take() {
                        RideExec::deliver(
                            &mut exec.outcomes.navigator,
                            obc_app::navigator::NavigatorOutcome::Released { token },
                            "navigator",
                        );
                    }
                }
            }
            #[cfg(not(has_nav))]
            {
                let _ = nav_cancel; // no plan can be in flight — the release is inert here
                let _: &NavResident = &nav; // the unit stand-in — nothing to plan with
            }

            // A BLE Config write persists units and name to RRAM, but the live `App` copy never
            // learns. Reload the BLE-owned fields before the change-detection save below, so the UI
            // re-captions in the same session and the app's diff save cannot clobber the phone's
            // write with its own stale copy. Only units and name are BLE-writable.
            if crate::link_control::take_ble_config_written() {
                // Merge only the BLE-owned fields. `merge_ble_settings` preserves a pending device
                // edit, so neither the phone's write nor the rider's edit is lost.
                app.merge_ble_settings(&settings_store.load().unwrap_or_default());
            }

            // Persist settings the moment an edited value leaves the settings subtree: one in-place
            // RRAM line, skipped when nothing is owed. The write is acknowledged back by revision, so
            // a failed one stays retryable and surfaces the advisory warning card.
            if let Some(effect) = exec.effects.settings.take() {
                use obc_app::settings::{SettingsEffect, SettingsOutcome};
                let outcome = match effect {
                    SettingsEffect::PersistRevision { token, revision } => match settings_store.save(app.settings()) {
                        Ok(()) => {
                            // The RRAM blob just moved, so the BLE config-read cache is stale. Flag
                            // it, so the BLE plane refreshes before its next read.
                            crate::link_control::mark_device_settings_changed();
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
                };
                RideExec::deliver(&mut exec.outcomes.settings, outcome, "settings");
            }

            // Every effect this frame carried has now been offered a home. Anything left is a domain
            // with no board executor at all, and saying so loudly is what stops it becoming a silent
            // wedge.
            if exec.effects.has_pending() {
                defmt::error!("exec: an effect this board cannot serve was decided");
            }

            // A debug camera-scale command: pin the map to an exact metres-per-pixel and force one
            // redraw, so a host zoom sweep gets exactly one stage-timed frame per setting.
            #[cfg(feature = "debug-uart")]
            if let Some(mpp) = obc_platform::debug_link::take_zoom() {
                app.set_map_mpp(mpp);
            }

            // A debug route-plan trigger: start a plan between two fixed coordinates exactly as the
            // confirm would, so a host can drive the resumable router repeatably and read the
            // `nav route:` breakdown.
            #[cfg(all(feature = "debug-uart", has_nav))]
            if let Some((from, to)) = obc_platform::debug_link::take_nav() {
                // Back may already have popped the screen while its cancellation is still queued. A
                // debug trigger in that window must not become a replacement plan that overwrites the
                // resident planner before the old run has released its allocation.
                if nav_run.is_some() || !app.debug_start_nav(from, to, "Bench") {
                    defmt::warn!("nav plan: ignored repeated debug N while a plan is active");
                }
            }

            if let Some(expected) = app.requested_assistant_resume() {
                use obc_storage::flat::Store;
                let exact = flat
                    .entries()
                    .find(|entry| entry.id.0 == expected.object)
                    .map(obc_storage::flat::metadata::fingerprint)
                    == Some(expected)
                    && flat.entries_ok();
                let source = crate::flat_store::reconcile_route(flat, exact.then_some(expected.object));
                let reader = source
                    .filter(|source| route_index.read_into(*source).is_ok())
                    .map(|source| RouteReader::new(&route_index, source));
                app.prepare_assistant_resume(reader.as_ref());
                route_index_valid = false;
                index_route = None;
            }
            let active = app.active_route_index();
            // Re-centre the synthetic GPS onto a freshly loaded route's start, so Follow does not
            // yank the camera off it.
            #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
            if active != prev_route {
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    synth.recenter(r.start_lon, r.start_lat);
                }
                prev_route = active;
            }

            if active != prev_active {
                let active_id = active.and_then(|i| app.route_ids().get(i).copied());
                crate::flat_store::reconcile_route(flat, active_id);
                prev_active = active;
            }

            // Cache the active route's chunk index across frames: rebuild it only when the route
            // changes, or retry if a build failed. It is not gated on rendering, because the matcher
            // needs the index on every fresh fix.
            if index_route != active {
                route_index_valid = false;
                match active {
                    Some(_) => {
                        // In place into the resident slot: a by-value build here is the
                        // stack-overflow footgun.
                        let id = active.and_then(|i| app.route_ids().get(i).copied());
                        let source = crate::flat_store::reconcile_route(flat, id);
                        if source.is_some_and(|source| route_index.read_into(source).is_ok()) {
                            route_index_valid = true;
                            index_route = active; // cached — no more rebuilds until the route changes
                        } else {
                            // Transient card glitch: leave the key mismatched so every frame retries,
                            // hiding the route this frame rather than the whole ride.
                            index_route = None;
                            defmt::warn!("flat: route index read failed — retrying next frame");
                        }
                    }
                    None => {
                        index_route = None;
                    }
                }
            }
            // This frame's route reader is the cached index plus a fresh geometry source, both cheap.
            // Geometry streams lazily where it is read: the matcher on a fresh fix, the renderer on a
            // redraw frame.
            let id = active.and_then(|i| app.route_ids().get(i).copied());
            let route_src = crate::flat_store::reconcile_route(flat, id);
            let route = match (route_index_valid.then_some(&route_index), route_src) {
                (Some(idx), Some(src)) => Some(RouteReader::new_cached(idx, src, route_cache)),
                _ => None,
            };
            // The selected route owes a durable checkpoint once per selection. The entry walk is
            // why it waits for the frame that already holds the parsed route.
            if let (Some(id), Some(reader)) = (app.requested_route_checkpoint(), route.as_ref()) {
                let source = crate::flat_store::route_fingerprint(flat, id).map(|route| {
                    obc_app::navigator::RouteCheckpointSource {
                        route,
                        distance_m: reader.total_distance_m,
                        unresolved_avoidance: reader.has_unresolved_avoidance(),
                    }
                });
                app.offer_route_checkpoint(id, source);
            }

            // One tight scope that ends before the next `.await`. The polyline buffer, the gesture
            // batch and the `PassPlan` are stack temporaries here; a binding still live across an
            // await would become a permanent slot in this task's future, which on this board is
            // resident `.bss`.
            app.bind_place_map(Some(crate::flat_store::planner_map_key(flat)));
            let plan = {
                // The keyed derived reads, answered immediately before the pass, into that stack
                // buffer and handed straight into `PassInputs::targets`: a polyline is 512 B and the
                // board's resident headroom is two orders of magnitude smaller, so no executor-owned
                // copy may exist.
                //
                // At most one read per pass. The need is a level and re-emits, so a second want lands
                // a pass later.
                let mut derived_pts: heapless::Vec<(i32, i32), { obc_app::NAV_PREVIEW_MAX }> = heapless::Vec::new();
                let mut derived = obc_app::device_core::DerivedInputs::NONE;
                if let Some(key) = exec.needs.ride_track {
                    // Stream the flat ride object once into the app's resident profile buffer and
                    // this frame's stack buffer.
                    let filled = crate::flat_store::fill_ride_track(flat, app, key.ride, &mut derived_pts);
                    // The profile is filled in place, which invalidates the view, so the `view`
                    // generation the answer carries must be the one the need has after the fill.
                    //
                    // The subject is the opposite: it must stay the one that was actually read. A
                    // delete landing between the plan and this fill can move the viewed ride, and
                    // answering under the new ride's key would put one ride's profile and polyline on
                    // another ride's detail screen. A moved subject is answered by not answering: the
                    // need re-emits and the next pass reads the ride the rider is looking at.
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
                    // The Route overview's shape preview. The previewed route is the active one, and
                    // its reader was built just above. It is answered either way, because a failure is
                    // an answer: an unreadable route settles with no shape instead of re-firing the
                    // level forever.
                    derived.nav_preview = Some(match route.as_ref() {
                        Some(r) => {
                            let points = if key.assistant {
                                r.assistant_preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>()
                            } else {
                                Ok(r.preview_polyline::<{ obc_app::NAV_PREVIEW_MAX }>())
                            };
                            match points {
                                Ok(points) => {
                                    let _ = derived_pts.extend_from_slice(&points);
                                    obc_app::device_core::DerivedInput::filled(key)
                                }
                                Err(_) => obc_app::device_core::DerivedInput::failed(key),
                            }
                        }
                        None => obc_app::device_core::DerivedInput::failed(key),
                    });
                }
                let targets = if exec.needs.ride_track.is_some() {
                    obc_app::device_core::DerivedTargets { ride_preview: derived_pts.as_slice(), nav_preview: &[] }
                } else {
                    obc_app::device_core::DerivedTargets { ride_preview: &[], nav_preview: derived_pts.as_slice() }
                };

                // This frame's chords, above the screen stack: a drawer opens or closes before the
                // gestures below are handed to whatever screen it left on top. The constituents were
                // swallowed by the recogniser, so nothing here can also move the selection.
                while let Ok(chord) = CHORDS.try_receive() {
                    let acted = app.apply_chord(chord);
                    defmt::info!(
                        "input: chord {=str} on {=str} ({=str})",
                        chord_name(chord),
                        app.top_screen().name(),
                        if acted { "opened/closed" } else { "refused" }
                    );
                }

                // This frame's gestures, as one batch. The pass applies them in order and drops a
                // `Hold` or `BackHold` queued behind a stack-changing gesture rather than delivering
                // it to the screen that replaced its target. Collected with no `.await` in between,
                // so the batch is a stack temporary.
                let mut gestures: heapless::Vec<obc_app::Gesture, { crate::input_plane::GESTURE_QUEUE }> =
                    heapless::Vec::new();
                while let Ok(g) = GESTURES.try_receive() {
                    // Every drained gesture, with the screen it lands on: the record that separates
                    // "the press never happened" from "the press landed on the wrong screen".
                    if let obc_app::Gesture::Step(n) = g {
                        defmt::info!("input: Step {=i32} on {=str}", n, app.top_screen().name());
                    } else {
                        defmt::info!("input: {=str} on {=str}", gesture_name(g), app.top_screen().name());
                    }
                    if gestures.push(g).is_err() {
                        defmt::warn!("input: {=str} dropped — the frame's gesture batch is full", gesture_name(g));
                    }
                }
                // One `App::run_pass` per frame, here, because this is the only point where the
                // sensors and the route reader are both live at once.
                // The store's own level, reported after this frame's staged effects. The store's
                // monotonic sequence is the revision, so the board reports a level rather than
                // counting commit edges. It is reported here, and not with the other levels at the
                // top of the frame, because the staged effects above are where a commit happens: a
                // level sampled before them carries the pre-commit value, which splits the commit's
                // revision and the recorder's finalized fact across two passes and makes the domain
                // read the store twice for one save.
                exec.facts.note_store_revision(crate::flat_store::catalog_scope(flat));
                peak_view.update(app, &Reader::new(flat_map, map_tables, map_cache));
                let clock = obc_app::device_core::PassClock { ride: RideClock(now), ui: InputClock(now) };
                // The hub sources are call-expression temporaries: they are stateless one-pointer
                // drains, and binding them for the loop's lifetime would park one hub pointer per
                // source in this task's future for no behavioural difference. That is why the three
                // builds each spell the whole call.
                #[cfg(feature = "debug-uart")]
                let plan = app.run_pass(obc_app::device_core::PassInputs {
                    now: clock,
                    gestures: &gestures,
                    sensors: Sensors {
                        altimeter: Some(&mut debug_alt),
                        compass: Some(&mut debug_compass),
                        fuel: Some(&mut fuel),
                        // Host-injected values land in the shared hub mailboxes, and a real strap
                        // feeds the same ones.
                        hr: Some(&mut consumer.hr()),
                        power: Some(&mut consumer.power()),
                        cadence: Some(&mut consumer.cadence()),
                        // No thermometer on this build, and the host feed streams no GPS time yet.
                        ..Sensors::new(&mut debug_loc)
                    },
                    route: route.as_ref(),

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
                        // The central manager feeds the shared hub mailboxes.
                        hr: Some(&mut consumer.hr()),
                        power: Some(&mut consumer.power()),
                        cadence: Some(&mut consumer.cadence()),
                        ..Sensors::new(&mut consumer.location())
                    },
                    route: route.as_ref(),

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

                    support: BOARD_SUPPORT,
                    outcomes: &mut exec.outcomes,
                    facts: &mut exec.facts,
                    derived,
                    targets,
                });
                plan
            };

            // The hold-cancel latch is the board's, and `stage_input` deliberately does not drain it:
            // a gesture that changed the screen stack invalidates a hold charging right now on the
            // high-priority plane, whose recogniser only this side can cancel.
            if app.take_hold_cancel() {
                display.cancel_holds();
            }
            // The `PassPlan` never crosses an `.await`: the three fields the tail needs are copied
            // out here and the plan is dropped inside the store phase.
            let obc_app::device_core::PassPlan { render, next_wake_ms: _, derived_needs, sources, effects, immediate } =
                plan;
            peak_view.reconcile(app);

            // Reconcile after input and fix delivery, so opening, fulfillment and leaving take effect
            // before this pass sleeps.
            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
            {
                let power = desired_sensor_power(app);
                if power != prev_power {
                    prev_power = power;
                    control.set_power(power);
                }
            }
            exec.needs = derived_needs;
            debug_assert!(
                !exec.effects.has_pending(),
                "every staged effect is served in this frame's store phase before the next plan lands"
            );
            exec.effects = effects;

            // The BLE acting half: what the pass just decided, out to the radio plane. Both of these
            // key on state this frame's gestures produced, so they read the app after the pass.
            {
                // While the Sensors screen's scan list is up, keep a discovery scan running.
                // `request_scan` must not be rung every pass: it pulses the manager's work edge,
                // which the manager's own scan window selects on, so a per-pass ring would collapse
                // that window. Ring once on the rising edge, then re-arm every 9 s.
                if app.sensor_scan_active() {
                    // `now >= rearm` in wrapping-monotonic terms; `0` is the "not scanning yet"
                    // sentinel that fires on the rising edge.
                    let due = sensor_scan_rearm_ms == 0 || now.wrapping_sub(sensor_scan_rearm_ms) as i32 >= 0;
                    if due {
                        crate::ble::request_scan();
                        sensor_scan_rearm_ms = now.wrapping_add(9_000).max(1); // never 0 (the "off" sentinel)
                    }
                } else {
                    // Falling edge: the scan list closed. Cancel discovery, because the re-arm may
                    // have left a stale scan request latched that would outrank the fresh save and
                    // hold the connect hostage for a whole window.
                    if sensor_scan_rearm_ms != 0 {
                        crate::ble::cancel_scan();
                    }
                    sensor_scan_rearm_ms = 0; // reset so the next entry rings on its rising edge
                }
                // Saved-sensor reconcile: the persisted `Settings.saved_sensors` is the source of
                // truth. Diff each slot against what was last pushed and drive the change through the
                // save and forget latches, fired once per change.
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

            if app.sample_terrain(&mut *nav.elev) {
                elev_fixes = elev_fixes.wrapping_add(1);
                if elev_fixes.is_multiple_of(64) {
                    let a = app.recorder.altitude();
                    let baro = app.recorder.baro_elevation_m().unwrap_or(f32::NAN);
                    defmt::debug!(
                        "altfuse: raw={=f32} m offset={=f32} m fused={=f32} m acc={=u32} gated={=u32} reseeds={=u16}",
                        baro,
                        a.offset_m().unwrap_or(f32::NAN),
                        a.fused_m(baro).unwrap_or(f32::NAN),
                        a.accepted(),
                        a.gated(),
                        a.reseeds()
                    );
                }
            }

            // Feed both hold charges: Select so the in-screen confirm bars track the hold, Back so a
            // card defers while it charges. `App`'s own input plane is not driven here, so without
            // this the render reads 0 and no hold is seen at all.
            let (hold_p, back_hold_p) = display.hold_progress();
            app.set_hold_progress(hold_p, back_hold_p);

            // The pass's own render decision is this frame's dirty signal. What it replaces is the
            // input to the board redraw folds below, not the folds: each demand here is physical and
            // stays the board's. Every one of them is full-frame, so each also drops a region-scoped
            // clip; the region only survives when the pass's own ticks were the sole dirt.
            let mut dirty = render;
            // Keep an already-painted loading base while reader work advances between planner owners.
            find_loading_painted &= app.find_preparing() && app.find_place_state() != obc_app::find_place::State::Start;
            #[cfg(has_nav)]
            let find_can_prepare = nav_guard.is_none();
            #[cfg(not(has_nav))]
            let find_can_prepare = true;
            let review_pending = app.assistant_route_pending();
            if (find_loading_painted || review_pending) && find_can_prepare {
                let reader = Reader::new(flat_map, map_tables, map_cache);
                app.prepare_find(Some(&reader), route.as_ref());
                if (find_loading_painted && !app.find_preparing()) || (review_pending && !app.assistant_route_pending())
                {
                    if find_loading_painted && app.find_place_state() == obc_app::find_place::State::Ready {
                        defmt::info!("find: ready results={=usize}", app.find_place_result_count());
                    }
                    find_loading_painted = false;
                    dirty.map = true;
                    dirty.region = None;
                }
            }
            if pending_map_redraw {
                dirty.map = true;
                dirty.region = None;
            }
            pending_map_redraw = false;
            // A panel relaunch landed since the last pass: the fresh core has no frame history and
            // the diff store was reset, so schedule the full repaint even if nothing else is dirty.
            if display.take_relaunch_repaint() {
                find_loading_painted = false;
                dirty.map = true;
                dirty.region = None;
            }
            // An install is armed and this is the frame the panel keeps: the warm reset into the
            // bootloader never paints, so whatever is on glass when the tail arms stays there for the
            // whole flash and the reboot, and there is no later frame to fix it with. On an
            // irreversible path that must be a property of this code rather than of the domain's
            // dirty behaviour.
            if exec.arm_pending.is_some() {
                dirty.map = true;
                dirty.region = None;
            }
            // While a hold charges on a cheap (non-map) screen, redraw it each frame so its bar
            // tracks the live progress, and once more on the frame the hold drops back to 0, so an
            // early release clears the bar instead of leaving it stuck mid-fill. A pure hold charge
            // emits no gesture, so nothing else dirties the map. It is gated so the expensive map
            // view is never re-rendered for a hold, and so a hold where no fill would draw repaints
            // nothing.
            if (hold_p > 0.0 || prev_hold_p > 0.0) && !app.base_draws_map() && app.top_wants_hold_fill() {
                dirty.map = true;
                dirty.region = None;
            }
            prev_hold_p = hold_p;

            // While a hold charges on the map view, defer expensive map redraws instead of rendering
            // them: a 150 to 300 ms map frame between two bulge pushes is the mid-charge freeze that
            // made the bulge jerky while riding. Only the map base is deferred. Once the hold fires,
            // charging drops to 0, so a navigation's redraw is never held up.
            if dirty.map && app.base_draws_map() && display.hold_charging() {
                pending_map_redraw = true;
                dirty.map = false;
            }

            // The Recalculating freeze. A planner run is live over a map base, so the map plane
            // holds still until it answers, and the reflective panel keeps the last frame on glass
            // for free. The map redraw is skipped, not queued: it is latched into
            // `pending_map_redraw`, so nothing is lost and the catch-up lands the pass the freeze
            // lifts. The overlay still paints, and the banner is what turns a frozen screen from
            // "the device wedged" into "it is recalculating".
            //
            // That edge is the engaged level's, not the plan start's. A plan drained under the
            // opaque planning spinner freezes nothing, and the pass that puts a map base back under
            // a still-running search has no plan edge in it at all. Keyed on the plan's edge, this
            // branch would find `dirty.overlay` already spent on the chrome frame and paint nothing
            // for the rest of the search.
            let frozen = app.reroute_freeze_active() || find_loading_painted;
            if frozen && dirty.map {
                pending_map_redraw = true;
                dirty.map = false;
                dirty.region = None;
            }

            // The store phase ends here: the tuple is the block's value and `store_guard` dies at the
            // closing brace, so the present await below cannot hold the guard.
            let banner_rows = app.reroute_banner_rows(FRAME_H as f32);
            let rendered: Option<RenderedFrame> = if frozen || (!dirty.map && dirty.overlay && banner_rows.is_some()) {
                // The banner rides the overlay plane, which on this board means: draw it into the
                // resident framebuffer and let the self-diffing present push the rows it changed. It
                // deliberately does not use the composite path the hold bulge uses, whose scratch
                // holds only twelve rows. Painting into the frame is safe while its base is
                // unchanged.
                match banner_rows.filter(|_| dirty.overlay) {
                    Some((y0, rows)) => {
                        let (stats, render_us) = display.render_frame(|f: &mut crate::ls021_flpr::Frame64| {
                            let mut fbdev = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
                            // Clip the framebuffer to the banner's own band, so a future overlay item
                            // cannot repaint map pixels the freeze is preserving.
                            fbdev.set_clip(Rectangle::new(
                                Point::new(0, y0 as i32),
                                Size::new(FRAME_W as u32, rows as u32),
                            ));
                            app.render_overlay(&mut fbdev, FRAME_W as f32, FRAME_H as f32, color_fn);
                            obc_render::RenderStats::default()
                        });
                        // Named apart from the `ui frame:` line every other non-map redraw shares, so
                        // a log reader can tell a banner repaint from a menu repaint. `debug-uart`
                        // only: the harness is its only reader.
                        #[cfg(feature = "debug-uart")]
                        defmt::info!("freeze: banner repaint rows {=u16}..{=u16}", y0, y0 + rows);
                        Some(RenderedFrame { needs_map: false, sheet_only: false, stats, render_us })
                    }
                    // Mid-freeze with no edge: nothing changed on either plane, so nothing to push.
                    None => None,
                }
            } else if dirty.map {
                // The map pipeline runs only when the base screen needs the streamed `Reader`. On a
                // menu, Statistics or Home redraw it is skipped entirely: no style-table parse, no
                // `Reader` build and so no stack spike, and no map render, so such a frame costs only
                // its own draw and the push. `sources.map` is the pass's own answer to "the base
                // screen draws the map", so the reader this frame opens is the one the pass planned
                // for rather than a second derivation of the same predicate.
                let needs_map = sources.map;
                // The flat map source is resolved once at boot and skipped on chrome-only frames,
                // which keeps menu redraws free of map I/O.
                let reader = needs_map.then(|| Reader::new(flat_map, map_tables, map_cache));
                if needs_map && reader.is_none() {
                    pending_map_redraw = true;
                    defmt::warn!(
                        "map: reader build failed this frame (flaky SD?) — kept frame, retrying redraw next frame"
                    );
                    None
                } else {
                    // A base that draws the map claims the arena's render arm for the render span and
                    // gives it back at the end of this block, which is the whole `render ⊥ nav` and
                    // `render ⊥ usb` enforcement: a live search or a live transfer is literally the
                    // holder. A chrome base claims nothing and renders with no scratch at all, which
                    // is what keeps those frames drawing while another arm is out.
                    let draws_map = app.base_draws_map();
                    let mut render_guard = if draws_map { crate::arena::claim_render().ok() } else { None };
                    let photo_active = app.photo_base_active();
                    let mut photo_guard = if photo_active { crate::arena::claim_photo() } else { None };
                    // Unreachable on the ordinary path, so a refusal is a gating bug, and
                    // `arena::claim_render` already reports it loudly. Degrade the way every other
                    // transient render failure does: keep the frame on glass and retry next pass.
                    if (draws_map && render_guard.is_none()) || (photo_active && photo_guard.is_none()) {
                        pending_map_redraw = true;
                        defmt::warn!(
                            "map: the scratch arena is held by {} — skipping this map redraw, retrying next frame",
                            defmt::Debug2Format(&crate::arena::owner())
                        );
                        None
                    } else {
                        // Render the whole frame into the resident plane, behind
                        // `MapDisplay::render_frame`. The present below, after the guard is gone,
                        // scans it out and goes around a live bulge's rows.
                        //
                        // A surviving `dirty.region` clips the render at both layers: the app's
                        // canvas rejects whole primitives whose bounds miss the region, and the
                        // framebuffer discards a straddler's out-of-region pixel writes. A spinner
                        // frame then costs the disc instead of the whole chrome.
                        //
                        // `needs_map` reads "the `Reader` was built", which is narrower than "the
                        // base draws the map": a sheet-only frame over the riding Map skips the
                        // base's draw, so it arrives here `false`.
                        let clip = if needs_map || photo_active { None } else { dirty.region };
                        app.set_render_clip(clip);
                        // Sampled before the render closure borrows `app`.
                        let sheet_only = app.sheet_only();

                        {
                            #[cfg(feature = "sd-bench")]
                            let read_before = crate::card_io::read_perf_snapshot();
                            let (stats, render_us) = display.render_frame(|f: &mut crate::ls021_flpr::Frame64| {
                                let mut fbdev = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
                                if let Some(r) = clip {
                                    fbdev.set_clip(r);
                                }
                                // One scene, because there is one map file: the `Reader` is both the
                                // geometry source and the POI, hours and nav one.
                                let panorama = peak_view.panorama();
                                let stats = app.render_scene_map_photo_timed(
                                    render_guard.as_deref_mut(),
                                    &mut fbdev,
                                    reader.as_ref(),
                                    route.as_ref(),
                                    panorama,
                                    FRAME_W as f32,
                                    FRAME_H as f32,
                                    color_fn,
                                    &InstantClock,
                                    photo_guard
                                        .as_deref_mut()
                                        .map(|runtime| obc_app::photo::FramePhoto::interactive(runtime, true)),
                                );

                                app.render_planning_banner(&mut fbdev, FRAME_W as f32, FRAME_H as f32, color_fn);
                                stats
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
                            // The guard, when a map base took one, dies here at the end of the render
                            // span, never across the present's await.
                            drop(render_guard);
                            Some(RenderedFrame { needs_map, sheet_only, stats, render_us })
                        }
                    }
                }
            } else if app.photo_pending() {
                if let Some(mut photo) = crate::arena::claim_photo() {
                    let reader = Reader::new(flat_map, map_tables, map_cache);
                    let (stats, render_us) = display.render_frame(|f: &mut crate::ls021_flpr::Frame64| {
                        let mut target = FbDevice64::new(f.bytes_mut(), FRAME_W as u32, FRAME_H as u32);
                        app.render_scene_map_photo_timed(
                            None,
                            &mut target,
                            Some(&reader),
                            route.as_ref(),
                            None,
                            FRAME_W as f32,
                            FRAME_H as f32,
                            color_fn,
                            &InstantClock,
                            Some(obc_app::photo::FramePhoto::interactive(&mut photo, false)),
                        )
                    });
                    Some(RenderedFrame { needs_map: false, sheet_only: false, stats, render_us })
                } else {
                    None
                }
            } else {
                None
            };
            if dirty.map && rendered.is_some() {
                find_loading_painted = app.find_preparing();
            }
            // Rendering can arm a marquee wake after the pass plan was made.
            (rendered, dirty.map, hold_p, app.ms_until_next_wake(now), immediate, t_store.elapsed().as_micros())
        };

        // Present phase, guard-free: the panel scans the frame with the store released, so a BLE
        // object operation interleaves with the scan instead of queueing behind it. `presented_ok`
        // anchors the DFU trial confirm in the tail.
        let mut presented_ok = false;
        if let Some(rf) = rendered {
            // A frame that a warm reset is about to freeze on the panel is presented full-frame: the
            // arm's `exclude` would leave the bulge rows showing the previous screen for the whole
            // install. Every other frame goes around a live bulge as usual.
            let exclude = if exec.arm_pending.is_some() { None } else { overlay_span };
            let (ok, push_us) = display.present_frame(exclude).await;
            presented_ok = ok;
            if ok {
                peak_view.note_frame_presented(app);
            }

            // Snapshot this frame's render stats for the host telemetry line. The reader is not
            // `TimedSource`-wrapped, so the card I/O folds into `collect_us`, and the bulge
            // composites on its own push, so `overlay_us` stays 0.
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

            // A transport fault latches a retry, like the reader-build failure, rather than
            // faulting.
            if !ok {
                find_loading_painted = false;
                pending_map_redraw = true;
            }

            // Three lines, because there are three kinds of frame and this record is what each one is
            // measured with. A sheet frame drew the sheet over a base it left standing, so it carries
            // neither map stats nor the claim that a whole screen was redrawn. A map frame carries
            // the map render stats. Anything else really is its own chrome.
            if rf.sheet_only {
                defmt::info!(
                    "sheet frame: render {=u64} us + push {=u64} us (sheet band, base left standing)",
                    rf.render_us,
                    push_us
                );
            } else if rf.needs_map {
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
                // A menu, Statistics or Home redraw: just its own chrome plus the self-diffed push.
                defmt::info!(
                    "ui frame: render {=u64} us + push {=u64} us (screen redraw, no map)",
                    rf.render_us,
                    push_us
                );
            }
        }

        // The panel's brightness follows whatever the app says it should be this frame: the quick
        // drawer's live preview while its editor is open, the committed setting otherwise. That
        // derived answer is why a cancelled edit needs no undo path here.
        apply_backlight(&mut backlight, &mut backlight_level, app.backlight_level());

        // The rider completed the guarded power-off hold, and the frame that says so has just been
        // pushed. `power_off` does not return.
        if app.power_off_requested() && presented_ok {
            obc_ports::PowerOff::power_off(&mut power_off);
        }

        // The hold bulge already pushed at the top of this pass. But if a screen present just landed,
        // its `exclude` skipped the bulge rows, which still show the old frame under the bulge.
        // Re-composite them over the fresh framebuffer now, so the band never lags the screen.
        if dirty_map && overlay_span.is_some() {
            display.present_bulge(overlay_span).await;
        }

        // Store tail: a second short guard for the store work that must follow the present. The
        // trial confirm is anchored on a frame having reached glass, and the deferred ride save must
        // grind against an already-presented screen rather than delay it.
        let tail_held_us = {
            let mut store_guard = shared.lock().await;
            let t_tail = Instant::now();
            let SharedSettings { settings: settings_store } = &mut *store_guard;

            // The DFU trial confirm, once, at the health anchor. A frame just landed on glass and the
            // card mounted at boot, so if this boot is a trial, write `Idle { installed }` — the
            // whole confirm — and hand the app the one-time "updated to vX" fact. A failed first
            // present retries the anchor later, and an unconfirmed trial rolls back next boot.
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

            // The staged install arm, once, after the present. The "Installing update" card is on
            // glass now, and the warm reset into the bootloader never paints — it only parks the
            // panel pins and keeps the COM wave alternating — so the panel holds this frame for the
            // whole snapshot and flash. The arm holds the store exclusively across its whole
            // card-to-flash stream, which is why it belongs in the tail rather than in the
            // guard-free block that decided it.
            //
            // The card must be on the stack before the panel is handed to the reset: the scheduler
            // can bounce a push onto a full stack and re-queue it, and arming meanwhile would freeze
            // the previous frame for the whole flash. The wait is bounded, because the install
            // matters more than the frame.
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
                let SharedSettings { settings: settings_store, .. } = &mut *store_guard;
                // Hand the confirm's carried scan ref to the arm; it is consumed either way. Absent
                // means `run_install` re-scans. On success this never returns.
                let failed = match crate::flat_store::writer() {
                    Some(writer) => {
                        crate::dfu::run_install(flat, &writer, settings_store, &mut wdt, cached_staged.take()).await
                    }
                    None => {
                        crate::dfu::status("refused (no_card): the store's write half is not up");
                        Some(obc_app::DfuInstallError::NoCard)
                    }
                };
                if let Some(error) = failed {
                    // `InstallBegan` is the operation's terminal answer, so an arm that then failed
                    // cannot ride the same operation, and inventing a second one would be an
                    // operation the rider never started. It is a fact instead: the update did not
                    // start. The specific error tier is lost on this path and lives on the log.
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

        // The pass's two guard holds, proving that neither contains the present. `tail` spikes on the
        // rare deferred-save pass.
        defmt::debug!(
            "store guard: phase {=u64} us + tail {=u64} us (present ran guard-free)",
            store_held_us,
            tail_held_us
        );

        // Publish render-stats telemetry at about 2 Hz, throttled here and not in the TX task, so the
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

        // Event-driven sleep: block until the next real wake — a recognised gesture, a hold starting
        // to charge (a press emits no gesture, so without this arm the loop slept through the whole
        // charge and the bulge's first frame on glass was the confirm pop), a fresh sensor or host
        // datapoint, a BLE link edge (connect, disconnect and the pairing passkey, so the passkey
        // card wakes the loop from warm sleep), or the soonest screen animation deadline the app
        // reports.
        //
        // A wake does not itself require a repaint. While something is actively animating, keep the
        // short cadence so it stays fluid; otherwise use the app's next-wake deadline, bounded by the
        // watchdog cap and the transfer pacing below.
        let charging = hold_p > 0.0 || display.hold_charging();
        // "A search is live" is the app's fact, never the board's run handle: the mode is set when
        // the plan command drains and cleared by the answer, which brackets `nav_run` on both sides.
        let planning = app.core_mode() == obc_app::device_core::ModeState::Searching;
        let animating = charging || planning || pending_map_redraw || display.overlay_owed() || overlay_span.is_some();
        // The app's deadline, plus the reasons to come straight back: the plan's `immediate`, and the
        // executor's own `owed`. An outstanding store round trip takes the short animation cadence
        // instead, because spinning at full speed against a commit that runs for hundreds of
        // milliseconds would starve the task answering it.
        let immediate = immediate || peak_view.busy();
        #[cfg(has_nav)]
        let visit_immediate = visit.immediate(&NAV_STORE_REPLY, immediate || exec.owed());
        #[cfg(not(has_nav))]
        let visit_immediate = false;
        let next_ms = if visit_immediate {
            Some(0)
        } else if animating || exec.polling_store() {
            Some(LOOP_MS as u32)
        } else if immediate || exec.owed() {
            Some(0)
        } else {
            next_wake_ms
        };
        let next_ms = if peak_view.busy() { Some(0) } else { next_ms };
        // The debug-uart build keeps a 2 Hz floor, so streamed telemetry and zoom commands stay
        // responsive on an otherwise-quiet screen.
        #[cfg(feature = "debug-uart")]
        let ms = next_ms.unwrap_or(WDT_FEED_CAP_MS).min(500);
        // The indefinite sleep is capped at about half the watchdog period, so an otherwise idle
        // device still wakes to feed the dog.
        #[cfg(not(feature = "debug-uart"))]
        let ms = next_ms.unwrap_or(WDT_FEED_CAP_MS).min(WDT_FEED_CAP_MS);
        // A map upload owns the device: the card is the only thing on glass, so no animation flag or
        // short app deadline may wake the loop faster than the progress pace, because every avoided
        // repaint is about 85 ms handed back to the write path. Gestures and sensor events still wake
        // the loop early, and the feed throttle above makes those wakes repaint nothing. This must
        // stay well under the watchdog feed cap: an upload loop starving that feed is what reset the
        // device on glass.
        let ms = if map_uploading { ms.max(MAP_XFER_PACE_MS) } else { ms };
        let _ = select5(
            GESTURES.ready_to_receive(),
            INPUT_WAKE.wait(),
            // A sensor or host datapoint, or a store movement, so an upload or delete rescans the
            // catalog now rather than at the next timer wake.
            wait_host_or_sensor_event(
                #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
                consumer,
            ),
            // A BLE link edge — connect, disconnect and the pairing passkey — so the passkey card
            // wakes the loop from warm sleep.
            crate::ble::wait_status_change(),
            Timer::after_millis(ms as u64),
        )
        .await;
    }
}

async fn metadata_call(request: crate::flat_store::Request) -> Result<(), obc_app::metadata::MetadataError> {
    use obc_app::metadata::MetadataError;
    let writer = crate::flat_store::writer().ok_or(MetadataError::Unsupported)?;
    let ticket = writer.try_call(request, &CATALOG_STORE_REPLY).map_err(|()| MetadataError::Busy)?;
    match writer.finish_call(ticket, &CATALOG_STORE_REPLY).await {
        Ok(crate::flat_store::Outcome::Metadata(result)) => result,
        Err(obc_storage::flat::StoreError::ReadOnly) => Err(MetadataError::RemountRequired),
        _ => Err(MetadataError::WriteFailed),
    }
}

fn catalog_metadata_error(error: obc_app::metadata::MetadataError) -> obc_app::catalog_state::CatalogError {
    use obc_app::{catalog_state::CatalogError as C, metadata::MetadataError as E};
    match error {
        E::Stale => C::Stale,
        E::RemountRequired => C::RemountRequired,
        E::Unsupported => C::Unsupported,
        _ => C::Unreadable,
    }
}
