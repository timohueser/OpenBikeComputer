//! The cross-task hand-off for the **BLE sensor** values — heart rate, power, cadence.
//!
//! The sibling of [`crate::sensor_link`] (GPS/baro) for the raw-value BLE sensors (epic #707): three
//! [`Signal`](embassy_sync::signal::Signal) mailboxes plus the HAL-trait source impls that drain
//! them on the fresh-mailbox contract (`try_take` yields a value once), so each source's `poll`
//! returns `Some` only on the tick a new sample arrived and `None` between — the seam's staleness
//! gate then renders a dropped strap as `--`.
//!
//! **Two producers, one mailbox.** Both the board's BLE central manager (SE6 — decodes 0x2A37 /
//! 0x2A63 / 0x2A5B into raw bpm/W/rpm) *and* the `debug-uart` injection path
//! ([`crate::debug_link`]'s `H`/`P`/`R` lines, SE8) call the same `dispatch_*` here, so the app's
//! `Sensors` wiring is identical whichever is feeding — **last-writer-wins**, which is exactly what a
//! bench wants when a real strap and an injected line coexist. The source ZSTs are named generically
//! (`SensorHr`, not `BleHr`) for that reason.
//!
//! Each `dispatch_*` also pulses [`crate::sensor_link`]'s shared `EVENT` wake (via `wake_event`), so
//! a fresh sensor value pulls the event-driven ride loop out of warm sleep exactly like a GPS fix —
//! copying the GPS dispatch pattern so no new wake path is invented. (The `debug-uart` loop selects
//! on the debug-link wake instead; that path pulses its own `EVENT` at the `debug_link::dispatch`
//! call site.)
//!
//! Gated behind `sensor-link` (it pulls embassy-sync), like `sensor_link`; the pure decode this
//! bridges lives in the radio-free `obc-ble` crate.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_ports::{CadenceSource, HeartRateSource, PowerSource};

/// Latest heart rate (bpm), fresh-mailbox (`try_take` yields it once), drained by [`SensorHr`].
static HR: Signal<CriticalSectionRawMutex, u16> = Signal::new();
/// Latest power (watts), drained by [`SensorPower`].
static POWER: Signal<CriticalSectionRawMutex, u16> = Signal::new();
/// Latest cadence (rpm), drained by [`SensorCadence`].
static CADENCE: Signal<CriticalSectionRawMutex, u8> = Signal::new();

/// Publish a fresh heart-rate sample (bpm) and pulse the shared sensor wake so the event-driven ride
/// loop wakes. Called by the BLE manager (SE6) on a 0x2A37 notification and by the `debug-uart`
/// injection path (SE8) on an `H` line — last-writer-wins.
pub fn dispatch_hr(bpm: u16) {
    HR.signal(bpm);
    crate::sensor_link::wake_event();
}

/// Publish a fresh power sample (watts). A signed meter reading is clamped at `0` by the producer,
/// so this is always non-negative.
pub fn dispatch_power(watts: u16) {
    POWER.signal(watts);
    crate::sensor_link::wake_event();
}

/// Publish a fresh cadence sample (rpm). A coasting rider publishes a fresh `0` (feet still),
/// distinct from no sample at all (the mailbox staying empty).
pub fn dispatch_cadence(rpm: u8) {
    CADENCE.signal(rpm);
    crate::sensor_link::wake_event();
}

/// The rider's heart rate, drained from the shared mailbox. Hand `&mut SensorHr` to `Sensors::hr`.
/// Generic name (not `BleHr`) because both the radio manager and the debug-uart injection feed it.
pub struct SensorHr;
impl HeartRateSource for SensorHr {
    fn poll(&mut self) -> Option<u16> {
        HR.try_take()
    }
}

/// The rider's power, drained from the shared mailbox. Hand `&mut SensorPower` to `Sensors::power`.
pub struct SensorPower;
impl PowerSource for SensorPower {
    fn poll(&mut self) -> Option<u16> {
        POWER.try_take()
    }
}

/// The rider's cadence, drained from the shared mailbox. Hand `&mut SensorCadence` to
/// `Sensors::cadence`.
pub struct SensorCadence;
impl CadenceSource for SensorCadence {
    fn poll(&mut self) -> Option<u8> {
        CADENCE.try_take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The three mailboxes are independent statics; a dispatch on one must not disturb the others,
    // and a source drains its own exactly once (fresh-mailbox), returning `None` until refilled.
    #[test]
    fn hr_mailbox_drains_once() {
        let mut src = SensorHr;
        assert_eq!(src.poll(), None, "empty until a dispatch");
        dispatch_hr(158);
        assert_eq!(src.poll(), Some(158), "drains the published value once");
        assert_eq!(src.poll(), None, "fresh-mailbox: empty again until the next dispatch");
    }

    #[test]
    fn power_mailbox_last_writer_wins() {
        let mut src = SensorPower;
        dispatch_power(200);
        dispatch_power(275); // a second producer overwrites before the app drains
        assert_eq!(src.poll(), Some(275), "last-writer-wins in the shared mailbox");
        assert_eq!(src.poll(), None);
    }

    #[test]
    fn cadence_zero_is_a_real_sample() {
        let mut src = SensorCadence;
        dispatch_cadence(0); // coasting — feet still, a fresh 0, not "no sensor"
        assert_eq!(src.poll(), Some(0), "a coasting 0 is a real reading, distinct from an empty mailbox");
        assert_eq!(src.poll(), None);
    }
}
