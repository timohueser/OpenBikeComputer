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
//!   N5. buttons + two-plane InterruptExecutor + fluid composite-on-push bulge           (#126)
//!   N6. debug/sensor stream over VCOM UART + load→ride→save-GPX = PARITY                (#127)
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
//!   BTN0 P1_13 | BTN1 P1_09 | BTN2 P1_08 | BTN3 P0_04
//! Map to obc-platform's board-agnostic `ButtonInput` debouncer → the shared gesture
//! recogniser, exactly like the STM32's four GPIO buttons. (PREV/NEXT/SELECT/BACK roles
//! assigned in N5.) Read via the **GPIOTE** peripheral (the `gpiote` feature). These are the
//! DK's own buttons — no jumpers — and they stay free because the display lives on P2 (below).
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
//!     SCK P1_11 | MISO P1_07 | MOSI P1_06 | CS P1_12
//!   CS is a free GPIO held LOW for the whole session (the held-low-CS workaround embedded-sdmmc's
//!   per-byte framing needs over embassy SPI — see `sd::NoCs`); the bus inits ≤400 kHz then
//!   re-clocks to 8 MHz (`sd::init`). embassy-nrf's `Spim` exposes **no** internal MISO pull-up
//!   (unlike embassy-stm32), so the card's DO line is pulled high by the breakout (or an external
//!   10 kΩ to 3V3). The EYESPI connector also carries a microSD that *shares the display bus*; we
//!   leave that slot **unpopulated** and use this dedicated SPIM instead (a clean reuse of the
//!   STM32's standalone SD-over-SPI reader + obc-platform's FatFs adapters). P1_06/P1_07 alias
//!   VCOM's unused RTS/CTS below — no conflict, since the VCOM link is 2-wire (TX/RX only).
//!
//! ## VCOM UARTE — debug-sensor / telemetry stream (#127)
//!   Instance **SERIAL20 / UARTE20**, the DK's `chosen` console wired to the onboard J-Link's
//!   USB-CDC VCOM: TX P1_04 | RX P1_05  (RTS P1_06 / CTS P1_07 available, unused).
//!   The nRF54L15 has **no USB peripheral**, so — unlike the STM32's second USB-CDC port — the
//!   fake GPS/baro/compass feed and ride telemetry ride this UART; defmt logs ride RTT on the
//!   same cable. obc-platform's debug-source protocol is transport-agnostic, so it moves over
//!   from USB unchanged.
//!
//! ## Spare interrupt for the high-priority InterruptExecutor (#126)
//!   The two-plane architecture runs input + the overlay on a high-priority `InterruptExecutor`
//!   that preempts the map render. On STM32 that executor was pended from the unused UART5
//!   vector; the nRF analog is a dedicated **software-interrupt vector**: reserve **SWI00** for
//!   it (the M33 also has SWI01/02/03 + EGU10/EGU20 free). It pends above thread mode but below
//!   the P0 GRTC time-driver, so `Timer`s still wake mid-render.
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
mod st7789;

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::{Delay, Timer};
use st7789::{St7789, HEIGHT, WIDTH};
use {defmt_rtt as _, panic_probe as _};

// The display panel + the banded `Panel` push seam are common to both builds (glass-demo + map).
use obc_platform::Panel;

// glass-demo (#122) only: the per-band whole-frame draw helper + its frame size.
#[cfg(feature = "glass-demo")]
use embedded_graphics::prelude::Size;
#[cfg(feature = "glass-demo")]
use obc_platform::Band;

// N4 map path (#125): the shared app, the streamed-map reader + its `.bss` cache, and the
// device-native RGB222 framebuffer the renderer draws into (`color_fn` = identity Rgb565; the
// framebuffer quantizes to RGB222 on store, the band push expands it back for the ST7789).
#[cfg(not(feature = "glass-demo"))]
use core::mem::MaybeUninit;
#[cfg(not(feature = "glass-demo"))]
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
#[cfg(not(feature = "glass-demo"))]
use obc_app::{App, AppState};
#[cfg(not(feature = "glass-demo"))]
use obc_platform::{device64_to_rgb565, FbDevice64};
#[cfg(not(feature = "glass-demo"))]
use obc_reader::{MapCache, Reader};
#[cfg(not(feature = "glass-demo"))]
use obc_render::zoom_for_mpp;

// SERIAL00 backs the display SPIM (#122); SERIAL22 the microSD SPIM (#123). Both handlers are
// always registered (harmless when a feature is off — the peripheral is simply never constructed,
// so its interrupt never fires); each `Spim::new` is handed `Irqs` for its instance.
bind_interrupts!(struct Irqs {
    SERIAL00 => spim::InterruptHandler<peripherals::SERIAL00>;
    SERIAL22 => spim::InterruptHandler<peripherals::SERIAL22>;
});

/// One band's worth of RGB565 scratch (`WIDTH * BAND_ROWS`), living in `.bss`. 20 rows ≈ 9.6 KB
/// and tiles the 320-row frame in 16 bands. The `Panel` impl fills it — the map path expands the
/// RGB222 frame into it, the glass-demo draws the frame per band — then byte-swaps to big-endian
/// and SPIM-DMAs it; borrowed exactly once below (single executor → no aliasing). `BAND_BYTES` is
/// what the N3 budget assert reserves.
const BAND_ROWS: usize = 20;
/// The band scratch in bytes (RGB565, 2 B/px) — the resident cost the budget assert reserves.
const BAND_BYTES: usize = st7789::WIDTH as usize * BAND_ROWS * 2;
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
//   - `App`        embeds the renderer scratch (`obc_render::MCU_RENDERER_BYTES`, ~74 KB nrf-mem)
//                  plus the resident elevation `Profile` (~8.5 KB at PROFILE_COLS=1024) and
//                  `Breadcrumb` (~6 KB at SPINE_CAP=512); ~96 KB total.
//   - framebuffer  the single RGB222 frame (#N4): 240×320 × 1 B/px = 75 KB — the `FB` static below
//                  (the map path) renders into it; the glass-demo build reserves the budget without
//                  allocating it, since it draws per band.
//   - `MapCache`   the streamed-map geometry-chunk cache (5 slots on nrf-mem, ~41 KB).
//   - `RouteCache` the decoded-route-chunk cache (4 slots on nrf-mem, ~12 KB).
//   - band scratch one RGB565 `Panel` band (`BAND_BYTES`, ~9.4 KB).
// plus `STACK_RESERVE` headroom for the main stack + embassy's executor/task arenas.

/// Total SRAM the nRF54L15 app core sees (`memory.x`: RAM 256K @ 0x2000_0000).
const NRF_RAM_BYTES: usize = 256 * 1024;
/// Headroom kept free under the resident statics for the main stack + embassy's executor/task
/// arenas (statics grow up from the RAM base, the stack down from the top). The resident set lands
/// ~234 KB, so this 16 KB reserve leaves ~22 KB of true stack room; sized for the render call depth
/// and revisited once N6 measures the real high-water mark.
const STACK_RESERVE: usize = 16 * 1024;
/// The single RGB222 framebuffer (#N4): one byte per pixel over the 240×320 panel = 75 KB.
const FB_BYTES: usize = st7789::WIDTH as usize * st7789::HEIGHT as usize;

/// The resident set that must coexist during a redraw (see the table above).
const RESIDENT_BYTES: usize = core::mem::size_of::<obc_app::App>()
    + FB_BYTES
    + core::mem::size_of::<obc_reader::MapCache>()
    + core::mem::size_of::<obc_route::RouteCache>()
    + BAND_BYTES;
const _: () = assert!(
    RESIDENT_BYTES + STACK_RESERVE <= NRF_RAM_BYTES,
    "nRF resident set (App + framebuffer + MapCache + RouteCache + band) + stack reserve overruns 256 KB — trim the `nrf-mem` caps (issue #124)"
);

// ============================ N4 resident set + map path (issue #125) ============================

/// The resident device-native RGB222 framebuffer: one byte per pixel over the 240×320 panel
/// (`FB_BYTES` = 75 KB), in `.bss`. [`App::render_map`](obc_app::App::render_map) quantizes into it
/// on store ([`FbDevice64`]); the band push expands it back to RGB565 for the ST7789. Borrowed once
/// below (single executor → no aliasing). Map-path only — the glass-demo draws per band, no full
/// frame.
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

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_nrf::init(Default::default());

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
        config.frequency = spim::Frequency::M8;
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

    // --- N4 first full map on glass (#125): stream the SD `.obcm`, render it into the resident
    // RGB222 framebuffer through the shared `obc-app`, and push it to the ST7789 band by band
    // (expanding RGB222 → RGB565). Single-buffered, single-executor, no buttons/sensors yet
    // (#126/#127) — the first time the real renderer paints a real map on the real target. ---
    #[cfg(not(feature = "glass-demo"))]
    {
        // microSD on its own SPIM (SERIAL22, P1 header — separate from the display bus on
        // SERIAL00/P2). Init ≤400 kHz (SD spec); `sd::init` re-clocks to 8 MHz once the card
        // answers. CS idles HIGH, then `init` holds it LOW for the session (the per-byte-CS
        // workaround — see `sd::NoCs`). `orc = 0xFF` so any over-read clocks the SD idle byte.
        let mut sd_cfg = spim::Config::default();
        sd_cfg.frequency = sd::SD_INIT_HZ;
        sd_cfg.orc = 0xFF;
        let sd_spi = spim::Spim::new(p.SERIAL22, Irqs, p.P1_11, p.P1_07, p.P1_06, sd_cfg);
        let sd_cs = Output::new(p.P1_12, Level::High, OutputDrive::Standard);
        let Some(mut storage) = sd::init(sd_spi, sd_cs) else {
            defmt::error!("SD: no card / mount failed — cannot load a map; idling with a heartbeat");
            idle_blink(&mut led).await
        };

        // Open the `.obcm` and hold it open for the session — the map **streams** from it
        // (issue #37), never read resident into the 256 KB part. `map_source` hands out a fresh
        // byte source over the open handle (here it backs the streamed `Reader` below).
        storage.open_map();
        let Some(map_src) = storage.map_source() else {
            defmt::error!("SD: no .obcm map in card root — idling with a heartbeat");
            idle_blink(&mut led).await
        };

        // Place the streamed-map geometry cache in `.bss`, built in place (an all-zero
        // `MaybeUninit::zeroed` → a `.bss` memset, never a stack temporary).
        // SAFETY: sole owner of MAP_CACHE; single executor → no aliasing.
        let map_cache: &MapCache = unsafe {
            let slot = core::ptr::addr_of_mut!(MAP_CACHE) as *mut MapCache;
            slot.write(MapCache::new());
            &*slot
        };

        // Build the streamed `Reader` once: it validates the OBCM and yields the bbox for the idle
        // camera centre, and backs the static render below (single frame → no per-redraw rebuild).
        let reader = match Reader::new(&map_src, map_cache) {
            Ok(r) => r,
            Err(e) => {
                defmt::error!("map: not valid OBCM: {} — idling with a heartbeat", defmt::Debug2Format(&e));
                idle_blink(&mut led).await
            }
        };
        let cam_lon = ((reader.bbox.min_lon as i64 + reader.bbox.max_lon as i64) / 2) as i32;
        let cam_lat = ((reader.bbox.min_lat as i64 + reader.bbox.max_lat as i64) / 2) as i32;
        info!(
            "map: streaming from SD; bbox lon[{=i32}..{=i32}] lat[{=i32}..{=i32}] → camera ({=i32},{=i32})",
            reader.bbox.min_lon, reader.bbox.max_lon, reader.bbox.min_lat, reader.bbox.max_lat, cam_lon, cam_lat
        );

        // Build the `App` straight onto the live Map at the bbox centre — map-first, since there
        // are no buttons to navigate yet. Constructed **in place** in `.bss` (`init_map` writes each
        // field where it sits; the ~74 KB renderer scratch is zeroed in place), never on the stack.
        // SAFETY: sole owner of APP; `init_map` fully initialises it before the `&mut` below reads it.
        let app: &mut App = unsafe {
            let slot = core::ptr::addr_of_mut!(APP) as *mut App;
            App::init_map(slot, AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP)));
            &mut *slot
        };

        // Display panel on the (flash-freed) P2 header — same wiring as the glass-demo. CS idles
        // HIGH and the driver pulses it low per transaction (the warm-reset-safe CSX framing — see
        // `st7789::St7789::transaction`); RST idles high. SERIAL00 write-only SPIM @8 MHz.
        let cs = Output::new(p.P2_05, Level::High, OutputDrive::Standard);
        let dc = Output::new(p.P2_03, Level::Low, OutputDrive::Standard);
        let rst = Output::new(p.P2_00, Level::High, OutputDrive::Standard);
        let mut config = spim::Config::default();
        config.frequency = spim::Frequency::M8;
        let spi = spim::Spim::new_txonly(p.SERIAL00, Irqs, p.P2_01, p.P2_02, config);
        // SAFETY: sole reference to BAND; the panel holds it for the rest of the program (single
        // executor → no aliasing).
        let band = unsafe { &mut *core::ptr::addr_of_mut!(BAND) };
        let mut panel = St7789::new(spi, dc, rst, cs, Delay, band);
        panel.init();
        info!("obc-fw-nrf54l N4: ST7789 up ({}x{}); rendering the SD map", WIDTH, HEIGHT);

        // The resident RGB222 framebuffer the renderer draws into.
        // SAFETY: sole reference to FB; single executor → no aliasing.
        let fb_buf = unsafe { &mut *core::ptr::addr_of_mut!(FB) };

        // Native renderer colour → identity `Rgb565`; `FbDevice64` quantizes to RGB222 on store
        // (the device-64 gamut the style table is tuned to — see `obc_platform::framebuffer`).
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

        // Render-on-demand (issue #47): `App` boots map-dirty, so the first iteration renders the
        // map once; with no input/sensors nothing then re-dirties it, so it stays clean — zero
        // further redraws, exactly the STM32's dirty-driven idle. LED0 still ticks each loop.
        loop {
            if app.take_dirty().map {
                let mut fb = FbDevice64::new(&mut fb_buf[..], WIDTH as u32, HEIGHT as u32);
                let stats = app.render_map(&mut fb, &reader, None, WIDTH as f32, HEIGHT as f32, color_fn);

                // Put the rendered frame on glass band by band: read the band's RGB222 rows out of
                // the framebuffer, expand each byte to RGB565 into the band scratch, and let the
                // `Panel` byte-swap + SPIM-DMA it (the seam the FLPR/LS021B7DD02 backend reuses).
                let pixels = fb.as_pixels();
                panel.begin_frame();
                let rows = panel.band_rows();
                let mut y0 = 0u16;
                while y0 < HEIGHT {
                    let h = rows.min(HEIGHT - y0);
                    let row0 = y0 as usize * WIDTH as usize;
                    panel.flush_band(y0, h, |scratch| {
                        for (dst, &byte) in scratch.iter_mut().zip(&pixels[row0..]) {
                            *dst = device64_to_rgb565(byte);
                        }
                    });
                    y0 += h;
                }
                panel.end_frame();

                defmt::info!(
                    "map frame on glass: lod {=usize} | feat {=usize}/{=usize} | chunks {=usize} | map-cache {=u32} hit / {=u32} miss",
                    stats.lod,
                    stats.features_drawn,
                    stats.features_tried,
                    stats.chunks_visited,
                    stats.map_chunk_hits,
                    stats.map_chunk_misses
                );
            }
            led.toggle();
            Timer::after_millis(500).await;
        }
    }
}
