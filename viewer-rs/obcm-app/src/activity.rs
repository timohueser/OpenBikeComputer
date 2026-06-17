//! The ride/tracking model — what the device is *doing*.
//!
//! For this slice it is just the operating [`Mode`]; the distance / time / climb
//! accumulators (fed from `Fix`es, read by the Elevation screen) hang off
//! [`Activity`] in a later slice. Kept separate from [`AppState`](crate::AppState)
//! (the camera) because the mode outlives any one screen and several screens read
//! and change it.

/// The device's operating mode (`docs/ui_framework_brief.md` §"Operating modes").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// No route active — the Home screensaver.
    #[default]
    Idle,
    /// A route is loaded and tracking is running — Map / Elevation.
    Riding,
    /// Tracking paused — the Ride control overlay is up.
    Paused,
}

/// The active ride. Carries the [`Mode`] now; the ride-stat accumulators land here
/// with the Elevation screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    pub mode: Mode,
}

impl Activity {
    /// A fresh activity in the given mode.
    pub fn new(mode: Mode) -> Self {
        Activity { mode }
    }
}
