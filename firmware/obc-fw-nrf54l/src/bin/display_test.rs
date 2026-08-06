//! **LS021 wiring checker** — the smallest possible binary that exercises the *real* display
//! path end to end: pin claims on the **rehomed** source bus (issue #1158), the real FLPR blob +
//! launch, the real `Ls021Flpr` presenter, and the real `com_task` — but no SD, no sensors, no
//! BLE, no app. It cycles a solid full-screen colour every second (black → red → green → blue →
//! white → colour bars), logging each step over RTT, so a black panel can be bisected:
//!
//! - RTT shows `FLPR alive` + colour steps but the glass stays black → wiring (or panel power):
//!   probe BCK/data on P2, the gate run on P1.10–14, COM on P1.22–24, and the 5 V/3.3 V rails.
//! - No `FLPR alive` → the blob/carve (memory-map drift) — a firmware problem, not wiring.
//! - Colours show here but the app stays black → normally an app-side bug (come back with RTT from
//!   the full build) — **except in the #1158 window**: until the storage-pivot integration PR
//!   merges, `main.rs` still claims the display data on the OLD pads (`P2.00–05`) plus SD-SPI, so
//!   the app build simply does not drive the rehomed harness. On that harness a black app screen is
//!   expected, not a bug. This checker is the only build on the new map until then.
//!
//! Flash with `cargo run --release --bin display_test` (rides the default feature set; the tiny
//! image flashes much faster than the app). LED1 heartbeats once per colour step; LED0 shimmers
//! at 60 Hz once COM starts (its pin carries VCOM) — that shimmer alone proves the COM task runs.
//!
//! Probing the drive signals (each frame push is a ~45 ms burst, once per second):
//! INTB P1.13 goes HIGH for the whole burst (the easiest scope trigger), GSP P1.10 pulses once
//! per frame, GCK P1.11 clocks 640 sub-lines, BSP P1.14 pulses per sub-line, BCK P2.07 runs the
//! ~0.75 MHz shift clock, data on P2.06/.08/.09/.10 + P2.00/.04 (the rehomed bus, issue #1158).
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
// The display backend reaches the FLPR through the mode mux since the storage pivot (#1158) — every
// push asks it for the coprocessor. Pulled in for that one seam; this checker never brings storage
// up, so the mux simply records that the display owns the hart and the sEMMC side stays dormant
// (its image is never copied into the carve and the card pads are only ever parked as inputs).
#[allow(dead_code)]
#[path = "../flpr_mux.rs"]
mod flpr_mux;
#[allow(dead_code)]
#[path = "../semmc.rs"]
mod semmc;

use defmt::{info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_time::Timer;
// The critical-section impl comes from linking nrf-mpsl (the default `ble` feature set) — MPSL is
// never initialised here; its cs impl works from reset, exactly as in the main app.
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

    // Pin claims — the FLPR only toggles OUT bits, the M33 owns direction/drive, so every line
    // must be configured here before launch. (`main.rs` still claims the display data on the old
    // pads plus SD-SPI; the storage-pivot integration PR rewires it and deletes the SPI path in
    // one step — until then this checker is the only build on the rehomed map.)
    // Gate + frame lines: the contiguous P1.10–14 run.
    let _gate_bus = [
        Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // GSP
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GCK
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
        Output::new(p.P1_13, Level::Low, OutputDrive::Standard), // INTB
    ];
    // Source bus on the rehomed map (issue #1158 — the sEMMC storage pivot gave the six fixed card
    // pads P2.00–05 to the card, so the display data moved onto the four pins the retired SD-SPI
    // path freed plus the two pads time-shared with sEMMC D3/D1). Matches `flpr_scan.c`'s
    // `DATA_MASK 0x751` and `obc_display::ls021::wire`.
    let _src_bus = [
        Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
        Output::new(p.P2_07, Level::Low, OutputDrive::Standard), // BCK (unchanged)
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // R0 (was SD-SPI SCK)
        Output::new(p.P2_08, Level::Low, OutputDrive::Standard), // R1 (was SD-SPI MOSI)
        Output::new(p.P2_09, Level::Low, OutputDrive::Standard), // G0 (was SD-SPI MISO)
        Output::new(p.P2_10, Level::Low, OutputDrive::Standard), // G1 (was SD-SPI CS)
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // B0 (shared: sEMMC D3)
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B1 (shared: sEMMC D1)
    ];
    // Park the four card-only sEMMC pads as inputs: the card breakout's pull-ups hold CLK/CMD/D0/D2
    // high = an idle SD bus, and with no clock edges the card stays inert while we drive the panel.
    //
    // ⚠️ **Deliberately different from `main.rs`**, which forbids exactly this: these four pads are
    // also owned by `semmc::configure_display_pads` (linked in via the `flpr_mux`/`semmc` modules
    // above), so claiming them here as embassy `Input`s is the double ownership main.rs's comment
    // rules out. It is harmless *only* because both owners want the identical end state — high-Z
    // input, no pull — and this bench never enters storage mode, so `configure_storage_pads` never
    // runs and the two can't diverge. Do not copy the pattern into the app.
    let _sd_parked = [
        Input::new(p.P2_01, Pull::None), // sEMMC CLK
        Input::new(p.P2_02, Pull::None), // sEMMC D0
        Input::new(p.P2_03, Pull::None), // sEMMC D2
        Input::new(p.P2_05, Pull::None), // sEMMC CMD
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
