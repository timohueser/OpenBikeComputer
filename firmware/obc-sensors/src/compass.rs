//! Chip-agnostic **electronic-compass** maths — a 3-axis magnetometer sample → a heading in degrees
//! clockwise from north, for the [`CompassSource`](obc_ports::CompassSource) seam (the heading when
//! stopped).
//!
//! Chip-agnostic on purpose: the current bring-up reads the AK09916 inside an ICM-20948
//! ([`crate::icm20948`]), but the shipping board may carry a plain 3-axis magnetometer. Any driver,
//! on any bus, reduces its reading to a [`MagSample`] (3 axes, µT, in the **device frame**) and
//! calls [`heading_deg`]; when the chip changes only the register map + raw→µT scaling move.
//!
//! ## Scope: flat heading only
//! Uses **only** the magnetometer's three axes (no accelerometer/gyro), so the heading is computed
//! *flat* (device roughly level). Enough for its one job: standing in for
//! [`Fix::course`](obc_ports::Fix::course) on a heading-up map while the rider is stopped. Tilt
//! compensation is a deliberate non-goal; adding it later is a new function taking an accel vector,
//! not a change to this signature.
//!
//! ## Calibration is the caller's job
//! [`heading_deg`] expects a sample **already** hard-iron-corrected and rotated into the device
//! frame (X forward / Y right / Z down) — both are board-mounting concerns living in the board
//! crate. This module is pure geometry: `atan2` of two axes plus a declination shift.

/// A single magnetometer reading: the three field-strength axes in **microtesla**, in the **device
/// frame** — `x` forward (top of screen), `y` right, `z` down. A driver scales raw counts to µT and
/// remaps the sensor's own axes into this frame before building one.
///
/// `z` is carried even though the flat heading ignores it: it's what an overflow check reads and
/// what a future tilt-compensated heading would consume.
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

/// The **flat** magnetic heading of a [`MagSample`], in degrees clockwise from north (`0` = north,
/// `90` = east) — matching [`Fix::course`](obc_ports::Fix::course) so the app can use either
/// interchangeably to orient a heading-up map.
///
/// With the device-frame convention (X forward, Y right, Z down) the heading of the forward axis is
/// `atan2(-y, x)`: facing north the horizontal field lies along +X (`y≈0` → `0°`); rotate the device
/// 90° clockwise to face east and north shifts to the device's left (`-y` → `+90°`).
///
/// `declination_deg` is the local **magnetic declination** (east-positive) converting magnetic north
/// to the *true* north the map and GPS course use. Pass `0.0` for raw magnetic heading.
///
/// The result is normalised to `[0, 360)`. Magnitude is irrelevant (only the field *direction*
/// matters), so an uncalibrated scale doesn't affect the angle — but a **hard-iron offset must
/// already be removed** by the caller, or the heading skews.
pub fn heading_deg(s: MagSample, declination_deg: f32) -> f32 {
    let deg = libm::atan2f(-s.y, s.x) * (180.0 / core::f32::consts::PI);
    normalize_deg(deg + declination_deg)
}

/// Wrap an angle in degrees into `[0, 360)`. A bounded `+= 360` / `-= 360` rather than a float modulo
/// so it stays exact for the small out-of-range inputs [`heading_deg`] produces and pulls in no
/// extra `libm`.
pub fn normalize_deg(mut deg: f32) -> f32 {
    while deg < 0.0 {
        deg += 360.0;
    }
    while deg >= 360.0 {
        deg -= 360.0;
    }
    deg
}

/// The smallest absolute angular distance between two headings in degrees — always in `[0, 180]`,
/// taking the short way around the circle (so `350°` and `10°` are `20°` apart, not `340°`). A
/// driver uses this to **dead-band** its output so sensor noise while held still doesn't repaint a
/// heading-up map.
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

    /// Allow a hair of float slop when comparing headings (atan2 + the radian↔degree scale).
    fn close(a: f32, b: f32) {
        let d = (a - b).abs();
        assert!(d < 0.01 || (360.0 - d) < 0.01, "{a} vs {b}");
    }

    #[test]
    fn cardinal_headings() {
        // Field along +X (forward) → facing north.
        close(heading_deg(MagSample::new(20.0, 0.0, -40.0), 0.0), 0.0);
        // North shifted to the device's left (−Y) → facing east.
        close(heading_deg(MagSample::new(0.0, -20.0, -40.0), 0.0), 90.0);
        // Field along −X → facing south.
        close(heading_deg(MagSample::new(-20.0, 0.0, -40.0), 0.0), 180.0);
        // North to the device's right (+Y) → facing west.
        close(heading_deg(MagSample::new(0.0, 20.0, -40.0), 0.0), 270.0);
    }

    #[test]
    fn magnitude_does_not_change_the_angle() {
        // Scaling the whole horizontal vector (an uncalibrated gain) leaves the heading put.
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
