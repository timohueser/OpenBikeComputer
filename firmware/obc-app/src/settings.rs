//! Persistent device settings + their byte codec.
//!
//! [`Settings`] is the small POD the settings screens edit and the host persists across a
//! reboot. It is `Copy + PartialEq` like [`AppState`](crate::AppState), so
//! [`App::apply_gesture`](crate::App::apply_gesture) detects a change with a single
//! comparison and flags a save — the same before/after trick `tick` already uses on the
//! camera state. The byte codec ([`encode`]/[`decode`]) is a versioned, CRC-checked,
//! fixed-length blob shared by **both** stores — the simulator writes it to a file, the
//! firmware to a reserved RRAM region (see [`SettingsStore`](crate::hal::SettingsStore)) — so
//! a blank or corrupt read falls back to [`Settings::default`] rather than loading garbage.

/// Measurement system for the ride readouts. The one setting with reach beyond the
/// settings screens: it re-captions and re-scales the [`Statistics`](crate::screen) tiles and
/// the off-route distance ([`write_off_route`](crate::screen::write_off_route)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Units {
    /// km / km·h⁻¹ / m — the default.
    #[default]
    Metric,
    /// mi / mi·h⁻¹ / ft.
    Imperial,
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
        }
    }
}

impl Settings {
    /// Clamp every field into its valid range — applied after a decode (see [`decode`]).
    fn sanitize(&mut self) {
        self.clock.sanitize();
        self.utc_offset_min = self.utc_offset_min.clamp(UTC_OFFSET_MIN, UTC_OFFSET_MAX);
        self.fix_interval_s = self.fix_interval_s.clamp(FIX_INTERVAL_MIN, FIX_INTERVAL_MAX);
    }
}

/// Codec version — bump when the byte layout changes; [`decode`] rejects any other version
/// (the host then falls back to [`Settings::default`], i.e. settings reset on a format change).
pub const VERSION: u8 = 1;

/// Fixed encoded length: 14 payload bytes + a 2-byte CRC. A fixed size means the RRAM store
/// reads a known span and the file store needs no length framing.
pub const ENCODED_LEN: usize = 16;

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
    let crc = crc16(&b[0..14]);
    b[14..16].copy_from_slice(&crc.to_le_bytes());
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
    let crc = u16::from_le_bytes([b[14], b[15]]);
    if crc != crc16(&b[0..14]) {
        return None;
    }
    let mut s = Settings {
        units: if b[1] == Units::Imperial as u8 { Units::Imperial } else { Units::Metric },
        gps_time: b[2] != 0,
        clock: DateTime { year: u16::from_le_bytes([b[3], b[4]]), month: b[5], day: b[6], hour: b[7], minute: b[8] },
        utc_offset_min: i16::from_le_bytes([b[9], b[10]]),
        fix_interval_s: u16::from_le_bytes([b[11], b[12]]),
        power_saver: b[13] != 0,
    };
    s.sanitize();
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-default settings value round-trips through the codec byte-for-byte.
    #[test]
    fn codec_round_trips() {
        let s = Settings {
            units: Units::Imperial,
            gps_time: true,
            clock: DateTime { year: 2026, month: 6, day: 29, hour: 14, minute: 40 },
            utc_offset_min: 120,
            fix_interval_s: 5,
            power_saver: true,
        };
        assert_eq!(decode(&encode(&s)), Some(s));
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
        // Re-encode by hand so the CRC matches the bogus payload.
        let mut b = encode(&s);
        let crc = crc16(&b[0..14]);
        b[14..16].copy_from_slice(&crc.to_le_bytes());
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
