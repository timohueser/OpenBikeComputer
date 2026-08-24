//! The **content pager** — the two-page auto-flip the detail compositions share (the Route
//! overview and the Ride detail): a media band and its paired stat rows swap together on a fixed
//! dwell, and the flip itself is the affordance (no page dots). One type owns the timing; the
//! screens own what each page draws.

use crate::screen::ScreenTick;

/// The pager's dwell — a plain fixed constant (not user-configurable): each page shows this long
/// before the auto-flip, so the two sibling pages read on the same rhythm.
pub(crate) const PAGE_FLIP_MS: u32 = 5_000;

/// A two-page auto-flip. The screen polls [`tick`](ContentPager::tick) from its `tick_timers` arm
/// and reads [`page`](ContentPager::page) in its `draw`.
#[derive(Debug, Default)]
pub(crate) struct ContentPager {
    /// Whether the second page is the one showing (the pager has exactly two).
    second: bool,
    /// Instant of the last flip (wrap-safe). `None` until the first tick anchors it, so the first
    /// page gets a full dwell on entry.
    last_flip_ms: Option<u32>,
}

impl ContentPager {
    /// True while the **second** page is showing — the branch both callers' draw bodies take.
    pub(crate) fn on_second_page(&self) -> bool {
        self.second
    }

    /// Flip when the dwell is up and report the residual dwell as the next wake. The elapsed check
    /// is `wrapping_sub`, so it stays correct across the `u32` millis wrap; a flip re-anchors, so a
    /// due deadline fires exactly once.
    pub(crate) fn tick(&mut self, now_ms: u32) -> ScreenTick {
        let last = *self.last_flip_ms.get_or_insert(now_ms);
        let changed = now_ms.wrapping_sub(last) >= PAGE_FLIP_MS;
        if changed {
            self.second = !self.second;
            self.last_flip_ms = Some(now_ms);
        }
        let anchor = self.last_flip_ms.unwrap_or(now_ms);
        let next = PAGE_FLIP_MS.saturating_sub(now_ms.wrapping_sub(anchor)).max(1);
        ScreenTick { changed, next_wake_ms: Some(next), region: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flip lands **exactly** on the dwell deadline, once, and re-arms a fresh dwell — a poll
    /// one millisecond later must not flip back.
    #[test]
    fn flips_once_exactly_at_the_deadline() {
        let mut p = ContentPager::default();
        assert!(!p.tick(0).changed, "the first poll only anchors the dwell");
        assert!(!p.tick(PAGE_FLIP_MS - 1).changed, "still dwelling one ms before the deadline");
        assert!(!p.on_second_page());
        assert!(p.tick(PAGE_FLIP_MS).changed, "flips exactly at the deadline");
        assert!(p.on_second_page());
        assert!(!p.tick(PAGE_FLIP_MS + 1).changed, "and only once — a fresh dwell re-armed");
        assert!(p.on_second_page(), "the 5,001 ms poll must not flip back");
        assert!(p.tick(2 * PAGE_FLIP_MS).changed, "flips back at the next deadline");
        assert!(!p.on_second_page());
    }

    /// The reported wake counts down the residual dwell and is never zero, so a host that sleeps
    /// exactly that long wakes to a due flip rather than to a no-op poll.
    #[test]
    fn next_wake_counts_down_the_residual_dwell() {
        let mut p = ContentPager::default();
        assert_eq!(p.tick(0).next_wake_ms, Some(PAGE_FLIP_MS));
        assert_eq!(p.tick(2_000).next_wake_ms, Some(PAGE_FLIP_MS - 2_000));
        assert_eq!(p.tick(PAGE_FLIP_MS).next_wake_ms, Some(PAGE_FLIP_MS), "the flip re-arms a full dwell");
    }

    /// The dwell is elapsed arithmetic, so it survives the `u32` millis wrap: anchored just below
    /// `u32::MAX`, the flip still lands on the deadline past the wrap and not before it.
    #[test]
    fn dwell_survives_the_clock_wrap() {
        let start = u32::MAX - 1_000;
        let mut p = ContentPager::default();
        p.tick(start);
        assert!(!p.tick(start.wrapping_add(PAGE_FLIP_MS - 1)).changed, "no early flip across the wrap");
        assert!(!p.on_second_page());
        assert!(p.tick(start.wrapping_add(PAGE_FLIP_MS)).changed, "the deadline still lands past the wrap");
        assert!(p.on_second_page());
    }
}
