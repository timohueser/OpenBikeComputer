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
#[derive(Debug, Default)]
pub struct SimBacklight {
    /// The level last applied, `0..BACKLIGHT_LEVELS`.
    level: u8,
}

impl SimBacklight {
    /// A backlight at full brightness — the factory level.
    pub fn new() -> Self {
        SimBacklight { level: BACKLIGHT_LEVELS - 1 }
    }

    /// The 0–255 gain the current level blits at, or `None` at full brightness — where the panel
    /// image is passed through untouched and the whole scaling pass is skipped.
    pub fn gain(&self) -> Option<u16> {
        (self.level + 1 < BACKLIGHT_LEVELS).then(|| 255 * (self.level as u16 + 1) / BACKLIGHT_LEVELS as u16)
    }
}

impl Backlight for SimBacklight {
    /// **Yes.** A window can always be drawn darker, so the simulator shows the rider the same four
    /// controls a lit device would — which is what makes the five levels reviewable at all.
    fn available(&self) -> bool {
        true
    }

    fn apply(&mut self, level: u8) -> Result<(), BacklightUnsupported> {
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
        let mut b = SimBacklight::new();
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
}
