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

/// The active ride: the [`Mode`] plus which route is loaded. The ride-stat
/// accumulators (distance / time / climb from `Fix`es) land here with the
/// Elevation screen.
#[derive(Debug, Clone, Copy, Default)]
pub struct Activity {
    pub mode: Mode,
    /// Index into the app's route [`Catalog`](crate::route::Catalog) of the loaded
    /// route, or `None` when idle. The summary is read from the catalog; the geometry
    /// is opened separately by the host (only the active route is resident).
    pub active_route: Option<usize>,
}

impl Activity {
    /// A fresh activity in the given mode, no route loaded.
    pub fn new(mode: Mode) -> Self {
        Activity { mode, active_route: None }
    }
}
