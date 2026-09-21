//! Bounded weekly opening schedules and shared Open / Closed / Unknown evaluation.
//! Overnight intervals belong to their start day and spill into the following day.

use obc_formats::obcm::{POI_HOURS_BLOB_LEN, POI_HOURS_DAYS, POI_HOURS_SLOTS_PER_DAY};
// The normative flag bits are owned by `obc-formats` and imported under the module-local
// `HOURS_FLAG_*` name this decoder reads. Not re-exported: consumers reach the flags through
// `obc_formats::obcm`.
use obc_formats::obcm::POI_HOURS_FLAG_TRUNCATED as HOURS_FLAG_TRUNCATED;

/// Shared eligibility fact. Only `Closed` excludes a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpeningStatus {
    Open,
    Closed,
    #[default]
    Unknown,
}

/// One open interval, quarter-hours from midnight (`0..=96`, where `96` is 24:00). `close_q <=
/// open_q` with both non-zero is an overnight wrap, and `(0, 0)` an unused slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interval {
    /// Opening time, quarter-hours from midnight (`0..=96`).
    pub open_q: u8,
    /// Closing time, quarter-hours from midnight (`0..=96`; `96` = 24:00).
    pub close_q: u8,
}

impl Interval {
    /// An unused slot is `(0, 0)`.
    #[inline]
    fn is_unused(&self) -> bool {
        self.open_q == 0 && self.close_q == 0
    }
}

/// A weekly opening-hours schedule decoded from one pooled 29-byte blob. Seven days, Monday
/// first, each with up to two [`Interval`]s, plus the `flags` byte. A small `Copy` stack value:
/// [`Reader::poi_hours`](crate::Reader::poi_hours) reads one on demand, with no cache involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeeklySchedule {
    /// `days[0]` is Monday; unused slots are `(0, 0)`.
    days: [[Interval; POI_HOURS_SLOTS_PER_DAY]; POI_HOURS_DAYS],
    /// `flags` bit 0 is seasonal, bit 1 truncated. Baked but ignored by the UI; exposed through
    /// [`WeeklySchedule::flags`].
    flags: u8,
}

/// Minutes in a day. The `minute_of_day` passed to [`WeeklySchedule::is_open`] is `0..=1439`, and
/// a `96`-quarter close maps to this value.
pub(crate) const MINUTES_PER_DAY: u16 = 1440;

impl WeeklySchedule {
    /// Decode a 29-byte pool blob into a schedule. A shorter slice yields `None`, so a truncated
    /// pool is handled cleanly. Every quarter-hour byte is taken as-is: the packer guarantees
    /// `0..=96`, and the eval helpers stay total for any byte value regardless.
    pub fn decode(blob: &[u8]) -> Option<WeeklySchedule> {
        if blob.len() < POI_HOURS_BLOB_LEN {
            return None;
        }
        let flags = blob[0];
        let mut days = [[Interval::default(); POI_HOURS_SLOTS_PER_DAY]; POI_HOURS_DAYS];
        // Day d, slot s occupies bytes `[1 + (d*2 + s)*2 .. +2]`, so the fixed 29-byte layout
        // makes every index below in-bounds.
        let mut i = 1;
        for day in &mut days {
            for slot in day.iter_mut() {
                if blob[i] > 96 || blob[i + 1] > 96 {
                    return None;
                }
                slot.open_q = blob[i];
                slot.close_q = blob[i + 1];
                i += 2;
            }
        }
        Some(WeeklySchedule { days, flags })
    }

    /// The raw `flags` byte. Any flag makes the current status unknown.
    #[inline]
    pub fn flags(&self) -> u8 {
        self.flags
    }

    /// True if the packer dropped a rule it could not model.
    #[inline]
    pub fn is_truncated(&self) -> bool {
        self.flags & HOURS_FLAG_TRUNCATED != 0
    }

    /// The up-to-two intervals for `weekday`, Monday first, with any trailing unused slot
    /// trimmed. An out-of-range `weekday` returns `&[]` rather than panicking.
    ///
    /// An overnight interval belongs to its start weekday and is returned on that day only, never
    /// split into the next morning. [`is_open`](Self::is_open) evaluates the wrap on that same day.
    pub fn today_intervals(&self, weekday: u8) -> &[Interval] {
        let Some(day) = self.days.get(weekday as usize) else {
            return &[];
        };
        // Both closed is empty. Slot 0 is always meaningful; slot 1 only if used.
        if day[0].is_unused() {
            // A day whose first slot is unused is a closed day: the packer never leaves a gap
            // before a used slot.
            &day[..0]
        } else if day[1].is_unused() {
            &day[..1]
        } else {
            &day[..2]
        }
    }

    /// Exact ranges within this calendar day, including the previous day's overnight spillover.
    pub fn intervals_on_day(&self, weekday: u8) -> heapless::Vec<Interval, 3> {
        let mut ranges = heapless::Vec::<Interval, 3>::new();
        if weekday >= 7 {
            return ranges;
        }
        let close_q = self
            .today_intervals((weekday + 6) % 7)
            .iter()
            .filter(|iv| iv.close_q <= iv.open_q)
            .map(|iv| iv.close_q)
            .max()
            .unwrap_or(0);
        if close_q > 0 {
            let _ = ranges.push(Interval { open_q: 0, close_q });
        }
        for iv in self.today_intervals(weekday) {
            let close_q = if iv.close_q <= iv.open_q { 96 } else { iv.close_q };
            if iv.open_q < close_q {
                let _ = ranges.push(Interval { open_q: iv.open_q, close_q });
            }
        }
        ranges.sort_unstable_by_key(|iv| iv.open_q);
        let mut merged = heapless::Vec::<Interval, 3>::new();
        for iv in ranges {
            if let Some(last) = merged.last_mut().filter(|last| iv.open_q <= last.close_q) {
                last.close_q = last.close_q.max(iv.close_q);
            } else {
                let _ = merged.push(iv);
            }
        }
        merged
    }

    /// Current status from exact weekly hours and an authoritative local clock.
    pub fn status(&self, local: Option<(u8, u16)>) -> OpeningStatus {
        let Some((weekday, minute)) = local else { return OpeningStatus::Unknown };
        if self.flags != 0 || weekday >= 7 || minute >= MINUTES_PER_DAY {
            return OpeningStatus::Unknown;
        }
        if self.is_open(weekday, minute) {
            OpeningStatus::Open
        } else {
            OpeningStatus::Closed
        }
    }

    /// Evaluate intervals on their start day plus overnight spillover from the previous day.
    /// Opening is inclusive, closing exclusive.
    pub fn is_open(&self, weekday: u8, minute_of_day: u16) -> bool {
        let Some(day) = self.days.get(weekday as usize) else { return false };
        let minute = minute_of_day.min(MINUTES_PER_DAY - 1);
        let today = day.iter().any(|iv| {
            if iv.is_unused() {
                return false;
            }
            let (open, close) = (u16::from(iv.open_q) * 15, u16::from(iv.close_q) * 15);
            minute >= open && (close <= open || minute < close)
        });
        let previous = &self.days[(weekday as usize + 6) % 7];
        today
            || previous
                .iter()
                .any(|iv| !iv.is_unused() && iv.close_q <= iv.open_q && minute < u16::from(iv.close_q) * 15)
    }
}

/// Weekday of a Gregorian date, Monday 0 to Sunday 6, the blob's day order, via Zeller's
/// congruence. Pure and `DateTime`-free, so the app can call it before
/// [`WeeklySchedule::is_open`].
///
/// `month` is `1..=12` and `day` `1..=31`; an out-of-range `month` is clamped, so a corrupt clock
/// never panics. Anchors are pinned in the tests.
pub fn weekday_from_ymd(year: u16, month: u8, day: u8) -> u8 {
    // Zeller's congruence. Jan and Feb count as months 13 and 14 of the prior year.
    let mut m = month.clamp(1, 12) as i32;
    let mut y = year as i32;
    if m < 3 {
        m += 12;
        y -= 1;
    }
    let k = y % 100; // year of century
    let j = y / 100; // zero-based century
    let q = day as i32;
    // Zeller: h = 0 is Saturday, 1 Sunday, 2 Monday, and so on.
    let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    // Remap Zeller's h to Monday 0 through Sunday 6.
    ((h + 5) % 7) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 29-byte blob from flags + per-day `(open_q, close_q)` slot pairs (Mon..Sun).
    fn blob(flags: u8, days: [[(u8, u8); 2]; 7]) -> [u8; POI_HOURS_BLOB_LEN] {
        let mut b = [0u8; POI_HOURS_BLOB_LEN];
        b[0] = flags;
        let mut i = 1;
        for day in &days {
            for &(o, c) in day {
                b[i] = o;
                b[i + 1] = c;
                i += 2;
            }
        }
        b
    }

    fn iv(open_q: u8, close_q: u8) -> Interval {
        Interval { open_q, close_q }
    }

    #[test]
    fn decode_round_trips_a_known_blob() {
        // Mon 08:00-18:00; Tue split 08:00-12:00 and 14:00-18:00; rest closed; truncated set.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72);
        days[1][0] = (32, 48);
        days[1][1] = (56, 72);
        let b = blob(HOURS_FLAG_TRUNCATED, days);
        let s = WeeklySchedule::decode(&b).expect("29-byte blob decodes");
        assert_eq!(s.flags(), HOURS_FLAG_TRUNCATED);
        assert!(s.is_truncated());
        assert_eq!(s.today_intervals(0), &[iv(32, 72)], "Mon one interval");
        assert_eq!(s.today_intervals(1), &[iv(32, 48), iv(56, 72)], "Tue two intervals");
        for d in 2..7u8 {
            assert_eq!(s.today_intervals(d), &[], "day {d} closed");
        }
    }

    #[test]
    fn decode_rejects_short_slice() {
        // A truncated pool buffer decodes to None, never a panic.
        let short = [0u8; POI_HOURS_BLOB_LEN - 1];
        assert_eq!(WeeklySchedule::decode(&short), None);
        assert_eq!(WeeklySchedule::decode(&[]), None);
    }

    #[test]
    fn today_intervals_selects_the_right_weekday() {
        // Each day a distinct single interval so the selection is unambiguous.
        let mut days = [[(0u8, 0u8); 2]; 7];
        for (d, day) in days.iter_mut().enumerate() {
            day[0] = ((d as u8) + 20, (d as u8) + 60);
        }
        let s = WeeklySchedule::decode(&blob(0, days)).unwrap();
        for d in 0..7u8 {
            assert_eq!(s.today_intervals(d), &[iv(d + 20, d + 60)], "weekday {d}");
        }
    }

    #[test]
    fn today_intervals_out_of_range_is_empty() {
        let s = WeeklySchedule::decode(&blob(0, [[(32, 72), (0, 0)]; 7])).unwrap();
        assert_eq!(s.today_intervals(7), &[], "weekday 7 out of range");
        assert_eq!(s.today_intervals(255), &[], "weekday 255 out of range");
    }

    #[test]
    fn is_open_normal_interval_boundaries() {
        // Mon 08:00-18:00 → open at 480 (08:00), closed at 1080 (18:00, exclusive).
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72); // 08:00-18:00
        let s = WeeklySchedule::decode(&blob(0, days)).unwrap();
        assert!(!s.is_open(0, 479), "07:59 closed");
        assert!(s.is_open(0, 480), "open exactly at 08:00");
        assert!(s.is_open(0, 1079), "17:59 open");
        assert!(!s.is_open(0, 1080), "closed exactly at 18:00");
        // A different weekday is closed.
        assert!(!s.is_open(1, 600), "Tue closed");
        // Out-of-range weekday ⇒ false.
        assert!(!s.is_open(7, 600));
    }

    #[test]
    fn is_open_24h_day() {
        // Mon (0,96) = open all day → open at every minute, incl. 00:00 and 23:59.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (0, 96);
        let s = WeeklySchedule::decode(&blob(0, days)).unwrap();
        assert!(s.is_open(0, 0), "00:00 open");
        assert!(s.is_open(0, 720), "noon open");
        assert!(s.is_open(0, 1439), "23:59 open");
        // A raw 1440 (24:00) clamps to 23:59, still open.
        assert!(s.is_open(0, 1440), "clamped 24:00 open");
    }

    #[test]
    fn is_open_closed_day() {
        // All slots (0,0) ⇒ never open at any minute.
        let s = WeeklySchedule::decode(&blob(0, [[(0, 0), (0, 0)]; 7])).unwrap();
        for minute in [0u16, 480, 720, 1080, 1439] {
            assert!(!s.is_open(2, minute), "closed at minute {minute}");
        }
    }

    #[test]
    fn is_open_overnight_wrap() {
        // Mon 22:00-02:00, so open late evening and early morning and closed midday, evaluated on
        // Monday, the start weekday.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (88, 8); // 22:00-02:00
        let s = WeeklySchedule::decode(&blob(0, days)).unwrap();
        assert!(s.is_open(0, 1320), "open exactly at 22:00");
        assert!(s.is_open(0, 1439), "23:59 open");
        assert!(!s.is_open(0, 0), "Monday morning precedes opening");
        assert!(s.is_open(1, 0), "Tuesday spillover");
        assert!(s.is_open(1, 119), "Tuesday 01:59 open");
        assert!(!s.is_open(1, 120), "Tuesday 02:00 closed");
        assert!(!s.is_open(0, 120), "closed exactly at 02:00");
        assert!(!s.is_open(0, 720), "noon closed");
        assert!(!s.is_open(0, 1319), "21:59 closed");
    }

    #[test]
    fn uncertain_hours_and_week_rollover() {
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[6][0] = (88, 8);
        let schedule = WeeklySchedule::decode(&blob(0, days)).unwrap();
        assert_eq!(schedule.status(Some((0, 119))), OpeningStatus::Open);
        assert_eq!(schedule.status(Some((0, 120))), OpeningStatus::Closed);
        assert_eq!(schedule.status(None), OpeningStatus::Unknown);
        for flag in [1, 2, 4, 128] {
            let uncertain = WeeklySchedule::decode(&blob(flag, days)).unwrap();
            assert_eq!(uncertain.status(Some((0, 119))), OpeningStatus::Unknown);
            assert_eq!(uncertain.status(Some((0, 120))), OpeningStatus::Unknown);
        }
    }

    #[test]
    fn is_open_two_intervals_split_lunch() {
        // Mon 08:00-12:00, 14:00-18:00 → closed over the 12:00-14:00 gap.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0] = [(32, 48), (56, 72)]; // 08-12, 14-18
        let s = WeeklySchedule::decode(&blob(0, days)).unwrap();
        assert!(s.is_open(0, 600), "10:00 open (first interval)");
        assert!(!s.is_open(0, 720), "12:00 closed (lunch, exclusive)");
        assert!(!s.is_open(0, 780), "13:00 closed (lunch gap)");
        assert!(s.is_open(0, 900), "15:00 open (second interval)");
    }

    #[test]
    fn weekday_from_ymd_matches_verified_anchors() {
        // Anchors verified against the system `date` command.
        assert_eq!(weekday_from_ymd(2000, 1, 1), 5, "2000-01-01 Saturday");
        assert_eq!(weekday_from_ymd(2024, 2, 29), 3, "2024-02-29 Thursday (leap)");
        assert_eq!(weekday_from_ymd(1900, 1, 1), 0, "1900-01-01 Monday");
        assert_eq!(weekday_from_ymd(2100, 3, 1), 0, "2100-03-01 Monday (century, non-leap)");
        assert_eq!(weekday_from_ymd(2026, 7, 5), 6, "2026-07-05 Sunday");
        assert_eq!(weekday_from_ymd(2023, 12, 31), 6, "2023-12-31 Sunday (year end)");
        assert_eq!(weekday_from_ymd(1970, 1, 1), 3, "1970-01-01 Thursday (epoch)");
    }

    #[test]
    fn weekday_from_ymd_stays_total_on_bad_month() {
        // A corrupt month clamps into range rather than panicking.
        let _ = weekday_from_ymd(2026, 0, 1);
        let _ = weekday_from_ymd(2026, 13, 1);
        let _ = weekday_from_ymd(2026, 255, 1);
    }
}
