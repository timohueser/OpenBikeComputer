//! The storage transport's health latch: a consecutive-failure circuit breaker.
//!
//! A board's card transport bounds each operation with its own deadlines, and that is not the same
//! as bounding a pass of the ride loop, which feeds the watchdog once and then runs however many
//! card operations the pass wants. A card that answers nothing pays the full deadline ladder for
//! every one of them, so enough of them in one pass outlast the dog and reset the device — which
//! looks like a firmware crash and destroys the evidence.
//!
//! The breaker counts consecutive failed attempts. Once the count reaches [`Breaker::LIMIT`] the
//! breaker is open and the transport must fail its operations immediately, without touching the
//! device. It stays open: nothing resets it, because nothing attempts the device again, so a
//! dead card or a soft peripheral that stopped booting costs its deadline ladder `LIMIT` times in
//! the session rather than once per operation for the rest of the ride. Only a power cycle retries.
//!
//! What it bounds is a *run* of failures. A transport that alternates one failure with one success
//! keeps the count at zero and is not bounded here; that pattern needs an accumulated-time budget
//! and is deliberately out of scope.

/// The count of consecutive failed transport attempts, and the verdict that follows from it.
///
/// A value, not a service: the board owns where it lives (one atomic word beside the driver) and
/// this type owns when it trips.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Breaker(u8);

impl Breaker {
    /// Consecutive failures that open the breaker.
    ///
    /// Chosen against the watchdog, not measured: the costliest failed operation on the nRF54L
    /// transport is a write deadline plus a firmware recovery, about 5.1 s, and the ride loop feeds
    /// a 24 s dog once per pass. Three failures is about 15 s, which leaves a pass room to finish;
    /// four does not.
    pub const LIMIT: u8 = 3;

    /// A transport with no failures behind it.
    pub const fn healthy() -> Self {
        Breaker(0)
    }

    /// Rebuild from the stored count.
    pub const fn from_failures(failures: u8) -> Self {
        Breaker(failures)
    }

    /// The stored count.
    pub const fn failures(self) -> u8 {
        self.0
    }

    /// Open: the transport must fail the operation immediately and touch nothing.
    pub const fn open(self) -> bool {
        self.0 >= Self::LIMIT
    }

    /// Fold one attempt's outcome in. A success clears the run; a failure extends it.
    pub const fn record(self, ok: bool) -> Self {
        if ok {
            Breaker(0)
        } else {
            Breaker(self.0.saturating_add(1))
        }
    }

    /// True when `self` is the attempt that opened the breaker, so the failure is reported once
    /// rather than on every later operation.
    pub const fn opened_from(self, before: Breaker) -> bool {
        self.open() && !before.open()
    }
}

#[cfg(test)]
mod tests {
    use super::Breaker;

    #[test]
    fn opens_on_a_run_of_failures_and_not_before() {
        let mut b = Breaker::healthy();
        for _ in 1..Breaker::LIMIT {
            b = b.record(false);
            assert!(!b.open(), "a run shorter than the limit still tries the device");
        }
        b = b.record(false);
        assert!(b.open());
    }

    #[test]
    fn a_success_clears_the_run() {
        let mut b = Breaker::healthy();
        for _ in 0..Breaker::LIMIT - 1 {
            b = b.record(false);
        }
        b = b.record(true);
        assert_eq!(b, Breaker::healthy());
        for _ in 0..Breaker::LIMIT - 1 {
            b = b.record(false);
        }
        assert!(!b.open(), "the failures before the success do not count toward the next run");
    }

    #[test]
    fn open_is_terminal_and_the_count_saturates() {
        let mut b = Breaker::healthy();
        for _ in 0..Breaker::LIMIT {
            b = b.record(false);
        }
        assert!(b.open());
        for _ in 0..300 {
            b = b.record(false);
        }
        assert!(b.open(), "the count must not wrap back under the limit");
        assert_eq!(b.failures(), u8::MAX);
    }

    #[test]
    fn the_opening_attempt_is_named_once() {
        let mut b = Breaker::healthy();
        let mut openings = 0;
        for _ in 0..10 {
            let after = b.record(false);
            if after.opened_from(b) {
                openings += 1;
            }
            b = after;
        }
        assert_eq!(openings, 1, "only one attempt reports the trip");
    }

    #[test]
    fn a_stored_count_round_trips() {
        let b = Breaker::healthy().record(false);
        assert_eq!(Breaker::from_failures(b.failures()), b);
    }
}
