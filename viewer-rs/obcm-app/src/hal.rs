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

/// A physical button on the device. There are exactly two: the rotary encoder's
/// **push**, and the dedicated **Back** button. (Encoder *rotation* is not a
/// button — it arrives as [`InputEvent::Turn`] detents.) This mirrors the input
/// model in `docs/bikepacking-computer-ui-spec.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The push action of the rotary encoder.
    Encoder,
    /// The dedicated Back button.
    Back,
}

/// A press or release edge for a single [`Button`]. The gesture layer reacts to
/// edges plus a clock (not held state), so a host reports one event per physical
/// transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    Down(Button),
    Up(Button),
}

/// A raw input event from the device's controls, *before* gesture recognition:
/// encoder detents and the encoder/Back button edges. The shared
/// [`Gestures`](crate::Gestures) layer turns a stream of these plus a millis
/// clock into the five UI [`Gesture`](crate::Gesture)s, identically on the host
/// and the MCU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Encoder rotated by `n` detents since the last report (signed: positive is
    /// clockwise / "next", negative is counter-clockwise / "previous").
    Turn(i32),
    /// An encoder-push or Back button edge.
    Button(ButtonEvent),
}

/// Source of raw control input. On the device this is the encoder driver + GPIO
/// edges; in the simulator it's the control panel's knob/buttons and keyboard.
/// The host drains it each tick (poll until `None`) and feeds the events to the
/// [`Gestures`](crate::Gestures) recognizer.
pub trait InputSource {
    /// The next pending raw event, or `None` when the queue is drained for this
    /// tick. Called in a loop until it returns `None`.
    fn poll(&mut self) -> Option<InputEvent>;
}
