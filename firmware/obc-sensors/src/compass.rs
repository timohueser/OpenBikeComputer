//! Chip-agnostic electronic-compass maths: a 3-axis magnetometer sample to a heading in degrees
//! clockwise from north, for the [`CompassSource`](obc_ports::CompassSource) seam.
//!
//! Any driver, on any bus, reduces its reading to a [`MagSample`] (3 axes, µT, in the device
//! frame) and calls [`heading_deg`], so a chip change moves only the register map and scaling.
//!
//! Only the magnetometer's three axes are used, so the heading is computed flat, with the device
//! roughly level. That is enough for its one job: standing in for
//! [`Fix::course`](obc_ports::Fix::course) on a heading-up map while the rider is stopped. Tilt
//! compensation would be a new function taking an accel vector, not a change to this signature.
//!
//! [`heading_deg`] expects a sample already hard-iron-corrected and rotated into the device frame
//! (X forward, Y right, Z down); both are board-mounting concerns.

/// A single magnetometer reading: the three field-strength axes in microtesla, in the device
/// frame, where `x` is forward, `y` right and `z` down.
///
/// `z` is carried even though the flat heading ignores it, because a tilt-compensated heading
/// needs all three axes and a driver that already reads the full burst would otherwise have to be
/// re-plumbed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MagSample {
    /// Field along device-forward (top of screen), µT.
    pub x: f32,
    /// Field along device-right, µT.
    pub y: f32,
    /// Field along device-down, µT.
    pub z: f32,
}

impl MagSample {
    /// A sample from its three µT axes.
    #[inline]
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        MagSample { x, y, z }
    }
}

/// The flat magnetic heading of a [`MagSample`], in degrees clockwise from north, matching
/// [`Fix::course`](obc_ports::Fix::course) so the app can use either to orient a heading-up map.
///
/// With X forward, Y right and Z down, the heading of the forward axis is `atan2(-y, x)`: facing
/// north the horizontal field lies along +X, and rotating 90° clockwise shifts north to the
/// device's left.
///
/// `declination_deg` is the local magnetic declination, east-positive, converting magnetic north
/// to true north. The result is normalised to `[0, 360)`. Magnitude is irrelevant, so an
/// uncalibrated scale does not affect the angle, but a hard-iron offset must already be removed.
pub fn heading_deg(s: MagSample, declination_deg: f32) -> f32 {
    let deg = libm::atan2f(-s.y, s.x) * (180.0 / core::f32::consts::PI);
    normalize_deg(deg + declination_deg)
}

/// Wrap an angle in degrees into `[0, 360)`. A bounded add and subtract rather than a float modulo,
/// so it stays exact for the small out-of-range inputs [`heading_deg`] produces.
pub fn normalize_deg(mut deg: f32) -> f32 {
    while deg < 0.0 {
        deg += 360.0;
    }
    while deg >= 360.0 {
        deg -= 360.0;
    }
    deg
}

/// The smallest absolute angular distance between two headings, always in `[0, 180]`, taking the
/// short way round. A driver uses it to dead-band its output, so sensor noise while held still does
/// not repaint a heading-up map.
pub fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    if d < 0.0 {
        d = -d;
    }
    if d > 180.0 {
        360.0 - d
    } else {
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Allow a hair of float slop when comparing headings.
    fn close(a: f32, b: f32) {
        let d = (a - b).abs();
        assert!(d < 0.01 || (360.0 - d) < 0.01, "{a} vs {b}");
    }

    #[test]
    fn cardinal_headings() {
        // Field along +X (forward) → facing north.
        close(heading_deg(MagSample::new(20.0, 0.0, -40.0), 0.0), 0.0);
        // North shifted to the device's left is facing east.
        close(heading_deg(MagSample::new(0.0, -20.0, -40.0), 0.0), 90.0);
        // Field along −X → facing south.
        close(heading_deg(MagSample::new(-20.0, 0.0, -40.0), 0.0), 180.0);
        // North to the device's right is facing west.
        close(heading_deg(MagSample::new(0.0, 20.0, -40.0), 0.0), 270.0);
    }

    #[test]
    fn magnitude_does_not_change_the_angle() {
        // Scaling the whole horizontal vector leaves the heading put.
        let a = heading_deg(MagSample::new(12.0, 5.0, 0.0), 0.0);
        let b = heading_deg(MagSample::new(120.0, 50.0, 0.0), 0.0);
        close(a, b);
    }

    #[test]
    fn declination_shifts_and_wraps() {
        // Magnetic north + 10° east declination → true heading 10°.
        close(heading_deg(MagSample::new(20.0, 0.0, 0.0), 10.0), 10.0);
        // Wrap across 0: facing west (270°) with +100° declination → 370 → 10.
        close(heading_deg(MagSample::new(0.0, 20.0, 0.0), 100.0), 10.0);
    }

    #[test]
    fn normalize_wraps_both_ways() {
        close(normalize_deg(-90.0), 270.0);
        close(normalize_deg(450.0), 90.0);
        close(normalize_deg(0.0), 0.0);
        close(normalize_deg(359.999), 359.999);
    }

    #[test]
    fn angle_diff_takes_the_short_way() {
        close(angle_diff(10.0, 20.0), 10.0);
        close(angle_diff(20.0, 10.0), 10.0); // symmetric
        close(angle_diff(350.0, 10.0), 20.0); // wraps across 0
        close(angle_diff(10.0, 350.0), 20.0);
        close(angle_diff(0.0, 180.0), 180.0); // antipodal is the max
        close(angle_diff(90.0, 90.0), 0.0);
    }
}
