//! Canonical intent and its SHA-256 digest (`Device_Object_Protocol_v3.md` §11).
//!
//! The digest is the equality authority for idempotency: the same OperationId with the same digest
//! "resumes or returns its work/result", and the same OperationId with a different digest is
//! `operationIdConflict` and changes nothing. §11 is explicit that "Full SHA-256 is the equality
//! authority; CRC or a truncated digest is forbidden."
//!
//! What is deliberately *not* in the digest is as load-bearing as what is: "The OperationId is the
//! lookup key and is not repeated in the digest. The principal scope is claim ownership and is not
//! part of semantic intent... Resume policy, RequestId, SessionId, connection, transport, chunks,
//! and human text are excluded. Inactive target fields are included as their required zero bytes,
//! so there is one encoding per intent."
//!
//! FinalizeDraft has no entry here at all, and that is the contract's design rather than an
//! omission: it "does not make a second claim and has no intent suffix. Its only request field is
//! the parent lookup key."

use sha2::{Digest, Sha256};

use crate::codec::{put_bytes, put_u16, put_u32, put_u64};
use crate::draft::BeginDraft;
use crate::frame::Opcode;
use crate::ids::StoreId;
use crate::metadata::MAX_PUT_ENVELOPE;
use crate::mutate::{AcknowledgeRideImported, DeleteObject, InstallUpdate, SetMetadata};
use crate::registry::ObjectKind;
use crate::upload::{AbortOperation, StartUpload};

/// The 36-byte prefix every wire-initiated canonical intent begins with.
pub const INTENT_PREFIX_LEN: usize = 36;

/// The prefix's 16-byte tag: ASCII `OBC-DOS3-INTENT` plus one `00` byte.
pub const INTENT_TAG: [u8; 16] = *b"OBC-DOS3-INTENT\0";

/// The intent codec version at prefix byte 34.
pub const INTENT_CODEC_VERSION: u8 = 1;

/// The longest suffix: StartUpload's 34 fixed bytes plus a full 128-byte metadata envelope.
pub const MAX_INTENT_SUFFIX_LEN: usize = 34 + MAX_PUT_ENVELOPE;

/// The longest canonical intent.
pub const MAX_INTENT_LEN: usize = INTENT_PREFIX_LEN + MAX_INTENT_SUFFIX_LEN;

/// The three device-local intent schemes the storage contract freezes.
///
/// They are here for one reason: to make it checkable that "The two families cannot collide,
/// because the wire prefix begins `OBC-DOS3-INTENT\0` and every local tag begins `O2-`, so no input
/// to one scheme is an input to the other." The local digests themselves are storage-side inputs
/// and belong to the storage contract, not to this codec.
pub mod local {
    /// The weather-context transition scheme.
    pub const WEATHER: &[u8] = b"O2-LOCAL-WX-INTENT\0";
    /// The post-boot update-state scheme.
    pub const UPDATE: &[u8] = b"O2-LOCAL-UPD-INTENT\0";
    /// The sideload-import scheme.
    pub const IMPORT: &[u8] = b"O2-LOCAL-IMP-INTENT\0";
    /// All three, in the order §11 lists them.
    pub const ALL: [&[u8]; 3] = [WEATHER, UPDATE, IMPORT];
}

/// One canonical intent's exact bytes.
///
/// The buffer is fixed at [`MAX_INTENT_LEN`] so building an intent allocates nothing — the same
/// property that lets the device compute one inside its claim path.
#[derive(Clone, Copy)]
pub struct CanonicalIntent {
    buffer: [u8; MAX_INTENT_LEN],
    len: usize,
}

impl core::fmt::Debug for CanonicalIntent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CanonicalIntent({} bytes)", self.len)
    }
}

impl PartialEq for CanonicalIntent {
    fn eq(&self, other: &Self) -> bool {
        self.bytes() == other.bytes()
    }
}

impl Eq for CanonicalIntent {}

impl CanonicalIntent {
    fn start(store_id: StoreId, opcode: Opcode) -> Self {
        let mut intent = CanonicalIntent { buffer: [0u8; MAX_INTENT_LEN], len: INTENT_PREFIX_LEN };
        put_bytes(&mut intent.buffer, 0, &INTENT_TAG);
        put_bytes(&mut intent.buffer, 16, store_id.as_bytes());
        put_u16(&mut intent.buffer, 32, opcode.to_u16());
        intent.buffer[34] = INTENT_CODEC_VERSION;
        intent
    }

    fn push(&mut self, bytes: &[u8]) {
        // Every caller writes a suffix the table bounds by `MAX_INTENT_SUFFIX_LEN`, so this cannot
        // overrun; the assertion documents that rather than defending against a caller.
        debug_assert!(self.len + bytes.len() <= MAX_INTENT_LEN);
        let end = self.len + bytes.len();
        self.buffer[self.len..end].copy_from_slice(bytes);
        self.len = end;
    }

    fn push_u16(&mut self, value: u16) {
        let mut raw = [0u8; 2];
        put_u16(&mut raw, 0, value);
        self.push(&raw);
    }

    fn push_u32(&mut self, value: u32) {
        let mut raw = [0u8; 4];
        put_u32(&mut raw, 0, value);
        self.push(&raw);
    }

    fn push_u64(&mut self, value: u64) {
        let mut raw = [0u8; 8];
        put_u64(&mut raw, 0, value);
        self.push(&raw);
    }

    /// The exact canonical bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.buffer[..self.len]
    }

    /// The full SHA-256 of those bytes — the equality authority for an OperationId claim.
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.bytes());
        hasher.finalize().into()
    }

    /// StartUpload's suffix: "ObjectKind `u16`; target mode `u8`; zero `u8`; LogicalObjectId `u64`;
    /// expected Revision `u64`; length `u64`; CRC `u32`; envelope length `u16`; exact canonical
    /// metadata envelope".
    pub fn for_start_upload(store_id: StoreId, request: &StartUpload<'_>) -> Self {
        let mut intent = Self::start(store_id, Opcode::StartUpload);
        intent.push_u16(request.kind.to_u16());
        intent.push(&[request.target.mode().to_u8(), 0]);
        intent.push_u64(request.target.logical_object_id().get());
        intent.push_u64(request.target.expected_revision().get());
        intent.push_u64(request.declared_length);
        intent.push_u32(request.expected_crc32);
        intent.push_u16(request.metadata.encoded_len() as u16);
        let mut envelope = [0u8; MAX_PUT_ENVELOPE];
        let len = request.metadata.encode_into(&mut envelope).unwrap_or(0);
        intent.push(&envelope[..len]);
        intent
    }

    /// BeginDraft's suffix.
    pub fn for_begin_draft(store_id: StoreId, request: &BeginDraft) -> Self {
        let mut intent = Self::start(store_id, Opcode::BeginDraft);
        intent.push_u16(request.kind.to_u16());
        intent.push(&[request.target.mode().to_u8(), 0]);
        intent.push_u64(request.target.logical_object_id().get());
        intent.push_u64(request.target.expected_revision().get());
        intent.push_u64(request.declared_manifest_length);
        intent.push_u32(request.declared_manifest_crc32);
        intent.push_u16(request.expected_part_count);
        intent.push_u16(0);
        intent
    }

    /// StartDraftPart's suffix.
    pub fn for_start_draft_part(store_id: StoreId, request: &crate::draft::StartDraftPart) -> Self {
        let mut intent = Self::start(store_id, Opcode::StartDraftPart);
        intent.push(request.parent_operation_id.as_bytes());
        intent.push_u16(request.part_kind.to_u16());
        intent.push_u16(0);
        intent.push_u64(request.part_key);
        intent.push_u64(request.declared_length);
        intent.push_u32(request.expected_crc32);
        intent
    }

    /// DeleteObject's suffix.
    pub fn for_delete_object(store_id: StoreId, request: &DeleteObject) -> Self {
        let mut intent = Self::start(store_id, Opcode::DeleteObject);
        intent.push_u16(request.target.kind.to_u16());
        intent.push_u64(request.target.logical_object_id.get());
        intent.push_u64(request.target.expected_revision.get());
        intent
    }

    /// SetMetadata's suffix.
    pub fn for_set_metadata(store_id: StoreId, request: &SetMetadata<'_>) -> Self {
        let mut intent = Self::start(store_id, Opcode::SetMetadata);
        intent.push_u16(request.target.kind.to_u16());
        intent.push_u64(request.target.logical_object_id.get());
        intent.push_u64(request.target.expected_revision.get());
        intent.push_u16(request.patch.encoded_len() as u16);
        let mut envelope = [0u8; MAX_PUT_ENVELOPE];
        let len = request.patch.encode_into(&mut envelope).unwrap_or(0);
        intent.push(&envelope[..len]);
        intent
    }

    /// AbortOperation's suffix: "target OperationId `[16]`; reason `u8`; seven zero bytes".
    pub fn for_abort_operation(store_id: StoreId, request: &AbortOperation) -> Self {
        let mut intent = Self::start(store_id, Opcode::AbortOperation);
        intent.push(request.target_operation_id.as_bytes());
        intent.push(&[request.reason.to_u8(), 0, 0, 0, 0, 0, 0, 0]);
        intent
    }

    /// InstallUpdate's suffix, whose ObjectKind field §11 pins at the literal value `7`.
    pub fn for_install_update(store_id: StoreId, request: &InstallUpdate) -> Self {
        let mut intent = Self::start(store_id, Opcode::InstallUpdate);
        intent.push_u16(ObjectKind::UpdatePackage.to_u16());
        intent.push_u64(request.logical_object_id.get());
        intent.push_u64(request.expected_revision.get());
        intent
    }

    /// AcknowledgeRideImported's suffix, whose ObjectKind field §11 pins at the literal value `3`.
    pub fn for_acknowledge_ride_imported(store_id: StoreId, request: &AcknowledgeRideImported) -> Self {
        let mut intent = Self::start(store_id, Opcode::AcknowledgeRideImported);
        intent.push_u16(ObjectKind::Ride.to_u16());
        intent.push_u64(request.logical_object_id.get());
        intent.push_u64(request.expected_revision.get());
        intent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{LogicalObjectId, OperationId, Revision};
    use crate::metadata::{MetadataEnvelope, MetadataWriter, SchemaClass};
    use crate::mutate::MutationTarget;
    use crate::registry::AbortReason;
    use crate::upload::{ResumePreference, Target};

    const STORE: StoreId = StoreId::new([0x3C; 16]);

    fn start_upload<'a>(buffer: &'a mut [u8], length: u64) -> StartUpload<'a> {
        let mut writer = MetadataWriter::new(buffer).unwrap();
        writer.push(0x8001, &[2]).unwrap();
        let bytes = writer.finish(ObjectKind::Route, SchemaClass::Put);
        StartUpload {
            operation_id: OperationId::new([0xAA; 16]),
            kind: ObjectKind::Route,
            target: Target::Create,
            resume: ResumePreference::ResumePermitted,
            declared_length: length,
            expected_crc32: 0xCBF4_3926,
            metadata: MetadataEnvelope::decode(bytes, MAX_PUT_ENVELOPE).unwrap(),
        }
    }

    #[test]
    fn the_prefix_is_exactly_the_thirty_six_bytes_the_table_names() {
        let mut buffer = [0u8; 32];
        let intent = CanonicalIntent::for_start_upload(STORE, &start_upload(&mut buffer, 1000));
        let bytes = intent.bytes();
        assert_eq!(&bytes[0..16], b"OBC-DOS3-INTENT\0");
        assert_eq!(&bytes[16..32], STORE.as_bytes());
        assert_eq!(u16::from_le_bytes([bytes[32], bytes[33]]), Opcode::StartUpload.to_u16());
        assert_eq!(bytes[34], INTENT_CODEC_VERSION);
        assert_eq!(bytes[35], 0);
        // 36-byte prefix + 34 fixed suffix bytes + a 13-byte route Put envelope.
        assert_eq!(bytes.len(), 36 + 34 + 13);
    }

    #[test]
    fn the_operation_id_and_resume_preference_are_not_in_the_digest() {
        let mut first_buffer = [0u8; 32];
        let mut second_buffer = [0u8; 32];
        let mut first = start_upload(&mut first_buffer, 1000);
        let mut second = start_upload(&mut second_buffer, 1000);
        second.operation_id = OperationId::new([0xBB; 16]);
        second.resume = ResumePreference::RestartAtZero;
        assert_eq!(
            CanonicalIntent::for_start_upload(STORE, &first).digest(),
            CanonicalIntent::for_start_upload(STORE, &second).digest()
        );

        // A semantic field does move it.
        first.declared_length = 1001;
        assert_ne!(
            CanonicalIntent::for_start_upload(STORE, &first).digest(),
            CanonicalIntent::for_start_upload(STORE, &second).digest()
        );
    }

    #[test]
    fn the_store_id_is_part_of_intent_identity() {
        let mut buffer = [0u8; 32];
        let request = start_upload(&mut buffer, 1000);
        assert_ne!(
            CanonicalIntent::for_start_upload(STORE, &request).digest(),
            CanonicalIntent::for_start_upload(StoreId::new([0x3D; 16]), &request).digest()
        );
    }

    #[test]
    fn every_mutating_opcode_has_its_exact_suffix_length() {
        let target = MutationTarget {
            operation_id: OperationId::new([1; 16]),
            kind: ObjectKind::Route,
            logical_object_id: LogicalObjectId::new(4),
            expected_revision: Revision::new(9),
        };
        let delete = CanonicalIntent::for_delete_object(STORE, &DeleteObject { target });
        assert_eq!(delete.bytes().len(), INTENT_PREFIX_LEN + 18);

        let install = CanonicalIntent::for_install_update(
            STORE,
            &InstallUpdate {
                operation_id: OperationId::new([2; 16]),
                logical_object_id: LogicalObjectId::new(1),
                expected_revision: Revision::new(2),
            },
        );
        assert_eq!(install.bytes().len(), INTENT_PREFIX_LEN + 18);
        assert_eq!(u16::from_le_bytes([install.bytes()[36], install.bytes()[37]]), 7);

        let ack = CanonicalIntent::for_acknowledge_ride_imported(
            STORE,
            &AcknowledgeRideImported {
                operation_id: OperationId::new([3; 16]),
                logical_object_id: LogicalObjectId::new(1),
                expected_revision: Revision::new(2),
            },
        );
        assert_eq!(ack.bytes().len(), INTENT_PREFIX_LEN + 18);
        assert_eq!(u16::from_le_bytes([ack.bytes()[36], ack.bytes()[37]]), 3);

        let abort = CanonicalIntent::for_abort_operation(
            STORE,
            &AbortOperation {
                operation_id: OperationId::new([4; 16]),
                target_operation_id: OperationId::new([5; 16]),
                reason: AbortReason::UserRequested,
            },
        );
        assert_eq!(abort.bytes().len(), INTENT_PREFIX_LEN + 24);
        assert!(abort.bytes()[INTENT_PREFIX_LEN + 17..].iter().all(|&b| b == 0));

        let begin = CanonicalIntent::for_begin_draft(
            STORE,
            &BeginDraft {
                parent_operation_id: OperationId::new([6; 16]),
                kind: ObjectKind::VolumeManifest,
                target: Target::Create,
                declared_manifest_length: 264,
                declared_manifest_crc32: 7,
                expected_part_count: 3,
            },
        );
        assert_eq!(begin.bytes().len(), INTENT_PREFIX_LEN + 36);

        let part = CanonicalIntent::for_start_draft_part(
            STORE,
            &crate::draft::StartDraftPart {
                child_operation_id: OperationId::new([7; 16]),
                parent_operation_id: OperationId::new([6; 16]),
                part_kind: crate::registry::DraftPartKind::MapShard,
                part_key: 2,
                declared_length: 10,
                expected_crc32: 11,
                resume: ResumePreference::RestartAtZero,
            },
        );
        assert_eq!(part.bytes().len(), INTENT_PREFIX_LEN + 40);
    }

    #[test]
    fn the_two_intent_families_cannot_collide() {
        // §11: the wire prefix begins `OBC-DOS3-INTENT\0` and every local tag begins `O2-`.
        for tag in local::ALL {
            assert!(tag.starts_with(b"O2-"));
            assert_ne!(&INTENT_TAG[..3], &tag[..3]);
        }
        assert_eq!(INTENT_TAG.len(), 16);
        assert_eq!(INTENT_TAG[15], 0);
    }

    #[test]
    fn sha256_is_the_full_digest_and_matches_a_known_answer() {
        // The reference check value for the hash this contract names.
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        let digest: [u8; 32] = hasher.finalize().into();
        assert_eq!(
            digest,
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23, 0xb0,
                0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
            ]
        );
    }
}
