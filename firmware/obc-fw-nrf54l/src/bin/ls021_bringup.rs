//! **LS021 bring-up L1** (issue #141, epic #139) — the free-running COM driver on glass.
//!
//! Still not the pixel driver. It brings up the **safety-critical, always-on COM signal**
//! in isolation: `VCOM`/`VB`/`VA` as a ~60 Hz, ~50 %-duty square wave (`VB` in phase with
//! `VCOM`, `VA` the exact inverse) so the Memory-in-Pixel cells never see a DC bias.
//!
//! COM is a **GPIO square wave** ([`ls021::com_task`]) — *not* a PWM peripheral, because
//! PWM20 will not drive the COM pins on this part (see the `ls021` module docs). To keep it
//! free-running while the M33 is busy, the task runs on a **high-priority `InterruptExecutor`
//! pended from SWI00 (P3)**; the thread-mode loop below then monopolises the CPU and COM
//! keeps toggling regardless — the L1 non-blocking proof. See `firmware/docs/ls021-bringup.md`
//! for the spec and the analyzer harness.
//!
//! **Panel-safe by construction.** Every one of the 15 signal lines boots `Output(Lo)` (the
//! datasheet boot state) — the 12 gate/source lines held `Lo`, the 3 COM lines `Lo` until the
//! task starts. After a brief settle window COM starts and free-runs forever (never static =
//! no DC bias). No gate/source frame is ever clocked here.
//!
//! Sequence:
//!   1. **Boot → all-`Lo` safe state (~2 s).** Rails settle; COM held `Lo`. No button gate,
//!      so a power-cycle gives a deterministic, hands-free LA capture.
//!   2. **COM auto-starts** on the interrupt executor; the M33 then spins an **unrelated CPU
//!      busy loop** forever and COM keeps toggling. Verify on the analyzer: `VCOM`≡`VB`, `VA`
//!      inverse, 54–66 Hz, 48–52 % duty, edges within ~100 µs.
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
use ls021::com_task;

// ~1 s of pure CPU spin (asm::delay ≈ 3.96 cyc/count on this M33 @128 MHz). Deliberately
// blocking (no `.await`) so it monopolises thread mode between heartbeats — the COM task on
// the higher-priority executor keeps toggling throughout, which is the L1 acceptance.
const BUSY_SPIN_COUNTS: u32 = 32_000_000;

/// High-priority executor the COM driver runs on, pended from the unused SWI00 software-
/// interrupt vector. SWI00 carries no peripheral; we only borrow its vector as the executor's
/// pend line. Running COM here (P3) means it preempts the thread-mode busy loop in `main`.
static EXECUTOR_COM: InterruptExecutor = InterruptExecutor::new();

#[interrupt]
unsafe fn SWI00() {
    EXECUTOR_COM.on_interrupt();
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Run the M33 at its full 128 MHz (embassy default is 64 MHz), parity with the data path.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };

    // The 12 gate/source signal lines — held `Output(Lo)` for the whole of L1 (datasheet
    // boot-safe state; no gate/source data this stage). DK pins are the L0 map in
    // `firmware/docs/ls021-bringup.md`. `P1_05` (host-driven J-Link VCOM RX) is left alone.
    let _held_lo = [
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GSP
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GCK
        Output::new(p.P1_04, Level::Low, OutputDrive::Standard), // GEN
        Output::new(p.P1_06, Level::Low, OutputDrive::Standard), // INTB
        Output::new(p.P1_07, Level::Low, OutputDrive::Standard), // BSP
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // BCK
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1
    ];

    // COM lines as high-drive GPIO (56–77 nF load each), boot `Lo` (safe state). Moved into
    // `com_task` when COM starts. VCOM = P2_07 (D16), VB = P2_08 (D6), VA = P2_10 (D7).
    let vcom = Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive);
    let vb = Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive);
    let va = Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive);

    let mut led = Output::new(p.P2_09, Level::Low, OutputDrive::Standard); // LED0 (active-HIGH)

    // Brief all-`Lo` settle window (~2 s, LED0 ~2 Hz), COM held `Lo`, then COM starts. No button
    // gate so the bench LA capture is deterministic; L2 will start COM after the real init.
    info!("LS021 L1: ALL-LO SAFE STATE (~2s, COM held Lo) — then COM auto-starts");
    for _ in 0..8 {
        led.toggle();
        Timer::after_millis(250).await;
    }

    // Start COM on the high-priority interrupt executor (P3) so it preempts the thread-mode
    // busy loop below. SWI00 carries the executor's pend; the GRTC wakeup keeps it ticking.
    interrupt::SWI00.set_priority(Priority::P3);
    let com_spawner = EXECUTOR_COM.start(interrupt::SWI00);
    com_spawner.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
    info!("LS021 L1: COM RUNNING — VCOM≡VB in-phase, VA inverse, ~60Hz/50%. M33 now spins a busy loop; COM keeps toggling.");

    // Prove COM is non-blocking: monopolise the M33 in a blocking spin loop (no `.await`),
    // heartbeat once per pass. On the analyzer COM keeps alternating cleanly the whole time,
    // driven by the interrupt executor. Loops forever — reset re-enters the all-`Lo` safe state.
    let mut heartbeat: u32 = 0;
    loop {
        cortex_m::asm::delay(BUSY_SPIN_COUNTS);
        led.toggle();
        info!("L1: COM free-running while the M33 spins (heartbeat {=u32})", heartbeat);
        heartbeat = heartbeat.wrapping_add(1);
    }
}
