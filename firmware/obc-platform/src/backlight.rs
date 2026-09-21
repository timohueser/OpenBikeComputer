//! The panel light's level-to-duty ladder: the one table a [`Backlight`](obc_ports::Backlight)
//! with real hardware behind it drives.
//!
//! It lives here rather than in a board crate because it is not a property of the nRF's PWM: a
//! constant-current driver sets the same five brightnesses through an I²C register and wants this
//! ladder unchanged. A board crate also cannot host a `#[test]`.
//!
//! The steps are not evenly spaced, because duty is linear in light and the eye is not. An even
//! ladder spends three of its five steps inside the top half of the range, where a rider can
//! barely tell them apart. The ladder is square-law, `duty ∝ (level + 1)²`, which spaces the
//! perceived steps nearly evenly and is exact integer arithmetic a reader can check.
//!
//! Per mille, because 1,000 is the PWM countertop the board configures, so the number in the table
//! is the compare value. The floor is 40 ‰ and never 0: a rider who cannot read the panel cannot
//! find the control that turns it back on.

use obc_ports::BACKLIGHT_LEVELS;

/// The countertop the ladder is written against. A duty of `DUTY_FULL` is a line held high.
pub const DUTY_FULL: u16 = 1_000;

/// Level → duty, in per mille of [`DUTY_FULL`]: `40 · (level + 1)²`.
pub const DUTY_PERMILLE: [u16; BACKLIGHT_LEVELS as usize] = [40, 160, 360, 640, DUTY_FULL];

/// The duty for `level`, saturating at the brightest step.
pub fn duty_permille(level: u8) -> u16 {
    DUTY_PERMILLE[level.min(BACKLIGHT_LEVELS - 1) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is the square law, so the documented curve and the shipped numbers cannot drift
    /// apart, and every step is brighter than the one below it.
    #[test]
    fn the_ladder_is_the_square_law_it_documents() {
        for (level, &duty) in DUTY_PERMILLE.iter().enumerate() {
            let step = level as u16 + 1;
            assert_eq!(duty, 40 * step * step, "level {level} is off the square law");
        }
        assert!(DUTY_PERMILLE.windows(2).all(|w| w[0] < w[1]), "the ladder must climb");
    }

    /// The two ends carry the contract: the dimmest step still drives the light and the brightest
    /// is the full period.
    #[test]
    fn the_floor_is_lit_the_top_is_full_and_a_stray_level_saturates() {
        assert!(duty_permille(0) > 0, "the dimmest level is still lit");
        assert_eq!(duty_permille(BACKLIGHT_LEVELS - 1), DUTY_FULL, "the brightest step is the whole period");
        assert_eq!(duty_permille(200), DUTY_FULL, "an out-of-range level saturates rather than wrapping dark");
    }
}
