//! The StorageInfo domain protocol: how much room is left on the medium.
//!
//! The smallest domain in DeviceCore, and the only one with no home of its own outside it. Free
//! space belongs to no product feature; it is a fact about the device that the System screen shows.
//! It still gets the full treatment — an intent, a bounded effect, a token-carrying outcome —
//! because measuring free space is a failable platform query: the board reads the mounted flat
//! store's free bitmap, while the simulator can query its own backing store.

use crate::device_core::{OperationToken, StorageInfoTag};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInfoIntent {
    /// Re-measure free space — entering the System screen, or a manual refresh. Idempotent: the
    /// domain coalesces a repeat into the measurement already in flight.
    RefreshRequested,
}

/// One bounded physical measurement, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInfoEffect {
    MeasureFreeSpace { token: OperationToken<StorageInfoTag> },
}

impl StorageInfoEffect {
    pub fn token(&self) -> OperationToken<StorageInfoTag> {
        match self {
            StorageInfoEffect::MeasureFreeSpace { token } => *token,
        }
    }
}

/// Why a measurement failed. Distinct from an absent
/// [`StorageInfoCapabilities::report_free_space`](crate::device_core::StorageInfoCapabilities):
/// a platform that cannot measure at all hides the figure instead of failing at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInfoError {
    NotMounted,
    /// The scan started and failed: a read error part-way through the allocation table.
    ScanFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInfoOutcome {
    Measured {
        token: OperationToken<StorageInfoTag>,
        free_bytes: u64,
    },
    Failed {
        token: OperationToken<StorageInfoTag>,
        error: StorageInfoError,
    },
    /// The executor abandoned the measurement without completing it.
    Cancelled {
        token: OperationToken<StorageInfoTag>,
    },
}

impl StorageInfoOutcome {
    pub fn token(&self) -> OperationToken<StorageInfoTag> {
        match self {
            StorageInfoOutcome::Measured { token, .. }
            | StorageInfoOutcome::Failed { token, .. }
            | StorageInfoOutcome::Cancelled { token } => *token,
        }
    }
}

// Layout tripwires: a token, and at most one byte count.
const _: () = assert!(core::mem::size_of::<StorageInfoIntent>() == 0, "one fieldless request");
const _: () = assert!(core::mem::size_of::<StorageInfoEffect>() <= 4, "a bare token");
const _: () = assert!(core::mem::size_of::<StorageInfoOutcome>() <= 16, "a token and a byte count");
const _: () = assert!(core::mem::size_of::<StorageInfoError>() <= 1, "a verdict, not a report");

use crate::device_core::TokenSource;

/// The storage-information domain: the refresh request, the operation token, and the figure the
/// System screen prints. The free-space number lives here rather than on `App` because it is this
/// domain's only product state.
#[derive(Debug, Default)]
pub struct StorageInfo {
    /// A refresh the executor has not taken yet. A repeat while one is waiting is the same request;
    /// a repeat while one is running re-arms it, because free space may have moved since the scan
    /// started.
    requested: bool,
    ops: TokenSource<StorageInfoTag>,
    /// Bytes still available on the mounted medium, or `None` until a measurement succeeds, which
    /// is what the System screen shows as `--`.
    free_bytes: Option<u64>,
}

impl StorageInfo {
    pub(crate) const fn new() -> Self {
        StorageInfo { requested: false, ops: TokenSource::new(), free_bytes: None }
    }

    pub(crate) fn admit_intent(&mut self, intent: StorageInfoIntent) {
        match intent {
            StorageInfoIntent::RefreshRequested => self.requested = true,
        }
    }

    pub(crate) fn next_effect(&mut self) -> Option<StorageInfoEffect> {
        core::mem::take(&mut self.requested).then(|| StorageInfoEffect::MeasureFreeSpace { token: self.ops.issue() })
    }

    /// Consume the answer to a measurement. A superseded or repeated answer changes nothing.
    ///
    /// A number replaces the figure. A failure replaces it with no figure, because we no longer know
    /// how much room is left and a count from an earlier scan under "Card free" would be a lie: the
    /// card the rider has since taken out is the case that matters. A cancellation leaves the figure
    /// alone, since nothing was attempted.
    ///
    /// Nothing is retried here: the rider asks again by re-entering the screen. A domain that
    /// re-armed itself would turn a dead card into a free-cluster walk every pass.
    pub(crate) fn apply_outcome(&mut self, outcome: StorageInfoOutcome) -> bool {
        if !self.ops.is_current(outcome.token()) {
            return false;
        }
        self.ops.invalidate(); // terminal: a duplicate of this answer is no longer current
        match outcome {
            StorageInfoOutcome::Measured { free_bytes, .. } => self.note_measured(Some(free_bytes)),
            StorageInfoOutcome::Failed { .. } => self.note_measured(None),
            StorageInfoOutcome::Cancelled { .. } => {}
        }
        true
    }

    /// A measurement answered with `free_bytes`, or `None` when it produced no figure. The
    /// assignment is unconditional, which is what makes a `None` blank the screen back to `--`.
    pub(crate) fn note_measured(&mut self, free_bytes: Option<u64>) {
        self.free_bytes = free_bytes;
    }

    pub(crate) fn free_bytes(&self) -> Option<u64> {
        self.free_bytes
    }

    #[cfg(test)]
    pub(crate) fn assert_boot_state(&self) {
        let StorageInfo { requested, ops, free_bytes } = self;
        assert!(!*requested, "no free-space refresh posted");
        assert_eq!(format!("{ops:?}"), "TokenSource(0)", "no measurement has been issued");
        assert!(free_bytes.is_none(), "the card scan has not answered");
    }
}

// Layout tripwire: a byte count, a token and a flag.
const _: () = assert!(core::mem::size_of::<StorageInfo>() <= 24, "one byte count and a generation");

#[cfg(test)]
mod storage_info_tests {
    use super::*;

    /// The request is an idempotent refresh: repeats before the measurement leaves coalesce into
    /// the one already waiting, rather than a second concurrent walk of the allocation table.
    #[test]
    fn the_refresh_is_idempotent() {
        let mut storage = StorageInfo::new();
        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        assert!(storage.next_effect().is_some(), "one measurement");
        assert!(storage.next_effect().is_none(), "…not two");
    }

    /// A failed measurement blanks the figure back to `--`, and does not retry. A stale "8.0 GB"
    /// under "Card free" on a device with no card in it is a lie the screen would keep telling, and
    /// both failure modes blank it because neither produced a figure.
    #[test]
    fn a_failed_measurement_blanks_the_figure_and_is_not_retried() {
        for error in [StorageInfoError::NotMounted, StorageInfoError::ScanFailed] {
            let mut storage = StorageInfo::new();
            storage.admit_intent(StorageInfoIntent::RefreshRequested);
            let token = storage.next_effect().expect("the measurement goes out").token();
            assert!(storage.apply_outcome(StorageInfoOutcome::Measured { token, free_bytes: 8_000 }));
            assert_eq!(storage.free_bytes(), Some(8_000));

            storage.admit_intent(StorageInfoIntent::RefreshRequested);
            let token = storage.next_effect().expect("the refresh goes out").token();
            assert!(storage.apply_outcome(StorageInfoOutcome::Failed { token, error }));
            assert_eq!(storage.free_bytes(), None, "{error:?} leaves the rider a `--`, never a stale count");
            assert!(storage.next_effect().is_none(), "and nothing re-armed itself");
            assert!(storage.next_effect().is_none());
        }
    }

    /// A cancellation is the executor saying it did not try, so the figure the rider is looking at
    /// is exactly as true as it was.
    #[test]
    fn an_abandoned_measurement_leaves_the_figure_alone() {
        let mut storage = StorageInfo::new();
        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        let token = storage.next_effect().expect("the measurement goes out").token();
        assert!(storage.apply_outcome(StorageInfoOutcome::Measured { token, free_bytes: 8_000 }));

        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        let token = storage.next_effect().expect("the refresh goes out").token();
        assert!(storage.apply_outcome(StorageInfoOutcome::Cancelled { token }));
        assert_eq!(storage.free_bytes(), Some(8_000));
    }

    /// A superseded or repeated answer changes nothing: the terminal answer invalidates the token
    /// the way every domain owner must.
    #[test]
    fn a_repeated_answer_is_no_longer_current() {
        let mut storage = StorageInfo::new();
        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        let token = storage.next_effect().expect("the measurement goes out").token();
        let answer = StorageInfoOutcome::Measured { token, free_bytes: 1_000 };
        assert!(storage.apply_outcome(answer));
        assert!(!storage.apply_outcome(answer), "a duplicate of a terminal answer is not current");
    }
}
