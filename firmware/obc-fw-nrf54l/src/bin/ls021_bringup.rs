//! **LS021 bring-up L3** (issue #143, epic #139) — full-frame **solid colour** on glass.
//!
//! The first **colour on glass**. After the datasheet power-on init (the L2 `INTB`-framed
//! all-black frame), it cycles a single solid colour across the whole frame — **white → red →
//! green → blue**, forever. A solid fill is the cleanest test of the data path: anything wrong
//! in a row, a column, or the MSB-vs-LSB handling shows up instantly.
//!   - **White** = every subpixel `Hi` on *both* the MSB and LSB sub-lines (full area on R, G,
//!     B) → confirms the MSB+LSB double-write lights both area blocks; should look neutral.
//!   - **Pure R/G/B** = that channel's two data lines `Hi`, the others `Lo` → confirms the
//!     `R/G/B` line mapping (a swap shows as the wrong colour) and that odd (`*0`) and even
//!     (`*1`) pixels both fill (no every-other-column striping).
//!
//! The fill primitive lives in [`ls021::PanelBus::fill_solid`]; see `firmware/docs/ls021-bringup.md`.
//!
//! **Panel-safe by construction.** All 15 signal lines boot `Output(Lo)` (the datasheet boot
//! state). COM (`VCOM`/`VB`/`VA`) is held `Lo` for the *entire* init frame — the datasheet
//! "COM held `Lo` during init" requirement — and only starts (free-running forever) **after**
//! the frame + the `T4 ≥ 30 µs` wait. Each colour frame is a one-shot blocking fill; COM keeps
//! toggling on the high-priority interrupt executor right through it, so there is no DC bias.
//!
//! Sequence (datasheet §6-2 / spec doc "Power-on"):
//!   1. **Settle (~2 s).** Rails up, all inputs `Lo`, COM `Lo` — the meter / safe-state window.
//!      Hands-free (no button gate) so a power-cycle gives a deterministic LA capture.
//!   2. **Init #0 — `INTB`-framed all-black frame** ([`PanelBus::init_black`]). COM still `Lo`.
//!   3. **Wait `T4 ≥ 30 µs`**, then **start COM** on the interrupt executor — runs forever.
//!   4. **BTN0-stepped grey ramp** (see step 4 in `main`): the current level (BLACK → ⅓ → ⅔ →
//!      WHITE) is continuously re-written — a repeating waveform for the LA — and **BTN0
//!      advances** to the next level, for paced scope/PulseView inspection.
//!
//! Build/flash (the bin only compiles with its feature):
//! ```sh
//! cargo run --release --bin ls021_bringup --features ls021-bringup
//! ```

#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_nrf::gpio::{Input, Level, Output, OutputDrive, Pull};
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
    let btn0 = Input::new(p.P1_13, Pull::Up); // DK BTN0 — active-LOW (pressed = Lo); advances the level

    // 1. Settle window (~2 s, LED0 ~2 Hz): all inputs `Lo`, COM `Lo`. Meter window; hands-free
    //    so the bench LA capture at reset is deterministic.
    info!("LS021 L3: SETTLE (~2s, all inputs Lo, COM held Lo) — then init-black, COM, colour cycle");
    for _ in 0..8 {
        led.toggle();
        Timer::after_millis(250).await;
    }

    // 2. Init #0 — INTB-framed all-black frame. Blocks thread-mode for the one-shot frame
    //    (~0.5 s); COM is not running yet, so monopolising the CPU is correct here.
    info!("LS021 L3: INIT-BLACK — INTB-framed frame, 640 sub-lines × 120 BCK, all data Lo");
    led.set_high(); // LED steady-on marks the init frame on the bench
    bus.init_black();
    led.set_low();
    info!("LS021 L3: init frame done (pixel memory = black)");

    // 3. T4 ≥ 30 µs, then start COM on the high-priority interrupt executor (P3). From here
    //    COM free-runs forever — VCOM≡VB in phase, VA inverse, ~60 Hz / 50 %.
    Timer::after_micros(50).await;
    interrupt::SWI00.set_priority(Priority::P3);
    let com_spawner = EXECUTOR_COM.start(interrupt::SWI00);
    com_spawner.spawn(defmt::unwrap!(com_task(vcom, vb, va)));
    info!("LS021 L3: COM RUNNING — starting colour cycle (MIP retains each frame; COM toggles)");

    // 4. BTN0-STEPPED ramp (for LA / PulseView inspection). The current level is written over and
    //    over — a repeating frame waveform that's easy to trigger and capture — and a BTN0 press
    //    advances to the next entry (grey ramp BLACK → ⅓ → ⅔ → WHITE, then the R/G/B primaries,
    //    then wrap). Per the pixel structure (MSB = top+bottom 2/3-area rows, LSB = middle
    //    1/3-area row), the greys' expected lit sub-rows are: BLACK none / ⅓ middle / ⅔ top+bottom
    //    / WHITE all three; each primary lights only its channel (full level on R, G, or B).
    //
    //    BTN0 is sampled once per frame (~0.6 s), so **hold it briefly until the level changes**;
    //    edge detection (press after release) means a held button advances only once. `LED0`
    //    toggles on each accepted press as confirmation.
    const COLOURS: [(&str, u8, u8, u8); 7] = [
        ("BLACK", 0, 0, 0),
        ("GREY 1/3", 1, 1, 1),
        ("GREY 2/3", 2, 2, 2),
        ("WHITE", 3, 3, 3),
        ("RED", 3, 0, 0),
        ("GREEN", 0, 3, 0),
        ("BLUE", 0, 0, 3),
    ];
    let mut i: usize = 0;
    let mut prev_pressed = false;
    {
        let (name, r, g, b) = COLOURS[0];
        info!("LS021 L3: FILL {=str} (r={=u8} g={=u8} b={=u8}) — press BTN0 to advance", name, r, g, b);
    }
    loop {
        let (_, r, g, b) = COLOURS[i % COLOURS.len()];
        bus.fill_solid(r, g, b); // refresh the current level — a repeating waveform to capture
        let pressed = btn0.is_low(); // active-LOW
        if pressed && !prev_pressed {
            i = i.wrapping_add(1);
            led.toggle(); // confirm the press registered
            let (name, r, g, b) = COLOURS[i % COLOURS.len()];
            info!("LS021 L3: BTN0 → FILL {=str} (r={=u8} g={=u8} b={=u8})", name, r, g, b);
        }
        prev_pressed = pressed;
    }
}
