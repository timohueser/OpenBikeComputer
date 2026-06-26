//! **LS021 bring-up L0** (issue #140, epic #139) — the bench test-signal firmware.
//!
//! This is *not* the panel driver. Its only jobs are (1) hold the LS021B7DD02 in its
//! datasheet **boot-safe state** (every signal input driven `Lo`) so the rails + idle
//! current can be metered cleanly, and (2) emit two known square waves so the **RP2040
//! logic analyzer** (sigrok-pico + `sigrok-cli`) can be validated before it's trusted for
//! L1–L4. No pixels, no gate/source scan, no COM drive. See
//! `firmware/docs/ls021-bringup.md` for the full pin map + protocol spec.
//!
//! **Panel-safe by construction.** All 15 panel signal pins boot `Output(Lo)`. The two
//! test signals are emitted on **logic lines only** — `BCK` (P2.06, ~0.75 MHz) and `GSP`
//! (P1.11, ~60 Hz) — with everything else held `Lo`. **COM (`VCOM`/`VA`/`VB`) is never
//! toggled here**, so there is no DC bias across the liquid crystal and no pixel latch
//! even with the panel plugged in. (Free-running COM is L1's job, #141.)
//!
//! Flow: boot → drive all-`Lo` → blink `LED0` + wait for **`BTN0`** (the quiescent
//! rails/idle-current window) → on press, run the cycle-counted LA test loop forever.
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

// --- LA test-signal tuning (M33 @ 128 MHz → 1 cycle ≈ 7.81 ns) ---
// `BCK` ~0.75 MHz → ~1333 ns period → ~666 ns (~85 cyc) per half-period. Calibrated against
// the RP2040 logic analyzer (two points: n=64→208 kHz, n=16→545 kHz) the per-half-period cost
// is ≈ 54 + 3.96·n cycles (`asm::delay(n)` ≈ 3.96 cyc/count on this M33 @128 MHz, ~54 cyc fixed
// for the GPIO writes + loop). n = 8 → ~85 cyc → ~0.75 MHz; GSP then follows at ~60 Hz via the
// fixed divider below.
const BCK_HALF_DELAY_CYC: u32 = 8;
// One loop iteration = one full `BCK` period (~1.333 µs). `GSP` ~60 Hz → 8.33 ms
// half-period → ~6250 `BCK` periods between `GSP` edges. `LED0` ~0.5 s heartbeat →
// ~375000 `BCK` periods between toggles.
const GSP_TOGGLE_EVERY: u32 = 6_250;
const LED_TOGGLE_EVERY: u32 = 375_000;

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Run the M33 at its full 128 MHz, like `main.rs` — embassy-nrf's `Config::default()`
    // boots it at 64 MHz, which would halve every cycle-counted delay below.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };

    // === Boot-safe state: every panel signal input driven Lo (datasheet "Boot"). ===
    // The two toggled logic lines (BCK, GSP) start Lo here too. The 13 held-Lo pins are
    // bound to live locals so embassy keeps them driven for the whole program (a dropped
    // `Output` releases the pin) — hence the `_name` bindings, not `_`.
    // Source data (odd R0/G0/B0, even R1/G1/B1) on the freed ext-flash P2 bus:
    let _r0 = Output::new(p.P2_00, Level::Low, OutputDrive::Standard);
    let _r1 = Output::new(p.P2_01, Level::Low, OutputDrive::Standard);
    let _g0 = Output::new(p.P2_02, Level::Low, OutputDrive::Standard);
    let _g1 = Output::new(p.P2_03, Level::Low, OutputDrive::Standard);
    let _b0 = Output::new(p.P2_04, Level::Low, OutputDrive::Standard);
    let _b1 = Output::new(p.P2_05, Level::Low, OutputDrive::Standard);
    // COM lines — held Lo at L0 (no DC bias). Drive strength for the 56–77 nF load is an
    // L1 question; Standard is fine while they sit Lo.
    let _vcom = Output::new(p.P2_07, Level::Low, OutputDrive::Standard);
    let _vb = Output::new(p.P2_08, Level::Low, OutputDrive::Standard);
    let _va = Output::new(p.P2_10, Level::Low, OutputDrive::Standard);
    // Gate/control logic lines (GCK/GEN/INTB/BSP held Lo; GSP toggled below).
    let _gck = Output::new(p.P1_12, Level::Low, OutputDrive::Standard);
    let _gen = Output::new(p.P1_04, Level::Low, OutputDrive::Standard);
    let _intb = Output::new(p.P1_06, Level::Low, OutputDrive::Standard);
    let _bsp = Output::new(p.P1_07, Level::Low, OutputDrive::Standard);

    // The two toggled test pins + the heartbeat LED + the start button.
    let mut bck = Output::new(p.P2_06, Level::Low, OutputDrive::Standard);
    let mut gsp = Output::new(p.P1_11, Level::Low, OutputDrive::Standard);
    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard); // LED0 (active-HIGH)
    let btn = Input::new(p.P1_13, Pull::Up); // BTN0 (active-LOW, internal pull-up)

    info!("LS021 L0: ALL-LO SAFE STATE — meter VDD1(3.3V)/VDD2(5V) + idle current; press BTN0 to start LA test");

    // === Quiescent hold: blink LED0 (~2 Hz) until BTN0 is pressed. ===
    // All inputs stay Lo here — the window to read rails + idle current against the
    // datasheet. `is_high()` = not pressed (active-LOW button).
    while btn.is_high() {
        led.toggle();
        Timer::after_millis(250).await;
    }

    info!(
        "LS021 L0: LA TEST — BCK(P2.06)~0.75MHz + GSP(P1.11)~60Hz; all other panel pins Lo (no COM, no bias)"
    );
    led.set_low();

    // === LA test loop: one cycle-counted bit-bang, forever. ===
    // BCK is a half-period square; GSP and LED0 are derived by counting BCK periods. No
    // `.await` — this is a deterministic busy-loop so the edges stay jitter-free for the
    // capture. Frequencies are approximate; the analyzer measures the truth.
    let mut gsp_cnt: u32 = GSP_TOGGLE_EVERY;
    let mut led_cnt: u32 = LED_TOGGLE_EVERY;
    loop {
        bck.set_high();
        cortex_m::asm::delay(BCK_HALF_DELAY_CYC);
        bck.set_low();
        cortex_m::asm::delay(BCK_HALF_DELAY_CYC);

        gsp_cnt -= 1;
        if gsp_cnt == 0 {
            gsp.toggle();
            gsp_cnt = GSP_TOGGLE_EVERY;
        }
        led_cnt -= 1;
        if led_cnt == 0 {
            led.toggle();
            led_cnt = LED_TOGGLE_EVERY;
        }
    }
}
