//! nRF54L15-DK board firmware for OpenBikeComputer — the **real hardware target**.
//!
//! The nRF54L15 + ST7789 EYESPI panel is what the project ships on. This crate ports the
//! shared `obc-app` onto it (load route → ride → save GPX on
//! glass, fake-sensor fed). Nothing app-facing lives here: `obc-render` / `obc-app` /
//! `obc-reader` / `obc-route` + `obc-platform` stay board-agnostic; only the nRF HAL
//! wiring + the ST7789 `DisplayDriver` backend are board-specific. See epic #120.
//!
//! Bring-up is phased so each hardware layer is verified (over defmt/RTT, and on glass via
//! the webcam capture at `/tmp/obc-cam/panel.jpg`) before the next is stacked:
//!   N0. crate skeleton + embassy bring-up: blinky + RTT + this peripheral plan          (#121)
//!   N1. display HAL + ST7789 SPIM backend + banded RGB565 push + font/colour demo (RGB222 at N4) (#122)
//!   N2. microSD on a dedicated SPIM (reuse obc-platform's FatFs byte adapters)            <- (#123)
//!   N3. board memory profile (host-vs-nRF) + budget assert                              (#124)
//!   N4. RGB222 full framebuffer + full map on glass (retire `Framebuffer565`)           (#125)
//!   N5. buttons + two-plane InterruptExecutor + fluid composite-on-push bulge               (#126)
//!   N6. debug/sensor stream over VCOM UART + load→ride→save-GPX = PARITY                 <- (#127)
//!   N7. docs + CI (add nRF to the docs + the required check)                            (#128)
//!
//! Clock: the M33 application core runs at 128 MHz; embassy-time is driven by the **GRTC**
//! (Global RTC) via the `time-driver-grtc` feature — the nRF54L has no legacy RTC time-driver.
//!
//! ============================ Peripheral / pin plan ============================
//! Pin names are the embassy-nrf `P{port}_{pin}` form (e.g. `P2_09` = GPIO port 2, pin 9).
//! LED/button/VCOM/SPI assignments are the nRF54L15-DK's, from Zephyr's `nrf54l15dk` DTS and
//! the DK HW user guide pin maps (Tables 3–5). The three GPIO ports have different reach: P2 =
//! MCU domain (fast, ≤64 MHz, the SERIAL00 home), P1 = PERI domain (≤8 MHz), P0 = LP domain.
//!
//! ## On-board LEDs (active-HIGH) — Zephyr `led0..3`
//!   LED0 P2_09 | LED1 P1_10 | LED2 P2_07 | LED3 P1_14
//! N1 blinks **LED0 (P2_09)** once per drawn frame as a liveness heartbeat.
//!
//! ## Push-buttons (active-LOW, internal pull-up) — Zephyr `sw0..3`, the UI input (#126)
//!   BTN0 P1_13 PREV | BTN1 P1_09 NEXT | BTN2 P1_08 BACK | BTN3 P0_04 SELECT
//! Map to obc-platform's board-agnostic `ButtonInput` debouncer → the shared gesture
//! recogniser, from four GPIO buttons. Roles (N5): BTN0/1 → encoder
//! Turn∓1, BTN3 → encoder press/hold, BTN2 → Back/back-hold (`ButtonInput::new` order is
//! prev, next, select, back). Read as plain **polled** `gpio::Input` (the debouncer samples
//! levels each loop — no GPIOTE async wait needed). These are the DK's own buttons — no
//! jumpers — and they stay free because the display lives on P2 (below).
//!
//! ## Display SPIM — ST7789 EYESPI stand-in (#122)
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
//!   leaves all of P1 free for SD (N2) + VCOM (N6) + the buttons. Band push expands the RGB222
//!   framebuffer → RGB565 and SPIM-DMAs a CASET/RASET window (the wire-pack lives in the ST7789
//!   backend's `flush_window`, behind the board-agnostic `DisplayDriver` seam the FLPR/LS021B7DD02
//!   backend implements too); the RGB222 source framebuffer lands at N4. N1 first drove that seam
//!   (in RGB565) with a banded font/colour
//!   bring-up screen — the font ladder + the device's 64-colour gamut — proving wiring + init +
//!   addressing + the colour/text path end-to-end before the real app rendered through it (that
//!   standalone demo was retired once the app drove both panels, issue #177).
//!
//! ## microSD SPIM — map/route/track storage (#123)
//!   Instance **SERIAL22 / SPIM22** — a standard-speed instance (SD doesn't need 32 MHz),
//!   *separate* from the display bus, on its own software CS. DK expansion-header SPI pins:
//!     SCK P1_11 | MISO P1_07 | MOSI P1_06 | CS P1_12   (FLPR build moves CS → P0_00, see #165 below)
//!   CS is a free GPIO held LOW for the whole session (the held-low-CS workaround embedded-sdmmc's
//!   per-byte framing needs over embassy SPI — see `sd::NoCs`); the bus inits ≤400 kHz then
//!   re-clocks to 8 MHz (`sd::init`). embassy-nrf's `Spim` exposes **no** internal MISO pull-up,
//!   so the card's DO line is pulled high by the breakout (or an external
//!   10 kΩ to 3V3). The EYESPI connector also carries a microSD that *shares the display bus*; we
//!   leave that slot **unpopulated** and use this dedicated SPIM instead (a standalone SD-over-SPI
//!   reader feeding obc-platform's FatFs adapters). P1_06/P1_07 are the
//!   VCOM's RTS/CTS pins (below) — we drive them as SD MOSI/MISO instead, which is only safe
//!   because the VCOM runs **without** hardware flow control (HWFC OFF in the Board Configurator —
//!   see the crate README); with HWFC on, the J-Link gates host→device bytes on the device's RTS
//!   (P1_06), so this firmware never asserts it and host→device RX would be dead.
//!
//! ## VCOM UARTE — debug-sensor / telemetry stream (#127)
//!   Instance **SERIAL20 / UARTE20**, the DK's `chosen` console wired to the onboard J-Link's
//!   USB-CDC VCOM: TX P1_04 | RX P1_05. We bring it up **2-wire (no RTS/CTS)**, so the DK's VCOM
//!   **hardware flow control must be disabled** (Board Configurator — see the crate README);
//!   otherwise device→host telemetry still flows but host→device (the fake-sensor feed + input
//!   injection) is silently gated off on the un-driven RTS. The nRF54L15 has **no USB peripheral**,
//!   so the fake GPS/baro/compass feed and ride
//!   telemetry ride this UART; defmt logs ride RTT on the same cable. obc-platform's debug-source
//!   protocol is transport-agnostic, so it runs over the UART unchanged.
//!
//! ## Spare interrupt for the high-priority InterruptExecutor (#126)
//!   The two-plane architecture runs input + the overlay on a high-priority `InterruptExecutor`
//!   that preempts the map render, pended from a dedicated **software-interrupt vector**: **SWI00**
//!   (the M33 also has SWI01/02/03 + EGU10/EGU20 free). N5 runs it at **P3** — above thread mode (so it preempts
//!   the map render) but below the P0 GRTC time-driver (so `Timer`s still wake mid-render).
//!
//! ## Flash / RAM
//!   From the `nrf54l15-app-s` `memory.x`: FLASH 1524K @ 0x0000_0000, RAM 256K @ 0x2000_0000.
//!   A future MCUboot retrofit re-partitions flash — don't hard-code flash assumptions (see
//!   `memory.x` and epic #120). RAM is tight (no external RAM):
//!   the single RGB222 framebuffer is ~75 KB and the renderer scratch + caches must fit the
//!   rest — the board memory profile + budget assert is N3 (#124).
//! =============================================================================

#![no_std]
#![no_main]

// microSD map storage (#123): the module carries its own dead-code allow for the route/track write
// half that the app wires in at N6 (#127).
mod sd;
// The ST7789 panel geometry (`WIDTH`/`HEIGHT`) is shared by both display backends; the `St7789`
// driver itself is the opt-in `tft` map backend (the default FLPR build below replaces it).
mod st7789;
// LS021 FLPR backend (issue #165) — the **default** display: `main.rs` runs the real app on the
// reflective LS021 panel via the FLPR (the VPR coprocessor) unless `--features tft` selects the
// ST7789 bring-up panel instead (issue #173). The FLPR `DisplayDriver` backend + launch live in `ls021_flpr`;
// `com::com_task` free-runs the COM lines (the M33-direct `PanelBus` bench driver was retired in
// issue #176 — the FLPR drives frames now; only the COM electrode square wave stays on the M33).
#[cfg(not(feature = "tft"))]
mod com;
#[cfg(not(feature = "tft"))]
mod ls021_flpr;
// The board's display-driver seam — the single screen-write interface both panels implement, so the
// map plane drives either through one path (`fb_mut` + `present`).
mod display;
// Persistent device settings over on-chip RRAM (the SD-independent settings store); the RRAM I/O is
// stubbed pending on-glass work, but the boot-load + save-on-dirty calls are wired in `run_app`.
mod settings;
// Real GPS (SAM-M10Q) + altimeter (BMP581) on a shared TWIM30 I²C bus (issue #218) — the concrete
// transport + the event-driven sensor task. Compiled only on the **real-sensor** build (the default:
// neither `synth` nor `debug-uart`), since `synth`/`debug-uart` supply the location source instead.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
mod sensors;

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::Timer;
use st7789::{HEIGHT, WIDTH};
use {defmt_rtt as _, panic_probe as _};

// `Delay` (the ST7789 power-on waits) + the `St7789` driver type are the opt-in `tft` backend; the
// default FLPR build replaces them with the `Ls021Flpr` `DisplayDriver` backend, so neither is there.
#[cfg(feature = "tft")]
use embassy_time::Delay;
#[cfg(feature = "tft")]
use st7789::St7789;

// The ST7789 is the opt-in `tft` backend, so every ST7789-backend `cfg` below keys on
// `feature = "tft"` and the rest of the file treats `not(feature = "tft")` as "the default
// FLPR/LS021 path."
//
// The frame-absolute draw view `Band` is used by every build — both map backends' `present_overlay`
// drawers paint the hold bulge into it (issue #163). The ST7789 map's banding + bulge composite live
// behind the seam in `display::st7789` (issue #174).
use obc_platform::Band;

// N4 map path (#125): the shared app, the streamed-map reader + its `.bss` cache, and the
// device-native RGB222 framebuffer the renderer draws into (`color_fn` = identity Rgb565; the
// framebuffer quantizes to RGB222 on store, the band push expands it back for the ST7789).
// N5 (#126) adds the two-plane split: the high-priority `InterruptExecutor` for the input/overlay
// plane, the lock-free gesture channel to the thread-mode map plane, the async bus mutex over the
// shared panel + framebuffer, and the blocking mutex over the `InputPlane` both planes draw the
// bulge from.
// The blocking `InputPlane` mutex both planes touch — on ST7789 the input plane re-pushes the overlay
// window from it; on the FLPR build (issue #163) the input plane recognises + animates the bulge under
// it while the map plane composites it into the partial push. Both map builds share it now.
use core::cell::RefCell;
use core::mem::MaybeUninit;
use display::{DisplayDriver, OverlayRegion};
// The ST7789 `Display` backend (panel + framebuffer) now lives behind the seam in `display::st7789`
// (issue #174); the map plane builds it into [`BUS`] and drives it only through `DisplayDriver`.
#[cfg(feature = "tft")]
use display::Display;
use embassy_executor::InterruptExecutor;
use embassy_nrf::gpio::{Input, Pull};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
// The shared GPS/altimeter I²C bus (#218) — real-sensor build only.
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use embassy_nrf::twim::{self, Twim};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::{Channel, Sender};
#[cfg(feature = "tft")]
use embassy_sync::mutex::Mutex;
use embassy_time::Instant;
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
use obc_app::{
    App, AppState, Gesture, InputClock, InputEvent, InputPlane, InputSource, RideClock, Sensors, SettingsStore,
    TrackSink,
};
use obc_platform::{ButtonInput, FbDevice64, RowDiff};
use obc_reader::{MapCache, MapTables, Reader};
use obc_render::{zoom_for_mpp, RenderStats};
// The N6 ride loop (#127): the decoded-route-geometry cache, the resident per-route chunk index,
// and the streamed route reader the matcher + map render share — one per-frame structure.
use obc_route::{RouteCache, RouteIndex, RouteReader};
// The runtime-built shared statics (the bus `Mutex`, the `InputPlane` mutex, the VCOM ring buffers)
// are parked in `.bss` with the same in-place `MaybeUninit` + `ptr::write` pattern as APP/MAP_CACHE/
// ROUTE_CACHE — see `main`. Earlier this used `StaticCell`, but its one-shot `used` flag panics
// ("already full") if it's ever non-zero on entry, which on this board's debug-reset path it was; an
// unconditional in-place write has no such flag, so it survives a warm reset. (Hence no `static_cell`
// dependency.)
// The `synth`-build stand-in GPS (`SynthLocation`) — always-compiled in obc-platform, imported only
// on the `synth` build (the default streams the real SAM-M10Q; `debug-uart` a recorded host ride).
// Walks a slow square loop so a saved ride is a non-degenerate `.gpx`.
#[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
use obc_platform::SynthLocation;
// The real-sensor `Signal` sources (#218): the `GpsLocation`/`BaroAltimeter`/`SensorTemp` ZSTs the
// ride loop polls, fed by `sensors::sensor_task`. Real-sensor build only (the default).
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
use obc_platform::sensor_link;
// Battery fuel gauge: a fixed-level stand-in until the nPM1300 PMIC gauge is read (issue follow-up).
use obc_platform::StubFuelGauge;

// LS021 FLPR backend (issue #165): the resident-framebuffer `DisplayDriver` backend + its launch, and the
// free-running COM driver. The FLPR scans the whole frame in one push, so there is no banded ST7789
// `Display`/bus mutex here — the map plane owns the panel, COM + the gesture-input plane run on the
// shared high-priority executor (see `main`).
#[cfg(not(feature = "tft"))]
use com::com_task;
#[cfg(not(feature = "tft"))]
use ls021_flpr::{launch_flpr, FlprError, Ls021Flpr};

// VCOM debug-sensor / telemetry stream (#127), behind `debug-uart`: the interrupt-buffered UARTE on
// the DK's J-Link VCOM. `BufferedUarte` keeps RX DMA continuously armed into a ring driven by the
// SERIAL20 interrupt, so the tens-of-ms map render never drops a streamed byte. `uarte` carries the
// shared `Config` (8N1 @ 115200).
#[cfg(feature = "debug-uart")]
use embassy_nrf::buffered_uarte::{self, BufferedUarte, BufferedUarteRx, BufferedUarteTx};
#[cfg(feature = "debug-uart")]
use embassy_nrf::uarte;

// SERIAL00 backs the display SPIM (#122); SERIAL22 the microSD SPIM (#123). Both handlers are
// always registered (harmless when a feature is off — the peripheral is simply never constructed,
// so its interrupt never fires); each `Spim::new` is handed `Irqs` for its instance.
bind_interrupts!(struct Irqs {
    SERIAL00 => spim::InterruptHandler<peripherals::SERIAL00>;
    SERIAL22 => spim::InterruptHandler<peripherals::SERIAL22>;
});

// VCOM UARTE20 RX/TX → the `BufferedUarte`'s interrupt-fed ring buffers (#127). A separate struct so
// it's bound only with the sensor stream; SERIAL20 is distinct from the two SPIM instances above, so
// the three handlers never clash.
#[cfg(feature = "debug-uart")]
bind_interrupts!(struct UartIrqs {
    SERIAL20 => buffered_uarte::InterruptHandler<peripherals::SERIAL20>;
});

// TWIM30 (== SERIAL30) backs the shared GPS + altimeter I²C bus (#218); bound only on the real-sensor
// build, so it never clashes with the SPIM/UARTE handlers above (a distinct instance either way).
#[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
bind_interrupts!(struct SensorIrqs {
    SERIAL30 => twim::InterruptHandler<peripherals::SERIAL30>;
});

/// One band's worth of RGB565 scratch (`WIDTH * BAND_ROWS`), living in `.bss`. 14 rows ≈ 6.6 KB
/// and tiles the 320-row frame in 23 bands. The banded push fills it — the map path expands the
/// RGB222 frame into it — then byte-swaps to big-endian
/// and SPIM-DMAs it; borrowed exactly once below (single executor → no aliasing). `BAND_BYTES` is
/// what the N3 budget assert reserves. (N6, #127: 20→14, freeing ~2.9 KB toward the ride-loop
/// residents + the deep render path's stack; must stay ≥ the overlay window `OVL_W × OVL_ROWS`,
/// asserted below — 240×14 = 3360 ≥ 16×192 = 3072.)
///
/// **ST7789-only** (`--features tft`). The default FLPR map path (issue #165) packs the RGB222
/// framebuffer straight to the LS021 wire (`ls021_pack_row`) and renders device-64 directly, so it
/// needs no RGB565 band scratch — freeing these ~6.6 KB is one of the levers that fits the app in the
/// 244 KB the FLPR carve-out leaves (the FLPR build drops `BAND_BYTES` from the budget below).
#[cfg(feature = "tft")]
const BAND_ROWS: usize = 14;
/// The band scratch in bytes (RGB565, 2 B/px) — the resident cost the budget assert reserves.
#[cfg(feature = "tft")]
const BAND_BYTES: usize = st7789::WIDTH as usize * BAND_ROWS * 2;
#[cfg(feature = "tft")]
static mut BAND: [u16; WIDTH as usize * BAND_ROWS] = [0; WIDTH as usize * BAND_ROWS];

// ============================ N3 board memory budget (issue #124) ============================
// The nRF54L15 has 256 KB RAM and no external RAM, so the whole resident working set of a full
// map redraw must fit there. This build-time assert fails the
// build — rather than overflowing RAM on glass — if the shared crates' caps (trimmed by the
// `nrf-mem` profile, enabled on the obc-app edge in Cargo.toml) ever outgrow the budget. It
// compiles for thumbv8m (usize = 4 B), so every `size_of` here is the true on-device size.
//
// The binding moment is a full redraw with everything resident at once (the `nrf-mem` profile
// trims each term — see the per-crate caps — to make this fit):
//   - `App`        embeds the renderer scratch (`obc_render::MCU_RENDERER_BYTES`, ~66 KB nrf-mem)
//                  plus the resident elevation `Profile` (~4.6 KB at PROFILE_COLS=512) and
//                  `Breadcrumb` (~6 KB at SPINE_CAP=512); ~88 KB total.
//   - framebuffer  the single RGB222 frame (#N4): 240×320 × 1 B/px = 75 KB — the `FB` static below,
//                  which the map path renders into.
//   - `MapCache`   the streamed-map geometry-chunk cache (3 slots on nrf-mem, ~25 KB).
//   - `RouteCache` the decoded-route-chunk cache (3 slots on nrf-mem, ~9 KB — trimmed 4→3 as a
//                  256 KB-DK stop-gap to claw ~3 KB back for the deep ride-loop render stack below).
//   - `RouteIndex` the active route's resident chunk index — the ride loop (#127) holds it across
//                  frames in the map plane's task future to stream geometry without re-walking it
//                  (128 chunks on nrf-mem, ~6 KB). Counted here because, unlike the host, on the
//                  256 KB part it materially shares the budget — and because
//                  `RouteIndex::read` builds it on the *stack*, so keeping it ~6 KB also keeps that
//                  transient build spike inside the stack reserve below.
//   - band scratch one RGB565 display band (`BAND_BYTES`, ~7.5 KB).
// plus `STACK_RESERVE` headroom for the main stack + embassy's executor/task arenas. Note the stack
// must also absorb a per-redraw `Reader::new` (the OBCM style table → a ~2.4 KB `Reader` value built
// as a stack temporary, plus its own ~4 KB read scratch): unlike N4/N5 (one `Reader` held in `.bss`
// for the whole run) the ride loop rebuilds it each frame, so the stack reserve carries that spike.
// The N6 ride loop (#127) trimmed the `nrf-mem` caps (MapCache 5→3, MAX_ROUTE_CHUNKS 512→128,
// MAX_SPANS 1280→768, MAX_FRAME_POINTS 2560→1536, MAX_FRAME_RINGS 768→384, PROFILE_COLS 1024→512,
// band 20→14) to make `RouteCache` + `RouteIndex` fit *and* keep ~33 KB of stack — the deep
// per-redraw render path (`Reader::new` style-table parse + streamed-chunk decode over
// embedded-sdmmc, with the high-priority overlay plane preempting it) overran a smaller reserve.

// The N6 ride loop (#127) trimmed the `nrf-mem` caps to make `RouteCache` + `RouteIndex` fit *and*
// keep ~33 KB of stack. The FLPR build (#165) does **not** re-trim them: it instead reclaims room two
// other ways — the FLPR carve-out leaves the M33 **244 KB** (not 256), but the production blob is
// ~660 B so the carve shrank 32→12 KB (≈20 KB more M33 RAM than F0's 28 KB carve), and the FLPR map
// path drops the ~6.6 KB RGB565 band scratch — a net loosening, so the same caps clear the budget.

/// Total SRAM the M33 app core sees. The opt-in ST7789 build (`--features tft`) links the full 256 KB
/// (`memory.x`: RAM 256K @ 0x2000_0000); the default FLPR build links only 244 KB
/// — the top 12 KB is the carved FLPR image + the shared handshake page (`build.rs` / `flpr.ld`).
#[cfg(feature = "tft")]
const NRF_RAM_BYTES: usize = 256 * 1024;
#[cfg(not(feature = "tft"))]
const NRF_RAM_BYTES: usize = 244 * 1024;
/// Headroom kept free under the resident statics for the main stack + embassy's executor/task
/// arenas (statics grow up from the RAM base, the stack down from the top). This is only the
/// build-time *floor* the assert enforces — the real stack is the residual `RAM − statics`. After
/// the RouteCache 4→3 trim the statics end ~221 KB in, leaving ~35 KB of true stack — enough to clear
/// the ~33 KB deep-render peak described above.
const STACK_RESERVE: usize = 16 * 1024;
/// The single RGB222 framebuffer (#N4): one byte per pixel over the 240×320 panel = 75 KB.
const FB_BYTES: usize = st7789::WIDTH as usize * st7789::HEIGHT as usize;

/// The resident set that must coexist during a redraw (see the table above). Includes the active
/// route's `RouteIndex` — the ride loop (#127) keeps it resident across frames, so on the 256 KB
/// part it shares the budget like the caches do.
const RESIDENT_BYTES: usize = core::mem::size_of::<obc_app::App>()
    + FB_BYTES
    + core::mem::size_of::<RowDiff<{ HEIGHT as usize }>>() // the self-diffing present store (#201, 1.28 KB)
    + core::mem::size_of::<obc_reader::MapCache>()
    + core::mem::size_of::<obc_reader::MapTables>()
    + core::mem::size_of::<obc_route::RouteCache>()
    + core::mem::size_of::<obc_route::RouteIndex>()
    + BAND_RESERVE;
/// The RGB565 band scratch the budget reserves: `BAND_BYTES` on the ST7789 path, **zero** on the FLPR
/// path (it packs the framebuffer straight to the wire — see [`BAND_ROWS`]).
#[cfg(feature = "tft")]
const BAND_RESERVE: usize = BAND_BYTES;
#[cfg(not(feature = "tft"))]
const BAND_RESERVE: usize = 0;
const _: () = assert!(
    RESIDENT_BYTES + STACK_RESERVE <= NRF_RAM_BYTES,
    "nRF resident set (App + framebuffer + MapCache + MapTables + RouteCache + RouteIndex + band) + stack reserve overruns RAM — trim the `nrf-mem` caps (issue #124)"
);

// ============================ N4 resident set + map path (issue #125) ============================

/// The resident device-native RGB222 framebuffer: one byte per pixel over the 240×320 panel
/// (`FB_BYTES` = 75 KB), in `.bss`. [`App::render_map`](obc_app::App::render_map) quantizes into it
/// on store ([`FbDevice64`]). On the **ST7789** path it is borrowed into the [`Display`] behind
/// [`BUS`] and the band push expands it back to RGB565, so the two planes (#126) reach it only under
/// that mutex (no aliasing, no torn frame). On the default **FLPR** path (issue #165) it is
/// owned by the `Ls021Flpr` panel — the map plane renders into it and `push_frame` packs it straight
/// to the LS021 wire.
static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// The **self-diffing present** store (issue #201/#200): one 32-bit hash per framebuffer row of the
/// last-pushed frame, in `.bss` (320 rows = 1.28 KB). The active display backend borrows it (`&mut`)
/// and, on present, re-hashes each row and pushes only the rows whose hash changed — so a Home clock
/// tick re-presents its clock band instead of all 320 rows (~97 ms → a few ms on the FLPR). One store,
/// borrowed by whichever backend is compiled (only one is). `RowDiff::new()` is all-zero (+ the unprimed
/// flag) ⇒ a `.bss` static, and the first present force-pushes the whole frame to seed it.
static mut ROW_DIFF: RowDiff<{ HEIGHT as usize }> = RowDiff::new();

/// The streamed-map geometry cache + the shared [`App`], placed in `.bss` and built **in place**
/// (a `ptr::write` into the reserved region): the ~96 KB `App` (incl. the
/// ~74 KB renderer scratch) and the ~41 KB cache must never form on the 256 KB part's small stack.
/// [`MapCache::new`](obc_reader::MapCache) is an all-zero `MaybeUninit::zeroed`, so writing it is a
/// `.bss` memset; [`App::init_map`](obc_app::App::init_map) writes each field where it sits. (The
/// `RouteCache` the budget assert reserves is allocated when route loading lands at N6.)
static mut MAP_CACHE: MaybeUninit<MapCache> = MaybeUninit::uninit();
/// The immutable map tables (header scalars + style table + LOD pyramid), parsed **once at boot**
/// into `.bss` and borrowed by every per-frame [`Reader`] (issue #179). Resident so the per-frame
/// render reader carries no styles/LODs of its own — no per-frame style-table SD read, no ~4 KB parse
/// stack spike on the deep render path (the lever that kept that path inside the 256 KB stack).
static mut MAP_TABLES: MaybeUninit<MapTables> = MaybeUninit::uninit();
static mut APP: MaybeUninit<App> = MaybeUninit::uninit();
/// The decoded-route-geometry cache (#127), placed in `.bss` and built in place like [`MAP_CACHE`]
/// ([`RouteCache::new`](obc_route::RouteCache) is an all-zero `MaybeUninit::zeroed` → a `.bss`
/// memset, never a stack temporary). The session-long cache (issue #98 P4) a redraw of the
/// unchanged route + the matcher's per-fix decode hit instead of re-reading `.obcr` geometry off
/// the card every frame; the budget assert above already reserves its bytes.
static mut ROUTE_CACHE: MaybeUninit<RouteCache> = MaybeUninit::uninit();

/// Build a `'static` value into a `.bss` [`MaybeUninit`] slot, returning the sole `&'static mut` to
/// it — the warm-reset-safe replacement for `StaticCell` that every runtime-built shared static (the
/// bus mutex, the `InputPlane` mutex, the VCOM rings, the map/route caches) is created through.
/// `StaticCell`'s one-shot `used` flag panics ("already full") if it is ever non-zero on entry, which
/// on this board's debug-reset path it can be; an unconditional in-place [`ptr::write`](core::ptr)
/// carries no such flag, so it survives a warm reset. Centralises the open-coded `addr_of_mut!` +
/// cast + `write` idiom into one `unsafe` + SAFETY contract. `#[inline(always)]` so the by-value
/// `val` never lands on the stack — a zeroed `MaybeUninit::new` (the big caches) packs straight to a
/// `.bss` memset, exactly as the open-coded `slot.write(..)` did.
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

/// Idle camera zoom for the N4 static map, in ground metres-per-pixel (the 0.5–4 mpp riding band).
/// A coarse-ish 2 mpp shows a town-scale overview — several roads / landuse polygons, so the
/// 64-colour gamut is visible at a glance — rather than a tight patch. Freely tunable: the frame is
/// a single static render until buttons land (#126).
const INIT_MPP: f32 = 2.0;

/// Heartbeat-only idle for an unrecoverable bring-up failure (no card, no `.obcm`, or a map that
/// isn't valid OBCM): blink LED0 forever rather than panic — a missing/bad card must **never** fault
/// (acceptance criterion). Diverges, so the map path below is unreachable
/// after it.
async fn idle_blink(led: &mut Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

/// Scan the card's `/routes/*.obcr` catalog into the app's Route menu. Deliberately its **own
/// `#[inline(never)]` frame**: the ~5 KB [`Catalog`](obc_app::Catalog) (`Vec<RouteSummary,
/// MAX_ROUTES>`, 64 × ~84 B) lives here and is popped on return, so it never sits on `main`'s frame
/// *beneath* the long-lived [`run_app`] ride loop. (When the loop was inline in `main`, LLVM could
/// reuse the catalog's dead stack slot for the loop's locals; once the loop moved into its own
/// function the two frames stop sharing, and a resident 5 KB catalog there silently steals from the
/// deep route-load render path's stack — overflowing the 256 KB part. Issue #175.)
#[inline(never)]
fn load_routes(storage: &mut sd::Storage, app: &mut App) {
    let catalog = storage.scan_routes();
    app.set_routes(&catalog);
}

/// Stack high-water guard (issue #175): [`paint`] fills the free stack with a sentinel early in
/// `main`; [`used`] then reports the deepest reach by finding the lowest still-painted word (the
/// stack runs `_stack_start` top → `_stack_end` bottom, and a deep call overwrites the sentinel). The
/// ride loop logs only on a *new* peak, so it's silent once warm but flags any future change that
/// creeps the deep route-load render toward the 256 KB-DK's ~36 KB ceiling — the exact silent
/// overflow this issue chased. Cheap (one boot paint + a per-frame scan); harmless on the 512 KB
/// nRF54LM20 target, where the stack budget is no longer tight.
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

// ============================ N5 two-plane input + overlay (issue #126) ============================
// The map render (`render_map` + the banded push) is a CPU- and SPI-bound call that would block its
// executor for tens of ms. To keep input + the hold bulge responsive *during* that, the device runs
// two planes (issue #48), here built around one shared SPI panel:
//   - Map plane (thread mode, the `main` loop): drains the gesture channel → `apply_gesture`,
//     advances screen animations, and re-renders the map only on `dirty.map`, compositing the live
//     bulge into each pushed band.
//   - Input plane (`input_overlay_task`, on a high-priority `InterruptExecutor` pended from SWI00):
//     samples the buttons, recognises gestures (into the channel), and re-pushes just the right-edge
//     overlay window so the bulge animates over a static map at full FPS with no map re-render.
// The shared resource is the panel SPI bus + the framebuffer: the async `BUS` mutex serialises
// pushes — and, since the map render runs inside it, the
// framebuffer write against the input plane's window read — without disabling interrupts (the GRTC
// time-driver + the input executor keep running while it's held). Keeping the framebuffer *inside*
// the mutex means the input plane never reads a half-rendered frame, so the bulge backdrop is always
// clean (no tearing); the cost is that a long map render holds the bus, so the bulge can briefly
// "stick" while a big segment repaints — a price worth paying for an artifact-free overlay, since a
// fluid-but-unlocked framebuffer tore the bulge. The `InputPlane` both planes draw the bulge from is
// behind a brief blocking mutex; lock order is always BUS-outer, INPUT_PLANE-inner.

// The two [`DisplayDriver`] backends now live behind the seam in their own modules (issue #174):
// `display::st7789` (the `Display` panel + framebuffer, opt-in `tft`) and `display::ls021_flpr` (the
// default FLPR backend). `main.rs` builds + drives whichever is compiled only through the seam — the
// ST7789 `Display` (re-exported here) behind [`BUS`], the FLPR `Ls021Flpr` owned by the map plane.

/// Bound of the input→map gesture channel. One frame yields a couple of gestures and the map plane
/// drains it each loop, so even across a slow map push it never fills; `try_send` drops on the
/// (unreachable) overflow rather than block the high-priority plane.
const GESTURE_QUEUE: usize = 16;

/// Recognised gestures flowing from the input plane (high priority) to the map plane (thread mode) —
/// the only lock-free shared state between the two planes.
static GESTURES: Channel<CriticalSectionRawMutex, Gesture, GESTURE_QUEUE> = Channel::new();

/// The high-priority executor the input/overlay plane runs on, pended from the SWI00 vector (an
/// unused software-interrupt line — see the module doc). Started in `main`; driven by the SWI00 ISR.
/// The FLPR build uses [`EXECUTOR_HP`] instead (it also free-runs COM there).
#[cfg(feature = "tft")]
static EXECUTOR_INPUT: InterruptExecutor = InterruptExecutor::new();

/// SWI00 ISR → poll the input-plane executor. SWI00 has no peripheral; we only borrow its interrupt
/// vector as the executor's pend line (its priority is set + the executor started in `main`).
#[cfg(feature = "tft")]
#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_INPUT.on_interrupt();
}

/// The FLPR build's single high-priority executor (issue #165): it free-runs **both** the COM driver
/// (which must keep alternating `VCOM`/`VB`/`VA` so the panel never DC-biases, even while the M33
/// busy-polls a frame push) **and** the gesture-input plane (so button latency stays exact during the
/// ~97 ms blocking whole-frame push). Pended from the same unused SWI00 vector @ P3.
#[cfg(not(feature = "tft"))]
static EXECUTOR_HP: InterruptExecutor = InterruptExecutor::new();

#[cfg(not(feature = "tft"))]
#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_HP.on_interrupt();
}

/// Input-plane loop period (ms): buttons sampled + gestures recognised + the bulge animated this
/// often, on the high-priority executor that preempts the map render — so press-to-feedback latency
/// and the auto-repeat cadence stay exact regardless of how long a map frame takes.
const LOOP_MS: u64 = 8;

// The hold-bulge's right-edge overlay **columns** (issue #126/#163). Both bulges erupt from the right
// screen edge ≤12 px deep, so this fixed 16-px column band bounds them with margin. Both map panels
// re-present the bulge through `DisplayDriver::present_overlay` over the clean framebuffer, addressing
// only the live bulge's *rows* (`InputPlane::overlay_rows`: encoder ≈ 59–171, Back ≈ 182–246) — ST7789
// a 16-px column window, the FLPR the full-width rows of that span — so the column constants are
// shared; the fixed row band (`OVL_Y0`/`OVL_ROWS`) is only the ST7789 trailing-clear + band-fit bound.
/// First overlay column: the rightmost 16 px (bulge depth ≤12 + margin).
const OVL_X0: u16 = WIDTH - 16;
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
const _: () =
    assert!(OVL_W as usize * OVL_ROWS as usize <= WIDTH as usize * BAND_ROWS, "overlay window larger than BAND");

// The live-bulge "present the rows *around* it" discipline (issue #163) now lives **inside** the
// self-diffing present: the FLPR map plane passes the bulge's row span to `Ls021Flpr::present_within`,
// which clips it out of the changed-row spans it pushes (`obc_platform::clip_span`). The map present
// leaves those rows for `MapDisplay::present_bulge`, exactly as the old free-standing `map_rows_around`
// did — but only the *changed* rows around the bulge are pushed now, not the whole complement.

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
struct InstantClock;
impl obc_render::Clock for InstantClock {
    fn now_us(&self) -> u64 {
        Instant::now().as_micros()
    }
}

/// VCOM RX → sensor signals (#127): read bytes from the
/// interrupt-fed ring and feed each complete `F`/`A`/`C`/`K`/`Z` line into `obc-platform`'s
/// fresh-fix signals, which the app's `DebugLocation`/`DebugAltimeter`/`DebugCompass`/`DebugInput`
/// poll. A UART never "disconnects", so — unlike the CDC version — one `LineReader` lives for the
/// whole session.
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

/// VCOM TX ← telemetry (#127): send one compact status
/// line each time the app publishes telemetry (~2 Hz via `set_telemetry`), so the host's readout
/// updates without the device polling or flooding the link. The buffered UARTE chunks the line to
/// DMA itself, so — unlike the CDC 64-byte-packet path — no manual packet splitting is needed (the
/// telemetry line ≤192 B fits the TX ring); just loop until the whole line is queued.
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
        buttons.update(now);
        // Recognise this frame's input under the shared InputPlane lock (a brief critical section,
        // never held across an await/push). Each gesture is pushed to the map plane; the bulge is
        // advanced regardless, so the press is confirmed on screen below even before the map plane
        // drains the channel.
        let (dirty, overlay_span) = input_plane.lock(|cell| {
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
            (plane.take_overlay_dirty(), plane.overlay_rows(WIDTH as i32, HEIGHT as i32))
        });

        // Repaint the bulge only when it changed (plus the one trailing clear `take_overlay_dirty`
        // reports): re-present just the right-edge region over a static map through the seam — while
        // live, **only the active bulge's rows** (issue #163), and the full band on the trailing clear
        // to wipe the last bulge. `present_overlay` fills the window from the clean framebuffer +
        // composites the bulge (the `InputPlane` lock is taken once, inside the drawer). Awaiting the
        // bus yields to the (thread-mode) map plane if it is mid-frame, so this never spins.
        //
        // Dev-only-best-effort (issue #208): the trailing clear is the **one** frame `take_overlay_dirty`
        // flags, not a retry-until-acked loop like the FLPR map plane's `last_overlay_span` clear. The
        // `bus.lock().await` here can't be dropped (it always eventually acquires), so in practice the
        // clear lands — but the coordination isn't hardened the way the shipping FLPR path is, by design:
        // this is the opt-in `tft` bring-up backend, and the issue #208 item-2 redesign would replace
        // this whole dance on both backends.
        if dirty {
            let region = match overlay_span {
                Some((y0, rows)) => OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows },
                None => OverlayRegion { x0: OVL_X0, y0: OVL_Y0, w: OVL_W, rows: OVL_ROWS },
            };
            bus.lock().await.present_overlay(region, &mut |band: &mut Band| {
                input_plane.lock(|cell| cell.borrow().render_overlay(band, WIDTH as f32, HEIGHT as f32, color_fn));
            });
        }
        Timer::after_millis(LOOP_MS).await;
    }
}

/// The FLPR build's input plane (issues #165/#163): recognises gestures + animates the hold bulge.
/// Runs on [`EXECUTOR_HP`] beside COM, preempting the thread-mode map render, so press latency + the
/// auto-repeat cadence stay exact across the blocking whole-frame push. Each [`LOOP_MS`] it samples
/// the buttons + (with `debug-uart`) the VCOM-injected `K` events and recognises gestures into
/// [`GESTURES`] for the map plane to apply — **under the shared [`InputPlane`] lock**, so the live
/// hold-bulge state it advances is the same one the map plane composites into its partial overlay push.
///
/// Unlike the ST7789 plane this task does **not** push to glass: the FLPR scans whole frames, so the
/// *map plane* owns every push (no shared SPI bus to serialise against). It re-presents only the bulge
/// rows when the overlay is dirty — see the map loop. This task is purely the recogniser; the brief
/// lock is never held across the `await`.
#[cfg(not(feature = "tft"))]
#[embassy_executor::task]
async fn input_task(
    mut buttons: ButtonInput<Input<'static>>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    gestures: Sender<'static, CriticalSectionRawMutex, Gesture, GESTURE_QUEUE>,
) {
    loop {
        let now = Instant::now().as_millis() as u32;
        buttons.update(now);
        // Recognise + animate the bulge under the shared lock (a brief critical section, never held
        // across the await), so the bulge state the map plane composites is the one this advanced.
        // Physical buttons + (with `debug-uart`) the VCOM-injected `K` events, one recogniser pass.
        input_plane.lock(|cell| {
            let plane = &mut *cell.borrow_mut();
            let mut dbg = debug_input();
            let mut input = ChainedInput { a: &mut buttons, b: &mut dbg };
            plane.recognize(InputClock(now), &mut input, |g| {
                if gestures.try_send(g).is_err() {
                    defmt::warn!("gesture channel full — dropped a gesture (map plane stalled?)");
                }
            });
        });
        Timer::after_millis(LOOP_MS).await;
    }
}

// ============================ The de-cfg'd map plane (issue #175) ============================
// The ride loop drives the screen through **one** handle, [`MapDisplay`], so [`run_app`] carries no
// per-backend `#[cfg]`. `MapDisplay` is one name with two `cfg`-selected definitions — the only place
// the backends diverge — each exposing the same three methods the loop calls:
//   - `poll_overlay`     — this frame's hold-bulge state (dirty edge + live row span);
//   - `render_present`   — render the clean frame into the framebuffer + push it to glass;
//   - `present_bulge`    — re-present the hold bulge over the clean map.
// The genuine asymmetry they hide: the ST7789 shares its bus with the input/overlay plane (which owns
// the bulge re-push) so its map loop has no overlay work; the FLPR owns the panel outright and pushes
// the bulge itself from the map plane (issue #163). Everything else in the loop is shared.

/// What [`MapDisplay::render_present`] reports for one map frame: whether the push reached glass
/// (`false` → a transport fault to retry, #66), the render's [`RenderStats`], and the render / push
/// timings (µs) the RTT log + the VCOM telemetry carry.
struct FramePresent {
    ok: bool,
    stats: RenderStats,
    render_us: u64,
    push_us: u64,
}

/// ST7789 (`--features tft`): the map plane's handle is just the `&'static` bus mutex it shares with
/// the input/overlay plane. That plane owns the hold-bulge re-push, so the map loop has no overlay
/// bookkeeping here.
#[cfg(feature = "tft")]
struct MapDisplay {
    bus: &'static Mutex<CriticalSectionRawMutex, Display>,
}

#[cfg(feature = "tft")]
impl MapDisplay {
    /// The ST7789 bulge rides the input/overlay plane (`input_overlay_task`), which the map loop holds
    /// no handle to — so the map loop never has a live bulge span: always "clean". This is *why* the
    /// ST7789 map present can't clip a live bulge the way the FLPR path does, and the coordination is
    /// accepted as dev-only-best-effort on this opt-in `tft` backend (issue #208 — see the caveat on
    /// [`Display::present`](crate::display)).
    #[inline(always)]
    fn poll_overlay(&mut self) -> (bool, Option<(u16, u16)>) {
        (false, None)
    }

    /// Lock the shared bus, render the clean frame into the framebuffer through the seam, and band
    /// the whole frame to GRAM. `overlay_span` is unused (the bulge is a separate fast plane).
    /// Dropping the guard on return lets the input plane push its bulge again. `#[inline(always)]` +
    /// a generic (non-`dyn`) `render` so the deep render folds into the caller's frame rather than
    /// nesting another (the #175 stack regression — see [`run_app`]).
    #[inline(always)]
    async fn render_present(
        &mut self,
        _overlay_span: Option<(u16, u16)>,
        mut render: impl FnMut(&mut dyn DisplayDriver) -> RenderStats,
    ) -> FramePresent {
        let mut guard = self.bus.lock().await;
        let display: &mut dyn DisplayDriver = &mut *guard;
        let t_render = Instant::now();
        let stats = render(display);
        let render_us = t_render.elapsed().as_micros();
        let t_push = Instant::now();
        let ok = display.present();
        let push_us = t_push.elapsed().as_micros();
        FramePresent { ok, stats, render_us, push_us }
    }

    /// No-op: the ST7789 hold bulge is pushed by the input/overlay plane, not the map loop.
    #[inline(always)]
    fn present_bulge(&mut self, _span: Option<(u16, u16)>, _dirty: bool) {}

    /// The ST7789 map loop can't see the input plane (its bulge rides the overlay plane), so the
    /// in-screen hold fills (the Reset bar) aren't driven on this dev-only backend — report no
    /// hold. The overlay bulge is still the live hold feedback here.
    #[inline(always)]
    fn hold_progress(&self) -> f32 {
        0.0
    }
}

/// FLPR LS021 (default): the map plane owns the panel outright (whole-frame scan per push → no shared
/// bus), plus the shared `InputPlane` it composites the bulge from and the gate/source GPIO lines it
/// must keep driven for the program's life.
#[cfg(not(feature = "tft"))]
struct MapDisplay {
    panel: Ls021Flpr<'static>,
    input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>,
    /// The last live bulge's rows, so the trailing clear wipes exactly them, not the whole hint band
    /// (issue #163).
    last_overlay_span: Option<(u16, u16)>,
    /// The gate + source lines the FLPR drives — held only to keep them configured as outputs for the
    /// program's life (never touched after launch); dropping them would float the panel.
    _gate_bus: [Output<'static>; 4],
    _src_bus: [Output<'static>; 8],
}

#[cfg(not(feature = "tft"))]
impl MapDisplay {
    /// Sample the shared `InputPlane` once per frame (the map plane is the sole owner of the FLPR
    /// overlay bookkeeping): the dirty edge (live while the bulge animates, plus one trailing clear)
    /// and the live bulge's **row span** (`None` when quiet), so the map present can go *around* it
    /// and `present_bulge` can re-present it (issue #163).
    #[inline(always)]
    fn poll_overlay(&mut self) -> (bool, Option<(u16, u16)>) {
        self.input_plane.lock(|c| {
            let p = &mut *c.borrow_mut();
            (p.take_overlay_dirty(), p.overlay_rows(WIDTH as i32, HEIGHT as i32))
        })
    }

    /// The live encoder hold-progress from the shared input plane (0.0–1.0). Fed to the map render
    /// so the in-screen confirm fills (the factory-Reset bar) track the hold — `App`'s own input
    /// plane isn't driven on the two-plane firmware, so without this the bar never fills.
    #[inline(always)]
    fn hold_progress(&self) -> f32 {
        self.input_plane.lock(|c| c.borrow().encoder_hold_progress())
    }

    /// Render the clean frame into the owned panel and **self-diff** it to glass (issue #201): push
    /// only the rows that changed since the last present. With a live bulge, [`present_within`] clips
    /// its rows out (`overlay_span`) and leaves them for `present_bulge` — the FLPR's ~100 ms full-frame
    /// scan would otherwise blank the bulge for that whole scan (the pop-flicker, issue #163), and even
    /// a partial clean push would flash it off. No shared bus: the map plane owns every push here.
    /// Marked `#[inline(always)]` with a generic (non-`dyn`) `render` so the deep render folds into the
    /// caller's frame rather than nesting another (the #175 stack regression).
    ///
    /// [`present_within`]: Ls021Flpr::present_within
    #[inline(always)]
    async fn render_present(
        &mut self,
        overlay_span: Option<(u16, u16)>,
        mut render: impl FnMut(&mut dyn DisplayDriver) -> RenderStats,
    ) -> FramePresent {
        let t_render = Instant::now();
        let stats = render(&mut self.panel);
        let render_us = t_render.elapsed().as_micros();
        let t_push = Instant::now();
        // Self-diffing present: the whole frame when quiet (the seam's `present`), or clipped around a
        // live bulge's rows (`present_within`) so `present_bulge` owns them (issue #163/#201).
        let ok = match overlay_span {
            None => self.panel.present(),
            Some(_) => self.panel.present_within(overlay_span),
        };
        if !ok {
            // The push didn't reach glass (a stalled FLPR), but the self-diffing present already
            // advanced its row-hash store to this frame — so the caller's latched `pending_map_redraw`
            // retry would diff the identical `fb` against an up-to-date store and re-push *nothing*,
            // stranding the rows that missed glass. Re-arm a full push so the retry re-seeds the store
            // and repaints every row (issue #201).
            self.panel.reset_diff();
        }
        let push_us = t_push.elapsed().as_micros();
        FramePresent { ok, stats, render_us, push_us }
    }

    /// Present the hold bulge over the clean map (the FLPR bulge rides this map plane — no shared SPI
    /// bus to serialise against). While the bulge is live this re-composites its rows every frame (the
    /// map present clipped them out via [`present_within`], so the fresh backdrop + bulge land here —
    /// no mid-pop flash). Only the active bulge's rows are touched (the FLPR fast-forwards the gate to
    /// them + early-stops).
    ///
    /// The trailing clear (bulge just went quiet) wipes **the same rows** the last bulge used, because
    /// the self-diffing map present no longer guarantees it touched those rows: the bulge composited
    /// glass content the row-hash diff can't see (the store tracks the clean `fb`), so if the map
    /// content there is unchanged the diff skips it and the stale bulge would strand without this clear
    /// (issue #201). The clear re-pushes the clean `fb` rows, which the store already agrees with, so
    /// the next present stays quiet there. It is driven off [`last_overlay_span`](Self#) (cleared only
    /// on a **successful** push), not the one-shot `overlay_dirty` edge — so a one-frame FLPR stall
    /// during the clear is retried on the next frame rather than stranding the bulge with no edge left
    /// to re-fire it.
    #[inline(always)]
    fn present_bulge(&mut self, overlay_span: Option<(u16, u16)>, overlay_dirty: bool) {
        // `overlay_dirty` (the one-shot trailing edge) no longer gates the clear: a stalled clear has
        // to be retried on later frames too, so `last_overlay_span` (dropped only on a successful push)
        // drives it instead — see the doc above.
        let _ = overlay_dirty;
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));
        let input_plane = self.input_plane;
        let composite = |panel: &mut Ls021Flpr, region: OverlayRegion| -> bool {
            panel.present_overlay(region, &mut |band: &mut Band| {
                input_plane.lock(|cell| cell.borrow().render_overlay(band, WIDTH as f32, HEIGHT as f32, color_fn));
            })
        };
        if let Some((y0, rows)) = overlay_span {
            let t_push = Instant::now();
            let ok = composite(&mut self.panel, OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows });
            let push_us = t_push.elapsed().as_micros();
            self.last_overlay_span = Some((y0, rows));
            if ok {
                // Per-tick during a hold (~every 8 ms) — `debug` so it doesn't flood the default log.
                defmt::debug!("overlay frame: bulge push {=u64} us ({=u16} rows @ y{=u16})", push_us, rows, y0);
            } else {
                defmt::warn!("overlay frame: bulge push failed (FLPR stalled?) — retrying next overlay tick");
            }
        } else if let Some((y0, rows)) = self.last_overlay_span {
            // Trailing clear: re-present just the last bulge's rows with nothing composited = the clean
            // map restored under the just-gone bulge (the self-diffing map present may have skipped
            // them, so this is what actually wipes the bulge — see the method docs). Drop
            // `last_overlay_span` only when the push lands, so a stalled FLPR retries next frame.
            if composite(&mut self.panel, OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows }) {
                self.last_overlay_span = None;
            } else {
                defmt::warn!("overlay frame: trailing clear failed (FLPR stalled?) — retrying next frame");
            }
        }
    }
}

/// The shared map plane + N6 ride loop (#127), driving present through [`MapDisplay`] so it carries
/// **no backend `#[cfg]`** (issue #175). Each tick: drain the gestures the input plane recognised,
/// advance the visible screens' timed content, reconcile the card to the app's intent (open the
/// selected route's geometry; begin / finalise-to-GPX the ride log), feed the sensors → `tick`
/// (integrate the fix, map-match, log the track point), then re-render the map only on `dirty.map`
/// and present it. A static screen does zero map renders. LED0 keeps a ~1 Hz heartbeat. Never returns.
///
/// The remaining `#[cfg]`s here are the orthogonal `debug-uart` *feature* (a host sensor feed +
/// telemetry vs. the `SynthLocation` stand-in), not the display backend — that is wholly behind
/// `MapDisplay`.
#[allow(clippy::too_many_arguments)]
// a one-call internal builder; the params are all distinct residents
// `#[inline(always)]`: this is a single-call-site `-> !` future. Inlining folds it (and the present
// methods above) back into `main`'s frame — recovering the ~5 KB of stack the bare extraction cost
// (the deep route-load render then overran the 256 KB part's stack). Keeps the clean source split.
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
    // The OBCM bbox centre (lon, lat) — only the `SynthLocation` stand-in needs it (the host feed and
    // the real GPS both stream absolute positions). So it's threaded only on the `synth` build.
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))] cam_center: (i32, i32),
) -> ! {
    // Native renderer colour → identity `Rgb565`; `FbDevice64` quantizes to RGB222 on store (the
    // device-64 gamut the style table is tuned to — see `obc_platform::framebuffer`).
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
    let (mut gps, mut baro, mut temp, mut gps_clock) =
        (sensor_link::GpsLocation, sensor_link::BaroAltimeter, sensor_link::SensorTemp, sensor_link::GpsClock);
    #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
    let mut synth = SynthLocation::new(cam_center.0, cam_center.1, Instant::now());
    // Battery: a fixed 75 % stand-in until the nPM1300 PMIC fuel gauge is wired in (see
    // `obc_platform::fuel`). Polled in `Sensors` like any other sensor, on both build paths.
    let mut fuel = StubFuelGauge::new(75);

    // Per-frame ride-loop state:
    // - `prev_route` re-centres SynthLocation onto a freshly-loaded route's start (`synth` build
    //   only — the host feed and the real GPS stream absolute positions, so they need no re-centre);
    // - `prev_active`/`prev_session` gate the SD reconcile on actual change (#73);
    // - `route_index`/`index_route` cache the active route's chunk index, rebuilt only on a route
    //   change (#44);
    // - `pending_map_redraw` re-arms a redraw a transient SD glitch couldn't service (#66);
    // - `last_telem*` throttle the host telemetry (debug-uart only).
    // (The FLPR bulge's last-row-span bookkeeping moved into `MapDisplay`.)
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
    // Stack-guard bookkeeping: log only when a new deepest reach is seen (silent once warmed up), so
    // a future change that pushes the deep render path closer to the 256 KB-DK's ~36 KB stack ceiling
    // shows up immediately instead of as a silent overflow (issue #175). Harmless on the 512 KB target.
    let mut stack_hw = 0usize;
    let mut last_led = 0u32;
    // Previous frame's hold-progress, so a hold that retracts on a non-map screen (released early, or
    // just completed) gets one trailing redraw to clear its on-screen bar — the falling edge the
    // charging redraw below would otherwise miss now that a cancelled long-press emits no gesture.
    let mut prev_hold_p = 0.0f32;

    // Settings: seed the app from the persistent RRAM store at boot (a blank/corrupt page decodes
    // to `None` → defaults), then persist on any change the settings screens make.
    app.set_settings(settings_store.load().unwrap_or_default());

    // Align the GPS to the persisted fix interval (#117): push it to the sensor task once at boot
    // (the task boots at a 1 s default), then again whenever the Power screen edits it. `prev_interval`
    // gates the re-VALSET so an unrelated settings change (units, clock) doesn't reconfigure the M10.
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    let mut prev_interval = app.settings().fix_interval_s;
    #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
    sensor_link::set_rate(prev_interval);

    loop {
        let now = Instant::now().as_millis() as u32;
        let hw = stackmeter::used();
        if hw > stack_hw {
            stack_hw = hw;
            defmt::info!("stack high-water {=usize} / {=usize} B (new peak)", hw, stackmeter::total());
        }

        // Apply the high-priority plane's recognised gestures, in order, then advance animations.
        // The screen transition lands a frame after the overlay already confirmed the press.
        while let Ok(g) = GESTURES.try_receive() {
            app.apply_gesture(g);
        }
        app.advance_animations(InputClock(now));

        // Persist settings the moment a settings screen changes one (the save-on-dirty path the
        // simulator shares): one in-place 16-byte RRAM line. Cheap, and skipped when nothing changed.
        if app.take_settings_dirty() {
            settings_store.save(app.settings());
            // Push a changed GPS fix interval to the sensor task → it re-VALSETs the M10's rate (#117).
            #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
            if app.settings().fix_interval_s != prev_interval {
                prev_interval = app.settings().fix_interval_s;
                sensor_link::set_rate(prev_interval);
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
        // state re-walk (#73). `has_track_action` is a non-consuming peek; the actual
        // `take_track_action` stays inside, so the one-shot is drained only when processed.
        let session = app.activity.session;
        if active != prev_active || session != prev_session || app.activity.has_track_action() {
            let action = app.activity.take_track_action();
            let mut name: heapless::String<64> = heapless::String::new();
            if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                let _ = name.push_str(&r.name);
            }
            storage.reconcile_route(active);
            storage.reconcile_track(action, session, &name);
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
        // on a fresh fix, the renderer on a redraw frame. Paired with the session-long `route_cache`.
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
        // BMP581 GPS + altimeter + temperature, coherent per fix (default, #218); or the SynthLocation
        // square loop, no other sensors (`synth`). `track_dyn` is consumed either way.
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
                clock: Some(&mut gps_clock), // SAM-M10Q UTC → the wall clock when "Set from GPS" is on (#223)
                compass: None,
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
        // previous frame couldn't service on a transient reader-build failure (#66).
        let mut dirty = app.take_dirty();
        dirty.map |= pending_map_redraw;
        pending_map_redraw = false;
        // While a hold *charges* on a cheap (non-map) screen — the factory-Reset prompt, the
        // hold-to-delete bar — redraw it each frame so its bar tracks the live progress, **and** once
        // more on the frame the hold drops back to 0 (the falling edge), so an early release clears
        // the bar instead of leaving it stuck mid-fill. A pure hold-charge (and a *cancelled* one)
        // emits no gesture, so nothing else dirties the map (issue #47). Gated on `!base_draws_map` so
        // the expensive map view is never re-rendered for a hold; there the overlay bulge is the live
        // feedback.
        if (hold_p > 0.0 || prev_hold_p > 0.0) && !app.base_draws_map() {
            dirty.map = true;
        }
        prev_hold_p = hold_p;

        // This frame's hold-bulge state, sampled once through the seam — `(false, None)` on ST7789
        // (its bulge rides the input plane); the live dirty edge + row span on the FLPR (issue #163).
        let (overlay_dirty, overlay_span) = display.poll_overlay();

        if dirty.map {
            // The map pipeline runs **only when the base screen actually draws the map** (the Map
            // view). On a menu / Statistics / Home redraw it's skipped entirely — no SD style-table
            // parse, no `Reader` build (so no stack spike), no map render — that screen draws just its
            // own chrome. A non-map frame costs only its own draw + the push.
            let needs_map = app.base_draws_map();
            // Build the streamed `Reader` **only** on a map frame, `None` otherwise. A *cheap* borrow
            // of the boot-parsed `MapTables` + a fresh `src` + the session-long `MapCache` (issue
            // #179) — no style-table SD read, no parse, no stack spike (what kept this deep path inside
            // the 256 KB stack). The only per-frame failure left is the source handle being momentarily
            // unavailable (a flaky SD link); skip the redraw, keep the last frame, latch a retry (#66).
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
                // frame under its bus lock; the FLPR scans it, going *around* a live bulge's rows so
                // the composite below paints them, issue #163). `render_map_timed` threads
                // `InstantClock` so the stats carry the collect/sort/draw timings; the hold bulge is
                // **not** composited here — it rides `present_bulge` on its own plane.
                let render = |d: &mut dyn DisplayDriver| {
                    let mut fbdev = FbDevice64::new(d.fb_mut(), WIDTH as u32, HEIGHT as u32);
                    app.render_map_timed(
                        &mut fbdev,
                        reader.as_ref(),
                        route.as_ref(),
                        WIDTH as f32,
                        HEIGHT as f32,
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
                        (app.state.viewport(WIDTH as f32, HEIGHT as f32).meters_per_pixel() * 1000.0) as u32;
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
                // reader-build failure (#66) rather than faulting.
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

        // The hold bulge (issue #163): the FLPR re-presents it from this map plane, compositing over
        // the clean map (the map present above clipped a live bulge's rows out via `present_within`, so
        // this paints them fresh — no mid-pop flash; the trailing clear is unconditional now that the
        // self-diffing present may skip those rows). On ST7789 this is a no-op — its bulge rides the
        // input/overlay plane on the shared bus.
        display.present_bulge(overlay_span, overlay_dirty);

        // Publish render-stats telemetry host-ward at ~2 Hz (#127): throttled here (not in the TX
        // task) so the link never floods and the device never stalls on it.
        #[cfg(feature = "debug-uart")]
        if now.wrapping_sub(last_telem_ms) >= 500 {
            last_telem_ms = now;
            obc_platform::debug_link::set_telemetry(last_telem);
        }

        if now.wrapping_sub(last_led) >= 500 {
            led.toggle();
            last_led = now;
        }
        Timer::after_millis(LOOP_MS).await;
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
        embassy_nrf::init(config)
    };

    // Paint the stack now (still shallow) so the ride loop's high-water guard can read the peak (#175).
    stackmeter::paint();

    // LED0 (P2_09) heartbeat — a liveness blink visible even before looking at the panel.
    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);

    // --- N6 load → ride → save-GPX (#127): stream the SD `.obcm` into the
    // resident RGB222 framebuffer through the shared `obc-app`, pick a route from the card catalog,
    // ride it (VCOM-streamed GPS or the `SynthLocation` square loop), map-match + log the track, and
    // write a `.gpx` to `/tracks` on Finish. Builds on N4's map (#125) + N5's two-plane input (#126):
    // the map plane (this loop) now also runs the full sensor → tick → reconcile → telemetry ride
    // loop, on a per-frame-reader structure. ---
    {
        // --- VCOM debug-sensor stream (#127), behind `debug-uart`. Bring it up first so the J-Link
        // VCOM is live while the SD card + panel come up; the parsed fixes land in obc-platform's
        // signals, ready for the app's sensor poll in the loop below. The nRF54L15 has no USB
        // peripheral, so the fake GPS/baro/compass feed and ride
        // telemetry ride UARTE20 on the DK's onboard J-Link VCOM (TX P1_04 / RX P1_05); defmt logs
        // share the same cable over RTT. The RX ring is interrupt-fed (`BufferedUarte`), so the
        // tens-of-ms map render never drops a byte. Without the feature the app rides the always-on
        // `SynthLocation` stand-in (no host needed). ---
        #[cfg(feature = "debug-uart")]
        {
            // 'static ring buffers backing the interrupt-fed UARTE (RX accumulates streamed bytes
            // across a long map render; TX queues the ≤192 B telemetry line). Parked in `.bss` and
            // written in place (the warm-reset-safe pattern used for the bus statics above), then the
            // `&'static mut` halves move into the spawned `'static` tasks.
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

        // microSD on its own SPIM (SERIAL22, P1 header — separate from the display bus on
        // SERIAL00/P2). Init ≤400 kHz (SD spec); `sd::init` re-clocks to 8 MHz once the card
        // answers. CS idles HIGH, then `init` holds it LOW for the session (the per-byte-CS
        // workaround — see `sd::NoCs`). `orc = 0xFF` so any over-read clocks the SD idle byte.
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
        let Some(mut storage) = sd::init(sd_spi, sd_cs) else {
            defmt::error!("SD: no card / mount failed — cannot load a map; idling with a heartbeat");
            idle_blink(&mut led).await
        };

        // Open the `.obcm` and hold it open for the session — the map **streams** from it (issue
        // #37), never read resident into the 256 KB part. (The `/routes/*.obcr` catalog is scanned
        // into the app's Route menu by `load_routes` *after* the app is built — in its own frame, so
        // the ~5 KB `Catalog` never sits on `main`'s stack beneath the long-lived ride loop, #175.)
        storage.open_map();

        // Place the streamed-map geometry cache in `.bss`, built in place (an all-zero
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary).
        // SAFETY: sole owner of MAP_CACHE; single executor → no aliasing.
        let map_cache: &MapCache = unsafe { init_static(core::ptr::addr_of_mut!(MAP_CACHE), MapCache::new()) };

        // Parse the OBCM **header + style table + LOD pyramid once at boot** into the resident
        // [`MAP_TABLES`] (issue #179). These tables are immutable for the session, so the loop's
        // per-frame readers *borrow* them instead of re-parsing — no per-frame style-table SD read and
        // no ~4 KB parse stack spike (a 1536-byte style scratch + the ~2.3 KB style array) on the deep
        // render path, which is what kept that path overrunning the 256 KB part's stack. The transient
        // parse cost is paid **here**, at boot, where the call stack is shallow; a missing or
        // structurally-bad map idles with a heartbeat (never faults). The idle camera centre is the
        // parsed bbox. `init_src`'s `storage` borrow ends with this block, so the loop can rebuild a
        // fresh source each redraw AND reconcile the card (`&mut storage`) between frames.
        // SAFETY: sole owner of MAP_TABLES; single executor → no aliasing; written exactly once here.
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
        let b = map_tables.bbox;
        info!(
            "map: streaming from SD; bbox lon[{=i32}..{=i32}] lat[{=i32}..{=i32}]",
            b.min_lon, b.max_lon, b.min_lat, b.max_lat
        );
        let (cam_lon, cam_lat) =
            (((b.min_lon as i64 + b.max_lon as i64) / 2) as i32, ((b.min_lat as i64 + b.max_lat as i64) / 2) as i32);

        // Boot to **Home** (issue #126): the user drives Home → Route menu → Map with the buttons.
        // Built **in place** in `.bss` (`init_idle` writes each field where it sits; the ~74 KB
        // renderer scratch is zeroed in place), never on the stack. The Route menu is filled from the
        // card's catalog scanned above; selecting an entry opens the Map at that route's start and
        // (N6, #127) streams its geometry into the render + the map-matcher.
        // SAFETY: sole owner of APP; `init_idle` fully initialises it before the `&mut` below reads it.
        let app: &mut App = unsafe {
            let slot = core::ptr::addr_of_mut!(APP) as *mut App;
            App::init_idle(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            &mut *slot
        };
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
        // The high-priority plane(s) run at P3 — above thread mode (so they preempt the map render)
        // and below the P0 GRTC time-driver (so their `Timer`s still wake mid-render). Shared vector.
        interrupt::SWI00.set_priority(Priority::P3);

        // --- Real GPS + altimeter on the shared TWIM30 I²C bus (issue #218). Default build only
        // (neither `synth` nor `debug-uart`). Build the bus + the TX-Ready interrupt line on the free
        // P0 pins and spawn the event-driven sensor task on the thread-mode executor; it probes both
        // chips, configures the M10, and publishes coherent (fix, altitude, temperature) datapoints
        // through `obc_platform::sensor_link`, which `run_app`'s `GpsLocation`/`BaroAltimeter`/
        // `SensorTemp` sources drain. The task is fully async (TWIM is DMA-backed), so it cooperates
        // with the loop; 1 Hz fix latency is a non-issue. SERIAL30's ISR runs at P3 (below the GRTC). ---
        #[cfg(all(not(feature = "debug-uart"), not(feature = "synth")))]
        {
            // EasyDMA can't fetch a write buffer from flash, so byte-literal register writes need a
            // RAM bounce buffer; 32 B covers the widest VALSET frame. Parked in `.bss` + written in
            // place (the warm-reset-safe pattern), then moved into the `Twim`.
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

        // ============= ST7789 backend (default): two-plane display + input/overlay (issue #126) =====
        // Build the panel + the shared `&'static` two-plane state, spawn the input/overlay plane (which
        // owns the bulge re-push), and hand the map plane back just the `bus` for the unified present.
        // Display on the (flash-freed) P2
        // header — CS idles HIGH and the driver pulses it low per transaction (the warm-reset-safe CSX
        // framing — see `st7789::St7789::transaction`); RST idles high. SERIAL00 write-only SPIM at
        // **32 MHz** (the max SERIAL00 reaches on the MCU-domain P2 pins) so a full-frame banded push
        // is ~38 ms; drop to `M16` if the jumpered bring-up bus sparkles. The shared state is parked in
        // `.bss` + written **in place** (the APP/MAP_CACHE pattern) rather than via `StaticCell`,
        // whose one-shot flag can panic "already full" on a warm reset.
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
            let diff: &'static mut RowDiff<{ HEIGHT as usize }> = unsafe { &mut *core::ptr::addr_of_mut!(ROW_DIFF) };
            info!("obc-fw-nrf54l N5: ST7789 up ({}x{}); two-plane input + map", WIDTH, HEIGHT);

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

            let input_spawner = EXECUTOR_INPUT.start(interrupt::SWI00);
            input_spawner.spawn(defmt::unwrap!(input_overlay_task(buttons, input_plane, bus, GESTURES.sender())));
            info!("input plane: SWI00 interrupt executor @ P3 (preempts the map render); map plane: thread mode");
            MapDisplay { bus }
        };

        // ============= FLPR LS021 backend (default; ST7789 is `--features tft`, issue #165) ==========
        // The map plane owns the `Ls021Flpr` panel directly (it scans a whole frame per push, so there
        // is no partial-window overlay to serialise — no bus mutex). The M33 configures every line the
        // FLPR drives (held as outputs for the program's life); `com_task` + the gesture `input_task`
        // share the one high-priority executor (COM must keep alternating during the blocking push).
        //
        // ⚠️ **Gate/BSP pins relocated off the SD/VCOM bus for the integration.** These five P1 lines
        // **must match `src/flpr/flpr_pingpong.c`'s masks** — confirm each is broken out on your DK and
        // remap all three together if not (the source bus, BCK, and COM stay on P2).
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
            // COM lines as high-drive GPIO (56–77 nF load each), boot `Lo`; held `Lo` through the init
            // frame, then moved into `com_task`. VCOM=P2.07, VB=P2.08, VA=P2.10 (M33-driven).
            let vcom = Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive);
            let vb = Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive);
            let va = Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive);

            // Launch the FLPR (copy the blob, arm the control block, wait ALIVE). A launch failure must
            // never fault — degrade to a heartbeat idle, exactly like a missing/bad map card.
            match launch_flpr().await {
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
            // self-diffing present store the masked push derives its dirty rows from (issue #201).
            // SAFETY: sole references to FB / ROW_DIFF; held by `panel` for the rest of the program
            // (the map plane is their only owner), never aliased.
            let fb: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
            let diff: &'static mut RowDiff<{ HEIGHT as usize }> = unsafe { &mut *core::ptr::addr_of_mut!(ROW_DIFF) };
            let mut panel = Ls021Flpr::new_fb(fb, diff);
            // Datasheet Initial #0: an INTB-framed all-black frame (FB boots zeroed = black) while COM
            // is still held `Lo`. Then T4 ≥ 30 µs, then start COM — from here it free-runs forever.
            panel.push_frame();
            Timer::after_micros(50).await;

            // The shared `InputPlane` (issue #163): `input_task` recognises + animates the bulge under
            // this lock; the map plane composites it into a partial overlay push. Parked in `.bss` +
            // written **in place** (the APP/FB pattern, not `StaticCell` — its one-shot flag can panic
            // "already full" on a warm reset).
            static mut INPUT_PLANE: MaybeUninit<BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>> =
                MaybeUninit::uninit();
            // SAFETY: sole writer; initialised before the `&'static` is shared with the input plane,
            // never rewritten (single executor builds it, two planes only read it).
            let input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>> = unsafe {
                init_static(core::ptr::addr_of_mut!(INPUT_PLANE), BlockingMutex::new(RefCell::new(InputPlane::new())))
            };

            let hp = EXECUTOR_HP.start(interrupt::SWI00);
            hp.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
            hp.spawn(defmt::unwrap!(input_task(buttons, input_plane, GESTURES.sender())));
            info!("FLPR LS021: COM + gesture/bulge plane on SWI00 @ P3; map plane: thread mode (blocking masked push)");
            MapDisplay { panel, input_plane, last_overlay_span: None, _gate_bus: gate_bus, _src_bus: src_bus }
        };

        // Place the decoded-route-geometry cache in `.bss`, built in place (a zeroed
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary — like `MAP_CACHE`).
        // SAFETY: sole owner of ROUTE_CACHE; single map plane → no aliasing.
        let route_cache: &RouteCache = unsafe { init_static(core::ptr::addr_of_mut!(ROUTE_CACHE), RouteCache::new()) };

        // The persistent settings store (#193): takes the `RRAMC` peripheral, reads/writes the
        // 16-byte blob in the carved RRAM page. Built here (where `p` is live) and moved into the
        // ride loop, which seeds the app at boot and saves on a settings edit.
        let settings_store = settings::RramSettingsStore::new(p.RRAMC);

        // Hand the built display + the resident set to the shared, backend-agnostic ride loop. The
        // `display` (one of the two `MapDisplay` definitions) is the only per-backend value crossing
        // this seam; the loop drives present through it with no further `#[cfg]` (issue #175). The
        // `debug-uart` split is the host-feed *feature*, not the backend: it threads the OBCM bbox
        // centre through for the `SynthLocation` stand-in when no host GPS is streamed.
        // `cam_center` is threaded only on the `synth` build (the host feed + the real GPS stream
        // absolute positions, so they need no synthetic-loop centre).
        #[cfg(any(feature = "debug-uart", not(feature = "synth")))]
        run_app(display, app, &mut storage, map_tables, map_cache, route_cache, &mut led, settings_store).await;
        #[cfg(all(not(feature = "debug-uart"), feature = "synth"))]
        run_app(
            display,
            app,
            &mut storage,
            map_tables,
            map_cache,
            route_cache,
            &mut led,
            settings_store,
            (cam_lon, cam_lat),
        )
        .await;
    }
}
