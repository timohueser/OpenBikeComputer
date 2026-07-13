//! Persistent device settings + their byte codec.
//!
//! [`Settings`] is the small POD the settings screens edit and the host persists across a reboot.
//! It is `Copy + PartialEq`, so [`App::apply_gesture`](crate::App::apply_gesture) detects a change
//! with a single comparison and flags a save. The byte codec ([`encode`]/[`decode`]) is a
//! versioned, CRC-checked, fixed-length blob shared by **both** stores (sim file, firmware RRAM
//! region — see [`SettingsStore`](obc_ports::SettingsStore)), so a blank or corrupt read falls
//! back to [`Settings::default`] rather than loading garbage.

use crate::i18n::{t, Msg};
use crate::stat_fields::{StatFieldList, MAX_STAT_FIELDS};

pub use obc_ports::DateTime;

/// First year accepted by the settings codec and Date & Time editor.
pub const DATETIME_MIN_YEAR: u16 = 2020;
/// Last year accepted by the settings codec and Date & Time editor.
pub const DATETIME_MAX_YEAR: u16 = 2099;

/// App-owned editing and persisted-value policy for the dependency-neutral [`DateTime`].
///
/// Calendar arithmetic (`add_minutes`, UTC offsets, leap years) stays inherent on `DateTime` in
/// `obc-ports`; these methods are available when this trait is in scope because their wrapping and
/// sanitising ranges are UI/storage choices specific to OpenBikeComputer.
pub trait DateTimeEditorExt {
    /// Force every field into the range accepted by the settings codec and editor.
    fn sanitize(&mut self);
    /// Step the year, wrapping through the app-supported range and re-clamping the day.
    fn step_year(&mut self, n: i32);
    /// Step the month, wrapping 1–12 and re-clamping the day.
    fn step_month(&mut self, n: i32);
    /// Step the day, wrapping within the current month.
    fn step_day(&mut self, n: i32);
    /// Step the hour, wrapping 0–23.
    fn step_hour(&mut self, n: i32);
    /// Step the minute, wrapping 0–59.
    fn step_minute(&mut self, n: i32);
}

impl DateTimeEditorExt for DateTime {
    fn sanitize(&mut self) {
        self.year = self.year.clamp(DATETIME_MIN_YEAR, DATETIME_MAX_YEAR);
        self.month = self.month.clamp(1, 12);
        self.hour = self.hour.min(23);
        self.minute = self.minute.min(59);
        clamp_day(self);
    }

    fn step_year(&mut self, n: i32) {
        self.year = wrap_inclusive(self.year, n, DATETIME_MIN_YEAR, DATETIME_MAX_YEAR);
        clamp_day(self);
    }

    fn step_month(&mut self, n: i32) {
        self.month = wrap_inclusive(self.month as u16, n, 1, 12) as u8;
        clamp_day(self);
    }

    fn step_day(&mut self, n: i32) {
        self.day = wrap_inclusive(self.day as u16, n, 1, DateTime::month_len(self.year, self.month) as u16) as u8;
    }

    fn step_hour(&mut self, n: i32) {
        self.hour = wrap_inclusive(self.hour as u16, n, 0, 23) as u8;
    }

    fn step_minute(&mut self, n: i32) {
        self.minute = wrap_inclusive(self.minute as u16, n, 0, 59) as u8;
    }
}

fn clamp_day(date: &mut DateTime) {
    date.day = date.day.clamp(1, DateTime::month_len(date.year, date.month));
}

fn wrap_inclusive(value: u16, step: i32, lo: u16, hi: u16) -> u16 {
    let span = (hi - lo) as i32 + 1;
    let offset = (value as i32 - lo as i32 + step).rem_euclid(span);
    (lo as i32 + offset) as u16
}

fn clamp_app_year(date: DateTime) -> DateTime {
    if date.year < DATETIME_MIN_YEAR {
        DateTime { year: DATETIME_MIN_YEAR, month: 1, day: 1, ..date }
    } else if date.year > DATETIME_MAX_YEAR {
        DateTime { year: DATETIME_MAX_YEAR, month: 12, day: 31, ..date }
    } else {
        date
    }
}

/// Advance a live app clock while retaining the settings model's bounded year behavior.
pub(crate) fn add_minutes_bounded(date: DateTime, mins: u32) -> DateTime {
    clamp_app_year(date.add_minutes(mins))
}

/// Apply the user's UTC offset while retaining the settings model's bounded year behavior.
fn with_offset_bounded(date: DateTime, offset: i16) -> DateTime {
    clamp_app_year(date.with_offset(offset))
}

/// The localized three-letter month name for a dependency-neutral calendar value.
pub(crate) fn month_name(date: DateTime, lang: Language) -> &'static str {
    const MONTHS: [Msg; 12] = [
        Msg::MonthJan,
        Msg::MonthFeb,
        Msg::MonthMar,
        Msg::MonthApr,
        Msg::MonthMay,
        Msg::MonthJun,
        Msg::MonthJul,
        Msg::MonthAug,
        Msg::MonthSep,
        Msg::MonthOct,
        Msg::MonthNov,
        Msg::MonthDec,
    ];
    t(MONTHS[(date.month.clamp(1, 12) - 1) as usize], lang)
}

/// Measurement system for the ride readouts. Re-captions and re-scales the
/// [`Statistics`](crate::screen) tiles and the off-route distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Units {
    /// km / km·h⁻¹ / m — the default.
    #[default]
    Metric,
    /// mi / mi·h⁻¹ / ft.
    Imperial,
}

/// The device-name byte cap — the BLE Config name field (matches the OBCR route-name cap).
pub const DEVICE_NAME_MAX: usize = 48;

/// The user-facing device name. A fixed inline buffer so [`Settings`] stays `Copy`; **empty means
/// "factory name"** — the BLE edge substitutes its serial-derived `OBC-XXXX` — so a fresh device
/// needs no name stored and a rename can be cleared back to factory by writing an empty name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceName {
    len: u8,
    bytes: [u8; DEVICE_NAME_MAX],
}

impl Default for DeviceName {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl DeviceName {
    /// The factory-name sentinel (see the type doc).
    pub const EMPTY: DeviceName = DeviceName { len: 0, bytes: [0; DEVICE_NAME_MAX] };

    /// Store `name`, truncated to the byte cap **on a char boundary** (never mid-UTF-8) —
    /// lossy by design, hence not the std `FromStr` shape.
    pub fn from_str_lossy(name: &str) -> DeviceName {
        let mut end = name.len().min(DEVICE_NAME_MAX);
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut n = Self::EMPTY;
        n.len = end as u8;
        n.bytes[..end].copy_from_slice(&name.as_bytes()[..end]);
        n
    }

    /// Rebuild from stored bytes (the codec's decode path): over-long or invalid-UTF-8 input —
    /// a corrupt or foreign blob that still passed the CRC — sanitises to [`Self::EMPTY`]
    /// (factory name), never to garbage the BLE edge would advertise.
    pub fn from_bytes(bytes: &[u8]) -> DeviceName {
        if bytes.len() > DEVICE_NAME_MAX || core::str::from_utf8(bytes).is_err() {
            return Self::EMPTY;
        }
        let mut n = Self::EMPTY;
        n.len = bytes.len() as u8;
        n.bytes[..bytes.len()].copy_from_slice(bytes);
        n
    }

    /// The stored name — `""` means factory.
    pub fn as_str(&self) -> &str {
        // Every constructor stored validated UTF-8, so this cannot fail.
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    /// True when no user name is stored (the BLE edge advertises the factory name).
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// Miles per kilometre — the distance/speed conversion factor (also mi·h⁻¹ per km·h⁻¹).
pub const MI_PER_KM: f32 = 0.621_371;
/// Feet per metre — the elevation/climb conversion factor.
pub const FT_PER_M: f32 = 3.280_84;
/// Feet in a mile — the cross-over from a "NNNft" to a "NNmi" off-route readout.
pub const FT_PER_MI: u32 = 5280;

impl Units {
    /// Whether imperial units are selected (the conversions below are no-ops otherwise).
    #[inline]
    pub const fn is_imperial(self) -> bool {
        matches!(self, Units::Imperial)
    }

    /// Convert a distance in km to the selected unit (km or mi).
    #[inline]
    pub fn dist(self, km: f32) -> f32 {
        if self.is_imperial() {
            km * MI_PER_KM
        } else {
            km
        }
    }

    /// Convert a speed in km·h⁻¹ to the selected unit (km·h⁻¹ or mi·h⁻¹).
    #[inline]
    pub fn speed(self, kmh: f32) -> f32 {
        if self.is_imperial() {
            kmh * MI_PER_KM
        } else {
            kmh
        }
    }

    /// Convert an elevation/climb in metres to the selected unit (m or ft).
    #[inline]
    pub fn elev(self, m: f32) -> f32 {
        if self.is_imperial() {
            m * FT_PER_M
        } else {
            m
        }
    }

    /// Speed-tile caption (`KPH` / `MPH`).
    #[inline]
    pub const fn speed_label(self) -> &'static str {
        if self.is_imperial() {
            "MPH"
        } else {
            "KPH"
        }
    }

    /// Distance-tile caption prefix (`KM` / `MI`).
    #[inline]
    pub const fn dist_label(self) -> &'static str {
        if self.is_imperial() {
            "MI"
        } else {
            "KM"
        }
    }

    /// Elevation readout suffix (`m` / `ft`).
    #[inline]
    pub const fn elev_label(self) -> &'static str {
        if self.is_imperial() {
            "ft"
        } else {
            "m"
        }
    }

    /// The label for the Units screen's value row (`Metric` / `Imperial`), in the UI `lang`
    /// (epic #602). Word-bearing, so it routes through the catalog; the symbol labels
    /// ([`speed_label`](Units::speed_label) etc.) stay language-independent.
    #[inline]
    pub const fn name(self, lang: Language) -> &'static str {
        if self.is_imperial() {
            t(Msg::UnitsImperial, lang)
        } else {
            t(Msg::UnitsMetric, lang)
        }
    }

    /// Flip to the other system — the Units screen's one action.
    #[inline]
    pub const fn toggled(self) -> Self {
        if self.is_imperial() {
            Units::Metric
        } else {
            Units::Imperial
        }
    }
}

/// How the Climb screen (epic #506) is reached. A device-only setting (the Stats settings screen
/// cycles it), persisted in the settings codec next to [`ble_enabled`](Settings::ble_enabled).
///
/// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
/// byte always decodes to the same mode (an unknown byte sanitises to the default, [`Auto`]).
///
/// [`Auto`]: ClimbMode::Auto
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClimbMode {
    /// The Climb screen is disabled: it's kept out of the Back-cycle entirely (Map ↔ Statistics
    /// only) and never auto-shown.
    Off = 0,
    /// The Climb screen is in the Back-cycle when a climb is active, but the device never switches
    /// to it on its own — the rider reaches it by cycling Back.
    Manual = 1,
    /// The Climb screen is in the Back-cycle **and** the device auto-switches to it on climb entry
    /// (from a riding view) and auto-returns to the Map on the crest — the headline behavior.
    Auto = 2,
}

impl Default for ClimbMode {
    /// **Auto** out of the box — the climb panel is self-discovering (it shows itself on the first
    /// climb). Easily changed here if a quieter default is wanted.
    fn default() -> Self {
        ClimbMode::Auto
    }
}

impl ClimbMode {
    /// The label for the Stats screen's Climb-panel row (`Off` / `Manual` / `Auto`), in the UI
    /// `lang` (epic #602).
    #[inline]
    pub const fn name(self, lang: Language) -> &'static str {
        match self {
            ClimbMode::Off => t(Msg::ClimbModeOff, lang),
            ClimbMode::Manual => t(Msg::ClimbModeManual, lang),
            ClimbMode::Auto => t(Msg::ClimbModeAuto, lang),
        }
    }

    /// Whether the Climb screen belongs in the Back-cycle at all — false only for [`Off`](ClimbMode::Off).
    #[inline]
    pub const fn is_on(self) -> bool {
        !matches!(self, ClimbMode::Off)
    }

    /// The next mode in the Off → Manual → Auto → Off ring — the Stats row's one action (a turn or
    /// press steps it).
    #[inline]
    pub const fn cycled(self) -> Self {
        match self {
            ClimbMode::Off => ClimbMode::Manual,
            ClimbMode::Manual => ClimbMode::Auto,
            ClimbMode::Auto => ClimbMode::Off,
        }
    }

    /// Rebuild from a stored byte, sanitising an unknown value to the default ([`Auto`](ClimbMode::Auto))
    /// — the decode-side clamp, exactly like the other codec fields.
    #[inline]
    fn from_byte(b: u8) -> Self {
        match b {
            0 => ClimbMode::Off,
            1 => ClimbMode::Manual,
            2 => ClimbMode::Auto,
            _ => ClimbMode::default(),
        }
    }
}

/// Whether — and when — the Map's bottom-centre **waypoint chip** (epic #523) is shown: the calm
/// `◆ NAME  <dist>` pill counting the along-route distance to the next named waypoint ahead. A
/// device-only setting (the Stats settings screen cycles it), persisted in the codec next to
/// [`climb_mode`](Settings::climb_mode).
///
/// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
/// byte always decodes to the same mode (an unknown byte sanitises to the default, [`Approach`]).
///
/// [`Approach`]: WaypointMode::Approach
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WaypointMode {
    /// The chip is never shown — the silencer for routes carrying junk/artifact waypoints from a
    /// planner's GPX export (a whole route of them can be muted here).
    Off = 0,
    /// The chip appears only as the next waypoint nears — within the approach radius
    /// (`WAYPOINT_APPROACH_M`, 500 m) ahead — counting the distance down, so a stop is noticed
    /// without standing chrome. **The default** (discoverability won over the conservative `Off`).
    Approach = 1,
    /// The chip is shown whenever a named waypoint lies ahead (subject to the shared
    /// no-fix / off-route / pan suppression), reading the along-route distance to it.
    Always = 2,
}

impl Default for WaypointMode {
    /// **Approach** out of the box — the calm middle ground: the chip surfaces as a waypoint nears
    /// (so the feature is self-discovering) but stays down the rest of the time. Locked 2026-07-08.
    fn default() -> Self {
        WaypointMode::Approach
    }
}

impl WaypointMode {
    /// The label for the Stats screen's Waypoints-panel row (`Off` / `Approach` / `Always`), in
    /// the UI `lang` (epic #602).
    #[inline]
    pub const fn name(self, lang: Language) -> &'static str {
        match self {
            WaypointMode::Off => t(Msg::WaypointModeOff, lang),
            WaypointMode::Approach => t(Msg::WaypointModeApproach, lang),
            WaypointMode::Always => t(Msg::WaypointModeAlways, lang),
        }
    }

    /// The next mode in the Off → Approach → Always → Off ring — the Stats row's one action (a turn
    /// or press steps it).
    #[inline]
    pub const fn cycled(self) -> Self {
        match self {
            WaypointMode::Off => WaypointMode::Approach,
            WaypointMode::Approach => WaypointMode::Always,
            WaypointMode::Always => WaypointMode::Off,
        }
    }

    /// Rebuild from a stored byte, sanitising an unknown value to the default
    /// ([`Approach`](WaypointMode::Approach)) — the decode-side clamp, exactly like the other codec
    /// fields.
    #[inline]
    fn from_byte(b: u8) -> Self {
        match b {
            0 => WaypointMode::Off,
            1 => WaypointMode::Approach,
            2 => WaypointMode::Always,
            _ => WaypointMode::default(),
        }
    }
}

/// How long the UI sits idle (no user input) before it navigates itself back to where it belongs —
/// the Home root when not tracking a ride, the Map when a ride is running (see
/// [`App::apply_idle_return`](crate::App::apply_idle_return)). A device-only setting, cycled by the
/// Power settings screen's value picker and persisted in the codec next to
/// [`climb_mode`](Settings::climb_mode).
///
/// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
/// byte always decodes to the same value (an unknown byte sanitises to the default, [`S30`]).
///
/// [`S30`]: IdleReturn::S30
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IdleReturn {
    /// 15 seconds.
    S15 = 0,
    /// 30 seconds — the default.
    S30 = 1,
    /// 1 minute.
    M1 = 2,
    /// 5 minutes.
    M5 = 3,
    /// Never — the idle-return mechanism is disabled entirely.
    Never = 4,
}

impl Default for IdleReturn {
    /// **30 s** out of the box — long enough not to yank an attentive rider mid-glance, short enough
    /// that a device left in a menu drifts back to a useful screen on its own.
    fn default() -> Self {
        IdleReturn::S30
    }
}

/// Walk `order` `n` detents from `cur`, wrapping at both ends — the shared value-picker step behind
/// every ordered enum row (Language, IdleReturn, …). Mirrors the list cursor's
/// [`step_selection`](crate::screen::list::step_selection) `rem_euclid` wrap, but on the value array
/// rather than a bare index. `cur` missing from `order` falls back to `fallback` (each caller's
/// default index); in practice `order` lists every variant, so that arm is unreachable.
fn step_order<T: Copy + PartialEq, const N: usize>(order: &[T; N], cur: T, n: i32, fallback: usize) -> T {
    let i = order.iter().position(|&v| v == cur).unwrap_or(fallback);
    order[(i as i32 + n).rem_euclid(N as i32) as usize]
}

impl IdleReturn {
    /// The ordered picker values (the left/right walk order), shortest to `Never`.
    const ORDER: [IdleReturn; 5] =
        [IdleReturn::S15, IdleReturn::S30, IdleReturn::M1, IdleReturn::M5, IdleReturn::Never];

    /// The label for the Power screen's value picker (`15 s` / `30 s` / `1 min` / `5 min` /
    /// `Never`), in the UI `lang` (epic #602). `Never` is a word; the durations are unit-glued
    /// numbers, catalogued whole so a language can localize the `s`/`min` grain if it ever needs to.
    #[inline]
    pub const fn name(self, lang: Language) -> &'static str {
        match self {
            IdleReturn::S15 => t(Msg::IdleS15, lang),
            IdleReturn::S30 => t(Msg::IdleS30, lang),
            IdleReturn::M1 => t(Msg::IdleM1, lang),
            IdleReturn::M5 => t(Msg::IdleM5, lang),
            IdleReturn::Never => t(Msg::IdleNever, lang),
        }
    }

    /// The idle timeout in millis, or `None` for [`Never`](IdleReturn::Never) (the mechanism is
    /// off). `None` also disables the idle wake, so a parked device isn't woken to no purpose.
    #[inline]
    pub const fn timeout_ms(self) -> Option<u32> {
        match self {
            IdleReturn::S15 => Some(15_000),
            IdleReturn::S30 => Some(30_000),
            IdleReturn::M1 => Some(60_000),
            IdleReturn::M5 => Some(300_000),
            IdleReturn::Never => None,
        }
    }

    /// Walk the picker `n` detents through [`ORDER`](IdleReturn::ORDER), wrapping at both ends — the
    /// Power row's left/right value step. Falls back to the default [`S30`](IdleReturn::S30) index.
    #[inline]
    pub fn stepped(self, n: i32) -> Self {
        step_order(&Self::ORDER, self, n, 1)
    }

    /// Rebuild from a stored byte, sanitising an unknown value to the default
    /// ([`S30`](IdleReturn::S30)) — the decode-side clamp, exactly like the other codec fields.
    #[inline]
    fn from_byte(b: u8) -> Self {
        match b {
            0 => IdleReturn::S15,
            1 => IdleReturn::S30,
            2 => IdleReturn::M1,
            3 => IdleReturn::M5,
            4 => IdleReturn::Never,
            _ => IdleReturn::default(),
        }
    }
}

/// The UI language (epic #602). A device-only setting, cycled by the Language settings screen's
/// value picker and persisted in the codec next to [`waypoint_mode`](Settings::waypoint_mode). Only
/// the on-glass **preference** ships here (L1); the translation catalog that actually reads it lands
/// later in the epic — until then every string stays English regardless of this value.
///
/// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
/// byte always decodes to the same language (an unknown byte sanitises to the default, [`En`]).
///
/// [`En`]: Language::En
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Language {
    /// English — the default.
    En = 0,
    /// German.
    De = 1,
    /// French.
    Fr = 2,
    /// Spanish.
    Es = 3,
}

impl Default for Language {
    /// **English** out of the box — the language every string is authored in; the other three are
    /// opt-in once the catalog lands.
    fn default() -> Self {
        Language::En
    }
}

impl Language {
    /// The number of variants — and the number of columns the i18n catalog must ship. A static
    /// assertion in [`i18n`](crate::i18n) ties `TABLE`'s column count to this, so the "index never
    /// panics" contract of [`t`](crate::i18n::t) is compiler-enforced: a fifth variant added
    /// without a fifth `{lang}.toml` column fails the build instead of panicking on the first draw
    /// (#614). Because [`ORDER`](Language::ORDER) is `[Language; COUNT]` and the picker only ever
    /// selects out of it, [`Settings::language`](crate::Settings::language) is always in range.
    pub const COUNT: usize = 4;

    /// The ordered picker values (the left/right walk order), English first. Sized `[_; COUNT]`, so
    /// wiring a newly-added variant into the picker without bumping [`COUNT`](Language::COUNT) — and
    /// thus adding its catalog column — won't compile.
    const ORDER: [Language; Self::COUNT] = [Language::En, Language::De, Language::Fr, Language::Es];

    /// The label for the Language screen's value picker — each language's **endonym** (its own name
    /// for itself), so the row reads to a speaker who can't yet read the current UI language. The
    /// accented forms (`Français` / `Español`) render via the Latin font extension (#601).
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            Language::En => "English",
            Language::De => "Deutsch",
            Language::Fr => "Français",
            Language::Es => "Español",
        }
    }

    /// Walk the picker `n` detents through [`ORDER`](Language::ORDER), wrapping at both ends — the
    /// Language row's left/right value step. Falls back to the default [`En`](Language::En) index.
    #[inline]
    pub fn stepped(self, n: i32) -> Self {
        step_order(&Self::ORDER, self, n, 0)
    }

    /// The next language in the ring — the Language row's press action (one detent forward, like a
    /// [`stepped(1)`](Language::stepped)).
    #[inline]
    pub fn cycled(self) -> Self {
        self.stepped(1)
    }

    /// Rebuild from a stored byte, sanitising an unknown value to the default ([`En`](Language::En))
    /// — the decode-side clamp, exactly like the other codec fields.
    #[inline]
    fn from_byte(b: u8) -> Self {
        match b {
            0 => Language::En,
            1 => Language::De,
            2 => Language::Fr,
            3 => Language::Es,
            _ => Language::default(),
        }
    }
}

/// UTC-offset stepper bounds + granularity (minutes). 15-minute steps cover the real-world
/// `:30` / `:45` zones (India +5:30, Nepal +5:45) over the −12:00…+14:00 span.
pub const UTC_OFFSET_MIN: i16 = -12 * 60;
pub const UTC_OFFSET_MAX: i16 = 14 * 60;
pub const UTC_OFFSET_STEP: i16 = 15;

/// GPS-fix-interval stepper bounds (seconds). The step itself *adapts* (1 s up to 10 s, then
/// 5 s) — see [`PowerScreen`](crate::screen) — so a long interval is a few detents, not dozens.
pub const FIX_INTERVAL_MIN: u16 = 1;
pub const FIX_INTERVAL_MAX: u16 = 120;

/// Stats-grid page auto-cycle period stepper bounds (seconds). With the elevation chart keeping the
/// encoder's `turn`/`hold`, a second page is only reachable by the auto-cycle — so there's no "off",
/// the minimum is a brisk-but-readable 2 s.
pub const STAT_CYCLE_MIN: u16 = 2;
pub const STAT_CYCLE_MAX: u16 = 20;
/// Default auto-cycle period — only matters once a rider pins more than one page of fields.
pub const STAT_CYCLE_DEFAULT: u16 = 5;

/// The fixed sensor-slot count (BLE sensors epic #707): one saved sensor per quantity — index
/// **0 HR · 1 Power · 2 Cadence**. The slot index *is* the kind, so the kind isn't stored.
pub const SENSOR_SLOTS: usize = 3;

/// A saved BLE sensor (SE7, epic #707) for one quantity slot: the stored advertising address the
/// board's central manager reconnects by (auto-reconnect across a reboot). The slot index carries the
/// kind (HR / power / cadence), so only the address is stored; there is **no name or bond** — v1
/// sensors are open GATT servers connected by address (locked: no sensor SMP). `Copy + Eq` so the
/// whole [`Settings`] stays `Copy + Eq` and the one-`==` settings-dirty check still holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SavedSensor {
    /// Whether this slot holds a saved sensor. `false` = empty (the address is then unused zeros),
    /// which is a fresh device's state and what an older-blob reset decodes to.
    pub present: bool,
    /// The advertiser address kind: `0` = public, `1` = random. Kept so the manager reconnects by the
    /// *same* address kind — a static-random watch (a broadcast Garmin) advertises `RANDOM`.
    pub addr_kind: u8,
    /// The 6-byte advertising address, little-endian as the wire carries it.
    pub addr: [u8; 6],
}

impl SavedSensor {
    /// The empty slot (no sensor saved) — the per-slot default.
    pub const EMPTY: SavedSensor = SavedSensor { present: false, addr_kind: 0, addr: [0; 6] };

    /// A present slot for `addr` of kind `addr_kind` (`0` public / `1` random) — the Sensors screen's
    /// pair write.
    pub const fn saved(addr_kind: u8, addr: [u8; 6]) -> SavedSensor {
        SavedSensor { present: true, addr_kind, addr }
    }
}

/// The whole persisted settings set. Plain old data — `Copy` + `Eq`, no floats — so a
/// before/after `==` flags a save and the codec is a trivial field-by-field pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Metric or imperial readouts.
    pub units: Units,
    /// `Set from GPS`: when set, the clock is GPS-stamped and only [`utc_offset_min`] is the
    /// user's; when clear, [`clock`] is set by hand.
    ///
    /// [`utc_offset_min`]: Settings::utc_offset_min
    /// [`clock`]: Settings::clock
    pub gps_time: bool,
    /// The manually-set (or last GPS-stamped) local date/time.
    pub clock: DateTime,
    /// Local time's offset from UTC, in minutes (`+02:00` → `120`).
    pub utc_offset_min: i16,
    /// Seconds between GPS fixes (the Power screen's interval).
    pub fix_interval_s: u16,
    /// GPS low-power mode (the Power screen's toggle).
    pub power_saver: bool,
    /// The rider's ordered Statistics-grid field selection (the Stat Fields screen edits it).
    pub stat_fields: StatFieldList,
    /// Seconds the Statistics grid dwells on each page before auto-cycling to the next.
    pub stat_cycle_s: u16,
    /// The user-facing device name (empty = factory `OBC-XXXX`). Written by the companion app over
    /// BLE, not any on-device screen — it lives here so the one settings blob persists it.
    pub device_name: DeviceName,
    /// The Bluetooth radio switch (the Bluetooth screen's toggle, epic #447 P8). Off = stop
    /// advertising + drop any live connection; on = the normal advertising lifecycle. **Device-only**
    /// — deliberately *not* one of the BLE-writable fields [`adopt_ble_fields`](Settings::adopt_ble_fields)
    /// pulls across (a phone must never be able to switch the radio out from under the rider, and
    /// couldn't turn it back on). Default **on**.
    pub ble_enabled: bool,
    /// How the Climb screen (epic #506) is reached — Off / Manual / Auto (the Stats settings screen
    /// cycles it). **Device-only**, like [`ble_enabled`](Settings::ble_enabled): deliberately *not*
    /// one of the BLE-writable fields [`adopt_ble_fields`](Settings::adopt_ble_fields) pulls across.
    /// Default **Auto** — the climb panel auto-shows on the first climb.
    pub climb_mode: ClimbMode,
    /// How long the UI sits idle before it navigates itself back to where it belongs (Home when not
    /// tracking, the Map mid-ride). **Device-only**, like [`climb_mode`](Settings::climb_mode):
    /// deliberately *not* one of the BLE-writable fields [`adopt_ble_fields`](Settings::adopt_ble_fields)
    /// pulls across. Default **30 s**; [`Never`](IdleReturn::Never) disables it entirely.
    pub idle_return: IdleReturn,
    /// Show the small floating `HH:MM` clock on the Map (the Display settings screen's toggle).
    /// **Device-only**, like [`climb_mode`](Settings::climb_mode): deliberately *not* one of the
    /// BLE-writable fields [`adopt_ble_fields`](Settings::adopt_ble_fields) pulls across. Default
    /// **on**.
    pub map_clock: bool,
    /// Show the scale bar at the Map's bottom-left (the Display settings screen's toggle).
    /// **Device-only**, like [`map_clock`](Settings::map_clock). Default **on**.
    pub map_scale_bar: bool,
    /// The rider's selected routing profile, an **index** into the loaded map's §8.6 profile table
    /// (N2/N5, epic #533). The Bike-type settings screen cycles it through the map's profile *names*;
    /// the planner is constructed with it ([`NavPlanner::new`](obc_route::NavPlanner)). Stored as a
    /// bare `u8` because the profile table is the map's, not the device's: a map with fewer profiles
    /// than this index falls back to profile 0 **at plan time** (guaranteed in the router, N3) and the
    /// UI renders profile 0's name for it so the rider isn't lied to (see
    /// [`NavProfiles`](crate::NavProfiles)). Not range-clamped on decode for that reason — the value
    /// only means anything against a map. **Device-only** (a bike type is picked on the device), so
    /// [`adopt_ble_fields`](Settings::adopt_ble_fields) never pulls it across. Default **0**.
    pub bike_profile_idx: u8,
    /// Whether — and when — the Map's bottom-centre waypoint chip appears (epic #523, the Stats
    /// settings screen cycles it). **Device-only**, like [`climb_mode`](Settings::climb_mode):
    /// deliberately *not* one of the BLE-writable fields [`adopt_ble_fields`](Settings::adopt_ble_fields)
    /// pulls across — a BLE Config write must never flip the rider's on-glass chrome. Default
    /// **Approach** (the chip surfaces only as a waypoint nears).
    pub waypoint_mode: WaypointMode,
    /// The UI language (epic #602, the Language settings screen cycles it). **Device-only**, like
    /// [`climb_mode`](Settings::climb_mode): deliberately *not* one of the BLE-writable fields
    /// [`adopt_ble_fields`](Settings::adopt_ble_fields) pulls across — the phone never repicks the
    /// rider's on-device language. Default **English**; every user-facing string is looked up in
    /// this language via [`t`](crate::i18n::t) at draw time.
    pub language: Language,
    /// The saved BLE sensors (SE7, epic #707), one slot per quantity — index **0 HR · 1 Power ·
    /// 2 Cadence**. An empty slot ([`SavedSensor::present`] `== false`) is "no sensor saved". Written
    /// by the Sensors settings screen on pair/forget; the board's central manager reconnects to a
    /// present slot's address whenever the radio is on. **Device-only**, like
    /// [`ble_enabled`](Settings::ble_enabled): never pulled across by
    /// [`adopt_ble_fields`](Settings::adopt_ble_fields) — a phone can't repick the rider's sensors.
    /// Default: all three slots empty.
    pub saved_sensors: [SavedSensor; SENSOR_SLOTS],
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            units: Units::Metric,
            gps_time: false,
            clock: DateTime::default(),
            utc_offset_min: 0,
            fix_interval_s: 1,
            power_saver: false,
            stat_fields: StatFieldList::default(),
            stat_cycle_s: STAT_CYCLE_DEFAULT,
            device_name: DeviceName::EMPTY,
            ble_enabled: true,
            climb_mode: ClimbMode::default(),
            idle_return: IdleReturn::default(),
            map_clock: true,
            map_scale_bar: true,
            bike_profile_idx: 0,
            waypoint_mode: WaypointMode::default(),
            language: Language::default(),
            saved_sensors: [SavedSensor::EMPTY; SENSOR_SLOTS],
        }
    }
}

impl Settings {
    /// The **local** wall-clock set-point the device shows: [`clock`](Settings::clock) verbatim in
    /// manual mode, or — when GPS-stamped ([`gps_time`](Settings::gps_time)) — the UTC anchor
    /// shifted into local time by [`utc_offset_min`](Settings::utc_offset_min) (via
    /// calendar offset operation, so a shift across midnight rolls the date too). In manual mode the
    /// clock is already local, so the offset is deliberately *not* applied (it would double-count).
    pub fn local_clock(&self) -> DateTime {
        if self.gps_time {
            with_offset_bounded(self.clock, self.utc_offset_min)
        } else {
            self.clock
        }
    }

    /// Adopt the **BLE-writable** fields — `units` and `device_name` — from `other`, leaving every
    /// on-device-only field (clock, GPS interval, power-saver, stat grid) untouched.
    ///
    /// This is the *phone → device* half of settings coherence (#456). The companion app can write
    /// units + name over BLE Config; that write lands in the persistent store, and the live app copy
    /// must adopt it same-session — both so the UI re-captions and so the app's next
    /// change-detection save doesn't clobber the phone's write with its own stale copy. The merge is
    /// deliberately narrow: only the two fields BLE actually owns are pulled across, so a BLE write
    /// racing an in-flight on-device edit of an *unrelated* field can't stomp it. Only the settings
    /// screens mutate the on-device-only fields (the invariant `take_settings_dirty` already relies
    /// on), and BLE only ever writes these two — so field-by-field is the correct grain.
    pub fn adopt_ble_fields(&mut self, other: &Settings) {
        self.units = other.units;
        self.device_name = other.device_name;
    }

    /// Clamp every field into its valid range — applied after a decode (see [`decode`]). The
    /// `stat_fields` selection is sanitised by [`StatFieldList::decode`] as it is parsed.
    fn sanitize(&mut self) {
        self.clock.sanitize();
        self.utc_offset_min = self.utc_offset_min.clamp(UTC_OFFSET_MIN, UTC_OFFSET_MAX);
        self.fix_interval_s = self.fix_interval_s.clamp(FIX_INTERVAL_MIN, FIX_INTERVAL_MAX);
        self.stat_cycle_s = self.stat_cycle_s.clamp(STAT_CYCLE_MIN, STAT_CYCLE_MAX);
    }
}

/// Codec version — bump when the byte layout changes; [`decode`] rejects any other version (the
/// host then falls back to [`Settings::default`], i.e. settings reset on a format change).
/// v4 appended the `ble_enabled` byte (#455); v5 appended the `climb_mode` byte (#511); v6 appended
/// the `idle_return` byte; v7 appended the `map_clock` + `map_scale_bar` bytes; v8 appended the
/// `bike_profile_idx` byte (routing-v2 N5, #538); v9 appended the `waypoint_mode` byte (epic #523);
/// v10 appended the `language` byte (epic #602); v11 appended the 24-byte `saved_sensors` block
/// (BLE-sensors SE7, #714) — 3 slots × 8 B (`present · addr_kind · addr[6]`).
pub const VERSION: u8 = 11;

/// Fixed encoded length: the [`PAYLOAD_LEN`] CRC-covered bytes + a 2-byte CRC, **rounded up to the
/// device RRAM's 16-byte write line** (the firmware store writes whole 128-bit lines) — so a codec
/// bump never needs the device store re-padded, the RRAM store reads a known span, and the file
/// store needs no length framing. Bytes past the CRC are unused zero padding.
pub const ENCODED_LEN: usize = (PAYLOAD_LEN + 2).div_ceil(16) * 16;

/// Payload size before the trailing CRC. The CRC follows immediately at this offset.
const PAYLOAD_LEN: usize = SENSORS_OFF + SENSOR_SLOTS * SAVED_SENSOR_LEN;
/// Byte offset of the field selection (right after the 14-byte head).
const STAT_FIELDS_OFF: usize = 14;
/// Byte offset of `stat_cycle_s` (right after the field selection).
const STAT_CYCLE_OFF: usize = STAT_FIELDS_OFF + 1 + MAX_STAT_FIELDS;
/// Byte offset of the device name (right after `stat_cycle_s`).
const NAME_OFF: usize = STAT_CYCLE_OFF + 2;
/// Byte offset of the `ble_enabled` flag (the v4 tail, right after the device name).
const BLE_OFF: usize = NAME_OFF + 1 + DEVICE_NAME_MAX;
/// Byte offset of the `climb_mode` byte (the v5 tail, right after `ble_enabled`).
const CLIMB_OFF: usize = BLE_OFF + 1;
/// Byte offset of the `idle_return` byte (the v6 tail, right after `climb_mode`).
const IDLE_OFF: usize = CLIMB_OFF + 1;
/// Byte offset of the `map_clock` flag (the v7 tail, right after `idle_return`).
const MAP_CLOCK_OFF: usize = IDLE_OFF + 1;
/// Byte offset of the `map_scale_bar` flag (the v7 tail, right after `map_clock`).
const SCALE_BAR_OFF: usize = MAP_CLOCK_OFF + 1;
/// Byte offset of the `bike_profile_idx` byte (the v8 tail, right after `map_scale_bar`).
const PROFILE_OFF: usize = SCALE_BAR_OFF + 1;
/// Byte offset of the `waypoint_mode` byte (the v9 tail, right after `bike_profile_idx`).
const WAYPOINT_OFF: usize = PROFILE_OFF + 1;
/// Byte offset of the `language` byte (the v10 tail, right after `waypoint_mode`).
const LANGUAGE_OFF: usize = WAYPOINT_OFF + 1;
/// Byte offset of the `saved_sensors` block (the v11 tail, right after `language`).
const SENSORS_OFF: usize = LANGUAGE_OFF + 1;
/// Bytes per saved-sensor slot: `present(1) · addr_kind(1) · addr[6]`.
const SAVED_SENSOR_LEN: usize = 8;

/// CRC-16/CCITT-FALSE (poly `0x1021`, init `0xFFFF`) over `data` — small, table-free, and
/// plenty to reject a blank/half-written blob. Guards the codec on both stores.
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// Pack [`Settings`] into its fixed [`ENCODED_LEN`]-byte blob: a version byte, the little-endian
/// fields, then a trailing CRC. The inverse of [`decode`]; shared verbatim by the sim file store
/// and the device RRAM store so one round-trip test covers both.
pub fn encode(s: &Settings) -> [u8; ENCODED_LEN] {
    let mut b = [0u8; ENCODED_LEN];
    b[0] = VERSION;
    b[1] = s.units as u8;
    b[2] = s.gps_time as u8;
    b[3..5].copy_from_slice(&s.clock.year.to_le_bytes());
    b[5] = s.clock.month;
    b[6] = s.clock.day;
    b[7] = s.clock.hour;
    b[8] = s.clock.minute;
    b[9..11].copy_from_slice(&s.utc_offset_min.to_le_bytes());
    b[11..13].copy_from_slice(&s.fix_interval_s.to_le_bytes());
    b[13] = s.power_saver as u8;
    // v2 tail: the field selection (length + fixed-width discriminants) then the cycle period.
    let (len, ids) = s.stat_fields.encode();
    b[STAT_FIELDS_OFF] = len;
    b[STAT_FIELDS_OFF + 1..STAT_FIELDS_OFF + 1 + MAX_STAT_FIELDS].copy_from_slice(&ids);
    b[STAT_CYCLE_OFF..STAT_CYCLE_OFF + 2].copy_from_slice(&s.stat_cycle_s.to_le_bytes());
    // v3 tail: the device name (length + the fixed zero-padded field).
    let name = s.device_name.as_str().as_bytes();
    b[NAME_OFF] = name.len() as u8;
    b[NAME_OFF + 1..NAME_OFF + 1 + name.len()].copy_from_slice(name);
    // v4 tail: the Bluetooth radio switch.
    b[BLE_OFF] = s.ble_enabled as u8;
    // v5 tail: the Climb-screen mode.
    b[CLIMB_OFF] = s.climb_mode as u8;
    // v6 tail: the idle-return timeout.
    b[IDLE_OFF] = s.idle_return as u8;
    // v7 tail: the two Map-chrome overlay toggles.
    b[MAP_CLOCK_OFF] = s.map_clock as u8;
    b[SCALE_BAR_OFF] = s.map_scale_bar as u8;
    // v8 tail: the selected routing-profile index (§8.6; resolved against the loaded map).
    b[PROFILE_OFF] = s.bike_profile_idx;
    // v9 tail: the Map waypoint-chip mode.
    b[WAYPOINT_OFF] = s.waypoint_mode as u8;
    // v10 tail: the UI language.
    b[LANGUAGE_OFF] = s.language as u8;
    // v11 tail: the saved-sensor slots — 3 × `present · addr_kind · addr[6]` (BLE-sensors SE7).
    for (q, slot) in s.saved_sensors.iter().enumerate() {
        let off = SENSORS_OFF + q * SAVED_SENSOR_LEN;
        b[off] = slot.present as u8;
        b[off + 1] = slot.addr_kind;
        b[off + 2..off + 2 + 6].copy_from_slice(&slot.addr);
    }
    let crc = crc16(&b[0..PAYLOAD_LEN]);
    b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
    b
}

/// Decode a blob written by [`encode`], or `None` if it is too short, the wrong version, or
/// fails the CRC — i.e. anything but a clean read of *this* format. The decoded value is
/// range-sanitised, so a `Some` is always a usable [`Settings`].
pub fn decode(bytes: &[u8]) -> Option<Settings> {
    if bytes.len() < ENCODED_LEN {
        return None;
    }
    let b = &bytes[..ENCODED_LEN];
    if b[0] != VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[PAYLOAD_LEN], b[PAYLOAD_LEN + 1]]);
    if crc != crc16(&b[0..PAYLOAD_LEN]) {
        return None;
    }
    let mut s = Settings {
        units: if b[1] == Units::Imperial as u8 { Units::Imperial } else { Units::Metric },
        gps_time: b[2] != 0,
        clock: DateTime { year: u16::from_le_bytes([b[3], b[4]]), month: b[5], day: b[6], hour: b[7], minute: b[8] },
        utc_offset_min: i16::from_le_bytes([b[9], b[10]]),
        fix_interval_s: u16::from_le_bytes([b[11], b[12]]),
        power_saver: b[13] != 0,
        stat_fields: StatFieldList::decode(
            b[STAT_FIELDS_OFF],
            &b[STAT_FIELDS_OFF + 1..STAT_FIELDS_OFF + 1 + MAX_STAT_FIELDS],
        ),
        stat_cycle_s: u16::from_le_bytes([b[STAT_CYCLE_OFF], b[STAT_CYCLE_OFF + 1]]),
        // A stored length past the cap (corrupt-but-CRC-valid input) sanitises to the factory
        // name, exactly like invalid UTF-8 inside `from_bytes` — never a garbage prefix.
        device_name: match b[NAME_OFF] as usize {
            n if n <= DEVICE_NAME_MAX => DeviceName::from_bytes(&b[NAME_OFF + 1..NAME_OFF + 1 + n]),
            _ => DeviceName::EMPTY,
        },
        ble_enabled: b[BLE_OFF] != 0,
        // An unknown climb-mode byte (an older/newer writer, a bit-flip the CRC missed) sanitises
        // to the default, exactly like the other out-of-range fields.
        climb_mode: ClimbMode::from_byte(b[CLIMB_OFF]),
        // Same clamp for the idle-return byte: an out-of-range value sanitises to the default.
        idle_return: IdleReturn::from_byte(b[IDLE_OFF]),
        // The v7 Map-chrome toggles: any non-zero byte is "on" (like the other bool fields).
        map_clock: b[MAP_CLOCK_OFF] != 0,
        map_scale_bar: b[SCALE_BAR_OFF] != 0,
        // The v8 routing-profile index: stored verbatim, **not** range-clamped here — an index past
        // the loaded map's profile count is resolved to profile 0 at plan time (N3) and shown as
        // profile 0's name in the UI (see the field doc), so a stale index is never a decode failure.
        bike_profile_idx: b[PROFILE_OFF],
        // The v9 waypoint-chip mode: an unknown byte sanitises to the default (Approach), like the
        // other enum codec fields.
        waypoint_mode: WaypointMode::from_byte(b[WAYPOINT_OFF]),
        // The v10 UI language: an unknown byte sanitises to the default (English), like the other
        // enum codec fields.
        language: Language::from_byte(b[LANGUAGE_OFF]),
        // The v11 saved-sensor slots (BLE-sensors SE7): 3 × `present · addr_kind · addr[6]`. A stored
        // `addr_kind` past `1` (corrupt-but-CRC-valid) reads as random (`!= 0`), the board's own
        // interpretation, so a bit-flip never mis-picks the address kind.
        saved_sensors: decode_saved_sensors(b),
    };
    s.sanitize();
    Some(s)
}

/// Decode the v11 saved-sensor block: 3 slots × `present(1) · addr_kind(1) · addr[6]`. An absent slot
/// (`present == 0`) reads as [`SavedSensor::EMPTY`] regardless of the stored address; a present slot
/// keeps its address and normalises `addr_kind` to `0`/`1` (`!= 0` = random), matching how the board
/// maps it to `AddrKind`.
fn decode_saved_sensors(b: &[u8]) -> [SavedSensor; SENSOR_SLOTS] {
    let mut slots = [SavedSensor::EMPTY; SENSOR_SLOTS];
    for (q, slot) in slots.iter_mut().enumerate() {
        let off = SENSORS_OFF + q * SAVED_SENSOR_LEN;
        if b[off] != 0 {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(&b[off + 2..off + 2 + 6]);
            *slot = SavedSensor::saved((b[off + 1] != 0) as u8, addr);
        }
    }
    slots
}

// ==================== durable object-id high-water marks (#450) ====================
//
// The device names stored objects by durable `u16` ids (`RT{id}.OBR` routes, `RD{id}.ORD` rides);
// the phone persists those ids (`deviceObjectID`, ride synced/tombstone sets), so an id must
// **never be reused** — even after the file it named is deleted and a reboot re-scans the card.
// `scan-max + 1` alone re-issues a deleted id; these high-water marks are the durable floor:
// one CRC-checked 16-byte RRAM line holding the next fresh id per namespace, bumped on every
// assignment. Allocation = `max(scan_max + 1, stored_next)`.
//
// The codec lives here — beside the settings blob codec, the established precedent — because the
// board crate is target-only: encode/decode/torn-line semantics must be host-testable.

/// The id high-water line's fixed length: one RRAM write line (16 bytes), like the bond and
/// boot-counter lines. Layout: `magic(4) · version(1) · pad(1) · next_route_id u16 LE ·
/// next_ride_id u16 LE · pad(2) · crc16 LE · pad(2)` — CRC-16 over bytes `[0..12]`.
pub const ID_MARKS_LEN: usize = 16;
/// The id-marks line's tag; anything else there (blank page, torn write, older layout) decodes to
/// "no floor" and allocation falls back to scan-max + 1 (exactly today's behaviour).
const ID_MARKS_MAGIC: [u8; 4] = *b"OBCI";
/// Id-marks layout version — bump on any field change (an old version reads as no floor).
const ID_MARKS_VERSION: u8 = 1;
/// CRC-covered prefix of the id-marks line.
const ID_MARKS_PAYLOAD: usize = 12;

/// The durable id floors: the next fresh **route** and **ride** object id the store may hand out.
/// `Default` (both 0) is "no floor" — a fresh device / reflash allocates from the scan alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdMarks {
    /// One past the highest route object id ever assigned (`RT{id}.OBR` uploads).
    pub next_route_id: u16,
    /// One past the highest ride object id ever assigned (`RD{id}.ORD` saves).
    pub next_ride_id: u16,
}

impl IdMarks {
    /// Allocate the next fresh **route** id: `max(scan_next, stored floor)`, bumping the floor past
    /// it — call with `scan_next` = one past the highest id the card scan saw, then persist `self`.
    pub fn alloc_route(&mut self, scan_next: u16) -> u16 {
        let id = self.next_route_id.max(scan_next);
        self.next_route_id = id.saturating_add(1);
        id
    }

    /// Allocate the next fresh **ride** id — the ride-namespace twin of
    /// [`alloc_route`](IdMarks::alloc_route).
    pub fn alloc_ride(&mut self, scan_next: u16) -> u16 {
        let id = self.next_ride_id.max(scan_next);
        self.next_ride_id = id.saturating_add(1);
        id
    }
}

/// Pack the id high-water marks into their fixed 16-byte RRAM line. Inverse of
/// [`decode_id_marks`].
pub fn encode_id_marks(m: &IdMarks) -> [u8; ID_MARKS_LEN] {
    let mut b = [0u8; ID_MARKS_LEN];
    b[0..4].copy_from_slice(&ID_MARKS_MAGIC);
    b[4] = ID_MARKS_VERSION;
    b[6..8].copy_from_slice(&m.next_route_id.to_le_bytes());
    b[8..10].copy_from_slice(&m.next_ride_id.to_le_bytes());
    let crc = crc16(&b[0..ID_MARKS_PAYLOAD]);
    b[ID_MARKS_PAYLOAD..ID_MARKS_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
    b
}

/// Decode an id high-water line, or `None` for anything but a clean read of this format — a blank
/// page, a torn write, a short slice, or an older layout. `None` means **no floor**: the caller
/// falls back to scan-max + 1, so a fresh device behaves exactly as before the marks existed.
pub fn decode_id_marks(bytes: &[u8]) -> Option<IdMarks> {
    if bytes.len() < ID_MARKS_LEN {
        return None;
    }
    let b = &bytes[..ID_MARKS_LEN];
    if b[0..4] != ID_MARKS_MAGIC || b[4] != ID_MARKS_VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[ID_MARKS_PAYLOAD], b[ID_MARKS_PAYLOAD + 1]]);
    if crc != crc16(&b[0..ID_MARKS_PAYLOAD]) {
        return None;
    }
    Some(IdMarks { next_route_id: u16::from_le_bytes([b[6], b[7]]), next_ride_id: u16::from_le_bytes([b[8], b[9]]) })
}

// ==================== store-epoch nonce (protocol v2, #632/#767; card-resident #776) ====================
//
// A per-id-era `u32` nonce that lets the phone detect an **id-era reset**: any event that loses the
// durable id floor (the id-marks line above) while the app keeps its library — a full-chip reflash,
// a factory reset / RMA / recovery, or a torn id-marks write — reopens already-issued object ids, so
// freshly-minted ids silently *alias* months-old phone-side state (the 2026-07-12 ride-sync
// incident). The nonce is minted from the TRNG and persisted in a small **card-resident file**
// (`EPOCH.OBE` in the card root), so the SD card is the sole home of the id-era name (#776): a card
// swap **transplants** the store's identity (swap back restores the old era, a card upgrade-by-copy
// carries the era along), and a card written by a *different* device presents *its* epoch — its own
// scope — closing the residual foreign-card hole the RRAM-line design left open. It is served over
// the pre-pairing `protocolVersion` read (V2, #766); the app scopes all id-keyed state by
// (device serial, store epoch), so an era change makes the old era's keys stop matching by
// construction — no migration code.
//
// The mint decision ([`store_epoch_mint`]) is a pure function so the subtle rule is host-tested
// without the board crate; the board glue reads the card epoch file + the RRAM id-marks line, draws
// one TRNG word, and writes back (epoch → card, id-marks → RRAM). Torn/absent/foreign file → `None`,
// exactly the id-marks (and other sidecar) conventions. The file carries no RRAM line-size padding —
// it is idiomatic with the `SYNCED.SET` / `ROUTES.CRC` card sidecars, not the retired RRAM line.

/// The store-epoch file's fixed length: 12 bytes, `magic(4) · version(1) · pad(1) · epoch u32 LE ·
/// crc16 LE` — CRC-16 over bytes `[0..10]`. A card sidecar, not an RRAM line, so no 16-byte write-line
/// padding (unlike the retired id-era RRAM line this replaced).
pub const STORE_EPOCH_LEN: usize = 12;
/// The store-epoch file's tag; anything else there (absent, torn write, older layout) decodes
/// to `None` — "no epoch", which the mint rule treats as clause 1 (mint a fresh nonce).
const STORE_EPOCH_MAGIC: [u8; 4] = *b"OBCE";
/// Store-epoch layout version — bump on any field change (an old version reads as no epoch).
const STORE_EPOCH_VERSION: u8 = 1;
/// CRC-covered prefix of the store-epoch file: `magic(4) · version(1) · pad(1) · epoch u32 LE`.
const STORE_EPOCH_PAYLOAD: usize = 10;

/// Pack the store-epoch nonce into its fixed 12-byte card file. Inverse of [`decode_store_epoch`].
pub fn encode_store_epoch(epoch: u32) -> [u8; STORE_EPOCH_LEN] {
    let mut b = [0u8; STORE_EPOCH_LEN];
    b[0..4].copy_from_slice(&STORE_EPOCH_MAGIC);
    b[4] = STORE_EPOCH_VERSION;
    b[6..10].copy_from_slice(&epoch.to_le_bytes());
    let crc = crc16(&b[0..STORE_EPOCH_PAYLOAD]);
    b[STORE_EPOCH_PAYLOAD..STORE_EPOCH_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
    b
}

/// Decode a store-epoch file, or `None` for anything but a clean read of this format — an absent
/// file (the board returns `None` before calling this), a torn write, a short slice, or an older
/// layout. `None` means **no epoch**: the mint rule draws a fresh one (clause 1).
pub fn decode_store_epoch(bytes: &[u8]) -> Option<u32> {
    if bytes.len() < STORE_EPOCH_LEN {
        return None;
    }
    let b = &bytes[..STORE_EPOCH_LEN];
    if b[0..4] != STORE_EPOCH_MAGIC || b[4] != STORE_EPOCH_VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[STORE_EPOCH_PAYLOAD], b[STORE_EPOCH_PAYLOAD + 1]]);
    if crc != crc16(&b[0..STORE_EPOCH_PAYLOAD]) {
        return None;
    }
    Some(u32::from_le_bytes([b[6], b[7], b[8], b[9]]))
}

/// The boot-time store-epoch mint decision (protocol v2, #632 item 5; card-resident #776) — a pure
/// function so the subtle rule is host-testable. Given the decoded **card epoch** (from `EPOCH.OBE`)
/// and the RRAM id-marks line (each `None` when absent/torn/foreign) plus one freshly-drawn TRNG word
/// `fresh`, returns:
///
/// - `None` — **keep** the card's epoch: this boot writes nothing (the common steady-state path,
///   including a *card swap* to another store with a valid epoch — that epoch is adopted verbatim,
///   the transplant semantics #776 exists for).
/// - `Some((new_epoch, marks))` — **mint**: persist `new_epoch` to the **card** epoch file **and**
///   (re)write the RRAM id-marks line to `marks` in the same boot pass.
///
/// Mint fires when the card epoch is absent (**clause 1**: absent/torn/foreign file) **or** the
/// id-marks line decodes to "no floor" (**clause 2**: a torn id-marks write — floors lost under an
/// intact card epoch would be *undetectable* aliasing, so a lost floor **is** a new era, and it
/// reopens the deleted-id band on the very card whose epoch was intact).
///
/// The marks (re)write is what makes clause 2 unambiguous: an already-valid id-marks line is kept
/// verbatim (its durable floors survive a clause-1-only mint), while an absent one is (re)seeded to
/// [`IdMarks::default`] — "no floor", which the store's `max(scan_max + 1, floor)` allocation
/// re-derives from the card scan at the first allocation (today's fallback; the board mints before
/// the scan runs). This establishes the invariant *a valid epoch implies a valid id-marks line at
/// mint*: without it a fresh device (no ride/upload → no id-marks line **by design**) would re-mint
/// on every boot via clause 2; with it, "valid epoch + no floor" is unambiguous torn-line evidence
/// — exactly what clause 2 exists to catch.
///
/// Note the function is agnostic to *where* the epoch is stored — the #776 move is entirely in the
/// board glue (it now reads/writes the card file, not an RRAM line); the decision logic is
/// unchanged, which is why the mint matrix + stability tests carry straight over.
pub fn store_epoch_mint(epoch: Option<u32>, marks: Option<IdMarks>, fresh: u32) -> Option<(u32, IdMarks)> {
    if epoch.is_some() && marks.is_some() {
        return None; // steady state: valid card epoch + valid floors → nothing to write this boot
    }
    Some((fresh, marks.unwrap_or_default()))
}

// ==================== DFU arm marker (boot-outcome popup) ====================
//
// The armer's breadcrumb: written to its settings-page line right after the `Armed` boot-state
// write, just before the reboot into the bootloader. At the next boot the board's
// `dfu::reconcile_boot_outcome` reads it back and — together with the boot-state page — derives
// the one-time verdict card: `Trial` = the confirm path owns it, `Armed` = the bootloader never
// ran the install, `Idle` + this marker = the staged version either accepted (it IS the installed
// header, first-install case) or failed (rejected / rolled back). Cleared wherever a verdict is
// delivered. Torn/blank/foreign decodes to `None` — "no arm happened", a plain boot.

/// The arm marker's fixed slot length: 3 whole 16-byte RRAM lines.
pub const ARM_MARKER_LEN: usize = 48;
/// The arm-marker tag; anything else there decodes to "no arm happened".
const ARM_MARKER_MAGIC: [u8; 4] = *b"OBCA";
/// Arm-marker layout version — bump on any field change (an old version reads as no marker).
const ARM_MARKER_VERSION: u8 = 1;
/// CRC-covered prefix: `magic(4) · version(1) · vlen(1) · pad(2) · generation u32 LE · version
/// string bytes(32)`.
const ARM_MARKER_PAYLOAD: usize = 44;

/// What the armer records before rebooting into the bootloader: the arm's generation and the
/// staged image's OBCU version string (the popup's "which update" fact).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmMarker {
    /// The `Armed` record's generation (the ticket the armer reported).
    pub generation: u32,
    /// The staged image's version string, verbatim from its OBCU header (≤ 32 bytes).
    pub staged: heapless::String<32>,
}

/// Pack an arm marker into its fixed [`ARM_MARKER_LEN`]-byte slot. Inverse of
/// [`decode_arm_marker`].
pub fn encode_arm_marker(m: &ArmMarker) -> [u8; ARM_MARKER_LEN] {
    let mut b = [0u8; ARM_MARKER_LEN];
    b[0..4].copy_from_slice(&ARM_MARKER_MAGIC);
    b[4] = ARM_MARKER_VERSION;
    let v = m.staged.as_bytes();
    let vlen = v.len().min(32);
    b[5] = vlen as u8;
    b[8..12].copy_from_slice(&m.generation.to_le_bytes());
    b[12..12 + vlen].copy_from_slice(&v[..vlen]);
    let crc = crc16(&b[0..ARM_MARKER_PAYLOAD]);
    b[ARM_MARKER_PAYLOAD..ARM_MARKER_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
    b
}

/// Decode an arm-marker slot, or `None` for anything but a clean read of this format — a blank
/// slot, a torn write, a short slice, an older layout, or a version string that isn't UTF-8.
/// `None` means **no arm happened**: the boot-outcome reconcile treats the boot as plain.
pub fn decode_arm_marker(bytes: &[u8]) -> Option<ArmMarker> {
    if bytes.len() < ARM_MARKER_LEN {
        return None;
    }
    let b = &bytes[..ARM_MARKER_LEN];
    if b[0..4] != ARM_MARKER_MAGIC || b[4] != ARM_MARKER_VERSION {
        return None;
    }
    let crc = u16::from_le_bytes([b[ARM_MARKER_PAYLOAD], b[ARM_MARKER_PAYLOAD + 1]]);
    if crc != crc16(&b[0..ARM_MARKER_PAYLOAD]) {
        return None;
    }
    let vlen = b[5] as usize;
    if vlen > 32 {
        return None;
    }
    let mut staged: heapless::String<32> = heapless::String::new();
    staged.push_str(core::str::from_utf8(&b[12..12 + vlen]).ok()?).ok()?;
    Some(ArmMarker { generation: u32::from_le_bytes([b[8], b[9], b[10], b[11]]), staged })
}

// ==================== synced-ride sidecar (#454) ====================
//
// The unsynced-ride delete guard (epic #447 P7, locked option b): the device records "the phone
// has downloaded this ride at least once" per ride, set when a ride-object download **completes**.
// It's persisted in a small SD **sidecar file in /tracks** (`SYNCED.SET`) so it survives a reflash
// and travels with the card/rides — deliberately *not* the RRAM settings carve (which a reflash may
// wipe and which doesn't move with the card). The Rides screen renders an unsynced ride's delete
// footer warning-red with a "not synced" cue; a synced ride gets the standard footer.
//
// The codec lives here — beside the id-marks + settings codecs, the established host-testable
// precedent — so the "torn/missing sidecar = nothing synced, never a crash" contract is unit-tested
// without the board crate. The format is intentionally simple: a magic + version + a `u16` count +
// that many little-endian `u16` ride ids + a trailing CRC-16 over everything before it. A blank
// page, a short slice, a torn write, or an unknown version all decode to the **empty** set — which
// reads as "nothing synced", the safe default (every ride shows the warning footer, all deletable).

/// The sidecar magic tag; anything else there decodes to the empty synced set.
const SYNCED_MAGIC: [u8; 4] = *b"OBCS";
/// Sidecar layout version — bump on any format change (an old version reads as empty).
const SYNCED_VERSION: u8 = 1;
/// Fixed header bytes before the id list: `magic(4) · version(1) · pad(1) · count u16 LE`.
const SYNCED_HEADER_LEN: usize = 8;

/// The persisted set of ride ids the phone has downloaded at least once. Bounded by
/// [`MAX_RIDES`](crate::ride::MAX_RIDES) (a ride can only be synced if it's stored). `Default` is the
/// empty set — "nothing synced".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncedRides {
    ids: heapless::Vec<u16, { crate::ride::MAX_RIDES }>,
}

impl SyncedRides {
    /// An empty synced set.
    pub fn new() -> Self {
        SyncedRides::default()
    }

    /// Whether ride `id` has been downloaded at least once.
    pub fn contains(&self, id: u16) -> bool {
        self.ids.contains(&id)
    }

    /// Record ride `id` as synced. Returns `true` if it was newly added (so the caller only rewrites
    /// the sidecar on an actual change). Idempotent; a full set silently ignores a new id.
    pub fn insert(&mut self, id: u16) -> bool {
        if self.ids.contains(&id) {
            return false;
        }
        self.ids.push(id).is_ok()
    }

    /// Drop ride `id` from the synced set (a deleted ride's id is retired so a later scan doesn't
    /// carry a stale flag — though ids never reuse, so this is belt-and-braces). Returns `true` if it
    /// was present.
    pub fn remove(&mut self, id: u16) -> bool {
        if let Some(pos) = self.ids.iter().position(|&x| x == id) {
            self.ids.swap_remove(pos);
            true
        } else {
            false
        }
    }

    /// The synced ids, for the codec / tests.
    pub fn ids(&self) -> &[u16] {
        &self.ids
    }
}

/// The encoded sidecar's byte length for `count` synced ids: the fixed header, the `u16` id list,
/// then the trailing CRC-16.
pub const fn synced_rides_len(count: usize) -> usize {
    SYNCED_HEADER_LEN + count * 2 + 2
}

/// The largest an encoded sidecar can be (a full synced set) — the buffer a host reserves to write it.
pub const SYNCED_RIDES_MAX_LEN: usize = synced_rides_len(crate::ride::MAX_RIDES);

/// Pack the synced-ride set into `out`, returning the encoded byte length. `out` must be at least
/// [`synced_rides_len`]`(set.ids().len())` (use a [`SYNCED_RIDES_MAX_LEN`] buffer). Inverse of
/// [`decode_synced_rides`].
pub fn encode_synced_rides(set: &SyncedRides, out: &mut [u8]) -> usize {
    let ids = set.ids();
    let len = synced_rides_len(ids.len());
    out[0..4].copy_from_slice(&SYNCED_MAGIC);
    out[4] = SYNCED_VERSION;
    out[5] = 0;
    out[6..8].copy_from_slice(&(ids.len() as u16).to_le_bytes());
    for (i, &id) in ids.iter().enumerate() {
        let o = SYNCED_HEADER_LEN + i * 2;
        out[o..o + 2].copy_from_slice(&id.to_le_bytes());
    }
    let crc = crc16(&out[..len - 2]);
    out[len - 2..len].copy_from_slice(&crc.to_le_bytes());
    len
}

/// Decode a synced-ride sidecar, always returning a set — a blank page, a short slice, a torn write,
/// an unknown version, a count that overruns the slice, or a CRC mismatch all yield the **empty**
/// set ("nothing synced", the safe default). Never panics on malformed input.
pub fn decode_synced_rides(bytes: &[u8]) -> SyncedRides {
    let empty = SyncedRides::new();
    if bytes.len() < SYNCED_HEADER_LEN + 2 {
        return empty; // shorter than an empty-set sidecar → treat as absent
    }
    if bytes[0..4] != SYNCED_MAGIC || bytes[4] != SYNCED_VERSION {
        return empty;
    }
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let len = synced_rides_len(count);
    if count > crate::ride::MAX_RIDES || bytes.len() < len {
        return empty; // a count that claims more ids than the slice (or the cap) holds is corrupt
    }
    let crc = u16::from_le_bytes([bytes[len - 2], bytes[len - 1]]);
    if crc != crc16(&bytes[..len - 2]) {
        return empty;
    }
    let mut set = SyncedRides::new();
    for i in 0..count {
        let o = SYNCED_HEADER_LEN + i * 2;
        let _ = set.insert(u16::from_le_bytes([bytes[o], bytes[o + 1]]));
    }
    set
}

// ==================== route-CRC sidecar (#632 item 6, V2) ====================
//
// The route-identity content fingerprint (epic #632 item 6, device half): the whole-object CRC-32
// of each stored route's OBCR bytes, keyed by durable object id, so the `routeList` entry can carry
// it and the app can verify *what* a linked id points at (identity-verified badges) and adopt an
// identical unlinked copy. Persisted in a small SD **sidecar in /routes** (`ROUTES.CRC`) — the
// direct analogue of the `/tracks` `SYNCED.SET` synced-ride sidecar — so it survives a reflash and
// travels with the card/routes, and is *not* the RRAM settings carve. A BLE upload writes the entry
// at commit (the CRC is already verified there); a side-loaded / pre-v2 route with no entry is filled
// lazily at first list build (one streaming CRC pass, then persisted).
//
// The codec lives here — beside the synced-set + id-marks codecs, the host-testable precedent — so
// the "torn/missing sidecar = empty map, never a crash" contract is unit-tested without the board
// crate. Same shape as `SYNCED.SET`: a magic + version + a `u16` count + that many
// `(id u16, crc32 u32)` little-endian pairs + a trailing CRC-16 over everything before it. A blank
// page, a short slice, a torn write, an unknown version, an overrunning count, or a CRC mismatch all
// decode to the **empty** map — which serves `0 = unknown` for every route (the safe default; the
// device then re-fills lazily).

/// The sidecar magic tag; anything else there decodes to the empty CRC map.
const ROUTE_CRCS_MAGIC: [u8; 4] = *b"ORCS";
/// Sidecar layout version — bump on any format change (an old version reads as empty).
const ROUTE_CRCS_VERSION: u8 = 1;
/// Fixed header bytes before the entry list: `magic(4) · version(1) · pad(1) · count u16 LE`.
const ROUTE_CRCS_HEADER_LEN: usize = 8;
/// One `(id u16 LE, crc32 u32 LE)` entry.
const ROUTE_CRCS_ENTRY_LEN: usize = 6;

/// The persisted map of route object id → whole-object CRC-32. Bounded by
/// [`MAX_ROUTES`](crate::route::MAX_ROUTES) (a CRC can only exist for a cataloged route). `Default`
/// is the empty map — "no CRC known for any route".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteCrcs {
    entries: heapless::Vec<(u16, u32), { crate::route::MAX_ROUTES }>,
}

impl RouteCrcs {
    /// An empty CRC map.
    pub fn new() -> Self {
        RouteCrcs::default()
    }

    /// The stored whole-object CRC-32 for route `id`, or `None` when the map has no entry for it
    /// (the caller then lazily fills it). Note a genuine CRC of `0` is a legal value stored and
    /// returned as `Some(0)` — it is only ever *served* on the wire as `0 = unknown`, never
    /// special-cased here.
    pub fn get(&self, id: u16) -> Option<u32> {
        self.entries.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
    }

    /// Upsert the CRC for route `id`. Returns `true` when the map changed (a new entry, or an
    /// existing entry whose CRC differs) so the caller only rewrites the sidecar on an actual
    /// change. A full map silently ignores a brand-new id.
    pub fn insert(&mut self, id: u16, crc: u32) -> bool {
        if let Some(slot) = self.entries.iter_mut().find(|(i, _)| *i == id) {
            if slot.1 == crc {
                return false;
            }
            slot.1 = crc;
            return true;
        }
        self.entries.push((id, crc)).is_ok()
    }

    /// Retire route `id`'s CRC entry (a deleted route — ids never reuse, so this is belt-and-braces
    /// tidiness). Returns `true` if it was present.
    pub fn remove(&mut self, id: u16) -> bool {
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == id) {
            self.entries.swap_remove(pos);
            true
        } else {
            false
        }
    }

    /// The `(id, crc)` entries, for the codec / tests.
    pub fn entries(&self) -> &[(u16, u32)] {
        &self.entries
    }
}

/// The encoded sidecar's byte length for `count` entries: the fixed header, the entry list, then the
/// trailing CRC-16.
pub const fn route_crcs_len(count: usize) -> usize {
    ROUTE_CRCS_HEADER_LEN + count * ROUTE_CRCS_ENTRY_LEN + 2
}

/// The largest an encoded sidecar can be (a full map) — the buffer a host reserves to write it.
pub const ROUTE_CRCS_MAX_LEN: usize = route_crcs_len(crate::route::MAX_ROUTES);

/// Pack the route-CRC map into `out`, returning the encoded byte length. `out` must be at least
/// [`route_crcs_len`]`(map.entries().len())` (use a [`ROUTE_CRCS_MAX_LEN`] buffer). Inverse of
/// [`decode_route_crcs`].
pub fn encode_route_crcs(map: &RouteCrcs, out: &mut [u8]) -> usize {
    let entries = map.entries();
    let len = route_crcs_len(entries.len());
    out[0..4].copy_from_slice(&ROUTE_CRCS_MAGIC);
    out[4] = ROUTE_CRCS_VERSION;
    out[5] = 0;
    out[6..8].copy_from_slice(&(entries.len() as u16).to_le_bytes());
    for (i, (id, crc)) in entries.iter().enumerate() {
        let o = ROUTE_CRCS_HEADER_LEN + i * ROUTE_CRCS_ENTRY_LEN;
        out[o..o + 2].copy_from_slice(&id.to_le_bytes());
        out[o + 2..o + 6].copy_from_slice(&crc.to_le_bytes());
    }
    let crc = crc16(&out[..len - 2]);
    out[len - 2..len].copy_from_slice(&crc.to_le_bytes());
    len
}

/// Decode a route-CRC sidecar, always returning a map — a blank page, a short slice, a torn write,
/// an unknown version, a count that overruns the slice (or the cap), or a CRC mismatch all yield the
/// **empty** map ("no CRC known", the safe default). Never panics on malformed input.
pub fn decode_route_crcs(bytes: &[u8]) -> RouteCrcs {
    let empty = RouteCrcs::new();
    if bytes.len() < ROUTE_CRCS_HEADER_LEN + 2 {
        return empty; // shorter than an empty-map sidecar → treat as absent
    }
    if bytes[0..4] != ROUTE_CRCS_MAGIC || bytes[4] != ROUTE_CRCS_VERSION {
        return empty;
    }
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    let len = route_crcs_len(count);
    if count > crate::route::MAX_ROUTES || bytes.len() < len {
        return empty; // a count claiming more entries than the slice (or the cap) holds is corrupt
    }
    let crc = u16::from_le_bytes([bytes[len - 2], bytes[len - 1]]);
    if crc != crc16(&bytes[..len - 2]) {
        return empty;
    }
    let mut map = RouteCrcs::new();
    for i in 0..count {
        let o = ROUTE_CRCS_HEADER_LEN + i * ROUTE_CRCS_ENTRY_LEN;
        let id = u16::from_le_bytes([bytes[o], bytes[o + 1]]);
        let c = u32::from_le_bytes([bytes[o + 2], bytes[o + 3], bytes[o + 4], bytes[o + 5]]);
        let _ = map.insert(id, c);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-default settings value — including a customised, reordered field selection with a
    /// two-span tile — round-trips through the codec byte-for-byte.
    #[test]
    fn codec_round_trips() {
        let mut stat_fields = StatFieldList::default();
        stat_fields.remove(0); // drop a default tile…
        assert!(stat_fields.push(crate::stat_fields::StatField::Clock)); // …and pin the wide clock
        let s = Settings {
            units: Units::Imperial,
            gps_time: true,
            clock: DateTime { year: 2026, month: 6, day: 29, hour: 14, minute: 40 },
            utc_offset_min: 120,
            fix_interval_s: 5,
            power_saver: true,
            stat_fields,
            stat_cycle_s: 8,
            device_name: DeviceName::from_str_lossy("Timo's OBC"),
            ble_enabled: false,
            climb_mode: ClimbMode::Manual,
            idle_return: IdleReturn::M5,
            map_clock: false,
            map_scale_bar: false,
            bike_profile_idx: 3,
            waypoint_mode: WaypointMode::Always,
            language: Language::De,
            saved_sensors: [
                SavedSensor::saved(1, [1, 2, 3, 4, 5, 6]),
                SavedSensor::EMPTY,
                SavedSensor::saved(0, [6, 5, 4, 3, 2, 1]),
            ],
        };
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    /// The v4 tail: the Bluetooth switch round-trips, defaults **on**, and is device-only —
    /// [`adopt_ble_fields`] must never pull it across, so a BLE Config write can't switch the
    /// radio out from under the rider (or strand it off with the link already gone).
    #[test]
    fn ble_enabled_round_trips_and_is_device_only() {
        assert!(Settings::default().ble_enabled, "the radio defaults on");
        let s = Settings { ble_enabled: false, ..Settings::default() };
        assert_eq!(decode(&encode(&s)), Some(s), "the off state round-trips");

        // Device-only across the #456 coherence paths: the phone's blob says on, ours stays off.
        let mut app = Settings { ble_enabled: false, ..Settings::default() };
        app.adopt_ble_fields(&Settings { units: Units::Imperial, ..Settings::default() });
        assert!(!app.ble_enabled, "adopt_ble_fields leaves the radio switch alone");
        assert_eq!(app.units, Units::Imperial, "while the BLE-owned fields still land");
    }

    /// The v5 tail: the Climb-screen mode round-trips, defaults **Auto** (the headline
    /// auto-show behavior), sanitises an out-of-range byte back to Auto, and is device-only —
    /// [`adopt_ble_fields`] must never pull it across (a phone can't reconfigure the climb UI).
    #[test]
    fn climb_mode_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().climb_mode, ClimbMode::Auto, "the climb panel defaults on (Auto)");

        // Each mode round-trips through the codec byte-for-byte.
        for mode in [ClimbMode::Off, ClimbMode::Manual, ClimbMode::Auto] {
            let s = Settings { climb_mode: mode, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{mode:?} round-trips");
        }

        // An out-of-range stored byte (a newer writer, a bit-flip the CRC missed) sanitises to the
        // default Auto, not a garbage variant — re-stamp the CRC so only the payload is "wrong".
        let mut b = encode(&Settings { climb_mode: ClimbMode::Off, ..Settings::default() });
        b[CLIMB_OFF] = 200;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert_eq!(got.climb_mode, ClimbMode::Auto, "an unknown climb-mode byte falls back to Auto");

        // Device-only: a BLE blob's climb_mode never lands via the #456 coherence merge.
        let mut app = Settings { climb_mode: ClimbMode::Off, ..Settings::default() };
        app.adopt_ble_fields(&Settings { climb_mode: ClimbMode::Auto, ..Settings::default() });
        assert_eq!(app.climb_mode, ClimbMode::Off, "adopt_ble_fields leaves the climb mode alone");
    }

    /// The v6 tail: the idle-return timeout round-trips every value, defaults **30 s**, sanitises an
    /// out-of-range byte back to the default, and is device-only — [`adopt_ble_fields`] must never
    /// pull it across (a phone can't reconfigure the idle behavior).
    #[test]
    fn idle_return_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().idle_return, IdleReturn::S30, "the idle return defaults to 30 s");

        // Each value round-trips through the codec byte-for-byte.
        for v in [IdleReturn::S15, IdleReturn::S30, IdleReturn::M1, IdleReturn::M5, IdleReturn::Never] {
            let s = Settings { idle_return: v, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{v:?} round-trips");
        }

        // An out-of-range stored byte sanitises to the default 30 s — re-stamp the CRC so only the
        // payload byte is "wrong".
        let mut b = encode(&Settings { idle_return: IdleReturn::S15, ..Settings::default() });
        b[IDLE_OFF] = 200;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert_eq!(got.idle_return, IdleReturn::S30, "an unknown idle-return byte falls back to 30 s");

        // Device-only: a BLE blob's idle_return never lands via the #456 coherence merge.
        let mut app = Settings { idle_return: IdleReturn::S15, ..Settings::default() };
        app.adopt_ble_fields(&Settings { idle_return: IdleReturn::Never, ..Settings::default() });
        assert_eq!(app.idle_return, IdleReturn::S15, "adopt_ble_fields leaves the idle return alone");
    }

    /// The picker's timeout mapping + the left/right walk order (wrapping at both ends).
    #[test]
    fn idle_return_timeout_and_stepping() {
        assert_eq!(IdleReturn::S15.timeout_ms(), Some(15_000));
        assert_eq!(IdleReturn::S30.timeout_ms(), Some(30_000));
        assert_eq!(IdleReturn::M1.timeout_ms(), Some(60_000));
        assert_eq!(IdleReturn::M5.timeout_ms(), Some(300_000));
        assert_eq!(IdleReturn::Never.timeout_ms(), None, "Never disables the mechanism");

        // Right walks toward Never, wrapping back to the shortest; left is the mirror.
        assert_eq!(IdleReturn::S15.stepped(1), IdleReturn::S30);
        assert_eq!(IdleReturn::M5.stepped(1), IdleReturn::Never);
        assert_eq!(IdleReturn::Never.stepped(1), IdleReturn::S15, "wraps past Never");
        assert_eq!(IdleReturn::S15.stepped(-1), IdleReturn::Never, "wraps past the start");
    }

    /// The v7 tail: the two Map-chrome toggles round-trip, default **on**, and are device-only —
    /// [`adopt_ble_fields`] must never pull them across (a phone can't reconfigure the map overlays).
    #[test]
    fn map_overlays_round_trip_and_are_device_only() {
        assert!(Settings::default().map_clock, "the map clock defaults on");
        assert!(Settings::default().map_scale_bar, "the scale bar defaults on");
        // The RRAM carve is unchanged — the two new bytes fit inside the same 16-byte line rounding.
        assert_eq!(ENCODED_LEN, 112, "the settings blob is 112 B / 7 RRAM lines (v11 saved_sensors tail)");

        // Every on/off combination round-trips byte-for-byte.
        for clock in [false, true] {
            for bar in [false, true] {
                let s = Settings { map_clock: clock, map_scale_bar: bar, ..Settings::default() };
                assert_eq!(decode(&encode(&s)), Some(s), "clock={clock} bar={bar} round-trips");
            }
        }

        // Device-only: a BLE blob's map toggles never land via the #456 coherence merge.
        let mut app = Settings { map_clock: false, map_scale_bar: false, ..Settings::default() };
        app.adopt_ble_fields(&Settings { map_clock: true, map_scale_bar: true, ..Settings::default() });
        assert!(!app.map_clock && !app.map_scale_bar, "adopt_ble_fields leaves the map overlays alone");
    }

    /// The v8 tail: the routing-profile index round-trips every value, defaults **0**, is stored
    /// **verbatim** (never range-clamped on decode — an out-of-range index is a live-map concern, not
    /// a codec one), and is device-only — [`adopt_ble_fields`] must never pull it across (a phone
    /// can't repick the rider's bike type).
    #[test]
    fn bike_profile_idx_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().bike_profile_idx, 0, "the profile index defaults to 0");
        // The one new byte still fits inside the same 16-byte RRAM line rounding as v7.
        assert_eq!(ENCODED_LEN, 112, "the settings blob is 112 B / 7 RRAM lines (v11 saved_sensors tail)");

        // Every index round-trips byte-for-byte — including a value past any real map's profile count,
        // which the codec stores verbatim (the router/UI own the fallback, not decode).
        for idx in [0u8, 1, 3, 7, 200] {
            let s = Settings { bike_profile_idx: idx, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "idx={idx} round-trips verbatim");
        }

        // Device-only: a BLE blob's profile index never lands via the #456 coherence merge.
        let mut app = Settings { bike_profile_idx: 2, ..Settings::default() };
        app.adopt_ble_fields(&Settings { bike_profile_idx: 5, ..Settings::default() });
        assert_eq!(app.bike_profile_idx, 2, "adopt_ble_fields leaves the bike profile alone");
    }

    /// The v9 tail: the Map waypoint-chip mode round-trips every value, defaults **Approach** (the
    /// discoverable middle ground), sanitises an out-of-range byte back to Approach, and is
    /// device-only — [`adopt_ble_fields`] must never pull it across (a BLE Config write can't flip
    /// the rider's on-glass chrome).
    #[test]
    fn waypoint_mode_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().waypoint_mode, WaypointMode::Approach, "the chip defaults to Approach");
        // The one new byte still fits inside the same 16-byte RRAM line rounding as v8.
        assert_eq!(ENCODED_LEN, 112, "the settings blob is 112 B / 7 RRAM lines (v11 saved_sensors tail)");

        // Each mode round-trips through the codec byte-for-byte.
        for mode in [WaypointMode::Off, WaypointMode::Approach, WaypointMode::Always] {
            let s = Settings { waypoint_mode: mode, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{mode:?} round-trips");
        }

        // An out-of-range stored byte (a newer writer, a bit-flip the CRC missed) sanitises to the
        // default Approach — re-stamp the CRC so only the payload byte is "wrong".
        let mut b = encode(&Settings { waypoint_mode: WaypointMode::Off, ..Settings::default() });
        b[WAYPOINT_OFF] = 200;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert_eq!(got.waypoint_mode, WaypointMode::Approach, "an unknown waypoint-mode byte falls back to Approach");

        // Device-only: a BLE blob's waypoint_mode never lands via the #456 coherence merge.
        let mut app = Settings { waypoint_mode: WaypointMode::Off, ..Settings::default() };
        app.adopt_ble_fields(&Settings { waypoint_mode: WaypointMode::Always, ..Settings::default() });
        assert_eq!(app.waypoint_mode, WaypointMode::Off, "adopt_ble_fields leaves the waypoint mode alone");
    }

    /// The v10 tail: the UI language round-trips every value, defaults **English**, sanitises an
    /// out-of-range byte back to English, and is device-only — [`adopt_ble_fields`] must never pull
    /// it across (a phone can't repick the rider's on-device language). Also pins that the appended
    /// byte still fits the same 16-byte RRAM line, so the device carve is unchanged.
    #[test]
    fn language_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().language, Language::En, "the UI language defaults to English");
        // The saved_sensors tail (v11) grew the blob to 112 B / 7 RRAM lines; the language byte kept
        // its v10 offset.
        assert_eq!(ENCODED_LEN, 112, "the settings blob is 112 B / 7 RRAM lines (v11 saved_sensors tail)");

        // Each language round-trips through the codec byte-for-byte.
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            let s = Settings { language: lang, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{lang:?} round-trips");
        }

        // An out-of-range stored byte (a newer writer, a bit-flip the CRC missed) sanitises to the
        // default English — re-stamp the CRC so only the payload byte is "wrong".
        let mut b = encode(&Settings { language: Language::De, ..Settings::default() });
        b[LANGUAGE_OFF] = 200;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert_eq!(got.language, Language::En, "an unknown language byte falls back to English");

        // A v9 blob (the previous layout) is version-rejected → the host falls back to defaults, so
        // the language reads English — the established cross-version contract (no in-place upgrade).
        let mut old = encode(&Settings { language: Language::De, ..Settings::default() });
        old[0] = 9;
        let crc = crc16(&old[0..PAYLOAD_LEN]);
        old[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&old), None, "an old-version blob is rejected (→ host uses defaults, language En)");

        // Device-only: a BLE blob's language never lands via the #456 coherence merge.
        let mut app = Settings { language: Language::De, ..Settings::default() };
        app.adopt_ble_fields(&Settings { language: Language::Es, ..Settings::default() });
        assert_eq!(app.language, Language::De, "adopt_ble_fields leaves the language alone");
    }

    /// The v11 tail: the three saved-sensor slots round-trip (present + absent), default all-empty,
    /// migrate an older blob to empty (the rejects-to-default house rule), normalise a corrupt
    /// `addr_kind`, and stay device-only ([`adopt_ble_fields`] never pulls them across — a phone can't
    /// repick the rider's sensors).
    #[test]
    fn saved_sensors_round_trip_and_migration() {
        assert_eq!(VERSION, 11, "saved_sensors is the v11 layout (settings reset on flash)");
        assert_eq!(
            Settings::default().saved_sensors,
            [SavedSensor::EMPTY; SENSOR_SLOTS],
            "a fresh device has no saved sensors",
        );

        // A mix of present + absent slots round-trips byte-for-byte: HR saved (random watch), Power
        // empty, Cadence saved (public sensor).
        let s = Settings {
            saved_sensors: [
                SavedSensor::saved(1, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]), // HR, random
                SavedSensor::EMPTY,                                          // Power, none
                SavedSensor::saved(0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]), // Cadence, public
            ],
            ..Settings::default()
        };
        assert_eq!(decode(&encode(&s)), Some(s), "present/absent slots round-trip");

        // An absent slot decodes to EMPTY even if stray address bytes sit in its region (present == 0
        // wins — no garbage address leaks into a "not set" slot).
        let mut b = encode(&s);
        let off = SENSORS_OFF + SAVED_SENSOR_LEN; // the (empty) Power slot (index 1)
        b[off] = 0; // present = false
        b[off + 1] = 1; // stray addr_kind
        b[off + 2..off + 2 + 6].copy_from_slice(&[9, 9, 9, 9, 9, 9]); // stray address
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&b).unwrap().saved_sensors[1], SavedSensor::EMPTY, "an absent slot ignores stray bytes");

        // A corrupt-but-CRC-valid `addr_kind` past 1 normalises to random (`!= 0`) — the board's own
        // reading, so a bit-flip never mis-picks the address kind.
        let mut b = encode(&s);
        let off = SENSORS_OFF; // the HR slot
        b[off + 1] = 200;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&b).unwrap().saved_sensors[0].addr_kind, 1, "an out-of-range addr_kind reads as random");

        // Migration: a v10 blob (the previous layout, before saved_sensors) is version-rejected → the
        // host falls back to defaults, so every slot reads empty — the rejects-to-default contract
        // (no in-place upgrade), exactly like every prior codec bump.
        let mut old = encode(&s);
        old[0] = 10;
        let crc = crc16(&old[0..PAYLOAD_LEN]);
        old[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&old), None, "a v10 blob is rejected → host uses defaults, sensors empty");

        // Device-only: a BLE blob's saved_sensors never lands via the #456 coherence merge.
        let mut app = s;
        app.adopt_ble_fields(&Settings::default());
        assert_eq!(app.saved_sensors, s.saved_sensors, "adopt_ble_fields leaves the saved sensors alone");
    }

    /// The picker's left/right walk order (wrapping at both ends) and the press cycle.
    #[test]
    fn language_stepping_and_cycling() {
        // Right walks En → De → Fr → Es, wrapping back to English; left is the mirror.
        assert_eq!(Language::En.stepped(1), Language::De);
        assert_eq!(Language::Es.stepped(1), Language::En, "wraps past the last language");
        assert_eq!(Language::En.stepped(-1), Language::Es, "wraps past the start");
        assert_eq!(Language::En.stepped(2), Language::Fr, "multi-detent flicks compound");
        // Press cycles one forward, exactly like a single right detent.
        assert_eq!(Language::Fr.cycled(), Language::Es);
        assert_eq!(Language::Es.cycled(), Language::En, "the press ring wraps");
        // The endonyms, in order.
        assert_eq!(Language::En.name(), "English");
        assert_eq!(Language::De.name(), "Deutsch");
        assert_eq!(Language::Fr.name(), "Français");
        assert_eq!(Language::Es.name(), "Español");
    }

    /// The v3 device-name tail: set → truncate on a char boundary at the 48-byte cap, and a
    /// corrupt stored name (bad UTF-8 or an impossible length) sanitises to factory, not garbage.
    #[test]
    fn device_name_codec_and_sanitising() {
        // 47 ASCII bytes + 'ü' (2 bytes) crosses the cap mid-char → truncates to the boundary.
        let mut long: heapless::String<64> = heapless::String::new();
        for _ in 0..47 {
            long.push('x').unwrap();
        }
        long.push('ü').unwrap();
        let name = DeviceName::from_str_lossy(&long);
        assert_eq!(name.as_str().len(), 47, "never split a UTF-8 sequence");

        let s = Settings { device_name: name, ..Settings::default() };
        assert_eq!(decode(&encode(&s)), Some(s));

        // Corrupt the stored name to invalid UTF-8, re-stamp the CRC: decode sanitises to factory.
        let mut b = encode(&s);
        b[NAME_OFF + 1] = 0xFF;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert!(got.device_name.is_empty(), "invalid UTF-8 falls back to the factory name");

        // An impossible stored length does too.
        let mut b = encode(&s);
        b[NAME_OFF] = 200;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert!(decode(&b).unwrap().device_name.is_empty());
    }

    /// The v2 tail sanitises on decode: an out-of-range cycle period is clamped, and an unknown
    /// field discriminant (a stale/newer writer) is dropped rather than loaded as a garbage tile.
    #[test]
    fn codec_sanitises_stat_tail() {
        let mut s = Settings { stat_cycle_s: 9999, ..Settings::default() };
        let mut b = encode(&s);
        // Corrupt a stored discriminant to an unknown value, then re-stamp the CRC so only the
        // payload (not the framing) is "wrong" — decode must still reject the bad tile.
        b[STAT_FIELDS_OFF + 1] = 250;
        let crc = crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert!(got.stat_cycle_s <= STAT_CYCLE_MAX, "the cycle period is clamped into range");
        assert_eq!(got.stat_fields.len(), s.stat_fields.len() - 1, "the unknown discriminant is dropped");
        // The default selection (minus the dropped head) decodes in order.
        s.stat_fields.remove(0);
        assert_eq!(got.stat_fields.as_slice(), s.stat_fields.as_slice());
    }

    /// The default round-trips too (the blank-store-falls-back path still produces a clean read).
    #[test]
    fn codec_round_trips_default() {
        let s = Settings::default();
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    /// A corrupt CRC, a blank region, a short slice, and a wrong version all decode to `None`
    /// (→ the host uses `Settings::default`), never a half-parsed value.
    #[test]
    fn codec_rejects_bad_blobs() {
        let mut b = encode(&Settings::default());
        b[6] ^= 0xFF; // flip a payload byte without fixing the CRC
        assert_eq!(decode(&b), None, "a CRC mismatch is rejected");
        assert_eq!(decode(&[0u8; ENCODED_LEN]), None, "a blank (all-zero) region is rejected");
        assert_eq!(decode(&[0xFF; ENCODED_LEN]), None, "an erased (all-ones) region is rejected");
        assert_eq!(decode(&encode(&Settings::default())[..ENCODED_LEN - 1]), None, "a short slice is rejected");
        let mut wrong = encode(&Settings::default());
        wrong[0] = VERSION + 1; // bump version, fix the CRC so only the version differs
        let crc = crc16(&wrong[0..PAYLOAD_LEN]);
        wrong[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&wrong), None, "a future version is rejected");
    }

    /// A valid-CRC blob carrying an out-of-range field is sanitised on decode, not trusted.
    #[test]
    fn decode_sanitises_out_of_range_fields() {
        let mut s = Settings::default();
        s.clock.month = 13;
        s.clock.day = 99;
        s.fix_interval_s = 9999;
        // `encode` already stamps a correct CRC over the whole (bogus-but-in-layout) payload, so the
        // blob is valid-CRC; `decode` must accept it and sanitise the out-of-range fields.
        let b = encode(&s);
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert!((1..=12).contains(&got.clock.month));
        assert!(got.clock.day >= 1 && got.clock.day <= 31);
        assert!(got.fix_interval_s <= FIX_INTERVAL_MAX);
    }

    // ---- durable id high-water marks (#450) ----

    /// The 16-byte id-marks line round-trips, and every torn/blank/foreign shape decodes to
    /// `None` — "no floor", the fall-back-to-scan-max behaviour.
    #[test]
    fn id_marks_codec_round_trips_and_rejects_torn_lines() {
        let m = IdMarks { next_route_id: 7, next_ride_id: 41 };
        assert_eq!(decode_id_marks(&encode_id_marks(&m)), Some(m));
        assert_eq!(decode_id_marks(&encode_id_marks(&IdMarks::default())), Some(IdMarks::default()));

        assert_eq!(decode_id_marks(&[0u8; ID_MARKS_LEN]), None, "a blank (all-zero) line is no floor");
        assert_eq!(decode_id_marks(&[0xFF; ID_MARKS_LEN]), None, "an erased (all-ones) line is no floor");
        assert_eq!(decode_id_marks(&encode_id_marks(&m)[..ID_MARKS_LEN - 1]), None, "a short slice is rejected");
        let mut torn = encode_id_marks(&m);
        torn[7] ^= 0xFF; // flip a payload byte without fixing the CRC — the torn-write shape
        assert_eq!(decode_id_marks(&torn), None, "a CRC mismatch (torn write) is no floor");
        let mut old = encode_id_marks(&m);
        old[4] = ID_MARKS_VERSION + 1;
        let crc = crc16(&old[0..ID_MARKS_PAYLOAD]);
        old[ID_MARKS_PAYLOAD..ID_MARKS_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_id_marks(&old), None, "a foreign layout version is no floor");
    }

    // ---- store-epoch nonce (protocol v2, #632/#767; card-resident #776) ----

    /// The 12-byte store-epoch card file round-trips, and every torn/absent/foreign shape decodes to
    /// `None` — "no epoch", which the mint rule reads as clause 1.
    #[test]
    fn store_epoch_codec_round_trips_and_rejects_torn_lines() {
        assert_eq!(encode_store_epoch(0).len(), STORE_EPOCH_LEN, "the file is 12 bytes, no RRAM padding");
        assert_eq!(decode_store_epoch(&encode_store_epoch(0xDEAD_BEEF)), Some(0xDEAD_BEEF));
        assert_eq!(decode_store_epoch(&encode_store_epoch(0)), Some(0), "a zero nonce is a legal value");

        assert_eq!(decode_store_epoch(&[0u8; STORE_EPOCH_LEN]), None, "a blank (all-zero) file is no epoch");
        assert_eq!(decode_store_epoch(&[0xFF; STORE_EPOCH_LEN]), None, "an erased (all-ones) file is no epoch");
        assert_eq!(decode_store_epoch(&[]), None, "an absent (empty) file is no epoch");
        assert_eq!(
            decode_store_epoch(&encode_store_epoch(0xDEAD_BEEF)[..STORE_EPOCH_LEN - 1]),
            None,
            "a short slice is rejected"
        );
        let mut torn = encode_store_epoch(0xDEAD_BEEF);
        torn[7] ^= 0xFF; // flip an epoch byte without fixing the CRC — the torn-write shape
        assert_eq!(decode_store_epoch(&torn), None, "a CRC mismatch (torn write) is no epoch");
        let mut old = encode_store_epoch(0xDEAD_BEEF);
        old[4] = STORE_EPOCH_VERSION + 1;
        let crc = crc16(&old[0..STORE_EPOCH_PAYLOAD]);
        old[STORE_EPOCH_PAYLOAD..STORE_EPOCH_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_store_epoch(&old), None, "a foreign layout version is no epoch");
    }

    /// The mint rule's four cases, plus the two invariants the 2026-07-12 review added. `FRESH` is
    /// the TRNG word the board draws; the pure function never draws it, so the test is deterministic.
    #[test]
    fn store_epoch_mint_rule() {
        const FRESH: u32 = 0x1234_5678;
        let floor = IdMarks { next_route_id: 9, next_ride_id: 4 };

        // Steady state: a valid card epoch + valid floors → keep the card's epoch, write nothing.
        assert_eq!(store_epoch_mint(Some(0xABCD), Some(floor), FRESH), None);

        // Clause 1 only (card epoch absent/torn, id-marks *intact*): mint a fresh epoch but keep the
        // existing floors verbatim — a torn/absent epoch file must never cost the durable id floor.
        assert_eq!(store_epoch_mint(None, Some(floor), FRESH), Some((FRESH, floor)));

        // Clause 2 (id-marks blank/torn, card epoch intact): a lost floor is a new era → mint a fresh
        // epoch even though the card's was valid, and (re)seed the floor to "no floor" (default),
        // which the store re-derives from the card scan via `max(scan_max + 1, floor)`.
        assert_eq!(store_epoch_mint(Some(0xABCD), None, FRESH), Some((FRESH, IdMarks::default())));

        // Fresh device (no card epoch + no floor): mint + seed default floors.
        assert_eq!(store_epoch_mint(None, None, FRESH), Some((FRESH, IdMarks::default())));
    }

    /// Card-swap semantics — the whole point of #776 (a pure-function pin). The epoch now rides the
    /// card, so swapping cards **transplants** the store identity with no mint, and swapping back
    /// restores the original era. The device's own RRAM floor stays intact throughout (a card swap is
    /// never an era event by itself — clause 2 is only about a *lost* floor).
    #[test]
    fn store_epoch_card_swap_transplants_the_era() {
        const FRESH: u32 = 0xDEAD_0001; // never consumed: every step below is a "keep"
        let floor = IdMarks { next_route_id: 3, next_ride_id: 2 };
        let e_a = 0xAAAA_1111u32; // card A's epoch
        let e_b = 0xBBBB_2222u32; // card B's epoch

        // Card A mounted, steady state → no mint, the served epoch is card A's.
        assert_eq!(store_epoch_mint(Some(e_a), Some(floor), FRESH), None, "card A steady: no mint");

        // Swap to card B (its own valid epoch, RRAM floor unchanged) → no mint, the store transplants
        // to card B's era. The served epoch is now e_b — a *different* store identity on the wire.
        assert_eq!(store_epoch_mint(Some(e_b), Some(floor), FRESH), None, "card B adopted verbatim — transplant");

        // Swap back to card A → no mint again, e_a served. The original era is restored intact.
        assert_eq!(store_epoch_mint(Some(e_a), Some(floor), FRESH), None, "swap-back restores card A's era");
    }

    /// The invariant *valid epoch ⇒ valid id-marks at mint*: after a mint the caller persists both
    /// (the epoch to the card file, the marks to the RRAM line), and a re-decode of what it wrote
    /// leaves **both** valid — so the next boot can't mistake the fresh state for a torn one.
    #[test]
    fn store_epoch_mint_writes_a_valid_marks_line() {
        const FRESH: u32 = 0x0BAD_F00D;
        // Clause-2 mint (blank id-marks + intact card epoch), the review's headline case.
        let (new_epoch, new_marks) = store_epoch_mint(Some(0x55), None, FRESH).expect("clause 2 mints");
        // Persist-then-reload both records exactly as the board does.
        assert_eq!(decode_store_epoch(&encode_store_epoch(new_epoch)), Some(FRESH), "epoch file valid post-mint");
        assert_eq!(decode_id_marks(&encode_id_marks(&new_marks)), Some(new_marks), "id-marks line valid post-mint");
    }

    /// Fresh-device stability: a device that never saves a ride or uploads a route mints **once**,
    /// and every subsequent boot (its epoch file + id-marks line now valid) keeps that same epoch —
    /// no clause-2 churn.
    #[test]
    fn store_epoch_fresh_device_stability() {
        const FRESH: u32 = 0xFEED_BEEF;
        // Boot 1: no card epoch + no floor → mint.
        let (epoch, marks) = store_epoch_mint(None, None, FRESH).expect("first boot mints");
        // The board writes both records; model them as the encoded card file + RRAM line it persisted.
        let epoch_line = encode_store_epoch(epoch);
        let marks_line = encode_id_marks(&marks);

        // Boots 2..N with no rides/uploads: both read back valid, so the decision is "keep" —
        // a *different* TRNG word each boot is irrelevant because the function never reaches it.
        for boot_fresh in [0x1111_1111u32, 0x2222_2222, 0x3333_3333] {
            let e = decode_store_epoch(&epoch_line);
            let m = decode_id_marks(&marks_line);
            assert_eq!(store_epoch_mint(e, m, boot_fresh), None, "a settled fresh device never re-mints");
        }
        assert_eq!(decode_store_epoch(&epoch_line), Some(epoch), "and the epoch is stable across boots");
    }

    // ---- DFU arm marker (boot-outcome popup) ----

    /// The 48-byte arm-marker slot round-trips (generation + verbatim version string), and every
    /// torn/blank/foreign shape decodes to `None` — "no arm happened", a plain boot.
    #[test]
    fn arm_marker_codec_round_trips_and_rejects_torn_slots() {
        let m = ArmMarker { generation: 3, staged: heapless::String::try_from("v0.4.0-12-gabc1234").unwrap() };
        assert_eq!(decode_arm_marker(&encode_arm_marker(&m)), Some(m.clone()));
        let empty = ArmMarker { generation: 1, staged: heapless::String::new() };
        assert_eq!(decode_arm_marker(&encode_arm_marker(&empty)), Some(empty), "an empty version string is legal");

        assert_eq!(decode_arm_marker(&[0u8; ARM_MARKER_LEN]), None, "a blank (all-zero) slot is no marker");
        assert_eq!(decode_arm_marker(&[0xFF; ARM_MARKER_LEN]), None, "an erased (all-ones) slot is no marker");
        assert_eq!(decode_arm_marker(&encode_arm_marker(&m)[..ARM_MARKER_LEN - 1]), None, "a short slice is rejected");
        let mut torn = encode_arm_marker(&m);
        torn[15] ^= 0xFF; // flip a version-string byte without fixing the CRC — the torn-write shape
        assert_eq!(decode_arm_marker(&torn), None, "a CRC mismatch (torn write) is no marker");
        let mut old = encode_arm_marker(&m);
        old[4] = ARM_MARKER_VERSION + 1;
        let crc = crc16(&old[0..ARM_MARKER_PAYLOAD]);
        old[ARM_MARKER_PAYLOAD..ARM_MARKER_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_arm_marker(&old), None, "a foreign layout version is no marker");
        let mut bad_utf8 = encode_arm_marker(&m);
        bad_utf8[12] = 0xFF; // a non-UTF-8 version byte
        let crc = crc16(&bad_utf8[0..ARM_MARKER_PAYLOAD]);
        bad_utf8[ARM_MARKER_PAYLOAD..ARM_MARKER_PAYLOAD + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode_arm_marker(&bad_utf8), None, "a non-UTF-8 version string is no marker");
    }

    /// The DoD guarantee: with the marks persisted across "reboots", an id is **never reused after
    /// a delete** — even when the delete lowers the card's scan-max below an already-issued id.
    /// Simulates the store as the set of live filename-encoded ids, exactly what a mount scan sees.
    #[test]
    fn id_allocation_never_reuses_after_delete() {
        let mut card: heapless::Vec<u16, 8> = heapless::Vec::new(); // the live RD{id}/RT{id} files
        let mut marks = IdMarks::default(); // fresh device: no floor
        let scan_next = |card: &[u16]| card.iter().max().map_or(0, |m| m + 1);

        // Three rides saved: 0, 1, 2 — identical to scan-max+1 while nothing deletes.
        for want in 0..3u16 {
            let id = marks.alloc_ride(scan_next(&card));
            assert_eq!(id, want);
            let _ = card.push(id);
        }

        // Delete the highest (id 2) — the trap: scan-max+1 alone would re-issue 2.
        card.retain(|&id| id != 2);
        // "Reboot": the floor survives in RRAM (marks kept), the scan is rebuilt from the card.
        let mut rebooted = decode_id_marks(&encode_id_marks(&marks)).expect("persisted floor survives");
        let id = rebooted.alloc_ride(scan_next(&card));
        assert_eq!(id, 3, "the deleted id 2 is never reused");
        let _ = card.push(id);

        // A torn floor line falls back cleanly: allocation degrades to scan-max+1 (no floor) —
        // ids can collide with tombstones again, but only exactly as they did before the marks.
        let mut torn = encode_id_marks(&rebooted);
        torn[9] ^= 0x55;
        let mut no_floor = decode_id_marks(&torn).unwrap_or_default();
        assert_eq!(no_floor.alloc_ride(scan_next(&card)), 4, "torn line → scan-max+1");

        // The two namespaces are independent: route allocations never disturb ride marks.
        let mut m = IdMarks::default();
        assert_eq!(m.alloc_route(5), 5);
        assert_eq!(m.next_ride_id, 0, "route allocation leaves the ride floor untouched");
        assert_eq!(m.alloc_route(0), 6, "and the route floor advanced past the assignment");
    }

    // ---- synced-ride sidecar (#454) ----

    /// A synced set round-trips through the sidecar codec — order-insensitive membership, exact ids.
    #[test]
    fn synced_rides_codec_round_trips() {
        let mut set = SyncedRides::new();
        assert!(set.insert(3));
        assert!(set.insert(7));
        assert!(set.insert(41));
        assert!(!set.insert(7), "a duplicate insert is a no-op");
        assert!(set.contains(3) && set.contains(7) && set.contains(41));
        assert!(!set.contains(4));

        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(&set, &mut buf);
        assert_eq!(n, synced_rides_len(3));
        let got = decode_synced_rides(&buf[..n]);
        assert_eq!(got, set);

        // The empty set is a valid, non-crashing round-trip too.
        let empty = SyncedRides::new();
        let n = encode_synced_rides(&empty, &mut buf);
        assert_eq!(decode_synced_rides(&buf[..n]), empty);
    }

    /// The DoD guarantee: a torn, blank, short, or foreign sidecar decodes to "nothing synced" —
    /// never a crash, never a false positive that would drop the warning footer on an unsynced ride.
    #[test]
    fn synced_rides_torn_or_missing_reads_as_nothing_synced() {
        let mut set = SyncedRides::new();
        set.insert(9);
        set.insert(12);
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(&set, &mut buf);

        assert_eq!(decode_synced_rides(&[]), SyncedRides::new(), "an absent sidecar → nothing synced");
        assert_eq!(decode_synced_rides(&[0u8; 4]), SyncedRides::new(), "a runt slice → nothing synced");
        assert_eq!(decode_synced_rides(&[0u8; SYNCED_HEADER_LEN + 2]), SyncedRides::new(), "a blank page");
        assert_eq!(decode_synced_rides(&[0xFF; 64]), SyncedRides::new(), "an erased page → nothing synced");

        let mut torn = buf;
        torn[SYNCED_HEADER_LEN] ^= 0xFF; // flip an id byte without fixing the CRC
        assert_eq!(decode_synced_rides(&torn[..n]), SyncedRides::new(), "a CRC mismatch → nothing synced");

        let mut bad_count = buf;
        bad_count[6..8].copy_from_slice(&0xFFFFu16.to_le_bytes()); // claim more ids than the slice holds
        assert_eq!(decode_synced_rides(&bad_count[..n]), SyncedRides::new(), "an overrunning count → nothing");

        let mut old = buf;
        old[4] = SYNCED_VERSION + 1;
        assert_eq!(decode_synced_rides(&old[..n]), SyncedRides::new(), "a foreign version → nothing synced");
    }

    /// `remove` retires an id (the deleted-ride cleanup) without disturbing the rest.
    #[test]
    fn synced_rides_remove_retires_one_id() {
        let mut set = SyncedRides::new();
        set.insert(1);
        set.insert(2);
        set.insert(3);
        assert!(set.remove(2));
        assert!(!set.remove(2), "removing an absent id is a no-op");
        assert!(set.contains(1) && !set.contains(2) && set.contains(3));
    }

    /// The route-CRC sidecar round-trips id → crc32 pairs byte-for-byte, empty included.
    #[test]
    fn route_crcs_codec_round_trips() {
        let mut map = RouteCrcs::new();
        assert!(map.insert(1, 0xDEAD_BEEF));
        assert!(map.insert(7, 0));
        assert!(map.insert(65535, 0x0000_0001));
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = encode_route_crcs(&map, &mut buf);
        assert_eq!(n, route_crcs_len(3));
        let got = decode_route_crcs(&buf[..n]);
        assert_eq!(got, map);
        assert_eq!(got.get(1), Some(0xDEAD_BEEF));
        assert_eq!(got.get(7), Some(0), "a genuine CRC of 0 is a stored, retrievable value");
        assert_eq!(got.get(2), None, "an unlisted route has no CRC (→ lazily filled)");

        let empty = RouteCrcs::new();
        let n = encode_route_crcs(&empty, &mut buf);
        assert_eq!(decode_route_crcs(&buf[..n]), empty);
    }

    /// A torn / missing / foreign route-CRC sidecar decodes to the empty map (serve `0 = unknown`).
    #[test]
    fn route_crcs_torn_or_missing_reads_as_empty() {
        let mut map = RouteCrcs::new();
        map.insert(9, 0x1111_2222);
        map.insert(12, 0x3333_4444);
        let mut buf = [0u8; ROUTE_CRCS_MAX_LEN];
        let n = encode_route_crcs(&map, &mut buf);

        assert_eq!(decode_route_crcs(&[]), RouteCrcs::new(), "an absent sidecar → empty");
        assert_eq!(decode_route_crcs(&[0u8; 4]), RouteCrcs::new(), "a runt slice → empty");
        assert_eq!(decode_route_crcs(&[0u8; ROUTE_CRCS_HEADER_LEN + 2]), RouteCrcs::new(), "a blank page");
        assert_eq!(decode_route_crcs(&[0xFF; 64]), RouteCrcs::new(), "an erased page → empty");

        let mut torn = buf;
        torn[ROUTE_CRCS_HEADER_LEN] ^= 0xFF; // flip an id byte without fixing the CRC
        assert_eq!(decode_route_crcs(&torn[..n]), RouteCrcs::new(), "a CRC mismatch → empty");

        let mut bad_count = buf;
        bad_count[6..8].copy_from_slice(&0xFFFFu16.to_le_bytes()); // claim more entries than the slice holds
        assert_eq!(decode_route_crcs(&bad_count[..n]), RouteCrcs::new(), "an overrunning count → empty");

        let mut old = buf;
        old[4] = ROUTE_CRCS_VERSION + 1;
        assert_eq!(decode_route_crcs(&old[..n]), RouteCrcs::new(), "a foreign version → empty");
    }

    /// `insert` upserts (a changed CRC rewrites, an identical one is a no-op) and `remove` retires
    /// one id — the upload-replace + delete cleanup paths.
    #[test]
    fn route_crcs_upsert_and_remove() {
        let mut map = RouteCrcs::new();
        assert!(map.insert(5, 0xAAAA), "a new id changes the map");
        assert!(!map.insert(5, 0xAAAA), "the same id+crc is a no-op");
        assert!(map.insert(5, 0xBBBB), "a replaced route's new crc rewrites in place");
        assert_eq!(map.get(5), Some(0xBBBB));
        assert_eq!(map.entries().len(), 1, "upsert never duplicates an id");
        assert!(map.remove(5));
        assert!(!map.remove(5), "removing an absent id is a no-op");
        assert_eq!(map.get(5), None);
    }

    /// `from_unix` is the exact inverse of `to_unix` (minute granularity) across epoch, a leap day,
    /// and a modern date — the Rides screen dates a ride off this.
    #[test]
    fn datetime_from_unix_inverts_to_unix() {
        for dt in [
            DateTime { year: 1970, month: 1, day: 1, hour: 0, minute: 0 },
            DateTime { year: 2000, month: 2, day: 29, hour: 12, minute: 34 },
            DateTime { year: 2026, month: 7, day: 5, hour: 9, minute: 41 },
            DateTime { year: 2038, month: 1, day: 19, hour: 3, minute: 14 },
        ] {
            assert_eq!(DateTime::from_unix(dt.to_unix()), dt, "round-trip {dt:?}");
        }
        // Seconds are dropped to the minute, not rounded.
        let d = DateTime::from_unix(59);
        assert_eq!((d.year, d.month, d.day, d.hour, d.minute), (1970, 1, 1, 0, 0));
    }

    /// February's day count follows the leap rule, and stepping the month off Jan 31 re-pins
    /// the day to the (possibly leap) Feb length rather than leaving an impossible Feb 31.
    #[test]
    fn datetime_month_length_is_leap_aware() {
        assert_eq!(DateTime::month_len(2024, 2), 29, "2024 is a leap year");
        assert_eq!(DateTime::month_len(2025, 2), 28, "2025 is not");
        assert_eq!(DateTime::month_len(2000, 2), 29, "div-by-400 is a leap year");
        assert_eq!(DateTime::month_len(2100, 2), 28, "div-by-100-not-400 is not");

        let mut leap = DateTime { year: 2024, month: 1, day: 31, hour: 0, minute: 0 };
        leap.step_month(1); // Jan 31 → Feb
        assert_eq!((leap.month, leap.day), (2, 29), "Feb 29 in a leap year");
        let mut common = DateTime { year: 2025, month: 1, day: 31, hour: 0, minute: 0 };
        common.step_month(1);
        assert_eq!((common.month, common.day), (2, 28), "Feb 28 in a common year");
    }

    /// Every field stepper wraps at its bounds rather than running off the end.
    #[test]
    fn datetime_steppers_wrap() {
        let mut d = DateTime { year: DATETIME_MAX_YEAR, month: 12, day: 30, hour: 23, minute: 59 };
        d.step_year(1);
        assert_eq!(d.year, DATETIME_MIN_YEAR, "year wraps 2099 → 2020");
        d.step_month(1);
        assert_eq!(d.month, 1, "month wraps 12 → 1");
        d.step_hour(1);
        assert_eq!(d.hour, 0, "hour wraps 23 → 0");
        d.step_minute(1);
        assert_eq!(d.minute, 0, "minute wraps 59 → 0");
        d.step_year(-1);
        assert_eq!(d.year, DATETIME_MAX_YEAR, "and backward off the bottom wraps to the top");
    }

    /// `add_minutes` carries across every boundary the field steppers deliberately *don't*:
    /// minute → hour → day → month → year, and through the leap-day specifically.
    #[test]
    fn datetime_add_minutes_carries_across_fields() {
        let base = DateTime { year: 2025, month: 6, day: 29, hour: 14, minute: 40 };
        // Within the minute field.
        assert_eq!(base.add_minutes(5).minute, 45);
        // Minute → hour carry (40 + 25 = 65 → 15, hour +1).
        let h = base.add_minutes(25);
        assert_eq!((h.hour, h.minute), (15, 5));
        // Minute → hour → day carry: 23:59 + 1 = next day 00:00.
        let midnight = DateTime { year: 2025, month: 6, day: 29, hour: 23, minute: 59 };
        let d = midnight.add_minutes(1);
        assert_eq!((d.day, d.hour, d.minute), (30, 0, 0), "23:59 + 1 rolls into the next day");
        // Day → month carry: Jun 30 23:00 + 2 h → Jul 1 01:00 (June has 30 days).
        let m = DateTime { year: 2025, month: 6, day: 30, hour: 23, minute: 0 }.add_minutes(120);
        assert_eq!((m.month, m.day, m.hour), (7, 1, 1), "end of June rolls into July");
        // Month → year carry: Dec 31 23:59 + 1 → Jan 1 of the next year.
        let y = DateTime { year: 2025, month: 12, day: 31, hour: 23, minute: 59 }.add_minutes(1);
        assert_eq!((y.year, y.month, y.day, y.hour, y.minute), (2026, 1, 1, 0, 0), "new year");
    }

    /// February's length is taken from the year the advance *lands* in, so a leap-year Feb 28 + 1
    /// day is Feb 29 while a common-year one is Mar 1.
    #[test]
    fn datetime_add_minutes_is_leap_aware() {
        let leap = DateTime { year: 2024, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!((leap.month, leap.day), (2, 29), "2024 has a Feb 29 to land on");
        let common = DateTime { year: 2025, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!((common.month, common.day), (3, 1), "2025 skips straight to March");
        // A multi-day advance that *crosses* Feb 29 counts it: Feb 27 2024 + 3 days = Mar 1.
        let across = DateTime { year: 2024, month: 2, day: 27, hour: 0, minute: 0 }.add_minutes(3 * 24 * 60);
        assert_eq!((across.month, across.day), (3, 1), "the leap day is one of the three crossed");
    }

    /// `with_offset` shifts a stamp by a signed minute offset, rolling the *date* in either
    /// direction when the shift crosses midnight (the GPS UTC-anchor → local-time conversion).
    #[test]
    fn datetime_with_offset_rolls_the_date_both_ways() {
        let base = DateTime { year: 2025, month: 6, day: 29, hour: 23, minute: 30 };
        assert_eq!(base.with_offset(0), base, "a zero offset is identity");
        let within = base.with_offset(15); // still the same day
        assert_eq!((within.day, within.hour, within.minute), (29, 23, 45));
        let next = base.with_offset(60); // 23:30 + 01:00 → 00:30 the next day
        assert_eq!((next.day, next.hour, next.minute), (30, 0, 30), "forward across midnight rolls the day");
        let early = DateTime { year: 2025, month: 6, day: 29, hour: 0, minute: 30 };
        let prev = early.with_offset(-45); // 00:30 − 00:45 → 23:45 the previous day (a :45 zone)
        assert_eq!((prev.day, prev.hour, prev.minute), (28, 23, 45), "backward across midnight rolls back");
        // A backward roll across a month boundary borrows the previous month's length.
        let month_edge = DateTime { year: 2025, month: 7, day: 1, hour: 0, minute: 0 };
        let back = month_edge.with_offset(-60); // → Jun 30 23:00
        assert_eq!((back.month, back.day, back.hour), (6, 30, 23), "the borrow steps into June (30 days)");
    }

    /// `add_minutes` is defensive against an unsanitised stamp: a day past the month length doesn't
    /// underflow the unsigned day-walk (a debug panic / garbage day), and a huge advance saturates
    /// at the end of `MAX_YEAR` rather than rolling to year 2100+.
    #[test]
    fn add_minutes_guards_bad_input_and_saturates_the_year() {
        // Day 99 in a 30-day month: clamped, not underflowed — and no panic.
        let bad = DateTime { year: 2025, month: 6, day: 99, hour: 0, minute: 0 };
        assert!((1..=30).contains(&bad.add_minutes(0).day), "an over-long day is re-pinned into the month");
        // Near the top of the range + two years of minutes pins at the last representable day.
        let near_max = DateTime { year: DATETIME_MAX_YEAR, month: 12, day: 31, hour: 12, minute: 0 };
        let sat = add_minutes_bounded(near_max, 2 * 365 * 24 * 60);
        assert_eq!(sat.year, DATETIME_MAX_YEAR, "the app clock never climbs past its maximum year");
        assert_eq!((sat.month, sat.day), (12, 31), "it saturates at Dec 31 rather than rolling over");
    }

    /// `local_clock` applies the UTC offset **only** in GPS mode — the hand-set manual clock is
    /// already local, so applying the offset there would double-count it.
    #[test]
    fn local_clock_applies_offset_only_in_gps_mode() {
        let clock = DateTime { year: 2025, month: 6, day: 29, hour: 12, minute: 0 };
        let manual = Settings { gps_time: false, clock, utc_offset_min: 120, ..Settings::default() };
        assert_eq!(manual.local_clock(), clock, "manual: the clock is already local, offset ignored");
        let gps = Settings { gps_time: true, clock, utc_offset_min: 120, ..Settings::default() };
        let local = gps.local_clock();
        assert_eq!((local.hour, local.minute), (14, 0), "GPS: local = UTC anchor + offset");
        assert_eq!((gps.clock.hour, gps.clock.minute), (12, 0), "the stored UTC anchor itself did not move");
    }

    /// `to_unix` against independently-computed references (`date -u +%s`), including the
    /// leap-day and year-boundary edges the era arithmetic has to carry.
    #[test]
    fn to_unix_matches_reference_timestamps() {
        let dt = |year, month, day, hour, minute| DateTime { year, month, day, hour, minute };
        assert_eq!(dt(2020, 1, 1, 0, 0).to_unix(), 1_577_836_800);
        assert_eq!(dt(2024, 2, 29, 12, 30).to_unix(), 1_709_209_800, "leap day");
        assert_eq!(dt(2026, 7, 2, 9, 33).to_unix(), 1_782_984_780);
        assert_eq!(dt(2026, 12, 31, 23, 59).to_unix(), 1_798_761_540, "year boundary");
        assert_eq!(dt(2099, 12, 31, 23, 59).to_unix(), 4_102_444_740, "the top of the range fits u32");
    }

    /// The unit conversions are no-ops for metric and the right scale for imperial.
    #[test]
    fn unit_conversions() {
        assert_eq!(Units::Metric.dist(10.0), 10.0);
        assert_eq!(Units::Metric.speed(30.0), 30.0);
        assert_eq!(Units::Metric.elev(100.0), 100.0);
        assert!((Units::Imperial.dist(10.0) - 6.21371).abs() < 1e-3, "10 km ≈ 6.21 mi");
        assert!((Units::Imperial.speed(100.0) - 62.1371).abs() < 1e-2, "100 km/h ≈ 62.1 mph");
        assert!((Units::Imperial.elev(1000.0) - 3280.84).abs() < 1e-1, "1000 m ≈ 3281 ft");
        assert_eq!(Units::Metric.toggled(), Units::Imperial);
        assert_eq!(Units::Imperial.toggled(), Units::Metric);
    }

    // ==================== settings coherence (#456) ====================
    //
    // The board firmware double-caches settings: the ride loop holds the live `App` copy, the BLE
    // `ObjectStore` holds a Config-read cache, and the RRAM blob (encode/decode below) is the
    // single source of truth behind both. These tests model that store with a byte buffer and
    // exercise the two coherence operations the board wires up:
    //   - `apply_config` (BLE write): sets units + name in the store cache and persists to RRAM;
    //   - the ride loop's *reload-before-save*: `adopt_ble_fields` from the fresh RRAM blob into the
    //     app copy, so the app's change-detection save can't clobber the phone's write;
    //   - the object-store *cache refresh*: reload the whole cache from RRAM so a Config read after
    //     an on-device change serves fresh values.
    // Modelling the store as the actual codec buffer keeps the test honest: it's the same bytes the
    // RRAM/file stores round-trip.

    /// A minimal stand-in for the persistent RRAM/file store: the one canonical settings blob.
    struct FakeStore {
        blob: [u8; ENCODED_LEN],
    }
    impl FakeStore {
        fn new(s: &Settings) -> Self {
            FakeStore { blob: encode(s) }
        }
        fn load(&self) -> Settings {
            decode(&self.blob).expect("the store always holds a valid blob in these tests")
        }
        fn save(&mut self, s: &Settings) {
            self.blob = encode(s);
        }
    }

    /// `adopt_ble_fields` pulls only units + name across, leaving every on-device-only field alone.
    #[test]
    fn adopt_ble_fields_is_narrow() {
        let mut app = Settings {
            units: Units::Metric,
            fix_interval_s: 7,
            power_saver: true,
            clock: DateTime { year: 2030, month: 3, day: 4, hour: 5, minute: 6 },
            ..Settings::default()
        };
        let ble = Settings {
            units: Units::Imperial,
            device_name: DeviceName::from_str_lossy("Timo's OBC"),
            // These would be *wrong* to adopt — the phone never writes them.
            fix_interval_s: 1,
            power_saver: false,
            ..Settings::default()
        };
        app.adopt_ble_fields(&ble);
        assert_eq!(app.units, Units::Imperial, "units are BLE-owned → adopted");
        assert_eq!(app.device_name.as_str(), "Timo's OBC", "the name is BLE-owned → adopted");
        assert_eq!(app.fix_interval_s, 7, "the GPS interval is device-only → untouched");
        assert!(app.power_saver, "power-saver is device-only → untouched");
        assert_eq!(app.clock.year, 2030, "the clock is device-only → untouched");
    }

    /// Direction 1 — phone → device, *with the clobber regression*: a BLE Config write lands, then
    /// the app runs its change-detection save. Without the reload the app would write its stale
    /// units back over the phone's; with the reload-before-save the phone's units survive.
    #[test]
    fn ble_write_then_app_save_keeps_ble_values() {
        // Boot: everyone metric, no name. The app also has a device-only edit pending (say a
        // fix-interval change) that its next save must persist.
        let boot = Settings::default();
        let mut store = FakeStore::new(&boot);
        let mut app = boot;
        app.fix_interval_s = 9; // a pending on-device edit the app will save this frame
        app.ble_enabled = false; // and a pending radio-off toggle (device-only, like the interval)

        // Phone writes units=imperial + a rename. `apply_config`: object-store cache + RRAM.
        let mut objstore = store.load();
        objstore.adopt_ble_fields(&Settings {
            units: Units::Imperial,
            device_name: DeviceName::from_str_lossy("Ridgeline"),
            ..Settings::default()
        });
        store.save(&objstore);

        // The ride loop sees the settings-changed signal and reloads BLE fields into the app copy
        // *before* its change-detection save.
        app.adopt_ble_fields(&store.load());
        // Now the app saves (its own dirty edit flushed on leaving the settings screen).
        store.save(&app);

        let persisted = store.load();
        assert_eq!(persisted.units, Units::Imperial, "the phone's units survive the app save (no clobber)");
        assert_eq!(persisted.device_name.as_str(), "Ridgeline", "the phone's rename survives too");
        assert_eq!(persisted.fix_interval_s, 9, "the app's own device-only edit still persists");
        assert!(!persisted.ble_enabled, "the radio-off toggle survives the coherence path (device-only)");
    }

    /// The clobber *without* the fix, pinned as the exact bug #456 removes: if the app saves its
    /// stale copy without reloading, it overwrites the phone's write. (Guards against a future
    /// refactor dropping the reload.)
    #[test]
    fn app_save_without_reload_would_clobber() {
        let mut store = FakeStore::new(&Settings::default());
        let app = Settings::default(); // still metric, no name

        // Phone writes imperial + a name.
        let mut objstore = store.load();
        objstore.adopt_ble_fields(&Settings {
            units: Units::Imperial,
            device_name: DeviceName::from_str_lossy("Ridgeline"),
            ..Settings::default()
        });
        store.save(&objstore);

        // App saves its stale copy *without* the reload — this is the pre-fix behaviour.
        store.save(&app);
        let clobbered = store.load();
        assert_eq!(clobbered.units, Units::Metric, "the bug: the app's stale metric clobbers the phone's imperial");
        assert!(clobbered.device_name.is_empty(), "and the phone's rename is lost");
    }

    /// Direction 2 — device → phone: units change on-device (app copy + RRAM), then a Config read
    /// must serve fresh values. The object-store cache is refreshed from RRAM before the read.
    #[test]
    fn app_change_then_ble_read_serves_fresh() {
        let boot = Settings::default(); // metric
        let mut store = FakeStore::new(&boot);
        // The BLE object-store cache, seeded at boot.
        let mut objstore_cache = store.load();
        assert_eq!(objstore_cache.units, Units::Metric);

        // On-device: the rider flips to imperial. The ride loop persists the app copy to RRAM.
        let mut app = boot;
        app.units = Units::Imperial;
        store.save(&app);

        // A Config read arrives. The object-store refreshes its cache from RRAM first (the fix),
        // so it serves the fresh value rather than the stale boot cache.
        objstore_cache = store.load();
        assert_eq!(objstore_cache.units, Units::Imperial, "the Config read serves the on-device change, no reboot");
    }
}
