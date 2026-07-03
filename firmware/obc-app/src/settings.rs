//! Persistent device settings + their byte codec.
//!
//! [`Settings`] is the small POD the settings screens edit and the host persists across a reboot.
//! It is `Copy + PartialEq`, so [`App::apply_gesture`](crate::App::apply_gesture) detects a change
//! with a single comparison and flags a save. The byte codec ([`encode`]/[`decode`]) is a
//! versioned, CRC-checked, fixed-length blob shared by **both** stores (sim file, firmware RRAM
//! region — see [`SettingsStore`](crate::hal::SettingsStore)), so a blank or corrupt read falls
//! back to [`Settings::default`] rather than loading garbage.

use crate::stat_fields::{StatFieldList, MAX_STAT_FIELDS};

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

    /// The label for the Units screen's value row (`Metric` / `Imperial`).
    #[inline]
    pub const fn name(self) -> &'static str {
        if self.is_imperial() {
            "Imperial"
        } else {
            "Metric"
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

/// Wrap `v` by `n` steps within the inclusive range `lo..=hi`. Shared by every
/// [`DateTime`] stepper so a turn past either end rolls round (year 2099→2020, hour 23→0),
/// matching the list selection's [`step_selection`](crate::screen::step_selection) feel.
fn wrap_inclusive(v: u16, n: i32, lo: u16, hi: u16) -> u16 {
    let span = (hi - lo) as i32 + 1;
    let off = (v as i32 - lo as i32 + n).rem_euclid(span);
    (lo as i32 + off) as u16
}

/// A wall-clock date + time of day (no seconds — the device only ever sets to the minute).
/// Edited field-by-field on the Date & Time screen; each stepper wraps within its field's
/// range and re-clamps the day to the month length (so Jan 31 → Feb shows Feb 28/29, never
/// Feb 31). Stored in [`Settings`] so a manually-set time survives a reboot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    /// 1–12.
    pub month: u8,
    /// 1–`month_len`.
    pub day: u8,
    /// 0–23.
    pub hour: u8,
    /// 0–59.
    pub minute: u8,
}

impl Default for DateTime {
    /// A neutral in-range stamp; the real time comes from the user or (later) the GPS.
    fn default() -> Self {
        DateTime { year: 2025, month: 1, day: 1, hour: 12, minute: 0 }
    }
}

impl DateTime {
    pub const MIN_YEAR: u16 = 2020;
    pub const MAX_YEAR: u16 = 2099;

    const MONTHS: [&'static str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

    /// Gregorian leap-year test (the Feb-length input to [`month_len`](DateTime::month_len)).
    pub const fn is_leap(year: u16) -> bool {
        (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
    }

    /// Days in `month` (1–12) of `year` — leap-aware for February.
    pub const fn month_len(year: u16, month: u8) -> u8 {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if Self::is_leap(year) => 29,
            2 => 28,
            _ => 30, // unreachable for a sanitised value; a safe fallback
        }
    }

    /// The three-letter month name (`Jan`…`Dec`).
    pub fn month_name(self) -> &'static str {
        Self::MONTHS[(self.month.clamp(1, 12) - 1) as usize]
    }

    /// Re-pin the day inside the current month after a month/year change (Jan 31 → Feb 28/29).
    fn clamp_day(&mut self) {
        let max = Self::month_len(self.year, self.month);
        self.day = self.day.clamp(1, max);
    }

    /// Force every field into its valid range — applied after a decode so a valid-CRC but
    /// out-of-range blob (an older writer, a bit-flip the CRC missed) can't show "month 0".
    fn sanitize(&mut self) {
        self.year = self.year.clamp(Self::MIN_YEAR, Self::MAX_YEAR);
        self.month = self.month.clamp(1, 12);
        self.hour = self.hour.min(23);
        self.minute = self.minute.min(59);
        self.clamp_day();
    }

    /// Step the year by `n` (wrapping 2020–2099), re-clamping the day for the new year's Feb.
    pub fn step_year(&mut self, n: i32) {
        self.year = wrap_inclusive(self.year, n, Self::MIN_YEAR, Self::MAX_YEAR);
        self.clamp_day();
    }

    /// Step the month by `n` (wrapping 1–12), re-clamping the day to the new month's length.
    pub fn step_month(&mut self, n: i32) {
        self.month = wrap_inclusive(self.month as u16, n, 1, 12) as u8;
        self.clamp_day();
    }

    /// Step the day by `n`, wrapping within the current month's length.
    pub fn step_day(&mut self, n: i32) {
        let max = Self::month_len(self.year, self.month);
        self.day = wrap_inclusive(self.day as u16, n, 1, max as u16) as u8;
    }

    /// Step the hour by `n`, wrapping 0–23.
    pub fn step_hour(&mut self, n: i32) {
        self.hour = wrap_inclusive(self.hour as u16, n, 0, 23) as u8;
    }

    /// Step the minute by `n`, wrapping 0–59.
    pub fn step_minute(&mut self, n: i32) {
        self.minute = wrap_inclusive(self.minute as u16, n, 0, 59) as u8;
    }

    /// The next calendar day — the day → month → year carry, leap-aware via
    /// [`month_len`](DateTime::month_len), saturating at Dec 31 [`MAX_YEAR`](DateTime::MAX_YEAR)
    /// rather than rolling to year 2100+. Shared by the forward clock advance
    /// ([`add_minutes`](DateTime::add_minutes)) and the positive-offset side of
    /// [`with_offset`](DateTime::with_offset).
    const fn next_day(self) -> DateTime {
        let mut dt = self;
        if dt.day < Self::month_len(dt.year, dt.month) {
            dt.day += 1;
        } else if dt.month < 12 {
            dt.month += 1;
            dt.day = 1;
        } else if dt.year < Self::MAX_YEAR {
            dt.year += 1;
            dt.month = 1;
            dt.day = 1;
        } // else already the last representable day — saturate (stay put).
        dt
    }

    /// The previous calendar day — the symmetric day → month → year *borrow*, saturating at Jan 1
    /// [`MIN_YEAR`](DateTime::MIN_YEAR). The negative-offset side of
    /// [`with_offset`](DateTime::with_offset).
    const fn prev_day(self) -> DateTime {
        let mut dt = self;
        if dt.day > 1 {
            dt.day -= 1;
        } else if dt.month > 1 {
            dt.month -= 1;
            dt.day = Self::month_len(dt.year, dt.month);
        } else if dt.year > Self::MIN_YEAR {
            dt.year -= 1;
            dt.month = 12;
            dt.day = 31;
        } // else already the first representable day — saturate (stay put).
        dt
    }

    /// Advance the stamp **forward** by `mins` minutes, carrying minute → hour → day → month → year
    /// (leap-aware via [`month_len`](DateTime::month_len)). Unlike the field steppers (which wrap
    /// within one field for the editor) this is a real clock advance: 23:59 + 1 rolls into the next
    /// day. Pure, forward-only, saturating at `MAX_YEAR` — the [`WallClock`](crate::WallClock) only
    /// ever advances its set-point by elapsed monotonic time.
    pub fn add_minutes(self, mins: u32) -> DateTime {
        let mut dt = self;
        // Defensive re-pin: `next_day` assumes an in-range date. An unsanitised stamp (a future raw
        // GPS-fix path) could carry a day past the month length and panic, so clamp before walking.
        dt.month = dt.month.clamp(1, 12);
        dt.day = dt.day.clamp(1, Self::month_len(dt.year, dt.month));
        let total_min = dt.minute as u32 + mins;
        dt.minute = (total_min % 60) as u8;
        let total_hour = dt.hour as u32 + total_min / 60;
        dt.hour = (total_hour % 24) as u8;
        let mut days = total_hour / 24;
        while days > 0 {
            dt = dt.next_day();
            days -= 1;
        }
        dt
    }

    /// This stamp shifted by a signed minute `offset` (a UTC offset), rolling the **date** across
    /// midnight in *either* direction — the GPS UTC-anchor → local-time conversion. A positive
    /// offset steps [`next_day`](DateTime::next_day), a negative one
    /// [`prev_day`](DateTime::prev_day); the loops run at most once for a real ±14 h zone (and stay
    /// bounded for any `i16`). Unlike [`add_minutes`](DateTime::add_minutes) this goes both ways,
    /// since an offset can move the local clock earlier than the anchor.
    pub fn with_offset(self, offset: i16) -> DateTime {
        // Total minutes since the anchor's midnight, ± the offset; split into a whole-day carry and
        // the local time-of-day so the date rolls and the clock re-pins independently.
        let tod = self.hour as i32 * 60 + self.minute as i32 + offset as i32;
        let local_tod = tod.rem_euclid(24 * 60);
        let mut day_shift = tod.div_euclid(24 * 60);
        let mut date = DateTime { hour: 0, minute: 0, ..self };
        while day_shift > 0 {
            date = date.next_day();
            day_shift -= 1;
        }
        while day_shift < 0 {
            date = date.prev_day();
            day_shift += 1;
        }
        // Re-pin the local time-of-day: a sub-day `add_minutes` only sets HH:MM, never re-rolling.
        date.add_minutes(local_tod as u32)
    }

    /// Unix seconds at this stamp's `HH:MM:00`, reading the stamp **as UTC** — the caller owns any
    /// zone shift (see [`App::wall_unix_now`](crate::App::wall_unix_now)). Days-from-civil (standard
    /// Gregorian era arithmetic); the whole 2020–2099 range fits a `u32` (Dec 31 2099 ≈ 4.10 × 10⁹).
    pub fn to_unix(self) -> u32 {
        // Shift Jan/Feb to the tail of the previous year so the leap day ends the "March year".
        let y = self.year as i64 - (self.month <= 2) as i64;
        let era = y.div_euclid(400);
        let yoe = y - era * 400; // year of era: 0..=399
        let m = self.month as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + self.day as i64 - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // day of era: 0..=146096
        let days = era * 146_097 + doe - 719_468; // days since 1970-01-01
        (days * 86_400 + self.hour as i64 * 3_600 + self.minute as i64 * 60) as u32
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
        }
    }
}

impl Settings {
    /// The **local** wall-clock set-point the device shows: [`clock`](Settings::clock) verbatim in
    /// manual mode, or — when GPS-stamped ([`gps_time`](Settings::gps_time)) — the UTC anchor
    /// shifted into local time by [`utc_offset_min`](Settings::utc_offset_min) (via
    /// [`DateTime::with_offset`], so a shift across midnight rolls the date too). In manual mode the
    /// clock is already local, so the offset is deliberately *not* applied (it would double-count).
    pub fn local_clock(&self) -> DateTime {
        if self.gps_time {
            self.clock.with_offset(self.utc_offset_min)
        } else {
            self.clock
        }
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
pub const VERSION: u8 = 3;

/// Fixed encoded length: the [`PAYLOAD_LEN`] CRC-covered bytes + a 2-byte CRC, **rounded up to the
/// device RRAM's 16-byte write line** (the firmware store writes whole 128-bit lines) — so a codec
/// bump never needs the device store re-padded, the RRAM store reads a known span, and the file
/// store needs no length framing. Bytes past the CRC are unused zero padding.
pub const ENCODED_LEN: usize = (PAYLOAD_LEN + 2).div_ceil(16) * 16;

/// Payload size before the trailing CRC. The CRC follows immediately at this offset.
const PAYLOAD_LEN: usize = NAME_OFF + 1 + DEVICE_NAME_MAX;
/// Byte offset of the field selection (right after the 14-byte head).
const STAT_FIELDS_OFF: usize = 14;
/// Byte offset of `stat_cycle_s` (right after the field selection).
const STAT_CYCLE_OFF: usize = STAT_FIELDS_OFF + 1 + MAX_STAT_FIELDS;
/// Byte offset of the device name (right after `stat_cycle_s`).
const NAME_OFF: usize = STAT_CYCLE_OFF + 2;

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
    };
    s.sanitize();
    Some(s)
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
        };
        assert_eq!(decode(&encode(&s)), Some(s));
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
        let crc = crc16(&wrong[0..14]);
        wrong[14..16].copy_from_slice(&crc.to_le_bytes());
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
        let mut d = DateTime { year: DateTime::MAX_YEAR, month: 12, day: 30, hour: 23, minute: 59 };
        d.step_year(1);
        assert_eq!(d.year, DateTime::MIN_YEAR, "year wraps 2099 → 2020");
        d.step_month(1);
        assert_eq!(d.month, 1, "month wraps 12 → 1");
        d.step_hour(1);
        assert_eq!(d.hour, 0, "hour wraps 23 → 0");
        d.step_minute(1);
        assert_eq!(d.minute, 0, "minute wraps 59 → 0");
        d.step_year(-1);
        assert_eq!(d.year, DateTime::MAX_YEAR, "and backward off the bottom wraps to the top");
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
        let near_max = DateTime { year: DateTime::MAX_YEAR, month: 12, day: 31, hour: 12, minute: 0 };
        let sat = near_max.add_minutes(2 * 365 * 24 * 60);
        assert_eq!(sat.year, DateTime::MAX_YEAR, "the year never climbs past MAX_YEAR");
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
}
