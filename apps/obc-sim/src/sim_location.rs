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

    /// Set or clear the simulated course over ground in degrees clockwise from north. A stationary
    /// manual fix clears it so the compass, rather than a stale GPS course, owns heading.
    pub fn set_course(&mut self, deg: Option<f32>) {
        if let Some(f) = &mut self.fix {
            f.course = deg;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_course_can_be_cleared_for_the_stopped_compass() {
        let mut source = SimLocationSource::new(Some(Fix { lat: 0, lon: 0, course: Some(90.0), speed_mps: Some(0.0) }));
        source.set_course(None);
        assert_eq!(source.current().and_then(|fix| fix.course), None);
        source.set_course(Some(225.0));
        assert_eq!(source.current().and_then(|fix| fix.course), Some(225.0));
    }
}
