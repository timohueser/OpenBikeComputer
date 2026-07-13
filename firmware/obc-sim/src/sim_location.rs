//! The simulator's [`LocationSource`] — a manually-set fix the control panel edits, the
//! host-side mirror of the device's GPS UART driver (same trait, same `Fix`, so
//! [`obc_app::App`] can't tell them apart).

use obc_ports::{Fix, LocationSource};

/// A location source backed by a single overridable fix, written via the setters.
pub struct SimLocationSource {
    fix: Option<Fix>,
}

impl SimLocationSource {
    pub fn new(fix: Option<Fix>) -> Self {
        SimLocationSource { fix }
    }

    /// The current fix (for the control panel to display / seed its widgets).
    pub fn current(&self) -> Option<Fix> {
        self.fix
    }

    /// Move the simulated user to `(lat, lon)` microdegrees, preserving course and
    /// speed (or starting a stationary fix if there wasn't one).
    pub fn set_position(&mut self, lat: i32, lon: i32) {
        match &mut self.fix {
            Some(f) => {
                f.lat = lat;
                f.lon = lon;
            }
            None => self.fix = Some(Fix::at(lat, lon)),
        }
    }

    /// Set the simulated course over ground in degrees (clockwise from north).
    pub fn set_course(&mut self, deg: f32) {
        if let Some(f) = &mut self.fix {
            f.course = Some(deg);
        }
    }
}

impl LocationSource for SimLocationSource {
    // Deliberately returns the same fix on every poll — *not* the fresh-fix cadence a real sensor
    // (or the GpxPlayer) follows. The manual panel is a position *override* for free-roaming, not a
    // ride-recording source: a stationary user books no distance and a drag reads as a teleport
    // (dropped), both acceptable. Ride recording exercises the fresh-fix path via the GPX player.
    fn poll(&mut self) -> Option<Fix> {
        self.fix
    }
}
