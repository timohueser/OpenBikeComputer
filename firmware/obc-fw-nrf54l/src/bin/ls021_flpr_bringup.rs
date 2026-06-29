//! **LS021 FLPR bring-up bench bin** (epic #149) — drives the FLPR `Panel` backend through test
//! patterns on real glass. Since #165 the backend itself ([`Ls021Flpr`]) lives in the shared
//! `src/ls021_flpr.rs` module so the *real app* (the default `main.rs` build) drives the same
//! LS021 panel through the same [`obc_platform::Panel`] seam; this bin keeps it exercised in
//! isolation — boot the FLPR, draw the line/box + solid cards through the seam, step them with
//! BTN0 — without the SD/sensor/app machinery.
//!
//! The FLPR scans the whole frame top-to-bottom in one `CMD_RUN_FRAME` (see the module doc): a
//! whole-frame generator (this bin's `line_test_card`, and the real `App::render_frame`) puts
//! pixels on the LS021 with **no panel-specific code**.
//!
//! Power-on sequence (datasheet §6-2, mirroring the L3 bin):
//!   1. **Settle (~2 s)** — rails up, all inputs `Lo`, COM `Lo`. Hands-free for a deterministic LA
//!      capture at reset; LED1 blinks.
//!   2. **Launch the FLPR**, wait for its `ALIVE` stamp ([`launch_flpr`]).
//!   3. **Init #0 — `INTB`-framed all-black frame** through the `Panel` seam (a black `clear`). COM `Lo`.
//!   4. **Wait `T4 ≥ 30 µs`, then start COM** on a high-priority `InterruptExecutor` — runs forever.
//!   5. **BTN0 steps** the screen: line card → white → greys → black → **partial-update demo** →
//!      wrap. The first five are drawn through the `Panel`/`Band` seam and FLPR-driven once (MIP
//!      retains each); the last (issue #163) re-pushes only a top strip via `push_spans` to prove
//!      the dirty-row masked scan — the backdrop below holds, the frame time scales to the strip.
//!
//! **Both cores on P2 at once.** The FLPR drives the source bus on `P2.00..06`; the M33 drives COM
//! on `P2.07/08/10` from `com_task`. Safe because every GPIO touch on either core is an atomic
//! `OUTSET`/`OUTCLR` of disjoint pin masks. The gate lines (`GSP`/`GCK`/`GEN`/`INTB`) + `BSP` are all
//! on **P1**, FLPR-driven — **relocated for the app integration** (issue #165, see the pin block in
//! `main` and the masks in `src/flpr/flpr_pingpong.c`).
//!
//! Build/flash (needs a RISC-V gcc for the blob — `brew install riscv64-elf-gcc`; and the
//! Board-Configurator ext-memory-off / 3.3 V-VDDM settings the LS021 panel already needs):
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
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use obc_render::text::{draw_text, Font, TextAlign};
use {defmt_rtt as _, panic_probe as _};

// The free-running COM driver is panel-board-agnostic infrastructure (not the M33-direct PanelBus,
// which the FLPR replaces).
#[path = "../com.rs"]
mod com;
// The shared FLPR `Panel` backend (#165): boot/launch + the resident-framebuffer ping-pong push.
#[path = "../ls021_flpr.rs"]
mod ls021_flpr;
use com::com_task;
use ls021_flpr::{launch_flpr, show, FlprError, Ls021Flpr, FB_H, FB_W};

/// Resident RGB222 (device-64) framebuffer, one byte per pixel — the production map plane's exact
/// type/size. `Ls021Flpr` owns it; `flush_band` fills it (the test cards' RGB565 → device-64
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

/// Pack an 8-bit-per-channel colour into native RGB565, so the test card below reads as plain hexes.
fn c565(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(r >> 3, g >> 2, b >> 3)
}

/// A **line / box diagnostic card** (issue #155 bench) — to tell apart "the font is drawn wrong"
/// from "the panel's area-gradation cell structure is just visible on solid strokes".
///
/// The `Display` font draws as a clean 1:1 Terminus 16×32 bitmap, so any fine-line texture in it
/// must come from the panel, not the renderer. This card isolates that: it draws bars and a box in
/// the foreground colour `fg` at the **same stroke widths as the font**, beside the actual `Display`
/// digits (also in `fg`). Two outcomes:
///   - the `fg` bars/box show the **same** striations as the font → it's the panel's per-pixel area
///     blocks (the 2/3-area MSB + 1/3-area LSB sub-cells), inherent to how it makes 64 colours;
///   - the `fg` bars/box are **clean** while the font is striped → a real pixel/pack bug, dig in.
///
/// **`fg` recolours the bars/box/combs/digits** so the *identical* card can be drawn per channel
/// (black / red / green / blue). Red/green/blue each ride a different source-bus line pair
/// (`R0/R1` · `G0/G1` · `B0/B1`), so if the artifact changes with colour it points at the source
/// shift, not the gate scan; if it's identical across all four it's the gate / area structure.
///
/// The right-hand column is a fixed (colour-independent) gradation reference — solid boxes at device
/// levels 0/2/1: level 2 lights only the 2/3 (MSB) area, level 1 only the 1/3 (LSB) area, so they
/// **should** look textured/dim by design while level 0 is a whole (off) cell. The 1-px combs at the
/// bottom expose any odd/even column interleave or row-drop error (they'd break the regular
/// alternation). Drawn through any [`DrawTarget`], so it drives the `Panel`/`Band` seam unchanged.
fn line_test_card<D>(target: &mut D, fg: Rgb565) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    /// Fill one axis-aligned box — a free helper so it borrows `target` only per call (a closure
    /// capturing `target` would hold the borrow across the `draw_text` calls below).
    fn bar<D: DrawTarget<Color = Rgb565>>(
        t: &mut D,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        c: Rgb565,
    ) -> Result<(), D::Error> {
        t.fill_solid(&Rectangle::new(Point::new(x, y), Size::new(w, h)), c)
    }

    let white = c565(255, 255, 255); // device (3,3,3) — whole cell on (background)
    let black = c565(0, 0, 0); // device (0,0,0) — whole cell off (gradation level-0 reference)
    let gray2 = c565(170, 170, 170); // device (2,2,2) — only the 2/3-area (MSB) block
    let gray1 = c565(85, 85, 85); // device (1,1,1) — only the 1/3-area (LSB) block
    let w = target.bounding_box().size.width as i32;

    target.clear(white)?;
    draw_text(target, "LINE / BOX TEST", Point::new(w / 2, 2), Font::Label, TextAlign::Center, fg);

    // Foreground (fg) vertical bars, widths 1..8 px (the font strokes are ~3-4 px).
    let mut x = 6;
    for bw in [1u32, 2, 3, 4, 5, 6, 8] {
        bar(target, x, 22, bw, 52, fg)?;
        x += bw as i32 + 14;
    }

    // Foreground (fg) horizontal bars, widths 1..8 px.
    let mut y = 84;
    for bw in [1u32, 2, 3, 4, 5, 6, 8] {
        bar(target, 6, y, 150, bw, fg)?;
        y += bw as i32 + 9;
    }

    // Fixed gradation reference column: solid boxes at device levels 0 / 2 / 1 (whole / 2-3 / 1-3 area).
    bar(target, 184, 22, 44, 40, black)?; // level 0 — whole cell off
    bar(target, 184, 70, 44, 40, gray2)?; // level 2 — 2/3-area only (should look textured)
    bar(target, 184, 118, 44, 40, gray1)?; // level 1 — 1/3-area only (should look textured/dimmer)

    // Direct A/B: a solid fg box the height of the Display cap, beside the actual Display digits.
    bar(target, 6, 200, 40, 32, fg)?;
    draw_text(target, "12.5", Point::new(54, 196), Font::Display, TextAlign::Left, fg);

    // 1-px combs: alternating columns then alternating rows. A clean regular pattern means columns
    // and rows are addressed right; a broken/solid/doubled patch means an interleave or row-drop bug.
    let cy = 248;
    for gx in (6..150).step_by(2) {
        bar(target, gx, cy, 1, 22, fg)?; // vertical comb (every other column)
    }
    for gry in (cy + 30..cy + 52).step_by(2) {
        bar(target, 6, gry, 150, 1, fg)?; // horizontal comb (every other row)
    }
    Ok(())
}

/// Top-strip height (rows) the partial-update demo re-pushes — small enough to make the big vertical
/// win obvious on the LA (40 of 320 rows scanned ⇒ a frame ~⅛ the full one) yet visible on glass.
const STRIP_H: u16 = 40;

/// **Partial / dirty-row update demo (issue #163)** — the headline proof of the masked scan, in
/// isolation. Draws a full backdrop once (the line card), then repeatedly re-pushes **only the top
/// [`STRIP_H`] rows** via [`push_spans`](Ls021Flpr::push_spans): the FLPR fast-forwards the clean
/// rows below (`GEN` idle, nothing latches) and early-stops, so the backdrop **holds** while the
/// strip animates and the per-frame time scales to ~`STRIP_H/320` of a full frame (each push logs
/// it). On the LA: a short data burst then a `GCK` fast-forward burst with `GEN` idle, `INTB`
/// dropping right after the last dirty row. Runs until BTN0 advances out (debounced like
/// [`wait_for_press`]).
async fn partial_update_demo(panel: &mut Ls021Flpr<'_>, btn0: &Input<'_>) {
    // Full backdrop once, so the retained region below the strip is recognizable on glass.
    show(panel, |t| {
        line_test_card(t, Rgb565::BLACK).ok();
    });
    info!(
        "LS021 FLPR: PARTIAL-UPDATE demo — re-pushing only the top {=u16} of {=usize} rows; the line card below holds",
        STRIP_H, FB_H
    );

    let mut tick = 0usize;
    loop {
        // Repaint ONLY the top strip of the resident framebuffer (device-64 bytes, 0b00_RR_GG_BB):
        // a black field with a 16-px WHITE bar (0x3F = level 3/3/3) sliding across, so each partial
        // push visibly changes the strip while everything below stays put.
        let fb = panel.fb_mut();
        let bar = (tick * 12) % (FB_W - 16);
        for row in 0..STRIP_H as usize {
            let base = row * FB_W;
            fb[base..base + FB_W].fill(0x00); // black
            fb[base + bar..base + bar + 16].fill(0x3F); // white bar (R=G=B=3)
        }
        // Drive ONLY rows [0, STRIP_H): the FLPR fast-forwards [STRIP_H, FB_H) and early-stops.
        panel.push_spans(&[(0, STRIP_H)]);
        tick += 1;

        // Poll BTN0 to advance out of the demo (debounced + drained, like `wait_for_press`).
        if btn0.is_low() {
            Timer::after_millis(15).await;
            if btn0.is_low() {
                while btn0.is_low() {
                    Timer::after_millis(5).await;
                }
                return;
            }
        }
        Timer::after_millis(350).await;
    }
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
    // moved to free P1 pins. The DK breaks out only P1.00–14 (P1.02/03 are NFC, off-limits) = one pin
    // short for everything the app puts on P1, so SD `CS` moved to P0.00 (in `main.rs`), freeing P1.12
    // for GEN; INTB took P1.10 (LED1). **These must match `flpr_pingpong.c`'s masks + `main.rs`'s pins**
    // — remap all three together if a pin isn't broken out on your DK.
    //   • Gate + frame (P1): GSP P1.00, GCK P1.01, GEN P1.12, INTB P1.10 (LED1).
    //   • Source bus: BSP P1.14 (P1) + BCK P2.06 + the 6 data lines P2.00..05 (P2).
    //   • Heartbeat: LED0 P2.09 (below).
    let _gate_bus = [
        Output::new(p.P1_00, Level::Low, OutputDrive::Standard), // GSP  (gate start pulse)
        Output::new(p.P1_01, Level::Low, OutputDrive::Standard), // GCK  (gate clock / area-plane select)
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN  (gate output enable)
        Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // INTB (frame envelope; LED1)
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
    // COM lines as high-drive GPIO (56–77 nF load each), boot `Lo` (safe state); held `Lo` through
    // the init frame, then moved into `com_task`. VCOM=P2.07, VB=P2.08, VA=P2.10 (M33-driven).
    let vcom = Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive);
    let vb = Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive);
    let va = Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive);

    // LED0 (P2.09) = the M33's heartbeat — proves the M33 keeps running alongside the FLPR. (Moved off
    // LED1/P1.10, which now carries INTB.)
    let mut led1 = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);
    let btn0 = Input::new(p.P1_13, Pull::Up); // DK BTN0 — active-LOW (pressed = Lo); steps the screen

    info!("LS021 FLPR bring-up: launcher up");

    // 1. Settle window (~2 s, LED1 ~2 Hz): all inputs `Lo`, COM `Lo`. Meter window; hands-free so the
    //    bench LA capture at reset is deterministic.
    info!("LS021 FLPR: SETTLE (~2s, all inputs Lo, COM held Lo) — then FLPR boot, init-black, COM, line card");
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
    info!("LS021 FLPR: COM RUNNING — BTN0 steps LINE-BLACK → L3-WHITE → L2-GRAY → L1-GRAY → L0-BLACK → PARTIAL-UPDATE");

    // 5. BTN0 steps the screen through the Panel seam: the line/box diagnostic card, then solids.
    //    MIP retains each.
    let gray_l2 = Rgb565::new(21, 42, 21); // → device (2,2,2)  (MSB plane, 2/3-area)
    let gray_l1 = Rgb565::new(10, 21, 10); // → device (1,1,1)  (LSB plane, 1/3-area)
    let mut i = 0usize;
    loop {
        // The partial-update demo (issue #163) owns its own animate-until-press loop; the rest are
        // draw-once full frames the MIP retains until the next BTN0 step.
        if i == 5 {
            partial_update_demo(&mut panel, &btn0).await;
        } else {
            match i {
                0 => {
                    info!("LS021 FLPR: SHOW LINE-BLACK");
                    show(&mut panel, |t| {
                        line_test_card(t, Rgb565::BLACK).ok();
                    });
                }
                1 => {
                    info!("LS021 FLPR: SHOW L3-WHITE");
                    show(&mut panel, |t| solid(t, Rgb565::WHITE));
                }
                2 => {
                    info!("LS021 FLPR: SHOW L2-GRAY (MSB plane)");
                    show(&mut panel, |t| solid(t, gray_l2));
                }
                3 => {
                    info!("LS021 FLPR: SHOW L1-GRAY (LSB plane)");
                    show(&mut panel, |t| solid(t, gray_l1));
                }
                _ => {
                    info!("LS021 FLPR: SHOW L0-BLACK");
                    show(&mut panel, |t| solid(t, Rgb565::BLACK));
                }
            }
            wait_for_press(&btn0).await;
        }
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
