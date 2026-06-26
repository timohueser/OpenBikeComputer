//! **LS021B7DD02 panel driver primitives** — M33-direct, pre-FLPR (epic #139).
//!
//! Shared building blocks for the `ls021_bringup` bin, grown one bring-up stage at a
//! time. It holds **L1 ([#141]): the free-running COM driver** ([`com_task`]) and
//! **L2 ([#142]): the gate-scan / source-shift primitives** ([`PanelBus`]) that run the
//! datasheet power-on init → all-black frame. L3–L4 grow `PanelBus` with real source data.
//! See `firmware/docs/ls021-bringup.md` for the normative protocol/timing spec this is
//! written against.
//!
//! ## COM driver ([`com_task`])
//!
//! The Memory-in-Pixel cells must never see a **DC bias**, so `VCOM`/`VB`/`VA` have to
//! alternate forever (~60 Hz, ~50 % duty) the *whole* time the panel is powered and
//! driven — even on a perfectly static image. `VB` is **in phase** with `VCOM`; `VA` is
//! its **exact inverse**.
//!
//! ### Why a GPIO toggle on a timer, not a PWM peripheral
//!
//! A PWM peripheral would be the textbook choice (zero-CPU, glitch-free). But on this part
//! **PWM20 will not drive the COM pins** `P2.07/08/10`: with the PWM running, the analyzer
//! showed the lines dead `Lo`, while a plain `gpio::Output` on the *same* pins toggles them
//! cleanly (as L0's signal-walk already proved). The PWM output simply does not route onto
//! that GPIO port here. So COM is generated the way the L1 issue explicitly sanctions as the
//! fallback — a **GRTC/timer-backed GPIO square wave**: [`com_task`] flips the three lines
//! and `await`s half a period, forever.
//!
//! To keep it free-running **while the M33 is busy elsewhere** (the L1 non-blocking
//! requirement), spawn `com_task` on a **high-priority `InterruptExecutor`** (see the bin):
//! the GRTC wakeup pends that executor and preempts thread-mode, so COM never stalls behind
//! a long-running thread-mode loop. The crossings are effectively simultaneous — three
//! back-to-back register writes, tens of ns apart, far below the ~100 µs edge spec — so
//! there is no meaningful overlap glitch.
//!
//! Built as a task rather than a struct so the COM pins move into it and toggle for the
//! life of the program. The L2 "hold COM `Lo` during init, then start" enable is just
//! *when* it is spawned: the pins boot `Output(Lo)` and stay `Lo` until the task runs.
//!
//! Each COM line is a real **56–77 nF** load, so the bin configures the three as
//! **high-drive (H0H1)** GPIO to slew it inside the datasheet ≤100 µs rise/fall (~2.5 mA).
//! If the analyzer shows rounded edges into the real load, external buffering is the
//! documented fallback (see the spec doc).
//!
//! [#141]: https://github.com/timohueser/OpenBikeComputer/issues/141

use cortex_m::asm::delay as cpu_delay;
use embassy_nrf::gpio::Output;
use embassy_time::Timer;

/// Half of the ~60 Hz COM period: `1 / 60 / 2 ≈ 8333 µs` → 60.0 Hz, 50 % duty. Inside the
/// datasheet `f_VCOM` 54–66 Hz / 48–52 % window.
pub const COM_HALF_PERIOD_US: u64 = 8333;

/// The free-running COM driver: a ~60 Hz square wave with `vcom`/`vb` in phase and `va` the
/// exact inverse. Runs forever — **spawn it on a high-priority `InterruptExecutor`** so it
/// keeps toggling while the thread-mode CPU is busy (see the module docs).
///
/// The three pins are owned by the task for the life of the program; pass them already
/// configured as high-drive outputs (they boot `Lo` = the COM-held-`Lo` init state, and the
/// first half-period below raises `va` to its inverse phase).
#[embassy_executor::task]
pub async fn com_task(mut vcom: Output<'static>, mut vb: Output<'static>, mut va: Output<'static>) {
    loop {
        // First half: VCOM/VB high, VA low.
        vcom.set_high();
        vb.set_high();
        va.set_low();
        Timer::after_micros(COM_HALF_PERIOD_US).await;
        // Second half: VCOM/VB low, VA high — the exact inverse crossing.
        vcom.set_low();
        vb.set_low();
        va.set_high();
        Timer::after_micros(COM_HALF_PERIOD_US).await;
    }
}

// ───────────────────────────── L2: gate-scan / source-shift ─────────────────────────────
//
// The pixel side of the panel: the 6-bit parallel **source** bus (`R0/G0/B0` = odd pixel,
// `R1/G1/B1` = even pixel + `BSP`/`BCK`) and the **gate** scan (`GSP`/`GCK`/`GEN`), framed
// by `INTB`. [`PanelBus`] owns these 12 lines and clocks the datasheet power-on init frame:
// every gate sub-line written **black** (all six data lines `Lo`). See the spec doc's
// "Horizontal/Vertical timing" + "Power-on" sections; this is the L2 ([#142]) realization.
//
// ## Why bit-bang, and why synchronous
//
// A full black frame is **640 sub-lines × 120 `BCK`** ≈ 77k clock edges. Doing that with
// async `Timer` `.await`s would pay the scheduler cost on every edge (a frame would take
// many seconds). So the primitives are **synchronous** and busy-wait with
// [`cortex_m::asm::delay`] — and that is safe here because the init frame runs **once**,
// while **COM is still held `Lo`** and nothing else needs the CPU (COM only starts, on the
// interrupt executor, *after* the frame). Blocking thread-mode for the ~0.5 s frame is fine.
//
// ## Why black is the right first scan (and forgiving)
//
// Black = every subpixel `Lo`, so it exercises the **scan** (does every gate line address?
// does every column shift?) independently of the **data**. A missed gate row keeps its
// power-on garbage (an MIP panel powers up with *retained/undefined* pixels, not black), a
// stuck column shows a stripe — so a genuinely uniform black field is the proof. It is also
// forgiving of any source-vs-gate **phase** error (everything's `Lo`), which is exactly why
// it comes before L3 colour: we pin the *timing relationships* down on the analyzer here,
// then trust them when real data matters.
//
// ## Timing (deliberately slow; analyzer-pinned)
//
// All edges are bit-banged **well under** the 0.758 MHz `BCK` max so the logic analyzer
// resolves every one cleanly — speed is a later concern (the eventual FLPR backend). Counts
// are in `asm::delay` units, **LA-calibrated in L0** (asm::delay ≈ 3.96 cyc/count on this
// M33 @128 MHz → ~32 counts/µs). The chart-read relationships (`GCK(1)∈GSP`, `BCK(1)∈BSP`,
// `GCK↔GEN` setup/hold, `GEN` hi) are honoured with generous margins and **verified on the
// analyzer** — adjust here if the capture disagrees with the datasheet chart.
//
// [#142]: https://github.com/timohueser/OpenBikeComputer/issues/142

/// `asm::delay` counts per microsecond on this M33 @128 MHz (L0 LA calibration:
/// ~3.96 cyc/count → 128/3.96 ≈ 32). All bit-bang delays below are multiples of this.
const COUNTS_PER_US: u32 = 32;

/// Columns clocked per sub-line: 240 columns ÷ 2 pixels-per-`BCK` = **120 `BCK`**.
pub const COLS_PER_SUBLINE: u16 = 120;
/// Sub-lines per frame: **320 rows × {MSB, LSB}** = **640** (every row is written twice).
pub const SUBLINES_PER_FRAME: u16 = 640;

/// `BCK` half-period, each phase. ~3 µs → `BCK` ≈ 165 kHz, comfortably under the 0.758 MHz
/// max and ≫ the 660 ns min hi/lo. (Frame ≈ 640×120×~7 µs ≈ 0.55 s — a fine one-shot.)
const BCK_HALF: u32 = 3 * COUNTS_PER_US;
/// Source-data stable before `BCK` rises (spec ~335 ns; we hold ~1 µs). For black the data
/// is constant `Lo`, but the gap models where L3 presents per-column RGB222.
const DATA_SETUP: u32 = COUNTS_PER_US;
/// `GCK`↔`GEN` setup *and* hold (spec ≥16.37 µs → ~17 µs each side of the `GEN` pulse).
const GEN_SETUP_HOLD: u32 = 17 * COUNTS_PER_US;
/// `GEN` high — the valid-output window (spec ≥24.56 µs → ~25 µs).
const GEN_HIGH: u32 = 25 * COUNTS_PER_US;
/// Inter-line `GCK` low gap.
const GCK_LOW: u32 = 5 * COUNTS_PER_US;

/// The 12 **gate + source** signal lines, owned together so the scan/shift primitives can
/// clock them. COM (`VCOM`/`VB`/`VA`) is *not* here — it stays separate and free-runs on
/// [`com_task`]; the bin starts it only after the init frame.
///
/// All 12 boot `Output(Lo)` (the datasheet boot-safe state). L2 only ever drives them to
/// produce **black** (data lines stay `Lo`); L3/L4 will present real RGB222 in
/// [`PanelBus::write_subline_black`]'s inner loop.
pub struct PanelBus {
    // Gate / frame:
    gsp: Output<'static>,  // gate start pulse (once per frame)
    gck: Output<'static>,  // gate clock (steps each sub-line)
    gen: Output<'static>,  // gate output enable (valid-output window)
    intb: Output<'static>, // init-frame framing (high only during the all-black init)
    // Source / shift:
    bsp: Output<'static>, // sub-line start pulse
    bck: Output<'static>, // source/shift clock (120 per sub-line)
    r0: Output<'static>,  // odd-pixel R/G/B
    g0: Output<'static>,
    b0: Output<'static>,
    r1: Output<'static>, // even-pixel R/G/B
    g1: Output<'static>,
    b1: Output<'static>,
}

impl PanelBus {
    /// Take the 12 gate/source lines (already configured `Output(Lo)` by the caller). Pin
    /// order matches the L0 harness map in `firmware/docs/ls021-bringup.md`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gsp: Output<'static>,
        gck: Output<'static>,
        gen: Output<'static>,
        intb: Output<'static>,
        bsp: Output<'static>,
        bck: Output<'static>,
        r0: Output<'static>,
        g0: Output<'static>,
        b0: Output<'static>,
        r1: Output<'static>,
        g1: Output<'static>,
        b1: Output<'static>,
    ) -> Self {
        Self { gsp, gck, gen, intb, bsp, bck, r0, g0, b0, r1, g1, b1 }
    }

    /// Force every source data line `Lo` — the black pixel pair. (L3 replaces this with the
    /// per-column RGB222 present step.)
    fn present_black(&mut self) {
        self.r0.set_low();
        self.g0.set_low();
        self.b0.set_low();
        self.r1.set_low();
        self.g1.set_low();
        self.b1.set_low();
    }

    /// Shift one **black** sub-line: pulse `BSP`, then clock **120 `BCK`** with all six data
    /// lines `Lo`. `BCK(1)` rises **within** `BSP` high (chart), then `BSP` drops.
    fn write_subline_black(&mut self) {
        self.present_black(); // data held Lo for the whole frame; L3 sets data per column here
        self.bsp.set_high();
        for col in 0..COLS_PER_SUBLINE {
            cpu_delay(DATA_SETUP); // data stable before BCK rises (spec ~335 ns)
            self.bck.set_high();
            if col == 0 {
                self.bsp.set_low(); // BCK(1) high fell within BSP high — now release BSP
            }
            cpu_delay(BCK_HALF);
            self.bck.set_low();
            cpu_delay(BCK_HALF);
        }
    }

    /// Advance the gate by one sub-line: a `GCK` cycle with a `GEN` valid-output window
    /// nested inside its high time. On the **first** step of a frame, `GSP` is released here
    /// so its high overlaps `GCK(1)` high (chart).
    fn gate_step(&mut self, first_of_frame: bool) {
        self.gck.set_high();
        if first_of_frame {
            self.gsp.set_low(); // GCK(1) high fell within GSP high — now release GSP
        }
        cpu_delay(GEN_SETUP_HOLD); // GCK→GEN setup ≥16.37 µs
        self.gen.set_high();
        cpu_delay(GEN_HIGH); // GEN high ≥24.56 µs
        self.gen.set_low();
        cpu_delay(GEN_SETUP_HOLD); // GEN→GCK hold ≥16.37 µs
        self.gck.set_low();
        cpu_delay(GCK_LOW);
    }

    /// Run **one full all-black frame**: `GSP` start pulse, then 640 sub-lines, each a
    /// black source shift + a gate step. The caller frames this with `INTB` high (see
    /// [`PanelBus::init_black`]). Every row is written twice — even sub-line = the MSB block,
    /// odd = the LSB block; for black both are identical (all `Lo`), so the loop is uniform,
    /// but the 640-count (= 320 × 2) is exactly the MSB-then-LSB double-write L3 needs.
    pub fn frame_black(&mut self) {
        self.gsp.set_high(); // start pulse: loads the first gate
        for sub in 0..SUBLINES_PER_FRAME {
            self.write_subline_black();
            self.gate_step(sub == 0); // release GSP on the first GCK (GCK(1) ∈ GSP high)
        }
        self.gsp.set_low(); // belt-and-suspenders; already released on sub 0
    }

    /// The datasheet **power-on init**: `INTB`-framed all-black frame (step 2 of the
    /// sequence). Pulls `INTB` high for the frame and back `Lo` after — clearing pixel
    /// memory to a known black. COM must still be held `Lo` by the caller throughout; it is
    /// started only after this returns (+ the `T4 ≥ 30 µs` wait).
    pub fn init_black(&mut self) {
        self.intb.set_high();
        self.frame_black();
        self.intb.set_low();
    }
}
