//! **LS021 bring-up L0** (issue #140, epic #139) — the bench test-signal firmware.
//!
//! Not the panel driver. It (1) holds the LS021B7DD02 in its datasheet **boot-safe state**
//! (every signal input driven `Lo`) so the rails + idle current can be metered, then on
//! **BTN0** (2) **walks a recognizable pulse across all 15 signal lines in a fixed order**,
//! one line at a time, so the **RP2040 logic analyzer** confirms the whole
//! **nRF DK → Pico → panel** wiring map: each line lights up in its own time-slot, so a
//! swap, an open, or a short is immediately visible on the capture. No gate/source frame,
//! no sustained COM drive. See `firmware/docs/ls021-bringup.md` for the harness map.
//!
//! **Panel-safe by construction:** all 15 pins boot `Output(Lo)`. The walk pulses **one line
//! at a time** for a few hundred ms — COM lines (`VCOM`/`VA`/`VB`) included, but only ever
//! briefly, so there is no sustained DC bias — with the other 14 held `Lo`. `BCK` is pulsed
//! at its real ~0.75 MHz rate in its slot (LA-calibrated, below); the rest blink at ~12 Hz
//! (the analyzer only needs to see *which* channel is active *when*).
//!
//! Walk order = the Pico channel order **D2..D16** (the harness map in the spec doc):
//!   `GSP GCK GEN INTB VB VA BSP BCK R0 R1 G0 G1 B0 B1 VCOM`
//!
//! Build/flash (the bin only compiles with its feature):
//! ```sh
//! cargo run --release --bin ls021_bringup --features ls021-bringup
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

// BCK ~0.75 MHz busy-toggle calibration (M33 @128 MHz). Calibrated on the RP2040 LA
// (n=64->208 kHz, n=16->545 kHz): half-period ≈ 54 + 3.96·n cyc; n=8 → ~85 cyc → ~0.75 MHz.
const BCK_HALF_DELAY_CYC: u32 = 8;
// ~130 ms of BCK at ~0.75 MHz (full period ~1.33 µs) — enough for the LA to see its slot.
const BCK_BURST_PERIODS: u32 = 100_000;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Run the M33 at its full 128 MHz (embassy default is 64 MHz), so the BCK busy-loop
    // calibration above holds.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };

    // The 15 panel signal lines, in the walk / Pico-channel order (index i → Pico D(2+i)).
    // Every one boots `Lo` = the datasheet boot-safe state. DK pins are the L0 map in
    // `firmware/docs/ls021-bringup.md`; `P1_05` (host-driven J-Link VCOM RX) is deliberately
    // left alone.
    let mut pins = [
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), //  0 GSP  → D2  (GP2)
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), //  1 GCK  → D3  (GP3)
        Output::new(p.P1_04, Level::Low, OutputDrive::Standard), //  2 GEN  → D4  (GP4)
        Output::new(p.P1_06, Level::Low, OutputDrive::Standard), //  3 INTB → D5  (GP5)
        Output::new(p.P2_08, Level::Low, OutputDrive::Standard), //  4 VB   → D6  (GP6)
        Output::new(p.P2_10, Level::Low, OutputDrive::Standard), //  5 VA   → D7  (GP7)
        Output::new(p.P1_07, Level::Low, OutputDrive::Standard), //  6 BSP  → D8  (GP8)
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), //  7 BCK  → D9  (GP9)
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), //  8 R0   → D10 (GP10)
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), //  9 R1   → D11 (GP11)
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // 10 G0   → D12 (GP12)
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // 11 G1   → D13 (GP13)
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // 12 B0   → D14 (GP14)
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // 13 B1   → D15 (GP15)
        Output::new(p.P2_07, Level::Low, OutputDrive::Standard), // 14 VCOM → D16 (GP16)
    ];
    const NAMES: [&str; 15] =
        ["GSP", "GCK", "GEN", "INTB", "VB", "VA", "BSP", "BCK", "R0", "R1", "G0", "G1", "B0", "B1", "VCOM"];
    const BCK_IDX: usize = 7;

    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard); // LED0 (active-HIGH)
    let btn = Input::new(p.P1_13, Pull::Up); // BTN0 (active-LOW, internal pull-up)

    info!("LS021 L0: ALL-LO SAFE STATE — meter VDD1(3.3V)/VDD2(5V) + idle current; press BTN0 to start signal walk");

    // Quiescent hold: blink LED0 (~2 Hz) until BTN0 pressed. All 15 inputs stay Lo — the clean
    // window to meter rails + idle current. `is_high()` = not pressed (active-LOW button).
    while btn.is_high() {
        led.toggle();
        Timer::after_millis(250).await;
    }
    info!("LS021 L0: SIGNAL WALK — pulsing each line D2..D16 in turn; BCK~0.75MHz, others ~12Hz; rest held Lo");

    // Walk: pulse one line at a time, the other 14 held `Lo`, so the LA sees each DK→Pico
    // mapping light up in its own slot (and the panel never sees a sustained COM bias). Loops
    // forever — reset (re-flash / power-cycle) to stop.
    loop {
        for i in 0..pins.len() {
            info!("walk D{=usize}: {=str}", i + 2, NAMES[i]);
            if i == BCK_IDX {
                // ~0.75 MHz burst (~130 ms) — also re-demonstrates the real BCK rate on the LA.
                for _ in 0..BCK_BURST_PERIODS {
                    pins[i].set_high();
                    cortex_m::asm::delay(BCK_HALF_DELAY_CYC);
                    pins[i].set_low();
                    cortex_m::asm::delay(BCK_HALF_DELAY_CYC);
                }
            } else {
                // 3 blinks at ~12.5 Hz (~240 ms) — cleanly captured even at a 1 kHz LA sample rate.
                for _ in 0..3 {
                    pins[i].set_high();
                    Timer::after_millis(40).await;
                    pins[i].set_low();
                    Timer::after_millis(40).await;
                }
            }
            Timer::after_millis(60).await; // all-Lo gap delimits the slot
        }
        led.toggle(); // heartbeat once per full walk
    }
}
