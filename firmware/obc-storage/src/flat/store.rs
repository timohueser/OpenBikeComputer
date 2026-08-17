//! The store: mount (`FLAT_Store_Format.md` §5.6), initialization (§8), the alternating commit
//! (§5.5), the ride journal's write half (§7.2) and the five seam operations
//! (`FLAT_Store_Protocol.md` §2).
//!
//! Resident state is the 8 KiB free bitmap, a handful of rows, and nothing else: the entry array
//! lives on the card and is read off it, which is what makes a lookup nine block reads and a mount a
//! fixed cost. A **scan** of that array — a mount, and each of a commit's two passes over it — moves
//! it in windows rather than a block at a time — [`STREAM_WINDOW`] for a commit's, and half that for a
//! mount's, which shares a frame with the store it is building — and a commit stages the body it writes
//! in one too, because the card charges a program cycle per command and only microseconds per block
//! inside one (see [`STREAM_BLOCKS`]). A **lookup** stays one block, because a binary search's
//! probes are scattered and a wide window would read 4 KiB to look at 128 bytes of it. None of this
//! changes a block address, a byte or an ordering: it is the same body in the same places before the
//! same synchronization. Every buffer here is fixed and on the stack — no allocation, on the device or
//! on the host.

use core::cell::{Cell, RefCell};

use obc_crc::Crc32;

use super::bitmap::FreeMap;
use super::catalog::{Entry, Gate, Header, Structure, INVALIDATED};
use super::device::BlockDevice;
use super::error::StoreError;
use super::journal::{self, Slot, TAIL_CAPACITY, ZERO_PAD};
use super::layout::{
    catalog_gate, extent_count, extents_for, slot_block, Ranges, BLOCK, CATALOG, ENTRIES_PER_BLOCK, ENTRY_CAPACITY,
    ENTRY_STRIDE, MOUNT_STREAM_BLOCKS, MOUNT_STREAM_WINDOW, PROGRAM_PAGE, SLOTS, SLOT_BLOCKS, STREAM_WINDOW,
    SUPERBLOCK,
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
    /// An object reached `Revision` `u64::MAX`, so nothing can supersede it (§3). Reads are still
    /// served. Wire face: `readOnly` / `revisionSpaceExhausted 2`.
    RevisionSpaceExhausted,
    /// A well-formed gate carries commit sequence `u64::MAX`, so §5.5 step 2 has no sequence to
    /// continue to. Reads are still served, and the wire face is the same
    /// `revisionSpaceExhausted 2`: from a client's side both are "this card's counters ran out".
    SequenceSpaceExhausted,
    /// No gate is well-formed, or no candidate body validated. Evidence preserved, no repair.
    /// Wire face: `readOnly` / `catalogUnreadable 1`.
    CatalogUnreadable,
    /// §5.6 step 1 classified the card as not a flat store. Initialization is the only transition.
    /// Wire face: `readOnly` / `unformatted 3`.
    Unformatted,
    /// The card is smaller than the superblock recorded: damaged or swapped, never silently
    /// truncated (§4). Wire face: `readOnly` / `unformatted 3` — the card the superblock describes
    /// is not the card in the slot, so there is no flat store here either.
    CardTooSmall,
}

impl Mode {
    /// True when a commit may run.
    pub fn writable(self) -> bool {
        self == Mode::ReadWrite
    }

    /// True when the catalog is usable. Only the two exhausted cases still serve reads.
    pub fn readable(self) -> bool {
        matches!(self, Mode::ReadWrite | Mode::RevisionSpaceExhausted | Mode::SequenceSpaceExhausted)
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
    /// The extents this reader resolved. A commit may have taken them out of the entry since — by
    /// removing it or by trimming it — and until the last reader closes they stay out of the
    /// allocator, which is why the hold keeps its own copy rather than consulting the catalog.
    ranges: Ranges,
    payload_len: u64,
    readers: u16,
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
    /// Set when the [`Store::entries`] iterator hit a media failure. §2's listing returns a plain
    /// iterator with nowhere to put an error, so the failure is recorded here and
    /// [`entries_ok`](Self::entries_ok) is how a caller finds out its listing was short.
    listing_failed: Cell<bool>,
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

/// Appends `input` to a reservation, one contiguous run per pass: whole blocks straight out of the
/// caller's slice, and a partial one through the row's staging block. The cursor it advances belongs to
/// the caller's [`Allocation`] too, which is why [`Store::write`] rewinds it when this fails.
fn fill<D: BlockDevice>(dev: &D, row: &mut Reservation, mut input: &[u8]) -> Result<(), StoreError> {
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
    Ok(())
}

/// Where a walk of one catalog copy's entry array has got to. The **window is the caller's buffer**,
/// and its length is the window size.
///
/// That split is not tidiness, it is the stack. A reader that owned its window was a value built in a
/// return slot and then moved, so each one could cost *two* windows of frame, and `commit` — which
/// needs a scan and a body stage at once — paid for up to four: 13,568 B measured here, and 17,664 B in
/// a reviewer's harness, against 2,796 B on the revision before any of this. Owning the windows in the
/// caller as plain locals and lending them is what makes it one slot each, and takes the same symbol to
/// **9,920 B**. It is the failure mode `resource_guard.py frames` was written for, in the same shape.
///
/// Worth recording, because it is the opposite of reassuring: that guard did not see any of it. It
/// substring-matches a demangled name, and `Store::commit` is a *trait* impl, which `llvm-objdump`
/// demangles with legacy escaping — `_$LT$obc_storage..flat..store..FlatStore$LT$D$GT$$u20$as$u20$…$GT$
/// ::commit`, with `..` where the needle `obc_storage::flat` expects `::`. So every trait-impl frame in
/// this module — `commit`, `open`, `read`, `journal`, `write` — was invisible to it, and the ceiling
/// was being held by `mount` alone. #1409 widens the needle to `obc_storage`, which the escaped names
/// do match; these numbers are measured against that.
///
/// A cursor and the buffer it is used with are a **pair**: the cursor says which blocks the buffer
/// holds, so lending a different buffer to a cursor with a live window would decode whatever that other
/// buffer contains. Every call site here keeps them adjacent for that reason.
///
/// The length is the cost model at this seam. A scan of the live prefix lends [`STREAM_WINDOW`] and
/// pays one card command every 32 entries; a binary search lends one [`BLOCK`], because its probes are
/// scattered and a wide window would read 4 KiB to look at 128 bytes of it. Both are this one
/// implementation, so a probe and a scan cannot drift apart in how they decode an entry.
struct EntryCursor {
    base: u64,
    extents: u32,
    /// Blocks the live prefix occupies. The window is clamped to it so a short catalog is never read
    /// wider than it is — a cost matter, not a safety one: the blocks past the prefix are the previous
    /// commit's leftovers and the copy's gate, and reading either into a scan buffer is harmless.
    live: u64,
    /// The window the buffer currently holds: its first block, and how many blocks of it are valid.
    cached: Option<(u64, u64)>,
}

impl EntryCursor {
    fn new(copy: usize, extents: u32, entries: u16) -> Self {
        EntryCursor {
            base: CATALOG[copy] + 1,
            extents,
            live: (entries as u64).div_ceil(ENTRIES_PER_BLOCK as u64),
            cached: None,
        }
    }

    fn get<D: BlockDevice>(&mut self, dev: &D, buf: &mut [u8], index: u16) -> Result<Entry, StoreError> {
        debug_assert!(!buf.is_empty() && buf.len().is_multiple_of(BLOCK), "a window is whole blocks");
        let block = index as u64 / ENTRIES_PER_BLOCK as u64;
        // The width test is the `no_std` safety half: a cursor paired with a *narrower* buffer than the
        // one that filled it would otherwise index past the end of it. No call site does that today —
        // the pairs are adjacent locals — but the failure mode is a panic on a device rather than a
        // wrong answer, and the fix is to treat a window the buffer cannot hold as a miss and re-read.
        let held = self.cached.filter(|(first, count)| {
            block >= *first && block - *first < *count && *count <= (buf.len() / BLOCK) as u64
        });
        let first = match held {
            Some((first, _)) => first,
            None => {
                // The window starts at the block asked for and runs forward, which is what makes a
                // scan pay one command per window: a walk of ascending indices never re-reads a block
                // it has already seen.
                let count = (buf.len() / BLOCK) as u64;
                let count = count.min(self.live.saturating_sub(block)).max(1);
                read_blocks(dev, self.base + block, &mut buf[..count as usize * BLOCK])?;
                self.cached = Some((block, count));
                block
            }
        };
        let at = (block - first) as usize * BLOCK + index as usize % ENTRIES_PER_BLOCK * ENTRY_STRIDE;
        Entry::decode(&buf[at..at + ENTRY_STRIDE], self.extents).map_err(|_| StoreError::Invalid)
    }
}

/// Writes a catalog body: the header block and then the entries stream through it, and it folds them
/// into the body CRC the gate will carry.
///
/// The window is [`STREAM_WINDOW`] rather than one block for the reason in [`STREAM_BLOCKS`], and it
/// changes nothing else: the same blocks land at the same addresses carrying the same bytes, in the
/// same order, before the same synchronization. What a cut sees is the subject of
/// [`sim::When::Inside`](super::sim::When::Inside).
struct BodyWriter<'a, 'b, D> {
    dev: &'a D,
    /// The block the first byte of the window belongs to.
    block: u64,
    /// The stage, lent by the caller for the same stack reason as [`EntryCursor`]'s.
    buf: &'b mut [u8],
    filled: usize,
    digest: Crc32,
}

impl<'a, 'b, D: BlockDevice> BodyWriter<'a, 'b, D> {
    /// A writer over the body of one catalog copy, positioned at its header block.
    fn new(dev: &'a D, block: u64, buf: &'b mut [u8]) -> Self {
        debug_assert!(!buf.is_empty() && buf.len().is_multiple_of(BLOCK), "a window is whole blocks");
        BodyWriter { dev, block, buf, filled: 0, digest: Crc32::new() }
    }

    /// The header block, which is block 0 of the body and the first thing the CRC covers. It goes
    /// through the window so that a body of one header and a few entries is one card command.
    fn push_header(&mut self, header: &[u8; BLOCK]) -> Result<(), StoreError> {
        self.digest.update(header);
        self.buf[self.filled..self.filled + BLOCK].copy_from_slice(header);
        self.filled += BLOCK;
        self.full()
    }

    fn push(&mut self, entry: &Entry) -> Result<(), StoreError> {
        let bytes = entry.encode();
        self.digest.update(&bytes);
        self.buf[self.filled..self.filled + ENTRY_STRIDE].copy_from_slice(&bytes);
        self.filled += ENTRY_STRIDE;
        self.full()
    }

    fn full(&mut self) -> Result<(), StoreError> {
        if self.filled == self.buf.len() {
            self.flush()?;
        }
        Ok(())
    }

    /// Writes the whole blocks the window holds, and only those. A window that is short of a block
    /// boundary is padded — the bytes past the live prefix are whatever an earlier commit left there,
    /// nothing reads them and no CRC covers them — but the write never runs past the last block the
    /// body occupies: at [`ENTRY_CAPACITY`] entries the block after it is the copy's gate.
    fn flush(&mut self) -> Result<(), StoreError> {
        let blocks = self.filled.div_ceil(BLOCK);
        self.buf[self.filled..blocks * BLOCK].fill(0);
        write_blocks(self.dev, self.block, &self.buf[..blocks * BLOCK])?;
        self.block += blocks as u64;
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
            listing_failed: Cell::new(false),
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

    /// True when the last [`Store::entries`] listing ran to the end of the array. A listing that hit a
    /// media failure stops early with no way to say so, so a caller that cares — anything reporting a
    /// complete list, `LIST` included — asks here before it treats the list as the catalog.
    pub fn entries_ok(&self) -> bool {
        !self.listing_failed.get()
    }

    /// The copy the store is serving. §5.5's next commit targets the other one.
    ///
    /// A card-layout fact with no caller above the seam: the harness reads the copy the store selected.
    #[cfg(any(test, feature = "std"))]
    pub fn serving_copy(&self) -> usize {
        self.serving
    }

    /// The mark §5.5 step 2 continues from, which a fallback mount leaves above the served sequence.
    #[cfg(any(test, feature = "std"))]
    pub fn high_water(&self) -> u64 {
        self.high_water
    }

    /// Free extents, each 1 MiB.
    pub fn free_extents(&self) -> u32 {
        self.free.free()
    }

    /// The next `ObjectId` the cursor will hand out. A create names this in its `Put`; the commit
    /// advances the cursor past it and never rewinds.
    ///
    /// Reading it reserves nothing. Two creates that both read it before either commits would name the
    /// same id, and the second one's commit is refused as a duplicate key — acceptable only because
    /// `FLAT_Store_Protocol.md` §1 serves one transfer at a time.
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
    ///
    /// A dropped `Allocation` releases nothing: the reservation row and its extents stay taken until
    /// this is called or the card is remounted, and there are only [`MAX_RESERVATIONS`] rows. Every
    /// path that abandons a transfer — a cancel, a refusal, a validator rejection, a lost link — has to
    /// come through here.
    pub fn cancel(&mut self, allocation: Allocation) {
        if let Some(row) = self.row(&allocation) {
            let ranges = row.ranges;
            self.reservations[allocation.slot as usize] = None;
            self.free.release(&ranges);
        }
    }

    /// Closes an open object. When the last reader lets go, every extent it was holding that the
    /// catalog no longer names goes back to the allocator — which is the whole of §6.2's hold rule,
    /// whether a commit removed the entry or only trimmed it.
    ///
    /// A caller that drops a `Handle` instead of closing it leaks the row (and its extents) until the
    /// next mount, so the engine must treat `open`/`close` as a pair.
    ///
    /// This returns nothing, and one failure is therefore silent: working out which of the hold's
    /// extents the catalog still names is a media read, and a read that fails leaves those extents
    /// allocated rather than guessing (§6.2). The row is released either way, so nothing leaks at the
    /// seam — the observable cost is a lower [`free_extents`](Self::free_extents) until an entry that
    /// names them is removed or the card is remounted. Like [`entries`](Store::entries), which reports a
    /// short listing through [`entries_ok`](Self::entries_ok), this is stated rather than hidden; unlike
    /// it, no caller has a decision to make on it, so there is no flag to ask.
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
        // What the entry still names, if it is still there at all. A media failure here leaves the
        // extents allocated until the next mount rebuilds the map from the catalog, which is the safe
        // direction: never hand out an extent an entry might name. A failed read is *not* evidence the
        // entry is gone, so it must not be read as one — freeing a live entry's extents would let the
        // next allocation overlap it, and an overlap is a rule only a mount checks.
        let Ok((retained, head)) = self.find(hold.id) else { return };
        let live = [retained, head].into_iter().flatten().find(|entry| entry.meta.revision == hold.revision);
        for (first, count) in hold.ranges.iter() {
            for extent in first..first + count {
                if !live.is_some_and(|entry| entry.ranges.names(extent)) {
                    self.free.release_one(u32::from(extent));
                }
            }
        }
    }

    /// The device, for a bench or a harness that needs the card underneath. Nothing above the seam has
    /// any business with it.
    #[cfg(any(test, feature = "std"))]
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
                // A counter that has run out mounts read-only rather than wrapping: a revision no
                // commit can supersede (§3), or a gate sequence §5.5 step 2 cannot continue from.
                self.mode = if loaded.exhausted {
                    Mode::RevisionSpaceExhausted
                } else if self.high_water == u64::MAX {
                    Mode::SequenceSpaceExhausted
                } else {
                    Mode::ReadWrite
                };
                if let Some(recording) = loaded.recording {
                    self.recover_ride(&recording);
                }
                return;
            }
        }
        // No copy is being served, so the free map describes nothing: a failed [`load`] leaves its own
        // attempt's bitmap behind, and `free_extents()` is public.
        self.free.reset(0);
    }

    /// §5.6 step 3 and 4 for one copy: the body CRC, every structural rule of §5.3, and the free
    /// bitmap built from the ranges as they go past. A failure leaves the caller free to try the
    /// next candidate — the bitmap is rebuilt from scratch each attempt.
    fn load(&mut self, copy: usize, gate: &Gate) -> Result<Loaded, StoreError> {
        self.free.reset(self.extents);
        // One window serves the header block and then the array, and it is the boot path's whole
        // buffer: a full catalog is the header plus 120 windows, where it used to be 480 single-block
        // reads. It is [`MOUNT_STREAM_WINDOW`] rather than [`STREAM_WINDOW`] because this frame is also
        // building the store — see that constant.
        let mut window = [0u8; MOUNT_STREAM_WINDOW];
        read_blocks(&self.dev, CATALOG[copy], &mut window[..BLOCK])?;
        let header = Header::decode(&window[..BLOCK], &self.store).map_err(|_| StoreError::Invalid)?;
        if header.entry_count != gate.entry_count || header.sequence != gate.sequence {
            return Err(StoreError::Invalid);
        }
        let mut digest = Crc32::new();
        digest.update(&window[..BLOCK]);

        let mut structure = Structure::default();
        let mut loaded = Loaded { next_object: header.next_object, recording: None, exhausted: false };
        let mut done = 0usize;
        while done < header.entry_count as usize {
            // Only the blocks the live prefix occupies, so a short catalog is not read wider than it
            // is. A cost matter, not a safety one — the blocks past the prefix are an earlier commit's
            // leftovers and then the copy's gate, and reading either into this buffer would be
            // harmless; it is the *writer* that must never reach the gate.
            let remaining = header.entry_count as usize - done;
            let blocks = remaining.div_ceil(ENTRIES_PER_BLOCK).min(MOUNT_STREAM_BLOCKS);
            read_blocks(
                &self.dev,
                CATALOG[copy] + 1 + (done / ENTRIES_PER_BLOCK) as u64,
                &mut window[..blocks * BLOCK],
            )?;
            let count = remaining.min(blocks * ENTRIES_PER_BLOCK);
            digest.update(&window[..count * ENTRY_STRIDE]);
            for index in 0..count {
                let at = index * ENTRY_STRIDE;
                let entry =
                    Entry::decode(&window[at..at + ENTRY_STRIDE], self.extents).map_err(|_| StoreError::Invalid)?;
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
        // One block, not a window: a binary search's probes are scattered, so a wide window would read
        // 4 KiB to look at 128 bytes of it.
        let mut cursor = EntryCursor::new(self.serving, self.extents, self.entry_count);
        let mut probe = [0u8; BLOCK];
        let mut low = 0u16;
        let mut high = self.entry_count;
        while low < high {
            let mid = low + (high - low) / 2;
            if cursor.get(&self.dev, &mut probe, mid)?.meta.id < id {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let mut retained = None;
        let mut head = None;
        for index in low..self.entry_count.min(low.saturating_add(2)) {
            let entry = cursor.get(&self.dev, &mut probe, index)?;
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
    ///
    /// The cursor and its window are the caller's, so a commit's two passes share one of each: the
    /// second pass over a catalog that fits inside a single window costs no read at all, the two passes
    /// can never disagree about which copy they are reading, and the frame carries one window rather
    /// than one per pass.
    fn merge<F>(
        &self,
        cursor: &mut EntryCursor,
        window: &mut [u8],
        plan: &[Resolved],
        mut emit: F,
    ) -> Result<u16, StoreError>
    where
        F: FnMut(&Entry) -> Result<(), StoreError>,
    {
        let mut order: [u8; MAX_BATCH] = [0; MAX_BATCH];
        for (index, slot) in order.iter_mut().enumerate().take(plan.len()) {
            *slot = index as u8;
        }
        let order = &mut order[..plan.len()];
        order.sort_unstable_by_key(|index| plan[*index as usize].key);

        let mut next = 0usize;
        let mut written = 0u16;
        for index in 0..self.entry_count {
            let entry = cursor.get(&self.dev, window, index)?;
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
                // Both counters stop one short of wrapping: nothing could supersede revision
                // `u64::MAX`, and §5.2's cursor must end up strictly greater than every id in the
                // array, which `u64::MAX` leaves no room for.
                if meta.revision.0 == u64::MAX || meta.id.0 == u64::MAX {
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
                        // §5.2's cursor never rewinds, so an id below it named an object once and may
                        // never name another. Without this the compare-and-swap below would wave a
                        // retired identity through — `find` comes back empty, so the expected revision
                        // is `1` again — and the hold table keys on `(ObjectId, Revision)`: a key
                        // re-created over different extents would serve a removed object's bytes to a
                        // reader that opened the live one. A create names `next_object_id()`.
                        if retained.is_none() && head.is_none() && meta.id.0 < self.next_object {
                            return Err(StoreError::Invalid);
                        }
                        // A revision that already exists is caught by the compare-and-swap below
                        // rather than by a check of its own: the head is either this revision, in
                        // which case one past it is not it, or a greater one.
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
        let holds = self.holds.borrow();
        for resolved in plan {
            // Both branches ask, because both take extents away from a reader: a removal takes the
            // whole entry, and an amend that trims a reserve takes its tail. `close` works out which
            // of a hold's extents the catalog has stopped naming and frees exactly those.
            let held = holds.iter().flatten().any(|hold| (hold.id, hold.revision) == resolved.key);
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
        // As with the hold table: no free row is `busy` on the wire, not `invalidRequest`.
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
        // A fragmented allocation is several writes, so one of them can fail with the others already on
        // the card. The row's cursor goes back where it was: it is the reservation's identity as much as
        // its position — `row` matches an `Allocation` on it — so a cursor left ahead of the caller's
        // would make the reservation unnameable, which is a row and its extents wedged until the next
        // mount, `cancel` included. Rewinding costs nothing instead: the bytes already on the card are
        // the bytes the retry writes there.
        let start = row.written;
        if let Err(error) = fill(dev, row, bytes) {
            row.written = start;
            return Err(error);
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

        // Everything the batch would write, checked against §5.3 before the card is touched — which
        // means a second pass over the live prefix, and is worth it for a reason stronger than §2.1's
        // "a commit that returns Err changed nothing". §2.1 is about *observable* state, and the
        // inactive copy is not observable. What the alternative would actually spend is the
        // **redundancy**: validating while writing means a refused batch has already invalidated the
        // other copy's gate and scribbled its body, so until the next commit succeeds the card is one
        // torn serving copy away from having no catalog at all. Today a refused batch touches the card
        // not at all, which `a_refused_batch_leaves_the_catalog_untouched` pins by asserting the other
        // copy's gate block is still zeros.
        //
        // The pass is not free and not the largest thing here either: at 1,024 entries it is 32 of the
        // commit's 72 read commands, and the M33's per-entry work is the bigger term (`flat::cost`).
        // §5.5 step 2 continues from the high-water mark, and there is nothing past `u64::MAX` to
        // continue to. A mount at that mark refuses writes outright (`Mode::SequenceSpaceExhausted`),
        // and so does the store from the commit that reaches it — but the arithmetic is checked here
        // too, because a refusal is the only admissible answer and a panic is not one.
        let sequence = self.high_water.checked_add(1).ok_or(StoreError::ReadOnly)?;
        let header = Header {
            store: self.store,
            sequence,
            next_object: self.next_object.max(greatest_id + 1),
            entry_count: count as u16,
        };
        let mut structure = Structure::default();
        // The commit's two windows, owned here and lent out: see [`EntryCursor`] for why they are not
        // owned by the reader and the writer that use them.
        let mut scan = [0u8; STREAM_WINDOW];
        let mut cursor = EntryCursor::new(self.serving, self.extents, self.entry_count);
        let written =
            self.merge(&mut cursor, &mut scan, plan, |entry| structure.accept(entry).map_err(|_| StoreError::Invalid))?;
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
        // §7.2's ride end: the last checkpoint's tail is on the card in a journal slot, not in the
        // ride's extents, and this is the commit that gives those bytes a length and a CRC. So the
        // partial page moves out of the slot and into the extents here — the same "payload
        // synchronized before the commit begins" rule the staging flush above obeys.
        staged |= self.flush_ride_tail(plan)?;
        if staged {
            sync(&self.dev)?;
        }

        let target = 1 - self.serving;
        write_blocks(&self.dev, catalog_gate(target), &INVALIDATED)?;
        sync(&self.dev)?;

        // §5.5 step 2's body, header block first and the entries after it, through one window: the
        // header is block 0 of the body and the first bytes its CRC covers, so streaming it here is
        // what makes a small catalog one card command rather than two.
        let mut stage = [0u8; STREAM_WINDOW];
        let mut writer = BodyWriter::new(&self.dev, CATALOG[target], &mut stage);
        writer.push_header(&header.encode())?;
        self.merge(&mut cursor, &mut scan, plan, |entry| writer.push(entry))?;
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
        // The counter that ran out mid-session, from the same rule §5.6 applies at mount: a store whose
        // high-water mark has reached `u64::MAX` has no sequence for the next commit, so this is the
        // last one this card accepts. Read-only from here, reads still served.
        if self.high_water == u64::MAX {
            self.mode = Mode::SequenceSpaceExhausted;
        }
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
            // An amend keeps the key and changes the metadata — a ride finalising is exactly that —
            // so a reader joining an existing row takes the entry just read, not the one the first
            // reader found. §2.1 promises a handle keeps reading the revision it resolved; it does not
            // promise a second handle inherits a stale length.
            //
            // The length, and only the length: the row keeps the extents the *first* reader resolved.
            // An amend can only trim, and a trim keeps a prefix, so the wider ranges serve every byte
            // of the amended length — while narrowing them here would lose the trimmed tail, which
            // `release` deferred to `close` precisely because this row exists. Nothing would free it
            // until the next mount.
            hold.payload_len = entry.meta.payload_len;
            return Ok(Handle { slot: slot as u8, id: entry.meta.id, revision: entry.meta.revision });
        }
        // A full table is transient: some other reader is holding all twelve rows, and the answer is
        // to ask again rather than to reject the request. The wire face is `busy`, not
        // `invalidRequest` — `StoreError` has no variant of its own for it.
        let slot = holds.iter().position(Option::is_none).ok_or(StoreError::Invalid)?;
        holds[slot] = Some(Hold {
            id: entry.meta.id,
            revision: entry.meta.revision,
            ranges: entry.ranges,
            payload_len: entry.meta.payload_len,
            readers: 1,
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
        self.listing_failed.set(false);
        Entries {
            dev: &self.dev,
            cursor: EntryCursor::new(self.serving, self.extents, self.entry_count),
            buf: [0; BLOCK],
            index: 0,
            count: self.entry_count,
            failed: &self.listing_failed,
        }
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
        // The flushed length advances in this local and becomes resident only once the slot that
        // accounts for those bytes is durable, below. A checkpoint that fails partway has flushed
        // nothing as far as the store is concerned: the caller still holds the whole tail, and its retry
        // rewrites the same pages with the same bytes at the same offsets (§7.2 — the payload of a
        // flushed prefix cannot change, which is why recovery is allowed to rewrite a page too). A
        // flushed length left one page ahead of the card's would put the retry's bytes at the wrong
        // payload offset and publish a ride twice the length it recorded.
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
            tail = &tail[PROGRAM_PAGE..];
        }
        // The loop above leaves less than one program page, which is well inside §7.1's 32,256-byte
        // tail area — so the slot's bound needs no check here, only on the way back in.

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
        // §7.1 calls a slot "written in one shot": what that requires is that no reader ever sees a
        // subset of it, and the single sync below is what provides it. The header, the tail and the pad
        // are separate writes because the tail is the caller's slice and the pad is in rodata — but they
        // are all pending until that sync, and a cut that commits any subset of them leaves bytes the
        // whole-slot CRC does not cover. A torn slot is skipped, exactly as a torn one-write slot is.
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
    /// The entry a batch is finalising the live ride with: the `Put` that names the recording entry's
    /// key and clears `RECORDING`. A `Remove` of that key is not one — the object is going away, and
    /// so are its bytes.
    fn finalising(&self, plan: &[Resolved]) -> Option<(RideState, Entry)> {
        let ride = self.ride?;
        let entry = plan
            .iter()
            .filter_map(|resolved| resolved.entry)
            .find(|entry| entry.meta.key() == (ride.id, ride.revision))?;
        (!entry.meta.flags.has(EntryFlags::RECORDING)).then_some((ride, entry))
    }

    /// Moves the bytes past `flushed length` out of the newest journal slot and into the ride's own
    /// extents, so the length and CRC the finalising commit publishes describe bytes that are on the
    /// card. Reports whether anything was written.
    ///
    /// Everything up to `flushed length` is already there — §7.2 wrote it a page at a time — and the
    /// remainder is the partial page no checkpoint ever flushes, because a checkpoint only ever writes
    /// whole 16 KiB pages. The slot is where those bytes live, so the slot is where they come from:
    /// re-reading it costs 63 blocks once per ride and needs no buffer of its own.
    ///
    /// `&self`, which it always could have been: it moves bytes on the card and settles no resident
    /// state — that is [`settle_ride`](Self::settle_ride)'s job, after the gate. Saying so is now load
    /// bearing as well as honest, because the commit around this call holds an [`EntryReader`] over
    /// `self.dev` across it.
    fn flush_ride_tail(&self, plan: &[Resolved]) -> Result<bool, StoreError> {
        let Some((ride, entry)) = self.finalising(plan) else { return Ok(false) };
        let length = entry.meta.payload_len;
        if length == ride.flushed {
            return Ok(false);
        }
        // A shorter object than the ride already flushed cannot be a finalisation of it, and neither
        // can one whose tail no slot holds: `next_sequence` is `1` only before the first checkpoint.
        if length < ride.flushed || ride.next_sequence == 1 {
            return Err(StoreError::Invalid);
        }
        let tail_len = length - ride.flushed;
        if tail_len > TAIL_CAPACITY as u64 {
            return Err(StoreError::Invalid);
        }
        let slot_index = ((ride.next_sequence - 1) % SLOTS as u64) as usize;
        let mut block = [0u8; BLOCK];
        read_blocks(&self.dev, slot_block(slot_index), &mut block)?;
        let slot = Slot::decode(&block, slot_index, &self.store, self.extents).map_err(|_| StoreError::Invalid)?;
        // The slot has to be this ride's, at this flush point, holding exactly the tail the caller is
        // publishing a length for. Anything else and the commit would describe bytes it cannot produce.
        // The comparison is against the *reserve* the ride is recording into, not against the entry
        // being written — that one's ranges are already trimmed to the finalised payload.
        if (slot.id, slot.revision, slot.ranges) != (ride.id, ride.revision, ride.ranges)
            || slot.flushed != ride.flushed
            || u64::from(slot.tail_len) != tail_len
        {
            return Err(StoreError::Invalid);
        }

        let base = slot_block(slot_index) + 1;
        let mut done = 0u64;
        while done < tail_len {
            read_blocks(&self.dev, base + done / BLOCK as u64, &mut block)?;
            let located = ride.ranges.locate(ride.flushed + done).ok_or(StoreError::Invalid)?;
            // A whole block goes out even for a partial tail: the bytes past `payload_len` are slack
            // inside the ride's last extent, which nothing reads and no CRC covers.
            write_blocks(&self.dev, located.block, &block)?;
            done += BLOCK as u64;
        }
        Ok(true)
    }

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
    dev: &'a D,
    cursor: EntryCursor,
    /// One block, not a window: the listing is paced by the wire that drains it, and widening it here
    /// would grow the frame of every caller holding this iterator — `LIST`'s, above the seam. Owned
    /// rather than lent, because this iterator outlives the call that built it.
    buf: [u8; BLOCK],
    index: u16,
    count: u16,
    failed: &'a Cell<bool>,
}

impl<D: BlockDevice> Iterator for Entries<'_, D> {
    type Item = EntryMeta;

    fn next(&mut self) -> Option<EntryMeta> {
        if self.index >= self.count {
            return None;
        }
        // A read failure ends the listing, because the signature has nowhere to put an error — but it
        // does not end it *silently*: `entries_ok` is how the caller learns the list is short.
        let Ok(entry) = self.cursor.get(self.dev, &mut self.buf, self.index) else {
            self.failed.set(true);
            self.index = self.count;
            return None;
        };
        self.index += 1;
        Some(entry.meta)
    }
}
