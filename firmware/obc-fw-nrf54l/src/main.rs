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
//
// `Band` is the frame-absolute draw view both map backends' `present_overlay` drawers paint the hold
// bulge into.
use obc_platform::Band;

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(feature = "tft")]
use display::Display;
use display::{DisplayDriver, OverlayRegion};
use embassy_executor::InterruptExecutor;
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
// The event-driven loop's wake select: `select3` over gesture / sensor / deadline on the map plane,
// `select` over a button edge / guard tick on the idle input plane — which the always-polling
// `debug-uart` input plane compiles out (the status loop's `select` keeps it on `ble` builds).
#[cfg(any(not(feature = "debug-uart"), not(has_map)))]
use embassy_futures::select::select;
use embassy_futures::select::select3;
// The status loop tells a BLE status edge from a gesture/animation wake by the select arm (the
// edge `Signal` is consumed by the await, so the arm is the information). `ble` builds only.
#[cfg(not(has_map))]
use embassy_futures::select::{Either, Either3};
use embassy_sync::channel::{Channel, Sender};
#[cfg(feature = "tft")]
use embassy_sync::mutex::Mutex;
use embassy_time::Instant;
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
// The map/ride half of obc-app lives only on `has_map` builds (the `ble` DK build compiles the map
// plane out); the input-plane/settings types below are every build's.
#[cfg(has_map)]
use obc_app::{App, AppState, RideClock, Sensors, TrackSink};
// `SettingsStore` (the load/save trait) is the ride loop's seam; the `ble` build's store lives
// inside `object_store` (which imports it itself).
#[cfg(has_map)]
use obc_app::SettingsStore;
use obc_app::{Gesture, InputClock, InputEvent, InputPlane, InputSource};
use obc_platform::{ButtonInput, RowDiff};
// The map render's framebuffer adapter (the status screen builds its own inside `ble.rs`).
#[cfg(has_map)]
use obc_platform::FbDevice64;
// The status loop polls the fuel gauge itself (`run_app` polls it through `Sensors`).
#[cfg(not(has_map))]
use obc_app::FuelGauge;
#[cfg(has_map)]
use obc_reader::{MapCache, MapTables, Reader};
#[cfg(has_map)]
use obc_render::zoom_for_mpp;
use obc_render::RenderStats;
// The ride loop's route types: the decoded-route-geometry cache, the resident per-route chunk index,
// and the streamed route reader the matcher + map render share.
#[cfg(has_map)]
use obc_route::{RouteCache, RouteIndex, RouteReader};
// The `synth`-build stand-in GPS: walks a slow square loop so a saved ride is a non-degenerate `.gpx`
// (the default streams the real SAM-M10Q; `debug-uart` a recorded host ride).
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
use obc_platform::SynthLocation;
// The real-sensor `Signal` sources: the `GpsLocation`/`BaroAltimeter`/`SensorTemp` ZSTs the ride loop
// polls, fed by `sensors::sensor_task`. Real-sensor build only, and only where the ride loop exists to
// poll them (`has_map`); the `ble` status build runs the sensor task but drains nothing here.
#[cfg(all(has_map, not(feature = "debug-uart"), not(feature = "synth")))]
use obc_platform::sensor_link;
// Battery fuel gauge: a fixed-level stand-in until the nPM1300 PMIC gauge is read.
use obc_platform::StubFuelGauge;

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
/// *floor* the assert enforces — the real stack is the residual `RAM − statics` (~34 KB on the default
/// build). Pinned to the **measured ~33 KB deep-route-load render peak** + margin, not a loose floor:
/// this is what fails a `ble` + map build on the 256 KB DK at compile time instead of letting it link
/// and overflow the stack on the first deep render.
#[cfg(has_map)]
const STACK_RESERVE: usize = 34 * 1024;
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

/// Scan the card's `/routes/*.obcr` catalog into the app's Route menu. Deliberately its **own
/// `#[inline(never)]` frame**: the ~5 KB [`Catalog`](obc_app::Catalog) (`Vec<RouteSummary,
/// MAX_ROUTES>`, 64 × ~84 B) lives here and is popped on return, so it never sits on `main`'s frame
/// *beneath* the long-lived [`run_app`] ride loop — where a resident 5 KB catalog would steal from the
/// deep route-load render path's stack and overflow the 256 KB part.
#[cfg(has_map)]
#[inline(never)]
fn load_routes(storage: &mut sd::Storage, app: &mut App) {
    let catalog = storage.scan_routes();
    app.set_routes(&catalog);
}

/// Stack high-water guard: [`paint`] fills the free stack with a sentinel early in `main`; [`used`]
/// then reports the deepest reach by finding the lowest still-painted word (the stack runs
/// `_stack_start` top → `_stack_end` bottom, and a deep call overwrites the sentinel). The ride loop
/// logs only on a *new* peak, so it's silent once warm but flags any future change that creeps the deep
/// route-load render toward the 256 KB-DK's ~36 KB ceiling. Cheap (one boot paint + a per-frame scan).
mod stackmeter {
    const PAINT: u32 = 0xC0DE_DEAD;
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
    }
    /// Bytes of stack used at the deepest point reached so far (the high-water mark).
    pub fn used() -> usize {
        let (top, bottom) = (top(), bottom());
        let mut a = bottom;
        while a < top {
            if unsafe { (a as *const u32).read_volatile() } != PAINT {
                break;
            }
            a += 4;
        }
        top - a
    }
    /// Total usable stack (`_stack_start - _stack_end`).
    pub fn total() -> usize {
        top() - bottom()
    }
}

// ============================ Two-plane input + overlay ============================
// The map render (`render_map` + the banded push) is a CPU- and SPI-bound call that would block its
// executor for tens of ms. To keep input + the hold bulge responsive *during* that, the device runs
// two planes around one shared SPI panel:
//   - Map plane (thread mode, the `main` loop): drains the gesture channel → `apply_gesture`,
//     advances screen animations, and re-renders the map only on `dirty.map`, compositing the live
//     bulge into each pushed band.
//   - Input plane (`input_overlay_task`, on a high-priority `InterruptExecutor` pended from SWI01):
//     samples the buttons, recognises gestures (into the channel), and re-pushes just the right-edge
//     overlay window so the bulge animates over a static map at full FPS with no map re-render.
// The shared resource is the panel SPI bus + the framebuffer: the async `BUS` mutex serialises pushes
// — and, since the map render runs inside it, the framebuffer write against the input plane's window
// read — without disabling interrupts (the GRTC time-driver + the input executor keep running while
// it's held). Keeping the framebuffer *inside* the mutex means the input plane never reads a
// half-rendered frame, so the bulge backdrop is always clean (no tearing); the cost is that a long map
// render holds the bus, so the bulge can briefly "stick" while a big segment repaints. The `InputPlane`
// both planes draw the bulge from is behind a brief blocking mutex; lock order is always BUS-outer,
// INPUT_PLANE-inner.

/// Bound of the input→map gesture channel. One frame yields a couple of gestures and the map plane
/// drains it each loop, so even across a slow map push it never fills; `try_send` drops on the
/// (unreachable) overflow rather than block the high-priority plane.
const GESTURE_QUEUE: usize = 16;

/// Recognised gestures flowing from the input plane (high priority) to the map plane (thread mode) —
/// the only lock-free shared state between the two planes.
static GESTURES: Channel<CriticalSectionRawMutex, Gesture, GESTURE_QUEUE> = Channel::new();

/// The high-priority executor the input/overlay plane runs on, pended from the SWI01 vector (SWI00 is
/// MPSL's low-prio lane on `ble` builds, so every build pends from SWI01). The FLPR build uses
/// [`EXECUTOR_HP`] instead (it also free-runs COM there).
#[cfg(feature = "tft")]
static EXECUTOR_INPUT: InterruptExecutor = InterruptExecutor::new();

/// SWI01 ISR → poll the input-plane executor. SWI01 has no peripheral; we only borrow its interrupt
/// vector as the executor's pend line.
#[cfg(feature = "tft")]
#[interrupt]
unsafe fn SWI01() {
    EXECUTOR_INPUT.on_interrupt();
}

/// The FLPR build's single high-priority executor: it free-runs **both** the COM driver (which must
/// keep alternating `VCOM`/`VB`/`VA` so the panel never DC-biases, whatever the map plane is doing)
/// **and** the gesture-input plane (so button latency stays exact during a ~44 ms full-frame scan —
/// the M33 now *awaits* that scan (#347), but a deep map render still occupies thread mode).
/// Pended from the same SWI01 vector @ P3.
#[cfg(not(feature = "tft"))]
static EXECUTOR_HP: InterruptExecutor = InterruptExecutor::new();

#[cfg(not(feature = "tft"))]
#[interrupt]
unsafe fn SWI01() {
    EXECUTOR_HP.on_interrupt();
}

/// Input-plane loop period (ms): buttons sampled + gestures recognised + the bulge animated this
/// often, on the high-priority executor that preempts the map render — so press-to-feedback latency
/// and the auto-repeat cadence stay exact regardless of how long a map frame takes.
const LOOP_MS: u64 = 8;

/// Insurance re-poll cadence (ms) for the **idle** input plane: once every button is released +
/// settled, the plane sleeps on a button falling edge ([`ButtonInput::wait_for_any_press`]) instead of
/// polling at [`LOOP_MS`], so a parked device burns no CPU sampling unchanging pins. This long guard
/// wakes it occasionally regardless, so a missed edge can never strand the UI.
#[cfg(not(feature = "debug-uart"))]
const IDLE_REPOLL_MS: u64 = 30_000;

// ── Hardware watchdog (#349): the last-resort net under a wedged plane. The ride loop feeds it,
// gated on the input plane's heartbeat, so **either** plane wedging trips the dog — not just
// thread mode staying alive. Deliberately generous: it must never fire on a slow frame or a deep
// SD reconcile, only on a genuine wedge. ──
/// Watchdog period: 24 s of 32768 Hz LFCLK ticks (the issue's 16–30 s band).
#[cfg(has_map)]
const WDT_TIMEOUT_TICKS: u32 = 24 * 32768;
/// Cap (ms) on the ride loop's event-driven sleep, ~WDT/2 — an otherwise-idle device still wakes
/// to feed the dog. One extra wake per ~12 s is negligible next to [`IDLE_REPOLL_MS`].
#[cfg(has_map)]
const WDT_FEED_CAP_MS: u32 = 12_000;
/// How stale [`INPUT_HB_MS`] may be before the ride loop **withholds** the feed. The idle input
/// plane legitimately sleeps [`IDLE_REPOLL_MS`] (30 s) between stamps, so the window is 2× that
/// plus margin — no false trip on a parked device; a wedged input plane trips the dog within
/// roughly this window + the WDT period (~90 s worst case, fine for a last resort).
#[cfg(has_map)]
const INPUT_HB_STALE_MS: u32 = 65_000;
/// The input plane's liveness heartbeat: `Instant` millis of its last recognizer pass / idle wake,
/// stamped by [`input_task`] / [`input_overlay_task`] and read by the ride loop's watchdog feed.
static INPUT_HB_MS: AtomicU32 = AtomicU32::new(0);

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
#[cfg(all(has_map, not(feature = "debug-uart"), not(feature = "synth")))]
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

// The hold-bulge's right-edge overlay **columns**. Both bulges erupt from the right screen edge ≤12 px
// deep, so this fixed 16-px column band bounds them with margin. Both map panels re-present the bulge
// through `DisplayDriver::present_overlay` over the clean framebuffer, addressing only the live bulge's
// *rows* (`InputPlane::overlay_rows`: encoder ≈ 59–171, Back ≈ 182–246) — ST7789 a 16-px column window,
// the FLPR the full-width rows of that span — so the column constants are shared; the fixed row band
// (`OVL_Y0`/`OVL_ROWS`) is only the ST7789 trailing-clear + band-fit bound.
/// First overlay column: the rightmost 16 px (bulge depth ≤12 + margin).
const OVL_X0: u16 = (FRAME_W - 16) as u16;
/// Overlay window width (columns).
const OVL_W: u16 = 16;
/// First overlay row of the full hint band (a little above the encoder bulge's top). ST7789-only — the
/// FLPR addresses the *live* bulge's rows (`InputPlane::overlay_rows`), never this fixed top.
#[cfg(feature = "tft")]
const OVL_Y0: u16 = 56;
/// Full hint-band height in rows (down past the Back bulge's bottom) — the ST7789 trailing-clear span
/// and the band-fit bound below. The FLPR has its own `MAX_OVERLAY_*` bound in `Ls021Flpr`.
#[cfg(feature = "tft")]
const OVL_ROWS: u16 = 192;
/// ST7789: the overlay window must fit the shared band scratch (it borrows a prefix of it). The FLPR
/// path has its own `MAX_OVERLAY_*` bound in `Ls021Flpr::push_overlay`.
#[cfg(feature = "tft")]
const _: () = assert!(OVL_W as usize * OVL_ROWS as usize <= FRAME_W * BAND_ROWS, "overlay window larger than BAND");

// The live-bulge "present the rows *around* it" discipline lives **inside** the self-diffing present:
// the map plane passes the bulge's row span to the seam's `DisplayDriver::present(exclude)`, which
// clips it out of the changed-row spans it pushes (`obc_platform::RowDiff::diff_clipped`), leaving
// those rows for the overlay plane (`MapDisplay::present_bulge` on the FLPR, `input_overlay_task` on
// the ST7789).

/// Chains two input sources for the gesture recogniser: drains `a` (the physical buttons) fully,
/// then `b` (the VCOM-injected `K` events with `debug-uart`, else [`NullInput`]). So a host can
/// drive the UI (taps/holds) over the same VCOM link, interleaved with real presses.
struct ChainedInput<'a> {
    a: &'a mut dyn InputSource,
    b: &'a mut dyn InputSource,
}
impl InputSource for ChainedInput<'_> {
    fn poll(&mut self) -> Option<InputEvent> {
        self.a.poll().or_else(|| self.b.poll())
    }
}

/// A never-yielding input source — the `debug-uart`-off stand-in for the VCOM-injected stream, so
/// the recogniser call site is one code path in both builds.
#[cfg(not(feature = "debug-uart"))]
struct NullInput;
#[cfg(not(feature = "debug-uart"))]
impl InputSource for NullInput {
    fn poll(&mut self) -> Option<InputEvent> {
        None
    }
}

/// The VCOM-injected input stream to chain after the physical buttons: the `debug-uart` source that
/// drains host-injected turns/edges (`K` lines), or [`NullInput`] when the feature is off. One
/// helper so the input plane builds it the same `cfg` way regardless.
fn debug_input() -> impl InputSource {
    #[cfg(feature = "debug-uart")]
    return obc_platform::debug_link::DebugInput;
    #[cfg(not(feature = "debug-uart"))]
    NullInput
}

/// A `no_std` [`Clock`](obc_render::Clock) over embassy's monotonic `Instant`, in microseconds — the
/// time base for the map render's per-stage timing (collect / sort / draw) the VCOM telemetry
/// carries. The same monotonic clock the loop's frame `Instant` reads, so the stages reconcile.
#[cfg(has_map)]
struct InstantClock;
#[cfg(has_map)]
impl obc_render::Clock for InstantClock {
    fn now_us(&self) -> u64 {
        Instant::now().as_micros()
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

/// The input + overlay plane. Runs on [`EXECUTOR_INPUT`], preempting the thread-mode map render:
/// every [`LOOP_MS`] it samples the buttons, recognises gestures (pushing each into [`GESTURES`] for
/// the map plane to apply), and — when the hold bulge changed — re-pushes just the right-edge overlay
/// window: read its columns back from the framebuffer (RGB222→RGB565) and composite the bulge over
/// them. No renderer scratch, no map re-render → the bulge animates fluidly over a static map. The
/// panel is reached only through `bus`, the recogniser/overlay only through `input_plane`.
#[cfg(feature = "tft")]
#[embassy_executor::task]
async fn input_overlay_task(
    mut buttons: ButtonInput<Input<'static>>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    bus: &'static Mutex<CriticalSectionRawMutex, Display>,
    gestures: Sender<'static, CriticalSectionRawMutex, Gesture, GESTURE_QUEUE>,
) {
    // Native RGB565 panel → identity colour map (same as the map plane).
    let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
    loop {
        let now = Instant::now().as_millis() as u32;
        INPUT_HB_MS.store(now, Ordering::Relaxed); // liveness stamp the ride loop's WDT feed gates on
        buttons.update(now);
        // Recognise this frame's input under the shared InputPlane lock (a brief critical section,
        // never held across an await/push). Each gesture is pushed to the map plane; the bulge is
        // advanced regardless, so the press is confirmed on screen below even before the map plane
        // drains the channel.
        let (dirty, overlay_span, overlay_active) = input_plane.lock(|cell| {
            let plane = &mut *cell.borrow_mut();
            // Physical buttons + (with `debug-uart`) the VCOM-injected `K` events, drained into one
            // recogniser pass — so a host can drive taps/holds like the real buttons.
            let mut dbg = debug_input();
            let mut input = ChainedInput { a: &mut buttons, b: &mut dbg };
            plane.recognize(InputClock(now), &mut input, |g| {
                if gestures.try_send(g).is_err() {
                    defmt::warn!("gesture channel full — dropped a gesture (map plane stalled?)");
                }
            });
            // `overlay_active` (the bulge is still live, incl. its retract) gates the idle sleep below.
            (plane.take_overlay_dirty(), plane.overlay_rows(FRAME_W as i32, FRAME_H as i32), plane.overlay_active())
        });

        // Repaint the bulge only when it changed (plus the one trailing clear `take_overlay_dirty`
        // reports): re-present just the right-edge region over a static map through the seam — while
        // live, **only the active bulge's rows**, and the full band on the trailing clear to wipe the
        // last bulge. `present_overlay` fills the window from the clean framebuffer + composites the
        // bulge (the `InputPlane` lock is taken once, inside the drawer). Awaiting the bus yields to the
        // (thread-mode) map plane if it is mid-frame, so this never spins.
        //
        // Dev-only-best-effort: the trailing clear is the **one** frame `take_overlay_dirty` flags, not
        // a retry-until-acked loop like the FLPR map plane's `last_overlay_span` clear — the coordination
        // isn't hardened the way the shipping FLPR path is (this is the opt-in `tft` bring-up backend).
        if dirty {
            let region = match overlay_span {
                Some((y0, rows)) => OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows },
                None => OverlayRegion { x0: OVL_X0, y0: OVL_Y0, w: OVL_W, rows: OVL_ROWS },
            };
            bus.lock()
                .await
                .present_overlay(region, &mut |band: &mut Band| {
                    input_plane
                        .lock(|cell| cell.borrow().render_overlay(band, FRAME_W as f32, FRAME_H as f32, color_fn));
                })
                .await;
        }
        // Event-driven sleep (issue #219): idle (all buttons released + settled, no live bulge) → sleep
        // on a button falling edge; otherwise keep the 8 ms poll so debounce / auto-repeat / the bulge
        // animation stay exact. `debug-uart` always polls (prompt host-injected input).
        #[cfg(feature = "debug-uart")]
        {
            let _ = overlay_active;
            Timer::after_millis(LOOP_MS).await;
        }
        #[cfg(not(feature = "debug-uart"))]
        if buttons.is_idle() && !overlay_active {
            let _ = select(buttons.wait_for_any_press(), Timer::after_millis(IDLE_REPOLL_MS)).await;
        } else {
            Timer::after_millis(LOOP_MS).await;
        }
    }
}

/// The FLPR build's input plane: recognises gestures + animates the hold bulge. Runs on [`EXECUTOR_HP`]
/// beside COM, preempting the thread-mode map render, so press latency + the auto-repeat cadence stay
/// exact across a deep map render. Each [`LOOP_MS`] it samples the buttons + (with
/// `debug-uart`) the VCOM-injected `K` events and recognises gestures into [`GESTURES`] for the map
/// plane to apply — **under the shared [`InputPlane`] lock**, so the live hold-bulge state it advances
/// is the same one the map plane composites into its partial overlay push.
///
/// Unlike the ST7789 plane this task does **not** push to glass: the FLPR scans whole frames, so the
/// *map plane* owns every push. This task is purely the recogniser; the brief lock is never held across
/// the `await`.
#[cfg(not(feature = "tft"))]
#[embassy_executor::task]
async fn input_task(
    mut buttons: ButtonInput<Input<'static>>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    gestures: Sender<'static, CriticalSectionRawMutex, Gesture, GESTURE_QUEUE>,
) {
    loop {
        let now = Instant::now().as_millis() as u32;
        INPUT_HB_MS.store(now, Ordering::Relaxed); // liveness stamp the ride loop's WDT feed gates on
        buttons.update(now);
        // Recognise + animate the bulge under the shared lock (a brief critical section, never held
        // across the await), so the bulge state the map plane composites is the one this advanced.
        // Physical buttons + (with `debug-uart`) the VCOM-injected `K` events, one recogniser pass.
        // Also read whether the hold bulge is still live (charging / popping / retracting): the input
        // plane must keep animating it even after the button is released, so it gates the idle sleep.
        let overlay_active = input_plane.lock(|cell| {
            let plane = &mut *cell.borrow_mut();
            let mut dbg = debug_input();
            let mut input = ChainedInput { a: &mut buttons, b: &mut dbg };
            plane.recognize(InputClock(now), &mut input, |g| {
                if gestures.try_send(g).is_err() {
                    defmt::warn!("gesture channel full — dropped a gesture (map plane stalled?)");
                }
            });
            plane.overlay_active()
        });
        // Event-driven sleep (issue #219): once every button is released + settled and no bulge is
        // animating, sleep on a button falling edge instead of polling — a parked device burns no CPU
        // here. While a button is down / debouncing / repeating, or a bulge is live, keep the 8 ms poll
        // so debounce + auto-repeat + the bulge animation stay exact. The `debug-uart` dev build always
        // polls so host-injected `K` input is seen promptly (power isn't the concern there).
        #[cfg(feature = "debug-uart")]
        {
            let _ = overlay_active;
            Timer::after_millis(LOOP_MS).await;
        }
        #[cfg(not(feature = "debug-uart"))]
        if buttons.is_idle() && !overlay_active {
            let _ = select(buttons.wait_for_any_press(), Timer::after_millis(IDLE_REPOLL_MS)).await;
        } else {
            Timer::after_millis(LOOP_MS).await;
        }
    }
}

// ============================ The de-cfg'd map plane ============================
// The ride loop drives the screen through **one** handle, [`MapDisplay`], so [`run_app`] carries no
// per-backend `#[cfg]`. `MapDisplay` is one name with two `cfg`-selected definitions — the only place
// the backends diverge — each exposing the same three methods the loop calls:
//   - `poll_overlay`     — this frame's hold-bulge state (dirty edge + live row span);
//   - `render_present`   — render the clean frame into the framebuffer + push it to glass;
//   - `present_bulge`    — re-present the hold bulge over the clean map.
// The genuine asymmetry they hide: the ST7789 shares its bus with the input/overlay plane (which owns
// the bulge re-push) so its map loop has no overlay work; the FLPR owns the panel outright and pushes
// the bulge itself from the map plane. Everything else in the loop is shared.

/// What [`MapDisplay::render_present`] reports for one map frame: whether the push reached glass
/// (`false` → a transport fault to retry, #66), the render's [`RenderStats`], and the render / push
/// timings (µs) the RTT log + the VCOM telemetry carry.
struct FramePresent {
    ok: bool,
    // Read by the ride loop's telemetry/log lines only — the status build presents text frames
    // whose stats are all `default()`, so it never looks.
    #[cfg_attr(not(has_map), allow(dead_code))]
    stats: RenderStats,
    render_us: u64,
    push_us: u64,
}

/// ST7789 (`--features tft`): the map plane's handle is the `&'static` bus mutex it shares with the
/// input/overlay plane, plus the shared `InputPlane` it *reads* the live bulge span + hold progress
/// from. That plane still owns the hold-bulge re-push (and the one-shot `overlay_dirty` edge that
/// drives it); the map loop only samples the read-only state so its present can go around a live
/// bulge.
#[cfg(feature = "tft")]
struct MapDisplay {
    bus: &'static Mutex<CriticalSectionRawMutex, Display>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
}

#[cfg(feature = "tft")]
impl MapDisplay {
    /// The live bulge's row span, read-only, so `render_present` excludes it from the push. The dirty
    /// edge stays `false`: `take_overlay_dirty` is the one-shot edge the input/overlay plane
    /// (`input_overlay_task`) consumes to drive the bulge re-push — the map loop must not steal it
    /// (its `present_bulge` is a no-op here).
    #[inline(always)]
    fn poll_overlay(&mut self) -> (bool, Option<(u16, u16)>) {
        (false, self.input_plane.lock(|c| c.borrow().overlay_rows(FRAME_W as i32, FRAME_H as i32)))
    }

    /// Lock the shared bus, render the clean frame into the framebuffer through the seam, and push the
    /// changed rows to GRAM — going around a live bulge's rows (`overlay_span`), so a redraw landing
    /// mid-hold no longer flashes the bulge off. Dropping the guard on return lets the input plane
    /// push its bulge again. `#[inline(always)]` + a generic (non-`dyn`) `render` so the deep render
    /// folds into the caller's frame rather than nesting another (the stack regression — see
    /// [`run_app`]).
    #[inline(always)]
    async fn render_present(
        &mut self,
        overlay_span: Option<(u16, u16)>,
        mut render: impl FnMut(&mut dyn DisplayDriver) -> RenderStats,
    ) -> FramePresent {
        let mut guard = self.bus.lock().await;
        let t_render = Instant::now();
        // `render` reaches the framebuffer through the seam's one dyn-safe method (`fb_mut`); the
        // async present below is called on the concrete backend (`where Self: Sized`).
        let stats = render(&mut *guard);
        let render_us = t_render.elapsed().as_micros();
        let t_push = Instant::now();
        let ok = guard.present(overlay_span).await;
        let push_us = t_push.elapsed().as_micros();
        FramePresent { ok, stats, render_us, push_us }
    }

    /// No-op: the ST7789 hold bulge is pushed by the input/overlay plane, not the map loop.
    #[inline(always)]
    async fn present_bulge(&mut self, _span: Option<(u16, u16)>, _dirty: bool) {}

    /// No-op: the #349 relaunch escalation is FLPR-only (there is no coprocessor to relaunch —
    /// a wedged ST7789 bus has no recovery lever beyond the retry the present already latches).
    #[inline(always)]
    fn take_relaunch_repaint(&mut self) -> bool {
        false
    }

    /// Never degraded: see [`take_relaunch_repaint`](Self::take_relaunch_repaint).
    #[cfg_attr(not(has_map), allow(dead_code))]
    #[inline(always)]
    fn degraded(&self) -> bool {
        false
    }

    /// The live encoder hold-progress from the shared input plane (0.0–1.0), so the in-screen confirm
    /// fills (the factory-Reset bar) track the hold on this backend too.
    #[inline(always)]
    fn hold_progress(&self) -> f32 {
        self.input_plane.lock(|c| c.borrow().encoder_hold_progress())
    }
}

/// Consecutive failed presents that trigger one FLPR relaunch (#349): each failure already costs a
/// full frame-deadline spin inside the transport (250 ms), so three in a row (~0.75 s) is far past any
/// transient — the FLPR is wedged, escalate.
#[cfg(not(feature = "tft"))]
const PUSH_FAILS_PER_RELAUNCH: u8 = 3;
/// Consecutive relaunches that may fail (the launch erroring, or the presents after it still timing
/// out) before the device stops touching the FLPR and degrades to the heartbeat idle (#349).
#[cfg(not(feature = "tft"))]
const MAX_CONSEC_RELAUNCHES: u8 = 3;

/// FLPR LS021 (default): the map plane owns the panel outright (whole-frame scan per push → no shared
/// bus), plus the shared `InputPlane` it composites the bulge from and the gate/source GPIO lines it
/// must keep driven for the program's life.
#[cfg(not(feature = "tft"))]
struct MapDisplay {
    panel: Ls021Flpr<'static>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    /// The last live bulge's rows, so the trailing clear wipes exactly them, not the whole hint band.
    last_overlay_span: Option<(u16, u16)>,
    /// Consecutive failed pushes (map presents **and** bulge pushes — a bulge-only wedge must
    /// escalate too) since the last success; [`PUSH_FAILS_PER_RELAUNCH`] of them fire a relaunch.
    push_fails: u8,
    /// Relaunches run without a successful push in between; [`MAX_CONSEC_RELAUNCHES`] of them
    /// degrade the device. Cleared by any push that reaches glass.
    consec_relaunches: u8,
    /// A relaunch landed → the ride loop must fold in a full map repaint (`take_relaunch_repaint`).
    relaunch_repaint: bool,
    /// Terminal (until power-cycle): the FLPR would not come back after [`MAX_CONSEC_RELAUNCHES`]
    /// attempts. All pushes become no-ops (each would cost a frame-deadline spin against a dead
    /// core); the ride loop drops to the heartbeat idle. COM + the M33-held panel GPIOs keep the
    /// glass DC-bias-safe throughout — see [`relaunch_flpr`]'s doc.
    degraded: bool,
    /// The gate + source lines the FLPR drives — held only to keep them configured as outputs for the
    /// program's life (never touched after launch); dropping them would float the panel.
    _gate_bus: [Output<'static>; 4],
    _src_bus: [Output<'static>; 8],
    /// The zero-CPU hardware COM generator (`com-hw` build): held for the program's life like the
    /// gate/source buses — dropping it would stop the toggle and let the panel DC-bias. The default DK
    /// build has no field here (the M33 `com_task` owns the COM pins instead).
    #[cfg(feature = "com-hw")]
    _com_hw: HwCom,
}

#[cfg(not(feature = "tft"))]
impl MapDisplay {
    /// Sample the shared `InputPlane` once per frame (the map plane is the sole owner of the FLPR
    /// overlay bookkeeping): the dirty edge (live while the bulge animates, plus one trailing clear)
    /// and the live bulge's **row span** (`None` when quiet), so the map present can go *around* it and
    /// `present_bulge` can re-present it.
    #[inline(always)]
    fn poll_overlay(&mut self) -> (bool, Option<(u16, u16)>) {
        self.input_plane.lock(|c| {
            let p = &mut *c.borrow_mut();
            (p.take_overlay_dirty(), p.overlay_rows(FRAME_W as i32, FRAME_H as i32))
        })
    }

    /// The live encoder hold-progress from the shared input plane (0.0–1.0). Fed to the map render
    /// so the in-screen confirm fills (the factory-Reset bar) track the hold — `App`'s own input
    /// plane isn't driven on the two-plane firmware, so without this the bar never fills. (The
    /// status build has no in-screen fills, so only the ride loop calls it.)
    #[cfg_attr(not(has_map), allow(dead_code))]
    #[inline(always)]
    fn hold_progress(&self) -> f32 {
        self.input_plane.lock(|c| c.borrow().encoder_hold_progress())
    }

    /// Render the clean frame into the owned panel and **self-diff** it to glass: push only the rows
    /// that changed since the last present. With a live bulge, the seam's `present(exclude)` clips its
    /// rows out (`overlay_span`) and leaves them for `present_bulge` — the FLPR's ~44 ms full-frame
    /// scan would otherwise blank the bulge for that whole scan (the pop-flicker), and even a partial
    /// clean push would flash it off. No shared bus: the map plane owns every push here. Marked
    /// `#[inline(always)]` with a generic (non-`dyn`) `render` so the deep render folds into the
    /// caller's frame rather than nesting another (the stack regression).
    #[inline(always)]
    async fn render_present(
        &mut self,
        overlay_span: Option<(u16, u16)>,
        mut render: impl FnMut(&mut dyn DisplayDriver) -> RenderStats,
    ) -> FramePresent {
        let t_render = Instant::now();
        let stats = render(&mut self.panel);
        let render_us = t_render.elapsed().as_micros();
        if self.degraded {
            // Terminal FLPR-down mode (#349): don't spin a frame deadline against a dead core —
            // drop the frame, reporting `ok` so the caller doesn't latch an endless retry. The
            // ride loop has already dropped (or is about to drop) to the heartbeat idle; the `ble`
            // status build keeps its radio useful with the glass frozen on the last good frame.
            return FramePresent { ok: true, stats, render_us, push_us: 0 };
        }
        let t_push = Instant::now();
        // Self-diffing present through the seam, clipped around a live bulge's rows so
        // `present_bulge` owns them (issue #163/#201/#345). The await frees the M33 for the whole
        // scan (#347) — and suspending the map plane here is exactly what guarantees the
        // framebuffer stays untouched while the FLPR reads it.
        let ok = self.panel.present(overlay_span).await;
        if !ok {
            // The push didn't reach glass (a stalled FLPR), but the self-diffing present already
            // advanced its row-hash store to this frame — so the caller's latched `pending_map_redraw`
            // retry would diff the identical `fb` against an up-to-date store and re-push *nothing*,
            // stranding the rows that missed glass. Re-arm a full push so the retry re-seeds the store
            // and repaints every row.
            self.panel.reset_diff();
        }
        let push_us = t_push.elapsed().as_micros();
        self.note_push(ok).await;
        FramePresent { ok, stats, render_us, push_us }
    }

    /// Present the hold bulge over the clean map (the FLPR bulge rides this map plane — no shared SPI
    /// bus to serialise against). While the bulge is live this re-composites its rows every frame (the
    /// map present clipped them out via its `exclude`, so the fresh backdrop + bulge land here — no
    /// mid-pop flash). Only the active bulge's rows are touched (the FLPR fast-forwards the gate to them
    /// + early-stops).
    ///
    /// The trailing clear (bulge just went quiet) wipes **the same rows** the last bulge used, because
    /// the self-diffing map present no longer guarantees it touched those rows: the bulge composited
    /// glass content the row-hash diff can't see (the store tracks the clean `fb`), so if the map
    /// content there is unchanged the diff skips it and the stale bulge would strand without this clear.
    /// The clear re-pushes the clean `fb` rows, which the store already agrees with, so the next present
    /// stays quiet there. It is driven off [`last_overlay_span`](Self#) (cleared only on a **successful**
    /// push), not the one-shot `overlay_dirty` edge — so a one-frame FLPR stall during the clear is
    /// retried on the next frame rather than stranding the bulge with no edge left to re-fire it.
    #[inline(always)]
    async fn present_bulge(&mut self, overlay_span: Option<(u16, u16)>, overlay_dirty: bool) {
        let _ = overlay_dirty; // `last_overlay_span` drives the clear so a stalled clear retries — see the doc.
        if self.degraded {
            return; // FLPR down for good (#349) — no push to retry against.
        }
        if let Some((y0, rows)) = overlay_span {
            let t_push = Instant::now();
            let ok = Self::composite_push(&mut self.panel, self.input_plane, y0, rows).await;
            let push_us = t_push.elapsed().as_micros();
            self.last_overlay_span = Some((y0, rows));
            if ok {
                // Per-tick during a hold — `debug` so it doesn't flood the default log.
                defmt::debug!("overlay frame: bulge push {=u64} us ({=u16} rows @ y{=u16})", push_us, rows, y0);
            } else {
                defmt::warn!("overlay frame: bulge push failed (FLPR stalled?) — retrying next overlay tick");
            }
            self.note_push(ok).await;
        } else if let Some((y0, rows)) = self.last_overlay_span {
            // Trailing clear: re-present just the last bulge's rows with nothing composited = the clean
            // map restored under the just-gone bulge (the self-diffing map present may have skipped
            // them, so this is what actually wipes the bulge — see the method docs). Drop
            // `last_overlay_span` only when the push lands, so a stalled FLPR retries next frame.
            let ok = Self::composite_push(&mut self.panel, self.input_plane, y0, rows).await;
            if ok {
                self.last_overlay_span = None;
            } else {
                defmt::warn!("overlay frame: trailing clear failed (FLPR stalled?) — retrying next frame");
            }
            self.note_push(ok).await;
        }
    }

    /// One overlay composite + push of the bulge band's rows `[y0, y0+rows)` through the seam —
    /// shared by the live-bulge repaint and the trailing clear above. An associated fn (not a
    /// closure — closures can't await) taking the panel + plane apart so `present_bulge` can call
    /// it around its `&mut self` borrows.
    #[inline(always)]
    async fn composite_push(
        panel: &mut Ls021Flpr<'static>,
        input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
        y0: u16,
        rows: u16,
    ) -> bool {
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        panel
            .present_overlay(OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows }, &mut |band: &mut Band| {
                input_plane.lock(|cell| cell.borrow().render_overlay(band, FRAME_W as f32, FRAME_H as f32, color_fn));
            })
            .await
    }

    /// Fold one push outcome into the **relaunch escalation** (#349) — every FLPR push (map present,
    /// bulge, trailing clear) reports here. A success clears both counters; the
    /// [`PUSH_FAILS_PER_RELAUNCH`]th consecutive failure runs a full [`relaunch_flpr`] (the failing
    /// push already logged its `dump_flpr_state` snapshot — hung vs reset vs corrupted shared RAM).
    /// When [`MAX_CONSEC_RELAUNCHES`] relaunches pass without a single successful push in between,
    /// the escalation stops for good: `degraded` latches, every later push becomes a no-op, and the
    /// ride loop drops to the heartbeat idle. **COM never stops either way** — it runs on the M33
    /// (`com_task` / `HwCom`), so the panel stays DC-bias-safe through a dead FLPR, a relaunch, and
    /// the degraded idle alike (see [`relaunch_flpr`]'s doc; that property is load-bearing).
    async fn note_push(&mut self, ok: bool) {
        if ok {
            self.push_fails = 0;
            self.consec_relaunches = 0;
            return;
        }
        self.push_fails += 1;
        if self.push_fails < PUSH_FAILS_PER_RELAUNCH {
            return;
        }
        self.push_fails = 0;
        if self.consec_relaunches >= MAX_CONSEC_RELAUNCHES {
            // The last K relaunches all failed to restore service (each proven by the next
            // N failed pushes, or by erroring outright) — stop pounding a dead core.
            self.degraded = true;
            defmt::error!(
                "FLPR: {=u8} consecutive relaunches failed — degrading to heartbeat idle (COM keeps the panel DC-bias-safe; power-cycle to retry)",
                MAX_CONSEC_RELAUNCHES
            );
            return;
        }
        self.consec_relaunches += 1;
        defmt::error!(
            "FLPR: {=u8} consecutive failed pushes — full relaunch (attempt {=u8}/{=u8})",
            PUSH_FAILS_PER_RELAUNCH,
            self.consec_relaunches,
            MAX_CONSEC_RELAUNCHES
        );
        match relaunch_flpr().await {
            Ok(()) => {
                // Fresh core, no frame history: the diff store may believe rows are on glass that
                // never landed — force the next present to repaint every row, and tell the ride
                // loop to schedule that present even if nothing else dirtied the map.
                self.panel.reset_diff();
                self.relaunch_repaint = true;
                defmt::info!("FLPR: relaunch OK — alive again, full repaint armed");
            }
            Err(e) => defmt::error!("FLPR: relaunch failed ({}) — escalating on the next failed pushes", e),
        }
    }

    /// One-shot: a relaunch landed since the last call, so the ride loop must fold in a full map
    /// repaint (the fresh FLPR has no frame history; the diff store was reset).
    #[inline(always)]
    fn take_relaunch_repaint(&mut self) -> bool {
        core::mem::take(&mut self.relaunch_repaint)
    }

    /// Terminal FLPR-down state (#349): [`MAX_CONSEC_RELAUNCHES`] relaunches failed. The ride loop
    /// checks this each pass and drops to the heartbeat idle. (The status build never calls it —
    /// there, a degraded display just freezes the glass while BLE keeps serving.)
    #[cfg_attr(not(has_map), allow(dead_code))]
    #[inline(always)]
    fn degraded(&self) -> bool {
        self.degraded
    }
}

/// The GPS power state the ride wants: deep-sleep when not tracking, full-power fixes while riding, or
/// the M10's low-power tracking when the `power_saver` toggle is on. Recomputed each frame in
/// [`run_app`] and pushed to the sensor task (via [`sensor_link::set_power`]) only on a change.
/// Real-sensor build only — the `synth` / `debug-uart` feeds have no power-managed receiver.
#[cfg(all(has_map, not(feature = "debug-uart"), not(feature = "synth")))]
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
#[cfg(has_map)]
#[allow(clippy::too_many_arguments)]
// `#[inline(always)]`: this is a single-call-site `-> !` future. Inlining folds it (and the present
// methods above) back into `main`'s frame — recovering the ~5 KB of stack the bare extraction cost
// (the deep route-load render then overran the 256 KB part's stack).
#[inline(always)]
async fn run_app(
    mut display: MapDisplay,
    app: &mut App,
    storage: &mut sd::Storage,
    map_tables: &MapTables,
    map_cache: &MapCache,
    route_cache: &RouteCache,
    led: &mut Output<'static>,
    // The persistent RRAM settings store (#193): seeds the app at boot, persists on a settings edit.
    mut settings_store: settings::RramSettingsStore,
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
    // `None` → defaults), then persist on any change the settings screens make.
    app.set_settings(settings_store.load().unwrap_or_default());

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
        let hw = stackmeter::used();
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
            let age = now.wrapping_sub(INPUT_HB_MS.load(Ordering::Relaxed));
            if age <= INPUT_HB_STALE_MS {
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

/// The `ble` status build's thread-mode plane — [`run_app`]'s deliberately dumb sibling: no map, no
/// ride, no SD reconcile. It paints the BLE status screen ([`ble::draw_status_screen`]) into the
/// resident framebuffer and presents it through the same [`MapDisplay`] seam, keeps the hold bulge
/// working (the input plane recognises + animates it exactly as on the map build), and sleeps
/// event-driven: a recognised gesture, a BLE link edge ([`ble::wait_status_change`]), or the short tick
/// while a bulge animates. Joined against [`ble::run`] on the thread-mode executor in `main`.
#[cfg(not(has_map))]
async fn run_status(
    mut display: MapDisplay,
    // Whether a card mounted at boot — a status line, never a fault. The card itself (and the RRAM
    // settings store) live in the BLE plane's `ObjectStore`.
    sd_ok: bool,
    led: &mut Output<'static>,
) -> ! {
    let mut fuel = StubFuelGauge::new(75);
    // The on-screen input counter — the dumb UI's visible ack that buttons + the input plane run beside
    // the radio (every recognised gesture bumps it; nothing navigates anywhere).
    let mut inputs: u32 = 0;
    let mut stack_hw = 0usize;
    let mut last_led = 0u32;
    let mut redraw = true; // boot: paint the first frame + seed the RowDiff store
    loop {
        let now = Instant::now().as_millis() as u32;
        let hw = stackmeter::used();
        if hw > stack_hw {
            stack_hw = hw;
            defmt::info!("stack high-water {=usize} / {=usize} B (new peak)", hw, stackmeter::total());
        }

        while GESTURES.try_receive().is_ok() {
            inputs += 1;
            redraw = true;
        }
        // A FLPR relaunch landed (#349): repaint the status screen in full (diff store was reset).
        // If instead the display *degraded*, presents become silent no-ops — the status build keeps
        // its radio useful with the glass frozen, rather than idling out a working BLE link.
        redraw |= display.take_relaunch_repaint();

        // This frame's hold-bulge state, exactly as the ride loop samples it — the status present
        // goes around a live bulge's rows and `present_bulge` re-composites them. Bulge pushes
        // FIRST, as in the ride loop (#348 follow-up): a fired hold's confirm pop must not queue
        // behind the status redraw it triggered.
        let (overlay_dirty, overlay_span) = display.poll_overlay();
        display.present_bulge(overlay_span, overlay_dirty).await;

        if redraw {
            let battery = fuel.poll().unwrap_or(0);
            ble::publish_battery(battery); // feed the BAS characteristic (A4) from the FuelGauge seam
            let render = |d: &mut dyn DisplayDriver| {
                ble::draw_status_screen(d.fb_mut(), battery, sd_ok, inputs);
                RenderStats::default()
            };
            let fp = display.render_present(overlay_span, render).await;
            redraw = !fp.ok; // a transport fault latches a retry, like the ride loop
            defmt::info!("status frame: render {=u64} us + push {=u64} us", fp.render_us, fp.push_us);
            // Re-composite the bulge rows the present's `exclude` skipped (see the ride loop's note).
            if overlay_span.is_some() {
                display.present_bulge(overlay_span, false).await;
            }
        }

        if now.wrapping_sub(last_led) >= 500 {
            led.toggle();
            last_led = now;
        }

        // Event-driven sleep: a gesture, a BLE link edge, or — while a bulge animates / a failed present
        // wants its retry — the short tick. The link-edge `Signal` is consumed by the await, so *which
        // arm fired* is the redraw signal.
        if overlay_dirty || overlay_span.is_some() || redraw {
            match select3(GESTURES.ready_to_receive(), ble::wait_status_change(), Timer::after_millis(LOOP_MS)).await {
                Either3::Second(_) => redraw = true,
                Either3::First(_) | Either3::Third(_) => {}
            }
        } else {
            match select(GESTURES.ready_to_receive(), ble::wait_status_change()).await {
                Either::Second(_) => redraw = true,
                Either::First(_) => {}
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
        load_routes(&mut storage, app);

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

            let input_spawner = EXECUTOR_INPUT.start(interrupt::SWI01);
            input_spawner.spawn(defmt::unwrap!(input_overlay_task(buttons, input_plane, bus, GESTURES.sender())));
            info!("input plane: SWI01 interrupt executor @ P3 (preempts the map render); map plane: thread mode");
            MapDisplay { bus, input_plane }
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
            cfg.timeout_ticks = WDT_TIMEOUT_TICKS;
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
        let app_fut = run_app(
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
        let app_fut = run_app(
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
        embassy_futures::join::join(ble_fut, run_status(display, sd_ok, &mut led)).await;
    }
}
