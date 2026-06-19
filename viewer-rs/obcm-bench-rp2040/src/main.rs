//! Headless RP2040 render-timing harness.
//!
//! Times `MapRenderer::render` (the base-map path) into a counting null target at
//! a few representative zooms, and streams the numbers over USB-CDC serial. No SD
//! card, no display, no input — a baked-in tile is read straight from XIP flash.
//!
//! Read the output on a Mac:  `ls /dev/tty.usbmodem*` then `tio /dev/tty.usbmodem*`.
//!
//! Caveat baked into every number: the RP2040 is a Cortex-M0+ with NO FPU, so the
//! f32-heavy render path is software-emulated. Treat results as a conservative
//! floor — the nRF54L (M33 + FPU) will be materially faster on the float work.

#![no_std]
#![no_main]

use core::ptr::addr_of_mut;

use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::{Instant, Timer};
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use obcm_reader::Reader;
use obcm_render::{MapRenderer, Viewport};
use panic_halt as _;

mod null_target;
use null_target::NullTarget;

// The 256-byte second-stage bootloader is provided by `embassy-rp` itself (it
// emits a `.boot2` static when the `rp2040` feature is on and `boot2-none` is
// off); our memory.x maps the `.boot2` section into the first flash page.

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

// The baked-in test tile, read in place from flash. Replace this file with the
// real cropped Freiburg tile (see the plan's "Tile prep"); the placeholder just
// lets the crate compile + link so the build/RAM gate can run before then.
static TILE: &[u8] = include_bytes!("../fixtures/fr_small.obcm");

// The ~199 KB renderer lives in .bss as a static — far too big for the stack.
// `new_const` is a const fn so there's no large temporary at construction.
// `#[used]` pins it even when the optimizer can prove the render path is
// unreachable (e.g. with a placeholder tile that fails `Reader::new`), so the
// real ~199 KB footprint is always linked and the RAM-fit gate stays honest.
#[used]
static mut RENDERER: MapRenderer = MapRenderer::new_const();

const PANEL_W: f32 = 240.0;
const PANEL_H: f32 = 320.0;

/// (label, cam_lon µdeg, cam_lat µdeg, zoom). mpp ≈ 0.111320 / zoom. The baked
/// tile covers ~5.2 x 3.8 km of central Freiburg (lon 7.821..7.891, lat
/// 47.978..48.012), so these are tuned to *fill* it at each zoom — an empty view
/// renders misleadingly fast. The printed `lod=` shows which LOD was selected.
const PRESETS: &[(&str, i32, i32, f32)] = &[
    ("wide", 7_850_000, 47_995_000, 0.0050),   // ~mpp 22  — whole tile (LOD1)
    ("town", 7_850_000, 47_995_000, 0.0093),   // ~mpp 12  — LOD2
    ("riding", 7_850_000, 47_995_000, 0.0186), // ~mpp 6   — device riding zoom (LOD2)
    ("close", 7_850_000, 47_995_000, 0.0398),  // ~mpp 2.8 — zoomed in (LOD2)
];

#[embassy_executor::task]
async fn logger_task(driver: Driver<'static, USB>) {
    embassy_usb_logger::run!(2048, log::LevelFilter::Info, driver);
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Onboard LED (Pico = GPIO25): a no-debugger heartbeat.
    //   3 slow blinks at startup = init + USB-logger spawn OK;
    //   then one toggle per render    = alive and looping;
    //   fast continuous blink         = spawn failed (task arena too small);
    //   no blink at all               = panic/hardfault before here, or non-Pico LED.
    // (Pico W has no GPIO25 LED — the heartbeat just won't show there.)
    let mut led = Output::new(p.PIN_25, Level::Low);

    let driver = Driver::new(p.USB, Irqs);
    if spawner.spawn(logger_task(driver)).is_err() {
        loop {
            led.toggle();
            Timer::after_millis(80).await;
        }
    }

    // Startup marker + ~1.5 s for the host to enumerate the CDC port.
    for _ in 0..3 {
        led.set_high();
        Timer::after_millis(120).await;
        led.set_low();
        Timer::after_millis(380).await;
    }

    let reader = match Reader::new(TILE) {
        Ok(r) => r,
        Err(_) => loop {
            log::error!("tile invalid ({} bytes) — rebake fixtures/fr_small.obcm", TILE.len());
            led.toggle(); // ~3 Hz blink = bad tile
            Timer::after_millis(150).await;
        },
    };

    // SAFETY: single-core, single-task; nothing else touches RENDERER.
    let renderer: &mut MapRenderer = unsafe { &mut *addr_of_mut!(RENDERER) };

    let bg = Rgb565::from(RawU16::new(0xFFFF)); // paper white; irrelevant to timing
    let color_fn = |rgb565: u16| Rgb565::from(RawU16::new(rgb565));

    log::info!("obcm render bench: panel {}x{}, tile {} bytes", PANEL_W as u32, PANEL_H as u32, TILE.len());

    loop {
        for &(label, lon, lat, zoom) in PRESETS {
            let vp = Viewport::new(PANEL_W, PANEL_H, lon, lat, zoom);
            let mut target = NullTarget::new(PANEL_W as u32, PANEL_H as u32);

            // (1) Decode-only: flash reads + varint, integer-heavy, ~no float.
            //     black_box stops LLVM from eliding the unused result.
            let t_c = Instant::now();
            core::hint::black_box(renderer.collect_only(&reader, &vp));
            let decode_us = t_c.elapsed().as_micros();

            // (2) Full frame: collect + project + rasterize. The float-heavy draw
            //     work is the remainder (total - decode).
            let t_r = Instant::now();
            let stats = renderer.render(&mut target, &reader, &vp, bg, color_fn);
            let total_us = t_r.elapsed().as_micros();
            let draw_us = total_us.saturating_sub(decode_us);

            log::info!(
                "{:<7} total {:>7} = decode {:>6} + draw {:>7} us | lod {} | ch {:>3} | feat {:>4} | pts {:>5} | px {:>6}",
                label,
                total_us,
                decode_us,
                draw_us,
                stats.lod,
                stats.chunks_visited,
                stats.features_drawn,
                stats.points_drawn,
                target.pixels,
            );

            led.toggle(); // heartbeat
            // Yield so the executor can service USB + drain the log buffer between
            // (long, executor-blocking soft-float) renders.
            Timer::after_millis(250).await;
        }
        log::info!("---");
        Timer::after_secs(1).await;
    }
}
