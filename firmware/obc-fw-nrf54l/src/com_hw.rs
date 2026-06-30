//! **Zero-CPU hardware COM driver** for the LS021B7DD02 (issue #219) — the `VCOM`/`VB`/`VA` square
//! wave generated entirely in silicon by a **TIMER → DPPI → GPIOTE** toggle chain, so the panel's
//! anti-DC-bias COM keeps alternating with **no M33 wakes** and the core can WFI between real events.
//!
//! ## Why this exists
//!
//! The Memory-in-Pixel cells must never see a DC bias, so `VCOM`/`VB`/`VA` have to alternate forever
//! (~60 Hz, ~50 % duty) the whole time the panel is powered. The bring-up driver ([`crate::com`])
//! does that from the M33 — a high-priority task toggling three GPIOs every half period — which wakes
//! the core ~120×/s **regardless** of the event-driven loop (issue #219), capping the idle-power win.
//! This driver moves the generation off-core: a free-running TIMER's compare event is published to a
//! DPPI channel that three GPIOTE **toggle** tasks subscribe to, so on every half period all three
//! lines flip simultaneously in hardware. The TIMER + DPPI + GPIOTE all run in System-ON sleep, so
//! once it's armed the M33 never has to wake for COM again — the lever that lets a parked device WFI.
//!
//! ## Why GPIOTE works where PWM didn't
//!
//! L1 proved **PWM20 will not drive** the COM pins (the lines sat dead `Lo`); a plain `gpio::Output`
//! on the same pins toggles cleanly, which is what [`crate::com`] relies on. GPIOTE drives the GPIO
//! through the dedicated GPIOTE→GPIO task path (the same one a plain `Output` write uses), not the
//! PWM output routing that failed — so a GPIOTE **toggle task** reaches the pin where PWM could not.
//!
//! ## Pins — must be GPIOTE-capable (P1/P3, or P0)
//!
//! In embassy-nrf 0.11 **only P0 (→GPIOTE30) and P1/P3 (→GPIOTE20) are GPIOTE-capable; P2 has no
//! GPIOTE mapping** (on either the nRF54L15 or the nRF54LM20). The DK wires COM on **P2** (a bring-up
//! artifact — COM is a slow wave that never needed P2's fast domain), so this driver **cannot** run
//! there and the DK keeps [`crate::com`]. It is the path the **production board** turns on (the
//! `com-hw` feature): route the three COM lines onto P1/P3 and this drives them for free. Keep all
//! three on **one** GPIOTE instance (one power domain) so a single DPPI channel toggles them in
//! lockstep — here GPIOTE20 (P1/P3) + TIMER21 + DPPIC20, all in the peripheral domain.
//!
//! ⚠️ **On-glass + logic-analyzer verification pending** (issue #219): this compiles for the target
//! and the wiring is golden-ref'd against [`crate::com`]'s waveform, but it has not yet run on glass
//! (no GPIOTE-COM board exists yet — the DK is P2-bound, the production board isn't fabbed). The COM
//! phase (the in-phase `VCOM`/`VB` pair + inverse `VA`) must be confirmed with the LA before relying
//! on it. The COM electrodes are a 56–77 nF load, so the three pins are driven **high-drive (H0H1)**.

use embassy_nrf::gpiote::OutputChannel;
use embassy_nrf::peripherals::{PPI20_CH0, TIMER21};
use embassy_nrf::ppi::Ppi;
use embassy_nrf::timer::{Frequency, Timer};
use embassy_nrf::Peri;

/// The armed hardware COM generator. Owns the TIMER, the DPPI channel, and the three GPIOTE output
/// channels for the life of the program — dropping it would stop the toggle and let the panel
/// DC-bias, so `main` holds it forever (like the M33 driver's task owning its pins). Built and
/// started by [`HwCom::start`]; nothing touches it afterwards — the hardware free-runs.
pub struct HwCom {
    _timer: Timer<'static>,
    _ppi: Ppi<'static, PPI20_CH0, 1, 3>,
    /// `[VCOM, VB, VA]` — held only to keep the GPIOTE channels configured (the DPPI tasks reference
    /// them); never touched after `start`.
    _channels: [OutputChannel<'static>; 3],
}

impl HwCom {
    /// Arm the hardware COM wave and start it free-running. `vcom` / `vb` / `va` are GPIOTE output
    /// channels already built in `main` from the COM pins (so the GpiotePin↔channel-instance pairing
    /// is checked at the call site) in [`OutputChannelPolarity::Toggle`](embassy_nrf::gpiote::OutputChannelPolarity::Toggle),
    /// **all three with an initial [`Level::Low`](embassy_nrf::gpio::Level)** — so COM is held `Lo`
    /// through the panel's power-on init-black frame, exactly like the M33 driver booting its pins
    /// `Output(Lo)`. Call this **after** that init frame: it drives `va` high once (the SET task) to
    /// establish the inverse phase, then enables the toggle — so from here `VCOM`/`VB` are in phase
    /// and `VA` is their exact inverse, and every hardware toggle preserves that forever. `timer` runs
    /// at 1 MHz with `CC[0]` = [`COM_HALF_PERIOD_US`](crate::com::COM_HALF_PERIOD_US) and an
    /// auto-reload short, so its compare event fires every half period; the DPPI channel publishes
    /// that event to all three toggle tasks at once.
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
        // Establish the inverse phase: VCOM/VB stay Lo (their init level), VA is forced Hi, so the
        // pair and VA start exactly antiphase. The synchronous toggle below then holds that forever.
        va.set();
        // One DPPI channel, the compare event published to all three GPIOTE toggle tasks: every half
        // period the three lines flip simultaneously in hardware.
        let tasks = [vcom.task_out(), vb.task_out(), va.task_out()];
        let mut ppi = Ppi::new_many_to_many(ppi_ch, [event], tasks);
        ppi.enable();
        timer.start();
        HwCom { _timer: timer, _ppi: ppi, _channels: [vcom, vb, va] }
    }
}
