//! **LS021 bring-up L2** (issue #142, epic #139) — power-on init → **uniform black** on glass.
//!
//! The first **controlled pixel state**. It runs the datasheet power-on sequence whose
//! mandatory init step writes the whole screen black, exercising the gate scan
//! (`GSP`/`GCK`/`GEN`) and the source shift (`BSP`/`BCK` + the 6 data lines) for the first
//! time. "Black" here is the proof, not the goal: an MIP panel powers up with *retained /
//! undefined* pixels (not black), so a genuinely **uniform** black field means every gate
//! line addressed and every column shifted — a missed row/column would leave a garbage
//! streak. The rigorous timing proof is the logic analyzer; the webcam confirms the glass.
//! The gate/source primitives live in [`ls021::PanelBus`]; see `firmware/docs/ls021-bringup.md`.
//!
//! **Panel-safe by construction.** All 15 signal lines boot `Output(Lo)` (the datasheet boot
//! state). COM (`VCOM`/`VB`/`VA`) is held `Lo` for the *entire* init frame — the datasheet
//! "COM held `Lo` during init" requirement — and only starts (free-running forever) **after**
//! the frame + the `T4 ≥ 30 µs` wait. The init frame runs **once**; the M33 then idles while
//! COM toggles on the high-priority interrupt executor.
//!
//! Sequence (datasheet §6-2 / spec doc "Power-on"):
//!   1. **Settle (~2 s).** Rails up, all inputs `Lo`, COM `Lo` — the meter / safe-state window.
//!      Hands-free (no button gate) so a power-cycle gives a deterministic LA capture.
//!   2. **Init #0 — `INTB`-framed all-black frame** ([`PanelBus::init_black`]). COM still `Lo`.
//!   3. **Wait `T4 ≥ 30 µs`**, then **start COM** on the interrupt executor — runs forever.
//!   4. **Idle/hold.** Pixel memory retains black; COM keeps toggling. Heartbeat once/0.5 s.
//!
//! Build/flash (the bin only compiles with its feature):
//! ```sh
//! cargo run --release --bin ls021_bringup --features ls021-bringup
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

#[path = "../ls021.rs"]
mod ls021;
use ls021::{com_task, PanelBus};

/// High-priority executor the COM driver runs on, pended from the unused SWI00 software-
/// interrupt vector (SWI00 carries no peripheral; we only borrow its vector as the pend
/// line). COM at P3 preempts the thread-mode idle loop so it free-runs CPU-independently.
static EXECUTOR_COM: InterruptExecutor = InterruptExecutor::new();

#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_COM.on_interrupt();
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Run the M33 at its full 128 MHz (embassy default is 64 MHz); the L0 asm::delay
    // bit-bang calibration in `ls021.rs` assumes this.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };

    // The 12 gate/source lines, all boot `Output(Lo)` (datasheet boot-safe state). DK pins
    // are the L0 harness map in `firmware/docs/ls021-bringup.md`. `PanelBus` owns them and
    // clocks the init frame. Standard drive (these are logic lines, not the COM cap load).
    let mut bus = PanelBus::new(
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GSP
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GCK
        Output::new(p.P1_04, Level::Low, OutputDrive::Standard), // GEN
        Output::new(p.P1_06, Level::Low, OutputDrive::Standard), // INTB
        Output::new(p.P1_07, Level::Low, OutputDrive::Standard), // BSP
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // BCK
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0 (odd)
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0 (odd)
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0 (odd)
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1 (even)
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1 (even)
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1 (even)
    );

    // COM lines as high-drive GPIO (56–77 nF load each), boot `Lo` (safe state); held `Lo`
    // through the init frame, then moved into `com_task`. VCOM=P2_07, VB=P2_08, VA=P2_10.
    let vcom = Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive);
    let vb = Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive);
    let va = Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive);

    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard); // LED0 (active-HIGH)

    // 1. Settle window (~2 s, LED0 ~2 Hz): all inputs `Lo`, COM `Lo`. Meter window; hands-free
    //    so the bench LA capture at reset is deterministic.
    info!("LS021 L2: SETTLE (~2s, all inputs Lo, COM held Lo) — then init-black, then COM");
    for _ in 0..8 {
        led.toggle();
        Timer::after_millis(250).await;
    }

    // 2. Init #0 — INTB-framed all-black frame. Blocks thread-mode for the one-shot frame
    //    (~0.5 s); COM is not running yet, so monopolising the CPU is correct here.
    info!("LS021 L2: INIT-BLACK — INTB-framed frame, 640 sub-lines × 120 BCK, all data Lo");
    led.set_high(); // LED steady-on marks the init frame on the bench
    bus.init_black();
    led.set_low();
    info!("LS021 L2: init frame done (pixel memory = black)");

    // 3. T4 ≥ 30 µs, then start COM on the high-priority interrupt executor (P3). From here
    //    COM free-runs forever — VCOM≡VB in phase, VA inverse, ~60 Hz / 50 %.
    Timer::after_micros(50).await;
    interrupt::SWI00.set_priority(Priority::P3);
    let com_spawner = EXECUTOR_COM.start(interrupt::SWI00);
    com_spawner.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
    info!("LS021 L2: COM RUNNING — panel should hold UNIFORM BLACK (MIP retains; COM toggles)");

    // 4. Idle/hold forever. Pixel memory retains black with no refresh; COM keeps toggling on
    //    the interrupt executor. Heartbeat once per 0.5 s. Reset re-enters the settle state.
    let mut heartbeat: u32 = 0;
    loop {
        Timer::after_millis(500).await;
        led.toggle();
        info!("L2: holding black, COM free-running (heartbeat {=u32})", heartbeat);
        heartbeat = heartbeat.wrapping_add(1);
    }
}
