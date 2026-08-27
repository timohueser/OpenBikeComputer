//! The board's two panel-power ports (#1515 D2): [`PanelBacklight`] and [`SystemOff`].
//!
//! One dims the panel. The other one ends the ride.

use embassy_nrf::peripherals::{P1_27, PWM20};
use embassy_nrf::pwm::{DutyCycle, SimpleConfig, SimplePwm};
use embassy_nrf::{pac, Peri};
use obc_platform::backlight::duty_permille;
use obc_ports::{Backlight, BacklightUnsupported, PowerOff, BACKLIGHT_LEVELS};

/// The board's [`Backlight`]: **PWM20 channel 0 on P1.27**, at 1 kHz (#1558).
///
/// The panel is a Sharp **LS021B7DD02**, a reflective memory-in-pixel LCD — it has no light of its
/// own, so what this drives is the front light beside it. That light is **not fitted yet**: the pin
/// is provisional (see the README pin map), and on the DK the same net also drives the buffered
/// LED2, which is what makes the duty ladder visible on a desk. The eventual circuit is this pin
/// into a MOSFET gate, and a constant-current driver over I²C after that — both are a new impl of
/// this trait and nothing above it moves.
///
/// **The port is honest about the signal, not about the photons.** [`apply`](Backlight::apply)
/// really does change the duty cycle on a real pin, which is why it answers `Ok` and
/// [`available`](Backlight::available) answers `true`; the drawer therefore shows its brightness
/// control. A rider on a board with no lamp soldered on sees a control that changes nothing —
/// **that is a wiring gap, not a lie the firmware tells**, and it is the whole point of landing the
/// PWM seam before the lamp.
///
/// ## Two details worth knowing before changing this
///
/// * **The duty ladder is not here.** It is `obc_platform::backlight` — board-agnostic, so it can
///   be tested on the host, and so the later I²C driver drives the same five brightnesses.
/// * **Polarity is inverted, on purpose.** In embassy's PWM a *normal* duty value is the count the
///   line is held **low**; an *inverted* one is the count it is held **high**. The ladder is
///   written as brightness, so the compare value goes in as
///   [`DutyCycle::inverted`] and a per-mille of 1,000 is a line held high for the whole period.
pub(crate) struct PanelBacklight {
    pwm: SimplePwm<'static>,
}

impl PanelBacklight {
    /// Arm the PWM and light the panel at the **factory level**.
    ///
    /// The rider's persisted level lands a moment later, in the ride loop, as soon as the settings
    /// page has been read (`ride.rs`). Starting bright rather than dark is what keeps a boot that
    /// never gets that far — a card fault sheet, say — readable.
    ///
    /// [`SimpleConfig::default`] is exactly the configuration wanted and is therefore not
    /// overridden: countertop 1,000 (so a duty value is a per mille) with the 16 MHz PWM clock
    /// divided by 16, which is **1 kHz** — above anything a rider can see flicker in, below
    /// anything that would make a MOSFET's switching losses interesting. Standard drive, and the
    /// line idles low, so a disabled PWM is a dark lamp rather than a lit one.
    pub(crate) fn new(pwm: Peri<'static, PWM20>, pin: Peri<'static, P1_27>) -> Self {
        let mut backlight = PanelBacklight { pwm: SimplePwm::new_1ch(pwm, pin, &SimpleConfig::default()) };
        let _ = backlight.apply(BACKLIGHT_LEVELS - 1);
        backlight
    }
}

impl Backlight for PanelBacklight {
    /// **Yes** — there is a duty cycle to move. The app asks once at boot and keeps the quick
    /// drawer's brightness control on the root row.
    fn available(&self) -> bool {
        true
    }

    fn apply(&mut self, level: u8) -> Result<(), BacklightUnsupported> {
        self.pwm.set_duty(0, DutyCycle::inverted(duty_permille(level)));
        Ok(())
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
