//! Direct mutations: DeleteObject, SetMetadata, InstallUpdate, AcknowledgeRideImported
//! (`Device_Object_Protocol_v3.md` §9).
//!
//! All four share the same 32- or 36-byte shape — an OperationId, a kind, a logical identity, and a
//! mandatory expected Revision — because all four are compare-and-swap catalog transactions whose
//! response is an `ObjectResult` in a `ResultEnvelope`. Every expected revision "is checked during
//! admission and again under the store commit lock".
//!
//! SetMetadata is the one with a tail, and the one with a rule a codec must not be tempted to
//! smooth over: "A patch envelope is well-formed with zero fields, so an empty patch is not a codec
//! error; it is refused as a request, with `invalidDescriptor/emptyMetadataPatch`, because a
//! mutation that changes nothing would still consume an OperationId, a claim, and a catalog
//! commit." The envelope decodes; the *request* is refused.

use crate::codec::{bytes16_at, put_bytes, put_u16, put_u64, u16_at, u64_at};
use crate::error::{detail, DecodeError};
use crate::ids::{LogicalObjectId, OperationId, Revision};
use crate::metadata::{MetadataEnvelope, Schema, SchemaClass, MAX_PUT_ENVELOPE};
use crate::registry::{object_kind, ObjectKind};
use crate::{BufferTooSmall, EncodeResult};

/// The DeleteObject request, and the fixed prefix of a SetMetadata request.
pub const DELETE_OBJECT_LEN: usize = 36;

/// The smallest SetMetadata payload: the prefix plus an empty envelope.
pub const MIN_SET_METADATA_LEN: usize = DELETE_OBJECT_LEN + 8;

/// The largest SetMetadata payload the common ceiling allows.
pub const MAX_SET_METADATA_LEN: usize = DELETE_OBJECT_LEN + MAX_PUT_ENVELOPE;

/// The InstallUpdate and AcknowledgeRideImported requests.
pub const COMMAND_LEN: usize = 32;

/// Bit 0 of the delete/patch flags word. §9 makes it mandatory.
pub const EXPECTED_REVISION_FLAG: u16 = 1 << 0;

/// The fields DeleteObject and SetMetadata share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationTarget {
    /// The idempotency key.
    pub operation_id: OperationId,
    /// The kind.
    pub kind: ObjectKind,
    /// The entry.
    pub logical_object_id: LogicalObjectId,
    /// The entry Revision the repository last reported for it.
    pub expected_revision: Revision,
}

impl MutationTarget {
    fn decode(payload: &[u8]) -> crate::Result<Self> {
        let flags = u16_at(payload, 18);
        if flags & !EXPECTED_REVISION_FLAG != 0 {
            return Err(DecodeError::unsupported_flags());
        }
        if flags & EXPECTED_REVISION_FLAG == 0 {
            // §9: "flags `u16` (expected revision bit 0 is mandatory)".
            return Err(DecodeError::invalid_combination());
        }
        Ok(MutationTarget {
            operation_id: OperationId::new(bytes16_at(payload, 0)),
            kind: object_kind(u16_at(payload, 16))?,
            logical_object_id: LogicalObjectId::new(u64_at(payload, 20)),
            expected_revision: Revision::new(u64_at(payload, 28)),
        })
    }

    fn encode_prefix(&self, out: &mut [u8]) {
        put_bytes(out, 0, self.operation_id.as_bytes());
        put_u16(out, 16, self.kind.to_u16());
        put_u16(out, 18, EXPECTED_REVISION_FLAG);
        put_u64(out, 20, self.logical_object_id.get());
        put_u64(out, 28, self.expected_revision.get());
    }
}

/// The DeleteObject request (§9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeleteObject {
    /// What to delete, and at which revision.
    pub target: MutationTarget,
}

impl DeleteObject {
    /// Decodes exactly [`DELETE_OBJECT_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        DecodeError::exact_len(payload, DELETE_OBJECT_LEN)?;
        Ok(DeleteObject { target: MutationTarget::decode(payload)? })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; DELETE_OBJECT_LEN] {
        let mut out = [0u8; DELETE_OBJECT_LEN];
        self.target.encode_prefix(&mut out);
        out
    }
}

/// The SetMetadata request (§9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetMetadata<'a> {
    /// What to patch, and at which revision.
    pub target: MutationTarget,
    /// The patch envelope. Every field of a patch schema is individually optional, and the envelope
    /// as a whole must carry at least one.
    pub patch: MetadataEnvelope<'a>,
}

impl<'a> SetMetadata<'a> {
    /// The exact encoded payload length.
    pub fn encoded_len(&self) -> usize {
        DELETE_OBJECT_LEN + self.patch.encoded_len()
    }

    /// Decodes a SetMetadata payload.
    ///
    /// Like StartUpload, the patch is validated against the registered schema for the target's own
    /// kind. A kind the registry gives no patch schema — trip, ride, weather, update package —
    /// "reject[s] SetMetadata as unsupported", which is `unsupportedCapability/logicalKind` rather
    /// than a descriptor fault: the request is well formed and names something the object system
    /// does not offer.
    pub fn decode(payload: &'a [u8]) -> crate::Result<Self> {
        DecodeError::min_len(payload, MIN_SET_METADATA_LEN)?;
        let (patch, used) = MetadataEnvelope::decode_prefix(&payload[DELETE_OBJECT_LEN..], MAX_PUT_ENVELOPE)?;
        if DELETE_OBJECT_LEN + used != payload.len() {
            return Err(DecodeError::trailing_bytes());
        }
        let target = MutationTarget::decode(payload)?;
        let schema = Schema::lookup(target.kind, SchemaClass::Patch)
            .ok_or_else(|| DecodeError::unsupported_capability(detail::capability::LOGICAL_KIND))?;
        if patch.field_count == 0 {
            return Err(DecodeError::invalid_descriptor(detail::descriptor::EMPTY_METADATA_PATCH));
        }
        schema.validate(&patch)?;
        Ok(SetMetadata { target, patch })
    }

    /// Encodes the payload into `out`, returning its exact length.
    pub fn encode_into(&self, out: &mut [u8]) -> EncodeResult {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(BufferTooSmall { needed, available: out.len() });
        }
        let out = &mut out[..needed];
        out.fill(0);
        self.target.encode_prefix(out);
        self.patch.encode_into(&mut out[DELETE_OBJECT_LEN..])?;
        Ok(needed)
    }
}

/// The InstallUpdate request (§9).
///
/// It is not cancellable once admitted: "An AbortOperation naming an InstallUpdate target is refused
/// with `unsupportedCapability/nonCancellableOperation`, guidance reject permanently."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallUpdate {
    /// The idempotency key.
    pub operation_id: OperationId,
    /// The update package to install.
    pub logical_object_id: LogicalObjectId,
    /// Its expected Revision.
    pub expected_revision: Revision,
}

/// The AcknowledgeRideImported request (§9).
///
/// "It is sent only after the client durably stores and verifies the download. Download completion
/// alone does not change import state."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcknowledgeRideImported {
    /// The idempotency key.
    pub operation_id: OperationId,
    /// The ride being acknowledged.
    pub logical_object_id: LogicalObjectId,
    /// Its expected Revision.
    pub expected_revision: Revision,
}

/// Decodes the shared 32-byte command shape.
fn decode_command(payload: &[u8]) -> crate::Result<(OperationId, LogicalObjectId, Revision)> {
    DecodeError::exact_len(payload, COMMAND_LEN)?;
    Ok((
        OperationId::new(bytes16_at(payload, 0)),
        LogicalObjectId::new(u64_at(payload, 16)),
        Revision::new(u64_at(payload, 24)),
    ))
}

/// Encodes it.
fn encode_command(
    operation_id: OperationId,
    logical_object_id: LogicalObjectId,
    revision: Revision,
) -> [u8; COMMAND_LEN] {
    let mut out = [0u8; COMMAND_LEN];
    put_bytes(&mut out, 0, operation_id.as_bytes());
    put_u64(&mut out, 16, logical_object_id.get());
    put_u64(&mut out, 24, revision.get());
    out
}

impl InstallUpdate {
    /// The kind this command always addresses (§11's canonical suffix pins the value `7`).
    pub const KIND: ObjectKind = ObjectKind::UpdatePackage;

    /// Decodes exactly [`COMMAND_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        let (operation_id, logical_object_id, expected_revision) = decode_command(payload)?;
        Ok(InstallUpdate { operation_id, logical_object_id, expected_revision })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; COMMAND_LEN] {
        encode_command(self.operation_id, self.logical_object_id, self.expected_revision)
    }
}

impl AcknowledgeRideImported {
    /// The kind this command always addresses (§11's canonical suffix pins the value `3`).
    pub const KIND: ObjectKind = ObjectKind::Ride;

    /// Decodes exactly [`COMMAND_LEN`] bytes.
    pub fn decode(payload: &[u8]) -> crate::Result<Self> {
        let (operation_id, logical_object_id, expected_revision) = decode_command(payload)?;
        Ok(AcknowledgeRideImported { operation_id, logical_object_id, expected_revision })
    }

    /// Encodes the request.
    pub fn encode(&self) -> [u8; COMMAND_LEN] {
        encode_command(self.operation_id, self.logical_object_id, self.expected_revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{MetadataWriter, SchemaClass};

    fn target() -> MutationTarget {
        MutationTarget {
            operation_id: OperationId::new([0x44; 16]),
            kind: ObjectKind::Route,
            logical_object_id: LogicalObjectId::new(6),
            expected_revision: Revision::new(21),
        }
    }

    #[test]
    fn delete_object_is_thirty_six_bytes_with_a_mandatory_revision_flag() {
        let request = DeleteObject { target: target() };
        let bytes = request.encode();
        assert_eq!(bytes.len(), 36);
        assert_eq!(DeleteObject::decode(&bytes).unwrap(), request);

        let mut without_flag = bytes;
        put_u16(&mut without_flag, 18, 0);
        assert_eq!(DeleteObject::decode(&without_flag).unwrap_err(), DecodeError::invalid_combination());

        let mut extra_flag = bytes;
        put_u16(&mut extra_flag, 18, 0b11);
        assert_eq!(DeleteObject::decode(&extra_flag).unwrap_err(), DecodeError::unsupported_flags());
    }

    #[test]
    fn set_metadata_round_trips_and_refuses_a_well_formed_empty_patch() {
        let mut buffer = [0u8; 96];
        let mut writer = MetadataWriter::new(&mut buffer).unwrap();
        writer.push(0x8002, &[1]).unwrap();
        writer.push(0x8003, "Kaiserstuhl loop".as_bytes()).unwrap();
        let patch_bytes = writer.finish(ObjectKind::Route, SchemaClass::Patch);
        let patch = MetadataEnvelope::decode(patch_bytes, MAX_PUT_ENVELOPE).unwrap();

        let request = SetMetadata { target: target(), patch };
        let mut out = [0u8; MAX_SET_METADATA_LEN];
        let len = request.encode_into(&mut out).unwrap();
        assert_eq!(SetMetadata::decode(&out[..len]).unwrap(), request);

        let empty =
            SetMetadata { target: target(), patch: MetadataEnvelope::empty(ObjectKind::Route, SchemaClass::Patch) };
        let len = empty.encode_into(&mut out).unwrap();
        assert_eq!(len, MIN_SET_METADATA_LEN);
        assert_eq!(
            SetMetadata::decode(&out[..len]).unwrap_err(),
            DecodeError::invalid_descriptor(detail::descriptor::EMPTY_METADATA_PATCH)
        );
        // The envelope on its own is still well formed — the refusal is the request's, not the
        // codec's.
        assert!(MetadataEnvelope::decode(&out[DELETE_OBJECT_LEN..len], MAX_PUT_ENVELOPE).is_ok());
    }

    #[test]
    fn the_two_domain_commands_are_thirty_two_bytes() {
        let install = InstallUpdate {
            operation_id: OperationId::new([9; 16]),
            logical_object_id: LogicalObjectId::new(2),
            expected_revision: Revision::new(3),
        };
        let bytes = install.encode();
        assert_eq!(bytes.len(), 32);
        assert_eq!(InstallUpdate::decode(&bytes).unwrap(), install);
        assert_eq!(InstallUpdate::KIND, ObjectKind::UpdatePackage);

        let ack = AcknowledgeRideImported {
            operation_id: OperationId::new([10; 16]),
            logical_object_id: LogicalObjectId::new(5),
            expected_revision: Revision::new(7),
        };
        assert_eq!(AcknowledgeRideImported::decode(&ack.encode()).unwrap(), ack);
        assert_eq!(AcknowledgeRideImported::KIND, ObjectKind::Ride);
        assert_eq!(AcknowledgeRideImported::decode(&ack.encode()[..31]).unwrap_err(), DecodeError::truncated());
    }
}
