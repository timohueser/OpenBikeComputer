//! Hardware-abstraction traits — the seam between the shared app and the host.
//!
//! On the **device**, a GPS chip and GPIO buttons implement these. In the
//! **simulator**, the control panel (and later a GPX replay) implement them. The
//! app polls the traits and is oblivious to which side it's running on.

use crate::settings::{DateTime, Settings};

/// A position/orientation fix, however it was obtained.
///
/// Position is integer microdegrees (1e-6°), matching the OBCM file format and
/// the renderer. `course` and `speed_mps` are optional because a real GPS only
/// knows them while the user is actually moving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    /// Latitude in microdegrees (1e-6°).
    pub lat: i32,
    /// Longitude in microdegrees (1e-6°).
    pub lon: i32,
    /// Course over ground in degrees clockwise from north (`0` = north,
    /// `90` = east), or `None` when stationary / unknown. The heading marker
    /// points along this.
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

/// Source of the user's location. The app calls [`poll`](LocationSource::poll) once per tick.
pub trait LocationSource {
    /// A fix **only when a fresh sample is available this tick**, `None` otherwise. Each `Some`
    /// is integrated as exactly one GPS sample at the current [`RideClock`](crate::RideClock).
    /// A source must **not** re-return the same fix every ~8 ms poll: the next per-second move
    /// would then look like an 8 ms teleport, get rejected as a glitch, and break the segment.
    ///
    /// A stationary rider still emits a fresh, *identical-position* fix at the GPS rate — return
    /// it (do **not** dedupe by position): a zero-distance interval reads as "stopped" and keeps
    /// the moving-time clock honest. `None` means strictly "no new fix yet".
    fn poll(&mut self) -> Option<Fix>;
}

/// Source of barometric altitude — a **pressure altimeter** separate from the GPS. Polled each
/// tick; the app integrates climb from this stream.
///
/// **Sample coupling.** The trait allows an independent baro cadence, but the shipping nRF54L
/// driver reads the BMP581 on each GPS **fix** (forced-mode) so the altitude is coherent with the
/// position. Consequence: **climb only accrues while GPS fixes arrive** — a tunnel pauses climb
/// until the fix returns (acceptable: no position to log during an outage anyway; the only lost
/// case is moving + climbing + no fix). A host wanting climb to survive a dropout must poll the
/// baro on its own clock.
///
/// A dedicated sensor rather than GPS altitude because GPS vertical accuracy is poor; only relative
/// change matters here, so absolute calibration drift is irrelevant (the accumulator dead-bands
/// small wiggles anyway).
pub trait AltimeterSource {
    /// The latest barometric altitude in meters, or `None` if no new sample this tick.
    fn poll(&mut self) -> Option<f32>;
}

/// Source of **ambient temperature** in °C — on the device, the BMP581 reports it nearly free
/// alongside each pressure reading. Polled each tick; `Some(celsius)` only on a fresh reading, the
/// app holds the last value on `None`. `None` on a host with no sensor. No screen consumes it yet.
pub trait TemperatureSource {
    /// The latest ambient temperature in °C, or `None` if no new sample this tick.
    fn poll(&mut self) -> Option<f32>;
}

/// A UTC timestamp from the GPS receiver, for setting the wall clock. Minute-resolution
/// [`DateTime`] **plus** the seconds-into-the-minute kept separately (since [`DateTime`] carries no
/// seconds): the app back-dates the [`WallClock`](crate::WallClock) epoch by `second` so the
/// displayed minute rolls over at the true instant rather than up to a fix-interval late.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpsTime {
    /// The receiver's UTC date + time of day (no seconds — see the struct docs).
    pub utc: DateTime,
    /// Seconds into the current minute (0–59), for the epoch back-date.
    pub second: u8,
}

/// Source of **UTC time** from the GPS receiver — backs the "Set from GPS" clock option. Polled
/// each tick; `Some` only on a fresh resolved UTC time. The receiver resolves time *before* a 3D
/// position, so this can deliver a stamp during acquisition — the app sets the clock even while the
/// "No GPS Fix" banner is still up.
///
/// Consumed **only** when [`Settings::gps_time`](crate::Settings::gps_time) is set, and **not**
/// persisted on every stamp (the set-point self-heals from GPS each boot; a per-second write would
/// thrash the store). `None` on a host with no GPS time.
pub trait ClockSource {
    /// The latest resolved UTC time from the receiver, or `None` if none is fresh this tick.
    fn poll(&mut self) -> Option<GpsTime>;
}

/// Source of the rider's **heading** from a magnetometer (electronic compass). Its one job is the
/// heading when the GPS can't supply a course: a real receiver drops [`Fix::course`] to `None`
/// below walking pace, so a stationary rider's heading-up map would otherwise snap to north. While
/// moving the GPS course still wins.
///
/// Polled each tick; `Some(degrees)` only on a fresh reading, the app holds the last on `None`.
/// Degrees are clockwise from north (`0` = north, `90` = east), matching [`Fix::course`].
pub trait CompassSource {
    /// The latest magnetic heading in degrees CW from north, or `None` if no new sample this tick.
    fn poll(&mut self) -> Option<f32>;
}

/// Source of the **battery state of charge** from the device's PMIC fuel gauge (e.g. an nPM1300
/// over I²C). Polled on a **slow cadence** (~30 s, not every tick — see `App::tick`), since charge
/// drifts over minutes and an I²C read shouldn't run at the frame rate. Stored in
/// [`AppState::battery_pct`](crate::AppState::battery_pct), where the Home gauge draws it.
///
/// `Some(percent)` (0–100) when available, else `None` (the app keeps the last value). A reading
/// that *changes* the stored level repaints the screensaver; an unchanged one is free. Out-of-range
/// values are the host's to clamp. Since the app throttles the call, an implementation may read the
/// hardware directly each `poll`.
pub trait FuelGauge {
    /// The latest battery charge in percent (0–100), or `None` if no reading is available.
    fn poll(&mut self) -> Option<u8>;
}

/// Sink for the recorded ride **track** — each accepted fix logged so the ride can be saved as a
/// `.gpx`. The app encodes the [`TrackPoint`](obc_route::TrackPoint) and hands it here; the host
/// appends to the log it owns (SD card on device, temp file in the sim). Begin / finalise / discard
/// are driven separately by the host reconciling the [`Activity`](crate::Activity) session — this
/// trait is just the per-fix append.
pub trait TrackSink {
    /// Append one recorded fix to the open ride log.
    fn record(&mut self, p: obc_route::TrackPoint);
}

/// The polled sensor set handed to [`App::tick`](crate::App::tick) each frame. Bundling the handles
/// keeps `tick` to a single argument; each trait stays separate since they model independent
/// hardware. The host builds one per tick from whichever are live.
pub struct Sensors<'a> {
    /// The user's position source.
    pub loc: &'a mut dyn LocationSource,
    /// The barometric altimeter, or `None` when no altitude source is wired — climb then doesn't
    /// accumulate.
    pub altimeter: Option<&'a mut dyn AltimeterSource>,
    /// The ambient-temperature source, or `None` when none is wired. On device it's the BMP581's
    /// per-fix reading, coherent with the altitude.
    pub temperature: Option<&'a mut dyn TemperatureSource>,
    /// The GPS UTC time source, or `None` when none is wired — the clock then stays whatever the
    /// user set by hand. Used only when "Set from GPS" is on.
    pub clock: Option<&'a mut dyn ClockSource>,
    /// The electronic compass, or `None` when none is wired — the heading-up map then holds north /
    /// the last GPS course while stopped.
    pub compass: Option<&'a mut dyn CompassSource>,
    /// The recorded-track sink, or `None` when nothing is logging — the ride then isn't recorded.
    pub track: Option<&'a mut dyn TrackSink>,
    /// The battery fuel gauge, or `None` when none is wired (holds the last value). Read on the slow
    /// ~30 s cadence, on every screen, since the Home screensaver shows the battery while idle.
    pub fuel: Option<&'a mut dyn FuelGauge>,
}

/// Persistent store for the device [`Settings`] — the seam between the shared settings model and
/// the host's medium (simulator file vs. a reserved region of the nRF54L's on-chip RRAM,
/// independent of the SD card so settings survive a reboot with no card). The app seeds from
/// [`load`](SettingsStore::load) at boot and asks the host to [`save`](SettingsStore::save)
/// whenever [`App::take_settings_dirty`](crate::App::take_settings_dirty) reports a change.
pub trait SettingsStore {
    /// The persisted settings, or `None` when none are stored yet or the blob is unreadable — the
    /// caller then starts from [`Settings::default`]. Decodes through
    /// [`settings::decode`](crate::settings::decode), so a `Some` is always valid.
    fn load(&mut self) -> Option<Settings>;
    /// Persist `s` (encoded via [`settings::encode`](crate::settings::encode)). Best-effort: a
    /// write failure is the host's to log — settings stay live in RAM regardless.
    fn save(&mut self, s: &Settings);
}

/// A physical button on the device. Exactly two: the rotary encoder's **push** and the dedicated
/// **Back** button. (Encoder *rotation* is not a button — it arrives as [`InputEvent::Turn`]
/// detents.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The push action of the rotary encoder.
    Encoder,
    /// The dedicated Back button.
    Back,
}

/// A press or release edge for a single [`Button`]. The gesture layer reacts to edges plus a clock
/// (not held state), so a host reports one event per physical transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    Down(Button),
    Up(Button),
}

/// A raw input event from the device's controls, *before* gesture recognition. The shared
/// [`Gestures`](crate::Gestures) layer turns a stream of these plus a millis clock into the five UI
/// [`Gesture`](crate::Gesture)s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Encoder rotated by `n` detents since the last report (signed: positive is clockwise /
    /// "next", negative is counter-clockwise / "previous").
    Turn(i32),
    /// An encoder-push or Back button edge.
    Button(ButtonEvent),
}

/// Source of raw control input (encoder driver + GPIO edges on device; knob/buttons/keyboard in the
/// sim). The host drains it each tick (poll until `None`) and feeds the events to the
/// [`Gestures`](crate::Gestures) recognizer.
pub trait InputSource {
    /// The next pending raw event, or `None` when the queue is drained for this tick. Called in a
    /// loop until it returns `None`.
    fn poll(&mut self) -> Option<InputEvent>;
}

/// Milliseconds from a clock consistent with the **sensor samples** — wall-clock on the device, GPX
/// **playback** time in the simulator. Passed to [`App::tick`](crate::App::tick) so the ride
/// accumulators measure sample-relative time and aren't scaled by the sim's replay-speed
/// multiplier. Distinct from [`InputClock`] so the two clocks can't be swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RideClock(pub u32);

/// Milliseconds from the host/MCU **wall clock** (monotonic real time). Passed to
/// [`App::handle_input`](crate::App::handle_input) for button hold-timing — a long-press is
/// real-time even while a GPX replay fast-forwards, which is why this is distinct from
/// [`RideClock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct InputClock(pub u32);
