//! The writes to the ride-archive Metadata object: the Assistant checkpoint handshake and the trip
//! progress records. The mounted-card Metadata writer owns durable serialization.
use crate::device_core::{MetadataTag, OperationToken, StoreRevision, TokenSource};
use crate::trip::TripProgress;
use obc_formats::trip_progress::Records;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataError {
    Unsupported,
    WriteFailed,
    Busy,
    RemountRequired,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataEffect {
    WriteCheckpoint {
        token: OperationToken<MetadataTag>,
        scope: Option<StoreRevision>,
    },
    /// Write [`App::trip_progress_payload`](crate::App::trip_progress_payload) by the bound rules.
    WriteProgress {
        token: OperationToken<MetadataTag>,
        scope: Option<StoreRevision>,
    },
}
impl MetadataEffect {
    pub fn token(self) -> OperationToken<MetadataTag> {
        let (Self::WriteCheckpoint { token, .. } | Self::WriteProgress { token, .. }) = self;
        token
    }
    pub fn scope(self) -> Option<StoreRevision> {
        let (Self::WriteCheckpoint { scope, .. } | Self::WriteProgress { scope, .. }) = self;
        scope
    }
    pub(crate) fn bind(&mut self, current: Option<StoreRevision>) {
        let (Self::WriteCheckpoint { scope, .. } | Self::WriteProgress { scope, .. }) = self;
        *scope = current;
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataOutcome {
    CheckpointWritten { token: OperationToken<MetadataTag> },
    ProgressWritten { token: OperationToken<MetadataTag> },
    Failed { token: OperationToken<MetadataTag>, error: MetadataError },
    Cancelled { token: OperationToken<MetadataTag> },
}
impl MetadataOutcome {
    pub fn token(self) -> OperationToken<MetadataTag> {
        match self {
            Self::CheckpointWritten { token }
            | Self::ProgressWritten { token }
            | Self::Failed { token, .. }
            | Self::Cancelled { token } => token,
        }
    }
}
pub(crate) struct MetadataMachine {
    ops: TokenSource<MetadataTag>,
    inflight: bool,
    blocked: bool,
    /// The trip progress records as the last catalog read found them, plus every Finish since.
    progress: Records,
    /// A Finish's record that the store does not hold yet.
    progress_owed: Option<TripProgress>,
    /// The write in flight is the owed record.
    writing_progress: bool,
}
impl MetadataMachine {
    pub const fn new() -> Self {
        Self {
            ops: TokenSource::new(),
            inflight: false,
            blocked: false,
            progress: Records::new(),
            progress_owed: None,
            writing_progress: false,
        }
    }
    fn issue(&mut self) -> Option<OperationToken<MetadataTag>> {
        if self.inflight || self.blocked {
            return None;
        }
        self.inflight = true;
        Some(self.ops.issue())
    }
    pub(crate) fn next_checkpoint_effect(&mut self) -> Option<MetadataEffect> {
        self.issue().map(|token| MetadataEffect::WriteCheckpoint { token, scope: None })
    }
    pub(crate) fn next_progress_effect(&mut self) -> Option<MetadataEffect> {
        self.progress_owed.as_ref()?;
        let token = self.issue()?;
        self.writing_progress = true;
        Some(MetadataEffect::WriteProgress { token, scope: None })
    }
    /// The owed record, while `token` is the write in flight.
    pub(crate) fn progress_payload(&self, token: OperationToken<MetadataTag>) -> Option<&TripProgress> {
        self.progress_owed.as_ref().filter(|_| self.writing_progress && self.ops.is_current(token))
    }
    pub(crate) fn apply_outcome(&mut self, outcome: MetadataOutcome) -> bool {
        if !self.inflight || !self.ops.is_current(outcome.token()) {
            return false;
        }
        // Busy and a store the write never reached retry; any other failure loses the record.
        let retry = matches!(
            outcome,
            MetadataOutcome::Failed { error: MetadataError::Busy, .. } | MetadataOutcome::Cancelled { .. }
        );
        if core::mem::take(&mut self.writing_progress) && !retry {
            self.progress_owed = None;
        }
        self.ops.invalidate();
        self.inflight = false;
        self.blocked = matches!(outcome, MetadataOutcome::Failed { error: MetadataError::RemountRequired, .. });
        true
    }
    /// A Finish's record. The resident records take it at once; the store takes it when it can.
    pub(crate) fn owe_progress(&mut self, record: TripProgress, stored: impl Fn(u64) -> bool) {
        obc_formats::trip_progress::record(&mut self.progress, record.clone(), stored);
        self.progress_owed = Some(record);
    }
    pub(crate) fn progress(&self) -> &[TripProgress] {
        &self.progress
    }
    /// The catalog read's records replace the resident ones.
    pub(crate) fn set_progress(&mut self, records: impl IntoIterator<Item = TripProgress>) {
        self.progress.clear();
        for record in records {
            obc_formats::trip_progress::record(&mut self.progress, record, |_| true);
        }
    }
    pub(crate) fn reset_store(&mut self) {
        self.ops.invalidate();
        self.inflight = false;
        self.writing_progress = false;
        self.blocked = false;
    }
}

#[cfg(test)]
impl MetadataMachine {
    pub(crate) fn assert_boot_state(&self) {
        assert!(
            !self.inflight
                && !self.blocked
                && self.progress.is_empty()
                && self.progress_owed.is_none()
                && !self.writing_progress
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_answers_cannot_unlock_a_pending_write_or_uncertain_card() {
        let mut machine = MetadataMachine::new();
        let first = machine.next_checkpoint_effect().unwrap().token();
        assert!(machine.next_checkpoint_effect().is_none());
        machine.reset_store();
        let current = machine.next_checkpoint_effect().unwrap().token();
        assert!(!machine.apply_outcome(MetadataOutcome::CheckpointWritten { token: first }));
        assert!(machine.next_checkpoint_effect().is_none());
        assert!(
            machine.apply_outcome(MetadataOutcome::Failed { token: current, error: MetadataError::RemountRequired })
        );
        assert!(!machine.apply_outcome(MetadataOutcome::CheckpointWritten { token: current }));
        assert!(machine.next_checkpoint_effect().is_none());
        machine.reset_store();
        let retry = machine.next_checkpoint_effect().unwrap().token();
        assert!(machine.apply_outcome(MetadataOutcome::Cancelled { token: retry }));
        assert!(machine.next_checkpoint_effect().is_some());
    }
}
