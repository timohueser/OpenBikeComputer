//! Persistent device settings + their byte codec.
//!
//! [`Settings`] is the small POD the settings screens edit and the host persists across a reboot.
//! It is `Copy + PartialEq`, so [`App::apply_gesture`](crate::App::apply_gesture) detects a change
//! with a single comparison and flags a save. The byte codec ([`encode`]/[`decode`]) is a
//! versioned, CRC-checked, fixed-length blob shared by **both** stores (sim file, firmware RRAM
//! region — see [`SettingsStore`](obc_ports::SettingsStore)), so a blank or corrupt read falls
//! back to [`Settings::default`] rather than loading garbage.

use crate::i18n::{t, Msg};
use crate::retention::RideRetention;
use crate::settings_enum::setting_enum;
use crate::stat_fields::{StatFieldList, MAX_STAT_FIELDS};

pub(crate) use obc_ports::DateTime;

/// First year accepted by the settings codec and Date & Time editor.
pub const DATETIME_MIN_YEAR: u16 = 2020;
/// Last year accepted by the settings codec and Date & Time editor.
pub const DATETIME_MAX_YEAR: u16 = 2099;

/// App-owned persisted-value policy for the dependency-neutral [`DateTime`].
///
/// Calendar arithmetic (`add_minutes`, UTC offsets, leap years) stays inherent on `DateTime` in
/// `obc-ports`; [`sanitize`](DateTimeEditorExt::sanitize) is available when this trait is in scope
/// because its storage range (2020–2099) is a choice specific to OpenBikeComputer. (Manual
/// date/time editing — and its per-field wrapping steppers — was removed in #641; only `sanitize`
/// remains, applied after a settings decode.)
pub trait DateTimeEditorExt {
    /// Force every field into the range accepted by the settings codec.
    fn sanitize(&mut self);
}

impl DateTimeEditorExt for DateTime {
    fn sanitize(&mut self) {
        self.year = self.year.clamp(DATETIME_MIN_YEAR, DATETIME_MAX_YEAR);
        self.month = self.month.clamp(1, 12);
        self.hour = self.hour.min(23);
        self.minute = self.minute.min(59);
        clamp_day(self);
    }
}

fn clamp_day(date: &mut DateTime) {
    date.day = date.day.clamp(1, DateTime::month_len(date.year, date.month));
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

setting_enum! {
    /// Measurement system for the ride readouts. Re-captions and re-scales the
    /// [`Statistics`](crate::screen) tiles and the off-route distance.
    ///
    /// [`name`](Units::name) is word-bearing, so it routes through the catalog (epic #602); the
    /// symbol captions ([`speed_label`](Units::speed_label) and friends) stay
    /// language-independent.
    pub enum Units {
        /// km / km·h⁻¹ / m — the default.
        Metric = 0, key Msg::UnitsMetric;
        /// mi / mi·h⁻¹ / ft.
        Imperial = 1, key Msg::UnitsImperial;
    }
    default Metric;
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
}

setting_enum! {
    /// How the Climb screen (epic #506) is reached. A device-only setting (the Stats settings screen
    /// cycles it), persisted in the settings codec next to [`ble_enabled`](Settings::ble_enabled).
    ///
    /// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
    /// byte always decodes to the same mode (an unknown byte sanitises to the default, [`Auto`]).
    ///
    /// [`Auto`]: ClimbMode::Auto
    pub enum ClimbMode {
        /// The Climb screen is disabled: it's kept out of the Back-cycle entirely (Map ↔ Statistics
        /// only) and never auto-shown.
        Off = 0, key Msg::ClimbModeOff;
        /// The Climb screen is in the Back-cycle when a climb is active, but the device never switches
        /// to it on its own — the rider reaches it by cycling Back.
        Manual = 1, key Msg::ClimbModeManual;
        /// The Climb screen is in the Back-cycle **and** the device auto-switches to it on climb entry
        /// (from a riding view) and auto-returns to the Map on the crest — the headline behavior.
        Auto = 2, key Msg::ClimbModeAuto;
    }
    /// **Auto** out of the box — the climb panel is self-discovering (it shows itself on the first
    /// climb). Easily changed here if a quieter default is wanted.
    default Auto;
}

impl ClimbMode {
    /// Whether the Climb screen belongs in the Back-cycle at all — false only for [`Off`](ClimbMode::Off).
    #[inline]
    pub const fn is_on(self) -> bool {
        !matches!(self, ClimbMode::Off)
    }
}

setting_enum! {
    /// Whether — and when — the Map's bottom-centre **waypoint chip** (epic #523) is shown: the calm
    /// `◆ NAME  <dist>` pill counting the along-route distance to the next named waypoint ahead. A
    /// device-only setting (the Stats settings screen cycles it), persisted in the codec next to
    /// [`climb_mode`](Settings::climb_mode).
    ///
    /// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
    /// byte always decodes to the same mode (an unknown byte sanitises to the default, [`Approach`]).
    ///
    /// [`Approach`]: WaypointMode::Approach
    pub enum WaypointMode {
        /// The chip is never shown — the silencer for routes carrying junk/artifact waypoints from a
        /// planner's GPX export (a whole route of them can be muted here).
        Off = 0, key Msg::WaypointModeOff;
        /// The chip appears only as the next waypoint nears — within the approach radius
        /// (`WAYPOINT_APPROACH_M`, 500 m) ahead — counting the distance down, so a stop is noticed
        /// without standing chrome. **The default** (discoverability won over the conservative `Off`).
        Approach = 1, key Msg::WaypointModeApproach;
        /// The chip is shown whenever a named waypoint lies ahead (subject to the shared
        /// no-fix / off-route / pan suppression), reading the along-route distance to it.
        Always = 2, key Msg::WaypointModeAlways;
    }
    /// **Approach** out of the box — the calm middle ground: the chip surfaces as a waypoint nears
    /// (so the feature is self-discovering) but stays down the rest of the time. Locked 2026-07-08.
    default Approach;
}

setting_enum! {
    /// Which sources feed the **"Up ahead" timeline** (epic #946, U4) — the ride compass's north
    /// station. A device-only setting, cycled in place by the Ride settings screen's press-to-cycle row
    /// and persisted in the codec next to [`ride_retention`](Settings::ride_retention).
    ///
    /// Scope is deliberately narrow: this is *who feeds the list*, nothing else. It never hides the
    /// map's POI markers or waypoint diamonds, never touches the nearby-POI browser (Menu → POIs, which
    /// answers the other question — "what's near me *now*?"), and never touches the stats waypoint panel
    /// or the "Next: \<category\>" stat fields. It composes with the list's own Hold category picker:
    /// the category filter applies *within* the configured sources.
    ///
    /// The labels are short by necessity: the value shares the Ride row's sub-caption line with its
    /// `◄` cue at 240 px, so the row reads as the sentence *"Up ahead shows ◄ Waypoints"* rather than
    /// repeating "only".
    ///
    /// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
    /// byte always decodes to the same value (an unknown byte sanitises to the default, [`Both`]).
    ///
    /// [`Both`]: UpAheadSource::Both
    pub enum UpAheadSource {
        /// Custom waypoints **and** route-corridor map POIs — the merged timeline the epic designed.
        /// **The default**: the merge is the feature.
        Both = 0, key Msg::UpAheadSourceBoth;
        /// The rider's own GPX waypoints only. The corridor query is never armed under this value, so
        /// the map `Reader` is never built for it either — the list costs exactly what the old waypoint
        /// list cost.
        WaypointsOnly = 1, key Msg::UpAheadSourceWaypoints;
        /// Route-corridor map POIs only. The documented trade: the waypoint plan leaves the ride menu
        /// entirely (it stays on the map and in the stats panel) — for riders who treat a planner's
        /// exported waypoints as clutter.
        MapPoisOnly = 2, key Msg::UpAheadSourceMapPois;
    }
    /// **Both** out of the box — one list answering "what's coming up on my route?" is the whole
    /// point of the timeline; the single-source values are the pressure valves.
    default Both;
}

impl UpAheadSource {
    /// Whether custom route waypoints feed the list under this value.
    #[inline]
    pub const fn shows_waypoints(self) -> bool {
        matches!(self, UpAheadSource::Both | UpAheadSource::WaypointsOnly)
    }

    /// Whether route-corridor map POIs feed the list under this value. Also the **arming** answer:
    /// `false` means no [`CorridorKey`](crate::corridor::CorridorKey) is ever declared, so the query
    /// never runs (see [`UpAheadScreen`](crate::screen::UpAheadScreen)).
    #[inline]
    pub const fn shows_pois(self) -> bool {
        matches!(self, UpAheadSource::Both | UpAheadSource::MapPoisOnly)
    }
}

setting_enum! {
    /// How long the UI sits idle (no user input) before it navigates itself back to where it belongs —
    /// the Home root when not tracking a ride, the Map when a ride is running (see
    /// [`App::apply_idle_return`](crate::App::apply_idle_return)). A device-only setting, cycled by the
    /// Power settings screen's value picker and persisted in the codec next to
    /// [`climb_mode`](Settings::climb_mode).
    ///
    /// `Never` is a word; the durations are unit-glued numbers, catalogued whole so a language can
    /// localize the `s`/`min` grain if it ever needs to.
    ///
    /// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
    /// byte always decodes to the same value (an unknown byte sanitises to the default, [`S30`]).
    ///
    /// [`S30`]: IdleReturn::S30
    pub enum IdleReturn {
        /// 15 seconds.
        S15 = 0, key Msg::IdleS15, Some(15_000);
        /// 30 seconds — the default.
        S30 = 1, key Msg::IdleS30, Some(30_000);
        /// 1 minute.
        M1 = 2, key Msg::IdleM1, Some(60_000);
        /// 5 minutes.
        M5 = 3, key Msg::IdleM5, Some(300_000);
        /// Never — the idle-return mechanism is disabled entirely.
        Never = 4, key Msg::IdleNever, None;
    }
    /// **30 s** out of the box — long enough not to yank an attentive rider mid-glance, short enough
    /// that a device left in a menu drifts back to a useful screen on its own.
    default S30;
    /// The idle timeout in millis, or `None` for [`Never`](IdleReturn::Never) (the mechanism is
    /// off). `None` also disables the idle wake, so a parked device isn't woken to no purpose.
    payload timeout_ms: Option<u32>;
}

setting_enum! {
    /// How often the device asks the phone for a fresh weather bundle (epic #1185 — WX11's settings
    /// entry; the WX8 due scheduler consumes it): Off, or every 15 / 30 / 60 / 120 minutes.
    /// Scheduled requests occur only during an active ride, and opening Weather is urgent regardless
    /// of this interval — both are WX8's lifecycle rules; this is just the persisted knob.
    ///
    /// The discriminants are a **double** on-disk contract: the settings-codec byte *and* the BLE
    /// Config `weather_refresh` field (obc-ble-interface-spec §11.8, `obc_ble::WeatherRefresh`) use
    /// these exact values, so the two stores can never disagree — the board crate's compile-time
    /// asserts pin the mapping variant by variant (`obc-fw-nrf54l/src/object_store.rs`).
    /// Appended, never renumbered; an unknown stored byte sanitises to the default,
    /// [`Every30`](WeatherRefresh::Every30).
    pub enum WeatherRefresh {
        /// No scheduled refresh (opening Weather still requests urgently).
        Off = 0, key Msg::WeatherRefreshOff, None;
        /// Every 15 minutes.
        Every15 = 1, key Msg::WeatherRefreshM15, Some(15);
        /// Every 30 minutes — the default (the epic's locked default interval).
        Every30 = 2, key Msg::WeatherRefreshM30, Some(30);
        /// Every 60 minutes.
        Every60 = 3, key Msg::WeatherRefreshM60, Some(60);
        /// Every 120 minutes.
        Every120 = 4, key Msg::WeatherRefreshM120, Some(120);
    }
    default Every30;
    /// The scheduled interval in minutes, or `None` for [`Off`](WeatherRefresh::Off) — the shape
    /// the WX8 scheduler consumes (mirrors `obc_ble::WeatherRefresh::minutes`).
    payload minutes: Option<u16>;
}

setting_enum! {
    /// The UI language (epic #602). A device-only setting, cycled by the Language settings screen's
    /// value picker and persisted in the codec next to [`waypoint_mode`](Settings::waypoint_mode).
    ///
    /// Each value's [`name`](Language::name) is its **endonym** (its own name for itself), so the
    /// picker row reads to a speaker who can't yet read the current UI language — which is also why
    /// this is the one settings enum whose labels are literals rather than catalog keys. The
    /// accented forms (`Français` / `Español`) render via the Latin font extension (#601).
    ///
    /// [`COUNT`](Language::COUNT) is the number of columns the i18n catalog must ship: a static
    /// assertion in [`i18n`](crate::i18n) ties `TABLE`'s column count to it, so the "index never
    /// panics" contract of [`t`](crate::i18n::t) is compiler-enforced — a fifth variant added
    /// without a fifth `{lang}.toml` column fails the build instead of panicking on the first draw
    /// (#614). Because the picker only ever selects out of [`ALL`](Language::ALL),
    /// [`Settings::language`](crate::Settings::language) is always in range.
    ///
    /// The discriminants are a **stable on-disk contract** — appended, never renumbered — so a stored
    /// byte always decodes to the same language (an unknown byte sanitises to the default, [`En`]).
    ///
    /// [`En`]: Language::En
    pub enum Language {
        /// English — the default.
        En = 0, text "English";
        /// German.
        De = 1, text "Deutsch";
        /// French.
        Fr = 2, text "Français";
        /// Spanish.
        Es = 3, text "Español";
    }
    /// **English** out of the box — the language every string is authored in; the other three are
    /// opt-in once the catalog lands.
    default En;
}

/// UTC-offset stepper bounds + granularity (minutes). 15-minute steps cover the real-world
/// `:30` / `:45` zones (India +5:30, Nepal +5:45) over the −12:00…+14:00 span.
pub const UTC_OFFSET_MIN: i16 = -12 * 60;
pub const UTC_OFFSET_MAX: i16 = 14 * 60;
pub const UTC_OFFSET_STEP: i16 = 15;

/// GPS-fix-interval stepper bounds (seconds). The step itself *adapts* (1 s up to 10 s, then
/// 5 s) — see [`PowerScreen`](crate::screen) — so a long interval is a few steps, not dozens.
pub const FIX_INTERVAL_MIN: u16 = 1;
pub const FIX_INTERVAL_MAX: u16 = 120;

/// Stats-grid page auto-cycle period stepper bounds (seconds). With the elevation chart keeping
/// Up/Down and Select-`hold` for itself, a second page is only reachable by the auto-cycle — so there's no "off",
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
    /// The last time source's **UTC** set-point — the anchor a GPS fix (or, after epic #638 S2, a
    /// BLE `setClock`) stamps. Manual editing was removed in #641: the only writers are those two
    /// trusted sources, so this is always UTC and [`local_clock`](Settings::local_clock) always
    /// folds in [`utc_offset_min`](Settings::utc_offset_min). Persisted so it seeds the boot display
    /// clock — display-only until re-stamped this boot (see
    /// [`App::clock_trusted`](crate::App::clock_trusted)).
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
    /// Draw the map's **terrain layer** — today the E3 contour lines (the Display settings screen's
    /// toggle). **Device-only**, like [`map_clock`](Settings::map_clock): deliberately *not* one of
    /// the BLE-writable fields [`adopt_ble_fields`](Settings::adopt_ble_fields) pulls across.
    /// Default **on** — the point of the setting is to *see* contours without hunting for a switch.
    /// Off drops every terrain-layer style from the renderer's collect pass (the Map screen
    /// restates this switch as `RenderConfig::terrain_layer` each frame), so the geometry is never
    /// decoded.
    ///
    /// **It hides the ink, not the bytes and not the I/O.** Contours are interleaved with everything
    /// else in the same `mid`/`fine` cells, so switching them off does not shrink the map on the card
    /// and does not avoid a single chunk read. The #1088 measurement — riding-zoom chunk reads
    /// roughly doubling, 5.9 → 10.9 kB per frame, ≈ +11 ms per uncached frame on the ~460 kB/s SD
    /// path — is a cost of *packing* contours, not of drawing them, and this toggle does not recover
    /// it. "Off" is not a performance control.
    ///
    /// **Provisional (#1096).** Contours are a judgement call no mockup settles, so this switch
    /// exists only so the #1097 ride review can put both states on the same glass on the same ride.
    /// It is **expected to be removed**: if contours win the toggle goes and they are simply on; if
    /// they lose the whole feature goes. Built as the cheapest honest switch, not a settled
    /// preference — don't grow migration concerns around it.
    pub map_contours: bool,
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
    /// How long after a ride is verifiably synced to the phone the device auto-deletes it (epic
    /// #638): Never / 1 day / 1 week / 1 month. **Device-only**, like
    /// [`climb_mode`](Settings::climb_mode) — the auto-expiry setting is device-local (the app never
    /// surfaces it), so [`adopt_ble_fields`](Settings::adopt_ble_fields) never pulls it across.
    /// Default **1 week**. Only synced rides are ever deleted; unsynced rides are never touched. S5
    /// adds the Auto-delete settings screen that edits this.
    pub ride_retention: RideRetention,
    /// Which sources feed the **"Up ahead" timeline** (epic #946, U4, the Ride settings screen
    /// cycles it): both, custom waypoints only, or map POIs only. **Device-only**, like
    /// [`climb_mode`](Settings::climb_mode) — a phone must never repick what the rider's ride menu
    /// shows, so [`adopt_ble_fields`](Settings::adopt_ble_fields) never pulls it across. Default
    /// **Both**; the scope is the Up-ahead list *only* (see [`UpAheadSource`]).
    pub up_ahead_source: UpAheadSource,
    /// How often the device raises a **scheduled weather request** (weather epic #1185: WX8
    /// #1193's due scheduler consumes it, WX11 #1196's Weather settings screen edits it) — the
    /// typed [`WeatherRefresh`] whose discriminants ARE the BLE §11.8 wire bytes (pinned by
    /// test). Division of labour (the #1221/#1224 merge resolution): the wire crate (`obc-ble`'s
    /// own `WeatherRefresh`) owns the vocabulary and the direction-dependent validation rule at
    /// the radio boundary — the board's Config write path validates there first and converts via
    /// [`WeatherRefresh::from_byte`] — while this enum is `obc-app`'s typed representation the
    /// settings screen cycles ([`stepped`](WeatherRefresh::stepped)) and the scheduler reads
    /// ([`minutes`](WeatherRefresh::minutes)). [`decode`] sanitises an out-of-range stored byte
    /// to the default — deliberately **30 min and not `Off`**: §7.3 pins that only an explicit
    /// rider choice may disable weather. **BLE-writable** (like [`units`](Settings::units) /
    /// [`device_name`](Settings::device_name)): the companion writes it via Config §7.3, so
    /// [`adopt_ble_fields`](Settings::adopt_ble_fields) pulls it across.
    pub weather_refresh: WeatherRefresh,
    /// The last **fired weather alert** per class (WX12 #1197): the dedup/cooldown anchors —
    /// event onset + position + severity, indexed by
    /// [`AlertClass::slot`](crate::weather_alerts::AlertClass). Persisted in this blob (the RRAM
    /// carve / sim file) so the same storm does not pop back up on the next *boot*, not just the
    /// next frame; dedup compares event times, so it needs no trusted clock at boot. Written by
    /// [`App::weather_alert_tick`](crate::App::weather_alert_tick) at alert-fire rate (rare),
    /// through the same #810 persistence handshake as any rider edit. **Device-only** state, not
    /// a preference: [`adopt_ble_fields`](Settings::adopt_ble_fields) never touches it.
    pub weather_alert_marks: crate::weather_alerts::AlertMarks,
}

/// The in-memory footprint, pinned. [`Settings`] is copied whole (the live `App` copy, the board's
/// Config cache, the `.rodata` [`DEFAULT`](Settings::DEFAULT) image), so a field that silently
/// widens the struct widens every one of those — this makes the growth an explicit decision.
const _: () = assert!(core::mem::size_of::<Settings>() == 184, "Settings grew — was that deliberate?");

impl Default for Settings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Settings {
    /// The factory settings as a **`const`** — the same value [`Default`] returns (`default()`
    /// delegates here, and the per-field defaults the literal names are pinned against each
    /// type's own `Default` by test, so the two cannot drift). Exists so the board's ~13.6 KB object store can be built from a `.rodata` image
    /// instead of a stack temporary: WX12 (#1197) grew this struct by 96 B and the optimizer
    /// stopped collapsing the store's two-copy construction — the exact transient boot spike the
    /// 2026-08-03 STKOF post-mortem banned, caught by the boot-chain guard. A future field must
    /// be const-constructible here, which is a compile error rather than a convention.
    pub const DEFAULT: Settings = Settings {
        units: Units::Metric,
        clock: DateTime::DEFAULT,
        utc_offset_min: 0,
        fix_interval_s: 1,
        power_saver: false,
        stat_fields: StatFieldList::DEFAULT,
        stat_cycle_s: STAT_CYCLE_DEFAULT,
        device_name: DeviceName::EMPTY,
        ble_enabled: true,
        climb_mode: ClimbMode::Auto,
        idle_return: IdleReturn::S30,
        map_clock: true,
        map_scale_bar: true,
        map_contours: true,
        bike_profile_idx: 0,
        waypoint_mode: WaypointMode::Approach,
        language: Language::En,
        saved_sensors: [SavedSensor::EMPTY; SENSOR_SLOTS],
        ride_retention: RideRetention::Week1,
        up_ahead_source: UpAheadSource::Both,
        weather_refresh: WeatherRefresh::Every30,
        weather_alert_marks: [None; crate::weather_alerts::ALERT_CLASSES],
    };
}

impl Settings {
    /// The **local** wall-clock set-point the device shows: the UTC [`clock`](Settings::clock)
    /// anchor shifted into local time by [`utc_offset_min`](Settings::utc_offset_min) (via a
    /// calendar offset operation, so a shift across midnight rolls the date too). Manual editing was
    /// removed in #641, so the anchor is always UTC and the offset always applies.
    pub fn local_clock(&self) -> DateTime {
        with_offset_bounded(self.clock, self.utc_offset_min)
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
        // WX8 (#1193): the §7.3 `weather_refresh` field is the third BLE-writable field. Pulled
        // across for the same clobber reason as the two above — an on-device edit's whole-blob save
        // must not overwrite the interval the phone just set.
        self.weather_refresh = other.weather_refresh;
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
/// (BLE-sensors SE7, #714) — 3 slots × 8 B (`present · addr_kind · addr[6]`); v12 appended the
/// `ride_retention` byte (auto-expiry epic #638, S3); v13 appended the `up_ahead_source` byte
/// ("Up ahead" epic #946, U4); v14 appended the `map_contours` byte (elevation EL10c, #1096 — a
/// **provisional** field; removing it is a version bump like any other, which is exactly the point
/// of it costing one appended byte); v15 appended the `weather_refresh` byte (weather epic #1185
/// — WX8 #1193's scheduler field and WX11 #1196's settings screen, one byte carrying the BLE
/// §11.8 discriminants); v16 appended the 54-byte `weather_alert_marks` block (WX12 #1197) —
/// 3 classes × 18 B (`flags · onset i64 · lat i32 · lon i32 · severity`), the persisted alert
/// dedup/cooldown anchors.
pub const VERSION: u8 = 16;

/// Fixed encoded length: the [`PAYLOAD_LEN`] CRC-covered bytes + a 2-byte CRC, **rounded up to the
/// device RRAM's 16-byte write line** (the firmware store writes whole 128-bit lines) — so a codec
/// bump never needs the device store re-padded, the RRAM store reads a known span, and the file
/// store needs no length framing. Bytes past the CRC are unused zero padding.
pub const ENCODED_LEN: usize = (PAYLOAD_LEN + 2).div_ceil(16) * 16;

/// Payload size before the trailing CRC. The CRC follows immediately at this offset.
const PAYLOAD_LEN: usize = ALERT_MARKS_OFF + crate::weather_alerts::ALERT_CLASSES * ALERT_MARK_LEN;
/// Byte offset of the `weather_alert_marks` block (the v16 tail, right after `weather_refresh`).
const ALERT_MARKS_OFF: usize = WEATHER_REFRESH_OFF + 1;
/// Bytes per persisted alert mark: `flags(1) · onset i64 LE(8) · lat i32 LE(4) · lon i32 LE(4)
/// · severity(1)`. The leading byte is a flag *pair*, not a bool: bit 0 = the slot holds a mark,
/// bit 1 = that mark has a position. Position presence has to survive the write — a mark fired
/// before the first GPS fix has no coordinate, and dedup must compare it by time alone rather
/// than by ground distance to a fabricated `(0, 0)` (see [`crate::weather_alerts::same_event`]).
/// Both bits fit the byte that was already there, so the record and the blob keep their size.
const ALERT_MARK_LEN: usize = 18;
/// `flags` bit 0: this slot holds a mark at all.
const ALERT_MARK_PRESENT: u8 = 1 << 0;
/// `flags` bit 1: the stored `lat`/`lon` are a real position (else the mark has none).
const ALERT_MARK_HAS_POS: u8 = 1 << 1;
/// Byte offset of the `weather_refresh` byte (the v15 tail, right after `map_contours`).
const WEATHER_REFRESH_OFF: usize = CONTOURS_OFF + 1;
/// Byte offset of the `map_contours` flag (the v14 tail, right after `up_ahead_source`).
const CONTOURS_OFF: usize = UP_AHEAD_OFF + 1;
/// Byte offset of the `up_ahead_source` byte (the v13 tail, right after `ride_retention`).
const UP_AHEAD_OFF: usize = RIDE_RET_OFF + 1;
/// Byte offset of the `ride_retention` byte (the v12 tail, right after the `saved_sensors` block).
const RIDE_RET_OFF: usize = SENSORS_OFF + SENSOR_SLOTS * SAVED_SENSOR_LEN;
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

/// Pack [`Settings`] into its fixed [`ENCODED_LEN`]-byte blob: a version byte, the little-endian
/// fields, then a trailing CRC. The inverse of [`decode`]; shared verbatim by the sim file store
/// and the device RRAM store so one round-trip test covers both.
pub fn encode(s: &Settings) -> [u8; ENCODED_LEN] {
    let mut b = [0u8; ENCODED_LEN];
    b[0] = VERSION;
    b[1] = s.units as u8;
    // Byte 2 was the `gps_time` flag (removed #641). Its offset is frozen so v11 blobs keep their
    // layout — written as a constant `0` and ignored on decode. (Repurpose it, don't reorder, if a
    // future field wants a byte here.)
    b[2] = 0;
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
    // v12 tail: the ride-retention (auto-delete) setting.
    b[RIDE_RET_OFF] = s.ride_retention.as_u8();
    // v13 tail: which sources feed the "Up ahead" timeline.
    b[UP_AHEAD_OFF] = s.up_ahead_source as u8;
    // v14 tail: the Map's terrain-layer (contours) toggle.
    b[CONTOURS_OFF] = s.map_contours as u8;
    // v15 tail: the weather-refresh interval — the enum discriminant IS the §11.8 wire byte.
    b[WEATHER_REFRESH_OFF] = s.weather_refresh as u8;
    // v16 tail: the per-class weather-alert marks (WX12) — `flags · onset · lat · lon · severity`.
    for (slot, mark) in s.weather_alert_marks.iter().enumerate() {
        let off = ALERT_MARKS_OFF + slot * ALERT_MARK_LEN;
        if let Some(mark) = mark {
            b[off] = ALERT_MARK_PRESENT;
            b[off + 1..off + 9].copy_from_slice(&mark.onset.to_le_bytes());
            if let Some((lat, lon)) = mark.pos {
                b[off] |= ALERT_MARK_HAS_POS;
                b[off + 9..off + 13].copy_from_slice(&lat.to_le_bytes());
                b[off + 13..off + 17].copy_from_slice(&lon.to_le_bytes());
            }
            b[off + 17] = mark.severity;
        }
    }
    let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
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
    if crc != crate::store_meta::crc16(&b[0..PAYLOAD_LEN]) {
        return None;
    }
    let mut s = Settings {
        units: Units::from_byte(b[1]),
        // Byte 2 (the retired `gps_time` flag, #641) is ignored — old blobs decode cleanly.
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
        // The v12 ride-retention (auto-delete) setting: an unknown byte sanitises to the default
        // (1 week), like the other enum codec fields.
        ride_retention: RideRetention::from_u8(b[RIDE_RET_OFF]),
        // The v13 Up-ahead source scope: an unknown byte sanitises to the default (Both), like the
        // other enum codec fields.
        up_ahead_source: UpAheadSource::from_byte(b[UP_AHEAD_OFF]),
        // The v14 terrain-layer (contours) toggle: any non-zero byte is "on", like the other bools.
        map_contours: b[CONTOURS_OFF] != 0,
        // The v15 weather-refresh interval: writers only ever store a validated §11.8
        // discriminant, so an out-of-range byte here is corruption the CRC missed — sanitise to
        // the device default, like the other enum codec fields. Deliberately the default (30 min)
        // and not `Off`: §7.3 pins that only an explicit rider choice may disable weather.
        weather_refresh: WeatherRefresh::from_byte(b[WEATHER_REFRESH_OFF]),
        // The v16 weather-alert marks (WX12): an absent slot (`present == 0`) is "no alert fired".
        // Every stored value is a legal mark (any onset/position/severity is comparable), so no
        // range clamp exists to apply.
        weather_alert_marks: decode_alert_marks(b),
    };
    s.sanitize();
    Some(s)
}

/// Decode the v16 weather-alert-mark block: 3 slots × `flags(1) · onset(8) · lat(4) · lon(4) ·
/// severity(1)`. An absent slot ([`ALERT_MARK_PRESENT`] clear) reads as `None` regardless of the
/// stored payload; a present slot without [`ALERT_MARK_HAS_POS`] keeps its position *absent*
/// rather than reading the zeroed coordinate bytes as null island.
fn decode_alert_marks(b: &[u8]) -> crate::weather_alerts::AlertMarks {
    let mut marks: crate::weather_alerts::AlertMarks = [None; crate::weather_alerts::ALERT_CLASSES];
    for (slot, mark) in marks.iter_mut().enumerate() {
        let off = ALERT_MARKS_OFF + slot * ALERT_MARK_LEN;
        if b[off] & ALERT_MARK_PRESENT != 0 {
            let pos = (b[off] & ALERT_MARK_HAS_POS != 0).then(|| {
                (
                    i32::from_le_bytes(b[off + 9..off + 13].try_into().unwrap()),
                    i32::from_le_bytes(b[off + 13..off + 17].try_into().unwrap()),
                )
            });
            *mark = Some(crate::weather_alerts::AlertMark {
                onset: i64::from_le_bytes(b[off + 1..off + 9].try_into().unwrap()),
                pos,
                severity: b[off + 17],
            });
        }
    }
    marks
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `Settings::DEFAULT` (the const the board's `.rodata` store image is built from, #1197)
    /// names every per-type default variant literally — pin each against its type's own
    /// `Default`, so a retuned default can't silently fork the two. **Every** field whose type
    /// has its own `Default` belongs here, including the ones the const spells as a plain literal
    /// (`Units`, `DeviceName`, `SavedSensor`): those are exactly the ones that fork silently.
    #[test]
    fn const_default_matches_every_field_default() {
        let d = Settings::DEFAULT;
        assert_eq!(d.units, Units::default());
        assert_eq!(d.clock, DateTime::default());
        assert_eq!(d.stat_fields, StatFieldList::default());
        assert_eq!(d.device_name, DeviceName::default());
        assert_eq!(d.climb_mode, ClimbMode::default());
        assert_eq!(d.idle_return, IdleReturn::default());
        assert_eq!(d.waypoint_mode, WaypointMode::default());
        assert_eq!(d.language, Language::default());
        assert_eq!(d.saved_sensors, [SavedSensor::default(); SENSOR_SLOTS]);
        assert_eq!(d.ride_retention, RideRetention::default());
        assert_eq!(d.up_ahead_source, UpAheadSource::default());
        assert_eq!(d.weather_refresh, WeatherRefresh::default());
        // And the whole const is its type's `Default` — the property the field list guards.
        assert_eq!(d, Settings::default());
    }

    /// A settings value with **every** field pushed off its default — including a customised,
    /// reordered stat-field selection with a two-span tile. Shared by the round-trip test and the
    /// golden blobs below, so the two always speak about the same bytes.
    fn every_field_set() -> Settings {
        let mut stat_fields = StatFieldList::default();
        stat_fields.remove(0); // drop a default tile…
        assert!(stat_fields.push(crate::stat_fields::StatField::Clock)); // …and pin the wide clock
        Settings {
            units: Units::Imperial,
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
            map_contours: false,
            bike_profile_idx: 3,
            waypoint_mode: WaypointMode::Always,
            language: Language::De,
            saved_sensors: [
                SavedSensor::saved(1, [1, 2, 3, 4, 5, 6]),
                SavedSensor::EMPTY,
                SavedSensor::saved(0, [6, 5, 4, 3, 2, 1]),
            ],
            ride_retention: RideRetention::Month1,
            up_ahead_source: UpAheadSource::MapPoisOnly,
            weather_refresh: WeatherRefresh::Every120,
            weather_alert_marks: [
                Some(crate::weather_alerts::AlertMark {
                    onset: 1_800_000_900,
                    pos: Some((47_123_456, 8_654_321)),
                    severity: 11,
                }),
                None,
                Some(crate::weather_alerts::AlertMark { onset: -1, pos: Some((-47_000_000, -8_000_000)), severity: 0 }),
            ],
        }
    }

    /// A non-default settings value — including a customised, reordered field selection with a
    /// two-span tile — round-trips through the codec byte-for-byte.
    #[test]
    fn codec_round_trips() {
        let s = every_field_set();
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    /// The golden blobs: `encode` writes exactly the bytes it wrote before the `setting_enum!`
    /// table replaced the hand-written enum kits (#1466) — for [`Settings::DEFAULT`] and for a
    /// value with every field set. Each value enum stores its **declared** discriminant, so a
    /// renumbered variant moves a byte here: this is the codec-side twin of the macro's
    /// compile-time discriminant asserts, and the pin the codec/table slices inherit.
    #[test]
    fn encode_matches_the_golden_blobs() {
        const DEFAULT_BLOB: [u8; ENCODED_LEN] = [
            16, 0, 0, 233, 7, 1, 1, 12, 0, 0, 0, 1, 0, 0, 6, 0, 1, 2, 3, 4, 5, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 1, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 2, 0, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 212, 126, 0, 0, 0, 0, 0, 0,
        ];
        const FULL_BLOB: [u8; ENCODED_LEN] = [
            16, 1, 0, 234, 7, 6, 29, 14, 40, 120, 0, 5, 0, 1, 6, 1, 2, 3, 4, 5, 9, 0, 0, 0, 0, 0, 0, 8, 0, 10, 84, 105,
            109, 111, 39, 115, 32, 79, 66, 67, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 3, 0, 0, 3, 2, 1, 1, 1, 1, 2, 3, 4, 5, 6, 0, 0, 0, 0, 0, 0,
            0, 0, 1, 0, 6, 5, 4, 3, 2, 1, 3, 2, 0, 4, 3, 132, 213, 73, 107, 0, 0, 0, 0, 0, 12, 207, 2, 241, 13, 132, 0,
            11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 255, 255, 255, 255, 255, 255, 255, 255, 64,
            214, 50, 253, 0, 238, 133, 255, 0, 2, 122, 0, 0, 0, 0, 0, 0,
        ];

        assert_eq!(encode(&Settings::DEFAULT), DEFAULT_BLOB, "the default blob is frozen");
        assert_eq!(encode(&every_field_set()), FULL_BLOB, "the fully-populated blob is frozen");
        assert_eq!(decode(&DEFAULT_BLOB), Some(Settings::DEFAULT), "…and both decode back");
        assert_eq!(decode(&FULL_BLOB), Some(every_field_set()));
    }

    /// One table over all seven `setting_enum!` types: every declared byte round-trips through
    /// `from_byte` at its table position, and any byte past the last discriminant clamps to the
    /// default. The macro is one implementation — tested once, here, rather than seven times.
    #[test]
    fn every_setting_enum_round_trips_its_bytes_and_clamps_the_rest() {
        macro_rules! check {
            ($T:ty) => {{
                for (i, v) in <$T>::ALL.iter().enumerate() {
                    assert_eq!(*v as u8, i as u8, "{}::{:?} stores byte {i}", stringify!($T), v);
                    assert_eq!(<$T>::from_byte(i as u8), *v, "{} byte {i} decodes back", stringify!($T));
                }
                for b in [<$T>::COUNT as u8, u8::MAX] {
                    assert_eq!(<$T>::from_byte(b), <$T>::default(), "{} clamps {b} to its default", stringify!($T));
                }
            }};
        }
        check!(Units);
        check!(ClimbMode);
        check!(WaypointMode);
        check!(UpAheadSource);
        check!(IdleReturn);
        check!(WeatherRefresh);
        check!(Language);
    }

    /// The v16 tail (WX12 #1197): the per-class weather-alert marks round-trip (present and
    /// absent slots, negative onsets/coordinates — asserted field-precisely on top of
    /// `codec_round_trips`' whole-struct pass), an absent slot stores all-zeros gated by the
    /// `flags` byte, a **positionless** mark survives as positionless (never as null island), and
    /// the block is **device-local state** — a BLE Config adopt never clobbers it.
    #[test]
    fn weather_alert_marks_round_trip_and_are_device_only() {
        use crate::weather_alerts::AlertMark;
        let mark = AlertMark { onset: 1_800_123_456, pos: Some((-12_345, 9_876_543)), severity: 7 };
        let s = Settings { weather_alert_marks: [None, Some(mark), None], ..Settings::default() };
        let b = encode(&s);
        assert_eq!(decode(&b), Some(s), "marks round-trip through the v16 tail");
        assert_eq!(b[ALERT_MARKS_OFF], 0, "slot 0 absent");
        assert_eq!(
            b[ALERT_MARKS_OFF + ALERT_MARK_LEN],
            ALERT_MARK_PRESENT | ALERT_MARK_HAS_POS,
            "slot 1 present, with a position"
        );

        // A mark fired before the first GPS fix: present, positionless. The zeroed coordinate
        // bytes must decode back to `None`, not to `(0, 0)` — that fabricated place is exactly
        // what would re-fire the same storm once the receiver locks.
        let blind = AlertMark { onset: 1_800_123_456, pos: None, severity: 7 };
        let s = Settings { weather_alert_marks: [Some(blind), None, None], ..Settings::default() };
        let b = encode(&s);
        assert_eq!(b[ALERT_MARKS_OFF], ALERT_MARK_PRESENT, "present, no position bit");
        assert_eq!(&b[ALERT_MARKS_OFF + 9..ALERT_MARKS_OFF + 17], &[0; 8], "no coordinate is written");
        assert_eq!(decode(&b).unwrap().weather_alert_marks[0], Some(blind), "and it decodes back positionless");

        // Adopting a BLE settings write (units/name/refresh) must not clear the local marks.
        let mut device = Settings { weather_alert_marks: [Some(mark), None, None], ..Settings::default() };
        let phone = Settings { units: Units::Imperial, ..Settings::default() }; // marks all None
        device.adopt_ble_fields(&phone);
        assert_eq!(device.units, Units::Imperial);
        assert_eq!(device.weather_alert_marks[0], Some(mark), "marks are device state, never adopted away");
    }

    /// The v15 tail (weather epic #1185 — WX8 #1193 + WX11 #1196, merged): the typed
    /// weather-refresh interval round-trips every value on its §11.8 wire discriminant, defaults
    /// to **30 min** (never `Off` — §7.3 pins that only an explicit rider choice disables
    /// weather), sanitises an out-of-range stored byte back to the default, walks its picker in
    /// wire order with wrap, and is **BLE-writable**: [`adopt_ble_fields`] pulls it across so an
    /// on-device edit's whole-blob save can't clobber the interval the phone just wrote.
    #[test]
    fn weather_refresh_round_trips_defaults_and_is_ble_writable() {
        assert_eq!(Settings::default().weather_refresh, WeatherRefresh::Every30, "default = 30 min, not Off");
        assert_eq!(ENCODED_LEN, 176, "the settings blob is 176 B / 11 RRAM lines (v16 weather_alert_marks tail)");

        // The stored byte IS the BLE Config §11.8 wire value — the double contract, pinned.
        for (r, wire, minutes) in [
            (WeatherRefresh::Off, 0u8, None),
            (WeatherRefresh::Every15, 1, Some(15u16)),
            (WeatherRefresh::Every30, 2, Some(30)),
            (WeatherRefresh::Every60, 3, Some(60)),
            (WeatherRefresh::Every120, 4, Some(120)),
        ] {
            assert_eq!(r as u8, wire, "{r:?} carries the §11.8 wire discriminant");
            assert_eq!(WeatherRefresh::from_byte(wire), r);
            assert_eq!(r.minutes(), minutes);
            let s = Settings { weather_refresh: r, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{r:?} round-trips");
            assert_eq!(encode(&s)[WEATHER_REFRESH_OFF], wire, "{r:?} stores the wire byte verbatim");
        }

        // An out-of-range stored byte (corruption the CRC missed) sanitises to the default —
        // re-stamp the CRC so only the payload byte is "wrong".
        let mut b = encode(&Settings { weather_refresh: WeatherRefresh::Off, ..Settings::default() });
        b[WEATHER_REFRESH_OFF] = 9;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&b).expect("valid CRC").weather_refresh, WeatherRefresh::Every30, "unknown → default");

        // The picker walks Off → 15 → 30 → 60 → 120 and wraps at both ends (the settings screen).
        assert_eq!(WeatherRefresh::Off.stepped(1), WeatherRefresh::Every15);
        assert_eq!(WeatherRefresh::Every120.stepped(1), WeatherRefresh::Off, "wraps past 120");
        assert_eq!(WeatherRefresh::Off.stepped(-1), WeatherRefresh::Every120, "wraps past Off");

        // BLE-writable: the #456 coherence merge adopts it (unlike the device-only fields).
        let mut app = Settings { weather_refresh: WeatherRefresh::Off, ..Settings::default() };
        app.adopt_ble_fields(&Settings { weather_refresh: WeatherRefresh::Every60, ..Settings::default() });
        assert_eq!(app.weather_refresh, WeatherRefresh::Every60, "adopt_ble_fields pulls the phone's interval across");
    }

    /// The v13 tail: the Up-ahead source scope round-trips every value, defaults **Both** (the
    /// merged timeline is the feature), sanitises an unknown byte back to Both, and is device-only
    /// — [`adopt_ble_fields`] must never pull it across (a phone can't repick what the rider's ride
    /// menu shows).
    #[test]
    fn up_ahead_source_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().up_ahead_source, UpAheadSource::Both, "the timeline defaults to Both");
        // The one new byte still fits inside the same 16-byte RRAM line rounding as v12.
        assert_eq!(ENCODED_LEN, 176, "the settings blob is 176 B / 11 RRAM lines (v16 weather_alert_marks tail)");

        for src in [UpAheadSource::Both, UpAheadSource::WaypointsOnly, UpAheadSource::MapPoisOnly] {
            let s = Settings { up_ahead_source: src, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{src:?} round-trips");
        }

        // An out-of-range stored byte (a newer writer, a bit-flip the CRC missed) sanitises to Both
        // — re-stamp the CRC so only the payload byte is "wrong".
        let mut b = encode(&Settings { up_ahead_source: UpAheadSource::MapPoisOnly, ..Settings::default() });
        b[UP_AHEAD_OFF] = 9;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&b).expect("valid CRC").up_ahead_source, UpAheadSource::Both, "unknown → default");

        // Device-only: a BLE blob's up_ahead_source never lands via the #456 coherence merge.
        let mut app = Settings { up_ahead_source: UpAheadSource::WaypointsOnly, ..Settings::default() };
        app.adopt_ble_fields(&Settings { up_ahead_source: UpAheadSource::MapPoisOnly, ..Settings::default() });
        assert_eq!(app.up_ahead_source, UpAheadSource::WaypointsOnly, "adopt_ble_fields leaves it alone");
    }

    /// The three values and the source predicates that drive both the list composition and the
    /// corridor arming: the ring cycles Both → Waypoints → Map POIs → Both, and exactly one value
    /// asks for **no** corridor query at all.
    #[test]
    fn up_ahead_source_cycles_and_scopes_the_two_sources() {
        assert_eq!(UpAheadSource::Both.cycled(), UpAheadSource::WaypointsOnly);
        assert_eq!(UpAheadSource::WaypointsOnly.cycled(), UpAheadSource::MapPoisOnly);
        assert_eq!(UpAheadSource::MapPoisOnly.cycled(), UpAheadSource::Both, "the ring wraps");

        assert!(UpAheadSource::Both.shows_waypoints() && UpAheadSource::Both.shows_pois());
        assert!(UpAheadSource::WaypointsOnly.shows_waypoints() && !UpAheadSource::WaypointsOnly.shows_pois());
        assert!(!UpAheadSource::MapPoisOnly.shows_waypoints() && UpAheadSource::MapPoisOnly.shows_pois());
    }

    /// A v11 blob written by a pre-#641 firmware carried the `gps_time` flag in byte 2. That byte's
    /// offset is now frozen and ignored, so an old blob with the flag **set** still decodes cleanly
    /// to the same `Settings` — no field shifts, no version bump, nothing surprises a decode.
    #[test]
    fn old_gps_time_byte_is_ignored_on_decode() {
        let s = Settings {
            clock: DateTime { year: 2026, month: 7, day: 14, hour: 9, minute: 5 },
            utc_offset_min: 60,
            ..Settings::default()
        };
        // Encode (byte 2 == 0 today), then forge the retired flag on and re-CRC to mimic an old blob.
        let mut old = encode(&s);
        old[2] = 1;
        let crc = crate::store_meta::crc16(&old[0..PAYLOAD_LEN]);
        old[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&old), Some(s), "the retired gps_time byte doesn't affect the decoded value");
    }

    /// The v12 tail: the ride-retention (auto-delete) setting round-trips, defaults **1 week**,
    /// sanitises an unknown byte to the default, and stays device-only ([`adopt_ble_fields`] never
    /// pulls it across — the auto-expiry setting is device-local).
    #[test]
    fn ride_retention_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().ride_retention, RideRetention::Week1, "the ride-retention default is 1 week");

        for r in [RideRetention::Never, RideRetention::Day1, RideRetention::Week1, RideRetention::Month1] {
            let s = Settings { ride_retention: r, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{r:?} round-trips");
        }

        // An out-of-range stored byte (a newer writer, a bit-flip the CRC missed) sanitises to the
        // default 1 week — re-stamp the CRC so only the payload byte is "wrong".
        let mut b = encode(&Settings { ride_retention: RideRetention::Never, ..Settings::default() });
        b[RIDE_RET_OFF] = 200;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&b).expect("valid CRC").ride_retention, RideRetention::Week1, "unknown → default");

        // Device-only: a BLE blob's ride_retention never lands via the #456 coherence merge.
        let mut app = Settings { ride_retention: RideRetention::Never, ..Settings::default() };
        app.adopt_ble_fields(&Settings { ride_retention: RideRetention::Month1, ..Settings::default() });
        assert_eq!(app.ride_retention, RideRetention::Never, "adopt_ble_fields leaves ride_retention alone");
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
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
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
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
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
        assert_eq!(ENCODED_LEN, 176, "the settings blob is 176 B / 11 RRAM lines (v16 weather_alert_marks tail)");

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

    /// The v14 tail (elevation EL10c, #1096): the Map's terrain-layer (contours) toggle round-trips,
    /// defaults **on** — the point of the switch is to see contours without hunting for it — and is
    /// device-only, like every other Display-screen toggle: [`adopt_ble_fields`] must never pull it
    /// across. **Provisional**: #1097 decides whether this field survives at all.
    #[test]
    fn map_contours_round_trips_and_is_device_only() {
        assert!(Settings::default().map_contours, "contours default on");
        // The one new byte still fits inside the same 16-byte RRAM line rounding as v13.
        assert_eq!(ENCODED_LEN, 176, "the settings blob is 176 B / 11 RRAM lines (v16 weather_alert_marks tail)");

        for on in [false, true] {
            let s = Settings { map_contours: on, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "map_contours={on} round-trips");
        }

        // Any non-zero stored byte is "on", like the other bool fields — re-stamp the CRC so only the
        // payload byte is unusual.
        let mut b = encode(&Settings { map_contours: false, ..Settings::default() });
        b[CONTOURS_OFF] = 7;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert!(decode(&b).expect("valid CRC").map_contours, "any non-zero byte reads as on");

        // Device-only: a BLE blob's contour toggle never lands via the #456 coherence merge.
        let mut app = Settings { map_contours: false, ..Settings::default() };
        app.adopt_ble_fields(&Settings { map_contours: true, ..Settings::default() });
        assert!(!app.map_contours, "adopt_ble_fields leaves the contour toggle alone");
    }

    /// The v8 tail: the routing-profile index round-trips every value, defaults **0**, is stored
    /// **verbatim** (never range-clamped on decode — an out-of-range index is a live-map concern, not
    /// a codec one), and is device-only — [`adopt_ble_fields`] must never pull it across (a phone
    /// can't repick the rider's bike type).
    #[test]
    fn bike_profile_idx_round_trips_and_is_device_only() {
        assert_eq!(Settings::default().bike_profile_idx, 0, "the profile index defaults to 0");
        // The one new byte still fits inside the same 16-byte RRAM line rounding as v7.
        assert_eq!(ENCODED_LEN, 176, "the settings blob is 176 B / 11 RRAM lines (v16 weather_alert_marks tail)");

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
        assert_eq!(ENCODED_LEN, 176, "the settings blob is 176 B / 11 RRAM lines (v16 weather_alert_marks tail)");

        // Each mode round-trips through the codec byte-for-byte.
        for mode in [WaypointMode::Off, WaypointMode::Approach, WaypointMode::Always] {
            let s = Settings { waypoint_mode: mode, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{mode:?} round-trips");
        }

        // An out-of-range stored byte (a newer writer, a bit-flip the CRC missed) sanitises to the
        // default Approach — re-stamp the CRC so only the payload byte is "wrong".
        let mut b = encode(&Settings { waypoint_mode: WaypointMode::Off, ..Settings::default() });
        b[WAYPOINT_OFF] = 200;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
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
        // The saved_sensors tail (v11) then the ride_retention byte (v12) grew the blob to 128 B /
        // 8 RRAM lines; the language byte kept its v10 offset.
        assert_eq!(ENCODED_LEN, 176, "the settings blob is 176 B / 11 RRAM lines (v16 weather_alert_marks tail)");

        // Each language round-trips through the codec byte-for-byte.
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            let s = Settings { language: lang, ..Settings::default() };
            assert_eq!(decode(&encode(&s)), Some(s), "{lang:?} round-trips");
        }

        // An out-of-range stored byte (a newer writer, a bit-flip the CRC missed) sanitises to the
        // default English — re-stamp the CRC so only the payload byte is "wrong".
        let mut b = encode(&Settings { language: Language::De, ..Settings::default() });
        b[LANGUAGE_OFF] = 200;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert_eq!(got.language, Language::En, "an unknown language byte falls back to English");

        // A v9 blob (the previous layout) is version-rejected → the host falls back to defaults, so
        // the language reads English — the established cross-version contract (no in-place upgrade).
        let mut old = encode(&Settings { language: Language::De, ..Settings::default() });
        old[0] = 9;
        let crc = crate::store_meta::crc16(&old[0..PAYLOAD_LEN]);
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
        assert_eq!(VERSION, 16, "saved_sensors is the v11 tail; the codec is now v16 (weather_alert_marks)");
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
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&b).unwrap().saved_sensors[1], SavedSensor::EMPTY, "an absent slot ignores stray bytes");

        // A corrupt-but-CRC-valid `addr_kind` past 1 normalises to random (`!= 0`) — the board's own
        // reading, so a bit-flip never mis-picks the address kind.
        let mut b = encode(&s);
        let off = SENSORS_OFF; // the HR slot
        b[off + 1] = 200;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(decode(&b).unwrap().saved_sensors[0].addr_kind, 1, "an out-of-range addr_kind reads as random");

        // Migration: a v10 blob (the previous layout, before saved_sensors) is version-rejected → the
        // host falls back to defaults, so every slot reads empty — the rejects-to-default contract
        // (no in-place upgrade), exactly like every prior codec bump.
        let mut old = encode(&s);
        old[0] = 10;
        let crc = crate::store_meta::crc16(&old[0..PAYLOAD_LEN]);
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
        assert_eq!(Language::En.stepped(2), Language::Fr, "multi-step flicks compound");
        // Press cycles one forward, exactly like a single right step.
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
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert!(got.device_name.is_empty(), "invalid UTF-8 falls back to the factory name");

        // An impossible stored length does too.
        let mut b = encode(&s);
        b[NAME_OFF] = 200;
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
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
        let crc = crate::store_meta::crc16(&b[0..PAYLOAD_LEN]);
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
        let crc = crate::store_meta::crc16(&wrong[0..PAYLOAD_LEN]);
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

    /// February's day count follows the leap rule (checked directly, and through `sanitize`, which
    /// re-pins an impossible Feb 31 to the month's real length).
    #[test]
    fn datetime_month_length_is_leap_aware() {
        assert_eq!(DateTime::month_len(2024, 2), 29, "2024 is a leap year");
        assert_eq!(DateTime::month_len(2025, 2), 28, "2025 is not");
        assert_eq!(DateTime::month_len(2000, 2), 29, "div-by-400 is a leap year");
        assert_eq!(DateTime::month_len(2100, 2), 28, "div-by-100-not-400 is not");

        let mut leap = DateTime { year: 2024, month: 2, day: 31, hour: 0, minute: 0 };
        leap.sanitize();
        assert_eq!(leap.day, 29, "Feb 31 in a leap year re-pins to Feb 29");
        let mut common = DateTime { year: 2025, month: 2, day: 31, hour: 0, minute: 0 };
        common.sanitize();
        assert_eq!(common.day, 28, "Feb 31 in a common year re-pins to Feb 28");
    }

    /// `add_minutes` carries across every boundary a live app clock deliberately advances through:
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
        // Zero is identity on an already-sane stamp.
        assert_eq!(base.add_minutes(0), base, "a zero advance changes nothing");
    }

    /// Multi-day, multi-month and multi-year advances land where an independent calendar
    /// (`datetime.timedelta`) puts them — the cases that separate a correct bulk carry from a
    /// day-at-a-time walk that quietly loses a leap day or a month length.
    #[test]
    fn datetime_add_minutes_matches_reference_over_long_advances() {
        let f = |d: DateTime| (d.year, d.month, d.day, d.hour, d.minute);
        // 400 days from New Year's Day 2025: across a year boundary into a common-year February.
        let a = DateTime { year: 2025, month: 1, day: 1, hour: 0, minute: 0 }.add_minutes(400 * 24 * 60);
        assert_eq!(f(a), (2026, 2, 5, 0, 0), "400 days from 2025-01-01");
        // 366 days from a leap day: 2024 → 2025 has no Feb 29 to land on.
        let b = DateTime { year: 2024, month: 2, day: 29, hour: 12, minute: 0 }.add_minutes(366 * 24 * 60);
        assert_eq!(f(b), (2025, 3, 1, 12, 0), "366 days from the 2024 leap day");
        // ~700 days in one call, with a minute-of-day carry on top.
        let c = DateTime { year: 2025, month: 6, day: 29, hour: 14, minute: 40 }.add_minutes(1_000_000);
        assert_eq!(f(c), (2027, 5, 25, 1, 20), "a million minutes");
        // The century rule both ways: 2100 is not a leap year, 2000 is.
        let d = DateTime { year: 2100, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!(f(d), (2100, 3, 1, 0, 0), "2100 is div-by-100-not-400: no Feb 29");
        let e = DateTime { year: 2000, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!(f(e), (2000, 2, 29, 0, 0), "2000 is div-by-400: Feb 29 exists");
        // Advancing in two steps is the same as advancing in one (the carry is associative).
        let base = DateTime { year: 2025, month: 6, day: 29, hour: 14, minute: 40 };
        assert_eq!(base.add_minutes(5_000).add_minutes(7_777), base.add_minutes(12_777), "split == whole");
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

    /// The offset's hard edges: a backward roll across a year boundary and across a leap day, and
    /// the widest real zones (UTC+14 / UTC−12), which move the date by a whole day each way.
    #[test]
    fn datetime_with_offset_crosses_year_and_leap_boundaries() {
        let f = |d: DateTime| (d.year, d.month, d.day, d.hour, d.minute);
        // Backward past midnight on New Year's Day: the year borrows too.
        let ny = DateTime { year: 2025, month: 1, day: 1, hour: 0, minute: 15 }.with_offset(-30);
        assert_eq!(f(ny), (2024, 12, 31, 23, 45), "New Year's Day − 30 min is New Year's Eve");
        // Backward into a leap day, and into the common-year Feb 28 for contrast.
        let leap = DateTime { year: 2024, month: 3, day: 1, hour: 0, minute: 30 }.with_offset(-60);
        assert_eq!(f(leap), (2024, 2, 29, 23, 30), "March 1 2024 borrows from Feb 29");
        let common = DateTime { year: 2025, month: 3, day: 1, hour: 0, minute: 0 }.with_offset(-1);
        assert_eq!(f(common), (2025, 2, 28, 23, 59), "March 1 2025 borrows from Feb 28");
        // Forward past midnight on New Year's Eve.
        let nye = DateTime { year: 2025, month: 12, day: 31, hour: 23, minute: 59 }.with_offset(1);
        assert_eq!(f(nye), (2026, 1, 1, 0, 0), "New Year's Eve + 1 min is New Year's Day");
        // The extreme zones: UTC+14 and UTC−12 from mid-morning both change the date.
        let mid = DateTime { year: 2025, month: 6, day: 29, hour: 10, minute: 0 };
        assert_eq!(f(mid.with_offset(14 * 60)), (2025, 6, 30, 0, 0), "UTC+14 pushes into tomorrow");
        assert_eq!(f(mid.with_offset(-12 * 60)), (2025, 6, 28, 22, 0), "UTC−12 pulls into yesterday");
        // Applying an offset and its negation is a round trip, in both directions and at an edge.
        for offset in [1i16, -1, 60, -60, 840, -720, 1439, -1439] {
            assert_eq!(mid.with_offset(offset).with_offset(-offset), mid, "round trip at {offset}");
            assert_eq!(ny.with_offset(offset).with_offset(-offset), ny, "round trip at {offset} from {ny:?}");
        }
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

    /// `local_clock` always applies the UTC offset — the anchor is UTC (manual editing was removed
    /// in #641), so local = anchor + offset, and a zero offset leaves it verbatim.
    #[test]
    fn local_clock_applies_the_utc_offset() {
        let clock = DateTime { year: 2025, month: 6, day: 29, hour: 12, minute: 0 };
        let zero = Settings { clock, utc_offset_min: 0, ..Settings::default() };
        assert_eq!(zero.local_clock(), clock, "a +00:00 offset leaves the UTC anchor unchanged");
        let plus2 = Settings { clock, utc_offset_min: 120, ..Settings::default() };
        let local = plus2.local_clock();
        assert_eq!((local.hour, local.minute), (14, 0), "local = UTC anchor + offset");
        assert_eq!((plus2.clock.hour, plus2.clock.minute), (12, 0), "the stored UTC anchor itself did not move");
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
        assert_eq!(Units::Metric.cycled(), Units::Imperial);
        assert_eq!(Units::Imperial.cycled(), Units::Metric);
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

// ==================== the Settings domain protocol (#1436) ====================
//
// SettingsMachine owns the dirty revision, the debounce, the retry and the stale-ack rule that
// [`SettingsMachine`] holds since #1397 S2. The platform executor writes **one** revision and says
// what happened;
// it decides nothing about when a write is owed or whether an old answer still counts.

use crate::device_core::{OperationToken, SettingsTag};

/// What moves the settings-persistence handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsIntent {
    /// A rider edit changed the live settings; `revision` is the value they now describe. A newer
    /// edit supersedes an older in-flight write, and the older ack is then no longer current.
    Changed { revision: u16 },
    /// The backoff after a failed write elapsed — retry the revision that is still owed.
    RetryDue,
}

/// The one bounded settings operation, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsEffect {
    /// Write the live settings to durable storage as `revision`. The values themselves are read
    /// from the resident [`Settings`] under the snapshot-at-execute rule — no settings copy ever
    /// rides the effect.
    PersistRevision { token: OperationToken<SettingsTag>, revision: u16 },
}

impl SettingsEffect {
    /// The operation this effect belongs to.
    pub fn token(&self) -> OperationToken<SettingsTag> {
        match self {
            SettingsEffect::PersistRevision { token, .. } => *token,
        }
    }
}

/// The result of one [`SettingsEffect`]. The typed failure reuses
/// [`SettingsSaveError`](obc_ports::SettingsSaveError) — the port already names every way a
/// settings write can fail, and a second vocabulary for the same thing would only drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsOutcome {
    /// `revision` reached durable storage.
    Persisted { token: OperationToken<SettingsTag>, revision: u16 },
    /// The write for `revision` failed; the value stays live in RAM and the revision stays owed.
    PersistFailed { token: OperationToken<SettingsTag>, revision: u16, error: obc_ports::SettingsSaveError },
    /// The executor abandoned the write without completing it — a platform with no durable store
    /// says so here instead of leaving the handshake parked forever.
    Cancelled { token: OperationToken<SettingsTag> },
}

impl SettingsOutcome {
    /// The operation this outcome answers.
    pub fn token(&self) -> OperationToken<SettingsTag> {
        match self {
            SettingsOutcome::Persisted { token, .. }
            | SettingsOutcome::PersistFailed { token, .. }
            | SettingsOutcome::Cancelled { token } => *token,
        }
    }
}

// Layout tripwires: a token, a revision, a reason — never a `Settings`.
const _: () = assert!(core::mem::size_of::<SettingsIntent>() <= 4, "a revision or nothing");
const _: () = assert!(core::mem::size_of::<SettingsEffect>() <= 8, "a token and a revision");
const _: () = assert!(core::mem::size_of::<SettingsOutcome>() <= 8, "a token, a revision and a reason");

/// Bounded backoff before a failed settings persist may re-emit (map-plane millis, #810). Fixed and
/// coarse: a persist failure is rare (an RRAM/file write error), the value stays live in RAM
/// meanwhile, and the retry only re-emits on a frame that runs for another reason — so this paces
/// retries without ever scheduling an idle wake.
pub(crate) const SETTINGS_RETRY_BACKOFF_MS: u32 = 2_000;

/// Wrap-safe "deadline reached" in the persist-backoff's **u16** millisecond space (the low 16 bits
/// of map-plane millis): true while `now` sits in the half-window at or past `deadline`. The u16
/// domain wraps every 65.5 s, so a frame gap longer than ~32.7 s can park a due retry in the "not
/// yet" half and slide it by up to one wrap — bounded, harmless for a rare failure path, and the
/// price of keeping the deadline to two resident bytes (#792 rule 2).
fn retry_deadline_reached(now: u16, deadline: u16) -> bool {
    now.wrapping_sub(deadline) < 0x8000
}

/// Where the settings-persistence handshake is (#810).
///
/// Deliberately **fieldless** (one byte): the Backoff deadline lives in the sibling
/// [`SettingsMachine::retry_at_ms`] field (meaningful only in Backoff), so this byte packs into an
/// existing padding hole instead of an 8-byte payload-carrying enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PersistState {
    /// The live settings are persisted at the current revision.
    #[default]
    Clean,
    /// An edit changed the live settings; a save is owed once the rider leaves the settings subtree.
    Dirty,
    /// A write was emitted for the current revision and awaits its answer.
    Awaiting,
    /// The last write failed; no retry re-emits before the deadline.
    Backoff,
}

/// The settings domain's persistence machine: the dirty revision, the subtree debounce, the retry
/// backoff and the stale-answer rule.
///
/// Editing is live in RAM the instant it happens; *persisting* it is an acknowledged, retryable
/// cross-boundary conversation keyed by the monotonic [`revision`](SettingsMachine::revision).
///
/// - **Clean** — persisted. An edit → **Dirty** (and bumps the revision).
/// - **Dirty** — a save is owed. Once outside the subtree, the next effect writes it → **Awaiting**.
/// - **Awaiting** — emitted and waiting; **not re-emitted** (no RRAM spam under a slow executor). A
///   matching success → **Clean**; a matching failure → **Backoff**. An edit here → **Dirty**
///   (supersede: the new revision re-emits, and the older answer no longer matches). An executor
///   that takes the write but never answers (the web demo has no durable store) parks here
///   terminally — by design: edits stay live in RAM and keep superseding, and nothing re-emits.
/// - **Backoff** — the last write failed; re-emits only once the deadline is reached. An edit →
///   **Dirty**, so a fresh revision skips the wait.
///
/// The revision is the supersede guard: an answer is honoured only when it equals the current one.
/// `u16` monotonic (wrapping) — a false match would need exactly 65,536 edits between an emit and
/// its answer, and only one revision is ever Awaiting.
#[derive(Debug, Default)]
pub(crate) struct SettingsMachine {
    /// The operation token for the write an executor is running.
    ops: crate::device_core::TokenSource<crate::device_core::SettingsTag>,
    /// The revision of the live settings, bumped by every edit whose before/after compare finds a
    /// change. Starts `0`, re-zeroed when the boot value is seeded.
    revision: u16,
    /// The [`Backoff`](PersistState::Backoff) retry deadline — the **low 16 bits** of map-plane
    /// millis. Meaningful only while [`persist`](SettingsMachine::persist) is Backoff.
    retry_at_ms: u16,
    /// Where the handshake is.
    persist: PersistState,
}

impl SettingsMachine {
    /// The boot state: Clean at revision 0 — the boot value came from the store or the default.
    pub(crate) const fn new() -> Self {
        SettingsMachine {
            ops: crate::device_core::TokenSource::new(),
            revision: 0,
            retry_at_ms: 0,
            persist: PersistState::Clean,
        }
    }

    /// Admit one settings intent.
    ///
    /// [`Changed`](SettingsIntent::Changed) from *any* prior state supersedes an in-flight or
    /// backing-off older revision: the new content re-emits, and the older answer, when it lands,
    /// no longer matches the revision and is ignored (#810).
    pub(crate) fn admit_intent(&mut self, intent: SettingsIntent) {
        match intent {
            SettingsIntent::Changed { revision } => {
                self.revision = revision;
                self.persist = PersistState::Dirty;
            }
            // The backoff elapsing is not a state change on its own: `next_effect` re-derives the
            // deadline from the clock it is handed, so a due retry emits without anything to latch.
            SettingsIntent::RetryDue => {}
        }
    }

    /// A rider edit: bump the revision and (re-)arm the save.
    pub(crate) fn note_edited(&mut self) {
        let revision = self.revision.wrapping_add(1);
        self.admit_intent(SettingsIntent::Changed { revision });
    }

    /// The boot value was just seeded from the store (or the default): it is already persisted, so
    /// reset to Clean at revision 0. Any pending edit is discarded — seeding is a boot/reload
    /// operation, not a rider edit.
    pub(crate) fn note_seeded(&mut self) {
        self.revision = 0;
        self.persist = PersistState::Clean;
    }

    /// Whether a write is owed **and** may be emitted now: the value is dirty, the rider has left
    /// the settings subtree, and we are neither already awaiting an answer nor inside a failed-write
    /// backoff window.
    pub(crate) fn wants_write(&self, in_settings_subtree: bool, now_ms: u32) -> bool {
        if in_settings_subtree {
            return false;
        }
        match self.persist {
            PersistState::Dirty => true,
            PersistState::Backoff => retry_deadline_reached(now_ms as u16, self.retry_at_ms),
            PersistState::Clean | PersistState::Awaiting => false,
        }
    }

    /// The next bounded settings operation, or `None` when none is owed this pass.
    ///
    /// The dirty state is *not* cleared here (the #810 fix): a failed write must keep the revision
    /// retryable, so Clean is reached only by a matching success.
    pub(crate) fn next_effect(&mut self, in_settings_subtree: bool, now_ms: u32) -> Option<SettingsEffect> {
        if !self.wants_write(in_settings_subtree, now_ms) {
            return None;
        }
        self.persist = PersistState::Awaiting;
        Some(SettingsEffect::PersistRevision { token: self.ops.issue(), revision: self.revision })
    }

    /// Consume the answer to a write. Returns `true` when the write **failed** and the rider must
    /// be told — the one part of this the domain cannot do itself.
    ///
    /// Both guards are checked, and they are independent: the token rejects a superseded
    /// *operation*, the revision rejects a stale *value*. A stale answer leaves the newer content
    /// pending either way.
    pub(crate) fn apply_outcome(&mut self, outcome: SettingsOutcome, now_ms: u32) -> bool {
        if !self.ops.is_current(outcome.token()) {
            return false;
        }
        self.ops.invalidate(); // terminal: a duplicate of this answer is no longer current
        match outcome {
            SettingsOutcome::Persisted { revision, .. } => {
                self.note_persisted(revision);
                false
            }
            SettingsOutcome::PersistFailed { revision, .. } => self.note_persist_failed(revision, now_ms),
            // A platform with no durable store says so here instead of parking the handshake
            // forever. The value stays dirty and retryable; nothing is claimed to have been written.
            SettingsOutcome::Cancelled { .. } => {
                if self.persist == PersistState::Awaiting {
                    self.persist = PersistState::Dirty;
                }
                false
            }
        }
    }

    /// `revision` reached durable storage. Clear to Clean **only** while it is still the latest — a
    /// newer edit has already moved the machine back to Dirty, and that content stays pending.
    ///
    /// The revision is the whole guard here, because the legacy protocol carries no token: an
    /// answer to a write nobody made cannot be distinguished from a stale one, and both leave the
    /// live value exactly where it is.
    pub(crate) fn note_persisted(&mut self, revision: u16) {
        if self.persist == PersistState::Awaiting && revision == self.revision {
            self.persist = PersistState::Clean;
        }
    }

    /// The write for `revision` failed. Keep it dirty and re-arm the bounded backoff, but only
    /// while it is still the in-flight latest. Returns whether the rider is told.
    pub(crate) fn note_persist_failed(&mut self, revision: u16, now_ms: u32) -> bool {
        if self.persist == PersistState::Awaiting && revision == self.revision {
            self.retry_at_ms = (now_ms as u16).wrapping_add(SETTINGS_RETRY_BACKOFF_MS as u16);
            self.persist = PersistState::Backoff;
        }
        true
    }

    /// Test hook: arm a pending save without driving a real edit, standing in for a settings-screen
    /// edit the drain/gating tests do not replay.
    #[cfg(test)]
    pub(crate) fn arm_save(&mut self) {
        self.note_edited();
    }

    /// Whether nothing at all is owed: Clean at revision 0 — the [`new`](SettingsMachine::new)
    /// state. The destructure is exhaustive, so a field added here must state its empty value too.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        let SettingsMachine { ops, revision, retry_at_ms, persist } = self;
        format!("{ops:?}") == "TokenSource(0)" && *revision == 0 && *retry_at_ms == 0 && *persist == PersistState::Clean
    }
}

// Layout tripwire: a revision, a deadline, a phase and a generation — never a `Settings`.
const _: () = assert!(core::mem::size_of::<SettingsMachine>() <= 12, "the handshake, not the values");

#[cfg(test)]
mod settings_machine_tests {
    use super::*;
    use obc_ports::SettingsSaveError;

    /// The token a write went out under, so a test can answer the operation the machine is actually
    /// holding.
    fn emit(
        machine: &mut SettingsMachine,
        now_ms: u32,
    ) -> (crate::device_core::OperationToken<crate::device_core::SettingsTag>, u16) {
        match machine.next_effect(false, now_ms).expect("a write is owed") {
            SettingsEffect::PersistRevision { token, revision } => (token, revision),
        }
    }

    /// The debounce: nothing is written while the rider is still inside the settings subtree — they
    /// are mid-edit — and exactly one write goes out when they leave.
    #[test]
    fn no_write_leaves_while_the_rider_is_inside_the_settings_subtree() {
        let mut machine = SettingsMachine::new();
        machine.note_edited();
        assert!(machine.next_effect(true, 100).is_none(), "still editing");
        assert!(machine.next_effect(true, 200).is_none(), "…and still editing");

        let (_, revision) = emit(&mut machine, 300);
        assert_eq!(revision, 1, "the edit's revision leaves once");
        assert!(machine.next_effect(false, 400).is_none(), "awaiting an answer — never re-emitted");
    }

    /// **#810.** A stale ack — one for a revision a newer edit has already superseded — must not
    /// clear the newer content. The revision is the guard, and it is checked independently of the
    /// token: the legacy protocol carries no token at all.
    #[test]
    fn a_stale_ack_does_not_clear_the_newer_state() {
        let mut machine = SettingsMachine::new();
        machine.note_edited(); // revision 1
        emit(&mut machine, 100);
        machine.note_edited(); // revision 2 supersedes it while the write is in flight

        machine.note_persisted(1);
        assert!(machine.wants_write(false, 200), "the newer content is still owed");
        let (_, revision) = emit(&mut machine, 200);
        assert_eq!(revision, 2, "and it is the newer revision that goes out");
        machine.note_persisted(2);
        assert!(!machine.wants_write(false, 300), "the matching ack is what clears it");
    }

    /// A failed write keeps the revision dirty, backs off, and retries **once** the window elapses —
    /// never before it, and never in a loop.
    #[test]
    fn a_failed_write_backs_off_and_retries_once() {
        let mut machine = SettingsMachine::new();
        machine.note_edited();
        let (_, revision) = emit(&mut machine, 1_000);

        assert!(machine.note_persist_failed(revision, 1_000), "the rider is told a save failed");
        assert!(!machine.wants_write(false, 1_000 + SETTINGS_RETRY_BACKOFF_MS - 1), "not before the window");
        assert!(machine.wants_write(false, 1_000 + SETTINGS_RETRY_BACKOFF_MS), "and exactly once at it");

        let (_, retried) = emit(&mut machine, 1_000 + SETTINGS_RETRY_BACKOFF_MS);
        assert_eq!(retried, revision, "the same content, not a new one");
        assert!(machine.next_effect(false, 1_000 + 4 * SETTINGS_RETRY_BACKOFF_MS).is_none(), "one retry in flight");
    }

    /// A platform that takes the write and never answers (the web demo has no durable store) parks
    /// — by design. Edits stay live in RAM and keep superseding, and nothing re-emits into a store
    /// that will not answer.
    #[test]
    fn an_executor_that_never_answers_parks_without_re_emitting() {
        let mut machine = SettingsMachine::new();
        machine.note_edited();
        emit(&mut machine, 100);
        for ms in [200, 10_000, 100_000, 1_000_000] {
            assert!(machine.next_effect(false, ms).is_none(), "no RRAM spam under a silent executor");
        }

        // …and a `Cancelled` answer is how such a platform says so honestly: the value stays dirty
        // and retryable rather than parked forever.
        machine.note_edited();
        let (token, _) = emit(&mut machine, 2_000_000);
        assert!(!machine.apply_outcome(SettingsOutcome::Cancelled { token }, 2_000_000));
        assert!(machine.wants_write(false, 2_000_001), "the write is owed again");
    }

    /// The token and the revision are independent guards: an answer to a *superseded operation* is
    /// refused before its revision is even looked at.
    #[test]
    fn a_superseded_operation_is_refused_on_its_token() {
        let mut machine = SettingsMachine::new();
        machine.note_edited();
        let (first, revision) = emit(&mut machine, 100);
        machine.note_edited();
        emit(&mut machine, 200);

        let stale = SettingsOutcome::PersistFailed { token: first, revision, error: SettingsSaveError::Backend };
        assert!(!machine.apply_outcome(stale, 200), "a superseded write cannot report a failure to the rider");
    }
}
