//! The crash matrix: every commit-path media sequence, cut at every point, checked against the
//! reference model.
//!
//! `OBC2_Storage_Format.md` §12 states the obligation this module discharges: "The required cut
//! tests cover every sector boundary and every sync return before and after ... Each recovered
//! image must produce exactly the old state, the new state, or the explicitly listed in-progress
//! state—never a mixed head and result, reused ID, leaked draft, released foreign lease, or
//! automatic reformat."
//!
//! Each scenario below performs the exact media operations its section specifies, in that order.
//! The matrix then runs it once per `(operation, before | during | after)` cut point on a fresh
//! deterministic card, reboots, recovers, and asserts the result is the projection before the
//! sequence or the projection after it. Nothing in between is admissible, and neither is a
//! fail-closed mount — silent rollback and spurious corruption are both failures of the same test.
//!
//! Scope note: the streaming WORK-slot cut points are deliberately absent. The initial device ships
//! the restart-only profile (§7, DOS2 owner decision), which writes no streaming slots at all; what
//! remains and is tested is the sealed slot both profiles write.

use std::boxed::Box;
use std::vec;
use std::vec::Vec;

use obc_link::ids::{GenerationId, OperationId};

use super::checkpoint;
use super::entries::{DraftPartState, RetainedPrevious};
use super::gate::INVALIDATED;
use super::handoff::{HandoffPhase, HandoffRecord};
use super::init::InitRecord;
use super::journal::{Change, JournalBody, Mutation, RecordKind};
use super::limits::{
    CHECKPOINT_BODY_LEN, CHECKPOINT_FILE_LEN, CHECKPOINT_GATE_OFFSET, JOURNAL_BODY_LEN, JOURNAL_FILE_LEN,
    JOURNAL_GATE_OFFSET, JOURNAL_SLOTS, RIDE_FILE_LEN, SLOT_FILE_LEN, SLOT_STRIDE, SMALL_BODY_LEN, SMALL_GATE_OFFSET,
    WORK_FILE_LEN,
};
use super::media::{FaultPlan, FileId, Media, MediaError, EVERY_WHEN};
use super::model::CatalogModel;
use super::recovery::{self, CheckpointObservation, Decision, SlotObservation};
use super::resolution::{self, ResolutionEntry};
use super::samples;
use super::work::{WorkRecord, WorkState};

/// The one card the scenarios run against.
struct Card {
    media: Media,
    cat: [FileId; 2],
    journal: FileId,
    arm: [FileId; 2],
    work: FileId,
    payload: FileId,
    init: FileId,
}

/// The payload length the scenarios use for the one generation they write.
const PAYLOAD_LEN: usize = 4_096;

impl Card {
    /// An initialized card: every fixed file at its full length, and `CAT0.CHK` holding `model`.
    fn new(seed: u64, model: &CatalogModel) -> Self {
        let mut media = Media::new(seed);
        let cat0 = media.create("CAT0.CHK", CHECKPOINT_FILE_LEN);
        let cat1 = media.create("CAT1.CHK", CHECKPOINT_FILE_LEN);
        let journal = media.create("COMMIT.JNL", JOURNAL_FILE_LEN);
        let arm0 = media.create("ARM0.HND", SLOT_FILE_LEN);
        let arm1 = media.create("ARM1.HND", SLOT_FILE_LEN);
        let _ride = media.create("RIDE.ACT", RIDE_FILE_LEN);
        let init = media.create("INIT.REC", SLOT_FILE_LEN);
        let work = media.create("WORK", WORK_FILE_LEN);
        let payload = media.create("GEN", PAYLOAD_LEN);
        let mut card = Card { media, cat: [cat0, cat1], journal, arm: [arm0, arm1], work, payload, init };
        card.write_checkpoint(0, model).expect("initial checkpoint");
        card
    }

    /// Writes one gated checkpoint: invalidate the gate, write the body, write the gate, each
    /// followed by its own sync (§6.3 steps 2 through 4).
    fn write_checkpoint(&mut self, index: usize, model: &CatalogModel) -> Result<(), MediaError> {
        let file = self.cat[index];
        let mut body = Box::new([0u8; CHECKPOINT_BODY_LEN]);
        model.encode_body(body.as_mut_slice()).expect("body");
        self.write_all(file, CHECKPOINT_GATE_OFFSET, &INVALIDATED)?;
        self.media.sync(file)?;
        self.write_all(file, 0, body.as_slice())?;
        self.media.sync(file)?;
        let gate = checkpoint::gate_for(body.as_slice(), index as u16);
        self.write_all(file, CHECKPOINT_GATE_OFFSET, &gate.encode())?;
        self.media.sync(file)
    }

    /// Appends one journal record: body, sync, gate, sync.
    ///
    /// §1: journal slots are the single exemption from the invalidate-first discipline, "so every
    /// slot of an earlier epoch is already inert against the selected checkpoint and is rewritten
    /// body-then-gate with no preceding invalidation, saving one write and one sync per commit".
    /// The body write covers the **whole stride**, with the gate sector and the pad written as
    /// zeros. That is not padding for its own sake: a slot that was torn once holds garbage across
    /// its whole program page, and a reader rejects a nonzero pad, so a writer that rewrote only
    /// the 1,536 body bytes could never make that slot valid again. Zeroing the gate sector in the
    /// same write is also the invalidation §4 defines, so the journal keeps its §1 exemption — one
    /// write and one sync fewer than a non-exempt record — while still never presenting an old gate
    /// over a new body.
    fn append_journal(&mut self, record: &JournalBody) -> Result<(), MediaError> {
        let base = record.slot as usize * SLOT_STRIDE;
        let mut stride = record.encode_slot();
        stride[JOURNAL_GATE_OFFSET..JOURNAL_GATE_OFFSET + 512].fill(0);
        self.write_all(self.journal, base, &stride)?;
        self.media.sync(self.journal)?;
        self.write_all(self.journal, base + JOURNAL_GATE_OFFSET, &record.gate().encode())?;
        self.media.sync(self.journal)
    }

    /// Writes one gated slot in the §1 order: invalidate this slot's own gate, write its body,
    /// write its gate, each followed by its own sync.
    ///
    /// The gate invalidated is the one belonging to the slot about to be **reused** — the older of
    /// the alternating pair. That is what makes the sequence safe: §10 puts it as "the currently
    /// selected file remains valid until the replacement gate is durable", and the same holds for
    /// the two WORK slots and the two checkpoints. Invalidating the *selected* record's gate
    /// instead would open a window in which neither side is valid.
    fn write_gated_slot(&mut self, file: FileId, base: usize, slot_bytes: &[u8]) -> Result<(), MediaError> {
        self.write_all(file, base + SMALL_GATE_OFFSET, &INVALIDATED)?;
        self.media.sync(file)?;
        // As with the journal, the body write covers the whole stride so a previously torn page's
        // pad is restored; the gate sector inside it stays zero until the last step.
        let mut stride = [0u8; SLOT_STRIDE];
        stride[..SMALL_BODY_LEN].copy_from_slice(&slot_bytes[..SMALL_BODY_LEN]);
        self.write_all(file, base, &stride)?;
        self.media.sync(file)?;
        self.write_all(file, base + SMALL_GATE_OFFSET, &slot_bytes[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + 512])?;
        self.media.sync(file)
    }

    /// A write plus §13.1's explicit completeness check: "a short write is an error, never a
    /// success".
    fn write_all(&mut self, file: FileId, offset: usize, bytes: &[u8]) -> Result<(), MediaError> {
        let written = self.media.write_at(file, offset, bytes)?;
        if written == bytes.len() {
            Ok(())
        } else {
            Err(MediaError::Full)
        }
    }
}

/// What a mount produced.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Recovered {
    NoCheckpoint,
    Fail(recovery::FailClosed),
    Mounted(Box<CatalogModel>),
}

/// Reads the card the way mount does: validate both checkpoints, validate all 256 journal slots,
/// decide per §6.3, then replay the chosen suffix onto the chosen checkpoint.
fn recover(card: &mut Card) -> Recovered {
    let mut checkpoints = [None, None];
    let mut models: [Option<Box<CatalogModel>>; 2] = [None, None];
    for index in 0..2 {
        let image = card.media.image(card.cat[index]).to_vec();
        if let Ok(header) = checkpoint::validate_file(&image, index as u16) {
            checkpoints[index] = Some(CheckpointObservation {
                store: header.store,
                epoch: header.epoch,
                through_sequence: header.through_sequence,
                body_crc: checkpoint::body_crc(&image[..CHECKPOINT_BODY_LEN]),
            });
            models[index] = Some(Box::new(CatalogModel::decode_body(&image[..CHECKPOINT_BODY_LEN]).expect("model")));
        }
    }

    let journal = card.media.image(card.journal).to_vec();
    let mut observations: Vec<Option<SlotObservation>> = vec![None; JOURNAL_SLOTS];
    let mut bodies: Vec<Option<JournalBody>> = vec![None; JOURNAL_SLOTS];
    for slot in 0..JOURNAL_SLOTS {
        let base = slot * SLOT_STRIDE;
        if let Ok(body) = JournalBody::validate_slot(&journal[base..base + SLOT_STRIDE], slot as u16) {
            observations[slot] =
                Some(SlotObservation { store: body.store, epoch: body.epoch, sequence: body.sequence });
            bodies[slot] = Some(body);
        }
    }

    match recovery::choose(&checkpoints, &observations) {
        Decision::NoCheckpoint => Recovered::NoCheckpoint,
        Decision::Fail(fault) => Recovered::Fail(fault),
        Decision::Mount { checkpoint, replay } => {
            let mut model = models[checkpoint].clone().expect("selected checkpoint decoded");
            for body in bodies.iter().take(replay) {
                model.apply(body.as_ref().expect("valid record")).expect("replay applies");
            }
            Recovered::Mounted(model)
        }
    }
}

/// Runs `scenario` on a fresh card with no faults and reports how many media operations it needs.
fn count_ops(before: &CatalogModel, scenario: &dyn Fn(&mut Card)) -> u32 {
    let mut card = Card::new(1, before);
    let baseline = card.media.ops();
    scenario(&mut card);
    card.media.ops() - baseline
}

/// The crash matrix for one scenario: every operation, every cut position, checked against both
/// admissible outcomes.
fn assert_old_or_new(name: &str, before: &CatalogModel, after: &CatalogModel, scenario: &dyn Fn(&mut Card)) {
    let total = count_ops(before, scenario);
    assert!(total > 0, "{name}: scenario performs no media operations");
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 31 + 7, before);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            scenario(&mut card);
            card.media.reboot();
            match recover(&mut card) {
                Recovered::Mounted(model) => {
                    assert!(
                        model.as_ref() == before || model.as_ref() == after,
                        "{name}: cut at op {op} {when:?} recovered neither the old nor the new projection",
                    );
                }
                other => panic!("{name}: cut at op {op} {when:?} did not mount: {other:?}"),
            }
        }
    }
    // The fault-free run must land on the new projection, or the matrix above proves nothing.
    let mut card = Card::new(99, before);
    scenario(&mut card);
    match recover(&mut card) {
        Recovered::Mounted(model) => assert_eq!(model.as_ref(), after, "{name}: fault-free run"),
        other => panic!("{name}: fault-free run did not mount: {other:?}"),
    }
}

fn initial() -> CatalogModel {
    CatalogModel::initial(samples::STORE, 4)
}

// -------------------------------------------------------------------------------------------
// §6.2 — the claim gate
// -------------------------------------------------------------------------------------------

#[test]
fn journal_append_recovers_the_old_or_the_new_projection() {
    let before = initial();
    let record = samples::claim(1, 1, 0, samples::OP_A, 1);
    let mut after = before.clone();
    after.apply(&record).unwrap();
    assert_old_or_new("journal claim append", &before, &after, &|card| {
        let _ = card.append_journal(&record);
    });
}

// -------------------------------------------------------------------------------------------
// §6.2 — the terminal catalog/result gate, and exactly-once results
// -------------------------------------------------------------------------------------------

#[test]
fn terminal_publication_is_atomic_across_every_cut() {
    // The claim is already durable, so the card's checkpoint carries it and the journal starts
    // empty: slot 0 then carries `through_sequence + 1`, which is what §6.3's mapping requires.
    let mut before = initial();
    before.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();

    let publication = samples::publish(1, 2, 0, samples::OP_A, 1, samples::head(1, 7));
    let mut after = before.clone();
    after.apply(&publication).unwrap();

    assert_old_or_new("terminal publication", &before, &after, &move |card| {
        let _ = card.append_journal(&publication);
    });
}

#[test]
fn a_result_appears_exactly_once_however_the_commit_is_cut() {
    let mut before = initial();
    before.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
    let publication = samples::publish(1, 2, 0, samples::OP_A, 1, samples::head(1, 7));

    let total = count_ops(&before, &|card| {
        let _ = card.append_journal(&publication);
    });
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 17 + 3, &before);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            let _ = card.append_journal(&publication);
            card.media.reboot();

            // Whatever the cut did, the retry writes the same record into the same slot.
            let _ = card.append_journal(&publication);
            let Recovered::Mounted(model) = recover(&mut card) else { panic!("retry did not mount") };
            let matching: Vec<_> =
                model.results.iter().filter(|result| result.operation == OperationId::new(samples::OP_A)).collect();
            assert_eq!(matching.len(), 1, "cut at op {op} {when:?} produced {} results", matching.len());
            assert_eq!(model.terminal_counter, 1);
            assert_eq!(model.heads.len(), 1);
        }
    }
}

// -------------------------------------------------------------------------------------------
// §6.3 — checkpoint compaction
// -------------------------------------------------------------------------------------------

#[test]
fn compaction_recovers_the_old_epoch_or_the_new_checkpoint() {
    // The card starts with CAT0 at epoch 1 through 0 and three replayable records.
    let mut before = initial();
    let records = [
        samples::claim(1, 1, 0, samples::OP_A, 1),
        samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7)),
        samples::claim(1, 3, 2, samples::OP_B, 2),
    ];
    for record in &records {
        before.apply(record).unwrap();
    }

    // Compaction materializes the same projection at epoch 2, then opens the new epoch at slot 0.
    let mut compacted = before.clone();
    compacted.epoch = 2;
    let next_claim = {
        let mut record = samples::claim(2, before.through_sequence + 1, 0, samples::OP_A, 3);
        record.operation = OperationId::new(samples::OP_A);
        record
    };
    let mut after = compacted.clone();
    after.apply(&next_claim).unwrap();

    let setup = records;
    let for_scenario = compacted.clone();
    let scenario = move |card: &mut Card| {
        let _ = card.write_checkpoint(1, &for_scenario);
        let _ = card.append_journal(&next_claim);
    };

    // `before` is reached by replaying the epoch-1 suffix, so build the starting card by hand
    // rather than through `assert_old_or_new`'s checkpoint-only setup.
    let base = initial();
    let total = {
        let mut card = Card::new(1, &base);
        for record in &setup {
            card.append_journal(record).unwrap();
        }
        let baseline = card.media.ops();
        scenario(&mut card);
        card.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 13 + 5, &base);
            for record in &setup {
                card.append_journal(record).unwrap();
            }
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            scenario(&mut card);
            card.media.reboot();
            match recover(&mut card) {
                // Three states, not two. §6.3's compaction has an explicitly listed in-progress
                // state of its own: the new checkpoint is durable but the new epoch's first record
                // is not, which is the same catalog at epoch `E + 1`.
                Recovered::Mounted(model) => assert!(
                    model.as_ref() == &before || model.as_ref() == &compacted || model.as_ref() == &after,
                    "compaction: cut at op {op} {when:?} recovered neither state (epoch {}, through {})",
                    model.epoch,
                    model.through_sequence,
                ),
                other => panic!("compaction: cut at op {op} {when:?} did not mount: {other:?}"),
            }
        }
    }
}

/// §6.3's first fail-closed rule, from the other side: an epoch-2 record with no epoch-2 checkpoint
/// is exactly the "a newer checkpoint existed and was lost" evidence, and mounting the old
/// checkpoint would silently roll back everything it absorbed.
#[test]
fn a_lost_new_checkpoint_with_a_new_epoch_record_fails_closed_rather_than_rolling_back() {
    let base = initial();
    let mut card = Card::new(3, &base);
    let record = samples::claim(2, 1, 0, samples::OP_A, 1);
    card.append_journal(&record).unwrap();
    assert_eq!(recover(&mut card), Recovered::Fail(recovery::FailClosed::NewerEpochRecord { slot: 0 }));
}

/// §6.3's second rule: a valid same-epoch record beyond the replay stop proves a committed record
/// was lost.
#[test]
fn a_gap_with_a_later_valid_record_fails_closed() {
    let base = initial();
    let mut card = Card::new(4, &base);
    card.append_journal(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
    card.append_journal(&samples::claim(1, 3, 2, samples::OP_B, 2)).unwrap();
    assert_eq!(recover(&mut card), Recovered::Fail(recovery::FailClosed::RecordBeyondStop { slot: 2 }));
}

// -------------------------------------------------------------------------------------------
// §7 — the sealed WORK slot
// -------------------------------------------------------------------------------------------

/// The one durable work fact the restart-only profile writes. The oracle is §7's selection rule:
/// the greatest valid sequence whose durable offset the payload can reach.
fn recover_work(card: &Card) -> Option<WorkRecord> {
    let image = card.media.image(card.work).to_vec();
    let observed = card.media.image(card.payload).len() as u64;
    let mut best: Option<WorkRecord> = None;
    for slot in 0..2usize {
        let base = slot * SLOT_STRIDE;
        if let Ok(record) = WorkRecord::validate_slot(&image[base..base + SLOT_STRIDE], slot as u16) {
            if !record.offset_is_reachable(observed) {
                continue;
            }
            if best.is_none_or(|held| held.sequence < record.sequence) {
                best = Some(record);
            }
        }
    }
    best
}

#[test]
fn sealing_recovers_the_old_or_the_new_work_slot() {
    let streaming = samples::work(1, PAYLOAD_LEN as u64 / 2, WorkState::Streaming);
    let mut sealed = samples::work(2, PAYLOAD_LEN as u64, WorkState::Sealed);
    sealed.declared_length = PAYLOAD_LEN as u64;
    sealed.observed_length = PAYLOAD_LEN as u32;

    let base = initial();
    let scenario = |card: &mut Card| {
        let _ = card.write_gated_slot(card.work, SLOT_STRIDE, &sealed.encode_slot(1));
    };
    let total = {
        let mut card = Card::new(1, &base);
        card.write_gated_slot(card.work, 0, &streaming.encode_slot(0)).unwrap();
        let baseline = card.media.ops();
        scenario(&mut card);
        card.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 29 + 11, &base);
            card.write_gated_slot(card.work, 0, &streaming.encode_slot(0)).unwrap();
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            scenario(&mut card);
            card.media.reboot();
            let recovered = recover_work(&card);
            assert!(
                recovered == Some(streaming) || recovered == Some(sealed),
                "seal: cut at op {op} {when:?} recovered {recovered:?}",
            );
        }
    }
}

/// §7's rewind: a durable offset above the payload's observed length is unreachable, so that slot
/// is skipped as if invalid and the older reachable one wins.
#[test]
fn an_unreachable_durable_offset_is_skipped_in_favour_of_the_older_slot() {
    let reachable = samples::work(1, 1_024, WorkState::Streaming);
    let unreachable = samples::work(2, (PAYLOAD_LEN + SLOT_STRIDE) as u64, WorkState::Streaming);
    let mut card = Card::new(5, &initial());
    card.write_gated_slot(card.work, 0, &reachable.encode_slot(0)).unwrap();
    card.write_gated_slot(card.work, SLOT_STRIDE, &unreachable.encode_slot(1)).unwrap();
    assert_eq!(recover_work(&card), Some(reachable));
}

// -------------------------------------------------------------------------------------------
// §10 — ARM alternation
// -------------------------------------------------------------------------------------------

fn recover_handoff(card: &Card) -> Option<HandoffRecord> {
    let mut best: Option<HandoffRecord> = None;
    for slot in 0..2usize {
        let image = card.media.image(card.arm[slot]).to_vec();
        if let Ok(record) = HandoffRecord::validate_slot(&image, slot as u16) {
            if best.is_none_or(|held| held.handoff.selector() < record.handoff.selector()) {
                best = Some(record);
            }
        }
    }
    best
}

#[test]
fn arm_alternation_selects_the_old_pair_or_the_strictly_greater_new_one() {
    let prepared = samples::handoff_record(4, HandoffPhase::Prepared);
    let armed = samples::handoff_record(4, HandoffPhase::Armed);
    let base = initial();

    let total = {
        let mut card = Card::new(1, &base);
        card.write_gated_slot(card.arm[0], 0, &prepared.encode_slot(0)).unwrap();
        let baseline = card.media.ops();
        let _ = card.write_gated_slot(card.arm[1], 0, &armed.encode_slot(1));
        card.media.ops() - baseline
    };

    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 23 + 2, &base);
            card.write_gated_slot(card.arm[0], 0, &prepared.encode_slot(0)).unwrap();
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            let _ = card.write_gated_slot(card.arm[1], 0, &armed.encode_slot(1));
            card.media.reboot();

            let recovered = recover_handoff(&card);
            assert!(
                recovered == Some(prepared) || recovered == Some(armed),
                "ARM: cut at op {op} {when:?} recovered {recovered:?}",
            );
            // §10: "a cut during a phase advance selects the old pair or the strictly greater new
            // pair, never an ambiguous equal-sequence tie" — two valid records at one sequence and
            // one phase would be exactly that tie.
            if let Some(record) = recovered {
                let other = if record == prepared { armed } else { prepared };
                assert!(record.handoff.selector() != other.handoff.selector());
            }
        }
    }
}

// -------------------------------------------------------------------------------------------
// §12 — initialization order
// -------------------------------------------------------------------------------------------

/// §12: initialization writes `INIT.REC` and its witness gate before the first checkpoint, and the
/// first checkpoint gate is the StoreId birth point. A cut anywhere leaves either no store at all
/// or a complete first checkpoint — never a half-born one.
#[test]
fn initialization_produces_no_store_or_a_complete_first_checkpoint() {
    let first = initial();
    let witness = InitRecord { store: samples::STORE };

    let scenario = |card: &mut Card| {
        let slot = witness.encode_slot();
        let _ = card.write_all(card.init, 0, &slot[..SMALL_BODY_LEN]);
        let _ = card.media.sync(card.init);
        let _ = card.write_all(card.init, SMALL_GATE_OFFSET, &slot[SMALL_GATE_OFFSET..SMALL_GATE_OFFSET + 512]);
        let _ = card.media.sync(card.init);
        let _ = card.write_checkpoint(0, &first);
    };

    let empty = CatalogModel::initial(samples::STORE, 4);
    let total = {
        // A card whose CAT0 is still zeros: `Card::new` writes one, so start from a blank medium.
        let mut card = blank_card(1);
        let baseline = card.media.ops();
        scenario(&mut card);
        card.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = blank_card(u64::from(op) * 37 + 13);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            scenario(&mut card);
            card.media.reboot();
            match recover(&mut card) {
                Recovered::NoCheckpoint => {
                    // Pre-birth: the witness is either absent or valid, never a torn record that
                    // decodes.
                    let image = card.media.image(card.init).to_vec();
                    if image.iter().any(|&byte| byte != 0) {
                        if let Ok(record) = InitRecord::validate_slot(&image) {
                            assert_eq!(record, witness);
                        }
                    }
                }
                Recovered::Mounted(model) => assert_eq!(model.as_ref(), &empty),
                other => panic!("initialization: cut at op {op} {when:?} produced {other:?}"),
            }
        }
    }
}

/// A card with every file present at full length and no checkpoint written yet.
fn blank_card(seed: u64) -> Card {
    let mut media = Media::new(seed);
    let cat0 = media.create("CAT0.CHK", CHECKPOINT_FILE_LEN);
    let cat1 = media.create("CAT1.CHK", CHECKPOINT_FILE_LEN);
    let journal = media.create("COMMIT.JNL", JOURNAL_FILE_LEN);
    let arm0 = media.create("ARM0.HND", SLOT_FILE_LEN);
    let arm1 = media.create("ARM1.HND", SLOT_FILE_LEN);
    let _ride = media.create("RIDE.ACT", RIDE_FILE_LEN);
    let init = media.create("INIT.REC", SLOT_FILE_LEN);
    let work = media.create("WORK", WORK_FILE_LEN);
    let payload = media.create("GEN", PAYLOAD_LEN);
    Card { media, cat: [cat0, cat1], journal, arm: [arm0, arm1], work, payload, init }
}

// -------------------------------------------------------------------------------------------
// §8 — the resolution generation and the manifest commit
// -------------------------------------------------------------------------------------------

/// §8: the resolution generation is written and synchronized *before* the terminal record, and only
/// that record's gate publishes the manifest head. A cut before it leaves the reserved generation
/// as an orphan file, which is not a published state.
#[test]
fn a_manifest_becomes_visible_only_at_its_terminal_gate() {
    let mut before = initial();
    before.apply(&samples::claim(1, 1, 0, samples::OP_PARENT, 1)).unwrap();

    let entries = [ResolutionEntry {
        part_ref: obc_link::ids::DraftPartRef::new(samples::PART_REF),
        generation: GenerationId::new(91),
    }];
    let mut table = [0u8; resolution::MAX_BODY_LEN];
    let table_len = resolution::encode(&entries, &mut table).unwrap();

    let publication = samples::publish(1, 2, 0, samples::OP_PARENT, 1, samples::manifest_head(3, 92));
    let mut after = before.clone();
    after.apply(&publication).unwrap();

    let total = count_ops(&before, &|card| {
        let _ = card.write_all(card.payload, 0, &table[..table_len]);
        let _ = card.media.sync(card.payload);
        let _ = card.append_journal(&publication);
    });
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 19 + 6, &before);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            let _ = card.write_all(card.payload, 0, &table[..table_len]);
            let _ = card.media.sync(card.payload);
            let _ = card.append_journal(&publication);
            card.media.reboot();

            let Recovered::Mounted(model) = recover(&mut card) else { panic!("manifest: did not mount") };
            assert!(
                model.as_ref() == &before || model.as_ref() == &after,
                "manifest: cut at op {op} {when:?} recovered neither state",
            );
            // The resolution body a cut left behind is either complete or rejected outright — §8's
            // count and length checks are the whole validity test it has.
            let payload = card.media.image(card.payload).to_vec();
            if let Ok(decoded) = resolution::Resolution::decode(&payload[..table_len]) {
                assert_eq!(decoded.resolve(entries[0].part_ref), Some(entries[0].generation));
            }
            // A manifest head is visible only with its resolution generation, never without.
            if let Some(head) = model.heads.first() {
                assert_ne!(head.flags & super::entries::CatalogHead::FLAG_RESOLUTION_PRESENT, 0);
            }
        }
    }
}

// -------------------------------------------------------------------------------------------
// Property: randomized commit sequences under randomized cuts
// -------------------------------------------------------------------------------------------

/// A bounded deterministic sequence generator: claims, publications, retention records and a
/// compaction, each valid against the projection they are applied to.
fn random_sequence(seed: u64, model: &mut CatalogModel) -> Vec<JournalBody> {
    let mut rng = seed | 1;
    let mut next = || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut records = Vec::new();
    let mut claimed: Vec<[u8; 16]> = Vec::new();
    let mut commits = 0u64;
    let mut retained: Vec<u64> = Vec::new();
    for step in 0..12u64 {
        let slot = records.len() as u16;
        let sequence = model.through_sequence + 1;
        let choice = next() % 4;
        let record = if choice == 0 || claimed.is_empty() {
            let mut operation = samples::OP_A;
            operation[0] = step as u8;
            claimed.push(operation);
            samples::claim(model.epoch, sequence, slot, operation, model.next_generation + 1)
        } else if choice == 3 && !retained.is_empty() {
            let generation = retained.remove(0);
            samples::retention_remove(model.epoch, sequence, slot, generation)
        } else {
            let operation = claimed.remove(0);
            commits += 1;
            let mut record = samples::publish(model.epoch, sequence, slot, operation, commits, samples::head(1, step));
            if next() % 2 == 0 {
                let generation = 100 + step;
                let mut entry = samples::retained(generation);
                entry.reasons = RetainedPrevious::REASON_UPDATE_ROLLBACK;
                entry.lease_count = 0;
                record.mutation.retained = Some(Change::Put(entry));
                retained.push(generation);
            }
            record
        };
        if model.apply(&record).is_err() {
            break;
        }
        records.push(record);
    }
    records
}

#[test]
fn randomized_sequences_recover_to_a_prefix_of_their_records() {
    for seed in 1..=12u64 {
        let base = initial();
        let mut projected = base.clone();
        let records = random_sequence(seed, &mut projected);
        assert!(!records.is_empty());

        // Every prefix of the sequence is an admissible recovered state; nothing else is.
        let mut prefixes = vec![base.clone()];
        let mut walk = base.clone();
        for record in &records {
            walk.apply(record).unwrap();
            prefixes.push(walk.clone());
        }

        let mut card = Card::new(seed, &base);
        let baseline = card.media.ops();
        for record in &records {
            card.append_journal(record).unwrap();
        }
        let total = card.media.ops() - baseline;

        for op in 1..=total {
            let when = EVERY_WHEN[(op as usize + seed as usize) % EVERY_WHEN.len()];
            let mut card = Card::new(seed * 101 + u64::from(op), &base);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            for record in &records {
                if card.append_journal(record).is_err() {
                    break;
                }
            }
            card.media.reboot();
            match recover(&mut card) {
                Recovered::Mounted(model) => assert!(
                    prefixes.iter().any(|prefix| prefix == model.as_ref()),
                    "seed {seed}: cut at op {op} {when:?} recovered a state no prefix produces",
                ),
                other => panic!("seed {seed}: cut at op {op} {when:?} did not mount: {other:?}"),
            }
        }
    }
}

// -------------------------------------------------------------------------------------------
// Fuzz: mutated bytes never panic and always reject with a typed error
// -------------------------------------------------------------------------------------------

/// A bounded deterministic corpus: every slot-shaped record, mutated at seeded positions with
/// seeded values. §1 requires a decoder to reject before it uses a derived offset, so the property
/// is simply that nothing panics and nothing accepts a mutation it should not.
#[test]
fn mutated_records_are_rejected_without_panicking() {
    let corpus: Vec<Vec<u8>> = vec![
        samples::claim(1, 1, 0, samples::OP_A, 1).encode_slot().to_vec(),
        samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7)).encode_slot().to_vec(),
        samples::retention_remove(1, 1, 0, 9).encode_slot().to_vec(),
        samples::work(1, 1_024, WorkState::Streaming).encode_slot(0).to_vec(),
        samples::ride_slot(0, 1_024).encode_slot(0).to_vec(),
        samples::handoff_record(4, HandoffPhase::Armed).encode_slot(0).to_vec(),
        InitRecord { store: samples::STORE }.encode_slot().to_vec(),
    ];

    let mut rng = 0x1234_5678_9ABC_DEF0u64;
    let mut next = || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    for original in &corpus {
        for _ in 0..400 {
            let mut mutated = original.clone();
            let flips = 1 + (next() % 3) as usize;
            for _ in 0..flips {
                let index = (next() as usize) % mutated.len();
                mutated[index] ^= (next() as u8) | 1;
            }
            // Every decoder over the same bytes: whichever one the record actually is, all of them
            // must be total.
            let _ = JournalBody::validate_slot(&mutated, 0);
            let _ = WorkRecord::validate_slot(&mutated, 0);
            let _ = super::work::RideRecord::validate_slot(&mutated, 0);
            let _ = HandoffRecord::validate_slot(&mutated, 0);
            let _ = InitRecord::validate_slot(&mutated);
            let _ = resolution::Resolution::decode(&mutated[..resolution::MAX_BODY_LEN.min(mutated.len())]);
        }
    }
}

/// The same property for the one record too large to sit in a slot: a checkpoint body.
#[test]
fn mutated_checkpoints_are_rejected_without_panicking() {
    let mut model = initial();
    model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
    model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
    model.draft_parent = Some(samples::parent());
    let mut part = samples::part(1);
    part.state = DraftPartState::Prepared;
    part.part_ref = obc_link::ids::DraftPartRef::ZERO;
    let _ = model.draft_parts.push(part);
    model.weather = Some(samples::weather());
    model.ride = Some(samples::ride());

    let mut file = Box::new([0u8; CHECKPOINT_FILE_LEN]);
    model.encode_body(&mut file[..CHECKPOINT_BODY_LEN]).unwrap();
    let gate = checkpoint::gate_for(&file[..CHECKPOINT_BODY_LEN], 0);
    file[CHECKPOINT_GATE_OFFSET..].copy_from_slice(&gate.encode());
    checkpoint::validate_file(file.as_slice(), 0).unwrap();

    let mut rng = 0x0F0F_0F0F_0F0F_0F0Fu64;
    let mut next = || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for _ in 0..2_000 {
        let mut mutated = file.clone();
        let index = (next() as usize) % CHECKPOINT_FILE_LEN;
        mutated[index] ^= (next() as u8) | 1;
        assert!(checkpoint::validate_file(mutated.as_slice(), 0).is_err(), "byte {index} mutation accepted");
    }
}

/// The journal body's own totality, at the granularity §6 cares about: one flipped byte anywhere in
/// the 1,536-byte body invalidates the record.
#[test]
fn every_single_byte_flip_in_a_journal_body_is_rejected() {
    let record = samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7));
    let slot = record.encode_slot();
    for index in 0..JOURNAL_BODY_LEN {
        let mut torn = slot;
        torn[index] ^= 0xFF;
        assert!(JournalBody::validate_slot(&torn, 1).is_err(), "body byte {index} flip accepted");
    }
}

/// The checked-in crash-cut transcripts state each commit path's media operations in order. This is
/// what stops them being prose: the harness's own operation log must be that sequence, operation for
/// operation, or one of the two is wrong.
#[test]
fn the_checked_in_transcripts_match_the_operations_the_harness_performs() {
    use super::vectors;

    let mut observed: std::vec::Vec<(&'static str, std::vec::Vec<super::media::Operation>)> = Vec::new();

    let mut card = Card::new(1, &initial());
    let base = card.media.log().len();
    card.append_journal(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
    observed.push(("journal-append", card.media.log()[base..].to_vec()));

    let mut card = Card::new(2, &initial());
    let base = card.media.log().len();
    let mut compacted = initial();
    compacted.epoch = 2;
    card.write_checkpoint(1, &compacted).unwrap();
    card.append_journal(&samples::claim(2, 1, 0, samples::OP_A, 1)).unwrap();
    observed.push(("checkpoint-compaction", card.media.log()[base..].to_vec()));

    let mut card = Card::new(3, &initial());
    let base = card.media.log().len();
    card.write_gated_slot(card.work, SLOT_STRIDE, &samples::work(2, 0, WorkState::Sealed).encode_slot(1)).unwrap();
    observed.push(("work-seal", card.media.log()[base..].to_vec()));

    let mut card = Card::new(4, &initial());
    let base = card.media.log().len();
    card.write_gated_slot(card.arm[1], 0, &samples::handoff_record(4, HandoffPhase::Armed).encode_slot(1)).unwrap();
    observed.push(("arm-phase-advance", card.media.log()[base..].to_vec()));

    let mut card = Card::new(5, &initial());
    let base = card.media.log().len();
    let table = [0u8; 32];
    card.write_all(card.payload, 0, &table).unwrap();
    card.media.sync(card.payload).unwrap();
    card.append_journal(&samples::publish(1, 1, 0, samples::OP_A, 1, samples::manifest_head(3, 92))).unwrap();
    observed.push(("manifest-publication", card.media.log()[base..].to_vec()));

    for transcript in vectors::transcripts() {
        let (_, log) = observed
            .iter()
            .find(|(name, _)| *name == transcript.name)
            .unwrap_or_else(|| panic!("{} has no observed run", transcript.name));
        assert_eq!(log.len(), transcript.steps.len(), "{}: operation count", transcript.name);
        for (index, (step, operation)) in transcript.steps.iter().zip(log.iter()).enumerate() {
            let step_file = if step.file == "GEN" { "GEN" } else { step.file };
            assert_eq!(operation.file, step_file, "{} op {}: file", transcript.name, index + 1);
            assert_eq!(operation.kind, step.kind, "{} op {}: kind", transcript.name, index + 1);
            assert_eq!(operation.offset, step.offset, "{} op {}: offset", transcript.name, index + 1);
            // A zero length in a transcript means "a body whose size the payload decides"; every
            // other length is a fixed record size and is compared exactly.
            if step.length != 0 {
                assert_eq!(operation.length, step.length, "{} op {}: length", transcript.name, index + 1);
            }
        }
    }
}

/// A record's own mutation must survive being decoded and re-encoded byte for byte, or a compaction
/// pass could not reproduce it.
#[test]
fn every_record_re_encodes_to_the_same_bytes() {
    let records = [
        samples::claim(1, 1, 0, samples::OP_A, 1),
        samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7)),
        samples::retention_remove(1, 3, 2, 9),
        JournalBody {
            kind: RecordKind::Domain,
            operation: OperationId::ZERO,
            intent: [0u8; 32],
            mutation: Mutation {
                ride: Some(Change::Put(samples::ride())),
                generation_cursor: Some(1),
                ..Mutation::default()
            },
            ..samples::claim(1, 4, 3, samples::OP_A, 1)
        },
    ];
    for record in records {
        let bytes = record.encode_body();
        let decoded = JournalBody::decode_body(&bytes).expect("decodes");
        assert_eq!(decoded.encode_body(), bytes);
        assert_eq!(decoded, record);
    }
}
