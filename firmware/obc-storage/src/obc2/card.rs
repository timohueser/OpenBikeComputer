//! A whole simulated card, so a [`KernelTransaction`] can be mounted, cut, rebooted and remounted.
//!
//! [`KernelTransaction`]: super::transaction::KernelTransaction
//!
//! [`media::Media`](super::media) models one faulting file. This composes a store out of them — the
//! two alternating checkpoints, the commit journal, and a `GEN`/`WORK` pair per generation — and
//! implements the [`KernelMedia`] seam over the result. It is host-only and is the same medium the
//! crash matrix drives, which is the point: the equivalence suite and the crash cuts see one card.
//!
//! It is not the board's storage. The board's arrives with #1359 over the §13.1 FAT adapter, and it
//! implements the same two traits. What this owns beyond them is the ordering §1 fixes for a
//! journal append and §6.3 fixes for a checkpoint, because those are media disciplines rather than
//! transaction logic and a store that got them wrong would still typecheck.

use std::boxed::Box;
use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

use obc_link::ids::{GenerationId, StoreId};

use super::checkpoint;
use super::gate::INVALIDATED;
use super::generation::GenerationMedia;
use super::journal::JournalBody;
use super::limits::{
    CHECKPOINT_BODY_LEN, CHECKPOINT_FILE_LEN, CHECKPOINT_GATE_OFFSET, GATE_LEN, JOURNAL_BODY_LEN, JOURNAL_FILE_LEN,
    JOURNAL_GATE_OFFSET, JOURNAL_SLOTS, SLOT_STRIDE, WORK_FILE_LEN,
};
use super::media::{FileId, Media, MediaError};
use super::model::CatalogModel;
use super::recovery::{self, CheckpointObservation, Decision, SlotObservation};
use super::transaction::KernelMedia;

/// The bytes a simulated card holds for payloads. Large enough that no scenario meets it by
/// accident, small enough that an admission refusal can be provoked by declaring more.
pub const CAPACITY: u64 = 8 * 1024 * 1024;

/// One generation's two files.
#[derive(Debug, Clone, Copy)]
struct Pair {
    generation: GenerationId,
    payload: FileId,
    work: FileId,
}

/// A simulated OBC2 card.
pub struct Card {
    media: Media,
    cat: [FileId; 2],
    journal: FileId,
    /// Which checkpoint file the last compaction wrote, so the next one alternates.
    active_checkpoint: usize,
    /// The generation leaves this card has created, as a directory listing would report them.
    ///
    /// Cleared by [`reboot`](Self::reboot): it is a cache of what the medium already holds, and a
    /// real store rebuilds it by listing §3's shards. Every entry is re-derivable from the file
    /// names, which is what makes clearing it safe rather than merely tidy.
    pairs: Vec<Pair>,
    open: Option<Pair>,
    /// The shards `ensure_shards` has been asked to create, in call order.
    pub shards_created: Vec<super::names::ShardName>,
    /// Refuses the next `ensure_shards`, so §11's preflight can be failed at the one media act that
    /// happens before the claim record.
    pub fail_ensure_shards: bool,
}

impl Card {
    /// An initialized, empty store: every fixed file at full length and a first checkpoint holding
    /// §12's initial projection.
    pub fn initialize(seed: u64, store: StoreId) -> (Self, Box<CatalogModel>) {
        let mut media = Media::new(seed);
        let cat0 = media.create("CAT0.CHK", CHECKPOINT_FILE_LEN);
        let cat1 = media.create("CAT1.CHK", CHECKPOINT_FILE_LEN);
        let journal = media.create("COMMIT.JNL", JOURNAL_FILE_LEN);
        let mut card = Card {
            media,
            cat: [cat0, cat1],
            journal,
            active_checkpoint: 0,
            pairs: Vec::new(),
            open: None,
            shards_created: Vec::new(),
            fail_ensure_shards: false,
        };
        let mut model = Box::new(CatalogModel::empty(store));
        model.reset_to_initial(store, obc_link::registry::ObjectKind::Weather.to_u16());
        card.install_checkpoint(0, &model);
        (card, model)
    }

    /// The medium underneath, for a harness that arms a cut or reboots.
    pub fn media_mut(&mut self) -> &mut Media {
        &mut self.media
    }

    /// The medium underneath.
    pub fn media(&self) -> &Media {
        &self.media
    }

    /// Cuts power and drops everything no sync made durable — including this card's own RAM.
    ///
    /// §12 mounts a store from what the medium holds, so a harness that kept a resident map of
    /// generation leaves across a reboot would let a test see something the card never had to
    /// prove. The map is cleared and re-derived from the file names on demand.
    ///
    /// One fidelity note. The medium has no `delete`, so [`collect_generation`] empties a leaf
    /// rather than removing it: after a reboot a collected generation is a zero-length file rather
    /// than an absent one. Nothing here asserts the difference — a reader of either observes no
    /// bytes — and the enumeration that would tell them apart is the collector's, which is #1359's.
    ///
    /// [`collect_generation`]: KernelMedia::collect_generation
    pub fn reboot(&mut self) {
        self.media.reboot();
        self.open = None;
        self.pairs.clear();
    }

    /// Mounts the card the way §6.3 says to: validate both checkpoints, validate all 256 journal
    /// slots, decide, then replay the chosen suffix onto the chosen checkpoint.
    ///
    /// Every byte comes through [`Media::read_at`], so the read path crosses the medium exactly as
    /// the write path does rather than peeking at a durable image the card would not hand out.
    pub fn mount(&mut self) -> Result<Box<CatalogModel>, MountFailure> {
        let mut checkpoints = [None, None];
        let mut models: [Option<Box<CatalogModel>>; 2] = [None, None];
        for index in 0..2 {
            let Ok(image) = self.media.read_at(self.cat[index], 0, CHECKPOINT_FILE_LEN) else { continue };
            let Ok(header) = checkpoint::validate_file(&image, index as u16) else { continue };
            checkpoints[index] = Some(CheckpointObservation {
                store: header.store,
                epoch: header.epoch,
                through_sequence: header.through_sequence,
                next_generation: header.next_generation,
                body_crc: checkpoint::body_crc(&image[..CHECKPOINT_BODY_LEN]),
            });
            match CatalogModel::decode_body(&image[..CHECKPOINT_BODY_LEN]) {
                Ok(model) => models[index] = Some(model),
                // `validate_file` already proved the body; a decode that disagrees is a codec bug.
                Err(error) => panic!("checkpoint {index} validated but did not decode: {error:?}"),
            }
        }

        let mut observations: Vec<Option<SlotObservation>> = vec![None; JOURNAL_SLOTS];
        let mut bodies: Vec<Option<JournalBody>> = vec![None; JOURNAL_SLOTS];
        for slot in 0..JOURNAL_SLOTS {
            let Ok(stride) = self.media.read_at(self.journal, slot * SLOT_STRIDE, SLOT_STRIDE) else { continue };
            if let Ok(body) = JournalBody::validate_slot(&stride, slot as u16) {
                observations[slot] =
                    Some(SlotObservation { store: body.store, epoch: body.epoch, sequence: body.sequence });
                bodies[slot] = Some(body);
            }
        }

        let (checkpoint, replay) = match recovery::choose(&checkpoints, &observations) {
            Decision::NoCheckpoint => return Err(MountFailure::NoCheckpoint),
            Decision::Fail(fault) => return Err(MountFailure::FailClosed(fault)),
            Decision::Mount { checkpoint, replay } => (checkpoint, replay),
            Decision::MountReadOnly { checkpoint, replay, .. } => (checkpoint, replay),
        };
        self.active_checkpoint = checkpoint;
        let mut model = models[checkpoint].clone().expect("the selected checkpoint decoded");
        for body in bodies.iter().take(replay) {
            let record = body.as_ref().expect("a replayed slot is a valid record");
            // §6.3 chose this suffix because every record in it applies; one that does not means
            // the decision and the projection disagree, and that is a finding.
            assert_eq!(model.apply(record), Ok(()), "a record of the chosen suffix did not apply");
        }
        Ok(model)
    }

    /// Writes one gated checkpoint in §6.3's order: invalidate, body, sync, gate, sync.
    pub fn write_checkpoint(&mut self, index: usize, model: &CatalogModel) -> Result<(), MediaError> {
        let file = self.cat[index];
        self.write_all(file, CHECKPOINT_GATE_OFFSET, &INVALIDATED)?;
        self.media.sync(file)?;
        let mut body = Box::new([0u8; CHECKPOINT_BODY_LEN]);
        model.encode_body(body.as_mut_slice()).expect("the projection encodes");
        self.write_all(file, 0, body.as_slice())?;
        self.media.sync(file)?;
        let gate = checkpoint::gate_for(body.as_slice(), index as u16);
        self.write_all(file, CHECKPOINT_GATE_OFFSET, &gate.encode())?;
        self.media.sync(file)?;
        self.active_checkpoint = index;
        Ok(())
    }

    /// Installs a checkpoint without counting media operations — harness setup only.
    fn install_checkpoint(&mut self, index: usize, model: &CatalogModel) {
        let mut body = Box::new([0u8; CHECKPOINT_BODY_LEN]);
        model.encode_body(body.as_mut_slice()).expect("the projection encodes");
        let gate = checkpoint::gate_for(body.as_slice(), index as u16);
        self.media.install(self.cat[index], 0, body.as_slice());
        self.media.install(self.cat[index], CHECKPOINT_GATE_OFFSET, &gate.encode());
        self.active_checkpoint = index;
    }

    fn write_all(&mut self, file: FileId, offset: usize, bytes: &[u8]) -> Result<(), MediaError> {
        // §13.1's write completeness: "a short write is an error, never a success".
        let written = self.media.write_at(file, offset, bytes)?;
        if written == bytes.len() {
            Ok(())
        } else {
            Err(MediaError::Full)
        }
    }

    /// This generation's two files, **creating them** if they are not there.
    ///
    /// Only the two calls that are allowed to make a generation exist — `ensure_shards` and
    /// `open_generation` — go through here. A read or a collection must not: creating a leaf
    /// because somebody asked about it would turn "no such generation" into "an empty one", and the
    /// difference is the whole of §9's answer to a reader that outlived its bytes.
    fn create_pair(&mut self, generation: GenerationId) -> Pair {
        if let Some(pair) = self.lookup_pair(generation) {
            return pair;
        }
        let payload = self.media.create_payload(&payload_name(generation));
        let work = self.media.create(&work_name(generation), WORK_FILE_LEN);
        let pair = Pair { generation, payload, work };
        self.pairs.push(pair);
        pair
    }

    /// This generation's two files, if the medium already holds them.
    fn lookup_pair(&mut self, generation: GenerationId) -> Option<Pair> {
        if let Some(pair) = self.pairs.iter().find(|pair| pair.generation == generation) {
            return Some(*pair);
        }
        // Not in the resident map: it may still be on the card — a reboot clears the map, and a
        // mount re-derives it from §3's names, which is exactly this lookup.
        let payload = self.media.file(&payload_name(generation)).ok()?;
        let work = self.media.file(&work_name(generation)).ok()?;
        let pair = Pair { generation, payload, work };
        self.pairs.push(pair);
        Some(pair)
    }

    fn open_pair(&mut self) -> Result<Pair, MediaError> {
        self.open.ok_or(MediaError::NoSuchFile)
    }
}

fn payload_name(generation: GenerationId) -> String {
    format!("GEN.{:016X}", generation.get())
}

fn work_name(generation: GenerationId) -> String {
    format!("WORK.{:016X}", generation.get())
}

/// Why a mount produced no store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountFailure {
    /// No valid checkpoint: an uninitialized or unreadable card.
    NoCheckpoint,
    /// §6.3's fail-closed classification.
    FailClosed(recovery::FailClosed),
}

impl GenerationMedia for Card {
    type Error = MediaError;

    fn ensure_shards(&mut self, generation: GenerationId) -> Result<(), MediaError> {
        if self.fail_ensure_shards {
            return Err(MediaError::Full);
        }
        // §12's lazy shards, recorded as the crash harness records them: the directory the leaf
        // lands in, created on first use and idempotent afterwards.
        let shard = super::names::LeafName::of(generation).shard;
        if !self.shards_created.contains(&shard) {
            self.shards_created.push(shard);
        }
        let pair = self.create_pair(generation);
        self.open = Some(pair);
        Ok(())
    }

    fn payload_length(&mut self) -> Result<u64, MediaError> {
        let pair = self.open_pair()?;
        Ok(self.media.len(pair.payload) as u64)
    }

    fn write_payload(&mut self, offset: u64, bytes: &[u8]) -> Result<(), MediaError> {
        let pair = self.open_pair()?;
        self.write_all(pair.payload, offset as usize, bytes)
    }

    fn sync_payload(&mut self) -> Result<(), MediaError> {
        let pair = self.open_pair()?;
        self.media.sync(pair.payload)
    }

    fn truncate_payload(&mut self) -> Result<(), MediaError> {
        let pair = self.open_pair()?;
        self.media.truncate(pair.payload)
    }

    fn write_work(&mut self, offset: usize, bytes: &[u8]) -> Result<(), MediaError> {
        let pair = self.open_pair()?;
        self.write_all(pair.work, offset, bytes)
    }

    fn sync_work(&mut self) -> Result<(), MediaError> {
        let pair = self.open_pair()?;
        self.media.sync(pair.work)
    }
}

impl KernelMedia for Card {
    fn append_journal(
        &mut self,
        slot: u16,
        body: &[u8; JOURNAL_BODY_LEN],
        gate: &[u8; GATE_LEN],
    ) -> Result<(), MediaError> {
        // §1's journal exemption: body-then-gate with no preceding invalidation, because every slot
        // of an earlier epoch is already inert against the selected checkpoint. The body write
        // covers the whole stride with the gate sector zeroed, so a slot that was torn once can be
        // made valid again and no old gate is ever presented over a new body.
        let base = slot as usize * SLOT_STRIDE;
        let mut stride = vec![0u8; SLOT_STRIDE];
        stride[..JOURNAL_BODY_LEN].copy_from_slice(body);
        self.write_all(self.journal, base, &stride)?;
        self.media.sync(self.journal)?;
        self.write_all(self.journal, base + JOURNAL_GATE_OFFSET, gate)?;
        self.media.sync(self.journal)
    }

    fn open_generation(&mut self, generation: GenerationId) -> Result<(), MediaError> {
        let pair = self.create_pair(generation);
        self.open = Some(pair);
        Ok(())
    }

    fn read_generation(&mut self, generation: GenerationId, offset: u64, into: &mut [u8]) -> Result<usize, MediaError> {
        // §9: a generation nothing names is gone, and a read of one is not an empty read — it is a
        // read of a file that is not there. Resolving it would hide exactly the mistake a lease
        // exists to prevent.
        let pair = self.lookup_pair(generation).ok_or(MediaError::NoSuchFile)?;
        let len = self.media.len(pair.payload);
        let start = (offset as usize).min(len);
        let take = into.len().min(len - start);
        let bytes = self.media.read_at(pair.payload, start, take)?;
        into[..take].copy_from_slice(&bytes);
        Ok(take)
    }

    fn collect_generation(&mut self, generation: GenerationId) -> Result<(), MediaError> {
        // §9: "Deleting an unreachable GEN/WORK pair may be interrupted at either file; both
        // orderings recover as harmless orphan cleanup because no catalog fact points to it." A
        // generation that was never created is already collected.
        let Some(pair) = self.lookup_pair(generation) else { return Ok(()) };
        // The medium has no `delete`, so **both** files are emptied — the pair, not half of it,
        // because a `WORK` slot left behind would still validate and §7's recovery reads it.
        let _ = self.media.truncate(pair.payload);
        let blank = vec![0u8; WORK_FILE_LEN];
        let _ = self.media.write_at(pair.work, 0, &blank);
        let _ = self.media.sync(pair.work);
        self.pairs.retain(|entry| entry.generation != generation);
        if self.open.is_some_and(|open| open.generation == generation) {
            self.open = None;
        }
        Ok(())
    }

    fn free_bytes(&mut self) -> u64 {
        let used: u64 = self.pairs.iter().map(|pair| self.media.len(pair.payload) as u64).sum();
        CAPACITY.saturating_sub(used)
    }

    fn reset_store(&mut self, store: StoreId) -> Result<(), MediaError> {
        // §16: reset destroys every object, result and lease. The fixed files are rewritten and
        // every generation's bytes go with them.
        for pair in core::mem::take(&mut self.pairs) {
            let _ = self.media.truncate(pair.payload);
        }
        self.open = None;
        let mut model = Box::new(CatalogModel::empty(store));
        model.reset_to_initial(store, obc_link::registry::ObjectKind::Weather.to_u16());
        let blank = vec![0u8; SLOT_STRIDE];
        for slot in 0..JOURNAL_SLOTS {
            self.write_all(self.journal, slot * SLOT_STRIDE, &blank)?;
        }
        self.media.sync(self.journal)?;
        self.write_checkpoint(0, &model)?;
        // The second checkpoint must not survive as an older store's valid gate.
        self.write_all(self.cat[1], CHECKPOINT_GATE_OFFSET, &INVALIDATED)?;
        self.media.sync(self.cat[1])
    }
}
