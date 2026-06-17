//! Hardware-abstraction traits — the seam between the shared app and the host.
//!
//! On the **device**, a GPS chip and GPIO buttons implement these. In the
//! **simulator**, the control panel (and later a GPX replay) implement them. The
//! app polls the traits and is oblivious to which side it's running on.

/// A position/orientation fix, however it was obtained (GPS chip, GPX replay,
/// manual control-panel override).
///
/// Position is integer microdegrees (1e-6°), matching the OBCM file format and
/// the renderer, so a fix drops straight into a [`crate::AppState`] camera with
/// no unit juggling. `course` and `speed_mps` are optional because a real GPS
/// only knows them while the user is actually moving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    /// Latitude in microdegrees (1e-6°).
    pub lat: i32,
    /// Longitude in microdegrees (1e-6°).
    pub lon: i32,
    /// Course over ground in degrees clockwise from north (`0` = north,
    /// `90` = east), or `None` when stationary / unknown. This is what the
    /// future heading marker points along.
    pub course: Option<f32>,
    /// Ground speed in meters per second, or `None` when stationary / unknown.
    pub speed_mps: Option<f32>,
}

impl Fix {
    /// A stationary fix at `(lat, lon)` with no course or speed.
    #[inline]
    pub fn at(lat: i32, lon: i32) -> Self {
        Fix { lat, lon, course: None, speed_mps: None }
    }
}

/// Source of the user's location. The app calls [`poll`](LocationSource::poll)
/// once per tick; on the device this wraps a GPS driver, in the simulator it's
/// the control panel or a GPX player.
pub trait LocationSource {
    /// The latest fix, or `None` if no fix is available yet (no satellite lock,
    /// empty replay, etc.). Returning the same fix on consecutive polls is fine —
    /// the app treats it idempotently.
    fn poll(&mut self) -> Option<Fix>;
}

/// A physical button on the device.
///
/// Provisional set — the real key map firms up with the hardware; this exists so
/// [`InputSource`] and the (future) app input handling have a concrete type to
/// name. Add/rename variants freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
}

/// A press or release edge for a single [`Button`]. The app reacts to edges, not
/// held state, so a host reports one event per transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    Down(Button),
    Up(Button),
}

/// Source of physical button input. On the device this is GPIO; in the simulator
/// it's the control panel's virtual buttons / keyboard.
///
/// Defined now so the HAL boundary is complete, but not yet consumed by
/// [`AppState`](crate::AppState) — button handling lands with the emulator's
/// button-press feature.
pub trait InputSource {
    /// The next pending button edge, or `None` when the queue is drained for this
    /// tick. Called in a loop until it returns `None`.
    fn poll(&mut self) -> Option<ButtonEvent>;
}
