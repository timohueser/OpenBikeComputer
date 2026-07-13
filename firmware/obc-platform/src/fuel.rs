//! A board-agnostic stand-in [`FuelGauge`] — a fixed battery level until the real PMIC fuel gauge
//! is wired in.
//!
//! The OBC's power path is built around a Nordic **nPM1300** PMIC with a coulomb-counting fuel gauge
//! on the same I²C/TWIM bus as its charger control. Reading a real state-of-charge from it is a
//! follow-up; until then [`StubFuelGauge`] gives the Home gauge a plausible level and exercises the
//! whole [`FuelGauge`] seam. To wire the real gauge, add an `Npm1300FuelGauge` here and swap the
//! board's `StubFuelGauge::new(..)` for it — nothing else changes, since the app only sees the trait.

use obc_app::FuelGauge;

/// A fixed-reading [`FuelGauge`] stand-in reporting a constant state of charge. `App::tick` polls it
/// on a slow battery cadence (~30 s) and repaints only when the level changes, so a constant value
/// never redraws.
pub struct StubFuelGauge {
    soc: u8,
}

impl StubFuelGauge {
    /// A stub reporting a constant `soc` percent (clamped to 0–100).
    pub const fn new(soc: u8) -> Self {
        StubFuelGauge { soc: if soc > 100 { 100 } else { soc } }
    }
}

impl FuelGauge for StubFuelGauge {
    fn poll(&mut self) -> Option<u8> {
        Some(self.soc)
    }
}
