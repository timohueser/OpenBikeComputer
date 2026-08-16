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
//!
//! One shape of the compaction scenario is provisional rather than normative, and the checked-in
//! transcript says so: it writes the checkpoint body in one call, while §6.3 specifies a bounded
//! forward pass of many smaller writes. The *ordering* around that write is normative and is what
//! these cut points prove; the streaming pass arrives with the compaction engine.

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
    JOURNAL_GATE_OFFSET, JOURNAL_SLOTS, RIDE_FILE_LEN, RIDE_SLOTS, SLOT_FILE_LEN, SLOT_STRIDE, SMALL_BODY_LEN,
    SMALL_GATE_OFFSET, WORK_FILE_LEN,
};
use super::media::{FaultPlan, FileId, Media, MediaError, When, EVERY_WHEN};
use super::model::CatalogModel;
use super::recovery::{self, CheckpointObservation, Decision, SlotObservation};
use super::resolution::{self, ResolutionEntry};
use super::samples;
use super::work::{self, RideRecord, RideRecovery, WorkRecord, WorkRecovery, WorkState};

/// The one card the scenarios run against.
struct Card {
    media: Media,
    cat: [FileId; 2],
    journal: FileId,
    arm: [FileId; 2],
    ride: FileId,
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
        let ride = media.create("RIDE.ACT", RIDE_FILE_LEN);
        let init = media.create("INIT.REC", SLOT_FILE_LEN);
        let work = media.create("WORK", WORK_FILE_LEN);
        let payload = media.create("GEN", PAYLOAD_LEN);
        let mut card = Card { media, cat: [cat0, cat1], journal, arm: [arm0, arm1], ride, work, payload, init };
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
///
/// Every byte comes through [`Media::read_at`], not through the durable image directly, so the read
/// path crosses the medium exactly as the write path does — a corrupt-read injection reaches
/// recovery, and a read that ran past a recorded length would fail here rather than silently work.
/// Reads are counted operations like any other, which is harmless: recovery runs after `reboot`,
/// and reboot clears the fault plan, so nothing recovery reads can shift a scenario's cut points.
fn recover(card: &mut Card) -> Recovered {
    let mut checkpoints = [None, None];
    let mut models: [Option<Box<CatalogModel>>; 2] = [None, None];
    for index in 0..2 {
        let Ok(image) = card.media.read_at(card.cat[index], 0, CHECKPOINT_FILE_LEN) else { continue };
        if let Ok(header) = checkpoint::validate_file(&image, index as u16) {
            checkpoints[index] = Some(CheckpointObservation {
                store: header.store,
                epoch: header.epoch,
                through_sequence: header.through_sequence,
                next_generation: header.next_generation,
                body_crc: checkpoint::body_crc(&image[..CHECKPOINT_BODY_LEN]),
            });
            match CatalogModel::decode_body(&image[..CHECKPOINT_BODY_LEN]) {
                Ok(model) => models[index] = Some(model),
                // `validate_file` already proved the body; a decode that disagrees with it is a
                // codec bug, not a card state, so it must not be swallowed.
                Err(error) => panic!("checkpoint {index} validated but did not decode: {error:?}"),
            }
        }
    }

    let mut observations: Vec<Option<SlotObservation>> = vec![None; JOURNAL_SLOTS];
    let mut bodies: Vec<Option<JournalBody>> = vec![None; JOURNAL_SLOTS];
    for slot in 0..JOURNAL_SLOTS {
        let Ok(stride) = card.media.read_at(card.journal, slot * SLOT_STRIDE, SLOT_STRIDE) else { continue };
        if let Ok(body) = JournalBody::validate_slot(&stride, slot as u16) {
            observations[slot] =
                Some(SlotObservation { store: body.store, epoch: body.epoch, sequence: body.sequence });
            bodies[slot] = Some(body);
        }
    }

    let (checkpoint, replay) = match recovery::choose(&checkpoints, &observations) {
        Decision::NoCheckpoint => return Recovered::NoCheckpoint,
        Decision::Fail(fault) => return Recovered::Fail(fault),
        Decision::Mount { checkpoint, replay } => (checkpoint, replay),
        // §5.2's exhausted-space mount replays exactly the same suffix; only admission differs, and
        // admission is a later slice's.
        Decision::MountReadOnly { checkpoint, replay, .. } => (checkpoint, replay),
    };
    let mut model = models[checkpoint].clone().expect("selected checkpoint decoded");
    for (index, body) in bodies.iter().take(replay).enumerate() {
        let record = body.as_ref().expect("a replayed slot is a valid record");
        // §6.3 chose this suffix precisely because every record in it applies. If one does not, the
        // decision and the model disagree, and that is a finding — never something to skip past.
        assert_eq!(model.apply(record), Ok(()), "record {index} of the chosen suffix did not apply");
    }
    Recovered::Mounted(model)
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

fn initial() -> Box<CatalogModel> {
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
    let base = initial();

    // §6.3's compaction is two durable steps with an in-progress state between them, and the
    // strength of the oracle comes from testing them **separately**: a three-state assertion over
    // the whole sequence would accept the new checkpoint appearing during the journal write, or the
    // new record appearing during the checkpoint write, which is precisely what the ordering
    // forbids. Step one's cut points admit only {before, compacted}; step two's only
    // {compacted, after}.
    let checkpoint_step = {
        let for_scenario = compacted.clone();
        move |card: &mut Card| {
            let _ = card.write_checkpoint(1, &for_scenario);
        }
    };
    let journal_step = move |card: &mut Card| {
        let _ = card.append_journal(&next_claim);
    };

    // Step one: "A cut before step 4 recovers the old checkpoint and old-epoch journal. A cut after
    // step 4 recovers the new checkpoint and ignores every old-epoch slot."
    let total = {
        let mut card = Card::new(1, &base);
        for record in &setup {
            card.append_journal(record).unwrap();
        }
        let baseline = card.media.ops();
        checkpoint_step(&mut card);
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
            checkpoint_step(&mut card);
            card.media.reboot();
            match recover(&mut card) {
                Recovered::Mounted(model) => assert!(
                    model.as_ref() == before.as_ref() || model.as_ref() == compacted.as_ref(),
                    "compaction gate: cut at op {op} {when:?} recovered neither state (epoch {}, through {})",
                    model.epoch,
                    model.through_sequence,
                ),
                other => panic!("compaction gate: cut at op {op} {when:?} did not mount: {other:?}"),
            }
        }
    }

    // Step two: the new epoch's first record, written only after the new checkpoint's gate is
    // durable. Its cut points admit the compacted state or the state after that record — never the
    // old epoch, because the checkpoint that absorbed it is already selected.
    let total = {
        let mut card = Card::new(2, &base);
        for record in &setup {
            card.append_journal(record).unwrap();
        }
        card.write_checkpoint(1, &compacted).unwrap();
        let baseline = card.media.ops();
        journal_step(&mut card);
        card.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 17 + 9, &base);
            for record in &setup {
                card.append_journal(record).unwrap();
            }
            card.write_checkpoint(1, &compacted).unwrap();
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            journal_step(&mut card);
            card.media.reboot();
            match recover(&mut card) {
                Recovered::Mounted(model) => assert!(
                    model.as_ref() == compacted.as_ref() || model.as_ref() == after.as_ref(),
                    "new-epoch record: cut at op {op} {when:?} recovered neither state (epoch {}, through {})",
                    model.epoch,
                    model.through_sequence,
                ),
                other => panic!("new-epoch record: cut at op {op} {when:?} did not mount: {other:?}"),
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
/// §7's recovery, driven through the production function rather than reimplemented here: read both
/// slots, then hand them and the payload's real bytes to [`work::recover_work`], which applies the
/// reachability rewind *and* the prefix-CRC proof.
fn recover_work(card: &mut Card) -> WorkRecovery {
    let mut slots = [None, None];
    for (slot, held) in slots.iter_mut().enumerate() {
        let Ok(stride) = card.media.read_at(card.work, slot * SLOT_STRIDE, SLOT_STRIDE) else { continue };
        *held = WorkRecord::validate_slot(&stride, slot as u16).ok();
    }
    let payload = card.media.read_at(card.payload, 0, PAYLOAD_LEN).unwrap_or_default();
    work::recover_work(&slots, &payload)
}

/// The payload bytes the WORK scenarios actually write, so a prefix CRC means something.
fn payload_bytes() -> Vec<u8> {
    (0..PAYLOAD_LEN).map(|index| (index * 7 + 11) as u8).collect()
}

#[test]
fn sealing_recovers_the_old_or_the_new_work_slot() {
    let payload = payload_bytes();
    let half = PAYLOAD_LEN as u64 / 2;
    let mut streaming = samples::work(1, half, WorkState::Streaming);
    streaming.prefix_crc = super::raw::crc32(&payload[..half as usize]);
    let mut sealed = samples::work(2, PAYLOAD_LEN as u64, WorkState::Sealed);
    sealed.declared_length = PAYLOAD_LEN as u64;
    sealed.observed_length = PAYLOAD_LEN as u32;
    sealed.prefix_crc = super::raw::crc32(&payload);

    let base = initial();
    // The payload is written and synced first (§7 step 1 and 2), then the slot.
    let setup = |card: &mut Card| {
        card.write_all(card.payload, 0, &payload).unwrap();
        card.media.sync(card.payload).unwrap();
        card.write_gated_slot(card.work, 0, &streaming.encode_slot(0)).unwrap();
    };
    let scenario = |card: &mut Card| {
        let _ = card.write_gated_slot(card.work, SLOT_STRIDE, &sealed.encode_slot(1));
    };
    let total = {
        let mut card = Card::new(1, &base);
        setup(&mut card);
        let baseline = card.media.ops();
        scenario(&mut card);
        card.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 29 + 11, &base);
            setup(&mut card);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            scenario(&mut card);
            card.media.reboot();
            let recovered = recover_work(&mut card);
            assert!(
                recovered == WorkRecovery::Resume(streaming) || recovered == WorkRecovery::Resume(sealed),
                "seal: cut at op {op} {when:?} recovered {recovered:?}",
            );
        }
    }
}

/// §7's rewind: a durable offset above the payload's observed length is unreachable, so that slot
/// is skipped as if invalid and the older reachable one wins.
#[test]
fn an_unreachable_durable_offset_is_skipped_in_favour_of_the_older_slot() {
    let payload = payload_bytes();
    let mut reachable = samples::work(1, 1_024, WorkState::Streaming);
    reachable.prefix_crc = super::raw::crc32(&payload[..1_024]);
    let unreachable = samples::work(2, (PAYLOAD_LEN + SLOT_STRIDE) as u64, WorkState::Streaming);
    let mut card = Card::new(5, &initial());
    card.write_all(card.payload, 0, &payload).unwrap();
    card.media.sync(card.payload).unwrap();
    card.write_gated_slot(card.work, 0, &reachable.encode_slot(0)).unwrap();
    card.write_gated_slot(card.work, SLOT_STRIDE, &unreachable.encode_slot(1)).unwrap();
    assert_eq!(recover_work(&mut card), WorkRecovery::Resume(reachable));
}

/// The other half of §7's clause, on a real card: a slot whose prefix CRC does not match the bytes
/// the payload now holds is never resumed — the work is discarded and its operation aborted.
#[test]
fn a_slot_whose_prefix_the_payload_contradicts_is_discarded() {
    let payload = payload_bytes();
    let mut slot = samples::work(1, 1_024, WorkState::Streaming);
    slot.prefix_crc = super::raw::crc32(&payload[..1_024]) ^ 0xFFFF;
    let mut card = Card::new(6, &initial());
    card.write_all(card.payload, 0, &payload).unwrap();
    card.media.sync(card.payload).unwrap();
    card.write_gated_slot(card.work, 0, &slot.encode_slot(0)).unwrap();
    assert_eq!(recover_work(&mut card), WorkRecovery::DiscardAndAbort);
}

// -------------------------------------------------------------------------------------------
// §7.1 — the active-ride journal
// -------------------------------------------------------------------------------------------

/// §7.1's recovery, through the production function: the ring's slots, the checkpoint's
/// ActiveRideState identity, and the payload's observed length.
fn recover_ride(card: &mut Card, identity: Option<work::RideIdentity>) -> RideRecovery {
    let mut slots: Vec<Option<RideRecord>> = Vec::new();
    for slot in 0..RIDE_SLOTS {
        let Ok(stride) = card.media.read_at(card.ride, slot * SLOT_STRIDE, SLOT_STRIDE) else {
            slots.push(None);
            continue;
        };
        slots.push(RideRecord::validate_slot(&stride, slot as u16).ok());
    }
    let observed = card.media.read_at(card.payload, 0, PAYLOAD_LEN).map(|bytes| bytes.len()).unwrap_or(0);
    work::recover_ride(&slots, identity, observed as u64)
}

fn ride_identity(record: &RideRecord) -> work::RideIdentity {
    work::RideIdentity {
        store: record.store,
        operation: record.operation,
        generation: record.generation,
        recovery_revision: record.recovery_revision,
    }
}

/// §7.1: "Each ride checkpoint first writes and synchronizes payload bytes. It then invalidates and
/// syncs slot `checkpoint_sequence mod 16`, writes and syncs that body, and writes and syncs its
/// gate. The previous highest valid slot remains authoritative until the new gate is durable."
///
/// A cut anywhere therefore recovers the previous checkpoint or the new one — never a mixture, and
/// never a slot the payload cannot back. Losing the newest slot costs one checkpoint interval,
/// which §1.1 accepts for a ride journal and which this asserts rather than assumes.
#[test]
fn a_ride_checkpoint_recovers_the_previous_or_the_new_slot() {
    let payload = payload_bytes();
    let first_offset = 1_024u64;
    let second_offset = 2_048u64;
    let mut first = samples::ride_slot(0, first_offset);
    first.prefix_crc = super::raw::crc32(&payload[..first_offset as usize]);
    let mut second = samples::ride_slot(1, second_offset);
    second.prefix_crc = super::raw::crc32(&payload[..second_offset as usize]);
    let identity = Some(ride_identity(&first));

    let base = initial();
    let setup = |card: &mut Card| {
        card.write_all(card.payload, 0, &payload[..first_offset as usize]).unwrap();
        card.media.sync(card.payload).unwrap();
        card.write_gated_slot(card.ride, 0, &first.encode_slot(0)).unwrap();
    };
    // Sequence 1 lives at ring position 1.
    let scenario = |card: &mut Card| {
        let _ = card.write_all(
            card.payload,
            first_offset as usize,
            &payload[first_offset as usize..second_offset as usize],
        );
        let _ = card.media.sync(card.payload);
        let _ = card.write_gated_slot(card.ride, SLOT_STRIDE, &second.encode_slot(1));
    };
    let total = {
        let mut card = Card::new(1, &base);
        setup(&mut card);
        let baseline = card.media.ops();
        scenario(&mut card);
        card.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 41 + 17, &base);
            setup(&mut card);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            scenario(&mut card);
            card.media.reboot();
            let recovered = recover_ride(&mut card, identity);
            assert!(
                recovered == RideRecovery::Resume(first) || recovered == RideRecovery::Resume(second),
                "ride checkpoint: cut at op {op} {when:?} recovered {recovered:?}",
            );
        }
    }
}

/// The ring wraps: sequence 16 is written over position 0, and the slot it replaces is the oldest
/// one, so a cut during that write still leaves the newest valid earlier slot authoritative.
#[test]
fn a_wrapping_ride_checkpoint_never_loses_more_than_one_interval() {
    let payload = payload_bytes();
    let mut newest = samples::ride_slot(15, 1_024);
    newest.prefix_crc = super::raw::crc32(&payload[..1_024]);
    let mut wrapped = samples::ride_slot(16, 2_048);
    wrapped.prefix_crc = super::raw::crc32(&payload[..2_048]);
    let identity = Some(ride_identity(&newest));

    let base = initial();
    let mut card = Card::new(7, &base);
    card.write_all(card.payload, 0, &payload[..2_048]).unwrap();
    card.media.sync(card.payload).unwrap();
    card.write_gated_slot(card.ride, 15 * SLOT_STRIDE, &newest.encode_slot(15)).unwrap();

    // The write that wraps onto position 0 is cut mid-page: position 0 held nothing, and slot 15
    // is untouched, so recovery still resumes from sequence 15.
    let baseline = card.media.ops();
    card.media.set_plan(FaultPlan::cut(baseline + 3, When::During));
    let _ = card.write_gated_slot(card.ride, 0, &wrapped.encode_slot(0));
    card.media.reboot();
    assert_eq!(recover_ride(&mut card, identity), RideRecovery::Resume(newest));

    // Completing the same write makes sequence 16 authoritative.
    card.write_gated_slot(card.ride, 0, &wrapped.encode_slot(0)).unwrap();
    assert_eq!(recover_ride(&mut card, identity), RideRecovery::Resume(wrapped));
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
///
/// **What this covers, and what it does not.** The cut points here are the *record* writes:
/// `INIT.REC`'s body and gate, and the first checkpoint's three writes with their syncs. §12's file
/// creation order — the `OBC2` directory, the shard trees, then each preallocated file zero-filled
/// in turn — is modelled by [`Media::create`], which is deliberately **not** a counted operation, so
/// it has no cut points in this matrix. That is honest rather than convenient: judging a cut inside
/// the creation prefix needs §12's mount classification (fresh card, resumable witness, ungated
/// pre-birth prefix, unknown shape), which is a later slice. Until it exists, this scenario proves
/// only the half that the record codecs own.
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
                    // Pre-birth. Three things must hold, and the point of stating all three is that
                    // "no checkpoint" on its own is a nearly vacuous assertion:
                    let init_image = card.media.image(card.init).to_vec();
                    // A witness that validates is *this* attempt's witness — the StoreId has never
                    // escaped, so a different one would mean the card was reused. One that does not
                    // validate is a torn write, which §12 makes a bounded restart case rather than
                    // a record with partial contents.
                    if let Ok(record) = InitRecord::validate_slot(&init_image) {
                        assert_eq!(record, witness);
                    }
                    // Neither checkpoint carries a valid gate: the StoreId birth point is the first
                    // checkpoint gate, so nothing may be advertised before it.
                    for index in 0..2 {
                        let image = card.media.image(card.cat[index]).to_vec();
                        assert!(
                            checkpoint::validate_file(&image, index as u16).is_err(),
                            "cut at op {op} {when:?}: a checkpoint validated while recovery saw none",
                        );
                    }
                    // And no journal record exists: initialization writes none.
                    let journal = card.media.image(card.journal).to_vec();
                    assert!(
                        JournalBody::validate_slot(&journal[..SLOT_STRIDE], 0).is_err(),
                        "cut at op {op} {when:?}: initialization produced a journal record",
                    );
                }
                Recovered::Mounted(model) => assert_eq!(model.as_ref(), empty.as_ref()),
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
    let ride = media.create("RIDE.ACT", RIDE_FILE_LEN);
    let init = media.create("INIT.REC", SLOT_FILE_LEN);
    let work = media.create("WORK", WORK_FILE_LEN);
    let payload = media.create("GEN", PAYLOAD_LEN);
    Card { media, cat: [cat0, cat1], journal, arm: [arm0, arm1], ride, work, payload, init }
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
                model.as_ref() == before.as_ref() || model.as_ref() == after.as_ref(),
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

/// The media operations one `append_journal` performs: body stride, sync, gate, sync.
const OPS_PER_APPEND: u32 = 4;

/// A bounded deterministic sequence generator.
///
/// It emits every presence bit a slice-1 record can carry — claims, publications, deletes, draft
/// parents and parts, retention puts and removes, weather, the ride domain record and its removal,
/// handoff puts and the zero-identity removal, both repository cursors, and the generation
/// reservation — because a generator that only ever exercised the same three bits would make the
/// cut matrix look strong while proving almost nothing.
fn random_sequence(seed: u64, model: &mut CatalogModel, steps: u64) -> Vec<JournalBody> {
    let mut rng = seed | 1;
    let mut next = || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut records: Vec<JournalBody> = Vec::new();
    let mut claimed: Vec<[u8; 16]> = Vec::new();
    let mut published: Vec<(u16, u64)> = Vec::new();
    let mut retained_generations: Vec<u64> = Vec::new();
    let mut commits = 0u64;
    let mut revision = 0u64;
    let mut parent_open = false;
    let mut parent_claimed = false;
    let mut handoff_sequence = 0u64;
    let mut ride_open = false;

    for step in 0..steps {
        let slot = records.len() as u16;
        let sequence = model.through_sequence + 1;
        let epoch = model.epoch;
        let cursor = model.next_generation + 1;
        let choice = next() % 10;

        let record = match choice {
            // A draft parent and its first part, claimed atomically (bits 0, 4, 6, 18).
            0 if !parent_claimed && !parent_open => {
                parent_open = true;
                parent_claimed = true;
                claimed.push(samples::OP_PARENT);
                let mut record = samples::claim(epoch, sequence, slot, samples::OP_PARENT, cursor);
                let mut parent = samples::parent();
                parent.state = super::entries::DraftParentState::Open;
                record.mutation.draft_parent = Some(Change::Put(parent));
                let mut part = samples::part(1);
                part.state = DraftPartState::Prepared;
                part.part_ref = obc_link::ids::DraftPartRef::ZERO;
                record.mutation.draft_part = Some(Change::Put(part));
                record
            }
            // The parent's terminal record: it removes the parent and, with it, every part row
            // (bits 1, 5, 10).
            1 if parent_open => {
                parent_open = false;
                claimed.retain(|operation| operation != &samples::OP_PARENT);
                commits += 1;
                let mut record = samples::publish(
                    epoch,
                    sequence,
                    slot,
                    samples::OP_PARENT,
                    commits,
                    samples::manifest_head(step, 90),
                );
                record.mutation.draft_parent =
                    Some(Change::Remove(obc_link::ids::OperationId::new(samples::OP_PARENT)));
                revision += 1;
                published.push((6, step));
                record
            }
            // A pre-claim ride domain record and its removal (bits 16, 17, 18).
            2 if !ride_open => {
                ride_open = true;
                let mut ride = samples::ride();
                ride.generation = GenerationId::new(model.next_generation);
                JournalBody {
                    store: samples::STORE,
                    epoch,
                    sequence,
                    slot,
                    kind: RecordKind::Domain,
                    operation: OperationId::ZERO,
                    intent: [0u8; 32],
                    mutation: Mutation {
                        ride: Some(Change::Put(ride)),
                        generation_cursor: Some(cursor),
                        ..Mutation::default()
                    },
                }
            }
            3 if ride_open => {
                ride_open = false;
                JournalBody {
                    store: samples::STORE,
                    epoch,
                    sequence,
                    slot,
                    kind: RecordKind::Domain,
                    operation: OperationId::ZERO,
                    intent: [0u8; 32],
                    mutation: Mutation { ride: Some(Change::Remove(())), ..Mutation::default() },
                }
            }
            // A handoff put under the install claim, and the zero-identity cleanup (bits 11, 12).
            4 if handoff_sequence == 0 => {
                handoff_sequence = 4;
                JournalBody {
                    store: samples::STORE,
                    epoch,
                    sequence,
                    slot,
                    kind: RecordKind::Handoff,
                    operation: OperationId::new(samples::OP_INSTALL),
                    intent: samples::INTENT,
                    mutation: Mutation {
                        handoff: Some(Change::Put(samples::handoff_ref(4, HandoffPhase::Armed))),
                        ..Mutation::default()
                    },
                }
            }
            5 if handoff_sequence != 0 => {
                handoff_sequence = 0;
                JournalBody {
                    store: samples::STORE,
                    epoch,
                    sequence,
                    slot,
                    kind: RecordKind::Handoff,
                    operation: OperationId::ZERO,
                    intent: [0u8; 32],
                    mutation: Mutation { handoff: Some(Change::Remove(())), ..Mutation::default() },
                }
            }
            // A head delete: active remove, head remove, revision, result (bits 1, 3, 10, 13).
            6 if !published.is_empty() && !claimed.is_empty() => {
                let (kind, id) = published.remove(0);
                let operation = claimed.remove(0);
                commits += 1;
                revision += 1;
                let mut record = samples::publish(epoch, sequence, slot, operation, commits, samples::head(kind, id));
                record.mutation.head =
                    Some(Change::Remove(super::entries::HeadKey { kind, id: obc_link::ids::LogicalObjectId::new(id) }));
                record
            }
            // A retention removal (bit 9).
            7 if !retained_generations.is_empty() => {
                let generation = retained_generations.remove(0);
                samples::retention_remove(epoch, sequence, slot, generation)
            }
            // An ordinary publication, sometimes retaining the generation it displaces, sometimes
            // carrying the weather state and the repository's logical-ID cursor (bits 8, 14, 15).
            _ if !claimed.is_empty() => {
                let operation = claimed.remove(0);
                commits += 1;
                revision += 1;
                let mut head = samples::head(1, step);
                head.revision = obc_link::ids::Revision::new(revision);
                let mut record = samples::publish(epoch, sequence, slot, operation, commits, head);
                published.push((1, step));
                if let Some(repository) = &mut record.mutation.repository {
                    repository.revision = Some(revision);
                    if next() % 2 == 0 {
                        repository.next_logical_id = Some(step + 1);
                    }
                }
                if next() % 2 == 0 {
                    let generation = 100 + step;
                    let mut entry = samples::retained(generation);
                    entry.reasons = RetainedPrevious::REASON_UPDATE_ROLLBACK;
                    entry.lease_count = 0;
                    record.mutation.retained = Some(Change::Put(entry));
                    retained_generations.push(generation);
                }
                if next() % 3 == 0 {
                    record.mutation.weather = Some(samples::weather());
                }
                record
            }
            // Nothing else was possible this step: claim a fresh operation (bits 0, 18).
            _ => {
                let mut operation = samples::OP_A;
                operation[0] = step as u8;
                operation[1] = seed as u8;
                claimed.push(operation);
                samples::claim(epoch, sequence, slot, operation, cursor)
            }
        };
        if model.apply(&record).is_err() {
            continue;
        }
        records.push(record);
    }
    records
}

/// The strict form of the property: a cut inside append `i` recovers the projection **before** that
/// append or the one **after** it — not "some prefix".
///
/// The weaker any-prefix form is what a multi-record silent rollback would pass: losing three
/// committed records still lands on a prefix. Binding the admissible set to the append the cut is
/// inside is what makes the assertion catch it, and it is computable because `append_journal`
/// performs exactly [`OPS_PER_APPEND`] operations per record.
#[test]
fn randomized_sequences_recover_to_the_append_the_cut_is_inside() {
    for seed in 1..=12u64 {
        let base = initial();
        let mut projected = base.clone();
        let records = random_sequence(seed, &mut projected, 16);
        assert!(records.len() >= 8, "seed {seed} produced only {} records", records.len());

        let mut prefixes = vec![base.clone()];
        let mut walk = base.clone();
        for record in &records {
            walk.apply(record).unwrap();
            prefixes.push(walk.clone());
        }

        let total = records.len() as u32 * OPS_PER_APPEND;
        for op in 1..=total {
            for when in EVERY_WHEN {
                let mut card = Card::new(seed * 101 + u64::from(op), &base);
                let baseline = card.media.ops();
                card.media.set_plan(FaultPlan::cut(baseline + op, when));
                for record in &records {
                    if card.append_journal(record).is_err() {
                        break;
                    }
                }
                card.media.reboot();
                // Appends are 1-based: op falls inside append `index`, so appends 1..index-1 are
                // durable and append `index` either landed or did not.
                let index = ((op - 1) / OPS_PER_APPEND) as usize;
                let admissible = [&prefixes[index], &prefixes[index + 1]];
                match recover(&mut card) {
                    Recovered::Mounted(model) => assert!(
                        admissible.iter().any(|prefix| prefix.as_ref() == model.as_ref()),
                        "seed {seed}: cut at op {op} {when:?} recovered through sequence {} — not {} or {}",
                        model.through_sequence,
                        admissible[0].through_sequence,
                        admissible[1].through_sequence,
                    ),
                    other => panic!("seed {seed}: cut at op {op} {when:?} did not mount: {other:?}"),
                }
            }
        }
    }
}

/// A long fault-free sequence that wraps the 64-entry result ring, compacts once, and continues.
///
/// The cut matrix above runs short sequences because it is quadratic; this one runs long enough to
/// reach the eviction path and the epoch change, and asserts recovery reproduces the model exactly.
#[test]
fn a_long_sequence_wraps_the_result_ring_and_survives_a_compaction() {
    let base = initial();
    let mut model = base.clone();
    let mut card = Card::new(21, &base);

    // 96 claim/publish pairs: 192 records, which is exactly §6.3's compaction trigger, and 96
    // terminal commits, so the 64-entry ring evicts 32 times.
    let mut records: Vec<JournalBody> = Vec::new();
    for step in 0..96u64 {
        let mut operation = samples::OP_A;
        operation[0] = step as u8;
        operation[1] = 0x5A;
        let slot = records.len() as u16;
        let claim = samples::claim(model.epoch, model.through_sequence + 1, slot, operation, model.next_generation + 1);
        model.apply(&claim).unwrap();
        card.append_journal(&claim).unwrap();
        records.push(claim);

        let slot = records.len() as u16;
        let publish = samples::publish(
            model.epoch,
            model.through_sequence + 1,
            slot,
            operation,
            step + 1,
            samples::head(1, step),
        );
        model.apply(&publish).unwrap();
        card.append_journal(&publish).unwrap();
        records.push(publish);
    }
    assert_eq!(model.results.len(), 64, "the ring did not fill");
    assert_eq!(model.result_start, 96 % 64, "the ring did not wrap");
    assert!(recovery::compaction_required(records.len()), "192 records is the compaction trigger");

    let Recovered::Mounted(recovered) = recover(&mut card) else { panic!("did not mount") };
    assert_eq!(recovered.as_ref(), model.as_ref());

    // Compaction absorbs all of it into CAT1 at epoch 2, and the next record opens the new epoch.
    let mut compacted = model.clone();
    compacted.epoch = 2;
    card.write_checkpoint(1, &compacted).unwrap();
    let next = samples::claim(2, compacted.through_sequence + 1, 0, samples::OP_B, compacted.next_generation + 1);
    card.append_journal(&next).unwrap();
    let mut after = compacted.clone();
    after.apply(&next).unwrap();

    let Recovered::Mounted(recovered) = recover(&mut card) else { panic!("did not mount after compaction") };
    assert_eq!(recovered.as_ref(), after.as_ref());
    assert_eq!(recovered.results.len(), 64, "compaction lost the ring");
}

// -------------------------------------------------------------------------------------------
// Fuzz: mutated bytes never panic and always reject with a typed error
// -------------------------------------------------------------------------------------------

/// A bounded deterministic corpus: every slot-shaped record, mutated at seeded positions with
/// seeded values.
///
/// The mutations do **not** re-stamp any CRC, so every one of them breaks a gate, a body CRC or a
/// zero run — which means the property is a rejection, not merely an absence of panics. Asserting
/// it that way is what catches a flip that cancels: two mutations whose net effect a weaker test
/// would silently accept.
#[test]
fn mutated_records_are_rejected_without_panicking() {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Shape {
        Journal,
        Work,
        Ride,
        Handoff,
        Init,
    }
    // Each entry carries the physical slot its bytes belong at: a record read at another position
    // is invalid by construction, which would make the rejection below meaningless.
    let corpus: Vec<(Shape, u16, Vec<u8>)> = vec![
        (Shape::Journal, 0, samples::claim(1, 1, 0, samples::OP_A, 1).encode_slot().to_vec()),
        (Shape::Journal, 1, samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7)).encode_slot().to_vec()),
        (Shape::Journal, 0, samples::retention_remove(1, 1, 0, 9).encode_slot().to_vec()),
        (Shape::Work, 0, samples::work(1, 1_024, WorkState::Streaming).encode_slot(0).to_vec()),
        (Shape::Work, 1, samples::work(2, 2_048, WorkState::Sealed).encode_slot(1).to_vec()),
        (Shape::Ride, 0, samples::ride_slot(0, 1_024).encode_slot(0).to_vec()),
        (Shape::Ride, 1, samples::ride_slot(17, 2_048).encode_slot(1).to_vec()),
        (Shape::Handoff, 0, samples::handoff_record(4, HandoffPhase::Armed).encode_slot(0).to_vec()),
        (Shape::Init, 0, InitRecord { store: samples::STORE }.encode_slot().to_vec()),
    ];

    let mut rng = 0x1234_5678_9ABC_DEF0u64;
    let mut next = || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    for (shape, slot, original) in &corpus {
        // The unmutated bytes decode, or the rejections below would prove nothing.
        match shape {
            Shape::Journal => assert!(JournalBody::validate_slot(original, *slot).is_ok()),
            Shape::Work => assert!(WorkRecord::validate_slot(original, *slot).is_ok()),
            Shape::Ride => assert!(RideRecord::validate_slot(original, *slot).is_ok()),
            Shape::Handoff => assert!(HandoffRecord::validate_slot(original, *slot).is_ok()),
            Shape::Init => assert!(InitRecord::validate_slot(original).is_ok()),
        }
        for round in 0..400 {
            let mut mutated = original.clone();
            let flips = 1 + (next() % 3) as usize;
            for _ in 0..flips {
                let index = (next() as usize) % mutated.len();
                mutated[index] ^= (next() as u8) | 1;
            }
            // Every byte of a slot is covered by the body CRC, the gate, or the zero-pad rule, so
            // *every* mutation must be refused — by the record's own decoder and by every other.
            let outcomes = [
                JournalBody::validate_slot(&mutated, *slot).is_ok(),
                WorkRecord::validate_slot(&mutated, *slot).is_ok(),
                RideRecord::validate_slot(&mutated, *slot).is_ok(),
                HandoffRecord::validate_slot(&mutated, *slot).is_ok(),
                InitRecord::validate_slot(&mutated).is_ok(),
            ];
            assert!(
                !outcomes.iter().any(|accepted| *accepted),
                "{shape:?} round {round}: a mutated slot was accepted by some decoder",
            );
            // The resolution body has no CRC of its own, so it is exercised for totality only.
            let _ = resolution::Resolution::decode(&mutated[..resolution::MAX_BODY_LEN.min(mutated.len())]);
        }
    }
}

/// Field-level mutations that are *structurally consistent* — CRC and gate re-stamped — so the flip
/// reaches the structural decoders instead of stopping at the checksum.
///
/// A CRC catches corruption; it says nothing about whether a decoder enforces a header rule. This
/// is the half of the fuzz that does, and each case names the rule it expects to trip.
#[test]
fn restamped_field_mutations_reach_the_structural_rules() {
    use super::error::Reason;

    // (offset in the body, replacement bytes, the reason the decoder must give)
    let cases: [(usize, &[u8], Reason); 9] = [
        (0, b"XXXX", Reason::Magic),
        (4, &2u16.to_le_bytes(), Reason::Version),
        (6, &64u16.to_le_bytes(), Reason::HeaderLength),
        (24, &0u64.to_le_bytes(), Reason::Sequence),
        (40, &300u16.to_le_bytes(), Reason::SlotIndex),
        (42, &9u16.to_le_bytes(), Reason::UnknownEnum),
        (92, &1_000u16.to_le_bytes(), Reason::Overflow),
        (94, &[1, 0], Reason::Reserved),
        (96 + 4, &(1u32 << 19).to_le_bytes(), Reason::Reserved),
    ];

    for (offset, replacement, expected) in cases {
        let record = samples::claim(1, 1, 0, samples::OP_A, 1);
        let mut slot = record.encode_slot();
        slot[offset..offset + replacement.len()].copy_from_slice(replacement);
        // Re-stamp the body CRC and rebuild the gate over the mutated body, so the record is
        // internally consistent and only the field rule can refuse it.
        let crc = super::raw::crc32_with_hole(&slot[..JOURNAL_BODY_LEN], super::limits::JOURNAL_BODY_CRC_OFFSET);
        slot[super::limits::JOURNAL_BODY_CRC_OFFSET..super::limits::JOURNAL_BODY_CRC_OFFSET + 4]
            .copy_from_slice(&crc.to_le_bytes());
        let gate = super::gate::Gate {
            magic: super::gate::MAGIC_JOURNAL,
            slot: super::raw::u16_at(&slot, 40),
            scope: super::raw::u64_at(&slot, 24),
            sequence: super::raw::u64_at(&slot, 32),
            body_crc: crc,
        };
        slot[JOURNAL_GATE_OFFSET..JOURNAL_GATE_OFFSET + 512].copy_from_slice(&gate.encode());

        let error = JournalBody::validate_slot(&slot, 0).expect_err("a re-stamped field mutation was accepted");
        assert_eq!(error.reason, expected, "field at {offset} gave {error:?}");
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
    // Half the rounds mutate raw bytes, which the body CRC alone refuses.
    for _ in 0..2_000 {
        let mut mutated = file.clone();
        let index = (next() as usize) % CHECKPOINT_FILE_LEN;
        mutated[index] ^= (next() as u8) | 1;
        assert!(checkpoint::validate_file(mutated.as_slice(), 0).is_err(), "byte {index} mutation accepted");
    }

    // The other half re-stamps the body CRC and the gate, so the mutation is internally consistent
    // and reaches the region rules — counts, sort order, reserved runs, entry enums. A CRC proves
    // nothing about those, and this is the only way a fuzz round gets past it.
    let mut reached_structure = 0;
    for _ in 0..4_000 {
        let mut mutated = file.clone();
        // Confine mutations to the header and the occupied regions; the zero tail is already
        // covered by the raw rounds above and would only ever trip the reserved-run rule.
        let index = (next() as usize) % 60_096;
        mutated[index] ^= (next() as u8) | 1;
        checkpoint::seal_body(&mut mutated[..CHECKPOINT_BODY_LEN]);
        let gate = checkpoint::gate_for(&mutated[..CHECKPOINT_BODY_LEN], 0);
        mutated[CHECKPOINT_GATE_OFFSET..].copy_from_slice(&gate.encode());

        match checkpoint::validate_file(mutated.as_slice(), 0) {
            // Accepting is legitimate: a flip inside, say, a payload length produces a different
            // but perfectly well-formed checkpoint. What must never happen is a panic, and what a
            // re-stamped mutation must never do is bypass a structural rule — so when it *is*
            // refused, the refusal has to be typed, which the type system already guarantees.
            Ok(header) => {
                assert!(header.head_count as usize <= super::limits::MAX_CATALOG_HEADS);
            }
            Err(error) => {
                reached_structure += 1;
                // The refusal names the shape that refused — the body itself, its gate, or the
                // entry whose rule the flip broke — and never something outside the checkpoint.
                use super::error::Record::*;
                assert!(
                    matches!(
                        error.record,
                        Checkpoint
                            | Gate
                            | RepositoryState
                            | CatalogHead
                            | ActiveOperation
                            | DraftParent
                            | DraftPart
                            | RetainedPrevious
                            | TerminalResult
                            | WeatherState
                            | ActiveRide
                            | HandoffRef
                    ),
                    "{error:?} is not a refusal a checkpoint can produce",
                );
            }
        }
    }
    assert!(reached_structure > 0, "no re-stamped mutation reached a structural rule");
}

// -------------------------------------------------------------------------------------------
// Steady state: both sides of an alternating pair valid at once
// -------------------------------------------------------------------------------------------

/// The cut matrices above all start from a pair with one valid side, which is the *initial* state,
/// not the steady one. In steady state both sides are valid and selection has to pick — and the
/// write that reuses the older side must not disturb the selected one at any point.
#[test]
fn a_steady_state_work_pair_selects_the_newer_slot_and_survives_its_own_reuse() {
    let payload = payload_bytes();
    let mut older = samples::work(1, 1_024, WorkState::Streaming);
    older.prefix_crc = super::raw::crc32(&payload[..1_024]);
    let mut newer = samples::work(2, 2_048, WorkState::Streaming);
    newer.prefix_crc = super::raw::crc32(&payload[..2_048]);
    let mut newest = samples::work(3, 3_072, WorkState::Streaming);
    newest.prefix_crc = super::raw::crc32(&payload[..3_072]);

    let base = initial();
    let setup = |card: &mut Card| {
        card.write_all(card.payload, 0, &payload).unwrap();
        card.media.sync(card.payload).unwrap();
        card.write_gated_slot(card.work, 0, &older.encode_slot(0)).unwrap();
        card.write_gated_slot(card.work, SLOT_STRIDE, &newer.encode_slot(1)).unwrap();
    };

    // Both slots are valid; the greater sequence is authoritative.
    let mut card = Card::new(31, &base);
    setup(&mut card);
    assert_eq!(recover_work(&mut card), WorkRecovery::Resume(newer));

    // Sequence 3 reuses slot 0 — the older of the pair. At every cut point the selected record is
    // still `newer` or already `newest`, and never nothing.
    let total = {
        let mut probe = Card::new(1, &base);
        setup(&mut probe);
        let baseline = probe.media.ops();
        probe.write_gated_slot(probe.work, 0, &newest.encode_slot(0)).unwrap();
        probe.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 53 + 3, &base);
            setup(&mut card);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            let _ = card.write_gated_slot(card.work, 0, &newest.encode_slot(0));
            card.media.reboot();
            let recovered = recover_work(&mut card);
            assert!(
                recovered == WorkRecovery::Resume(newer) || recovered == WorkRecovery::Resume(newest),
                "steady-state WORK: cut at op {op} {when:?} recovered {recovered:?}",
            );
        }
    }
}

/// The same for the ARM pair, whose selection is `(handoff_sequence, phase)` rather than a
/// sequence: both files valid, the strictly greater pair wins, and advancing to the next handoff
/// sequence reuses the file holding the older pair without ever leaving the selection empty.
#[test]
fn a_steady_state_arm_pair_selects_the_greater_pair_and_survives_its_own_reuse() {
    let prepared = samples::handoff_record(4, HandoffPhase::Prepared);
    let armed = samples::handoff_record(4, HandoffPhase::Armed);
    let next_handoff = samples::handoff_record(5, HandoffPhase::Prepared);

    let base = initial();
    let setup = |card: &mut Card| {
        card.write_gated_slot(card.arm[0], 0, &prepared.encode_slot(0)).unwrap();
        card.write_gated_slot(card.arm[1], 0, &armed.encode_slot(1)).unwrap();
    };

    let mut card = Card::new(33, &base);
    setup(&mut card);
    assert_eq!(recover_handoff(&card), Some(armed), "the greater (sequence, phase) pair must win");

    let total = {
        let mut probe = Card::new(1, &base);
        setup(&mut probe);
        let baseline = probe.media.ops();
        probe.write_gated_slot(probe.arm[0], 0, &next_handoff.encode_slot(0)).unwrap();
        probe.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 59 + 5, &base);
            setup(&mut card);
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            let _ = card.write_gated_slot(card.arm[0], 0, &next_handoff.encode_slot(0));
            card.media.reboot();
            let recovered = recover_handoff(&card);
            assert!(
                recovered == Some(armed) || recovered == Some(next_handoff),
                "steady-state ARM: cut at op {op} {when:?} recovered {recovered:?}",
            );
        }
    }
}

/// And for the checkpoints: with both valid, the greater `through_sequence` is mounted, and writing
/// the inactive one leaves the selected one untouched at every cut.
#[test]
fn a_steady_state_checkpoint_pair_mounts_the_greater_through_sequence() {
    let base = initial();
    let mut older = base.clone();
    older.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
    let mut newer = older.clone();
    newer.epoch = 2;
    newer.apply(&samples::publish(2, 2, 0, samples::OP_A, 1, samples::head(1, 7))).unwrap();

    let mut card = Card::new(35, &base);
    card.write_checkpoint(0, &older).unwrap();
    card.write_checkpoint(1, &newer).unwrap();
    let Recovered::Mounted(mounted) = recover(&mut card) else { panic!("did not mount") };
    assert_eq!(mounted.as_ref(), newer.as_ref(), "the greater through-sequence must be mounted");

    // Rewriting CAT0 — the inactive side — at every cut point never disturbs CAT1.
    let mut third = newer.clone();
    third.epoch = 3;
    third.apply(&samples::claim(3, 3, 0, samples::OP_B, 2)).unwrap();
    let total = {
        let mut probe = Card::new(1, &base);
        probe.write_checkpoint(0, &older).unwrap();
        probe.write_checkpoint(1, &newer).unwrap();
        let baseline = probe.media.ops();
        probe.write_checkpoint(0, &third).unwrap();
        probe.media.ops() - baseline
    };
    for op in 1..=total {
        for when in EVERY_WHEN {
            let mut card = Card::new(u64::from(op) * 61 + 7, &base);
            card.write_checkpoint(0, &older).unwrap();
            card.write_checkpoint(1, &newer).unwrap();
            let baseline = card.media.ops();
            card.media.set_plan(FaultPlan::cut(baseline + op, when));
            let _ = card.write_checkpoint(0, &third);
            card.media.reboot();
            match recover(&mut card) {
                Recovered::Mounted(model) => assert!(
                    model.as_ref() == newer.as_ref() || model.as_ref() == third.as_ref(),
                    "steady-state checkpoints: cut at op {op} {when:?} mounted through {}",
                    model.through_sequence,
                ),
                other => panic!("steady-state checkpoints: cut at op {op} {when:?} produced {other:?}"),
            }
        }
    }
}

// -------------------------------------------------------------------------------------------
// §13.1's other fault modes: short write, media full, corrupt read
// -------------------------------------------------------------------------------------------

/// §13.1: "a short write is an error, never a success". The writer checks the returned length, so a
/// short write aborts the commit before its gate — which recovers as the old state, exactly like a
/// cut in the same place.
#[test]
fn a_short_write_aborts_the_commit_rather_than_completing_it() {
    let before = initial();
    let record = samples::claim(1, 1, 0, samples::OP_A, 1);
    // Every write of the append in turn: the body stride, then the gate.
    for (op, short) in [(1u32, 4_096usize), (3, 128)] {
        let mut card = Card::new(71, &before);
        let baseline = card.media.ops();
        card.media.set_plan(FaultPlan { short_write: Some((baseline + op, short)), ..FaultPlan::default() });
        assert_eq!(card.append_journal(&record), Err(MediaError::Full), "a short write must not report success");
        card.media.reboot();
        let Recovered::Mounted(model) = recover(&mut card) else { panic!("did not mount") };
        assert_eq!(model.as_ref(), before.as_ref(), "a short write left a partially committed record");
    }
}

/// A full medium refuses the write outright. The commit does not happen and the store is exactly
/// what it was — a refusal, never a partial record.
#[test]
fn a_full_medium_leaves_the_store_unchanged() {
    let before = initial();
    let record = samples::claim(1, 1, 0, samples::OP_A, 1);
    for op in [1u32, 3] {
        let mut card = Card::new(73, &before);
        let baseline = card.media.ops();
        card.media.set_plan(FaultPlan { media_full: Some(baseline + op), ..FaultPlan::default() });
        assert_eq!(card.append_journal(&record), Err(MediaError::Full));
        card.media.reboot();
        let Recovered::Mounted(model) = recover(&mut card) else { panic!("did not mount") };
        assert_eq!(model.as_ref(), before.as_ref());
    }
}

/// A corrupt read can cost visibility — a record or a checkpoint that does not decode this time —
/// but it can never invent state. The mount either lands on a state the records produce, or reports
/// no checkpoint at all; it never produces a catalog no sequence of records could.
#[test]
fn a_corrupt_read_loses_visibility_but_never_invents_state() {
    let before = initial();
    let record = samples::claim(1, 1, 0, samples::OP_A, 1);
    let mut after = before.clone();
    after.apply(&record).unwrap();

    // Recovery reads: CAT0, CAT1, then all 256 journal slots. Poison one read of each kind.
    for read_index in [1u32, 3, 4] {
        let mut card = Card::new(77, &before);
        card.append_journal(&record).unwrap();
        card.media.reboot();
        let baseline = card.media.ops();
        card.media.set_plan(FaultPlan { corrupt_read: Some(baseline + read_index), ..FaultPlan::default() });
        match recover(&mut card) {
            Recovered::Mounted(model) => assert!(
                model.as_ref() == before.as_ref() || model.as_ref() == after.as_ref(),
                "corrupt read at {read_index} invented a state",
            ),
            // Losing the only valid checkpoint to a bad read is a legitimate outcome: nothing was
            // repaired and nothing was written.
            Recovered::NoCheckpoint => {}
            other => panic!("corrupt read at {read_index} produced {other:?}"),
        }
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
