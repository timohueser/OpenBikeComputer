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
use obc_reader::Reader;
use obc_render::{zoom_for_mpp, MapRenderer, Viewport};
use panic_probe as _;

mod null_target;
use null_target::NullTarget;

// User-supplied OBCM v5 tile centred on Teningen (48.1223 N, 7.8142 E), with well-defined
// LODs. Baked into flash via include_bytes! — must be OBCM v5 and fit the F429's 2 MB flash
// alongside code (keep it <= ~1.7 MB; the old fr_small tile was 1.6 MB).
static TILE: &[u8] = include_bytes!("../../obcm-bench-rp2040/fixtures/teningen.obcm");

// Renderer in .bss (SRAM); the stack lives in CCM (see memory.x), so the full
// 192 KB SRAM is available. `small-scratch` keeps it ~160 KB. `#[used]` pins it.
#[used]
static mut RENDERER: MapRenderer = MapRenderer::new_const();

const PANEL_W: f32 = 240.0;
const PANEL_H: f32 = 320.0;

// Camera centre: the dense core of Teningen (48.1265 N, 7.8136 E) in microdegrees —
// the median of every building vertex in the tile, so the fine zooms land in the
// busiest part of town (heaviest feature/line density), not a quiet residential block.
const CAM_LON: i32 = 7_813_599; // 7.81360 E
const CAM_LAT: i32 = 48_126_492; // 48.12649 N

// The benchmarked zooms, in ground metres-per-pixel. zoom_for_mpp() maps each to the
// Viewport's pixels-per-microdegree. The LOD per zoom is chosen from the tile's own
// table (select_lod_for_mpp) — with this stylesheet's breakpoints (2/4/10/20 mpp) the
// six zooms walk the whole pyramid, finest to coarsest:
//   0.5, 1.0 -> LOD4 | 3.0 -> LOD3 | 5.0 -> LOD2 | 12 -> LOD1 | 22 -> LOD0
// (At 22 mpp the ~7 km frame slightly overruns the ~9 km tile's nearer edge — a thin
//  empty margin — but the full LOD0 feature set still renders.)
const MPP_PRESETS: &[(&str, f32)] = &[
    ("ride_0.5", 0.5),
    ("pan_1.0", 1.0),
    ("z3.0", 3.0),
    ("z5.0", 5.0),
    ("z12", 12.0),
    ("z22", 22.0),
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
        for &(label, mpp) in MPP_PRESETS {
            let vp = Viewport::new(PANEL_W, PANEL_H, CAM_LON, CAM_LAT, zoom_for_mpp(mpp));
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
