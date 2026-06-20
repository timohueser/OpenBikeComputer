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
//!      `App::render_frame` — the first time `obc-app` runs on hardware.   <- this commit
//!
//! ## RAM split (issue #34 / #8)
//! The renderer's per-frame scratch is ~200 KB and the framebuffer is 150 KB; the
//! two do not both fit the F429's 192 KB internal SRAM. For the prototype the whole
//! `App` (which embeds the renderer) is placed in **SDRAM**, just past the
//! framebuffer — simplest, runs the full-size renderer. The cost is render-time:
//! the scratch is now behind the FMC's wait states (slower than the internal-RAM
//! `mcu-render-bench`); the per-frame time logged over RTT quantifies the delta.
//! A `small-scratch` cargo feature (internal-RAM scratch) is the fallback if that
//! delta ever matters — not needed yet.
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
use embassy_time::Instant;
#[cfg(not(feature = "glass-demo"))]
use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565};
#[cfg(not(feature = "glass-demo"))]
use obc_app::{App, AppState, Fix, LocationSource, RideClock, Sensors};
#[cfg(not(feature = "glass-demo"))]
use obc_reader::Reader;
#[cfg(not(feature = "glass-demo"))]
use obc_render::zoom_for_mpp;

#[cfg(feature = "glass-demo")]
mod demo;

const SDRAM_ADDR: usize = 0xD000_0000;
/// Framebuffer at the base of SDRAM: 240x320 RGB565 = 150 KB.
const FB_ADDR: usize = SDRAM_ADDR;
const W: usize = 240;
const H: usize = 320;
/// Framebuffer extent in pixels / bytes (RGB565, 2 bytes each).
const FB_PIXELS: usize = W * H;
/// Only the app path needs this — it places the `App` in SDRAM just past the FB.
#[cfg(not(feature = "glass-demo"))]
const FB_BYTES: usize = FB_PIXELS * 2;

/// Baked-in OBCM **v5** map tile (issue #34): a small ~1.4 MB Teningen tile in
/// flash via `include_bytes!`, so the Map screen has data before SD/USB exist
/// (steps #36/#38). Packed from `packer/small.obcm`; must stay well under the F429's
/// 2 MB flash beside code. (The issue named the bench's `teningen.obcm`, which was
/// gitignored and never committed — this committed tile is its stand-in.)
#[cfg(not(feature = "glass-demo"))]
static TILE: &[u8] = include_bytes!("../tiles/teningen.obcm");

/// Camera centre / stub fix: Teningen's dense core (microdegrees), the busiest part
/// of the tile — the same spot the `mcu-render-bench` aimed at, so the on-glass
/// render is comparable to the bench's timings.
#[cfg(not(feature = "glass-demo"))]
const CAM_LON: i32 = 7_813_599; // 7.81360 E
#[cfg(not(feature = "glass-demo"))]
const CAM_LAT: i32 = 48_126_492; // 48.12649 N

/// Map zoom presets cycled on glass, in ground **metres-per-pixel** (kept in the
/// 0.5–4 mpp band — the riding/panning range). `zoom_for_mpp` maps each to the
/// camera zoom; the tile's LOD table (breakpoints 2/4 mpp) walks LOD2→LOD1 across
/// them, so the per-frame log covers more than one detail level.
#[cfg(not(feature = "glass-demo"))]
const MPP_PRESETS: &[(&str, f32)] = &[("z0.5", 0.5), ("z1.0", 1.0), ("z2.0", 2.0), ("z4.0", 4.0)];

/// A fixed-fix [`LocationSource`]: there is no GPS yet (step #38 streams a fake one
/// over USB-CDC), so this returns a constant fix at the tile's dense core. In Follow
/// mode that parks the camera there and the Map draws the user marker at centre.
#[cfg(not(feature = "glass-demo"))]
struct StubLocation(Fix);
#[cfg(not(feature = "glass-demo"))]
impl LocationSource for StubLocation {
    fn poll(&mut self) -> Option<Fix> {
        Some(self.0)
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

    // --- SDRAM framebuffer as a flat &mut [u16] — the `DrawTarget` the app/demo
    // render into below (replacing phase C's raw test pattern). Cleared to black up
    // front so the panel shows black, not SDRAM garbage, in the window between LTDC
    // turn-on and the first rendered frame.
    // SAFETY: the FMC maps a contiguous FB_PIXELS×u16 region at FB_ADDR (verified in
    // phase B); nothing else aliases it — the LTDC only *reads* it by DMA. ---
    let fb_buf: &'static mut [u16] =
        unsafe { core::slice::from_raw_parts_mut(FB_ADDR as *mut u16, FB_PIXELS) };
    fb_buf.fill(0x0000);
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
        let mut fb = Framebuffer565::new(fb_buf, W as u32, H as u32);
        let _ = demo::font_palette_demo(&mut fb);
        defmt::info!("glass-demo: font + palette rendered; halting");
        cortex_m::asm::bkpt();
        loop {
            cortex_m::asm::wfi();
        }
    }

    // --- obc-app on glass (issue #34): boot Home/Idle, then the Map on the baked tile. ---
    #[cfg(not(feature = "glass-demo"))]
    {
        let reader = match Reader::new(TILE) {
            Ok(r) => r,
            Err(_) => {
                defmt::error!("baked tile is not valid OBCM ({=usize} bytes)", TILE.len());
                cortex_m::asm::bkpt();
                loop {
                    cortex_m::asm::wfi();
                }
            }
        };
        // Native RGB565 panel → the color_fn is the identity (device-64 quantization
        // is a host/simulator concern; see obc-platform::framebuffer).
        let color_fn = |c: u16| Rgb565::from(RawU16::new(c));

        // Place the App in SDRAM, just past the framebuffer (RAM-split note in the
        // module header). Built in place via ptr::write so the ~200 KB scratch never
        // lands on the stack — opt-level 3 + LTO emit App::new* straight to the slot.
        let app_align = core::mem::align_of::<App>();
        let app_addr = (SDRAM_ADDR + FB_BYTES + app_align - 1) & !(app_align - 1);
        let app_ptr = app_addr as *mut App;
        defmt::info!(
            "App in SDRAM @ {=u32:#010x} ({=usize} bytes); FB {=usize} bytes",
            app_addr as u32,
            core::mem::size_of::<App>(),
            FB_BYTES
        );

        // Phase A — the real power-on screen: Home / Idle.
        // SAFETY: app_ptr is a valid, aligned, exclusively-owned SDRAM slot, fully
        // initialized by ptr::write before any read.
        unsafe {
            app_ptr.write(App::new_idle(AppState::new(
                CAM_LON,
                CAM_LAT,
                zoom_for_mpp(MPP_PRESETS[0].1),
            )));
        }
        {
            let app = unsafe { &mut *app_ptr };
            let mut fb = Framebuffer565::new(&mut *fb_buf, W as u32, H as u32);
            let _ = app.render_frame(&mut fb, &reader, None, W as f32, H as f32, color_fn);
        }
        defmt::info!("booted to Home (Idle)");
        Timer::after_millis(2500).await;

        // Phase B — open the Map on the baked tile (Riding, Follow). No input yet
        // (step #35), so build the map-first App directly rather than walking Home ->
        // Route menu by gesture. drop_in_place runs Phase A's (trivial) destructor.
        // SAFETY: same slot; Phase A's borrow has ended at the block above.
        unsafe {
            core::ptr::drop_in_place(app_ptr);
            app_ptr.write(App::new(AppState::new(
                CAM_LON,
                CAM_LAT,
                zoom_for_mpp(MPP_PRESETS[0].1),
            )));
        }
        let app = unsafe { &mut *app_ptr };
        let mut stub = StubLocation(Fix::at(CAM_LAT, CAM_LON));
        let mut fb = Framebuffer565::new(fb_buf, W as u32, H as u32);
        defmt::info!("Map open on baked tile; cycling zoom 0.5-4 mpp");

        // Per-frame: park the camera (Follow snaps to the stub fix), pick the next
        // zoom preset, render into the live framebuffer, and log the render time so
        // it can be sanity-checked against the internal-RAM mcu-render-bench.
        let mut frame = 0usize;
        loop {
            let (label, mpp) = MPP_PRESETS[frame % MPP_PRESETS.len()];
            app.state.zoom = zoom_for_mpp(mpp);
            let now_ms = Instant::now().as_millis() as u32;
            app.tick(
                RideClock(now_ms),
                Sensors { loc: &mut stub, altimeter: None, track: None },
                None,
            );

            let t0 = Instant::now();
            let stats = app.render_frame(&mut fb, &reader, None, W as f32, H as f32, color_fn);
            let render_us = t0.elapsed().as_micros();

            defmt::info!(
                "map {=str}: render {=u64} us | lod {=usize} | feat {=usize}/{=usize} | chunks {=usize}",
                label,
                render_us,
                stats.lod,
                stats.features_drawn,
                stats.features_tried,
                stats.chunks_visited
            );
            frame += 1;
            Timer::after_millis(1500).await;
        }
    }
}
