//! Dependency-light semantic boundaries between the OpenBikeComputer core and its hosts.
//!
//! This crate owns values and narrow synchronous traits that cross the app/host boundary: sensor
//! samples, input edges, track points, settings values, and their sources/sinks. It deliberately
//! owns no drivers, buses, executor primitives, global mailboxes, UI policy, rendering, or
//! allocation. A board or host supplies implementations at its composition edge.

#![no_std]
#![forbid(unsafe_code)]

/// A position/orientation fix, however it was obtained.
///
/// Position is integer microdegrees (1e-6°), matching the persistent map formats and renderer.
/// `course` and `speed_mps` are optional because a real GPS only knows them while moving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fix {
    /// Latitude in microdegrees (1e-6°).
    pub lat: i32,
    /// Longitude in microdegrees (1e-6°).
    pub lon: i32,
    /// Course over ground in degrees clockwise from north, or `None` when unknown.
    pub course: Option<f32>,
    /// Ground speed in metres per second, or `None` when unknown.
    pub speed_mps: Option<f32>,
}

impl Fix {
    /// A stationary fix at `(lat, lon)` with no course or speed.
    #[inline]
    pub const fn at(lat: i32, lon: i32) -> Self {
        Self { lat, lon, course: None, speed_mps: None }
    }
}

/// Source of fresh location samples.
pub trait LocationSource {
    /// A fix only when a fresh sample is available this poll; `None` means no new sample.
    /// Identical-position stationary fixes remain fresh samples and must not be deduplicated.
    fn poll(&mut self) -> Option<Fix>;
}

/// Source of fresh barometric-altitude samples in metres.
///
/// The shipping board reads barometer and temperature coherently with each GPS fix. The port does
/// not prescribe that scheduling: callers still receive `Some` only for a fresh sample and `None`
/// between readings.
pub trait AltimeterSource {
    /// The next fresh barometric altitude, or `None` when no sample arrived.
    fn poll(&mut self) -> Option<f32>;
}

/// Source of fresh ambient-temperature samples in degrees Celsius.
pub trait TemperatureSource {
    /// The next fresh ambient temperature, or `None` when no sample arrived.
    fn poll(&mut self) -> Option<f32>;
}

/// A wall-clock date and time of day, at minute resolution.
///
/// This is also stored in the app settings model. Defining it here lets [`GpsTime`] and settings use
/// one nominal value without copying every fresh GPS timestamp across a crate-specific twin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    /// 1–12.
    pub month: u8,
    /// 1–the number of days in `month`.
    pub day: u8,
    /// 0–23.
    pub hour: u8,
    /// 0–59.
    pub minute: u8,
}

impl Default for DateTime {
    /// A neutral in-range stamp; a host or user supplies the real time.
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl DateTime {
    /// The [`Default`] stamp as a `const` — one link in the const `Settings::DEFAULT` chain
    /// (`obc-app`), which exists so the board can build its object store from a `.rodata` image
    /// instead of a stack temporary (the #1197 boot-chain fix).
    pub const DEFAULT: DateTime = DateTime { year: 2025, month: 1, day: 1, hour: 12, minute: 0 };
}

impl DateTime {
    /// Gregorian leap-year test.
    pub const fn is_leap(year: u16) -> bool {
        (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
    }

    /// Days in `month` of `year`, with a safe fallback for an unsanitized month.
    pub const fn month_len(year: u16, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if Self::is_leap(year) => 29,
            2 => 28,
            _ => 30,
        }
    }

    /// Re-pin an unsanitised `month`/`day` into the calendar, so a nonsense stamp can't smear the
    /// arithmetic below. An out-of-range `hour`/`minute` needs no pinning: it carries.
    fn pinned(self) -> Self {
        let month = self.month.clamp(1, 12);
        Self { month, day: self.day.clamp(1, Self::month_len(self.year, month)), ..self }
    }

    /// Advance this stamp by `mins`.
    ///
    /// O(1): the whole shift is one epoch-second round trip, not a day-at-a-time walk — this is on
    /// the per-frame path (`WallClock::now`, which Home and Map read every frame). The result
    /// saturates at the representable window's edges (`1970-01-01 00:00` / `2106-02-07 06:28`, the
    /// `u32` epoch-second ceiling) rather than wrapping; callers wanting a narrower calendar (the
    /// app's 2020–2099) clamp on top.
    pub fn add_minutes(self, mins: u32) -> Self {
        Self::from_unix_clamped(self.pinned().to_unix_i64() + mins as i64 * 60)
    }

    /// Shift this UTC stamp by a signed minute offset, carrying across dates. O(1) and saturating,
    /// on the same terms as [`add_minutes`](Self::add_minutes).
    pub fn with_offset(self, offset: i16) -> Self {
        Self::from_unix_clamped(self.pinned().to_unix_i64() + offset as i64 * 60)
    }

    /// Unix seconds at this stamp's `HH:MM:00`, interpreting it as UTC.
    pub fn to_unix(self) -> u32 {
        self.to_unix_i64() as u32
    }

    /// [`to_unix`](Self::to_unix) without the `u32` truncation: negative before the epoch and past
    /// `u32::MAX` beyond 2106, so the calendar arithmetic can clamp instead of wrap.
    fn to_unix_i64(self) -> i64 {
        let y = self.year as i64 - (self.month <= 2) as i64;
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = self.month as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86_400 + self.hour as i64 * 3_600 + self.minute as i64 * 60
    }

    /// [`from_unix`](Self::from_unix) over the signed range, pinning anything outside the
    /// representable window to its nearest edge instead of wrapping the `u32` conversion.
    fn from_unix_clamped(secs: i64) -> Self {
        Self::from_unix(secs.clamp(0, u32::MAX as i64) as u32)
    }

    /// Build a UTC date/time from Unix seconds, dropping seconds.
    pub fn from_unix(secs: u32) -> Self {
        let secs = secs as i64;
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
        let year = (y + (month <= 2) as i64) as u16;
        Self { year, month, day, hour: (rem / 3_600) as u8, minute: (rem % 3_600 / 60) as u8 }
    }
}

/// Why a [`SettingsStore::save`] failed — a bounded, `Copy` reason a host can carry back to the app
/// in a settings-persistence failure outcome without borrowing a
/// backend error. The variants are intentionally coarse: the app retries the same revision on any of
/// them, so a rider-visible advisory is all the detail the protocol needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSaveError {
    /// The backing store rejected or failed the write (RRAM line-write error, a file I/O failure, a
    /// full or absent medium). The live value stays authoritative in RAM; the app re-arms a retry.
    Backend,
}

/// Persistence for an owner-defined settings value.
///
/// `None` from [`load`](SettingsStore::load) means no valid persisted value is available.
/// [`save`](SettingsStore::save) returns a typed result so the app can acknowledge a durable write
/// and keep a failed one retryable (#810) — the live value remains authoritative in RAM regardless.
pub trait SettingsStore {
    /// The settings model owned by the consumer of this port.
    type Value;

    /// Load the persisted value, or `None` when storage is blank, invalid, or unavailable.
    fn load(&mut self) -> Option<Self::Value>;

    /// Persist `value`, reporting whether the write reached durable storage. A returned
    /// [`SettingsSaveError`] leaves the revision retryable; `Ok(())` is the app's cue to mark it
    /// acknowledged.
    fn save(&mut self, value: &Self::Value) -> Result<(), SettingsSaveError>;
}

/// How many discrete brightness steps a [`Backlight`] accepts: levels `0..BACKLIGHT_LEVELS`, from
/// the dimmest lit step to full. **There is no level that turns the light off** — a rider who
/// cannot read the panel cannot find the control that turns it back on.
pub const BACKLIGHT_LEVELS: u8 = 5;

/// A board that has no controllable panel light. Returned by every call of a [`Backlight`] the
/// hardware cannot honour, so a refusal is a value the caller can log rather than a silent no-op
/// that pretends the panel dimmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BacklightUnsupported;

/// The panel's brightness control.
///
/// **One call, three meanings**, sequenced by the caller — which is why there is no separate
/// `preview` / `commit` / `revert`. Each of the three is "drive the panel at this level now", and
/// the *only* difference is what the caller does with the value afterwards:
///
/// * **preview** — the rider stepped the editor: apply the staged level, persist nothing;
/// * **commit** — the rider confirmed: apply the same level, and the caller stores it;
/// * **revert** — the rider cancelled: apply the level that is still stored.
///
/// So a host that applies the app's answer every frame gets all three for free, and the port keeps
/// no state a cancel would have to unwind.
pub trait Backlight {
    /// Whether this panel has a light at all. `false` is a standing property of the hardware, not
    /// a transient state, and it means every [`apply`](Backlight::apply) refuses.
    ///
    /// It exists because a *refusal the rider cannot see* is not honesty. A host states this
    /// answer to the app once at composition, and the app removes the brightness control rather
    /// than drawing a slider that moves while nothing changes.
    fn available(&self) -> bool;

    /// Drive the panel at `level` (`0..`[`BACKLIGHT_LEVELS`], saturating above), or refuse when
    /// this hardware has no light to drive. Idempotent: applying the level already showing is a
    /// no-op, so a caller may apply every frame.
    ///
    /// Refusal and [`available`](Backlight::available) must agree: an implementation that answers
    /// `false` refuses every level, and one that answers `true` accepts every level in range.
    fn apply(&mut self, level: u8) -> Result<(), BacklightUnsupported>;
}

/// Turning the device off, for good — the port the quick drawer's guarded confirmation ends in.
///
/// It **does not return**: on hardware the part enters system-off and only a wake source restarts
/// it, and a host stands in with whatever ending is honest for it. The caller must therefore have
/// presented its last frame before calling, because nothing after the call runs.
pub trait PowerOff {
    /// Enter the deepest off state this device has.
    fn power_off(&mut self) -> !;
}

/// A resolved GPS UTC timestamp. Seconds remain separate because [`DateTime`] is minute-resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpsTime {
    pub utc: DateTime,
    /// Seconds into the current minute (0–59).
    pub second: u8,
}

/// Source of fresh resolved UTC timestamps.
pub trait ClockSource {
    /// The next fresh receiver timestamp, or `None` when no stamp arrived.
    fn poll(&mut self) -> Option<GpsTime>;
}

/// Source of fresh heart-rate samples in beats per minute.
pub trait HeartRateSource {
    /// The next fresh heart-rate sample, or `None` when no sample arrived.
    fn poll(&mut self) -> Option<u16>;
}

/// Source of fresh power samples in watts.
pub trait PowerSource {
    /// The next fresh power sample, or `None` when no sample arrived.
    fn poll(&mut self) -> Option<u16>;
}

/// Source of fresh cadence samples in revolutions per minute.
pub trait CadenceSource {
    /// The next fresh cadence sample, or `None` when no sample arrived. `Some(0)` means coasting.
    fn poll(&mut self) -> Option<u8>;
}

/// Source of fresh electronic-compass headings in degrees clockwise from north.
pub trait CompassSource {
    /// The next fresh heading, or `None` when no sample arrived.
    fn poll(&mut self) -> Option<f32>;
}

/// Source of battery state of charge in percent, called on the app's slow polling cadence.
pub trait FuelGauge {
    /// The latest available charge reading, or `None`; callers retain the last value.
    fn poll(&mut self) -> Option<u8>;
}

/// One accepted recorded fix and its sensor values.
///
/// The route crate re-exports this same nominal type and encodes it directly into the fixed-size
/// track record; there is no conversion or staging copy at the sink boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackPoint {
    /// Longitude in microdegrees.
    pub lon: i32,
    /// Latitude in microdegrees.
    pub lat: i32,
    /// Barometric elevation in metres.
    pub ele: i16,
    /// Milliseconds on the ride/sample clock.
    pub t_ms: u32,
    /// Starts a new track segment after a pause or GPS gap.
    pub segment_start: bool,
    /// Heart rate in bpm, or `None` when absent/stale.
    pub hr: Option<u8>,
    /// Cadence in rpm, or `None` when absent/stale.
    pub cadence: Option<u8>,
    /// Power in watts, or `None` when absent/stale.
    pub power: Option<u16>,
}

/// A track append could not be durably logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackError;

/// Sink for accepted recorded fixes.
pub trait TrackSink {
    /// Append one point durably; return `Err` rather than panicking when the medium fails.
    fn record(&mut self, point: TrackPoint) -> Result<(), TrackError>;
}

/// The polled sensor capabilities handed to the app each frame.
pub struct Sensors<'a> {
    /// User location source.
    pub loc: &'a mut dyn LocationSource,
    /// Optional barometric altitude source.
    pub altimeter: Option<&'a mut dyn AltimeterSource>,
    /// Optional ambient temperature source.
    pub temperature: Option<&'a mut dyn TemperatureSource>,
    /// Optional resolved GPS time source.
    pub clock: Option<&'a mut dyn ClockSource>,
    /// Optional electronic compass.
    pub compass: Option<&'a mut dyn CompassSource>,
    /// Optional recorded-track sink.
    pub track: Option<&'a mut dyn TrackSink>,
    /// Optional battery fuel gauge.
    pub fuel: Option<&'a mut dyn FuelGauge>,
    /// Optional heart-rate source.
    pub hr: Option<&'a mut dyn HeartRateSource>,
    /// Optional power source.
    pub power: Option<&'a mut dyn PowerSource>,
    /// Optional cadence source.
    pub cadence: Option<&'a mut dyn CadenceSource>,
}

impl<'a> Sensors<'a> {
    /// The location-only set: every optional capability absent. A host that has more fills them in
    /// with struct-update syntax (`Sensors { fuel: Some(g), ..Sensors::new(loc) }`), so adding a
    /// tenth optional here doesn't ripple through every caller.
    pub fn new(loc: &'a mut dyn LocationSource) -> Self {
        Sensors {
            loc,
            altimeter: None,
            temperature: None,
            clock: None,
            compass: None,
            track: None,
            fuel: None,
            hr: None,
            power: None,
            cadence: None,
        }
    }
}

/// A physical control button whose press **edges** the gesture layer times.
///
/// The device has four buttons: **Up** / **Down** on the left flank, **Select** / **Back** on the
/// right, and **all four** reach the gesture layer as edges. So one timing model — the step
/// cadence as well as the long-press threshold — lives in the shared recognizer, and a host that
/// models real buttons gets exactly the device's behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// The left-flank **Up** button (upper): previous / decrease.
    Up,
    /// The left-flank **Down** button (lower): next / increase.
    Down,
    /// The right-flank **Select** button (upper): confirm / open.
    Select,
    /// The right-flank **Back** button (lower).
    Back,
}

/// A press or release edge for one [`Button`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    /// Press edge.
    Down(Button),
    /// Release edge.
    Up(Button),
}

/// A raw control event before gesture recognition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Signed selection steps **injected directly**, bypassing the Up/Down edges: **negative = Up**
    /// ("previous"), **positive = Down** ("next"). This is the variant for the sources a device has
    /// no button for — a mouse wheel, a `--script` recipe, the debug link's `K t` line — and it may
    /// carry a batched jump of several steps at once. Physical buttons never use it: they forward
    /// [`Button::Up`] / [`Button::Down`] edges and the recognizer derives the steps.
    Step(i32),
    /// One button edge.
    Button(ButtonEvent),
}

/// Source of pending raw control events.
pub trait InputSource {
    /// The next queued event, or `None` once the queue is drained for this tick.
    fn poll(&mut self) -> Option<InputEvent>;
}

/// Milliseconds from the clock consistent with sensor samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RideClock(pub u32);

/// Milliseconds from the host/MCU monotonic wall clock used for input timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct InputClock(pub u32);
