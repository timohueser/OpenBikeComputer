//! The Assistant checkpoint handshake. The mounted-card Metadata writer owns durable serialization.
use crate::device_core::{MetadataTag, OperationToken, StoreRevision, TokenSource};

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
    WriteCheckpoint { token: OperationToken<MetadataTag>, scope: Option<StoreRevision> },
}
impl MetadataEffect {
    pub fn token(self) -> OperationToken<MetadataTag> {
        let Self::WriteCheckpoint { token, .. } = self;
        token
    }
    pub fn scope(self) -> Option<StoreRevision> {
        let Self::WriteCheckpoint { scope, .. } = self;
        scope
    }
    pub(crate) fn bind(&mut self, current: Option<StoreRevision>) {
        let Self::WriteCheckpoint { scope, .. } = self;
        *scope = current;
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataOutcome {
    CheckpointWritten { token: OperationToken<MetadataTag> },
    Failed { token: OperationToken<MetadataTag>, error: MetadataError },
    Cancelled { token: OperationToken<MetadataTag> },
}
impl MetadataOutcome {
    pub fn token(self) -> OperationToken<MetadataTag> {
        match self {
            Self::CheckpointWritten { token } | Self::Failed { token, .. } | Self::Cancelled { token } => token,
        }
    }
}
pub(crate) struct MetadataMachine {
    ops: TokenSource<MetadataTag>,
    inflight: bool,
    blocked: bool,
}
impl MetadataMachine {
    pub const fn new() -> Self {
        Self { ops: TokenSource::new(), inflight: false, blocked: false }
    }
    pub(crate) fn next_checkpoint_effect(&mut self) -> Option<MetadataEffect> {
        if self.inflight || self.blocked {
            return None;
        }
        self.inflight = true;
        Some(MetadataEffect::WriteCheckpoint { token: self.ops.issue(), scope: None })
    }
    pub(crate) fn apply_outcome(&mut self, outcome: MetadataOutcome) -> bool {
        if !self.inflight || !self.ops.is_current(outcome.token()) {
            return false;
        }
        self.ops.invalidate();
        self.inflight = false;
        self.blocked = matches!(outcome, MetadataOutcome::Failed { error: MetadataError::RemountRequired, .. });
        true
    }
    pub(crate) fn reset_store(&mut self) {
        self.ops.invalidate();
        self.inflight = false;
        self.blocked = false;
    }
}

#[cfg(test)]
impl MetadataMachine {
    pub(crate) fn assert_boot_state(&self) {
        assert!(!self.inflight && !self.blocked);
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
