//! Hardware-abstraction traits — the seam between the shared app and the host.
//!
//! On the **device**, a GPS chip and GPIO buttons implement these. In the
//! **simulator**, the control panel (and later a GPX replay) implement them. The
//! app polls the traits and is oblivious to which side it's running on.

use crate::settings::{DateTime, Settings};

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
    /// heading marker points along.
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
    /// A fix **only when a fresh sample is available this tick**, `None` otherwise —
    /// identical cadence semantics to [`AltimeterSource::poll`]. Each `Some` is integrated
    /// as exactly one real GPS sample at the current [`RideClock`](crate::RideClock): the app
    /// advances its motion integrator (previous fix + timestamp) on *every* returned fix. So a
    /// source must **not** re-return the same fix on every ~8 ms poll — doing so would make the
    /// next per-second move look like an 8 ms teleport, get it rejected as a glitch, and record
    /// zero distance with a segment break on each fix. Return `None` on ticks with no new fix.
    ///
    /// A stationary rider still emits a fresh, *identical-position* fix at the GPS rate — that
    /// is a real sample and must be returned (do **not** dedupe by position): the integrator
    /// reads a zero-distance interval as "stopped" and keeps the moving-time clock honest,
    /// rather than treating it as a dropout. `None` means strictly "no new fix yet" — no
    /// satellite lock, a cold start, an empty replay, or the gap between the receiver's
    /// per-second fixes.
    fn poll(&mut self) -> Option<Fix>;
}

/// Source of barometric altitude — the device's **pressure altimeter**, a sensor
/// separate from the GPS. The app polls it each tick like a [`LocationSource`]; so
/// [`poll`](AltimeterSource::poll) returns `Some(meters)` only when a *fresh* sample is available
/// and `None` otherwise — the app integrates climb from this stream.
///
/// **Sample coupling (issue #218).** The trait is *written* to allow an independent baro cadence,
/// and a host that has one (the simulator's manual slider, a free-running baro) should drive it that
/// way. But the shipping nRF54L driver takes a **coherent** sample: it reads the BMP581 on each GPS
/// **fix** (forced-mode, one reading per fix) so the altitude is from the same instant as the
/// position. The tradeoff that buys: **climb only accrues while GPS fixes arrive** — a GPS outage
/// (a tunnel) pauses climb until the fix returns. Accepted because during an outage there's no
/// position to log anyway; the only lost case is *moving + climbing + no fix*. A host wanting climb
/// to survive a dropout must poll the baro on its own clock instead of coupling it to the fix.
///
/// Why a dedicated sensor rather than GPS altitude: GPS vertical accuracy is poor and
/// noisy, whereas a barometric altimeter resolves the *relative* height changes that make
/// up "climbed" far better. Only relative change matters here, so absolute calibration
/// (weather drift) is irrelevant — the climb accumulator dead-bands small wiggles anyway.
pub trait AltimeterSource {
    /// The latest barometric altitude in meters, or `None` if no new sample this tick.
    fn poll(&mut self) -> Option<f32>;
}

/// Source of **ambient temperature** in °C — on the device, the BMP581 altimeter reports it nearly
/// free alongside each pressure reading (issue #218), so the GPS-coherent baro read publishes a
/// temperature sample on the same instant. Polled each tick like the other sensors, on its own
/// cadence: [`poll`](TemperatureSource::poll) returns `Some(celsius)` only when a *fresh* reading is
/// available and `None` otherwise. The app keeps the last value (a `None` between samples holds it).
///
/// `None` on a host with no temperature sensor (the simulator's manual panel, tests) — the app then
/// simply has no temperature to show. No screen consumes it yet; it's stored for a future readout
/// (e.g. a Statistics-grid field), so an implementation needs no rate-limit of its own.
pub trait TemperatureSource {
    /// The latest ambient temperature in °C, or `None` if no new sample this tick.
    fn poll(&mut self) -> Option<f32>;
}

/// A UTC timestamp from the GPS receiver (issue #223) — the [`ClockSource`] hands one over so the
/// app can set the wall clock from GPS. Minute-resolution [`DateTime`] (the device only shows
/// `HH:MM`) **plus** the seconds-into-the-minute, kept separately because [`DateTime`] carries no
/// seconds: the app back-dates the [`WallClock`](crate::WallClock) epoch by `second` so the
/// displayed minute rolls over at the true instant rather than up to a fix-interval late.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpsTime {
    /// The receiver's UTC date + time of day (no seconds — see the struct docs).
    pub utc: DateTime,
    /// Seconds into the current minute (0–59), for the epoch back-date.
    pub second: u8,
}

/// Source of **UTC time** from the GPS receiver (issue #223) — what makes the "Set from GPS" clock
/// option actually work. Polled each tick like the other sensors:
/// [`poll`](ClockSource::poll) returns `Some` only when a *fresh* resolved UTC time is available
/// and `None` otherwise (no time lock yet, or the gap between fixes). The receiver resolves time
/// *before* a 3D position, so this can deliver a stamp during acquisition — the app sets the clock
/// even while the "No GPS Fix" banner is still up.
///
/// The app consumes it **only** when [`Settings::gps_time`](crate::Settings::gps_time) is set, and
/// does **not** persist on every stamp (the set-point self-heals from GPS each boot; a per-second
/// write would thrash the store). `None` on a host with no GPS time (the simulator, tests) — the
/// clock then stays whatever the user set by hand.
pub trait ClockSource {
    /// The latest resolved UTC time from the receiver, or `None` if none is fresh this tick.
    fn poll(&mut self) -> Option<GpsTime>;
}

/// Source of the rider's **heading** from a magnetometer (electronic compass) — the direction
/// the device is pointing, independent of motion. Its one job is the heading when the GPS can't
/// supply a course: a real receiver drops [`Fix::course`] to `None` below walking pace (see
/// [`LocationSource`]), so a stationary rider's heading-up map would otherwise snap to north.
/// The compass fills that gap; while the rider is moving the GPS course still wins.
///
/// Polled each tick like the other sensors, on its own cadence: [`poll`](CompassSource::poll)
/// returns `Some(degrees)` only when a *fresh* reading is available and `None` otherwise. The
/// app retains the last reading, so a `None` between samples simply holds the current heading.
/// Degrees are clockwise from north (`0` = north, `90` = east), matching [`Fix::course`].
pub trait CompassSource {
    /// The latest magnetic heading in degrees CW from north, or `None` if no new sample this tick.
    fn poll(&mut self) -> Option<f32>;
}

/// Source of the **battery state of charge** from the device's PMIC fuel gauge (e.g. an
/// nPM1300 over I²C). Unlike the other sensors, the app polls this on a **slow cadence** (~30 s,
/// not every tick — see `App::tick`), since battery charge drifts over minutes and a real I²C
/// read shouldn't run at the frame rate. It stores the reading in
/// [`AppState::battery_pct`](crate::AppState::battery_pct), where the Home gauge draws it. Until a
/// real gauge is wired, a host supplies a fixed-value stub (`obc_platform::StubFuelGauge`); a sim
/// may drive it from a control.
///
/// [`poll`](FuelGauge::poll) returns `Some(percent)` (0–100) when a reading is available and
/// `None` otherwise (gauge not ready / bus error) — the app keeps the last value on `None`. A
/// reading that *changes* the stored level repaints the screensaver; an unchanged one is free (so
/// a constant stub never redraws). Out-of-range values are the host's to clamp. Because the app
/// already throttles the call, an implementation may read the hardware directly each `poll`
/// without its own rate-limit.
pub trait FuelGauge {
    /// The latest battery charge in percent (0–100), or `None` if no reading is available.
    fn poll(&mut self) -> Option<u8>;
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
    /// The ambient-temperature source, or `None` when none is wired (the sim's manual panel, tests)
    /// — the app then has no temperature to store. On the device it's the BMP581's per-fix reading,
    /// coherent with the altitude (issue #218).
    pub temperature: Option<&'a mut dyn TemperatureSource>,
    /// The GPS UTC time source, or `None` when none is wired (the sim, tests) — the clock then
    /// stays whatever the user set by hand. On the device it's the SAM-M10Q's resolved time, used
    /// only when "Set from GPS" is on (issue #223).
    pub clock: Option<&'a mut dyn ClockSource>,
    /// The electronic compass, or `None` when no heading source is wired (tests, a host that
    /// only streams position) — the heading-up map then just holds north / the last GPS course
    /// while stopped, instead of following a magnetometer.
    pub compass: Option<&'a mut dyn CompassSource>,
    /// The recorded-track sink, or `None` when nothing is logging (the sim's manual panel,
    /// tests) — the ride then simply isn't recorded.
    pub track: Option<&'a mut dyn TrackSink>,
    /// The battery fuel gauge, or `None` when none is wired (tests) — the gauge then holds its
    /// last value (the boot stand-in). Read on the app's slow battery cadence (~30 s), on every
    /// screen, since the Home screensaver shows the battery while idle, not just while riding.
    pub fuel: Option<&'a mut dyn FuelGauge>,
}

/// Persistent store for the device [`Settings`] — the seam that keeps the *what* (the
/// settings model + its screens, all shared) apart from the *where* (file vs. on-chip RRAM).
///
/// The host owns the medium: the simulator reads/writes a file, the firmware a reserved
/// region of the nRF54L's on-chip RRAM — independent of the SD card, so settings survive a
/// reboot with no card present. The app seeds itself from [`load`](SettingsStore::load) at
/// boot (via [`App::set_settings`](crate::App::set_settings)) and asks the host to
/// [`save`](SettingsStore::save) whenever [`App::take_settings_dirty`](crate::App::take_settings_dirty)
/// reports a change — so persistence is the host's job and the shared layer stays oblivious to it.
pub trait SettingsStore {
    /// The persisted settings, or `None` when none are stored yet or the blob is unreadable
    /// (blank/corrupt) — the caller then starts from [`Settings::default`]. Implementations
    /// decode through [`settings::decode`](crate::settings::decode), so a `Some` is always valid.
    fn load(&mut self) -> Option<Settings>;
    /// Persist `s` (encoded via [`settings::encode`](crate::settings::encode)). Best-effort: a
    /// write failure is the host's to log, not the app's to handle — settings stay live in RAM
    /// regardless.
    fn save(&mut self, s: &Settings);
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
