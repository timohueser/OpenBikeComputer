//! Typed terminal results (`Device_Object_Protocol_v3.md` §10).
//!
//! `ResultEnvelope` is `result_type u8`, three reserved zero bytes, then exactly one typed body.
//! It carries no body length "because it is always the final element of the payload that contains
//! it: a decoder that meets a ResultEnvelope takes the remainder of the frame as its body and MUST
//! reject any trailing byte beyond the typed body's fixed size."
//!
//! That rule is also why §2.1 says a message ending in a `ResultEnvelope` "can never grow a tail in
//! either direction". [`ResultEnvelope::decode`] enforces it: the body is the remainder, and a
//! remainder of the wrong size is a framing error, not a truncated field.

use crate::codec::{bytes16_at, put_bytes, put_u16, put_u32, put_u64, u16_at, u32_at, u64_at};
use crate::error::{reject_nonzero, DecodeError};
use crate::ids::{DraftPartRef, LogicalObjectId, OperationId, Revision, StoreId};
use crate::registry::{draft_part_kind, object_kind, DraftPartKind, ObjectKind, ObjectOutcome};
use crate::{BufferTooSmall, EncodeResult};

/// The envelope's fixed prefix: type byte plus three reserved zero bytes.
pub const ENVELOPE_PREFIX_LEN: usize = 4;

/// `ObjectResult` body size.
pub const OBJECT_RESULT_LEN: usize = 64;

/// `DraftPartResult` body size.
pub const DRAFT_PART_RESULT_LEN: usize = 88;

/// `AbortResult` body size.
pub const ABORT_RESULT_LEN: usize = 56;

/// The largest complete envelope: the prefix plus the largest body.
pub const MAX_RESULT_ENVELOPE_LEN: usize = ENVELOPE_PREFIX_LEN + DRAFT_PART_RESULT_LEN;

/// A logical terminal outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectResult {
    /// The operation this result belongs to.
    pub operation_id: OperationId,
    /// The store it committed in.
    pub store_id: StoreId,
    /// The kind it belongs to.
    pub kind: ObjectKind,
    /// What happened.
    pub outcome: ObjectOutcome,
    /// The logical identity.
    pub logical_object_id: LogicalObjectId,
    /// The new object revision.
    pub revision: Revision,
    /// The committed head's length, or the deleted old head's for a delete.
    pub length: u64,
    /// That head's CRC-32/IEEE.
    pub crc32: u32,
}

impl ObjectResult {
    /// Decodes exactly [`OBJECT_RESULT_LEN`] bytes.
    pub fn decode(body: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(body, OBJECT_RESULT_LEN)?;
        Ok(ObjectResult {
            operation_id: OperationId::new(bytes16_at(body, 0)),
            store_id: StoreId::new(bytes16_at(body, 16)),
            kind: object_kind(u16_at(body, 32))?,
            outcome: ObjectOutcome::from_u16(u16_at(body, 34)).ok_or_else(DecodeError::unknown_enum)?,
            logical_object_id: LogicalObjectId::new(u64_at(body, 36)),
            revision: Revision::new(u64_at(body, 44)),
            length: u64_at(body, 52),
            crc32: u32_at(body, 60),
        })
    }

    /// Encodes the body.
    pub fn encode(&self) -> [u8; OBJECT_RESULT_LEN] {
        let mut out = [0u8; OBJECT_RESULT_LEN];
        put_bytes(&mut out, 0, self.operation_id.as_bytes());
        put_bytes(&mut out, 16, self.store_id.as_bytes());
        put_u16(&mut out, 32, self.kind.to_u16());
        put_u16(&mut out, 34, self.outcome.to_u16());
        put_u64(&mut out, 36, self.logical_object_id.get());
        put_u64(&mut out, 44, self.revision.get());
        put_u64(&mut out, 52, self.length);
        put_u32(&mut out, 60, self.crc32);
        out
    }
}

/// A sealed draft part's terminal result. It has no `LogicalObjectId` and no `GenerationId`: a
/// sealed part is not a logical object and its physical identity never crosses the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftPartResult {
    /// The child operation that sealed the part.
    pub child_operation_id: OperationId,
    /// The store it sealed in.
    pub store_id: StoreId,
    /// The parent draft.
    pub parent_operation_id: OperationId,
    /// The opaque reference minted at seal.
    pub draft_part_ref: DraftPartRef,
    /// The part kind.
    pub part_kind: DraftPartKind,
    /// The part key, unique with the kind inside the parent.
    pub part_key: u64,
    /// The sealed length.
    pub length: u64,
    /// The sealed CRC-32/IEEE.
    pub crc32: u32,
}

impl DraftPartResult {
    /// Decodes exactly [`DRAFT_PART_RESULT_LEN`] bytes.
    pub fn decode(body: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(body, DRAFT_PART_RESULT_LEN)?;
        reject_nonzero(body, 66, 2)?;
        Ok(DraftPartResult {
            child_operation_id: OperationId::new(bytes16_at(body, 0)),
            store_id: StoreId::new(bytes16_at(body, 16)),
            parent_operation_id: OperationId::new(bytes16_at(body, 32)),
            draft_part_ref: DraftPartRef::new(bytes16_at(body, 48)),
            part_kind: draft_part_kind(u16_at(body, 64))?,
            part_key: u64_at(body, 68),
            length: u64_at(body, 76),
            crc32: u32_at(body, 84),
        })
    }

    /// Encodes the body.
    pub fn encode(&self) -> [u8; DRAFT_PART_RESULT_LEN] {
        let mut out = [0u8; DRAFT_PART_RESULT_LEN];
        put_bytes(&mut out, 0, self.child_operation_id.as_bytes());
        put_bytes(&mut out, 16, self.store_id.as_bytes());
        put_bytes(&mut out, 32, self.parent_operation_id.as_bytes());
        put_bytes(&mut out, 48, self.draft_part_ref.as_bytes());
        put_u16(&mut out, 64, self.part_kind.to_u16());
        put_u64(&mut out, 68, self.part_key);
        put_u64(&mut out, 76, self.length);
        put_u32(&mut out, 84, self.crc32);
        out
    }
}

/// What an `AbortOperation` command did to its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AbortDisposition {
    /// The target was nonterminal and is now durably Aborted.
    Cancelled = 0,
    /// The target was already terminal and is unchanged.
    AlreadyTerminal = 1,
    /// No such target — returned only when authorization can be established without leaking
    /// another principal's target.
    AlreadyAbsent = 2,
}

impl AbortDisposition {
    /// Every disposition, in wire order.
    pub const ALL: [AbortDisposition; 3] =
        [AbortDisposition::Cancelled, AbortDisposition::AlreadyTerminal, AbortDisposition::AlreadyAbsent];

    /// Decodes a wire `u8`.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(AbortDisposition::Cancelled),
            1 => Some(AbortDisposition::AlreadyTerminal),
            2 => Some(AbortDisposition::AlreadyAbsent),
            _ => None,
        }
    }

    /// The wire `u8`.
    pub const fn to_u8(self) -> u8 {
        self as u8
    }

    /// The name used in fixture JSON and diagnostics.
    pub const fn name(self) -> &'static str {
        match self {
            AbortDisposition::Cancelled => "cancelled",
            AbortDisposition::AlreadyTerminal => "alreadyTerminal",
            AbortDisposition::AlreadyAbsent => "alreadyAbsent",
        }
    }
}

/// The typed success of an `AbortOperation` command.
///
/// §6.4: "The target parent never receives an AbortResult; that typed success belongs only to the
/// separate abort command."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbortResult {
    /// The abort command's own operation.
    pub operation_id: OperationId,
    /// The store.
    pub store_id: StoreId,
    /// The operation the command named.
    pub target_operation_id: OperationId,
    /// What it did.
    pub disposition: AbortDisposition,
}

impl AbortResult {
    /// Decodes exactly [`ABORT_RESULT_LEN`] bytes.
    pub fn decode(body: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(body, ABORT_RESULT_LEN)?;
        reject_nonzero(body, 49, 7)?;
        Ok(AbortResult {
            operation_id: OperationId::new(bytes16_at(body, 0)),
            store_id: StoreId::new(bytes16_at(body, 16)),
            target_operation_id: OperationId::new(bytes16_at(body, 32)),
            disposition: AbortDisposition::from_u8(body[48]).ok_or_else(DecodeError::unknown_enum)?,
        })
    }

    /// Encodes the body.
    pub fn encode(&self) -> [u8; ABORT_RESULT_LEN] {
        let mut out = [0u8; ABORT_RESULT_LEN];
        put_bytes(&mut out, 0, self.operation_id.as_bytes());
        put_bytes(&mut out, 16, self.store_id.as_bytes());
        put_bytes(&mut out, 32, self.target_operation_id.as_bytes());
        out[48] = self.disposition.to_u8();
        out
    }
}

/// One typed terminal result and its discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultEnvelope {
    /// A logical outcome.
    Object(ObjectResult),
    /// A sealed draft part.
    DraftPart(DraftPartResult),
    /// An abort command's success.
    Abort(AbortResult),
}

impl ResultEnvelope {
    /// The `result_type` byte.
    pub const fn result_type(&self) -> u8 {
        match self {
            ResultEnvelope::Object(_) => 1,
            ResultEnvelope::DraftPart(_) => 2,
            ResultEnvelope::Abort(_) => 3,
        }
    }

    /// The name used in fixture JSON.
    pub const fn name(&self) -> &'static str {
        match self {
            ResultEnvelope::Object(_) => "ObjectResult",
            ResultEnvelope::DraftPart(_) => "DraftPartResult",
            ResultEnvelope::Abort(_) => "AbortResult",
        }
    }

    /// The exact encoded length: prefix plus the typed body.
    pub const fn encoded_len(&self) -> usize {
        ENVELOPE_PREFIX_LEN
            + match self {
                ResultEnvelope::Object(_) => OBJECT_RESULT_LEN,
                ResultEnvelope::DraftPart(_) => DRAFT_PART_RESULT_LEN,
                ResultEnvelope::Abort(_) => ABORT_RESULT_LEN,
            }
    }

    /// Decodes an envelope that is exactly the remainder of a payload.
    pub fn decode(bytes: &[u8]) -> crate::Result<Self> {
        DecodeError::min_len(bytes, ENVELOPE_PREFIX_LEN)?;
        reject_nonzero(bytes, 1, 3)?;
        let body = &bytes[ENVELOPE_PREFIX_LEN..];
        match bytes[0] {
            1 => Ok(ResultEnvelope::Object(ObjectResult::decode(body)?)),
            2 => Ok(ResultEnvelope::DraftPart(DraftPartResult::decode(body)?)),
            3 => Ok(ResultEnvelope::Abort(AbortResult::decode(body)?)),
            _ => Err(DecodeError::unknown_enum()),
        }
    }

    /// Encodes the envelope into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out[0] = self.result_type();
        out[1..ENVELOPE_PREFIX_LEN].fill(0);
        match self {
            ResultEnvelope::Object(body) => out[ENVELOPE_PREFIX_LEN..].copy_from_slice(&body.encode()),
            ResultEnvelope::DraftPart(body) => out[ENVELOPE_PREFIX_LEN..].copy_from_slice(&body.encode()),
            ResultEnvelope::Abort(body) => out[ENVELOPE_PREFIX_LEN..].copy_from_slice(&body.encode()),
        }
        Ok(needed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn object_result() -> ObjectResult {
        ObjectResult {
            operation_id: OperationId::new([0x11; 16]),
            store_id: StoreId::new([0x22; 16]),
            kind: ObjectKind::Route,
            outcome: ObjectOutcome::Committed,
            logical_object_id: LogicalObjectId::new(7),
            revision: Revision::new(12),
            length: 4096,
            crc32: 0xCBF4_3926,
        }
    }

    #[test]
    fn every_body_is_its_frozen_size() {
        assert_eq!(object_result().encode().len(), 64);
        let draft = DraftPartResult {
            child_operation_id: OperationId::new([1; 16]),
            store_id: StoreId::new([2; 16]),
            parent_operation_id: OperationId::new([3; 16]),
            draft_part_ref: DraftPartRef::new([4; 16]),
            part_kind: DraftPartKind::MapShard,
            part_key: 9,
            length: 100,
            crc32: 5,
        };
        assert_eq!(draft.encode().len(), 88);
        let abort = AbortResult {
            operation_id: OperationId::new([5; 16]),
            store_id: StoreId::new([6; 16]),
            target_operation_id: OperationId::new([7; 16]),
            disposition: AbortDisposition::AlreadyTerminal,
        };
        assert_eq!(abort.encode().len(), 56);
    }

    #[test]
    fn envelope_round_trips_and_rejects_a_trailing_byte() {
        let envelope = ResultEnvelope::Object(object_result());
        let mut out = [0u8; MAX_RESULT_ENVELOPE_LEN];
        let len = envelope.encode_into(&mut out).unwrap();
        assert_eq!(len, 68);
        assert_eq!(ResultEnvelope::decode(&out[..len]).unwrap(), envelope);

        let mut trailing = vec![0u8; len + 1];
        trailing[..len].copy_from_slice(&out[..len]);
        assert_eq!(ResultEnvelope::decode(&trailing).unwrap_err(), DecodeError::trailing_bytes());
        assert_eq!(ResultEnvelope::decode(&out[..len - 1]).unwrap_err(), DecodeError::truncated());
    }

    #[test]
    fn reserved_prefix_bytes_and_unknown_types_are_rejected() {
        let envelope = ResultEnvelope::Abort(AbortResult {
            operation_id: OperationId::ZERO,
            store_id: StoreId::ZERO,
            target_operation_id: OperationId::ZERO,
            disposition: AbortDisposition::Cancelled,
        });
        let mut out = [0u8; MAX_RESULT_ENVELOPE_LEN];
        let len = envelope.encode_into(&mut out).unwrap();
        out[2] = 1;
        assert_eq!(ResultEnvelope::decode(&out[..len]).unwrap_err(), DecodeError::reserved_bits());
        out[2] = 0;
        out[0] = 4;
        assert_eq!(ResultEnvelope::decode(&out[..len]).unwrap_err(), DecodeError::unknown_enum());
    }

    #[test]
    fn the_reserved_weather_outcome_decodes_but_is_marked() {
        let mut body = object_result();
        body.outcome = ObjectOutcome::ReservedSupersededWeather;
        let bytes = body.encode();
        let decoded = ObjectResult::decode(&bytes).unwrap();
        assert!(decoded.outcome.is_reserved());
        assert_eq!(decoded, body);
    }
}
