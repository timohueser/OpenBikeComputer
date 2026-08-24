//! nRF54L15-DK board firmware for OpenBikeComputer — the **real hardware target**.
//!
//! The nRF54L15 driving the reflective LS021B7DD02 memory-LCD through the FLPR coprocessor is
//! what the project ships on — the **only** display path. This crate ports the
//! shared `obc-app` onto it (load route → ride → save the ride object). Nothing app-facing lives here:
//! `obc-render` / `obc-app` / `obc-reader` / `obc-route` + `obc-platform` stay board-agnostic;
//! only the nRF HAL wiring + the display presenter backend are board-specific.
//!
//! **The `ble` build** (`cargo run --release --no-default-features --features ble`): the same
//! firmware with the BLE stack folded in (`ble/`: MPSL, the SoftDevice Controller, TrouBLE) —
//! **map + BLE in one image** (#270). [`ble::run`] is spawned beside the ride loop; both drive the
//! shared SD + settings store ([`SharedStore`] — the ride loop locks it per frame across the
//! render but never across the present (#809), the object plane per chunk). Fits the 256 KB DK
//! on the culled `nrf-mem`
//! caps; the budget assert + the ~79.4 KB residual stack (deep-ride peak ~36 KB; deepest boot
//! chain ~28.1 KB — the ride task's 6.9 KB poll frame under `link::init_store`'s ~14.7 KB
//! transient) are the margins. Both numbers moved on 2026-08-03: the elevation epic's `.bss`
//! (TERRAIN + its extent tables) shrank the residual by ~3.7 KB, and EL7's inlined terrain parse
//! plus `init_store`'s double-copy briefly summed past it — a boot-bricking STKOF; `mount_terrain`
//! and `ObjectStore::empty`/`hydrate` are the fix, and any future fat boot-path local belongs in
//! the same `#[inline(never)]` pattern (#677). Both moved again with #1146 P2: the scratch arena
//! handed the residual back ~76 KB (48.6 → 125.0; P3 then spent 24.5 KB of it on the render caps,
//! leaving 99.9), and the ride task's poll frame fell 20.4 → 6.9 KB
//! because LLVM stopped materialising a ~13.5 KB staging copy in it — measured, not designed, so
//! read that half as slack the resource guard now pins rather than as budget to spend.
//! `--no-default-features` stays mandatory — it swaps the critical-section impl to MPSL's.
//!
//! Clock: the M33 application core runs at 128 MHz; embassy-time is driven by the **GRTC**
//! (Global RTC) via the `time-driver-grtc` feature — the nRF54L has no legacy RTC time-driver.
//! `ble` builds additionally source HFCLK from the **crystal** (an MPSL hard requirement) and
//! leave LFCLK on the MPSL-calibrated internal RC (see `ble.rs` — the unprogrammed XO INTCAPs).
//!
//! ## Flash / RAM
//!   From the `memory.x` build.rs emits (#617): the app's FLASH is 1484K @ **0x0000_8000** — the
//!   32K below is the `obc-boot` bootloader (flash it once, then iterate here exactly as before;
//!   see `../obc-boot/README.md`), and the RRAM top holds the `BOOT_STATE` (0x0017_B000) +
//!   `SETTINGS` (0x0017_C000) pages. Don't hard-code flash addresses — the bootloader sets VTOR
//!   to 0x8000 before the jump, so the linker map is the only authority. RAM 256K @ 0x2000_0000
//!   is tight (no external RAM): the single RGB222 framebuffer is ~75 KB and the renderer
//!   scratch + caches must fit the rest — see the budget assert below.

#![no_std]
#![no_main]

mod board;
mod sd;
// The **flat store** on this board (FS7.5-c1, epic #1256): the card binding, the boot mount into a
// `.bss` slot, and the one storage task the owner's hybrid topology puts the write half behind
// (reads direct, writes serialized). This is the slice that first puts `obc_storage::flat` into the
// shipping image; a card is a flat store *or* a FAT volume, never both, and boot classifies it.
mod flat_ride;
mod flat_store;
// The microSD host over Nordic's sEMMC soft peripheral on the FLPR (epic #1158): the card in
// native 4-bit SD mode, 32 MHz reads / 21.3 MHz writes. `sd.rs`'s whole transport.
mod card_io;
mod semmc;
// Which of the two soft-peripheral images owns the FLPR right now (epic #1158) — the display scan
// blob or the sEMMC host. The mode scheduler: lazy switching, one *never park mid-scan* gate, and
// the storage bring-up that holds the mode across `Semmc::start`.
mod flpr_mux;
// The **scratch arena** (#1146 P2): one RAM block time-shared by the three biggest blocks that are
// never live together — the per-frame render scratch, the nav block, and the USB staging buffer.
// The only place this feature's `unsafe` lives; the owner rules it composes with are the
// host-tested `obc_app::ArenaGate`.
mod arena;
// LS021 FLPR backend — the display: `main.rs` runs the real app on the reflective LS021
// panel via the FLPR (the VPR coprocessor). The FLPR presenter backend + launch live in
// `ls021_flpr`; `com::com_task` free-runs the COM lines (the FLPR drives frames; only the COM
// electrode square wave stays on the M33).
mod com;
// Zero-CPU hardware COM: drive the COM square wave from a TIMER→DPPI→GPIOTE toggle chain instead of
// the M33 `com_task`, so the panel's anti-DC-bias COM keeps alternating with no core wakes and the M33
// can WFI between events. The opt-in path and the default `com::com_task` both own the canonical
// P1.22/P1.23/P1.24 COM nets; hardware-waveform verification is why `com-hw` stays off by default.
#[cfg(feature = "com-hw")]
mod com_hw;
// The LS021/FLPR panel — this crate's display-contract presenter backend (the impls are folded in
// at the bottom of the module), the single screen-write interface the map plane drives through
// (`fb_mut` + `present`). The seam itself + the other backend (the simulator) live in obc-platform.
mod ls021_flpr;
// The two-plane display machinery both backends share (issue #351), one module per plane. `main`
// constructs the panels and spawns the tasks; the planes live here.
//   - The **input plane**: the high-priority input/overlay task + the gesture channel, and their
//     executor/ISR statics (the COM task is spawned onto that executor too).
mod input_plane;
//   - The **map plane**: the cfg-selected `MapDisplay` handle the ride loop drives the panel through.
mod map_plane;
// The map/ride thread-mode plane: `run_app` + its loop-only helpers. In every build (#270); on
// `ble` builds it runs joined with the BLE stack, both driving the shared SD + settings store.
mod ride;
// Persistent device settings over on-chip RRAM (the SD-independent settings store); boot-load +
// save-on-dirty are wired in `run_app`.
mod settings;
// The app-side DFU armer (epic #615 S4, #619): the board driver over `obc_dfu::armer` — stage
// scan + rollback snapshot (adapters in `sd.rs`), the boot-state page write (via `settings.rs`),
// the debug-link status stream, and the ride loop's trial confirm. In every build; the
// `dfu-install` trigger itself rides the `debug-uart` link until S5's UI lands.
mod dfu;
// Real GPS (SAM-M10Q) + altimeter (BMP581) on the shared TWIM22 I²C bus — the concrete transport + the
// event-driven sensor task. Compiled only on the **real-sensor** build (the default: neither `synth`
// nor `debug-uart`), since `synth`/`debug-uart` supply the location source instead.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
mod sensors;
// The BLE stack: MPSL + SDC + TrouBLE, the advertise loop, and the link-status plumbing.
// `ble` builds only; spawned beside the ride loop (see `spawn_ble_stack`).
#[cfg(feature = "ble")]
mod ble;
// The USB device plane (#889): the LM20's USBHS behind a vendor interface, carrying the *same*
// companion protocol as the radio — a browser or the desktop app plugs in and speaks the object
// model directly. **In every build** (there is no `usb` feature): the plane is part of the device,
// not an option of it. Spawned beside the ride loop (see `spawn_usb_stack`).
mod usb;
// The transport-free companion-link core: the §4.4 command handler, descriptor classification, the
// identity blobs, and the one shared `ObjectStore`. Both the radio and the USB plane call into it,
// which is what keeps "USB is a second transport, not a second protocol" true in the code rather than
// only in the spec. (One transfer at a time is the flat engine's, scoped per wire by
// `Engine::on_link_up` / `on_link_lost`.)
#[cfg(feature = "ble")]
mod link;
// The device object store: object ids / revision / upload state over the SD catalog, and the Config ↔
// RRAM-settings bridge every control plane drives. `ble` builds only.
#[cfg(feature = "ble")]
mod object_store;

// The feature matrix, now that there is nothing to guard against. MPSL *provides* the
// critical-section impl (its radio timing forbids global-interrupt-disable critical sections; two
// impls = duplicate link symbols), and #931 removed the only other one — `cs-single-core`, the flag
// for a radio-less shape that had not compiled for months and that nothing in CI built. `ble` is
// therefore the shipping build and the only one: it carries the map ride loop, and `debug-uart`
// (VCOM-fed ride) and `synth` (synthetic ride) compose with it — a headless ride beside a live BLE
// link is a useful combined-build test rig. A radio-less image, if one is ever wanted, is the `link`
// cfg rename described in Cargo.toml's `ble` note, not a feature flag to re-add.

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
// The async `Mutex` guards the shared SD + settings store ([`SharedStore`], every build) — a lock
// whose guard can outlive an `.await` (the BLE plane's per-chunk ops); the ride loop's holds are
// synchronous — render yes, present never (#809).
use embassy_sync::mutex::Mutex;
// The map/ride half of obc-app, alongside the shared `InputPlane`.
use obc_app::InputPlane;
use obc_app::{App, AppState};
use obc_display::ls021::{RowDiff, FRAME_H, FRAME_W};
use obc_reader::{MapCache, MapTables};
use obc_render::zoom_for_mpp;
// The decoded-route-geometry cache — resident in `.bss`, handed to the ride loop.
use obc_route::RouteCache;

// LS021 FLPR backend: the resident-framebuffer presenter backend + its launch, and the
// free-running COM driver. The M33 `com_task` is the DK/default path; the `com-hw` build drives COM
// from hardware instead, so the task isn't spawned there.
#[cfg(not(feature = "com-hw"))]
use com::com_task;
#[cfg(feature = "com-hw")]
use com_hw::HwCom;
use ls021_flpr::{launch_flpr, relaunch_flpr, FlprError, Frame64, Ls021Flpr};
// The two planes' entry points `main` reaches: the input plane's executor + task + gesture channel,
// and the map plane's display handle + boot-fault screen. (Unqualified so the input-plane items don't
// read against the `input_plane` value binding constructed below — they're different namespaces, but
// this keeps the call sites clean.)
use input_plane::{input_task, EXECUTOR_HP, GESTURES};
use map_plane::{show_boot_fault, MapDisplay};

// VCOM debug-sensor / telemetry stream, behind `debug-uart`: the interrupt-buffered UARTE on the DK's
// J-Link VCOM. `BufferedUarte` keeps RX DMA continuously armed into a ring driven by the SERIAL20
// interrupt, so the tens-of-ms map render never drops a streamed byte. 8N1 @ 115200.
#[cfg(feature = "debug-uart")]
use embassy_nrf::buffered_uarte::{BufferedUarteRx, BufferedUarteTx};
// ============================ Board memory budget ============================
// The nRF54L15 has 256 KB RAM and no external RAM, so the whole resident working set of a full map
// redraw must fit there. This build-time assert fails the build — rather than overflowing RAM on
// glass — if the shared crates' caps (trimmed by the `nrf-mem` profile, enabled on the obc-app edge in
// Cargo.toml) ever outgrow the budget. It compiles for thumbv8m (usize = 4 B), so every `size_of` here
// is the true on-device size.
//
// The binding moment is a full redraw with everything resident at once:
//   - `App`        the screen tree + ride/activity state: the resident elevation `Profile` (~2.3 KB at
//                  PROFILE_COLS=256), the `Breadcrumb` (~3 KB at SPINE_CAP=256), the POI/corridor
//                  scratches and the catalogs; ~44 KB total. It owns **no** render scratch since
//                  #1146 P1 — that block is an arm of the scratch arena below. (The #270 cull: these
//                  caps were roughly halved again so the map plane leaves room for the BLE
//                  stack in one image — the LM20 re-decides them generously.)
//   - scratch arena
//                  ONE block, `max(arms)` (#1146 P2 — `arena.rs`), replacing three that were never
//                  live together: the per-frame **render** scratch (`obc_render::RenderScratch`,
//                  117,408 B at the nrf-mem caps — every decode / collect / span / draw buffer a
//                  redraw needs at once), the **nav** block (`NavScratch` + `NavTileCache` +
//                  `NavPlanner`, ~80.6 KB, live only inside one route search), and the **USB**
//                  staging buffer (128 KiB, live only while a cable transfer streams). The USB arm
//                  is now the maximum: 13,664 B above the 117,408 B render arm, deliberately spent
//                  on two 64 KiB halves for efficient 128-block card writes.
//   - framebuffer  the single RGB222 frame: 240×320 × 1 B/px = 75 KB — the `FB` static below.
//   - `MapCache`   the streamed-map geometry/index cache (4 slots + 16 KB scratch, 37,084 B).
//   - `RouteCache` the decoded-route-chunk cache (2 slots on nrf-mem, ~6 KB).
//   - `RouteIndex` the active route's resident chunk index — the ride loop holds it across frames in
//                  the map plane's task future to stream geometry without re-walking it (128 chunks on
//                  nrf-mem, ~6 KB). Counted here because on the 256 KB part it materially shares the
//                  budget. Built **in place** in that resident slot (`RouteIndex::read_into`) — the
//                  earlier by-value `RouteIndex::read` build put the ~6.7 KB on the stack at the ride
//                  pass's deepest point, and the post-upload rescan's rebuild overflowed the main
//                  stack the moment `.bss` crept 216 B (STKOF HardFault, 2026-07-12).
//   - map extents  the open map's resolved FAT chain (#500) and the one `'static` source over it.
//                  This block used to be **20,732 B** — a caller-placed eleven-shard table plus eleven
//                  extent tables and eleven sources, resident even for a single-file map. FS7.5-c2
//                  deleted the set mount that needed them (#1420): one map is one file, so one
//                  table and one source is the whole of it.
// plus `STACK_RESERVE` headroom for the main stack + embassy's executor/task arenas. The stack must
// also absorb a per-redraw `Reader::new` (the OBCM style table → a ~2.4 KB `Reader` value built as a
// stack temporary, plus its own ~4 KB read scratch): the ride loop rebuilds it each frame, so the
// stack reserve carries that spike.
//
// The coprocessor carve-out leaves the M33 the SRAM below `SEMMC_RAM_BASE` (not the full 512 KB).
// It is **24 KB** now (#1158): the display blob is ~820 B in a 4 KB image/stack region plus the 4 KB
// handshake page, and below them sits the 20 KB sEMMC soft-peripheral carve — the SD host
// controller the FLPR runs in storage mode. And the FLPR map path packs the framebuffer straight to
// the LS021 wire, so there is no RGB565 band scratch to reserve — the same caps clear the budget.

/// Total SRAM the M33 app core sees — what the coprocessor carves leave, taken straight from the
/// generated contract (`build.rs` derives the carved `memory.x` and this constant from the same
/// `SEMMC_RAM_BASE` — the bottom of the lowest carve — so the budget can't fork from the linker map).
const NRF_RAM_BYTES: usize = ls021_flpr::M33_RAM_BYTES;
/// Headroom the [`RESIDENT_BYTES`] assert keeps free under the *itemized* statics, for the main
/// stack and embassy's executor/task arenas (statics grow up from the RAM base, the stack down from
/// the top).
///
/// ## ⚠️ This is NOT the stack-safety gate, and it used to claim it was
///
/// The claim it carried until FS7.5-c1 — that squeezing the residual below the measured deep-path
/// peak "fails at compile time instead of overflowing the stack on glass" — is **false**, and the
/// change that disproved it is the one that fixed this comment. FS7.5-c1 added 11,848 B of resident,
/// took the real residual stack under the recorded `ble` deep-ride peak, and this assert passed with
/// ~25 KB to spare.
///
/// The reason is structural, not a tuning problem. The assert compares [`RESIDENT_BYTES`] — a
/// **hand-maintained sum of the blocks someone remembered to add** — against `NRF_RAM_BYTES`. The
/// linker's answer is bigger: measured on this build, `RESIDENT_BYTES` is 401,034 B against a linked
/// `.bss + .data + .uninit` of 453,880 B, so the sum **undercounts the truth by 52,846 B**. Task
/// pools, `.L_MergedGlobals`, alignment padding, every static nobody itemized — none of it is here,
/// and none of it can be, because the total is a link-time fact and this is a `const`.
///
/// So read this constant as what it is: a coarse compile-time tripwire that catches someone adding a
/// *named, itemized* block far too large for the part. The gates that actually bound the stack are
/// in `resource_baseline.json`, measured from the linked ELF:
///
/// - `residual_stack_min` — the linked `_stack_start − __euninit`, charged for `.uninit` as well as
///   `.bss`, so moving bytes between sections cannot pretend to save any.
/// - `deep_ride_high_water` + `deep_ride_margin_min` — the residual against what the board **actually
///   reached on glass**. This is the one that answers the question this comment used to claim, and it
///   is the one FS7.5-c1 added after finding there was no such gate.
///
/// On the combined `ble` build the SDC/host futures and MPSL's ISRs also ride the main stack on top
/// of the deep-render path, which is why its recorded peak is the higher of the two.
const STACK_RESERVE: usize = 64 * 1024;
/// The single RGB222 framebuffer: one byte per pixel over the 240×320 frame = 75 KB.
const FB_BYTES: usize = FRAME_W * FRAME_H;

/// The map plane's residents (the table above). Includes the active route's `RouteIndex`, kept
/// resident across frames. The per-frame `RenderScratch` is **not** here any more: since #1146 P2 it
/// is an arm of [`ARENA_RESIDENT`], counted once for all three arms. Present in every build now
/// (#270 — map + BLE coexist).
const MAP_RESIDENT: usize = core::mem::size_of::<obc_app::App>()
    + core::mem::size_of::<obc_reader::MapCache>()
    + core::mem::size_of::<obc_reader::MapTables>()
    + core::mem::size_of::<obc_route::RouteCache>()
    + core::mem::size_of::<obc_route::RouteIndex>()
    + TERRAIN_RESIDENT;
/// The map's **terrain** (EL7, epic #1068): the [`TERRAIN`] slot (an OBCT reader + its four 512 B
/// tiles, ~2.1 KB) and the sidecar's own extent table/source in `sd.rs` (~1.3 KB). Resident
/// whether or not a terrain file is on the card, deliberately: the slot is what makes the emit
/// path's sampler a `.bss` object instead of a plan-frame local (#419/#501), and a budget that
/// moved with the card's contents would not be a budget.
///
/// **Its own term since #1146 P2, and deliberately not an arena arm.** It used to be counted inside
/// the router's `NAV_RESIDENT` sum, which read as though it lived and died with a search — it does
/// not: `App::sample_terrain` reads it at **every fresh fix** during a ride (EL8), i.e. exactly
/// while the map plane is rendering and no search is running. Folding it into the nav arm would have
/// handed the render arm's `memset` the altimeter's tile cache. It is state, not scratch.
#[cfg(has_nav)]
const TERRAIN_RESIDENT: usize = core::mem::size_of::<
    obc_elevation::TerrainElevation<'static, { obc_elevation::DEFAULT_TILE_SLOTS }>,
>() + core::mem::size_of::<obc_formats::io::WindowSource<'static>>();
#[cfg(not(has_nav))]
const TERRAIN_RESIDENT: usize = 0;
/// The **scratch arena** (#1146 P2, `arena.rs`): `max(render, nav, usb)`, once — the single term
/// that replaced `RenderScratch` in [`MAP_RESIDENT`], the three `NAV_*` blocks in the router's old
/// sum, and `STAGE_LEN` in [`usb::RESIDENT_BYTES`].
const ARENA_RESIDENT: usize = arena::ARENA_BYTES;
/// The BLE stack's residents (`ble::RESIDENT_BYTES`: the MPSL handle + SDC memory block + TrouBLE's
/// host arena + its global packet pool + the CRACEN RNG); zero without the feature. Keeping both terms
/// in one sum is what makes "`ble` + map don't fit on 256 KB" a *compile-time* fact: the map plane is
/// unconditional, so on the `ble` build both planes land here and this assert arbitrates.
#[cfg(feature = "ble")]
const BLE_RESIDENT: usize = ble::RESIDENT_BYTES;
#[cfg(not(feature = "ble"))]
const BLE_RESIDENT: usize = 0;

/// The USB device plane's residents (`usb::RESIDENT_BYTES`: the driver's EP-OUT staging buffer, the
/// descriptor + control buffers, and the two planes' frame/chunk scratch). Unconditional — the plane
/// ships in every build — and counted in the same sum as the map and BLE planes so "USB doesn't fit
/// beside them" is a *compile-time* fact rather than an on-glass overflow.
const USB_RESIDENT: usize = usb::RESIDENT_BYTES;

/// The resident set that must coexist during a redraw (see the table above).
const RESIDENT_BYTES: usize = FB_BYTES
    + core::mem::size_of::<RowDiff<FRAME_H>>() // the self-diffing present store (#201, 1.28 KB)
    + MAP_RESIDENT
    + ARENA_RESIDENT
    + BLE_RESIDENT
    + USB_RESIDENT
    + FLAT_RESIDENT
    + RIDE_RESIDENT;

/// The **flat store**'s residents (FS7.5-c1, `flat_store::RESIDENT_BYTES`): the mounted
/// `FlatStore<FlatCard>` in its `.bss` slot — ~10.5 KB, of which §6.2's free bitmap is 8 KiB — plus
/// the storage task's ~1 KB request queue. The alignment bounce its spans go through is `sd`'s,
/// already counted in this sum's FAT half.
///
/// **Unconditional, and that is deliberate.** The store mounts on every boot, because §5.6 step 1 is
/// how the board finds out which stack owns the card; a term that appeared only for flat cards would
/// be a budget that moved with the card's contents, which is not a budget. It is also the term that
/// makes the dev window's real cost legible: both stacks are linked at once until c4 closes it, and
/// this is the price of that, itemized rather than hiding in anonymous `.bss`.
///
const FLAT_RESIDENT: usize = flat_store::RESIDENT_BYTES;

/// FS8's recorder-owned append delta, present in every shipping build and separate from the store
/// so the ride task can lend it across a serialized journal request without growing its poll frame.
/// The store owns the durable partial 16 KiB page; this is only ~10 s of samples plus the footer.
const RIDE_RESIDENT: usize = flat_ride::RESIDENT_BYTES;
// ⚠️ **The budget has a cliff in it now** (#1146 P2), and it points both ways — read this before
// "optimizing" any of the three arena arms, and before waving one through:
//
//   * `ARENA_RESIDENT` is `max(render, nav, usb)`, not their sum. Growing an arm that is **below**
//     the maximum is **free** — genuinely, byte-for-byte free, not merely cheap: the nav and render
//     arms are both below today's 128 KiB USB arm. A
//     change that shaves KBs off one of those buys the device nothing, and the review time spent on
//     it is the whole cost.
//   * Growing the arm that **is** the maximum (today: USB) costs the full delta, 1:1, exactly as
//     it did when each block owned its own RAM.
//   * Crossing the line is where the surprise lives: an arm growing *past* the current maximum costs
//     only the part above it, and from then on it is the arm every future growth is measured
//     against. `arena.rs` has a compile-time assert that fires if that ever happens, because at that
//     point every note here names the wrong arm.
const _: () = assert!(
    RESIDENT_BYTES + STACK_RESERVE <= NRF_RAM_BYTES,
    "nRF resident set (framebuffer + RowDiff + map plane [App/MapCache/MapTables/RouteCache/RouteIndex/terrain/volume-set tables] + the #1146 scratch arena [max of render/nav/usb] + BLE stack [MPSL/SDC mem/host arena] + the USB plane + the flat store) + stack reserve overruns RAM — re-trim the `nrf-mem` caps, and mind the arena's max-of-arms cliff above: shrinking an arm that is not the largest frees nothing"
);

// A report-only table of exact target-side allocation sizes. Keeping the table in this crate gives
// it access to the board-private BLE arena types while `cfg(feature = "resource-report")` ensures
// it is absent from every shipping ELF. `resource_guard.py report` extracts this section without
// executing the firmware; the fixed-width names make the table self-describing and stale-parser
// failures loud. Do not use these entries for linked resident RAM: `.bss + .data` from the shipping
// ELF is the authority for that separate gate.
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

    #[cfg(feature = "ble")]
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
        // The protocol-v4 adapter's three buffers (FS7.5-c3a): the reaction buffer, one staged
        // control record and one held stream record. Named because the middle one is what §5's
        // cross-channel hold is made of, and a change in it is a change in that guarantee.
        entry("ble_v4_adapter", ble::V4_ADAPTER_BYTES),
    ];

    #[cfg(not(feature = "ble"))]
    const BLE_ENTRIES: [Entry; 11] = [
        entry("ble_total", 0),
        entry("ble_mpsl", 0),
        entry("ble_sdc_memory", 0),
        entry("ble_host_resources", 0),
        entry("ble_packet_pool", 0),
        entry("ble_cracen", 0),
        entry("ble_object_store", 0),
        entry("ble_server", 0),
        entry("ble_gap_name", 0),
        entry("ble_sensor_manager", 0),
        entry("ble_v4_adapter", 0),
    ];

    /// The terrain seam's entries (EL7). Unconditional, like the `nav_*` ones: these are the
    /// *types'* sizes, reported in every profile even where `has_nav` keeps the statics out of the
    /// image (the linked `.bss + .data` gate is what says whether they were allocated).
    const TERRAIN_ENTRIES: [Entry; 2] = [
        entry(
            "terrain",
            core::mem::size_of::<obc_elevation::TerrainElevation<'static, { obc_elevation::DEFAULT_TILE_SLOTS }>>(),
        ),
        // The §1.3 window the OBCT container is parsed through, now that terrain lives **inside**
        // the map file. It replaces the `terrain_extents` row — the sidecar's own extent table and
        // source — because there is no sidecar file to extent-map any more.
        entry("terrain_window", core::mem::size_of::<obc_formats::io::WindowSource<'static>>()),
    ];

    const ENTRIES: usize = 38;

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
        // One map, one file: the `set_shards` / `set_extent_tables` / `set_sources` rows (6,432 +
        // 14,212 + 88 = 20,732 B) were the volume-set mount's, and FS7.5-c2 deleted it. What a map
        // costs to read is one resolved FAT chain and one source over it.
        entry("route_cache", core::mem::size_of::<RouteCache>()),
        entry("route_index", core::mem::size_of::<obc_route::RouteIndex>()),
        // WX7's complete reader + generation-aware frame/directory/tile cache type. This is a
        // target-ABI size report, not a second allocation; the linked resident gate remains the
        // authority once WX10 places the cache in the rain-render path.
        entry("weather_reader_cache", obc_weather::READER_CACHE_RESIDENT_BYTES),
        // The **scratch arena** (#1146 P2) and its three arms. `arena_total` is the only one of the
        // three that is resident RAM — it is `max` of the other two, not their sum, and both are
        // reported beside it precisely so a reader can see *which* arm sets the total and how much
        // free headroom the other still has (the growth asymmetry; see `arena.rs`).
        //
        // Protocol v4 restores the USB arm as two 64 KiB halves: one is an aligned FLPR DMA source
        // while USB and CRC fill the other. Render already sets the same 128 KiB arena ceiling, so
        // this is an arm-composition change and not another resident allocation.
        entry("arena_total", arena::ARENA_BYTES),
        entry("arena_render", arena::RENDER_ARM_BYTES),
        entry("arena_nav", arena::NAV_ARM_BYTES),
        entry("arena_usb", arena::USB_ARM_BYTES),
        // The terrain seam's two statics (EL7 + FS7.5 §1.3): the sampler + tile cache, and the byte
        // window the OBCT container is parsed through — no longer a sidecar's extent table, because
        // there is no sidecar. Named because a change in the tile-slot count must be legible here
        // rather than as anonymous `.bss`.
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
        // The USB device plane's named statics (#889). Unconditional since USB stopped being a
        // feature, so it is itemized here for the same reason the BLE arenas are: it is the newest
        // resident block, and a growth in it should be legible in the report rather than only as a
        // few thousand anonymous bytes of `.bss`. This is the *named* half — the driver's own
        // endpoint bookkeeping and the task future are not nameable here and land in the linked
        // `.bss + .data` gate, which is the authority for resident RAM. Since FS7.5-c3b it includes
        // the v4 adapter's three record buffers (`usb::v4::RESIDENT_BYTES`), which is where the two
        // buffers the v1 planes owned went and then some: §5.2's 4,112-byte ceiling buys a stream
        // record that reaches the card as one write.
        entry("usb_named", usb::RESIDENT_BYTES),
        // The sEMMC storage transport's two resident blocks (#1158) — the baseline named these
        // when the pivot landed, but the rows themselves were forgotten, which broke the report
        // gate on every PR: the 4-block alignment bounce (`sd.rs`, fires only for misaligned
        // callers) and the `Semmc` host-driver state itself.
        entry("sd_bounce", card_io::BOUNCE_BYTES),
        entry("semmc_driver", core::mem::size_of::<semmc::Semmc>()),
        // The **flat store** (FS7.5-c1), itemized in separate rows because they answer
        // different questions. `flat_store` is the store type itself — §6.2's 8 KiB free bitmap plus
        // the hold/reservation rows — so a change in `MAX_OPEN_OBJECTS` or an extent-index widening
        // is legible here. `flat_requests` is the storage task's queue, the one part of that layer
        // whose size is a design choice rather than a consequence: depth × the largest request. The
        // `flat_catalog_uploads` is the bounded handoff from successful route/trip commits to the
        // app's typed upload events. The binding's alignment buffer is not another row — it is
        // `sd_bounce`, shared (see
        // `flat_store`'s note on why it places none of its own).
        entry("flat_store", core::mem::size_of::<obc_storage::flat::FlatStore<flat_store::FlatCard>>()),
        entry("flat_requests", flat_store::REQUEST_QUEUE_BYTES),
        entry("flat_catalog_uploads", flat_store::CATALOG_UPLOAD_BYTES),
        // FS8's live-ride append delta + recovery summary. The store owns the durable partial
        // 16 KiB page; this static retains only ~10 s of samples plus the final footer.
        entry("flat_ride_delta", flat_ride::RESIDENT_BYTES),
        // The read cutover's own resident cost on the flat arm (FS7.5-c2): the session-long
        // `StoreSource` over the map object **and** the display name the same boot step captures.
        // Named beside the store it reads from so the two halves of "what does reading a flat card
        // cost" are in one place, and named as one row because they are one boot step's residue.
        entry("flat_map_read", flat_store::MAP_READ_BYTES),
        // The selected Route and WeatherBundle revisions each spend one bounded flat-store hold
        // row and one `StoreSource`. They are released/reopened on catalog movement, unlike the map
        // source which remains held for the whole boot.
        entry("flat_route_read", flat_store::ROUTE_READ_BYTES),
        entry("flat_weather_read", flat_store::WEATHER_READ_BYTES),
        // The protocol-v4 transfer engine (FS7.5-c3a), which lives in the storage task because that
        // is the one execution context allowed to write. Mostly its staging buffer, still the
        // 512-byte minimum after c3b brought USB — and that is a decision, not an omission: §5.2's
        // ceiling makes a full stream record four whole stages wide, so it bypasses the stage
        // entirely and reaches the card in one write. See `flat_store::ENGINE_STAGE`.
        entry("flat_engine", flat_store::ENGINE_BYTES),
    ];
}

/// The resident device-native RGB222 framebuffer: one byte per pixel over the 240×320 panel
/// (`FB_BYTES` = 75 KB), in `.bss`. [`App::render_map`](obc_app::App::render_map) quantizes into it on
/// store ([`FbDevice64`]). It is owned by the `Ls021Flpr` panel — the map plane renders into it and
/// `push_frame` packs it straight to the LS021 wire (the FLPR reads it directly out of shared SRAM).
static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// The **self-diffing present** store: one 32-bit hash per framebuffer row of the last-pushed frame,
/// in `.bss` (320 rows = 1.28 KB). The active display backend borrows it (`&mut`) and, on present,
/// re-hashes each row and pushes only the rows whose hash changed — so a Home clock tick re-presents
/// its clock band instead of all 320 rows (~44 ms since #348 → a few ms on the FLPR). `RowDiff::new()` is all-zero
/// (+ the unprimed flag) ⇒ a `.bss` static, and the first present force-pushes the whole frame to seed it.
static mut ROW_DIFF: RowDiff<FRAME_H> = RowDiff::new();

/// The streamed-map geometry cache + the shared [`App`], placed in `.bss` and built **in place** (a
/// `ptr::write` into the reserved region): the ~44 KB `App` and the 37 KB cache must never form on
/// the part's small stack. [`MapCache::new`](obc_reader::MapCache) is an all-zero
/// `MaybeUninit::zeroed`, so writing it is a `.bss` memset.
static mut MAP_CACHE: MaybeUninit<MapCache> = MaybeUninit::uninit();
/// The immutable map tables (header scalars + style table + LOD pyramid), parsed **once at boot** into
/// `.bss` and borrowed by every per-frame [`Reader`]. Resident so the per-frame render reader carries
/// no styles/LODs of its own — no per-frame style-table SD read, no ~4 KB parse stack spike on the deep
/// render path (the lever that kept that path inside the 256 KB stack).
static mut MAP_TABLES: MaybeUninit<MapTables> = MaybeUninit::uninit();
static mut APP: MaybeUninit<App> = MaybeUninit::uninit();
/// The decoded-route-geometry cache, placed in `.bss` and built in place like [`MAP_CACHE`]
/// ([`RouteCache::new`](obc_route::RouteCache) is an all-zero `MaybeUninit::zeroed`). The session-long
/// cache spares a redraw of the unchanged route + the matcher's per-fix decode from re-reading `.obcr`
/// geometry off the card every frame.
static mut ROUTE_CACHE: MaybeUninit<RouteCache> = MaybeUninit::uninit();
/// The SD/settings mutex is initialized on exactly one boot path: the normal ride application or
/// the USB recovery plane used when the selected map is structurally unreadable.
static mut SHARED_STORE_SLOT: MaybeUninit<SharedStoreMutex> = MaybeUninit::uninit();
// (The router's fixed A* table, its graph-tile cache and the resumable planner's slot were three
// `.bss` statics here until #1146 P2. They are one struct now — `arena::NavArm` — living in the
// scratch arena, which the ride loop claims for the span of a search and the render path claims
// back for its frames. Nothing about their per-request discipline changed: `NavScratch` /
// `NavTileCache` are reset before the first step reads them and the planner is still
// `ptr::write`-replaced per request, so nothing outlives one search. The terrain below is the block
// that deliberately stayed resident — see `TERRAIN_RESIDENT`.)
/// The mounted map's **terrain** (EL7, epic #1068): the OBCT reader plus its `N = 4` tile cache —
/// ~2.1 KB of resident raster + a 32-byte header, in `.bss` for exactly the reason the caches above
/// are (`TerrainElevation` embeds the cache; a stack copy of one inside the emit path is precisely
/// the #419/#501 fat local). Written once at boot **only when the map carries a §1.3 terrain
/// region**; the ride loop then holds a `&'static mut` to it (or to [`NULL_ELEV`]) and hands it to
/// every planner step.
#[cfg(has_nav)]
static mut TERRAIN: MaybeUninit<obc_elevation::TerrainElevation<'static, { obc_elevation::DEFAULT_TILE_SLOTS }>> =
    MaybeUninit::uninit();
/// The §1.3 window the OBCT container is read through — the map file's bytes, re-based so the
/// container's first byte is byte `0`. `.bss` and `'static` because [`TERRAIN`]'s parsed reader
/// borrows its source for the session, so the window must outlive every frame that samples it.
#[cfg(has_nav)]
static mut TERRAIN_WINDOW: MaybeUninit<obc_formats::io::WindowSource<'static>> = MaybeUninit::uninit();
/// The no-terrain source (a ZST): what the ride loop hands the planner when no sidecar mounted, so
/// the emit path has one uniform seam and no `Option` branch per point.
#[cfg(has_nav)]
static mut NULL_ELEV: obc_route::NullElevation = obc_route::NullElevation;

/// Mount the map's **embedded terrain** (EL7 + FS7.5) into the `.bss` [`TERRAIN`] slot and hand back
/// the resident sampler.
///
/// **Terrain is inside the map file now** (`OBCM_Spec.md` §1.3, #1420): the header names a byte
/// window holding one OBCT container verbatim, and every offset inside that container is relative to
/// its own first byte — so what the parse needs is a *window*, not a file. This forms one
/// ([`obc_formats::io::WindowSource`]) over `map` and parses through it. There is no `.OBD` sidecar
/// any more: no second file to open, no second FAT chain to resolve, no orphaned raster of a map
/// that was replaced, and — the reason it matters here — **one arm instead of two**, because a flat
/// card has no filesystem to hang a sidecar off in the first place.
///
/// The source must be `'static`: [`TerrainElevation`](obc_elevation::TerrainElevation) borrows the
/// flat store's mounted map source for the session.
///
/// **Every other failure is `None`, and `None` is not a fault** — the rule the sidecar had, and this
/// part *is* unchanged: no region, a window the header places outside the file, or a container that
/// will not parse all mean *this map has no terrain*, and the ride loop uses [`NULL_ELEV`] so routes
/// plan and ride exactly as they did, flat. A rider whose raster is unreadable has the map they
/// would have had without one.
///
/// `#[inline(never)]` is load-bearing (#677): `TerrainElevation` embeds its tile cache, so the
/// by-value parse temporary is ~2.1 KB. In this transient frame it pops with the call; inlined
/// into the ride task's async block it became a permanent ~2 KB slot in the task's poll frame —
/// allocated at entry on **every** poll — which is what tipped boot over the residual main stack
/// (STKOF HardFault at `link::init_store`'s prologue, 2026-08-03).
///
/// `clippy::mut_from_ref` fires on the signature and is allowed here for a reason the lint cannot
/// see: the `&'static mut` does not come from `map`. It comes from [`TERRAIN`], a `.bss` slot this
/// function solely owns and writes at most once per boot — the same shape `mount_terrain` always
/// had, and the lint only started firing because the input it used to take (`&mut sd::Storage`)
/// happened to be mutable.
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

/// Build a `'static` value into a `.bss` [`MaybeUninit`] slot, returning the sole `&'static mut` to it
/// — the warm-reset-safe replacement for `StaticCell` that every runtime-built shared static (the bus
/// mutex, the `InputPlane` mutex, the VCOM rings, the map/route caches) is created through.
/// `StaticCell`'s one-shot `used` flag panics ("already full") if it is ever non-zero on entry, which
/// on this board's debug-reset path it can be; an unconditional in-place [`ptr::write`](core::ptr)
/// carries no such flag, so it survives a warm reset. `#[inline(always)]` so the by-value `val` never
/// lands on the stack — a zeroed `MaybeUninit::new` packs straight to a `.bss` memset.
///
/// # Safety
/// `slot` must point at a `static mut MaybeUninit<T>` that is initialised **exactly once** for the
/// program's life through this call and never aliased elsewhere — the returned reference is the only
/// one handed out. (`MaybeUninit<T>` shares `T`'s layout, so the cast is sound.) Each call site
/// passes a distinct slot.
#[inline(always)]
unsafe fn init_static<T>(slot: *mut MaybeUninit<T>, val: T) -> &'static mut T {
    let ptr = slot as *mut T;
    ptr.write(val);
    &mut *ptr
}

/// The two persistent resources both thread-mode planes drive: the mounted SD card (`None` = no
/// card — the map plane then idles, the BLE plane still serves config/bond/diagnostics) and the RRAM
/// settings store. Held behind an async [`Mutex`] so a locker can keep the guard **across an
/// `.await`** where it must (the BLE object plane takes it per chunk, around channel awaits) —
/// which a `RefCell` can't (the ble planes avoid that only by never borrowing across an await).
/// The ride loop holds it in two short per-pass scopes — the store phase (reconcile + the sync
/// map render, which borrows the open SD handles) and the post-present tail — and **never across
/// the display present** (#809), so BLE store ops interleave with the ~44 ms FLPR scan.
/// `NoopRawMutex` suffices: both planes are cooperative futures on the one
/// thread-mode executor and no ISR touches storage, so no critical section is needed.
pub(crate) struct SharedStore {
    pub(crate) storage: Option<sd::Storage>,
    pub(crate) settings: settings::RramSettingsStore,
}
/// The shared-store handle threaded into [`ride::run_app`] and the BLE object plane (#270).
///
/// A newtype over the async [`Mutex`] rather than an alias, because since the storage pivot
/// (#1158) taking the card means two things, not one: getting the `Storage` value **and** getting
/// the FLPR into storage mode. Wrapping the lock is what makes every one of the ~50 `.lock().await`
/// sites acquire both without a single one of them changing — and what makes it impossible to add a
/// site that forgets. See [`flpr_mux::storage_session`](crate::flpr_mux::storage_session) for what
/// the second half does (it waits out a live panel scan by yielding; it is deliberately *not* a
/// second lock, so it can be held across an `await` and cannot deadlock against a frame push).
pub(crate) struct SharedStoreMutex(Mutex<NoopRawMutex, SharedStore>);

impl SharedStoreMutex {
    pub(crate) const fn new(store: SharedStore) -> Self {
        Self(Mutex::new(store))
    }

    /// Take the card: the store value, plus the FLPR in storage mode.
    pub(crate) async fn lock(&self) -> StoreGuard<'_> {
        let inner = self.0.lock().await;
        StoreGuard { inner, _flpr: flpr_mux::storage_session().await }
    }
}

/// What [`SharedStoreMutex::lock`] hands back — the mutex guard with the storage session riding
/// along, so the mode is ensured for exactly the scope the store is held. Derefs to [`SharedStore`],
/// which is why no call site had to move.
pub(crate) struct StoreGuard<'a> {
    inner: embassy_sync::mutex::MutexGuard<'a, NoopRawMutex, SharedStore>,
    _flpr: flpr_mux::StorageSession,
}

impl core::ops::Deref for StoreGuard<'_> {
    type Target = SharedStore;
    fn deref(&self) -> &SharedStore {
        &self.inner
    }
}

impl core::ops::DerefMut for StoreGuard<'_> {
    fn deref_mut(&mut self) -> &mut SharedStore {
        &mut self.inner
    }
}

/// Spawn-trampoline for the BLE stack (#270). Constructing [`ble::run`]'s spawn token
/// materializes its future as a **stack temporary in the constructing function's poll frame** —
/// and Rust allocates a poll frame's full slot set at entry, so when that future was ~31 KB,
/// doing it in `main` cost `main` a ~31 KB frame from its first instruction and overflowed the
/// combined build's ~40 KB stack before boot even reached the SD card (caught by DWT watchpoint:
/// stack frames overwriting `.bss` task pools → a bogus "Busy" at the com_task spawn). The #677
/// evictions since shrank the future to ~4 KB (the big values live in `ble`'s `.bss` statics now),
/// but the trampoline stays: it keeps `main`'s frame independent of whatever `ble::run`'s future
/// grows to. This tiny task is polled directly by the executor at ~2 KB depth; its own pool
/// static holds just the arguments until the inner spawn moves them into `ble::run`'s.
#[cfg(feature = "ble")]
#[embassy_executor::task]
async fn spawn_ble_stack(
    spawner: Spawner,
    mpsl_p: nrf_sdc::mpsl::Peripherals<'static>,
    sdc_p: nrf_sdc::Peripherals<'static>,
    cracen_p: embassy_nrf::Peri<'static, embassy_nrf::peripherals::CRACEN>,
    stores: link::LinkStores,
    sensor_injector: obc_platform::sensor_hub::SampleInjector<'static>,
) {
    spawner.spawn(defmt::unwrap!(ble::run(spawner, mpsl_p, sdc_p, cracen_p, stores, sensor_injector)));
}

/// Spawn-trampoline for the USB device plane (#889) — the same reasoning as [`spawn_ble_stack`]:
/// constructing [`usb::run`]'s spawn token materializes its future as a stack temporary in the
/// constructing function's poll frame, and a poll frame's full slot set is allocated at entry. Doing
/// it in `main` would charge `main` for the USB task's whole state machine from its first
/// instruction, on every poll, forever. This tiny task's own pool static holds just the arguments
/// until the inner spawn moves them into [`usb::run`]'s.
#[embassy_executor::task]
async fn spawn_usb_stack(spawner: Spawner, usb_p: embassy_nrf::Peri<'static, embassy_nrf::peripherals::USBHS>) {
    spawner.spawn(defmt::unwrap!(usb::run(usb_p)));
}

/// Keep card provisioning and map replacement available when boot cannot open a map.
///
/// The ordinary composition point sits after map parsing because the ride loop needs the parsed
/// tables. A damaged map must not make the very cable used to replace it disappear, though. This
/// reduced boot path starts only USB; the storage task — the write half **and** the protocol-v4
/// engine — was already spawned further up, on every card, so the plane has an engine to answer
/// with here exactly as it does on a normal boot.
///
/// **It is card-agnostic since FS7.5-c3b**, where it used to be the FAT arm's. A v4 `PUT` goes to
/// the flat store, so this needs no `Storage`, no object store, no shared mutex and no arena arm —
/// and on an unformatted card the honest outcome is the engine's own: ordinary object operations
/// answer `readOnly/unformatted`, while the explicit destructive `FORMAT` operation can initialize
/// it over this recovery link.
///
/// The one thing it must still do is seed the firmware revision, because §5.2.1's EP0 device-info
/// request serves it and "an update is available" compares against it.
async fn spawn_map_recovery_usb(
    spawner: Spawner,
    usb_p: embassy_nrf::Peri<'static, embassy_nrf::peripherals::USBHS>,
    rramc: embassy_nrf::Peri<'static, embassy_nrf::peripherals::RRAMC>,
) -> Option<arena::UsbGuard> {
    let mut settings_store = settings::RramSettingsStore::new(rramc);
    dfu::seed_firmware_revision(&mut settings_store);
    // No ride loop, map render or route search exists on this boot path. Retain the arena guard in
    // the caller's diverging fault scope and pre-grant it, so the first post-FORMAT map upload gets
    // the same double-buffer DMA path as an ordinary mounted boot.
    let stage = arena::claim_usb(obc_app::TransferReady::recovery_boot()).ok();
    usb::set_stage_granted(stage.is_some());
    spawner.spawn(defmt::unwrap!(spawn_usb_stack(spawner, usb_p)));
    defmt::warn!("usb: card-recovery plane active — format if needed, then upload a map and reboot");
    stage
}

/// Idle camera zoom for the boot map, in ground metres-per-pixel (the 0.5–4 mpp riding band). A
/// coarse-ish 2 mpp shows a town-scale overview rather than a tight patch.
const INIT_MPP: f32 = 2.0;

/// Heartbeat-only idle for an unrecoverable bring-up failure: blink LED0 forever rather than panic —
/// a missing/bad card must **never** fault (acceptance criterion). The storage faults (no card, no
/// `.obcm`, unreadable map) first paint an undismissable [`show_boot_fault`] screen so the
/// rider sees *why*, then land here; a bare heartbeat (no glass) remains only for a failure with no
/// panel to draw on — an FLPR launch that never came alive. Diverges.
async fn idle_blink(led: &mut Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

/// Stack high-water guard: [`paint`] fills the free stack with a sentinel early in `main`; [`used`]
/// then reports the deepest reach by finding the lowest still-painted word (the stack runs
/// `_stack_start` top → `_stack_end` bottom, and a deep call overwrites the sentinel). The ride loop
/// logs only on a *new* peak, so it's silent once warm but flags any future change that creeps the deep
/// route-load render toward the 256 KB-DK's ~36 KB ceiling (#352 kept it, cheap, for the DK era).
///
/// The scan must be **bottom-up to the first non-painted word** — a frame doesn't write every word it
/// covers (big uninitialized locals leave painted *islands* inside the used region), so any scan that
/// starts from the used side under-reports by whole buffers (measured on glass: a top-down variant saw
/// 4.7 KB where the truth was 26 KB). What makes it cheap instead: sentinel evidence is *permanent*
/// (an overwritten word never repaints), so [`used`] runs the full scan at most once per
/// [`SCAN_INTERVAL_MS`] and returns the cached mark between scans — a peak is never lost, it just
/// logs up to a second late. Steady state is one timestamp compare per wake; each actual scan costs
/// reads proportional to the *remaining headroom* (~40–90 µs at 128 MHz).
mod stackmeter {
    use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    const PAINT: u32 = 0xC0DE_DEAD;
    /// Minimum gap between two full scans. Peaks land in the log within this of the wake that made
    /// them — prompt enough for the VCOM-harness milestone readings (each step dwells ≥1 s).
    const SCAN_INTERVAL_MS: u32 = 1000;
    /// Wall-clock (loop `now`, ms) of the last full scan.
    static LAST_SCAN_MS: AtomicU32 = AtomicU32::new(0);
    /// The last scan's result. 0 = never scanned (paint leaves ≥512 B unpainted below the paint-time
    /// SP, so a real measurement can't be 0) → the first call always scans.
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
    /// Bytes of stack used at the deepest point reached so far (the high-water mark). `now` is the
    /// caller's loop timestamp (ms); calls within [`SCAN_INTERVAL_MS`] of the last scan return the
    /// cached mark without touching the stack region.
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
    /// Total usable stack (`_stack_start - _stack_end`).
    pub fn total() -> usize {
        top() - bottom()
    }

    /// Arm the ARMv8-M **MSPLIM** hardware stack-limit register (#677): any main-stack push or
    /// `sp` move below the limit raises a STKOF UsageFault (→ HardFault here) **at the moment of
    /// overflow**, instead of the overflow silently smashing whatever static tops `.bss` — which
    /// is `defmt_rtt::BUFFER`, so the pre-#677 failure mode was a corrupted RTT ring, a trashed
    /// MPSL exception frame, and a wild-prefetch crash with an unreadable backtrace. The limit
    /// sits [`HANDLER_MARGIN`] above the true bottom so the fault handler + panic-probe's defmt
    /// output have real stack to run on (exception-entry writes below MSPLIM are suppressed by
    /// the core, so even that margin never corrupts the statics below `_stack_end`).
    pub fn arm_limit() {
        /// Room left below the limit for the STKOF exception frame + the HardFault/panic path
        /// (≤104 B frame + defmt formatting). Costs 512 B of usable headroom — accounted as part
        /// of `STACK_RESERVE`'s margin over the measured deep-path peak.
        const HANDLER_MARGIN: usize = 512;
        // SAFETY: raising a fault on genuine overflow is strictly safer than the silent
        // corruption it replaces; nothing legitimately moves MSP below `_stack_end`.
        unsafe { cortex_m::register::msplim::write((bottom() + HANDLER_MARGIN) as u32) };
    }

    /// [`used`], but forcing a **fresh** full scan regardless of the [`SCAN_INTERVAL_MS`]
    /// throttle — for a caller that just finished a stack-notable operation (the nav router's
    /// per-plan RTT line) and wants the peak *including it* now, not up to a second late.
    /// Sentinel evidence is permanent, so this is still just one bottom-up scan (~40–90 µs).
    /// Its one caller is the router's RTT line, so it's gated with it (`has_nav`).
    #[cfg(has_nav)]
    pub fn rescan(now: u32) -> usize {
        LAST_USED.store(0, Ordering::Relaxed); // 0 = "never scanned" → `used` always rescans
        used(now)
    }
}

/// VCOM RX → sensor signals: read bytes from the interrupt-fed ring and feed each complete
/// `F`/`A`/`C`/`K`/`Z` line into `obc-platform`'s fresh-fix signals, which the app's
/// `DebugLocation`/`DebugAltimeter`/`DebugCompass`/`DebugInput` poll. Injected HR/power/cadence
/// (`H`/`P`/`R`) route through the hub's [`SampleInjector`](obc_platform::sensor_hub::SampleInjector)
/// — the *same* mailboxes the BLE manager feeds (last-writer-wins). A UART never "disconnects", so
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

/// VCOM TX ← telemetry + DFU status: send one compact line each time the app publishes telemetry
/// (~2 Hz via `set_telemetry`) or the DFU armer queues a `D` status line (S4, #619 — the on-glass
/// gate's phase/error readout), so the host's readout updates without the device polling or
/// flooding the link. The buffered UARTE chunks the line to DMA itself, so no manual packet
/// splitting is needed (both lines ≤192 B fit the TX ring); just loop until the whole line is queued.
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

/// The board's one instance-owned sensor hand-off (#808): every cross-task sensor stream owned by a
/// single [`SensorHub`](obc_platform::SensorHub) placed in static storage here, split into typed
/// producer/consumer/control handles wired to the sensor task, the ride loop, the BLE central
/// manager, and the debug-uart injection path at composition (below) — the successor to the former
/// process-global `sensor_link`/`sensor_values` mailboxes. `const`-constructed, so its `.bss`
/// footprint is exactly the scattered statics it replaces. Absent only on a radio-less pure
/// `synth` build: a `synth + ble` build still needs the injector for BLE sensor connections even
/// though the ride loop itself drives `SynthLocation` and consumes no hub stream.
#[cfg(any(feature = "ble", not(all(not(feature = "debug-uart"), feature = "synth"))))]
static SENSOR_HUB: obc_platform::SensorHub = obc_platform::SensorHub::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = board::init!();

    // Arm the hardware stack limit first (#677 — overflow = an immediate, precise fault, never
    // silent corruption of the statics below the stack), then paint the stack (still shallow) so
    // the ride loop's high-water guard can read the peak.
    stackmeter::arm_limit();
    stackmeter::paint();

    // The boot banner: which build is running, as `pkg-version+fw_git` (the DIS Firmware Revision
    // string, A4). This is the line the DFU on-glass gate reads to prove the device came back as
    // the staged version after an install (epic #615 S4).
    info!("obc-fw-nrf54l {=str}+{=str}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));

    let reset_reas = {
        let v = board::take_reset_reason!();
        if v & 0x6 != 0 {
            // Bits 1..2 are the two watchdogs. The WDT31/WDT0 instance used here reports as bit 2.
            defmt::error!("boot: WATCHDOG reset (RESETREAS=0x{=u32:08x}) — a plane wedged last session", v);
        } else {
            defmt::info!("boot: RESETREAS=0x{=u32:08x}", v);
        }
        v
    };

    // LED1 (P1_25) heartbeat — a liveness blink visible even before looking at the panel.
    // (LED0's pin P1.22 carries VCOM — its buffered LED shimmers at 60 Hz, a free COM-alive light.)
    let mut led = Output::new(p.P1_25, Level::Low, OutputDrive::Standard);

    // load → ride → save: stream the SD `.obcm` into the resident RGB222 framebuffer through the
    // shared `obc-app`, pick a route from the card catalog, ride it (VCOM-streamed GPS or the
    // `SynthLocation` square loop), map-match + record the final ride sample bytes, and append the
    // flat object's footer on Finish (GPX export happens phone-side after sync).
    {
        // --- VCOM debug-sensor stream, behind `debug-uart`. Bring it up first so the J-Link VCOM is
        // live while the SD card + panel come up; the parsed fixes land in obc-platform's signals, ready
        // for the app's sensor poll in the loop below. The nRF54L15 has no USB peripheral, so the fake
        // GPS/baro/compass feed and ride telemetry ride UARTE20 on the DK's onboard J-Link VCOM (TX
        // P1_16 / RX P1_17); defmt logs share the same cable over RTT. The RX ring is interrupt-fed
        // (`BufferedUarte`), so the tens-of-ms map render never drops a byte. Without the feature the app
        // rides the always-on `SynthLocation` stand-in. ---
        #[cfg(feature = "debug-uart")]
        {
            // 'static ring buffers backing the interrupt-fed UARTE (RX accumulates streamed bytes across
            // a long map render; TX queues the ≤192 B telemetry line). Parked in `.bss` and written in
            // place (the warm-reset-safe pattern), then the `&'static mut` halves move into the spawned
            // `'static` tasks. RX is 512 B: a multi-second synchronous nav plan (epic #116, R4)
            // starves the RX task between watchdog callbacks, and the ~1 Hz host feed overran the
            // old 256 B ring mid-plan (`BufferedUarte buffer overrun` on-glass) — 512 B rides out
            // several seconds of F/A/C lines. Cheap harness resilience, not a fix for the
            // starvation itself (accepted for v1 — see `ride::nav_plan_only`).
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

        // --- Real GPS + altimeter, default build only. The event-driven sensor task probes both
        // chips and publishes coherent datapoints into `SENSOR_HUB`; board owns TWIM22/TX-ready and
        // its P3 interrupt policy. ---
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        {
            // EasyDMA can't fetch a write buffer from flash, so byte-literal register writes need a RAM
            // bounce buffer; 32 B covers the widest VALSET frame. Parked in `.bss` + written in place
            // (the warm-reset-safe pattern), then moved into the `Twim`.
            static mut TWIM_TX_BUF: MaybeUninit<[u8; 32]> = MaybeUninit::uninit();
            // SAFETY: written once here, then owned solely by the `Twim` for the program's life.
            let twim_tx = unsafe { init_static(core::ptr::addr_of_mut!(TWIM_TX_BUF), [0u8; 32]) };
            let (twim, txready) = board::input_hardware!(sensors p, twim_tx);
            _spawner.spawn(defmt::unwrap!(sensors::sensor_task(twim, txready, SENSOR_HUB.task_link())));
            info!("sensors: SAM-M10Q + BMP581 task spawned on TWIM22 (SDA P1.04 / SCL P1.03, TX-Ready P1.05)");
        }

        // ============= FLPR LS021 backend: two-plane display + input =============
        // The map plane owns the `Ls021Flpr` panel directly (it scans a whole frame per push, so there
        // is no partial-window overlay to serialise — no bus mutex). The M33 configures every line the
        // FLPR drives (held as outputs for the program's life); `com_task` + the gesture `input_task`
        // share the one high-priority executor (COM must keep alternating whatever the map plane does).
        //
        // ⚠️ These five P1 gate/BSP lines **must match `src/flpr/flpr_scan.c`'s masks** — confirm
        // each is broken out on your DK and remap all three together if not (the source bus and BCK
        // stay on P2; COM is the P1.22–24 group pinned in `board`).
        let mut display = {
            // Gate + frame lines: one contiguous P1.10–14 run (with BSP below) so the gate harness
            // is a single uninterrupted cable on the DK's port-1 header. (P1.01/02 stay NFC.)
            let gate_bus = [
                Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // GSP
                Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GCK
                Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
                Output::new(p.P1_13, Level::Low, OutputDrive::Standard), // INTB
            ];
            // Source bus on the **rehomed** map (epic #1158): BSP on P1.14 (the lone P1 source
            // line), BCK unchanged on P2.07, and the six data lines split between the four pads the
            // retired SD-SPI path freed and the two shared with the card.
            //
            // `B0`/`B1` (P2.00/P2.04) are the shared ones: claimed here as plain `Output`s so the
            // M33 owns their direction and drive for the program's life, exactly like every other
            // display line — but their `CTRLSEL` is flipped per mode by `flpr_mux` (GPIO for the
            // display blob's `OUTSET`/`OUTCLR`, VPR for the sEMMC peripheral). The four **card-only**
            // pads (P2.01/02/03/05 = CLK/D0/D2/CMD) are deliberately *not* embassy peripherals: they
            // belong to the soft peripheral, which configures them itself, and their display-mode
            // parking (Input, no pull — the external pull-ups hold the SD bus idle-high) is
            // `semmc::configure_display_pads`'s job. Claiming them here would mean two owners for
            // one pad and an `Output` drop could re-drive a card line mid-transfer.
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
            // COM electrode lines (56–77 nF load each → high-drive), boot `Lo` and held `Lo` through the
            // init-black frame below, then started. Default (DK): three plain `Output`s the M33
            // `com_task` toggles at 60 Hz — VCOM=P1.22, VB=P1.23 in phase, VA=P1.24 inverse. COM lived
            // on P2.07/08/10 until the #1158 rehome gave the whole of P2 to the source bus + the microSD
            // card (those three are `BCK`/`R1`/`G1` now). With `com-hw`, the same P1.22–24 nets become
            // GPIOTE20 **toggle** channels that a TIMER+DPPI chain free-runs with zero CPU (so the M33
            // can WFI between events). `HwCom::start` establishes VA's inverse phase before enabling
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

            // Launch the FLPR (copy the blob, arm the control block, wait ALIVE), with **one full
            // relaunch retry** on failure (#349) — a one-off cold-boot race deserves a second
            // attempt before the device gives up on the panel. A launch failure must never fault —
            // degrade to a bare heartbeat idle. (Unlike the storage faults below, there's no live
            // panel here to paint a fault screen on, so this one stays LED-only.)
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

            // The resident RGB222 plane the app renders into and the FLPR packs to the wire —
            // wrapped as the contracts' `Frame64` and owned by the map plane *next to* the
            // presenter — plus the self-diffing present store the masked push derives its dirty
            // rows from.
            // SAFETY: sole references to FB / ROW_DIFF; held by the map plane's frame/panel pair
            // for the rest of the program (the map plane is their only owner), never aliased.
            let fb: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
            let diff: &'static mut RowDiff<FRAME_H> = unsafe { &mut *core::ptr::addr_of_mut!(ROW_DIFF) };
            let frame = Frame64::new(fb);
            let mut panel = Ls021Flpr::new(diff);
            // Datasheet Initial #0: an INTB-framed all-black frame (FB boots zeroed = black) while COM is
            // still held `Lo`. Then T4 ≥ 30 µs, then start COM — from here it free-runs forever.
            panel.push_frame(&frame).await;
            Timer::after_micros(50).await;

            // The shared `InputPlane`: `input_task` recognises + animates the bulge under this lock; the
            // map plane composites it into a partial overlay push. Parked in `.bss` + written **in
            // place** (not `StaticCell` — its one-shot flag can panic "already full" on a warm reset).
            static mut INPUT_PLANE: MaybeUninit<BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>> =
                MaybeUninit::uninit();
            // SAFETY: sole writer; initialised before the `&'static` is shared with the input plane,
            // never rewritten (single executor builds it, two planes only read it).
            let input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>> = unsafe {
                init_static(core::ptr::addr_of_mut!(INPUT_PLANE), BlockingMutex::new(RefCell::new(InputPlane::new())))
            };

            let hp = EXECUTOR_HP.start(interrupt::SWI01);
            // COM starts only **now**, after the COM-held-`Lo` init-black frame above. Default: the M33
            // `com_task` on the high-priority executor (it must keep toggling during the blocking
            // whole-frame push). `com-hw`: the zero-CPU TIMER+DPPI+GPIOTE driver — no task, no core wakes
            // — so the M33 can WFI between events; `HwCom` is held in `MapDisplay` for the program's life.
            #[cfg(not(feature = "com-hw"))]
            hp.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
            #[cfg(feature = "com-hw")]
            let com_hw = {
                let c = HwCom::start(p.TIMER21, p.PPI20_CH0, vcom, vb, va);
                info!("FLPR LS021: COM on hardware TIMER21+DPPI+GPIOTE20 (zero-CPU); M33 can WFI between events");
                c
            };
            hp.spawn(defmt::unwrap!(input_task(buttons, input_plane, GESTURES.sender())));
            info!("FLPR LS021: gesture/bulge plane on SWI01 @ P3; map plane: thread mode (event-driven, #219)");
            MapDisplay {
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
            }
        };

        // microSD in **native 4-bit SD mode over Nordic's sEMMC soft peripheral** (epic #1158) —
        // the same FLPR the panel runs on, time-multiplexed by `flpr_mux`. No SPI instance, no
        // chip-select, no bus config: the six card pads (P2.00–05) belong to the soft peripheral,
        // which owns their direction, drive (E0/E1 + `GPIOHSPADCTRL.BIAS = 2`) and `CTRLSEL` per
        // mode — see `semmc::configure_storage_pads` / `configure_display_pads`.
        //
        // Ordering is load-bearing: the display comes up **first** (above), so bring-up flips the
        // FLPR from display to storage exactly once and every later switch is the measured
        // 29 µs / 138 µs pair. `bring_up_storage` holds the mode across the whole `Semmc::start`
        // await — nothing else that touches the coprocessor has been spawned yet (the BLE and USB
        // planes come hundreds of lines below), which is what makes that contract structural.
        //
        // Arm the completion vector first: P1, the default peripheral lane, beside the display's
        // EGU20 frame-ack (the ISR is one store + a latched-event clear, so priority only has to
        // stay under the P0 GRTC driver). The per-boot *gates* are `Semmc::boot_firmware`'s.
        // SAFETY: enabling an NVIC line whose handler is bound in `board::VPR00`.
        unsafe {
            interrupt::VPR00.set_priority(Priority::P1);
            interrupt::VPR00.enable();
        }
        // A missing/bad card is fatal — the map streams from it. The display is already up (brought
        // up above, before the card), so instead of failing silently we put an **undismissable**
        // fault screen on glass, then heartbeat-idle. (SharedStore keeps an Option seam for a future
        // card-less variant where BLE config/bond/diagnostics still serve.)
        //
        // The *reason* travels with the failure (#1163 review, P3): a reader that never booted and a
        // card that is merely too small each get their own screen, because "NO SD CARD" would send
        // the rider to the wrong fix. `sd::bring_up_card` has already logged the specific class.
        if let Err(fault) = sd::bring_up_card() {
            defmt::error!(
                "SD: storage unusable — showing the {=str} fault screen, then heartbeat idle",
                fault.copy().0
            );
            show_boot_fault(&mut display, fault).await;
            idle_blink(&mut led).await
        }

        // ── Which stack owns this card (FS7.5-c1, #1420) ────────────────────────────────────────
        //
        // The flat store owns the raw card from LBA 0 and FAT is a filesystem *on* a card, so a card
        // is one or the other and never both. `FLAT_Store_Format.md` §5.6 step 1 is the test, and
        // `FlatStore::mount` **is** that test — it never fails, it classifies — so the board runs no
        // second superblock reader of its own that could disagree with the store's rule. A card
        // without a flat superblock is rejected: the shipping image no longer mounts FAT.
        //
        // `mount_at_boot` is `#[inline(never)]` into a `.bss` slot: a ~10.5 KB store must not become
        // a permanent slot in this task's poll frame, and `mount`'s own ~14 KB constructor frame must
        // stay a transient sibling step of the boot chain (#677/#1084/#1108, and the boot-chain root
        // `resource_guard.py board` measures). Well inside the #729 watchdog invariant below: a mount
        // is bounded by the catalog it reads — a few hundred milliseconds at the largest one §9 allows.
        let flat_started = embassy_time::Instant::now();
        let flat = flat_store::mount_at_boot();
        let flat_catalog = flat_store::report(flat, flat_started.elapsed().as_micros());

        // ── Flat map source (FS7.5-c2, #1420) ──────────────────────────────────────────────────
        //
        // The `&'static StoreSource` is a plain
        // `ByteSource` a render calls straight through; the storage task spawned below owns *writes*
        // only, and a render's `read_at` never touches its channel (the #1256 ruling of 2026-08-18,
        // and `flat_store::storage_task`'s docs for what the store's per-card-command borrow
        // granularity buys and what it does not).
        //
        // **The write half's owner, and since c3a the protocol-v4 engine's — spawned on *every*
        // card, not only a flat one.** `arm` is what makes `writer()` hand out senders at all, so
        // the two are one act; what changed in c3a is that the act is unconditional.
        //
        // The reason is `FLAT_Store_Protocol.md` §3.9, not convenience. A FAT card is a card that is
        // **not a flat store** — §5.6 step 1, `Mode::Unformatted` — and the protocol already has the
        // honest answer for one: ordinary opcodes return `readOnly` with detail `unformatted 3`,
        // including the reads, because there is nothing to read; explicit FORMAT is the one
        // recovery exception. Mounting the engine against the
        // store that is always mounted therefore makes a FAT card answer the truth about itself,
        // where a build that only armed on flat cards would have the BLE adapter holding a record it
        // had no engine to answer with. One code path, two cards, and neither of them a special
        // case; see the c3a section of the board README for what a phone sees on each.
        _spawner.spawn(defmt::unwrap!(flat_store::storage_task(flat, flat_store::arm())));

        let flat_map: &'static dyn obc_formats::io::ByteSource = match flat_store::classify(flat) {
            flat_store::Card::Flat => match flat_store::open_map(flat) {
                Some(source) => source,
                None => {
                    let fault = flat_store::boot_fault_for(flat_catalog);
                    defmt::error!(
                        "flat: no map to render from — showing the {=str} fault screen, then heartbeat idle",
                        fault.copy().0
                    );
                    let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                    show_boot_fault(&mut display, fault).await;
                    idle_blink(&mut led).await
                }
            },
            flat_store::Card::FlatBroken(fault) => {
                let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                show_boot_fault(&mut display, fault).await;
                idle_blink(&mut led).await
            }
            flat_store::Card::NotFlat => {
                defmt::error!("flat: card is not formatted as a flat store — FAT compatibility is retired");
                let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                show_boot_fault(&mut display, obc_app::BootFault::StorageFault).await;
                idle_blink(&mut led).await
            }
        };

        // Place the streamed-map geometry cache in `.bss`, built in place (an all-zero
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary).
        // SAFETY: sole owner of MAP_CACHE; single executor → no aliasing.
        let map_cache: &MapCache = unsafe { init_static(core::ptr::addr_of_mut!(MAP_CACHE), MapCache::new()) };

        // Parse the OBCM **header + style table + LOD pyramid once at boot** into the resident
        // [`MAP_TABLES`]. These tables are immutable for the session, so the loop's per-frame readers
        // *borrow* them instead of re-parsing — no per-frame style-table SD read and no ~4 KB parse stack
        // spike (a 1536-byte style scratch + the ~2.3 KB style array) on the deep render path, which is
        // what kept that path overrunning the 256 KB part's stack. The transient parse cost is paid
        // **here**, at boot, where the call stack is shallow; a missing or structurally-bad map shows a
        // fault screen, then idles. The idle camera centre is the parsed bbox.
        // SAFETY: sole owner of MAP_TABLES; single executor → no aliasing; written exactly once here.
        let map_tables: &MapTables = unsafe {
            let parsed = MapTables::parse(flat_map);
            let slot = core::ptr::addr_of_mut!(MAP_TABLES) as *mut MapTables;
            match parsed {
                Ok(t) => {
                    slot.write(t);
                    &*slot
                }
                Err(e) => {
                    defmt::error!(
                        "map: not valid OBCM: {} — showing MAP UNREADABLE with USB recovery",
                        defmt::Debug2Format(&e)
                    );
                    // USB map recovery speaks protocol v4 against the flat store, so a replacement
                    // can still be uploaded when the current map will not parse.
                    let _recovery_stage = spawn_map_recovery_usb(_spawner, p.USBHS, p.RRAMC).await;
                    show_boot_fault(&mut display, obc_app::BootFault::BadMap).await;
                    idle_blink(&mut led).await
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

        // The map's **embedded terrain** (EL7 + FS7.5 §1.3): mounted right behind the tables, which
        // is where it has to be now — the region is a header field, so there is nothing to mount
        // until the header is parsed. Folded into the `.bss` `TERRAIN` slot via `mount_terrain`,
        // whose `#[inline(never)]` keeps the ~2.1 KB parse temporary out of this task's poll frame.
        //
        // The flat store hands it the same `'static` source over the map.
        #[cfg(has_nav)]
        let terrain = mount_terrain(flat_map, map_tables);

        // Boot to **Home**: the user drives Home → Route menu → Map with the buttons. The opt-in
        // `sd-bench` image instead starts directly on the live Map so an unattended RTT run exercises
        // the real map-reader path as SynthLocation moves. Built **in place**
        // in `.bss` (`init_idle`/`init_map` write each field where it sits), never on the stack. The Route menu is
        // filled from the card's catalog scanned above; selecting an entry opens the Map at that route's
        // start and streams its geometry into the render + the map-matcher.
        // SAFETY: sole owner of APP; `init_idle` fully initialises it before the `&mut` below reads it.
        let app: &mut App = unsafe {
            let slot = core::ptr::addr_of_mut!(APP) as *mut App;
            #[cfg(not(feature = "sd-bench"))]
            App::init_idle(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            #[cfg(feature = "sd-bench")]
            App::init_map(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            &mut *slot
        };
        // FS9 still owns the legacy update implementation. Ride objects are flat-store-native;
        // this optional FAT seam is retained only for the staged updater until that later slice.
        let storage: Option<sd::Storage> = None;
        {
            // Routes and trips are flat-store objects. One bounded snapshot seeds the menu, newest
            // first so a fresh upload remains visible on a card with more than the UI cap.
            // A transient media read here needs no re-arm of its own since #1397 S6b: the ride
            // loop reports the store's live `sequence()` as `ExternalFacts::note_store_revision`,
            // and the very first pass sees that level move from "no store" to a revision — which is
            // one `CatalogIntent::Refresh`, i.e. exactly one `CatalogEffect::ReadCatalog`, on the
            // first frame. The boot snapshot below is what the menu shows until that read lands.
            let _ = flat_store::load_routes(flat, app);
            let _ = flat_store::load_trips(flat, app);
            let _ = flat_store::load_rides(flat, app);
            // Mirror the map's §8.6 routing-profile names into the app for the Bike-type settings
            // screen + created-route overview label (N5). Map metadata, so it runs on the `ble` image
            // too — the setting renders there but is inert (no router in that build).
            app.set_nav_profiles(map_tables.nav_profiles());
            // Device-info for the System settings screen (T8 item 6): the running firmware version
            // (the same `git describe` tag the GATT device-info + DFU confirm use) and the loaded
            // map's name + OBCM version. The card-free scan runs later, on the screen's entry request.
            {
                use core::fmt::Write as _;
                let mut fw: heapless::String<32> = heapless::String::new();
                let _ = write!(fw, "{}+{}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));
                app.set_fw_version(&fw);
                app.set_map_info(flat_store::map_name(), map_tables.version);
            }
        }

        // Place the decoded-route-geometry cache in `.bss`, built in place (a zeroed
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary — like `MAP_CACHE`).
        // SAFETY: sole owner of ROUTE_CACHE; single map plane → no aliasing.
        let route_cache: &RouteCache = unsafe { init_static(core::ptr::addr_of_mut!(ROUTE_CACHE), RouteCache::new()) };

        // The router's **resident** half (epic #116 R4 + EL7): since #1146 P2 the A* table, the
        // graph-tile cache and the planner slot are the scratch arena's nav arm — claimed per search,
        // not owned here — so all that is threaded into the ride loop is the map's terrain (or the
        // null source), which is sampled at fix cadence and therefore never joined the arena. On the
        // `ble` build (`not(has_nav)`, see build.rs) it is the unit stand-in and the ride loop
        // answers plan requests with the generic failure tier.
        #[cfg(has_nav)]
        let nav = ride::NavResident {
            // The terrain the emit phase fills from, or the null source — both `.bss`, never a frame.
            // SAFETY: `NULL_ELEV` is a ZST written nowhere else; `terrain` is the sole reference to
            // the `TERRAIN` slot, moved here.
            elev: match terrain {
                Some(t) => t,
                None => unsafe { &mut *core::ptr::addr_of_mut!(NULL_ELEV) },
            },
        };
        #[cfg(not(has_nav))]
        let nav = ride::NavResident;

        // The persistent settings store: takes the `RRAMC` peripheral, reads/writes the blob in the
        // carved RRAM page. Built here (where `p` is live) and moved into the ride loop, which seeds the
        // app at boot and saves on a settings edit. Every boot also bumps the persisted boot counter,
        // the diagnostics blob's one durable fact.
        let mut settings_store = settings::RramSettingsStore::new(p.RRAMC);
        let boot_count = settings_store.bump_boot_count(reset_reas);
        defmt::info!("boot #{=u32}", boot_count);

        // Snapshot the running image's version off the DFU boot-state page (#996) before any plane
        // that publishes it exists: the BLE DIS strings are seeded inside `ble::run`, the USB plane
        // answers §5.2.1's EP0 device-info request from the same source, and both are spawned
        // below. It is a
        // one-shot read of a page this store already owns — see `dfu::seed_firmware_revision`.
        dfu::seed_firmware_revision(&mut settings_store);

        // INVARIANT (#729): because EVERY trial boot now enters with the dog already counting,
        // everything between app entry and this line must complete well inside one WDT period
        // (24 s) on a trial boot — or the dog resets a perfectly healthy trial image and the
        // bootloader rolls it back. Today that's seconds of headroom, and nothing blocking sits
        // upstream (a missing/slow card does NOT block boot — the build idles without one).
        // Keep it that way: never move a blocking or open-ended retry loop (SD mount, sensor
        // bring-up) above this point.
        let wdt_handle = match board::watchdog!(p.WDT0, ride::WDT_TIMEOUT_TICKS) {
            Ok((_wdt, [handle])) => Some(handle),
            Err(_) => {
                defmt::warn!("WDT: already running with a foreign config — cannot feed it; expect one reset");
                None
            }
        };

        // The shared store: the mounted card + the RRAM settings move behind one async mutex, so the
        // ride loop and the BLE object plane can each lock it per pass. Built in place in `.bss`
        // (warm-reset-safe via `init_static`, like the caches above). `storage`/`settings_store` are
        // consumed here; both planes reach them through the guard. `storage` was unwrapped to a
        // `Storage` at boot (the build idles without a card), so it goes in as `Some` — the `Option`
        // is the seam a future card-less variant would use.
        let shared_store: &'static SharedStoreMutex = unsafe {
            init_static(
                core::ptr::addr_of_mut!(SHARED_STORE_SLOT),
                SharedStoreMutex::new(SharedStore { storage, settings: settings_store }),
            )
        };

        // (The companion-link object store is built after the store-epoch mint below — the catalog
        // scan must see the settled id-era, exactly as it did when `ble::run` built it.)

        // --- Store-epoch nonce (protocol v2, #632 item 5 / #767; card-resident #776): mint & persist
        // the store's id-era nonce. The epoch now lives on the **card** (`EPOCH.OBE`, so the card
        // carries its own era name — a swap transplants the store identity), while the id-marks floor
        // stays in RRAM. Runs **unconditionally, in every build flavor** — the era invariant ("any
        // boot that could allocate ids under a lost floor declares a new era") must hold across
        // mixed-flavor bench flashing: if only ble builds minted, a non-ble build could boot on a
        // torn/absent id-marks line, rewrite it valid over a ride, and a later ble flash would see a
        // valid card epoch + valid floor and never mint — permanent undetected aliasing. Placed right
        // after the shared store is built (the earliest point the card *and* the RRAM lines are
        // readable — the card was mounted at boot and moved into the guard as `Some`) and before
        // anything can allocate an object id: the ride loop (`run_app`, below) and the ble build's
        // ObjectStore (inside `ble::run`, spawned below) both start after this block in every flavor.
        // On a clause-2 mint we seed "no floor", which the later card scan resolves via the existing
        // `max(scan_max + 1, floor)` allocation — see `store_epoch_mint`. The read epoch is the value
        // served over the pre-pairing `protocolVersion` read; it is threaded into `ble::run` below so
        // that path never re-reads the card. A no-card boot never reaches here (it diverged into the
        // fault/idle path above), so `storage` is `Some`; the `None` arm keeps the future card-less
        // seam honest (no store ⇒ no epoch ⇒ the version read degrades to version-only). The TRNG word
        // comes from a throwaway CRACEN reborrow: `Cracen` construction is side-effect-free, each op
        // self-enables/-disables the RNG, and `Drop` is a no-op — so on ble builds the peripheral is
        // pristine when it then moves into the LL (whose crypto RNG it becomes). `cracen_p`
        // partial-moves `p.CRACEN` out so the reborrow has a `&mut`; non-ble builds drop it afterwards.
        let mut cracen_p = p.CRACEN;
        // `_store_epoch` (underscore like `_spawner`): read only by the `ble` spawn below, so non-ble
        // builds bind it without a use. The mint pass's *writes* (card epoch + RRAM marks) run in
        // every flavor regardless — only the served value is ble-specific.
        let _store_epoch: Option<u32> = {
            let mut guard = shared_store.lock().await;
            if guard.storage.is_none() {
                defmt::info!("store-epoch: no mounted store — no epoch to mint or serve");
                None
            } else {
                // Read the card epoch (immutable storage borrow) then the RRAM floor (mutable settings
                // borrow) — sequential, non-overlapping field borrows.
                let card_epoch = guard.storage.as_ref().unwrap().load_card_epoch();
                let marks = guard.settings.load_id_marks();
                // One TRNG word — cheap, and only the mint path consumes it (the pure decision fn
                // ignores it when it keeps the card's epoch).
                let fresh = {
                    let mut cracen = embassy_nrf::cracen::Cracen::new_blocking(cracen_p.reborrow());
                    cracen.blocking_next_u32()
                };
                match obc_app::store_meta::store_epoch_mint(card_epoch, marks, fresh) {
                    Some((new_epoch, new_marks)) => {
                        // ORDERING: card epoch FIRST, id-marks only on its success. The epoch persist
                        // is a FAT write — categorically more tearable than the RRAM line it replaced —
                        // and writing the marks under a failed epoch write would be fatal in the
                        // clause-2 case: the card keeps the OLD valid epoch, the fresh floor reads
                        // valid, and the next boot sees steady state — the era reset goes permanently
                        // undetected (the exact aliasing this mechanism exists to catch). By skipping
                        // the marks write on failure, clause 2 re-fires and the mint retries next boot;
                        // serving `None` (the 2-byte version-only read) keeps this session honest too —
                        // the store has no *proven* era name, and the app's fail-closed ack gate
                        // handles it exactly as designed. Residual double-fault window, named honestly:
                        // if the epoch write fails here but a LATER ride-finish marks write succeeds,
                        // the next boot is steady under the compromised old epoch. Contrived — both are
                        // FAT writes to the same card, so a card that can't persist EPOCH.OBE likely
                        // can't save rides either — but it is the one path this ordering can't close.
                        if guard.storage.as_mut().unwrap().save_card_epoch(new_epoch) {
                            guard.settings.save_id_marks(&new_marks);
                            defmt::info!(
                                "store-epoch: minted id-era nonce {=u32:#010x} to card (+ id-marks re-seeded)",
                                new_epoch
                            );
                            Some(new_epoch)
                        } else {
                            defmt::error!(
                                "store-epoch: card epoch persist FAILED — id-marks left untouched so the \
                                 mint retries next boot; serving no epoch this session (app acks fail-closed)"
                            );
                            None
                        }
                    }
                    None => {
                        // Kept: mint returns `None` only when the card epoch was `Some`, so unwrap is safe.
                        defmt::info!("store-epoch: kept card id-era nonce");
                        card_epoch
                    }
                }
            }
        };

        // --- The one companion-link object store, and the handles every link plane is composed
        // with. Built here rather than inside `ble::run`: with USB as a second transport (#889),
        // two independently-constructed stores would each keep their own catalog, id allocator and
        // upload temp over the *same* SD card. Built **after** the epoch mint above, which is where
        // `ble::run` used to build it — the catalog scan must see the settled id-era.
        //
        // `link::init_store` is `#[inline(never)]`, so its ~13.5 KB construction temporary lives in
        // *that* transient frame (a measured ~27.6 KB prologue, popped immediately) and `main`'s
        // frame pays only the reference — the #677 rule, unchanged. ---
        #[cfg(feature = "ble")]
        let link_stores = {
            let objects = {
                let mut guard = shared_store.lock().await;
                link::init_store(&mut guard)
            };
            link::LinkStores { shared: shared_store, objects, epoch: _store_epoch }
        };

        #[cfg(feature = "ble")]
        {
            let (mpsl_p, sdc_p) = board::radio_hardware!(p);
            // CRACEN goes to the LL's crypto RNG — already partial-moved out of `p` by the
            // store-epoch mint pass above (which only reborrowed it; see its comment). `store_epoch`
            // is the mint pass's outcome (the value the pre-pairing `protocolVersion` read serves;
            // `None` ⇒ version-only), threaded through so `ble::run` never re-reads the card.
            _spawner.spawn(defmt::unwrap!(spawn_ble_stack(
                _spawner,
                mpsl_p,
                sdc_p,
                cracen_p,
                link_stores,
                SENSOR_HUB.injector()
            )));
        }

        // --- The USB device plane (#889), in **every** build: the LM20's USBHS on its dedicated
        // D+/D−/VBUS pins (zero GPIO cost — nothing above needs re-planning), speaking the same
        // object model as the radio over a vendor bulk interface. Spawned through its own trampoline
        // for the same #677 reason as the BLE stack: constructing the task's future in `main` would
        // put its whole state machine in `main`'s poll frame. ---
        _spawner.spawn(defmt::unwrap!(spawn_usb_stack(_spawner, p.USBHS)));

        // Hand the built display + the resident set to the shared, backend-agnostic ride loop. The
        // `display` (one of the two `MapDisplay` definitions) is the only per-backend value crossing this
        // seam; the loop drives present through it with no further `#[cfg]`. `cam_center` is threaded
        // only on the `synth` build (the host feed + the real GPS stream absolute positions, so they need
        // no synthetic-loop centre).
        #[cfg(any(feature = "debug-uart", not(feature = "synth")))]
        let app_fut = ride::run_app(
            display,
            app,
            shared_store,
            map_tables,
            map_cache,
            flat_map,
            flat,
            route_cache,
            nav,
            &mut led,
            wdt_handle,
            // The hub's consumer (every non-`synth` build) + control (real-sensor only) handles —
            // ownership visible right here at composition (#808), not reached through a global.
            SENSOR_HUB.consumer(),
            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
            SENSOR_HUB.control(),
        );
        #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
        let app_fut = ride::run_app(
            display,
            app,
            shared_store,
            map_tables,
            map_cache,
            flat_map,
            flat,
            route_cache,
            nav,
            &mut led,
            wdt_handle,
            (cam_lon, cam_lat),
        );
        // The ride loop is `main`'s tail future in every build. On `ble` builds the BLE stack runs
        // beside it as the task spawned above — both on the one thread-mode executor, both driving
        // the shared SD + settings store (the ride loop locks it per frame across the render; the
        // object plane per chunk between frames).
        app_fut.await;
    }
}
