//! Chip-agnostic **electronic-compass** maths — a 3-axis magnetometer sample → a heading in degrees
//! clockwise from north, for the [`CompassSource`](obc_app::CompassSource) seam (the heading-when-
//! stopped, see `obc-app/src/hal.rs`).
//!
//! ## Why this is its own module (the "swap the chip" story)
//! The current bring-up reads the **AK09916** inside an ICM-20948 (register map in
//! [`crate::icm20948`]), but the shipping board is expected to carry a **separate, plain 3-axis
//! magnetometer**. So the only thing that should change when the chip does is the register map +
//! raw→µT scaling in the chip module — *this* file (the heading geometry) and the whole app-facing
//! [`CompassSource`](obc_app::CompassSource) seam stay put. Any magnetometer driver, on any bus,
//! reduces its reading to a [`MagSample`] (3 axes, µT, in the **device frame**) and calls
//! [`heading_deg`]; nothing here knows or cares which chip produced it.
//!
//! ## Scope: flat heading only (the 3 DOF we use)
//! We deliberately use **only** the magnetometer's three axes — no accelerometer, no gyro — so the
//! heading is computed *flat* (assuming the device is roughly level). That's enough for its one job:
//! standing in for [`Fix::course`](obc_app::Fix::course) on a heading-up map while the rider is
//! stopped (the GPS drops course below walking pace). Tilt compensation would need a gravity vector
//! from an accelerometer and is a deliberate non-goal for now — [`heading_deg`] is written so adding
//! it later is a new function that consumes an accel vector, not a change to this signature.
//!
//! ## Calibration is the caller's job
//! [`heading_deg`] expects a sample that's **already** had hard-iron offset removed and been rotated
//! into the device frame (X forward / Y right / Z down) — both are board-mounting concerns, so they
//! live next to the I²C transactions in the board crate, not here. This module is pure geometry:
//! `atan2` of two axes plus a magnetic-declination shift, so it unit-tests on the host with no
//! hardware (the same split as [`crate::bmp581`] / [`crate::ubx`]).

/// A single magnetometer reading: the three field-strength axes in **microtesla**, expressed in the
/// **device frame** — `x` forward (toward the top of the screen), `y` to the right, `z` down. A
/// magnetometer driver scales its raw counts to µT and remaps the sensor's own axes into this frame
/// (a per-board-mounting rotation) before building one; [`heading_deg`] then needs no chip knowledge.
///
/// `z` is carried even though the flat heading ignores it: it's what an overflow check reads and what
/// a future tilt-compensated heading (with an accelerometer) would consume. Keeping all three here
/// means that upgrade doesn't churn the type.
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
/// `90` = east) — matching [`Fix::course`](obc_app::Fix::course) so the app can use either
/// interchangeably to orient a heading-up map.
///
/// With the device-frame convention (X forward, Y right, Z down) the heading of the forward axis is
/// `atan2(-y, x)`: facing north the horizontal field lies along +X (`y≈0` → `0°`); rotate the device
/// 90° clockwise to face east and north shifts to the device's left (`-y` → `+90°`).
///
/// `declination_deg` is the local **magnetic declination** (east-positive) that converts magnetic
/// north to the *true* north the map and GPS course use. Pass `0.0` to get raw magnetic heading; a
/// caller with a position fix can supply the real value later (e.g. from the WMM or a coarse table) —
/// that's the only thing standing between this and a true-north heading, and it's a pure add here.
///
/// The result is normalised to `[0, 360)`. Magnitude is irrelevant (only the *direction* of the
/// horizontal field matters), so an uncalibrated scale factor doesn't affect the angle — but a
/// **hard-iron offset must already be removed** by the caller, or the heading skews (see the module
/// docs).
pub fn heading_deg(s: MagSample, declination_deg: f32) -> f32 {
    let deg = libm::atan2f(-s.y, s.x) * (180.0 / core::f32::consts::PI);
    normalize_deg(deg + declination_deg)
}

/// Wrap an angle in degrees into `[0, 360)`. A bounded `+= 360` / `-= 360` rather than a float modulo
/// so it stays exact for the small out-of-range inputs [`heading_deg`] produces (an `atan2` result in
/// `[-180, 180]` plus a modest declination) and pulls in no extra `libm`.
pub fn normalize_deg(mut deg: f32) -> f32 {
    while deg < 0.0 {
        deg += 360.0;
    }
    while deg >= 360.0 {
        deg -= 360.0;
    }
    deg
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
}
