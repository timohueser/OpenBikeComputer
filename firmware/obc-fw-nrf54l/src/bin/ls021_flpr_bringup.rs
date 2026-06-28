//! **LS021 FLPR bring-up bench bin** (epic #149) — drives the FLPR `Panel` backend through test
//! patterns on real glass. Since #165 the backend itself ([`Ls021Flpr`]) lives in the shared
//! `src/ls021_flpr.rs` module so the *real app* (`main.rs --features panel-ls021`) drives the same
//! LS021 panel through the same [`obc_platform::Panel`] seam; this bin keeps it exercised in
//! isolation — boot the FLPR, draw the glass-demo + line/box + solid cards through the seam, step
//! them with BTN0 — without the SD/sensor/app machinery.
//!
//! The FLPR scans the whole frame top-to-bottom in one `CMD_RUN_FRAME` (see the module doc): the
//! whole-frame generators that drive the ST7789 (`demo::font_palette_demo`, and the real
//! `App::render_frame`) put pixels on the LS021 with **no panel-specific code**.
//!
//! Power-on sequence (datasheet §6-2, mirroring the L3 bin):
//!   1. **Settle (~2 s)** — rails up, all inputs `Lo`, COM `Lo`. Hands-free for a deterministic LA
//!      capture at reset; LED1 blinks.
//!   2. **Launch the FLPR**, wait for its `ALIVE` stamp ([`launch_flpr`]).
//!   3. **Init #0 — `INTB`-framed all-black frame** through the `Panel` seam (a black `clear`). COM `Lo`.
//!   4. **Wait `T4 ≥ 30 µs`, then start COM** on a high-priority `InterruptExecutor` — runs forever.
//!   5. **BTN0 steps** the screen: GLASS-DEMO → line card → white → greys → black → wrap. Each is
//!      drawn through the `Panel`/`Band` seam and FLPR-driven once (MIP retains it).
//!
//! **Both cores on P2 at once.** The FLPR drives the source bus on `P2.00..06`; the M33 drives COM
//! on `P2.07/08/10` from `com_task`. Safe because every GPIO touch on either core is an atomic
//! `OUTSET`/`OUTCLR` of disjoint pin masks. The gate lines (`GSP`/`GCK`/`GEN`/`INTB`) + `BSP` are all
//! on **P1**, FLPR-driven — **relocated for the app integration** (issue #165, see the pin block in
//! `main` and the masks in `src/flpr/flpr_pingpong.c`).
//!
//! Build/flash (needs a RISC-V gcc for the blob — `brew install riscv64-elf-gcc`; and the
//! Board-Configurator ext-memory-off / 3.3 V-VDDM settings the `ls021_bringup` epic already needs):
//! ```sh
//! cargo run --release --bin ls021_flpr_bringup --features ls021-flpr
//! ```

#![no_std]
#![no_main]

use defmt::{error, info, warn};
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_time::Timer;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use {defmt_rtt as _, panic_probe as _};

// The free-running COM driver is panel-board-agnostic infrastructure (not the M33-direct PanelBus,
// which the FLPR replaces). Pull in `com_task`; the rest of the module is unused here (module-level
// allow). The glass-demo generator is shared verbatim with the ST7789 `--features glass-demo` build.
#[path = "../demo.rs"]
mod demo;
#[path = "../ls021.rs"]
#[allow(dead_code)]
mod ls021;
// The shared FLPR `Panel` backend (#165): boot/launch + the resident-framebuffer ping-pong push.
#[path = "../ls021_flpr.rs"]
mod ls021_flpr;
use demo::font_palette_demo;
use ls021::com_task;
use ls021_flpr::{launch_flpr, show, FlprError, Ls021Flpr, FB_H, FB_W};

/// Resident RGB222 (device-64) framebuffer, one byte per pixel — the production map plane's exact
/// type/size. `Ls021Flpr` owns it; `flush_band` fills it (the glass-demo's RGB565 → device-64
/// quantise), `push_frame` packs + drives it.
static mut FB: [u8; FB_W * FB_H] = [0u8; FB_W * FB_H];

/// One band's worth of RGB565 scratch the [`Panel`] seam hands the generator (`BAND_ROWS` full
/// `WIDTH`-pixel rows). The frame is resident in [`FB`]; this is only the transient per-band buffer
/// the generator draws into before `flush_band` quantises it into the plane, so it can be small.
const BAND_ROWS: usize = 16;
static mut BAND: [u16; FB_W * BAND_ROWS] = [0u16; FB_W * BAND_ROWS];

/// High-priority executor the COM driver runs on, pended from the unused SWI00 software-interrupt
/// vector. COM at P3 preempts thread-mode so it free-runs CPU-independently while the M33 busy-polls
/// the FLPR's per-frame ack.
static EXECUTOR_COM: InterruptExecutor = InterruptExecutor::new();

#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_COM.on_interrupt();
}

/// Wait for one clean BTN0 press: drain any still-held press (so a held button can't double-step),
/// then poll every ~5 ms for a debounced press edge. (Same as the L3 bin.)
async fn wait_for_press(btn: &Input<'_>) {
    while btn.is_low() {
        Timer::after_millis(5).await;
    }
    Timer::after_millis(20).await;
    loop {
        if btn.is_low() {
            Timer::after_millis(15).await;
            if btn.is_low() {
                return;
            }
        }
        Timer::after_millis(5).await;
    }
}

/// Clear `t` to a solid RGB565 colour — a whole-frame generator for the BTN0 solid cards (the clean
/// single-value waveforms the LA speed-tune reads). The device snaps each to its RGB222 gamut.
fn solid(t: &mut obc_platform::Band, c: Rgb565) {
    t.clear(c).ok();
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Match the rest of the nRF firmware: run the M33 at its full 128 MHz.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };

    // The M33 owns pin *configuration* for every line the FLPR drives; the FLPR only ever pulses
    // OUTSET/OUTCLR (atomic, never an OUT read-modify-write) so the two cores never collide on the
    // shared ports. Kept alive for the life of the program so the pins stay configured. All boot
    // `Output(Lo)` (the datasheet boot-safe state).
    //
    // ⚠️ **Gate/BSP pins relocated for the app integration (issue #165).** The bench map reused the
    // SD/VCOM pins ("safe this epic only"); the real app needs those, so the five P1 gate/BSP lines
    // moved to free P1 pins. **They must match `flpr_pingpong.c`'s masks and `main.rs`'s pins** —
    // confirm each is broken out on your DK and remap all three together if not.
    //   • Gate + frame (P1): GSP P1.00, GCK P1.01, GEN P1.15, INTB P1.16.
    //     (P1.02/03 are NFC pins on this part, GPIO only behind `nfc-pins-as-gpio` — avoided.)
    //   • Source bus: BSP P1.14 (P1) + BCK P2.06 + the 6 data lines P2.00..05 (P2).
    //   • LED0 P2.09: spare M33 marker.
    let _gate_bus = [
        Output::new(p.P1_00, Level::Low, OutputDrive::Standard), // GSP  (gate start pulse)
        Output::new(p.P1_01, Level::Low, OutputDrive::Standard), // GCK  (gate clock / area-plane select)
        Output::new(p.P1_15, Level::Low, OutputDrive::Standard), // GEN  (gate output enable)
        Output::new(p.P1_16, Level::Low, OutputDrive::Standard), // INTB (frame envelope)
    ];
    let _src_bus = [
        Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP  (the lone P1 source line)
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // BCK  (P2.06)
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0   (P2.00, odd)
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1   (P2.01, even)
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0   (P2.02, odd)
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1   (P2.03, even)
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0   (P2.04, odd)
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1   (P2.05, even)
    ];
    let _led0 = Output::new(p.P2_09, Level::Low, OutputDrive::Standard); // spare M33 marker

    // COM lines as high-drive GPIO (56–77 nF load each), boot `Lo` (safe state); held `Lo` through
    // the init frame, then moved into `com_task`. VCOM=P2.07, VB=P2.08, VA=P2.10 (M33-driven).
    let vcom = Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive);
    let vb = Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive);
    let va = Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive);

    // LED1 (P1.10) = the M33's heartbeat — proves the M33 keeps running alongside the FLPR.
    let mut led1 = Output::new(p.P1_10, Level::Low, OutputDrive::Standard);
    let btn0 = Input::new(p.P1_13, Pull::Up); // DK BTN0 — active-LOW (pressed = Lo); steps the screen

    info!("LS021 FLPR bring-up: launcher up");

    // 1. Settle window (~2 s, LED1 ~2 Hz): all inputs `Lo`, COM `Lo`. Meter window; hands-free so the
    //    bench LA capture at reset is deterministic.
    info!("LS021 FLPR: SETTLE (~2s, all inputs Lo, COM held Lo) — then FLPR boot, init-black, COM, glass-demo");
    for _ in 0..8 {
        led1.toggle();
        Timer::after_millis(250).await;
    }

    // 2. Arm the control block, launch the FLPR, poll for its ALIVE stamp.
    match launch_flpr().await {
        Ok(()) => info!("LS021 FLPR: alive — building the Panel backend, driving the init-black frame (COM still Lo)"),
        Err(FlprError::BadMagic) => {
            error!("LS021 FLPR: booted but control-block magic mismatched — memory-map drift? (ls021-flpr.md)");
            halt(&mut led1).await
        }
        Err(FlprError::NoBoot) => {
            warn!("LS021 FLPR: no alive stamp — FLPR didn't boot or can't reach shared RAM; halting (LED1 blink)");
            halt(&mut led1).await
        }
    }

    // Build the Panel backend over the resident framebuffer + the band scratch.
    // SAFETY: the sole references taken to FB/BAND; held by `panel` for the rest of the program and
    // this single-executor build never aliases them (COM/SWI touch neither).
    let mut panel = Ls021Flpr::new_banded(unsafe { &mut *core::ptr::addr_of_mut!(FB) }, unsafe {
        &mut *core::ptr::addr_of_mut!(BAND)
    });

    // 3. Init #0 — an INTB-framed all-black frame, FLPR-driven through the Panel seam. COM not yet up.
    led1.set_high(); // LED1 steady-on marks the init frame on the bench
    info!("LS021 FLPR: SHOW INIT-BLACK");
    show(&mut panel, |t| solid(t, Rgb565::BLACK));
    led1.set_low();

    // 4. T4 ≥ 30 µs, then start COM on the high-priority interrupt executor (P3). From here COM
    //    free-runs forever — VCOM≡VB in phase, VA inverse, ~60 Hz / 50 %.
    Timer::after_micros(50).await;
    interrupt::SWI00.set_priority(Priority::P3);
    let com_spawner = EXECUTOR_COM.start(interrupt::SWI00);
    com_spawner.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
    info!("LS021 FLPR: COM RUNNING — BTN0 steps GLASS-DEMO → LINE-BLACK → L3-WHITE → L2-GRAY → L1-GRAY → L0-BLACK");

    // 5. BTN0 steps the screen through the Panel seam: the glass-demo (font ladder + 64-colour
    //    gamut, identical to the ST7789 `--features glass-demo` build), the line/box diagnostic card,
    //    then solids. MIP retains each.
    let gray_l2 = Rgb565::new(21, 42, 21); // → device (2,2,2)  (MSB plane, 2/3-area)
    let gray_l1 = Rgb565::new(10, 21, 10); // → device (1,1,1)  (LSB plane, 1/3-area)
    let mut i = 0usize;
    loop {
        match i {
            0 => {
                info!("LS021 FLPR: SHOW GLASS-DEMO");
                show(&mut panel, |t| {
                    font_palette_demo(t).ok();
                });
            }
            1 => {
                info!("LS021 FLPR: SHOW LINE-BLACK");
                show(&mut panel, |t| {
                    demo::line_test_card(t, Rgb565::BLACK).ok();
                });
            }
            2 => {
                info!("LS021 FLPR: SHOW L3-WHITE");
                show(&mut panel, |t| solid(t, Rgb565::WHITE));
            }
            3 => {
                info!("LS021 FLPR: SHOW L2-GRAY (MSB plane)");
                show(&mut panel, |t| solid(t, gray_l2));
            }
            4 => {
                info!("LS021 FLPR: SHOW L1-GRAY (LSB plane)");
                show(&mut panel, |t| solid(t, gray_l1));
            }
            _ => {
                info!("LS021 FLPR: SHOW L0-BLACK");
                show(&mut panel, |t| solid(t, Rgb565::BLACK));
            }
        }
        wait_for_press(&btn0).await;
        led1.toggle();
        i = (i + 1) % 6;
    }
}

/// Blink LED1 forever — an unrecoverable FLPR-launch failure idles here rather than driving an
/// un-launched FLPR (a bad-hardware path must never fault). Diverges.
async fn halt(led1: &mut Output<'static>) -> ! {
    loop {
        led1.toggle();
        Timer::after_millis(500).await;
    }
}
