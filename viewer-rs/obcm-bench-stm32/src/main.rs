//! STM32F429 render-timing bench — a near-direct nRF54L proxy (Cortex-M4F with a
//! hardware FPU, same class as the nRF54L's M33). Runs the SAME render + decode/draw
//! split as `obcm-bench-rp2040` (both with `small-scratch`), so comparing the two
//! boards isolates the FPU. Output is defmt over RTT via the onboard ST-LINK
//! (`cargo run` → probe-rs flashes + streams).
//!
//! Clock: 180 MHz (the F429's max) from the 16 MHz HSI, so we don't depend on the
//! DISC1's HSE source. The nRF54L runs ~128 MHz, so this slightly over-clocks the
//! proxy — normalize by clock when extrapolating.

#![no_std]
#![no_main]

use core::ptr::addr_of_mut;

use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_time::{Instant, Timer};
use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use obcm_reader::Reader;
use obcm_render::{MapRenderer, Viewport};
use panic_probe as _;

mod null_target;
use null_target::NullTarget;

// Same baked tile as the RP2040 bench → identical work. 1.6 MB fits the F429's 2 MB flash.
static TILE: &[u8] = include_bytes!("../../obcm-bench-rp2040/fixtures/fr_small.obcm");

// Renderer in .bss (SRAM); the stack lives in CCM (see memory.x), so the full
// 192 KB SRAM is available. `small-scratch` keeps it ~160 KB. `#[used]` pins it.
#[used]
static mut RENDERER: MapRenderer = MapRenderer::new_const();

const PANEL_W: f32 = 240.0;
const PANEL_H: f32 = 320.0;

// The *realistic* device zooms: finest-LOD panning tops out ~1.0 mpp and actual
// riding sits ~0.5 mpp — far more zoomed-in (a city block, not the whole town) than
// the old presets, so only a few hundred LOD2 features are in view.
const PRESETS: &[(&str, i32, i32, f32)] = &[
    ("pan_1mpp", 7_850_000, 47_995_000, 0.111_32), // 1.0 mpp — finest-LOD pan, ~240x320 m
    ("ride_0.5", 7_850_000, 47_995_000, 0.222_64), // 0.5 mpp — riding, ~120x160 m
];

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 180 MHz from HSI: 16 MHz / 8 = 2 MHz → ×180 = 360 MHz VCO → /2 = 180 MHz.
    let mut config = embassy_stm32::Config::default();
    {
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
        config.rcc.sys = Sysclk::PLL1_P;
        config.rcc.ahb_pre = AHBPrescaler::DIV1; // HCLK 180 MHz
        config.rcc.apb1_pre = APBPrescaler::DIV4; // PCLK1 45 MHz (max 45)
        config.rcc.apb2_pre = APBPrescaler::DIV2; // PCLK2 90 MHz (max 90)
    }
    let _p = embassy_stm32::init(config);

    defmt::info!(
        "obcm render bench: F429 @ 180 MHz (M4F/FPU), small-scratch, tile {=usize} bytes",
        TILE.len()
    );

    let reader = match Reader::new(TILE) {
        Ok(r) => r,
        Err(_) => loop {
            defmt::error!("tile invalid ({=usize} bytes)", TILE.len());
            Timer::after_secs(2).await;
        },
    };

    // SAFETY: single-core, single-task; nothing else touches RENDERER.
    let renderer: &mut MapRenderer = unsafe { &mut *addr_of_mut!(RENDERER) };
    let bg = Rgb565::from(RawU16::new(0xFFFF));
    let color_fn = |rgb565: u16| Rgb565::from(RawU16::new(rgb565));

    let mut cycle = 0u32;
    loop {
        for &(label, lon, lat, zoom) in PRESETS {
            let vp = Viewport::new(PANEL_W, PANEL_H, lon, lat, zoom);
            let mut target = NullTarget::new(PANEL_W as u32, PANEL_H as u32);

            // Decode-only (flash/RAM reads + varint, ~no float).
            let t_c = Instant::now();
            core::hint::black_box(renderer.collect_only(&reader, &vp));
            let decode_us = t_c.elapsed().as_micros();

            // Full frame; draw (the float-heavy part) is the remainder.
            let t_r = Instant::now();
            let stats = renderer.render(&mut target, &reader, &vp, bg, color_fn);
            let total_us = t_r.elapsed().as_micros();
            let draw_us = total_us.saturating_sub(decode_us);

            defmt::info!(
                "{=str} total {=u64} = decode {=u64} + draw {=u64} us | lod {=usize} | ch {=usize} | feat {=usize} | pts {=usize} | px {=u32}",
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
        }
        defmt::info!("---");
        cycle += 1;
        if cycle >= 3 {
            defmt::info!("done after {=u32} cycles", cycle);
            // Clean halt so `probe-rs run` exits and releases the ST-LINK on its own
            // (a hard SIGTERM mid-session leaves the probe in a stuck USB state).
            cortex_m::asm::bkpt();
            loop {
                cortex_m::asm::wfi();
            }
        }
        Timer::after_secs(1).await;
    }
}
