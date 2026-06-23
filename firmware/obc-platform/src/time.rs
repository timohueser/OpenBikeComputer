//! Board-agnostic embassy-time helpers, shared by every OBC board crate.

use embassy_time::{Duration, Instant};

/// Panic-free `Instant::elapsed()` (issue #51). embassy-time's own `Instant::elapsed()` is
/// `Instant::now() - *self`, and `Instant - Instant` calls `duration_since`, which `unwrap!`s a
/// `checked_sub` — so it **panics** the instant `now()` reads *less* than the captured instant.
/// `now()` doing exactly that (a momentarily non-monotonic read) is a known embassy time-driver
/// race when a narrow hardware timer is extended to a 64-bit tick count, and the panic `udf`s →
/// HardFault → the board halts. Every `.elapsed()` in the firmware (the frame `now`, the
/// flip-reload timeouts, the render-stat deltas) only wants "how long since", and a transient
/// backwards read meaning "zero time passed" is harmless to all of them — so clamp to zero via
/// `saturating_duration_since` instead of panicking the device. This is a generic embassy-time
/// property, not STM32-specific, so it lives here and every board reuses one copy rather than
/// re-deriving the fix.
pub trait SaturatingElapsed {
    /// How long since this `Instant`, clamped to zero on a momentarily non-monotonic `now()`.
    fn saturating_elapsed(&self) -> Duration;
}

impl SaturatingElapsed for Instant {
    fn saturating_elapsed(&self) -> Duration {
        Instant::now().saturating_duration_since(*self)
    }
}
