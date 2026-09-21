//! Panel keep-alive across an install: the parked LS021 pins and a software COM wave.
//!
//! The app leaves a painted frame on the Memory-in-Pixel panel. The panel holds that frame only
//! if nothing clocks garbage into it and the COM lines keep alternating. A MIP cell must never
//! see a DC bias, so `VCOM`/`VB` (in phase) and `VA` (their inverse) must keep flipping at
//! ~60 Hz for as long as the panel has power. The normative waveform is in
//! `obc-fw-nrf54l/src/com.rs`; this module copies it.
//!
//! The bootloader parks the gate and source lines as driven-low outputs (the pins live in
//! `main.rs`) and flips the COM lines from [`Com::poll`], which measures the half period with
//! DWT cycle counts because this crate has no executor and no timers. If the DWT does not
//! count, `poll` sees no elapsed time and the lines stay parked low: degraded, never a hang.
//! The `Idle` fast path builds none of this.

use cortex_m::asm;
use embassy_nrf::gpio::Output;

/// Half of the ~60 Hz COM period in cycles at the 64 MHz boot clock. 8333 µs is 60.0 Hz at
/// 50 % duty, inside the datasheet 54–66 Hz window.
const HALF_PERIOD_CYCLES: u32 = 8_333 * 64;

/// Cycles per millisecond at the 64 MHz boot clock.
const CYCLES_PER_MS: u32 = 64_000;

/// The free-running software COM wave. It owns the three COM pins for the life of the slow
/// path; dropping it before the app jump returns them to their reset state.
pub struct Com {
    vcom: Output<'static>,
    vb: Output<'static>,
    va: Output<'static>,
    /// Wave phase: `true` = `VCOM`/`VB` high, `VA` low.
    high: bool,
    /// CYCCNT at the last flip.
    last: u32,
}

impl Com {
    /// Wrap the three COM pins. `main` makes them high-drive outputs at `Level::Low`, because
    /// the COM electrodes are a 56–77 nF load. The wave starts one half period later.
    pub fn start(vcom: Output<'static>, vb: Output<'static>, va: Output<'static>) -> Com {
        Com { vcom, vb, va, high: false, last: cortex_m::peripheral::DWT::cycle_count() }
    }

    /// Flip to the next COM phase if a half period has elapsed. The anchor resets to now on
    /// each flip, so a late poll stretches one half period instead of compressing the next. The
    /// long-run duty stays near 50 %, which is the actual anti-DC-bias requirement.
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

    /// Busy-wait `ms` milliseconds in 1 ms slices and poll the wave between slices. Everything
    /// past the fast path waits through here.
    pub fn delay_ms(&mut self, ms: u32) {
        for _ in 0..ms {
            asm::delay(CYCLES_PER_MS);
            self.poll();
        }
    }
}
