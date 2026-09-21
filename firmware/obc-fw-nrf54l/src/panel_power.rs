//! The board's two panel-power ports: [`PanelBacklight`] and [`SystemOff`].

use embassy_nrf::peripherals::{P1_27, PWM20};
use embassy_nrf::pwm::{DutyCycle, SimpleConfig, SimplePwm};
use embassy_nrf::{pac, Peri};
use obc_platform::backlight::duty_permille;
use obc_ports::{Backlight, BacklightUnsupported, PowerOff, BACKLIGHT_LEVELS};

/// The board's [`Backlight`]: PWM20 channel 0 on P1.27, at 1 kHz.
///
/// The panel is a reflective memory-in-pixel LCD with no light of its own, so what this drives is
/// the front light beside it. That light is not fitted yet: the pin is provisional, and on the DK
/// the same net drives the buffered LED2. The eventual circuit is this pin into a MOSFET gate, and a
/// constant-current driver over I²C after that; both are a new impl of this trait.
///
/// The port is honest about the signal, not about the photons. [`apply`](Backlight::apply) really
/// does change the duty cycle on a real pin, which is why it answers `Ok` and
/// [`available`](Backlight::available) answers `true`, so the drawer shows its brightness control.
/// On a board with no lamp soldered on that control changes nothing, which is a wiring gap.
///
/// Two details worth knowing before changing this. The duty ladder is not here: it is
/// `obc_platform::backlight`, so it can be tested on the host and the later I²C driver drives the
/// same five brightnesses. And the polarity is inverted on purpose: in embassy's PWM a normal duty
/// value is the count the line is held low, and an inverted one the count it is held high, while the
/// ladder is written as brightness.
pub(crate) struct PanelBacklight {
    pwm: SimplePwm<'static>,
}

impl PanelBacklight {
    /// Arm the PWM and light the panel at the factory level.
    ///
    /// The rider's persisted level lands in the ride loop, once the settings page has been read.
    /// That is not a few instructions later: display bring-up, the card mount, the flat-store open,
    /// the USB and BLE spawns and a store lock all sit between the two, so a lamp would run at full
    /// brightness for the whole of bring-up. The trade is deliberate, because a boot that faults
    /// before the settings read still has a readable fault sheet, but it is a visible flash.
    ///
    /// [`SimpleConfig::default`] is exactly the configuration wanted: a count-top of 1,000, so a
    /// duty value is a per mille, with the 16 MHz PWM clock divided by 16, which is 1 kHz — above
    /// anything a rider can see flicker in, below anything that would make a MOSFET's switching
    /// losses interesting. From here the line idles low, so a disabled PWM is a dark lamp.
    ///
    /// The frequency is load-bearing for the loop, not only for the lamp. `set_duty` ends in a busy
    /// wait on `SEQEND`, about one PWM period, so 1 ms here. The change gate in `ride.rs` keeps that
    /// off the per-pass path. Drop to 200 Hz for a slower MOSFET and the same line becomes a 5 ms
    /// stall in the loop that feeds the watchdog.
    ///
    /// Before this call the pad is high-impedance, not low: reset leaves P1.27 an input with no
    /// pull, and nothing drives it in `obc-boot`, which covers the whole DFU install window.
    /// Dropping a [`SimplePwm`] returns the pin to high-Z too. On the DK the buffered LED2 makes
    /// that harmless; on the shipping board the MOSFET gate needs a pull-down.
    pub(crate) fn new(pwm: Peri<'static, PWM20>, pin: Peri<'static, P1_27>) -> Self {
        let mut backlight = PanelBacklight { pwm: SimplePwm::new_1ch(pwm, pin, &SimpleConfig::default()) };
        let _ = backlight.apply(BACKLIGHT_LEVELS - 1);
        backlight
    }
}

impl Backlight for PanelBacklight {
    /// There is a duty cycle to move. The app asks once at boot and keeps the quick drawer's
    /// brightness control on the root row.
    fn available(&self) -> bool {
        true
    }

    fn apply(&mut self, level: u8) -> Result<(), BacklightUnsupported> {
        self.pwm.set_duty(0, DutyCycle::inverted(duty_permille(level)));
        Ok(())
    }
}

/// The board's [`PowerOff`]: system OFF, the part's deepest state.
///
/// `REGULATORS.SYSTEMOFF` stops the CPU and every peripheral, RAM is not retained, and the part
/// leaves this state only through a reset or a configured wake source, which is why the trait's
/// `power_off` cannot return. The `wfi` loop after the write is the datasheet's own belt: the write
/// may take a few cycles to take effect, and nothing must run in between.
///
/// The panel keeps the last frame it was given, because a memory LCD holds its pixels with no
/// power, so the sheet the caller presented is what a rider sees on a dark device.
///
/// The backlight pin is not driven low first, and nothing here owns the port to do it with. The PWM
/// stops wherever its waveform left P1.27, so a fitted lamp could hold a switched-off device lit.
/// The fix when the lamp lands is to drive the line low, or to let the gate pull-down do it.
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
