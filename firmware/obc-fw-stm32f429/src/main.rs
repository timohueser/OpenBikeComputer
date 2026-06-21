//! STM32F429I-DISC1 board firmware for the OpenBikeComputer hardware prototype.
//!
//! This is the on-glass stand-in while the real target (nRF54L + MIP panel) is in
//! development: it brings the shared `obc-app` up on a real display + buttons + SD so
//! the HAL seams become concrete and the nRF firmware is a port, not a rewrite. The
//! LTDC + ILI9341 path here is specific to this Discovery board; nothing app-facing
//! depends on it.
//!
//! Bring-up is phased so each hardware layer is verified over defmt/RTT before the
//! next is stacked:
//!   A. clocks + RTT + GPIO              [done]
//!   B. FMC SDRAM (8 MB @ 0xD0000000)    [done: 8 MB verified, 0 errors]
//!   C. ILI9341 (SPI5) + LTDC + SDRAM framebuffer + test pattern   [done: issue #33]
//!   D. obc-app on glass (issue #34): the SDRAM framebuffer becomes an
//!      `embedded-graphics` `DrawTarget`, and the shared `App` (boots to Home/Idle,
//!      then opens the Map on a baked-in OBCM tile) is rendered through it via
//!      `App::render_frame` — the first time `obc-app` runs on hardware.
//!   E. Pushbutton input (issue #35): four GPIO buttons → obc-platform's
//!      board-agnostic `ButtonInput` debouncer → the shared gesture recognizer, so
//!      the UI is driven on-device — Home → Route menu → Map and back.
//!   F. microSD over SPI (issue #36): SPI4 + FatFs (`embedded-sdmmc`) → the card's map
//!      `.obcm` loads resident, the `/routes/*.obcr` catalog fills the Route menu, the
//!      chosen route streams from the card, and the ride logs to `/tracks` and saves as a
//!      `.gpx`. The FatFs byte adapters live in `obc-platform` (shared with the nRF
//!      board); only the SPI bus + chip-select here are board-specific.   <- this commit
//!
//! ## RAM split (issue #34 / #8)
//! The renderer's per-frame scratch is ~200 KB and the framebuffer is 150 KB; the
//! two do not both fit the F429's 192 KB internal SRAM. For the prototype the whole
//! `App` (which embeds the renderer) is placed in **SDRAM**, just past the two
//! framebuffers (double-buffered, 2x150 KB — see below) — simplest, runs the full-size
//! renderer. The 8 MB SDRAM swallows all of it. The cost is render-time:
//! the scratch is now behind the FMC's wait states (slower than the internal-RAM
//! `mcu-render-bench`); the per-frame time logged over RTT quantifies the delta.
//! A `small-scratch` cargo feature (internal-RAM scratch) is the fallback if that
//! delta ever matters — not needed yet.
//!
//! ## Double buffering
//! Rendering clears and repaints the whole frame, so drawing straight into the buffer
//! the LTDC is scanning makes the panel flash on every redraw — fine for a static
//! demo, but ugly once the UI animates (the hold bulge redraws continuously). So the
//! app path keeps **two** framebuffers: the LTDC scans the *front* while the app
//! renders the next frame into the *back*, then [`flip_to`] points the layer at the
//! back and reloads it at the next vertical blank (tear-free) and the roles swap. The
//! panel only ever shows a fully-rendered frame. (The `glass-demo` path stays single-
//! buffered — it draws one static screen and halts.)
//!
//! Clock: 180 MHz core from the 16 MHz HSI (no dependency on the DISC1 HSE), plus a
//! PLLSAI leg for the LTDC pixel clock.
//!
//! ## SDRAM pin map (FMC bank 2, IS42S16400J, 8 MB @ 0xD000_0000)
//! Address  A0-A11 : PF0 PF1 PF2 PF3 PF4 PF5 PF12 PF13 PF14 PF15 PG0 PG1
//! Bank     BA0-1  : PG4 PG5
//! Data     D0-D15 : PD14 PD15 PD0 PD1 PE7 PE8 PE9 PE10 PE11 PE12 PE13 PE14 PE15 PD8 PD9 PD10
//! Mask     NBL0-1 : PE0 PE1
//! Control         : SDCKE1 PB5 | SDCLK PG8 | SDNCAS PG15 | SDNE1 PB6 | SDNRAS PF11 | SDNWE PC0
//!
//! ## Pushbutton pin map (issue #35) — DISC1 headers, internal pull-ups, active-low
//! One common pin to GND; each switch to its GPIO (no external pull-ups/resistors —
//! the F429's internal pull-ups hold the lines high, a press pulls one low):
//!   PREV PD4 | NEXT PE3 | SELECT PE4 | BACK PD5
//! Clear of FMC/LTDC/SPI5/LEDs, and deliberately kept off SPI4's data pins so the SD
//! card (#36 uses SD-over-**SPI** / embedded-sdmmc, not SDIO) can take SPI4. PE4 =
//! SPI4_NSS is still SELECT, but SPI-mode SD uses a software CS, so it never needs
//! hardware NSS. PD4/PD5 are free GPIO broken out on the headers. PREV/BACK were moved
//! off PE2/PE5 for exactly this.
//!
//! ## microSD pin map (issue #36) — SPI4, chip-select held low, internal MISO pull-up
//! SPI4's only usable data pinout on the DISC1 (PE11-14 is all FMC): SCK PE2 / MISO PE5
//! / MOSI PE6. CS is a free GPIO (PD7), held LOW for the whole session — embassy's SPI
//! can't tolerate embedded-sdmmc toggling CS between a command and its reply (the card
//! drops the bus and CMD0's response is lost), so a no-op CS is used instead (see
//! `sd::NoCs` / `sd::init`). Wire the breakout: SCK→PE2, MOSI→PE6, MISO→PE5, CS→PD7,
//! GND→GND, VCC→3V3 (bare socket) or 5V (regulated breakout). Card is FAT16/FAT32, init
//! at ≤400 kHz then re-clocked (see `sd::init`).
//!   SCK PE2 | MISO PE5 | MOSI PE6 | CS PD7
//!
//! ## Display pin map (onboard ILI9341, 240x320)
//! Config (SPI5, 8-bit mode-0) : SCK PF7 | MOSI PF9 | CS/NCS PC2 | DCX/WRX PD13   (reset = NRST)
//! LTDC sync                   : HSYNC PC6 | VSYNC PA4 | CLK PG7 | DE PF10
//! LTDC RGB666 (R0/R1,G0/G1,B0/B1 not wired) — AF14 unless noted AF9:
//!   R2 PC10  R3 PB0*  R4 PA11  R5 PA12  R6 PB1*  R7 PG6
//!   G2 PA6   G3 PG10* G4 PB10  G5 PB11  G6 PC7   G7 PD3
//!   B2 PD6   B3 PG11  B4 PG12* B5 PA3   B6 PB8   B7 PB9        (* = AF9)
//! (all per ST's 32f429idiscovery-bsp; IS42S16400J + ILI9341 timings from there too.)

#![no_std]
#![no_main]

use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_stm32::fmc::Fmc;
use embassy_stm32::gpio::{AfType, Flex, Level, Output, OutputType, Pin, Speed};
use embassy_stm32::ltdc::{
    Ltdc, LtdcConfiguration, LtdcLayer, LtdcLayerConfig, PixelFormat, PolarityActive, PolarityEdge,
};
use embassy_stm32::spi::Spi;
use embassy_stm32::time::Hertz;
use embassy_stm32::Peri;
use embassy_time::{Delay, Timer};
use obc_platform::Framebuffer565;
use panic_probe as _;

// The shared-app render path — only the `not(glass-demo)` build drives it (the demo
// just exercises the framebuffer + text). Kept behind the cfg so the demo build is
// warning-free too.
#[cfg(not(feature = "glass-demo"))]
use embassy_stm32::gpio::{Input, Pull};
#[cfg(not(feature = "glass-demo"))]
use embassy_time::Instant;
#[cfg(not(feature = "glass-demo"))]
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
#[cfg(not(feature = "glass-demo"))]
use obc_app::{
    App, AppState, Fix, InputClock, LocationSource, RideClock, RouteSummary, Sensors, TrackSink,
};
#[cfg(not(feature = "glass-demo"))]
use obc_platform::ButtonInput;
#[cfg(not(feature = "glass-demo"))]
use obc_reader::{BBox, Reader};
#[cfg(not(feature = "glass-demo"))]
use obc_render::zoom_for_mpp;
#[cfg(not(feature = "glass-demo"))]
use obc_route::RouteReader;

#[cfg(feature = "glass-demo")]
mod demo;
// microSD map/route/track storage over SPI4 + FatFs (issue #36) — only the real-app build
// touches the card; the glass demo just exercises the framebuffer.
#[cfg(not(feature = "glass-demo"))]
mod sd;

const SDRAM_ADDR: usize = 0xD000_0000;
/// Front framebuffer at the base of SDRAM: 240x320 RGB565 = 150 KB. The app path adds a
/// second (back) buffer one [`FB_BYTES`] past it for double buffering; the LTDC is first
/// pointed here at init and thereafter flipped between the two by [`flip_to`].
const FB_ADDR: usize = SDRAM_ADDR;
const W: usize = 240;
const H: usize = 320;
/// Framebuffer extent in pixels / bytes (RGB565, 2 bytes each).
const FB_PIXELS: usize = W * H;
/// Only the app path needs this — the back-buffer offset, and it places the `App` in
/// SDRAM just past *both* framebuffers.
#[cfg(not(feature = "glass-demo"))]
const FB_BYTES: usize = FB_PIXELS * 2;

/// Baked-in OBCM **v5** map tile (issue #34): a small ~1.4 MB Teningen tile in
/// flash via `include_bytes!`. With #36 the map normally comes off the SD card; this stays
/// as the **fallback** when no card / no `.obcm` is present, so the device still boots to a
/// usable Map. Packed from `packer/small.obcm`. Behind the `baked-tile` feature (default on);
/// `--no-default-features` drops it for a tiny, fast-to-flash image while iterating.
#[cfg(all(not(feature = "glass-demo"), feature = "baked-tile"))]
static TILE: &[u8] = include_bytes!("../tiles/teningen.obcm");

/// SDRAM region (bytes) reserved for the resident map read off the SD card — placed just past
/// the App (see the loop). Caps the on-card tile size; comfortably fits the small tiles the
/// prototype uses (teningen is 1.35 MB) with megabytes of SDRAM to spare.
#[cfg(not(feature = "glass-demo"))]
const MAP_CAP: usize = 4 * 1024 * 1024;

/// Initial camera zoom, in ground **metres-per-pixel** (in the 0.5–4 mpp riding band).
/// Used for the Idle [`AppState`]; opening the Map via the Route menu resets to the
/// riding zoom, and PREV/NEXT then zoom from there.
#[cfg(not(feature = "glass-demo"))]
const INIT_MPP: f32 = 1.0;

/// Stand-in moving GPS until #38's USB feed: side length (m) and speed (m/s) of the square
/// loop [`SynthLocation`] walks. Slow enough to watch the user marker / breadcrumb crawl, big
/// enough that a saved ride is a real ~0.8 km loop that re-imports as a sane route.
#[cfg(not(feature = "glass-demo"))]
const SYNTH_LEG_M: f32 = 200.0;
#[cfg(not(feature = "glass-demo"))]
const SYNTH_SPEED_MPS: f32 = 5.0;

/// Microdegrees of latitude per metre north (the map/route coordinate convention). Longitude
/// scales this by 1/cos(lat), via [`obc_route::cos_lat`].
#[cfg(not(feature = "glass-demo"))]
const UDEG_PER_M: f32 = 1_000_000.0 / 111_320.0;

/// Main-loop button-sample period (ms). Buttons are sampled every tick so quick taps
/// and hold timing are caught even when a heavy Map frame stretches the render cadence.
#[cfg(not(feature = "glass-demo"))]
const LOOP_MS: u64 = 8;

/// Keep redrawing for this long (ms) after the last input/gesture/hold activity, so a
/// confirm-ring bulge plays its pop/retract out (POP ≈ 220 ms) before frames stop.
#[cfg(not(feature = "glass-demo"))]
const ANIM_TAIL_MS: u32 = 300;

/// Idle redraw heartbeat (ms): redraw at least this often even with no input, so
/// time-based screens (e.g. Statistics) stay live. A static Map then costs ~1 fps.
#[cfg(not(feature = "glass-demo"))]
const IDLE_REFRESH_MS: u32 = 1000;

/// A stand-in moving [`LocationSource`] until #38 streams a real GPS over USB-CDC: the fix
/// walks a slow square loop around a centre, driven by the wall clock. Unlike a constant fix,
/// this gives the ride accumulators, breadcrumb and `.obct` log real motion — so a saved
/// ride is a non-degenerate `.gpx` that re-imports cleanly (issue #36's save-loop deliverable).
/// The centre is the map (or loaded route's) start, re-pointed via [`recenter`](Self::recenter).
#[cfg(not(feature = "glass-demo"))]
struct SynthLocation {
    center_lon: i32,
    center_lat: i32,
    /// 1/cos(lat) folded into the east-metres → microdegrees scale, refreshed on recenter.
    udeg_per_m_east: f32,
    start: Instant,
}

#[cfg(not(feature = "glass-demo"))]
impl SynthLocation {
    fn new(center_lon: i32, center_lat: i32, start: Instant) -> Self {
        let mut s = SynthLocation { center_lon, center_lat, udeg_per_m_east: 0.0, start };
        s.recenter(center_lon, center_lat);
        s
    }

    /// Move the loop's centre (e.g. onto a freshly-loaded route's start) and refresh the
    /// longitude scale for the new latitude.
    fn recenter(&mut self, lon: i32, lat: i32) {
        self.center_lon = lon;
        self.center_lat = lat;
        self.udeg_per_m_east = UDEG_PER_M / obc_route::cos_lat(lat);
    }
}

#[cfg(not(feature = "glass-demo"))]
impl LocationSource for SynthLocation {
    fn poll(&mut self) -> Option<Fix> {
        // Position along the square as a function of elapsed time. Each leg takes leg_s seconds;
        // the heading is the leg's constant bearing (no trig needed). The loop is centred on the
        // square so the camera sits in its middle.
        let t = self.start.elapsed().as_millis() as f32 / 1000.0;
        let leg_s = SYNTH_LEG_M / SYNTH_SPEED_MPS;
        let phase = t % (4.0 * leg_s);
        let leg = (phase / leg_s) as u32;
        let d = (phase - leg as f32 * leg_s) * SYNTH_SPEED_MPS; // metres into this leg
        let (east, north, course) = match leg {
            0 => (d, 0.0, 90.0),                        // →E along the south edge
            1 => (SYNTH_LEG_M, d, 0.0),                 // →N up the east edge
            2 => (SYNTH_LEG_M - d, SYNTH_LEG_M, 270.0), // →W along the north edge
            _ => (0.0, SYNTH_LEG_M - d, 180.0),         // →S down the west edge
        };
        let east = east - SYNTH_LEG_M / 2.0; // centre the square on the centre point
        let north = north - SYNTH_LEG_M / 2.0;
        Some(Fix {
            lon: self.center_lon + (east * self.udeg_per_m_east) as i32,
            lat: self.center_lat + (north * UDEG_PER_M) as i32,
            course: Some(course),
            speed_mps: Some(SYNTH_SPEED_MPS),
        })
    }
}

/// ILI9341 power-on / RGB-interface init, transcribed verbatim from ST's
/// `32f429idiscovery-bsp` `ili9341_Init`: `(command, data bytes, delay-ms-after)`.
/// The load-bearing entries for LTDC operation are 0xB0=0xC2 (RGB interface control)
/// and the second 0xB6 / 0xF6 writes that select the RGB (DPI) data path.
const ILI9341_INIT: &[(u8, &[u8], u16)] = &[
    (0xCA, &[0xC3, 0x08, 0x50], 0),
    (0xCF, &[0x00, 0xC1, 0x30], 0),
    (0xED, &[0x64, 0x03, 0x12, 0x81], 0),
    (0xE8, &[0x85, 0x00, 0x78], 0),
    (0xCB, &[0x39, 0x2C, 0x00, 0x34, 0x02], 0),
    (0xF7, &[0x20], 0),
    (0xEA, &[0x00, 0x00], 0),
    (0xB1, &[0x00, 0x1B], 0),
    (0xB6, &[0x0A, 0xA2], 0),
    (0xC0, &[0x10], 0),
    (0xC1, &[0x10], 0),
    (0xC5, &[0x45, 0x15], 0),
    (0xC7, &[0x90], 0),
    (0x36, &[0xC8], 0), // MADCTL: MY|MX|BGR (ST's orientation; BGR colour order)
    (0xF2, &[0x00], 0),
    (0xB0, &[0xC2], 0), // RGB interface signal control -> RGB/DPI mode (bypass, like ST)
    (0xB6, &[0x0A, 0xA7, 0x27, 0x04], 0),
    (0x2A, &[0x00, 0x00, 0x00, 0xEF], 0), // column 0..239
    (0x2B, &[0x00, 0x00, 0x01, 0x3F], 0), // page 0..319
    (0xF6, &[0x01, 0x00, 0x06], 0),
    (0x2C, &[], 200), // memory write, then settle
    (0x26, &[0x01], 0),
    (
        0xE0,
        &[0x0F, 0x29, 0x24, 0x0C, 0x0E, 0x09, 0x4E, 0x78, 0x3C, 0x09, 0x13, 0x05, 0x17, 0x11, 0x00],
        0,
    ),
    (
        0xE1,
        &[0x00, 0x16, 0x1B, 0x04, 0x11, 0x07, 0x31, 0x33, 0x42, 0x05, 0x0C, 0x0A, 0x28, 0x2F, 0x0F],
        0,
    ),
    (0x11, &[], 200), // sleep out, then settle (datasheet >= 120 ms before display on)
    (0x29, &[], 0),   // display on
    (0x2C, &[], 0),   // memory write start
];

/// Configure one GPIO as an LTDC alternate-function output. Used directly (instead of
/// `Ltdc::new_with_pins`) because the DISC1 wires only RGB666 — the low colour bits
/// have no pin, so the typed 24-pin constructor doesn't fit. The returned `Flex` must
/// be kept alive for the AF config to persist.
fn af_pin(pin: Peri<'static, impl Pin>, af: u8) -> Flex<'static> {
    let mut f = Flex::new(pin);
    f.set_as_af_unchecked(af, AfType::output(OutputType::PushPull, Speed::VeryHigh));
    f
}

/// Idle forever after an unrecoverable bring-up failure. Breaks to the debugger **only when
/// one is attached** (so `probe-rs run` regains control and releases the ST-LINK), then
/// low-power idles. A standalone boot (plain NRST / battery, no debugger) must NOT execute a
/// bare `bkpt` — with `C_DEBUGEN` clear it escalates to a HardFault — so it just `wfi`s.
fn halt() -> ! {
    if cortex_m::peripheral::DCB::is_debugger_attached() {
        cortex_m::asm::bkpt();
    }
    loop {
        cortex_m::asm::wfi();
    }
}

/// Point LTDC layer 1 at the framebuffer at `addr`, reloading at the next vertical
/// blank — the double-buffer flip. Unlike the immediate reload used at init, the
/// **vblank** reload switches buffers between frames, so there's no tear; the bounded
/// poll on the hardware-cleared `VBR` bit then waits for the switch to actually land
/// (so the old front is free to reuse) without ever spinning forever — the same
/// "no blocking wait that can hang probe-rs" rule init follows by avoiding the reload
/// interrupt. A frame is ~15 ms at the 6 MHz DOTCLK, so the reload lands well within
/// the 50 ms cap in normal operation.
#[cfg(not(feature = "glass-demo"))]
fn flip_to(addr: usize) {
    use stm32_metapac::ltdc::vals::Vbr;
    use stm32_metapac::LTDC;
    LTDC.layer(LtdcLayer::Layer1 as usize).cfbar().modify(|w| w.set_cfbadd(addr as u32));
    LTDC.srcr().modify(|w| w.set_vbr(Vbr::RELOAD));
    let t0 = Instant::now();
    while LTDC.srcr().read().vbr() == Vbr::RELOAD {
        if t0.elapsed().as_millis() > 50 {
            break;
        }
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 180 MHz core from HSI (16/8=2 MHz -> x180 -> /2). Plus a PLLSAI leg for the LTDC
    // pixel clock: VCO = 2 MHz x 96 = 192 MHz, /R(4) = 48 MHz, /PLLSAIDIVR(8) = 6 MHz
    // DOTCLK (matching ST's BSP). embassy's ltdc driver hard-codes PLLSAIDIVR=2, so the
    // DIV8 is forced back on via the PAC right after `Ltdc::new()` below.
    let p = {
        let mut config = embassy_stm32::Config::default();
        use embassy_stm32::rcc::*;
        config.rcc.hsi = true;
        config.rcc.pll_src = PllSource::HSI;
        config.rcc.pll = Some(Pll {
            prediv: PllPreDiv::DIV8,
            mul: PllMul::MUL180,
            divp: Some(PllPDiv::DIV2),
            divq: None,
            divr: None,
        });
        config.rcc.pllsai = Some(Pll {
            prediv: PllPreDiv::DIV8, // shared M with the main PLL -> 2 MHz input
            mul: PllMul::MUL96,      // VCO = 2 x 96 = 192 MHz (same VCO as ST's BSP)
            divp: None,
            divq: None,
            divr: Some(PllRDiv::DIV4), // PLLSAI_R = 192 / 4 = 48 MHz
        });
        config.rcc.sys = Sysclk::PLL1_P;
        config.rcc.ahb_pre = AHBPrescaler::DIV1; // HCLK 180 MHz
        config.rcc.apb1_pre = APBPrescaler::DIV4; // PCLK1 45 MHz
        config.rcc.apb2_pre = APBPrescaler::DIV2; // PCLK2 90 MHz
        embassy_stm32::init(config)
    };

    defmt::info!("obc-fw-stm32f429: phase C (ILI9341 + LTDC), 180 MHz core / 6 MHz pixel clock");

    let mut green = Output::new(p.PG13, Level::High, Speed::Low);
    let _red = Output::new(p.PG14, Level::Low, Speed::Low);

    // --- SDRAM (framebuffer storage) ---
    let mut sdram = Fmc::sdram_a12bits_d16bits_4banks_bank2(
        p.FMC,
        p.PF0,
        p.PF1,
        p.PF2,
        p.PF3,
        p.PF4,
        p.PF5,
        p.PF12,
        p.PF13,
        p.PF14,
        p.PF15,
        p.PG0,
        p.PG1,
        p.PG4,
        p.PG5,
        p.PD14,
        p.PD15,
        p.PD0,
        p.PD1,
        p.PE7,
        p.PE8,
        p.PE9,
        p.PE10,
        p.PE11,
        p.PE12,
        p.PE13,
        p.PE14,
        p.PE15,
        p.PD8,
        p.PD9,
        p.PD10,
        p.PE0,
        p.PE1,
        p.PB5,
        p.PG8,
        p.PG15,
        p.PB6,
        p.PF11,
        p.PC0,
        stm32_fmc::devices::is42s16400j_7::Is42s16400j {},
    );
    let ram_ptr: *mut u32 = sdram.init(&mut Delay);
    defmt::info!("SDRAM base {=u32:#010x}", ram_ptr as u32);

    // --- SDRAM framebuffer(s). Cleared to black up front so the panel shows black, not
    // SDRAM garbage, in the window between LTDC turn-on and the first rendered frame.
    // The LTDC starts scanning FB_ADDR (the front buffer); the app path adds a second
    // buffer just past it and double-buffers (see the render loop), so each buffer is
    // wrapped in its own short-lived slice instead of one long-lived `&mut` — two live
    // `&mut` over the same SDRAM would alias.
    // SAFETY: the FMC maps a contiguous FB_PIXELS×u16 region at FB_ADDR (verified in
    // phase B); the borrow ends at the statement, and the LTDC only *reads* it by DMA. ---
    unsafe { core::slice::from_raw_parts_mut(FB_ADDR as *mut u16, FB_PIXELS) }.fill(0x0000);
    defmt::info!("framebuffer ready: {=usize}x{=usize} RGB565 in SDRAM, cleared", W, H);

    // --- LTDC: drive the panel's RGB lines, scanning the SDRAM framebuffer ---
    // Brought up BEFORE the ILI9341 so the sync/DE/DOTCLK are already running when the
    // panel enters RGB mode (ST's BSP does the same order); the panel locks its RGB
    // interface onto the live sync instead of free-running.
    // 240x320, timings from ST's BSP: HSYNC 10 / HBP 20 / HFP 10, VSYNC 2 / VBP 2 / VFP 4.
    let ltdc_config = LtdcConfiguration {
        active_width: W as u16,
        active_height: H as u16,
        h_back_porch: 20,
        h_front_porch: 10,
        v_back_porch: 2,
        v_front_porch: 4,
        h_sync: 10,
        v_sync: 2,
        h_sync_polarity: PolarityActive::ActiveLow,
        v_sync_polarity: PolarityActive::ActiveLow,
        data_enable_polarity: PolarityActive::ActiveLow,
        pixel_clock_polarity: PolarityEdge::RisingEdge,
    };

    let mut ltdc = Ltdc::new(p.LTDC);
    // `Ltdc::new` forces PLLSAIDIVR = 2 (giving 48/2 = 24 MHz); override it to 8 so the
    // DOTCLK is 48/8 = 6 MHz, exactly matching ST's BSP (the ILI9341 RGB interface mis-
    // samples above ~6 MHz, which sheared the image at our earlier 8 MHz).
    stm32_metapac::RCC
        .dckcfgr()
        .modify(|w| w.set_pllsaidivr(stm32_metapac::rcc::vals::Pllsaidivr::DIV8));
    // Drive only the 18 wired RGB666 bits + 4 sync/clk/de lines (AF14, except the
    // four AF9 pins). Kept alive in `_ltdc_pins` so the AF config persists.
    let _ltdc_pins = [
        af_pin(p.PC6, 14),
        af_pin(p.PA4, 14),
        af_pin(p.PG7, 14),
        af_pin(p.PF10, 14), // HSYNC VSYNC CLK DE
        af_pin(p.PC10, 14),
        af_pin(p.PA11, 14),
        af_pin(p.PA12, 14),
        af_pin(p.PG6, 14), // R2 R4 R5 R7
        af_pin(p.PA6, 14),
        af_pin(p.PB10, 14),
        af_pin(p.PB11, 14),
        af_pin(p.PC7, 14),
        af_pin(p.PD3, 14), // G2 G4 G5 G6 G7
        af_pin(p.PD6, 14),
        af_pin(p.PG11, 14),
        af_pin(p.PA3, 14),
        af_pin(p.PB8, 14),
        af_pin(p.PB9, 14), // B2 B3 B5 B6 B7
        af_pin(p.PB0, 9),
        af_pin(p.PB1, 9),
        af_pin(p.PG10, 9),
        af_pin(p.PG12, 9), // R3 R6 G3 B4
    ];
    ltdc.init(&ltdc_config);

    let layer_config = LtdcLayerConfig {
        pixel_format: PixelFormat::RGB565,
        layer: LtdcLayer::Layer1,
        window_x0: 0,
        window_x1: W as u16,
        window_y0: 0,
        window_y1: H as u16,
    };
    ltdc.init_layer(&layer_config, None);
    // embassy's init_layer programs CFBLL = active*bpp + 7, but RM0090 specifies
    // active*bpp + 3; that extra 4-byte line over-read carries into the next line and
    // is the per-line horizontal shear. Reprogram CFBLR with the spec value (and set
    // CFBP explicitly — the metapac field write zeroes the other field otherwise).
    // RGB565: pitch 240*2 = 480, line length 480 + 3 = 483.
    stm32_metapac::LTDC.layer(0).cfblr().modify(|w| {
        w.set_cfbp(480);
        w.set_cfbll(483);
    });

    // Point layer 1 at the SDRAM framebuffer and request a vblank reload. Done via the
    // PAC rather than `Ltdc::set_buffer().await` so bring-up doesn't depend on the LTDC
    // reload interrupt (a never-firing wait would hang probe-rs -> stuck ST-LINK).
    {
        use stm32_metapac::ltdc::vals::Imr;
        use stm32_metapac::LTDC;
        LTDC.layer(LtdcLayer::Layer1 as usize).cfbar().modify(|w| w.set_cfbadd(FB_ADDR as u32));
        LTDC.srcr().write(|w| w.set_imr(Imr::RELOAD)); // immediate reload (no vblank/interrupt wait)
    }

    // --- ILI9341 init over SPI5, now that the LTDC sync is live (panel into RGB/DPI
    // mode + display on; it locks onto the running HSYNC/VSYNC/DE) ---
    let mut spi_cfg = embassy_stm32::spi::Config::default();
    spi_cfg.frequency = Hertz(5_000_000); // ST uses 5.6 MHz; mode 0 is the default
    let mut spi = Spi::new_blocking_txonly(p.SPI5, p.PF7, p.PF9, spi_cfg);
    let mut cs = Output::new(p.PC2, Level::High, Speed::VeryHigh);
    let mut dcx = Output::new(p.PD13, Level::High, Speed::VeryHigh);

    Timer::after_millis(20).await; // panel power-up after NRST release
    for &(cmd, data, delay_ms) in ILI9341_INIT {
        cs.set_low();
        dcx.set_low(); // command
        let _ = spi.blocking_write(&[cmd]);
        if !data.is_empty() {
            dcx.set_high(); // data
            let _ = spi.blocking_write(data);
        }
        cs.set_high();
        if delay_ms > 0 {
            Timer::after_millis(delay_ms as u64).await;
        }
    }
    defmt::info!("ILI9341 init done");

    green.set_high();
    defmt::info!("display up: LTDC scanning SDRAM framebuffer");

    // --- glass-demo (issue #33 leftover): font ladder + palette on glass, then halt.
    // The device analog of the simulator's `--text-demo`; verifies the text raster +
    // RGB565 colour path in isolation before the whole app is pointed at the panel. ---
    #[cfg(feature = "glass-demo")]
    {
        // Single-buffer demo (a static screen, then halt — no flicker to fix).
        // SAFETY: the LTDC scans FB_ADDR and we only write it; sole borrow.
        let fb_buf = unsafe { core::slice::from_raw_parts_mut(FB_ADDR as *mut u16, FB_PIXELS) };
        let mut fb = Framebuffer565::new(fb_buf, W as u32, H as u32);
        let _ = demo::font_palette_demo(&mut fb);
        defmt::info!("glass-demo: font + palette rendered; halting");
        halt()
    }

    // --- obc-app on glass, button-driven, now reading real data off SD (issues #34/#35/#36):
    // boot Home/Idle, list the card's routes, ride the chosen route on the card's map, and save
    // the ride back as a `.gpx`. A missing/bad card degrades to the baked tile + a stub route. ---
    #[cfg(not(feature = "glass-demo"))]
    {
        // microSD over SPI4 (CS = PD7). Init the bus slow (SD spec ≤400 kHz); `sd::init`
        // re-clocks it after the card is up. A `None` here is a missing/bad card — handled by
        // falling back below, never a panic (acceptance criterion).
        let mut sd_cfg = embassy_stm32::spi::Config::default();
        sd_cfg.frequency = Hertz(400_000);
        sd_cfg.miso_pull = Pull::Up; // hold DO high when the card isn't driving the line
        let sd_spi = Spi::new_blocking(p.SPI4, p.PE2, p.PE6, p.PE5, sd_cfg);
        let sd_cs = Output::new(p.PD7, Level::High, Speed::VeryHigh);
        let mut storage = sd::init(sd_spi, sd_cs);

        // SDRAM layout: the App sits just past both framebuffers (RAM-split note in the module
        // header), and the resident map is read into the region just past the App. Compute both
        // addresses up front so the map load below lands clear of the App slot.
        let app_align = core::mem::align_of::<App>();
        let app_addr = (SDRAM_ADDR + 2 * FB_BYTES + app_align - 1) & !(app_align - 1);
        let map_addr = (app_addr + core::mem::size_of::<App>() + 7) & !7;

        // Map: read the card's `.obcm` tile resident into SDRAM, else fall back to the baked-in
        // tile. The buffer lives in SDRAM for the whole run; `Reader` borrows it read-only.
        // SAFETY: the 8 MB SDRAM holds [map_addr, map_addr+MAP_CAP) clear of the framebuffers
        // and the (not-yet-written) App slot; this is its sole owner.
        let map_buf = unsafe { core::slice::from_raw_parts_mut(map_addr as *mut u8, MAP_CAP) };
        let map_len = match storage.as_ref() {
            Some(s) => s.load_map(map_buf),
            None => None,
        };
        let map_bytes: &[u8] = if let Some(n) = map_len {
            defmt::info!("map: {=usize} B from SD card", n);
            &map_buf[..n]
        } else {
            // No SD map: fall back to the baked-in tile, or halt if it's been compiled out
            // (the fast-flash build) — the SD init result has already been logged above.
            #[cfg(feature = "baked-tile")]
            {
                defmt::info!("map: {=usize} B baked-in tile (no SD card map)", TILE.len());
                TILE
            }
            #[cfg(not(feature = "baked-tile"))]
            {
                defmt::error!("no SD card map and baked-tile is disabled — halting");
                halt()
            }
        };
        let reader = match Reader::new(map_bytes) {
            Ok(r) => r,
            Err(_) => {
                defmt::error!("map is not valid OBCM ({=usize} bytes)", map_bytes.len());
                halt()
            }
        };

        // Idle camera + synthetic-GPS centre = the loaded map's bbox centre, so any tile frames
        // sensibly (not only the baked teningen one, which the constant fix used to assume).
        let cam_lon = ((reader.bbox.min_lon as i64 + reader.bbox.max_lon as i64) / 2) as i32;
        let cam_lat = ((reader.bbox.min_lat as i64 + reader.bbox.max_lat as i64) / 2) as i32;

        // Native RGB565 panel → the color_fn is the identity (device-64 quantization is a
        // host/simulator concern; see obc-platform::framebuffer).
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

        // Place + build the App in SDRAM at the reserved slot. ptr::write keeps the ~200 KB
        // scratch off the stack (opt-level 3 + LTO emit App::new* straight to the slot).
        let app_ptr = app_addr as *mut App;
        defmt::info!(
            "App in SDRAM @ {=u32:#010x} ({=usize} B); map @ {=u32:#010x}",
            app_addr as u32,
            core::mem::size_of::<App>(),
            map_addr as u32
        );
        // Power-on screen: Home / Idle — the user drives navigation from here.
        // SAFETY: app_ptr is a valid, aligned, exclusively-owned SDRAM slot, fully initialized
        // by ptr::write before any read.
        unsafe {
            app_ptr.write(App::new_idle(AppState::new(cam_lon, cam_lat, zoom_for_mpp(INIT_MPP))));
        }
        let app = unsafe { &mut *app_ptr };

        // Routes: the card's `/routes` catalog feeds the Route menu; with no card a single stub
        // route keeps the Map reachable (the pre-#36 behaviour).
        match storage.as_mut() {
            Some(s) => {
                let catalog = s.scan_routes();
                app.set_routes(&catalog);
            }
            None => {
                let mut name = heapless::String::new();
                let _ = name.push_str("Teningen");
                app.set_routes(&[RouteSummary {
                    name,
                    distance_km: 0,
                    climb_m: 0,
                    bbox: BBox {
                        min_lon: cam_lon,
                        min_lat: cam_lat,
                        max_lon: cam_lon,
                        max_lat: cam_lat,
                    },
                    start_lon: cam_lon,
                    start_lat: cam_lat,
                }]);
            }
        }

        // Four pushbuttons (issue #35): one common pin to GND, each switch to its GPIO, internal
        // pull-ups, active-low. PREV/BACK on PD4/PD5 keep SPI4's data pins free for the SD card.
        let mut buttons = ButtonInput::new(
            Input::new(p.PD4, Pull::Up), // PREV   → Turn(-1)
            Input::new(p.PE3, Pull::Up), // NEXT   → Turn(+1)
            Input::new(p.PE4, Pull::Up), // SELECT → encoder press / hold
            Input::new(p.PD5, Pull::Up), // BACK   → back / back-hold
        );

        // Stand-in moving GPS (until #38's USB feed) so a ride is real: a slow square loop near
        // the map centre, re-centred on the route start when a route loads.
        let start = Instant::now();
        let mut synth = SynthLocation::new(cam_lon, cam_lat, start);

        // Double buffering: the LTDC scans the *front* while the app renders the *back*, then we
        // flip at the next vblank (tear-free) and swap. FB_ADDR is the initial front; the back
        // sits one FB_BYTES past it. SAFETY: the back is never the buffer the LTDC is scanning.
        let mut front_addr = FB_ADDR;
        let mut back_addr = FB_ADDR + FB_BYTES;
        unsafe { core::slice::from_raw_parts_mut(back_addr as *mut u16, FB_PIXELS) }.fill(0x0000);
        defmt::info!(
            "input live: PD4 PREV / PE3 NEXT / PE4 SELECT / PD5 BACK; double-buffered; SD card {=bool}",
            storage.is_some()
        );

        // Render-on-demand loop. Buttons are sampled every LOOP_MS; the screen redraws only on
        // input/gesture activity, an animating hold bulge (ANIM_TAIL_MS), or the IDLE_REFRESH
        // heartbeat — so a static Map costs ~1 fps. The active route's geometry and the ride log
        // are opened only on redraw frames, which also paces the `.obct` log to the redraw rate.
        let now0 = start.elapsed().as_millis() as u32;
        let mut prev_gesture = app.last_gesture();
        let mut prev_route: Option<usize> = None;
        let mut last_activity = now0;
        let mut last_render = now0.wrapping_sub(IDLE_REFRESH_MS); // force the first frame
        loop {
            let now = start.elapsed().as_millis() as u32;
            buttons.update(now);
            let had_events = buttons.has_pending();
            app.handle_input(InputClock(now), &mut buttons);

            // Re-centre the synthetic GPS onto a freshly-loaded route's start so Follow doesn't
            // yank the camera off it.
            let active = app.activity.active_route;
            if active != prev_route {
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    synth.recenter(r.start_lon, r.start_lat);
                }
                prev_route = active;
            }

            // Reconcile the card to the app's intent: open/close the active route's geometry and
            // the ride log (begin on load, finalise-to-GPX on Finish), reading the save name from
            // the active route. Cheap when nothing changed.
            if let Some(s) = storage.as_mut() {
                let action = app.activity.take_track_action();
                let session = app.activity.session;
                let mut name: heapless::String<64> = heapless::String::new();
                if let Some(r) = active.and_then(|i| app.routes().get(i)) {
                    let _ = name.push_str(&r.name);
                }
                s.reconcile_route(active);
                s.reconcile_track(action, session, &name);
            }

            // Redraw window: fresh events, an in-flight hold ring, a changed gesture, or the
            // idle heartbeat (the synthetic fix moving is deliberately *not* activity — same as
            // a real GPS would be once #38 lands; the heartbeat keeps the moving map live).
            let gesture = app.last_gesture();
            let ring = app.encoder_hold_progress() > 0.0 || app.back_hold_progress() > 0.0;
            if had_events || ring || gesture != prev_gesture {
                last_activity = now;
            }
            prev_gesture = gesture;
            let interactive = now.wrapping_sub(last_activity) < ANIM_TAIL_MS;
            let heartbeat = now.wrapping_sub(last_render) >= IDLE_REFRESH_MS;
            let redraw = interactive || heartbeat;

            // Open the active route's geometry + ride-log sink only on redraw frames — bounding
            // per-frame SD I/O and pacing the ride log to the redraw rate (~1 Hz idle, up to the
            // panel rate while interacting) rather than logging a point every loop.
            let route_src =
                if redraw { storage.as_ref().and_then(|s| s.route_source()) } else { None };
            let route = route_src.as_ref().and_then(|s| RouteReader::open(s).ok());
            let mut tsink =
                if redraw { storage.as_ref().and_then(|s| s.track_sink()) } else { None };
            let track_dyn = tsink.as_mut().map(|t| t as &mut dyn TrackSink);

            app.tick(
                RideClock(now),
                Sensors { loc: &mut synth, altimeter: None, track: track_dyn },
                route.as_ref(),
            );

            if redraw {
                let t0 = Instant::now();
                // Render into the back buffer (not the one being scanned out).
                // SAFETY: back_addr is the buffer the LTDC is *not* scanning; the only live
                // `&mut` over it, dropped before the flip.
                let back =
                    unsafe { core::slice::from_raw_parts_mut(back_addr as *mut u16, FB_PIXELS) };
                let mut fb = Framebuffer565::new(back, W as u32, H as u32);
                let stats = app.render_frame(
                    &mut fb,
                    &reader,
                    route.as_ref(),
                    W as f32,
                    H as f32,
                    color_fn,
                );
                let render_us = t0.elapsed().as_micros();

                // Flip to the freshly-drawn buffer at the next vblank, then swap roles.
                flip_to(back_addr);
                core::mem::swap(&mut front_addr, &mut back_addr);
                last_render = now;
                defmt::debug!(
                    "frame: {=u64} us | lod {=usize} | feat {=usize}/{=usize}",
                    render_us,
                    stats.lod,
                    stats.features_drawn,
                    stats.features_tried
                );
            }
            Timer::after_millis(LOOP_MS).await;
        }
    }
}
