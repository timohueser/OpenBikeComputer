//! **LS021 wiring checker** — the smallest possible binary that exercises the *real* display
//! path end to end: pin claims exactly as the app (`main.rs`), the real FLPR blob + launch, the
//! real `Ls021Flpr` presenter, and the real `com_task` — but no SD, no sensors, no BLE, no app.
//! It cycles a solid full-screen colour every second (black → red → green → blue → white →
//! colour bars), logging each step over RTT, so a black panel can be bisected:
//!
//! - RTT shows `FLPR alive` + colour steps but the glass stays black → wiring (or panel power):
//!   probe BCK/data on P2, the gate run on P1.10–14, COM on P1.22–24, and the 5 V/3.3 V rails.
//! - No `FLPR alive` → the blob/carve (memory-map drift) — a firmware problem, not wiring.
//! - Colours show here but the app stays black → an app-side bug; come back with RTT from the
//!   full build.
//!
//! Flash with `cargo run --release --bin display_test` (rides the default feature set; the tiny
//! image flashes much faster than the app). LED1 heartbeats once per colour step; LED0 shimmers
//! at 60 Hz once COM starts (its pin carries VCOM) — that shimmer alone proves the COM task runs.
//!
//! Probing the drive signals (each frame push is a ~45 ms burst, once per second):
//! INTB P1.13 goes HIGH for the whole burst (the easiest scope trigger), GSP P1.10 pulses once
//! per frame, GCK P1.11 clocks 640 sub-lines, BSP P1.14 pulses per sub-line, BCK P2.07 runs the
//! ~0.75 MHz shift clock, data on P2.00–05.
#![no_std]
#![no_main]

// The real driver modules, pulled in by path (this is a `bin`, not a lib consumer). A checker
// this small exercises only the panel-bring-up subset of each, so the items the app uses and it
// doesn't — `Ls021Flpr::reset_diff`, the carve constants the budget assert reads — are dead here
// by construction, not by accident.
#[allow(dead_code)]
#[path = "../com.rs"]
mod com;
#[allow(dead_code)]
#[path = "../ls021_flpr.rs"]
mod ls021_flpr;

use defmt::{info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_time::Timer;
// The critical-section impl comes from linking nrf-mpsl (the default `ble` feature set) — MPSL is
// never initialised here; its cs impl works from reset, exactly as in `sd_bench`.
use nrf_mpsl as _;
use panic_probe as _;

use ls021_flpr::{launch_flpr, relaunch_flpr, Frame64, Ls021Flpr, FB_H, FB_W};
use obc_display::ls021::RowDiff;

/// The resident device-64 framebuffer the FLPR scans (1 byte/pixel, RGB222 in bits 0..=5).
static mut FB: [u8; FB_W * FB_H] = [0; FB_W * FB_H];
static mut ROW_DIFF: RowDiff<FB_H> = RowDiff::new();

/// The colour cycle: device-64 bytes (R = bits 0–1, G = bits 2–3, B = bits 4–5).
const CYCLE: [(u8, &str); 5] = [(0x00, "black"), (0x03, "red"), (0x0C, "green"), (0x30, "blue"), (0x3F, "white")];

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };
    info!("display_test: LS021 wiring checker ({=str}+{=str})", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT"));

    // LED1 heartbeat (one blink per colour step). LED0's pin carries VCOM below.
    let mut led = Output::new(p.P1_25, Level::Low, OutputDrive::Standard);

    // Pin claims — byte-for-byte the app's map (`main.rs`): the FLPR only toggles OUT bits, the
    // M33 owns direction/drive, so every line must be configured here before launch.
    // Gate + frame lines: the contiguous P1.10–14 run.
    let _gate_bus = [
        Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // GSP
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GCK
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
        Output::new(p.P1_13, Level::Low, OutputDrive::Standard), // INTB
    ];
    // Source bus: BSP on P1.14, BCK + the 6 data lines on P2 (P2.06 stays the SD's — untouched).
    let _src_bus = [
        Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
        Output::new(p.P2_07, Level::Low, OutputDrive::Standard), // BCK
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0 (odd)
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1 (even)
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1
    ];
    // COM electrodes, boot `Lo`, held `Lo` through the init-black frame, then free-running.
    let (vcom, vb, va) = (
        Output::new(p.P1_22, Level::Low, OutputDrive::HighDrive),
        Output::new(p.P1_23, Level::Low, OutputDrive::HighDrive),
        Output::new(p.P1_24, Level::Low, OutputDrive::HighDrive),
    );

    // Launch the FLPR (blob copy + control block + ALIVE wait), one relaunch retry as in the app.
    let launched = match launch_flpr().await {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!("display_test: FLPR launch failed ({}) — one relaunch retry", e);
            relaunch_flpr().await
        }
    };
    if let Err(e) = launched {
        defmt::error!("display_test: FLPR did not come up ({}) — memory-map/firmware problem, NOT wiring", e);
        loop {
            led.toggle();
            Timer::after_millis(100).await; // fast angry blink
        }
    }
    info!("display_test: FLPR alive — pushing init-black, then starting COM");

    // SAFETY: sole references to FB / ROW_DIFF for the program's life.
    let fb: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
    let diff: &'static mut RowDiff<FB_H> = unsafe { &mut *core::ptr::addr_of_mut!(ROW_DIFF) };
    let mut frame = Frame64::new(fb);
    let mut panel = Ls021Flpr::new(diff);

    // Datasheet Initial #0: INTB-framed all-black frame with COM still `Lo`, T4 ≥ 30 µs, then COM.
    panel.push_frame(&frame).await;
    Timer::after_micros(50).await;
    spawner.spawn(defmt::unwrap!(com::com_task(vcom, vb, va)));
    info!("display_test: COM free-running (watch LED0 shimmer) — colour cycle starts");

    let mut step = 0usize;
    loop {
        let full_frame = step % (CYCLE.len() + 1);
        if full_frame < CYCLE.len() {
            let (byte, name) = CYCLE[full_frame];
            frame.bytes_mut().fill(byte);
            info!("display_test: frame {=usize} — solid {=str} (0x{=u8:02x})", step, name, byte);
        } else {
            // Colour bars: 6 vertical bands (R,G,B,yellow-ish,cyan-ish,white) — one frame that
            // shows every data line and the odd/even column split at once.
            const BANDS: [u8; 6] = [0x03, 0x0C, 0x30, 0x0F, 0x3C, 0x3F];
            for row in frame.bytes_mut().chunks_exact_mut(FB_W) {
                for (x, px) in row.iter_mut().enumerate() {
                    *px = BANDS[x * BANDS.len() / FB_W];
                }
            }
            info!("display_test: frame {=usize} — colour bars", step);
        }
        if !panel.push_frame(&frame).await {
            warn!("display_test: present stalled (FLPR ack timeout)");
        }
        led.toggle();
        step += 1;
        Timer::after_millis(1000).await;
    }
}
