//! The simulator's [`CompassSource`] — a manually-set heading the control panel edits, the
//! host-side mirror of the device's magnetometer. Read through the same trait; the app only
//! consults it while stopped on a heading-up map.

use obc_ports::CompassSource;

/// A compass backed by a single overridable heading (degrees CW from north). `None` until the
/// panel sets one, so before then the app holds north / the last GPS course while stopped.
#[derive(Debug, Default)]
pub struct SimCompass {
    heading: Option<f32>,
}

impl SimCompass {
    pub fn new() -> Self {
        SimCompass { heading: None }
    }

    /// Set the simulated magnetic heading (degrees CW from north).
    pub fn set(&mut self, deg: f32) {
        self.heading = Some(deg);
    }
}

impl CompassSource for SimCompass {
    // Returns the latest heading on every poll rather than a fresh-only cadence. The app dedups
    // by value, so a constant reading never forces a redraw — and a real fresh-only driver looks
    // the same to the app, which retains the last reading between samples.
    fn poll(&mut self) -> Option<f32> {
        self.heading
    }
}
