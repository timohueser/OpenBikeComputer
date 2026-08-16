//! The `WORK` slot and the `RIDE.ACT` slot (`OBC2_Storage_Format.md` §7 and §7.1).
//!
//! Both are the same shape — a 512-byte body at a slot base, its gate at base `+ 512`, and a zero
//! pad to the 16,384-byte stride — and both answer the same question: how far a payload is durably
//! written. They differ in what selects the authoritative slot. A `WORK` file alternates two slots;
//! `RIDE.ACT` is a 16-slot ring written at `checkpoint_sequence mod 16`. In both cases recovery
//! takes the greatest valid sequence **whose durable offset is at most the payload's observed
//! length**, because a durable offset above the observed length is not merely stale, it is
//! unreachable: §13.1's adapter cannot seek past a recorded length.
//!
//! ## What the restart-only profile leaves unwritten
//!
//! §7: "The restart-only profile writes no WORK slots." The initial device advertises no resumable
//! kind, so it records no durable upload progress: every readmission truncates the claimed
//! generation and streams from offset zero, and recovery classifies a claimed unsealed generation
//! as restartable work at offset zero. **Seal still writes its sealed WORK slot** — the one durable
//! work fact both profiles share — and the file, its slot layout, and its preallocation stay part
//! of the frozen format so a card moves between profiles without conversion. This module therefore
//! encodes and decodes both states; what the initial firmware never *writes* is a `Streaming` slot.

use obc_link::ids::{DraftPartRef, GenerationId, OperationId, StoreId};

use super::error::{DecodeError, Reason, Record, Result};
use super::gate::{BodyBinding, Gate, MAGIC_RIDE, MAGIC_WORK};
use super::limits::{SLOT_STRIDE, SMALL_BODY_CRC_OFFSET, SMALL_BODY_LEN, SMALL_GATE_OFFSET, WORK_SLOTS};
use super::raw::{
    bytes16_at, bytes32_at, crc32, crc32_with_hole, i64_at, is_zero, put_bytes, put_i64, put_u16, put_u32, put_u64,
    require_zero, u16_at, u32_at, u64_at,
};

/// `WORK` body magic.
pub const WORK_MAGIC: [u8; 4] = *b"O2WK";
/// The header length a `WORK` body declares.
pub const WORK_HEADER_LEN: usize = 176;
/// `RIDE.ACT` body magic.
pub const RIDE_MAGIC: [u8; 4] = *b"O2RA";
/// The header length a `RIDE.ACT` body declares.
pub const RIDE_HEADER_LEN: usize = 140;

/// The state of a work record (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkState {
    /// Payload bytes are being accepted.
    Streaming = 1,
    /// The payload is sealed: exact length and whole-object CRC verified, file closed.
    Sealed = 2,
}

/// Which namespace the work record's subject lives in (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    /// A logical object — an ordinary upload, or a parent-owned manifest.
    LogicalObject = 1,
    /// A draft part.
    DraftPart = 2,
}

/// One `WORK` slot body (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkRecord {
    /// The store this record belongs to.
    pub store: StoreId,
    /// The child for a part, the parent for its manifest, or the ordinary operation.
    pub operation: OperationId,
    /// Its canonical-intent digest.
    pub intent: [u8; 32],
    /// The parent claim for a draft child; inactive zero otherwise.
    pub parent: OperationId,
    /// The opaque reference after child seal; inactive zero otherwise.
    pub part_ref: DraftPartRef,
    /// The private generation these bytes are being written as, and the gate's scope.
    pub generation: GenerationId,
    /// The declared payload length.
    pub declared_length: u64,
    /// The declared payload CRC-32.
    pub declared_crc: u32,
    /// Streaming or sealed.
    pub state: WorkState,
    /// Flags; bit 0 is "resumable".
    pub flags: u8,
    /// The durable next offset. Never above the declared length.
    pub durable_offset: u64,
    /// The finalized CRC-32 of the prefix through the durable offset.
    pub prefix_crc: u32,
    /// The work-checkpoint sequence, and the gate's logical sequence.
    pub sequence: u32,
    /// The terminal-commit counter at last durable progress.
    pub progress_counter: u64,
    /// `ObjectKind` or `DraftPartKind`.
    pub subject_kind: u16,
    /// Which of those two the kind is.
    pub subject: Subject,
    /// The draft part key, or zero.
    pub part_key: u64,
    /// The payload file length observed after its sync.
    pub observed_length: u32,
}

impl WorkRecord {
    /// Bit 0 of `flags`: this upload advertises resumable durable progress.
    pub const FLAG_RESUMABLE: u8 = 1 << 0;

    /// Encodes the 512-byte body with its CRC stamped.
    pub fn encode_body(&self) -> [u8; SMALL_BODY_LEN] {
        let mut out = [0u8; SMALL_BODY_LEN];
        put_bytes(&mut out, 0, &WORK_MAGIC);
        put_u16(&mut out, 4, super::gate::FORMAT_VERSION);
        put_u16(&mut out, 6, WORK_HEADER_LEN as u16);
        put_bytes(&mut out, 8, self.store.as_bytes());
        put_bytes(&mut out, 24, self.operation.as_bytes());
        put_bytes(&mut out, 40, &self.intent);
        put_bytes(&mut out, 72, self.parent.as_bytes());
        put_bytes(&mut out, 88, self.part_ref.as_bytes());
        put_u64(&mut out, 104, self.generation.get());
        put_u64(&mut out, 112, self.declared_length);
        put_u32(&mut out, 120, self.declared_crc);
        out[124] = self.state as u8;
        out[125] = self.flags;
        put_u64(&mut out, 128, self.durable_offset);
        put_u32(&mut out, 136, self.prefix_crc);
        put_u32(&mut out, 140, self.sequence);
        put_u64(&mut out, 144, self.progress_counter);
        put_u16(&mut out, 152, self.subject_kind);
        out[154] = self.subject as u8;
        put_u64(&mut out, 156, self.part_key);
        put_u32(&mut out, 164, self.observed_length);
        let crc = crc32_with_hole(&out, SMALL_BODY_CRC_OFFSET);
        put_u32(&mut out, SMALL_BODY_CRC_OFFSET, crc);
        out
    }

    /// Writes the complete 16,384-byte slot into `out`: body, gate, and the pad to the next stride.
    ///
    /// The in-place form is the one production code uses. A 16 KiB array returned by value is a
    /// 16 KiB stack temporary at every call site, and the board's task stacks are measured in tens
    /// of kilobytes. [`encode_slot`](Self::encode_slot) is the same bytes for a host that does not
    /// care.
    pub fn encode_slot_into(&self, out: &mut [u8], slot: u16) -> Result<()> {
        if out.len() != SLOT_STRIDE {
            return Err(DecodeError::new(Record::Work, Reason::Length));
        }
        out.fill(0);
        let body = self.encode_body();
        put_bytes(out, 0, &body);
        let gate = Gate {
            magic: MAGIC_WORK,
            slot,
            scope: self.generation.get(),
            sequence: u64::from(self.sequence),
            body_crc: u32_at(&body, SMALL_BODY_CRC_OFFSET),
        };
        put_bytes(out, SMALL_GATE_OFFSET, &gate.encode());
        Ok(())
    }

    /// The same slot, returned by value. Host-only: see [`encode_slot_into`](Self::encode_slot_into).
    #[cfg(any(test, feature = "std"))]
    pub fn encode_slot(&self, slot: u16) -> [u8; SLOT_STRIDE] {
        let mut out = [0u8; SLOT_STRIDE];
        self.encode_slot_into(&mut out, slot).expect("a stride-sized buffer");
        out
    }

    /// Decodes the 512-byte body.
    pub fn decode_body(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::Work;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() != SMALL_BODY_LEN {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != WORK_MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != super::gate::FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) as usize != WORK_HEADER_LEN {
            return Err(err(Reason::HeaderLength));
        }
        let state = match bytes[124] {
            1 => WorkState::Streaming,
            2 => WorkState::Sealed,
            _ => return Err(err(Reason::UnknownEnum)),
        };
        let flags = bytes[125];
        if flags & !Self::FLAG_RESUMABLE != 0 {
            return Err(err(Reason::Reserved));
        }
        let subject = match bytes[154] {
            1 => Subject::LogicalObject,
            2 => Subject::DraftPart,
            _ => return Err(err(Reason::UnknownEnum)),
        };
        // The two draft-only fields are "valid including all zero" for a draft child and "inactive
        // zero otherwise", and the ref is minted only at seal.
        if subject == Subject::LogicalObject && (!is_zero(bytes, 72, 16) || !is_zero(bytes, 156, 8)) {
            return Err(err(Reason::Reserved));
        }
        let sealed_part = subject == Subject::DraftPart && state == WorkState::Sealed;
        if !sealed_part && !is_zero(bytes, 88, 16) {
            return Err(err(Reason::Reserved));
        }
        let declared_length = u64_at(bytes, 112);
        if declared_length > super::limits::MAX_GENERATION_LEN {
            return Err(err(Reason::Overflow));
        }
        // §7: "The durable offset cannot exceed the declared length."
        if u64_at(bytes, 128) > declared_length {
            return Err(err(Reason::Overflow));
        }
        require_zero(R, bytes, 126, 2)?;
        require_zero(R, bytes, 155, 1)?;
        require_zero(R, bytes, 168, 8)?;
        require_zero(R, bytes, WORK_HEADER_LEN, SMALL_BODY_CRC_OFFSET - WORK_HEADER_LEN)?;
        Ok(WorkRecord {
            store: StoreId::new(bytes16_at(bytes, 8)),
            operation: OperationId::new(bytes16_at(bytes, 24)),
            intent: bytes32_at(bytes, 40),
            parent: OperationId::new(bytes16_at(bytes, 72)),
            part_ref: DraftPartRef::new(bytes16_at(bytes, 88)),
            generation: GenerationId::new(u64_at(bytes, 104)),
            declared_length,
            declared_crc: u32_at(bytes, 120),
            state,
            flags,
            durable_offset: u64_at(bytes, 128),
            prefix_crc: u32_at(bytes, 136),
            sequence: u32_at(bytes, 140),
            progress_counter: u64_at(bytes, 144),
            subject_kind: u16_at(bytes, 152),
            subject,
            part_key: u64_at(bytes, 156),
            observed_length: u32_at(bytes, 164),
        })
    }

    /// Validates a complete slot: body, gate binding, and a zero pad to the stride.
    pub fn validate_slot(slot_bytes: &[u8], slot: u16) -> Result<Self> {
        const R: Record = Record::Work;
        if slot_bytes.len() != SLOT_STRIDE {
            return Err(DecodeError::new(R, Reason::Length));
        }
        let record = Self::decode_body(&slot_bytes[..SMALL_BODY_LEN])?;
        let gate = Gate::decode(&slot_bytes[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + 512], MAGIC_WORK, slot)?;
        gate.bind(&BodyBinding {
            stored_crc: u32_at(slot_bytes, SMALL_BODY_CRC_OFFSET),
            fresh_crc: crc32_with_hole(&slot_bytes[..SMALL_BODY_LEN], SMALL_BODY_CRC_OFFSET),
            scope: record.generation.get(),
            sequence: u64::from(record.sequence),
        })?;
        require_zero(R, slot_bytes, 1_024, SLOT_STRIDE - 1_024)?;
        Ok(record)
    }

    /// True when this slot's durable offset is reachable in a payload of `observed` bytes.
    ///
    /// §7's rewind rule: "a slot recording an offset the payload cannot reach is skipped as if
    /// invalid". The record's own `observed_length` is what it saw at its own checkpoint; the
    /// argument is what recovery sees now, and the smaller of the two is what binds.
    pub fn offset_is_reachable(&self, observed: u64) -> bool {
        self.durable_offset <= observed
    }
}

/// The recovery-evidence state of a ride (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideEvidence {
    /// Samples are being appended.
    Recording = 1,
    /// The stop sequence has begun.
    Stopping = 2,
    /// Final length and CRC are recorded.
    Sealed = 3,
}

/// One `RIDE.ACT` slot body (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideRecord {
    /// The store this record belongs to.
    pub store: StoreId,
    /// The ride-recovery revision, fixed for this ride.
    pub recovery_revision: u64,
    /// The local publication operation, durable from the initial domain record.
    pub operation: OperationId,
    /// The prospective ride generation, and the gate's scope.
    pub generation: GenerationId,
    /// Recovery-evidence state.
    pub state: RideEvidence,
    /// Flags; bit 0 is "historical route snapshot present".
    pub flags: u8,
    /// Start UTC, signed Unix seconds.
    pub start_utc: i64,
    /// Historical route logical ID; inactive zero when the flag is clear.
    pub route_id: u64,
    /// Historical route revision; inactive zero when the flag is clear.
    pub route_revision: u64,
    /// The durable payload offset.
    pub durable_offset: u64,
    /// The finalized CRC-32 through that offset.
    pub prefix_crc: u32,
    /// The ride-checkpoint sequence, and the gate's logical sequence.
    pub sequence: u32,
    /// Durable sample count.
    pub sample_count: u64,
    /// Durable elapsed milliseconds.
    pub elapsed_ms: u64,
    /// Sealed length; inactive zero before seal, and zero is valid when sealed.
    pub sealed_length: u64,
    /// Sealed CRC-32; inactive zero before seal, and zero is valid when sealed.
    pub sealed_crc: u32,
    /// The payload file length observed after its sync.
    pub observed_length: u32,
}

impl RideRecord {
    /// Bit 0 of `flags`: the start-of-ride route snapshot fields are valid.
    pub const FLAG_ROUTE_SNAPSHOT: u8 = 1 << 0;

    /// Encodes the 512-byte body with its CRC stamped.
    pub fn encode_body(&self) -> [u8; SMALL_BODY_LEN] {
        let mut out = [0u8; SMALL_BODY_LEN];
        put_bytes(&mut out, 0, &RIDE_MAGIC);
        put_u16(&mut out, 4, super::gate::FORMAT_VERSION);
        put_u16(&mut out, 6, RIDE_HEADER_LEN as u16);
        put_bytes(&mut out, 8, self.store.as_bytes());
        put_u64(&mut out, 24, self.recovery_revision);
        put_bytes(&mut out, 32, self.operation.as_bytes());
        put_u64(&mut out, 48, self.generation.get());
        out[56] = self.state as u8;
        out[57] = self.flags;
        put_i64(&mut out, 64, self.start_utc);
        put_u64(&mut out, 72, self.route_id);
        put_u64(&mut out, 80, self.route_revision);
        put_u64(&mut out, 88, self.durable_offset);
        put_u32(&mut out, 96, self.prefix_crc);
        put_u32(&mut out, 100, self.sequence);
        put_u64(&mut out, 104, self.sample_count);
        put_u64(&mut out, 112, self.elapsed_ms);
        put_u64(&mut out, 120, self.sealed_length);
        put_u32(&mut out, 128, self.sealed_crc);
        put_u32(&mut out, 136, self.observed_length);
        let crc = crc32_with_hole(&out, SMALL_BODY_CRC_OFFSET);
        put_u32(&mut out, SMALL_BODY_CRC_OFFSET, crc);
        out
    }

    /// Writes the complete 16,384-byte slot into `out`: body, gate, and the pad to the next stride.
    ///
    /// The in-place form is the one production code uses. A 16 KiB array returned by value is a
    /// 16 KiB stack temporary at every call site, and the board's task stacks are measured in tens
    /// of kilobytes. [`encode_slot`](Self::encode_slot) is the same bytes for a host that does not
    /// care.
    pub fn encode_slot_into(&self, out: &mut [u8], slot: u16) -> Result<()> {
        if out.len() != SLOT_STRIDE {
            return Err(DecodeError::new(Record::RideSlot, Reason::Length));
        }
        out.fill(0);
        let body = self.encode_body();
        put_bytes(out, 0, &body);
        let gate = Gate {
            magic: MAGIC_RIDE,
            slot,
            scope: self.generation.get(),
            sequence: u64::from(self.sequence),
            body_crc: u32_at(&body, SMALL_BODY_CRC_OFFSET),
        };
        put_bytes(out, SMALL_GATE_OFFSET, &gate.encode());
        Ok(())
    }

    /// The same slot, returned by value. Host-only: see [`encode_slot_into`](Self::encode_slot_into).
    #[cfg(any(test, feature = "std"))]
    pub fn encode_slot(&self, slot: u16) -> [u8; SLOT_STRIDE] {
        let mut out = [0u8; SLOT_STRIDE];
        self.encode_slot_into(&mut out, slot).expect("a stride-sized buffer");
        out
    }

    /// Decodes the 512-byte body.
    pub fn decode_body(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::RideSlot;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() != SMALL_BODY_LEN {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != RIDE_MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != super::gate::FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) as usize != RIDE_HEADER_LEN {
            return Err(err(Reason::HeaderLength));
        }
        let state = match bytes[56] {
            1 => RideEvidence::Recording,
            2 => RideEvidence::Stopping,
            3 => RideEvidence::Sealed,
            _ => return Err(err(Reason::UnknownEnum)),
        };
        let flags = bytes[57];
        if flags & !Self::FLAG_ROUTE_SNAPSHOT != 0 {
            return Err(err(Reason::Reserved));
        }
        if flags & Self::FLAG_ROUTE_SNAPSHOT == 0 {
            require_zero(R, bytes, 72, 16)?;
        }
        if state != RideEvidence::Sealed && !is_zero(bytes, 120, 12) {
            return Err(err(Reason::Reserved));
        }
        require_zero(R, bytes, 58, 6)?;
        require_zero(R, bytes, 132, 4)?;
        require_zero(R, bytes, RIDE_HEADER_LEN, SMALL_BODY_CRC_OFFSET - RIDE_HEADER_LEN)?;
        Ok(RideRecord {
            store: StoreId::new(bytes16_at(bytes, 8)),
            recovery_revision: u64_at(bytes, 24),
            operation: OperationId::new(bytes16_at(bytes, 32)),
            generation: GenerationId::new(u64_at(bytes, 48)),
            state,
            flags,
            start_utc: i64_at(bytes, 64),
            route_id: u64_at(bytes, 72),
            route_revision: u64_at(bytes, 80),
            durable_offset: u64_at(bytes, 88),
            prefix_crc: u32_at(bytes, 96),
            sequence: u32_at(bytes, 100),
            sample_count: u64_at(bytes, 104),
            elapsed_ms: u64_at(bytes, 112),
            sealed_length: u64_at(bytes, 120),
            sealed_crc: u32_at(bytes, 128),
            observed_length: u32_at(bytes, 136),
        })
    }

    /// Validates a complete slot, including that the slot index is the one §7.1's ring rule puts
    /// this sequence in: `checkpoint_sequence mod 16`.
    pub fn validate_slot(slot_bytes: &[u8], slot: u16) -> Result<Self> {
        const R: Record = Record::RideSlot;
        if slot_bytes.len() != SLOT_STRIDE {
            return Err(DecodeError::new(R, Reason::Length));
        }
        let record = Self::decode_body(&slot_bytes[..SMALL_BODY_LEN])?;
        if (record.sequence as usize % super::limits::RIDE_SLOTS) != slot as usize {
            return Err(DecodeError::new(R, Reason::SlotIndex));
        }
        let gate = Gate::decode(&slot_bytes[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + 512], MAGIC_RIDE, slot)?;
        gate.bind(&BodyBinding {
            stored_crc: u32_at(slot_bytes, SMALL_BODY_CRC_OFFSET),
            fresh_crc: crc32_with_hole(&slot_bytes[..SMALL_BODY_LEN], SMALL_BODY_CRC_OFFSET),
            scope: record.generation.get(),
            sequence: u64::from(record.sequence),
        })?;
        require_zero(R, slot_bytes, 1_024, SLOT_STRIDE - 1_024)?;
        Ok(record)
    }

    /// True when this slot's durable offset is reachable in a payload of `observed` bytes (§7.1).
    pub fn offset_is_reachable(&self, observed: u64) -> bool {
        self.durable_offset <= observed
    }
}

/// The slot recovery selects out of a set of candidates: the greatest valid sequence whose durable
/// offset is at most the payload's observed length (§7, §7.1).
///
/// Returning `None` is not a fault. §7: if no slot records a reachable offset "the payload is
/// truncated to zero and work restarts at offset zero under the same GenerationId, which is the
/// same state `BeginWork` leaves behind"; §7.1 says the same for a ride.
pub fn select_by_reachable_sequence<T: Copy>(
    candidates: &[(u32, u64, T)],
    observed_length: u64,
) -> Option<(u32, u64, T)> {
    let mut best: Option<(u32, u64, T)> = None;
    for &(sequence, offset, value) in candidates {
        if offset > observed_length {
            continue;
        }
        match best {
            Some((best_sequence, _, _)) if best_sequence >= sequence => {}
            _ => best = Some((sequence, offset, value)),
        }
    }
    best
}

/// What §7 recovery decided about one generation's durable work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkRecovery {
    /// No slot records a reachable offset: "the payload is truncated to zero and work restarts at
    /// offset zero under the same GenerationId, which is the same state `BeginWork` leaves behind".
    /// It is also what the restart-only profile recovers for every claimed unsealed generation,
    /// which writes no streaming slot at all.
    RestartAtZero,
    /// Resume from this slot's durable offset. Its prefix CRC has been verified against the payload.
    Resume(WorkRecord),
    /// "If a qualifying slot's prefix CRC mismatches, the work is discarded and its operation
    /// terminally aborted; nothing is ever resumed from an unverifiable prefix."
    DiscardAndAbort,
}

/// §7's selection and verification, as one function.
///
/// `payload` is the generation's bytes as they are now. The selection rule and the verification are
/// deliberately one step: §7 makes the rewind mandatory ("a durable offset above the observed
/// length is not merely stale, it is unreachable") and then requires the prefix to be *proved*
/// before a byte is accepted after it. A caller that selected a slot and skipped the CRC would
/// resume from a prefix the card no longer holds.
pub fn recover_work(slots: &[Option<WorkRecord>; WORK_SLOTS], payload: &[u8]) -> WorkRecovery {
    let observed = payload.len() as u64;
    // The selection is [`select_by_reachable_sequence`]'s, not a second copy of it: the two WORK
    // slots become its candidate list, and what this function adds is the prefix proof.
    let mut candidates: [(u32, u64, Option<WorkRecord>); WORK_SLOTS] = [(0, u64::MAX, None); WORK_SLOTS];
    for (candidate, held) in candidates.iter_mut().zip(slots.iter()) {
        if let Some(record) = held {
            *candidate = (record.sequence, record.durable_offset, Some(*record));
        }
    }
    let selected = select_by_reachable_sequence(&candidates, observed);
    let Some(record) = selected.and_then(|(_, _, record)| record) else { return WorkRecovery::RestartAtZero };
    let offset = record.durable_offset as usize;
    if crc32(&payload[..offset]) == record.prefix_crc {
        WorkRecovery::Resume(record)
    } else {
        WorkRecovery::DiscardAndAbort
    }
}

/// What §7.1 recovery decided about the one active ride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RideRecovery {
    /// The checkpoint says no ride exists, so the RIDE file cannot invent one: "A RIDE file alone
    /// cannot invent an active ride."
    NoRide,
    /// "A recording state with no matching slot is the initial durable offset zero; recovery
    /// recreates or truncates the GEN payload to zero."
    RestartAtZero,
    /// Resume from this slot. Losing the newest slot to a torn page costs at most one checkpoint
    /// interval of samples and elapsed time, which §1.1 accepts for a ride journal.
    Resume(RideRecord),
    /// Two valid slots claim one sequence with differing bodies (§7.1).
    Corrupt,
}

/// The identity a `RIDE.ACT` slot must match to be this ride's evidence (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideIdentity {
    /// The store.
    pub store: StoreId,
    /// The local publication operation the initial domain record fixed.
    pub operation: OperationId,
    /// The prospective ride generation.
    pub generation: GenerationId,
    /// The ride-recovery revision.
    pub recovery_revision: u64,
}

/// §7.1's selection, over the 16-slot ring.
///
/// `slots[i]` is `Some` only when ring position `i` validated completely — which already proves
/// `sequence mod 16 == i`, since [`RideRecord::validate_slot`] makes the ring position part of
/// validity. What is left is the identity match, the greatest reachable sequence, and the
/// equal-sequence conflict rule.
pub fn recover_ride(
    slots: &[Option<RideRecord>],
    identity: Option<RideIdentity>,
    observed_payload_len: u64,
) -> RideRecovery {
    // "A RIDE file alone cannot invent an active ride": without ActiveRideState there is no ride,
    // however many valid slots the file holds.
    let Some(identity) = identity else { return RideRecovery::NoRide };

    let mut best: Option<RideRecord> = None;
    for record in slots.iter().flatten() {
        let matches = record.store == identity.store
            && record.operation == identity.operation
            && record.generation == identity.generation
            && record.recovery_revision == identity.recovery_revision;
        if !matches {
            continue;
        }
        // "equal-sequence differing bodies are corruption" — the ring maps one sequence onto one
        // position, so two valid slots at one sequence can only differ.
        if let Some(held) = best {
            if held.sequence == record.sequence && held != *record {
                return RideRecovery::Corrupt;
            }
        }
        // §7.1 applies §7's rewind with the slot count changed: a slot recording an offset the
        // payload cannot reach "is skipped as if invalid".
        if !record.offset_is_reachable(observed_payload_len) {
            continue;
        }
        match best {
            Some(held) if held.sequence >= record.sequence => {}
            _ => best = Some(*record),
        }
    }
    match best {
        Some(record) => RideRecovery::Resume(record),
        None => RideRecovery::RestartAtZero,
    }
}

#[cfg(test)]
mod tests {
    use super::super::samples::{ride_slot as ride, work};
    use super::*;

    #[test]
    fn work_round_trips_through_a_whole_slot() {
        let record = work(3, 4_096, WorkState::Streaming);
        let slot = record.encode_slot(1);
        assert_eq!(WorkRecord::validate_slot(&slot, 1).unwrap(), record);
        assert!(WorkRecord::validate_slot(&slot, 0).is_err());
    }

    #[test]
    fn ride_round_trips_and_binds_its_slot_to_the_ring_position() {
        let record = ride(17, 8_192);
        let slot = record.encode_slot(1);
        assert_eq!(RideRecord::validate_slot(&slot, 1).unwrap(), record);
        // 17 mod 16 is 1, so the same body read at slot 2 is not this ring position.
        assert_eq!(RideRecord::validate_slot(&record.encode_slot(2), 2).unwrap_err().reason, Reason::SlotIndex);
    }

    #[test]
    fn a_durable_offset_above_the_declared_length_is_not_a_record() {
        let mut record = work(3, 4_096, WorkState::Streaming);
        record.durable_offset = record.declared_length + 1;
        assert_eq!(WorkRecord::decode_body(&record.encode_body()).unwrap_err().reason, Reason::Overflow);
    }

    #[test]
    fn a_sealed_part_may_carry_a_ref_and_a_streaming_one_may_not() {
        let mut record = work(3, 4_096, WorkState::Streaming);
        record.subject = Subject::DraftPart;
        record.part_key = 1;
        record.parent = OperationId::new([0xC3; 16]);
        record.part_ref = DraftPartRef::new([0x5A; 16]);
        assert_eq!(WorkRecord::decode_body(&record.encode_body()).unwrap_err().reason, Reason::Reserved);
        record.state = WorkState::Sealed;
        record.durable_offset = record.declared_length;
        assert_eq!(WorkRecord::decode_body(&record.encode_body()).unwrap(), record);
    }

    #[test]
    fn draft_fields_on_a_logical_object_record_are_inactive_zero() {
        let mut record = work(3, 4_096, WorkState::Streaming);
        record.parent = OperationId::new([0xC3; 16]);
        assert_eq!(WorkRecord::decode_body(&record.encode_body()).unwrap_err().reason, Reason::Reserved);
    }

    #[test]
    fn sealed_ride_facts_are_inactive_zero_before_seal() {
        let mut record = ride(1, 4_096);
        record.sealed_length = 4_096;
        assert_eq!(RideRecord::decode_body(&record.encode_body()).unwrap_err().reason, Reason::Reserved);
        record.state = RideEvidence::Sealed;
        assert_eq!(RideRecord::decode_body(&record.encode_body()).unwrap(), record);
    }

    /// §7's mandatory rewind: the greatest sequence whose offset the payload can actually reach.
    #[test]
    fn selection_skips_a_slot_whose_offset_the_payload_cannot_reach() {
        let candidates = [(1u32, 4_096u64, 'a'), (2, 12_288, 'b')];
        assert_eq!(select_by_reachable_sequence(&candidates, 12_288).unwrap().2, 'b');
        // A cut left the directory entry short: slot 2's offset is unreachable, so slot 1 wins.
        assert_eq!(select_by_reachable_sequence(&candidates, 8_192).unwrap().2, 'a');
        // Neither is reachable: work restarts at offset zero, which is not a fault.
        assert!(select_by_reachable_sequence(&candidates, 0).is_none());
    }

    /// §7's prefix clause: a qualifying slot is resumed only after its finalized CRC is proved
    /// against the bytes the payload actually holds.
    #[test]
    fn work_recovery_verifies_the_prefix_before_resuming() {
        let payload = std::vec![0xABu8; 4_096];
        let mut slot = work(1, 1_024, WorkState::Streaming);
        slot.prefix_crc = super::super::raw::crc32(&payload[..1_024]);
        assert_eq!(recover_work(&[Some(slot), None], &payload), WorkRecovery::Resume(slot));

        // "If a qualifying slot's prefix CRC mismatches, the work is discarded and its operation
        // terminally aborted; nothing is ever resumed from an unverifiable prefix."
        let mut mismatched = slot;
        mismatched.prefix_crc ^= 1;
        assert_eq!(recover_work(&[Some(mismatched), None], &payload), WorkRecovery::DiscardAndAbort);
    }

    /// The restart-only profile writes no streaming slot at all, so its claimed generation recovers
    /// as restartable work at offset zero — the same outcome as a pair whose offsets are all
    /// unreachable.
    #[test]
    fn work_recovery_restarts_at_zero_without_a_reachable_slot() {
        let payload = std::vec![0u8; 1_024];
        assert_eq!(recover_work(&[None, None], &payload), WorkRecovery::RestartAtZero);
        let unreachable = work(2, 8_192, WorkState::Streaming);
        assert_eq!(recover_work(&[Some(unreachable), None], &payload), WorkRecovery::RestartAtZero);
    }

    /// The newest reachable slot wins even when a newer one records an unreachable offset — §7's
    /// mandatory rewind, now with the CRC proof attached.
    #[test]
    fn work_recovery_rewinds_to_the_newest_reachable_slot() {
        let payload = std::vec![0x5Au8; 2_048];
        let mut older = work(1, 1_024, WorkState::Streaming);
        older.prefix_crc = super::super::raw::crc32(&payload[..1_024]);
        let newer = work(2, 8_192, WorkState::Streaming);
        assert_eq!(recover_work(&[Some(older), Some(newer)], &payload), WorkRecovery::Resume(older));
    }

    fn identity() -> RideIdentity {
        let sample = ride(0, 0);
        RideIdentity {
            store: sample.store,
            operation: sample.operation,
            generation: sample.generation,
            recovery_revision: sample.recovery_revision,
        }
    }

    /// §7.1: the ring's greatest matching sequence wins, and a torn newest page costs at most one
    /// checkpoint interval because recovery falls back to the newest valid earlier slot.
    #[test]
    fn ride_recovery_selects_the_greatest_matching_sequence_and_falls_back() {
        let slots = [Some(ride(0, 1_024)), Some(ride(1, 2_048)), None];
        assert_eq!(recover_ride(&slots, Some(identity()), 4_096), RideRecovery::Resume(ride(1, 2_048)));

        // The newest slot's page was torn, so it is not a valid slot at all: the previous one is
        // authoritative and one interval of samples is lost.
        let torn = [Some(ride(0, 1_024)), None, None];
        assert_eq!(recover_ride(&torn, Some(identity()), 4_096), RideRecovery::Resume(ride(0, 1_024)));
    }

    /// "A RIDE file alone cannot invent an active ride."
    #[test]
    fn ride_slots_without_active_ride_state_are_not_a_ride() {
        let slots = [Some(ride(0, 1_024))];
        assert_eq!(recover_ride(&slots, None, 4_096), RideRecovery::NoRide);
    }

    /// A slot whose identity does not match the checkpoint's ActiveRideState is another ride's
    /// evidence, not this one's.
    #[test]
    fn ride_slots_from_another_ride_are_ignored() {
        let mut foreign = ride(3, 2_048);
        foreign.generation = GenerationId::new(999);
        assert_eq!(recover_ride(&[Some(foreign)], Some(identity()), 4_096), RideRecovery::RestartAtZero);
    }

    /// §7.1 applies §7's rewind with the slot count changed.
    #[test]
    fn ride_recovery_skips_an_unreachable_offset_and_restarts_when_none_qualifies() {
        let slots = [Some(ride(0, 1_024)), Some(ride(1, 12_288)), None];
        assert_eq!(recover_ride(&slots, Some(identity()), 4_096), RideRecovery::Resume(ride(0, 1_024)));
        assert_eq!(recover_ride(&slots, Some(identity()), 0), RideRecovery::RestartAtZero);
    }

    /// "equal-sequence differing bodies are corruption."
    #[test]
    fn two_differing_ride_slots_at_one_sequence_are_corruption() {
        let mut other = ride(0, 2_048);
        other.sample_count += 1;
        assert_eq!(recover_ride(&[Some(ride(0, 1_024)), Some(other)], Some(identity()), 4_096), RideRecovery::Corrupt);
    }

    #[test]
    fn a_nonzero_pad_invalidates_either_slot() {
        let mut slot = work(3, 4_096, WorkState::Streaming).encode_slot(0);
        slot[5_000] = 1;
        assert_eq!(WorkRecord::validate_slot(&slot, 0).unwrap_err().reason, Reason::Reserved);
        let mut slot = ride(0, 4_096).encode_slot(0);
        slot[5_000] = 1;
        assert_eq!(RideRecord::validate_slot(&slot, 0).unwrap_err().reason, Reason::Reserved);
    }
}
