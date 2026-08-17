//! The store: mount (`FLAT_Store_Format.md` §5.6), initialization (§8), the alternating commit
//! (§5.5), the ride journal's write half (§7.2) and the five seam operations
//! (`FLAT_Store_Protocol.md` §2).
//!
//! Resident state is the 8 KiB free bitmap, a handful of rows, and nothing else: the entry array
//! lives on the card and is read block by block, which is what makes a lookup nine block reads and
//! a mount a fixed cost. Every buffer here is fixed and on the stack — no allocation, on the device
//! or on the host.

use core::cell::RefCell;

use obc_crc::Crc32;

use super::bitmap::FreeMap;
use super::catalog::{Entry, Gate, Header, Structure, INVALIDATED};
use super::device::BlockDevice;
use super::error::StoreError;
use super::journal::{self, Slot, TAIL_CAPACITY, ZERO_PAD};
use super::layout::{
    catalog_gate, extent_count, extents_for, slot_block, Ranges, BLOCK, CATALOG, ENTRIES_PER_BLOCK, ENTRY_CAPACITY,
    ENTRY_STRIDE, PROGRAM_PAGE, SLOTS, SLOT_BLOCKS, SUPERBLOCK,
};
use super::seam::{
    Allocation, EntryFlags, EntryMeta, Mutation, ObjectId, PutSource, Revision, RideCheckpoint, Store, StoreId,
};
use super::superblock::Superblock;

/// Entry mutations one commit carries. Two is what the largest real batch needs — publish the new
/// head and retain or remove the displaced one — and four leaves margin without making the plan
/// arrays interesting.
pub const MAX_BATCH: usize = 4;
/// Reservations live at once: one transfer (`FLAT_Store_Protocol.md` §1) plus the ride reserve a
/// start allocates while one is in flight.
pub const MAX_RESERVATIONS: usize = 2;
/// Open objects at once: the eleven map shards a rendered set mounts, plus one transfer.
pub const MAX_OPEN_OBJECTS: usize = 12;

/// Why a mounted store refuses writes. The wire's `readOnly` details (`FLAT_Store_Protocol.md` §3.9)
/// are these, and a store that mounted read-only never becomes writable without initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReadWrite,
    /// An object reached `Revision` `u64::MAX`. Reads are still served (§3.9).
    RevisionSpaceExhausted,
    /// No gate is well-formed, or no candidate body validated. Evidence preserved, no repair.
    CatalogUnreadable,
    /// §5.6 step 1 classified the card as not a flat store. Initialization is the only transition.
    Unformatted,
    /// The card is smaller than the superblock recorded: damaged or swapped, never silently
    /// truncated (§4).
    CardTooSmall,
}

impl Mode {
    /// True when a commit may run.
    pub fn writable(self) -> bool {
        self == Mode::ReadWrite
    }

    /// True when the catalog is usable. Only the exhausted case still serves reads.
    pub fn readable(self) -> bool {
        matches!(self, Mode::ReadWrite | Mode::RevisionSpaceExhausted)
    }
}

/// An open object. Keeps reading the revision it resolved even across a commit that replaces or
/// removes it, until it is closed.
#[derive(Debug, PartialEq, Eq)]
pub struct Handle {
    slot: u8,
    id: ObjectId,
    revision: Revision,
}

impl Handle {
    pub fn id(&self) -> ObjectId {
        self.id
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }
}

/// What a ride recovery found (§7.3). The payload CRC is the seed the resumed session continues its
/// running CRC from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideRecovery {
    pub id: ObjectId,
    pub revision: Revision,
    /// The checkpoint the store recovered. Recording resumes at this plus one.
    pub checkpoint_sequence: u64,
    /// Payload bytes already in the ride's own extents.
    pub flushed: u64,
    pub tail_len: u32,
    pub payload_crc: u32,
    /// The physical slot it came from, which is `checkpoint_sequence mod 16`.
    pub(super) slot: u16,
}

impl RideRecovery {
    /// The ride's payload length at the recovered checkpoint.
    pub fn payload_len(&self) -> u64 {
        self.flushed + self.tail_len as u64
    }
}

#[derive(Debug, Clone, Copy)]
struct Hold {
    id: ObjectId,
    revision: Revision,
    ranges: Ranges,
    payload_len: u64,
    readers: u16,
    /// A commit removed this entry while it was open; its extents go back to the allocator when the
    /// last reader closes.
    orphaned: bool,
}

#[derive(Debug, Clone, Copy)]
struct Reservation {
    nonce: u32,
    ranges: Ranges,
    reserved: u64,
    written: u64,
    /// The partial block a write left behind, flushed by the commit that publishes it.
    staging: [u8; BLOCK],
}

#[derive(Debug, Clone, Copy)]
struct RideState {
    id: ObjectId,
    revision: Revision,
    ranges: Ranges,
    flushed: u64,
    next_sequence: u64,
}

/// The flat card store.
pub struct FlatStore<D> {
    dev: D,
    store: StoreId,
    extents: u32,
    mode: Mode,
    /// The copy the store is currently serving. §5.5's commit targets the other one.
    serving: usize,
    sequence: u64,
    /// The greatest commit sequence any **well-formed** gate carried, which is what a commit
    /// continues from — not the sequence of the copy that happened to validate.
    high_water: u64,
    next_object: u64,
    entry_count: u16,
    free: FreeMap,
    holds: RefCell<[Option<Hold>; MAX_OPEN_OBJECTS]>,
    reservations: [Option<Reservation>; MAX_RESERVATIONS],
    nonce: u32,
    ride: Option<RideState>,
    recovered: Option<RideRecovery>,
}

fn read_blocks<D: BlockDevice>(dev: &D, lba: u64, buf: &mut [u8]) -> Result<(), StoreError> {
    dev.read(lba, buf).map_err(|_| StoreError::Media)
}

fn write_blocks<D: BlockDevice>(dev: &D, lba: u64, buf: &[u8]) -> Result<(), StoreError> {
    dev.write(lba, buf).map_err(|_| StoreError::Media)
}

fn sync<D: BlockDevice>(dev: &D) -> Result<(), StoreError> {
    dev.sync().map_err(|_| StoreError::Media)
}

/// Reads the entry array of one catalog copy, one block at a time.
struct EntryReader<'a, D> {
    dev: &'a D,
    base: u64,
    extents: u32,
    cached: Option<u64>,
    buf: [u8; BLOCK],
}

impl<'a, D: BlockDevice> EntryReader<'a, D> {
    fn new(dev: &'a D, copy: usize, extents: u32) -> Self {
        EntryReader { dev, base: CATALOG[copy] + 1, extents, cached: None, buf: [0; BLOCK] }
    }

    fn get(&mut self, index: u16) -> Result<Entry, StoreError> {
        let block = index as u64 / ENTRIES_PER_BLOCK as u64;
        if self.cached != Some(block) {
            read_blocks(self.dev, self.base + block, &mut self.buf)?;
            self.cached = Some(block);
        }
        let at = index as usize % ENTRIES_PER_BLOCK * ENTRY_STRIDE;
        Entry::decode(&self.buf[at..at + ENTRY_STRIDE], self.extents).map_err(|_| StoreError::Invalid)
    }
}

/// Writes a catalog body: the entries stream through it, and it folds them into the body CRC the
/// gate will carry.
struct BodyWriter<'a, D> {
    dev: &'a D,
    block: u64,
    buf: [u8; BLOCK],
    filled: usize,
    digest: Crc32,
}

impl<'a, D: BlockDevice> BodyWriter<'a, D> {
    fn push(&mut self, entry: &Entry) -> Result<(), StoreError> {
        let bytes = entry.encode();
        self.digest.update(&bytes);
        self.buf[self.filled..self.filled + ENTRY_STRIDE].copy_from_slice(&bytes);
        self.filled += ENTRY_STRIDE;
        if self.filled == BLOCK {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), StoreError> {
        // The bytes past the live prefix are whatever an earlier commit left there: nothing reads
        // them and no CRC covers them, so zeroing the tail of the last block is a convenience.
        self.buf[self.filled..].fill(0);
        write_blocks(self.dev, self.block, &self.buf)?;
        self.block += 1;
        self.filled = 0;
        Ok(())
    }

    fn finish(mut self) -> Result<u32, StoreError> {
        if self.filled > 0 {
            self.flush()?;
        }
        Ok(self.digest.finalize())
    }
}

/// One resolved mutation: what it will write, what it displaces, and what it gives back.
#[derive(Debug, Clone, Copy)]
struct Resolved {
    key: (ObjectId, Revision),
    /// The entry to write, or `None` for a removal.
    entry: Option<Entry>,
    /// True when this key is not in the catalog yet.
    creates: bool,
    /// Extents this mutation releases at the gate: a removed entry's, or a trimmed reserve's tail.
    freed: Ranges,
    /// The reservation this mutation consumes.
    reservation: Option<u8>,
}

impl<D: BlockDevice> FlatStore<D> {
    /// §5.6: superblock, gates, one body, the free bitmap, and the ride journal only when an entry
    /// says a ride was recording. There is no journal replay, no garbage collection and no recovery
    /// scan, so those five steps are the whole of mount.
    ///
    /// A card this cannot bring up mounts read-only rather than failing to exist: the seam has to be
    /// able to answer `readOnly`, and initialization is the only transition into this format.
    pub fn mount(dev: D) -> Self {
        let mut store = FlatStore {
            dev,
            store: StoreId([0; 16]),
            extents: 0,
            mode: Mode::Unformatted,
            serving: 0,
            sequence: 0,
            high_water: 0,
            next_object: 0,
            entry_count: 0,
            free: FreeMap::default(),
            holds: RefCell::new([None; MAX_OPEN_OBJECTS]),
            reservations: [None; MAX_RESERVATIONS],
            nonce: 0,
            ride: None,
            recovered: None,
        };
        store.bring_up();
        store
    }

    /// §8: explicit, destructive, and the only transition into this format. The superblocks are
    /// destroyed first and written last, so a valid superblock implies a valid catalog
    /// unconditionally.
    pub fn initialize(dev: D, store: StoreId) -> Result<Self, StoreError> {
        let total_blocks = dev.block_count().map_err(|_| StoreError::Media)?;
        for copy in SUPERBLOCK {
            write_blocks(&dev, copy, &INVALIDATED)?;
        }
        sync(&dev)?;

        write_blocks(&dev, catalog_gate(1), &INVALIDATED)?;
        sync(&dev)?;

        for slot in 0..SLOTS {
            write_blocks(&dev, slot_block(slot), &INVALIDATED)?;
        }
        sync(&dev)?;

        let header = Header { store, sequence: 1, next_object: 1, entry_count: 0 };
        let body = header.encode();
        write_blocks(&dev, CATALOG[0], &body)?;
        sync(&dev)?;
        let gate = Gate { copy: 0, store, sequence: 1, entry_count: 0, body_crc: super::raw::crc32(&body) };
        write_blocks(&dev, catalog_gate(0), &gate.encode())?;
        sync(&dev)?;

        let superblock = Superblock { store, total_blocks }.encode();
        for copy in SUPERBLOCK {
            write_blocks(&dev, copy, &superblock)?;
        }
        sync(&dev)?;

        let store = Self::mount(dev);
        if store.mode.writable() {
            Ok(store)
        } else {
            Err(StoreError::Media)
        }
    }

    /// Why this store refuses writes, if it does.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The card's identity. A client that has not seen it must treat its whole cache as void.
    pub fn store_id(&self) -> StoreId {
        self.store
    }

    /// The catalog commit sequence — the staleness hint a client compares its listing against.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Entries the catalog holds.
    pub fn entry_count(&self) -> u16 {
        self.entry_count
    }

    /// The copy the store is serving. §5.5's next commit targets the other one.
    pub fn serving_copy(&self) -> usize {
        self.serving
    }

    /// Free extents, each 1 MiB.
    pub fn free_extents(&self) -> u32 {
        self.free.free()
    }

    /// The next `ObjectId` the cursor will hand out. A create names this in its `Put`; the commit
    /// advances the cursor past it and never rewinds.
    pub fn next_object_id(&self) -> ObjectId {
        ObjectId(self.next_object)
    }

    /// What §7.3 recovered, if a ride was recording when the card lost power.
    pub fn recovered_ride(&self) -> Option<RideRecovery> {
        self.recovered
    }

    /// Copies the recovered tail into `buf`. The bytes are the ride's payload past `flushed`.
    pub fn recovered_tail(&self, buf: &mut [u8]) -> Result<usize, StoreError> {
        let Some(recovered) = self.recovered else { return Err(StoreError::NotFound) };
        let want = recovered.tail_len as usize;
        if buf.len() < want {
            return Err(StoreError::Invalid);
        }
        let base = slot_block(recovered.slot as usize) + 1;
        let mut block = [0u8; BLOCK];
        let mut done = 0;
        while done < want {
            read_blocks(&self.dev, base + (done / BLOCK) as u64, &mut block)?;
            let take = (want - done).min(BLOCK);
            buf[done..done + take].copy_from_slice(&block[..take]);
            done += take;
        }
        Ok(want)
    }

    /// Releases a reservation without publishing it. The bytes written into it are unreachable and
    /// their extents are free again immediately — the same state the next mount would compute.
    pub fn cancel(&mut self, allocation: Allocation) {
        if let Some(row) = self.row(&allocation) {
            let ranges = row.ranges;
            self.reservations[allocation.slot as usize] = None;
            self.free.release(&ranges);
        }
    }

    /// Closes an open object. When the last reader of an entry a commit removed lets go, its extents
    /// return to the allocator.
    pub fn close(&mut self, handle: Handle) {
        let mut holds = self.holds.borrow_mut();
        let Some(hold) = holds[handle.slot as usize] else { return };
        if (hold.id, hold.revision) != (handle.id, handle.revision) {
            return;
        }
        if hold.readers > 1 {
            holds[handle.slot as usize] = Some(Hold { readers: hold.readers - 1, ..hold });
            return;
        }
        holds[handle.slot as usize] = None;
        drop(holds);
        if hold.orphaned {
            self.free.release(&hold.ranges);
        }
    }

    /// The device, for a bench or a harness that needs the card underneath.
    pub fn device(&self) -> &D {
        &self.dev
    }

    fn bring_up(&mut self) {
        let mut block = [0u8; BLOCK];
        let mut superblock = None;
        for copy in SUPERBLOCK {
            if read_blocks(&self.dev, copy, &mut block).is_ok() {
                if let Ok(decoded) = Superblock::decode(&block) {
                    superblock = Some(decoded);
                    break;
                }
            }
        }
        let Some(superblock) = superblock else { return };
        self.store = superblock.store;
        self.extents = extent_count(superblock.total_blocks);
        match self.dev.block_count() {
            Ok(observed) if observed >= superblock.total_blocks => {}
            Ok(_) => {
                self.mode = Mode::CardTooSmall;
                return;
            }
            Err(_) => {
                self.mode = Mode::CatalogUnreadable;
                return;
            }
        }
        self.mode = Mode::CatalogUnreadable;

        // Two gate reads decide which copy to try and where the sequence continues from. Only
        // well-formed gates contribute, so garbage in a dead gate's sequence field cannot poison the
        // high-water mark.
        let mut gates: [Option<Gate>; 2] = [None, None];
        for (copy, gate) in gates.iter_mut().enumerate() {
            if read_blocks(&self.dev, catalog_gate(copy), &mut block).is_ok() {
                *gate = Gate::decode(&block, copy, &self.store).ok();
            }
        }
        self.high_water = gates.iter().flatten().map(|gate| gate.sequence).max().unwrap_or(0);
        let order: [usize; 2] = match (gates[0], gates[1]) {
            (Some(a), Some(b)) if a.sequence == b.sequence => return,
            (Some(a), Some(b)) if b.sequence > a.sequence => [1, 0],
            _ => [0, 1],
        };

        for copy in order {
            let Some(gate) = gates[copy] else { continue };
            if let Ok(loaded) = self.load(copy, &gate) {
                self.serving = copy;
                self.sequence = gate.sequence;
                self.next_object = loaded.next_object;
                self.entry_count = gate.entry_count;
                self.mode = if loaded.exhausted { Mode::RevisionSpaceExhausted } else { Mode::ReadWrite };
                if let Some(recording) = loaded.recording {
                    self.recover_ride(&recording);
                }
                return;
            }
        }
    }

    /// §5.6 step 3 and 4 for one copy: the body CRC, every structural rule of §5.3, and the free
    /// bitmap built from the ranges as they go past. A failure leaves the caller free to try the
    /// next candidate — the bitmap is rebuilt from scratch each attempt.
    fn load(&mut self, copy: usize, gate: &Gate) -> Result<Loaded, StoreError> {
        self.free.reset(self.extents);
        let mut block = [0u8; BLOCK];
        read_blocks(&self.dev, CATALOG[copy], &mut block)?;
        let header = Header::decode(&block, &self.store).map_err(|_| StoreError::Invalid)?;
        if header.entry_count != gate.entry_count || header.sequence != gate.sequence {
            return Err(StoreError::Invalid);
        }
        let mut digest = Crc32::new();
        digest.update(&block);

        let mut structure = Structure::default();
        let mut loaded = Loaded { next_object: header.next_object, recording: None, exhausted: false };
        let mut done = 0usize;
        while done < header.entry_count as usize {
            read_blocks(&self.dev, CATALOG[copy] + 1 + (done / ENTRIES_PER_BLOCK) as u64, &mut block)?;
            let count = (header.entry_count as usize - done).min(ENTRIES_PER_BLOCK);
            digest.update(&block[..count * ENTRY_STRIDE]);
            for index in 0..count {
                let at = index * ENTRY_STRIDE;
                let entry =
                    Entry::decode(&block[at..at + ENTRY_STRIDE], self.extents).map_err(|_| StoreError::Invalid)?;
                structure.accept(&entry).map_err(|_| StoreError::Invalid)?;
                self.free.claim(&entry.ranges).map_err(|_| StoreError::Invalid)?;
                if entry.meta.flags.has(EntryFlags::RECORDING) {
                    loaded.recording = Some(entry);
                }
                loaded.exhausted |= entry.meta.revision.0 == u64::MAX;
            }
            done += count;
        }
        structure.finish(&header).map_err(|_| StoreError::Invalid)?;
        if digest.finalize() != gate.body_crc {
            return Err(StoreError::Invalid);
        }
        Ok(loaded)
    }

    /// §7.3: read the 16 slots, take the candidate with the greatest checkpoint sequence. That is the
    /// whole mandatory decision. The slot CRC is checked from the greatest sequence down, so the 32
    /// KiB of tail bytes are only ever read for a slot that is about to be selected.
    ///
    /// A recording entry with no valid slot is the state a ride start leaves before its first
    /// checkpoint: the ride resumes at sequence 1 with nothing flushed.
    fn recover_ride(&mut self, entry: &Entry) {
        let mut candidates: [Option<Slot>; SLOTS] = [None; SLOTS];
        let mut block = [0u8; BLOCK];
        for (slot, candidate) in candidates.iter_mut().enumerate() {
            if read_blocks(&self.dev, slot_block(slot), &mut block).is_err() {
                continue;
            }
            *candidate =
                Slot::decode(&block, slot, &self.store, self.extents).ok().filter(|decoded| decoded.describes(entry));
        }
        let mut ride = RideState {
            id: entry.meta.id,
            revision: entry.meta.revision,
            ranges: entry.ranges,
            flushed: 0,
            next_sequence: 1,
        };
        while let Some(index) = candidates
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.map(|slot| (index, slot.sequence)))
            .max_by_key(|(_, sequence)| *sequence)
            .map(|(index, _)| index)
        {
            let slot = candidates[index].take().expect("the index came from a present slot");
            if !self.slot_intact(&slot) {
                continue;
            }
            ride.flushed = slot.flushed;
            ride.next_sequence = slot.sequence + 1;
            self.recovered = Some(RideRecovery {
                id: slot.id,
                revision: slot.revision,
                checkpoint_sequence: slot.sequence,
                flushed: slot.flushed,
                tail_len: slot.tail_len,
                payload_crc: slot.payload_crc,
                slot: slot.slot,
            });
            break;
        }
        self.ride = Some(ride);
    }

    /// The other half of a slot's candidacy: the slot CRC over all 32,768 bytes. The 63 tail blocks are
    /// read in chunks, because this is a digest fold with nothing to decode block by block.
    fn slot_intact(&self, slot: &Slot) -> bool {
        let mut digest = journal::header_digest(&slot.header_bytes(&self.store));
        let base = slot_block(slot.slot as usize) + 1;
        let mut chunk = [0u8; ZERO_PAD.len()];
        let mut read = 0u64;
        while read < SLOT_BLOCKS - 1 {
            let blocks = (SLOT_BLOCKS - 1 - read).min(chunk.len() as u64 / BLOCK as u64) as usize;
            if read_blocks(&self.dev, base + read, &mut chunk[..blocks * BLOCK]).is_err() {
                return false;
            }
            digest.update(&chunk[..blocks * BLOCK]);
            read += blocks as u64;
        }
        digest.finalize() == slot.slot_crc
    }

    /// The retained and the head entry of one `ObjectId`: a binary search over the live prefix, then
    /// at most two entry reads.
    fn find(&self, id: ObjectId) -> Result<(Option<Entry>, Option<Entry>), StoreError> {
        let mut reader = EntryReader::new(&self.dev, self.serving, self.extents);
        let mut low = 0u16;
        let mut high = self.entry_count;
        while low < high {
            let mid = low + (high - low) / 2;
            if reader.get(mid)?.meta.id < id {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let mut retained = None;
        let mut head = None;
        for index in low..self.entry_count.min(low.saturating_add(2)) {
            let entry = reader.get(index)?;
            if entry.meta.id != id {
                break;
            }
            if entry.meta.flags.has(EntryFlags::RETAINED) {
                retained = Some(entry);
            } else {
                head = Some(entry);
            }
        }
        Ok((retained, head))
    }

    fn row(&self, allocation: &Allocation) -> Option<&Reservation> {
        self.reservations.get(allocation.slot as usize)?.as_ref().filter(|row| {
            (row.nonce, row.written, row.reserved) == (allocation.nonce, allocation.written, allocation.reserved)
        })
    }

    /// Walks the new entry array in key order: the serving copy's entries with the batch applied.
    fn merge<F>(&self, plan: &[Resolved], mut emit: F) -> Result<u16, StoreError>
    where
        F: FnMut(&Entry) -> Result<(), StoreError>,
    {
        let mut order: [u8; MAX_BATCH] = [0; MAX_BATCH];
        for (index, slot) in order.iter_mut().enumerate().take(plan.len()) {
            *slot = index as u8;
        }
        let order = &mut order[..plan.len()];
        order.sort_unstable_by_key(|index| plan[*index as usize].key);

        let mut reader = EntryReader::new(&self.dev, self.serving, self.extents);
        let mut next = 0usize;
        let mut written = 0u16;
        for index in 0..self.entry_count {
            let entry = reader.get(index)?;
            while next < order.len() && plan[order[next] as usize].key < entry.meta.key() {
                if let Some(fresh) = plan[order[next] as usize].entry.as_ref() {
                    emit(fresh)?;
                    written += 1;
                }
                next += 1;
            }
            if next < order.len() && plan[order[next] as usize].key == entry.meta.key() {
                if let Some(replacement) = plan[order[next] as usize].entry.as_ref() {
                    emit(replacement)?;
                    written += 1;
                }
                next += 1;
            } else {
                emit(&entry)?;
                written += 1;
            }
        }
        while next < order.len() {
            if let Some(fresh) = plan[order[next] as usize].entry.as_ref() {
                emit(fresh)?;
                written += 1;
            }
            next += 1;
        }
        Ok(written)
    }

    /// Resolves one mutation against the catalog: what it writes, whether it creates a row, and what
    /// it hands back.
    fn resolve(&self, mutation: &Mutation) -> Result<Resolved, StoreError> {
        match mutation {
            Mutation::Remove { id, revision } => {
                let (retained, head) = self.find(*id)?;
                let existing = [retained, head]
                    .into_iter()
                    .flatten()
                    .find(|entry| entry.meta.revision == *revision)
                    .ok_or(StoreError::NotFound)?;
                Ok(Resolved {
                    key: (*id, *revision),
                    entry: None,
                    creates: false,
                    freed: existing.ranges,
                    reservation: None,
                })
            }
            Mutation::Put { meta, source } => {
                if meta.id == ObjectId::NONE || meta.revision.0 == 0 {
                    return Err(StoreError::Invalid);
                }
                if meta.revision.0 == u64::MAX {
                    return Err(StoreError::ReadOnly);
                }
                let (retained, head) = self.find(meta.id)?;
                let existing =
                    [retained, head].into_iter().flatten().find(|entry| entry.meta.revision == meta.revision);
                match source {
                    PutSource::Amend => {
                        let existing = existing.ok_or(StoreError::NotFound)?;
                        if meta.kind != existing.meta.kind {
                            return Err(StoreError::Invalid);
                        }
                        let mut entry = Entry { meta: *meta, ranges: existing.ranges };
                        let freed = if meta.flags.holds_slack() {
                            Ranges::default()
                        } else {
                            entry.ranges.trim_to(extents_for(meta.payload_len) as u32)
                        };
                        if entry.ranges.is_empty() {
                            return Err(StoreError::Invalid);
                        }
                        Ok(Resolved { key: meta.key(), entry: Some(entry), creates: false, freed, reservation: None })
                    }
                    PutSource::Fresh(allocation) => {
                        if existing.is_some() {
                            return Err(StoreError::Invalid);
                        }
                        let expected = head.map_or(1, |entry| entry.meta.revision.0 + 1);
                        if meta.revision.0 != expected {
                            return Err(StoreError::RevisionConflict {
                                current: head.map_or(Revision(0), |entry| entry.meta.revision),
                            });
                        }
                        if let Some(head) = head {
                            if head.meta.kind != meta.kind {
                                return Err(StoreError::Invalid);
                            }
                        }
                        let row = self.row(allocation).ok_or(StoreError::Invalid)?;
                        if !meta.flags.holds_slack() && meta.payload_len != row.written {
                            return Err(StoreError::Invalid);
                        }
                        let mut entry = Entry { meta: *meta, ranges: row.ranges };
                        let freed = if meta.flags.holds_slack() {
                            Ranges::default()
                        } else {
                            entry.ranges.trim_to(extents_for(meta.payload_len) as u32)
                        };
                        if entry.ranges.is_empty() {
                            return Err(StoreError::Invalid);
                        }
                        Ok(Resolved {
                            key: meta.key(),
                            entry: Some(entry),
                            creates: true,
                            freed,
                            reservation: Some(allocation.slot),
                        })
                    }
                }
            }
        }
    }

    /// Marks the extents `plan` gives back free, unless a reader still holds the entry that named
    /// them — a RAM-only hold that needs no durable record, because after a reboot the extents are
    /// free and there is no reader left to be surprised.
    fn release(&mut self, plan: &[Resolved]) {
        let mut holds = self.holds.borrow_mut();
        for resolved in plan {
            let mut held = false;
            if resolved.entry.is_none() {
                for hold in holds.iter_mut().flatten() {
                    if (hold.id, hold.revision) == resolved.key {
                        hold.orphaned = true;
                        held = true;
                    }
                }
            }
            if !held {
                self.free.release(&resolved.freed);
            }
        }
    }
}

/// What loading one catalog copy established.
struct Loaded {
    next_object: u64,
    recording: Option<Entry>,
    exhausted: bool,
}

impl<D: BlockDevice> Store for FlatStore<D> {
    type Handle = Handle;

    fn allocate(&mut self, bytes: u64) -> Result<Allocation, StoreError> {
        if !self.mode.writable() {
            return Err(StoreError::ReadOnly);
        }
        if bytes == 0 {
            return Err(StoreError::Invalid);
        }
        let extents = extents_for(bytes);
        if u64::from(self.free.free()) < extents {
            return Err(StoreError::NoSpace { required: bytes });
        }
        let ranges = self.free.first_fit(extents as u32).ok_or(StoreError::TooFragmented)?;
        let slot = self.reservations.iter().position(Option::is_none).ok_or(StoreError::Invalid)?;
        self.free.claim(&ranges).map_err(|_| StoreError::Invalid)?;
        self.nonce = self.nonce.wrapping_add(1);
        self.reservations[slot] =
            Some(Reservation { nonce: self.nonce, ranges, reserved: bytes, written: 0, staging: [0; BLOCK] });
        Ok(Allocation { slot: slot as u8, nonce: self.nonce, reserved: bytes, written: 0 })
    }

    fn write(&mut self, allocation: &mut Allocation, bytes: &[u8]) -> Result<(), StoreError> {
        if !self.mode.writable() {
            return Err(StoreError::ReadOnly);
        }
        if self.row(allocation).is_none() {
            return Err(StoreError::Invalid);
        }
        if allocation.written + bytes.len() as u64 > allocation.reserved {
            return Err(StoreError::Invalid);
        }
        let dev = &self.dev;
        let row = self.reservations[allocation.slot as usize].as_mut().expect("the row was just validated");
        let mut input = bytes;
        while !input.is_empty() {
            let staged = (row.written % BLOCK as u64) as usize;
            let located = row.ranges.locate(row.written - staged as u64).ok_or(StoreError::Invalid)?;
            if staged == 0 && input.len() >= BLOCK {
                let blocks = (input.len() / BLOCK).min((located.contiguous / BLOCK as u64) as usize);
                write_blocks(dev, located.block, &input[..blocks * BLOCK])?;
                row.written += (blocks * BLOCK) as u64;
                input = &input[blocks * BLOCK..];
            } else {
                let take = (BLOCK - staged).min(input.len());
                row.staging[staged..staged + take].copy_from_slice(&input[..take]);
                row.written += take as u64;
                input = &input[take..];
                if row.written.is_multiple_of(BLOCK as u64) {
                    write_blocks(dev, located.block, &row.staging)?;
                }
            }
        }
        allocation.written = row.written;
        Ok(())
    }

    /// §5.5, and the only durable state transition an object ever undergoes. Payload bytes are
    /// written and synchronized before it begins, so a cut at any point before the gate leaves those
    /// bytes anonymous and their extents free at the next mount.
    fn commit(&mut self, mutations: &[Mutation]) -> Result<u64, StoreError> {
        if !self.mode.writable() {
            return Err(StoreError::ReadOnly);
        }
        if mutations.is_empty() || mutations.len() > MAX_BATCH {
            return Err(StoreError::Invalid);
        }
        let mut plan: [Resolved; MAX_BATCH] = [Resolved {
            key: (ObjectId::NONE, Revision(0)),
            entry: None,
            creates: false,
            freed: Ranges::default(),
            reservation: None,
        }; MAX_BATCH];
        let mut count = self.entry_count as i32;
        let mut greatest_id = 0u64;
        for (index, mutation) in mutations.iter().enumerate() {
            if mutations[..index].iter().any(|earlier| earlier.key() == mutation.key()) {
                return Err(StoreError::Invalid);
            }
            plan[index] = self.resolve(mutation)?;
            // Two entries fed by one reservation would name the same extents, and overlap is a rule
            // only a mount checks — by which point the card would already be unreadable.
            if plan[index].reservation.is_some()
                && plan[..index].iter().any(|earlier| earlier.reservation == plan[index].reservation)
            {
                return Err(StoreError::Invalid);
            }
            count += i32::from(plan[index].creates) - i32::from(plan[index].entry.is_none());
            greatest_id = greatest_id.max(plan[index].key.0 .0);
        }
        let plan = &plan[..mutations.len()];
        if count < 0 || count as usize > ENTRY_CAPACITY {
            return Err(StoreError::CatalogFull);
        }

        // Everything the batch would write, checked against §5.3 before the card is touched.
        let header = Header {
            store: self.store,
            sequence: self.high_water + 1,
            next_object: self.next_object.max(greatest_id + 1),
            entry_count: count as u16,
        };
        let mut structure = Structure::default();
        let written = self.merge(plan, |entry| structure.accept(entry).map_err(|_| StoreError::Invalid))?;
        structure.finish(&header).map_err(|_| StoreError::Invalid)?;
        if written != header.entry_count {
            return Err(StoreError::Invalid);
        }

        // The payload is durable before the commit begins: whatever a `write` left in a staging
        // block goes to the card now.
        let mut staged = false;
        for resolved in plan.iter() {
            let Some(slot) = resolved.reservation else { continue };
            let dev = &self.dev;
            let row = self.reservations[slot as usize].as_mut().expect("resolve validated the reservation");
            let partial = (row.written % BLOCK as u64) as usize;
            if partial > 0 {
                let located = row.ranges.locate(row.written - partial as u64).ok_or(StoreError::Invalid)?;
                row.staging[partial..].fill(0);
                write_blocks(dev, located.block, &row.staging)?;
            }
            staged = true;
        }
        if staged {
            sync(&self.dev)?;
        }

        let target = 1 - self.serving;
        write_blocks(&self.dev, catalog_gate(target), &INVALIDATED)?;
        sync(&self.dev)?;

        let body = header.encode();
        write_blocks(&self.dev, CATALOG[target], &body)?;
        let mut digest = Crc32::new();
        digest.update(&body);
        let mut writer = BodyWriter { dev: &self.dev, block: CATALOG[target] + 1, buf: [0; BLOCK], filled: 0, digest };
        self.merge(plan, |entry| writer.push(entry))?;
        let body_crc = writer.finish()?;
        sync(&self.dev)?;

        let gate = Gate {
            copy: target as u8,
            store: self.store,
            sequence: header.sequence,
            entry_count: header.entry_count,
            body_crc,
        };
        write_blocks(&self.dev, catalog_gate(target), &gate.encode())?;
        sync(&self.dev)?;

        // The gate landed: `target` is the truth, and everything the batch displaced is free.
        self.serving = target;
        self.sequence = header.sequence;
        self.high_water = header.sequence;
        self.next_object = header.next_object;
        self.entry_count = header.entry_count;
        self.release(plan);
        for resolved in plan.iter() {
            if let Some(slot) = resolved.reservation {
                self.reservations[slot as usize] = None;
            }
        }
        self.settle_ride(plan);
        Ok(self.sequence)
    }

    fn open(&self, id: ObjectId, revision: Option<Revision>) -> Result<Handle, StoreError> {
        if !self.mode.readable() {
            return Err(StoreError::ReadOnly);
        }
        let (retained, head) = self.find(id)?;
        let entry = match revision {
            None => head,
            Some(revision) => [retained, head].into_iter().flatten().find(|entry| entry.meta.revision == revision),
        }
        .ok_or(StoreError::NotFound)?;
        // §5.3: the store did not write a reserve's bytes, so there is nothing here to serve.
        if entry.meta.flags.has(EntryFlags::RESERVED) {
            return Err(StoreError::Invalid);
        }
        let mut holds = self.holds.borrow_mut();
        let key = (entry.meta.id, entry.meta.revision);
        if let Some(slot) = holds.iter().position(|hold| hold.is_some_and(|hold| (hold.id, hold.revision) == key)) {
            let hold = holds[slot].as_mut().expect("the row was just found");
            hold.readers += 1;
            return Ok(Handle { slot: slot as u8, id: entry.meta.id, revision: entry.meta.revision });
        }
        let slot = holds.iter().position(Option::is_none).ok_or(StoreError::Invalid)?;
        holds[slot] = Some(Hold {
            id: entry.meta.id,
            revision: entry.meta.revision,
            ranges: entry.ranges,
            payload_len: entry.meta.payload_len,
            readers: 1,
            orphaned: false,
        });
        Ok(Handle { slot: slot as u8, id: entry.meta.id, revision: entry.meta.revision })
    }

    fn read(&self, handle: &Handle, offset: u64, buf: &mut [u8]) -> Result<usize, StoreError> {
        let holds = self.holds.borrow();
        let hold = holds[handle.slot as usize]
            .filter(|hold| (hold.id, hold.revision) == (handle.id, handle.revision))
            .ok_or(StoreError::Invalid)?;
        drop(holds);
        if offset >= hold.payload_len {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(hold.payload_len - offset) as usize;
        let mut done = 0usize;
        let mut block = [0u8; BLOCK];
        while done < want {
            let located = hold.ranges.locate(offset + done as u64).ok_or(StoreError::Invalid)?;
            let run = ((want - done) as u64).min(located.contiguous) as usize;
            if located.offset == 0 && run >= BLOCK {
                let blocks = run / BLOCK;
                read_blocks(&self.dev, located.block, &mut buf[done..done + blocks * BLOCK])?;
                done += blocks * BLOCK;
            } else {
                read_blocks(&self.dev, located.block, &mut block)?;
                let take = (BLOCK - located.offset).min(run);
                buf[done..done + take].copy_from_slice(&block[located.offset..located.offset + take]);
                done += take;
            }
        }
        Ok(done)
    }

    fn entries(&self) -> impl Iterator<Item = EntryMeta> + '_ {
        Entries { reader: EntryReader::new(&self.dev, self.serving, self.extents), index: 0, count: self.entry_count }
    }

    /// §7.2 steps 2 and 3: flush whole payload pages out of the front of the tail into the recording
    /// entry's own extents, then write one slot. A payload page is written only when every byte in it
    /// is already in a slot on the card, and once written it is never touched again.
    fn journal(&mut self, checkpoint: RideCheckpoint) -> Result<(), StoreError> {
        if !self.mode.writable() {
            return Err(StoreError::ReadOnly);
        }
        let Some(mut ride) = self.ride else { return Err(StoreError::Invalid) };
        if (checkpoint.id, checkpoint.revision) != (ride.id, ride.revision) {
            return Err(StoreError::Invalid);
        }
        let mut tail = checkpoint.tail;
        while tail.len() >= PROGRAM_PAGE {
            let located = ride
                .ranges
                .locate(ride.flushed)
                .filter(|located| located.contiguous >= PROGRAM_PAGE as u64)
                .ok_or(StoreError::NoSpace { required: ride.flushed + PROGRAM_PAGE as u64 })?;
            write_blocks(&self.dev, located.block, &tail[..PROGRAM_PAGE])?;
            sync(&self.dev)?;
            ride.flushed += PROGRAM_PAGE as u64;
            self.ride = Some(ride);
            tail = &tail[PROGRAM_PAGE..];
        }
        if tail.len() > TAIL_CAPACITY {
            return Err(StoreError::Invalid);
        }

        let slot = Slot {
            slot: (ride.next_sequence % SLOTS as u64) as u16,
            id: ride.id,
            revision: ride.revision,
            sequence: ride.next_sequence,
            flushed: ride.flushed,
            tail_len: tail.len() as u32,
            payload_crc: checkpoint.payload_crc,
            ranges: ride.ranges,
            slot_crc: 0,
        };
        let base = slot_block(slot.slot as usize);
        write_blocks(&self.dev, base, &slot.seal(&self.store, tail))?;
        let whole = tail.len() / BLOCK;
        if whole > 0 {
            write_blocks(&self.dev, base + 1, &tail[..whole * BLOCK])?;
        }
        let mut next = 1 + whole as u64;
        if !tail.len().is_multiple_of(BLOCK) {
            let mut partial = [0u8; BLOCK];
            partial[..tail.len() - whole * BLOCK].copy_from_slice(&tail[whole * BLOCK..]);
            write_blocks(&self.dev, base + next, &partial)?;
            next += 1;
        }
        while next < SLOT_BLOCKS {
            let step = (SLOT_BLOCKS - next).min(ZERO_PAD.len() as u64 / BLOCK as u64);
            write_blocks(&self.dev, base + next, &ZERO_PAD[..step as usize * BLOCK])?;
            next += step;
        }
        sync(&self.dev)?;

        ride.next_sequence += 1;
        self.ride = Some(ride);
        Ok(())
    }
}

impl<D: BlockDevice> FlatStore<D> {
    /// The resident ride state after a commit that started, amended or ended the ride.
    ///
    /// Ride end zeroes the 16 slot headers (§7.2), and this runs *after* the gate, so a media failure
    /// here cannot be reported: the commit already happened and `commit` promises that an `Err`
    /// changed nothing. §7.2 covers the cost of losing it — a cut during that zeroing is harmless,
    /// because no entry carries `RECORDING` and §5.6 never reads the slots.
    fn settle_ride(&mut self, plan: &[Resolved]) {
        let started =
            plan.iter().filter_map(|resolved| resolved.entry).find(|entry| entry.meta.flags.has(EntryFlags::RECORDING));
        if let Some(entry) = started {
            let same = self.ride.filter(|ride| (ride.id, ride.revision) == (entry.meta.id, entry.meta.revision));
            self.ride = Some(RideState {
                id: entry.meta.id,
                revision: entry.meta.revision,
                ranges: entry.ranges,
                flushed: same.map_or(0, |ride| ride.flushed),
                next_sequence: same.map_or(1, |ride| ride.next_sequence),
            });
            return;
        }
        let Some(ride) = self.ride else { return };
        if !plan.iter().any(|resolved| resolved.key == (ride.id, ride.revision)) {
            return;
        }
        self.ride = None;
        self.recovered = None;
        for slot in 0..SLOTS {
            if write_blocks(&self.dev, slot_block(slot), &INVALIDATED).is_err() {
                return;
            }
        }
        let _ = sync(&self.dev);
    }
}

/// §2's read-only catalog view: every entry, in the catalog's own `(ObjectId, Revision)` order.
struct Entries<'a, D> {
    reader: EntryReader<'a, D>,
    index: u16,
    count: u16,
}

impl<D: BlockDevice> Iterator for Entries<'_, D> {
    type Item = EntryMeta;

    fn next(&mut self) -> Option<EntryMeta> {
        if self.index >= self.count {
            return None;
        }
        let entry = self.reader.get(self.index).ok()?;
        self.index += 1;
        Some(entry.meta)
    }
}
