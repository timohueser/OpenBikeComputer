//! Uploads, checkpoints, and the two aborts (`Device_Object_Protocol_v3.md` §6.1 through §6.4).
//!
//! ## Zero is not a sentinel
//!
//! §6.1: "Target mode alone distinguishes the two encodings: in create mode both fields are
//! constrained to zero because there is nothing yet to name, and in replace mode both carry
//! arbitrary opaque `u64` values, zero included, exactly as the repository reported them. A device
//! MUST NOT treat a zero LogicalObjectId or a zero expected Revision in replace mode as absent, as
//! a wildcard, or as a create request."
//!
//! [`Target`] is how that survives contact with a type system: create carries no identity fields at
//! all, so there is no zero to misread, and replace carries both unconditionally.
//!
//! ## The three resumable acceptances agree on their flag word
//!
//! UploadAccepted, DraftPartAccepted, and the FinalizeDraft acceptance all put [`AcceptanceFlags`]
//! in a `u16` at offset 2 — resumed-work bit 0, restart-at-zero bit 1 — while offset 1 is whatever
//! that message needs. "so one decoder reads the flag word identically in all three." The two flags
//! are never both set, and restart-at-zero forces the reported durable offset and finalized prefix
//! CRC to zero; all three rules are enforced here rather than left to the reader.

use crate::codec::{bytes16_at, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::error::{reject_nonzero, DecodeError};
use crate::ids::{LogicalObjectId, OperationId, Revision, SessionId};
use crate::metadata::{MetadataEnvelope, MAX_PUT_ENVELOPE};
use crate::registry::{object_kind, AbortReason, ObjectKind};
use crate::result::ResultEnvelope;
use crate::{BufferTooSmall, EncodeResult};

/// The fixed prefix of a StartUpload request, before its metadata envelope.
pub const START_UPLOAD_PREFIX_LEN: usize = 48;

/// The smallest StartUpload payload: the prefix plus an empty envelope.
pub const MIN_START_UPLOAD_LEN: usize = START_UPLOAD_PREFIX_LEN + 8;

/// The largest StartUpload payload the *schema ceiling* allows (§1). No registered kind reaches it.
pub const MAX_START_UPLOAD_LEN: usize = START_UPLOAD_PREFIX_LEN + MAX_PUT_ENVELOPE;

/// The largest StartUpload a conforming v3.0 device actually produces: weather's 68-byte envelope.
pub const MAX_PRODUCIBLE_START_UPLOAD_LEN: usize = START_UPLOAD_PREFIX_LEN + 68;

/// The UploadAccepted acceptance body, frozen at this size by the vectors contract §2.1.
pub const UPLOAD_ACCEPTANCE_LEN: usize = 64;

/// The CheckpointUpload request.
pub const CHECKPOINT_REQUEST_LEN: usize = 12;

/// The CheckpointUpload response.
pub const CHECKPOINT_RESPONSE_LEN: usize = 20;

/// The FinishUpload request: exactly a SessionId.
pub const FINISH_UPLOAD_LEN: usize = 4;

/// The AbortSession request.
pub const ABORT_SESSION_LEN: usize = 8;

/// The AbortOperation request.
pub const ABORT_OPERATION_LEN: usize = 40;

/// The common four-byte disposition prefix of a `Start*` acceptance.
pub const DISPOSITION_PREFIX_LEN: usize = 4;

/// Create versus replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TargetMode {
    /// Create: there is nothing yet to name.
    Create = 0,
    /// Replace: compare-and-swap against an exact expected revision.
    Replace = 1,
}

impl TargetMode {
    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(TargetMode::Create),
            1 => Some(TargetMode::Replace),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            TargetMode::Create => "create",
            TargetMode::Replace => "replace",
        }
    }
}

/// What a mutation names: a creation, or an exact `(LogicalObjectId, Revision)` compare-and-swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Create. Both identity fields encode as zero.
    Create,
    /// Replace the named entry at exactly this revision.
    Replace {
        /// The identity the repository reported.
        logical_object_id: LogicalObjectId,
        /// The entry Revision the repository last reported for it.
        expected_revision: Revision,
    },
}

impl Target {
    /// The wire mode byte.
    pub const fn mode(self) -> TargetMode {
        match self {
            Target::Create => TargetMode::Create,
            Target::Replace { .. } => TargetMode::Replace,
        }
    }

    /// The wire LogicalObjectId field: zero in create mode.
    pub const fn logical_object_id(self) -> LogicalObjectId {
        match self {
            Target::Create => LogicalObjectId::ZERO,
            Target::Replace { logical_object_id, .. } => logical_object_id,
        }
    }

    /// The wire expected-Revision field: zero in create mode.
    pub const fn expected_revision(self) -> Revision {
        match self {
            Target::Create => Revision::ZERO,
            Target::Replace { expected_revision, .. } => expected_revision,
        }
    }

    /// Decodes the `(mode, id, revision)` triple, rejecting every other combination.
    pub(crate) fn decode(mode: u8, logical_object_id: u64, expected_revision: u64) -> crate::Result<Self> {
        match TargetMode::from_u8(mode).ok_or_else(DecodeError::unknown_enum)? {
            TargetMode::Create => {
                if logical_object_id != 0 || expected_revision != 0 {
                    return Err(DecodeError::invalid_combination());
                }
                Ok(Target::Create)
            }
            TargetMode::Replace => Ok(Target::Replace {
                logical_object_id: LogicalObjectId::new(logical_object_id),
                expected_revision: Revision::new(expected_revision),
            }),
        }
    }
}

/// The resume byte: a preference, not a demand, with exactly two legal values (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResumePreference {
    /// Discard any durable work and restart at zero.
    RestartAtZero = 0,
    /// Resume from the last durable checkpoint if the device holds one and the kind allows it.
    ResumePermitted = 1,
}

impl ResumePreference {
    /// Decodes a wire `u8`. "Any other value is `invalidDescriptor/unknownEnum`."
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(ResumePreference::RestartAtZero),
            1 => Some(ResumePreference::ResumePermitted),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            ResumePreference::RestartAtZero => "restartAtZero",
            ResumePreference::ResumePermitted => "resumePermitted",
        }
    }

    /// §6.1's admission table: what a device reports for this preference given the durable work it
    /// holds and whether the kind advertises resumable upload.
    ///
    /// "every combination is accepted — a resume is never a reason to refuse an upload".
    pub const fn admit(self, durable_work_present: bool, kind_is_resumable: bool) -> AcceptanceFlags {
        match (self, durable_work_present, kind_is_resumable) {
            (ResumePreference::ResumePermitted, true, true) => AcceptanceFlags::RESUMED,
            (ResumePreference::ResumePermitted, true, false) => AcceptanceFlags::RESTARTED,
            (ResumePreference::RestartAtZero, true, _) => AcceptanceFlags::RESTARTED,
            (_, false, _) => AcceptanceFlags::NONE,
        }
    }
}

/// The `u16` flag word at offset 2 of all three resumable acceptances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AcceptanceFlags(u16);

impl AcceptanceFlags {
    /// Bit 0 — durable work was resumed.
    pub const RESUMED_WORK: u16 = 1 << 0;
    /// Bit 1 — durable work was discarded and the client streams from byte zero.
    pub const RESTART_AT_ZERO: u16 = 1 << 1;
    /// Every defined bit; the rest are zero.
    pub const ALL: u16 = Self::RESUMED_WORK | Self::RESTART_AT_ZERO;

    /// Neither flag: there was no durable work.
    pub const NONE: AcceptanceFlags = AcceptanceFlags(0);
    /// Resumed at the last durable checkpoint.
    pub const RESUMED: AcceptanceFlags = AcceptanceFlags(Self::RESUMED_WORK);
    /// Restarted at zero.
    pub const RESTARTED: AcceptanceFlags = AcceptanceFlags(Self::RESTART_AT_ZERO);

    /// Wraps a raw word, rejecting undefined bits and the combination §6.1 forbids.
    pub fn decode(bits: u16) -> crate::Result<Self> {
        if bits & !Self::ALL != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        if bits == Self::ALL {
            // "Restart-at-zero and resumed-work are never both set."
            return Err(DecodeError::invalid_combination());
        }
        Ok(AcceptanceFlags(bits))
    }

    /// The raw word.
    pub const fn bits(self) -> u16 {
        self.0
    }

    /// True when durable work was resumed.
    pub const fn resumed_work(self) -> bool {
        self.0 & Self::RESUMED_WORK != 0
    }

    /// True when the client must stream from byte zero.
    pub const fn restart_at_zero(self) -> bool {
        self.0 & Self::RESTART_AT_ZERO != 0
    }
}

/// The StartUpload request (§6.1). It is only ever a logical-object Put.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartUpload<'a> {
    /// The idempotency key, chosen before the request.
    pub operation_id: OperationId,
    /// The logical kind.
    pub kind: ObjectKind,
    /// Create, or replace at an exact revision.
    pub target: Target,
    /// Whether the client permits a resume.
    pub resume: ResumePreference,
    /// The declared payload length.
    pub declared_length: u64,
    /// The declared whole-object CRC-32/IEEE.
    pub expected_crc32: u32,
    /// Exactly one metadata envelope.
    pub metadata: MetadataEnvelope<'a>,
}

impl<'a> StartUpload<'a> {
    /// The exact encoded payload length.
    pub fn encoded_len(&self) -> usize {
        START_UPLOAD_PREFIX_LEN + self.metadata.encoded_len()
    }

    /// Decodes a StartUpload payload.
    pub fn decode(payload: &'a [u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, MIN_START_UPLOAD_LEN)?;
        let (metadata, used) = MetadataEnvelope::decode_prefix(&payload[START_UPLOAD_PREFIX_LEN..], MAX_PUT_ENVELOPE)?;
        if START_UPLOAD_PREFIX_LEN + used != payload.len() {
            return Err(DecodeError::trailing_bytes());
        }
        Ok(StartUpload {
            operation_id: OperationId::new(bytes16_at(payload, 0)),
            kind: object_kind(u16_at(payload, 16))?,
            target: Target::decode(payload[18], u64_at(payload, 20), u64_at(payload, 28))?,
            resume: ResumePreference::from_u8(payload[19]).ok_or_else(DecodeError::unknown_enum)?,
            declared_length: u64_at(payload, 36),
            expected_crc32: u32_at(payload, 44),
            metadata,
        })
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        put_bytes(out, 0, self.operation_id.as_bytes());
        put_u16(out, 16, self.kind.to_u16());
        out[18] = self.target.mode().to_u8();
        out[19] = self.resume.to_u8();
        put_u64(out, 20, self.target.logical_object_id().get());
        put_u64(out, 28, self.target.expected_revision().get());
        put_u64(out, 36, self.declared_length);
        put_u32(out, 44, self.expected_crc32);
        self.metadata.encode_into(&mut out[START_UPLOAD_PREFIX_LEN..])?;
        Ok(needed)
    }
}

/// The 64-byte accepted body of an UploadAccepted (§6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadAcceptance {
    /// Echoed target mode.
    pub target_mode: TargetMode,
    /// Resumed-work / restart-at-zero.
    pub flags: AcceptanceFlags,
    /// Echoed OperationId.
    pub operation_id: OperationId,
    /// A fresh stream capability.
    pub session_id: SessionId,
    /// The assigned or named logical identity.
    pub logical_object_id: LogicalObjectId,
    /// The repository revision observed at admission — a diagnostic snapshot, **not** the next CAS
    /// token (§6.1).
    pub admission_revision: Revision,
    /// The authoritative durable next offset.
    pub durable_next_offset: u64,
    /// The checkpoint granule for this session.
    pub checkpoint_granule: u32,
    /// The largest stream payload this session accepts.
    pub max_stream_payload: u16,
    /// Finalized CRC-32/IEEE over `[0, durable_next_offset)`; zero exactly when that offset is zero.
    pub finalized_prefix_crc32: u32,
}

/// Rejects the acceptance invariants shared by all three resumable acceptances.
pub(crate) fn check_acceptance_invariants(
    flags: AcceptanceFlags,
    durable_next_offset: u64,
    prefix_crc32: u32,
) -> crate::Result<()> {
    if flags.restart_at_zero() && (durable_next_offset != 0 || prefix_crc32 != 0) {
        // §6.1: "Restart-at-zero forces the reported durable next offset to zero and the finalized
        // prefix CRC to zero".
        return Err(DecodeError::invalid_combination());
    }
    if durable_next_offset == 0 && prefix_crc32 != 0 {
        // §6.1: the CRC "is zero when the durable next offset is zero, which is the only case in
        // which zero is not a computed CRC".
        return Err(DecodeError::invalid_combination());
    }
    if flags.resumed_work() && durable_next_offset == 0 {
        // Resumed work reports the last durable checkpoint, and a checkpoint at offset zero is not
        // one: §6.1's table gives "no durable work" both flags clear.
        return Err(DecodeError::invalid_combination());
    }
    Ok(())
}

impl UploadAcceptance {
    /// Decodes exactly [`UPLOAD_ACCEPTANCE_LEN`] bytes of accepted body.
    pub fn decode(body: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(body, UPLOAD_ACCEPTANCE_LEN)?;
        reject_nonzero(body, 54, 2)?;
        reject_nonzero(body, 60, 4)?;
        let flags = AcceptanceFlags::decode(u16_at(body, 2))?;
        let durable_next_offset = u64_at(body, 40);
        let finalized_prefix_crc32 = u32_at(body, 56);
        check_acceptance_invariants(flags, durable_next_offset, finalized_prefix_crc32)?;
        Ok(UploadAcceptance {
            target_mode: TargetMode::from_u8(body[1]).ok_or_else(DecodeError::unknown_enum)?,
            flags,
            operation_id: OperationId::new(bytes16_at(body, 4)),
            session_id: SessionId::new(u32_at(body, 20)).ok_or_else(DecodeError::unknown_enum)?,
            logical_object_id: LogicalObjectId::new(u64_at(body, 24)),
            admission_revision: Revision::new(u64_at(body, 32)),
            durable_next_offset,
            checkpoint_granule: u32_at(body, 48),
            max_stream_payload: u16_at(body, 52),
            finalized_prefix_crc32,
        })
    }

    /// Encodes the accepted body, disposition byte included.
    pub fn encode(&self) -> [u8; UPLOAD_ACCEPTANCE_LEN] {
        let mut out = [0u8; UPLOAD_ACCEPTANCE_LEN];
        out[0] = 0;
        out[1] = self.target_mode.to_u8();
        put_u16(&mut out, 2, self.flags.bits());
        put_bytes(&mut out, 4, self.operation_id.as_bytes());
        put_u32(&mut out, 20, self.session_id.get());
        put_u64(&mut out, 24, self.logical_object_id.get());
        put_u64(&mut out, 32, self.admission_revision.get());
        put_u64(&mut out, 40, self.durable_next_offset);
        put_u32(&mut out, 48, self.checkpoint_granule);
        put_u16(&mut out, 52, self.max_stream_payload);
        put_u32(&mut out, 56, self.finalized_prefix_crc32);
        out
    }
}

/// A `Start*` response: an acceptance, or the retained terminal result of the same intent.
///
/// A retained **Aborted** operation is not a disposition at all — §6.1: "it returns a
/// `response|error` control frame containing exactly its bare 48-byte, text-free terminal
/// ErrorBody" — so it arrives as [`crate::Response::Error`], not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition<T> {
    /// Disposition `0`: accepted, with a session.
    Accepted(T),
    /// Disposition `1`: already terminal, replaying the retained result.
    AlreadyTerminal(ResultEnvelope),
}

impl<T> Disposition<T> {
    /// The disposition byte.
    pub const fn to_u8(&self) -> u8 {
        match self {
            Disposition::Accepted(_) => 0,
            Disposition::AlreadyTerminal(_) => 1,
        }
    }

    /// The name used in fixture JSON.
    pub const fn name(&self) -> &'static str {
        match self {
            Disposition::Accepted(_) => "accepted",
            Disposition::AlreadyTerminal(_) => "alreadyTerminal",
        }
    }
}

/// Decodes the `already terminal` shape: a four-byte disposition/reserved prefix, then the
/// retained [`ResultEnvelope`].
pub(crate) fn decode_terminal(payload: &[u8]) -> crate::Result<ResultEnvelope> {
    DecodeError::min_len(payload, DISPOSITION_PREFIX_LEN)?;
    reject_nonzero(payload, 1, 3)?;
    ResultEnvelope::decode(&payload[DISPOSITION_PREFIX_LEN..])
}

/// Encodes that same shape.
pub(crate) fn encode_terminal(out: &mut [u8], envelope: &ResultEnvelope) -> EncodeResult {
    let needed = DISPOSITION_PREFIX_LEN + envelope.encoded_len();
    if out.len() < needed {
        return Err(BufferTooSmall { needed, available: out.len() });
    }
    out[0] = 1;
    out[1..DISPOSITION_PREFIX_LEN].fill(0);
    envelope.encode_into(&mut out[DISPOSITION_PREFIX_LEN..needed])?;
    Ok(needed)
}

/// The UploadAccepted response.
pub type UploadAccepted = Disposition<UploadAcceptance>;

impl UploadAccepted {
    /// Decodes an UploadAccepted payload.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, 1)?;
        match payload[0] {
            0 => Ok(Disposition::Accepted(UploadAcceptance::decode(payload)?)),
            1 => Ok(Disposition::AlreadyTerminal(decode_terminal(payload)?)),
            _ => Err(DecodeError::unknown_enum()),
        }
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        match self {
            Disposition::Accepted(body) => {
                let encoded = body.encode();
                if out.len() < encoded.len() {
                    return Err(BufferTooSmall { needed: encoded.len(), available: out.len() });
                }
                out[..encoded.len()].copy_from_slice(&encoded);
                Ok(encoded.len())
            }
            Disposition::AlreadyTerminal(envelope) => encode_terminal(out, envelope),
        }
    }
}

/// A CheckpointUpload request (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointUpload {
    /// The session being checkpointed.
    pub session_id: SessionId,
    /// The session's in-memory next offset.
    pub received_next_offset: u64,
}

impl CheckpointUpload {
    /// Decodes exactly [`CHECKPOINT_REQUEST_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, CHECKPOINT_REQUEST_LEN)?;
        Ok(CheckpointUpload {
            session_id: SessionId::new(u32_at(payload, 0)).ok_or_else(DecodeError::unknown_enum)?,
            received_next_offset: u64_at(payload, 4),
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; CHECKPOINT_REQUEST_LEN] {
        let mut out = [0u8; CHECKPOINT_REQUEST_LEN];
        put_u32(&mut out, 0, self.session_id.get());
        put_u64(&mut out, 4, self.received_next_offset);
        out
    }

    /// §6.2's boundary rule: the offset "is an exact multiple of the checkpoint granule, except at
    /// the declared end, where it equals the declared length".
    pub fn is_on_boundary(&self, granule: u32, declared_length: u64) -> bool {
        self.received_next_offset == declared_length
            || (granule != 0 && self.received_next_offset.is_multiple_of(u64::from(granule)))
    }
}

/// A CheckpointUpload response (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointAccepted {
    /// The session.
    pub session_id: SessionId,
    /// The offset that is now durable.
    pub durable_next_offset: u64,
    /// Finalized CRC-32/IEEE over exactly `[0, durable_next_offset)`.
    pub finalized_prefix_crc32: u32,
    /// The work record's checkpoint sequence: `1` for the first, strictly increasing, never
    /// wrapping, and scoped to the work record rather than the session — so it continues across a
    /// resume.
    pub checkpoint_sequence: u32,
}

impl CheckpointAccepted {
    /// Decodes exactly [`CHECKPOINT_RESPONSE_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, CHECKPOINT_RESPONSE_LEN)?;
        let checkpoint_sequence = u32_at(payload, 16);
        if checkpoint_sequence == 0 {
            // "The checkpoint sequence starts at `1` for the first durable checkpoint."
            return Err(DecodeError::invalid_combination());
        }
        Ok(CheckpointAccepted {
            session_id: SessionId::new(u32_at(payload, 0)).ok_or_else(DecodeError::unknown_enum)?,
            durable_next_offset: u64_at(payload, 4),
            finalized_prefix_crc32: u32_at(payload, 12),
            checkpoint_sequence,
        })
    }

    /// Encodes the response.
    pub fn encode(&self) -> [u8; CHECKPOINT_RESPONSE_LEN] {
        let mut out = [0u8; CHECKPOINT_RESPONSE_LEN];
        put_u32(&mut out, 0, self.session_id.get());
        put_u64(&mut out, 4, self.durable_next_offset);
        put_u32(&mut out, 12, self.finalized_prefix_crc32);
        put_u32(&mut out, 16, self.checkpoint_sequence);
        out
    }
}

/// The FinishUpload request: exactly a SessionId (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinishUpload {
    /// The session to seal.
    pub session_id: SessionId,
}

impl FinishUpload {
    /// Decodes exactly [`FINISH_UPLOAD_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, FINISH_UPLOAD_LEN)?;
        Ok(FinishUpload { session_id: SessionId::new(u32_at(payload, 0)).ok_or_else(DecodeError::unknown_enum)? })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; FINISH_UPLOAD_LEN] {
        self.session_id.get().to_le_bytes()
    }
}

/// The AbortSession request (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbortSession {
    /// The session to detach.
    pub session_id: SessionId,
    /// Why.
    pub reason: AbortReason,
}

impl AbortSession {
    /// Decodes exactly [`ABORT_SESSION_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, ABORT_SESSION_LEN)?;
        reject_nonzero(payload, 5, 3)?;
        Ok(AbortSession {
            session_id: SessionId::new(u32_at(payload, 0)).ok_or_else(DecodeError::unknown_enum)?,
            reason: AbortReason::from_u8(payload[4]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; ABORT_SESSION_LEN] {
        let mut out = [0u8; ABORT_SESSION_LEN];
        put_u32(&mut out, 0, self.session_id.get());
        out[4] = self.reason.to_u8();
        out
    }
}

/// The AbortSession response: exactly one byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AbortSessionOutcome {
    /// The session was detached.
    Detached = 0,
    /// The operation was already terminal, so there was no session left to detach.
    AlreadyTerminal = 1,
}

impl AbortSessionOutcome {
    /// Decodes exactly one byte.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, 1)?;
        match payload[0] {
            0 => Ok(AbortSessionOutcome::Detached),
            1 => Ok(AbortSessionOutcome::AlreadyTerminal),
            _ => Err(DecodeError::unknown_enum()),
        }
    }

    /// Encodes the response.
    pub const fn encode(self) -> [u8; 1] {
        [self as u8]
    }

    /// The name used in fixture JSON.
    pub const fn name(self) -> &'static str {
        match self {
            AbortSessionOutcome::Detached => "detached",
            AbortSessionOutcome::AlreadyTerminal => "alreadyTerminal",
        }
    }
}

/// The AbortOperation request: the explicit persistent cancellation command (§6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbortOperation {
    /// A new OperationId for the abort command itself. It claims the reserved
    /// cancellation/recovery slot rather than a normal claim slot (§11).
    pub operation_id: OperationId,
    /// The operation to cancel — a draft parent or an ordinary operation.
    pub target_operation_id: OperationId,
    /// Why.
    pub reason: AbortReason,
}

impl AbortOperation {
    /// Decodes exactly [`ABORT_OPERATION_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, ABORT_OPERATION_LEN)?;
        reject_nonzero(payload, 33, 7)?;
        Ok(AbortOperation {
            operation_id: OperationId::new(bytes16_at(payload, 0)),
            target_operation_id: OperationId::new(bytes16_at(payload, 16)),
            reason: AbortReason::from_u8(payload[32]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; ABORT_OPERATION_LEN] {
        let mut out = [0u8; ABORT_OPERATION_LEN];
        put_bytes(&mut out, 0, self.operation_id.as_bytes());
        put_bytes(&mut out, 16, self.target_operation_id.as_bytes());
        out[32] = self.reason.to_u8();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::StoreId;
    use crate::metadata::{MetadataWriter, SchemaClass};
    use crate::registry::ObjectOutcome;
    use crate::result::ObjectResult;
    use std::vec;

    fn route_put(buffer: &mut [u8]) -> MetadataEnvelope<'_> {
        let mut writer = MetadataWriter::new(buffer).unwrap();
        writer.push(0x8001, &[2]).unwrap();
        let bytes = writer.finish(ObjectKind::Route, SchemaClass::Put);
        MetadataEnvelope::decode(bytes, MAX_PUT_ENVELOPE).unwrap()
    }

    #[test]
    fn start_upload_create_encodes_both_identity_fields_as_zero() {
        let mut buffer = [0u8; 32];
        let request = StartUpload {
            operation_id: OperationId::new([0x0A; 16]),
            kind: ObjectKind::Route,
            target: Target::Create,
            resume: ResumePreference::ResumePermitted,
            declared_length: 12_345,
            expected_crc32: 0xDEAD_BEEF,
            metadata: route_put(&mut buffer),
        };
        let mut out = [0u8; MAX_START_UPLOAD_LEN];
        let len = request.encode_into(&mut out).unwrap();
        assert_eq!(len, 48 + 13);
        assert!(out[20..36].iter().all(|&b| b == 0));
        assert_eq!(StartUpload::decode(&out[..len]).unwrap(), request);
    }

    #[test]
    fn replace_mode_treats_zero_as_an_ordinary_value() {
        let mut buffer = [0u8; 32];
        let request = StartUpload {
            operation_id: OperationId::new([1; 16]),
            kind: ObjectKind::Weather,
            target: Target::Replace { logical_object_id: LogicalObjectId::ZERO, expected_revision: Revision::ZERO },
            resume: ResumePreference::RestartAtZero,
            declared_length: 1,
            expected_crc32: 0,
            metadata: MetadataEnvelope::empty(ObjectKind::Weather, SchemaClass::Put),
        };
        let _ = &mut buffer;
        let mut out = [0u8; MAX_START_UPLOAD_LEN];
        let len = request.encode_into(&mut out).unwrap();
        let decoded = StartUpload::decode(&out[..len]).unwrap();
        assert_eq!(
            decoded.target,
            Target::Replace { logical_object_id: LogicalObjectId::ZERO, expected_revision: Revision::ZERO }
        );
    }

    #[test]
    fn create_mode_with_a_nonzero_identity_is_an_invalid_combination() {
        let mut buffer = [0u8; 32];
        let request = StartUpload {
            operation_id: OperationId::new([1; 16]),
            kind: ObjectKind::Route,
            target: Target::Create,
            resume: ResumePreference::RestartAtZero,
            declared_length: 1,
            expected_crc32: 0,
            metadata: route_put(&mut buffer),
        };
        let mut out = [0u8; MAX_START_UPLOAD_LEN];
        let len = request.encode_into(&mut out).unwrap();
        put_u64(&mut out, 20, 5);
        assert_eq!(StartUpload::decode(&out[..len]).unwrap_err(), DecodeError::invalid_combination());
        put_u64(&mut out, 20, 0);
        put_u64(&mut out, 28, 5);
        assert_eq!(StartUpload::decode(&out[..len]).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn the_resume_byte_has_exactly_two_legal_values() {
        let mut buffer = [0u8; 32];
        let request = StartUpload {
            operation_id: OperationId::new([1; 16]),
            kind: ObjectKind::Route,
            target: Target::Create,
            resume: ResumePreference::RestartAtZero,
            declared_length: 1,
            expected_crc32: 0,
            metadata: route_put(&mut buffer),
        };
        let mut out = [0u8; MAX_START_UPLOAD_LEN];
        let len = request.encode_into(&mut out).unwrap();
        out[19] = 2;
        assert_eq!(StartUpload::decode(&out[..len]).unwrap_err(), DecodeError::unknown_enum());
    }

    #[test]
    fn the_resume_admission_table_matches_the_spec_row_for_row() {
        use ResumePreference::*;
        assert_eq!(ResumePermitted.admit(true, true), AcceptanceFlags::RESUMED);
        assert_eq!(ResumePermitted.admit(true, false), AcceptanceFlags::RESTARTED);
        assert_eq!(ResumePermitted.admit(false, true), AcceptanceFlags::NONE);
        assert_eq!(ResumePermitted.admit(false, false), AcceptanceFlags::NONE);
        assert_eq!(RestartAtZero.admit(true, true), AcceptanceFlags::RESTARTED);
        assert_eq!(RestartAtZero.admit(true, false), AcceptanceFlags::RESTARTED);
        assert_eq!(RestartAtZero.admit(false, true), AcceptanceFlags::NONE);
        assert_eq!(RestartAtZero.admit(false, false), AcceptanceFlags::NONE);
    }

    fn acceptance(flags: AcceptanceFlags, offset: u64, crc: u32) -> UploadAcceptance {
        UploadAcceptance {
            target_mode: TargetMode::Create,
            flags,
            operation_id: OperationId::new([2; 16]),
            session_id: SessionId::new(1).unwrap(),
            logical_object_id: LogicalObjectId::new(4),
            admission_revision: Revision::new(11),
            durable_next_offset: offset,
            checkpoint_granule: 262_144,
            max_stream_payload: 1008,
            finalized_prefix_crc32: crc,
        }
    }

    #[test]
    fn upload_acceptance_is_sixty_four_bytes_and_rejects_the_pre_freeze_size() {
        let body = acceptance(AcceptanceFlags::NONE, 0, 0);
        let bytes = body.encode();
        assert_eq!(bytes.len(), 64);
        assert_eq!(UploadAccepted::decode(&bytes).unwrap(), Disposition::Accepted(body));
        // The 56-byte pre-freeze twin must fail rather than decode short.
        assert_eq!(UploadAccepted::decode(&bytes[..56]).unwrap_err(), DecodeError::truncated());
    }

    #[test]
    fn acceptance_flag_invariants_are_enforced() {
        let mut bytes = acceptance(AcceptanceFlags::NONE, 0, 0).encode();
        put_u16(&mut bytes, 2, AcceptanceFlags::ALL);
        assert_eq!(UploadAccepted::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());

        // Restart-at-zero with a nonzero durable offset.
        let mut bytes = acceptance(AcceptanceFlags::RESTARTED, 0, 0).encode();
        put_u64(&mut bytes, 40, 4096);
        assert_eq!(UploadAccepted::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());

        // A nonzero CRC over an empty prefix.
        let mut bytes = acceptance(AcceptanceFlags::NONE, 0, 0).encode();
        put_u32(&mut bytes, 56, 7);
        assert_eq!(UploadAccepted::decode(&bytes).unwrap_err(), DecodeError::invalid_combination());

        // A "flag" written into the byte at offset 1, which is target mode here.
        let mut bytes = acceptance(AcceptanceFlags::NONE, 0, 0).encode();
        bytes[1] = 2;
        assert_eq!(UploadAccepted::decode(&bytes).unwrap_err(), DecodeError::unknown_enum());

        // A real resume: nonzero offset with a real prefix CRC.
        let body = acceptance(AcceptanceFlags::RESUMED, 262_144, 0xCBF4_3926);
        let bytes = body.encode();
        assert_eq!(UploadAccepted::decode(&bytes).unwrap(), Disposition::Accepted(body));
    }

    #[test]
    fn already_terminal_disposition_carries_a_result_envelope() {
        let envelope = ResultEnvelope::Object(ObjectResult {
            operation_id: OperationId::new([3; 16]),
            store_id: StoreId::new([4; 16]),
            kind: ObjectKind::Route,
            outcome: ObjectOutcome::Committed,
            logical_object_id: LogicalObjectId::new(1),
            revision: Revision::new(2),
            length: 10,
            crc32: 3,
        });
        let response: UploadAccepted = Disposition::AlreadyTerminal(envelope);
        let mut out = [0u8; 128];
        let len = response.encode_into(&mut out).unwrap();
        assert_eq!(len, 4 + 68);
        assert_eq!(UploadAccepted::decode(&out[..len]).unwrap(), response);
        out[2] = 1;
        assert_eq!(UploadAccepted::decode(&out[..len]).unwrap_err(), DecodeError::reserved_bits());
    }

    #[test]
    fn checkpoint_round_trips_and_pins_its_boundary_rule() {
        let request = CheckpointUpload { session_id: SessionId::new(9).unwrap(), received_next_offset: 524_288 };
        assert_eq!(CheckpointUpload::decode(&request.encode()).unwrap(), request);
        assert!(request.is_on_boundary(262_144, 1_000_000));
        let end = CheckpointUpload { session_id: SessionId::new(9).unwrap(), received_next_offset: 1_000_000 };
        assert!(end.is_on_boundary(262_144, 1_000_000));
        let ragged = CheckpointUpload { session_id: SessionId::new(9).unwrap(), received_next_offset: 5 };
        assert!(!ragged.is_on_boundary(262_144, 1_000_000));

        let response = CheckpointAccepted {
            session_id: SessionId::new(9).unwrap(),
            durable_next_offset: 524_288,
            finalized_prefix_crc32: 0x1234_5678,
            checkpoint_sequence: 2,
        };
        assert_eq!(CheckpointAccepted::decode(&response.encode()).unwrap(), response);
        let mut zero_sequence = response.encode();
        put_u32(&mut zero_sequence, 16, 0);
        assert_eq!(CheckpointAccepted::decode(&zero_sequence).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn the_small_session_messages_round_trip() {
        let finish = FinishUpload { session_id: SessionId::new(3).unwrap() };
        assert_eq!(FinishUpload::decode(&finish.encode()).unwrap(), finish);

        let abort = AbortSession { session_id: SessionId::new(3).unwrap(), reason: AbortReason::UserRequested };
        assert_eq!(AbortSession::decode(&abort.encode()).unwrap(), abort);
        let mut bad = abort.encode();
        bad[5] = 1;
        assert_eq!(AbortSession::decode(&bad).unwrap_err(), DecodeError::reserved_bits());
        let mut bad = abort.encode();
        bad[4] = 0;
        assert_eq!(AbortSession::decode(&bad).unwrap_err(), DecodeError::unknown_enum());

        assert_eq!(AbortSessionOutcome::decode(&[0]).unwrap(), AbortSessionOutcome::Detached);
        assert_eq!(AbortSessionOutcome::decode(&[1]).unwrap(), AbortSessionOutcome::AlreadyTerminal);
        assert_eq!(AbortSessionOutcome::decode(&[2]).unwrap_err(), DecodeError::unknown_enum());
        assert_eq!(AbortSessionOutcome::decode(&[]).unwrap_err(), DecodeError::truncated());

        let command = AbortOperation {
            operation_id: OperationId::new([5; 16]),
            target_operation_id: OperationId::new([6; 16]),
            reason: AbortReason::Superseded,
        };
        assert_eq!(AbortOperation::decode(&command.encode()).unwrap(), command);
        let mut trailing = vec![0u8; ABORT_OPERATION_LEN + 1];
        trailing[..ABORT_OPERATION_LEN].copy_from_slice(&command.encode());
        assert_eq!(AbortOperation::decode(&trailing).unwrap_err(), DecodeError::trailing_bytes());
    }
}
