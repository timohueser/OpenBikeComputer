//! The simulator's two panel-power ports (#1515 D2): [`SimBacklight`] and [`SimPowerOff`].
//!
//! Neither has hardware behind it here, so each is modelled where the rider can actually see it: a
//! brightness level scales the pixels the framebuffer is blitted with, and switching off ends the
//! process.

use obc_ports::{Backlight, BacklightUnsupported, PowerOff, BACKLIGHT_LEVELS};

/// The simulator's backlight: it darkens the blitted panel image rather than a lamp.
///
/// This is not decoration. The five levels have to be *looked at* on the 64-colour rendering
/// before anyone can say whether five is the right number and whether the dimmest is still
/// readable, and a window that ignores the setting cannot answer that.
///
/// It also models the **other** platform. `--no-backlight` builds one that answers `false`, refuses
/// every level and never scales the image, so the three-control sheet a lightless host draws stays
/// reviewable in the window. That is no longer the board — it drives a PWM backlight since #1558.
#[derive(Debug)]
pub struct SimBacklight {
    /// The level last applied, `0..BACKLIGHT_LEVELS`.
    level: u8,
    /// Whether this simulated platform has a light at all (`--no-backlight` clears it).
    available: bool,
}

impl SimBacklight {
    /// A backlight at full brightness — the factory level. `available` models the platform: `false`
    /// is the board's lightless panel.
    pub fn new(available: bool) -> Self {
        SimBacklight { level: BACKLIGHT_LEVELS - 1, available }
    }

    /// The 0–255 gain the current level blits at, or `None` at full brightness — where the panel
    /// image is passed through untouched and the whole scaling pass is skipped. A platform with no
    /// light never scales: there is nothing driving a level.
    pub fn gain(&self) -> Option<u16> {
        (self.available && self.level + 1 < BACKLIGHT_LEVELS)
            .then(|| 255 * (self.level as u16 + 1) / BACKLIGHT_LEVELS as u16)
    }
}

impl Backlight for SimBacklight {
    /// A window can always be drawn darker, so the simulator normally shows the rider the same four
    /// controls a lit device would — which is what makes the five levels reviewable at all.
    /// `--no-backlight` says otherwise, and then the sheet loses its brightness control.
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

/// The simulator's power-off: the window's frame is already presented, so the honest ending is the
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

    /// Full brightness costs the blit nothing; every dimmer level scales it, and the dimmest is
    /// still lit — there is no level that blacks the panel out.
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
        assert_eq!(gains, [51, 102, 153, 204, 255]);
        assert!(gains[0] > 0, "the dimmest level is still lit");
        // An out-of-range level saturates rather than wrapping into darkness.
        b.apply(200).unwrap();
        assert_eq!(b.gain(), None);
    }

    /// `--no-backlight` models the board: the port refuses, and the window stops scaling the panel
    /// image — so what the operator looks at is the arrangement the hardware really gives.
    #[test]
    fn a_platform_with_no_light_refuses_and_never_scales() {
        let mut b = SimBacklight::new(false);
        assert!(!Backlight::available(&b), "the sheet drops its brightness control on this one");
        assert_eq!(b.apply(0), Err(BacklightUnsupported), "every level is refused");
        assert_eq!(b.gain(), None, "and the blit is never scaled, whatever the stored level says");
    }
}
