//! The storage transport's health latch: a consecutive-failure circuit breaker with a half-open
//! re-arm.
//!
//! A board's card transport bounds each operation with its own deadlines, and that is not the same
//! as bounding a pass of the ride loop, which feeds the watchdog once and then runs however many
//! card operations the pass wants. A card that answers nothing pays the full deadline ladder for
//! every one of them, so enough of them in one pass outlast the dog and reset the device — which
//! looks like a firmware crash and destroys the evidence.
//!
//! The breaker counts consecutive failed attempts. At [`Breaker::LIMIT`] it is open and the
//! transport must refuse its operations without touching the device. After
//! [`Breaker::COOL_DOWN_MS`] it admits exactly one probe: a success clears the run and the ride
//! goes on, a failure re-opens it and restarts the cool-down. A connector that was shaken loose and
//! seats again therefore costs the rider one cool-down, not the rest of the ride.
//!
//! Only an attempt that reached the device counts. A caller-side refusal — a malformed buffer, a
//! span past the card, the breaker's own refusal — is [`Outcome::Refused`]: it neither extends the
//! run nor clears it, because it is no evidence either way about the card.
//!
//! ## The bound this protects
//!
//! The ride loop feeds a 24,000 ms watchdog once per pass. The costliest single failed operation on
//! the nRF54L transport is a joined multi-block write, where each leg's deadline is followed by a
//! firmware recovery (a warm re-boot, and an image re-copy if that fails):
//!
//! | leg | deadline | recovery | ms |
//! | :-- | --: | --: | --: |
//! | the ACMD23 pre-erase probe | 500 | 1,100 | 1,600 |
//! | the data phase | 4,000 | 1,100 | 5,100 |
//! | the CMD12 stop | 500 | 1,100 | 1,600 |
//! | **one failed write** | | | **8,300** |
//!
//! A `LIMIT` of 2 costs a pass 16,600 ms and clears the dog; 3 costs 24,900 ms and does not. The
//! cool-down is longer than the watchdog period, so no pass can admit two probes, and one probe is
//! one ladder.
//!
//! What this bounds is a *run* of failures. A transport that alternates one failure with one
//! success keeps the count at zero and is not bounded here; that pattern needs an accumulated-time
//! budget and is deliberately out of scope.

/// The ride loop's watchdog period, and the costliest single failed operation, as the module's
/// table prices them. They are here so the bound is a check and not only prose: a limit or a
/// cool-down that stops clearing the dog fails the build.
const WATCHDOG_MS: u32 = 24_000;
const WORST_FAILED_OPERATION_MS: u32 = 8_300;
const _: () = assert!(Breaker::LIMIT as u32 * WORST_FAILED_OPERATION_MS < WATCHDOG_MS);
const _: () = assert!((Breaker::LIMIT as u32 + 1) * WORST_FAILED_OPERATION_MS > WATCHDOG_MS);
const _: () = assert!(Breaker::COOL_DOWN_MS > WATCHDOG_MS);

/// What one storage operation turned out to be, as the breaker weighs it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The device was reached and answered.
    Success,
    /// The device was reached and the operation failed there: a deadline, an abort, a card error,
    /// or a soft peripheral that would not boot.
    TransportFault,
    /// The operation was refused before anything reached the device, so it is evidence about the
    /// caller and not about the card.
    Refused,
}

/// The count of consecutive transport faults, and the verdict that follows from it.
///
/// A value, not a service: the board owns where it lives (two words beside the driver) and this
/// type owns when it trips, when it probes and when it clears. Times are milliseconds from the
/// board's own monotonic clock, compared with wrapping arithmetic, so the clock's wrap is not a
/// special case.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Breaker {
    failures: u8,
    /// When the breaker last opened. Meaningless while it is closed.
    opened_at_ms: u32,
}

impl Breaker {
    /// Consecutive transport faults that open the breaker. See the module's bound.
    pub const LIMIT: u8 = 2;

    /// How long the breaker stays shut before it admits one probe.
    ///
    /// A connector that was shaken loose reseats within seconds, so a rider gets their ride back
    /// quickly. It is also longer than the watchdog period, which is what keeps a single pass to at
    /// most one probe.
    pub const COOL_DOWN_MS: u32 = 30_000;

    /// A transport with no failures behind it.
    pub const fn healthy() -> Self {
        Breaker { failures: 0, opened_at_ms: 0 }
    }

    /// Rebuild from the two stored words.
    pub const fn restore(failures: u8, opened_at_ms: u32) -> Self {
        Breaker { failures, opened_at_ms }
    }

    /// The stored count.
    pub const fn failures(self) -> u8 {
        self.failures
    }

    /// The stored open time.
    pub const fn opened_at_ms(self) -> u32 {
        self.opened_at_ms
    }

    /// Open: the transport has given up on the card, whether it is inside a cool-down or waiting
    /// for the probe that ends one. This is what the rider is told about.
    pub const fn open(self) -> bool {
        self.failures >= Self::LIMIT
    }

    /// Whether this operation may touch the device.
    ///
    /// True while closed, and true again once the cool-down has run out — which is what admits the
    /// probe. Exactly one operation gets through per cool-down, because the probe's own outcome
    /// either clears the breaker or restarts the cool-down before the next call asks.
    pub const fn admits(self, now_ms: u32) -> bool {
        !self.open() || now_ms.wrapping_sub(self.opened_at_ms) >= Self::COOL_DOWN_MS
    }

    /// Whether this ride-loop pass may run the half-open probe.
    ///
    /// A due probe still waits for proof that this pass fed the watchdog. This makes recovery a
    /// complete pass with a fresh deadline, instead of extra work appended to an old pass.
    pub const fn recovery_due(self, watchdog_fed: bool, now_ms: u32) -> bool {
        watchdog_fed && self.open() && self.admits(now_ms)
    }

    /// Whether background work may touch storage without becoming a half-open probe.
    pub const fn background_admitted(self) -> bool {
        !self.open()
    }

    /// Fold one operation's outcome in.
    pub const fn record(self, outcome: Outcome, now_ms: u32) -> Self {
        match outcome {
            Outcome::Refused => self,
            Outcome::Success => Breaker::healthy(),
            Outcome::TransportFault => {
                let failures = self.failures.saturating_add(1);
                // A fault that opens the breaker and a failed probe that re-opens it both start the
                // cool-down here.
                let opened_at_ms = if failures >= Self::LIMIT { now_ms } else { self.opened_at_ms };
                Breaker { failures, opened_at_ms }
            }
        }
    }

    /// True when `self` is the outcome that opened the breaker, so the failure is reported at the
    /// edge rather than on every later operation.
    pub const fn opened_from(self, before: Breaker) -> bool {
        self.open() && !before.open()
    }
}

#[cfg(test)]
mod tests {
    use super::{Breaker, Outcome};

    const T0: u32 = 1_000;

    fn fault(b: Breaker, now: u32) -> Breaker {
        b.record(Outcome::TransportFault, now)
    }

    /// Drive `LIMIT` faults and return the open breaker.
    fn opened(now: u32) -> Breaker {
        let mut b = Breaker::healthy();
        for _ in 0..Breaker::LIMIT {
            b = fault(b, now);
        }
        assert!(b.open());
        b
    }

    #[test]
    fn opens_on_a_run_of_faults_and_not_before() {
        let mut b = Breaker::healthy();
        for _ in 1..Breaker::LIMIT {
            b = fault(b, T0);
            assert!(!b.open(), "a run shorter than the limit still tries the device");
        }
        b = fault(b, T0);
        assert!(b.open());
        assert!(!b.admits(T0), "an open breaker refuses at once");
    }

    #[test]
    fn a_success_clears_the_run() {
        let mut b = Breaker::healthy();
        for _ in 0..Breaker::LIMIT - 1 {
            b = fault(b, T0);
        }
        b = b.record(Outcome::Success, T0);
        assert_eq!(b, Breaker::healthy());
        for _ in 0..Breaker::LIMIT - 1 {
            b = fault(b, T0);
        }
        assert!(!b.open(), "the faults before the success do not count toward the next run");
    }

    #[test]
    fn a_refusal_neither_counts_nor_clears() {
        let one = fault(Breaker::healthy(), T0);
        assert_eq!(one.record(Outcome::Refused, T0), one, "a refusal is no evidence about the card");
        let open = opened(T0);
        assert_eq!(open.record(Outcome::Refused, T0 + 1), open, "and it does not restart the cool-down");
    }

    #[test]
    fn the_probe_arrives_once_the_cool_down_runs_out() {
        let b = opened(T0);
        assert!(!b.admits(T0 + Breaker::COOL_DOWN_MS - 1));
        assert!(b.admits(T0 + Breaker::COOL_DOWN_MS));
    }

    #[test]
    fn recovery_requires_a_fresh_watchdog_feed_and_an_open_cool_down() {
        let due = T0 + Breaker::COOL_DOWN_MS;
        let open = opened(T0);
        assert!(!open.recovery_due(false, due), "a stale input heartbeat cannot start the long pass");
        assert!(!open.recovery_due(true, due - 1), "the cool-down still applies");
        assert!(open.recovery_due(true, due));
        assert!(!Breaker::healthy().recovery_due(true, due), "a healthy pass has nothing to recover");
    }

    #[test]
    fn background_work_stays_suppressed_at_the_half_open_edge() {
        let open = opened(T0);
        assert!(!open.background_admitted());
        assert!(open.admits(T0 + Breaker::COOL_DOWN_MS), "the dedicated probe is due");
        assert!(!open.background_admitted(), "map reads do not become the probe");
        assert!(Breaker::healthy().background_admitted());
    }

    #[test]
    fn a_good_probe_ends_the_latch_and_a_bad_one_restarts_it() {
        let probe_at = T0 + Breaker::COOL_DOWN_MS;

        let recovered = opened(T0).record(Outcome::Success, probe_at);
        assert_eq!(recovered, Breaker::healthy(), "a card that came back is a working card");

        let still_bad = fault(opened(T0), probe_at);
        assert!(still_bad.open());
        assert!(!still_bad.admits(probe_at + Breaker::COOL_DOWN_MS - 1), "the cool-down starts again");
        assert!(still_bad.admits(probe_at + Breaker::COOL_DOWN_MS));
    }

    #[test]
    fn the_count_saturates_rather_than_wrapping_back_under_the_limit() {
        let mut b = opened(T0);
        for _ in 0..300 {
            b = fault(b, T0);
        }
        assert!(b.open());
        assert_eq!(b.failures(), u8::MAX);
    }

    #[test]
    fn the_cool_down_survives_the_clock_wrapping() {
        let near_wrap = u32::MAX - Breaker::COOL_DOWN_MS / 2;
        let b = opened(near_wrap);
        let after_wrap = near_wrap.wrapping_add(Breaker::COOL_DOWN_MS);
        assert!(after_wrap < near_wrap, "the test must actually cross the wrap");
        assert!(!b.admits(after_wrap.wrapping_sub(1)));
        assert!(b.admits(after_wrap));
    }

    #[test]
    fn the_opening_outcome_is_named_once_per_latch() {
        let mut b = Breaker::healthy();
        let mut openings = 0;
        for _ in 0..10 {
            let after = fault(b, T0);
            if after.opened_from(b) {
                openings += 1;
            }
            b = after;
        }
        assert_eq!(openings, 1, "only one outcome reports the trip");
    }

    #[test]
    fn the_stored_words_round_trip() {
        let b = opened(T0);
        assert_eq!(Breaker::restore(b.failures(), b.opened_at_ms()), b);
    }
}
