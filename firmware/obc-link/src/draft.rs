//! Multipart drafts: BeginDraft, StartDraftPart, and FinalizeDraft
//! (`Device_Object_Protocol_v3.md` §6.5).
//!
//! A draft is identified by its parent `OperationId`; each part has a child `OperationId`. §11 makes
//! FinalizeDraft the one carve-out from the four claim actions: "It addresses an existing claim —
//! the BeginDraft parent — **by OperationId alone**: it computes no canonical intent, makes no
//! second claim", which is why its request is exactly sixteen bytes and it has no intent suffix.
//!
//! Note what is *not* in a DraftPartAccepted: the `DraftPartRef`. §6.5: "that opaque reference does
//! not exist until sealing is durable and appears only in DraftPartResult and QueryDraft's sealed
//! entry."

use crate::codec::{bytes16_at, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::error::{reject_nonzero, DecodeError};
use crate::ids::{LogicalObjectId, OperationId, Revision, SessionId};
use crate::registry::{draft_part_kind, object_kind, DraftPartKind, ObjectKind};
use crate::upload::{
    check_acceptance_invariants, decode_terminal, encode_terminal, AcceptanceFlags, Disposition, ResumePreference,
    Target,
};
use crate::{BufferTooSmall, EncodeResult};

/// The BeginDraft request.
pub const BEGIN_DRAFT_LEN: usize = 52;

/// The BeginDraft accepted body, disposition prefix included.
pub const BEGIN_DRAFT_ACCEPTANCE_LEN: usize = 32;

/// The StartDraftPart request.
pub const START_DRAFT_PART_LEN: usize = 64;

/// The DraftPartAccepted accepted body, frozen at this size by the vectors contract §2.1.
pub const DRAFT_PART_ACCEPTANCE_LEN: usize = 72;

/// The FinalizeDraft request: exactly the parent OperationId.
pub const FINALIZE_DRAFT_LEN: usize = 16;

/// The FinalizeDraft accepted body, frozen at this size by the vectors contract §2.1.
pub const FINALIZE_ACCEPTANCE_LEN: usize = 64;

/// The BeginDraft request (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginDraft {
    /// The parent operation, which claims the future logical publication.
    pub parent_operation_id: OperationId,
    /// The kind the draft will publish. v3.0 uses volume manifest.
    pub kind: ObjectKind,
    /// Create, or replace at an exact revision — the same rules as StartUpload.
    pub target: Target,
    /// The declared final manifest length.
    pub declared_manifest_length: u64,
    /// The declared final manifest CRC-32/IEEE.
    pub declared_manifest_crc32: u32,
    /// The exact number of parts, nonzero and within the advertised maximum.
    pub expected_part_count: u16,
}

impl BeginDraft {
    /// Decodes exactly [`BEGIN_DRAFT_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, BEGIN_DRAFT_LEN)?;
        if payload[19] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        reject_nonzero(payload, 50, 2)?;
        let expected_part_count = u16_at(payload, 48);
        if expected_part_count == 0 {
            // §6.5: "The exact part count is nonzero and no greater than the advertised maximum."
            // The upper bound is device capacity, which a codec cannot know; zero is structural.
            return Err(DecodeError::invalid_combination());
        }
        Ok(BeginDraft {
            parent_operation_id: OperationId::new(bytes16_at(payload, 0)),
            kind: object_kind(u16_at(payload, 16))?,
            target: Target::decode(payload[18], u64_at(payload, 20), u64_at(payload, 28))?,
            declared_manifest_length: u64_at(payload, 36),
            declared_manifest_crc32: u32_at(payload, 44),
            expected_part_count,
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; BEGIN_DRAFT_LEN] {
        let mut out = [0u8; BEGIN_DRAFT_LEN];
        put_bytes(&mut out, 0, self.parent_operation_id.as_bytes());
        put_u16(&mut out, 16, self.kind.to_u16());
        out[18] = self.target.mode().to_u8();
        put_u64(&mut out, 20, self.target.logical_object_id().get());
        put_u64(&mut out, 28, self.target.expected_revision().get());
        put_u64(&mut out, 36, self.declared_manifest_length);
        put_u32(&mut out, 44, self.declared_manifest_crc32);
        put_u16(&mut out, 48, self.expected_part_count);
        out
    }
}

/// The state byte of an open draft parent. Only `open` exists on this response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DraftParentState {
    /// Open and accepting children.
    Open = 0,
}

/// The 28 bytes BeginDraft returns after its four-byte disposition prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeginDraftAcceptance {
    /// Echoed parent operation.
    pub parent_operation_id: OperationId,
    /// The draft revision, which begins at 1 and increments when a child is durably claimed,
    /// sealed, or durably aborted — never for a payload checkpoint.
    pub draft_revision: u64,
    /// The declared part count, echoed.
    pub expected_part_count: u16,
    /// The parent state.
    pub state: DraftParentState,
}

impl BeginDraftAcceptance {
    /// Decodes exactly [`BEGIN_DRAFT_ACCEPTANCE_LEN`] bytes, disposition prefix included.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, BEGIN_DRAFT_ACCEPTANCE_LEN)?;
        reject_nonzero(payload, 1, 3)?;
        if payload[31] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        if payload[30] != 0 {
            return Err(DecodeError::unknown_enum());
        }
        Ok(BeginDraftAcceptance {
            parent_operation_id: OperationId::new(bytes16_at(payload, 4)),
            draft_revision: u64_at(payload, 20),
            expected_part_count: u16_at(payload, 28),
            state: DraftParentState::Open,
        })
    }

    /// Encodes the accepted body, disposition prefix included.
    pub fn encode(&self) -> [u8; BEGIN_DRAFT_ACCEPTANCE_LEN] {
        let mut out = [0u8; BEGIN_DRAFT_ACCEPTANCE_LEN];
        put_bytes(&mut out, 4, self.parent_operation_id.as_bytes());
        put_u64(&mut out, 20, self.draft_revision);
        put_u16(&mut out, 28, self.expected_part_count);
        out[30] = self.state as u8;
        out
    }
}

/// The BeginDraft response.
pub type BeginDraftAccepted = Disposition<BeginDraftAcceptance>;

impl BeginDraftAccepted {
    /// Decodes a BeginDraft response payload.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, 1)?;
        match payload[0] {
            0 => Ok(Disposition::Accepted(BeginDraftAcceptance::decode(payload)?)),
            1 => Ok(Disposition::AlreadyTerminal(decode_terminal(payload)?)),
            _ => Err(DecodeError::unknown_enum()),
        }
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        match self {
            Disposition::Accepted(body) => copy_out(out, &body.encode()),
            Disposition::AlreadyTerminal(envelope) => encode_terminal(out, envelope),
        }
    }
}

fn copy_out(out: &mut [u8], bytes: &[u8]) -> EncodeResult {
    if out.len() < bytes.len() {
        return Err(BufferTooSmall { needed: bytes.len(), available: out.len() });
    }
    out[..bytes.len()].copy_from_slice(bytes);
    Ok(bytes.len())
}

/// The StartDraftPart request (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartDraftPart {
    /// The child operation, distinct from the parent and from every other child.
    pub child_operation_id: OperationId,
    /// The parent draft.
    pub parent_operation_id: OperationId,
    /// The part kind, which must be advertised.
    pub part_kind: DraftPartKind,
    /// The part key. `(kind, key)` is unique within the parent.
    pub part_key: u64,
    /// The declared part length.
    pub declared_length: u64,
    /// The declared part CRC-32/IEEE.
    pub expected_crc32: u32,
    /// Whether the client permits a resume, admitted through §6.1's table unchanged.
    pub resume: ResumePreference,
}

impl StartDraftPart {
    /// Decodes exactly [`START_DRAFT_PART_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, START_DRAFT_PART_LEN)?;
        reject_nonzero(payload, 34, 2)?;
        reject_nonzero(payload, 57, 7)?;
        let child_operation_id = OperationId::new(bytes16_at(payload, 0));
        let parent_operation_id = OperationId::new(bytes16_at(payload, 16));
        if child_operation_id == parent_operation_id {
            // §6.5: "The child OperationId must be distinct from the parent".
            return Err(DecodeError::invalid_combination());
        }
        Ok(StartDraftPart {
            child_operation_id,
            parent_operation_id,
            part_kind: draft_part_kind(u16_at(payload, 32))?,
            part_key: u64_at(payload, 36),
            declared_length: u64_at(payload, 44),
            expected_crc32: u32_at(payload, 52),
            resume: ResumePreference::from_u8(payload[56]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; START_DRAFT_PART_LEN] {
        let mut out = [0u8; START_DRAFT_PART_LEN];
        put_bytes(&mut out, 0, self.child_operation_id.as_bytes());
        put_bytes(&mut out, 16, self.parent_operation_id.as_bytes());
        put_u16(&mut out, 32, self.part_kind.to_u16());
        put_u64(&mut out, 36, self.part_key);
        put_u64(&mut out, 44, self.declared_length);
        put_u32(&mut out, 52, self.expected_crc32);
        out[56] = self.resume.to_u8();
        out
    }
}

/// The 72-byte accepted body of a DraftPartAccepted (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftPartAcceptance {
    /// Resumed-work / restart-at-zero, read exactly as UploadAccepted's.
    pub flags: AcceptanceFlags,
    /// Echoed child operation.
    pub child_operation_id: OperationId,
    /// Echoed parent operation.
    pub parent_operation_id: OperationId,
    /// A fresh stream capability.
    pub session_id: SessionId,
    /// Echoed part kind.
    pub part_kind: DraftPartKind,
    /// Echoed part key.
    pub part_key: u64,
    /// The authoritative durable next offset.
    pub durable_next_offset: u64,
    /// The checkpoint granule.
    pub checkpoint_granule: u32,
    /// The largest stream payload this session accepts.
    pub max_stream_payload: u16,
    /// Finalized CRC over `[0, durable_next_offset)`; zero exactly when that offset is zero.
    pub finalized_prefix_crc32: u32,
}

impl DraftPartAcceptance {
    /// Decodes exactly [`DRAFT_PART_ACCEPTANCE_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, DRAFT_PART_ACCEPTANCE_LEN)?;
        if payload[1] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        reject_nonzero(payload, 42, 2)?;
        reject_nonzero(payload, 66, 2)?;
        let flags = AcceptanceFlags::decode(u16_at(payload, 2))?;
        let durable_next_offset = u64_at(payload, 52);
        let finalized_prefix_crc32 = u32_at(payload, 68);
        check_acceptance_invariants(flags, durable_next_offset, finalized_prefix_crc32)?;
        Ok(DraftPartAcceptance {
            flags,
            child_operation_id: OperationId::new(bytes16_at(payload, 4)),
            parent_operation_id: OperationId::new(bytes16_at(payload, 20)),
            session_id: SessionId::new(u32_at(payload, 36)).ok_or_else(DecodeError::unknown_enum)?,
            part_kind: draft_part_kind(u16_at(payload, 40))?,
            part_key: u64_at(payload, 44),
            durable_next_offset,
            checkpoint_granule: u32_at(payload, 60),
            max_stream_payload: u16_at(payload, 64),
            finalized_prefix_crc32,
        })
    }

    /// Encodes the accepted body.
    pub fn encode(&self) -> [u8; DRAFT_PART_ACCEPTANCE_LEN] {
        let mut out = [0u8; DRAFT_PART_ACCEPTANCE_LEN];
        put_u16(&mut out, 2, self.flags.bits());
        put_bytes(&mut out, 4, self.child_operation_id.as_bytes());
        put_bytes(&mut out, 20, self.parent_operation_id.as_bytes());
        put_u32(&mut out, 36, self.session_id.get());
        put_u16(&mut out, 40, self.part_kind.to_u16());
        put_u64(&mut out, 44, self.part_key);
        put_u64(&mut out, 52, self.durable_next_offset);
        put_u32(&mut out, 60, self.checkpoint_granule);
        put_u16(&mut out, 64, self.max_stream_payload);
        put_u32(&mut out, 68, self.finalized_prefix_crc32);
        out
    }
}

/// The DraftPartAccepted response.
pub type DraftPartAccepted = Disposition<DraftPartAcceptance>;

impl DraftPartAccepted {
    /// Decodes a DraftPartAccepted payload.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, 1)?;
        match payload[0] {
            0 => Ok(Disposition::Accepted(DraftPartAcceptance::decode(payload)?)),
            1 => Ok(Disposition::AlreadyTerminal(decode_terminal(payload)?)),
            _ => Err(DecodeError::unknown_enum()),
        }
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        match self {
            Disposition::Accepted(body) => copy_out(out, &body.encode()),
            Disposition::AlreadyTerminal(envelope) => encode_terminal(out, envelope),
        }
    }
}

/// The FinalizeDraft request: exactly the parent OperationId (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalizeDraft {
    /// The parent whose manifest is being finalized.
    pub parent_operation_id: OperationId,
}

impl FinalizeDraft {
    /// Decodes exactly [`FINALIZE_DRAFT_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, FINALIZE_DRAFT_LEN)?;
        Ok(FinalizeDraft { parent_operation_id: OperationId::new(bytes16_at(payload, 0)) })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; FINALIZE_DRAFT_LEN] {
        self.parent_operation_id.to_bytes()
    }
}

/// The 64-byte parent-manifest acceptance FinalizeDraft returns (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FinalizeAcceptance {
    /// Resumed-work / restart-at-zero for the manifest stream.
    pub flags: AcceptanceFlags,
    /// Echoed parent operation.
    pub parent_operation_id: OperationId,
    /// A fresh stream capability for the manifest bytes.
    pub session_id: SessionId,
    /// The assigned or named logical identity of the release.
    pub logical_object_id: LogicalObjectId,
    /// The repository revision observed at admission — a diagnostic snapshot.
    pub admission_revision: Revision,
    /// The authoritative durable manifest offset.
    pub durable_manifest_offset: u64,
    /// The checkpoint granule.
    pub checkpoint_granule: u32,
    /// The largest stream payload this session accepts.
    pub max_stream_payload: u16,
    /// Finalized CRC over `[0, durable_manifest_offset)`.
    pub finalized_prefix_crc32: u32,
}

impl FinalizeAcceptance {
    /// Decodes exactly [`FINALIZE_ACCEPTANCE_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, FINALIZE_ACCEPTANCE_LEN)?;
        if payload[1] != 0 {
            return Err(DecodeError::reserved_bits());
        }
        reject_nonzero(payload, 54, 2)?;
        reject_nonzero(payload, 60, 4)?;
        let flags = AcceptanceFlags::decode(u16_at(payload, 2))?;
        let durable_manifest_offset = u64_at(payload, 40);
        let finalized_prefix_crc32 = u32_at(payload, 56);
        check_acceptance_invariants(flags, durable_manifest_offset, finalized_prefix_crc32)?;
        Ok(FinalizeAcceptance {
            flags,
            parent_operation_id: OperationId::new(bytes16_at(payload, 4)),
            session_id: SessionId::new(u32_at(payload, 20)).ok_or_else(DecodeError::unknown_enum)?,
            logical_object_id: LogicalObjectId::new(u64_at(payload, 24)),
            admission_revision: Revision::new(u64_at(payload, 32)),
            durable_manifest_offset,
            checkpoint_granule: u32_at(payload, 48),
            max_stream_payload: u16_at(payload, 52),
            finalized_prefix_crc32,
        })
    }

    /// Encodes the accepted body.
    pub fn encode(&self) -> [u8; FINALIZE_ACCEPTANCE_LEN] {
        let mut out = [0u8; FINALIZE_ACCEPTANCE_LEN];
        put_u16(&mut out, 2, self.flags.bits());
        put_bytes(&mut out, 4, self.parent_operation_id.as_bytes());
        put_u32(&mut out, 20, self.session_id.get());
        put_u64(&mut out, 24, self.logical_object_id.get());
        put_u64(&mut out, 32, self.admission_revision.get());
        put_u64(&mut out, 40, self.durable_manifest_offset);
        put_u32(&mut out, 48, self.checkpoint_granule);
        put_u16(&mut out, 52, self.max_stream_payload);
        put_u32(&mut out, 56, self.finalized_prefix_crc32);
        out
    }
}

/// The FinalizeDraft response.
pub type FinalizeDraftAccepted = Disposition<FinalizeAcceptance>;

impl FinalizeDraftAccepted {
    /// Decodes a FinalizeDraft response payload.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, 1)?;
        match payload[0] {
            0 => Ok(Disposition::Accepted(FinalizeAcceptance::decode(payload)?)),
            1 => Ok(Disposition::AlreadyTerminal(decode_terminal(payload)?)),
            _ => Err(DecodeError::unknown_enum()),
        }
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        match self {
            Disposition::Accepted(body) => copy_out(out, &body.encode()),
            Disposition::AlreadyTerminal(envelope) => encode_terminal(out, envelope),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upload::TargetMode;

    fn begin() -> BeginDraft {
        BeginDraft {
            parent_operation_id: OperationId::new([0x31; 16]),
            kind: ObjectKind::VolumeManifest,
            target: Target::Create,
            declared_manifest_length: 96 + 56 * 3,
            declared_manifest_crc32: 0x1111_2222,
            expected_part_count: 3,
        }
    }

    #[test]
    fn begin_draft_round_trips_and_rejects_a_zero_part_count() {
        let request = begin();
        let bytes = request.encode();
        assert_eq!(bytes.len(), 52);
        assert_eq!(BeginDraft::decode(&bytes).unwrap(), request);
        assert_eq!(bytes[18], TargetMode::Create.to_u8());

        let mut zero = bytes;
        put_u16(&mut zero, 48, 0);
        assert_eq!(BeginDraft::decode(&zero).unwrap_err(), DecodeError::invalid_combination());

        let mut reserved = bytes;
        reserved[19] = 1;
        assert_eq!(BeginDraft::decode(&reserved).unwrap_err(), DecodeError::reserved_bits());
    }

    #[test]
    fn begin_draft_acceptance_is_thirty_two_bytes_and_stays_open() {
        let body = BeginDraftAcceptance {
            parent_operation_id: OperationId::new([0x31; 16]),
            draft_revision: 1,
            expected_part_count: 3,
            state: DraftParentState::Open,
        };
        let bytes = body.encode();
        assert_eq!(bytes.len(), 32);
        assert_eq!(BeginDraftAccepted::decode(&bytes).unwrap(), Disposition::Accepted(body));
        let mut bad_state = bytes;
        bad_state[30] = 1;
        assert_eq!(BeginDraftAccepted::decode(&bad_state).unwrap_err(), DecodeError::unknown_enum());
    }

    #[test]
    fn a_child_may_not_reuse_its_parents_operation_id() {
        let request = StartDraftPart {
            child_operation_id: OperationId::new([7; 16]),
            parent_operation_id: OperationId::new([7; 16]),
            part_kind: DraftPartKind::MapShard,
            part_key: 1,
            declared_length: 10,
            expected_crc32: 0,
            resume: ResumePreference::ResumePermitted,
        };
        assert_eq!(StartDraftPart::decode(&request.encode()).unwrap_err(), DecodeError::invalid_combination());
    }

    #[test]
    fn start_draft_part_and_its_acceptance_round_trip_at_their_frozen_sizes() {
        let request = StartDraftPart {
            child_operation_id: OperationId::new([8; 16]),
            parent_operation_id: OperationId::new([7; 16]),
            part_kind: DraftPartKind::TerrainBlob,
            part_key: 0xFFFF_FFFF_FFFF_FFFF,
            declared_length: 1 << 20,
            expected_crc32: 0xABCD_1234,
            resume: ResumePreference::ResumePermitted,
        };
        let bytes = request.encode();
        assert_eq!(bytes.len(), 64);
        assert_eq!(StartDraftPart::decode(&bytes).unwrap(), request);

        let body = DraftPartAcceptance {
            flags: AcceptanceFlags::RESUMED,
            child_operation_id: OperationId::new([8; 16]),
            parent_operation_id: OperationId::new([7; 16]),
            session_id: SessionId::new(4).unwrap(),
            part_kind: DraftPartKind::TerrainBlob,
            part_key: 0xFFFF_FFFF_FFFF_FFFF,
            durable_next_offset: 262_144,
            checkpoint_granule: 262_144,
            max_stream_payload: 4080,
            finalized_prefix_crc32: 0x99AA_BBCC,
        };
        let bytes = body.encode();
        assert_eq!(bytes.len(), 72);
        assert_eq!(DraftPartAccepted::decode(&bytes).unwrap(), Disposition::Accepted(body));
        // The 68-byte pre-freeze twin must fail rather than decode short.
        assert_eq!(DraftPartAccepted::decode(&bytes[..68]).unwrap_err(), DecodeError::truncated());
        // Offset 1 is reserved here, so a flag written into it is rejected.
        let mut flagged = bytes;
        flagged[1] = 1;
        assert_eq!(DraftPartAccepted::decode(&flagged).unwrap_err(), DecodeError::reserved_bits());
    }

    #[test]
    fn finalize_draft_is_sixteen_bytes_and_its_acceptance_is_sixty_four() {
        let request = FinalizeDraft { parent_operation_id: OperationId::new([0x31; 16]) };
        assert_eq!(FinalizeDraft::decode(&request.encode()).unwrap(), request);

        let body = FinalizeAcceptance {
            flags: AcceptanceFlags::RESTARTED,
            parent_operation_id: OperationId::new([0x31; 16]),
            session_id: SessionId::new(5).unwrap(),
            logical_object_id: LogicalObjectId::new(2),
            admission_revision: Revision::new(30),
            durable_manifest_offset: 0,
            checkpoint_granule: 262_144,
            max_stream_payload: 4080,
            finalized_prefix_crc32: 0,
        };
        let bytes = body.encode();
        assert_eq!(bytes.len(), 64);
        assert_eq!(FinalizeDraftAccepted::decode(&bytes).unwrap(), Disposition::Accepted(body));
        assert_eq!(FinalizeDraftAccepted::decode(&bytes[..56]).unwrap_err(), DecodeError::truncated());
        let mut flagged = bytes;
        flagged[1] = 2;
        assert_eq!(FinalizeDraftAccepted::decode(&flagged).unwrap_err(), DecodeError::reserved_bits());
    }
}
