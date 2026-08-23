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
