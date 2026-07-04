//! nRF54L15-DK board firmware for OpenBikeComputer — the **real hardware target**.
//!
//! The nRF54L15 driving the reflective LS021B7DD02 memory-LCD through the FLPR coprocessor is
//! what the project ships on — the **default build**; the ST7789 EYESPI TFT stays as the opt-in
//! `tft` bring-up backend. This crate ports the
//! shared `obc-app` onto it (load route → ride → save GPX). Nothing app-facing lives here:
//! `obc-render` / `obc-app` / `obc-reader` / `obc-route` + `obc-platform` stay board-agnostic;
//! only the nRF HAL wiring + the display `DisplayDriver` backends are board-specific.
//!
//! **The `ble` build** (`cargo run --release --no-default-features --features ble`): the same
//! firmware with the BLE stack folded in (`ble.rs`: MPSL, the SoftDevice Controller, TrouBLE).
//! On the 256 KB DK it compiles the **map plane out** (the build.rs-emitted `has_map` cfg — no
//! `App`/`MapCache`/`RouteCache`/`RouteIndex`, ~128 KB freed) and boots [`run_status`] — a
//! text-only BLE status UI on the same panel — instead of [`run_app`]; SD, RRAM settings,
//! buttons, sensors, and the FLPR display all stay up, so the radio runs inside the real
//! executor/interrupt/storage layout. The 512 KB LM20 re-enables map + BLE together by relaxing
//! `has_map` in build.rs; the budget assert arbitrates.
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
//!   BTN0 P1_13 PREV | BTN1 P1_09 NEXT | BTN2 P1_08 BACK | BTN3 P0_04 SELECT
//! Map to obc-platform's board-agnostic `ButtonInput` debouncer → the shared gesture recogniser.
//! Roles: BTN0/1 → encoder Turn∓1, BTN3 → encoder press/hold, BTN2 → Back/back-hold
//! (`ButtonInput::new` order is prev, next, select, back). Read as plain **polled** `gpio::Input`
//! (the debouncer samples levels each loop — no GPIOTE async wait needed). They stay free because
//! the display lives on P2 (below).
//!
//! ## Display SPIM — ST7789 EYESPI stand-in
//!   Instance **SERIAL00 / SPIM00** — the only instance that reaches 32 MHz (fast/MCU power
//!   domain, port P2); the panel wants a fast bus so it gets this one. Its pins are the DK's
//!   on-board QSPI-flash bus (P2.00–P2.05). We never use that flash (maps live on SD), so the
//!   **Board Configurator** app electronically disconnects it ("external memory → GPIO on the
//!   P2 header") and routes the pins out — no soldering on current board revisions. The whole
//!   panel then sits on the P2 header:
//!     SCK P2_01 | MOSI P2_02 | CS P2_05 | DC P2_03 | RST P2_00   (MISO P2_04 unused, write-only)
//!   CS is toggled in software per transaction (embassy-nrf drives no hardware CS), framing each
//!   command/data write the way the ST7789 expects — see `st7789::St7789::transaction`. The panel's
//!   level shifters want 3–5 V logic, so the DK I/O rail must be raised from its 1.8 V default
//!   to **3.3 V** (VDDM, also in the Board Configurator — HW guide §2.2.1); Vin is fed from the
//!   DK's 5 V (VBUS) so the panel's onboard 3.3 V LDO keeps headroom. Putting the display on P2
//!   leaves all of P1 free for SD + VCOM + the buttons. The band push expands the RGB222
//!   framebuffer → RGB565 and SPIM-DMAs a CASET/RASET window (wire-pack in the ST7789 backend's
//!   `flush_window`, behind the board-agnostic `DisplayDriver` seam the LS021B7DD02 backend
//!   implements too).
//!
//! ## microSD SPIM — map/route/track storage
//!   Instance **SERIAL22 / SPIM22** — a standard-speed instance (SD doesn't need 32 MHz),
//!   *separate* from the display bus, on its own software CS. DK expansion-header SPI pins:
//!     SCK P1_11 | MISO P1_07 | MOSI P1_06 | CS P1_12   (FLPR build moves CS → P0_00, see below)
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
//!     SERIAL30 sensor-bus ISR.
//!   - **Thread mode**: the map/status plane (`run_app` / `run_status`) + the sensor task.
//!
//!   MPSL's P0 lane preempts everything, including the P3 planes — safe by construction for the
//!   panel: the FLPR scans the framebuffer autonomously (#347), so M33 preemption can no longer
//!   stretch a frame push at all (only the ack's delivery, which is untimed).
//!
//! ## Flash / RAM
//!   From the `nrf54l15-app-s` `memory.x`: FLASH 1524K @ 0x0000_0000, RAM 256K @ 0x2000_0000.
//!   A future MCUboot retrofit re-partitions flash — don't hard-code flash assumptions (see
//!   `memory.x`). RAM is tight (no external RAM): the single RGB222 framebuffer is ~75 KB and the
//!   renderer scratch + caches must fit the rest — see the budget assert below.

#![no_std]
#![no_main]

mod sd;
// The ST7789 driver — the opt-in `tft` map backend only. The frame geometry both backends share
// lives in `display::FRAME_W`/`FRAME_H`, so the default FLPR build compiles none of this.
#[cfg(feature = "tft")]
mod st7789;
// LS021 FLPR backend — the **default** display: `main.rs` runs the real app on the reflective LS021
// panel via the FLPR (the VPR coprocessor) unless `--features tft` selects the ST7789 panel instead.
// The FLPR `DisplayDriver` backend + launch live in `ls021_flpr`; `com::com_task` free-runs the COM
// lines (the FLPR drives frames; only the COM electrode square wave stays on the M33).
#[cfg(not(feature = "tft"))]
mod com;
// Zero-CPU hardware COM: drive the COM square wave from a TIMER→DPPI→GPIOTE toggle chain instead of
// the M33 `com_task`, so the panel's anti-DC-bias COM keeps alternating with no core wakes and the M33
// can WFI between events. Opt-in (`com-hw`) + production-board-only — the DK wires COM on P2, which has
// no GPIOTE — so the default DK build keeps `com::com_task`. See `com_hw.rs`.
#[cfg(all(feature = "com-hw", not(feature = "tft")))]
mod com_hw;
// `com-hw` drives the LS021 panel's COM, so it's meaningless on the ST7789 (`tft`) backend; and its
// placeholder COM pins (P1.04/05) are the VCOM-UART pins the `debug-uart` host feed uses, so the two
// can't share the board. Fail fast with a clear message rather than a confusing double-consume error.
#[cfg(all(feature = "com-hw", feature = "tft"))]
compile_error!("`com-hw` drives the LS021 COM lines — it has no effect with `tft` (the ST7789 backend)");
#[cfg(all(feature = "com-hw", feature = "debug-uart"))]
compile_error!("`com-hw` and `debug-uart` both claim P1.04/P1.05 — the hardware-COM build is the production low-power path, not the host-feed dev build");
#[cfg(not(feature = "tft"))]
mod ls021_flpr;
// The board's display-driver seam — the single screen-write interface both panels implement, so the
// map plane drives either through one path (`fb_mut` + `present`).
mod display;
// The two-plane display machinery both backends share (issue #351): the cfg-selected `MapDisplay`
// handle, the high-priority input/overlay tasks + the gesture channel, and their executor/ISR
// statics. `main` constructs the panels and spawns the tasks; the planes live there.
mod planes;
// The map/ride thread-mode plane: `run_app` + its loop-only helpers. `has_map` builds only (the
// `ble` DK build compiles the whole map plane out and boots `run_status` instead).
#[cfg(has_map)]
mod ride;
// The `ble` status build's thread-mode plane: `run_status`, the text-only BLE status UI.
#[cfg(not(has_map))]
mod status;
// Persistent device settings over on-chip RRAM (the SD-independent settings store); boot-load +
// save-on-dirty are wired in `run_app`.
mod settings;
// Real GPS (SAM-M10Q) + altimeter (BMP581) on a shared TWIM30 I²C bus — the concrete transport + the
// event-driven sensor task. Compiled only on the **real-sensor** build (the default: neither `synth`
// nor `debug-uart`), since `synth`/`debug-uart` supply the location source instead.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
mod sensors;
// The BLE stack: MPSL + SDC + TrouBLE, the advertise loop, and the link-status plumbing +
// status-screen drawer [`run_status`] presents. `ble` builds only.
#[cfg(feature = "ble")]
mod ble;
// The device object store: object ids / revision / upload state over the SD catalog, and the Config ↔
// RRAM-settings bridge the BLE control plane drives. `ble` builds only.
#[cfg(feature = "ble")]
mod object_store;

// The `ble` feature-matrix guards. MPSL *provides* the critical-section impl (its radio timing
// forbids global-interrupt-disable critical sections; two impls = duplicate link symbols), so the
// default `cs-single-core` must be off — fail with the right invocation rather than a cryptic linker
// error. The other combinations are meaningless on the status build: no ride loop exists for the host
// feed / synthetic walk to drive, and the status UI targets the shipping LS021/FLPR panel.
#[cfg(all(feature = "ble", feature = "cs-single-core"))]
compile_error!(
    "`ble` uses MPSL's critical-section impl — build with `cargo run --release --no-default-features --features ble`"
);
#[cfg(all(feature = "ble", feature = "tft"))]
compile_error!("the `ble` build drives the LS021/FLPR panel — it does not support `tft`");
#[cfg(all(feature = "ble", feature = "debug-uart"))]
compile_error!("the `ble` status build has no ride loop — `debug-uart`'s host feed has nothing to drive");
#[cfg(all(feature = "ble", feature = "synth"))]
compile_error!("the `ble` status build has no ride loop — `synth` has nothing to drive");

use defmt::info;
use display::{FRAME_H, FRAME_W};
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

// `Delay` (the ST7789 power-on waits) + the `St7789` driver type are the opt-in `tft` backend; the
// default FLPR build replaces them with the `Ls021Flpr` `DisplayDriver` backend, so neither is there.
#[cfg(feature = "tft")]
use embassy_time::Delay;
#[cfg(feature = "tft")]
use st7789::St7789;

// The ST7789 is the opt-in `tft` backend, so every ST7789-backend `cfg` below keys on
// `feature = "tft"` and the rest of the file treats `not(feature = "tft")` as the default FLPR path.

use core::cell::RefCell;
use core::mem::MaybeUninit;

#[cfg(feature = "tft")]
use display::Display;
use embassy_nrf::gpio::{Input, Pull};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
#[cfg(has_map)]
use embassy_nrf::wdt;
// The shared GPS/altimeter I²C bus — real-sensor build only.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use embassy_nrf::twim::{self, Twim};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
#[cfg(feature = "tft")]
use embassy_sync::mutex::Mutex;
// The map/ride half of obc-app lives only on `has_map` builds (the `ble` DK build compiles the map
// plane out); the shared `InputPlane` is every build's.
use obc_app::InputPlane;
#[cfg(has_map)]
use obc_app::{App, AppState};
use obc_platform::{ButtonInput, RowDiff};
#[cfg(has_map)]
use obc_reader::{MapCache, MapTables};
#[cfg(has_map)]
use obc_render::zoom_for_mpp;
// The decoded-route-geometry cache — resident in `.bss`, handed to the ride loop.
#[cfg(has_map)]
use obc_route::RouteCache;

// LS021 FLPR backend: the resident-framebuffer `DisplayDriver` backend + its launch, and the
// free-running COM driver. The M33 `com_task` is the DK/default path; the `com-hw` build drives COM
// from hardware instead, so the task isn't spawned there.
#[cfg(all(not(feature = "tft"), not(feature = "com-hw")))]
use com::com_task;
#[cfg(all(feature = "com-hw", not(feature = "tft")))]
use com_hw::HwCom;
#[cfg(not(feature = "tft"))]
use ls021_flpr::{launch_flpr, relaunch_flpr, FlprError, Ls021Flpr};

// VCOM debug-sensor / telemetry stream, behind `debug-uart`: the interrupt-buffered UARTE on the DK's
// J-Link VCOM. `BufferedUarte` keeps RX DMA continuously armed into a ring driven by the SERIAL20
// interrupt, so the tens-of-ms map render never drops a streamed byte. 8N1 @ 115200.
#[cfg(feature = "debug-uart")]
use embassy_nrf::buffered_uarte::{self, BufferedUarte, BufferedUarteRx, BufferedUarteTx};
#[cfg(feature = "debug-uart")]
use embassy_nrf::uarte;

// SERIAL00 backs the display SPIM; SERIAL22 the microSD SPIM. Both handlers are always registered
// (harmless when a feature is off — the peripheral is never constructed, so its interrupt never fires).
bind_interrupts!(struct Irqs {
    SERIAL00 => spim::InterruptHandler<peripherals::SERIAL00>;
    SERIAL22 => spim::InterruptHandler<peripherals::SERIAL22>;
});

// VCOM UARTE20 RX/TX → the `BufferedUarte`'s interrupt-fed ring buffers.
#[cfg(feature = "debug-uart")]
bind_interrupts!(struct UartIrqs {
    SERIAL20 => buffered_uarte::InterruptHandler<peripherals::SERIAL20>;
});

// TWIM30 (== SERIAL30) backs the shared GPS + altimeter I²C bus; bound only on the real-sensor build.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
bind_interrupts!(struct SensorIrqs {
    SERIAL30 => twim::InterruptHandler<peripherals::SERIAL30>;
});

/// One band's worth of RGB565 scratch (`WIDTH * BAND_ROWS`), living in `.bss`. 14 rows ≈ 6.6 KB and
/// tiles the 320-row frame in 23 bands. The banded push fills it — the map path expands the RGB222
/// frame into it — then byte-swaps to big-endian and SPIM-DMAs it; borrowed exactly once below (single
/// executor → no aliasing). Must stay ≥ the overlay window `OVL_W × OVL_ROWS`, asserted below
/// (240×14 = 3360 ≥ 16×192 = 3072).
///
/// **ST7789-only** (`--features tft`). The default FLPR map path packs the RGB222 framebuffer straight
/// to the LS021 wire and renders device-64 directly, so it needs no RGB565 band scratch (the FLPR
/// build drops `BAND_BYTES` from the budget below).
#[cfg(feature = "tft")]
const BAND_ROWS: usize = 14;
/// The band scratch in bytes (RGB565, 2 B/px) — the resident cost the budget assert reserves.
#[cfg(feature = "tft")]
const BAND_BYTES: usize = FRAME_W * BAND_ROWS * 2;
#[cfg(feature = "tft")]
static mut BAND: [u16; FRAME_W * BAND_ROWS] = [0; FRAME_W * BAND_ROWS];

// ============================ Board memory budget ============================
// The nRF54L15 has 256 KB RAM and no external RAM, so the whole resident working set of a full map
// redraw must fit there. This build-time assert fails the build — rather than overflowing RAM on
// glass — if the shared crates' caps (trimmed by the `nrf-mem` profile, enabled on the obc-app edge in
// Cargo.toml) ever outgrow the budget. It compiles for thumbv8m (usize = 4 B), so every `size_of` here
// is the true on-device size.
//
// The binding moment is a full redraw with everything resident at once:
//   - `App`        embeds the renderer scratch (`obc_render::MCU_RENDERER_BYTES`, ~66 KB nrf-mem)
//                  plus the resident elevation `Profile` (~4.6 KB at PROFILE_COLS=512) and
//                  `Breadcrumb` (~6 KB at SPINE_CAP=512); ~88 KB total.
//   - framebuffer  the single RGB222 frame: 240×320 × 1 B/px = 75 KB — the `FB` static below.
//   - `MapCache`   the streamed-map geometry-chunk cache (3 slots on nrf-mem, ~25 KB).
//   - `RouteCache` the decoded-route-chunk cache (3 slots on nrf-mem, ~9 KB).
//   - `RouteIndex` the active route's resident chunk index — the ride loop holds it across frames in
//                  the map plane's task future to stream geometry without re-walking it (128 chunks on
//                  nrf-mem, ~6 KB). Counted here because on the 256 KB part it materially shares the
//                  budget, and because `RouteIndex::read` builds it on the *stack*, so keeping it ~6 KB
//                  keeps that transient build spike inside the stack reserve below.
//   - band scratch one RGB565 display band (`BAND_BYTES`, ~7.5 KB; ST7789 only).
// plus `STACK_RESERVE` headroom for the main stack + embassy's executor/task arenas. The stack must
// also absorb a per-redraw `Reader::new` (the OBCM style table → a ~2.4 KB `Reader` value built as a
// stack temporary, plus its own ~4 KB read scratch): the ride loop rebuilds it each frame, so the
// stack reserve carries that spike.
//
// The FLPR build reclaims room without re-trimming the caps: the carve-out leaves the M33 **248 KB**
// (not 256) but the production blob is ~820 B so the carve is only ~8 KB, and the FLPR map path drops
// the ~6.6 KB RGB565 band scratch — a net loosening, so the same caps clear the budget.

/// Total SRAM the M33 app core sees. The opt-in ST7789 build (`--features tft`) links the full 256 KB
/// (`memory.x`: RAM 256K @ 0x2000_0000); the default FLPR build links what the carve leaves — taken
/// straight from the generated contract (`build.rs` derives the carved `memory.x` and this constant
/// from the same `FLPR_RAM_BASE`, so the budget can't fork from the linker map).
#[cfg(feature = "tft")]
const NRF_RAM_BYTES: usize = 256 * 1024;
#[cfg(not(feature = "tft"))]
const NRF_RAM_BYTES: usize = ls021_flpr::M33_RAM_BYTES;
/// Headroom kept free under the resident statics for the main stack + embassy's executor/task arenas
/// (statics grow up from the RAM base, the stack down from the top). This is only the build-time
/// *floor* the assert enforces — the real stack is the residual `RAM − statics` (~37.8 KB on the
/// default build). Pinned above the **measured deep-path peak**: 35,808 / 37,760 B on 2026-07-04
/// (debug-uart FLPR build, post-#351 split; VCOM-harness full ride — fix on Home → route load →
/// ride → finish-to-GPX), so a change that squeezes the residual below what the deepest path
/// actually reaches fails at compile time (e.g. a `ble` + map build on the 256 KB DK) instead of
/// overflowing the stack on glass.
#[cfg(has_map)]
const STACK_RESERVE: usize = 36 * 1024;
/// The `ble` status build's floor: no deep-render path, but the SDC/host futures and MPSL's ISRs all
/// ride the main stack — and the ~128 KB the excluded map plane frees leaves room to be generous.
#[cfg(not(has_map))]
const STACK_RESERVE: usize = 32 * 1024;
/// The single RGB222 framebuffer: one byte per pixel over the 240×320 frame = 75 KB.
const FB_BYTES: usize = FRAME_W * FRAME_H;

/// The map plane's residents (the table above). Includes the active route's `RouteIndex`, kept
/// resident across frames. **Zero on the `ble` DK build** — the whole plane is compiled out.
#[cfg(has_map)]
const MAP_RESIDENT: usize = core::mem::size_of::<obc_app::App>()
    + core::mem::size_of::<obc_reader::MapCache>()
    + core::mem::size_of::<obc_reader::MapTables>()
    + core::mem::size_of::<obc_route::RouteCache>()
    + core::mem::size_of::<obc_route::RouteIndex>();
#[cfg(not(has_map))]
const MAP_RESIDENT: usize = 0;
/// The BLE stack's residents (`ble::RESIDENT_BYTES`: the MPSL handle + SDC memory block + TrouBLE's
/// host arena + its global packet pool + the CRACEN RNG); zero without the feature. Keeping both terms
/// in one sum is what makes "`ble` + map don't fit on 256 KB" a *compile-time* fact: when the LM20
/// relaxes `has_map` in build.rs, both planes land here and this assert arbitrates.
#[cfg(feature = "ble")]
const BLE_RESIDENT: usize = ble::RESIDENT_BYTES;
#[cfg(not(feature = "ble"))]
const BLE_RESIDENT: usize = 0;

/// The resident set that must coexist during a redraw (see the table above).
const RESIDENT_BYTES: usize = FB_BYTES
    + core::mem::size_of::<RowDiff<FRAME_H>>() // the self-diffing present store (#201, 1.28 KB)
    + BAND_RESERVE
    + MAP_RESIDENT
    + BLE_RESIDENT;
/// The RGB565 band scratch the budget reserves: `BAND_BYTES` on the ST7789 path, **zero** on the FLPR
/// path (it packs the framebuffer straight to the wire — see [`BAND_ROWS`]).
#[cfg(feature = "tft")]
const BAND_RESERVE: usize = BAND_BYTES;
#[cfg(not(feature = "tft"))]
const BAND_RESERVE: usize = 0;
const _: () = assert!(
    RESIDENT_BYTES + STACK_RESERVE <= NRF_RAM_BYTES,
    "nRF resident set (framebuffer + RowDiff + band + map plane [App/MapCache/MapTables/RouteCache/RouteIndex] + BLE stack [MPSL/SDC mem/host arena]) + stack reserve overruns RAM — the map plane and the BLE stack do not coexist on the 256 KB DK (issue #270); on the LM20 trim the `nrf-mem` caps (#124) instead"
);

/// The resident device-native RGB222 framebuffer: one byte per pixel over the 240×320 panel
/// (`FB_BYTES` = 75 KB), in `.bss`. [`App::render_map`](obc_app::App::render_map) quantizes into it on
/// store ([`FbDevice64`]). On the **ST7789** path it is borrowed into the [`Display`] behind [`BUS`]
/// and the band push expands it back to RGB565, so the two planes reach it only under that mutex (no
/// aliasing, no torn frame). On the default **FLPR** path it is owned by the `Ls021Flpr` panel — the
/// map plane renders into it and `push_frame` packs it straight to the LS021 wire.
static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// The **self-diffing present** store: one 32-bit hash per framebuffer row of the last-pushed frame,
/// in `.bss` (320 rows = 1.28 KB). The active display backend borrows it (`&mut`) and, on present,
/// re-hashes each row and pushes only the rows whose hash changed — so a Home clock tick re-presents
/// its clock band instead of all 320 rows (~44 ms since #348 → a few ms on the FLPR). `RowDiff::new()` is all-zero
/// (+ the unprimed flag) ⇒ a `.bss` static, and the first present force-pushes the whole frame to seed it.
static mut ROW_DIFF: RowDiff<FRAME_H> = RowDiff::new();

/// The streamed-map geometry cache + the shared [`App`], placed in `.bss` and built **in place** (a
/// `ptr::write` into the reserved region): the ~96 KB `App` (incl. the ~74 KB renderer scratch) and the
/// ~41 KB cache must never form on the 256 KB part's small stack. [`MapCache::new`](obc_reader::MapCache)
/// is an all-zero `MaybeUninit::zeroed`, so writing it is a `.bss` memset.
#[cfg(has_map)]
static mut MAP_CACHE: MaybeUninit<MapCache> = MaybeUninit::uninit();
/// The immutable map tables (header scalars + style table + LOD pyramid), parsed **once at boot** into
/// `.bss` and borrowed by every per-frame [`Reader`]. Resident so the per-frame render reader carries
/// no styles/LODs of its own — no per-frame style-table SD read, no ~4 KB parse stack spike on the deep
/// render path (the lever that kept that path inside the 256 KB stack).
#[cfg(has_map)]
static mut MAP_TABLES: MaybeUninit<MapTables> = MaybeUninit::uninit();
#[cfg(has_map)]
static mut APP: MaybeUninit<App> = MaybeUninit::uninit();
/// The decoded-route-geometry cache, placed in `.bss` and built in place like [`MAP_CACHE`]
/// ([`RouteCache::new`](obc_route::RouteCache) is an all-zero `MaybeUninit::zeroed`). The session-long
/// cache spares a redraw of the unchanged route + the matcher's per-fix decode from re-reading `.obcr`
/// geometry off the card every frame.
#[cfg(has_map)]
static mut ROUTE_CACHE: MaybeUninit<RouteCache> = MaybeUninit::uninit();

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

/// Idle camera zoom for the boot map, in ground metres-per-pixel (the 0.5–4 mpp riding band). A
/// coarse-ish 2 mpp shows a town-scale overview rather than a tight patch.
#[cfg(has_map)]
const INIT_MPP: f32 = 2.0;

/// Heartbeat-only idle for an unrecoverable bring-up failure (no card, no `.obcm`, or a map that isn't
/// valid OBCM): blink LED0 forever rather than panic — a missing/bad card must **never** fault
/// (acceptance criterion). Diverges.
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
}

/// VCOM RX → sensor signals: read bytes from the interrupt-fed ring and feed each complete
/// `F`/`A`/`C`/`K`/`Z` line into `obc-platform`'s fresh-fix signals, which the app's
/// `DebugLocation`/`DebugAltimeter`/`DebugCompass`/`DebugInput` poll. A UART never "disconnects", so
/// one `LineReader` lives for the whole session.
#[cfg(feature = "debug-uart")]
#[embassy_executor::task]
async fn vcom_rx_task(mut rx: BufferedUarteRx<'static, peripherals::SERIAL20>) {
    let mut buf = [0u8; 64];
    let mut reader = obc_platform::debug_link::LineReader::new();
    loop {
        match rx.read(&mut buf).await {
            Ok(n) => obc_platform::debug_link::feed_bytes(&mut reader, &buf[..n]),
            Err(e) => defmt::warn!("VCOM RX error: {}", defmt::Debug2Format(&e)),
        }
    }
}

/// VCOM TX ← telemetry: send one compact status line each time the app publishes telemetry (~2 Hz via
/// `set_telemetry`), so the host's readout updates without the device polling or flooding the link. The
/// buffered UARTE chunks the line to DMA itself, so no manual packet splitting is needed (the telemetry
/// line ≤192 B fits the TX ring); just loop until the whole line is queued.
#[cfg(feature = "debug-uart")]
#[embassy_executor::task]
async fn vcom_tx_task(mut tx: BufferedUarteTx<'static, peripherals::SERIAL20>) {
    loop {
        let t = obc_platform::debug_link::wait_telemetry().await;
        let line = obc_platform::debug_link::format_telemetry(&t);
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

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Run the M33 at its full **128 MHz** — embassy-nrf's `Config::default()` boots it at only
    // 64 MHz (`ClockSpeed::CK64`), which halves the per-band RGB222->RGB565->12-bit conversion *and*
    // keeps the high-speed SERIAL00 SPIM off the clock domain it needs for 32 MHz. Both directly
    // gate the banded push (the visible top-to-bottom fill), so this is the single biggest frame-time
    // lever on this panel.
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

    // Paint the stack now (still shallow) so the ride loop's high-water guard can read the peak.
    stackmeter::paint();

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

    // LED0 (P2_09) heartbeat — a liveness blink visible even before looking at the panel.
    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);

    // load → ride → save-GPX: stream the SD `.obcm` into the resident RGB222 framebuffer through the
    // shared `obc-app`, pick a route from the card catalog, ride it (VCOM-streamed GPS or the
    // `SynthLocation` square loop), map-match + log the track, and write a `.gpx` to `/tracks` on Finish.
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
            // `'static` tasks.
            static mut RX_BUF: MaybeUninit<[u8; 256]> = MaybeUninit::uninit();
            static mut TX_BUF: MaybeUninit<[u8; 256]> = MaybeUninit::uninit();
            // SAFETY: each ring is written once here, then handed to exactly one task half — no alias.
            let (rx_buf, tx_buf): (&'static mut [u8; 256], &'static mut [u8; 256]) = unsafe {
                (
                    init_static(core::ptr::addr_of_mut!(RX_BUF), [0; 256]),
                    init_static(core::ptr::addr_of_mut!(TX_BUF), [0; 256]),
                )
            };
            let uart = BufferedUarte::new(
                p.SERIAL20,
                p.P1_05, // RXD: host → device (fixes / input injection)
                p.P1_04, // TXD: device → host (telemetry)
                UartIrqs,
                uarte::Config::default(), // 8N1 @ 115200 — matches `obc-usb-host`'s default baud
                rx_buf,
                tx_buf,
            );
            let (rx, tx) = uart.split();
            _spawner.spawn(defmt::unwrap!(vcom_rx_task(rx)));
            _spawner.spawn(defmt::unwrap!(vcom_tx_task(tx)));
            info!("VCOM debug sensors up on UARTE20 (J-Link VCOM, TX P1_04 / RX P1_05) @ 115200");
        }

        // microSD on its own SPIM (SERIAL22, P1 header — separate from the display bus on SERIAL00/P2).
        // Init ≤400 kHz (SD spec); `sd::init` re-clocks to 8 MHz once the card answers. CS idles HIGH,
        // then `init` holds it LOW for the session (the per-byte-CS workaround — see `sd::NoCs`).
        // `orc = 0xFF` so any over-read clocks the SD idle byte.
        let mut sd_cfg = spim::Config::default();
        sd_cfg.frequency = sd::SD_INIT_HZ;
        sd_cfg.orc = 0xFF;
        let sd_spi = spim::Spim::new(p.SERIAL22, Irqs, p.P1_11, p.P1_07, p.P1_06, sd_cfg);
        // CS is a plain GPIO held LOW for the session (the `sd::NoCs` workaround), not a SPIM-bus
        // pin — so it can sit on any free GPIO. ST7789 build (`--features tft`): P1.12. Default FLPR:
        // P1.12 carries GEN, and the DK's P1.00–14 are one pin short, so CS moves to **P0.00** (M33
        // GPIO, the same port + drive as BTN3) — one jumper on the SD breakout. The SPIM bus pins
        // (SCK/MISO/MOSI on P1.11/07/06) are unchanged in both builds.
        #[cfg(feature = "tft")]
        let sd_cs = Output::new(p.P1_12, Level::High, OutputDrive::Standard);
        #[cfg(not(feature = "tft"))]
        let sd_cs = Output::new(p.P0_00, Level::High, OutputDrive::Standard);
        let storage = sd::init(sd_spi, sd_cs);
        // A missing/bad card is fatal only where the map streams from it. The `ble` status build keeps
        // booting — the card is a status line there ("sd --").
        #[cfg(has_map)]
        let Some(mut storage) = storage
        else {
            defmt::error!("SD: no card / mount failed — cannot load a map; idling with a heartbeat");
            idle_blink(&mut led).await
        };

        // Open the `.obcm` and hold it open for the session — the map **streams** from it, never read
        // resident into the 256 KB part. (The `/routes/*.obcr` catalog is scanned into the app's Route
        // menu by `load_routes` *after* the app is built — in its own frame, so the ~5 KB `Catalog`
        // never sits on `main`'s stack beneath the long-lived ride loop.)
        #[cfg(has_map)]
        storage.open_map();

        // Place the streamed-map geometry cache in `.bss`, built in place (an all-zero
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary).
        // SAFETY: sole owner of MAP_CACHE; single executor → no aliasing.
        #[cfg(has_map)]
        let map_cache: &MapCache = unsafe { init_static(core::ptr::addr_of_mut!(MAP_CACHE), MapCache::new()) };

        // Parse the OBCM **header + style table + LOD pyramid once at boot** into the resident
        // [`MAP_TABLES`]. These tables are immutable for the session, so the loop's per-frame readers
        // *borrow* them instead of re-parsing — no per-frame style-table SD read and no ~4 KB parse stack
        // spike (a 1536-byte style scratch + the ~2.3 KB style array) on the deep render path, which is
        // what kept that path overrunning the 256 KB part's stack. The transient parse cost is paid
        // **here**, at boot, where the call stack is shallow; a missing or structurally-bad map idles
        // with a heartbeat. The idle camera centre is the parsed bbox. `init_src`'s `storage` borrow
        // ends with this block, so the loop can rebuild a fresh source each redraw AND reconcile the card
        // (`&mut storage`) between frames.
        // SAFETY: sole owner of MAP_TABLES; single executor → no aliasing; written exactly once here.
        #[cfg(has_map)]
        let map_tables: &MapTables = unsafe {
            let Some(init_src) = storage.map_source() else {
                defmt::error!("SD: no .obcm map in card root — idling with a heartbeat");
                idle_blink(&mut led).await
            };
            let slot = core::ptr::addr_of_mut!(MAP_TABLES) as *mut MapTables;
            match MapTables::parse(&init_src) {
                Ok(t) => {
                    slot.write(t);
                    &*slot
                }
                Err(e) => {
                    defmt::error!("map: not valid OBCM: {} — idling with a heartbeat", defmt::Debug2Format(&e));
                    idle_blink(&mut led).await
                }
            }
        };
        #[cfg(has_map)]
        let (cam_lon, cam_lat) = {
            let b = map_tables.bbox;
            info!(
                "map: streaming from SD; bbox lon[{=i32}..{=i32}] lat[{=i32}..{=i32}]",
                b.min_lon, b.max_lon, b.min_lat, b.max_lat
            );
            (((b.min_lon as i64 + b.max_lon as i64) / 2) as i32, ((b.min_lat as i64 + b.max_lat as i64) / 2) as i32)
        };

        // Boot to **Home**: the user drives Home → Route menu → Map with the buttons. Built **in place**
        // in `.bss` (`init_idle` writes each field where it sits; the ~74 KB renderer scratch is zeroed
        // in place), never on the stack. The Route menu is filled from the card's catalog scanned above;
        // selecting an entry opens the Map at that route's start and streams its geometry into the render
        // + the map-matcher.
        // SAFETY: sole owner of APP; `init_idle` fully initialises it before the `&mut` below reads it.
        #[cfg(has_map)]
        let app: &mut App = unsafe {
            let slot = core::ptr::addr_of_mut!(APP) as *mut App;
            App::init_idle(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            &mut *slot
        };
        #[cfg(has_map)]
        ride::load_routes(&mut storage, app);

        // The four DK push-buttons (active-low, internal pull-up; polled by `ButtonInput`). User
        // mapping: BTN0 PREV, BTN1 NEXT, BTN3 SELECT, BTN2 BACK — `new(prev, next, select, back)`.
        // Shared by both backends — their pins (P1.13/09/08, P0.04) clash with neither panel's bus.
        let buttons = ButtonInput::new(
            Input::new(p.P1_13, Pull::Up), // BTN0 PREV   → Turn(-1)
            Input::new(p.P1_09, Pull::Up), // BTN1 NEXT   → Turn(+1)
            Input::new(p.P0_04, Pull::Up), // BTN3 SELECT → encoder press / hold
            Input::new(p.P1_08, Pull::Up), // BTN2 BACK   → back / back-hold
        );
        // The high-priority plane(s) run at P3 — above thread mode (so they preempt the map render) and
        // below the P0 GRTC time-driver (so their `Timer`s still wake mid-render). Shared vector (SWI01
        // — SWI00 is MPSL's low-prio lane on `ble` builds).
        interrupt::SWI01.set_priority(Priority::P3);

        // --- Real GPS + altimeter on the shared TWIM30 I²C bus. Default build only (neither `synth` nor
        // `debug-uart`). Build the bus + the TX-Ready interrupt line on the free P0 pins and spawn the
        // event-driven sensor task on the thread-mode executor; it probes both chips, configures the M10,
        // and publishes coherent (fix, altitude, temperature) datapoints through
        // `obc_platform::sensor_link`, which `run_app`'s `GpsLocation`/`BaroAltimeter`/`SensorTemp`
        // sources drain. The task is fully async (TWIM is DMA-backed). SERIAL30's ISR runs at P3. ---
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
            let twim = Twim::new(p.SERIAL30, SensorIrqs, p.P0_01, p.P0_02, twim_cfg, twim_tx);
            interrupt::SERIAL30.set_priority(Priority::P3);
            // TX-Ready (DDC data-ready) on the lone spare GPIO. Active-high, so pull down: a floating
            // / unconfigured line then reads low and the task's poll fallback drives fixes instead.
            let txready = Input::new(p.P0_03, Pull::Down);
            _spawner.spawn(defmt::unwrap!(sensors::sensor_task(twim, txready)));
            info!("sensors: SAM-M10Q + BMP581 task spawned on TWIM30 (SDA P0.01 / SCL P0.02, TX-Ready P0.03)");
        }

        // ============= ST7789 backend (opt-in `tft`): two-plane display + input/overlay =============
        // Build the panel + the shared `&'static` two-plane state, spawn the input/overlay plane (which
        // owns the bulge re-push), and hand the map plane back just the `bus` for the unified present.
        // Display on the (flash-freed) P2 header — CS idles HIGH and the driver pulses it low per
        // transaction (the warm-reset-safe CSX framing — see `st7789::St7789::transaction`); RST idles
        // high. SERIAL00 write-only SPIM at **32 MHz** (the max SERIAL00 reaches on the MCU-domain P2
        // pins) so a full-frame banded push is ~38 ms; drop to `M16` if the jumpered bring-up bus
        // sparkles. The shared state is parked in `.bss` + written **in place** rather than via
        // `StaticCell`, whose one-shot flag can panic "already full" on a warm reset.
        #[cfg(feature = "tft")]
        let display = {
            let cs = Output::new(p.P2_05, Level::High, OutputDrive::Standard);
            let dc = Output::new(p.P2_03, Level::Low, OutputDrive::Standard);
            let rst = Output::new(p.P2_00, Level::High, OutputDrive::Standard);
            let mut config = spim::Config::default();
            config.frequency = spim::Frequency::M32;
            let spi = spim::Spim::new_txonly(p.SERIAL00, Irqs, p.P2_01, p.P2_02, config);
            // SAFETY: sole references to BAND / FB; hereafter both are reached only through the bus
            // mutex (the two planes never touch them concurrently → no aliasing, no torn frame).
            let band = unsafe { &mut *core::ptr::addr_of_mut!(BAND) };
            let mut panel = St7789::new(spi, dc, rst, cs, Delay, band);
            panel.init();
            let fb: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
            // SAFETY: sole reference to ROW_DIFF; held by `Display` behind the bus mutex for the rest of
            // the program (the map plane is its only writer), never aliased.
            let diff: &'static mut RowDiff<FRAME_H> = unsafe { &mut *core::ptr::addr_of_mut!(ROW_DIFF) };
            info!("obc-fw-nrf54l N5: ST7789 up ({}x{}); two-plane input + map", FRAME_W, FRAME_H);

            static mut BUS: MaybeUninit<Mutex<CriticalSectionRawMutex, Display>> = MaybeUninit::uninit();
            // SAFETY: sole writer; initialised here before the `&'static` is shared with the input
            // plane, never written again (single executor builds it, two planes only read it).
            let bus: &'static Mutex<CriticalSectionRawMutex, Display> =
                unsafe { init_static(core::ptr::addr_of_mut!(BUS), Mutex::new(Display { panel, fb, diff })) };
            static mut INPUT_PLANE: MaybeUninit<BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>> =
                MaybeUninit::uninit();
            // SAFETY: as BUS — sole writer, initialised before shared, never rewritten.
            let input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>> = unsafe {
                init_static(core::ptr::addr_of_mut!(INPUT_PLANE), BlockingMutex::new(RefCell::new(InputPlane::new())))
            };

            let input_spawner = planes::EXECUTOR_INPUT.start(interrupt::SWI01);
            input_spawner.spawn(defmt::unwrap!(planes::input_overlay_task(
                buttons,
                input_plane,
                bus,
                planes::GESTURES.sender()
            )));
            info!("input plane: SWI01 interrupt executor @ P3 (preempts the map render); map plane: thread mode");
            planes::MapDisplay { bus, input_plane }
        };

        // ============= FLPR LS021 backend (default; ST7789 is `--features tft`) =============
        // The map plane owns the `Ls021Flpr` panel directly (it scans a whole frame per push, so there
        // is no partial-window overlay to serialise — no bus mutex). The M33 configures every line the
        // FLPR drives (held as outputs for the program's life); `com_task` + the gesture `input_task`
        // share the one high-priority executor (COM must keep alternating whatever the map plane does).
        //
        // ⚠️ These five P1 gate/BSP lines **must match `src/flpr/flpr_scan.c`'s masks** — confirm
        // each is broken out on your DK and remap all three together if not (the source bus, BCK, and
        // COM stay on P2).
        #[cfg(not(feature = "tft"))]
        let display = {
            // Gate + frame lines (P1) — GSP P1.00, GCK P1.01, GEN P1.12, INTB P1.10; held configured.
            // The DK breaks out only P1.00–14 (P1.02/03 are NFC, off-limits), which is one pin short
            // for everything on P1 — so SD `CS` moved to P0.00 (below), freeing P1.12 for GEN, and
            // INTB takes P1.10 (LED1 — it glows while a frame is drawing, a free activity indicator).
            let gate_bus = [
                Output::new(p.P1_00, Level::Low, OutputDrive::Standard), // GSP
                Output::new(p.P1_01, Level::Low, OutputDrive::Standard), // GCK
                Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN  (freed SD-CS pin)
                Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // INTB (LED1)
            ];
            // Source bus: BSP on P1.14 (the lone P1 source line), BCK + the 6 data lines on P2.
            let src_bus = [
                Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
                Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // BCK
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
                Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive),
                Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive),
                Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive),
            );
            #[cfg(feature = "com-hw")]
            let (vcom, vb, va) = {
                use embassy_nrf::gpiote::{OutputChannel, OutputChannelPolarity::Toggle};
                (
                    OutputChannel::new(p.GPIOTE20_CH0, p.P1_04, Level::Low, OutputDrive::HighDrive, Toggle),
                    OutputChannel::new(p.GPIOTE20_CH1, p.P1_05, Level::Low, OutputDrive::HighDrive, Toggle),
                    OutputChannel::new(p.GPIOTE20_CH2, p.P1_15, Level::Low, OutputDrive::HighDrive, Toggle),
                )
            };

            // Launch the FLPR (copy the blob, arm the control block, wait ALIVE), with **one full
            // relaunch retry** on failure (#349) — a one-off cold-boot race deserves a second
            // attempt before the device gives up on the panel. A launch failure must never fault —
            // degrade to a heartbeat idle, exactly like a missing/bad map card.
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

            // The resident RGB222 plane the app renders into and the FLPR packs to the wire, plus the
            // self-diffing present store the masked push derives its dirty rows from.
            // SAFETY: sole references to FB / ROW_DIFF; held by `panel` for the rest of the program (the
            // map plane is their only owner), never aliased.
            let fb: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
            let diff: &'static mut RowDiff<FRAME_H> = unsafe { &mut *core::ptr::addr_of_mut!(ROW_DIFF) };
            let mut panel = Ls021Flpr::new_fb(fb, diff);
            // Datasheet Initial #0: an INTB-framed all-black frame (FB boots zeroed = black) while COM is
            // still held `Lo`. Then T4 ≥ 30 µs, then start COM — from here it free-runs forever.
            panel.push_frame().await;
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

            let hp = planes::EXECUTOR_HP.start(interrupt::SWI01);
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
            hp.spawn(defmt::unwrap!(planes::input_task(buttons, input_plane, planes::GESTURES.sender())));
            info!("FLPR LS021: gesture/bulge plane on SWI01 @ P3; map plane: thread mode (event-driven, #219)");
            planes::MapDisplay {
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

        // Place the decoded-route-geometry cache in `.bss`, built in place (a zeroed
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary — like `MAP_CACHE`).
        // SAFETY: sole owner of ROUTE_CACHE; single map plane → no aliasing.
        #[cfg(has_map)]
        let route_cache: &RouteCache = unsafe { init_static(core::ptr::addr_of_mut!(ROUTE_CACHE), RouteCache::new()) };

        // The persistent settings store: takes the `RRAMC` peripheral, reads/writes the blob in the
        // carved RRAM page. Built here (where `p` is live) and moved into the ride loop, which seeds the
        // app at boot and saves on a settings edit. Every boot also bumps the persisted boot counter,
        // the diagnostics blob's one durable fact.
        let mut settings_store = settings::RramSettingsStore::new(p.RRAMC);
        let boot_count = settings_store.bump_boot_count(reset_reas);
        defmt::info!("boot #{=u32}", boot_count);

        // The hardware watchdog (#349): the last-resort net under both planes, fed by the ride
        // loop (gated on the input plane's heartbeat). Map builds only — the `ble` status build's
        // loop sleeps indefinitely by design and has no feeder. 24 s is generous on purpose: the
        // dog must never fire on a slow frame or a long SD reconcile, only on a genuine wedge. It
        // counts through sleep but **pauses under a debugger halt** (`HaltConfig::Pause`) so a
        // breakpoint doesn't cascade into a reset — and so probe-rs can flash with the dog live.
        // Once started a WDT can never be stopped; a warm reset carries it over, in which case
        // `try_new` re-adopts it if the config matches (ours is constant, so it does). A foreign
        // config (e.g. an older image's) can't be adopted or fed — log it and run unfed: the stale
        // period fires once and the next boot starts clean.
        #[cfg(has_map)]
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

        // The `ble` build's object store: the mounted card (`None` degrades route ops to typed errors,
        // config still works) + the RRAM settings both move in here; the BLE planes drive it. The status
        // screen keeps only the boot-time `sd` flag.
        #[cfg(all(feature = "ble", not(has_map)))]
        let (ble_store, sd_ok) = {
            let store = object_store::ObjectStore::new(storage, settings_store);
            let sd_ok = store.sd_ok();
            (store, sd_ok)
        };

        // --- The BLE stack, `ble` builds: group the peripheral claims (MPSL: GRTC CH7–11 + TIMER10/20
        // + TEMP + its PPI/PPIB lanes; SDC: the PPI10 fan-out + PPIB bridges; CRACEN for the LL's crypto
        // RNG) and build the never-returning stack future — polled from the tail join below. Nothing
        // here clashes with the rest of `main` (embassy's GRTC time driver allocates channels from CH0
        // up; TIMER10/20 and the PPI lanes are otherwise unused). ---
        #[cfg(all(feature = "ble", not(has_map)))]
        let ble_fut = {
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
            ble::run(_spawner, mpsl_p, sdc_p, p.CRACEN, ble_store)
        };

        // Hand the built display + the resident set to the shared, backend-agnostic ride loop. The
        // `display` (one of the two `MapDisplay` definitions) is the only per-backend value crossing this
        // seam; the loop drives present through it with no further `#[cfg]`. `cam_center` is threaded
        // only on the `synth` build (the host feed + the real GPS stream absolute positions, so they need
        // no synthetic-loop centre).
        #[cfg(all(has_map, any(feature = "debug-uart", not(feature = "synth"))))]
        let app_fut = ride::run_app(
            display,
            app,
            &mut storage,
            map_tables,
            map_cache,
            route_cache,
            &mut led,
            settings_store,
            wdt_handle,
        );
        #[cfg(all(has_map, not(feature = "debug-uart"), feature = "synth"))]
        let app_fut = ride::run_app(
            display,
            app,
            &mut storage,
            map_tables,
            map_cache,
            route_cache,
            &mut led,
            settings_store,
            wdt_handle,
            (cam_lon, cam_lat),
        );
        #[cfg(all(has_map, not(feature = "ble")))]
        app_fut.await;
        // The LM20 shape (map + BLE in one image) needs the ride loop and the object plane to share the
        // SD card and the settings store — an arbitration that doesn't exist yet. The DK budget assert
        // already forbids the combo by size; this keeps the gap explicit.
        #[cfg(all(has_map, feature = "ble"))]
        compile_error!(
            "map + BLE in one image is the LM20 shape (#270): the A6 object plane and the ride loop must arbitrate SD/settings first"
        );
        // The `ble` DK build: the radio + the status UI, joined forever.
        #[cfg(not(has_map))]
        embassy_futures::join::join(ble_fut, status::run_status(display, sd_ok, &mut led)).await;
    }
}
