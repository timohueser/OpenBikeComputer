//! The board's two panel-power ports (#1515 D2): [`PanelBacklight`] and [`SystemOff`].
//!
//! One of them works. The other one says so.

use embassy_nrf::pac;
use obc_ports::{Backlight, BacklightUnsupported, PowerOff};

/// The board's [`Backlight`] — and it **refuses**.
///
/// This device drives a Sharp **LS021B7DD02**, a reflective memory-in-pixel LCD: it is lit by the
/// light already falling on it, and the board's pin ledger (`board.rs`, mirrored in the README pin
/// map) accounts for every P0/P1/P2 pad — the six source lines, the four gate/COM lines, the shared
/// microSD bus, the I²C sensors, the four buttons and the heartbeat LED. **None of them is a
/// light**, and the nPM1300 the power path is designed around is not wired yet either.
///
/// So there is nothing here to dim, and a stub that returned `Ok(())` would be a lie the rider
/// could read off the screen: the drawer's editor would move a slider that changes nothing while
/// the firmware reported success. It returns [`BacklightUnsupported`] instead, which the caller
/// logs once — and, because a refusal only the RTT log can see is not honesty either,
/// [`available`](Backlight::available) answers `false`, which takes the whole brightness control
/// off the drawer's root row. See the open hardware question on #1515.
///
/// **When a light does exist**, this is the whole seam: give the struct its PWM channel, map the
/// level to a duty cycle, and answer `true`. Nothing above it changes.
pub(crate) struct PanelBacklight;

impl Backlight for PanelBacklight {
    /// **No.** The app asks this once at boot and removes the quick drawer's brightness control,
    /// so the rider is not offered a slider with nothing behind it.
    fn available(&self) -> bool {
        false
    }

    fn apply(&mut self, level: u8) -> Result<(), BacklightUnsupported> {
        let _ = level;
        Err(BacklightUnsupported)
    }
}

/// The board's [`PowerOff`]: **system OFF**, the part's deepest state.
///
/// `REGULATORS.SYSTEMOFF` stops the CPU and every peripheral; RAM is not retained and the part
/// leaves this state only through a reset or a configured wake source, which is why the trait's
/// `power_off` cannot return. The `wfi` loop after the write is the datasheet's own belt: the write
/// may take a few cycles to take effect, and nothing must run in between.
///
/// The panel keeps the last frame it was given — a memory LCD holds its pixels with no power — so
/// the "POWERING OFF" sheet the caller presented is still what a rider sees on a dark device.
pub(crate) struct SystemOff;

impl PowerOff for SystemOff {
    fn power_off(&mut self) -> ! {
        defmt::info!("power: entering system OFF");
        pac::REGULATORS_S.systemoff().write(|w| w.set_systemoff(true));
        loop {
            cortex_m::asm::wfi();
        }
    }
}
