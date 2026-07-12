//! Panel keep-alive across an install — the parked LS021 pins + the software COM wave.
//!
//! The app paints a static "Installing update" frame as its last act before the arm's warm
//! reset (`obc-fw-nrf54l/src/ride.rs`, the install drain); the Memory-in-Pixel panel then
//! *holds* that frame with no scan at all — but only if nothing clocks garbage into it and
//! the COM lines keep alternating. A MIP cell must never see a **DC bias**, so `VCOM`/`VB`
//! (in phase) and `VA` (their exact inverse) have to keep flipping at ~60 Hz the whole time
//! the panel is powered — the normative waveform lives in `obc-fw-nrf54l/src/com.rs`, which
//! this module copies (deliberate duplication, the same policy as the SD constants in
//! `sd.rs`). Before this module, every install left the wave frozen at whatever level the
//! reset caught it on, for the whole multi-ten-second flash — a sustained DC stress the
//! panel driver's own contract forbids. So on every non-fast-path entry the bootloader:
//!
//! - **parks** the panel's gate + source lines as driven-low outputs (the pin array in
//!   `main.rs` — pin choices live there, like the LED's), so a floating `GCK`/`INTB` can't
//!   advance the gate or latch garbage rows while the app slot is being rewritten, and
//! - free-runs the **COM wave** in software: [`Com::poll`] flips the three lines whenever a
//!   half period has elapsed on the DWT cycle counter, called from the sliced delays
//!   ([`Com::delay_ms`]) and the install engine's per-chunk progress hook — the same
//!   chokepoints that pet the watchdog.
//!
//! The crate's "no executor, no timers" constraint holds: pacing is CYCCNT deltas over
//! `cortex_m::asm::delay` slices, no interrupt sources, nothing that can panic. With the
//! DWT not counting (its enable failed), `poll` sees no elapsed time and the wave simply
//! stays parked low — degraded to the pre-module behavior, never a hang. The plain `Idle`
//! fast path never constructs any of this: a normal boot leaves the panel pins exactly as
//! reset left them, and the app brings the panel up itself.

use cortex_m::asm;
use embassy_nrf::gpio::Output;

/// Half of the ~60 Hz COM period in CPU cycles at the 64 MHz boot clock: the app's
/// `COM_HALF_PERIOD_US` (8333 µs — 60.0 Hz, 50 % duty, inside the datasheet 54–66 Hz /
/// 48–52 % window) × 64 cycles/µs.
const HALF_PERIOD_CYCLES: u32 = 8_333 * 64;

/// Cycles per millisecond at the 64 MHz boot clock (mirrors `led.rs` — kept local so the
/// two crude delay paths stay independently readable).
const CYCLES_PER_MS: u32 = 64_000;

/// The free-running software COM wave. Owns the three COM pins for the life of the
/// bootloader's slow path — dropping it (the `boot` return before the app jump) puts the
/// pins back to their reset state, exactly like every other pin the bootloader touched,
/// and the app's own COM driver takes over from its usual boot-`Lo` state.
pub struct Com {
    vcom: Output<'static>,
    vb: Output<'static>,
    va: Output<'static>,
    /// Wave phase: `true` = `VCOM`/`VB` high, `VA` low.
    high: bool,
    /// CYCCNT at the last flip — the half-period anchor.
    last: u32,
}

impl Com {
    /// Wrap the three COM pins (constructed in `main` as **high-drive** outputs, all
    /// `Level::Low` — the held-`Lo` init state the app's driver also boots with; the COM
    /// electrodes are a 56–77 nF load). The wave starts on the first [`poll`](Com::poll)
    /// a half period later; until then the lines hold `Lo`, matching the app before its
    /// COM task spawns.
    pub fn start(vcom: Output<'static>, vb: Output<'static>, va: Output<'static>) -> Com {
        Com { vcom, vb, va, high: false, last: cortex_m::peripheral::DWT::cycle_count() }
    }

    /// Flip to the next COM phase if a half period has elapsed — one `wrapping_sub` and at
    /// most three register writes, cheap enough to call per 4 KB install chunk and per
    /// delay slice. The anchor resets to *now* on each flip (never catch-up bursts): a
    /// late poll stretches one half period rather than compressing the next, and the
    /// long-run duty stays ~50 % — the actual anti-DC-bias requirement.
    pub fn poll(&mut self) {
        let now = cortex_m::peripheral::DWT::cycle_count();
        if now.wrapping_sub(self.last) < HALF_PERIOD_CYCLES {
            return;
        }
        self.last = now;
        self.high = !self.high;
        if self.high {
            self.vcom.set_high();
            self.vb.set_high();
            self.va.set_low();
        } else {
            self.vcom.set_low();
            self.vb.set_low();
            self.va.set_high();
        }
    }

    /// Busy-wait `ms` milliseconds in 1 ms slices, polling the wave between slices — the
    /// COM-keeping replacement for `led::delay_ms` everywhere past the fast path (the SD
    /// backoffs and the blink patterns route their waits through here).
    pub fn delay_ms(&mut self, ms: u32) {
        for _ in 0..ms {
            asm::delay(CYCLES_PER_MS);
            self.poll();
        }
    }
}
