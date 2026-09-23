//! Persistent device settings and their byte codec.
//!
//! [`Settings`] is `Copy + PartialEq`, so a single comparison detects a rider edit and flags a
//! save. The codec ([`encode`]/[`decode`]) is a versioned, CRC-checked, fixed-length blob shared by
//! the sim file store and the firmware RRAM store; a blank or corrupt read falls back to
//! [`Settings::default`].

use crate::i18n::{t, Msg};
use crate::screen::BRIGHTNESS_MAX;
use crate::settings_enum::setting_enum;
use crate::settings_table::settings_table;
use crate::stat_fields::StatFieldList;
pub use obc_formats::bike::BikeType;

pub(crate) use obc_ports::DateTime;

pub const DATETIME_MIN_YEAR: u16 = 2020;
pub const DATETIME_MAX_YEAR: u16 = 2099;

/// The storage range this project imposes on the dependency-neutral [`DateTime`]. Calendar
/// arithmetic stays inherent on `DateTime` in `obc-ports`; only the 2020–2099 range is ours.
pub trait DateTimeEditorExt {
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

pub(crate) fn add_minutes_bounded(date: DateTime, mins: u32) -> DateTime {
    clamp_app_year(date.add_minutes(mins))
}

fn with_offset_bounded(date: DateTime, offset: i16) -> DateTime {
    clamp_app_year(date.with_offset(offset))
}

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
    pub enum Units {
        Metric = 0, key Msg::UnitsMetric;
        Imperial = 1, key Msg::UnitsImperial;
    }
    default Metric;
}

/// The device-name byte cap, shared with the BLE Config name field.
pub const DEVICE_NAME_MAX: usize = 48;

/// The user-facing device name. A fixed inline buffer so [`Settings`] stays `Copy`. An empty name
/// means the factory name: the BLE edge substitutes its serial-derived `OBC-XXXX`.
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
    pub const EMPTY: DeviceName = DeviceName { len: 0, bytes: [0; DEVICE_NAME_MAX] };

    /// Store `name`, truncated to the byte cap on a char boundary, never mid-UTF-8.
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

    /// Rebuild from stored bytes. Over-long or invalid-UTF-8 input sanitises to [`Self::EMPTY`],
    /// never to garbage the BLE edge would advertise.
    pub fn from_bytes(bytes: &[u8]) -> DeviceName {
        if bytes.len() > DEVICE_NAME_MAX || core::str::from_utf8(bytes).is_err() {
            return Self::EMPTY;
        }
        let mut n = Self::EMPTY;
        n.len = bytes.len() as u8;
        n.bytes[..bytes.len()].copy_from_slice(bytes);
        n
    }

    pub fn as_str(&self) -> &str {
        // Every constructor stored validated UTF-8, so this cannot fail.
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

pub const MI_PER_KM: f32 = 0.621_371;
pub const FT_PER_M: f32 = 3.280_84;
pub const FT_PER_MI: u32 = 5280;

impl Units {
    #[inline]
    pub const fn is_imperial(self) -> bool {
        matches!(self, Units::Imperial)
    }

    #[inline]
    pub fn dist(self, km: f32) -> f32 {
        if self.is_imperial() {
            km * MI_PER_KM
        } else {
            km
        }
    }

    #[inline]
    pub fn speed(self, kmh: f32) -> f32 {
        if self.is_imperial() {
            kmh * MI_PER_KM
        } else {
            kmh
        }
    }

    #[inline]
    pub fn elev(self, m: f32) -> f32 {
        if self.is_imperial() {
            m * FT_PER_M
        } else {
            m
        }
    }

    #[inline]
    pub const fn speed_label(self) -> &'static str {
        if self.is_imperial() {
            "MPH"
        } else {
            "KPH"
        }
    }

    #[inline]
    pub const fn dist_label(self) -> &'static str {
        if self.is_imperial() {
            "MI"
        } else {
            "KM"
        }
    }

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
    /// The colour theme used by every device screen.
    pub enum Theme {
        Light = 0, key Msg::ThemeLight;
        Dark = 1, key Msg::ThemeDark;
    }
    default Light;
}

setting_enum! {
    /// How the Climb screen is reached. The Stats settings screen cycles it.
    pub enum ClimbMode {
        /// Kept out of the Back-cycle entirely and never auto-shown.
        Off = 0, key Msg::ClimbModeOff;
        /// In the Back-cycle while a climb is active; the device never switches to it on its own.
        Manual = 1, key Msg::ClimbModeManual;
        /// In the Back-cycle, and the device switches to it on climb entry and back to the Map on
        /// the crest.
        Auto = 2, key Msg::ClimbModeAuto;
    }
    default Auto;
}

/// The bike type's name in `lang`. The device never shows a map's profile names.
pub const fn bike_type_name(bike: BikeType, lang: Language) -> &'static str {
    t(
        match bike {
            BikeType::Road => Msg::BikeTypeRoad,
            BikeType::Gravel => Msg::BikeTypeGravel,
            BikeType::Mtb => Msg::BikeTypeMtb,
            BikeType::Touring => Msg::BikeTypeTouring,
        },
        lang,
    )
}

impl ClimbMode {
    #[inline]
    pub const fn is_on(self) -> bool {
        !matches!(self, ClimbMode::Off)
    }
}

setting_enum! {
    /// Whether, and when, the Map's bottom-centre waypoint chip is shown: the `◆ NAME  <dist>` pill
    /// counting the along-route distance to the next named waypoint ahead.
    pub enum WaypointMode {
        /// Never shown — the silencer for routes carrying junk waypoints from a planner's export.
        Off = 0, key Msg::WaypointModeOff;
        /// Shown only within the approach radius (`WAYPOINT_APPROACH_M`, 500 m) ahead.
        Approach = 1, key Msg::WaypointModeApproach;
        /// Shown whenever a named waypoint lies ahead, subject to the shared no-fix, off-route and
        /// pan suppression.
        Always = 2, key Msg::WaypointModeAlways;
    }
    default Approach;
}

setting_enum! {
    /// Which sources feed the Up-ahead timeline.
    pub enum UpAheadSource {
        /// Custom waypoints and route-corridor map POIs.
        Both = 0, key Msg::UpAheadSourceBoth;
        /// The rider's own GPX waypoints only. The corridor query is never armed under this value,
        /// so the map `Reader` is never built for it either.
        WaypointsOnly = 1, key Msg::UpAheadSourceWaypoints;
        /// Route-corridor map POIs only. The waypoint plan leaves the timeline entirely; it stays
        /// on the map and in the stats panel.
        MapPoisOnly = 2, key Msg::UpAheadSourceMapPois;
    }
    default Both;
}

impl UpAheadSource {
    #[inline]
    pub const fn shows_waypoints(self) -> bool {
        matches!(self, UpAheadSource::Both | UpAheadSource::WaypointsOnly)
    }

    /// Whether route-corridor map POIs feed the list. Also the arming answer: `false` means no
    /// [`CorridorKey`](crate::corridor::CorridorKey) is ever declared, so the query never runs.
    #[inline]
    pub const fn shows_pois(self) -> bool {
        matches!(self, UpAheadSource::Both | UpAheadSource::MapPoisOnly)
    }
}

setting_enum! {
    /// How long the UI sits idle before it navigates itself back to where it belongs: the Home root
    /// when not tracking a ride, the Map when a ride is running. The Power settings screen picks it.
    ///
    /// The durations are unit-glued numbers, catalogued whole so a language can localize the
    /// `s`/`min` grain.
    pub enum IdleReturn {
        S15 = 0, key Msg::IdleS15, Some(15_000);
        S30 = 1, key Msg::IdleS30, Some(30_000);
        M1 = 2, key Msg::IdleM1, Some(60_000);
        M5 = 3, key Msg::IdleM5, Some(300_000);
        Never = 4, key Msg::IdleNever, None;
    }
    default S30;
    /// The idle timeout in millis; `None` for [`Never`](IdleReturn::Never), which also disables the
    /// idle wake, so a parked device is not woken to no purpose.
    payload timeout_ms: Option<u32>;
}

setting_enum! {
    /// The UI language.
    ///
    /// Each value's [`name`](Language::name) is its endonym, so the picker row reads to a speaker
    /// who cannot yet read the current UI language. That is why this is the one settings enum whose
    /// labels are literals and not catalog keys.
    ///
    /// [`COUNT`](Language::COUNT) is the number of columns the i18n catalog must ship: a static
    /// assertion in [`i18n`](crate::i18n) ties the table's column count to it, so a new variant
    /// without its `{lang}.toml` column fails the build instead of panicking on the first draw.
    pub enum Language {
        En = 0, text "English";
        De = 1, text "Deutsch";
        Fr = 2, text "Français";
        Es = 3, text "Español";
    }
    default En;
}

impl Language {
    pub const fn article_code(self) -> [u8; 2] {
        match self {
            Self::En => *b"en",
            Self::De => *b"de",
            Self::Fr => *b"fr",
            Self::Es => *b"es",
        }
    }
}

/// UTC-offset stepper bounds and granularity in minutes. 15-minute steps cover the real `:30` and
/// `:45` zones over the −12:00…+14:00 span.
pub const UTC_OFFSET_MIN: i16 = -12 * 60;
pub const UTC_OFFSET_MAX: i16 = 14 * 60;
pub const UTC_OFFSET_STEP: i16 = 15;

/// GPS-fix-interval stepper bounds in seconds.
pub const FIX_INTERVAL_MIN: u16 = 1;
pub const FIX_INTERVAL_MAX: u16 = 120;

/// Stats-grid page auto-cycle bounds in seconds. There is no "off" value: the auto-cycle is the
/// only way to reach a second page, so the minimum is a brisk-but-readable 2 s.
pub const STAT_CYCLE_MIN: u16 = 2;
pub const STAT_CYCLE_MAX: u16 = 20;
pub const STAT_CYCLE_DEFAULT: u16 = 5;

/// One saved sensor per quantity: index 0 HR, 1 Power, 2 Cadence. The slot index is the kind, so
/// the kind is not stored.
pub const SENSOR_SLOTS: usize = 3;

/// A saved BLE sensor for one quantity slot: the advertising address the board's central manager
/// reconnects by across a reboot. There is no name and no bond; these sensors are open GATT servers
/// connected by address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SavedSensor {
    /// Whether this slot holds a saved sensor. `false` leaves the address as unused zeros.
    pub present: bool,
    /// The advertiser address kind: `0` public, `1` random. The manager must reconnect by the same
    /// kind — a static-random watch advertises `RANDOM`.
    pub addr_kind: u8,
    /// The 6-byte advertising address, little-endian as the wire carries it.
    pub addr: [u8; 6],
}

impl SavedSensor {
    pub const EMPTY: SavedSensor = SavedSensor { present: false, addr_kind: 0, addr: [0; 6] };

    pub const fn saved(addr_kind: u8, addr: [u8; 6]) -> SavedSensor {
        SavedSensor { present: true, addr_kind, addr }
    }
}

setting_enum! {
    /// Maximum recommendations and candidates from each Find a Place source.
    pub enum FindResults {
        Two = 0, text "2";
        Four = 1, text "4";
        Six = 2, text "6";
    }
    default Four;
}

impl FindResults {
    pub const fn limit(self) -> usize {
        2 + 2 * self as usize
    }
}

settings_table! {
    /// The whole persisted settings set. Plain old data — `Copy` + `Eq`, no floats — so a
    /// before/after `==` flags a save and the codec is a field-by-field pack.
    ///
    /// One row per persisted field, in blob order, which is also the order [`encode`] writes.
    /// Everything the blob needs of a field is on its row.
    pub struct Settings {
        units: Units = Units::Metric, since(19), ble_writable;
        /// A rider or phone has set the local UTC offset. GPS time alone does not establish it.
        local_offset_known: bool = false, since(19);
        /// The last time source's UTC set-point, stamped by a GPS fix or a BLE `setClock`. Only
        /// those two trusted sources write it, so it is always UTC and
        /// [`local_clock`](Settings::local_clock) always folds in
        /// [`utc_offset_min`](Settings::utc_offset_min). Persisted so it seeds the boot display
        /// clock, which stays display-only until re-stamped this boot.
        clock: DateTime = DateTime::DEFAULT, since(19), sanitize_with(DateTimeEditorExt::sanitize);
        /// Local time's offset from UTC, in minutes (`+02:00` → `120`).
        utc_offset_min: i16 = 0, since(19), range(UTC_OFFSET_MIN, UTC_OFFSET_MAX);
        fix_interval_s: u16 = 1, since(19), range(FIX_INTERVAL_MIN, FIX_INTERVAL_MAX);
        /// GPS low-power mode.
        power_saver: bool = false, since(19);
        /// The rider's ordered Statistics-grid field selection.
        stat_fields: StatFieldList = StatFieldList::DEFAULT, since(19);
        /// Seconds the Statistics grid dwells on each page before auto-cycling to the next.
        stat_cycle_s: u16 = STAT_CYCLE_DEFAULT, since(19), range(STAT_CYCLE_MIN, STAT_CYCLE_MAX);
        /// The user-facing device name; empty means the factory `OBC-XXXX`. Written by the companion
        /// app over BLE, not by any on-device screen.
        device_name: DeviceName = DeviceName::EMPTY, since(19), ble_writable;
        /// The Bluetooth radio switch. Off stops advertising and drops any live connection. It is
        /// device-only because a phone that switched the radio off could not switch it back on.
        ble_enabled: bool = true, since(19);
        climb_mode: ClimbMode = ClimbMode::Auto, since(19);
        idle_return: IdleReturn = IdleReturn::S30, since(19);
        /// Show the small floating `HH:MM` clock on the Map.
        map_clock: bool = true, since(19);
        map_scale_bar: bool = true, since(19);
        /// The current bike type. Loading a route sets it to the route's type.
        bike_type: BikeType = BikeType::Road, since(19);
        waypoint_mode: WaypointMode = WaypointMode::Approach, since(19);
        language: Language = Language::En, since(19);
        /// One slot per quantity; an empty slot is "no sensor saved". Written by the Sensors screen
        /// on pair and forget; the board's central manager reconnects to a present slot's address
        /// whenever the radio is on.
        saved_sensors: [SavedSensor; SENSOR_SLOTS] = [SavedSensor::EMPTY; SENSOR_SLOTS], since(19);
        up_ahead_source: UpAheadSource = UpAheadSource::Both, since(19);
        /// Draw the map's terrain layer, today the contour lines. Off drops every terrain-layer
        /// style from the renderer's collect pass, so the geometry is never decoded — but contours
        /// share cells with everything else, so off neither shrinks the map on the card nor avoids a
        /// chunk read. It hides the ink, not the bytes; it is not a performance control.
        ///
        /// Provisional: the switch exists to judge contours on glass, and is expected to be removed
        /// whichever way that judgement goes.
        map_contours: bool = true, since(19);
        brightness: u8 = BRIGHTNESS_MAX, since(19), range(0, BRIGHTNESS_MAX);
        /// Find a Place search filter, shared by all categories and the paged browser.
        find_hide_closed: bool = true, since(21);
        find_results: FindResults = FindResults::Four, since(21);
        /// Map icon switches are independent of Find a Place filters.
        map_peaks: bool = true, since(22);
        map_landmarks: bool = true, since(22);
        map_pois: bool = true, since(22);
        /// Bit positions follow PoiCategory::ALL, not the wire category IDs.
        map_poi_categories: u8 = 0x7f, since(22), range(0, 0x7f);
        theme: Theme = Theme::Light, since(23);
        /// The rider's maximum heart rate (bpm) and FTP (W), which the effort zones are taken
        /// from. `0` is not set, and then that metric has no zones.
        max_hr: u8 = 0, since(24), sanitize_with(crate::effort::sanitize_max_hr);
        ftp_w: u16 = 0, since(24), sanitize_with(crate::effort::sanitize_ftp);
    }

    pub const DEFAULT;

    pub fn adopt_ble_fields;

    /// Clamp every field into its valid range, applied after a decode. One line per `range` or
    /// `sanitize_with` row; a field with neither marker is deliberately never clamped, and its doc
    /// says why.
    fn sanitize;

    /// Pack [`Settings`] into its fixed [`ENCODED_LEN`]-byte blob: a version byte, the little-endian
    /// fields, then a trailing CRC.
    pub fn encode;

    /// Decode a blob written by [`encode`] at any supported version: every field the stored version
    /// declared is read, and the fields appended after it take their declared defaults, so a
    /// firmware update that appends a setting keeps the rider's values.
    ///
    /// `None` — the host then falls back to [`Settings::default`] — if the version is outside
    /// [`MIN_SUPPORTED`]`..=`[`VERSION`], if the blob is shorter than that version's
    /// [`encoded_len`], or if the CRC over that version's payload fails. Bytes past its encoded
    /// length are ignored, which is what makes the board's fixed-`SLOT_LEN` read work after a bump.
    pub fn decode;
}

/// The in-memory footprint, pinned. [`Settings`] is copied whole into the live `App`, the board's
/// Config cache and the `.rodata` [`DEFAULT`](Settings::DEFAULT) image, so a field that widens the
/// struct widens every one of those.
const _: () = assert!(core::mem::size_of::<Settings>() == 122, "Settings grew — was that deliberate?");

impl Settings {
    pub(crate) fn find_hours_filter(&self) -> obc_reader::reader::places::HoursFilter {
        use obc_reader::reader::places::HoursFilter;
        if self.find_hide_closed {
            HoursFilter::HideClosed
        } else {
            HoursFilter::All
        }
    }

    pub(crate) fn effort_limits(&self) -> crate::effort::Limits {
        crate::effort::Limits { max_hr: self.max_hr, ftp_w: self.ftp_w }
    }

    /// The local wall-clock set-point the device shows: the UTC [`clock`](Settings::clock) anchor
    /// shifted by [`utc_offset_min`](Settings::utc_offset_min), so a shift across midnight rolls
    /// the date too.
    pub fn local_clock(&self) -> DateTime {
        with_offset_bounded(self.clock, self.utc_offset_min)
    }
}

pub const VERSION: u8 = 24;

/// The oldest layout [`decode`] accepts. An older blob resets to defaults.
pub const MIN_SUPPORTED: u8 = 19;

/// The encoded length of a `payload`-byte payload: the CRC-covered bytes plus a 2-byte CRC, rounded
/// up to the device RRAM's 16-byte write line. Bytes past the CRC are unused zero padding.
///
/// A function rather than only [`ENCODED_LEN`] because [`decode`] applies the same rounding to the
/// stored version's payload.
pub const fn encoded_len(payload: usize) -> usize {
    (payload + 2).div_ceil(16) * 16
}

/// Fixed encoded length of a blob written by the current version.
pub const ENCODED_LEN: usize = encoded_len(PAYLOAD_LEN);

/// Payload size before the trailing CRC, which follows immediately at this offset.
const PAYLOAD_LEN: usize = off::END;

// Literal byte offsets pin the current settings layout.
const _: () = {
    assert!(off::units == 1, "units moved");
    assert!(off::local_offset_known == 2, "offset authority moved");
    assert!(off::clock == 3, "clock moved");
    assert!(off::utc_offset_min == 9, "utc_offset_min moved");
    assert!(off::fix_interval_s == 11, "fix_interval_s moved");
    assert!(off::power_saver == 13, "power_saver moved");
    assert!(off::stat_fields == 14, "stat_fields moved");
    assert!(off::stat_cycle_s == 27, "stat_cycle_s moved");
    assert!(off::device_name == 29, "device_name moved");
    assert!(off::ble_enabled == 78, "ble_enabled moved");
    assert!(off::climb_mode == 79, "climb_mode moved");
    assert!(off::idle_return == 80, "idle_return moved");
    assert!(off::map_clock == 81, "map_clock moved");
    assert!(off::map_scale_bar == 82, "map_scale_bar moved");
    assert!(off::bike_type == 83, "bike_type moved");
    assert!(off::waypoint_mode == 84, "waypoint_mode moved");
    assert!(off::language == 85, "language moved");
    assert!(off::saved_sensors == 86, "saved_sensors moved");
    assert!(off::up_ahead_source == 110, "up_ahead_source moved");
    assert!(off::map_contours == 111, "map_contours moved");
    assert!(off::brightness == 112, "brightness moved");
    assert!(off::find_hide_closed == 113);
    assert!(off::find_results == 114);
    assert!(off::theme == 119);
    assert!(off::max_hr == 120);
    assert!(off::ftp_w == 121);
    assert!(PAYLOAD_LEN == 123, "the CRC moved");
    assert!(ENCODED_LEN == 128, "the blob is no longer 8 RRAM lines");
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_table::SAVED_SENSOR_LEN;

    /// `Settings::DEFAULT` names every per-type default variant literally, so pin each against its
    /// type's own `Default`. Every field whose type has a `Default` belongs here, above all the
    /// ones the const spells as a plain literal: those are the ones that fork silently.
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
        assert_eq!(d.up_ahead_source, UpAheadSource::default());
        assert_eq!(d.find_results, FindResults::default());
        assert_eq!(d.theme, Theme::default());

        // And the whole const is its type's `Default` — the property the field list guards.
        assert_eq!(d, Settings::default());
    }

    /// A settings value with every field pushed off its default. Shared by the round-trip test and
    /// the golden blobs below, so the two always speak about the same bytes.
    fn every_field_set() -> Settings {
        let mut stat_fields = StatFieldList::default();
        stat_fields.remove(0);
        assert!(stat_fields.push(crate::stat_fields::StatField::Clock));
        Settings {
            units: Units::Imperial,
            local_offset_known: false,
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
            map_peaks: false,
            map_landmarks: false,
            map_pois: false,
            map_poi_categories: 3,
            bike_type: BikeType::Touring,
            waypoint_mode: WaypointMode::Always,
            language: Language::De,
            saved_sensors: [
                SavedSensor::saved(1, [1, 2, 3, 4, 5, 6]),
                SavedSensor::EMPTY,
                SavedSensor::saved(0, [6, 5, 4, 3, 2, 1]),
            ],
            up_ahead_source: UpAheadSource::MapPoisOnly,
            find_hide_closed: false,
            find_results: FindResults::Six,
            theme: Theme::Dark,
            max_hr: 185,
            ftp_w: 250,

            brightness: 1,
        }
    }

    /// Re-stamp the CRC over a doctored blob, so [`decode`] sees a valid blob whose payload is
    /// wrong.
    fn re_stamp_crc(b: &mut [u8; ENCODED_LEN]) {
        let crc = crate::crc16::crc16(&b[0..PAYLOAD_LEN]);
        b[PAYLOAD_LEN..PAYLOAD_LEN + 2].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn codec_round_trips() {
        let s = every_field_set();
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    #[test]
    fn version_22_blob_keeps_existing_values_and_defaults_theme() {
        let mut expected = every_field_set();
        expected.theme = Theme::Light;

        let mut old = encode(&expected);
        old[0] = 22;
        let old_payload_len = off::theme;
        let crc = crate::crc16::crc16(&old[..old_payload_len]);
        old[old_payload_len..old_payload_len + 2].copy_from_slice(&crc.to_le_bytes());

        assert_eq!(decode(&old), Some(expected));
    }

    /// One table over every declared field: the fixture moves every row off its default, that
    /// value round-trips through the codec, and `adopt_ble_fields` pulls across exactly the
    /// `ble_writable` rows. The per-field walk is generated from the table itself, so a new row is
    /// covered the moment it is declared.
    #[test]
    fn every_declared_field_round_trips_and_keeps_its_ble_split() {
        let base = Settings::DEFAULT;
        let other = Settings { local_offset_known: true, ..every_field_set() };
        assert_eq!(decode(&encode(&other)), Some(other), "every field round-trips through the codec");

        let mut adopted = base;
        adopted.adopt_ble_fields(&other);
        // Hand-written, like the offset literals: the fields the phone owns. Deriving this from the
        // table's own `ble_writable` markers would restate the token that generates
        // `adopt_ble_fields` and could never fail.
        Settings::assert_field_table(&base, &other, &adopted, &["units", "device_name"]);
    }

    #[test]
    fn decode_sanitises_an_unknown_enum_byte_through_the_blob() {
        // The `brightness` row's `range` marker is the only thing between a corrupt byte and a
        // panel driven at a level the port never offered. It clamps up, not down: the safe
        // direction for a light is bright.
        let mut b = encode(&Settings { brightness: 0, ..Settings::default() });
        b[off::brightness] = 200;
        re_stamp_crc(&mut b);
        assert_eq!(decode(&b).unwrap().brightness, BRIGHTNESS_MAX, "an out-of-range level clamps to the brightest");
    }

    #[test]
    fn an_unknown_bike_type_byte_reads_as_road() {
        let mut b = encode(&Settings { bike_type: BikeType::Mtb, ..Settings::default() });
        b[off::bike_type] = 9;
        re_stamp_crc(&mut b);
        assert_eq!(decode(&b).expect("valid CRC").bike_type, BikeType::Road);
    }

    #[test]
    fn a_bool_field_reads_any_non_zero_byte_as_on() {
        let mut b = encode(&Settings { map_contours: false, ..Settings::default() });
        b[off::map_contours] = 7;
        re_stamp_crc(&mut b);
        assert!(decode(&b).expect("valid CRC").map_contours, "any non-zero byte reads as on");
    }

    /// The saved-sensor block's decode tolerances: an absent slot ignores stray bytes, a corrupt
    /// `addr_kind` normalises to random, and a blob below the version floor is rejected, so the
    /// host falls back to defaults rather than upgrading in place.
    #[test]
    fn saved_sensors_decode_tolerances_and_migration() {
        let s = Settings {
            saved_sensors: [
                SavedSensor::saved(1, [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]), // HR, random
                SavedSensor::EMPTY,                                          // Power, none
                SavedSensor::saved(0, [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]), // Cadence, public
            ],
            ..Settings::default()
        };

        // An absent slot decodes to EMPTY even if stray address bytes sit in its region.
        let mut b = encode(&s);
        let slot = off::saved_sensors + SAVED_SENSOR_LEN; // the (empty) Power slot (index 1)
        b[slot] = 0; // present = false
        b[slot + 1] = 1; // stray addr_kind
        b[slot + 2..slot + 2 + 6].copy_from_slice(&[9, 9, 9, 9, 9, 9]); // stray address
        re_stamp_crc(&mut b);
        assert_eq!(decode(&b).unwrap().saved_sensors[1], SavedSensor::EMPTY, "an absent slot ignores stray bytes");

        // An `addr_kind` past 1 normalises to random, matching how the board reads it.
        let mut b = encode(&s);
        b[off::saved_sensors + 1] = 200;
        re_stamp_crc(&mut b);
        assert_eq!(decode(&b).unwrap().saved_sensors[0].addr_kind, 1, "an out-of-range addr_kind reads as random");

        // A blob below the version floor is rejected, so the host falls back to defaults.
        let mut old = encode(&s);
        old[0] = 10;
        re_stamp_crc(&mut old);
        assert_eq!(decode(&old), None, "a v10 blob is rejected → host uses defaults, sensors empty");
    }

    /// One table over every `setting_enum!` type: each declared byte round-trips through
    /// `from_byte` at its table position, and a byte past the last discriminant clamps to the
    /// default. The macro is one implementation, so it is tested once here.
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
        check!(FindResults);
        check!(Theme);
        check!(IdleReturn);

        check!(Language);
    }

    /// The ring cycles Both → Waypoints → Map POIs → Both, and exactly one value asks for no
    /// corridor query at all.
    #[test]
    fn up_ahead_source_cycles_and_scopes_the_two_sources() {
        assert_eq!(UpAheadSource::Both.cycled(), UpAheadSource::WaypointsOnly);
        assert_eq!(UpAheadSource::WaypointsOnly.cycled(), UpAheadSource::MapPoisOnly);
        assert_eq!(UpAheadSource::MapPoisOnly.cycled(), UpAheadSource::Both, "the ring wraps");

        assert!(UpAheadSource::Both.shows_waypoints() && UpAheadSource::Both.shows_pois());
        assert!(UpAheadSource::WaypointsOnly.shows_waypoints() && !UpAheadSource::WaypointsOnly.shows_pois());
        assert!(!UpAheadSource::MapPoisOnly.shows_waypoints() && UpAheadSource::MapPoisOnly.shows_pois());
    }

    #[test]
    fn local_offset_authority_round_trips_in_reserved_byte_two() {
        let configured = Settings { local_offset_known: true, ..Settings::default() };
        let bytes = encode(&configured);
        assert_eq!(bytes[2], 1);
        assert_eq!(decode(&bytes), Some(configured));
        assert!(!decode(&encode(&Settings::default())).unwrap().local_offset_known);
    }

    #[test]
    fn idle_return_timeout_and_stepping() {
        assert_eq!(IdleReturn::S15.timeout_ms(), Some(15_000));
        assert_eq!(IdleReturn::S30.timeout_ms(), Some(30_000));
        assert_eq!(IdleReturn::M1.timeout_ms(), Some(60_000));
        assert_eq!(IdleReturn::M5.timeout_ms(), Some(300_000));
        assert_eq!(IdleReturn::Never.timeout_ms(), None, "Never disables the mechanism");

        assert_eq!(IdleReturn::S15.stepped(1), IdleReturn::S30);
        assert_eq!(IdleReturn::M5.stepped(1), IdleReturn::Never);
        assert_eq!(IdleReturn::Never.stepped(1), IdleReturn::S15, "wraps past Never");
        assert_eq!(IdleReturn::S15.stepped(-1), IdleReturn::Never, "wraps past the start");
    }

    #[test]
    fn language_stepping_and_cycling() {
        assert_eq!(Language::ALL.map(Language::article_code), obc_formats::articles::LANGUAGES);
        assert_eq!(Language::En.stepped(1), Language::De);
        assert_eq!(Language::Es.stepped(1), Language::En, "wraps past the last language");
        assert_eq!(Language::En.stepped(-1), Language::Es, "wraps past the start");
        assert_eq!(Language::En.stepped(2), Language::Fr, "multi-step flicks compound");
        assert_eq!(Language::Fr.cycled(), Language::Es);
        assert_eq!(Language::Es.cycled(), Language::En, "the press ring wraps");
        assert_eq!(Language::En.name(), "English");
        assert_eq!(Language::De.name(), "Deutsch");
        assert_eq!(Language::Fr.name(), "Français");
        assert_eq!(Language::Es.name(), "Español");
    }

    /// The device name truncates on a char boundary at the byte cap, and a corrupt stored name
    /// sanitises to factory, not garbage.
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

        let mut b = encode(&s);
        b[off::device_name + 1] = 0xFF;
        re_stamp_crc(&mut b);
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert!(got.device_name.is_empty(), "invalid UTF-8 falls back to the factory name");

        // An impossible stored length does too.
        let mut b = encode(&s);
        b[off::device_name] = 200;
        re_stamp_crc(&mut b);
        assert!(decode(&b).unwrap().device_name.is_empty());
    }

    /// An out-of-range cycle period is clamped on decode, and an unknown field discriminant is
    /// dropped rather than loaded as a garbage tile.
    #[test]
    fn codec_sanitises_stat_tail() {
        let mut s = Settings { stat_cycle_s: 9999, ..Settings::default() };
        let mut b = encode(&s);
        // Re-stamp the CRC so only the payload, not the framing, is wrong.
        b[off::stat_fields + 1] = 250;
        re_stamp_crc(&mut b);
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert!(got.stat_cycle_s <= STAT_CYCLE_MAX, "the cycle period is clamped into range");
        assert_eq!(got.stat_fields.len(), s.stat_fields.len() - 1, "the unknown discriminant is dropped");
        s.stat_fields.remove(0);
        assert_eq!(got.stat_fields.as_slice(), s.stat_fields.as_slice());
    }

    #[test]
    fn codec_round_trips_default() {
        let s = Settings::default();
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    /// A rejected blob decodes to `None`, never to a half-parsed value.
    #[test]
    fn codec_rejects_bad_blobs() {
        let mut b = encode(&Settings::default());
        b[6] ^= 0xFF; // flip a payload byte without fixing the CRC
        assert_eq!(decode(&b), None, "a CRC mismatch is rejected");
        assert_eq!(decode(&[0u8; ENCODED_LEN]), None, "a blank (all-zero) region is rejected");
        assert_eq!(decode(&[0xFF; ENCODED_LEN]), None, "an erased (all-ones) region is rejected");
        assert_eq!(decode(&encode(&Settings::default())[..ENCODED_LEN - 1]), None, "a short slice is rejected");
        let mut wrong = encode(&Settings::default());
        wrong[0] = VERSION + 1; // re-stamped below, so only the version differs
        re_stamp_crc(&mut wrong);
        assert_eq!(decode(&wrong), None, "a future version is rejected");
        let mut below = encode(&Settings::default());
        below[0] = MIN_SUPPORTED - 1;
        re_stamp_crc(&mut below);
        assert_eq!(decode(&below), None, "a version below the supported floor is rejected");
    }

    /// A miniature settings table for tail-defaulting, which the real table cannot exercise: it
    /// has exactly one supported version, so nothing in it is ever defaulted. Four rows across
    /// three versions, with hand-written blobs and hand-written expectations.
    mod mini {
        #![allow(dead_code)] // generated for every table, not exercised here

        use crate::settings::{encoded_len, SavedSensor, SENSOR_SLOTS};
        use crate::settings_table::settings_table;

        pub const VERSION: u8 = 3;
        pub const MIN_SUPPORTED: u8 = 1;
        const PAYLOAD_LEN: usize = off::END;
        pub const ENCODED_LEN: usize = encoded_len(PAYLOAD_LEN);

        // The layout in literals, as at the real declaration. The versions straddle a write line,
        // so the encoded length really is version-relative.
        const _: () = {
            assert!(payload_len(1) == 3 && encoded_len(payload_len(1)) == 16);
            assert!(payload_len(2) == 5 && encoded_len(payload_len(2)) == 16);
            assert!(payload_len(3) == 29 && encoded_len(payload_len(3)) == 32);
        };

        settings_table! {
            /// Four fields, appended one version at a time, with one composite among them.
            pub struct Mini {
                a: u8 = 7, since(1);
                b: bool = true, since(1), ble_writable;
                c: u16 = 500, since(2), range(10, 1000);
                d: [SavedSensor; SENSOR_SLOTS] = [SavedSensor::EMPTY; SENSOR_SLOTS], since(3);
            }

            pub const DEFAULT;

            pub fn adopt_ble_fields;

            fn sanitize;

            pub fn encode;

            pub fn decode;
        }

        /// Stamp the CRC of a hand-written blob over its own `plen` bytes. A hand-computed CRC
        /// would test arithmetic, not framing.
        pub fn stamped<const N: usize>(mut b: [u8; N], plen: usize) -> [u8; N] {
            let crc = crate::crc16::crc16(&b[0..plen]);
            b[plen..plen + 2].copy_from_slice(&crc.to_le_bytes());
            b
        }

        /// A v1 blob: version, `a`, `b`, its CRC, then the write line's padding.
        pub fn v1_blob() -> [u8; 16] {
            stamped([1, 42, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 3)
        }
    }

    /// A blob written by an older version decodes field-precisely on this firmware, with the fields
    /// that version did not have taking their declared defaults instead of the whole value being
    /// thrown away.
    #[test]
    fn an_older_versions_blob_decodes_with_its_tail_defaulted() {
        use mini::Mini;

        assert_eq!(
            mini::decode(&mini::v1_blob()),
            Some(Mini { a: 42, b: true, c: 500, d: [SavedSensor::EMPTY; SENSOR_SLOTS] }),
            "v1 declared a and b; c and d take their declared defaults"
        );

        let v2 = mini::stamped([2, 42, 0, 0x2C, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], 5);
        assert_eq!(
            mini::decode(&v2),
            Some(Mini { a: 42, b: false, c: 300, d: [SavedSensor::EMPTY; SENSOR_SLOTS] }),
            "v2 declared c as well; only d is defaulted"
        );

        let v3 = mini::stamped(
            [
                3, 42, 1, 0x2C, 0x01, // version · a · b · c
                1, 1, 1, 2, 3, 4, 5, 6, // slot 0: present, random address 01:02:03:04:05:06
                0, 0, 0, 0, 0, 0, 0, 0, // slot 1: absent
                1, 0, 9, 9, 9, 9, 9, 9, // slot 2: present, public address 09:…
                0, 0, 0, // CRC (stamped) and the write line's last byte
            ],
            29,
        );
        let full = Mini {
            a: 42,
            b: true,
            c: 300,
            d: [
                SavedSensor::saved(1, [1, 2, 3, 4, 5, 6]),
                SavedSensor::EMPTY,
                SavedSensor::saved(0, [9, 9, 9, 9, 9, 9]),
            ],
        };
        assert_eq!(mini::decode(&v3), Some(full), "the newest version reads every field");
        assert_eq!(mini::decode(&mini::encode(&full)), Some(full), "and round-trips through its own encode");
    }

    /// Length and CRC are relative to the stored version, not the running one. That is what makes
    /// the simulator's shorter file and the board's longer fixed-`SLOT_LEN` read both work after a
    /// version bump, without weakening any rejection.
    #[test]
    fn the_framing_checks_follow_the_stored_version() {
        let v1 = mini::v1_blob();
        assert!(mini::decode(&v1).is_some(), "the blob these cases are built from is valid");

        assert_eq!(mini::decode(&v1[..15]), None, "a byte short of its own encoded length → rejected");

        let mut padded = [0xAA; mini::ENCODED_LEN];
        padded[..16].copy_from_slice(&v1);
        assert_eq!(mini::decode(&padded), mini::decode(&v1), "bytes past its encoded length are ignored");

        let mut wrong = [0u8; mini::ENCODED_LEN];
        wrong[..3].copy_from_slice(&v1[..3]);
        assert_eq!(
            mini::decode(&mini::stamped(wrong, 29)),
            None,
            "a v1 blob whose CRC covers the current version's payload is rejected"
        );

        let mut future = v1;
        future[0] = mini::VERSION + 1;
        assert_eq!(mini::decode(&mini::stamped(future, 3)), None, "a newer version is still rejected");
        let mut ancient = v1;
        ancient[0] = mini::MIN_SUPPORTED - 1;
        assert_eq!(mini::decode(&mini::stamped(ancient, 3)), None, "a version below the floor is still rejected");
    }

    #[test]
    fn decode_sanitises_out_of_range_fields() {
        let mut s = Settings::default();
        s.clock.month = 13;
        s.clock.day = 99;
        s.fix_interval_s = 9999;
        // `encode` stamps a correct CRC over the bogus-but-in-layout payload, so the blob is valid
        // and `decode` must accept it and sanitise.
        let b = encode(&s);
        let got = decode(&b).expect("valid CRC → Some, just sanitised");
        assert!((1..=12).contains(&got.clock.month));
        assert!(got.clock.day >= 1 && got.clock.day <= 31);
        assert!(got.fix_interval_s <= FIX_INTERVAL_MAX);
    }

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

    /// `add_minutes` carries across every boundary a live app clock advances through: minute, hour,
    /// day, month, year, and the leap day.
    #[test]
    fn datetime_add_minutes_carries_across_fields() {
        let base = DateTime { year: 2025, month: 6, day: 29, hour: 14, minute: 40 };
        assert_eq!(base.add_minutes(5).minute, 45);
        let h = base.add_minutes(25);
        assert_eq!((h.hour, h.minute), (15, 5));
        let midnight = DateTime { year: 2025, month: 6, day: 29, hour: 23, minute: 59 };
        let d = midnight.add_minutes(1);
        assert_eq!((d.day, d.hour, d.minute), (30, 0, 0), "23:59 + 1 rolls into the next day");
        let m = DateTime { year: 2025, month: 6, day: 30, hour: 23, minute: 0 }.add_minutes(120);
        assert_eq!((m.month, m.day, m.hour), (7, 1, 1), "end of June rolls into July");
        let y = DateTime { year: 2025, month: 12, day: 31, hour: 23, minute: 59 }.add_minutes(1);
        assert_eq!((y.year, y.month, y.day, y.hour, y.minute), (2026, 1, 1, 0, 0), "new year");
        assert_eq!(base.add_minutes(0), base, "a zero advance changes nothing");
    }

    /// Long advances land where an independent calendar (`datetime.timedelta`) puts them — the
    /// cases that separate a correct bulk carry from a day-at-a-time walk that loses a leap day.
    #[test]
    fn datetime_add_minutes_matches_reference_over_long_advances() {
        let f = |d: DateTime| (d.year, d.month, d.day, d.hour, d.minute);
        let a = DateTime { year: 2025, month: 1, day: 1, hour: 0, minute: 0 }.add_minutes(400 * 24 * 60);
        assert_eq!(f(a), (2026, 2, 5, 0, 0), "400 days from 2025-01-01");
        let b = DateTime { year: 2024, month: 2, day: 29, hour: 12, minute: 0 }.add_minutes(366 * 24 * 60);
        assert_eq!(f(b), (2025, 3, 1, 12, 0), "366 days from the 2024 leap day");
        let c = DateTime { year: 2025, month: 6, day: 29, hour: 14, minute: 40 }.add_minutes(1_000_000);
        assert_eq!(f(c), (2027, 5, 25, 1, 20), "a million minutes");
        let d = DateTime { year: 2100, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!(f(d), (2100, 3, 1, 0, 0), "2100 is div-by-100-not-400: no Feb 29");
        let e = DateTime { year: 2000, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!(f(e), (2000, 2, 29, 0, 0), "2000 is div-by-400: Feb 29 exists");
        let base = DateTime { year: 2025, month: 6, day: 29, hour: 14, minute: 40 };
        assert_eq!(base.add_minutes(5_000).add_minutes(7_777), base.add_minutes(12_777), "split == whole");
    }

    /// February's length is taken from the year the advance lands in.
    #[test]
    fn datetime_add_minutes_is_leap_aware() {
        let leap = DateTime { year: 2024, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!((leap.month, leap.day), (2, 29), "2024 has a Feb 29 to land on");
        let common = DateTime { year: 2025, month: 2, day: 28, hour: 0, minute: 0 }.add_minutes(24 * 60);
        assert_eq!((common.month, common.day), (3, 1), "2025 skips straight to March");
        let across = DateTime { year: 2024, month: 2, day: 27, hour: 0, minute: 0 }.add_minutes(3 * 24 * 60);
        assert_eq!((across.month, across.day), (3, 1), "the leap day is one of the three crossed");
    }

    /// The GPS UTC-anchor to local-time conversion: a signed minute offset rolls the date in
    /// either direction when the shift crosses midnight.
    #[test]
    fn datetime_with_offset_rolls_the_date_both_ways() {
        let base = DateTime { year: 2025, month: 6, day: 29, hour: 23, minute: 30 };
        assert_eq!(base.with_offset(0), base, "a zero offset is identity");
        let within = base.with_offset(15);
        assert_eq!((within.day, within.hour, within.minute), (29, 23, 45));
        let next = base.with_offset(60);
        assert_eq!((next.day, next.hour, next.minute), (30, 0, 30), "forward across midnight rolls the day");
        let early = DateTime { year: 2025, month: 6, day: 29, hour: 0, minute: 30 };
        let prev = early.with_offset(-45);
        assert_eq!((prev.day, prev.hour, prev.minute), (28, 23, 45), "backward across midnight rolls back");
        let month_edge = DateTime { year: 2025, month: 7, day: 1, hour: 0, minute: 0 };
        let back = month_edge.with_offset(-60);
        assert_eq!((back.month, back.day, back.hour), (6, 30, 23), "the borrow steps into June (30 days)");
    }

    /// The offset's hard edges: year and leap-day boundaries, and the widest real zones.
    #[test]
    fn datetime_with_offset_crosses_year_and_leap_boundaries() {
        let f = |d: DateTime| (d.year, d.month, d.day, d.hour, d.minute);
        let ny = DateTime { year: 2025, month: 1, day: 1, hour: 0, minute: 15 }.with_offset(-30);
        assert_eq!(f(ny), (2024, 12, 31, 23, 45), "New Year's Day − 30 min is New Year's Eve");
        let leap = DateTime { year: 2024, month: 3, day: 1, hour: 0, minute: 30 }.with_offset(-60);
        assert_eq!(f(leap), (2024, 2, 29, 23, 30), "March 1 2024 borrows from Feb 29");
        let common = DateTime { year: 2025, month: 3, day: 1, hour: 0, minute: 0 }.with_offset(-1);
        assert_eq!(f(common), (2025, 2, 28, 23, 59), "March 1 2025 borrows from Feb 28");
        let nye = DateTime { year: 2025, month: 12, day: 31, hour: 23, minute: 59 }.with_offset(1);
        assert_eq!(f(nye), (2026, 1, 1, 0, 0), "New Year's Eve + 1 min is New Year's Day");
        let mid = DateTime { year: 2025, month: 6, day: 29, hour: 10, minute: 0 };
        assert_eq!(f(mid.with_offset(14 * 60)), (2025, 6, 30, 0, 0), "UTC+14 pushes into tomorrow");
        assert_eq!(f(mid.with_offset(-12 * 60)), (2025, 6, 28, 22, 0), "UTC−12 pulls into yesterday");
        for offset in [1i16, -1, 60, -60, 840, -720, 1439, -1439] {
            assert_eq!(mid.with_offset(offset).with_offset(-offset), mid, "round trip at {offset}");
            assert_eq!(ny.with_offset(offset).with_offset(-offset), ny, "round trip at {offset} from {ny:?}");
        }
    }

    /// `add_minutes` is defensive against an unsanitised stamp: a day past the month length must
    /// not underflow the unsigned day-walk.
    #[test]
    fn add_minutes_guards_bad_input_and_saturates_the_year() {
        let bad = DateTime { year: 2025, month: 6, day: 99, hour: 0, minute: 0 };
        assert!((1..=30).contains(&bad.add_minutes(0).day), "an over-long day is re-pinned into the month");
        let near_max = DateTime { year: DATETIME_MAX_YEAR, month: 12, day: 31, hour: 12, minute: 0 };
        let sat = add_minutes_bounded(near_max, 2 * 365 * 24 * 60);
        assert_eq!(sat.year, DATETIME_MAX_YEAR, "the app clock never climbs past its maximum year");
        assert_eq!((sat.month, sat.day), (12, 31), "it saturates at Dec 31 rather than rolling over");
    }

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

    /// `to_unix` against references computed independently with `date -u +%s`.
    #[test]
    fn to_unix_matches_reference_timestamps() {
        let dt = |year, month, day, hour, minute| DateTime { year, month, day, hour, minute };
        assert_eq!(dt(2020, 1, 1, 0, 0).to_unix(), 1_577_836_800);
        assert_eq!(dt(2024, 2, 29, 12, 30).to_unix(), 1_709_209_800, "leap day");
        assert_eq!(dt(2026, 7, 2, 9, 33).to_unix(), 1_782_984_780);
        assert_eq!(dt(2026, 12, 31, 23, 59).to_unix(), 1_798_761_540, "year boundary");
        assert_eq!(dt(2099, 12, 31, 23, 59).to_unix(), 4_102_444_740, "the top of the range fits u32");
    }

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

    // The board firmware keeps a live `App` copy while the RRAM blob remains the source of truth.
    // These tests model that store with the codec's own byte buffer and exercise the coherence
    // operations around BLE writes and ride-loop saves.

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
            // Wrong to adopt: the phone never writes these.
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

    /// Phone to device: a BLE Config write lands, then the app runs its change-detection save.
    /// With the reload before that save, the phone's values survive.
    #[test]
    fn ble_write_then_app_save_keeps_ble_values() {
        let boot = Settings::default();
        let mut store = FakeStore::new(&boot);
        let mut app = boot;
        app.fix_interval_s = 9; // a pending on-device edit
        app.ble_enabled = false; // and a pending device-only toggle

        // The phone writes units and a rename into the object-store cache and RRAM.
        let mut objstore = store.load();
        objstore.adopt_ble_fields(&Settings {
            units: Units::Imperial,
            device_name: DeviceName::from_str_lossy("Ridgeline"),
            ..Settings::default()
        });
        store.save(&objstore);

        // The ride loop reloads BLE fields into the app copy before its change-detection save.
        app.adopt_ble_fields(&store.load());
        store.save(&app);

        let persisted = store.load();
        assert_eq!(persisted.units, Units::Imperial, "the phone's units survive the app save (no clobber)");
        assert_eq!(persisted.device_name.as_str(), "Ridgeline", "the phone's rename survives too");
        assert_eq!(persisted.fix_interval_s, 9, "the app's own device-only edit still persists");
        assert!(!persisted.ble_enabled, "the radio-off toggle survives the coherence path (device-only)");
    }

    /// Without the reload, the app's stale copy overwrites the phone's write. Pinned so a refactor
    /// that drops the reload fails here.
    #[test]
    fn app_save_without_reload_would_clobber() {
        let mut store = FakeStore::new(&Settings::default());
        let app = Settings::default();

        let mut objstore = store.load();
        objstore.adopt_ble_fields(&Settings {
            units: Units::Imperial,
            device_name: DeviceName::from_str_lossy("Ridgeline"),
            ..Settings::default()
        });
        store.save(&objstore);

        // The app saves its stale copy without the reload.
        store.save(&app);
        let clobbered = store.load();
        assert_eq!(clobbered.units, Units::Metric, "the bug: the app's stale metric clobbers the phone's imperial");
        assert!(clobbered.device_name.is_empty(), "and the phone's rename is lost");
    }

    /// Device to phone: units change on-device, then a Config read must serve fresh values.
    #[test]
    fn app_change_then_ble_read_serves_fresh() {
        let boot = Settings::default();
        let mut store = FakeStore::new(&boot);
        // The BLE object-store cache, seeded at boot.
        let mut objstore_cache = store.load();
        assert_eq!(objstore_cache.units, Units::Metric);

        // The rider flips to imperial and the ride loop persists the app copy to RRAM.
        let mut app = boot;
        app.units = Units::Imperial;
        store.save(&app);

        // The object store refreshes its cache from RRAM before serving the read.
        objstore_cache = store.load();
        assert_eq!(objstore_cache.units, Units::Imperial, "the Config read serves the on-device change, no reboot");
    }
}

// The settings domain protocol. [`SettingsMachine`] owns the dirty revision, the debounce, the
// retry and the stale-ack rule. The platform executor writes one revision and says what happened;
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

/// Persist the current settings revision. Values are read from resident settings at execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsEffect {
    /// Write the live settings to durable storage as `revision`. The values are read from the
    /// resident [`Settings`] at execution; no settings copy ever rides the effect.
    PersistRevision { token: OperationToken<SettingsTag>, revision: u16 },
}

impl SettingsEffect {
    pub fn token(&self) -> OperationToken<SettingsTag> {
        match self {
            SettingsEffect::PersistRevision { token, .. } => *token,
        }
    }
}

/// The result of one [`SettingsEffect`]. The typed failure reuses
/// [`SettingsSaveError`](obc_ports::SettingsSaveError), because the port already names every way a
/// settings write can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsOutcome {
    /// `revision` reached durable storage.
    Persisted { token: OperationToken<SettingsTag>, revision: u16 },
    /// The write for `revision` failed; the value stays live in RAM and the revision stays owed.
    PersistFailed { token: OperationToken<SettingsTag>, revision: u16, error: obc_ports::SettingsSaveError },
    /// The executor abandoned the write. A platform with no durable store says so here instead of
    /// leaving the handshake parked forever.
    Cancelled { token: OperationToken<SettingsTag> },
}

impl SettingsOutcome {
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

/// Bounded backoff before a failed settings persist may re-emit, in map-plane millis. The retry
/// only re-emits on a frame that runs for another reason, so it never schedules an idle wake.
pub(crate) const SETTINGS_RETRY_BACKOFF_MS: u32 = 2_000;

/// Wrap-safe "deadline reached" in the backoff's u16 millisecond space: true while `now` sits in
/// the half-window at or past `deadline`. The u16 domain wraps every 65.5 s, so a frame gap longer
/// than about 32.7 s can slide a due retry by up to one wrap. That is the price of keeping the
/// deadline to two resident bytes.
fn retry_deadline_reached(now: u16, deadline: u16) -> bool {
    now.wrapping_sub(deadline) < 0x8000
}

/// Where the settings-persistence handshake is. Fieldless, so it packs into an existing padding
/// hole: the Backoff deadline lives in the sibling [`SettingsMachine::retry_at_ms`] field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PersistState {
    /// The live settings are persisted at the current revision.
    #[default]
    Clean,
    /// A save is owed once the rider leaves the settings subtree.
    Dirty,
    /// A write was emitted for the current revision and awaits its answer.
    Awaiting,
    /// The last write failed; no retry re-emits before the deadline.
    Backoff,
}

/// The settings domain's persistence machine: the dirty revision, the subtree debounce, the retry
/// backoff and the stale-answer rule.
///
/// Editing is live in RAM the instant it happens; persisting it is an acknowledged, retryable
/// conversation keyed by the monotonic [`revision`](SettingsMachine::revision). An in-flight write
/// is never re-emitted, so an executor that takes the write and never answers parks instead of
/// spamming the store, while edits stay live in RAM and keep superseding. An answer is honoured
/// only while its revision is still the current one.
#[derive(Debug, Default)]
pub(crate) struct SettingsMachine {
    ops: crate::device_core::TokenSource<crate::device_core::SettingsTag>,
    /// Bumped by every edit whose before/after compare finds a change. Re-zeroed when the boot
    /// value is seeded.
    revision: u16,
    /// The Backoff retry deadline: the low 16 bits of map-plane millis.
    retry_at_ms: u16,
    persist: PersistState,
}

impl SettingsMachine {
    /// The boot state: Clean at revision 0, because the boot value came from the store or the
    /// default.
    pub(crate) const fn new() -> Self {
        SettingsMachine {
            ops: crate::device_core::TokenSource::new(),
            revision: 0,
            retry_at_ms: 0,
            persist: PersistState::Clean,
        }
    }

    /// Admit one settings intent. A [`Changed`](SettingsIntent::Changed) from any prior state
    /// supersedes an in-flight or backing-off older revision: the new content re-emits, and the
    /// older answer no longer matches when it lands.
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

    /// The boot value was seeded from the store or the default, so it is already persisted: reset
    /// to Clean at revision 0. A pending edit is discarded, because seeding is not a rider edit.
    pub(crate) fn note_seeded(&mut self) {
        self.revision = 0;
        self.persist = PersistState::Clean;
    }

    /// Whether a write is owed and may be emitted now: the value is dirty, the rider has left the
    /// settings subtree, and no answer or backoff window is outstanding.
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

    /// Offer a write after editing ends. A matching success clears the dirty state.
    pub(crate) fn next_effect(&mut self, in_settings_subtree: bool, now_ms: u32) -> Option<SettingsEffect> {
        if !self.wants_write(in_settings_subtree, now_ms) {
            return None;
        }
        self.persist = PersistState::Awaiting;
        let (token, revision) = (self.ops.issue(), self.revision);
        Some(SettingsEffect::PersistRevision { token, revision })
    }

    /// Consume the answer to a write. Returns `true` when the write failed and the rider must be
    /// told, which is the one part of this the domain cannot do itself.
    ///
    /// The two guards are independent: the token rejects a superseded operation, the revision
    /// rejects a stale value. A stale answer leaves the newer content pending either way.
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
            // The value stays dirty and retryable; nothing is claimed to have been written.
            SettingsOutcome::Cancelled { .. } => {
                if self.persist == PersistState::Awaiting {
                    self.persist = PersistState::Dirty;
                }
                false
            }
        }
    }

    /// `revision` reached durable storage. Clear to Clean only while it is still the latest: a
    /// newer edit has already moved the machine back to Dirty, and that content stays pending.
    pub(crate) fn note_persisted(&mut self, revision: u16) {
        if self.persist == PersistState::Awaiting && revision == self.revision {
            self.persist = PersistState::Clean;
        }
    }

    /// The write for `revision` failed. Keep it dirty and re-arm the bounded backoff, but only
    /// while it is still the in-flight latest.
    ///
    /// Always returns `true`: a write did fail, so the rider is told whatever the revision guard
    /// says. The guard decides only whether that revision stays retryable.
    pub(crate) fn note_persist_failed(&mut self, revision: u16, now_ms: u32) -> bool {
        if self.persist == PersistState::Awaiting && revision == self.revision {
            self.retry_at_ms = (now_ms as u16).wrapping_add(SETTINGS_RETRY_BACKOFF_MS as u16);
            self.persist = PersistState::Backoff;
        }
        true
    }

    /// Test hook: arm a pending save without driving a real edit.
    #[cfg(test)]
    pub(crate) fn arm_save(&mut self) {
        self.note_edited();
    }

    /// Whether nothing is owed: Clean at revision 0. The destructure is exhaustive, so a field
    /// added here must state its empty value too.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        let SettingsMachine { ops, revision, retry_at_ms, persist } = self;
        format!("{ops:?}") == "TokenSource(0)" && *revision == 0 && *retry_at_ms == 0 && *persist == PersistState::Clean
    }
}

// Layout tripwire: a revision, a deadline, a phase and a generation, never a `Settings`.
const _: () = assert!(core::mem::size_of::<SettingsMachine>() <= 12, "the handshake, not the values");

#[cfg(test)]
mod settings_machine_tests {
    use super::*;
    use obc_ports::SettingsSaveError;

    /// The token a write went out under, so a test can answer the operation the machine holds.
    fn emit(
        machine: &mut SettingsMachine,
        now_ms: u32,
    ) -> (crate::device_core::OperationToken<crate::device_core::SettingsTag>, u16) {
        match machine.next_effect(false, now_ms).expect("a write is owed") {
            SettingsEffect::PersistRevision { token, revision } => (token, revision),
        }
    }

    /// The debounce: nothing is written while the rider is mid-edit inside the settings subtree,
    /// and exactly one write goes out when they leave.
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

    /// A stale ack, for a revision a newer edit has already superseded, must not clear the newer
    /// content. The revision is the guard, checked independently of the token.
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

    #[test]
    fn a_failed_write_backs_off_and_retries_once() {
        let mut machine = SettingsMachine::new();
        machine.note_edited();
        let (_, revision) = emit(&mut machine, 1_000);

        machine.note_persist_failed(revision, 1_000);
        assert!(!machine.wants_write(false, 1_000 + SETTINGS_RETRY_BACKOFF_MS - 1), "not before the window");
        assert!(machine.wants_write(false, 1_000 + SETTINGS_RETRY_BACKOFF_MS), "and exactly once at it");

        let (_, retried) = emit(&mut machine, 1_000 + SETTINGS_RETRY_BACKOFF_MS);
        assert_eq!(retried, revision, "the same content, not a new one");
        assert!(machine.next_effect(false, 1_000 + 4 * SETTINGS_RETRY_BACKOFF_MS).is_none(), "one retry in flight");
    }

    /// A platform that takes the write and never answers parks by design: edits stay live in RAM
    /// and keep superseding, and nothing re-emits into a store that will not answer.
    #[test]
    fn an_executor_that_never_answers_parks_without_re_emitting() {
        let mut machine = SettingsMachine::new();
        machine.note_edited();
        emit(&mut machine, 100);
        for ms in [200, 10_000, 100_000, 1_000_000] {
            assert!(machine.next_effect(false, ms).is_none(), "no RRAM spam under a silent executor");
        }

        // A `Cancelled` answer is how such a platform says so: the value stays dirty and retryable.
        machine.note_edited();
        let (token, _) = emit(&mut machine, 2_000_000);
        assert!(!machine.apply_outcome(SettingsOutcome::Cancelled { token }, 2_000_000));
        assert!(machine.wants_write(false, 2_000_001), "the write is owed again");
    }

    /// The rider is told a save failed even when the failure is for a superseded revision. A stale
    /// failure re-arms nothing, so the newer content is owed at once rather than after a backoff.
    #[test]
    fn a_stale_failure_is_still_shown_but_re_arms_nothing() {
        let mut machine = SettingsMachine::new();
        machine.note_edited(); // revision 1
        emit(&mut machine, 100);
        machine.note_edited(); // revision 2 supersedes it

        assert!(machine.note_persist_failed(1, 100), "the rider is told a save failed");
        assert!(machine.wants_write(false, 100), "but revision 2 is owed now, not after a backoff");
        assert_eq!(emit(&mut machine, 100).1, 2);
    }

    /// An answer to a superseded operation is refused on its token, before its revision is looked
    /// at.
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
