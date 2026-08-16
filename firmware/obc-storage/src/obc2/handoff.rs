//! The update A/B handoff: `HandoffRef` and the `ARM0.HND`/`ARM1.HND` record
//! (`OBC2_Storage_Format.md` §10).
//!
//! `HandoffRef` is the 240-byte fact that binds one `InstallUpdate` operation to the OBCU boot-state
//! page. It appears twice: as the body of an ARM record, and as the checkpoint's one handoff
//! projection (§5.1), which is why its codec lives here rather than beside the other projection
//! entries — the ARM record is where its rules are stated.
//!
//! Recovery selects the valid ARM file with the lexicographically greatest `(handoff_sequence,
//! phase)` pair, so both fields are load-bearing and both are cross-checked against the gate:
//! §10 requires the ref's sequence to equal the body and gate scope and its phase to equal the
//! gate's logical sequence. A record whose three copies disagree is not a weaker record, it is an
//! invalid one.

use obc_link::ids::{GenerationId, LogicalObjectId, OperationId, Revision, StoreId};

use super::error::{DecodeError, Reason, Record, Result};
use super::gate::{BodyBinding, Gate, MAGIC_HANDOFF};
use super::limits::{HANDOFF_REF_LEN, SLOT_FILE_LEN, SMALL_BODY_CRC_OFFSET, SMALL_BODY_LEN, SMALL_GATE_OFFSET};
use super::raw::{
    bytes16_at, bytes32_at, crc32_with_hole, put_bytes, put_u16, put_u32, put_u64, require_zero, u16_at, u32_at, u64_at,
};

/// ARM body magic.
pub const MAGIC: [u8; 4] = *b"O2UH";
/// The header length the ARM body declares.
pub const HEADER_LEN: usize = 64;

/// The handoff phase (§10). Phases advance strictly in numeric order and each is written once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HandoffPhase {
    /// The package and optional snapshot are pinned; the OBCU page is not written yet.
    Prepared = 1,
    /// The OBCU Armed page is written and verified.
    Armed = 2,
    /// A Trial page has been observed after the reset.
    TrialObserved = 3,
    /// A terminal OBCU outcome has been observed.
    OutcomeObserved = 4,
    /// The outcome is committed and the handoff is ready for cleanup.
    Complete = 5,
}

impl HandoffPhase {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            1 => HandoffPhase::Prepared,
            2 => HandoffPhase::Armed,
            3 => HandoffPhase::TrialObserved,
            4 => HandoffPhase::OutcomeObserved,
            5 => HandoffPhase::Complete,
            _ => return None,
        })
    }
}

/// The OBCU outcome a handoff has observed (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObcuOutcome {
    /// Nothing observed yet.
    None = 0,
    /// The image installed.
    Installed = 1,
    /// The trial rolled back.
    RolledBack = 2,
    /// The staged image was rejected.
    StageRejected = 3,
    /// The arm was abandoned.
    ArmAbandoned = 4,
}

impl ObcuOutcome {
    fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => ObcuOutcome::None,
            1 => ObcuOutcome::Installed,
            2 => ObcuOutcome::RolledBack,
            3 => ObcuOutcome::StageRejected,
            4 => ObcuOutcome::ArmAbandoned,
            _ => return None,
        })
    }
}

/// The 240-byte handoff reference (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffRef {
    /// Store-global, nonzero, never wrapping.
    pub sequence: u64,
    /// The phase this record represents.
    pub phase: HandoffPhase,
    /// The observed OBCU outcome.
    pub outcome: ObcuOutcome,
    /// Flags; bit 0 is "rollback snapshot present".
    pub flags: u16,
    /// The `InstallUpdate` claim.
    pub operation: OperationId,
    /// Its canonical-intent digest.
    pub intent: [u8; 32],
    /// The immutable update-package generation.
    pub package_generation: GenerationId,
    /// Package length.
    pub package_length: u64,
    /// Package payload CRC-32.
    pub package_crc: u32,
    /// The nonzero OBCU arm generation.
    pub arm_generation: u32,
    /// SHA-256 of the exact encoded OBCU Armed blob, including its CRC.
    pub armed_blob_sha256: [u8; 32],
    /// The staged package's 64-byte OBCU `ImageHeader`.
    pub image_header: [u8; 64],
    /// The terminal-result commit sequence; zero until committed.
    pub terminal_commit: u64,
    /// The observed OBCU outcome generation; zero until observed.
    pub outcome_generation: u32,
    /// The private rollback-snapshot generation; inactive zero when the flag is clear.
    pub rollback_generation: GenerationId,
    /// Rollback-snapshot length; inactive zero when the flag is clear.
    pub rollback_length: u64,
    /// Rollback-snapshot CRC-32; inactive zero when the flag is clear.
    pub rollback_crc: u32,
    /// The update-package logical object.
    pub logical_id: LogicalObjectId,
    /// The latest update repository revision this handoff represents.
    pub revision: Revision,
}

impl HandoffRef {
    /// Encoded length.
    pub const LEN: usize = HANDOFF_REF_LEN;
    /// Bit 0: the rollback-snapshot fields are valid.
    pub const FLAG_ROLLBACK_SNAPSHOT: u16 = 1 << 0;

    /// Encodes the exact bytes.
    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut out = [0u8; Self::LEN];
        put_u64(&mut out, 0, self.sequence);
        out[8] = self.phase as u8;
        out[9] = self.outcome as u8;
        put_u16(&mut out, 10, self.flags);
        put_bytes(&mut out, 16, self.operation.as_bytes());
        put_bytes(&mut out, 32, &self.intent);
        put_u64(&mut out, 64, self.package_generation.get());
        put_u64(&mut out, 72, self.package_length);
        put_u32(&mut out, 80, self.package_crc);
        put_u32(&mut out, 84, self.arm_generation);
        put_bytes(&mut out, 88, &self.armed_blob_sha256);
        put_bytes(&mut out, 120, &self.image_header);
        put_u64(&mut out, 184, self.terminal_commit);
        put_u32(&mut out, 192, self.outcome_generation);
        put_u64(&mut out, 200, self.rollback_generation.get());
        put_u64(&mut out, 208, self.rollback_length);
        put_u32(&mut out, 216, self.rollback_crc);
        put_u64(&mut out, 224, self.logical_id.get());
        put_u64(&mut out, 232, self.revision.get());
        out
    }

    /// Decodes one reference.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::HandoffRef;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() != Self::LEN {
            return Err(err(Reason::Length));
        }
        let sequence = u64_at(bytes, 0);
        if sequence == 0 {
            return Err(err(Reason::Sequence));
        }
        let phase = HandoffPhase::from_u8(bytes[8]).ok_or(err(Reason::UnknownEnum))?;
        let outcome = ObcuOutcome::from_u8(bytes[9]).ok_or(err(Reason::UnknownEnum))?;
        let flags = u16_at(bytes, 10);
        if flags & !Self::FLAG_ROLLBACK_SNAPSHOT != 0 {
            return Err(err(Reason::Reserved));
        }
        let arm_generation = u32_at(bytes, 84);
        if arm_generation == 0 {
            return Err(err(Reason::Reserved));
        }
        if flags & Self::FLAG_ROLLBACK_SNAPSHOT == 0 {
            require_zero(R, bytes, 200, 20)?;
        }
        require_zero(R, bytes, 12, 4)?;
        require_zero(R, bytes, 196, 4)?;
        require_zero(R, bytes, 220, 4)?;
        Ok(HandoffRef {
            sequence,
            phase,
            outcome,
            flags,
            operation: OperationId::new(bytes16_at(bytes, 16)),
            intent: bytes32_at(bytes, 32),
            package_generation: GenerationId::new(u64_at(bytes, 64)),
            package_length: u64_at(bytes, 72),
            package_crc: u32_at(bytes, 80),
            arm_generation,
            armed_blob_sha256: bytes32_at(bytes, 88),
            image_header: {
                let mut header = [0u8; 64];
                header.copy_from_slice(&bytes[120..184]);
                header
            },
            terminal_commit: u64_at(bytes, 184),
            outcome_generation: u32_at(bytes, 192),
            rollback_generation: GenerationId::new(u64_at(bytes, 200)),
            rollback_length: u64_at(bytes, 208),
            rollback_crc: u32_at(bytes, 216),
            logical_id: LogicalObjectId::new(u64_at(bytes, 224)),
            revision: Revision::new(u64_at(bytes, 232)),
        })
    }

    /// The `(handoff_sequence, phase)` pair recovery selects the greatest of (§10).
    pub fn selector(&self) -> (u64, u8) {
        (self.sequence, self.phase as u8)
    }
}

/// One `ARM0.HND`/`ARM1.HND` body (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffRecord {
    /// The store this record belongs to.
    pub store: StoreId,
    /// The reference it carries. Its sequence is the body's and the gate's scope.
    pub handoff: HandoffRef,
}

impl HandoffRecord {
    /// Encodes the 512-byte body with its CRC stamped.
    pub fn encode_body(&self) -> [u8; SMALL_BODY_LEN] {
        let mut out = [0u8; SMALL_BODY_LEN];
        put_bytes(&mut out, 0, &MAGIC);
        put_u16(&mut out, 4, super::gate::FORMAT_VERSION);
        put_u16(&mut out, 6, HEADER_LEN as u16);
        put_bytes(&mut out, 8, self.store.as_bytes());
        put_u64(&mut out, 24, self.handoff.sequence);
        put_u16(&mut out, 32, HandoffRef::LEN as u16);
        put_bytes(&mut out, 64, &self.handoff.encode());
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
        if out.len() != SLOT_FILE_LEN {
            return Err(DecodeError::new(Record::Handoff, Reason::Length));
        }
        out.fill(0);
        let body = self.encode_body();
        put_bytes(out, 0, &body);
        let gate = Gate {
            magic: MAGIC_HANDOFF,
            slot,
            scope: self.handoff.sequence,
            sequence: self.handoff.phase as u64,
            body_crc: u32_at(&body, SMALL_BODY_CRC_OFFSET),
        };
        put_bytes(out, SMALL_GATE_OFFSET, &gate.encode());
        Ok(())
    }

    /// The same slot, returned by value. Host-only: see [`encode_slot_into`](Self::encode_slot_into).
    #[cfg(any(test, feature = "std"))]
    pub fn encode_slot(&self, slot: u16) -> [u8; SLOT_FILE_LEN] {
        let mut out = [0u8; SLOT_FILE_LEN];
        self.encode_slot_into(&mut out, slot).expect("a stride-sized buffer");
        out
    }

    /// Decodes the 512-byte body.
    pub fn decode_body(bytes: &[u8]) -> Result<Self> {
        const R: Record = Record::Handoff;
        let err = |reason| DecodeError::new(R, reason);
        if bytes.len() != SMALL_BODY_LEN {
            return Err(err(Reason::Length));
        }
        if bytes[0..4] != MAGIC {
            return Err(err(Reason::Magic));
        }
        if u16_at(bytes, 4) != super::gate::FORMAT_VERSION {
            return Err(err(Reason::Version));
        }
        if u16_at(bytes, 6) as usize != HEADER_LEN {
            return Err(err(Reason::HeaderLength));
        }
        if u16_at(bytes, 32) as usize != HandoffRef::LEN {
            return Err(err(Reason::Overflow));
        }
        require_zero(R, bytes, 34, 30)?;
        require_zero(R, bytes, 304, 204)?;
        let handoff = HandoffRef::decode(&bytes[64..304])?;
        // §10: "The HandoffRef sequence must equal the outer body ... a mismatch invalidates the
        // ARM record."
        if handoff.sequence != u64_at(bytes, 24) {
            return Err(err(Reason::Sequence));
        }
        Ok(HandoffRecord { store: StoreId::new(bytes16_at(bytes, 8)), handoff })
    }

    /// Validates a complete slot: body, gate binding, and a zero pad to the stride.
    pub fn validate_slot(slot_bytes: &[u8], slot: u16) -> Result<Self> {
        const R: Record = Record::Handoff;
        if slot_bytes.len() != SLOT_FILE_LEN {
            return Err(DecodeError::new(R, Reason::Length));
        }
        let record = Self::decode_body(&slot_bytes[..SMALL_BODY_LEN])?;
        let gate = Gate::decode(&slot_bytes[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + 512], MAGIC_HANDOFF, slot)?;
        gate.bind(&BodyBinding {
            stored_crc: u32_at(slot_bytes, SMALL_BODY_CRC_OFFSET),
            fresh_crc: crc32_with_hole(&slot_bytes[..SMALL_BODY_LEN], SMALL_BODY_CRC_OFFSET),
            scope: record.handoff.sequence,
            sequence: record.handoff.phase as u64,
        })?;
        require_zero(R, slot_bytes, 1_024, SLOT_FILE_LEN - 1_024)?;
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::super::samples::{handoff_record as record, handoff_ref};
    use super::*;

    #[test]
    fn round_trips_through_a_whole_slot() {
        let record = record(4, HandoffPhase::Armed);
        let slot = record.encode_slot(1);
        assert_eq!(HandoffRecord::validate_slot(&slot, 1).unwrap(), record);
        // The same bytes read as the other side of the alternation are not a record.
        assert!(HandoffRecord::validate_slot(&slot, 0).is_err());
    }

    #[test]
    fn a_nonzero_pad_invalidates_the_slot() {
        let mut slot = record(4, HandoffPhase::Armed).encode_slot(0);
        slot[2_000] = 1;
        assert_eq!(HandoffRecord::validate_slot(&slot, 0).unwrap_err().reason, Reason::Reserved);
    }

    #[test]
    fn the_gate_sequence_is_the_phase_so_a_phase_edit_invalidates_the_record() {
        let mut slot = record(4, HandoffPhase::Armed).encode_slot(0);
        slot[64 + 8] = HandoffPhase::Complete as u8;
        let crc = crc32_with_hole(&slot[..SMALL_BODY_LEN], SMALL_BODY_CRC_OFFSET);
        slot[SMALL_BODY_CRC_OFFSET..SMALL_BODY_CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        // Even with a repaired body CRC the gate no longer names this body.
        assert!(HandoffRecord::validate_slot(&slot, 0).is_err());
    }

    #[test]
    fn a_zero_sequence_or_arm_generation_is_rejected() {
        let mut reference = handoff_ref(1, HandoffPhase::Prepared);
        reference.sequence = 0;
        assert_eq!(HandoffRef::decode(&reference.encode()).unwrap_err().reason, Reason::Sequence);
        let mut reference = handoff_ref(1, HandoffPhase::Prepared);
        reference.arm_generation = 0;
        assert_eq!(HandoffRef::decode(&reference.encode()).unwrap_err().reason, Reason::Reserved);
    }

    #[test]
    fn rollback_fields_are_inactive_zero_without_the_flag() {
        let mut reference = handoff_ref(1, HandoffPhase::Prepared);
        reference.rollback_length = 4_096;
        assert_eq!(HandoffRef::decode(&reference.encode()).unwrap_err().reason, Reason::Reserved);
        reference.flags = HandoffRef::FLAG_ROLLBACK_SNAPSHOT;
        assert_eq!(HandoffRef::decode(&reference.encode()).unwrap(), reference);
    }

    /// §10's observation fields at their exact widths and offsets.
    ///
    /// The terminal-result commit sequence at 184 is a `u64` and the observed outcome generation at
    /// 192 is a `u32`, and they are adjacent — so a swapped `put_u64`/`put_u32` writes four bytes of
    /// one into the other's slot and still round-trips through a decoder that reads the same way.
    /// Pinning the *bytes* is what catches it: every field is checked at its own offset, with values
    /// whose bytes are all distinct.
    #[test]
    fn the_observation_fields_land_at_their_own_offsets_and_widths() {
        let mut reference = handoff_ref(4, HandoffPhase::OutcomeObserved);
        reference.outcome = ObcuOutcome::Installed;
        reference.terminal_commit = 0x0102_0304_0506_0708;
        reference.outcome_generation = 0x090A_0B0C;
        let bytes = reference.encode();

        assert_eq!(&bytes[184..192], &0x0102_0304_0506_0708u64.to_le_bytes(), "terminal commit at 184 is a u64");
        assert_eq!(&bytes[192..196], &0x090A_0B0Cu32.to_le_bytes(), "outcome generation at 192 is a u32");
        // 196..200 is reserved, and a u64 written at 192 would have spilled into it.
        assert_eq!(&bytes[196..200], &[0, 0, 0, 0], "the reserved run after the outcome generation");
        assert_eq!(bytes[9], ObcuOutcome::Installed as u8, "the OBCU outcome at byte 9");
        assert_eq!(HandoffRef::decode(&bytes).unwrap(), reference);
    }

    /// Every registered outcome decodes at every phase that can carry it, and an unregistered one
    /// does not.
    #[test]
    fn every_registered_obcu_outcome_round_trips() {
        for (outcome, value) in [
            (ObcuOutcome::None, 0u8),
            (ObcuOutcome::Installed, 1),
            (ObcuOutcome::RolledBack, 2),
            (ObcuOutcome::StageRejected, 3),
            (ObcuOutcome::ArmAbandoned, 4),
        ] {
            let mut reference = handoff_ref(4, HandoffPhase::Complete);
            reference.outcome = outcome;
            let bytes = reference.encode();
            assert_eq!(bytes[9], value);
            assert_eq!(HandoffRef::decode(&bytes).unwrap().outcome, outcome);
        }
        let mut bytes = handoff_ref(4, HandoffPhase::Complete).encode();
        bytes[9] = 5;
        assert_eq!(HandoffRef::decode(&bytes).unwrap_err().reason, Reason::UnknownEnum);
    }

    /// Every phase round-trips, and the gate binds to it: §10 makes the phase the gate's logical
    /// sequence, so a record at one phase cannot be read as another.
    #[test]
    fn every_phase_round_trips_through_its_own_gate() {
        for phase in [
            HandoffPhase::Prepared,
            HandoffPhase::Armed,
            HandoffPhase::TrialObserved,
            HandoffPhase::OutcomeObserved,
            HandoffPhase::Complete,
        ] {
            let record = record(4, phase);
            let slot = record.encode_slot(0);
            assert_eq!(HandoffRecord::validate_slot(&slot, 0).unwrap(), record);
            assert_eq!(HandoffRecord::validate_slot(&slot, 0).unwrap().handoff.phase, phase);
        }
    }

    #[test]
    fn selection_orders_by_sequence_then_phase() {
        assert!(handoff_ref(4, HandoffPhase::Prepared).selector() < handoff_ref(4, HandoffPhase::Armed).selector());
        assert!(handoff_ref(4, HandoffPhase::Complete).selector() < handoff_ref(5, HandoffPhase::Prepared).selector());
    }
}
