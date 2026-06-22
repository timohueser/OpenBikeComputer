//! The simulator's [`CompassSource`] — a manually-set heading the control panel edits, the
//! host-side mirror of the device's electronic compass (magnetometer).
//!
//! On the device the heading-when-stopped comes from a magnetometer; here it's a slider. Like
//! [`SimLocationSource`](crate::sim_location::SimLocationSource) it just replays the latest value,
//! so the shared [`App`](obc_app::App) reads it through the same [`CompassSource`] trait it will
//! read the real driver through — and only consults it while stopped on a heading-up map.

use obc_app::CompassSource;

/// A compass backed by a single overridable heading (degrees CW from north). `None` until the
/// panel sets one, so before the user touches the slider the app simply has no compass heading
/// (it holds north / the last GPS course while stopped).
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
    // Returns the latest heading on every poll rather than a fresh-only cadence. The app only
    // adopts it while stopped on a heading-up map and dedups by value, so a constant reading
    // never forces a redraw — and a real fresh-only magnetometer driver looks the same to the
    // app, which retains the last reading between samples. See [`CompassSource`].
    fn poll(&mut self) -> Option<f32> {
        self.heading
    }
}
