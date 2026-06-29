//! The device wall clock — a live current time derived from a stored set-point and the
//! monotonic millis clock, plus the per-minute repaint edge a clock-bearing screen ticks on.
//!
//! There is no RTC. `now_ms` is boot-relative monotonic millis, and [`Settings::clock`] is only a
//! **set-point**: the time the user (or, later, a GPS fix) last *established*. The live time is
//! that set-point advanced by however long has elapsed since it was stamped — so [`WallClock`] is
//! the one place that turns "the time we were told" + "millis since" into "the time now", and
//! every screen that shows a clock reads it through here so they all agree and tick together.
//!
//! [`Settings::clock`]: crate::Settings::clock

use crate::settings::DateTime;

/// Derives the current wall-clock [`DateTime`] from a set-point (`base`) and the monotonic millis
/// at which that set-point was true (`epoch_ms`).
///
/// `now(now_ms)` is `base` advanced by the elapsed minutes since `epoch_ms` — **recomputed from
/// the set-point every call**, so it can never accumulate drift and is a pure function of
/// `(base, epoch_ms, now_ms)`. [`set`](WallClock::set) re-stamps both halves: the manual Date &
/// Time editor calls it on an edit, and a future GPS fix will re-stamp through the same seam (with
/// the manual set-point as the fallback when there's no fix). Owned by
/// [`App`](crate::App); handed to screens as the already-computed `now` so they never see the
/// epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallClock {
    /// The set-point: the time that was true at [`epoch_ms`](WallClock::epoch_ms).
    base: DateTime,
    /// The monotonic millis at which [`base`](WallClock::base) was established.
    epoch_ms: u32,
}

impl WallClock {
    /// A clock whose set-point `base` is true at the boot origin (`epoch_ms = 0`). The host seeds
    /// it from the persisted [`Settings::clock`](crate::Settings::clock) at boot and re-stamps it
    /// on the first real edit, so without an RTC the clock simply resumes from the last-set value
    /// and ticks forward — accurate to within however long the device was powered off, until a GPS
    /// fix (or the user) re-stamps it.
    pub fn new(base: DateTime) -> Self {
        WallClock { base, epoch_ms: 0 }
    }

    /// Re-stamp: declare that `base` is the time **now** (`now_ms`). Called when the time is set —
    /// by the Date & Time editor today, by a GPS fix later — so the clock resumes ticking from the
    /// freshly established value rather than the stale one.
    pub fn set(&mut self, base: DateTime, now_ms: u32) {
        self.base = base;
        self.epoch_ms = now_ms;
    }

    /// The current wall-clock time at `now_ms`: the set-point advanced by the whole minutes elapsed
    /// since it was stamped. `wrapping_sub` so the elapsed span is correct across the ~49.7-day u32
    /// millis wrap (the real elapsed is always far below it). Minute resolution — the clock only
    /// ever displays `HH:MM`, and the set-point itself carries no seconds.
    pub fn now(&self, now_ms: u32) -> DateTime {
        let elapsed_min = now_ms.wrapping_sub(self.epoch_ms) / 60_000;
        self.base.add_minutes(elapsed_min)
    }
}

/// A per-minute repaint edge for a screen drawing an `HH:MM` clock. The screen holds one and calls
/// [`changed`](MinuteTicker::changed) from its `animate`, dirtying itself exactly once each time
/// the displayed minute rolls over — the timed-redraw the render-on-demand host needs to repaint a
/// static screen as the clock advances, without it polling the clock on a blind heartbeat.
///
/// The minute is the finest field of an `HH:MM` readout, so a minute change also subsumes every
/// coarser rollover (hour, day) above it; a screen that showed seconds would compare those instead.
/// Reusable by every clock-bearing screen, so they all tick on the same once-a-minute cadence.
///
/// The **first** observation only *initialises* the baseline and reports no change: a screen's
/// first paint is already driven by whatever made it appear (the boot dirty, or the navigation
/// back to it), which draws the current minute — so the ticker need only catch the *subsequent*
/// rollovers, matching the "dirty only on an actual change" contract the Statistics spring-back
/// follows (and avoiding a spurious second paint right after the first).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MinuteTicker {
    /// The last minute seen, or `None` before the first observation (which initialises it without
    /// reporting a change).
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

    /// `now` returns the set-point advanced by the whole minutes since the epoch — and recomputes
    /// from the set-point each call, so reading it twice for the same `now_ms` is identical (no
    /// accumulation), and a sub-minute elapsed adds nothing.
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
