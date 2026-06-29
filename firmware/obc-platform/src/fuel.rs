//! A board-agnostic stand-in [`FuelGauge`] — a fixed battery level until the real PMIC fuel
//! gauge is wired in.
//!
//! The OBC's power path is built around a Nordic **nPM1300** PMIC, which carries a coulomb-
//! counting fuel gauge reachable over the same I²C/TWIM bus as its charger control. Reading a
//! real state-of-charge from it (Nordic ships a fuel-gauge algorithm crate) is a follow-up; until
//! then the board drives [`StubFuelGauge`] so the Home gauge has a plausible level and the whole
//! [`FuelGauge`] seam (trait → [`Sensors`](obc_app::Sensors) → `App::tick` → `AppState`) is
//! exercised end-to-end.
//!
//! **To wire the real gauge:** add an `Npm1300FuelGauge` here that owns the TWIM handle, and have
//! its [`poll`](FuelGauge::poll) return `Some(percent)` when a fresh SoC sample is ready (`None`
//! between). Swap the board's `StubFuelGauge::new(..)` for it — nothing else changes, since the
//! app only sees the trait.

use obc_app::FuelGauge;

/// A fixed-reading [`FuelGauge`] stand-in: reports a constant state of charge. `App::tick` only
/// polls it on its slow battery cadence (~30 s, `obc_app`'s `BATTERY_POLL_MS`) and repaints only
/// when the level changes, so a constant value here costs nothing and never redraws. Replace with
/// an `nPM1300`-backed reader when the PMIC fuel gauge is brought up; see the module docs.
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
        // A constant battery always reads the same; the app dedupes, so this never forces a redraw.
        Some(self.soc)
    }
}
