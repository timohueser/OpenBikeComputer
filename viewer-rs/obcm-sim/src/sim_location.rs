//! The simulator's [`LocationSource`] — a manually-set fix the control panel
//! edits. This is the host-side mirror of what, on the device, will be a GPS UART
//! driver: same trait, same `Fix`, so [`obcm_app::App`] can't tell them apart.

use obcm_app::{Fix, LocationSource};

/// A location source backed by a single overridable fix. The control panel writes
/// to it via the setters; a future GPX player would swap in its own
/// [`LocationSource`] implementation without touching the app.
pub struct SimLocationSource {
    fix: Option<Fix>,
}

// The setters are the control panel's write API; the loop reads through `poll`.
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
    fn poll(&mut self) -> Option<Fix> {
        self.fix
    }
}
