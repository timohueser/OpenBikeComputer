//! Deterministic sample records shared by the kernel's tests, the crash harness and the fixture
//! producer.
//!
//! One place for them so a record is described once: the codec tests, the reference-model tests,
//! the crash matrix and the checked-in vectors all exercise the *same* bytes, which is what makes a
//! vector regeneration diff readable. Every identity here is a constant chosen once and derived
//! from nothing.

use obc_link::ids::{DraftPartRef, GenerationId, LogicalObjectId, OperationId, Revision, StoreId, WeatherRequestId};

use super::entries::{
    ActiveOperation, ActiveRide, CatalogHead, DraftParent, DraftParentState, DraftPart, DraftPartState, HeadKey,
    OperationPhase, PartKey, RepositoryState, ResultType, RetainedPrevious, RideState, TerminalResult, WeatherState,
};
use super::handoff::{HandoffPhase, HandoffRecord, HandoffRef, ObcuOutcome};
use super::journal::{Change, JournalBody, Mutation, RecordKind, RepositoryChange};
use super::work::{RideEvidence, RideRecord, Subject, WorkRecord, WorkState};

/// The suite's StoreId.
pub const STORE: StoreId =
    StoreId::new([0x3c, 0x92, 0x00, 0x00, 0x99, 0x16, 0x4e, 0xba, 0xab, 0xc2, 0x34, 0x2f, 0xe0, 0x8f, 0x6b, 0x10]);
/// The primary OperationId.
pub const OP_A: [u8; 16] = [0xa1; 16];
/// A second OperationId.
pub const OP_B: [u8; 16] = [0xb2; 16];
/// A draft parent OperationId.
pub const OP_PARENT: [u8; 16] = [0xc3; 16];
/// A draft child OperationId.
pub const OP_CHILD: [u8; 16] = [0xd4; 16];
/// An `InstallUpdate` OperationId.
pub const OP_INSTALL: [u8; 16] = [0xe5; 16];
/// The local ride-publication OperationId.
pub const OP_RIDE: [u8; 16] = [0xc1; 16];
/// A sealed part's opaque reference.
pub const PART_REF: [u8; 16] = [0x5a; 16];
/// The canonical-intent digest the samples carry.
pub const INTENT: [u8; 32] = [0x11; 32];
/// The principal-scope digest the samples carry.
pub const PRINCIPAL: [u8; 32] = [0x22; 32];

/// One repository row.
pub fn repository(kind: u16, revision: u64) -> RepositoryState {
    RepositoryState { kind, flags: 0, revision: Revision::new(revision), next_logical_id: LogicalObjectId::new(2) }
}

/// One published head with an eight-byte envelope.
pub fn head(kind: u16, id: u64) -> CatalogHead {
    let mut envelope = [0u8; 96];
    envelope[..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    CatalogHead {
        key: HeadKey { kind, id: LogicalObjectId::new(id) },
        flags: 0,
        revision: Revision::new(9),
        generation: GenerationId::new(42),
        length: 3_000,
        crc: 0x1234_5678,
        envelope_len: 8,
        envelope,
        resolution: GenerationId::ZERO,
    }
}

/// A published volume manifest, carrying its resolution generation.
pub fn manifest_head(id: u64, resolution: u64) -> CatalogHead {
    let mut head = head(6, id);
    head.flags = CatalogHead::FLAG_RESOLUTION_PRESENT;
    head.resolution = GenerationId::new(resolution);
    head
}

/// One streaming claim row.
pub fn active(operation: [u8; 16]) -> ActiveOperation {
    ActiveOperation {
        operation: OperationId::new(operation),
        intent: INTENT,
        principal: PRINCIPAL,
        opcode: 0x0100,
        subject_kind: 1,
        phase: OperationPhase::Streaming,
        flags: ActiveOperation::FLAG_GENERATION_RESERVED,
        logical_id: 7,
        expected_revision: 0,
        generation: GenerationId::new(42),
        progress_counter: 3,
        work_sequence: 0,
        abort_reason: 0,
    }
}

/// The one draft parent.
pub fn parent() -> DraftParent {
    DraftParent {
        parent: OperationId::new(OP_PARENT),
        intent: [0x44; 32],
        manifest_generation: GenerationId::new(90),
        manifest_kind: 6,
        declared_parts: 2,
        state: DraftParentState::Open,
        replace: false,
        target_id: LogicalObjectId::ZERO,
        expected_revision: Revision::ZERO,
        manifest_length: 776,
        manifest_crc: 0x0f0f_0f0f,
        draft_revision: 1,
        progress_counter: 0,
        work_sequence: 0,
        resolution: GenerationId::ZERO,
    }
}

/// One sealed draft part.
pub fn part(key: u64) -> DraftPart {
    DraftPart {
        key: PartKey { parent: OperationId::new(OP_PARENT), kind: 1, key },
        child: OperationId::new(OP_CHILD),
        part_ref: DraftPartRef::new(PART_REF),
        generation: GenerationId::new(91),
        length: 1_024,
        crc: 0x1111_2222,
        state: DraftPartState::Sealed,
    }
}

/// One retained generation, held by a live lease.
pub fn retained(generation: u64) -> RetainedPrevious {
    RetainedPrevious {
        reasons: RetainedPrevious::REASON_LIVE_LEASE,
        lease_count: 2,
        kind: 1,
        logical_id: LogicalObjectId::new(7),
        generation: GenerationId::new(generation),
        length: 3_000,
        crc: 0xaabb_ccdd,
        retain_through: 0,
        object_revision: Revision::new(8),
    }
}

/// One committed `ObjectResult`.
pub fn result(commit_sequence: u64, operation: [u8; 16]) -> TerminalResult {
    let mut body = [0u8; 88];
    body[..64].copy_from_slice(&[0x5a; 64]);
    TerminalResult {
        commit_sequence,
        operation: OperationId::new(operation),
        intent: INTENT,
        principal: PRINCIPAL,
        committed: true,
        result_type: ResultType::Object,
        body,
    }
}

/// The satisfied weather-request state.
pub fn weather() -> WeatherState {
    WeatherState {
        satisfied: true,
        flags: WeatherState::FLAG_HEAD_PRESENT,
        request: WeatherRequestId::new(5),
        context_revision: 3,
        logical_id: LogicalObjectId::ZERO,
        captured_revision: Revision::new(11),
        latitude_e7: 480_000_000,
        longitude_e7: -1_200_000_000,
        radius_m: 40_000,
        earliest_issued: 1_700_000_000,
        valid_until: 1_700_086_400,
        head_request: WeatherRequestId::new(5),
    }
}

/// The recording active-ride state.
pub fn ride() -> ActiveRide {
    ActiveRide {
        state: RideState::Recording,
        flags: ActiveRide::FLAG_ROUTE_SNAPSHOT,
        recovery_revision: 12,
        operation: OperationId::new(OP_RIDE),
        principal: [0x33; 32],
        generation: GenerationId::new(77),
        start_utc: 1_700_000_000,
        route_id: LogicalObjectId::new(4),
        route_revision: Revision::new(2),
    }
}

/// One handoff reference at `sequence` and `phase`.
pub fn handoff_ref(sequence: u64, phase: HandoffPhase) -> HandoffRef {
    HandoffRef {
        sequence,
        phase,
        outcome: ObcuOutcome::None,
        flags: 0,
        operation: OperationId::new(OP_INSTALL),
        intent: [0x55; 32],
        package_generation: GenerationId::new(31),
        package_length: 262_144,
        package_crc: 0x9999_8888,
        arm_generation: 7,
        armed_blob_sha256: [0x66; 32],
        image_header: [0x77; 64],
        terminal_commit: 0,
        outcome_generation: 0,
        rollback_generation: GenerationId::ZERO,
        rollback_length: 0,
        rollback_crc: 0,
        logical_id: LogicalObjectId::new(3),
        revision: Revision::new(5),
    }
}

/// One ARM record.
pub fn handoff_record(sequence: u64, phase: HandoffPhase) -> HandoffRecord {
    HandoffRecord { store: STORE, handoff: handoff_ref(sequence, phase) }
}

/// One `WORK` slot body.
pub fn work(sequence: u32, offset: u64, state: WorkState) -> WorkRecord {
    WorkRecord {
        store: STORE,
        operation: OperationId::new(OP_A),
        intent: INTENT,
        parent: OperationId::ZERO,
        part_ref: DraftPartRef::ZERO,
        generation: GenerationId::new(42),
        declared_length: 65_536,
        declared_crc: 0x1234_5678,
        state,
        flags: 0,
        durable_offset: offset,
        prefix_crc: 0x9abc_def0,
        sequence,
        progress_counter: 3,
        subject_kind: 1,
        subject: Subject::LogicalObject,
        part_key: 0,
        observed_length: offset as u32,
    }
}

/// One `RIDE.ACT` slot body.
pub fn ride_slot(sequence: u32, offset: u64) -> RideRecord {
    RideRecord {
        store: STORE,
        recovery_revision: 12,
        operation: OperationId::new(OP_RIDE),
        generation: GenerationId::new(77),
        state: RideEvidence::Recording,
        flags: 0,
        start_utc: 1_700_000_000,
        route_id: 0,
        route_revision: 0,
        durable_offset: offset,
        prefix_crc: 0x2222_3333,
        sequence,
        sample_count: offset / 16,
        elapsed_ms: 60_000,
        sealed_length: 0,
        sealed_crc: 0,
        observed_length: offset as u32,
    }
}

/// A claim record: the first durable act of an operation.
pub fn claim(epoch: u64, sequence: u64, slot: u16, operation: [u8; 16], generation_cursor: u64) -> JournalBody {
    let mut row = active(operation);
    row.generation = GenerationId::new(generation_cursor - 1);
    JournalBody {
        store: STORE,
        epoch,
        sequence,
        slot,
        kind: RecordKind::Claim,
        operation: OperationId::new(operation),
        intent: INTENT,
        mutation: Mutation {
            active: Some(Change::Put(row)),
            generation_cursor: Some(generation_cursor),
            ..Mutation::default()
        },
    }
}

/// A terminal publication record: head, repository revision, result, and the active-row removal.
pub fn publish(
    epoch: u64,
    sequence: u64,
    slot: u16,
    operation: [u8; 16],
    commit_sequence: u64,
    head_entry: CatalogHead,
) -> JournalBody {
    JournalBody {
        store: STORE,
        epoch,
        sequence,
        slot,
        kind: RecordKind::Terminal,
        operation: OperationId::new(operation),
        intent: INTENT,
        mutation: Mutation {
            active: Some(Change::Remove(OperationId::new(operation))),
            head: Some(Change::Put(head_entry)),
            repository: Some(RepositoryChange {
                kind: head_entry.key.kind,
                revision: Some(head_entry.revision.get()),
                next_logical_id: None,
                flags: 0,
            }),
            result: Some(result(commit_sequence, operation)),
            ..Mutation::default()
        },
    }
}

/// A retention record clearing one entry.
pub fn retention_remove(epoch: u64, sequence: u64, slot: u16, generation: u64) -> JournalBody {
    JournalBody {
        store: STORE,
        epoch,
        sequence,
        slot,
        kind: RecordKind::Retention,
        operation: OperationId::ZERO,
        intent: [0u8; 32],
        mutation: Mutation { retained: Some(Change::Remove(GenerationId::new(generation))), ..Mutation::default() },
    }
}
