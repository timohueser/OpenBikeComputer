//! **LS021 bring-up L3** (issue #143, epic #139) — full-frame **solid colour** on glass.
//!
//! The first **colour on glass**. After the datasheet power-on init (the L2 `INTB`-framed
//! all-black frame), **BTN0 steps** through solid colours across the whole frame — **white → red
//! → green → blue** — then a **64-colour palette** (every RGB222 value, 8×8 grid) and a
//! **black-on-white shapes** contrast card. A solid fill is the cleanest test of the data path:
//! anything wrong in a row, a column, or the MSB-vs-LSB handling shows up instantly; the palette
//! and shapes then exercise per-column (spatial) data.
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
//!   4. **BTN0 steps** the pattern: white → R → G → B → **64-colour palette** (8×8 grid of every
//!      RGB222 value) → **black-on-white shapes** (a contrast/readability card), all via
//!      [`PanelBus::fill_with`] → wrap. Each is drawn once (MIP retains it) then BTN0 is polled
//!      responsively for the next press.
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
    let btn0 = Input::new(p.P1_13, Pull::Up); // DK BTN0 — active-LOW (pressed = Lo); steps the pattern

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

    // 4. BTN0 steps the pattern: white → R → G → B → 64-colour palette → wrap. Each pattern is
    //    drawn **once** (MIP retains it — no refresh needed) and then we poll BTN0 in a tight
    //    async loop, so the press is caught within a few ms (not once-per-0.6 s-fill like before).
    //    `LED0` toggles on each accepted press.
    const PATTERNS: [(&str, Draw); 6] = [
        ("WHITE", Draw::Solid(3, 3, 3)),
        ("RED", Draw::Solid(3, 0, 0)),
        ("GREEN", Draw::Solid(0, 3, 0)),
        ("BLUE", Draw::Solid(0, 0, 3)),
        ("PALETTE", Draw::Spatial(palette)),
        ("SHAPES", Draw::Spatial(shapes)),
    ];
    led.set_low();
    let mut i: usize = 0;
    loop {
        let (name, draw) = &PATTERNS[i];
        info!("LS021 L3: SHOW {=str} — press BTN0 for next", name);
        match draw {
            Draw::Solid(r, g, b) => bus.fill_solid(*r, *g, *b),
            Draw::Spatial(f) => bus.fill_with(*f),
        }
        wait_for_press(&btn0).await; // responsive, debounced, one advance per press
        led.toggle();
        i = (i + 1) % PATTERNS.len();
    }
}

/// A cycleable test pattern: a uniform RGB222 `Solid`, or a `Spatial` per-pixel pattern (the
/// palette, the shapes card) drawn via [`PanelBus::fill_with`].
enum Draw {
    Solid(u8, u8, u8),
    Spatial(fn(u16, u16) -> (u8, u8, u8)),
}

/// Wait for one clean BTN0 press. Robust + responsive: first drains any still-held press (so a
/// button held from the previous advance can't double-step), then polls every ~5 ms for a
/// **debounced** press edge. Polling only runs here (between the blocking fills), so latency is a
/// few ms instead of one ~0.6 s frame.
async fn wait_for_press(btn: &Input<'_>) {
    // 1. Ensure released (handles a still-down button), then let the release settle.
    while btn.is_low() {
        Timer::after_millis(5).await;
    }
    Timer::after_millis(20).await;
    // 2. Wait for a press confirmed stable across a short debounce window.
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

/// The 64-colour test palette: an **8×8 grid** of every RGB222 value over the 240×320 panel.
/// `x`/`y` are pixel coordinates; the cell is `x/30` across (8 cells × 30 px = 240) and `y/40`
/// down (8 × 40 = 320). Cell index `0..63` packs as `r<<4 | g<<2 | b`, so columns step blue/green
/// and rows step red — every 2-bit-per-channel combination appears exactly once.
fn palette(x: u16, y: u16) -> (u8, u8, u8) {
    let col = (x / 30).min(7); // 0..7 across
    let row = (y / 40).min(7); // 0..7 down
    let idx = row * 8 + col; // 0..63
    (((idx >> 4) & 3) as u8, ((idx >> 2) & 3) as u8, (idx & 3) as u8)
}

/// `true` if `(x, y)` is inside the `w × h` rectangle at `(x0, y0)`.
fn in_rect(x: u16, y: u16, x0: u16, y0: u16, w: u16, h: u16) -> bool {
    x >= x0 && x < x0 + w && y >= y0 && y < y0 + h
}

/// `true` if `(x, y)` is on the `t`-px border of the `w × h` rectangle at `(x0, y0)`.
fn frame(x: u16, y: u16, x0: u16, y0: u16, w: u16, h: u16, t: u16) -> bool {
    in_rect(x, y, x0, y0, w, h) && !in_rect(x, y, x0 + t, y0 + t, w - 2 * t, h - 2 * t)
}

/// **Black shapes on a white field** — a quick contrast / readability check. Filled squares of
/// decreasing size (top), a line-width ramp of vertical and horizontal bars (10/6/4/2/1 px), and
/// a thin outline frame (bottom), to see how fine a black feature stays legible on the reflective
/// panel. Black `(0,0,0)` inside a shape, white `(3,3,3)` elsewhere.
fn shapes(x: u16, y: u16) -> (u8, u8, u8) {
    let black =
        // Filled squares, decreasing size.
        in_rect(x, y, 16, 16, 100, 100)
            || in_rect(x, y, 130, 16, 60, 60)
            || in_rect(x, y, 130, 88, 30, 30)
            || in_rect(x, y, 172, 88, 14, 14)
            // Vertical bars: 10 / 6 / 4 / 2 / 1 px wide.
            || in_rect(x, y, 16, 150, 10, 100)
            || in_rect(x, y, 44, 150, 6, 100)
            || in_rect(x, y, 66, 150, 4, 100)
            || in_rect(x, y, 84, 150, 2, 100)
            || in_rect(x, y, 98, 150, 1, 100)
            // Horizontal bars: 10 / 6 / 4 / 2 / 1 px tall.
            || in_rect(x, y, 130, 150, 90, 10)
            || in_rect(x, y, 130, 174, 90, 6)
            || in_rect(x, y, 130, 192, 90, 4)
            || in_rect(x, y, 130, 206, 90, 2)
            || in_rect(x, y, 130, 216, 90, 1)
            // Thin outline frame.
            || frame(x, y, 16, 264, 208, 44, 2);
    if black {
        (0, 0, 0)
    } else {
        (3, 3, 3)
    }
}
