//! The **StorageInfo** domain protocol: how much room is left on the medium (#1436).
//!
//! The smallest domain in DeviceCore, and the only one with no home of its own outside it — free
//! space belongs to no product feature, it is a fact about the device that the System screen shows.
//! It still gets the full treatment (an intent, a bounded effect, a token-carrying outcome) because
//! measuring free space is a real, slow, failable scan: on the board a FAT free-cluster walk, on the
//! simulator a filesystem query.
//!
//! The legacy shape is [`HostCommand::ScanCardFree`](crate::HostCommand) answered by
//! [`HostEvent::CardScanned`](crate::HostEvent), whose `Option<u64>` folded "unavailable" and
//! "the scan failed" into one `None`. Here they are separate, because they are different sentences
//! to put in front of a rider.

use crate::device_core::{OperationToken, StorageInfoTag};

/// What the UI asks of the storage-information domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInfoIntent {
    /// Re-measure free space — entering the System screen, or a manual refresh. Idempotent: the
    /// domain coalesces a repeat into the measurement already in flight.
    RefreshRequested,
}

/// One bounded physical measurement, carrying the [`OperationToken`] the domain issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInfoEffect {
    /// Measure the bytes still available on the mounted medium.
    MeasureFreeSpace { token: OperationToken<StorageInfoTag> },
}

impl StorageInfoEffect {
    /// The operation this effect belongs to.
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
    /// No medium is mounted.
    NotMounted,
    /// The scan started and failed — a read error part-way through the allocation table.
    ScanFailed,
}

/// The result of one [`StorageInfoEffect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageInfoOutcome {
    /// `free_bytes` are available on the medium.
    Measured { token: OperationToken<StorageInfoTag>, free_bytes: u64 },
    /// The measurement failed.
    Failed { token: OperationToken<StorageInfoTag>, error: StorageInfoError },
    /// The executor abandoned the measurement without completing it.
    Cancelled { token: OperationToken<StorageInfoTag> },
}

impl StorageInfoOutcome {
    /// The operation this outcome answers.
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

// ==================== the StorageInfo state machine (#1397 S2) ====================

use crate::device_core::TokenSource;

/// The storage-information domain: the refresh request, the operation token, and the figure the
/// System screen prints.
///
/// The free-space *number* lives here rather than on `App` because it is this domain's only
/// product state — a measurement it owns end to end, from the request the System screen makes on
/// entry to the value that replaces the `--`.
#[derive(Debug, Default)]
pub struct StorageInfo {
    /// A refresh the executor has not taken yet. Idempotent by construction: a repeat while one is
    /// waiting is the same request, and a repeat while one is *running* re-arms it, because free
    /// space genuinely may have moved since the scan started.
    requested: bool,
    /// The operation token for the measurement an executor is running.
    ops: TokenSource<StorageInfoTag>,
    /// Bytes still available on the mounted medium, or `None` until a measurement succeeds — which
    /// is what the System screen shows as `--`.
    free_bytes: Option<u64>,
}

impl StorageInfo {
    /// The boot state: nothing requested, nothing measured.
    pub(crate) const fn new() -> Self {
        StorageInfo { requested: false, ops: TokenSource::new(), free_bytes: None }
    }

    /// Admit a refresh.
    pub(crate) fn admit_intent(&mut self, intent: StorageInfoIntent) {
        match intent {
            StorageInfoIntent::RefreshRequested => self.requested = true,
        }
    }

    /// The next bounded measurement, or `None` when none is owed.
    pub(crate) fn next_effect(&mut self) -> Option<StorageInfoEffect> {
        core::mem::take(&mut self.requested).then(|| StorageInfoEffect::MeasureFreeSpace { token: self.ops.issue() })
    }

    /// Consume the answer to a measurement. A superseded or repeated answer changes nothing.
    ///
    /// A failure is **reported, not retried**: the screen keeps whatever it last knew (or its `--`)
    /// and the rider can ask again by re-entering. A domain that re-armed itself here would turn a
    /// dead card into a free-cluster walk every pass.
    pub(crate) fn apply_outcome(&mut self, outcome: StorageInfoOutcome) -> bool {
        if !self.ops.is_current(outcome.token()) {
            return false;
        }
        self.ops.invalidate(); // terminal: a duplicate of this answer is no longer current
        self.note_measured(match outcome {
            StorageInfoOutcome::Measured { free_bytes, .. } => Some(free_bytes),
            StorageInfoOutcome::Failed { .. } | StorageInfoOutcome::Cancelled { .. } => None,
        });
        true
    }

    /// A measurement answered with `free_bytes`, or `None` when it did not produce a figure — the
    /// token-free half, for the legacy protocol that carries no token.
    ///
    /// A failure leaves the last known figure alone rather than blanking it: the rider is looking
    /// at a number that was true, and replacing it with `--` because one scan failed says less.
    pub(crate) fn note_measured(&mut self, free_bytes: Option<u64>) {
        if let Some(free_bytes) = free_bytes {
            self.free_bytes = Some(free_bytes);
        }
    }

    /// Whether a refresh is posted but undelivered — the `ScanCardFree` peek.
    pub(crate) fn refresh_pending(&self) -> bool {
        self.requested
    }

    /// Free space on the mounted medium, or `None` until a measurement has answered.
    pub(crate) fn free_bytes(&self) -> Option<u64> {
        self.free_bytes
    }

    /// Assert the boot state, field by field.
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

    /// The request is an idempotent **refresh**: repeats before the measurement leaves coalesce into
    /// the one that is already waiting, and the System screen re-entered while a scan is running
    /// arms the next one rather than a second concurrent walk of the allocation table.
    #[test]
    fn the_refresh_is_idempotent() {
        let mut storage = StorageInfo::new();
        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        assert!(storage.next_effect().is_some(), "one measurement");
        assert!(storage.next_effect().is_none(), "…not two");
    }

    /// A failed scan is a **reported** failure, not a retry loop: the last figure the rider was
    /// shown stands, nothing re-arms itself, and asking again is the rider's move.
    #[test]
    fn a_failed_measurement_is_reported_and_not_retried() {
        let mut storage = StorageInfo::new();
        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        let effect = storage.next_effect().expect("the measurement goes out");
        let token = effect.token();
        assert!(storage.apply_outcome(StorageInfoOutcome::Measured { token, free_bytes: 8_000 }));
        assert_eq!(storage.free_bytes(), Some(8_000));

        storage.admit_intent(StorageInfoIntent::RefreshRequested);
        let effect = storage.next_effect().expect("the refresh goes out");
        let token = effect.token();
        assert!(storage.apply_outcome(StorageInfoOutcome::Failed { token, error: StorageInfoError::ScanFailed }));
        assert_eq!(storage.free_bytes(), Some(8_000), "the figure that was true is not blanked by one bad scan");
        assert!(!storage.refresh_pending(), "and nothing re-armed itself");
        assert!(storage.next_effect().is_none());
    }

    /// A superseded or repeated answer changes nothing — the terminal answer invalidates the token
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
