//! The streaming checkpoint materialization of `OBC2_Storage_Format.md` §6.3, step 3.
//!
//! Compaction's third step writes the inactive checkpoint's complete 65,024-byte body. §6.3 is
//! precise about how: it "materializes that body without ever holding the projection in RAM. It is a
//! single forward pass over the inactive checkpoint file, region by region and, inside a region,
//! entry by entry in key order." Three sources feed it, newest wins:
//!
//! - **the RAM index** ([`RamIndex`]), "authoritative for everything it holds";
//! - **a journal-carried head entry**, when a head-putting record has been replayed since the active
//!   checkpoint — found through the per-head `u16` journal-slot reference, never by scanning;
//! - **the active checkpoint's stored bytes**, "by one bounded read", for everything else.
//!
//! Staging is bounded at "one entry … plus one 512-byte sector buffer", which is
//! [`STAGING_BYTES`]. The body CRC accumulates across the pass, so nothing has to be re-read to
//! compute it, and §6.3's step 4 writes the gate last: "an interrupted pass leaves an invalid
//! checkpoint rather than a half-new one".
//!
//! ## Which fields are card-sourced, and why there are two of them
//!
//! §6.3 names one card source explicitly — a head's catalog-projection envelope and its resolution
//! `GenerationId` with the flag that travels with it. §13 names a second in passing, where it sizes
//! the result ring index at "OperationId and commit sequence with the result body re-read from
//! card". Both are card-sourced here, and the staging bound §6.3 states is itself the evidence:
//! 208 bytes is the terminal-result entry, and a pass that held every result in RAM would need no
//! such stage.
//!
//! That second source needed one thing §6.3 gives only the first. A result appended by a record
//! replayed since the active checkpoint is in **no** checkpoint yet, so "re-read from card" has to
//! mean re-read from the journal — through the same kind of `u16` slot reference §6.3 gives a head.
//! [`ResultIndexEntry`] therefore carries one, and it costs nothing: §13 budgets the ring index at
//! 32 bytes per entry for an `OperationId` and a commit sequence, which is 24, and the reference
//! fits inside the eight bytes that were already there.
//!
//! ## What this deliberately does not do
//!
//! It writes no gate, syncs nothing, and never touches the journal. §6.3's five steps are an
//! ordering the *store* performs — invalidate the inactive gate and sync, run this pass and sync,
//! write the gate and sync, and only then open the new epoch at slot zero — and that ordering is
//! what the crash matrix cuts. This function is step 3 alone, so that the ordering has exactly one
//! implementation and it is not this one.

use obc_link::ids::GenerationId;

use super::checkpoint::{self, Region};
use super::entries::{CatalogHead, HeadKey, TerminalResult};
use super::gate::Gate;
use super::index::{HeadIndexEntry, RamIndex, ResultIndexEntry};
use super::limits::{CHECKPOINT_BODY_CRC_OFFSET, CHECKPOINT_BODY_LEN, MAX_TERMINAL_RESULTS, SECTOR};
use super::raw::put_u32;

/// The largest entry shape, and therefore the pass's whole entry stage (§6.3).
///
/// **This is 240, not the 208 §6.3 and §13 originally stated.** Both called 208 "the largest entry
/// shape", and 208 is the terminal-result entry — but §5.1's own region table gives the
/// update-handoff projection 240 bytes, which is larger. The bound's *shape* was right and its
/// arithmetic was 32 bytes short. This constant is derived from the regions rather than restated
/// from the prose, so the two cannot disagree silently again; the spec now says 240 too.
pub const MAX_ENTRY_LEN: usize = super::limits::HANDOFF_REF_LEN;

/// The figure §6.3 and §13 state for the entry stage, kept so the discrepancy above is a value a
/// test can compare rather than a comment.
pub const SPEC_STATED_ENTRY_LEN: usize = TerminalResult::LEN;

/// §6.3's complete staging bound: one entry plus one sector.
pub const STAGING_BYTES: usize = MAX_ENTRY_LEN + SECTOR;

/// The two per-head fields §6.3 leaves on the card.
///
/// They travel together on purpose: §5.3 puts the resolution-present flag and the resolution
/// `GenerationId` in one head entry, and §6.3 makes the flag card-resident with its field, so a
/// source that returned one without the other could publish a manifest head with no resolution or a
/// resolution no flag admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardHeadFields {
    /// The catalog-projection envelope's length, `8..=96` (§5.3).
    pub envelope_len: u16,
    /// The envelope bytes, zero-padded to the 96-byte class ceiling.
    pub envelope: [u8; CatalogHead::ENVELOPE_CAPACITY],
    /// Whether this head names a resolution generation.
    pub resolution_present: bool,
    /// The resolution generation, meaningful only with the flag set.
    pub resolution: GenerationId,
}

impl CardHeadFields {
    /// The fields of a decoded head entry, whether it came from a checkpoint or from a journal
    /// record's carried entry.
    pub fn of(head: &CatalogHead) -> Self {
        CardHeadFields {
            envelope_len: head.envelope_len,
            envelope: head.envelope,
            resolution_present: head.flags & CatalogHead::FLAG_RESOLUTION_PRESENT != 0,
            resolution: head.resolution,
        }
    }
}

/// The environment one materialization pass runs in: where card-resident fields come from, and
/// where the body's sectors go.
///
/// One trait rather than three, with one error type, because a pass has exactly one failure mode
/// worth distinguishing at this level — the card did not answer — and splitting it would make the
/// call site carry two error parameters for no decision anyone makes differently.
pub trait CheckpointPass {
    /// What a read of the active checkpoint or a sector write can fail with.
    type Error;

    /// The two card-resident fields of one head.
    ///
    /// The implementation resolves §6.3's newest source: the journal record at
    /// `entry.journal_slot` when the index names one, and the active checkpoint's stored bytes
    /// otherwise. Which of the two it was is not this pass's business — only that it is the newest.
    fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, Self::Error>;

    /// The 208 stored bytes of one terminal result.
    ///
    /// `physical` is the ring index the entry occupies in *both* checkpoints — the ring's start and
    /// count are index-resident, so a result does not move when the checkpoint is rewritten. The
    /// newest source is chosen exactly as it is for a head: from the journal record at
    /// `key.journal_slot` when the index names one, because a result appended since the active
    /// checkpoint exists nowhere else, and from the active checkpoint's ring otherwise.
    fn result_entry(
        &mut self,
        physical: usize,
        key: &ResultIndexEntry,
    ) -> Result<[u8; TerminalResult::LEN], Self::Error>;

    /// Writes one 512-byte sector of the body at `offset`.
    fn write_body_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), Self::Error>;
}

/// Why a materialization pass stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionError<E> {
    /// The pass environment failed: a bounded read of the active checkpoint, or a sector write.
    Media(E),
    /// A head's card-resident envelope length is not one §5.3 admits, so the entry it would produce
    /// is not a decodable head. Writing it would make the new checkpoint invalid at its first mount.
    Envelope {
        /// The head whose source returned it.
        head: HeadKey,
        /// The length the source gave.
        len: u16,
    },
    /// The card's stored result does not decode, or is not the operation the RAM ring names at that
    /// position. Either way the two sources disagree and the pass must not pick one.
    Result {
        /// The ring position.
        physical: usize,
    },
}

/// Runs §6.3's step 3: materializes the complete body through `pass`, returning its body CRC.
///
/// The CRC is the one §5 stores at body offset 65,020 and the one the gate of step 4 must carry;
/// it is stamped into the final sector before that sector is written, so the body the card holds is
/// already sealed when the pass returns.
///
/// The pass does **not** reset the index's journal-slot references. §6.3 makes them "meaningful only
/// within the selected epoch", and the epoch does not change until step 4's gate is durable — so
/// [`RamIndex::clear_journal_slots`](super::index::RamIndex::clear_journal_slots) belongs after that
/// gate, not here. Clearing them earlier would leave a cut between steps 3 and 4 mounting the old
/// checkpoint with an index that had forgotten where its newest head entries were.
pub fn materialize<P: CheckpointPass>(index: &RamIndex, pass: &mut P) -> Result<u32, CompactionError<P::Error>> {
    let mut emitter = Emitter::new(pass);
    let mut stage = [0u8; MAX_ENTRY_LEN];

    emitter.push(&index.header().encode())?;

    // Every region is emitted whole — its occupied prefix in key order, then its remaining entries
    // as zeros — because §5.1 requires exactly that and `validate_body` proves it at every mount.
    emitter.region(checkpoint::REPOSITORIES, index.repositories.len(), |emitter, slot| {
        stage[..super::entries::RepositoryState::LEN].copy_from_slice(&index.repositories[slot].encode());
        emitter.push(&stage[..super::entries::RepositoryState::LEN])
    })?;

    for slot in 0..checkpoint::HEADS.capacity {
        if slot >= index.heads.len() {
            emitter.zeros(checkpoint::HEADS.entry)?;
            continue;
        }
        let entry = &index.heads[slot];
        let fields = emitter.pass.head_fields(entry).map_err(CompactionError::Media)?;
        let head = compose_head(entry, &fields)?;
        stage[..CatalogHead::LEN].copy_from_slice(&head.encode());
        emitter.push(&stage[..CatalogHead::LEN])?;
    }

    emitter.region(checkpoint::ACTIVE, index.actives.len(), |emitter, slot| {
        stage[..super::entries::ActiveOperation::LEN].copy_from_slice(&index.actives[slot].encode());
        emitter.push(&stage[..super::entries::ActiveOperation::LEN])
    })?;

    emitter.region(checkpoint::DRAFT_PARENT, usize::from(index.draft_parent.is_some()), |emitter, _| {
        let parent = index.draft_parent.expect("the count said one is present");
        stage[..super::entries::DraftParent::LEN].copy_from_slice(&parent.encode());
        emitter.push(&stage[..super::entries::DraftParent::LEN])
    })?;

    emitter.region(checkpoint::DRAFT_PARTS, index.draft_parts.len(), |emitter, slot| {
        stage[..super::entries::DraftPart::LEN].copy_from_slice(&index.draft_parts[slot].encode());
        emitter.push(&stage[..super::entries::DraftPart::LEN])
    })?;

    emitter.region(checkpoint::RETAINED, index.retained.len(), |emitter, slot| {
        stage[..super::entries::RetainedPrevious::LEN].copy_from_slice(&index.retained[slot].encode());
        emitter.push(&stage[..super::entries::RetainedPrevious::LEN])
    })?;

    // §5.1's one exception: the result region is circular, so it is emitted in physical order and
    // the ring's start and count decide which positions are occupied.
    let mut occupant = [usize::MAX; MAX_TERMINAL_RESULTS];
    for step in 0..index.results.len() {
        occupant[(index.result_start + step) % MAX_TERMINAL_RESULTS] = step;
    }
    for (physical, &step) in occupant.iter().enumerate().take(checkpoint::RESULTS.capacity) {
        if step == usize::MAX {
            emitter.zeros(checkpoint::RESULTS.entry)?;
            continue;
        }
        let key = index.results[step];
        let bytes = emitter.pass.result_entry(physical, &key).map_err(CompactionError::Media)?;
        stage[..TerminalResult::LEN].copy_from_slice(&bytes);
        // The two sources must agree about what lives here. A card entry that decodes to another
        // operation is a mis-sourced read, and a checkpoint written from it would answer
        // `QueryOperation` with someone else's result.
        match TerminalResult::decode(&stage[..TerminalResult::LEN]) {
            Ok(stored) if stored.operation == key.operation && stored.commit_sequence == key.commit_sequence => {}
            _ => return Err(CompactionError::Result { physical }),
        }
        emitter.push(&stage[..TerminalResult::LEN])?;
    }

    emitter.region(checkpoint::HANDOFF, usize::from(index.handoff.is_some()), |emitter, _| {
        let handoff = index.handoff.expect("the count said one is present");
        stage[..super::limits::HANDOFF_REF_LEN].copy_from_slice(&handoff.encode());
        emitter.push(&stage[..super::limits::HANDOFF_REF_LEN])
    })?;

    emitter.region(checkpoint::WEATHER, usize::from(index.weather.is_some()), |emitter, _| {
        let weather = index.weather.expect("the count said one is present");
        stage[..super::entries::WeatherState::LEN].copy_from_slice(&weather.encode());
        emitter.push(&stage[..super::entries::WeatherState::LEN])
    })?;

    emitter.region(checkpoint::RIDE, usize::from(index.ride.is_some()), |emitter, _| {
        let ride = index.ride.expect("the count said one is present");
        stage[..super::entries::ActiveRide::LEN].copy_from_slice(&ride.encode());
        emitter.push(&stage[..super::entries::ActiveRide::LEN])
    })?;

    // The zero tail, and then the four bytes of the CRC field itself — emitted as zeros because §1
    // treats a CRC field as zero while its record is checksummed, and stamped by `finish`.
    emitter.zeros(checkpoint::TAIL.end - checkpoint::TAIL.start)?;
    emitter.zeros(CHECKPOINT_BODY_LEN - CHECKPOINT_BODY_CRC_OFFSET)?;
    emitter.finish()
}

/// The `O2CG` gate step 4 writes over a body this pass produced.
pub fn gate_for(index: &RamIndex, body_crc: u32, slot: u16) -> Gate {
    Gate { magic: super::gate::MAGIC_CHECKPOINT, slot, scope: index.epoch, sequence: index.through_sequence, body_crc }
}

fn compose_head<E>(entry: &HeadIndexEntry, fields: &CardHeadFields) -> Result<CatalogHead, CompactionError<E>> {
    if fields.envelope_len < CatalogHead::MIN_ENVELOPE || fields.envelope_len as usize > CatalogHead::ENVELOPE_CAPACITY
    {
        return Err(CompactionError::Envelope { head: entry.key(), len: fields.envelope_len });
    }
    let mut flags = entry.flags & !CatalogHead::FLAG_RESOLUTION_PRESENT;
    if fields.resolution_present {
        flags |= CatalogHead::FLAG_RESOLUTION_PRESENT;
    }
    Ok(CatalogHead {
        key: entry.key(),
        flags,
        revision: entry.revision,
        generation: entry.generation,
        length: entry.length,
        crc: entry.crc,
        envelope_len: fields.envelope_len,
        envelope: fields.envelope,
        resolution: if fields.resolution_present { fields.resolution } else { GenerationId::ZERO },
    })
}

/// The forward pass's whole state: one sector, its fill, and the running CRC.
struct Emitter<'p, P: CheckpointPass> {
    pass: &'p mut P,
    sector: [u8; SECTOR],
    filled: usize,
    base: usize,
    crc: obc_crc::Crc32,
}

impl<'p, P: CheckpointPass> Emitter<'p, P> {
    fn new(pass: &'p mut P) -> Self {
        Emitter { pass, sector: [0u8; SECTOR], filled: 0, base: 0, crc: obc_crc::Crc32::new() }
    }

    fn push(&mut self, mut bytes: &[u8]) -> Result<(), CompactionError<P::Error>> {
        while !bytes.is_empty() {
            let room = SECTOR - self.filled;
            let take = room.min(bytes.len());
            self.sector[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
            self.filled += take;
            bytes = &bytes[take..];
            self.maybe_flush()?;
        }
        Ok(())
    }

    fn zeros(&mut self, mut count: usize) -> Result<(), CompactionError<P::Error>> {
        while count > 0 {
            let room = SECTOR - self.filled;
            let take = room.min(count);
            self.sector[self.filled..self.filled + take].fill(0);
            self.filled += take;
            count -= take;
            self.maybe_flush()?;
        }
        Ok(())
    }

    /// Emits one region: its occupied prefix through `entry`, then its remaining entries as zeros.
    fn region(
        &mut self,
        region: Region,
        occupied: usize,
        mut entry: impl FnMut(&mut Self, usize) -> Result<(), CompactionError<P::Error>>,
    ) -> Result<(), CompactionError<P::Error>> {
        for slot in 0..region.capacity {
            if slot < occupied {
                entry(self, slot)?;
            } else {
                self.zeros(region.entry)?;
            }
        }
        Ok(())
    }

    /// Writes a full sector — unless it is the body's last one, which [`finish`](Self::finish) must
    /// keep staged so the CRC it accumulated can be stamped into it before it reaches the card.
    fn maybe_flush(&mut self) -> Result<(), CompactionError<P::Error>> {
        if self.filled < SECTOR || self.base + SECTOR >= CHECKPOINT_BODY_LEN {
            return Ok(());
        }
        self.crc.update(&self.sector);
        self.pass.write_body_sector(self.base, &self.sector).map_err(CompactionError::Media)?;
        self.base += SECTOR;
        self.filled = 0;
        Ok(())
    }

    /// Stamps the accumulated CRC into the final sector and writes it.
    fn finish(mut self) -> Result<u32, CompactionError<P::Error>> {
        debug_assert_eq!(self.base + self.filled, CHECKPOINT_BODY_LEN, "the pass emitted a body of the wrong length");
        // The CRC field lies in the final sector and was emitted as four zeros, which is exactly
        // what §1's "a CRC field is treated as zero while its containing record is checksummed"
        // asks for. So the accumulator is already the value, and the only thing left is to place it.
        self.crc.update(&self.sector[..self.filled]);
        let crc = self.crc.finalize();
        let hole = CHECKPOINT_BODY_CRC_OFFSET - self.base;
        put_u32(&mut self.sector, hole, crc);
        self.pass.write_body_sector(self.base, &self.sector).map_err(CompactionError::Media)?;
        Ok(crc)
    }
}

#[cfg(test)]
mod tests {
    use super::super::entries::{DraftPartState, RetainedPrevious};
    use super::super::index::NO_JOURNAL_SLOT;
    use super::super::journal::{Change, JournalBody};
    use super::super::model::CatalogModel;
    use super::super::samples;
    use super::*;
    use std::boxed::Box;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    /// A pass whose card is one materialized checkpoint body, plus the journal-carried head entries
    /// a replay produced.
    ///
    /// This is the shape the store has at compaction: an active checkpoint on the card, and up to
    /// 192 replayed records whose head entries are newer than it. Sourcing from both is what §6.3's
    /// per-head journal-slot reference exists for.
    struct ModelPass {
        active: Box<[u8; CHECKPOINT_BODY_LEN]>,
        journal: BTreeMap<u16, CatalogHead>,
        journal_results: BTreeMap<u16, TerminalResult>,
        out: Vec<u8>,
        sectors: Vec<usize>,
        head_reads: usize,
        result_reads: usize,
    }

    impl ModelPass {
        fn new(active: &CatalogModel) -> Self {
            let mut body = Box::new([0u8; CHECKPOINT_BODY_LEN]);
            active.encode_body(body.as_mut_slice()).expect("the active checkpoint encodes");
            ModelPass {
                active: body,
                journal: BTreeMap::new(),
                journal_results: BTreeMap::new(),
                out: std::vec![0u8; CHECKPOINT_BODY_LEN],
                sectors: Vec::new(),
                head_reads: 0,
                result_reads: 0,
            }
        }

        fn carry(&mut self, slot: u16, head: CatalogHead) {
            self.journal.insert(slot, head);
        }

        fn carry_result(&mut self, slot: u16, result: TerminalResult) {
            self.journal_results.insert(slot, result);
        }

        /// The head as the active checkpoint stores it, by its key.
        fn stored_head(&self, key: HeadKey) -> Option<CatalogHead> {
            let header = super::super::checkpoint::CheckpointHeader::decode(self.active.as_slice()).ok()?;
            (0..header.head_count as usize)
                .filter_map(|index| CatalogHead::decode(&self.active[checkpoint::HEADS.slot(index)]).ok())
                .find(|head| head.key == key)
        }
    }

    impl CheckpointPass for ModelPass {
        type Error = ();

        fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, ()> {
            self.head_reads += 1;
            // §6.3: the journal-carried entry wins when a head-putting record has been replayed
            // since the active checkpoint, and it is found through the reference rather than a scan.
            if entry.journal_slot != NO_JOURNAL_SLOT {
                let head = self.journal.get(&entry.journal_slot).ok_or(())?;
                return Ok(CardHeadFields::of(head));
            }
            self.stored_head(entry.key()).as_ref().map(CardHeadFields::of).ok_or(())
        }

        fn result_entry(&mut self, physical: usize, key: &ResultIndexEntry) -> Result<[u8; TerminalResult::LEN], ()> {
            self.result_reads += 1;
            if key.journal_slot != NO_JOURNAL_SLOT {
                let result = self.journal_results.get(&key.journal_slot).ok_or(())?;
                return Ok(result.encode());
            }
            let mut out = [0u8; TerminalResult::LEN];
            out.copy_from_slice(&self.active[checkpoint::RESULTS.slot(physical)]);
            Ok(out)
        }

        fn write_body_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), ()> {
            self.sectors.push(offset);
            self.out[offset..offset + SECTOR].copy_from_slice(sector);
            Ok(())
        }
    }

    fn expected(model: &CatalogModel) -> Box<[u8; CHECKPOINT_BODY_LEN]> {
        let mut body = Box::new([0u8; CHECKPOINT_BODY_LEN]);
        model.encode_body(body.as_mut_slice()).expect("the oracle encodes");
        body
    }

    /// The headline proof: the streamed body is byte-identical to what the reference model
    /// materializes in one shot.
    fn assert_streams_to_the_model(model: &CatalogModel, pass: &mut ModelPass) {
        let index = super::super::index::RamIndex::project(model);
        let crc = materialize(&index, pass).expect("the pass completes");
        let oracle = expected(model);
        assert_eq!(pass.out.as_slice(), oracle.as_slice(), "the streamed body differs from encode_body");
        assert_eq!(crc, checkpoint::body_crc(oracle.as_slice()), "the accumulated CRC is not the body's");
        // And the result is a checkpoint a mount would accept.
        checkpoint::validate_body(&pass.out).expect("the streamed body validates");
    }

    #[test]
    fn an_empty_store_streams_to_the_same_bytes() {
        let model = CatalogModel::initial(samples::STORE, 4);
        let mut pass = ModelPass::new(&model);
        assert_streams_to_the_model(&model, &mut pass);
    }

    /// The staging §6.3 bounds, as a measurement rather than a claim — and the one place the
    /// measurement and the prose disagree.
    #[test]
    fn the_pass_stages_one_entry_and_one_sector() {
        // §5.1's regions, entry size by entry size. The largest is the update-handoff projection.
        let regions = [
            checkpoint::REPOSITORIES,
            checkpoint::HEADS,
            checkpoint::ACTIVE,
            checkpoint::DRAFT_PARENT,
            checkpoint::DRAFT_PARTS,
            checkpoint::RETAINED,
            checkpoint::RESULTS,
            checkpoint::HANDOFF,
            checkpoint::WEATHER,
            checkpoint::RIDE,
        ];
        let largest = regions.iter().map(|region| region.entry).max().unwrap();
        assert_eq!(largest, MAX_ENTRY_LEN);
        assert_eq!(MAX_ENTRY_LEN, 240, "the largest entry shape is the 240-byte handoff projection");
        assert_eq!(SPEC_STATED_ENTRY_LEN, 208, "§6.3 and §13 both state 208, which is the terminal result");
        assert_eq!(
            MAX_ENTRY_LEN - SPEC_STATED_ENTRY_LEN,
            32,
            "§6.3's original staging bound was 32 bytes short of §5.1's largest region entry",
        );
        assert_eq!(STAGING_BYTES, 240 + 512);

        // The emitter's own state is the sector, its fill, the running offset and the CRC — no
        // buffer sized by a region, and nothing that grows with the number of heads.
        let live = core::mem::size_of::<Emitter<'_, ModelPass>>();
        assert!(live <= SECTOR + 64, "the emitter carries {live} bytes of state, which is more than one sector");
    }

    /// Every write is one sector, in ascending order, covering the body exactly once.
    #[test]
    fn the_pass_is_a_single_forward_pass_of_whole_sectors() {
        let model = CatalogModel::initial(samples::STORE, 4);
        let mut pass = ModelPass::new(&model);
        let index = super::super::index::RamIndex::project(&model);
        materialize(&index, &mut pass).unwrap();

        assert_eq!(pass.sectors.len(), CHECKPOINT_BODY_LEN / SECTOR, "the body is not a whole number of sectors");
        for (step, offset) in pass.sectors.iter().enumerate() {
            assert_eq!(*offset, step * SECTOR, "sector {step} was written out of order");
        }
    }

    /// A populated store: every region occupied, the ring wrapped, and all three singletons present.
    #[test]
    fn a_populated_projection_streams_to_the_same_bytes() {
        let mut model = CatalogModel::initial(samples::STORE, 4);
        for step in 1..=70u64 {
            let mut operation = samples::OP_A;
            operation[0] = step as u8;
            model.apply(&samples::claim(1, step * 2 - 1, 0, operation, model.next_generation + 1)).unwrap();
            model.apply(&samples::publish(1, step * 2, 0, operation, step, samples::head(1, step))).unwrap();
        }
        assert_eq!(model.results.len(), super::super::limits::MAX_TERMINAL_RESULTS, "the ring did not fill");
        assert!(model.result_start > 0, "the ring did not wrap, so physical order is not exercised");

        model.draft_parent = Some(samples::parent());
        let mut part = samples::part(1);
        part.state = DraftPartState::Sealed;
        let _ = model.draft_parts.push(part);
        let _ = model.retained.push(samples::retained(500));
        model.handoff = Some(samples::handoff_ref(4, super::super::handoff::HandoffPhase::Armed));
        model.weather = Some(samples::weather());
        model.ride = Some(samples::ride());

        let mut pass = ModelPass::new(&model);
        assert_streams_to_the_model(&model, &mut pass);
        assert_eq!(pass.head_reads, model.heads.len(), "one bounded read per head, and no more");
        assert_eq!(pass.result_reads, model.results.len(), "one bounded read per occupied ring entry");
    }

    /// §6.3's newest-source rule, which is the whole point of the journal-slot reference: a head
    /// whose envelope a replayed record changed must stream with the *record's* bytes, not the
    /// active checkpoint's.
    #[test]
    fn a_journal_carried_head_entry_wins_over_the_active_checkpoint() {
        // The active checkpoint: one published head, one result, and a second operation claimed.
        let mut active = CatalogModel::initial(samples::STORE, 4);
        active.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        active.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        active.apply(&samples::claim(1, 3, 2, samples::OP_B, 2)).unwrap();

        // One replayed record replaces that head with a manifest head — a different envelope and a
        // resolution generation the old entry did not have — and appends its own result. Neither of
        // those bytes exists in the active checkpoint.
        let mut newer = samples::manifest_head(7, 92);
        newer.key = samples::head(1, 7).key;
        newer.envelope_len = 24;
        newer.envelope[..24].copy_from_slice(&[0xA5; 24]);
        let record = samples::publish(1, 4, 3, samples::OP_B, 2, newer);
        let mut after = active.clone();
        after.apply(&record).unwrap();

        let mut pass = ModelPass::new(&active);
        pass.carry(3, newer);
        pass.carry_result(3, *after.results.last().unwrap());
        let mut index = super::super::index::RamIndex::project(&after);
        assert!(index.note_head_record(newer.key, 3));
        assert!(index.note_result_record(3));

        let crc = materialize(&index, &mut pass).expect("the pass completes");
        let oracle = expected(&after);
        assert_eq!(pass.out.as_slice(), oracle.as_slice(), "the pass took the stale checkpoint bytes");
        assert_eq!(crc, checkpoint::body_crc(oracle.as_slice()));
        checkpoint::validate_body(&pass.out).expect("the streamed body validates");

        // And the streamed head really carries the newer bytes, resolution flag included.
        let streamed = CatalogHead::decode(&pass.out[checkpoint::HEADS.slot(0)]).unwrap();
        assert_eq!(streamed.envelope_len, 24);
        assert_ne!(streamed.flags & CatalogHead::FLAG_RESOLUTION_PRESENT, 0);
        assert_eq!(streamed.resolution, GenerationId::new(92));
    }

    /// Randomized projections, streamed and compared byte for byte. This is the property the slice
    /// owes: for every state the model can reach, the two materializations agree.
    #[test]
    fn randomized_projections_stream_to_encode_body_byte_for_byte() {
        let mut rng = 0xC0FF_EE12_3456_789Au64;
        let mut next = || {
            rng ^= rng >> 12;
            rng ^= rng << 25;
            rng ^= rng >> 27;
            rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };

        for round in 0..24u64 {
            let mut model = CatalogModel::initial(samples::STORE, 4);
            let publications = 1 + next() % 80;
            for step in 1..=publications {
                let mut operation = samples::OP_A;
                operation[0] = step as u8;
                operation[1] = round as u8;
                model.apply(&samples::claim(1, step * 2 - 1, 0, operation, model.next_generation + 1)).unwrap();
                let kind = 1 + (next() % 3) as u16;
                let mut head = samples::head(kind, step);
                // A varying envelope, inside §5.3's `8..96` class ceiling.
                head.envelope_len = 8 + (next() % 89) as u16;
                for byte in head.envelope[..head.envelope_len as usize].iter_mut() {
                    *byte = next() as u8;
                }
                if next() % 4 == 0 {
                    head.flags |= CatalogHead::FLAG_RESOLUTION_PRESENT;
                    head.resolution = GenerationId::new(next() % 10_000);
                }
                model.apply(&samples::publish(1, step * 2, 0, operation, step, head)).unwrap();
            }
            if next() % 2 == 0 {
                model.draft_parent = Some(samples::parent());
                for key in 1..=(1 + next() % 8) {
                    let mut part = samples::part(key);
                    part.state = DraftPartState::Sealed;
                    let position = model.draft_parts.iter().position(|held| held.key.sort_key() > part.key.sort_key());
                    match position {
                        Some(index) => {
                            let _ = model.draft_parts.push(part);
                            let last = model.draft_parts.len() - 1;
                            model.draft_parts[index..=last].rotate_right(1);
                        }
                        None => {
                            let _ = model.draft_parts.push(part);
                        }
                    }
                }
            }
            for generation in 0..(next() % 5) {
                let mut entry = samples::retained(1_000 + generation);
                entry.reasons = RetainedPrevious::REASON_UPDATE_ROLLBACK;
                entry.lease_count = 0;
                let _ = model.retained.push(entry);
            }
            if next() % 3 == 0 {
                model.weather = Some(samples::weather());
            }
            if next() % 3 == 0 {
                model.ride = Some(samples::ride());
            }
            if next() % 3 == 0 {
                model.handoff = Some(samples::handoff_ref(4, super::super::handoff::HandoffPhase::Prepared));
            }

            let mut pass = ModelPass::new(&model);
            assert_streams_to_the_model(&model, &mut pass);
        }
    }

    /// A source that hands back an envelope length no head entry can hold must stop the pass. The
    /// alternative is a checkpoint that fails `validate_body` at its first mount, which is a
    /// recovery-failed store produced by a routine compaction.
    #[test]
    fn an_impossible_envelope_length_stops_the_pass() {
        struct BadEnvelope(ModelPass, u16);
        impl CheckpointPass for BadEnvelope {
            type Error = ();
            fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, ()> {
                let mut fields = self.0.head_fields(entry)?;
                fields.envelope_len = self.1;
                Ok(fields)
            }
            fn result_entry(&mut self, p: usize, k: &ResultIndexEntry) -> Result<[u8; TerminalResult::LEN], ()> {
                self.0.result_entry(p, k)
            }
            fn write_body_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), ()> {
                self.0.write_body_sector(offset, sector)
            }
        }

        let mut model = CatalogModel::initial(samples::STORE, 4);
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        let index = super::super::index::RamIndex::project(&model);
        let key = model.heads[0].key;

        for len in [0u16, 7, 97, 1_000] {
            let mut pass = BadEnvelope(ModelPass::new(&model), len);
            assert_eq!(materialize(&index, &mut pass), Err(CompactionError::Envelope { head: key, len }));
        }
    }

    /// And a result the card returns for the wrong ring position is a disagreement between two
    /// sources, not a choice.
    #[test]
    fn a_result_that_is_not_the_ring_positions_own_stops_the_pass() {
        struct SwappedResults(ModelPass);
        impl CheckpointPass for SwappedResults {
            type Error = ();
            fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, ()> {
                self.0.head_fields(entry)
            }
            fn result_entry(&mut self, physical: usize, k: &ResultIndexEntry) -> Result<[u8; TerminalResult::LEN], ()> {
                // Hand back the *next* position's bytes.
                self.0.result_entry((physical + 1) % MAX_TERMINAL_RESULTS, k)
            }
            fn write_body_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), ()> {
                self.0.write_body_sector(offset, sector)
            }
        }

        let mut model = CatalogModel::initial(samples::STORE, 4);
        for step in 1..=3u64 {
            let mut operation = samples::OP_A;
            operation[0] = step as u8;
            model.apply(&samples::claim(1, step * 2 - 1, 0, operation, model.next_generation + 1)).unwrap();
            model.apply(&samples::publish(1, step * 2, 0, operation, step, samples::head(1, step))).unwrap();
        }
        let index = super::super::index::RamIndex::project(&model);
        let mut pass = SwappedResults(ModelPass::new(&model));
        assert_eq!(materialize(&index, &mut pass), Err(CompactionError::Result { physical: 0 }));
    }

    /// A media failure at any sector stops the pass and is reported as itself.
    #[test]
    fn a_failing_sector_write_is_reported_rather_than_swallowed() {
        struct FailAt(ModelPass, usize);
        impl CheckpointPass for FailAt {
            type Error = usize;
            fn head_fields(&mut self, entry: &HeadIndexEntry) -> Result<CardHeadFields, usize> {
                self.0.head_fields(entry).map_err(|()| 0)
            }
            fn result_entry(&mut self, p: usize, k: &ResultIndexEntry) -> Result<[u8; TerminalResult::LEN], usize> {
                self.0.result_entry(p, k).map_err(|()| 0)
            }
            fn write_body_sector(&mut self, offset: usize, sector: &[u8; SECTOR]) -> Result<(), usize> {
                if offset == self.1 {
                    return Err(offset);
                }
                self.0.write_body_sector(offset, sector).map_err(|()| 0)
            }
        }

        let model = CatalogModel::initial(samples::STORE, 4);
        let index = super::super::index::RamIndex::project(&model);
        for sector in [0usize, 40 * SECTOR, CHECKPOINT_BODY_LEN - SECTOR] {
            let mut pass = FailAt(ModelPass::new(&model), sector);
            assert_eq!(materialize(&index, &mut pass), Err(CompactionError::Media(sector)));
        }
    }

    /// The journal-slot references survive the pass, because the epoch does not change until step
    /// 4's gate is durable and a cut before it mounts the old checkpoint again.
    #[test]
    fn the_pass_leaves_the_journal_slot_references_alone() {
        let mut model = CatalogModel::initial(samples::STORE, 4);
        model.apply(&samples::claim(1, 1, 0, samples::OP_A, 1)).unwrap();
        model.apply(&samples::publish(1, 2, 1, samples::OP_A, 1, samples::head(1, 7))).unwrap();
        let mut index = super::super::index::RamIndex::project(&model);
        let key = model.heads[0].key;
        assert!(index.note_head_record(key, 1));

        let mut pass = ModelPass::new(&model);
        pass.carry(1, model.heads[0]);
        materialize(&index, &mut pass).unwrap();
        assert_eq!(index.head(key).unwrap().journal_slot, 1);

        // Only the driver, after the gate, clears them.
        index.clear_journal_slots();
        assert_eq!(index.head(key).unwrap().journal_slot, NO_JOURNAL_SLOT);
    }

    /// The resident half of a head and its card-resident half recompose to exactly the entry they
    /// came from.
    ///
    /// That is what makes the two halves a decomposition rather than a lossy summary — and what lets
    /// one source implementation serve both this pass and §9's reachability walk, which needs the
    /// same fields for a different reason.
    #[test]
    fn a_head_and_its_card_fields_recompose_the_entry_they_came_from() {
        for head in [samples::head(1, 7), samples::manifest_head(3, 92)] {
            let entry = HeadIndexEntry::from_head(&head, NO_JOURNAL_SLOT);
            assert_eq!(compose_head::<()>(&entry, &CardHeadFields::of(&head)), Ok(head));
        }
    }

    /// The gate step 4 writes binds the body this pass produced: its epoch, its through-sequence and
    /// its CRC.
    #[test]
    fn the_gate_binds_the_body_the_pass_produced() {
        let mut model = CatalogModel::initial(samples::STORE, 4);
        model.epoch = 2;
        model
            .apply(&JournalBody {
                sequence: 1,
                epoch: 2,
                mutation: super::super::journal::Mutation {
                    retained: Some(Change::Put(samples::retained(9))),
                    ..Default::default()
                },
                kind: super::super::journal::RecordKind::Retention,
                operation: obc_link::ids::OperationId::ZERO,
                intent: [0u8; 32],
                ..samples::claim(2, 1, 0, samples::OP_A, 1)
            })
            .unwrap();

        let index = super::super::index::RamIndex::project(&model);
        let mut pass = ModelPass::new(&model);
        let crc = materialize(&index, &mut pass).unwrap();
        let gate = gate_for(&index, crc, 1);
        assert_eq!(gate, checkpoint::gate_for(&pass.out, 1));
    }
}
