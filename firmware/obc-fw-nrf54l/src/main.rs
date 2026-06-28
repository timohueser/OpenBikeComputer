//! nRF54L15-DK board firmware for OpenBikeComputer — the **real hardware target**.
//!
//! Unlike the STM32F429 prototype (a bridge that made the HAL seams concrete), the
//! nRF54L15 + ST7789 EYESPI panel is what the project ships on. This crate ports the
//! shared `obc-app` onto it to STM32-prototype parity (load route → ride → save GPX on
//! glass, fake-sensor fed). Nothing app-facing lives here: `obc-render` / `obc-app` /
//! `obc-reader` / `obc-route` + `obc-platform` stay board-agnostic; only the nRF HAL
//! wiring + the ST7789 `Panel` backend are board-specific. See epic #120.
//!
//! Bring-up is phased so each hardware layer is verified (over defmt/RTT, and on glass via
//! the webcam capture at `/tmp/obc-cam/panel.jpg`) before the next is stacked:
//!   N0. crate skeleton + embassy bring-up: blinky + RTT + this peripheral plan          (#121)
//!   N1. `Panel` HAL + ST7789 SPIM backend + banded RGB565 push + glass demo (RGB222 at N4)    (#122)
//!   N2. microSD on a dedicated SPIM (reuse obc-platform's FatFs byte adapters)            <- (#123)
//!   N3. board memory profile (host-vs-nRF) + budget assert                              (#124)
//!   N4. RGB222 full framebuffer + full map on glass (retire `Framebuffer565`)           (#125)
//!   N5. buttons + two-plane InterruptExecutor + fluid composite-on-push bulge               (#126)
//!   N6. debug/sensor stream over VCOM UART + load→ride→save-GPX = PARITY                 <- (#127)
//!   N7. docs + CI (add nRF; drop STM32 from the required check)                         (#128)
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
//! recogniser, exactly like the STM32's four GPIO buttons. Roles (N5): BTN0/1 → encoder
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
//!   framebuffer → RGB565 and SPIM-DMAs a CASET/RASET window (the wire format lives in
//!   `Panel::flush_band`, the same seam the future FLPR/LS021B7DD02 reuses); the RGB222 source
//!   framebuffer lands at N4. N1 already drives that `Panel` seam (in RGB565) with a banded
//!   `glass-demo` — the font ladder + the device's 64-colour gamut — to prove wiring + init +
//!   addressing + the colour/text path end-to-end.
//!
//! ## microSD SPIM — map/route/track storage (#123)
//!   Instance **SERIAL22 / SPIM22** — a standard-speed instance (SD doesn't need 32 MHz),
//!   *separate* from the display bus, on its own software CS. DK expansion-header SPI pins:
//!     SCK P1_11 | MISO P1_07 | MOSI P1_06 | CS P1_12   (FLPR build moves CS → P0_00, see #165 below)
//!   CS is a free GPIO held LOW for the whole session (the held-low-CS workaround embedded-sdmmc's
//!   per-byte framing needs over embassy SPI — see `sd::NoCs`); the bus inits ≤400 kHz then
//!   re-clocks to 8 MHz (`sd::init`). embassy-nrf's `Spim` exposes **no** internal MISO pull-up
//!   (unlike embassy-stm32), so the card's DO line is pulled high by the breakout (or an external
//!   10 kΩ to 3V3). The EYESPI connector also carries a microSD that *shares the display bus*; we
//!   leave that slot **unpopulated** and use this dedicated SPIM instead (a clean reuse of the
//!   STM32's standalone SD-over-SPI reader + obc-platform's FatFs adapters). P1_06/P1_07 are the
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
//!   so — unlike the STM32's second USB-CDC port — the fake GPS/baro/compass feed and ride
//!   telemetry ride this UART; defmt logs ride RTT on the same cable. obc-platform's debug-source
//!   protocol is transport-agnostic, so it moves over from USB unchanged.
//!
//! ## Spare interrupt for the high-priority InterruptExecutor (#126)
//!   The two-plane architecture runs input + the overlay on a high-priority `InterruptExecutor`
//!   that preempts the map render. On STM32 that executor was pended from the unused UART5
//!   vector; the nRF analog is a dedicated **software-interrupt vector**: **SWI00** (the M33 also
//!   has SWI01/02/03 + EGU10/EGU20 free). N5 runs it at **P3** — above thread mode (so it preempts
//!   the map render) but below the P0 GRTC time-driver (so `Timer`s still wake mid-render).
//!
//! ## Flash / RAM
//!   From the `nrf54l15-app-s` `memory.x`: FLASH 1524K @ 0x0000_0000, RAM 256K @ 0x2000_0000.
//!   A future MCUboot retrofit re-partitions flash — don't hard-code flash assumptions (see
//!   `memory.x` and epic #120). RAM is tight (no external SDRAM, unlike the STM32 prototype):
//!   the single RGB222 framebuffer is ~75 KB and the renderer scratch + caches must fit the
//!   rest — the board memory profile + budget assert is N3 (#124).
//! =============================================================================

#![no_std]
#![no_main]

// glass-demo (#122): the font/palette panel bring-up, drawn per band (no SD, no full framebuffer).
#[cfg(feature = "glass-demo")]
mod demo;
// microSD map storage (#123): only the default (map) build touches the card — the glass-demo panel
// bring-up needs no SD. The module carries its own dead-code allow for the route/track write half
// that the app wires in at N6 (#127).
#[cfg(not(feature = "glass-demo"))]
mod sd;
// The ST7789 panel geometry (`WIDTH`/`HEIGHT`) is shared by both display backends; the `St7789`
// driver itself is the default-build map/glass-demo backend (the FLPR build below replaces it).
mod st7789;
// LS021 FLPR backend (issue #165): build `main.rs` with `--features panel-ls021` to run the real app
// on the reflective LS021 panel via the FLPR (the VPR coprocessor) instead of the ST7789. The FLPR
// `Panel` backend + launch live in `ls021_flpr`; `ls021::com_task` free-runs the COM lines (the rest
// of `ls021` — the M33-direct `PanelBus` — is unused here, hence the dead-code allow).
#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
#[allow(dead_code)]
mod ls021;
#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
mod ls021_flpr;
// The board's display-driver seam — the single screen-write interface both panels implement, so the
// map plane drives either through one path (`fb_mut` + `present`). The glass-demo bring-up draws
// straight through the `Panel` band seam, so it doesn't pull this in.
#[cfg(not(feature = "glass-demo"))]
mod display;

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::Timer;
use st7789::{HEIGHT, WIDTH};
use {defmt_rtt as _, panic_probe as _};

// `Delay` (the ST7789 power-on waits) + the `St7789` driver type are the default (and glass-demo)
// backend; the FLPR build replaces them with the `Ls021Flpr` `Panel` backend, so neither is there.
#[cfg(not(feature = "panel-ls021"))]
use embassy_time::Delay;
#[cfg(not(feature = "panel-ls021"))]
use st7789::St7789;

// The display panel + the banded `Panel` push seam, the per-band/per-window frame-absolute draw
// view (`Band` — the glass-demo draws the whole frame through it; the map path composites the hold
// bulge through it), and its frame `Size` are common to both builds.
// `glass-demo` and `panel-ls021` are mutually exclusive display backends — guarding it here lets the
// rest of the file treat `panel-ls021` as implying `not(glass-demo)`.
#[cfg(all(feature = "glass-demo", feature = "panel-ls021"))]
compile_error!("`glass-demo` and `panel-ls021` are mutually exclusive display backends");

// The frame-absolute draw view `Band` is used by every build — the glass-demo draws the whole frame
// through it, and the `present_overlay` drawer paints the hold bulge into it on both map panels
// (issue #163). `Size` (the frame size for `Band::new_window`) + the banded `Panel` trait are the
// ST7789 / glass-demo path's: the ST7789 map composites the bulge window + bands the framebuffer to
// the panel, and the glass-demo draws bands through `Panel`; the FLPR path renders device-64 straight
// into its plane (no banding) and composites inside `Ls021Flpr::push_overlay`.
#[cfg(not(feature = "panel-ls021"))]
use embedded_graphics::prelude::Size;
use obc_platform::Band;
#[cfg(not(feature = "panel-ls021"))]
use obc_platform::Panel;

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
#[cfg(not(feature = "glass-demo"))]
use core::cell::RefCell;
#[cfg(not(feature = "glass-demo"))]
use core::mem::MaybeUninit;
#[cfg(not(feature = "glass-demo"))]
use display::{DisplayDriver, OverlayRegion};
#[cfg(not(feature = "glass-demo"))]
use embassy_executor::InterruptExecutor;
#[cfg(not(feature = "glass-demo"))]
use embassy_nrf::gpio::{Input, Pull};
#[cfg(not(feature = "glass-demo"))]
use embassy_nrf::interrupt;
#[cfg(not(feature = "glass-demo"))]
use embassy_nrf::interrupt::{InterruptExt, Priority};
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
use embassy_nrf::spim::Spim;
#[cfg(not(feature = "glass-demo"))]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(not(feature = "glass-demo"))]
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
#[cfg(not(feature = "glass-demo"))]
use embassy_sync::channel::{Channel, Sender};
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
use embassy_sync::mutex::Mutex;
#[cfg(not(feature = "glass-demo"))]
use embassy_time::Instant;
#[cfg(not(feature = "glass-demo"))]
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
#[cfg(not(feature = "glass-demo"))]
use obc_app::{App, AppState, Gesture, InputClock, InputEvent, InputPlane, InputSource, RideClock, Sensors, TrackSink};
#[cfg(not(feature = "glass-demo"))]
use obc_platform::{ButtonInput, FbDevice64};
// `device64_to_rgb565` expands the RGB222 framebuffer to RGB565 for the ST7789 bulge composite +
// overlay window — the FLPR map path packs device-64 straight to the wire, so it isn't needed there.
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
use obc_platform::device64_to_rgb565;
#[cfg(not(feature = "glass-demo"))]
use obc_reader::{MapCache, Reader};
#[cfg(not(feature = "glass-demo"))]
use obc_render::zoom_for_mpp;
// The N6 ride loop (#127): the decoded-route-geometry cache, the resident per-route chunk index,
// and the streamed route reader the matcher + map render share — the same per-frame structure the
// STM32 ride loop uses.
#[cfg(not(feature = "glass-demo"))]
use obc_route::{RouteCache, RouteIndex, RouteReader};
// The runtime-built shared statics (the bus `Mutex`, the `InputPlane` mutex, the VCOM ring buffers)
// are parked in `.bss` with the same in-place `MaybeUninit` + `ptr::write` pattern as APP/MAP_CACHE/
// ROUTE_CACHE — see `main`. Earlier this used `StaticCell`, but its one-shot `used` flag panics
// ("already full") if it's ever non-zero on entry, which on this board's debug-reset path it was; an
// unconditional in-place write has no such flag, so it survives a warm reset. (Hence no `static_cell`
// dependency.)
// The `debug-uart`-off fallback's stand-in GPS (`SynthLocation`) — always-compiled in obc-platform
// (it *is* the no-feed path), so it's imported only when wired (the VCOM build streams a real ride
// instead). Walks a slow square loop so a saved ride is a non-degenerate `.gpx`.
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-uart")))]
use obc_platform::SynthLocation;

// LS021 FLPR backend (issue #165): the resident-framebuffer `Panel` backend + its launch, and the
// free-running COM driver. The FLPR scans the whole frame in one push, so there is no banded ST7789
// `Display`/bus mutex here — the map plane owns the panel, COM + the gesture-input plane run on the
// shared high-priority executor (see `main`).
#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
use ls021::com_task;
#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
use ls021_flpr::{launch_flpr, FlprError, Ls021Flpr};

// VCOM debug-sensor / telemetry stream (#127), behind `debug-uart`: the interrupt-buffered UARTE on
// the DK's J-Link VCOM. `BufferedUarte` keeps RX DMA continuously armed into a ring driven by the
// SERIAL20 interrupt, so the tens-of-ms map render never drops a streamed byte (the nRF analog of
// the STM32's interrupt-buffered OTG RX). `uarte` carries the shared `Config` (8N1 @ 115200).
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

/// One band's worth of RGB565 scratch (`WIDTH * BAND_ROWS`), living in `.bss`. 14 rows ≈ 6.6 KB
/// and tiles the 320-row frame in 23 bands. The `Panel` impl fills it — the map path expands the
/// RGB222 frame into it, the glass-demo draws the frame per band — then byte-swaps to big-endian
/// and SPIM-DMAs it; borrowed exactly once below (single executor → no aliasing). `BAND_BYTES` is
/// what the N3 budget assert reserves. (N6, #127: 20→14, freeing ~2.9 KB toward the ride-loop
/// residents + the deep render path's stack; must stay ≥ the overlay window `OVL_W × OVL_ROWS`,
/// asserted below — 240×14 = 3360 ≥ 16×192 = 3072.)
///
/// **ST7789-only.** The FLPR map path (`--features panel-ls021`, issue #165) packs the RGB222
/// framebuffer straight to the LS021 wire (`ls021_pack_row`) and renders device-64 directly, so it
/// needs no RGB565 band scratch — freeing these ~6.6 KB is one of the levers that fits the app in the
/// 244 KB the FLPR carve-out leaves (the FLPR build drops `BAND_BYTES` from the budget below).
#[cfg(not(feature = "panel-ls021"))]
const BAND_ROWS: usize = 14;
/// The band scratch in bytes (RGB565, 2 B/px) — the resident cost the budget assert reserves.
#[cfg(not(feature = "panel-ls021"))]
const BAND_BYTES: usize = st7789::WIDTH as usize * BAND_ROWS * 2;
#[cfg(not(feature = "panel-ls021"))]
static mut BAND: [u16; WIDTH as usize * BAND_ROWS] = [0; WIDTH as usize * BAND_ROWS];

// ============================ N3 board memory budget (issue #124) ============================
// The nRF54L15 has 256 KB RAM and no external SDRAM (unlike the STM32 prototype's 8 MB), so the
// whole resident working set of a full map redraw must fit there. This build-time assert is the
// nRF analog of the STM32's SDRAM-placement guard (`obc-fw-stm32f429/src/main.rs`): it fails the
// build — rather than overflowing RAM on glass — if the shared crates' caps (trimmed by the
// `nrf-mem` profile, enabled on the obc-app edge in Cargo.toml) ever outgrow the budget. It
// compiles for thumbv8m (usize = 4 B), so every `size_of` here is the true on-device size.
//
// The binding moment is a full redraw with everything resident at once (the `nrf-mem` profile
// trims each term — see the per-crate caps — to make this fit):
//   - `App`        embeds the renderer scratch (`obc_render::MCU_RENDERER_BYTES`, ~66 KB nrf-mem)
//                  plus the resident elevation `Profile` (~4.6 KB at PROFILE_COLS=512) and
//                  `Breadcrumb` (~6 KB at SPINE_CAP=512); ~88 KB total.
//   - framebuffer  the single RGB222 frame (#N4): 240×320 × 1 B/px = 75 KB — the `FB` static below
//                  (the map path) renders into it; the glass-demo build reserves the budget without
//                  allocating it, since it draws per band.
//   - `MapCache`   the streamed-map geometry-chunk cache (3 slots on nrf-mem, ~25 KB).
//   - `RouteCache` the decoded-route-chunk cache (3 slots on nrf-mem, ~9 KB — trimmed 4→3 as a
//                  256 KB-DK stop-gap to claw ~3 KB back for the deep ride-loop render stack below).
//   - `RouteIndex` the active route's resident chunk index — the ride loop (#127) holds it across
//                  frames in the map plane's task future to stream geometry without re-walking it
//                  (128 chunks on nrf-mem, ~6 KB). Counted here because, unlike the host/STM32
//                  (8 MB SDRAM), on the 256 KB part it materially shares the budget — and because
//                  `RouteIndex::read` builds it on the *stack*, so keeping it ~6 KB also keeps that
//                  transient build spike inside the stack reserve below.
//   - band scratch one RGB565 `Panel` band (`BAND_BYTES`, ~7.5 KB).
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

/// Total SRAM the M33 app core sees. The default ST7789 build links the full 256 KB
/// (`memory.x`: RAM 256K @ 0x2000_0000); the FLPR build (`--features panel-ls021`) links only 244 KB
/// — the top 12 KB is the carved FLPR image + the shared handshake page (`build.rs` / `flpr.ld`).
#[cfg(not(feature = "panel-ls021"))]
const NRF_RAM_BYTES: usize = 256 * 1024;
#[cfg(feature = "panel-ls021")]
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
/// part it shares the budget like the caches do (the STM32 parks it in 8 MB SDRAM, so its guard
/// omits it).
const RESIDENT_BYTES: usize = core::mem::size_of::<obc_app::App>()
    + FB_BYTES
    + core::mem::size_of::<obc_reader::MapCache>()
    + core::mem::size_of::<obc_route::RouteCache>()
    + core::mem::size_of::<obc_route::RouteIndex>()
    + BAND_RESERVE;
/// The RGB565 band scratch the budget reserves: `BAND_BYTES` on the ST7789 path, **zero** on the FLPR
/// path (it packs the framebuffer straight to the wire — see [`BAND_ROWS`]).
#[cfg(not(feature = "panel-ls021"))]
const BAND_RESERVE: usize = BAND_BYTES;
#[cfg(feature = "panel-ls021")]
const BAND_RESERVE: usize = 0;
const _: () = assert!(
    RESIDENT_BYTES + STACK_RESERVE <= NRF_RAM_BYTES,
    "nRF resident set (App + framebuffer + MapCache + RouteCache + RouteIndex + band) + stack reserve overruns RAM — trim the `nrf-mem` caps (issue #124)"
);

// ============================ N4 resident set + map path (issue #125) ============================

/// The resident device-native RGB222 framebuffer: one byte per pixel over the 240×320 panel
/// (`FB_BYTES` = 75 KB), in `.bss`. [`App::render_map`](obc_app::App::render_map) quantizes into it
/// on store ([`FbDevice64`]). On the **ST7789** path it is borrowed into the [`Display`] behind
/// [`BUS`] and the band push expands it back to RGB565, so the two planes (#126) reach it only under
/// that mutex (no aliasing, no torn frame). On the **FLPR** path (`panel-ls021`, issue #165) it is
/// owned by the `Ls021Flpr` panel — the map plane renders into it and `push_frame` packs it straight
/// to the LS021 wire. Map-path only — the glass-demo draws per band, no full frame.
#[cfg(not(feature = "glass-demo"))]
static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// The streamed-map geometry cache + the shared [`App`], placed in `.bss` and built **in place** —
/// the nRF analog of the STM32's `ptr::write`-into-SDRAM placement: the ~96 KB `App` (incl. the
/// ~74 KB renderer scratch) and the ~41 KB cache must never form on the 256 KB part's small stack.
/// [`MapCache::new`](obc_reader::MapCache) is an all-zero `MaybeUninit::zeroed`, so writing it is a
/// `.bss` memset; [`App::init_map`](obc_app::App::init_map) writes each field where it sits. (The
/// `RouteCache` the budget assert reserves is allocated when route loading lands at N6.)
#[cfg(not(feature = "glass-demo"))]
static mut MAP_CACHE: MaybeUninit<MapCache> = MaybeUninit::uninit();
#[cfg(not(feature = "glass-demo"))]
static mut APP: MaybeUninit<App> = MaybeUninit::uninit();
/// The decoded-route-geometry cache (#127), placed in `.bss` and built in place like [`MAP_CACHE`]
/// ([`RouteCache::new`](obc_route::RouteCache) is an all-zero `MaybeUninit::zeroed` → a `.bss`
/// memset, never a stack temporary). The session-long cache (issue #98 P4) a redraw of the
/// unchanged route + the matcher's per-fix decode hit instead of re-reading `.obcr` geometry off
/// the card every frame; the budget assert above already reserves its bytes.
#[cfg(not(feature = "glass-demo"))]
static mut ROUTE_CACHE: MaybeUninit<RouteCache> = MaybeUninit::uninit();

/// Idle camera zoom for the N4 static map, in ground metres-per-pixel (the 0.5–4 mpp riding band).
/// A coarse-ish 2 mpp shows a town-scale overview — several roads / landuse polygons, so the
/// 64-colour gamut is visible at a glance — rather than a tight patch. Freely tunable: the frame is
/// a single static render until buttons land (#126).
#[cfg(not(feature = "glass-demo"))]
const INIT_MPP: f32 = 2.0;

/// Heartbeat-only idle for an unrecoverable bring-up failure (no card, no `.obcm`, or a map that
/// isn't valid OBCM): blink LED0 forever rather than panic — a missing/bad card must **never** fault
/// (acceptance criterion, carried from the STM32). Diverges, so the map path below is unreachable
/// after it.
#[cfg(not(feature = "glass-demo"))]
async fn idle_blink(led: &mut Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

// ============================ N5 two-plane input + overlay (issue #126) ============================
// The map render (`render_map` + the banded push) is a CPU- and SPI-bound call that would block its
// executor for tens of ms. To keep input + the hold bulge responsive *during* that, the device runs
// two planes — the STM32 #48 structure, adapted from two LTDC layers to one shared SPI panel:
//   - Map plane (thread mode, the `main` loop): drains the gesture channel → `apply_gesture`,
//     advances screen animations, and re-renders the map only on `dirty.map`, compositing the live
//     bulge into each pushed band.
//   - Input plane (`input_overlay_task`, on a high-priority `InterruptExecutor` pended from SWI00):
//     samples the buttons, recognises gestures (into the channel), and re-pushes just the right-edge
//     overlay window so the bulge animates over a static map at full FPS with no map re-render.
// The shared resource is the panel SPI bus + the framebuffer (the nRF analog of the STM32 LTDC vblank
// bit): the async `BUS` mutex serialises pushes — and, since the map render runs inside it, the
// framebuffer write against the input plane's window read — without disabling interrupts (the GRTC
// time-driver + the input executor keep running while it's held). Keeping the framebuffer *inside*
// the mutex means the input plane never reads a half-rendered frame, so the bulge backdrop is always
// clean (no tearing); the cost is that a long map render holds the bus, so the bulge can briefly
// "stick" while a big segment repaints — a price worth paying for an artifact-free overlay, since a
// fluid-but-unlocked framebuffer tore the bulge. The `InputPlane` both planes draw the bulge from is
// behind a brief blocking mutex; lock order is always BUS-outer, INPUT_PLANE-inner.

/// The concrete display panel type behind [`BUS`]: the ST7789 over the SERIAL00 SPIM, its three GPIO
/// control lines, and the `'static` RGB565 band scratch. Named so [`Display`] + the mutex can be
/// `'static`. (`Spim`/`Output` aren't generic over the instance, so this fully specifies the type.)
/// ST7789-only — the FLPR build's map plane owns its `Ls021Flpr` panel directly (no two-plane bus
/// mutex, since there is no partial-window overlay push to serialise against).
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
type DisplayPanel = St7789<'static, Spim<'static>, Output<'static>, Output<'static>, Output<'static>, Delay>;

/// The shared display the two planes split: the ST7789 panel + the resident RGB222 framebuffer it
/// pushes. Both reach both only through [`BUS`], so the map render's framebuffer write is serialised
/// against the input plane's overlay-window read — the bulge backdrop is never torn.
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
struct Display {
    panel: DisplayPanel,
    fb: &'static mut [u8],
}

#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
impl DisplayDriver for Display {
    fn fb_mut(&mut self) -> &mut [u8] {
        self.fb
    }

    /// Push the whole RGB222 framebuffer to the ST7789, band by band, **straight from RGB222 to the
    /// panel's 12-bit RGB444 wire** ([`flush_band_rgb222`](st7789::St7789::flush_band_rgb222) — the
    /// fast path, no RGB565 intermediate). The transient hold bulge is **not** drawn here: it rides
    /// its own overlay re-push on the input plane (issue #126/#163), so the map present stays a single
    /// clean pack with no overlay coupling. ST7789 GRAM writes don't fault, so always `true`.
    fn present(&mut self) -> bool {
        let Display { panel, fb } = self;
        st7789::reset_push_timers();
        panel.begin_frame();
        let rows = panel.band_rows();
        let mut y0 = 0u16;
        while y0 < HEIGHT {
            let h = rows.min(HEIGHT - y0);
            let row0 = y0 as usize * WIDTH as usize;
            let n = WIDTH as usize * h as usize;
            panel.flush_band_rgb222(y0, h, &fb[row0..row0 + n]);
            y0 += h;
        }
        panel.end_frame();
        let (fill_us, pack_us, spi_us) = st7789::push_timers();
        defmt::info!("ST7789 push: fill {=u32} + pack {=u32} + spi {=u32} us", fill_us, pack_us, spi_us);
        true
    }

    /// Re-push just the overlay rectangle: the ST7789 is column-addressable, so it addresses exactly
    /// the `region` window (`flush_window`). Fill the scratch from the clean framebuffer backdrop
    /// (RGB222 → RGB565), composite the bulge over it via `draw_overlay`, then DMA the window — no map
    /// re-render, no torn frame (the caller holds the bus). GRAM writes don't fault, so always `true`.
    fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool {
        let Display { panel, fb } = self;
        let fb: &[u8] = fb;
        panel.flush_window(region.x0, region.y0, region.w, region.rows, |scratch| {
            let w = region.w as usize;
            for row in 0..region.rows as usize {
                let fb_row = (region.y0 as usize + row) * WIDTH as usize + region.x0 as usize;
                let dst = &mut scratch[row * w..(row + 1) * w];
                for (px, &byte) in dst.iter_mut().zip(&fb[fb_row..fb_row + w]) {
                    *px = device64_to_rgb565(byte);
                }
            }
            let mut band = Band::new_window(
                scratch,
                Size::new(WIDTH as u32, HEIGHT as u32),
                region.x0,
                region.y0,
                region.w,
                region.rows,
            );
            draw_overlay(&mut band);
        });
        true
    }
}

/// The FLPR LS021 backend behind the same seam: the app renders into the resident plane, then this
/// drives it in one masked full-frame push (the FLPR scans all 320 rows). The hold bulge is **not**
/// composited here — it rides `present_overlay` (issue #163). A stalled FLPR returns `false` so the
/// caller keeps the last frame and retries.
#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
impl DisplayDriver for Ls021Flpr<'_> {
    fn fb_mut(&mut self) -> &mut [u8] {
        Ls021Flpr::fb_mut(self)
    }
    fn present(&mut self) -> bool {
        self.push_frame()
    }

    /// Re-present the overlay rectangle's **rows** with the bulge composited (issue #163): the LS021
    /// can't latch a sub-span of columns, so the FLPR rewrites the full-width rows `[y0, y0+rows)`
    /// (only `[x0, x0+w)` carry the overlay) and fast-forwards the gate over the rest — see
    /// [`push_overlay`](Ls021Flpr::push_overlay) for the stack-frugal, lock-once composite.
    fn present_overlay(&mut self, region: OverlayRegion, draw_overlay: &mut dyn FnMut(&mut Band)) -> bool {
        self.push_overlay(region.x0, region.y0, region.w, region.rows, draw_overlay)
    }
}

/// Bound of the input→map gesture channel. One frame yields a couple of gestures and the map plane
/// drains it each loop, so even across a slow map push it never fills; `try_send` drops on the
/// (unreachable) overflow rather than block the high-priority plane. (Mirrors the STM32 GESTURE_QUEUE.)
#[cfg(not(feature = "glass-demo"))]
const GESTURE_QUEUE: usize = 16;

/// Recognised gestures flowing from the input plane (high priority) to the map plane (thread mode) —
/// the only lock-free shared state between the two planes.
#[cfg(not(feature = "glass-demo"))]
static GESTURES: Channel<CriticalSectionRawMutex, Gesture, GESTURE_QUEUE> = Channel::new();

/// The high-priority executor the input/overlay plane runs on, pended from the SWI00 vector (an
/// unused software-interrupt line — see the module doc). Started in `main`; driven by the SWI00 ISR.
/// The FLPR build uses [`EXECUTOR_HP`] instead (it also free-runs COM there).
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
static EXECUTOR_INPUT: InterruptExecutor = InterruptExecutor::new();

/// SWI00 ISR → poll the input-plane executor. SWI00 has no peripheral; we only borrow its interrupt
/// vector as the executor's pend line (its priority is set + the executor started in `main`).
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_INPUT.on_interrupt();
}

/// The FLPR build's single high-priority executor (issue #165): it free-runs **both** the COM driver
/// (which must keep alternating `VCOM`/`VB`/`VA` so the panel never DC-biases, even while the M33
/// busy-polls a frame push) **and** the gesture-input plane (so button latency stays exact during the
/// ~97 ms blocking whole-frame push). Pended from the same unused SWI00 vector @ P3.
#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
static EXECUTOR_HP: InterruptExecutor = InterruptExecutor::new();

#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_HP.on_interrupt();
}

/// Input-plane loop period (ms): buttons sampled + gestures recognised + the bulge animated this
/// often, on the high-priority executor that preempts the map render — so press-to-feedback latency
/// and the auto-repeat cadence stay exact regardless of how long a map frame takes.
#[cfg(not(feature = "glass-demo"))]
const LOOP_MS: u64 = 8;

// The hold-bulge's right-edge overlay **columns** (issue #126/#163). Both bulges erupt from the right
// screen edge ≤12 px deep, so this fixed 16-px column band bounds them with margin. Both map panels
// re-present the bulge through `DisplayDriver::present_overlay` over the clean framebuffer, addressing
// only the live bulge's *rows* (`InputPlane::overlay_rows`: encoder ≈ 59–171, Back ≈ 182–246) — ST7789
// a 16-px column window, the FLPR the full-width rows of that span — so the column constants are
// shared; the fixed row band (`OVL_Y0`/`OVL_ROWS`) is only the ST7789 trailing-clear + band-fit bound.
/// First overlay column: the rightmost 16 px (bulge depth ≤12 + margin).
#[cfg(not(feature = "glass-demo"))]
const OVL_X0: u16 = WIDTH - 16;
/// Overlay window width (columns).
#[cfg(not(feature = "glass-demo"))]
const OVL_W: u16 = 16;
/// First overlay row of the full hint band (a little above the encoder bulge's top). ST7789-only — the
/// FLPR addresses the *live* bulge's rows (`InputPlane::overlay_rows`), never this fixed top.
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
const OVL_Y0: u16 = 56;
/// Full hint-band height in rows (down past the Back bulge's bottom) — the ST7789 trailing-clear span
/// and the band-fit bound below. The FLPR has its own `MAX_OVERLAY_*` bound in `Ls021Flpr`.
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
const OVL_ROWS: u16 = 192;
/// ST7789: the overlay window must fit the shared band scratch (it borrows a prefix of it). The FLPR
/// path has its own `MAX_OVERLAY_*` bound in `Ls021Flpr::push_overlay`.
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
const _: () =
    assert!(OVL_W as usize * OVL_ROWS as usize <= WIDTH as usize * BAND_ROWS, "overlay window larger than BAND");

/// Chains two input sources for the gesture recogniser: drains `a` (the physical buttons) fully,
/// then `b` (the VCOM-injected `K` events with `debug-uart`, else [`NullInput`]). So a host can
/// drive the UI (taps/holds) over the same VCOM link, interleaved with real presses — exactly the
/// STM32's `ChainedInput`.
#[cfg(not(feature = "glass-demo"))]
struct ChainedInput<'a> {
    a: &'a mut dyn InputSource,
    b: &'a mut dyn InputSource,
}
#[cfg(not(feature = "glass-demo"))]
impl InputSource for ChainedInput<'_> {
    fn poll(&mut self) -> Option<InputEvent> {
        self.a.poll().or_else(|| self.b.poll())
    }
}

/// A never-yielding input source — the `debug-uart`-off stand-in for the VCOM-injected stream, so
/// the recogniser call site is one code path in both builds.
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-uart")))]
struct NullInput;
#[cfg(all(not(feature = "glass-demo"), not(feature = "debug-uart")))]
impl InputSource for NullInput {
    fn poll(&mut self) -> Option<InputEvent> {
        None
    }
}

/// The VCOM-injected input stream to chain after the physical buttons: the `debug-uart` source that
/// drains host-injected turns/edges (`K` lines), or [`NullInput`] when the feature is off. One
/// helper so the input plane builds it the same `cfg` way regardless.
#[cfg(not(feature = "glass-demo"))]
fn debug_input() -> impl InputSource {
    #[cfg(feature = "debug-uart")]
    return obc_platform::debug_usb::DebugInput;
    #[cfg(not(feature = "debug-uart"))]
    NullInput
}

/// A `no_std` [`Clock`](obc_render::Clock) over embassy's monotonic `Instant`, in microseconds — the
/// time base for the map render's per-stage timing (collect / sort / draw) the VCOM telemetry
/// carries. The same monotonic clock the loop's frame `Instant` reads, so the stages reconcile.
#[cfg(not(feature = "glass-demo"))]
struct InstantClock;
#[cfg(not(feature = "glass-demo"))]
impl obc_render::Clock for InstantClock {
    fn now_us(&self) -> u64 {
        Instant::now().as_micros()
    }
}

/// VCOM RX → sensor signals (#127, the nRF analog of the STM32 `usb_rx_task`): read bytes from the
/// interrupt-fed ring and feed each complete `F`/`A`/`C`/`K`/`Z` line into `obc-platform`'s
/// fresh-fix signals, which the app's `DebugLocation`/`DebugAltimeter`/`DebugCompass`/`DebugInput`
/// poll. A UART never "disconnects", so — unlike the CDC version — one `LineReader` lives for the
/// whole session.
#[cfg(feature = "debug-uart")]
#[embassy_executor::task]
async fn vcom_rx_task(mut rx: BufferedUarteRx<'static, peripherals::SERIAL20>) {
    let mut buf = [0u8; 64];
    let mut reader = obc_platform::debug_usb::LineReader::new();
    loop {
        match rx.read(&mut buf).await {
            Ok(n) => obc_platform::debug_usb::feed_bytes(&mut reader, &buf[..n]),
            Err(e) => defmt::warn!("VCOM RX error: {}", defmt::Debug2Format(&e)),
        }
    }
}

/// VCOM TX ← telemetry (#127, the nRF analog of the STM32 `usb_tx_task`): send one compact status
/// line each time the app publishes telemetry (~2 Hz via `set_telemetry`), so the host's readout
/// updates without the device polling or flooding the link. The buffered UARTE chunks the line to
/// DMA itself, so — unlike the CDC 64-byte-packet path — no manual packet splitting is needed (the
/// telemetry line ≤192 B fits the TX ring); just loop until the whole line is queued.
#[cfg(feature = "debug-uart")]
#[embassy_executor::task]
async fn vcom_tx_task(mut tx: BufferedUarteTx<'static, peripherals::SERIAL20>) {
    loop {
        let t = obc_platform::debug_usb::wait_telemetry().await;
        let line = obc_platform::debug_usb::format_telemetry(&t);
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
#[cfg(all(not(feature = "glass-demo"), not(feature = "panel-ls021")))]
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
#[cfg(all(feature = "panel-ls021", not(feature = "glass-demo")))]
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

    // LED0 (P2_09) heartbeat — a liveness blink visible even before looking at the panel.
    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);

    // --- glass-demo (#122): the font ladder + the device's 64-colour gamut, streamed to the
    // ST7789 band by band through the board-agnostic `Panel` seam, then a static heartbeat. The
    // shared-app path (load → ride → save-GPX) replaces this at N6 (#127). ---
    #[cfg(feature = "glass-demo")]
    {
        // Display control lines on the (flash-freed) P2 header. CS idles HIGH (deasserted) and the
        // driver pulses it low per transaction — embassy-nrf's Spim drives no hardware CS, so the
        // panel's CSX is framed in software (the rising edge re-aligns the panel each transaction,
        // which is what lets a warm MCU reset recover — see `st7789::St7789::transaction`). RST
        // idles high (released).
        let cs = Output::new(p.P2_05, Level::High, OutputDrive::Standard);
        let dc = Output::new(p.P2_03, Level::Low, OutputDrive::Standard);
        let rst = Output::new(p.P2_00, Level::High, OutputDrive::Standard);

        // SERIAL00 as a write-only SPIM: the panel never talks back, so MISO (P2_04) is omitted.
        // 8 MHz is comfortable over the jumpered bring-up bus; SERIAL00 reaches 32 MHz on a clean
        // board — worth revisiting once the panel is on a PCB.
        let mut config = spim::Config::default();
        config.frequency = spim::Frequency::M32;
        let spi = spim::Spim::new_txonly(p.SERIAL00, Irqs, p.P2_01, p.P2_02, config);

        // SAFETY: the sole reference taken to BAND; the panel holds it for the rest of the program
        // and this single-executor build never aliases it.
        let band = unsafe { &mut *core::ptr::addr_of_mut!(BAND) };
        let mut panel = St7789::new(spi, dc, rst, cs, Delay, band);
        panel.init();
        info!("obc-fw-nrf54l N1: ST7789 up ({}x{}) on SPIM00@8MHz; rendering glass-demo", WIDTH, HEIGHT);

        // Render the (static) screen once, band by band: each band gets the *whole* frame drawn
        // into it through `Band`, which clips the draw to the band's rows — so the frame reassembles
        // seam-free and the generator never has to know it's banded.
        panel.begin_frame();
        let rows = panel.band_rows();
        let mut y0 = 0u16;
        while y0 < HEIGHT {
            let h = rows.min(HEIGHT - y0);
            panel.flush_band(y0, h, |scratch| {
                let mut t = Band::new(scratch, Size::new(WIDTH as u32, HEIGHT as u32), y0, h);
                demo::font_palette_demo(&mut t).ok();
            });
            y0 += h;
        }
        panel.end_frame();
        info!("glass-demo rendered (font ladder + 64 swatches); heartbeat idle");

        loop {
            led.toggle();
            Timer::after_millis(500).await;
        }
    }

    // --- N6 load → ride → save-GPX, the STM32-parity target (#127): stream the SD `.obcm` into the
    // resident RGB222 framebuffer through the shared `obc-app`, pick a route from the card catalog,
    // ride it (VCOM-streamed GPS or the `SynthLocation` square loop), map-match + log the track, and
    // write a `.gpx` to `/tracks` on Finish. Builds on N4's map (#125) + N5's two-plane input (#126):
    // the map plane (this loop) now also runs the full sensor → tick → reconcile → telemetry ride
    // loop, the per-frame-reader structure the STM32 ride loop uses. ---
    #[cfg(not(feature = "glass-demo"))]
    {
        // --- VCOM debug-sensor stream (#127), behind `debug-uart`. Bring it up first so the J-Link
        // VCOM is live while the SD card + panel come up; the parsed fixes land in obc-platform's
        // signals, ready for the app's sensor poll in the loop below. The nRF54L15 has no USB
        // peripheral, so — unlike the STM32's USB-CDC port — the fake GPS/baro/compass feed and ride
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
                let r = core::ptr::addr_of_mut!(RX_BUF) as *mut [u8; 256];
                r.write([0; 256]);
                let t = core::ptr::addr_of_mut!(TX_BUF) as *mut [u8; 256];
                t.write([0; 256]);
                (&mut *r, &mut *t)
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
        // pin — so it can sit on any free GPIO. Default (ST7789): P1.12. FLPR build (`panel-ls021`):
        // P1.12 carries GEN, and the DK's P1.00–14 are one pin short, so CS moves to **P0.00** (M33
        // GPIO, the same port + drive as BTN3) — one jumper on the SD breakout. The SPIM bus pins
        // (SCK/MISO/MOSI on P1.11/07/06) are unchanged in both builds.
        #[cfg(not(feature = "panel-ls021"))]
        let sd_cs = Output::new(p.P1_12, Level::High, OutputDrive::Standard);
        #[cfg(feature = "panel-ls021")]
        let sd_cs = Output::new(p.P0_00, Level::High, OutputDrive::Standard);
        let Some(mut storage) = sd::init(sd_spi, sd_cs) else {
            defmt::error!("SD: no card / mount failed — cannot load a map; idling with a heartbeat");
            idle_blink(&mut led).await
        };

        // Open the `.obcm` and hold it open for the session — the map **streams** from it
        // (issue #37), never read resident into the 256 KB part. Then fill the Route menu from the
        // card's `/routes/*.obcr` catalog — both done here while `storage` is still freely mutable,
        // *before* the loop's per-frame readers take their short immutable borrows. The owned `Vec`
        // outlives those borrows and feeds `set_routes` once the app is built below.
        storage.open_map();
        let catalog = storage.scan_routes();

        // Place the streamed-map geometry cache in `.bss`, built in place (an all-zero
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary).
        // SAFETY: sole owner of MAP_CACHE; single executor → no aliasing.
        let map_cache: &MapCache = unsafe {
            let slot = core::ptr::addr_of_mut!(MAP_CACHE) as *mut MapCache;
            slot.write(MapCache::new());
            &*slot
        };

        // Read just the OBCM **header** for the idle camera centre — its bbox, plus the magic/version
        // validation. Deliberately NOT a full `Reader::new`: that also reads + parses the style table
        // into a `[Option<Style>; 256]` (a ~2.4 KB `Reader` value plus a same-size on-stack temporary
        // and a 1.5 KB decode buffer), a frame the 256 KB part's modest stack shouldn't pay just for a
        // camera centre. The loop's per-frame readers do the full parse where it's needed (and skip a
        // redraw on a transient build failure), so a structurally-bad map degrades to "no map", never
        // a fault. The throwaway source is dropped here, releasing its `storage` borrow so the loop can
        // rebuild a fresh reader each redraw AND reconcile the card (`&mut storage`) between frames.
        let (cam_lon, cam_lat) = {
            let Some(init_src) = storage.map_source() else {
                defmt::error!("SD: no .obcm map in card root — idling with a heartbeat");
                idle_blink(&mut led).await
            };
            match obc_reader::read_header(&init_src) {
                Ok(h) => {
                    info!(
                        "map: streaming from SD; bbox lon[{=i32}..{=i32}] lat[{=i32}..{=i32}]",
                        h.bbox.min_lon, h.bbox.max_lon, h.bbox.min_lat, h.bbox.max_lat
                    );
                    (
                        ((h.bbox.min_lon as i64 + h.bbox.max_lon as i64) / 2) as i32,
                        ((h.bbox.min_lat as i64 + h.bbox.max_lat as i64) / 2) as i32,
                    )
                }
                Err(e) => {
                    defmt::error!("map: not valid OBCM: {} — idling with a heartbeat", defmt::Debug2Format(&e));
                    idle_blink(&mut led).await
                }
            }
        };

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
        app.set_routes(&catalog);

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
        #[cfg(not(feature = "panel-ls021"))]
        let bus = {
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
            info!("obc-fw-nrf54l N5: ST7789 up ({}x{}); two-plane input + map", WIDTH, HEIGHT);

            static mut BUS: MaybeUninit<Mutex<CriticalSectionRawMutex, Display>> = MaybeUninit::uninit();
            // SAFETY: sole writer; initialised here before the `&'static` is shared with the input
            // plane, never written again (single executor builds it, two planes only read it).
            let bus: &'static Mutex<CriticalSectionRawMutex, Display> = unsafe {
                let slot = core::ptr::addr_of_mut!(BUS) as *mut Mutex<CriticalSectionRawMutex, Display>;
                slot.write(Mutex::new(Display { panel, fb }));
                &*slot
            };
            static mut INPUT_PLANE: MaybeUninit<BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>> =
                MaybeUninit::uninit();
            // SAFETY: as BUS — sole writer, initialised before shared, never rewritten.
            let input_plane: &'static BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>> = unsafe {
                let slot = core::ptr::addr_of_mut!(INPUT_PLANE)
                    as *mut BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>;
                slot.write(BlockingMutex::new(RefCell::new(InputPlane::new())));
                &*slot
            };

            let input_spawner = EXECUTOR_INPUT.start(interrupt::SWI00);
            input_spawner.spawn(defmt::unwrap!(input_overlay_task(buttons, input_plane, bus, GESTURES.sender())));
            info!("input plane: SWI00 interrupt executor @ P3 (preempts the map render); map plane: thread mode");
            bus
        };

        // ============= FLPR LS021 backend (`--features panel-ls021`, issue #165) =====================
        // The map plane owns the `Ls021Flpr` panel directly (it scans a whole frame per push, so there
        // is no partial-window overlay to serialise — no bus mutex). The M33 configures every line the
        // FLPR drives (held as outputs for the program's life); `com_task` + the gesture `input_task`
        // share the one high-priority executor (COM must keep alternating during the blocking push).
        //
        // ⚠️ **Gate/BSP pins relocated off the SD/VCOM bus for the integration.** These five P1 lines
        // **must match `src/flpr/flpr_pingpong.c`'s masks and the bring-up bin's pins** — confirm each
        // is broken out on your DK and remap all three together if not (the source bus, BCK, and COM
        // stay on P2 exactly as the bring-up bin has them).
        #[cfg(feature = "panel-ls021")]
        let (mut panel, input_plane, _flpr_gate_bus, _flpr_src_bus) = {
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

            // The resident RGB222 plane the app renders into and the FLPR packs to the wire.
            // SAFETY: sole reference to FB; held by `panel` for the rest of the program, never aliased.
            let fb: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
            let mut panel = Ls021Flpr::new_fb(fb);
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
                let slot = core::ptr::addr_of_mut!(INPUT_PLANE)
                    as *mut BlockingMutex<CriticalSectionRawMutex, RefCell<InputPlane>>;
                slot.write(BlockingMutex::new(RefCell::new(InputPlane::new())));
                &*slot
            };

            let hp = EXECUTOR_HP.start(interrupt::SWI00);
            hp.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
            hp.spawn(defmt::unwrap!(input_task(buttons, input_plane, GESTURES.sender())));
            info!("FLPR LS021: COM + gesture/bulge plane on SWI00 @ P3; map plane: thread mode (blocking masked push)");
            (panel, input_plane, gate_bus, src_bus)
        };

        // Native renderer colour → identity `Rgb565`; `FbDevice64` quantizes to RGB222 on store
        // (the device-64 gamut the style table is tuned to — see `obc_platform::framebuffer`).
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

        // Place the decoded-route-geometry cache in `.bss`, built in place (a zeroed
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary — like `MAP_CACHE`).
        // SAFETY: sole owner of ROUTE_CACHE; single map plane → no aliasing.
        let route_cache: &RouteCache = unsafe {
            let slot = core::ptr::addr_of_mut!(ROUTE_CACHE) as *mut RouteCache;
            slot.write(RouteCache::new());
            &*slot
        };

        // Sensor sources. Default (`debug-uart`): the host-streamed GPS / altimeter / compass, parsed
        // by the VCOM tasks into obc-platform's signals — these ZST handles just `try_take` from them
        // on the app's ~1 Hz fresh-fix contract. Fallback: the `SynthLocation` square loop (walked
        // from a boot-relative `start`), no altimeter/compass. Same `Sensors` either way, so the app
        // can't tell. The loop's `now` reads `Instant::now()` directly — the same monotonic base.
        #[cfg(feature = "debug-uart")]
        let (mut debug_loc, mut debug_alt, mut debug_compass) = (
            obc_platform::debug_usb::DebugLocation,
            obc_platform::debug_usb::DebugAltimeter,
            obc_platform::debug_usb::DebugCompass,
        );
        #[cfg(not(feature = "debug-uart"))]
        let mut synth = SynthLocation::new(cam_lon, cam_lat, Instant::now());

        // --- map plane (issue #126/#47) + the N6 ride loop (#127): each tick, drain the gestures the
        // input plane recognised, advance the visible screens' timed content, reconcile the card to
        // the app's intent (open the selected route's geometry; begin / finalise-to-GPX the ride
        // log), feed the sensors → `tick` (integrate the fix, map-match, log the track point), then
        // re-render the map only on `dirty.map`. A static screen does zero map renders, so the input
        // plane's bulge pushes are the only bus traffic — the bulge runs at full FPS. LED0 keeps a
        // ~1 Hz heartbeat. ---
        //
        // Per-frame ride-loop state (mirrors the STM32 loop):
        // - `prev_route` re-centres SynthLocation onto a freshly-loaded route's start (debug-uart-off
        //   only — the VCOM feed streams absolute positions, so it needs no re-centre);
        // - `prev_active`/`prev_session` gate the SD reconcile on actual change (#73);
        // - `route_index`/`index_route` cache the active route's chunk index, rebuilt only on a route
        //   change (#44);
        // - `pending_map_redraw` re-arms a redraw a transient SD glitch couldn't service (#66);
        // - `last_telem*` throttle the host telemetry (debug-uart only).
        #[cfg(not(feature = "debug-uart"))]
        let mut prev_route: Option<usize> = None;
        let mut prev_active: Option<usize> = None;
        let mut prev_session: Option<u32> = None;
        let mut route_index: Option<RouteIndex> = None;
        let mut index_route: Option<usize> = None;
        let mut pending_map_redraw = false;
        // The last live bulge's row span (FLPR overlay, #163): when the bulge goes quiet the trailing
        // clear wipes exactly *these* rows — the same small push as the pop frames — rather than the
        // whole hint band, so the animation's tail stays smooth (no full-band frame at the end).
        #[cfg(feature = "panel-ls021")]
        let mut last_overlay_span: Option<(u16, u16)> = None;
        #[cfg(feature = "debug-uart")]
        let mut last_telem_ms: u32 = 0;
        #[cfg(feature = "debug-uart")]
        let mut last_telem = obc_platform::debug_usb::Telemetry::default();
        let mut last_led = 0u32;
        loop {
            let now = Instant::now().as_millis() as u32;

            // Apply the high-priority plane's recognised gestures, in order, then advance animations.
            // The screen transition lands a frame after the overlay already confirmed the press.
            while let Ok(g) = GESTURES.try_receive() {
                app.apply_gesture(g);
            }
            app.advance_animations(InputClock(now));

            // A pending debug `Z` camera-scale command (render benchmark): pin the map to an exact
            // meters-per-pixel and force one redraw, so a host zoom sweep gets exactly one fresh,
            // stage-timed frame per setting instead of stepping the encoder's 1.2× detents.
            #[cfg(feature = "debug-uart")]
            if let Some(mpp) = obc_platform::debug_usb::take_zoom() {
                app.set_map_mpp(mpp);
            }

            let active = app.activity.active_route;
            // Re-centre the synthetic GPS onto a freshly-loaded route's start so Follow doesn't yank
            // the camera off it (debug-uart-off only — the VCOM feed streams absolute positions).
            #[cfg(not(feature = "debug-uart"))]
            if active != prev_route {
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    synth.recenter(r.start_lon, r.start_lat);
                }
                prev_route = active;
            }

            // Reconcile the card to the app's intent: open/close the active route's geometry and the
            // ride log (begin on load, finalise-to-GPX on Finish), reading the save name from the
            // active route. Gated on the same edges `reconcile_*` test internally (a route swap, a
            // session change, or a pending track action) so the dominant static frame does no per-tick
            // `String<64>` copy or state re-walk (#73). `has_track_action` is a non-consuming peek; the
            // actual `take_track_action` stays inside, so the one-shot is drained only when processed.
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

            // Cache the active route's chunk index across frames: rebuild it (the header + full
            // chunk-meta walk off SD) only when the route changes, or retry if a prior build failed on
            // a flaky link. Not gated on rendering — the matcher in `tick` needs the index on every
            // fresh fix, independent of whether the map redraws.
            if index_route != active {
                route_cache.clear(); // a route switch: drop stale slots (the cache keys by chunk index only)
                match active {
                    Some(_) => match storage.build_route_index() {
                        Some(idx) => {
                            route_index = Some(idx);
                            index_route = active; // cached — no more rebuilds until the route changes
                        }
                        None => {
                            // Transient SD glitch: leave the key mismatched so every frame retries,
                            // hiding the route this frame rather than the whole ride.
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
            // This frame's route reader = the cached index + a fresh geometry source (both cheap, no
            // I/O — the source just wraps the open handle). Geometry streams lazily where it's read:
            // the matcher on a fresh fix, the renderer on a redraw frame. Paired with the session-long
            // `route_cache` so a redraw of the unchanged route (and the per-fix decode) hit RAM.
            let route_src = storage.route_source();
            let route = match (route_index.as_ref(), route_src.as_ref()) {
                (Some(idx), Some(src)) => Some(RouteReader::new_cached(idx, src, route_cache)),
                _ => None,
            };
            // The ride-log sink, built every tick (it only wraps the open log handle, no I/O), so a
            // fresh fix is written to the `.gpx` the moment it arrives, at the fix rate, independent of
            // the render cadence.
            let mut tsink = storage.track_sink();
            let track_dyn = tsink.as_mut().map(|t| t as &mut dyn TrackSink);

            // Feed the sensors → integrate the fix → map-match to the route → log the track point.
            // Default: the VCOM-streamed GPS + altimeter + compass. Fallback: the SynthLocation square
            // loop, no altimeter/compass. `track_dyn` is consumed either way.
            #[cfg(feature = "debug-uart")]
            app.tick(
                RideClock(now),
                Sensors {
                    loc: &mut debug_loc,
                    altimeter: Some(&mut debug_alt),
                    compass: Some(&mut debug_compass),
                    track: track_dyn,
                },
                route.as_ref(),
            );
            #[cfg(not(feature = "debug-uart"))]
            app.tick(
                RideClock(now),
                Sensors { loc: &mut synth, altimeter: None, compass: None, track: track_dyn },
                route.as_ref(),
            );

            // Drain the per-frame dirty signal now that input + tick have run, and fold back a redraw a
            // previous frame couldn't service on a transient reader-build failure (#66).
            let mut dirty = app.take_dirty();
            dirty.map |= pending_map_redraw;
            pending_map_redraw = false;

            // FLPR: the hold bulge rides the map plane (there is no shared SPI bus, so no concurrent
            // overlay push to serialise against). Sample the overlay state once per frame from the
            // shared `InputPlane` — the map plane is the single owner of that bookkeeping here: the
            // dirty edge (`take_overlay_dirty` is true while the bulge is live, plus one trailing clear)
            // and the live bulge's **row span** (`None` when quiet), so the push below re-presents only
            // the active bulge's rows (issue #163), not the whole hint band. The ST7789 build drives its
            // bulge from the input plane instead, so it has no overlay work in this loop.
            #[cfg(feature = "panel-ls021")]
            let (overlay_dirty, overlay_span) = input_plane.lock(|c| {
                let p = &mut *c.borrow_mut();
                (p.take_overlay_dirty(), p.overlay_rows(WIDTH as i32, HEIGHT as i32))
            });

            if dirty.map {
                // The map pipeline runs **only when the base screen actually draws the map** (the Map
                // view). On a menu / Statistics / Home redraw it's skipped entirely — no SD style-table
                // parse, no `Reader` build (so no stack spike for it), no map render — and that screen
                // draws just its own chrome (it ignores the reader). So nothing map-related runs when no
                // map is on screen; a non-map frame costs only its own draw + the push.
                let needs_map = app.base_draws_map();
                // Build the streamed `Reader` (against the session-long `MapCache` for cross-frame chunk
                // reuse) **only** on a map frame, `None` otherwise. A transient SD read failure on a map
                // frame fails the build → skip the redraw, keep the last frame, latch a retry (#66)
                // rather than show a half-read map.
                let map_src = if needs_map { storage.map_source() } else { None };
                let reader = map_src.as_ref().and_then(|s| Reader::new(s, map_cache).ok());
                if needs_map && reader.is_none() {
                    pending_map_redraw = true;
                    defmt::warn!(
                        "map: reader build failed this frame (flaky SD?) — kept frame, retrying redraw next frame"
                    );
                } else {
                    // Acquire the display through the seam — the **only** per-backend line in the map
                    // present. ST7789 holds the shared bus mutex for the whole frame, so the input/
                    // overlay plane never reads a torn framebuffer (the map render writes it under this
                    // lock); the FLPR map plane owns its panel outright (its overlay rides this same
                    // plane, so there is no concurrent push to serialise). Everything below is one path.
                    #[cfg(not(feature = "panel-ls021"))]
                    let mut guard = bus.lock().await;
                    #[cfg(not(feature = "panel-ls021"))]
                    let display: &mut dyn DisplayDriver = &mut *guard;
                    #[cfg(feature = "panel-ls021")]
                    let display: &mut dyn DisplayDriver = &mut panel;

                    // Render the whole frame into the resident RGB222 plane, then present it through the
                    // seam. `render_map_timed` threads `InstantClock` so the returned `RenderStats`
                    // carries the per-stage collect/sort/draw timings the VCOM telemetry reports;
                    // `route` streams its geometry into the render (the route line + the user/breadcrumb
                    // overlay). The hold bulge is **not** composited here — it rides `present_overlay`
                    // on its own plane (issue #163), so the framebuffer stays the clean map.
                    let t_render = Instant::now();
                    let stats = {
                        let mut fbdev = FbDevice64::new(display.fb_mut(), WIDTH as u32, HEIGHT as u32);
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
                    let render_us = t_render.elapsed().as_micros();

                    // Snapshot this frame's render stats for the host telemetry line — the same numbers
                    // as the RTT `map frame` log. The nRF reader isn't `TimedSource`-wrapped, so the
                    // SD/cache I/O folds into `collect_us` (`read_us` stays 0); the bulge composites on
                    // its own overlay push, not within the render, so `overlay_us` stays 0.
                    #[cfg(feature = "debug-uart")]
                    {
                        let mpp_milli =
                            (app.state.viewport(WIDTH as f32, HEIGHT as f32).meters_per_pixel() * 1000.0) as u32;
                        last_telem = obc_platform::debug_usb::Telemetry {
                            frame_us: render_us as u32,
                            lod: stats.lod as u8,
                            feat_drawn: stats.features_drawn as u32,
                            feat_tried: stats.features_tried as u32,
                            feat_dropped: stats.features_dropped as u32,
                            chunks: stats.chunks_visited as u32,
                            cache_hits: stats.map_chunk_hits,
                            cache_misses: stats.map_chunk_misses,
                            sd_reads: stats.map_sd_reads,
                            bytes_read: stats.map_bytes_read,
                            collect_us: stats.collect_us,
                            read_us: 0,
                            sort_us: stats.sort_us,
                            draw_us: stats.draw_us,
                            overlay_us: 0,
                            mpp_milli,
                        };
                    }

                    // Present the whole clean frame to glass (blocking; each backend logs its own push
                    // breakdown — ST7789 fill/pack/spi, the FLPR frame/pack). A transport fault
                    // (`present` → false, e.g. a stalled FLPR) latches a retry like the reader-build
                    // failure (#66) rather than faulting.
                    let t_push = Instant::now();
                    let ok = display.present();
                    let push_us = t_push.elapsed().as_micros();
                    // Release the ST7789 bus before logging so the input plane can push its bulge.
                    #[cfg(not(feature = "panel-ls021"))]
                    drop(guard);
                    if !ok {
                        pending_map_redraw = true;
                    }
                    // A map frame carries the map render stats; a non-map (menu / Statistics / Home)
                    // frame is just a screen redraw + push, so log it as such — no meaningless
                    // lod/feat/chunks, and it reads clearly that the map pipeline did not run.
                    if needs_map {
                        defmt::info!(
                            "map frame: render {=u64} us + push {=u64} us | lod {=usize} | feat {=usize}/{=usize} | chunks {=usize} | map-cache {=u32} hit / {=u32} miss",
                            render_us,
                            push_us,
                            stats.lod,
                            stats.features_drawn,
                            stats.features_tried,
                            stats.chunks_visited,
                            stats.map_chunk_hits,
                            stats.map_chunk_misses
                        );
                    } else {
                        defmt::info!(
                            "ui frame: render {=u64} us + push {=u64} us (screen redraw, no map)",
                            render_us,
                            push_us
                        );
                    }
                }
            }

            // FLPR hold bulge (issue #163). The bulge lives on the map plane, so it is **re-applied on
            // top after a map present** (a redraw mid-hold — e.g. the screen transition a completed hold
            // fires — would otherwise blink the pop animation off) and on overlay-only frames; either
            // way it is a separate push over the clean map, never baked into it. While live, present
            // **only the active bulge's rows** (encoder ≈ 59–171 OR Back ≈ 182–246) — the FLPR
            // fast-forwards the gate to the region + early-stops, a fraction of a full frame. The
            // trailing edge (bulge just went quiet) wipes **the same rows** the last bulge used (kept in
            // `last_overlay_span`), not the whole band, so the tail stays as smooth as the pop — unless
            // a map present this frame already restored the clean map there.
            #[cfg(feature = "panel-ls021")]
            {
                let composite = |panel: &mut Ls021Flpr, region: OverlayRegion| -> bool {
                    panel.present_overlay(region, &mut |band: &mut Band| {
                        input_plane
                            .lock(|cell| cell.borrow().render_overlay(band, WIDTH as f32, HEIGHT as f32, color_fn));
                    })
                };
                if let Some((y0, rows)) = overlay_span {
                    let t_push = Instant::now();
                    let ok = composite(&mut panel, OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows });
                    let push_us = t_push.elapsed().as_micros();
                    last_overlay_span = Some((y0, rows));
                    if ok {
                        defmt::info!("overlay frame: bulge push {=u64} us ({=u16} rows @ y{=u16})", push_us, rows, y0);
                    } else {
                        defmt::warn!("overlay frame: bulge push failed (FLPR stalled?) — retrying next overlay tick");
                    }
                } else if overlay_dirty && !dirty.map {
                    // Trailing clear on a static frame: re-present just the last bulge's rows with nothing
                    // composited = the clean map restored under the just-gone bulge (a map present this
                    // frame would already have done it, hence the `!dirty.map`).
                    if let Some((y0, rows)) = last_overlay_span.take() {
                        composite(&mut panel, OverlayRegion { x0: OVL_X0, y0, w: OVL_W, rows });
                    }
                }
            }

            // Publish render-stats telemetry host-ward at ~2 Hz (#127): throttled here (not in the TX
            // task) so the link never floods and the device never stalls on it.
            #[cfg(feature = "debug-uart")]
            if now.wrapping_sub(last_telem_ms) >= 500 {
                last_telem_ms = now;
                obc_platform::debug_usb::set_telemetry(last_telem);
            }

            if now.wrapping_sub(last_led) >= 500 {
                led.toggle();
                last_led = now;
            }
            Timer::after_millis(LOOP_MS).await;
        }
    }
}
