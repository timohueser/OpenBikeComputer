//! The simulator's two panel-power ports: [`SimBacklight`] and [`SimPowerOff`].
//!
//! Neither has hardware behind it here, so each is modelled where the rider can see it: a
//! brightness level scales the pixels the framebuffer is blitted with, and switching off ends the
//! process.

use obc_platform::backlight::{duty_permille, DUTY_FULL};
use obc_ports::{Backlight, BacklightUnsupported, PowerOff, BACKLIGHT_LEVELS};

/// The simulator's backlight: it darkens the blitted panel image rather than a lamp.
///
/// The levels have to be looked at on the 64-colour rendering before anyone can say whether the
/// dimmest is still readable, and a window that ignores the setting cannot answer that.
///
/// `--no-backlight` builds a platform that answers `false`, refuses every level and never scales
/// the image, so the three-control sheet a lightless host draws stays reviewable.
#[derive(Debug)]
pub struct SimBacklight {
    /// The level last applied, `0..BACKLIGHT_LEVELS`.
    level: u8,
    /// Whether this simulated platform has a light at all. `--no-backlight` clears it.
    available: bool,
}

impl SimBacklight {
    /// A backlight at full brightness, which is the factory level. `available` models the
    /// platform, and `false` is the lightless host.
    pub fn new(available: bool) -> Self {
        SimBacklight { level: BACKLIGHT_LEVELS - 1, available }
    }

    /// The 0 to 255 gain the current level blits at, or `None` at full brightness, where the panel
    /// image passes through untouched and the scaling pass is skipped. A platform with no light
    /// never scales.
    ///
    /// The ladder is the board's own, [`obc_platform::backlight`], so the window cannot review one
    /// curve while the hardware drives another. It is not applied raw: a duty cycle is linear in
    /// light, and the bytes this scales are sRGB, which a display turns back into light through
    /// about a 2.2 gamma. The duty therefore goes in through that gamma, and the window emits what
    /// the lamp would. Scaling by the raw per mille would blit near-black for the dimmest step.
    pub fn gain(&self) -> Option<u16> {
        (self.available && self.level + 1 < BACKLIGHT_LEVELS).then(|| {
            let duty = f64::from(duty_permille(self.level)) / f64::from(DUTY_FULL);
            (255.0 * duty.powf(1.0 / 2.2)).round() as u16
        })
    }
}

impl Backlight for SimBacklight {
    /// A window can always be drawn darker, so the simulator shows the same controls a lit device
    /// would. `--no-backlight` says otherwise, and the sheet loses its brightness control.
    fn available(&self) -> bool {
        self.available
    }

    fn apply(&mut self, level: u8) -> Result<(), BacklightUnsupported> {
        if !self.available {
            return Err(BacklightUnsupported);
        }
        self.level = level.min(BACKLIGHT_LEVELS - 1);
        Ok(())
    }
}

/// The simulator's power-off. The window's frame is already presented, so the honest ending is the
/// process ending.
#[derive(Debug, Default)]
pub struct SimPowerOff;

impl PowerOff for SimPowerOff {
    fn power_off(&mut self) -> ! {
        std::process::exit(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Full brightness costs the blit nothing, every dimmer level scales it, and the dimmest is
    /// still lit: no level blacks the panel out.
    ///
    /// The figures are the board's duty ladder seen through a 2.2 display gamma. They are written
    /// out rather than recomputed, so a change to the shared table has to be looked at here too.
    #[test]
    fn the_gain_is_skipped_at_full_and_never_reaches_zero() {
        let mut b = SimBacklight::new(true);
        assert_eq!(b.gain(), None, "the factory level passes the image through");
        let gains: Vec<u16> = (0..BACKLIGHT_LEVELS)
            .map(|l| {
                b.apply(l).expect("the simulator always has a panel");
                b.gain().unwrap_or(255)
            })
            .collect();
        assert_eq!(gains, [59, 111, 160, 208, 255]);
        assert!(gains[0] > 0, "the dimmest level is still lit");
        // An out-of-range level saturates rather than wrapping into darkness.
        b.apply(200).unwrap();
        assert_eq!(b.gain(), None);
    }

    /// With `--no-backlight` the port refuses and the window stops scaling the panel image, so the
    /// operator sees the arrangement a lightless platform gives.
    #[test]
    fn a_platform_with_no_light_refuses_and_never_scales() {
        let mut b = SimBacklight::new(false);
        assert!(!Backlight::available(&b), "the sheet drops its brightness control on this one");
        assert_eq!(b.apply(0), Err(BacklightUnsupported), "every level is refused");
        assert_eq!(b.gain(), None, "and the blit is never scaled, whatever the stored level says");
    }
}
