//! `hours.rs` — device-side view over a hours-pool blob (spec §7.5, epic #439 P3
//! #443): decode a pooled 29-byte weekly schedule, select today's intervals, and
//! answer *open now* — a trivial weekday lookup, the `opening_hours` grammar having
//! already run at pack time (`obc-pack`'s `hours.rs`).
//!
//! This is the **read** counterpart to the packer's blob encoder. The
//! [`Interval`]/[`WeeklySchedule`] algorithm types remain reader-side; both sides import the
//! normative blob width/dimensions/flags from `obc-formats`.
//!
//! ## Blob layout (29 bytes, spec §7.5)
//! `flags u8` + 7 days (`Mon` index 0 .. `Sun` index 6) × 2 slots × `(open_q u8,
//! close_q u8)`. A time-of-day is quarter-hours from midnight, `0..=96` (`96` =
//! 24:00). Per interval: unused slot `(0, 0)`; closed day = both slots `(0, 0)`;
//! 24 h = slot 0 `(0, 96)`; overnight wrap = `close_q <= open_q` (both nonzero),
//! open past midnight. `flags` bit 0 = seasonal, bit 1 = truncated (both baked but
//! UI-ignored in v1).
//!
//! Everything here is a small **stack** value — no heap, no static. The whole
//! decoded schedule is `1 + 7*2*2 = 29` bytes plus the flags byte, sitting on the
//! caller's stack for the lifetime of the detail screen.

use obc_formats::obcm::{POI_HOURS_BLOB_LEN, POI_HOURS_DAYS, POI_HOURS_SLOTS_PER_DAY};
// The normative flag bits are owned by `obc-formats`; imported under the module-local `HOURS_FLAG_*`
// name this decoder reads. Not re-exported — consumers reach the flags via `obc_formats::obcm`
// (which is also where the seasonal bit is read from: the decoder only names the one it acts on).
use obc_formats::obcm::POI_HOURS_FLAG_TRUNCATED as HOURS_FLAG_TRUNCATED;

/// One open interval, quarter-hours from midnight (`0..=96`, `96` = 24:00). Mirrors
/// the packer's `hours::Interval`; `close_q <= open_q` (both nonzero) is an overnight
/// wrap, `(0, 0)` an unused slot. Defined here so obc-reader carries no obc-pack dep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Interval {
    /// Opening time, quarter-hours from midnight (`0..=96`).
    pub open_q: u8,
    /// Closing time, quarter-hours from midnight (`0..=96`; `96` = 24:00).
    pub close_q: u8,
}

impl Interval {
    /// An unused slot is `(0, 0)` — a day's second (or only) slot when it holds no
    /// second (or any) interval.
    #[inline]
    fn is_unused(&self) -> bool {
        self.open_q == 0 && self.close_q == 0
    }
}

/// A weekly opening-hours schedule decoded from one pooled 29-byte blob (spec §7.5).
/// Seven days (`Mon` index 0 .. `Sun` index 6), each up to two [`Interval`]s, plus
/// the `flags` byte (seasonal / truncated — baked but UI-ignored in v1). A small
/// `Copy` stack value: [`Reader::poi_hours`](crate::Reader::poi_hours) reads one on
/// demand for the detail screen, no cache or static involved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeeklySchedule {
    /// `days[0]` = Monday .. `days[6]` = Sunday; each up to two intervals (unused
    /// slots are `(0, 0)`).
    days: [[Interval; POI_HOURS_SLOTS_PER_DAY]; POI_HOURS_DAYS],
    /// `flags` bit 0 = seasonal, bit 1 = truncated (spec §7.5). Baked but ignored by
    /// the v1 UI; exposed via [`WeeklySchedule::flags`] for a future season-aware pass.
    flags: u8,
}

/// Minutes in a day. `minute_of_day` passed to [`WeeklySchedule::is_open`] is
/// `0..=1439`; `24:00` (a `96`-quarter close) maps to this value.
pub(crate) const MINUTES_PER_DAY: u16 = 1440;

impl WeeklySchedule {
    /// Decode a 29-byte pool blob (spec §7.5) into a schedule. `blob` must be exactly
    /// [`POI_HOURS_BLOB_LEN`](obc_formats::obcm::POI_HOURS_BLOB_LEN) bytes; a shorter slice yields
    /// `None` (a corrupt/truncated pool is handled cleanly, never a panic). Every
    /// quarter-hour byte is taken as-is — the packer guarantees `0..=96`, and the eval
    /// helpers stay total for any byte value regardless.
    pub fn decode(blob: &[u8]) -> Option<WeeklySchedule> {
        if blob.len() < POI_HOURS_BLOB_LEN {
            return None;
        }
        let flags = blob[0];
        let mut days = [[Interval::default(); POI_HOURS_SLOTS_PER_DAY]; POI_HOURS_DAYS];
        // Day d, slot s occupies bytes [1 + (d*2 + s)*2 .. +2]; the fixed 29-byte
        // layout means every index below is in-bounds for a >= 29-byte slice.
        let mut i = 1;
        for day in &mut days {
            for slot in day.iter_mut() {
                slot.open_q = blob[i];
                slot.close_q = blob[i + 1];
                i += 2;
            }
        }
        Some(WeeklySchedule { days, flags })
    }

    /// The raw `flags` byte (spec §7.5): bit 0 seasonal, bit 1 truncated. Baked but
    /// ignored by the v1 UI — exposed for a future season-aware pass.
    #[inline]
    pub fn flags(&self) -> u8 {
        self.flags
    }

    /// True if the packer dropped a rule it couldn't model (truncated flag set).
    #[inline]
    pub fn is_truncated(&self) -> bool {
        self.flags & HOURS_FLAG_TRUNCATED != 0
    }

    /// The up-to-two intervals for `weekday` (`0` = Monday .. `6` = Sunday), with any
    /// trailing unused `(0, 0)` slot trimmed: a closed day returns `&[]`, a one-interval
    /// day a 1-slot slice, a two-interval day both. An out-of-range `weekday` returns
    /// `&[]` (clamped-empty, never a panic) — the detail screen renders "Closed today".
    ///
    /// An **overnight** interval (e.g. `22:00–02:00`) belongs to its **start** weekday
    /// and is returned on that day only — it is never split into the next morning's
    /// day. So "today's intervals" shows an interval that opened today, even if it runs
    /// past midnight; [`is_open`](Self::is_open) evaluates the wrap on that same start day.
    pub fn today_intervals(&self, weekday: u8) -> &[Interval] {
        let Some(day) = self.days.get(weekday as usize) else {
            return &[];
        };
        // Both closed ⇒ empty. Otherwise slot 0 is always meaningful; slot 1 only if used.
        if day[0].is_unused() {
            // A day whose first slot is unused is a closed day (the packer never leaves a
            // gap before a used slot), so nothing is open.
            &day[..0]
        } else if day[1].is_unused() {
            &day[..1]
        } else {
            &day[..2]
        }
    }

    /// True iff `minute_of_day` (`0..=1439`) falls in an open interval for `weekday`
    /// (`0` = Monday .. `6` = Sunday). Out-of-range `weekday` ⇒ `false`. Each interval's
    /// quarter-hours become minutes (`q * 15`); the semantics per §7.5:
    ///
    /// - **Normal** `[open, close)` — open when `open*15 <= minute < close*15`
    ///   (half-open: open exactly at `open`, closed exactly at `close`).
    /// - **24 h** `(0, 96)` — `close*15 == 1440`, so `minute < 1440` is always open.
    /// - **Closed day** `(0, 0)` — `is_unused`, contributes nothing ⇒ never open.
    /// - **Overnight wrap** `close_q <= open_q` (both nonzero) — the interval runs past
    ///   midnight; open when `minute >= open*15` **or** `minute < close*15`. Evaluated on
    ///   the interval's **start** weekday (the morning spill is part of the same start-day
    ///   interval, not the next day's schedule — matching what `today_intervals` shows).
    ///
    /// `minute_of_day` is clamped to `0..=1439` so a caller passing `1440` (a raw 24:00)
    /// still evaluates sanely.
    pub fn is_open(&self, weekday: u8, minute_of_day: u16) -> bool {
        let Some(day) = self.days.get(weekday as usize) else {
            return false;
        };
        let minute = minute_of_day.min(MINUTES_PER_DAY - 1);
        for iv in day {
            if iv.is_unused() {
                continue;
            }
            let open = (iv.open_q as u16) * 15;
            let close = (iv.close_q as u16) * 15;
            let hit = if close > open {
                // Normal (incl. 24 h: open 0, close 1440) — half-open [open, close).
                minute >= open && minute < close
            } else {
                // Overnight wrap (close <= open, both nonzero): open past midnight.
                minute >= open || minute < close
            };
            if hit {
                return true;
            }
        }
        false
    }
}

/// Weekday of a Gregorian date, **Mon = 0 .. Sun = 6** (the blob's day order, spec
/// §7.5), via Zeller's congruence. Pure, `DateTime`-free — the shared bottom of the
/// hours stack that `obc-app` (#444) calls as `weekday_from_ymd(dt.year, dt.month,
/// dt.day)` before [`WeeklySchedule::is_open`]/[`today_intervals`].
///
/// `month` is `1..=12`, `day` `1..=31`; an out-of-range `month` is clamped into range
/// so the function stays total (a corrupt clock never panics). Valid for any Gregorian
/// year the `u16` holds. Anchors pinned in the tests: `2000-01-01` = Sat (5),
/// `2024-02-29` = Thu (3), `1900-01-01` = Mon (0), `2100-03-01` = Mon (0),
/// `1970-01-01` = Thu (3).
pub fn weekday_from_ymd(year: u16, month: u8, day: u8) -> u8 {
    // Zeller's congruence (Gregorian). Treat Jan/Feb as months 13/14 of the prior year.
    let mut m = month.clamp(1, 12) as i32;
    let mut y = year as i32;
    if m < 3 {
        m += 12;
        y -= 1;
    }
    let k = y % 100; // year of century
    let j = y / 100; // zero-based century
    let q = day as i32;
    // Zeller: h = 0 = Saturday, 1 = Sunday, 2 = Monday, ... 6 = Friday.
    let h = (q + (13 * (m + 1)) / 5 + k + k / 4 + j / 4 + 5 * j).rem_euclid(7);
    // Remap Zeller's h (Sat=0) to Mon=0..Sun=6: (h + 5) mod 7.
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
        // Mon 08:00-18:00 (32,72); Tue split 08:00-12:00,14:00-18:00; rest closed;
        // truncated flag set.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72);
        days[1][0] = (32, 48);
        days[1][1] = (56, 72);
        let b = blob(HOURS_FLAG_TRUNCATED, days);
        let s = WeeklySchedule::decode(&b).expect("29-byte blob decodes");
        assert_eq!(s.flags(), HOURS_FLAG_TRUNCATED);
        // The seasonal bit being clear is already pinned by the `flags()` equality above.
        assert!(s.is_truncated());
        assert_eq!(s.today_intervals(0), &[iv(32, 72)], "Mon one interval");
        assert_eq!(s.today_intervals(1), &[iv(32, 48), iv(56, 72)], "Tue two intervals");
        for d in 2..7u8 {
            assert_eq!(s.today_intervals(d), &[], "day {d} closed");
        }
    }

    #[test]
    fn decode_rejects_short_slice() {
        // A truncated pool buffer (< 29 bytes) decodes to None, never a panic/UB.
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
        // Mon 22:00-02:00 → open_q=88 (1320 min), close_q=8 (120 min). Open late evening
        // and early morning, closed midday — evaluated on Monday, the start weekday.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (88, 8); // 22:00-02:00
        let s = WeeklySchedule::decode(&blob(0, days)).unwrap();
        assert!(s.is_open(0, 1320), "open exactly at 22:00");
        assert!(s.is_open(0, 1439), "23:59 open");
        assert!(s.is_open(0, 0), "00:00 open (wrap)");
        assert!(s.is_open(0, 119), "01:59 open (wrap)");
        assert!(!s.is_open(0, 120), "closed exactly at 02:00");
        assert!(!s.is_open(0, 720), "noon closed");
        assert!(!s.is_open(0, 1319), "21:59 closed");
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
        // Anchors verified against the system `date` command (Mon=0..Sun=6).
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
        // A corrupt month clamps into 1..=12 rather than panicking.
        let _ = weekday_from_ymd(2026, 0, 1);
        let _ = weekday_from_ymd(2026, 13, 1);
        let _ = weekday_from_ymd(2026, 255, 1);
    }
}
