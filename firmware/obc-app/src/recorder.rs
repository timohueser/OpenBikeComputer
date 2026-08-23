//! The **Recorder** domain protocol: the ride session and its persistence lifecycle (#1436).
//!
//! Recorder owns the ride identity, when a ride starts, when its accumulated samples are worth
//! writing, when a checkpoint is owed, and what "saved" means. The executor writes bytes and
//! reports what happened — it never reconciles a ride session out of several application fields,
//! which is exactly the duplication #1433 §7.3 removes.
//!
//! This module currently holds only the vocabulary; the state machine arrives in a later slice.
//!
//! **Where the samples live.** A track batch is bulk and never rides an effect. Recorder stages its
//! samples in its own bounded buffer and an [`Append`](RecorderEffect::Append) names *how many* are
//! ready; the executor drains that buffer during the permitted phase and reports how many it wrote.

use crate::device_core::{OperationToken, RecorderTag};
use crate::CatalogObjectId;

/// What the rider asks of the ride recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderIntent {
    /// Begin recording a ride.
    Start,
    /// Close the open ride and keep it as a durable ride object.
    Save,
    /// Close the open ride and throw it away.
    Discard,
}

/// One bounded physical recording operation, carrying the [`OperationToken`] Recorder issued.
///
/// There is deliberately no `Start` effect: starting is a Recorder state change, and the first
/// physical work a ride causes is its first [`Append`](RecorderEffect::Append).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderEffect {
    /// Write the `samples` points Recorder has staged.
    Append { token: OperationToken<RecorderTag>, samples: u16 },
    /// Make the ride recoverable across a power loss up to this point.
    Checkpoint { token: OperationToken<RecorderTag> },
    /// Close the ride into a durable ride object.
    Finalize { token: OperationToken<RecorderTag> },
    /// Delete the open ride and its journal.
    Discard { token: OperationToken<RecorderTag> },
}

impl RecorderEffect {
    /// The operation this effect belongs to.
    pub fn token(&self) -> OperationToken<RecorderTag> {
        match self {
            RecorderEffect::Append { token, .. }
            | RecorderEffect::Checkpoint { token }
            | RecorderEffect::Finalize { token }
            | RecorderEffect::Discard { token } => *token,
        }
    }
}

/// Why a recording operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderError {
    /// The medium refused or failed the write. Recorder keeps the samples staged and retries; the
    /// rider sees the recording warning rather than a lost ride.
    Write,
    /// No writable store is mounted — recording cannot proceed at all.
    NoStore,
}

/// The result of one [`RecorderEffect`].
///
/// A failed [`Finalize`](RecorderEffect::Finalize) is a [`Failed`](RecorderOutcome::Failed) with a
/// typed reason, not a silent discard: the epic's "a ride finalize fails after its last checkpoint"
/// trace depends on the ride still existing afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderOutcome {
    /// `samples` staged points reached the medium.
    Appended { token: OperationToken<RecorderTag>, samples: u16 },
    /// The ride is recoverable up to the checkpoint.
    Checkpointed { token: OperationToken<RecorderTag> },
    /// The ride is closed and committed under `ride`.
    Finalized { token: OperationToken<RecorderTag>, ride: CatalogObjectId },
    /// The open ride is gone.
    Discarded { token: OperationToken<RecorderTag> },
    /// The operation failed.
    Failed { token: OperationToken<RecorderTag>, error: RecorderError },
    /// The executor abandoned the operation without completing it.
    Cancelled { token: OperationToken<RecorderTag> },
}

impl RecorderOutcome {
    /// The operation this outcome answers.
    pub fn token(&self) -> OperationToken<RecorderTag> {
        match self {
            RecorderOutcome::Appended { token, .. }
            | RecorderOutcome::Checkpointed { token }
            | RecorderOutcome::Finalized { token, .. }
            | RecorderOutcome::Discarded { token }
            | RecorderOutcome::Failed { token, .. }
            | RecorderOutcome::Cancelled { token } => *token,
        }
    }
}

// Layout tripwires: a recorder message is a token, a count, or a ride identity — never a batch.
const _: () = assert!(core::mem::size_of::<RecorderIntent>() <= 1, "three fieldless requests");
const _: () = assert!(core::mem::size_of::<RecorderEffect>() <= 8, "a token and a sample count");
const _: () = assert!(core::mem::size_of::<RecorderOutcome>() <= 16, "a token and a ride identity");
const _: () = assert!(core::mem::size_of::<RecorderError>() <= 1, "a verdict, not a report");
