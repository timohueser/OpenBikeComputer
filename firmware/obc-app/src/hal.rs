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

/// Source of barometric altitude — the device's **pressure altimeter**, a sensor
/// entirely separate from the GPS (its own bus, its own sample rate). The app polls it
/// each tick like a [`LocationSource`], but the two are **asynchronous**: a baro sample
/// and a GPS fix do not arrive together. So [`poll`](AltimeterSource::poll) returns
/// `Some(meters)` only when a *fresh* sample is available and `None` otherwise — the app
/// integrates climb from this stream independently of position fixes, so going off-route
/// (or briefly losing GPS) never stops the climb total.
///
/// Why a dedicated sensor rather than GPS altitude: GPS vertical accuracy is poor and
/// noisy, whereas a barometric altimeter resolves the *relative* height changes that make
/// up "climbed" far better. Only relative change matters here, so absolute calibration
/// (weather drift) is irrelevant — the climb accumulator dead-bands small wiggles anyway.
pub trait AltimeterSource {
    /// The latest barometric altitude in meters, or `None` if no new sample this tick.
    fn poll(&mut self) -> Option<f32>;
}

/// Sink for the recorded ride **track** — where each accepted fix is logged so the ride can
/// be saved as a `.gpx`. The app encodes the [`TrackPoint`](obc_route::TrackPoint) (so the
/// firmware and sim share one record format) and hands it here; the host appends the bytes
/// to the SD-card log it owns (the sim writes a temp file). Begin / finalise-to-GPX /
/// discard are driven separately by the host reconciling the [`Activity`](crate::Activity)
/// session — see `App::tick`'s caller — so this trait is just the per-fix append.
pub trait TrackSink {
    /// Append one recorded fix to the open ride log.
    fn record(&mut self, p: obc_route::TrackPoint);
}

/// The polled sensor set handed to [`App::tick`](crate::App::tick) each frame: the user's
/// location, optionally the barometric altimeter, and optionally the track [`TrackSink`].
/// Bundling the handles keeps `tick` to a single argument — adding one later is a new field
/// here, not a new `tick` parameter — while leaving each trait separate, since they model
/// independent hardware. The host builds one per tick from whichever are live (GPX replay
/// vs. manual panel in the sim; a real GPS + barometer + SD log on the device).
pub struct Sensors<'a> {
    /// The user's position source.
    pub loc: &'a mut dyn LocationSource,
    /// The barometric altimeter, or `None` when no altitude source is wired (e.g. the
    /// simulator's manual control) — climb then simply doesn't accumulate.
    pub altimeter: Option<&'a mut dyn AltimeterSource>,
    /// The recorded-track sink, or `None` when nothing is logging (the sim's manual panel,
    /// tests) — the ride then simply isn't recorded.
    pub track: Option<&'a mut dyn TrackSink>,
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

/// Milliseconds from a clock consistent with the **sensor samples** — wall-clock on the
/// device, GPX **playback** time in the simulator. Passed to [`App::tick`](crate::App::tick)
/// so the ride accumulators (moving time → Avg. Speed) measure sample-relative time and
/// aren't scaled by the simulator's replay-speed multiplier. A type distinct from
/// [`InputClock`] so the two clocks can't be handed to the wrong method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RideClock(pub u32);

/// Milliseconds from the host/MCU **wall clock** (monotonic real time). Passed to
/// [`App::handle_input`](crate::App::handle_input) for button hold-timing — a long-press
/// is real-time even while a GPX replay is fast-forwarding, which is exactly why this is
/// distinct from [`RideClock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct InputClock(pub u32);
