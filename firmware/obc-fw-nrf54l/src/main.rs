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
//! caps; the budget assert + the ~53 KB residual stack (deep-ride peak ~36 KB) are the margins.
//! `--no-default-features` stays mandatory — it swaps the critical-section impl to MPSL's.
//!
//! Clock: the M33 application core runs at 128 MHz; embassy-time is driven by the **GRTC**
//! (Global RTC) via the `time-driver-grtc` feature — the nRF54L has no legacy RTC time-driver.
//! `ble` builds additionally source HFCLK from the **crystal** (an MPSL hard requirement) and
//! leave LFCLK on the MPSL-calibrated internal RC (see `ble.rs` — the unprogrammed XO INTCAPs).
//!
//! ============================ Peripheral / pin plan ============================
//! Pin names are the embassy-nrf `P{port}_{pin}` form (e.g. `P2_09` = GPIO port 2, pin 9).
//! LED/button/VCOM/SPI assignments are the nRF54L15-DK's, from Zephyr's `nrf54l15dk` DTS and
//! the DK HW user guide pin maps (Tables 3–5). The three GPIO ports have different reach: P2 =
//! MCU domain (fast, ≤64 MHz, the SERIAL00 home), P1 = PERI domain (≤8 MHz), P0 = LP domain.
//!
//! ## On-board LEDs (active-HIGH) — Zephyr `led0..3`
//!   LED0 P2_09 | LED1 P1_10 | LED2 P2_07 | LED3 P1_14
//! LED0 (P2_09) blinks once per drawn frame as a liveness heartbeat.
//!
//! ## Push-buttons (active-LOW, internal pull-up) — Zephyr `sw0..3`, the UI input
//!   BTN0 P1_13 UP | BTN1 P1_09 DOWN | BTN2 P1_08 BACK | BTN3 P0_04 SELECT
//! Map to obc-platform's board-agnostic `ButtonInput` debouncer → the shared gesture recogniser.
//! Roles: BTN0/1 → Up/Down Step∓1, BTN3 → Select press/hold, BTN2 → Back/back-hold
//! (`ButtonInput::new` order is up, down, select, back). Read as plain **polled** `gpio::Input`
//! (the debouncer samples levels each loop — no GPIOTE async wait needed). They stay free because
//! the display lives on P2 (below).
//!
//! ## Display — LS021B7DD02 via the FLPR coprocessor
//!   The reflective memory-LCD is driven by the nRF's **FLPR** (VPR RISC-V) coprocessor, which
//!   scans the resident RGB222 framebuffer straight out of shared SRAM and packs each line to the
//!   panel's parallel gate/source wire itself — no SPIM, no full-frame second buffer (issue #347).
//!   The M33 holds the panel's gate + source + COM lines as plain GPIO for the program's life; the
//!   authoritative pin map (GSP/GCK/GEN/INTB on P1, BSP on P1.14, BCK + the 6 data lines on P2, COM
//!   on P2 or — with `com-hw` — GPIOTE-capable P1) lives at the FLPR bring-up block below. The DK's
//!   on-board QSPI-flash pins (P2.00–P2.05) carry the source bus; we never use that flash (maps
//!   live on SD), so the **Board Configurator** electronically disconnects it ("external memory →
//!   GPIO on the P2 header") — no soldering on current board revisions. The panel's logic wants
//!   3–5 V, so the DK I/O rail is raised from its 1.8 V default to **3.3 V** (VDDM, also in the
//!   Board Configurator — HW guide §2.2.1). The display path presents through the board-agnostic
//!   display contracts (`obc_display::display_contracts`), so the rendering stack never couples to
//!   the panel.
//!   (SERIAL00 / SPIM00 — the only 32 MHz instance — is now unused; the FLPR needs no SPI bus.)
//!
//! ## microSD SPIM — map/route/track storage
//!   Instance **SERIAL22 / SPIM22** — a standard-speed instance (SD doesn't need 32 MHz),
//!   *separate* from the display bus, on its own software CS. DK expansion-header SPI pins:
//!     SCK P1_11 | MISO P1_07 | MOSI P1_06 | CS P0_00   (P1.12 carries the FLPR's GEN, see below)
//!   CS is a free GPIO held LOW for the whole session (the held-low-CS workaround embedded-sdmmc's
//!   per-byte framing needs over embassy SPI — see `sd::NoCs`); the bus inits ≤400 kHz then
//!   re-clocks to 8 MHz (`sd::init`). embassy-nrf's `Spim` exposes **no** internal MISO pull-up,
//!   so the card's DO line is pulled high by the breakout (or an external 10 kΩ to 3V3). The
//!   EYESPI connector also carries a microSD that *shares the display bus*; we leave that slot
//!   **unpopulated** and use this dedicated SPIM instead. P1_06/P1_07 are the VCOM's RTS/CTS pins
//!   (below) — we drive them as SD MOSI/MISO instead, which is only safe because the VCOM runs
//!   **without** hardware flow control (HWFC OFF in the Board Configurator — see the crate README);
//!   with HWFC on, the J-Link gates host→device bytes on the device's RTS (P1_06), so this firmware
//!   never asserts it and host→device RX would be dead.
//!
//! ## VCOM UARTE — debug-sensor / telemetry stream
//!   Instance **SERIAL20 / UARTE20**, the DK's `chosen` console wired to the onboard J-Link's
//!   USB-CDC VCOM: TX P1_04 | RX P1_05. Brought up **2-wire (no RTS/CTS)**, so the DK's VCOM
//!   **hardware flow control must be disabled** (Board Configurator — see the crate README);
//!   otherwise device→host telemetry still flows but host→device (the fake-sensor feed + input
//!   injection) is silently gated off on the un-driven RTS. The nRF54L15 has **no USB peripheral**,
//!   so the fake GPS/baro/compass feed and ride telemetry ride this UART; defmt logs ride RTT on
//!   the same cable. obc-platform's debug-source protocol is transport-agnostic, so it runs over
//!   the UART unchanged.
//!
//! ## Spare interrupt for the high-priority InterruptExecutor
//!   Input + the overlay run on a high-priority `InterruptExecutor` that preempts the map render,
//!   pended from a dedicated **software-interrupt vector**: **SWI01** (**SWI00 belongs to MPSL**
//!   on `ble` builds — see the ladder below — so the executor sits on SWI01 in every build). Runs
//!   at **P3** — above thread mode (so it preempts the map render) but below the P0 GRTC
//!   time-driver (so `Timer`s still wake mid-render).
//!
//! ## Interrupt priority ladder (reconciled with the BLE stack)
//!   - **P0 (highest)**: the GRTC time driver (`GRTC_0`) — and, on `ble` builds, MPSL's
//!     timing-critical lane (`RADIO_0`, `TIMER10`, `GRTC_3`; MPSL raises these itself).
//!   - **P1 (embassy default)**: on `ble` builds MPSL's low-priority scheduling (**`SWI00`** — why
//!     the input executor sits on SWI01) + `CLOCK_POWER`; plus the default-priority peripheral
//!     ISRs every build has (display/SD SPIM, VCOM UARTE, RRAMC, and the EGU20 frame-ack — the
//!     FLPR's per-frame doorbell the async present awaits, #347).
//!   - **P3**: the SWI01 `InterruptExecutor` (input/bulge plane + the DK COM task) and the
//!     SERIAL22 sensor-bus ISR.
//!   - **Thread mode**: the map plane (`run_app`), the BLE stack (`ble::run`, `ble` builds), and the sensor task.
//!
//!   MPSL's P0 lane preempts everything, including the P3 planes — safe by construction for the
//!   panel: the FLPR scans the framebuffer autonomously (#347), so M33 preemption can no longer
//!   stretch a frame push at all (only the ack's delivery, which is untimed).
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

mod sd;
// LS021 FLPR backend — the display: `main.rs` runs the real app on the reflective LS021
// panel via the FLPR (the VPR coprocessor). The FLPR presenter backend + launch live in
// `ls021_flpr`; `com::com_task` free-runs the COM lines (the FLPR drives frames; only the COM
// electrode square wave stays on the M33).
mod com;
// Zero-CPU hardware COM: drive the COM square wave from a TIMER→DPPI→GPIOTE toggle chain instead of
// the M33 `com_task`, so the panel's anti-DC-bias COM keeps alternating with no core wakes and the M33
// can WFI between events. Opt-in (`com-hw`) + production-board-only — the DK wires COM on P2, which has
// no GPIOTE — so the default DK build keeps `com::com_task`. See `com_hw.rs`.
#[cfg(feature = "com-hw")]
mod com_hw;
// (`com-hw`'s COM pins moved to P1.06/07/13 on the LM20 — GPIOTE20-capable and clash-free with
// the VCOM UART on P1.16/17, so `com-hw` + `debug-uart` now compose. The whole harness rides
// two DK headers: port P1 for gates+sensors+COM, port P2 for display data + SD.)
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
// Real GPS (SAM-M10Q) + altimeter (BMP581) on a shared TWIM30 I²C bus — the concrete transport + the
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
// cross-transport one-transfer gate, the identity blobs, and the one shared `ObjectStore`. Both the
// radio and the USB plane call into it, which is what keeps "USB is a second transport, not a second
// protocol" true in the code rather than only in the spec.
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
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

use core::cell::RefCell;
use core::mem::MaybeUninit;

use embassy_nrf::gpio::{Input, Pull};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_nrf::wdt;
// The shared GPS/altimeter I²C bus — real-sensor build only.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use embassy_nrf::twim::{self, Twim};
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
use obc_platform::ButtonInput;
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
use embassy_nrf::buffered_uarte::{self, BufferedUarte, BufferedUarteRx, BufferedUarteTx};
#[cfg(feature = "debug-uart")]
use embassy_nrf::uarte;

// SERIAL00 backs the microSD SPIM (the only 32 MHz instance). Both handlers are always registered
// (harmless when a feature is off — the peripheral is never constructed, so its interrupt never fires).
bind_interrupts!(struct Irqs {
    SERIAL00 => spim::InterruptHandler<peripherals::SERIAL00>;
});

// VCOM UARTE20 RX/TX → the `BufferedUarte`'s interrupt-fed ring buffers.
#[cfg(feature = "debug-uart")]
bind_interrupts!(struct UartIrqs {
    SERIAL20 => buffered_uarte::InterruptHandler<peripherals::SERIAL20>;
});

// TWIM22 (== SERIAL22, the instance the SD freed when it moved to SPIM00) backs the shared
// GPS + altimeter I²C bus on P1 — one header for the whole harness; bound only on the
// real-sensor build. (TWIM30 would force the bus onto P0: the LP-domain instance is P0-only.)
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
bind_interrupts!(struct SensorIrqs {
    SERIAL22 => twim::InterruptHandler<peripherals::SERIAL22>;
});

// ============================ Board memory budget ============================
// The nRF54L15 has 256 KB RAM and no external RAM, so the whole resident working set of a full map
// redraw must fit there. This build-time assert fails the build — rather than overflowing RAM on
// glass — if the shared crates' caps (trimmed by the `nrf-mem` profile, enabled on the obc-app edge in
// Cargo.toml) ever outgrow the budget. It compiles for thumbv8m (usize = 4 B), so every `size_of` here
// is the true on-device size.
//
// The binding moment is a full redraw with everything resident at once:
//   - `App`        embeds the renderer scratch (`obc_render::MCU_RENDERER_BYTES`, 8,352 B nrf-mem)
//                  plus the resident elevation `Profile` (~2.3 KB at PROFILE_COLS=256) and
//                  `Breadcrumb` (~3 KB at SPINE_CAP=256); 34,944 B total. (The #270 cull: these
//                  caps were roughly halved again so the map plane leaves room for the BLE
//                  stack in one image — the LM20 re-decides them generously.)
//   - framebuffer  the single RGB222 frame: 240×320 × 1 B/px = 75 KB — the `FB` static below.
//   - `MapCache`   the streamed-map geometry-chunk cache (2 slots + 8 KB scratch on nrf-mem, 14,444 B).
//   - `RouteCache` the decoded-route-chunk cache (2 slots on nrf-mem, ~6 KB).
//   - `RouteIndex` the active route's resident chunk index — the ride loop holds it across frames in
//                  the map plane's task future to stream geometry without re-walking it (128 chunks on
//                  nrf-mem, ~6 KB). Counted here because on the 256 KB part it materially shares the
//                  budget. Built **in place** in that resident slot (`RouteIndex::read_into`) — the
//                  earlier by-value `RouteIndex::read` build put the ~6.7 KB on the stack at the ride
//                  pass's deepest point, and the post-upload rescan's rebuild overflowed the main
//                  stack the moment `.bss` crept 216 B (STKOF HardFault, 2026-07-12).
// plus `STACK_RESERVE` headroom for the main stack + embassy's executor/task arenas. The stack must
// also absorb a per-redraw `Reader::new` (the OBCM style table → a ~2.4 KB `Reader` value built as a
// stack temporary, plus its own ~4 KB read scratch): the ride loop rebuilds it each frame, so the
// stack reserve carries that spike.
//
// The FLPR carve-out leaves the M33 the SRAM below `FLPR_RAM_BASE` (not the full 256 KB), but the
// production blob is ~820 B so the carve is only ~8 KB; and the FLPR map path packs the framebuffer
// straight to the LS021 wire, so there is no RGB565 band scratch to reserve — the same caps clear
// the budget.

/// Total SRAM the M33 app core sees — what the FLPR carve leaves, taken straight from the generated
/// contract (`build.rs` derives the carved `memory.x` and this constant from the same `FLPR_RAM_BASE`,
/// so the budget can't fork from the linker map).
const NRF_RAM_BYTES: usize = ls021_flpr::M33_RAM_BYTES;
/// Headroom kept free under the resident statics for the main stack + embassy's executor/task arenas
/// (statics grow up from the RAM base, the stack down from the top). This is only the build-time
/// *floor* the assert enforces — the real stack is the residual `RAM − statics` (~37.8 KB on the
/// default build). Pinned above the **measured deep-path peak**: 35,808 / 37,760 B on 2026-07-04
/// (debug-uart FLPR build, post-#351 split; VCOM-harness full ride — fix on Home → route load →
/// ride → finish-to-save), so a change that squeezes the residual below what the deepest path
/// actually reaches fails at compile time (e.g. a `ble` + map build on the 256 KB DK) instead of
/// overflowing the stack on glass.
/// On the combined `ble` build the SDC/host futures and MPSL's ISRs also ride the main stack on top
/// of the deep-render path, so keep the same generous floor.
const STACK_RESERVE: usize = 64 * 1024;
/// The single RGB222 framebuffer: one byte per pixel over the 240×320 frame = 75 KB.
const FB_BYTES: usize = FRAME_W * FRAME_H;

/// The map plane's residents (the table above). Includes the active route's `RouteIndex`, kept
/// resident across frames. Present in every build now (#270 — map + BLE coexist).
const MAP_RESIDENT: usize = core::mem::size_of::<obc_app::App>()
    + core::mem::size_of::<obc_reader::MapCache>()
    + core::mem::size_of::<obc_reader::MapTables>()
    + core::mem::size_of::<obc_route::RouteCache>()
    + core::mem::size_of::<obc_route::RouteIndex>()
    + NAV_RESIDENT;
/// The on-device router's residents (epic #116, R4): the fixed A* scratch + graph-tile cache
/// (~14.3 KB, the `NAV_*` statics below). Zero on the `ble` build — `has_nav` gates the router
/// out of the combined image because these two statics push its stack region ~1.9 KB below the
/// measured deep-render peak on the 256 KB DK (see build.rs); the LM20 deletes the gate.
#[cfg(has_nav)]
const NAV_RESIDENT: usize = core::mem::size_of::<obc_route::NavScratch>()
    + core::mem::size_of::<obc_reader::NavTileCache>()
    + core::mem::size_of::<obc_route::NavPlanner>();
#[cfg(not(has_nav))]
const NAV_RESIDENT: usize = 0;
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
    + BLE_RESIDENT
    + USB_RESIDENT;
const _: () = assert!(
    RESIDENT_BYTES + STACK_RESERVE <= NRF_RAM_BYTES,
    "nRF resident set (framebuffer + RowDiff + map plane [App/MapCache/MapTables/RouteCache/RouteIndex] + BLE stack [MPSL/SDC mem/host arena]) + stack reserve overruns RAM — re-trim the `nrf-mem` caps (#270 culled them so map + BLE share the 256 KB DK; the LM20 relaxes everything)"
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
    const BLE_ENTRIES: [Entry; 10] = [
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
    ];

    #[cfg(not(feature = "ble"))]
    const BLE_ENTRIES: [Entry; 10] = [
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
    ];

    const ENTRIES: usize = 24;

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
        entry("renderer", core::mem::size_of::<obc_render::MapRenderer>()),
        entry("nav_scratch", core::mem::size_of::<obc_route::NavScratch>()),
        entry("nav_tile_cache", core::mem::size_of::<obc_reader::NavTileCache>()),
        entry("nav_planner", core::mem::size_of::<obc_route::NavPlanner>()),
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
        // The USB device plane's named statics (#889). Unconditional since USB stopped being a
        // feature, so it is itemized here for the same reason the BLE arenas are: it is the newest
        // resident block, and a growth in it should be legible in the report rather than only as a
        // few thousand anonymous bytes of `.bss`. This is the *named* half — the driver's own
        // endpoint bookkeeping and the task future are not nameable here and land in the linked
        // `.bss + .data` gate, which is the authority for resident RAM.
        entry("usb_named", usb::RESIDENT_BYTES),
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
/// `ptr::write` into the reserved region): the 34,944 B `App` (including 8,352 B renderer scratch) and
/// 14,444 B cache must never form on the 256 KB part's small stack. [`MapCache::new`](obc_reader::MapCache)
/// is an all-zero `MaybeUninit::zeroed`, so writing it is a `.bss` memset.
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
/// The on-device router's fixed A* table (epic #116, R4): ~10 kB of open-addressed node slots +
/// heap, in `.bss` (`NavScratch::new()` is `const` and all-zero → a memset), **never** a local —
/// `plan_route` is sync and runs at the ride loop's shallow per-pass depth, but a ~10 kB frame
/// there would still eat a third of the deep-render path's headroom (#270/#419). Reset per plan.
/// `has_nav`-gated: the `ble` build ships without the router (see build.rs; the LM20 ungates it).
#[cfg(has_nav)]
static mut NAV_SCRATCH: MaybeUninit<obc_route::NavScratch> = MaybeUninit::uninit();
/// The router's graph-tile cache (~4 kB, 2 whole nav chunks): the per-settle spatial re-fetch
/// reads through it, so its `misses` count **is** the plan's SD-read count (the number the
/// `nav route:` RTT line logs). Same `.bss` placement + `has_nav` gate as [`NAV_SCRATCH`].
#[cfg(has_nav)]
static mut NAV_TILES: MaybeUninit<obc_reader::NavTileCache> = MaybeUninit::uninit();
/// The **resumable planner**'s slot (#499, ~9.5 KB — it owns the OBCR emitter across steps).
/// Unlike the two buffers above it is *not* `init_static`-once: the ride loop `ptr::write`s a
/// fresh planner per create-route request (no `Drop`, so overwriting is sound) and only reads it
/// while its `NavRun` bookkeeping says a plan is active. Zero-sum note: this ~9.5 KB used to be
/// the emit frame's stack local — moving it here trades the plan path's stack high-water for
/// `.bss`, byte for byte.
#[cfg(has_nav)]
static mut NAV_PLANNER: MaybeUninit<obc_route::NavPlanner> = MaybeUninit::uninit();

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
pub(crate) type SharedStoreMutex = Mutex<NoopRawMutex, SharedStore>;

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
async fn spawn_usb_stack(
    spawner: Spawner,
    usb_p: embassy_nrf::Peri<'static, embassy_nrf::peripherals::USBHS>,
    stores: link::LinkStores,
) {
    spawner.spawn(defmt::unwrap!(usb::run(usb_p, stores)));
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
/// footprint is exactly the scattered statics it replaces. Absent on the pure `synth` build (which
/// drives `SynthLocation` and reads no hub stream).
#[cfg(not(all(not(feature = "debug-uart"), feature = "synth")))]
static SENSOR_HUB: obc_platform::SensorHub = obc_platform::SensorHub::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Run the M33 at its full **128 MHz** — embassy-nrf's `Config::default()` boots it at only
    // 64 MHz (`ClockSpeed::CK64`), which halves the M33's map render (the CPU-bound `render_map` +
    // the RGB222 quantise into the framebuffer). The FLPR then scans that framebuffer itself, so the
    // render is the M33's biggest per-frame cost — this is the single biggest frame-time lever.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        // BLE: the HF **crystal** is an MPSL hard requirement (radio timing); LFCLK stays the internal
        // RC, MPSL-calibrated — NOT the 32 k crystal, whose internal load caps nothing programs on the
        // nRF54L yet (off-frequency LFXO → HCI 0x3E on every connect). See `ble.rs`.
        #[cfg(feature = "ble")]
        {
            config.hfclk_source = embassy_nrf::config::HfclkSource::ExternalXtal;
            config.lfclk_source = embassy_nrf::config::LfclkSource::InternalRC;
        }
        embassy_nrf::init(config)
    };

    // Arm the hardware stack limit first (#677 — overflow = an immediate, precise fault, never
    // silent corruption of the statics below the stack), then paint the stack (still shallow) so
    // the ride loop's high-water guard can read the peak.
    stackmeter::arm_limit();
    stackmeter::paint();

    // The boot banner: which build is running, as `pkg-version+fw_git` (the DIS Firmware Revision
    // string, A4). This is the line the DFU on-glass gate reads to prove the device came back as
    // the staged version after an install (epic #615 S4).
    info!("obc-fw-nrf54l {=str}+{=str}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));

    // Why did this boot happen? (#349) `RESETREAS` @ 0x5010_E600 (the secure RESET block; raw MMIO
    // — the same precedent as the VPR00/EGU20 registers in `ls021_flpr`). A **watchdog** reset
    // (dog0 = our WDT31/`WDT0` instance) is logged distinctly — it means a plane wedged and the
    // dog fired last session. Write-1-to-clear, cleared here so the *next* boot reads only its own
    // cause; the raw mask is also annotated onto the RRAM boot-counter line below.
    let reset_reas = {
        const RESETREAS: *mut u32 = 0x5010_E600 as *mut u32;
        let v = unsafe { RESETREAS.read_volatile() };
        unsafe { RESETREAS.write_volatile(v) }; // W1C
        if v & 0x6 != 0 {
            // bits 1..2 = the two watchdogs. On-glass: the `WDT0` instance this build feeds
            // (= the WDT31 block) reports as **bit 2** — don't trust the PAC's dog0/dog1 naming.
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
    // `SynthLocation` square loop), map-match + log the track, and write the `RD{id}.ORD` ride
    // object to `/tracks` on Finish (GPX export happens phone-side after sync).
    {
        // --- VCOM debug-sensor stream, behind `debug-uart`. Bring it up first so the J-Link VCOM is
        // live while the SD card + panel come up; the parsed fixes land in obc-platform's signals, ready
        // for the app's sensor poll in the loop below. The nRF54L15 has no USB peripheral, so the fake
        // GPS/baro/compass feed and ride telemetry ride UARTE20 on the DK's onboard J-Link VCOM (TX
        // P1_04 / RX P1_05); defmt logs share the same cable over RTT. The RX ring is interrupt-fed
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
            let uart = BufferedUarte::new(
                p.SERIAL20,
                p.P1_17, // RXD: host → device (fixes / input injection)
                p.P1_16, // TXD: device → host (telemetry)
                UartIrqs,
                uarte::Config::default(), // 8N1 @ 115200 — matches `obc-usb-host`'s default baud
                rx_buf,
                tx_buf,
            );
            let (rx, tx) = uart.split();
            _spawner.spawn(defmt::unwrap!(vcom_rx_task(rx, SENSOR_HUB.injector())));
            _spawner.spawn(defmt::unwrap!(vcom_tx_task(tx)));
            info!("VCOM debug sensors up on UARTE20 (J-Link VCOM 'UART1', TX P1_16 / RX P1_17) @ 115200");
        }

        // The four DK push-buttons (active-low, internal pull-up; polled by `ButtonInput`). User
        // mapping: BTN0 UP, BTN1 DOWN, BTN3 SELECT, BTN2 BACK — `new(up, down, select, back)`.
        // Shared by both backends — their pins (P1.13/09/08, P0.04) clash with neither panel's bus.
        let buttons = ButtonInput::new(
            Input::new(p.P1_26, Pull::Up), // BTN0 UP     → Step(-1)
            Input::new(p.P1_09, Pull::Up), // BTN1 DOWN   → Step(+1)
            Input::new(p.P0_05, Pull::Up), // BTN3 SELECT → Select press / hold
            Input::new(p.P1_08, Pull::Up), // BTN2 BACK   → back / back-hold
        );
        // The high-priority plane(s) run at P3 — above thread mode (so they preempt the map render) and
        // below the P0 GRTC time-driver (so their `Timer`s still wake mid-render). Shared vector (SWI01
        // — SWI00 is MPSL's low-prio lane on `ble` builds).
        interrupt::SWI01.set_priority(Priority::P3);

        // --- Real GPS + altimeter on the shared TWIM30 I²C bus. Default build only (neither `synth` nor
        // `debug-uart`). Build the bus + the TX-Ready interrupt line on the free P0 pins and spawn the
        // event-driven sensor task on the thread-mode executor; it probes both chips, configures the M10,
        // and publishes coherent (fix, altitude, temperature) datapoints through its
        // `SensorTaskLink` into `SENSOR_HUB`, which `run_app`'s consumer sources drain. The task is
        // fully async (TWIM is DMA-backed). SERIAL22's ISR runs at P3. ---
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        {
            // EasyDMA can't fetch a write buffer from flash, so byte-literal register writes need a RAM
            // bounce buffer; 32 B covers the widest VALSET frame. Parked in `.bss` + written in place
            // (the warm-reset-safe pattern), then moved into the `Twim`.
            static mut TWIM_TX_BUF: MaybeUninit<[u8; 32]> = MaybeUninit::uninit();
            // SAFETY: written once here, then owned solely by the `Twim` for the program's life.
            let twim_tx = unsafe { init_static(core::ptr::addr_of_mut!(TWIM_TX_BUF), [0u8; 32]) };
            let mut twim_cfg = twim::Config::default();
            twim_cfg.frequency = twim::Frequency::K400; // fast-mode; both chips' DDC/I²C support it
            twim_cfg.sda_pullup = true; // belt-and-braces over the Qwiic board's external pull-ups
            twim_cfg.scl_pullup = true;
            // SDA P1.04 sits beside the clock-capable SCL P1.03 (the datasheet's data-near-clock rule).
            let twim = Twim::new(p.SERIAL22, SensorIrqs, p.P1_04, p.P1_03, twim_cfg, twim_tx);
            interrupt::SERIAL22.set_priority(Priority::P3);
            // TX-Ready (DDC data-ready) on the lone spare GPIO. Active-high, so pull down: a floating
            // / unconfigured line then reads low and the task's poll fallback drives fixes instead.
            let txready = Input::new(p.P1_05, Pull::Down);
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
        // each is broken out on your DK and remap all three together if not (the source bus, BCK, and
        // COM stay on P2).
        let mut display = {
            // Gate + frame lines: one contiguous P1.10–14 run (with BSP below) so the gate harness
            // is a single uninterrupted cable on the DK's port-1 header. (P1.01/02 stay NFC.)
            let gate_bus = [
                Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // GSP
                Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GCK
                Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
                Output::new(p.P1_13, Level::Low, OutputDrive::Standard), // INTB
            ];
            // Source bus: BSP on P1.14 (the lone P1 source line), BCK + the 6 data lines on P2.
            let src_bus = [
                Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
                Output::new(p.P2_07, Level::Low, OutputDrive::Standard), // BCK (P2.06 is the SD's SPIM00 SCK clock pin)
                Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0 (odd)
                Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1 (even)
                Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0
                Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1
                Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0
                Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1
            ];
            // COM electrode lines (56–77 nF load each → high-drive), boot `Lo` and held `Lo` through the
            // init-black frame below, then started. Default (DK): three plain `Output`s the M33
            // `com_task` toggles at 60 Hz — COM is on P2, which has **no GPIOTE**, so it must be
            // M33-driven (VCOM=P2.07, VB=P2.08 in phase, VA=P2.10 inverse). With `com-hw` (production
            // board): the COM lines are GPIOTE **toggle** channels a TIMER+DPPI free-runs with zero CPU
            // (so the M33 can WFI between events) — so they move off P2 onto GPIOTE-capable P1 pins (all
            // on GPIOTE20 → one DPPI channel toggles them in lockstep). The pins here are **placeholders**
            // (P1.04/05/15) to be matched to the production board's COM routing; the freed P2.07/08/10
            // then go unused. `HwCom::start` establishes VA's inverse phase before enabling the toggle.
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

        // microSD on **SERIAL00 / SPIM00** — the LM20's only high-speed (PLL-clocked, 32 MHz)
        // instance, on the fast P2 pads: SCK P2.06 (a dedicated clock pin — SPIM00's SCK mux
        // reaches only P2.01/P2.06, and P2.01 carries display data), MOSI/SDO P2.08, MISO/SDI
        // P2.09. Init ≤400 kHz (SD spec); `sd::init` re-clocks to `sd::SD_FAST_HZ` once the card
        // answers. CS idles HIGH, then `init` holds it LOW for the session (the per-byte-CS
        // workaround — see `sd::NoCs`). `orc = 0xFF` so any over-read clocks the SD idle byte.
        let mut sd_cfg = spim::Config::default();
        sd_cfg.frequency = sd::SD_INIT_HZ;
        sd_cfg.orc = 0xFF;
        let sd_spi = spim::Spim::new(p.SERIAL00, Irqs, p.P2_06, p.P2_09, p.P2_08, sd_cfg);
        // 32 MHz on the fast pads needs **extra-high drive (E0/E1)** on SCK/MOSI plus the highest
        // HS-pad slew (datasheet §8.9.6 — "always use the highest slew rate"). embassy's
        // `OutputDrive` can't express E0/E1, so poke the retained PIN_CNF/BIAS registers directly
        // after `Spim::new` has claimed the pins. (High drive alone caps the timing budget at
        // ~16 MHz — tSPIM,SUMI 34 ns vs 13 ns at extra-high.)
        {
            use embassy_nrf::pac;
            use embassy_nrf::pac::gpio::vals::Drive;
            for pin in [6usize, 8, 9] {
                pac::P2_S.pin_cnf(pin).modify(|w| {
                    w.set_drive0(Drive::E);
                    w.set_drive1(Drive::E);
                });
            }
            pac::GPIOHSPADCTRL_S.bias().modify(|w| w.set_hsbias(3));
        }
        // CS is a plain GPIO held LOW for the session (the `sd::NoCs` workaround), not a SPIM-bus
        // pin — it sits on **P2.10** so the whole SD bus lives on one port next to its clock.
        let sd_cs = Output::new(p.P2_10, Level::High, OutputDrive::Standard);
        let storage = sd::init(sd_spi, sd_cs);
        // A missing/bad card is fatal — the map streams from it. The display is already up (brought
        // up above, before the card), so instead of failing silently we put an **undismissable**
        // fault screen on glass, then heartbeat-idle. (SharedStore keeps an Option seam for a future
        // card-less variant where BLE config/bond/diagnostics still serve.)
        let Some(mut storage) = storage else {
            defmt::error!("SD: no card / mount failed — showing the NO SD CARD fault screen, then heartbeat idle");
            show_boot_fault(&mut display, obc_app::BootFault::NoCard).await;
            idle_blink(&mut led).await
        };

        // Reclaim any map upload that never committed (issue #927) before the catalog is read: a
        // torn transfer leaves its final `MP{id}.OBM` with the held-back magic still zeroed, which
        // every catalog refuses — so without this sweep its hundreds of megabytes would sit on the
        // card forever, invisible to the one surface that could explain them. The map twin of the
        // object store's `is_aborted_commit` sweep over `/routes`, and it must run **before**
        // `open_map` so the selection never lands on a corpse.
        storage.sweep_aborted_maps();

        // The same reclaim for a torn **volume set** (issue #1039): a set is `1..=32` shard files
        // plus a manifest, so its abandoned upload is gigabytes rather than hundreds of megabytes,
        // and the file that identifies it is the zero-magic `MS{id}.OBS` token the upload writes
        // before the first shard. Runs before `open_map` for the same reason the map sweep does —
        // and after it, so a card carrying both kinds of corpse is clean in one boot.
        storage.sweep_aborted_sets();

        // Open the selected `.obcm` and hold it open for the session — the map **streams** from it,
        // never read resident into the 256 KB part. (The `/routes/*.obcr` catalog is scanned into the
        // app's Route menu by `load_routes` *after* the app is built — in its own frame, so the ~5 KB
        // `Catalog` never sits on `main`'s stack beneath the long-lived ride loop.)
        storage.open_map();

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
        // fault screen, then idles. The idle camera centre is the parsed bbox. `init_src`'s `storage` borrow
        // ends with this block, so the loop can rebuild a fresh source each redraw AND reconcile the card
        // (`&mut storage`) between frames.
        // SAFETY: sole owner of MAP_TABLES; single executor → no aliasing; written exactly once here.
        // Which fault a map-less boot deserves is the *catalog's* answer, not `map_source`'s: a card
        // holding a volume set has a map on it that this build declines to mount, and NO MAP would
        // send the rider looking for a file that is right there. `Storage::boot_fault` is NO MAP
        // unless `open_map` found a map and refused it; the rule itself is `obc_app::boot_fault`,
        // tested where tests run. Read before the `map_source` borrow so the `else` arm is free of it.
        let map_fault = storage.boot_fault();
        let map_tables: &MapTables = unsafe {
            let Some(init_src) = storage.map_source() else {
                defmt::error!(
                    "SD: no map to stream from — showing the {} fault screen, then heartbeat idle",
                    defmt::Debug2Format(&map_fault)
                );
                show_boot_fault(&mut display, map_fault).await;
                idle_blink(&mut led).await
            };
            let slot = core::ptr::addr_of_mut!(MAP_TABLES) as *mut MapTables;
            match MapTables::parse(&init_src) {
                Ok(t) => {
                    slot.write(t);
                    &*slot
                }
                Err(e) => {
                    defmt::error!(
                        "map: not valid OBCM: {} — showing the MAP UNREADABLE fault screen, then idle",
                        defmt::Debug2Format(&e)
                    );
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

        // Boot to **Home**: the user drives Home → Route menu → Map with the buttons. Built **in place**
        // in `.bss` (`init_idle` writes each field where it sits; the 8,352 B renderer scratch is zeroed
        // in place), never on the stack. The Route menu is filled from the card's catalog scanned above;
        // selecting an entry opens the Map at that route's start and streams its geometry into the render
        // + the map-matcher.
        // SAFETY: sole owner of APP; `init_idle` fully initialises it before the `&mut` below reads it.
        let app: &mut App = unsafe {
            let slot = core::ptr::addr_of_mut!(APP) as *mut App;
            App::init_idle(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            &mut *slot
        };
        {
            ride::load_routes(&mut storage, app);
            ride::load_rides(&mut storage, app);
            // Trip folders (epic #526 TR4): scan `TP{id}.OBT` and resolve each trip's stages against
            // the route catalog just loaded — after `load_routes`, so the stage resolution sees it.
            ride::load_trips(&mut storage, app);
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
                app.set_map_info(storage.map_name(), map_tables.version);
            }
            // Issue #504: the map loaded but its extent table was refused (fragmented past the cap /
            // failed verification), so reads fall back to the slow FAT-seek path. Surface it once as a
            // dismissable notice — the `Warning` event pushes the card over Home; the ride loop shows it
            // on the first frame. A contiguous map (the common case) sets nothing.
            if storage.map_degraded() {
                app.apply_event(obc_app::HostEvent::Warning(obc_app::WarningFlags::MAP_SLOW));
            }
        }

        // Place the decoded-route-geometry cache in `.bss`, built in place (a zeroed
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary — like `MAP_CACHE`).
        // SAFETY: sole owner of ROUTE_CACHE; single map plane → no aliasing.
        let route_cache: &RouteCache = unsafe { init_static(core::ptr::addr_of_mut!(ROUTE_CACHE), RouteCache::new()) };

        // The router's A* table + graph-tile cache (epic #116, R4), placed in `.bss` and built in
        // place (both `new()`s are const all-zero → memsets). Threaded into the ride loop as one
        // `NavBuffers` bundle, which `plan_nav` uses per drained create-route request — never
        // stack locals. On the `ble` build (`not(has_nav)`, see build.rs) the bundle is the unit
        // stand-in — no statics exist and the ride loop answers requests with the generic
        // failure tier.
        // SAFETY: sole owners of NAV_SCRATCH / NAV_TILES; single map plane → no aliasing.
        #[cfg(has_nav)]
        let nav = ride::NavBuffers {
            scratch: unsafe { init_static(core::ptr::addr_of_mut!(NAV_SCRATCH), obc_route::NavScratch::new()) },
            tiles: unsafe { init_static(core::ptr::addr_of_mut!(NAV_TILES), obc_reader::NavTileCache::new()) },
            // SAFETY: the sole handle to the planner slot; the ride loop (re)writes it per
            // request and reads it only while its plan bookkeeping is active.
            planner: unsafe { &mut *core::ptr::addr_of_mut!(NAV_PLANNER) },
        };
        #[cfg(not(has_nav))]
        let nav = ride::NavBuffers;

        // The persistent settings store: takes the `RRAMC` peripheral, reads/writes the blob in the
        // carved RRAM page. Built here (where `p` is live) and moved into the ride loop, which seeds the
        // app at boot and saves on a settings edit. Every boot also bumps the persisted boot counter,
        // the diagnostics blob's one durable fact.
        let mut settings_store = settings::RramSettingsStore::new(p.RRAMC);
        let boot_count = settings_store.bump_boot_count(reset_reas);
        defmt::info!("boot #{=u32}", boot_count);

        // The hardware watchdog (#349): the last-resort net under both planes, fed by the ride
        // loop (gated on the input plane's heartbeat) in every build. 24 s is generous on purpose: the
        // dog must never fire on a slow frame or a long SD reconcile, only on a genuine wedge. It
        // counts through sleep but **pauses under a debugger halt** (`HaltConfig::Pause`) so a
        // breakpoint doesn't cascade into a reset — and so probe-rs can flash with the dog live.
        // Once started a WDT can never be stopped; a warm reset carries it over, in which case
        // `try_new` re-adopts it if the config matches (ours is constant, so it does). A foreign
        // config (e.g. an older image's) can't be adopted or fed — log it and run unfed: the stale
        // period fires once and the next boot starts clean. Since DR1 (#729) the bootloader plays
        // the same game from its side: it adopts + pets this dog across a DFU install (the arm's
        // warm reset carries it in) and pre-starts an identical one before jumping into a trial
        // boot — which is then exactly what this `try_new` adopts. The config contract (timeout,
        // halt/sleep behavior, one handle) is documented on `obc_dfu::WDT_TIMEOUT_TICKS`.
        //
        // INVARIANT (#729): because EVERY trial boot now enters with the dog already counting,
        // everything between app entry and this line must complete well inside one WDT period
        // (24 s) on a trial boot — or the dog resets a perfectly healthy trial image and the
        // bootloader rolls it back. Today that's seconds of headroom, and nothing blocking sits
        // upstream (a missing/slow card does NOT block boot — the build idles without one).
        // Keep it that way: never move a blocking or open-ended retry loop (SD mount, sensor
        // bring-up) above this point.
        let wdt_handle = {
            let mut cfg = wdt::Config::default();
            cfg.timeout_ticks = ride::WDT_TIMEOUT_TICKS;
            cfg.action_during_debug_halt = wdt::HaltConfig::Pause;
            match wdt::Watchdog::try_new::<_, 1>(p.WDT0, cfg) {
                Ok((_wdt, [handle])) => Some(handle),
                Err(_) => {
                    defmt::warn!("WDT: already running with a foreign config — cannot feed it; expect one reset");
                    None
                }
            }
        };

        // The shared store: the mounted card + the RRAM settings move behind one async mutex, so the
        // ride loop and the BLE object plane can each lock it per pass. Built in place in `.bss`
        // (warm-reset-safe via `init_static`, like the caches above). `storage`/`settings_store` are
        // consumed here; both planes reach them through the guard. `storage` was unwrapped to a
        // `Storage` at boot (the build idles without a card), so it goes in as `Some` — the `Option`
        // is the seam a future card-less variant would use.
        static mut SHARED_STORE: MaybeUninit<SharedStoreMutex> = MaybeUninit::uninit();
        let shared_store: &'static SharedStoreMutex = unsafe {
            init_static(
                core::ptr::addr_of_mut!(SHARED_STORE),
                Mutex::new(SharedStore { storage: Some(storage), settings: settings_store }),
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
                match obc_app::settings::store_epoch_mint(card_epoch, marks, fresh) {
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

        // --- The BLE stack, `ble` builds: group the peripheral claims (MPSL: GRTC CH7–11 + TIMER10/20
        // + TEMP + its PPI/PPIB lanes; SDC: the PPI10 fan-out + PPIB bridges; CRACEN for the LL's crypto
        // RNG) and spawn [`spawn_ble_stack`] — the trampoline that spawns [`ble::run`] from a shallow
        // poll frame (see its doc: constructing the ~31 KB `ble::run` future in `main` put a ~31 KB
        // temporary slot in **`main`'s poll frame**, allocated at frame entry, which overflowed the
        // combined build's ~40 KB stack before the first line of `main` ran — #270). Nothing here
        // clashes with the rest of `main` (embassy's GRTC time driver allocates channels from CH0 up;
        // TIMER10/20 and the PPI lanes are otherwise unused). ---
        #[cfg(feature = "ble")]
        {
            let mpsl_p = nrf_sdc::mpsl::Peripherals::new(
                p.GRTC_CH7,
                p.GRTC_CH8,
                p.GRTC_CH9,
                p.GRTC_CH10,
                p.GRTC_CH11,
                p.TIMER10,
                p.TIMER20,
                p.TEMP,
                p.PPI10_CH0,
                p.PPI20_CH1,
                p.PPIB11_CH0,
                p.PPIB21_CH0,
            );
            let sdc_p = nrf_sdc::Peripherals::new(
                p.PPI00_CH1,
                p.PPI00_CH3,
                p.PPI10_CH1,
                p.PPI10_CH2,
                p.PPI10_CH3,
                p.PPI10_CH4,
                p.PPI10_CH5,
                p.PPI10_CH6,
                p.PPI10_CH7,
                p.PPI10_CH8,
                p.PPI10_CH9,
                p.PPI10_CH10,
                p.PPI10_CH11,
                p.PPIB00_CH1,
                p.PPIB00_CH2,
                p.PPIB00_CH3,
                p.PPIB10_CH1,
                p.PPIB10_CH2,
                p.PPIB10_CH3,
            );
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
        _spawner.spawn(defmt::unwrap!(spawn_usb_stack(_spawner, p.USBHS, link_stores)));

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
