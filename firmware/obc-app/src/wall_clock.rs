//! The device wall clock — a live current time derived from a stored set-point and the monotonic
//! millis clock, plus the per-minute repaint edge a clock-bearing screen ticks on.
//!
//! There is no RTC. `now_ms` is boot-relative monotonic millis, and [`Settings::clock`] is only a
//! **set-point**: the time the user (or a GPS fix) last *established*. The live time is that
//! set-point advanced by however long has elapsed since it was stamped. Every clock-bearing screen
//! reads through here so they all agree and tick together.
//!
//! [`Settings::clock`]: crate::Settings::clock

use crate::settings::DateTime;

/// Derives the current wall-clock [`DateTime`] from a set-point (`base`) and the monotonic millis
/// at which that set-point was true (`epoch_ms`). [`now`](WallClock::now) is **recomputed from the
/// set-point every call**, so it can never accumulate drift. [`set`](WallClock::set) re-stamps both
/// halves (the Date & Time editor today, a GPS fix later). Owned by [`App`](crate::App); screens
/// get the already-computed `now` and never see the epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallClock {
    /// The set-point: the time that was true at [`epoch_ms`](WallClock::epoch_ms).
    base: DateTime,
    /// The monotonic millis at which [`base`](WallClock::base) was established.
    epoch_ms: u32,
    /// Whether a set-point has ever been **established** — false for the bare boot construction, true
    /// once [`set`](WallClock::set) has run (the persisted clock restored at boot, a manual edit, or
    /// a GPS/BLE re-stamp). Distinguishes "the device has been told a time" from a fresh clock that
    /// has never known one; the Home date line (#683) hides while this is false, since a date with no
    /// trusted origin would mislead. (Finer GPS/BLE-only trust is auto-expiry epic #638's job.)
    established: bool,
}

impl WallClock {
    /// A clock whose set-point `base` is true at the boot origin (`epoch_ms = 0`), not yet
    /// [`established`](WallClock::is_established). Seeded from the persisted
    /// [`Settings::clock`](crate::Settings::clock) at boot; without an RTC the clock resumes from the
    /// last-set value, off by however long the device was powered down until a GPS fix (or the user)
    /// re-stamps it.
    pub fn new(base: DateTime) -> Self {
        WallClock { base, epoch_ms: 0, established: false }
    }

    /// Re-stamp: declare that `base` is the time **now** (`now_ms`), so the clock resumes ticking
    /// from the freshly established value — and mark it [`established`](WallClock::is_established).
    pub fn set(&mut self, base: DateTime, now_ms: u32) {
        self.base = base;
        self.epoch_ms = now_ms;
        self.established = true;
    }

    /// Whether a set-point has ever been established (see the field) — the Home date line's
    /// "do we know the date?" gate.
    pub fn is_established(&self) -> bool {
        self.established
    }

    /// The current wall-clock time at `now_ms`: the set-point advanced by the whole minutes elapsed
    /// since it was stamped. `wrapping_sub` keeps the elapsed span correct across the ~49.7-day u32
    /// millis wrap. Minute resolution — the clock only ever displays `HH:MM`.
    pub fn now(&self, now_ms: u32) -> DateTime {
        let elapsed_min = now_ms.wrapping_sub(self.epoch_ms) / 60_000;
        self.base.add_minutes(elapsed_min)
    }

    /// Unix seconds at `now_ms`, reading the set-point as UTC: [`to_unix`](DateTime::to_unix) plus
    /// the **full elapsed seconds** since the stamp. Unlike [`now`](WallClock::now) this keeps the
    /// sub-minute remainder — the GPS re-stamp back-dates `epoch_ms` by the fix's
    /// seconds-into-the-minute, so second-level truth survives the minute-resolution set-point. The
    /// set-point is *local* time; the caller
    /// ([`App::wall_unix_now`](crate::App::wall_unix_now)) folds the UTC offset back out.
    pub fn unix_now(&self, now_ms: u32) -> u32 {
        self.base.to_unix().wrapping_add(now_ms.wrapping_sub(self.epoch_ms) / 1000)
    }

    /// Milliseconds from `now_ms` until the displayed `HH:MM` next rolls over — the timed-redraw
    /// deadline the **event-driven** host arms a single wake timer to, so the M33 can WFI until
    /// then rather than free-run to discover the change. Measured from the millis offset into the
    /// current minute; `wrapping_sub` keeps it wrap-safe like [`now`](WallClock::now). Always in
    /// `1..=60_000` (never 0 — at an exact boundary the full minute remains).
    pub fn ms_to_next_minute(&self, now_ms: u32) -> u32 {
        60_000 - now_ms.wrapping_sub(self.epoch_ms) % 60_000
    }
}

/// A per-minute repaint edge for a screen drawing an `HH:MM` clock. The screen holds one and calls
/// [`changed`](MinuteTicker::changed) from its `tick_timers`, dirtying itself exactly once each time
/// the displayed minute rolls over — so a static screen repaints as the clock advances without
/// polling on a blind heartbeat. The minute change subsumes every coarser rollover above it.
///
/// The **first** observation only *initialises* the baseline and reports no change: a screen's
/// first paint is already driven by whatever made it appear, so the ticker need only catch the
/// *subsequent* rollovers (and avoid a spurious second paint right after the first).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MinuteTicker {
    /// The last minute seen, or `None` before the first observation.
    last: Option<u8>,
}

impl MinuteTicker {
    /// Record the displayed minute of `now`, returning whether it changed since a *previous*
    /// observation (always `false` on the very first call — see the type docs).
    pub fn changed(&mut self, now: DateTime) -> bool {
        let changed = self.last.is_some_and(|m| m != now.minute);
        self.last = Some(now.minute);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(hour: u8, minute: u8) -> DateTime {
        DateTime { year: 2025, month: 6, day: 29, hour, minute }
    }

    #[test]
    fn now_advances_by_elapsed_minutes() {
        let mut c = WallClock::new(dt(14, 40));
        c.set(dt(14, 40), 1_000); // stamped at t = 1 s
        assert_eq!(c.now(1_000), dt(14, 40), "at the epoch it reads the set-point");
        assert_eq!(c.now(1_000 + 59_999), dt(14, 40), "a sub-minute elapsed advances nothing");
        assert_eq!(c.now(1_000 + 60_000), dt(14, 41), "one minute on, one minute later");
        assert_eq!(c.now(1_000 + 25 * 60_000), dt(15, 5), "25 min carries minute → hour");
        assert_eq!(c.now(1_000), dt(14, 40), "re-reading the epoch is unchanged — no drift");
    }

    /// The elapsed span is taken with `wrapping_sub`, so a `now_ms` that has wrapped past u32::MAX
    /// since the epoch still yields the true (small) elapsed minutes, not a ~49-day jump.
    #[test]
    fn now_is_correct_across_the_millis_wrap() {
        // Stamp 30 s before the counter wraps, then read it 90 s later — past u32::MAX. The elapsed
        // span is still 90 s = 1 whole minute, so 23:59 must roll to 00:00 of the next day.
        let mut c = WallClock::new(dt(23, 59));
        c.set(dt(23, 59), u32::MAX - 30_000);
        let now = c.now(u32::MAX.wrapping_add(60_000).wrapping_sub(30_000));
        assert_eq!((now.day, now.hour, now.minute), (30, 0, 0), "wrap-safe elapsed rolls the day");
    }

    /// `set` re-stamps both halves: the clock then ticks from the new set-point, ignoring the old.
    #[test]
    fn set_restamps_the_setpoint_and_epoch() {
        let mut c = WallClock::new(dt(0, 0));
        c.set(dt(9, 30), 100_000); // user sets 09:30 at t = 100 s
        assert_eq!(c.now(100_000), dt(9, 30));
        assert_eq!(c.now(100_000 + 2 * 60_000), dt(9, 32), "advances from the new stamp");
    }

    /// `ms_to_next_minute` reports the time left until the displayed minute rolls over — the wake
    /// deadline the event-driven loop arms. Measured from the offset into the current
    /// minute, never 0, and wrap-safe like `now`.
    #[test]
    fn ms_to_next_minute_counts_down_to_the_rollover() {
        let mut c = WallClock::new(dt(14, 40));
        c.set(dt(14, 40), 1_000); // stamped at t = 1 s
        assert_eq!(c.ms_to_next_minute(1_000), 60_000, "at a minute boundary the whole minute remains");
        assert_eq!(c.ms_to_next_minute(1_000 + 25_000), 35_000, "25 s in, 35 s to go");
        assert_eq!(c.ms_to_next_minute(1_000 + 59_999), 1, "just before the rollover, 1 ms left");
        assert_eq!(c.ms_to_next_minute(1_000 + 60_000), 60_000, "and resets a full minute after rolling");
        // Wrap-safe: stamped just before the u32 millis wrap, read past it — still the true remainder.
        let mut w = WallClock::new(dt(0, 0));
        w.set(dt(0, 0), u32::MAX - 30_000);
        assert_eq!(w.ms_to_next_minute(u32::MAX.wrapping_add(10_000)), 20_000, "wrap-safe: 40 s elapsed → 20 s left");
    }

    /// `unix_now` keeps the sub-minute remainder the `HH:MM` reading drops: a set-point stamped
    /// mid-minute (the GPS back-dating) yields second-accurate unix time, wrap-safe like `now`.
    #[test]
    fn unix_now_keeps_seconds_and_survives_the_wrap() {
        let base = DateTime { year: 2026, month: 7, day: 2, hour: 9, minute: 33 };
        let mut c = WallClock::new(base);
        c.set(base, 10_000); // 09:33:00 was true at t = 10 s
        assert_eq!(c.unix_now(10_000), base.to_unix());
        assert_eq!(c.unix_now(10_000 + 61_500), base.to_unix() + 61, "whole elapsed seconds, not minutes");
        // Wrap-safe: stamped 30 s before the u32 millis wrap, read 30 s past it — 60 s elapsed.
        let mut w = WallClock::new(base);
        w.set(base, u32::MAX - 30_000);
        assert_eq!(w.unix_now(u32::MAX.wrapping_add(30_000)), base.to_unix() + 60);
    }

    /// The ticker initialises silently on the first observation, then fires only when the minute
    /// actually rolls over.
    #[test]
    fn minute_ticker_fires_once_per_minute() {
        let mut t = MinuteTicker::default();
        assert!(!t.changed(dt(14, 40)), "the first observation just initialises the baseline");
        assert!(!t.changed(dt(14, 40)), "the same minute again is no change");
        assert!(t.changed(dt(14, 41)), "the minute rolled over");
        assert!(t.changed(dt(15, 42)), "an hour+minute jump still reads as a minute change");
        assert!(!t.changed(dt(15, 42)), "and settles back to quiet");
    }
}
