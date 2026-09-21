//! Zero-CPU hardware COM driver for the LS021B7DD02: the `VCOM`, `VB` and `VA` square wave
//! generated in silicon by a TIMER, DPPI and GPIOTE toggle chain, so the panel's anti-DC-bias COM
//! keeps alternating with no M33 wakes.
//!
//! The Memory-in-Pixel cells must never see a DC bias, so the three lines have to alternate forever,
//! about 60 Hz at 50 % duty, the whole time the panel is powered. The M33 driver ([`crate::com`])
//! does that from a high-priority task that wakes the core about 120 times a second. Here a
//! free-running TIMER's compare event is published to a DPPI channel that three GPIOTE toggle tasks
//! subscribe to, so all three lines flip simultaneously in hardware. The TIMER, DPPI and GPIOTE all
//! run in System-ON sleep, so once armed the M33 never wakes for COM again.
//!
//! In embassy-nrf 0.11 only P0, through GPIOTE30, and P1 and P3, through GPIOTE20, are
//! GPIOTE-capable; P2 has no GPIOTE mapping. The board routes VCOM, VB and VA to P1.22 to P1.24,
//! and both this driver and the default M33 one own those nets. All three stay on GPIOTE20, so one
//! DPPI channel toggles them in lockstep.
//!
//! On-glass and logic-analyzer verification is still pending: this compiles for the target and the
//! wiring matches [`crate::com`]'s waveform, but the COM phase — the in-phase `VCOM` and `VB` pair
//! with an inverse `VA` — must be confirmed with an analyzer before relying on it. The COM
//! electrodes are a 56 to 77 nF load, so the three pins are driven high-drive.

use embassy_nrf::gpiote::OutputChannel;
use embassy_nrf::peripherals::{PPI20_CH0, TIMER21};
use embassy_nrf::ppi::Ppi;
use embassy_nrf::timer::{Frequency, Timer};
use embassy_nrf::Peri;

/// The armed hardware COM generator. It owns the TIMER, the DPPI channel and the three GPIOTE
/// output channels for the life of the program: dropping it would stop the toggle and let the panel
/// take a DC bias, so `main` holds it forever. Nothing touches it after [`HwCom::start`].
pub struct HwCom {
    _timer: Timer<'static>,
    _ppi: Ppi<'static, PPI20_CH0, 1, 3>,
    /// `[VCOM, VB, VA]`, held only to keep the GPIOTE channels configured, because the DPPI tasks
    /// reference them. Never touched after `start`.
    _channels: [OutputChannel<'static>; 3],
}

impl HwCom {
    /// Arm the hardware COM wave and start it free-running. The three channels are built in `main`
    /// from the COM pins, in toggle polarity and all three at `Level::Low`, so COM is held low
    /// through the panel's power-on init-black frame.
    ///
    /// Call this after that init frame: it drives `va` high once to establish the inverse phase,
    /// then enables the toggle, so from here `VCOM` and `VB` are in phase and `VA` is their exact
    /// inverse, and every hardware toggle preserves that. The timer runs at 1 MHz with `CC[0]` at
    /// [`COM_HALF_PERIOD_US`](crate::com::COM_HALF_PERIOD_US) and an auto-reload short.
    pub fn start(
        timer: Peri<'static, TIMER21>,
        ppi_ch: Peri<'static, PPI20_CH0>,
        vcom: OutputChannel<'static>,
        vb: OutputChannel<'static>,
        va: OutputChannel<'static>,
    ) -> Self {
        let timer = Timer::new(timer);
        timer.set_frequency(Frequency::F1MHz); // 1 µs/tick → CC in microseconds
        let cc = timer.cc(0);
        cc.write(crate::com::COM_HALF_PERIOD_US as u32); // 8333 µs = 60.0 Hz, 50 % duty
        cc.short_compare_clear(); // COMPARE0 → CLEAR: free-running auto-reload, no CPU
        let event = cc.event_compare();
        // Establish the inverse phase: VCOM and VB stay low at their init level and VA is forced
        // high, so the pair and VA start exactly antiphase.
        va.set();
        // One DPPI channel, with the compare event published to all three GPIOTE toggle tasks, so
        // every half period the three lines flip simultaneously in hardware.
        let tasks = [vcom.task_out(), vb.task_out(), va.task_out()];
        let mut ppi = Ppi::new_many_to_many(ppi_ch, [event], tasks);
        ppi.enable();
        timer.start();
        HwCom { _timer: timer, _ppi: ppi, _channels: [vcom, vb, va] }
    }
}
