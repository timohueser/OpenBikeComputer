//! LED0 blink codes — the bootloader's entire UI (no display, ever). Patterns are documented in
//! the README's LED table; keep the two in sync. Everything is a crude `asm::delay` busy-wait at
//! the default 64 MHz boot clock — the bootloader has no timers on purpose.

use cortex_m::asm;
use embassy_nrf::gpio::Output;

/// Cycles per millisecond at the default 64 MHz boot clock (the app raises itself to 128 MHz
/// only after the jump, so every delay in this crate runs at 64 MHz).
const CYCLES_PER_MS: u32 = 64_000;

/// Busy-wait `ms` milliseconds.
pub fn delay_ms(ms: u32) {
    // Max backoff is 8000 ms → 512 M cycles, comfortably inside u32.
    asm::delay(ms * CYCLES_PER_MS);
}

/// LED0 (P2_09, active-HIGH — the DK's on-board LED, the same one the app blinks per frame).
pub struct Led {
    pin: Output<'static>,
}

impl Led {
    /// Wrap the already-configured pin (constructed in `main` so the pin choice lives there).
    pub fn new(pin: Output<'static>) -> Led {
        Led { pin }
    }

    pub fn off(&mut self) {
        self.pin.set_low();
    }

    /// Heartbeat toggle — the install engine's progress hook drives this at a phase-dependent
    /// cadence (slow while verifying, fast while flashing).
    pub fn toggle(&mut self) {
        self.pin.toggle();
    }

    /// One solid pulse (the proof-of-life blink on entry).
    pub fn pulse_ms(&mut self, ms: u32) {
        self.pin.set_high();
        delay_ms(ms);
        self.pin.set_low();
    }

    /// `n` short blinks — the counted error codes (2 = staged image invalid, 3 = SD trouble).
    pub fn blink_code(&mut self, n: u32) {
        for _ in 0..n {
            self.pin.set_high();
            delay_ms(120);
            self.pin.set_low();
            delay_ms(180);
        }
    }

    /// SOS (· · · — — — · · ·), forever — the fatal-readback halt. The state page still holds
    /// the `Armed` record, so a power cycle retries the whole install from scratch; staying
    /// parked here (instead of resetting) avoids a silent reset storm hammering the card.
    pub fn sos_forever(&mut self) -> ! {
        loop {
            for &(on, off) in &[(150u32, 150u32); 3] {
                self.pin.set_high();
                delay_ms(on);
                self.pin.set_low();
                delay_ms(off);
            }
            for _ in 0..3 {
                self.pin.set_high();
                delay_ms(450);
                self.pin.set_low();
                delay_ms(150);
            }
            for _ in 0..3 {
                self.pin.set_high();
                delay_ms(150);
                self.pin.set_low();
                delay_ms(150);
            }
            delay_ms(1200);
        }
    }
}
