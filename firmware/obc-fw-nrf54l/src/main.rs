//! Board firmware for the nRF54LM20 — the shipping hardware target. It ports the shared
//! `obc-app` onto the board: the nRF HAL wiring and the LS021 display backend. One image carries
//! the ride loop, the BLE stack and the USB device plane; `ble::run` and `usb::run` are spawned
//! beside the ride loop and share RRAM settings through [`SharedSettings`].
//!
//! `--no-default-features` is mandatory: it swaps the critical-section implementation to MPSL's.
//! embassy-time runs on the GRTC (`time-driver-grtc`) because the nRF54L has no legacy RTC time
//! driver. MPSL requires HFCLK from the crystal. LFCLK stays on the MPSL-calibrated internal RC,
//! because the XO load caps are unprogrammed (see `ble.rs`).
//!
//! Memory: `build.rs` generates `memory.x`. The application starts at 0x0000_8000 — the 32K below
//! holds the `obc-boot` bootloader — and the RRAM top holds the `BOOT_STATE` (0x0017_B000) and
//! `SETTINGS` (0x0017_C000) pages. Never hard-code flash addresses: the bootloader sets VTOR
//! before the jump, so the linker map is the only authority. The M33 gets the SRAM below the
//! coprocessor carves; `firmware/tools/resource_baseline.json` holds the RAM and stack gates.

#![no_std]
#![no_main]

mod board;
mod buzzer;
// The raw-card flat store and the board adapter that binds it to sEMMC.
#[cfg(feature = "seed-rides")]
#[allow(dead_code)]
mod demo_rides;
mod flat_ride;
mod flat_store;
// The microSD host over Nordic's sEMMC soft peripheral on the FLPR: native 4-bit SD mode,
// 32 MHz reads / 21.3 MHz writes.
mod card_io;
mod semmc;
// Which soft-peripheral image owns the FLPR now: the display scan blob or the sEMMC host. Lazy
// switching, with a gate that never parks mid-scan.
mod flpr_mux;
// One RAM block time-shared by the render scratch, the nav block and the USB staging buffer,
// which are never live at the same time. The only place this feature's `unsafe` lives.
mod arena;
#[cfg(has_nav)]
mod assistant;
#[cfg(has_nav)]
mod detour;
mod peak_view;
#[cfg(has_nav)]
mod visit;
// The COM electrode square wave. The FLPR drives the frames; only this stays on the M33.
mod com;
// The COM square wave from a TIMER-DPPI-GPIOTE toggle chain instead of the M33 `com_task`, so
// the core can sleep between events. Off by default: the waveform needs hardware verification.
#[cfg(feature = "com-hw")]
mod com_hw;
// The LS021/FLPR panel — this crate's presenter backend. The display seam itself and the
// simulator backend live in obc-platform.
mod ls021_flpr;
// The high-priority input/overlay task, the gesture channel and their executor statics. The COM
// task is spawned onto the same executor.
mod input_plane;
mod map_plane;
mod panel_power;
mod ride;
mod settings;
// The app-side DFU armer over `obc_dfu::armer`: stage scan, rollback snapshot, the boot-state
// page write, the status stream, and the ride loop's trial confirm.
mod dfu;
// Real GPS (SAM-M10Q) and altimeter (BMP581) on the shared TWIM22 bus. Built only for the
// real-sensor build, because `synth` and `debug-uart` supply the location source instead.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
mod sensors;
// The BLE stack: MPSL, SDC and TrouBLE, the advertise loop, and the link-status plumbing.
mod ble;
// The USB device plane: USBHS behind a vendor interface, carrying the same companion protocol
// as the radio.
mod usb;
// The transport-free companion-link core: the command handler, descriptor classification, the
// identity blobs, and the one shared `LinkControl`. The radio and the USB plane both call into
// it, which is what keeps USB a second transport and not a second protocol.
mod link;
// The link-control cache and RRAM hand-offs for Config, bonding, clock and DFU requests.
mod link_control;

// The radio is not optional: MPSL provides the critical-section implementation, its radio timing
// forbids the interrupt-disable kind, and two implementations give duplicate link symbols. The
// one build carries the ride loop, the radio and the USB plane; `debug-uart` (VCOM-fed ride) and
// `synth` (synthetic ride) compose with it.

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
#[cfg(feature = "debug-uart")]
use embassy_nrf::peripherals;
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

use core::cell::RefCell;
use core::mem::MaybeUninit;

use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
// The async `Mutex` guards the shared SD + settings store: its guard can be held across an
// `.await`, which the BLE plane's per-chunk operations need.
use embassy_sync::mutex::Mutex;
use obc_app::InputPlane;
use obc_app::{App, AppState};
use obc_display::ls021::{RowDiff, FRAME_H, FRAME_W};
use obc_reader::{MapCache, MapTables};
use obc_render::zoom_for_mpp;
use obc_route::RouteCache;

#[cfg(not(feature = "com-hw"))]
use com::com_task;
#[cfg(feature = "com-hw")]
use com_hw::HwCom;
use input_plane::{input_task, CHORDS, EXECUTOR_HP, GESTURES};
use ls021_flpr::{launch_flpr, relaunch_flpr, FlprError, Frame64, Ls021Flpr};
use map_plane::{run_map_recovery, show_boot_fault, MapDisplay};

// The debug-sensor and telemetry stream behind `debug-uart`. `BufferedUarte` keeps RX DMA armed
// into an interrupt-driven ring, so the tens-of-ms map render never drops a streamed byte.
#[cfg(feature = "debug-uart")]
use embassy_nrf::buffered_uarte::{BufferedUarteRx, BufferedUarteTx};

/// Headroom for the main stack and embassy's executor and task arenas. The gates that bound the
/// stack are `residual_stack_min` and `deep_ride_margin_min` in
/// `firmware/tools/resource_baseline.json`, measured from the linked ELF.
#[allow(dead_code)] // read by `resource_report`
const STACK_RESERVE: usize = 64 * 1024;
/// The single RGB222 framebuffer: one byte per pixel over the 240×320 frame = 75 KB.
const FB_BYTES: usize = FRAME_W * FRAME_H;

/// The itemized resident set: the blocks that must all be live during a redraw. `resource_report`
/// publishes the parts and `resource_baseline.json` holds the gates that bind, so nothing reads
/// this sum; it is what keeps every itemized part referenced in a build without that feature.
#[allow(dead_code)]
const RESIDENT_BYTES: usize = FB_BYTES
    + core::mem::size_of::<RowDiff<FRAME_H>>()
    + MAP_RESIDENT
    + arena::ARENA_BYTES
    + ble::RESIDENT_BYTES
    + usb::RESIDENT_BYTES
    + flat_store::RESIDENT_BYTES
    + flat_ride::RESIDENT_BYTES;

/// The map plane's residents, including the route index the ride loop holds across frames. The
/// per-frame render scratch is not here: it is one arm of the scratch arena.
const MAP_RESIDENT: usize = core::mem::size_of::<obc_app::App>()
    + core::mem::size_of::<obc_reader::MapCache>()
    + core::mem::size_of::<obc_reader::MapTables>()
    + core::mem::size_of::<obc_route::RouteCache>()
    + core::mem::size_of::<obc_route::RouteIndex>()
    + TERRAIN_RESIDENT;

/// The terrain slot and the window it is parsed through. It is resident whether or not the card's
/// map carries terrain: the slot is what makes the emit path's sampler a `.bss` object instead of a
/// plan-frame local, and a budget that moved with the card's contents would not be a budget. It is
/// deliberately not an arena arm, because `App::sample_terrain` reads it at every fresh fix during a
/// ride, which is while the map plane renders and no search runs. It is state, not scratch.
#[cfg(has_nav)]
const TERRAIN_RESIDENT: usize = core::mem::size_of::<
    obc_elevation::TerrainElevation<'static, { obc_elevation::DEFAULT_TILE_SLOTS }>,
>() + core::mem::size_of::<obc_formats::io::WindowSource<'static>>();
#[cfg(not(has_nav))]
const TERRAIN_RESIDENT: usize = 0;

// A report-only table of target-side allocation sizes, kept out of every shipping ELF by
// `cfg(feature = "resource-report")`. `resource_guard.py report` extracts it without running the
// firmware. These entries are not the linked resident RAM: `.bss + .data` from the shipping ELF is
// the authority for that gate.
#[cfg(feature = "resource-report")]
mod resource_report {
    use super::*;

    const NAME_BYTES: usize = 32;

    #[derive(Clone, Copy)]
    #[repr(C)]
    pub struct Entry {
        name: [u8; NAME_BYTES],
        bytes: u32,
    }

    const fn entry(name: &str, bytes: usize) -> Entry {
        let src = name.as_bytes();
        assert!(src.len() < NAME_BYTES);
        assert!(bytes <= u32::MAX as usize);
        let mut dst = [0; NAME_BYTES];
        let mut i = 0;
        while i < src.len() {
            dst[i] = src[i];
            i += 1;
        }
        Entry { name: dst, bytes: bytes as u32 }
    }

    const BLE_ENTRIES: [Entry; 11] = [
        entry("ble_total", ble::RESIDENT_BYTES),
        entry("ble_mpsl", ble::MPSL_BYTES),
        entry("ble_sdc_memory", ble::SDC_MEM_SIZE),
        entry("ble_host_resources", ble::HOST_RESOURCES_BYTES),
        entry("ble_packet_pool", ble::PACKET_POOL_BYTES),
        entry("ble_cracen", ble::CRACEN_BYTES),
        entry("ble_object_store", ble::OBJECT_STORE_BYTES),
        entry("ble_server", ble::SERVER_BYTES),
        entry("ble_gap_name", ble::GAP_NAME_BYTES),
        entry("ble_sensor_manager", ble::SENSOR_MANAGER_BYTES),
        entry("ble_v4_adapter", ble::V4_ADAPTER_BYTES),
    ];

    /// The terrain entries are the *types'* sizes, reported in every profile, even where `has_nav`
    /// keeps the statics out of the image.
    const TERRAIN_ENTRIES: [Entry; 2] = [
        entry(
            "terrain",
            core::mem::size_of::<obc_elevation::TerrainElevation<'static, { obc_elevation::DEFAULT_TILE_SLOTS }>>(),
        ),
        entry("terrain_window", core::mem::size_of::<obc_formats::io::WindowSource<'static>>()),
    ];

    const ENTRIES: usize = 36;

    #[used]
    #[no_mangle]
    #[link_section = ".obc_resources"]
    pub static OBC_RESOURCE_TABLE: [Entry; ENTRIES] = [
        entry("format_version", 1),
        entry("framebuffer", FB_BYTES),
        entry("row_diff", core::mem::size_of::<RowDiff<FRAME_H>>()),
        entry("app", core::mem::size_of::<App>()),
        entry("map_cache", core::mem::size_of::<MapCache>()),
        entry("map_tables", core::mem::size_of::<MapTables>()),
        entry("route_cache", core::mem::size_of::<RouteCache>()),
        entry("route_index", core::mem::size_of::<obc_route::RouteIndex>()),
        // `arena_total` is the only resident one of these four: it is the `max` of the three arms, not
        // their sum. The arms are reported beside it so a reader can see which one sets the total.
        entry("arena_total", arena::ARENA_BYTES),
        entry("arena_render", arena::RENDER_ARM_BYTES),
        entry("arena_nav", arena::NAV_ARM_BYTES),
        entry("arena_usb", arena::USB_ARM_BYTES),
        TERRAIN_ENTRIES[0],
        TERRAIN_ENTRIES[1],
        entry("stack_reserve", STACK_RESERVE),
        BLE_ENTRIES[0],
        BLE_ENTRIES[1],
        BLE_ENTRIES[2],
        BLE_ENTRIES[3],
        BLE_ENTRIES[4],
        BLE_ENTRIES[5],
        BLE_ENTRIES[6],
        BLE_ENTRIES[7],
        BLE_ENTRIES[8],
        BLE_ENTRIES[9],
        BLE_ENTRIES[10],
        // The USB plane's *named* statics. The driver's own endpoint bookkeeping and the task future
        // cannot be named here and land in the linked `.bss + .data` gate instead.
        entry("usb_named", usb::RESIDENT_BYTES),
        // The sEMMC transport's two resident blocks: the 4-block alignment bounce in `sd.rs` and the
        // host-driver state.
        entry("sd_bounce", card_io::BOUNCE_BYTES),
        entry("semmc_driver", core::mem::size_of::<semmc::Semmc>()),
        // `flat_store` is the store type itself, `flat_requests` the storage task's queue (depth times
        // the largest request), and `flat_catalog_uploads` the bounded handoff from commits to the app's
        // upload events. The binding's alignment buffer is `sd_bounce`, shared.
        entry("flat_store", core::mem::size_of::<obc_storage::flat::FlatStore<flat_store::FlatCard>>()),
        entry("flat_requests", flat_store::REQUEST_QUEUE_BYTES),
        entry("flat_catalog_uploads", flat_store::CATALOG_UPLOAD_BYTES),
        // The live-ride append delta: about 10 s of samples plus the final footer. The store owns the
        // durable partial page.
        entry("flat_ride_delta", flat_ride::RESIDENT_BYTES),
        // The map read's session-long `StoreSource` and the display name the same boot step captures.
        entry("flat_map_read", flat_store::MAP_READ_BYTES),
        entry("flat_route_read", flat_store::ROUTE_READ_BYTES),
        // The protocol-v4 transfer engine, which lives in the storage task because that is the one
        // context allowed to write. Mostly its 512-byte staging buffer; a full stream record is wider
        // than the stage, so it bypasses it and reaches the card in one write.
        entry("flat_engine", flat_store::ENGINE_BYTES),
    ];
}

/// The resident RGB222 framebuffer, in `.bss`. The `Ls021Flpr` panel owns it: the map plane
/// renders into it and `push_frame` packs it straight to the LS021 wire.
static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// One 32-bit hash per framebuffer row of the last pushed frame. On present the backend re-hashes
/// each row and pushes only the rows whose hash changed. `RowDiff::new()` is all-zero, so it stays
/// in `.bss`, and the first present force-pushes the whole frame to seed it.
static mut ROW_DIFF: RowDiff<FRAME_H> = RowDiff::new();

/// The streamed-map geometry cache, built in place in `.bss`: these large values must never form
/// on the stack.
static mut MAP_CACHE: MaybeUninit<MapCache> = MaybeUninit::uninit();
/// The immutable map tables (header scalars, style table, LOD pyramid), parsed once at boot and
/// borrowed by every per-frame `Reader`. Resident so no frame repeats the parse or style-table
/// reads on the deep render path.
static mut MAP_TABLES: MaybeUninit<MapTables> = MaybeUninit::uninit();
static mut APP: MaybeUninit<App> = MaybeUninit::uninit();
/// The decoded-route-geometry cache, built in place like [`MAP_CACHE`]. It spares the render and
/// the matcher from re-reading route geometry off the card every frame.
static mut ROUTE_CACHE: MaybeUninit<RouteCache> = MaybeUninit::uninit();
/// The shared RRAM settings mutex is initialized on exactly one boot path: the normal ride
/// application or the USB recovery plane used when the selected map is structurally unreadable.
static mut SHARED_SETTINGS_SLOT: MaybeUninit<SharedSettingsMutex> = MaybeUninit::uninit();
/// The mounted map's terrain: the OBCT reader plus its four-tile cache, about 2.1 KB. It is
/// `.bss` because `TerrainElevation` embeds the cache and a stack copy inside the emit path would
/// be a fat local. Written once at boot, and only when the map carries a terrain region.
#[cfg(has_nav)]
static mut TERRAIN: MaybeUninit<obc_elevation::TerrainElevation<'static, { obc_elevation::DEFAULT_TILE_SLOTS }>> =
    MaybeUninit::uninit();
/// The window the OBCT container is read through. It must be `'static`: [`TERRAIN`]'s parsed
/// reader borrows its source for the session.
#[cfg(has_nav)]
static mut TERRAIN_WINDOW: MaybeUninit<obc_formats::io::WindowSource<'static>> = MaybeUninit::uninit();
/// The no-terrain source, a ZST. The emit path then has one uniform seam and no `Option` branch
/// per point.
#[cfg(has_nav)]
static mut NULL_ELEV: obc_route::NullElevation = obc_route::NullElevation;

/// Parse and place the immutable map tables without retaining the by-value result in the async
/// boot frame. The parse temporary lives only in this shallow synchronous frame; the caller keeps
/// the returned pointer-sized result across recovery awaits.
///
/// # Safety
/// `slot` must be the uniquely owned static map-table slot and must be written only by this call.
#[inline(never)]
unsafe fn parse_map_tables(
    map: &'static dyn obc_formats::io::ByteSource,
    slot: *mut MaybeUninit<MapTables>,
) -> Result<&'static MapTables, obc_reader::Error> {
    let tables = MapTables::parse(map)?;
    let ptr = slot as *mut MapTables;
    ptr.write(tables);
    Ok(&*ptr)
}

/// Mount the map's embedded terrain into the `.bss` [`TERRAIN`] slot and hand back the sampler.
///
/// Terrain is a byte window inside the map file, and every offset in the container is relative to
/// the container's own first byte, so the parse needs a window and not a file. The source must be
/// `'static`, because `TerrainElevation` borrows it for the session.
///
/// Every failure gives `None`, and `None` is not a fault: the map simply has no terrain, and the
/// ride loop uses [`NULL_ELEV`] so routes plan and ride flat.
///
/// `#[inline(never)]` is load-bearing. `TerrainElevation` embeds its tile cache, so the by-value
/// parse temporary is about 2.1 KB. In this transient frame it pops with the call; inlined into
/// the ride task's async block it becomes a permanent slot in the poll frame, which overflows the
/// main stack.
///
/// The `&'static mut` does not come from `map`, which is what `clippy::mut_from_ref` cannot see:
/// it comes from [`TERRAIN`], a slot this function solely owns and writes at most once per boot.
#[cfg(has_nav)]
#[inline(never)]
#[allow(clippy::mut_from_ref)]
fn mount_terrain(
    map: &'static dyn obc_formats::io::ByteSource,
    tables: &MapTables,
) -> Option<&'static mut obc_elevation::TerrainElevation<'static, { obc_elevation::DEFAULT_TILE_SLOTS }>> {
    let region = tables.terrain()?;
    let Some(window) = obc_formats::io::WindowSource::new(map, region.offset, region.len) else {
        defmt::warn!(
            "map: the §1.3 terrain region ({=u64}..+{=u64}) is not inside the file — routes stay flat",
            region.offset,
            region.len
        );
        return None;
    };
    // SAFETY: sole owner of TERRAIN_WINDOW, written at most once per boot before any reference
    // escapes. `WindowSource` owns nothing to drop.
    let window: &'static _ = unsafe { init_static(core::ptr::addr_of_mut!(TERRAIN_WINDOW), window) };
    let terrain = match obc_elevation::TerrainElevation::parse(window) {
        Ok(terrain) => terrain,
        Err(error) => {
            defmt::warn!(
                "map: the embedded terrain container will not parse ({}) — routes stay flat",
                defmt::Debug2Format(&error)
            );
            return None;
        }
    };
    defmt::info!("map: terrain mounted from the §1.3 region ({=u64} B)", region.len);
    // SAFETY: sole owner of TERRAIN, written at most once per boot before any reference escapes.
    Some(unsafe { init_static(core::ptr::addr_of_mut!(TERRAIN), terrain) })
}

/// Build a `'static` value into a `.bss` [`MaybeUninit`] slot and return the sole `&'static mut`
/// to it — the warm-reset-safe replacement for `StaticCell`. `StaticCell`'s one-shot `used` flag
/// panics if it is non-zero on entry, which on this board's debug-reset path it can be; an
/// unconditional in-place write carries no such flag. `#[inline(always)]` keeps the by-value `val`
/// off the stack.
///
/// # Safety
/// `slot` must point at a `static mut MaybeUninit<T>` that is initialised exactly once for the
/// program's life through this call and never aliased elsewhere — the returned reference is the only
/// one handed out. (`MaybeUninit<T>` shares `T`'s layout, so the cast is sound.) Each call site
/// passes a distinct slot.
#[inline(always)]
unsafe fn init_static<T>(slot: *mut MaybeUninit<T>, val: T) -> &'static mut T {
    let ptr = slot as *mut T;
    ptr.write(val);
    &mut *ptr
}

/// The RRAM settings store shared by the ride loop and both link planes.
pub(crate) struct SharedSettings {
    pub(crate) settings: settings::RramSettingsStore,
}
pub(crate) type SharedSettingsMutex = Mutex<NoopRawMutex, SharedSettings>;

/// Spawn-trampoline for the BLE stack. Constructing [`ble::run`]'s spawn token materializes its
/// future as a stack temporary in the constructing function's poll frame, and a poll frame's full
/// slot set is allocated at entry — so doing it in `main` would charge `main` for that future on
/// every poll. This tiny task keeps `main`'s frame independent of whatever the future grows to.
#[embassy_executor::task]
async fn spawn_ble_stack(
    spawner: Spawner,
    mpsl_p: nrf_sdc::mpsl::Peripherals<'static>,
    sdc_p: nrf_sdc::Peripherals<'static>,
    cracen_p: embassy_nrf::Peri<'static, embassy_nrf::peripherals::CRACEN>,
    state: link::LinkState,
    sensor_injector: obc_platform::sensor_hub::SampleInjector<'static>,
) {
    spawner.spawn(defmt::unwrap!(ble::run(spawner, mpsl_p, sdc_p, cracen_p, state, sensor_injector)));
}

/// Spawn-trampoline for the USB device plane, for the same reason as [`spawn_ble_stack`]:
/// constructing the task's future in `main` would put its whole state machine in `main`'s frame.
#[embassy_executor::task]
async fn spawn_usb_stack(spawner: Spawner, usb_p: embassy_nrf::Peri<'static, embassy_nrf::peripherals::USBHS>) {
    spawner.spawn(defmt::unwrap!(usb::run(usb_p)));
}

/// Keep card provisioning and map replacement available when boot cannot open a map.
///
/// The ordinary composition point sits after map parsing, because the ride loop needs the parsed
/// tables. A damaged map must not make the cable that replaces it disappear. This reduced boot
/// path starts only USB; the storage task, with the protocol-v4 engine, is already spawned on
/// every card. It needs no app state and no settings mutex. On an unformatted card
/// the engine answers ordinary operations with `readOnly/unformatted`, and an explicit `FORMAT`
/// can still initialize it over this link.
///
/// It must seed the firmware revision, because the EP0 device-info request serves it and "an
/// update is available" compares against it.
async fn spawn_map_recovery_usb(
    spawner: Spawner,
    usb_p: embassy_nrf::Peri<'static, embassy_nrf::peripherals::USBHS>,
    rramc: embassy_nrf::Peri<'static, embassy_nrf::peripherals::RRAMC>,
) -> Option<arena::UsbGuard> {
    let mut settings_store = settings::RramSettingsStore::new(rramc);
    dfu::seed_firmware_revision(&mut settings_store);
    // No ride loop, map render or route search exists on this boot path. Pre-grant the arena guard
    // so the first post-FORMAT map upload gets the same double-buffer DMA path as a mounted boot.
    let stage = arena::claim_usb(obc_app::TransferReady::recovery_boot()).ok();
    usb::set_stage_granted(stage.is_some());
    spawner.spawn(defmt::unwrap!(spawn_usb_stack(spawner, usb_p)));
    defmt::warn!("usb: card-recovery plane active — format if needed, then upload a map and reboot");
    stage
}

/// Idle camera zoom for the boot map, in ground metres per pixel: a town-scale overview.
const INIT_MPP: f32 = 2.0;

/// Heartbeat-only idle for an unrecoverable bring-up failure: blink the LED forever rather than
/// panic — a missing or bad card must never fault. Storage faults paint a [`show_boot_fault`]
/// screen first; a bare heartbeat is for a failure with no panel to draw on. Diverges.
async fn idle_blink(led: &mut Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

/// Stack high-water guard. [`paint`] fills the free stack with a sentinel early in `main`, and
/// [`used`] reports the deepest reach by finding the lowest word that is still painted.
///
/// The scan must run bottom-up to the first word that is not painted. A frame does not write every
/// word it covers, so large uninitialized locals leave painted islands inside the used region and
/// a scan from the used side under-reports by whole buffers. Sentinel evidence is permanent, so
/// [`used`] runs a full scan at most once per [`SCAN_INTERVAL_MS`] and returns the cached mark
/// between scans. Each scan costs reads proportional to the remaining headroom, 40 to 90 µs.
mod stackmeter {
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    const PAINT: u32 = 0xC0DE_DEAD;
    const SCAN_INTERVAL_MS: u32 = 1000;
    static LAST_SCAN_MS: AtomicU32 = AtomicU32::new(0);
    /// The last scan's result. 0 means never scanned: `paint` leaves at least 512 B unpainted below
    /// the paint-time SP, so a real measurement cannot be 0.
    static LAST_USED: AtomicUsize = AtomicUsize::new(0);
    extern "C" {
        static _stack_start: u32;
        static _stack_end: u32;
    }
    #[inline(always)]
    fn top() -> usize {
        core::ptr::addr_of!(_stack_start) as usize
    }
    #[inline(always)]
    fn bottom() -> usize {
        core::ptr::addr_of!(_stack_end) as usize
    }
    /// Paint everything below the current SP (minus a margin) down to the stack bottom.
    pub fn paint() {
        let sp = cortex_m::register::msp::read() as usize;
        let mut a = bottom();
        let stop = sp.saturating_sub(512);
        while a < stop {
            unsafe { (a as *mut u32).write_volatile(PAINT) };
            a += 4;
        }
        LAST_USED.store(0, Ordering::Relaxed);
    }
    /// Bytes of stack used at the deepest point reached so far. Calls within
    /// [`SCAN_INTERVAL_MS`] of the last scan return the cached mark without reading the stack.
    pub fn used(now: u32) -> usize {
        let cached = LAST_USED.load(Ordering::Relaxed);
        if cached != 0 && now.wrapping_sub(LAST_SCAN_MS.load(Ordering::Relaxed)) < SCAN_INTERVAL_MS {
            return cached;
        }
        let (top, bottom) = (top(), bottom());
        let mut a = bottom;
        while a < top {
            if unsafe { (a as *const u32).read_volatile() } != PAINT {
                break;
            }
            a += 4;
        }
        LAST_SCAN_MS.store(now, Ordering::Relaxed);
        LAST_USED.store(top - a, Ordering::Relaxed);
        top - a
    }
    pub fn total() -> usize {
        top() - bottom()
    }

    /// Arm the ARMv8-M MSPLIM hardware stack-limit register: a push or `sp` move below the limit
    /// raises a STKOF UsageFault at the moment of overflow, instead of silently smashing the static
    /// that tops `.bss`. The limit sits [`HANDLER_MARGIN`] above the true bottom so the fault
    /// handler and the panic output have stack to run on. The core suppresses exception-entry
    /// writes below MSPLIM, so even that margin never corrupts the statics below `_stack_end`.
    pub fn arm_limit() {
        /// Room below the limit for the STKOF exception frame and the panic path.
        const HANDLER_MARGIN: usize = 512;
        // SAFETY: raising a fault on genuine overflow is strictly safer than the silent
        // corruption it replaces; nothing legitimately moves MSP below `_stack_end`.
        unsafe { cortex_m::register::msplim::write((bottom() + HANDLER_MARGIN) as u32) };
    }

    /// [`used`], but forcing a fresh full scan instead of obeying the [`SCAN_INTERVAL_MS`]
    /// throttle — for a caller that has just finished a stack-notable operation and wants the peak
    /// including it now.
    #[cfg(has_nav)]
    pub fn rescan(now: u32) -> usize {
        LAST_USED.store(0, Ordering::Relaxed); // 0 = "never scanned" → `used` always rescans
        used(now)
    }
}

/// VCOM RX to sensor signals: read bytes from the interrupt-fed ring and feed each complete line
/// into `obc-platform`'s fresh-fix signals. Injected heart rate, power and cadence go through the
/// hub's `SampleInjector`, the same mailboxes the BLE manager feeds. A UART never disconnects, so
/// one `LineReader` lives for the whole session.
#[cfg(feature = "debug-uart")]
#[embassy_executor::task]
async fn vcom_rx_task(
    mut rx: BufferedUarteRx<'static, peripherals::SERIAL20>,
    injector: obc_platform::sensor_hub::SampleInjector<'static>,
) {
    let mut buf = [0u8; 64];
    let mut reader = obc_platform::debug_link::LineReader::new();
    loop {
        match rx.read(&mut buf).await {
            Ok(n) => obc_platform::debug_link::feed_bytes(&mut reader, &buf[..n], injector),
            Err(e) => defmt::warn!("VCOM RX error: {}", defmt::Debug2Format(&e)),
        }
    }
}

/// VCOM TX: send one line each time the app publishes telemetry or the DFU armer queues a status
/// line. The buffered UARTE chunks the line to DMA itself, so the loop only has to queue it all.
#[cfg(feature = "debug-uart")]
#[embassy_executor::task]
async fn vcom_tx_task(mut tx: BufferedUarteTx<'static, peripherals::SERIAL20>) {
    use embassy_futures::select::{select, Either};
    loop {
        let line: heapless::String<192> =
            match select(obc_platform::debug_link::wait_telemetry(), obc_platform::debug_link::wait_dfu_status()).await
            {
                Either::First(t) => obc_platform::debug_link::format_telemetry(&t),
                Either::Second(s) => {
                    let mut d = heapless::String::new();
                    let _ = d.push_str("D ");
                    let _ = d.push_str(&s);
                    let _ = d.push('\n');
                    d
                }
            };
        let mut bytes = line.as_bytes();
        while !bytes.is_empty() {
            match tx.write(bytes).await {
                Ok(0) => break,
                Ok(n) => bytes = &bytes[n..],
                Err(e) => {
                    defmt::warn!("VCOM TX error: {}", defmt::Debug2Format(&e));
                    break;
                }
            }
        }
    }
}

/// The board's one sensor hand-off: every cross-task sensor stream, split into typed producer,
/// consumer and control handles that are wired at composition below. In every build — a `synth`
/// build still needs the injector for BLE sensor connections.
static SENSOR_HUB: obc_platform::SensorHub = obc_platform::SensorHub::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = board::init!();

    // Arm the hardware stack limit first, so an overflow is an immediate fault and never silent
    // corruption, then paint the stack while it is still shallow.
    stackmeter::arm_limit();
    stackmeter::paint();

    // The boot banner. The DFU on-glass gate reads this line to prove the device came back as the
    // staged version after an install.
    info!("obc-fw-nrf54l {=str}+{=str}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));

    let reset_reas = {
        let v = board::take_reset_reason!();
        if v & 0x6 != 0 {
            // Bits 1 and 2 are the two watchdogs. The WDT0 instance used here reports as bit 2.
            defmt::error!("boot: WATCHDOG reset (RESETREAS=0x{=u32:08x})", v);
        } else {
            defmt::info!("boot: RESETREAS=0x{=u32:08x}", v);
        }
        v
    };

    // LED1 heartbeat — a liveness blink visible before looking at the panel. (LED0's pin carries
    // VCOM, so its buffered LED shimmers at 60 Hz: a free COM-alive light.)
    let mut led = Output::new(p.P1_25, Level::Low, OutputDrive::Standard);

    // The panel's brightness port: PWM20 channel 0 on P1.27, the provisional backlight net. Armed
    // here, where the peripherals live, and driven by the ride loop.
    let backlight = panel_power::PanelBacklight::new(p.PWM20, p.P1_27);

    // load, ride, save: stream the map into the framebuffer through the shared `obc-app`, pick a
    // route from the catalog, ride it, map-match and record the samples, and append the ride
    // object's footer on Finish.
    {
        // The VCOM debug-sensor stream, behind `debug-uart`. Bring it up first, so the link is live
        // while the card and panel come up. The DK has no USB peripheral, so the fake GPS, baro and
        // compass feed and the ride telemetry ride UARTE20 on the J-Link VCOM (TX P1_16, RX P1_17);
        // defmt logs share the same cable over RTT. Without the feature the app rides `SynthLocation`.
        #[cfg(feature = "debug-uart")]
        {
            // The `'static` rings behind the interrupt-fed UARTE, written in place so a warm reset
            // is safe. RX is 512 B because a multi-second synchronous nav plan starves the RX task,
            // and the host feed overran a 256 B ring mid-plan.
            static mut RX_BUF: MaybeUninit<[u8; 512]> = MaybeUninit::uninit();
            static mut TX_BUF: MaybeUninit<[u8; 256]> = MaybeUninit::uninit();
            // SAFETY: each ring is written once here, then handed to exactly one task half — no alias.
            let (rx_buf, tx_buf): (&'static mut [u8; 512], &'static mut [u8; 256]) = unsafe {
                (
                    init_static(core::ptr::addr_of_mut!(RX_BUF), [0; 512]),
                    init_static(core::ptr::addr_of_mut!(TX_BUF), [0; 256]),
                )
            };
            let uart = board::input_hardware!(uart p, rx_buf, tx_buf);
            let (rx, tx) = uart.split();
            _spawner.spawn(defmt::unwrap!(vcom_rx_task(rx, SENSOR_HUB.injector())));
            _spawner.spawn(defmt::unwrap!(vcom_tx_task(tx)));
            info!("VCOM debug sensors up on UARTE20 (J-Link VCOM 'UART1', TX P1_16 / RX P1_17) @ 115200");
        }

        // The board owns active-low/pull-up pin order; the input plane owns gesture semantics.
        let buttons = board::input_hardware!(buttons p);

        // Real GPS and altimeter, default build only. The event-driven sensor task probes both chips
        // and publishes datapoints into `SENSOR_HUB`.
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        {
            // EasyDMA cannot fetch a write buffer from flash, so byte-literal register writes need a
            // RAM bounce buffer. 32 B covers the widest VALSET frame.
            static mut TWIM_TX_BUF: MaybeUninit<[u8; 32]> = MaybeUninit::uninit();
            // SAFETY: written once here, then owned solely by the `Twim` for the program's life.
            let twim_tx = unsafe { init_static(core::ptr::addr_of_mut!(TWIM_TX_BUF), [0u8; 32]) };
            let (twim, txready) = board::input_hardware!(sensors p, twim_tx);
            _spawner.spawn(defmt::unwrap!(sensors::sensor_task(twim, txready, SENSOR_HUB.task_link())));
            info!("sensors: SAM-M10Q + BMP581 task spawned on TWIM22 (SDA P1.04 / SCL P1.03, TX-Ready P1.05)");
        }

        // The map plane owns the `Ls021Flpr` panel directly: it scans a whole frame per push, so
        // there is no partial-window overlay to serialize and no bus mutex. The M33 configures every
        // line the FLPR drives and holds them as outputs for the program's life. `com_task` and the
        // gesture `input_task` share the one high-priority executor, because COM must keep
        // alternating whatever the map plane does.
        //
        // These gate and BSP lines must match the masks in `src/flpr/flpr_scan.c`. Remap both
        // together.
        let (mut display, hp) = {
            // Gate and frame lines: one contiguous P1.10-14 run with BSP below, so the gate harness
            // is a single uninterrupted cable on the DK's port-1 header.
            let gate_bus = [
                Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // GSP
                Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GCK
                Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
                Output::new(p.P1_13, Level::Low, OutputDrive::Standard), // INTB
            ];
            // `B0` and `B1` (P2.00/P2.04) are shared with the card. They are claimed here as plain
            // `Output`s so the M33 owns their direction and drive, but `flpr_mux` flips their
            // `CTRLSEL` per mode (GPIO for the display blob, VPR for the sEMMC peripheral). The four
            // card-only pads (P2.01/02/03/05) are deliberately not embassy peripherals: the soft
            // peripheral configures them itself and `semmc::configure_display_pads` parks them.
            // Claiming them here would give one pad two owners, and an `Output` drop could re-drive
            // a card line mid-transfer.
            let src_bus = [
                Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
                Output::new(p.P2_07, Level::Low, OutputDrive::Standard), // BCK (unchanged)
                Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // R0 (was SD-SPI SCK)
                Output::new(p.P2_08, Level::Low, OutputDrive::Standard), // R1 (was SD-SPI MOSI)
                Output::new(p.P2_09, Level::Low, OutputDrive::Standard), // G0 (was SD-SPI MISO)
                Output::new(p.P2_10, Level::Low, OutputDrive::Standard), // G1 (was SD-SPI CS)
                Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // B0 (shared: sEMMC D3)
                Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B1 (shared: sEMMC D1)
            ];
            // COM electrode lines: 56 to 77 nF of load each, so high-drive. They boot `Lo` and stay
            // `Lo` through the init-black frame below, then start. Default: three `Output`s the M33
            // `com_task` toggles at 60 Hz — VCOM P1.22 and VB P1.23 in phase, VA P1.24 inverse. With
            // `com-hw` the same nets become GPIOTE20 toggle channels that a TIMER and DPPI chain
            // free-runs with no CPU. `HwCom::start` establishes VA's inverse phase before it enables
            // the toggle.
            #[cfg(not(feature = "com-hw"))]
            let (vcom, vb, va) = (
                Output::new(p.P1_22, Level::Low, OutputDrive::HighDrive),
                Output::new(p.P1_23, Level::Low, OutputDrive::HighDrive),
                Output::new(p.P1_24, Level::Low, OutputDrive::HighDrive),
            );
            #[cfg(feature = "com-hw")]
            let (vcom, vb, va) = {
                use embassy_nrf::gpiote::{OutputChannel, OutputChannelPolarity::Toggle};
                (
                    OutputChannel::new(p.GPIOTE20_CH0, p.P1_22, Level::Low, OutputDrive::HighDrive, Toggle),
                    OutputChannel::new(p.GPIOTE20_CH1, p.P1_23, Level::Low, OutputDrive::HighDrive, Toggle),
                    OutputChannel::new(p.GPIOTE20_CH2, p.P1_24, Level::Low, OutputDrive::HighDrive, Toggle),
                )
            };

            // Launch the FLPR (copy the blob, arm the control block, wait for ALIVE), with one full
            // relaunch retry on failure, because a cold-boot race deserves a second attempt. A launch
            // failure must never fault. There is no live panel to paint a fault screen on here, so it
            // degrades to a bare heartbeat.
            let launched = match launch_flpr().await {
                Ok(()) => Ok(()),
                Err(e) => {
                    defmt::warn!("FLPR: boot launch failed ({}) — one relaunch retry", e);
                    relaunch_flpr().await
                }
            };
            match launched {
                Ok(()) => info!("obc-fw-nrf54l: FLPR alive — LS021 panel backend up; init-black then COM"),
                Err(FlprError::BadMagic) => {
                    defmt::error!("FLPR: control-block magic mismatch (memory-map drift) — idling with a heartbeat");
                    idle_blink(&mut led).await
                }
                Err(FlprError::NoBoot) => {
                    defmt::error!("FLPR: no alive stamp (didn't boot / can't reach shared RAM) — idling");
                    idle_blink(&mut led).await
                }
            }

            // SAFETY: sole references to FB and ROW_DIFF. The map plane's frame and panel pair own
            // them for the rest of the program and never alias them.
            let fb: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
            let diff: &'static mut RowDiff<FRAME_H> = unsafe { &mut *core::ptr::addr_of_mut!(ROW_DIFF) };
            let frame = Frame64::new(fb);
            let mut panel = Ls021Flpr::new(diff);
            // Datasheet Initial #0: an INTB-framed all-black frame (a zeroed FB is black) while COM
            // is still held `Lo`. Then wait T4 of at least 30 µs before COM starts.
            panel.push_frame(&frame).await;
            Timer::after_micros(50).await;

            // The shared `InputPlane`: `input_task` recognises and animates the bulge under this
            // lock, and the map plane composites it into a partial overlay push. Written in place,
            // not `StaticCell`, whose one-shot flag can panic on a warm reset.
            static mut INPUT_PLANE: MaybeUninit<BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>> =
                MaybeUninit::uninit();
            // SAFETY: sole writer, initialised before the `&'static` is shared with the input plane
            // and never rewritten.
            let input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>> = unsafe {
                init_static(core::ptr::addr_of_mut!(INPUT_PLANE), BlockingMutex::new(RefCell::new(InputPlane::new())))
            };

            let hp = EXECUTOR_HP.start(interrupt::SWI01);
            // COM starts only now, after the init-black frame above. The default `com_task` runs on
            // the high-priority executor, because it must keep toggling during the blocking
            // whole-frame push. With `com-hw` the hardware chain does it with no task and no wakes.
            #[cfg(not(feature = "com-hw"))]
            hp.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
            #[cfg(feature = "com-hw")]
            let com_hw = {
                let c = HwCom::start(p.TIMER21, p.PPI20_CH0, vcom, vb, va);
                info!("FLPR LS021: COM on hardware TIMER21+DPPI+GPIOTE20 (zero-CPU); M33 can WFI between events");
                c
            };
            hp.spawn(defmt::unwrap!(input_task(buttons, input_plane, GESTURES.sender(), CHORDS.sender())));
            info!("FLPR LS021: gesture/bulge plane on SWI01 @ P3; map plane: thread mode (event-driven, #219)");
            let display = MapDisplay {
                frame,
                panel,
                input_plane,
                last_overlay_span: None,
                push_fails: 0,
                consec_relaunches: 0,
                relaunch_repaint: false,
                degraded: false,
                _gate_bus: gate_bus,
                _src_bus: src_bus,
                #[cfg(feature = "com-hw")]
                _com_hw: com_hw,
            };
            (display, hp)
        };
        // The piezo: PWM21 on P1.06 and P1.07, provisional DK pins until a board revision fits one.
        let buzzer = buzzer::Buzzer::new(hp, p.PWM21, p.P1_06, p.P1_07);

        // The microSD runs in native 4-bit SD mode over Nordic's sEMMC soft peripheral — the same
        // FLPR the panel runs on, time-multiplexed by `flpr_mux`. There is no SPI instance and no
        // chip select: the six card pads belong to the soft peripheral, which owns their direction,
        // drive and `CTRLSEL` per mode (see `semmc::configure_storage_pads`).
        //
        // The order is load-bearing. The display comes up first, so bring-up flips the FLPR from
        // display to storage exactly once. `bring_up_storage` holds the mode across the whole
        // `Semmc::start` await, and nothing else that touches the coprocessor is spawned yet, which
        // is what makes that contract structural.
        //
        // Arm the completion vector first, on P1: the ISR is one store and a latched-event clear, so
        // its priority only has to stay under the P0 GRTC driver.
        // SAFETY: enabling an NVIC line whose handler is bound in `board::VPR00`.
        unsafe {
            interrupt::VPR00.set_priority(Priority::P1);
            interrupt::VPR00.enable();
        }
        // A missing or bad card is fatal, because the map streams from it. The display is already
        // up, so paint an undismissable fault screen, then heartbeat-idle. The reason travels with
        // the failure: a reader that never booted and a card that is merely too small get their own
        // screen, because "NO SD CARD" would send the rider to the wrong fix.
        if let Err(fault) = flat_store::bring_up_card() {
            defmt::error!(
                "SD: storage unusable — showing the {=str} fault screen, then heartbeat idle",
                fault.copy().0
            );
            show_boot_fault(&mut display, fault).await;
            idle_blink(&mut led).await
        }

        // The flat store owns the raw card from LBA 0, and FAT is a filesystem on a card, so a card
        // is one or the other and never both. `FlatStore::mount` is the test: it never fails, it
        // classifies, so the board runs no second superblock reader that could disagree with it. A
        // card without a flat superblock is rejected.
        //
        // `mount_at_boot` is `#[inline(never)]` into a `.bss` slot: a 10.5 KB store must not become a
        // permanent slot in this task's poll frame, and `mount`'s own 14 KB constructor frame must
        // stay a transient step of the boot chain. It is well inside the watchdog invariant below: a
        // mount is bounded by the catalog it reads.
        let flat_started = embassy_time::Instant::now();
        let flat = flat_store::mount_at_boot();
        #[cfg(feature = "seed-rides")]
        {
            #[cfg(demo_ride_file)]
            let result = demo_rides::seed_file(flat, include_bytes!(concat!(env!("OUT_DIR"), "/demo_ride.obcr")));
            #[cfg(not(demo_ride_file))]
            let result = {
                let start = option_env!("OBC_DEMO_RIDE_START").unwrap_or("0").parse().unwrap_or(0);
                demo_rides::seed(flat, start)
            };
            match result {
                #[cfg(demo_ride_file)]
                Ok(id) => {
                    info!("demo ride: object {}", id.0);
                    info!("demo rides: complete");
                }
                #[cfg(not(demo_ride_file))]
                Ok(ids) => {
                    for (name, id) in demo_rides::NAMES.iter().zip(ids) {
                        info!("demo rides: {} = object {}", name, id.0);
                    }
                    info!("demo rides: complete");
                }
                Err(error) => {
                    defmt::error!("demo rides: refused: {:?}", defmt::Debug2Format(&error));
                    idle_blink(&mut led).await
                }
            }
            if cfg!(demo_ride_file) {
                idle_blink(&mut led).await
            }
        }
        let flat_catalog = flat_store::report(flat, flat_started.elapsed().as_micros());

        // The `&'static StoreSource` is a plain `ByteSource` a render calls straight through. The
        // storage task spawned below owns writes only, and a render's `read_at` never touches its
        // channel.
        //
        // The task is spawned on every card, not only a flat one, and `arm` is what makes `writer()`
        // hand out senders at all. A card that is not a flat store is `Mode::Unformatted`, and the
        // protocol has the honest answer for one: ordinary opcodes return `readOnly`, and an explicit
        // FORMAT is the one recovery exception. Arming against the store that is always mounted is
        // what lets such a card answer the truth about itself.
        _spawner.spawn(defmt::unwrap!(flat_store::storage_task(flat, flat_store::arm())));

        let flat_map: &'static dyn obc_formats::io::ByteSource = match flat_store::classify(flat) {
            flat_store::Card::Flat => match flat_store::open_map(flat) {
                Some(source) => source,
                None => {
                    let fault = flat_store::boot_fault_for(flat_catalog);
                    defmt::error!(
                        "flat: no map to render from — showing the {=str} fault screen with USB recovery",
                        fault.copy().0
                    );
                    let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                    run_map_recovery(&mut display, &mut led, fault).await
                }
            },
            flat_store::Card::FlatBroken(fault) => {
                let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                run_map_recovery(&mut display, &mut led, fault).await
            }
            flat_store::Card::NotFlat => {
                defmt::error!("flat: card is not formatted as a flat store — FAT compatibility is retired");
                let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                run_map_recovery(&mut display, &mut led, obc_app::BootFault::StorageFault).await
            }
        };

        // Place the streamed-map geometry cache in `.bss`, built in place: an all-zero
        // `MaybeUninit::zeroed` is a memset, never a stack temporary.
        // SAFETY: sole owner of MAP_CACHE; single executor → no aliasing.
        let map_cache: &MapCache = unsafe { init_static(core::ptr::addr_of_mut!(MAP_CACHE), MapCache::new()) };

        // Parse the OBCM header, style table and LOD pyramid once at boot into the resident
        // [`MAP_TABLES`]. They are immutable for the session, so the per-frame readers borrow them
        // instead of re-parsing. The transient parse cost is paid here, where the call stack is
        // shallow.
        // SAFETY: sole owner of MAP_TABLES; single executor → no aliasing; written exactly once here.
        let map_tables: &MapTables = unsafe {
            match parse_map_tables(flat_map, core::ptr::addr_of_mut!(MAP_TABLES)) {
                Ok(tables) => tables,
                Err(e) => {
                    defmt::error!(
                        "map: not valid OBCM: {} — showing MAP UNREADABLE with USB recovery",
                        defmt::Debug2Format(&e)
                    );
                    // USB map recovery speaks protocol v4 against the flat store, so a replacement
                    // can still be uploaded when the current map will not parse.
                    let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                    run_map_recovery(&mut display, &mut led, obc_app::BootFault::BadMap).await
                }
            }
        };
        let (cam_lon, cam_lat) = {
            let b = map_tables.bbox;
            info!(
                "map: streaming from SD; bbox lon[{=i32}..{=i32}] lat[{=i32}..{=i32}]",
                b.min_lon, b.max_lon, b.min_lat, b.max_lat
            );
            (((b.min_lon as i64 + b.max_lon as i64) / 2) as i32, ((b.min_lat as i64 + b.max_lat as i64) / 2) as i32)
        };

        // The map's embedded terrain. It mounts right behind the tables, because the region is a
        // header field and there is nothing to mount until the header is parsed. `mount_terrain`'s
        // `#[inline(never)]` keeps the parse temporary out of this task's poll frame.
        #[cfg(has_nav)]
        let terrain = mount_terrain(flat_map, map_tables);

        // Boot to Home: the rider drives Home, Route menu, Map with the buttons. The `sd-bench`
        // image starts on the live Map instead, so an unattended run exercises the map-reader path
        // as `SynthLocation` moves. Built in place in `.bss`, never on the stack.
        // SAFETY: sole owner of APP; `init_idle` fully initialises it before the `&mut` below reads it.
        let app: &mut App = unsafe {
            let slot = core::ptr::addr_of_mut!(APP) as *mut App;
            #[cfg(not(feature = "sd-bench"))]
            App::init_idle(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            #[cfg(feature = "sd-bench")]
            App::init_map(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            &mut *slot
        };
        {
            // Routes and trips are flat-store objects. One bounded snapshot seeds the menu, newest
            // first, so a fresh upload stays visible on a card with more than the UI cap. The ride
            // loop reports the store's live `sequence()`, and the first pass sees that move from "no
            // store" to a revision, which arms exactly one catalog read. This snapshot is what the
            // menu shows until that read lands.
            let _ = flat_store::load_routes(flat, app);
            let _ = flat_store::load_trips(flat, app);
            let _ = flat_store::load_rides(flat, app);
            // Device info for the System settings screen: the running firmware version and the
            // loaded map's name and OBCM version.
            {
                use core::fmt::Write as _;
                let mut fw: heapless::String<32> = heapless::String::new();
                let _ = write!(fw, "{}+{}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));
                app.set_fw_version(&fw);
                app.set_map_info(flat_store::map_name(), map_tables.version);
            }
        }

        // Place the decoded-route-geometry cache in `.bss`, built in place from an all-zero const
        // literal, never as a stack temporary.
        // SAFETY: sole owner of ROUTE_CACHE; single map plane → no aliasing.
        let route_cache: &RouteCache = unsafe { init_static(core::ptr::addr_of_mut!(ROUTE_CACHE), RouteCache::new()) };

        // The router's resident half. The A* table, the graph-tile cache and the planner slot are
        // the arena's nav arm, claimed per search, so all that is threaded into the ride loop is the
        // map's terrain (or the null source), which is sampled at fix cadence. Without `has_nav` it
        // is the unit stand-in and the ride loop answers plan requests with the failure tier.
        #[cfg(has_nav)]
        let nav = ride::NavResident {
            // SAFETY: `NULL_ELEV` is a ZST written nowhere else, and `terrain` is the sole reference
            // to the `TERRAIN` slot, moved here.
            elev: match terrain {
                Some(t) => t,
                None => unsafe { &mut *core::ptr::addr_of_mut!(NULL_ELEV) },
            },
        };
        #[cfg(not(has_nav))]
        let nav = ride::NavResident;

        // The persistent settings store over the carved RRAM page. Built here, where `p` is live,
        // and moved into the ride loop. Every boot bumps the persisted boot counter.
        let mut settings_store = settings::RramSettingsStore::new(p.RRAMC);
        let boot_count = settings_store.bump_boot_count(reset_reas);
        defmt::info!("boot #{=u32}", boot_count);

        // Snapshot the running image's version off the DFU boot-state page before any plane that
        // publishes it exists: the BLE DIS strings and the USB device-info request both serve it, and
        // both planes are spawned below.
        dfu::seed_firmware_revision(&mut settings_store);

        // INVARIANT: every trial boot enters with the watchdog already counting, so everything
        // between app entry and this line must complete well inside one WDT period (24 s). If it
        // does not, the dog resets a healthy trial image and the bootloader rolls it back. Never move
        // a blocking or open-ended retry loop above this point.
        let wdt_handle = match board::watchdog!(p.WDT0, ride::WDT_TIMEOUT_TICKS) {
            Ok((_wdt, [handle])) => Some(handle),
            Err(_) => {
                defmt::warn!("WDT: already running with a foreign config — cannot feed it; expect one reset");
                None
            }
        };

        // The settings store moves behind one async mutex, so the ride loop and both link planes can
        // lock it per operation. The flat store owns the card through its separate command seam.
        let shared_settings: &'static SharedSettingsMutex = unsafe {
            init_static(
                core::ptr::addr_of_mut!(SHARED_SETTINGS_SLOT),
                SharedSettingsMutex::new(SharedSettings { settings: settings_store }),
            )
        };

        let cracen_p = p.CRACEN;

        // The one companion-link control state, and the handles every link plane is composed with.
        // It is built here and not inside `ble::run`, because with USB as a second transport two
        // independently-constructed states would each keep a stale settings cache.
        //
        // `link::init_control` is `#[inline(never)]`, so its construction temporary lives in
        // that transient frame and `main`'s frame pays only the reference.
        let link_state = {
            let control = {
                let mut guard = shared_settings.lock().await;
                link::init_control(&mut guard)
            };
            link::LinkState { shared: shared_settings, control }
        };

        {
            let (mpsl_p, sdc_p) = board::radio_hardware!(p);
            // CRACEN goes to the link layer's crypto RNG.
            _spawner.spawn(defmt::unwrap!(spawn_ble_stack(
                _spawner,
                mpsl_p,
                sdc_p,
                cracen_p,
                link_state,
                SENSOR_HUB.injector()
            )));
        }

        // The USB device plane: USBHS on its dedicated D+/D-/VBUS pins, speaking the same object
        // model as the radio over a vendor bulk interface. Spawned through its own trampoline for the
        // same reason as the BLE stack.
        _spawner.spawn(defmt::unwrap!(spawn_usb_stack(_spawner, p.USBHS)));

        // Hand the display and the resident set to the shared, backend-agnostic ride loop.
        // `display` is the only per-backend value crossing this seam. `cam_center` is threaded only
        // on the `synth` build: the host feed and the real GPS stream absolute positions.
        #[cfg(any(feature = "debug-uart", not(feature = "synth")))]
        let app_fut = ride::run_app(
            display,
            app,
            shared_settings,
            map_tables,
            map_cache,
            flat_map,
            flat,
            route_cache,
            nav,
            &mut led,
            backlight,
            buzzer,
            wdt_handle,
            // The hub's consumer and control handles: ownership is visible here at composition.
            SENSOR_HUB.consumer(),
            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
            SENSOR_HUB.control(),
        );
        #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
        let app_fut = ride::run_app(
            display,
            app,
            shared_settings,
            map_tables,
            map_cache,
            flat_map,
            flat,
            route_cache,
            nav,
            &mut led,
            backlight,
            buzzer,
            wdt_handle,
            (cam_lon, cam_lat),
        );
        // The ride loop is `main`'s tail future, and the BLE stack runs beside it as the task
        // spawned above — both on the one thread-mode executor, both driving the shared store.
        app_fut.await;
    }
}
