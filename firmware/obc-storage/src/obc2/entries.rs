//! The catalog projection entries of `OBC2_Storage_Format.md` §5.3.
//!
//! One module for all of them because they have exactly one definition each and two readers: the
//! checkpoint regions of §5.1 and the fixed mutation of §6.1, which carries the same entry shapes
//! at its own offsets. Encoding a row once and placing it in either container is what keeps a
//! journal record and the checkpoint it compacts into byte-identical.
//!
//! Three conventions run through every entry here:
//!
//! - **Absent is all zero.** §6.1: "An absent fixed entry is all zero", and §5.1: outside a
//!   region's occupied prefix "the remaining entries are all zero". [`absent`] is that one test.
//! - **A removal carries only key bytes.** §6.1's table fixes, per entry, which ranges are key and
//!   which occupied byte must still be `1`; every other byte must be zero. Each entry type's
//!   `decode_removal` enforces exactly its row of that table.
//! - **Inactive alternatives are zero.** A field the entry's own flags make meaningless is zero,
//!   and a decoder rejects it nonzero. Where the spec says "valid including zero", the flag — never
//!   the value — is what decides presence.
//!
//! Kind fields stay raw `u16`. §5.3 fixes their width and their place in a sort key; which kinds
//! are registered is `Device_Object_Registries_v2.md`'s business and a repository's to enforce, so
//! a checkpoint holding a kind this build does not know is not thereby corrupt. The one registry
//! rule §5.3 does state — an active row's opcode is a registered wire opcode or a registered
//! storage-internal claim tag — is enforced here, against `obc-link`'s opcode registry.

use obc_link::frame::Opcode;
use obc_link::ids::{DraftPartRef, GenerationId, LogicalObjectId, OperationId, Revision, WeatherRequestId};

use super::error::{DecodeError, Reason, Record, Result};
use super::raw::{
    bytes16_at, bytes32_at, i32_at, i64_at, is_zero, put_bytes, put_i32, put_i64, put_u16, put_u32, put_u64, u16_at,
    u32_at, u64_at,
};

/// The storage-internal claim tag for a weather-context change (§5.3).
pub const CLAIM_TAG_WEATHER_CONTEXT: u16 = 0xFF01;
/// The storage-internal claim tag for post-boot update-state reconciliation (§5.3, §10.2).
pub const CLAIM_TAG_UPDATE_RECONCILIATION: u16 = 0xFF02;

/// True when every byte of an entry slot is zero, which is what "absent" means everywhere.
pub fn absent(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == 0)
}

fn err(record: Record, reason: Reason) -> DecodeError {
    DecodeError::new(record, reason)
}

/// Checks the fixed length a decoder was handed.
fn fixed(record: Record, bytes: &[u8], len: usize) -> Result<()> {
    if bytes.len() == len {
        Ok(())
    } else {
        Err(err(record, Reason::Length))
    }
}

/// Checks that `bytes[off..off + len]` is zero, as a reserved run must be.
fn reserved(record: Record, bytes: &[u8], off: usize, len: usize) -> Result<()> {
    if is_zero(bytes, off, len) {
        Ok(())
    } else {
        Err(err(record, Reason::Reserved))
    }
}

/// Checks an `occupied` byte that the entry shape requires to be exactly `1`.
fn occupied(record: Record, bytes: &[u8]) -> Result<()> {
    if bytes[0] == 1 {
        Ok(())
    } else {
        Err(err(record, Reason::Occupied))
    }
}

/// Checks that every byte outside the listed key ranges is zero (§6.1's removal table).
fn only_keys(record: Record, bytes: &[u8], keys: &[(usize, usize)]) -> Result<()> {
    for (index, &byte) in bytes.iter().enumerate() {
        let in_key = keys.iter().any(|&(start, end)| index >= start && index < end);
        if !in_key && byte != 0 {
            return Err(err(record, Reason::KeyBytes));
        }
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------
// Repository state — 24 bytes, keyed by ObjectKind
// -------------------------------------------------------------------------------------------

/// One repository's revision and logical-ID allocation state (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepositoryState {
    /// `ObjectKind`, the row's key.
    pub kind: u16,
    /// Flags; bit 0 is "logical-ID space exhausted".
    pub flags: u16,
    /// The repository revision.
    pub revision: Revision,
    /// The next logical-ID candidate. Zero is a valid first candidate.
    pub next_logical_id: LogicalObjectId,
}

impl RepositoryState {
    /// Encoded length.
    pub const LEN: usize = 24;
    /// Bit 0 of `flags`: the logical-ID space is exhausted rather than wrapped.
    pub const FLAG_ID_EXHAUSTED: u16 = 1 << 0;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_u16(&mut out, 0, self.kind);
        put_u16(&mut out, 2, self.flags);
        put_u64(&mut out, 8, self.revision.get());
        put_u64(&mut out, 16, self.next_logical_id.get());
        out
    }

    /// Decodes one row.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::RepositoryState;
        fixed(R, bytes, Self::LEN)?;
        if u16_at(bytes, 2) & !Self::FLAG_ID_EXHAUSTED != 0 {
            return Err(err(R, Reason::Reserved));
        }
        reserved(R, bytes, 4, 4)?;
        Ok(RepositoryState {
            kind: u16_at(bytes, 0),
            flags: u16_at(bytes, 2),
            revision: Revision::new(u64_at(bytes, 8)),
            next_logical_id: LogicalObjectId::new(u64_at(bytes, 16)),
        })
    }
}

// -------------------------------------------------------------------------------------------
// Catalog head — 160 bytes, keyed by (ObjectKind, LogicalObjectId)
// -------------------------------------------------------------------------------------------

/// A catalog head's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct HeadKey {
    /// Object kind.
    pub kind: u16,
    /// Logical object ID.
    pub id: LogicalObjectId,
}

/// One published logical head (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogHead {
    /// `(ObjectKind, LogicalObjectId)`.
    pub key: HeadKey,
    /// Flags; bit 0 is "resolution present" and is store-private.
    pub flags: u8,
    /// The repository revision this head was published at.
    pub revision: Revision,
    /// The physical generation the head resolves to.
    pub generation: GenerationId,
    /// Payload length.
    pub length: u64,
    /// Payload CRC-32.
    pub crc: u32,
    /// The declared envelope length, `8..=96`.
    pub envelope_len: u16,
    /// The canonical catalog-projection envelope, zero-padded to 96 bytes.
    pub envelope: [u8; 96],
    /// The resolution generation, meaningful only with the resolution-present flag.
    pub resolution: GenerationId,
}

impl CatalogHead {
    /// Encoded length.
    pub const LEN: usize = 160;
    /// Bit 0 of `flags`: this head names a resolution generation at offset 144.
    pub const FLAG_RESOLUTION_PRESENT: u8 = 1 << 0;
    /// The registry's catalog-projection ceiling, reserved in every head entry.
    pub const ENVELOPE_CAPACITY: usize = 96;
    /// The smallest legal envelope.
    pub const MIN_ENVELOPE: u16 = 8;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = 1;
        out[1] = self.flags;
        put_u16(&mut out, 2, self.key.kind);
        put_u64(&mut out, 4, self.key.id.get());
        put_u64(&mut out, 12, self.revision.get());
        put_u64(&mut out, 20, self.generation.get());
        put_u64(&mut out, 28, self.length);
        put_u32(&mut out, 36, self.crc);
        put_u16(&mut out, 40, self.envelope_len);
        put_bytes(&mut out, 48, &self.envelope);
        put_u64(&mut out, 144, self.resolution.get());
        out
    }

    /// Decodes one occupied head entry.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::CatalogHead;
        fixed(R, bytes, Self::LEN)?;
        occupied(R, bytes)?;
        let flags = bytes[1];
        if flags & !Self::FLAG_RESOLUTION_PRESENT != 0 {
            return Err(err(R, Reason::Reserved));
        }
        let envelope_len = u16_at(bytes, 40);
        if !(Self::MIN_ENVELOPE..=Self::ENVELOPE_CAPACITY as u16).contains(&envelope_len) {
            return Err(err(R, Reason::Overflow));
        }
        reserved(R, bytes, 42, 6)?;
        // The envelope is a 96-byte reservation holding `envelope_len` canonical bytes "followed by
        // zero"; a nonzero tail would make two different byte strings decode to one envelope.
        reserved(R, bytes, 48 + envelope_len as usize, Self::ENVELOPE_CAPACITY - envelope_len as usize)?;
        if flags & Self::FLAG_RESOLUTION_PRESENT == 0 {
            reserved(R, bytes, 144, 8)?;
        }
        reserved(R, bytes, 152, 8)?;
        let mut envelope = [0u8; Self::ENVELOPE_CAPACITY];
        envelope.copy_from_slice(&bytes[48..144]);
        Ok(CatalogHead {
            key: HeadKey { kind: u16_at(bytes, 2), id: LogicalObjectId::new(u64_at(bytes, 4)) },
            flags,
            revision: Revision::new(u64_at(bytes, 12)),
            generation: GenerationId::new(u64_at(bytes, 20)),
            length: u64_at(bytes, 28),
            crc: u32_at(bytes, 36),
            envelope_len,
            envelope,
            resolution: GenerationId::new(u64_at(bytes, 144)),
        })
    }

    /// Encodes the removal form: the occupied byte and the key bytes, nothing else.
    pub fn encode_removal(key: HeadKey) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = 1;
        put_u16(&mut out, 2, key.kind);
        put_u64(&mut out, 4, key.id.get());
        out
    }

    /// Decodes the removal form.
    pub fn decode_removal(bytes: &[u8]) -> Result<HeadKey> {
        const R: Record = Record::CatalogHead;
        fixed(R, bytes, Self::LEN)?;
        occupied(R, bytes)?;
        only_keys(R, bytes, &[(0, 1), (2, 4), (4, 12)])?;
        Ok(HeadKey { kind: u16_at(bytes, 2), id: LogicalObjectId::new(u64_at(bytes, 4)) })
    }
}

// -------------------------------------------------------------------------------------------
// Active operation — 128 bytes, keyed by OperationId
// -------------------------------------------------------------------------------------------

/// The storage phase of a claimed operation (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationPhase {
    /// Claimed, no payload byte accepted.
    Prepared = 1,
    /// A draft parent is open.
    DraftOpen = 2,
    /// Payload bytes are being accepted.
    Streaming = 3,
    /// The payload is sealed and immutable.
    Sealed = 4,
    /// Domain validation is running.
    Validating = 5,
    /// A complete commit mutation exists.
    Publishing = 6,
    /// The operation is handed off outside the store.
    ExternalHandoff = 7,
    /// The operation is unwinding.
    Aborting = 8,
}

impl OperationPhase {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => OperationPhase::Prepared,
            2 => OperationPhase::DraftOpen,
            3 => OperationPhase::Streaming,
            4 => OperationPhase::Sealed,
            5 => OperationPhase::Validating,
            6 => OperationPhase::Publishing,
            7 => OperationPhase::ExternalHandoff,
            8 => OperationPhase::Aborting,
            _ => return None,
        })
    }
}

/// One claimed operation (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveOperation {
    /// The claim's idempotency key and this row's sort key.
    pub operation: OperationId,
    /// The complete SHA-256 canonical-intent digest.
    pub intent: [u8; 32],
    /// The opaque stable principal-scope digest.
    pub principal: [u8; 32],
    /// A registered wire opcode, or one of the two storage-internal claim tags.
    pub opcode: u16,
    /// Logical `ObjectKind` or `DraftPartKind`; zero only when the opcode has no subject.
    pub subject_kind: u16,
    /// Storage phase.
    pub phase: OperationPhase,
    /// Flags; see the `FLAG_*` constants.
    pub flags: u8,
    /// Logical object ID, or `AbortOperation`'s target bytes `0..8`.
    pub logical_id: u64,
    /// Expected revision, or `AbortOperation`'s target bytes `8..16`.
    pub expected_revision: u64,
    /// The private generation, meaningful only with [`ActiveOperation::FLAG_GENERATION_RESERVED`].
    pub generation: GenerationId,
    /// The terminal-commit counter at this operation's last durable progress.
    pub progress_counter: u64,
    /// The latest work-checkpoint sequence; inactive zero without work.
    pub work_sequence: u32,
    /// `AbortOperation`'s reason; zero for every other opcode.
    pub abort_reason: u8,
}

impl ActiveOperation {
    /// Encoded length.
    pub const LEN: usize = 128;
    /// Bit 0: the operation's upload is resumable.
    pub const FLAG_RESUMABLE: u8 = 1 << 0;
    /// Bit 1: the operation is a draft parent.
    pub const FLAG_DRAFT_PARENT: u8 = 1 << 1;
    /// Bit 2: the operation is a draft child.
    pub const FLAG_DRAFT_CHILD: u8 = 1 << 2;
    /// Bit 3: the row is the reserved cancellation/recovery slot.
    pub const FLAG_RESERVED_SLOT: u8 = 1 << 3;
    /// Bit 4: a generation is reserved, and then zero is a valid `generation`.
    pub const FLAG_GENERATION_RESERVED: u8 = 1 << 4;
    const FLAG_MASK: u8 = 0b0001_1111;

    /// True when `opcode` is one a stored row may carry: a registered wire opcode, or one of the
    /// two `0xFF00`-block storage-internal claim tags (§5.3).
    pub fn opcode_is_registered(opcode: u16) -> bool {
        Opcode::from_u16(opcode).is_some()
            || opcode == CLAIM_TAG_WEATHER_CONTEXT
            || opcode == CLAIM_TAG_UPDATE_RECONCILIATION
    }

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_bytes(&mut out, 0, self.operation.as_bytes());
        put_bytes(&mut out, 16, &self.intent);
        put_bytes(&mut out, 48, &self.principal);
        put_u16(&mut out, 80, self.opcode);
        put_u16(&mut out, 82, self.subject_kind);
        out[84] = self.phase as u8;
        out[85] = self.flags;
        put_u64(&mut out, 88, self.logical_id);
        put_u64(&mut out, 96, self.expected_revision);
        put_u64(&mut out, 104, self.generation.get());
        put_u64(&mut out, 112, self.progress_counter);
        put_u32(&mut out, 120, self.work_sequence);
        out[124] = self.abort_reason;
        out
    }

    /// Decodes one occupied row.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::ActiveOperation;
        fixed(R, bytes, Self::LEN)?;
        let opcode = u16_at(bytes, 80);
        if !Self::opcode_is_registered(opcode) {
            return Err(err(R, Reason::UnknownEnum));
        }
        let phase = OperationPhase::from_u8(bytes[84]).ok_or(err(R, Reason::UnknownEnum))?;
        let flags = bytes[85];
        if flags & !Self::FLAG_MASK != 0 {
            return Err(err(R, Reason::Reserved));
        }
        reserved(R, bytes, 86, 2)?;
        // §5.3: the reason byte is `AbortOperation`'s alone.
        if opcode != Opcode::AbortOperation as u16 && bytes[124] != 0 {
            return Err(err(R, Reason::Reserved));
        }
        reserved(R, bytes, 125, 3)?;
        Ok(ActiveOperation {
            operation: OperationId::new(bytes16_at(bytes, 0)),
            intent: bytes32_at(bytes, 16),
            principal: bytes32_at(bytes, 48),
            opcode,
            subject_kind: u16_at(bytes, 82),
            phase,
            flags,
            logical_id: u64_at(bytes, 88),
            expected_revision: u64_at(bytes, 96),
            generation: GenerationId::new(u64_at(bytes, 104)),
            progress_counter: u64_at(bytes, 112),
            work_sequence: u32_at(bytes, 120),
            abort_reason: bytes[124],
        })
    }

    /// Encodes the removal form: the 16 key bytes only.
    pub fn encode_removal(operation: OperationId) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_bytes(&mut out, 0, operation.as_bytes());
        out
    }

    /// Decodes the removal form.
    pub fn decode_removal(bytes: &[u8]) -> Result<OperationId> {
        const R: Record = Record::ActiveOperation;
        fixed(R, bytes, Self::LEN)?;
        only_keys(R, bytes, &[(0, 16)])?;
        Ok(OperationId::new(bytes16_at(bytes, 0)))
    }
}

// -------------------------------------------------------------------------------------------
// Draft parent — 128 bytes, keyed by parent OperationId
// -------------------------------------------------------------------------------------------

/// The lifecycle state of the one draft parent (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftParentState {
    /// Accepting parts.
    Open = 1,
    /// The parent-owned manifest is streaming.
    ManifestStreaming = 2,
    /// The manifest passed its checks and a resolution generation is reserved.
    Finalizing = 3,
    /// The parent is unwinding.
    Aborting = 4,
}

impl DraftParentState {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => DraftParentState::Open,
            2 => DraftParentState::ManifestStreaming,
            3 => DraftParentState::Finalizing,
            4 => DraftParentState::Aborting,
            _ => return None,
        })
    }
}

/// The one draft parent (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftParent {
    /// The parent claim, and this row's key.
    pub parent: OperationId,
    /// The `BeginDraft` intent digest.
    pub intent: [u8; 32],
    /// The private parent-manifest generation.
    pub manifest_generation: GenerationId,
    /// The final manifest's object kind.
    pub manifest_kind: u16,
    /// The declared part count.
    pub declared_parts: u16,
    /// Lifecycle state.
    pub state: DraftParentState,
    /// Target mode: create `0`, replace `1`.
    pub replace: bool,
    /// Target logical ID, zero for create.
    pub target_id: LogicalObjectId,
    /// Expected revision, zero for create.
    pub expected_revision: Revision,
    /// Declared final manifest length.
    pub manifest_length: u64,
    /// Declared final manifest CRC-32.
    pub manifest_crc: u32,
    /// Monotonic draft revision; `1` for a newly created parent.
    pub draft_revision: u64,
    /// The terminal-commit counter at last durable progress.
    pub progress_counter: u64,
    /// The latest parent-manifest WORK sequence.
    pub work_sequence: u32,
    /// The reserved resolution generation, meaningful only in `Finalizing`.
    pub resolution: GenerationId,
}

impl DraftParent {
    /// Encoded length.
    pub const LEN: usize = 128;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_bytes(&mut out, 0, self.parent.as_bytes());
        put_bytes(&mut out, 16, &self.intent);
        put_u64(&mut out, 48, self.manifest_generation.get());
        put_u16(&mut out, 56, self.manifest_kind);
        put_u16(&mut out, 58, self.declared_parts);
        out[60] = self.state as u8;
        out[61] = u8::from(self.replace);
        put_u64(&mut out, 64, self.target_id.get());
        put_u64(&mut out, 72, self.expected_revision.get());
        put_u64(&mut out, 80, self.manifest_length);
        put_u32(&mut out, 88, self.manifest_crc);
        put_u64(&mut out, 96, self.draft_revision);
        put_u64(&mut out, 104, self.progress_counter);
        put_u32(&mut out, 112, self.work_sequence);
        put_u64(&mut out, 116, self.resolution.get());
        out
    }

    /// Decodes the one occupied row.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::DraftParent;
        fixed(R, bytes, Self::LEN)?;
        let state = DraftParentState::from_u8(bytes[60]).ok_or(err(R, Reason::UnknownEnum))?;
        let replace = match bytes[61] {
            0 => false,
            1 => true,
            _ => return Err(err(R, Reason::UnknownEnum)),
        };
        if state != DraftParentState::Finalizing && !is_zero(bytes, 116, 8) {
            return Err(err(R, Reason::Reserved));
        }
        reserved(R, bytes, 62, 2)?;
        reserved(R, bytes, 92, 4)?;
        reserved(R, bytes, 124, 4)?;
        Ok(DraftParent {
            parent: OperationId::new(bytes16_at(bytes, 0)),
            intent: bytes32_at(bytes, 16),
            manifest_generation: GenerationId::new(u64_at(bytes, 48)),
            manifest_kind: u16_at(bytes, 56),
            declared_parts: u16_at(bytes, 58),
            state,
            replace,
            target_id: LogicalObjectId::new(u64_at(bytes, 64)),
            expected_revision: Revision::new(u64_at(bytes, 72)),
            manifest_length: u64_at(bytes, 80),
            manifest_crc: u32_at(bytes, 88),
            draft_revision: u64_at(bytes, 96),
            progress_counter: u64_at(bytes, 104),
            work_sequence: u32_at(bytes, 112),
            resolution: GenerationId::new(u64_at(bytes, 116)),
        })
    }

    /// Encodes the removal form: the 16 key bytes only.
    pub fn encode_removal(parent: OperationId) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_bytes(&mut out, 0, parent.as_bytes());
        out
    }

    /// Decodes the removal form.
    pub fn decode_removal(bytes: &[u8]) -> Result<OperationId> {
        const R: Record = Record::DraftParent;
        fixed(R, bytes, Self::LEN)?;
        only_keys(R, bytes, &[(0, 16)])?;
        Ok(OperationId::new(bytes16_at(bytes, 0)))
    }
}

// -------------------------------------------------------------------------------------------
// Draft part — 96 bytes, keyed by (parent OperationId, DraftPartKind, part key)
// -------------------------------------------------------------------------------------------

/// A draft part's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartKey {
    /// The parent claim.
    pub parent: OperationId,
    /// `DraftPartKind`.
    pub kind: u16,
    /// The part key inside its kind.
    pub key: u64,
}

impl PartKey {
    /// The §5.1 sort order: the 16 parent bytes lexicographically, then kind, then part key.
    pub fn sort_key(&self) -> ([u8; 16], u16, u64) {
        (self.parent.to_bytes(), self.kind, self.key)
    }
}

/// The storage state of a draft part (§5.3). These values are storage's own, not the wire's: a
/// codec translates rather than casts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftPartState {
    /// Claimed, before the first accepted payload byte.
    Prepared = 4,
    /// Accepting payload bytes.
    Streaming = 1,
    /// Sealed, immutable, holding its minted `DraftPartRef`.
    Sealed = 2,
    /// Durably aborted.
    Aborted = 3,
}

impl DraftPartState {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => DraftPartState::Streaming,
            2 => DraftPartState::Sealed,
            3 => DraftPartState::Aborted,
            4 => DraftPartState::Prepared,
            _ => return None,
        })
    }

    /// The wire `QueryDraft` part state this storage state projects onto (§5.3).
    pub fn wire_state(self) -> u8 {
        match self {
            DraftPartState::Prepared => 0,
            DraftPartState::Streaming => 1,
            DraftPartState::Sealed => 2,
            DraftPartState::Aborted => 3,
        }
    }
}

/// One draft-part membership row (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftPart {
    /// `(parent, DraftPartKind, part key)`.
    pub key: PartKey,
    /// The child claim.
    pub child: OperationId,
    /// The opaque reference minted at seal; zero while prepared or streaming.
    pub part_ref: DraftPartRef,
    /// The private generation the part's payload was written as.
    pub generation: GenerationId,
    /// Payload length.
    pub length: u64,
    /// Payload CRC-32.
    pub crc: u32,
    /// Storage state.
    pub state: DraftPartState,
}

impl DraftPart {
    /// Encoded length.
    pub const LEN: usize = 96;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_bytes(&mut out, 0, self.key.parent.as_bytes());
        put_bytes(&mut out, 16, self.child.as_bytes());
        put_bytes(&mut out, 32, self.part_ref.as_bytes());
        put_u16(&mut out, 48, self.key.kind);
        put_u64(&mut out, 52, self.key.key);
        put_u64(&mut out, 60, self.generation.get());
        put_u64(&mut out, 68, self.length);
        put_u32(&mut out, 76, self.crc);
        out[80] = self.state as u8;
        out
    }

    /// Decodes one occupied row.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::DraftPart;
        fixed(R, bytes, Self::LEN)?;
        let state = DraftPartState::from_u8(bytes[80]).ok_or(err(R, Reason::UnknownEnum))?;
        reserved(R, bytes, 50, 2)?;
        reserved(R, bytes, 81, 15)?;
        Ok(DraftPart {
            key: PartKey {
                parent: OperationId::new(bytes16_at(bytes, 0)),
                kind: u16_at(bytes, 48),
                key: u64_at(bytes, 52),
            },
            child: OperationId::new(bytes16_at(bytes, 16)),
            part_ref: DraftPartRef::new(bytes16_at(bytes, 32)),
            generation: GenerationId::new(u64_at(bytes, 60)),
            length: u64_at(bytes, 68),
            crc: u32_at(bytes, 76),
            state,
        })
    }

    /// Encodes the removal form: parent, kind and part key only.
    pub fn encode_removal(key: PartKey) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_bytes(&mut out, 0, key.parent.as_bytes());
        put_u16(&mut out, 48, key.kind);
        put_u64(&mut out, 52, key.key);
        out
    }

    /// Decodes the removal form.
    pub fn decode_removal(bytes: &[u8]) -> Result<PartKey> {
        const R: Record = Record::DraftPart;
        fixed(R, bytes, Self::LEN)?;
        only_keys(R, bytes, &[(0, 16), (48, 50), (52, 60)])?;
        Ok(PartKey { parent: OperationId::new(bytes16_at(bytes, 0)), kind: u16_at(bytes, 48), key: u64_at(bytes, 52) })
    }
}

// -------------------------------------------------------------------------------------------
// Retained previous generation — 64 bytes, keyed by GenerationId
// -------------------------------------------------------------------------------------------

/// One generation held back from collection (§5.3, §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedPrevious {
    /// Reason flags; see the `REASON_*` constants. An entry with no reason is removed.
    pub reasons: u8,
    /// Live leases on this generation at the moment it was displaced.
    pub lease_count: u16,
    /// The object kind it was the head of.
    pub kind: u16,
    /// The logical object it was the head of.
    pub logical_id: LogicalObjectId,
    /// The generation, and this entry's key.
    pub generation: GenerationId,
    /// Payload length.
    pub length: u64,
    /// Payload CRC-32.
    pub crc: u32,
    /// Retain-through terminal counter; `0` means reason-controlled.
    pub retain_through: u64,
    /// The object revision this generation was the head at — a diagnostic, never a lookup key.
    pub object_revision: Revision,
}

impl RetainedPrevious {
    /// Encoded length.
    pub const LEN: usize = 64;
    /// Bit 0: a reader holds a lease on these bytes.
    pub const REASON_LIVE_LEASE: u8 = 1 << 0;
    /// Bit 1: this is the way back to the running image.
    pub const REASON_UPDATE_ROLLBACK: u8 = 1 << 1;
    /// Bit 2: a repository's own bounded continuity policy holds it.
    pub const REASON_DOMAIN_RETENTION: u8 = 1 << 2;
    const REASON_MASK: u8 = 0b0000_0111;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = 1;
        out[1] = self.reasons;
        put_u16(&mut out, 2, self.lease_count);
        put_u16(&mut out, 4, self.kind);
        put_u64(&mut out, 8, self.logical_id.get());
        put_u64(&mut out, 16, self.generation.get());
        put_u64(&mut out, 24, self.length);
        put_u32(&mut out, 32, self.crc);
        put_u64(&mut out, 40, self.retain_through);
        put_u64(&mut out, 48, self.object_revision.get());
        out
    }

    /// Decodes one occupied entry.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::RetainedPrevious;
        fixed(R, bytes, Self::LEN)?;
        occupied(R, bytes)?;
        let reasons = bytes[1];
        if reasons & !Self::REASON_MASK != 0 {
            return Err(err(R, Reason::Reserved));
        }
        // §9: "An entry whose reasons have all been cleared is removed", so a stored entry always
        // carries at least one reason.
        if reasons == 0 {
            return Err(err(R, Reason::Combination));
        }
        // §9: the count "never exceeds the four-lease capacity of section 2, which admission proves
        // before publication".
        if u16_at(bytes, 2) as usize > super::limits::MAX_LEASES {
            return Err(err(R, Reason::Overflow));
        }
        if reasons & Self::REASON_LIVE_LEASE == 0 && u16_at(bytes, 2) != 0 {
            return Err(err(R, Reason::Combination));
        }
        reserved(R, bytes, 6, 2)?;
        reserved(R, bytes, 36, 4)?;
        reserved(R, bytes, 56, 8)?;
        Ok(RetainedPrevious {
            reasons,
            lease_count: u16_at(bytes, 2),
            kind: u16_at(bytes, 4),
            logical_id: LogicalObjectId::new(u64_at(bytes, 8)),
            generation: GenerationId::new(u64_at(bytes, 16)),
            length: u64_at(bytes, 24),
            crc: u32_at(bytes, 32),
            retain_through: u64_at(bytes, 40),
            object_revision: Revision::new(u64_at(bytes, 48)),
        })
    }

    /// Encodes the removal form: the occupied byte and the generation key.
    pub fn encode_removal(generation: GenerationId) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = 1;
        put_u64(&mut out, 16, generation.get());
        out
    }

    /// Decodes the removal form.
    pub fn decode_removal(bytes: &[u8]) -> Result<GenerationId> {
        const R: Record = Record::RetainedPrevious;
        fixed(R, bytes, Self::LEN)?;
        occupied(R, bytes)?;
        only_keys(R, bytes, &[(0, 1), (16, 24)])?;
        Ok(GenerationId::new(u64_at(bytes, 16)))
    }
}

// -------------------------------------------------------------------------------------------
// Terminal result — 208 bytes, keyed and ordered by commit sequence
// -------------------------------------------------------------------------------------------

/// A terminal result's payload shape (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultType {
    /// A diagnostic-text-free `ErrorBody`.
    Aborted = 0,
    /// `ObjectResult`.
    Object = 1,
    /// `DraftPartResult`.
    DraftPart = 2,
    /// `AbortResult`.
    Abort = 3,
    /// The storage-local `DomainResult`.
    Domain = 4,
}

impl ResultType {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => ResultType::Aborted,
            1 => ResultType::Object,
            2 => ResultType::DraftPart,
            3 => ResultType::Abort,
            4 => ResultType::Domain,
            _ => return None,
        })
    }

    /// The exact encoded length §5.3 fixes for this shape.
    pub const fn encoded_len(self) -> u16 {
        match self {
            ResultType::Aborted => 48,
            ResultType::Object => 64,
            ResultType::DraftPart => 88,
            ResultType::Abort => 56,
            ResultType::Domain => 48,
        }
    }
}

/// One retained terminal result (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalResult {
    /// The terminal-commit counter after increment — not the journal sequence.
    pub commit_sequence: u64,
    /// The operation this result answers for.
    pub operation: OperationId,
    /// Its canonical-intent digest.
    pub intent: [u8; 32],
    /// The opaque stable principal-scope digest, so status can be authorized.
    pub principal: [u8; 32],
    /// Terminal state: committed `1`, aborted `2`.
    pub committed: bool,
    /// Result shape.
    pub result_type: ResultType,
    /// The exact result or `ErrorBody` bytes, zero-padded to 88.
    pub body: [u8; 88],
}

impl TerminalResult {
    /// Encoded length.
    pub const LEN: usize = 208;
    /// The body reservation.
    pub const BODY_CAPACITY: usize = 88;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_u64(&mut out, 0, self.commit_sequence);
        put_bytes(&mut out, 8, self.operation.as_bytes());
        put_bytes(&mut out, 24, &self.intent);
        put_bytes(&mut out, 56, &self.principal);
        out[88] = if self.committed { 1 } else { 2 };
        out[89] = self.result_type as u8;
        put_u16(&mut out, 90, self.result_type.encoded_len());
        put_bytes(&mut out, 104, &self.body);
        out
    }

    /// Decodes one occupied entry.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::TerminalResult;
        fixed(R, bytes, Self::LEN)?;
        let committed = match bytes[88] {
            1 => true,
            2 => false,
            _ => return Err(err(R, Reason::UnknownEnum)),
        };
        let result_type = ResultType::from_u8(bytes[89]).ok_or(err(R, Reason::UnknownEnum))?;
        if u16_at(bytes, 90) != result_type.encoded_len() {
            return Err(err(R, Reason::Overflow));
        }
        reserved(R, bytes, 92, 12)?;
        // The body is "the exact result or diagnostic-text-free ErrorBody, followed by zero".
        let body_len = result_type.encoded_len() as usize;
        reserved(R, bytes, 104 + body_len, Self::BODY_CAPACITY - body_len)?;
        reserved(R, bytes, 192, 16)?;
        let mut body = [0u8; Self::BODY_CAPACITY];
        body.copy_from_slice(&bytes[104..192]);
        Ok(TerminalResult {
            commit_sequence: u64_at(bytes, 0),
            operation: OperationId::new(bytes16_at(bytes, 8)),
            intent: bytes32_at(bytes, 24),
            principal: bytes32_at(bytes, 56),
            committed,
            result_type,
            body,
        })
    }
}

// -------------------------------------------------------------------------------------------
// Weather request state — 80 bytes, singleton
// -------------------------------------------------------------------------------------------

/// The one weather-request state (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WeatherState {
    /// `true` when a bundle has answered the current request context.
    pub satisfied: bool,
    /// Flags; bit 0 is "weather head present".
    pub flags: u16,
    /// The durable request identity.
    pub request: WeatherRequestId,
    /// The request-context revision.
    pub context_revision: u64,
    /// The reserved weather logical object.
    pub logical_id: LogicalObjectId,
    /// The weather repository revision captured for response compare-and-swap.
    pub captured_revision: Revision,
    /// Required centre latitude, signed degrees times 10,000,000.
    pub latitude_e7: i32,
    /// Required centre longitude, signed degrees times 10,000,000.
    pub longitude_e7: i32,
    /// Required radius, metres.
    pub radius_m: u32,
    /// Earliest issued UTC, signed Unix seconds.
    pub earliest_issued: i64,
    /// Required valid-until UTC, signed Unix seconds.
    pub valid_until: i64,
    /// The head's request ID; inactive zero only when head-present is clear.
    pub head_request: WeatherRequestId,
}

impl WeatherState {
    /// Encoded length.
    pub const LEN: usize = 80;
    /// Bit 0: a weather catalog head exists.
    pub const FLAG_HEAD_PRESENT: u16 = 1 << 0;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = 1;
        out[1] = if self.satisfied { 2 } else { 1 };
        put_u16(&mut out, 2, self.flags);
        put_u64(&mut out, 4, self.request.get());
        put_u64(&mut out, 12, self.context_revision);
        put_u64(&mut out, 20, self.logical_id.get());
        put_u64(&mut out, 28, self.captured_revision.get());
        put_i32(&mut out, 36, self.latitude_e7);
        put_i32(&mut out, 40, self.longitude_e7);
        put_u32(&mut out, 44, self.radius_m);
        put_i64(&mut out, 52, self.earliest_issued);
        put_i64(&mut out, 60, self.valid_until);
        put_u64(&mut out, 68, self.head_request.get());
        out
    }

    /// Decodes the singleton.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::WeatherState;
        fixed(R, bytes, Self::LEN)?;
        occupied(R, bytes)?;
        let satisfied = match bytes[1] {
            1 => false,
            2 => true,
            _ => return Err(err(R, Reason::UnknownEnum)),
        };
        let flags = u16_at(bytes, 2);
        if flags & !Self::FLAG_HEAD_PRESENT != 0 {
            return Err(err(R, Reason::Reserved));
        }
        if flags & Self::FLAG_HEAD_PRESENT == 0 {
            reserved(R, bytes, 68, 8)?;
        }
        reserved(R, bytes, 48, 4)?;
        reserved(R, bytes, 76, 4)?;
        Ok(WeatherState {
            satisfied,
            flags,
            request: WeatherRequestId::new(u64_at(bytes, 4)),
            context_revision: u64_at(bytes, 12),
            logical_id: LogicalObjectId::new(u64_at(bytes, 20)),
            captured_revision: Revision::new(u64_at(bytes, 28)),
            latitude_e7: i32_at(bytes, 36),
            longitude_e7: i32_at(bytes, 40),
            radius_m: u32_at(bytes, 44),
            earliest_issued: i64_at(bytes, 52),
            valid_until: i64_at(bytes, 60),
            head_request: WeatherRequestId::new(u64_at(bytes, 68)),
        })
    }
}

// -------------------------------------------------------------------------------------------
// Active ride state — 128 bytes, singleton
// -------------------------------------------------------------------------------------------

/// The lifecycle state of the one active or recoverable ride (§5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideState {
    /// Samples are being recorded.
    Recording = 1,
    /// The stop sequence has begun.
    Stopping = 2,
    /// The payload is sealed and a matching sealed RIDE slot exists.
    Sealed = 3,
    /// An ordinary publication claim owns it.
    Claimed = 4,
    /// Recovery could not reconcile it.
    RecoveryFault = 5,
}

impl RideState {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => RideState::Recording,
            2 => RideState::Stopping,
            3 => RideState::Sealed,
            4 => RideState::Claimed,
            5 => RideState::RecoveryFault,
            _ => return None,
        })
    }
}

/// The one active-ride state (§5.3). It is authoritative for existence, identity and lifecycle;
/// `RIDE.ACT` is authoritative for payload progress and seal facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveRide {
    /// Lifecycle state.
    pub state: RideState,
    /// Flags; bit 0 is "historical route snapshot present".
    pub flags: u8,
    /// The ride-recovery revision: the initial domain journal sequence, fixed for this ride.
    pub recovery_revision: u64,
    /// The CSPRNG local publication operation, durable from the first domain record.
    pub operation: OperationId,
    /// The device-local ride-producer principal digest.
    pub principal: [u8; 32],
    /// The prospective ride generation.
    pub generation: GenerationId,
    /// Start UTC, signed Unix seconds.
    pub start_utc: i64,
    /// Historical route logical ID; inactive zero when the flag is clear.
    pub route_id: LogicalObjectId,
    /// Historical route revision; inactive zero when the flag is clear.
    pub route_revision: Revision,
}

impl ActiveRide {
    /// Encoded length.
    pub const LEN: usize = 128;
    /// Bit 0: the start-of-ride route snapshot fields are valid.
    pub const FLAG_ROUTE_SNAPSHOT: u8 = 1 << 0;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = 1;
        out[1] = self.state as u8;
        out[2] = self.flags;
        put_u64(&mut out, 8, self.recovery_revision);
        put_bytes(&mut out, 16, self.operation.as_bytes());
        put_bytes(&mut out, 32, &self.principal);
        put_u64(&mut out, 64, self.generation.get());
        put_i64(&mut out, 72, self.start_utc);
        put_u64(&mut out, 80, self.route_id.get());
        put_u64(&mut out, 88, self.route_revision.get());
        out
    }

    /// Decodes the singleton.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::ActiveRide;
        fixed(R, bytes, Self::LEN)?;
        occupied(R, bytes)?;
        let state = RideState::from_u8(bytes[1]).ok_or(err(R, Reason::UnknownEnum))?;
        let flags = bytes[2];
        if flags & !Self::FLAG_ROUTE_SNAPSHOT != 0 {
            return Err(err(R, Reason::Reserved));
        }
        if flags & Self::FLAG_ROUTE_SNAPSHOT == 0 {
            reserved(R, bytes, 80, 16)?;
        }
        reserved(R, bytes, 3, 5)?;
        // §5.3: "payload progress and seal facts are authoritative only in RIDE.ACT".
        reserved(R, bytes, 96, 32)?;
        Ok(ActiveRide {
            state,
            flags,
            recovery_revision: u64_at(bytes, 8),
            operation: OperationId::new(bytes16_at(bytes, 16)),
            principal: bytes32_at(bytes, 32),
            generation: GenerationId::new(u64_at(bytes, 64)),
            start_utc: i64_at(bytes, 72),
            route_id: LogicalObjectId::new(u64_at(bytes, 80)),
            route_revision: Revision::new(u64_at(bytes, 88)),
        })
    }

    /// Encodes the removal form: the occupied byte alone, since the row is a singleton.
    pub fn encode_removal() -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        out[0] = 1;
        out
    }

    /// Decodes the removal form.
    pub fn decode_removal(bytes: &[u8]) -> Result<()> {
        const R: Record = Record::ActiveRide;
        fixed(R, bytes, Self::LEN)?;
        occupied(R, bytes)?;
        only_keys(R, bytes, &[(0, 1)])
    }
}

#[cfg(test)]
mod tests {
    use super::super::samples::{active, head, parent, part, repository, retained, ride, weather, OP_A};
    use super::*;
    use std::vec::Vec;

    fn result(sequence: u64) -> TerminalResult {
        super::super::samples::result(sequence, OP_A)
    }

    #[test]
    fn every_entry_round_trips() {
        assert_eq!(RepositoryState::decode(&repository(1, 4).encode()).unwrap(), repository(1, 4));
        assert_eq!(CatalogHead::decode(&head(1, 7).encode()).unwrap(), head(1, 7));
        assert_eq!(ActiveOperation::decode(&active(OP_A).encode()).unwrap(), active(OP_A));
        assert_eq!(DraftParent::decode(&parent().encode()).unwrap(), parent());
        assert_eq!(DraftPart::decode(&part(1).encode()).unwrap(), part(1));
        assert_eq!(RetainedPrevious::decode(&retained(9).encode()).unwrap(), retained(9));
        assert_eq!(TerminalResult::decode(&result(1).encode()).unwrap(), result(1));
        assert_eq!(WeatherState::decode(&weather().encode()).unwrap(), weather());
        assert_eq!(ActiveRide::decode(&ride().encode()).unwrap(), ride());
    }

    #[test]
    fn removals_carry_only_key_bytes() {
        let key = HeadKey { kind: 1, id: LogicalObjectId::new(7) };
        assert_eq!(CatalogHead::decode_removal(&CatalogHead::encode_removal(key)).unwrap(), key);
        let op = OperationId::new(OP_A);
        assert_eq!(ActiveOperation::decode_removal(&ActiveOperation::encode_removal(op)).unwrap(), op);
        assert_eq!(DraftParent::decode_removal(&DraftParent::encode_removal(op)).unwrap(), op);
        let part_key = PartKey { parent: op, kind: 1, key: 3 };
        assert_eq!(DraftPart::decode_removal(&DraftPart::encode_removal(part_key)).unwrap().key, 3);
        let generation = GenerationId::new(9);
        assert_eq!(
            RetainedPrevious::decode_removal(&RetainedPrevious::encode_removal(generation)).unwrap(),
            generation
        );
        ActiveRide::decode_removal(&ActiveRide::encode_removal()).unwrap();
    }

    #[test]
    fn a_removal_with_one_nonzero_non_key_byte_is_rejected() {
        let mut bytes = CatalogHead::encode_removal(HeadKey { kind: 1, id: LogicalObjectId::new(7) });
        bytes[20] = 1;
        assert_eq!(CatalogHead::decode_removal(&bytes).unwrap_err().reason, Reason::KeyBytes);

        let mut bytes = ActiveRide::encode_removal();
        bytes[64] = 1;
        assert_eq!(ActiveRide::decode_removal(&bytes).unwrap_err().reason, Reason::KeyBytes);
    }

    #[test]
    fn a_removal_missing_its_occupied_byte_is_rejected() {
        let mut bytes = CatalogHead::encode_removal(HeadKey { kind: 1, id: LogicalObjectId::new(7) });
        bytes[0] = 0;
        assert_eq!(CatalogHead::decode_removal(&bytes).unwrap_err().reason, Reason::Occupied);
    }

    #[test]
    fn an_unregistered_opcode_is_not_a_storable_row() {
        assert!(ActiveOperation::opcode_is_registered(0x0100));
        assert!(ActiveOperation::opcode_is_registered(CLAIM_TAG_WEATHER_CONTEXT));
        assert!(ActiveOperation::opcode_is_registered(CLAIM_TAG_UPDATE_RECONCILIATION));
        assert!(!ActiveOperation::opcode_is_registered(0xFF03));
        let mut bytes = active(OP_A).encode();
        bytes[80..82].copy_from_slice(&0x0999u16.to_le_bytes());
        assert_eq!(ActiveOperation::decode(&bytes).unwrap_err().reason, Reason::UnknownEnum);
    }

    #[test]
    fn an_abort_reason_on_a_non_abort_row_is_rejected() {
        let mut bytes = active(OP_A).encode();
        bytes[124] = 3;
        assert_eq!(ActiveOperation::decode(&bytes).unwrap_err().reason, Reason::Reserved);
        // The same byte on an AbortOperation row is the reason field and decodes.
        let mut abort = active(OP_A);
        abort.opcode = Opcode::AbortOperation as u16;
        abort.abort_reason = 3;
        assert_eq!(ActiveOperation::decode(&abort.encode()).unwrap().abort_reason, 3);
    }

    #[test]
    fn inactive_alternatives_must_be_zero() {
        let mut head = head(1, 7);
        head.resolution = GenerationId::new(5);
        assert_eq!(CatalogHead::decode(&head.encode()).unwrap_err().reason, Reason::Reserved);
        head.flags = CatalogHead::FLAG_RESOLUTION_PRESENT;
        assert_eq!(CatalogHead::decode(&head.encode()).unwrap(), head);

        let mut ride = ride();
        ride.flags = 0;
        assert_eq!(ActiveRide::decode(&ride.encode()).unwrap_err().reason, Reason::Reserved);

        let mut weather = weather();
        weather.flags = 0;
        assert_eq!(WeatherState::decode(&weather.encode()).unwrap_err().reason, Reason::Reserved);
    }

    #[test]
    fn an_envelope_outside_its_declared_length_must_be_zero() {
        let mut head = head(1, 7);
        head.envelope[8] = 1;
        assert_eq!(CatalogHead::decode(&head.encode()).unwrap_err().reason, Reason::Reserved);
        head.envelope_len = 7;
        assert_eq!(CatalogHead::decode(&head.encode()).unwrap_err().reason, Reason::Overflow);
        head.envelope_len = 97;
        assert_eq!(CatalogHead::decode(&head.encode()).unwrap_err().reason, Reason::Overflow);
    }

    #[test]
    fn a_result_length_must_match_its_type() {
        let mut bytes = result(1).encode();
        bytes[90..92].copy_from_slice(&88u16.to_le_bytes());
        assert_eq!(TerminalResult::decode(&bytes).unwrap_err().reason, Reason::Overflow);
        for (result_type, len) in [
            (ResultType::Aborted, 48),
            (ResultType::Object, 64),
            (ResultType::DraftPart, 88),
            (ResultType::Abort, 56),
            (ResultType::Domain, 48),
        ] {
            assert_eq!(result_type.encoded_len(), len);
        }
    }

    #[test]
    fn a_retained_entry_with_no_reason_or_an_impossible_lease_count_is_rejected() {
        let mut entry = retained(9);
        entry.reasons = 0;
        assert_eq!(RetainedPrevious::decode(&entry.encode()).unwrap_err().reason, Reason::Combination);
        let mut entry = retained(9);
        entry.lease_count = 5;
        assert_eq!(RetainedPrevious::decode(&entry.encode()).unwrap_err().reason, Reason::Overflow);
        let mut entry = retained(9);
        entry.reasons = RetainedPrevious::REASON_UPDATE_ROLLBACK;
        assert_eq!(RetainedPrevious::decode(&entry.encode()).unwrap_err().reason, Reason::Combination);
    }

    #[test]
    fn draft_part_storage_states_project_onto_the_wire_values() {
        assert_eq!(DraftPartState::Prepared.wire_state(), 0);
        assert_eq!(DraftPartState::Streaming.wire_state(), 1);
        assert_eq!(DraftPartState::Sealed.wire_state(), 2);
        assert_eq!(DraftPartState::Aborted.wire_state(), 3);
    }

    /// Every entry is total: no byte pattern of its exact length panics, and every refusal is
    /// typed. This is the property the §6 fuzz corpus generalizes.
    #[test]
    fn decoding_is_total_over_single_byte_mutations() {
        type Decoder = fn(&[u8]) -> bool;
        let samples: Vec<(Vec<u8>, Decoder)> = std::vec![
            (head(1, 7).encode().to_vec(), (|b| CatalogHead::decode(b).is_ok()) as Decoder),
            (active(OP_A).encode().to_vec(), |b| ActiveOperation::decode(b).is_ok()),
            (parent().encode().to_vec(), |b| DraftParent::decode(b).is_ok()),
            (part(1).encode().to_vec(), |b| DraftPart::decode(b).is_ok()),
            (retained(9).encode().to_vec(), |b| RetainedPrevious::decode(b).is_ok()),
            (result(1).encode().to_vec(), |b| TerminalResult::decode(b).is_ok()),
            (weather().encode().to_vec(), |b| WeatherState::decode(b).is_ok()),
            (ride().encode().to_vec(), |b| ActiveRide::decode(b).is_ok()),
            (repository(1, 4).encode().to_vec(), |b| RepositoryState::decode(b).is_ok()),
        ];
        for (bytes, decode) in samples {
            for index in 0..bytes.len() {
                let mut mutated = bytes.clone();
                mutated[index] ^= 0xFF;
                let _ = decode(&mutated);
            }
        }
    }
}
