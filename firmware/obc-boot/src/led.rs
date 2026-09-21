//! LED0 blink codes, the bootloader's only UI. The README holds the LED table; keep the two in
//! sync. All delays are busy-waits at the 64 MHz boot clock, because the bootloader has no
//! timers. Past the fast path they run through [`Com::delay_ms`], which keeps the panel COM wave
//! alternating.

use cortex_m::asm;
use embassy_nrf::gpio::Output;

use crate::com::Com;

/// Cycles per millisecond at the 64 MHz boot clock.
const CYCLES_PER_MS: u32 = 64_000;

/// Busy-wait without the COM wave. Only for the proof-of-life pulse before the fast path.
pub fn delay_ms(ms: u32) {
    // Max backoff is 8000 ms → 512 M cycles, comfortably inside u32.
    asm::delay(ms * CYCLES_PER_MS);
}

/// LED0 (P2_09, active-HIGH).
pub struct Led {
    pin: Output<'static>,
}

impl Led {
    pub fn new(pin: Output<'static>) -> Led {
        Led { pin }
    }

    pub fn off(&mut self) {
        self.pin.set_low();
    }

    pub fn toggle(&mut self) {
        self.pin.toggle();
    }

    pub fn pulse_ms(&mut self, ms: u32) {
        self.pin.set_high();
        delay_ms(ms);
        self.pin.set_low();
    }

    /// `n` short blinks: 2 = staged image invalid, 3 = SD trouble.
    pub fn blink_code(&mut self, n: u32, com: &mut Com) {
        for _ in 0..n {
            self.pin.set_high();
            com.delay_ms(120);
            self.pin.set_low();
            com.delay_ms(180);
        }
    }

    /// SOS forever: the fatal-readback halt. The state page keeps the `Armed` record, so a
    /// power cycle retries the install. The park replaces a reset, which would hammer the card
    /// in a loop. `keep_alive` runs once per cycle to pet a watchdog adopted from the arm's
    /// warm reset.
    pub fn sos_forever(&mut self, com: &mut Com, mut keep_alive: impl FnMut()) -> ! {
        loop {
            keep_alive();
            for &(on, off) in &[(150u32, 150u32); 3] {
                self.pin.set_high();
                com.delay_ms(on);
                self.pin.set_low();
                com.delay_ms(off);
            }
            for _ in 0..3 {
                self.pin.set_high();
                com.delay_ms(450);
                self.pin.set_low();
                com.delay_ms(150);
            }
            for _ in 0..3 {
                self.pin.set_high();
                com.delay_ms(150);
                self.pin.set_low();
                com.delay_ms(150);
            }
            com.delay_ms(1200);
        }
    }
}
