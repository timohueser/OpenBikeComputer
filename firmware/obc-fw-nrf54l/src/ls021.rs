//! **LS021B7DD02 panel driver primitives** — M33-direct, pre-FLPR (epic #139).
//!
//! Shared building blocks for the `ls021_bringup` bin, grown one bring-up stage at a
//! time. It holds **L1 ([#141]): the free-running COM driver** ([`com_task`]), **L2 ([#142]):
//! the gate-scan / source-shift primitives** ([`PanelBus`]) that run the datasheet power-on
//! init → all-black frame, and **L3 ([#143]): full-frame solid colour** ([`PanelBus::fill_solid`])
//! — the same scan now carrying real RGB222 data (white, then pure R/G/B). L4 grows it into
//! spatially-varying patterns. See `firmware/docs/ls021-bringup.md` for the normative
//! protocol/timing spec this is written against.
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

// ──────────────────────────── L2/L3: gate-scan / source-shift ────────────────────────────
//
// The pixel side of the panel: the 6-bit parallel **source** bus (`R0/G0/B0` = odd pixel,
// `R1/G1/B1` = even pixel + `BSP`/`BCK`) and the **gate** scan (`GSP`/`GCK`/`GEN`), enveloped
// by `INTB`. [`PanelBus`] owns these 12 lines and clocks frames. **L2 ([#142])** ran the
// datasheet power-on init (an all-black frame); **L3 ([#143])** drives the *same* scan with
// real data: [`PanelBus::fill_solid`] presents a per-channel RGB222 level on every column,
// MSB plane then LSB plane. See the spec doc's "Horizontal/Vertical timing" + "Power-on".
//
// ## Two rules that make a pixel light correctly
//
// **Rule 1 — `INTB` high for the whole frame.** The datasheet vertical chart (§6-5) and
// power-on sequence (§6-5-4) both hold `INTB` **high for the entire duration of *every* frame
// write** — the power-on "Initial #0" black frame *and* every subsequent image. `INTB` low (the
// inter-frame "Hold" state) means "no write": the panel keeps its current pixel memory whatever
// the gate/source scan does. (L2's black init looked right because it *was* `INTB`-framed; the
// first L3 colour attempt drove a perfect scan with `INTB` low and nothing latched — black held.)
//
// **Rule 2 — MSB and LSB are the *same* gate line, selected by `GCK` *level*.** A pixel is
// **not** two (or three) stacked gate lines. The display has **320 gate lines, one per pixel
// row** (§6-6, the `L1..L320` map). What §6-6 draws as three stacked bands (MSB top · LSB middle
// · MSB bottom) is the **area layout *inside one pixel cell***: the MSB block is the top+bottom
// bands wired together = **2/3 area**, the LSB block is the middle band = **1/3 area**. Both
// blocks live on the **same** gate line and are written when that line is selected. §6-4 says it
// outright: "Updates Gate Line 1: First transfer MSB data of 1 line, Second transfer LSB data of
// 1 line." The *only* thing that routes a latch to the 2/3 block vs the 1/3 block is the **`GCK`
// level**: the horizontal chart (§6-5-2) holds `GCK` **HIGH while transferring the MSB plane and
// LOW while transferring the LSB plane**. There is no separate MSB/LSB select pin — `GCK` level
// is it. So one pixel row is **one `GCK` period**:
//
//   1. raise `GCK` (this rising edge also **advances** the gate to the row) → shift MSB plane →
//      `GEN` pulse latches the **2/3-area** cells;
//   2. lower `GCK` (same gate line, *no* advance) → shift LSB plane → `GEN` pulse latches the
//      **1/3-area** cells;
//   3. next row — **one gate advance per pixel row**.
//
// > **The bug this replaces.** The old model treated the two planes as *separate gate lines*
// > (advance between them) *and* fired **both** `GEN` pulses with `GCK` held `Lo`. With `GCK` low
// > selecting the 1/3 block, both latches hit the **LSB cells** — the 2/3 MSB cells never got
// > data (dim, only the middle band lit, MSB bands noise). And advancing between the planes put
// > MSB on line N, LSB on line N+1, so at grey levels every *other* row toggled. All three of the
// > observed symptoms are this one mistake.
//
// ## Frame geometry (datasheet §6-5)
//
// `INTB` high → `GSP` start pulse → **320 pixel rows**, each **one `GCK` period** (MSB phase
// high, LSB phase low, a `GEN` latch in *each*), bracketed by a few dummy gate advances. The
// vertical chart's ~640 `GCK` are *edges* (320 periods × 2), **not** 640 separate rows; ~648
// total = 320 periods + a handful of dummy periods. Each sub-line is **124 `BCK`** (120
// pixel-pairs + 4 flush dummies). That is still ~77k clock edges — far too many for async
// `Timer` `.await`s (scheduler cost per edge) — so the primitives are **synchronous** busy-waits
// ([`cortex_m::asm::delay`]). Safe because a fill blocks thread-mode for ~0.6 s while COM
// free-runs on its own interrupt executor.
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

/// Data columns clocked per sub-line: 240 columns ÷ 2 pixels-per-`BCK` = **120**.
pub const COLS_PER_SUBLINE: u16 = 120;
/// Dummy/flush `BCK` after the 120 data clocks: the datasheet horizontal chart clocks **124**
/// `BCK` per line (120 pixel-pairs + 4 trailing dummies that push the last pixels through the
/// source shift register). Clocking only 120 leaves the right-edge columns unlatched.
const BCK_DUMMY: u16 = 4;
/// Total `BCK` per sub-line = **124** (120 data + 4 dummy).
pub const BCK_PER_SUBLINE: u16 = COLS_PER_SUBLINE + BCK_DUMMY;
/// Visible pixel rows in a frame. **Each pixel row is ONE gate line that carries *both* area
/// planes** — the MSB plane in the `GCK`-high phase, the LSB plane in the `GCK`-low phase (see
/// the module comment, Rule 2). So a frame is **320 gate advances**, one per row — *not* 640.
pub const ROWS_PER_FRAME: u16 = 320;
/// Dummy gate advances bracketing the 320 data rows (pipeline fill / "necessary signal" blank).
/// The vertical chart clocks ~648 `GCK` *edges* ≈ 324 `GCK` *periods* per frame; with one period
/// per pixel row that is 320 data rows + a few dummy periods at each end. Re-pin on the LA if the
/// first/last visible row is off by one — the lead count sets where row 0 lands.
const GATE_DUMMY_LEAD: u16 = 2;
const GATE_DUMMY_TRAIL: u16 = 6;

/// `BCK` half-period, each phase. ~3 µs → `BCK` ≈ 165 kHz, comfortably under the 0.758 MHz
/// max and ≫ the 660 ns min hi/lo. (Frame ≈ 320 rows × 2 sub-lines × ~870 µs ≈ 0.6 s.)
const BCK_HALF: u32 = 2 * COUNTS_PER_US;
/// Source-data stable before `BCK` rises (spec ~335 ns; we hold ~1 µs). For a solid fill the
/// data is constant across the sub-line, but the gap models the per-column setup L4 needs.
const DATA_SETUP: u32 = COUNTS_PER_US;
/// `GCK`↔`GEN` setup *and* hold (spec ≥16.37 µs → ~17 µs each side of the `GEN` pulse). On a
/// data row the long sub-line shift already provides the `GCK`-edge→`GEN` setup; this is the
/// guaranteed `GEN`→next-`GCK`-edge hold (and a generous floor on the setup).
const GEN_SETUP_HOLD: u32 = 17 * COUNTS_PER_US;
/// `GEN` high — the valid-output window (spec ≥24.56 µs → ~25 µs). Fired once per phase: at
/// `GCK` HIGH it latches the 2/3 block, at `GCK` LOW the 1/3 block.
const GEN_HIGH: u32 = 25 * COUNTS_PER_US;
/// `GCK` high pulse width for a **dummy** advance only. On a *data* row `GCK` instead stays high
/// for the entire MSB sub-line shift (and low for the entire LSB sub-line), so this just sizes
/// the empty pipeline-flush advances.
const GCK_HIGH: u32 = 10 * COUNTS_PER_US;
/// Settle after a `GCK` level change (advance edge, or the MSB→LSB phase drop) before shifting.
const GCK_SETTLE: u32 = 5 * COUNTS_PER_US;
/// Frame-framing setup: `INTB`→`GSP` and `GSP`→first `GCK` (chart `thsINTB`/`thsGSP`); generous.
const FRAME_SETUP: u32 = 10 * COUNTS_PER_US;

/// The 12 **gate + source** signal lines, owned together so the scan/shift primitives can
/// clock them. COM (`VCOM`/`VB`/`VA`) is *not* here — it stays separate and free-runs on
/// [`com_task`]; the bin starts it only after the init frame.
///
/// All 12 boot `Output(Lo)` (the datasheet boot-safe state). The gate lines scan the same
/// way for every stage; the **data** lines carry black for the L2 init ([`PanelBus::init_black`])
/// and a real RGB222 level for L3 solid fills ([`PanelBus::fill_solid`]).
pub struct PanelBus {
    // Gate / frame:
    gsp: Output<'static>,  // gate start pulse (once per frame)
    gck: Output<'static>, // gate clock — HIGH = MSB / 2-3-area phase, LOW = LSB / 1-3-area phase; one period per pixel row
    gen: Output<'static>, // gate output enable — pulsed once per phase (latches the block GCK level selects)
    intb: Output<'static>, // frame envelope — HIGH for the whole frame write (every frame)
    // Source / shift:
    bsp: Output<'static>, // sub-line start pulse
    bck: Output<'static>, // source/shift clock (120 data + 4 dummy per sub-line)
    r0: Output<'static>,  // odd-pixel R/G/B
    g0: Output<'static>,
    b0: Output<'static>,
    r1: Output<'static>, // even-pixel R/G/B
    g1: Output<'static>,
    b1: Output<'static>,
}

/// Drive one source data line to a bit (`true` = `Hi` = subpixel shown).
#[inline]
fn set(pin: &mut Output<'static>, on: bool) {
    if on {
        pin.set_high();
    } else {
        pin.set_low();
    }
}

/// Split an RGB222 `(r, g, b)` level triple into the per-channel bits of one area plane: the
/// **MSB** plane is the 2/3-area block bit (`l >> 1`), the **LSB** plane the 1/3-area bit
/// (`l & 1`). Used by the spatial [`PanelBus::fill_with`] path (uniform [`PanelBus::fill_solid`]
/// inlines the same split).
#[inline]
fn plane_bits(level: (u8, u8, u8), msb: bool) -> (bool, bool, bool) {
    let bit = |l: u8| if msb { (l >> 1) & 1 != 0 } else { l & 1 != 0 };
    (bit(level.0), bit(level.1), bit(level.2))
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

    /// Present one RGB pixel-pair on the source bus. For a uniform fill the **odd**
    /// (`R0/G0/B0`) and **even** (`R1/G1/B1`) pixels carry the *same* bits — driving both
    /// proves odd and even columns each fill (no every-other-column striping). Black is just
    /// `present(false, false, false)`. (L4 will differ the two pixels for spatial patterns.)
    fn present(&mut self, r: bool, g: bool, b: bool) {
        set(&mut self.r0, r);
        set(&mut self.g0, g);
        set(&mut self.b0, b);
        set(&mut self.r1, r);
        set(&mut self.g1, g);
        set(&mut self.b1, b);
    }

    /// Shift one **data** sub-line carrying the given RGB bits on every column: pulse `BSP`,
    /// then clock **124 `BCK`** (120 data + 4 dummy). `BCK(1)` rises **within** `BSP` high
    /// (chart), then `BSP` drops. For a solid fill the data is constant across the sub-line, so
    /// it is presented once up front; the per-column `DATA_SETUP` gap models the setup L4 needs.
    ///
    /// The caller has already set `GCK` to the level for this plane (HIGH for MSB, LOW for LSB);
    /// this routine touches only the source bus, never `GCK`/`GEN`.
    fn write_data_subline(&mut self, r: bool, g: bool, b: bool) {
        self.present(r, g, b);
        self.bsp.set_high();
        for col in 0..BCK_PER_SUBLINE {
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

    /// Pulse `GEN` to latch the just-shifted source sub-line into the **currently-selected**
    /// gate line. **The caller sets the `GCK` level first** and that level chooses the target
    /// block: `GCK` HIGH latches the **2/3-area (MSB)** cells, `GCK` LOW the **1/3-area (LSB)**
    /// cells. Fired clear of the `GCK` edges (`GCK`↔`GEN` setup/hold ≥16.37 µs — the long data
    /// shift supplies the setup, the trailing delay supplies the hold).
    fn gen_pulse(&mut self) {
        cpu_delay(GEN_SETUP_HOLD); // data / GCK level settled → GEN setup ≥16.37 µs
        self.gen.set_high();
        cpu_delay(GEN_HIGH); // valid-output window ≥24.56 µs
        self.gen.set_low();
        cpu_delay(GEN_SETUP_HOLD); // GEN → next GCK edge hold ≥16.37 µs
    }

    /// Write **one pixel row** = one gate line carrying both area planes. `msb`/`lsb` are the
    /// per-channel `(R, G, B)` bits for the 2/3-area and 1/3-area blocks of every column.
    ///
    /// One `GCK` period:
    /// - **MSB phase** — raise `GCK` (the rising edge advances the gate to this row), shift the
    ///   MSB plane, `gen_pulse` → latches the **2/3-area** cells while `GCK` is HIGH;
    /// - **LSB phase** — drop `GCK` (same gate line, *not* an advance), shift the LSB plane,
    ///   `gen_pulse` → latches the **1/3-area** cells while `GCK` is LOW.
    ///
    /// The advance to the *next* row is the `GCK` rising edge that opens the next call's MSB
    /// phase, so there is exactly **one** gate advance per pixel row.
    fn write_gate_line(&mut self, msb: (bool, bool, bool), lsb: (bool, bool, bool)) {
        // ── MSB phase: GCK HIGH selects the 2/3-area block; this rising edge advances the gate ──
        self.gck.set_high();
        cpu_delay(GCK_SETTLE);
        self.write_data_subline(msb.0, msb.1, msb.2);
        self.gen_pulse(); // latch 2/3-area cells — GCK still HIGH

        // ── LSB phase: GCK LOW selects the 1/3-area block; SAME gate line, no advance ──
        self.gck.set_low();
        cpu_delay(GCK_SETTLE);
        self.write_data_subline(lsb.0, lsb.1, lsb.2);
        self.gen_pulse(); // latch 1/3-area cells — GCK now LOW
    }

    /// One **dummy** gate advance: a clean `GCK` period (high→low) with `GEN`/`BCK` idle — the
    /// pipeline-fill / "necessary signal" blank lines bracketing the 320 data rows. `release_gsp`
    /// drops `GSP` on the rising edge (used for the very first advance of a frame, so `GSP` high
    /// overlaps `GCK(1)` per the chart).
    fn dummy_advance(&mut self, release_gsp: bool) {
        self.gck.set_high();
        if release_gsp {
            self.gsp.set_low(); // GCK(1) rising edge within GSP high — release GSP
        }
        cpu_delay(GCK_HIGH);
        self.gck.set_low();
        cpu_delay(GCK_SETTLE);
    }

    /// Run **one full solid-colour frame** at the given per-channel RGB222 levels (`0..=3`,
    /// clamped to the low 2 bits). The datasheet frame envelope:
    /// 1. **`INTB` high for the whole frame** — *every* frame is framed this way (the power-on
    ///    "Initial #0" black frame and every image write alike; `INTB` low between frames means
    ///    "no write"). This is what makes pixels actually latch.
    /// 2. **`GSP`** start pulse, then **320 pixel rows**, each **one gate line written in one
    ///    `GCK` period** ([`write_gate_line`]): MSB plane latched into the 2/3-area block while
    ///    `GCK` is HIGH, then LSB plane into the 1/3-area block while `GCK` is LOW — bracketed by
    ///    dummy gate advances.
    ///
    /// Each channel renders as two area blocks — the **MSB** block (2/3 area) and **LSB** block
    /// (1/3 area), each 1-bit on/off → 4 levels/channel → 64 colours. A level `l` maps to
    /// `(msb, lsb) = (l>>1 & 1, l & 1)`: `3`→white/full, `2`→⅔ (MSB on), `1`→⅓ (LSB on),
    /// `0`→black. (`Hi` = subpixel shown; `R/G/B[0]` = odd pixel, `[1]` = even.)
    ///
    /// Blocking busy-wait — fine because COM free-runs on its own interrupt executor and
    /// preempts this thread-mode fill. The L2 black init is just `fill_solid(0, 0, 0)`.
    pub fn fill_solid(&mut self, level_r: u8, level_g: u8, level_b: u8) {
        // 2/3-area (MSB) and 1/3-area (LSB) bit for each channel.
        let msb = ((level_r >> 1) & 1 != 0, (level_g >> 1) & 1 != 0, (level_b >> 1) & 1 != 0);
        let lsb = (level_r & 1 != 0, level_g & 1 != 0, level_b & 1 != 0);

        self.intb.set_high(); // frame envelope HIGH for the whole write (every frame, not just init)
        cpu_delay(FRAME_SETUP); // thsINTB: INTB stable before GSP
        self.gsp.set_high(); // start pulse: loads the first gate
        cpu_delay(FRAME_SETUP); // thsGSP: GSP stable before the first GCK

        // Leading dummy gate advances (GEN/BCK idle); GSP releases on the very first GCK edge.
        for i in 0..GATE_DUMMY_LEAD {
            self.dummy_advance(i == 0);
        }
        // 320 pixel rows — each ONE gate line: MSB phase (2/3 block) then LSB phase (1/3 block),
        // one gate advance per row.
        for _ in 0..ROWS_PER_FRAME {
            self.write_gate_line(msb, lsb);
        }
        // Trailing dummy gate advances — the "necessary signal" blank.
        for _ in 0..GATE_DUMMY_TRAIL {
            self.dummy_advance(false);
        }

        self.gsp.set_low(); // belt-and-suspenders; already released on the first leading GCK
        self.intb.set_low(); // end of frame — panel now holds the image
    }

    /// The datasheet power-on **Initial #0**: an all-black frame. Identical to any image write
    /// ([`fill_solid`] raises `INTB` itself) — just black data, run while COM is still held `Lo`
    /// — so it is simply `fill_solid(0, 0, 0)`. Clears pixel memory to a known black before COM
    /// starts; the caller starts COM only after this returns (+ the `T4 ≥ 30 µs` wait).
    pub fn init_black(&mut self) {
        self.fill_solid(0, 0, 0);
    }

    /// Shift one sub-line of one area `plane` (MSB/LSB) where **each column's** RGB222 level comes
    /// from `color(x, y)`. The `col`-th `BCK` clocks a pixel *pair*: column `2*col` on the odd
    /// lines (`R0/G0/B0`), `2*col+1` on the even lines (`R1/G1/B1`). The 4 trailing dummy `BCK`
    /// present black. Same `BSP`/`BCK` timing as [`write_data_subline`] — only the data is per
    /// column instead of uniform (this is the L4 spatial-pattern path).
    /// Present one `BCK`-column's pixel pair on the source bus: column `2*col` on `R0/G0/B0`,
    /// `2*col+1` on `R1/G1/B1`, for the given area plane. Columns `≥ COLS_PER_SUBLINE` are the
    /// trailing dummy columns → black.
    fn present_pair<F: Fn(u16, u16) -> (u8, u8, u8)>(&mut self, col: u16, y: u16, msb: bool, color: &F) {
        if col < COLS_PER_SUBLINE {
            let (ro, go, bo) = plane_bits(color(2 * col, y), msb);
            let (re, ge, be) = plane_bits(color(2 * col + 1, y), msb);
            set(&mut self.r0, ro);
            set(&mut self.g0, go);
            set(&mut self.b0, bo);
            set(&mut self.r1, re);
            set(&mut self.g1, ge);
            set(&mut self.b1, be);
        } else {
            self.present(false, false, false); // trailing dummy columns → black
        }
    }

    /// Shift one spatial source sub-line **DDR** — a *distinct* `BCK`-column on each clock edge.
    ///
    /// The panel latches the source bus on **both** `BCK` edges (cracked on glass, issue #155): the
    /// original single-edge drive held each pair constant across the whole `BCK` period, so the panel
    /// captured it twice → every pixel pair landed in four columns (left half stretched 2×, right
    /// half dropped, 64→32 colours). Driving DDR — column `2k` set up before the **rising** edge,
    /// column `2k+1` before the **falling** edge — feeds one distinct pair per edge, so the `120`
    /// data columns ship in `60` `BCK` cycles and reassemble as the full 240-wide line (and the
    /// sub-line clocks out ~2× faster). `BCK(1)` (the first rising edge) is still enveloped by `BSP`.
    fn shift_subline_with<F: Fn(u16, u16) -> (u8, u8, u8)>(&mut self, y: u16, msb: bool, color: &F) {
        self.bsp.set_high();
        let mut col = 0;
        while col < BCK_PER_SUBLINE {
            // ── rising-edge column: 2*col / 2*col+1 ──
            self.present_pair(col, y, msb, color);
            cpu_delay(DATA_SETUP);
            self.bck.set_high(); // rising edge latches this column
            if col == 0 {
                self.bsp.set_low(); // BCK(1) rose within BSP high — release BSP
            }
            cpu_delay(BCK_HALF);
            // ── falling-edge column: the next one ──
            self.present_pair(col + 1, y, msb, color);
            cpu_delay(DATA_SETUP);
            self.bck.set_low(); // falling edge latches the next column
            cpu_delay(BCK_HALF);
            col += 2;
        }
    }

    /// Like [`write_gate_line`] but per-column: write row `y` (one gate line, both area planes)
    /// pulling each pixel's level from `color(x, y)`. MSB plane in the `GCK`-high phase (2/3-area
    /// block), LSB plane in the `GCK`-low phase (1/3-area block).
    fn write_gate_line_with<F: Fn(u16, u16) -> (u8, u8, u8)>(&mut self, y: u16, color: &F) {
        self.gck.set_high();
        cpu_delay(GCK_SETTLE);
        self.shift_subline_with(y, true, color); // MSB → 2/3 block, GCK HIGH
        self.gen_pulse();
        self.gck.set_low();
        cpu_delay(GCK_SETTLE);
        self.shift_subline_with(y, false, color); // LSB → 1/3 block, GCK LOW
        self.gen_pulse();
    }

    /// Like [`fill_solid`] but **spatial**: every pixel's RGB222 level comes from `color(x, y)`
    /// (`x` = `0..240` column, `y` = `0..320` row). Same `INTB`/`GSP`/dummy frame envelope and
    /// `GCK`-level plane select — only the per-column data differs. This is the L4 path; e.g. a
    /// full 64-colour palette is `fill_with(|x, y| …)`.
    pub fn fill_with<F: Fn(u16, u16) -> (u8, u8, u8)>(&mut self, color: F) {
        self.intb.set_high();
        cpu_delay(FRAME_SETUP);
        self.gsp.set_high();
        cpu_delay(FRAME_SETUP);
        for i in 0..GATE_DUMMY_LEAD {
            self.dummy_advance(i == 0);
        }
        for y in 0..ROWS_PER_FRAME {
            self.write_gate_line_with(y, &color);
        }
        for _ in 0..GATE_DUMMY_TRAIL {
            self.dummy_advance(false);
        }
        self.gsp.set_low();
        self.intb.set_low();
    }
}

// ──────────────────────────── Shared test patterns (L3 / F3 / F4) ────────────────────────────
//
// The structured cards both bring-up bins put on glass: a 64-colour palette and a black-on-white
// shapes card, each an `fn(x, y) -> (u8, u8, u8)` of RGB222 levels (`0..=3` per channel). They
// live here (not in either bin) so the **M33-direct** path ([`PanelBus::fill_with`], the L3
// `ls021_bringup` bin) and the **FLPR-driven** path (the F4 `ls021_flpr_bringup` bin, which packs
// these into the RGB222 framebuffer) render the *same* source — that identity is exactly the F4
// on-glass verification ("visually identical to the #148 M33-direct captures").

/// The 64-colour test palette: an **8×8 grid** of every RGB222 value over the 240×320 panel.
/// `x`/`y` are pixel coordinates; the cell is `x/30` across (8 cells × 30 px = 240) and `y/40`
/// down (8 × 40 = 320). Cell index `0..63` packs as `r<<4 | g<<2 | b`, so columns step blue/green
/// and rows step red — every 2-bit-per-channel combination appears exactly once.
pub fn palette(x: u16, y: u16) -> (u8, u8, u8) {
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
pub fn shapes(x: u16, y: u16) -> (u8, u8, u8) {
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
