//! The typed commands the engine hands out, and the outcomes it takes back.
//!
//! The engine performs no storage I/O and owns no transport. Everything it needs done is a
//! [`Command`] the board glue executes against the DOS2 transaction seam, and everything it learns
//! is an [`Outcome`] fed back through [`Engine::resume`](super::Engine::resume). That is what makes
//! the same engine drivable by a fake transaction in a test and by the real kernel on the board,
//! and what keeps the state machines here provable without a card.
//!
//! Commands are named after the transaction lifecycle they drive — claim, append, checkpoint, seal,
//! validate, publish, abort — plus the two a download needs (resolve and read) and the one the
//! device-control plane needs (§16 state that is not the object system's).

use crate::error::{detail, presence, ErrorBody, ErrorCategory, Owner, RetryGuidance};
use crate::frame::Opcode;
use crate::ids::{LogicalObjectId, OperationId, Revision, StoreId};
use crate::registry::ObjectKind;
use crate::result::ResultEnvelope;
use crate::upload::Target;

use super::session::PrincipalScope;

/// The intent of a claim, in the form §11's four claim-lock actions need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimIntent {
    /// The idempotency key the producer chose before the work.
    pub operation_id: OperationId,
    /// The principal scope the claim is stored under.
    pub principal: PrincipalScope,
    /// Which operation is claiming.
    pub opcode: Opcode,
    /// The SHA-256 of §11's canonical intent. Byte equality is the whole comparison.
    pub digest: [u8; 32],
    /// The kind the claim is about.
    pub kind: ObjectKind,
    /// Create, or replace with its exact expected revision.
    pub target: Target,
    /// The declared payload length; zero for a direct mutation.
    pub declared_length: u64,
    /// The declared whole-object CRC; zero for a direct mutation.
    pub expected_crc: u32,
}

/// What the claim lock decided (§11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// Action 1: nothing was claimed, and this claim is now durable with empty work.
    Claimed {
        /// The identity the repository assigned or confirmed.
        logical_object_id: LogicalObjectId,
        /// The repository revision observed at admission — a diagnostic snapshot, not a CAS token.
        repository_revision: Revision,
    },
    /// Action 3, live work: the same principal and digest already own live work.
    ///
    /// Under the restart-only profile of §6.1 the device holds no durable progress, so the work is
    /// discarded and restarted and the acceptance carries restart-at-zero.
    Restarted {
        /// The identity the repository assigned or confirmed.
        logical_object_id: LogicalObjectId,
        /// The repository revision observed at admission.
        repository_revision: Revision,
    },
    /// Action 3, terminal success: the retained typed result is replayed.
    Committed(ResultEnvelope),
    /// Action 3, terminal failure: the retained bare body is replayed.
    Aborted(TerminalError),
    /// Action 4: same principal, different intent.
    Conflict,
    /// Action 2: the ID belongs to another principal; no intent is compared or exposed.
    ForeignPrincipal,
    /// Preflight refused before any state was created (§11).
    Refused(FailureCause),
}

/// A resolved, pinned download source (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedSource {
    /// The head's identity.
    pub logical_object_id: LogicalObjectId,
    /// The revision pinned at admission.
    pub revision: Revision,
    /// The whole source length.
    pub total_length: u64,
    /// The whole-source CRC.
    pub crc32: u32,
}

/// A terminal failure as it is retained and replayed (§11).
///
/// The replay form is fixed: owner none, no retry delay, guidance forced to reject permanently, and
/// both claim-status bits set. Only an authoritative conflict revision may survive with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalError {
    /// The category the failure was recorded under.
    pub category: ErrorCategory,
    /// The detail namespace: zero, or the affected ObjectKind for `semanticValidation`.
    pub namespace: u16,
    /// The category-scoped detail.
    pub detail: u16,
    /// The authoritative revision, when the failure was a compare-and-swap.
    pub current_revision: Option<Revision>,
}

impl TerminalError {
    /// The retained body, in the exact replay shape §11 freezes.
    pub fn body(&self) -> ErrorBody<'static> {
        let mut body = ErrorBody::bare(self.category, self.detail, RetryGuidance::REJECT_PERMANENTLY);
        body.detail_namespace = self.namespace;
        body.presence = presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL;
        if let Some(revision) = self.current_revision {
            body.presence |= presence::CURRENT_REVISION;
            body.current_revision = revision;
        }
        body
    }
}

/// Why a command could not be carried out, in the vocabulary of §12.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureCause {
    /// The typed validator refused the sealed bytes, in the kind's own detail namespace.
    SemanticValidation {
        /// The kind that owns the rule.
        kind: ObjectKind,
        /// The registry's semantic detail.
        detail: u16,
    },
    /// A compare-and-swap failed; the authoritative revision comes back with it.
    RevisionConflict {
        /// `object`, `repository`, or `singleton`.
        detail: u16,
        /// The revision the repository actually holds.
        current: Revision,
    },
    /// Declared length or CRC did not match the bytes.
    Checksum {
        /// `wholePayload`, `durablePrefix`, or `cursor`.
        detail: u16,
    },
    /// The medium failed under the command.
    MediaIo {
        /// `read`, `write`, `synchronize`, or `uncertainCommit`.
        detail: u16,
    },
    /// There is no usable medium.
    MediaUnavailable {
        /// `noCard`, `unmounted`, or `recoveryReadOnly`.
        detail: u16,
    },
    /// Admission cannot reserve the space.
    InsufficientSpace {
        /// Bytes the operation needs.
        required: u64,
        /// Bytes the device has.
        available: u64,
    },
    /// A compiled capacity is exhausted.
    ResourceLimit {
        /// The §12 `resourceLimit` detail.
        detail: u16,
    },
    /// The authorized target does not exist.
    ObjectNotFound {
        /// The §12 `objectNotFound` detail.
        detail: u16,
    },
    /// Another owner holds the resource.
    Busy {
        /// The §12 `busy` detail.
        detail: u16,
        /// The owner class to report; never a token.
        owner: Owner,
    },
    /// The device's own invariant broke.
    Internal {
        /// The §12 `internal` detail.
        detail: u16,
    },
}

impl FailureCause {
    /// The category this cause is reported under.
    pub const fn category(&self) -> ErrorCategory {
        match self {
            FailureCause::SemanticValidation { .. } => ErrorCategory::SEMANTIC_VALIDATION,
            FailureCause::RevisionConflict { .. } => ErrorCategory::REVISION_CONFLICT,
            FailureCause::Checksum { .. } => ErrorCategory::CHECKSUM_FAILURE,
            FailureCause::MediaIo { .. } => ErrorCategory::MEDIA_IO,
            FailureCause::MediaUnavailable { .. } => ErrorCategory::MEDIA_UNAVAILABLE,
            FailureCause::InsufficientSpace { .. } => ErrorCategory::INSUFFICIENT_SPACE,
            FailureCause::ResourceLimit { .. } => ErrorCategory::RESOURCE_LIMIT,
            FailureCause::ObjectNotFound { .. } => ErrorCategory::OBJECT_NOT_FOUND,
            FailureCause::Busy { .. } => ErrorCategory::BUSY,
            FailureCause::Internal { .. } => ErrorCategory::INTERNAL,
        }
    }

    /// The live response body, with the two claim-status bits the caller determines (§12).
    ///
    /// "Their values are determined by where the failure occurred, not by the category": a preflight
    /// refusal clears both, a failure against live work sets bit 5, and one that leaves the claim
    /// terminal sets both.
    pub fn body(&self, claim: ClaimStatus) -> ErrorBody<'static> {
        let (detail, guidance) = match self {
            FailureCause::SemanticValidation { detail, .. } => (*detail, RetryGuidance::REJECT_PERMANENTLY),
            FailureCause::RevisionConflict { detail, .. } => (*detail, RetryGuidance::REFRESH),
            FailureCause::Checksum { detail } => (*detail, RetryGuidance::RETRY_SAME_REQUEST),
            FailureCause::MediaIo { detail } => (*detail, RetryGuidance::RECONNECT_THEN_QUERY),
            FailureCause::MediaUnavailable { detail } => (*detail, RetryGuidance::RETRY_AFTER_USER_ACTION),
            FailureCause::InsufficientSpace { .. } => {
                (detail::space::RESERVATION_BYTES, RetryGuidance::RETRY_AFTER_USER_ACTION)
            }
            FailureCause::ResourceLimit { detail } => (*detail, RetryGuidance::RETRY_AFTER_USER_ACTION),
            FailureCause::ObjectNotFound { detail } => (*detail, RetryGuidance::REJECT_PERMANENTLY),
            FailureCause::Busy { detail, .. } => (*detail, RetryGuidance::RETRY_AFTER_OWNER_RELEASE),
            FailureCause::Internal { detail } => (*detail, RetryGuidance::RETRY_AFTER_DELAY),
        };
        let mut body = ErrorBody::bare(self.category(), detail, guidance);
        body.presence = claim.presence();
        match self {
            FailureCause::SemanticValidation { kind, .. } => body.detail_namespace = kind.to_u16(),
            FailureCause::RevisionConflict { current, .. } => {
                body.presence |= presence::CURRENT_REVISION;
                body.current_revision = *current;
            }
            FailureCause::InsufficientSpace { required, available } => {
                body.presence |= presence::REQUIRED_BYTES | presence::AVAILABLE_BYTES;
                body.required_bytes = *required;
                body.available_bytes = *available;
            }
            FailureCause::Busy { owner, .. } => body.owner = *owner,
            _ => {}
        }
        body
    }

    /// The terminal record this cause leaves behind when it aborts a claim.
    pub const fn terminal(&self) -> TerminalError {
        let (namespace, detail, revision) = match self {
            FailureCause::SemanticValidation { kind, detail } => (kind.to_u16(), *detail, None),
            FailureCause::RevisionConflict { detail, current } => (0, *detail, Some(*current)),
            FailureCause::Checksum { detail }
            | FailureCause::MediaIo { detail }
            | FailureCause::MediaUnavailable { detail }
            | FailureCause::ResourceLimit { detail }
            | FailureCause::ObjectNotFound { detail }
            | FailureCause::Busy { detail, .. }
            | FailureCause::Internal { detail } => (0, *detail, None),
            FailureCause::InsufficientSpace { .. } => (0, detail::space::RESERVATION_BYTES, None),
        };
        TerminalError { category: self.category(), namespace, detail, current_revision: revision }
    }
}

/// Where a failure stands relative to the durable claim (§12's bits 5 and 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimStatus {
    /// No durable claim exists for this OperationId under this request.
    None,
    /// The operation is claimed and still live.
    Live,
    /// The claim is terminal and the identifier is spent.
    Terminal,
}

impl ClaimStatus {
    /// The presence bits this status contributes.
    pub const fn presence(self) -> u16 {
        match self {
            ClaimStatus::None => 0,
            ClaimStatus::Live => presence::DURABLE_CLAIM_EXISTS,
            ClaimStatus::Terminal => presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL,
        }
    }
}

/// Why work is being abandoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortCause {
    /// A failure the engine met while advancing the operation.
    Failed(FailureCause),
    /// A stream fault released the session and, restart-only, the work with it (§13).
    StreamFault {
        /// The transport detail that faulted.
        detail: u16,
    },
    /// The link went away under restart-only work (§13).
    LinkLost,
    /// The client asked for it: AbortSession on restart-only work, or AbortOperation (§6.4).
    Cancelled {
        /// The reason the request carried.
        reason: crate::registry::AbortReason,
    },
}

impl AbortCause {
    /// The terminal record this cause leaves.
    pub const fn terminal(&self) -> TerminalError {
        match self {
            AbortCause::Failed(cause) => cause.terminal(),
            AbortCause::StreamFault { detail } => TerminalError {
                category: ErrorCategory::INVALID_OFFSET,
                namespace: 0,
                detail: *detail,
                current_revision: None,
            },
            AbortCause::LinkLost => TerminalError {
                category: ErrorCategory::LINK_LOST,
                namespace: 0,
                detail: detail::link::STREAM,
                current_revision: None,
            },
            AbortCause::Cancelled { reason } => TerminalError {
                category: ErrorCategory::CANCELLED,
                namespace: 0,
                detail: reason.to_u8() as u16,
                current_revision: None,
            },
        }
    }
}

/// One command for the board glue to execute against the transaction seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    /// Run §11's claim lock for this intent, and make the claim durable when it is action 1.
    Claim(ClaimIntent),
    /// Append payload bytes at exactly this offset. Nothing is durable until a checkpoint.
    Append {
        /// The absolute payload offset.
        offset: u64,
        /// The bytes, borrowed from the arriving stream record.
        bytes: &'a [u8],
    },
    /// Synchronize the payload prefix and the work record (§6.2).
    Checkpoint {
        /// The prefix end this checkpoint covers.
        offset: u64,
    },
    /// Verify declared length and whole-object CRC and seal the immutable generation.
    Seal {
        /// The length StartUpload declared.
        declared_length: u64,
        /// The CRC StartUpload declared.
        expected_crc: u32,
    },
    /// Run the kind's typed validator over the sealed bytes.
    Validate,
    /// Recheck the compare-and-swap under the commit lock and publish, in one durable commit.
    Publish,
    /// Durably abandon the claim and release its work.
    Abort(AbortCause),
    /// Resolve the current committed head and take a RAM lease over it (§7).
    Resolve {
        /// The kind named by StartDownload.
        kind: ObjectKind,
        /// The head named by StartDownload.
        logical_object_id: LogicalObjectId,
        /// The accepted start offset.
        start_offset: u64,
    },
    /// Read from the pinned source.
    ReadSource {
        /// The absolute offset to read at.
        offset: u64,
        /// How many bytes the next stream frame can carry.
        length: u16,
    },
    /// Release the reader lease exactly once.
    ReleaseLease,
    /// Answer a device-control request from device state (§16). It claims nothing.
    DeviceControl(DeviceControlRequest<'a>),
    /// Report the state of an OperationId the client asked about (§8.1).
    QueryOperation(OperationId),
}

/// A §16 device-control request, forwarded verbatim for the glue to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceControlRequest<'a> {
    /// `0x0400` — identity and diagnostics.
    GetDeviceStatus,
    /// `0x0401` — read the config block.
    GetConfig,
    /// `0x0402` — write the whole config block.
    SetConfig(crate::control::ConfigBlock),
    /// `0x0403` — offer a trusted time.
    SetClock(crate::control::SetClock),
    /// `0x0404` — remove bonding material.
    ForgetBond(crate::control::ForgetBond),
    /// `0x0405` — echo the payload back byte-identically.
    Echo(&'a [u8]),
    /// `0x0406` — destroy the store and create a new StoreId.
    ResetStore(StoreId),
}

/// The glue's answer to a device-control request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceControlAnswer<'a> {
    /// `GetDeviceStatus`.
    DeviceStatus(crate::control::DeviceStatus),
    /// `GetConfig` or `SetConfig` — both return the block as it stands after the request.
    Config(crate::control::ConfigBlock),
    /// `SetClock`.
    ClockStatus(crate::control::ClockStatus),
    /// `ForgetBond`.
    BondForgotten,
    /// `Echo`, whose payload is byte-identical to the request's.
    Echo(&'a [u8]),
    /// `ResetStore`, carrying the new StoreId.
    ResetStore(StoreId),
    /// The plane refused, with its own body.
    Refused(FailureCause),
}

/// What an operation looks like to `QueryOperation` (§8.1), as the glue reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationReport {
    /// Neither active nor retained.
    Unknown,
    /// Claimed and live.
    InProgress(crate::query::OperationProgress),
    /// Terminal success.
    Committed(ResultEnvelope),
    /// Terminal failure.
    Aborted(TerminalError),
    /// The query is not this principal's to answer (§3: authorization precedes status).
    NotAuthorized,
}

/// What the glue reports back after executing a [`Command`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome<'a> {
    /// The claim lock's decision.
    Claim(ClaimOutcome),
    /// The bytes were accepted into the work buffer.
    Appended,
    /// The prefix and the work record are durable (§6.2).
    Checkpointed {
        /// The durable prefix end.
        durable_offset: u64,
        /// The finalized CRC over `[0, durable_offset)`.
        prefix_crc: u32,
        /// The work record's checkpoint sequence.
        sequence: u32,
    },
    /// Length and CRC verified; the generation is sealed.
    Sealed,
    /// The typed validator accepted the bytes.
    Validated,
    /// The catalog commit and the terminal result are durable.
    ///
    /// The envelope is the typed result of §10 the request's own family returns: an `ObjectResult`
    /// for a logical Put or a direct mutation, an `AbortResult` for an abort command.
    Published(ResultEnvelope),
    /// The claim is durably terminal with this retained body.
    Aborted(TerminalError),
    /// The head is resolved and leased.
    Resolved(PinnedSource),
    /// Source bytes for the next download frame.
    SourceBytes {
        /// The offset they were read at.
        offset: u64,
        /// The bytes.
        bytes: &'a [u8],
    },
    /// The lease is released.
    LeaseReleased,
    /// The device-control plane's answer.
    DeviceControl(DeviceControlAnswer<'a>),
    /// The operation report a QueryOperation asked for.
    OperationReport(OperationReport),
    /// The command failed.
    Failed(FailureCause),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retained_body_carries_both_claim_bits_and_permanent_guidance() {
        let terminal = FailureCause::RevisionConflict { detail: detail::revision::OBJECT, current: Revision::new(43) };
        let body = terminal.terminal().body();
        assert_eq!(body.guidance, RetryGuidance::REJECT_PERMANENTLY);
        assert!(body.durable_claim_exists() && body.claim_is_terminal());
        assert_eq!(body.current_revision, Revision::new(43));
        assert!(body.text.is_empty());
        assert_eq!(body.owner, Owner::NONE);
    }

    #[test]
    fn a_live_failure_reports_where_it_happened_rather_than_its_category() {
        let cause = FailureCause::Checksum { detail: detail::checksum::WHOLE_PAYLOAD };
        assert_eq!(cause.body(ClaimStatus::None).presence, 0);
        assert_eq!(cause.body(ClaimStatus::Live).presence, presence::DURABLE_CLAIM_EXISTS);
        assert_eq!(
            cause.body(ClaimStatus::Terminal).presence,
            presence::DURABLE_CLAIM_EXISTS | presence::CLAIM_IS_TERMINAL
        );
    }

    #[test]
    fn a_semantic_refusal_carries_the_kinds_namespace_and_a_space_refusal_both_byte_counts() {
        let semantic = FailureCause::SemanticValidation { kind: ObjectKind::Weather, detail: 3 };
        let body = semantic.body(ClaimStatus::Terminal);
        assert_eq!(body.detail_namespace, ObjectKind::Weather.to_u16());

        let space = FailureCause::InsufficientSpace { required: 4_096, available: 1_024 };
        let body = space.body(ClaimStatus::None);
        assert_eq!(body.presence, presence::REQUIRED_BYTES | presence::AVAILABLE_BYTES);
        assert_eq!((body.required_bytes, body.available_bytes), (4_096, 1_024));
    }
}
