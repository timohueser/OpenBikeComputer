//! The three state machines of §15, as pure typed transitions.
//!
//! §15 names them exactly: upload is
//! `claimed -> prepared -> streaming -> sealed -> validating -> publishing -> terminal`, download is
//! `resolving -> pinned -> streaming -> completed -> released`, and a direct mutation is
//! `claimed -> validating -> publishing -> terminal`. "Any claimed operation except InstallUpdate
//! ... may enter `aborting` in place of its next phase."
//!
//! Nothing here touches a session, a buffer, or a byte. A transition is a total function of state
//! and event, so the illegal ones are a value ([`IllegalTransition`]) rather than a panic, and the
//! engine above reports them as `internal/invariant` instead of advancing on a state it cannot be
//! in. The wire projection of §8.1 lives here too, because the phase byte a `QueryOperation`
//! reports must come from the same value the engine advances.

use crate::registry::Phase;

/// A refused transition: this event has no meaning in this state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalTransition;

/// The upload machine of §15, including the manifest stream of a finalized draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadPhase {
    /// The durable claim exists and a session has been issued; §8.1 projects both onto `prepared`.
    Prepared,
    /// Payload bytes are being accepted.
    Streaming,
    /// Length and CRC are verified and the generation is sealed.
    Sealed,
    /// The typed validator is running.
    Validating,
    /// The catalog commit is in flight.
    Publishing,
    /// Unwinding towards a durable Aborted result.
    Aborting,
    /// The durable result has replaced the claim. Not a phase: §8.1 reports it as a result.
    Terminal,
}

/// What can happen to an upload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadEvent {
    /// A payload frame was accepted.
    BytesAccepted,
    /// FinishUpload asked for the seal.
    Finish,
    /// The bytes are sealed.
    Sealed,
    /// The validator accepted them.
    Validated,
    /// The commit is durable.
    Published,
    /// Something failed, or the client cancelled: unwind.
    Abandon,
    /// The durable Aborted result exists.
    Aborted,
}

impl UploadPhase {
    /// Applies one event.
    pub const fn apply(self, event: UploadEvent) -> Result<Self, IllegalTransition> {
        Ok(match (self, event) {
            (UploadPhase::Prepared | UploadPhase::Streaming, UploadEvent::BytesAccepted) => UploadPhase::Streaming,
            (UploadPhase::Prepared | UploadPhase::Streaming, UploadEvent::Finish) => UploadPhase::Sealed,
            (UploadPhase::Sealed, UploadEvent::Sealed) => UploadPhase::Validating,
            (UploadPhase::Validating, UploadEvent::Validated) => UploadPhase::Publishing,
            (UploadPhase::Publishing, UploadEvent::Published) => UploadPhase::Terminal,
            (UploadPhase::Aborting, UploadEvent::Aborted) => UploadPhase::Terminal,
            (
                UploadPhase::Prepared
                | UploadPhase::Streaming
                | UploadPhase::Sealed
                | UploadPhase::Validating
                | UploadPhase::Publishing,
                UploadEvent::Abandon,
            ) => UploadPhase::Aborting,
            _ => return Err(IllegalTransition),
        })
    }

    /// True while the session may still accept payload bytes.
    pub const fn accepts_bytes(self) -> bool {
        matches!(self, UploadPhase::Prepared | UploadPhase::Streaming)
    }

    /// The §8.1 phase byte, or `None` once the claim is terminal.
    pub const fn wire_phase(self) -> Option<Phase> {
        Some(match self {
            UploadPhase::Prepared => Phase::Prepared,
            UploadPhase::Streaming => Phase::Streaming,
            UploadPhase::Sealed => Phase::Sealed,
            UploadPhase::Validating => Phase::Validating,
            UploadPhase::Publishing => Phase::Publishing,
            UploadPhase::Aborting => Phase::Aborting,
            UploadPhase::Terminal => return None,
        })
    }
}

/// The download machine of §7 and §15. It is not a claimed operation and has no phase byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadPhase {
    /// StartDownload is admitted and the head is being resolved.
    Resolving,
    /// The head is resolved and the RAM lease is taken.
    Pinned,
    /// Frames are going out at the session's next offset.
    Streaming,
    /// FinishDownload verified length and CRC.
    Completed,
    /// The lease is given back, exactly once.
    Released,
}

/// What can happen to a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadEvent {
    /// The head resolved and the lease was taken.
    Pinned,
    /// A frame went out.
    FrameSent,
    /// FinishDownload verified the whole source.
    Finished,
    /// AbortSession, teardown, a terminal stream fault, or the successful finish's release.
    Release,
}

impl DownloadPhase {
    /// Applies one event.
    pub const fn apply(self, event: DownloadEvent) -> Result<Self, IllegalTransition> {
        Ok(match (self, event) {
            (DownloadPhase::Resolving, DownloadEvent::Pinned) => DownloadPhase::Pinned,
            (DownloadPhase::Pinned | DownloadPhase::Streaming, DownloadEvent::FrameSent) => DownloadPhase::Streaming,
            (DownloadPhase::Pinned | DownloadPhase::Streaming, DownloadEvent::Finished) => DownloadPhase::Completed,
            (DownloadPhase::Pinned | DownloadPhase::Streaming | DownloadPhase::Completed, DownloadEvent::Release) => {
                DownloadPhase::Released
            }
            _ => return Err(IllegalTransition),
        })
    }

    /// True while the source may still be read.
    pub const fn is_streamable(self) -> bool {
        matches!(self, DownloadPhase::Pinned | DownloadPhase::Streaming)
    }
}

/// The direct-mutation machine of §15: DeleteObject, SetMetadata, and AbortOperation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPhase {
    /// The claim is durable and the typed check is running.
    Validating,
    /// The catalog commit is in flight.
    Publishing,
    /// Unwinding towards a durable Aborted result.
    Aborting,
    /// The durable result has replaced the claim.
    Terminal,
}

/// What can happen to a direct mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEvent {
    /// The typed check passed.
    Validated,
    /// The commit is durable.
    Published,
    /// Something failed: unwind.
    Abandon,
    /// The durable Aborted result exists.
    Aborted,
}

impl CommandPhase {
    /// Applies one event.
    pub const fn apply(self, event: CommandEvent) -> Result<Self, IllegalTransition> {
        Ok(match (self, event) {
            (CommandPhase::Validating, CommandEvent::Validated) => CommandPhase::Publishing,
            (CommandPhase::Publishing, CommandEvent::Published) => CommandPhase::Terminal,
            (CommandPhase::Aborting, CommandEvent::Aborted) => CommandPhase::Terminal,
            (CommandPhase::Validating | CommandPhase::Publishing, CommandEvent::Abandon) => CommandPhase::Aborting,
            _ => return Err(IllegalTransition),
        })
    }

    /// The §8.1 phase byte, or `None` once the claim is terminal.
    pub const fn wire_phase(self) -> Option<Phase> {
        Some(match self {
            CommandPhase::Validating => Phase::Validating,
            CommandPhase::Publishing => Phase::Publishing,
            CommandPhase::Aborting => Phase::Aborting,
            CommandPhase::Terminal => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upload_machine_walks_exactly_the_sequence_section_15_names() {
        let mut phase = UploadPhase::Prepared;
        for event in [
            UploadEvent::BytesAccepted,
            UploadEvent::Finish,
            UploadEvent::Sealed,
            UploadEvent::Validated,
            UploadEvent::Published,
        ] {
            phase = phase.apply(event).unwrap();
        }
        assert_eq!(phase, UploadPhase::Terminal);
        assert_eq!(phase.wire_phase(), None);
    }

    #[test]
    fn every_pre_terminal_upload_phase_can_enter_aborting_and_nothing_leaves_terminal() {
        for phase in [
            UploadPhase::Prepared,
            UploadPhase::Streaming,
            UploadPhase::Sealed,
            UploadPhase::Validating,
            UploadPhase::Publishing,
        ] {
            assert_eq!(phase.apply(UploadEvent::Abandon), Ok(UploadPhase::Aborting));
        }
        assert_eq!(UploadPhase::Terminal.apply(UploadEvent::Abandon), Err(IllegalTransition));
        assert_eq!(UploadPhase::Terminal.apply(UploadEvent::Finish), Err(IllegalTransition));
        assert_eq!(UploadPhase::Aborting.apply(UploadEvent::BytesAccepted), Err(IllegalTransition));
    }

    #[test]
    fn a_sealed_upload_no_longer_accepts_bytes() {
        assert!(UploadPhase::Prepared.accepts_bytes());
        assert!(UploadPhase::Streaming.accepts_bytes());
        for phase in [UploadPhase::Sealed, UploadPhase::Validating, UploadPhase::Publishing, UploadPhase::Aborting] {
            assert!(!phase.accepts_bytes());
        }
    }

    #[test]
    fn a_download_ends_at_released_rather_than_completed() {
        let mut phase = DownloadPhase::Resolving;
        for event in [DownloadEvent::Pinned, DownloadEvent::FrameSent, DownloadEvent::Finished, DownloadEvent::Release]
        {
            phase = phase.apply(event).unwrap();
        }
        assert_eq!(phase, DownloadPhase::Released);
        assert_eq!(phase.apply(DownloadEvent::Release), Err(IllegalTransition), "the lease is released exactly once");
    }

    #[test]
    fn a_direct_mutation_has_no_streaming_phase() {
        let mut phase = CommandPhase::Validating;
        for event in [CommandEvent::Validated, CommandEvent::Published] {
            phase = phase.apply(event).unwrap();
        }
        assert_eq!(phase, CommandPhase::Terminal);
        assert_eq!(CommandPhase::Validating.apply(CommandEvent::Published), Err(IllegalTransition));
    }
}
