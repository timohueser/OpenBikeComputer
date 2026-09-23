//! The store: mount, initialization, the alternating catalog commit, the ride journal's write half
//! and the seam operations (`FLAT_Store_Format.md`, `FLAT_Store_Protocol.md`). The entry array stays
//! on the card; resident state is the 8 KiB free bitmap and a few rows.
//!
//! Every operation takes `&self`, mutators included, and the resident state that moves lives behind
//! cells. Card commands run with no cell borrow held, so a storage task can interleave a render read
//! into a commit's gaps; [`granularity`](super::granularity) enforces that from inside the block
//! driver. Two exceptions: [`write`](Store::write) and `commit` hold `reservations` across their
//! staging flush, which is safe only because writers are serialized by construction, and
//! [`load`](FlatStore::load) holds the free map through its scan while nothing else can reach the
//! store. Borrows are `borrow_mut`, not `try_borrow_mut`: no borrow here spans a call back into the
//! store, and a borrow panic on the device is a hard fault.

use core::cell::{Cell, RefCell};

use obc_crc::Crc32;

use super::bitmap::FreeMap;
use super::catalog::{Entry, Gate, Header, Structure, INVALIDATED};
use super::device::BlockDevice;
use super::error::StoreError;
use super::journal::{self, Slot, TAIL_CAPACITY, ZERO_PAD};
use super::layout::{
    catalog_gate, slot_block, slot_header_block, Geometry, Ranges, BLOCK, CATALOG, ENTRIES_PER_BLOCK, ENTRY_CAPACITY,
    ENTRY_STRIDE, EXTENT_AREA, MAX_RANGES, MOUNT_STREAM_BLOCKS, MOUNT_STREAM_WINDOW, PROGRAM_PAGE, SLOTS, SLOT_BLOCKS,
    STREAM_WINDOW, SUPERBLOCK,
};
use super::seam::{
    Allocation, EntryFlags, EntryMeta, Mutation, ObjectId, PutSource, Revision, RideCheckpoint, Store, StoreId,
    RIDE_RESUME_LEN,
};
use super::superblock::Superblock;

/// Entry mutations one commit carries. The largest real batch is two; four leaves margin.
pub const MAX_BATCH: usize = 4;
/// Two reservations serve a transfer plus a recording start, or a sealed detour leg plus its
/// trim/splice output. A competing start or transfer is refused while both rows are occupied.
pub const MAX_RESERVATIONS: usize = 2;

pub mod open_objects {
    pub const MAP: usize = 1;
    /// The active route's geometry, held from load until the ride ends. A detour retains another
    /// reference to this exact revision and shares the row.
    pub const ROUTE: usize = 1;

    /// The one transfer the protocol admits at a time, which may run mid-ride.
    pub const TRANSFER: usize = 1;
    /// A row that belongs to nobody, so a short-lived open never waits for a session-long holder.
    pub const SPARE: usize = 1;
    /// The spare row acquire-before-release needs, which a census of live opens cannot see.
    pub const SWAP: usize = 1;

    pub const ACCOUNTED: usize = MAP + ROUTE + TRANSFER + SPARE + SWAP;
}

/// Open objects at once — the sum of [`open_objects`]'s rows, and nothing else.
pub const MAX_OPEN_OBJECTS: usize = 5;

// A module-level `const`, not an associated one: an associated `const` is evaluated lazily, so a
// table that stopped adding up would compile silently until something named it.
const _: () = assert!(
    open_objects::ACCOUNTED == MAX_OPEN_OBJECTS,
    "MAX_OPEN_OBJECTS must equal the sum of `open_objects`'s rows: add a named row for the new \
     session-long open and raise the constant to match",
);
// `Handle::slot` is a `u8` and the holds array is indexed by it.
const _: () = assert!(MAX_OPEN_OBJECTS <= u8::MAX as usize);
// Journal snapshots are reconstructed one [`ZERO_PAD`] window at a time. A partial final window
// would need a second buffer shape.
const _: () = assert!((SLOT_BLOCKS as usize * BLOCK).is_multiple_of(ZERO_PAD.len()));

/// Why a mounted store refuses writes. A store that mounted read-only never becomes writable
/// without initialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    ReadWrite,
    /// An object reached revision `u64::MAX`, so nothing can supersede it. Reads are still served.
    /// Wire face: `readOnly` / `revisionSpaceExhausted 2`.
    RevisionSpaceExhausted,
    /// A well-formed gate carries commit sequence `u64::MAX`, so there is no sequence to continue
    /// to. Reads are still served. Wire face: `readOnly` / `revisionSpaceExhausted 2`.
    SequenceSpaceExhausted,
    /// No catalog gate is well-formed, no candidate body validated, or required ride rollover
    /// repair failed. Evidence is preserved for the next mount.
    /// Wire face: `readOnly` / `catalogUnreadable 1`.
    CatalogUnreadable,
    /// The final catalog gate write or barrier failed and can have published a new catalog. Only
    /// existing handles remain readable. Wire face: `readOnly` / `catalogUnreadable 1`.
    RemountRequired,
    /// The card is not a flat store. Initialization is the only transition.
    /// Wire face: `readOnly` / `unformatted 3`.
    Unformatted,
    /// The card is smaller than the superblock recorded: damaged or swapped, never silently
    /// truncated. Wire face: `readOnly` / `unformatted 3`.
    CardTooSmall,
}

const _: () = assert!(core::mem::size_of::<Mode>() == 1, "mount mode stays in its existing byte");

impl Mode {
    pub fn writable(self) -> bool {
        self == Mode::ReadWrite
    }

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

/// One contiguous run of absolute 512-byte card blocks, as [`FlatStore::block_runs`] resolves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockRun {
    pub start_block: u64,
    pub blocks: u64,
}

/// What a ride recovery found. The payload CRC is the seed the resumed session continues its
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
    /// Recorder-owned state CRC-covered by the selected logical checkpoint.
    pub resume: [u8; RIDE_RESUME_LEN],
    /// The physical slot it came from, which is `checkpoint_sequence mod 16`.
    pub(super) slot: u16,
}

impl RideRecovery {
    pub fn payload_len(&self) -> u64 {
        self.flushed + self.tail_len as u64
    }
}

#[derive(Debug, Clone, Copy)]
struct Hold {
    id: ObjectId,
    revision: Revision,
    /// The extents this reader resolved. A commit may have taken them out of the entry since, and
    /// until the last reader closes they stay out of the allocator.
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

/// An unpublished immutable payload. This owner occupies one reservation and no hold row.
/// Release it through [`FlatStore::release_sealed`]; dropping it leaves the reservation taken.
#[derive(Debug)]
pub struct SealedAllocation<'a> {
    allocation: Allocation,
    ranges: Ranges,
    mount: usize,
    _mount: core::marker::PhantomData<&'a ()>,
}

impl SealedAllocation<'_> {
    pub fn len(&self) -> u64 {
        self.allocation.written
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, Copy)]
struct RideState {
    id: ObjectId,
    revision: Revision,
    ranges: Ranges,
    flushed: u64,
    next_sequence: u64,
    /// Tail bytes and payload CRC of the newest durable header. These anchors let `journal` derive
    /// and verify rollover CRCs without rereading the ride.
    tail_len: u32,
    payload_crc: u32,
    resume: [u8; RIDE_RESUME_LEN],
    /// Nonzero after both rollover gates are durable but before their proof page is confirmed in
    /// the payload extent. A retry repairs from this proof without rewriting either gate.
    pending_proof: u64,
    /// Caller append identity already incorporated by the pending logical gate. Length plus the
    /// delta's own CRC make the retry contract explicit.
    pending_append_len: u32,
    pending_append_crc: u32,
}

struct SlotWrite<'a> {
    sequence: u64,
    flushed: u64,
    /// The previous logical slot copied into this one. `None` starts a new page after rollover (or
    /// the ride's first checkpoint); `append` follows its logical tail.
    source: Option<Slot>,
    append: &'a [u8],
    payload_crc: u32,
    proof: bool,
    proof_sequence: u64,
    resume: &'a [u8; RIDE_RESUME_LEN],
}

/// The catalog the store is serving, and the counters that move with it. One `Copy` value in one
/// [`Cell`], because the gate write is the instant all of them become true together.
#[derive(Debug, Clone, Copy)]
struct Served {
    mode: Mode,
    /// The copy the store is serving. A commit targets the other one.
    copy: usize,
    sequence: u64,
    /// The greatest commit sequence any well-formed gate carried, which is what a commit
    /// continues from — not the sequence of the copy that happened to validate.
    high_water: u64,
    next_object: u64,
    entry_count: u16,
    /// Body fingerprint from the gate that selected this retained catalog.
    body_crc: u32,
}

pub struct FlatStore<D> {
    dev: D,
    store: StoreId,
    /// The card's own extent size, read from its superblock and the source of every address below.
    geometry: Geometry,
    extents: u32,
    served: Cell<Served>,
    free: RefCell<FreeMap>,
    holds: RefCell<[Option<Hold>; MAX_OPEN_OBJECTS]>,
    reservations: RefCell<[Option<Reservation>; MAX_RESERVATIONS]>,
    nonce: Cell<u32>,
    ride: Cell<Option<RideState>>,
    recovered: Cell<Option<RideRecovery>>,
    /// Set when the [`Store::entries`] iterator hit a media failure. The listing has nowhere to put
    /// an error, so [`entries_ok`](Self::entries_ok) is how a caller finds out its listing was short.
    listing_failed: Cell<bool>,
    route_added_at: Cell<u32>,
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

/// The reservation an [`Allocation`] names, or `None` when the token names no live row: a stale slot,
/// a row that has been cancelled and re-taken, or a cursor that has moved out from under the caller.
fn row_of<'a>(rows: &'a [Option<Reservation>; MAX_RESERVATIONS], allocation: &Allocation) -> Option<&'a Reservation> {
    rows.get(allocation.slot as usize)?.as_ref().filter(|row| {
        (row.nonce, row.written, row.reserved) == (allocation.nonce, allocation.written, allocation.reserved)
    })
}

/// Appends `input` to a reservation, one contiguous run per pass: whole blocks straight out of the
/// caller's slice, and a partial one through the row's staging block. The cursor it advances belongs
/// to the caller's [`Allocation`], which is why [`Store::write`] rewinds it when this fails.
fn fill<D: BlockDevice>(
    dev: &D,
    geometry: Geometry,
    row: &mut Reservation,
    mut input: &[u8],
) -> Result<(), StoreError> {
    while !input.is_empty() {
        let staged = (row.written % BLOCK as u64) as usize;
        let located = row.ranges.locate(geometry, row.written - staged as u64).ok_or(StoreError::Invalid)?;
        if staged == 0 && input.len() >= BLOCK {
            // Bounded in `u64` and narrowed after — see [`Located::whole_blocks`]. Narrowing first
            // is a hang on the device.
            let blocks = located.whole_blocks(input.len());
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

/// Where a walk of one catalog copy's entry array has got to. The window is the caller's buffer,
/// because a reader that owned its window cost a second window of frame and this module's frames are
/// measured. A cursor and its buffer are a pair, so every call site keeps them adjacent.
///
/// A scan lends [`STREAM_WINDOW`]; a binary search lends one [`BLOCK`], because its probes are
/// scattered and a wide window would read 4 KiB to look at 128 bytes of it.
struct EntryCursor {
    base: u64,
    extents: u32,
    /// Blocks the live prefix occupies. The window is clamped to it so a short catalog is never read
    /// wider than it is.
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
        // A cursor paired with a narrower buffer than the one that filled it would index past the
        // end of it. Treat a window the buffer cannot hold as a miss and re-read.
        let held = self.cached.filter(|(first, count)| {
            block >= *first && block - *first < *count && *count <= (buf.len() / BLOCK) as u64
        });
        let first = match held {
            Some((first, _)) => first,
            None => {
                // The window starts at the block asked for and runs forward, so a walk of ascending
                // indices never re-reads a block it has already seen.
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

/// Writes a catalog body: the header block and then the entries stream through it, folded into the
/// body CRC the gate will carry.
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
    fn new(dev: &'a D, block: u64, buf: &'b mut [u8]) -> Self {
        debug_assert!(!buf.is_empty() && buf.len().is_multiple_of(BLOCK), "a window is whole blocks");
        BodyWriter { dev, block, buf, filled: 0, digest: Crc32::new() }
    }

    /// The header block is block 0 of the body and the first thing the CRC covers. It goes through
    /// the window so that a body of one header and a few entries is one card command.
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

    /// Writes the whole blocks the window holds, and only those. A short window is padded, but the
    /// write never runs past the last block the body occupies: the block after it is the copy's gate.
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
    reservation: Option<u8>,
}

impl<D: BlockDevice> FlatStore<D> {
    /// Supply a trusted UTC time for route publications. Unknown time leaves age unknown.
    pub fn set_route_added_at(&self, utc: Option<u32>) {
        self.route_added_at.set(utc.unwrap_or(0));
    }

    fn read_ranges(&self, ranges: &Ranges, payload_len: u64, offset: u64, buf: &mut [u8]) -> Result<usize, StoreError> {
        if offset >= payload_len {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(payload_len - offset) as usize;
        let mut done = 0usize;
        let mut block = [0u8; BLOCK];
        while done < want {
            let located = ranges.locate(self.geometry, offset + done as u64).ok_or(StoreError::Invalid)?;
            // The same narrowing rule as [`Located::whole_blocks`], one unit up: the bound is taken
            // in `u64` against a byte count that can exceed a device `usize`.
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

    /// Patch bytes already appended to a live, unpublished allocation.
    ///
    /// The on-device OBCR emitter backfills its streamed header after geometry and index emission, so
    /// the board needs one bounded random write before publication. No committed object is
    /// addressable here.
    pub fn patch_allocation(&self, allocation: &Allocation, offset: u64, bytes: &[u8]) -> Result<(), StoreError> {
        if !self.mode().writable() {
            return Err(StoreError::ReadOnly);
        }
        let end = offset.checked_add(bytes.len() as u64).ok_or(StoreError::Invalid)?;
        if end > allocation.written {
            return Err(StoreError::Invalid);
        }
        let mut rows = self.reservations.borrow_mut();
        if row_of(&rows, allocation).is_none() {
            return Err(StoreError::Invalid);
        }
        let row = rows[allocation.slot as usize].as_mut().expect("the row was just validated");
        let partial = (row.written % BLOCK as u64) as usize;
        let staged_at = row.written - partial as u64;
        let mut done = 0usize;
        let mut block = [0u8; BLOCK];
        while done < bytes.len() {
            let at = offset + done as u64;
            let block_at = at - at % BLOCK as u64;
            let within = (at - block_at) as usize;
            let take = (BLOCK - within).min(bytes.len() - done);
            if partial != 0 && block_at == staged_at {
                row.staging[within..within + take].copy_from_slice(&bytes[done..done + take]);
            } else {
                let located = row.ranges.locate(self.geometry, block_at).ok_or(StoreError::Invalid)?;
                read_blocks(&self.dev, located.block, &mut block)?;
                block[within..within + take].copy_from_slice(&bytes[done..done + take]);
                write_blocks(&self.dev, located.block, &block)?;
            }
            done += take;
        }
        Ok(())
    }

    /// Flush the final partial block and revoke every writable copy of this allocation. On failure
    /// the supplied token remains valid. Sealing publishes no catalog entry and makes no persistence
    /// claim: this is temporary storage until remount.
    pub fn seal(&self, allocation: Allocation) -> Result<SealedAllocation<'_>, StoreError> {
        if !self.mode().writable() {
            return Err(StoreError::ReadOnly);
        }
        let mut rows = self.reservations.borrow_mut();
        row_of(&rows, &allocation).ok_or(StoreError::Invalid)?;
        let row = rows[allocation.slot as usize].as_mut().expect("validated reservation");
        let partial = (row.written % BLOCK as u64) as usize;
        if partial != 0 {
            let located = row.ranges.locate(self.geometry, row.written - partial as u64).ok_or(StoreError::Invalid)?;
            row.staging[partial..].fill(0);
            write_blocks(&self.dev, located.block, &row.staging)?;
        }
        let nonce = self.nonce.get().wrapping_add(1);
        self.nonce.set(nonce);
        row.nonce = nonce;
        Ok(SealedAllocation {
            allocation: Allocation { nonce, ..allocation },
            ranges: row.ranges,
            mount: self as *const Self as usize,
            _mount: core::marker::PhantomData,
        })
    }

    /// Consume the sole cleanup capability. A fenced mount retains the reservation until remount.
    pub fn release_sealed<'a>(&self, sealed: SealedAllocation<'a>) -> Result<(), SealedAllocation<'a>> {
        if sealed.mount != self as *const Self as usize {
            return Err(sealed);
        }
        self.cancel(sealed.allocation);
        Ok(())
    }

    pub(super) fn read_sealed(
        &self,
        sealed: &SealedAllocation<'_>,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, StoreError> {
        if sealed.mount != self as *const Self as usize {
            return Err(StoreError::Invalid);
        }
        // The immutable snapshot needs neither reservation nor hold table, so a writer can keep the
        // other reservation borrowed across its card command while this read runs.
        self.read_ranges(&sealed.ranges, sealed.len(), offset, buf)
    }

    /// CRC-32/IEEE of the bytes appended to a live allocation, including its unflushed tail. This is
    /// the on-device producer's final verification pass after its header patch.
    pub fn allocation_crc(&self, allocation: &Allocation) -> Result<u32, StoreError> {
        let rows = self.reservations.borrow();
        let row = row_of(&rows, allocation).ok_or(StoreError::Invalid)?;
        let partial = (row.written % BLOCK as u64) as usize;
        let flushed = row.written - partial as u64;
        let mut digest = Crc32::new();
        let mut window = [0u8; STREAM_WINDOW];
        let mut done = 0u64;
        while done < flushed {
            let located = row.ranges.locate(self.geometry, done).ok_or(StoreError::Invalid)?;
            let take = (flushed - done).min(located.contiguous).min(STREAM_WINDOW as u64) as usize;
            debug_assert!(take.is_multiple_of(BLOCK));
            read_blocks(&self.dev, located.block, &mut window[..take])?;
            digest.update(&window[..take]);
            done += take as u64;
        }
        digest.update(&row.staging[..partial]);
        Ok(digest.finalize())
    }

    /// Superblock, gates, one body, the free bitmap, and the ride journal only when an entry says a
    /// ride was recording. There is no journal replay, no garbage collection and no recovery scan.
    ///
    /// A card this cannot bring up mounts read-only rather than failing to exist: initialization is
    /// the only transition into this format.
    ///
    /// Must stay `#[inline(never)]`: CI measures this frame, and an inlined `mount` is charged to its
    /// caller's symbol instead. It also keeps the indirect-return ABI, so a caller hands in the
    /// destination and the ~10.5 KB store is built there rather than copied.
    #[inline(never)]
    pub fn mount(dev: D) -> Self {
        let mut store = Self::blank(dev);
        store.bring_up();
        store
    }

    /// [`mount`](Self::mount), into a slot the caller owns — for a caller that cannot afford a
    /// ~10.5 KB value to exist on a stack even for the length of a move. `mount` returns by value, and
    /// LLVM builds that value as a local and `memcpy`s it into the destination.
    ///
    /// Must stay `#[inline(never)]` for both of `mount`'s reasons.
    #[inline(never)]
    pub fn mount_in_place(slot: &mut core::mem::MaybeUninit<Self>, dev: D) -> &mut Self {
        // `blank` is `#[inline(always)]`, so the literal is written through this pointer rather than
        // built beside it.
        let store = slot.write(Self::blank(dev));
        store.bring_up();
        store
    }

    /// The struct literal, before [`bring_up`](Self::bring_up) has read a single block.
    ///
    /// `#[inline(always)]` so [`mount_in_place`](Self::mount_in_place) writes it through the caller's
    /// pointer rather than building it and copying. The `const` blocks keep the 8 KiB blank free map
    /// in `.rodata` instead of materialising it in a stack temporary, which this frame measures.
    #[inline(always)]
    fn blank(dev: D) -> Self {
        FlatStore {
            dev,
            store: StoreId([0; 16]),
            geometry: Geometry::DEFAULT,
            extents: 0,
            served: Cell::new(Served {
                mode: Mode::Unformatted,
                copy: 0,
                sequence: 0,
                high_water: 0,
                next_object: 0,
                entry_count: 0,
                body_crc: 0,
            }),
            free: const { RefCell::new(FreeMap::BLANK) },
            holds: const { RefCell::new([None; MAX_OPEN_OBJECTS]) },
            reservations: const { RefCell::new([None; MAX_RESERVATIONS]) },
            nonce: Cell::new(0),
            ride: Cell::new(None),
            recovered: Cell::new(None),
            listing_failed: Cell::new(false),
            route_added_at: Cell::new(0),
        }
    }

    /// Explicit, destructive, and the only transition into this format. The superblocks are
    /// destroyed first and written last, so a valid superblock implies a valid catalog.
    ///
    /// The card's extent size is decided here — `max(1 MiB, card / 65,536)` rounded up to a power of
    /// two — and the superblock is the only place it is ever written.
    pub fn initialize(dev: D, store: StoreId) -> Result<Self, StoreError> {
        Self::write_empty_store(&dev, store)?;
        let store = Self::mount(dev);
        if store.mode().writable() {
            Ok(store)
        } else {
            Err(StoreError::Media)
        }
    }

    pub fn format_media(&self, store: StoreId) -> Result<(), StoreError> {
        if self.mode() == Mode::RemountRequired {
            return Err(StoreError::ReadOnly);
        }
        Self::write_empty_store(&self.dev, store)
    }

    fn write_empty_store(dev: &D, store: StoreId) -> Result<(), StoreError> {
        let total_blocks = dev.block_count().map_err(|_| StoreError::Media)?;
        let superblock = Superblock::for_card(store, total_blocks).ok_or(StoreError::Invalid)?;
        for copy in SUPERBLOCK {
            write_blocks(dev, copy, &INVALIDATED)?;
        }
        sync(dev)?;

        write_blocks(dev, catalog_gate(1), &INVALIDATED)?;
        sync(dev)?;

        for slot in 0..SLOTS {
            write_blocks(dev, slot_header_block(slot), &INVALIDATED)?;
        }
        sync(dev)?;

        let header = Header { store, sequence: 1, next_object: 1, entry_count: 0 };
        let body = header.encode();
        write_blocks(dev, CATALOG[0], &body)?;
        sync(dev)?;
        let gate = Gate { copy: 0, store, sequence: 1, entry_count: 0, body_crc: super::raw::crc32(&body) };
        write_blocks(dev, catalog_gate(0), &gate.encode())?;
        sync(dev)?;

        let superblock = superblock.encode();
        for copy in SUPERBLOCK {
            write_blocks(dev, copy, &superblock)?;
        }
        sync(dev)
    }

    /// Complete a durability barrier without exposing the card to callers above the store.
    /// This does not validate the catalog or clear a remount requirement.
    pub fn sync_media(&self) -> Result<(), StoreError> {
        sync(&self.dev)
    }

    /// Stop mutations and fresh catalog reads after uncertain publication or verification.
    /// Existing handles keep their ranges until they close; only a fresh mount clears this mode.
    pub(crate) fn require_remount(&self) {
        self.served.set(Served { mode: Mode::RemountRequired, ..self.served.get() });
    }

    pub fn mode(&self) -> Mode {
        self.served.get().mode
    }

    /// The card's identity. A client that has not seen it must treat its whole cache as void.
    pub fn store_id(&self) -> StoreId {
        self.store
    }

    /// The identity and catalog marks that make this retained mount safe to reuse.
    pub fn mounted_media_state(&self) -> super::MountedMediaState {
        let served = self.served.get();
        super::MountedMediaState {
            store: self.store,
            extent_size: self.geometry.extent_size(),
            extent_count: self.extents,
            served: super::CatalogGateIdentity {
                copy: served.copy as u8,
                sequence: served.sequence,
                entry_count: served.entry_count,
                body_crc: served.body_crc,
            },
            high_water: served.high_water,
        }
    }

    /// The catalog commit sequence — the staleness hint a client compares its listing against.
    pub fn sequence(&self) -> u64 {
        self.served.get().sequence
    }

    /// Whether the current catalog has sequence space for `count` further commits.
    ///
    /// This checks the greatest well-formed gate rather than [`Self::sequence`]: a mount can fall back
    /// to the older catalog copy, but the next commit must still continue past the newer gate.
    /// Callers that publish an object which may need a compensating removal use `count == 2`.
    pub fn has_commit_capacity(&self, count: u64) -> bool {
        let served = self.served.get();
        served.mode.writable() && served.high_water.checked_add(count).is_some()
    }

    /// Resolve the current readable head without acquiring another reader hold.
    pub fn current_revision(&self, id: ObjectId) -> Result<Option<Revision>, StoreError> {
        if !self.mode().readable() {
            return Err(StoreError::ReadOnly);
        }
        Ok(self
            .find(id)?
            .1
            .filter(|entry| {
                entry.meta.flags == EntryFlags::NONE
                    || (entry.meta.kind == super::ObjectKind::Route && entry.meta.flags.is_route_head())
            })
            .map(|entry| entry.meta.revision))
    }

    pub fn entry_count(&self) -> u16 {
        self.served.get().entry_count
    }

    /// True when the last [`Store::entries`] listing ran to the end of the array. Anything reporting
    /// a complete list asks here before it treats the list as the catalog.
    pub fn entries_ok(&self) -> bool {
        self.mode().readable() && !self.listing_failed.get()
    }

    /// The copy the store is serving. A card-layout fact with no caller above the seam.
    #[cfg(any(test, feature = "std"))]
    pub fn serving_copy(&self) -> usize {
        self.served.get().copy
    }

    /// The mark the next commit continues from, which a fallback mount leaves above the served
    /// sequence.
    #[cfg(any(test, feature = "std"))]
    pub fn high_water(&self) -> u64 {
        self.served.get().high_water
    }

    /// Free extents, each of this card's recorded extent size.
    pub fn free_extents(&self) -> u32 {
        self.free.borrow().free()
    }

    /// That size, in bytes. A caller that wants free bytes rather than free extents needs both.
    pub fn extent_size(&self) -> u64 {
        self.geometry.extent_size()
    }

    /// The absolute block runs one committed entry's extents occupy, in payload order, and how many
    /// runs there are.
    ///
    /// The one place above the seam that learns a block number. The firmware boot handoff has to
    /// name physical runs, because the bootloader that consumes them has no catalog and no store.
    /// A run is a whole extent range, so it also covers the slack past the payload's last byte.
    pub fn block_runs(
        &self,
        id: ObjectId,
        revision: Revision,
        out: &mut [BlockRun; MAX_RANGES],
    ) -> Result<usize, StoreError> {
        if !self.mode().readable() {
            return Err(StoreError::ReadOnly);
        }
        let (retained, head) = self.find(id)?;
        let entry = [retained, head]
            .into_iter()
            .flatten()
            .find(|entry| entry.meta.revision == revision)
            .ok_or(StoreError::NotFound)?;
        let blocks = self.geometry.extent_blocks();
        let mut count = 0usize;
        for (first, extents) in entry.ranges.iter() {
            out[count] = BlockRun { start_block: EXTENT_AREA + blocks * first as u64, blocks: blocks * extents as u64 };
            count += 1;
        }
        Ok(count)
    }

    /// The next `ObjectId` the cursor will hand out. Reading it reserves nothing: two creates that
    /// both read it before either commits would name the same id, and the second one's commit is
    /// refused as a duplicate key.
    pub fn next_object_id(&self) -> ObjectId {
        ObjectId(self.served.get().next_object)
    }

    /// What mount recovered, if a ride was recording when the card lost power.
    pub fn recovered_ride(&self) -> Option<RideRecovery> {
        self.recovered.get()
    }

    /// Random access over the checkpoint-durable bytes of the recording ride.
    ///
    /// Separate from [`Store::open`]: an entry carrying `RECORDING` still has the catalog length from
    /// ride start. The range can straddle the boundary between write-once payload pages and the
    /// selected journal tail, and this follows it without scanning the ride prefix.
    pub fn read_recovered(&self, offset: u64, buf: &mut [u8]) -> Result<usize, StoreError> {
        let recovered = self.recovered.get().ok_or(StoreError::NotFound)?;
        let ride = self.ride.get().filter(|ride| (ride.id, ride.revision) == (recovered.id, recovered.revision));
        let ride = ride.ok_or(StoreError::Invalid)?;
        let length = recovered.payload_len();
        if offset >= length {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(length - offset) as usize;
        let mut done = 0usize;
        let mut block = [0u8; BLOCK];

        while done < want && offset + (done as u64) < recovered.flushed {
            let at = offset + done as u64;
            let located = ride.ranges.locate(self.geometry, at).ok_or(StoreError::Invalid)?;
            let take = ((want - done) as u64).min(recovered.flushed - at).min((BLOCK - located.offset) as u64) as usize;
            read_blocks(&self.dev, located.block, &mut block)?;
            buf[done..done + take].copy_from_slice(&block[located.offset..located.offset + take]);
            done += take;
        }

        while done < want {
            let tail_at = offset + done as u64 - recovered.flushed;
            let within = tail_at as usize % BLOCK;
            read_blocks(&self.dev, slot_block(recovered.slot as usize) + tail_at / BLOCK as u64, &mut block)?;
            let take = (want - done).min(BLOCK - within);
            buf[done..done + take].copy_from_slice(&block[within..within + take]);
            done += take;
        }
        Ok(want)
    }

    /// Releases a reservation without publishing it. The bytes written into it are unreachable and
    /// their extents are free again immediately.
    ///
    /// A dropped `Allocation` releases nothing: the row and its extents stay taken until this is
    /// called or the card is remounted, and there are only [`MAX_RESERVATIONS`] rows.
    pub fn cancel(&self, allocation: Allocation) {
        if self.mode() == Mode::RemountRequired {
            return;
        }
        let mut rows = self.reservations.borrow_mut();
        let Some(row) = row_of(&rows, &allocation) else { return };
        let ranges = row.ranges;
        rows[allocation.slot as usize] = None;
        drop(rows);
        self.free.borrow_mut().release(&ranges);
    }

    /// Closes an open object. When the last reader lets go, every extent it was holding that the
    /// catalog no longer names goes back to the allocator, whether a commit removed the entry or only
    /// trimmed it.
    ///
    /// A caller that drops a `Handle` instead of closing it leaks the row and its extents until the
    /// next mount. One failure is silent: working out which extents the catalog still names is a media
    /// read, and a read that fails leaves those extents allocated rather than guessing.
    pub fn close(&self, handle: Handle) {
        let mut holds = self.holds.borrow_mut();
        let Some(hold) = holds[handle.slot as usize] else { return };
        if (hold.id, hold.revision) != (handle.id, handle.revision) {
            return;
        }
        // Another reader still holds this row, so this close spends a refcount and nothing else. The
        // row, its ranges and its length survive untouched.
        if hold.readers > 1 {
            holds[handle.slot as usize] = Some(Hold { readers: hold.readers - 1, ..hold });
            return;
        }
        holds[handle.slot as usize] = None;
        // Dropped before the `find` below: no borrow across a card command.
        drop(holds);
        if self.mode() == Mode::RemountRequired {
            return;
        }
        // What the entry still names, if it is still there at all. A media failure here leaves the
        // extents allocated until the next mount. A failed read is not evidence the entry is gone:
        // freeing a live entry's extents would let the next allocation overlap it, and an overlap is a
        // rule only a mount checks.
        let Ok((retained, head)) = self.find(hold.id) else { return };
        let live = [retained, head].into_iter().flatten().find(|entry| entry.meta.revision == hold.revision);
        let mut free = self.free.borrow_mut();
        for (first, count) in hold.ranges.iter() {
            for extent in first..first + count {
                if !live.is_some_and(|entry| entry.ranges.names(extent)) {
                    free.release_one(u32::from(extent));
                }
            }
        }
    }

    pub(crate) fn has_open_capacity(&self) -> bool {
        self.holds.borrow().iter().any(Option::is_none)
    }

    /// The payload length `handle` resolved, or `None` when it names a row that is no longer its own.
    /// This is the length the handle keeps reading, not the entry's current one: an amend that trimmed
    /// the entry since does not shorten a reader that is already past it.
    pub fn handle_len(&self, handle: &Handle) -> Option<u64> {
        let holds = self.holds.borrow();
        holds[handle.slot as usize]
            .filter(|hold| (hold.id, hold.revision) == (handle.id, handle.revision))
            .map(|hold| hold.payload_len)
    }

    /// The device, for a bench or a harness that needs the card underneath.
    #[cfg(any(test, feature = "std"))]
    pub fn device(&self) -> &D {
        &self.dev
    }

    /// Breaks the no-borrow-across-a-card-command rule on purpose: the positive control for
    /// [`granularity`](super::granularity), which would report zero refusals forever and read as a
    /// pass if it ever stopped re-entering the store.
    #[cfg(test)]
    pub(super) fn hold_free_across_a_command(&self) {
        let _free = self.free.borrow_mut();
        let mut block = [0u8; BLOCK];
        let _ = read_blocks(&self.dev, SUPERBLOCK[0], &mut block);
    }

    /// How many extent ranges the head revision of `id` holds. For fixtures that must prove an object
    /// is fragmented rather than assume it.
    #[cfg(test)]
    pub(super) fn head_range_count(&self, id: ObjectId) -> Option<usize> {
        self.find(id).ok()?.1.map(|entry| entry.ranges.len())
    }

    /// The one method that takes `&mut self`, and the reason `store`, `geometry` and `extents` need no
    /// cell: it runs inside [`mount`](Self::mount) on a store nothing else can reach yet.
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
        // The decode above is what guarantees the count fits the entry's `u16` index, so nothing
        // below has to clamp it.
        self.geometry = superblock.geometry;
        self.extents = superblock.extent_count();
        let mut served = self.served.get();
        match self.dev.block_count() {
            Ok(observed) if observed >= superblock.total_blocks => {}
            Ok(_) => {
                served.mode = Mode::CardTooSmall;
                self.served.set(served);
                return;
            }
            Err(_) => {
                served.mode = Mode::CatalogUnreadable;
                self.served.set(served);
                return;
            }
        }
        served.mode = Mode::CatalogUnreadable;

        // Two gate reads decide which copy to try and where the sequence continues from. Only
        // well-formed gates contribute, so garbage in a dead gate's sequence cannot poison the
        // high-water mark.
        let mut gates: [Option<Gate>; 2] = [None, None];
        for (copy, gate) in gates.iter_mut().enumerate() {
            if read_blocks(&self.dev, catalog_gate(copy), &mut block).is_ok() {
                *gate = Gate::decode(&block, copy, &self.store).ok();
            }
        }
        served.high_water = gates.iter().flatten().map(|gate| gate.sequence).max().unwrap_or(0);
        let order: [usize; 2] = match (gates[0], gates[1]) {
            (Some(a), Some(b)) if a.sequence == b.sequence => {
                self.served.set(served);
                return;
            }
            (Some(a), Some(b)) if b.sequence > a.sequence => [1, 0],
            _ => [0, 1],
        };

        for copy in order {
            let Some(gate) = gates[copy] else { continue };
            if let Ok(loaded) = self.load(copy, &gate) {
                served.copy = copy;
                served.sequence = gate.sequence;
                served.next_object = loaded.next_object;
                served.entry_count = gate.entry_count;
                served.body_crc = gate.body_crc;
                // A counter that has run out mounts read-only rather than wrapping.
                served.mode = if loaded.exhausted {
                    Mode::RevisionSpaceExhausted
                } else if served.high_water == u64::MAX {
                    Mode::SequenceSpaceExhausted
                } else {
                    Mode::ReadWrite
                };
                self.served.set(served);
                if let Some(recording) = loaded.recording {
                    self.recover_ride(&recording);
                }
                return;
            }
        }
        self.served.set(served);
        // No copy is being served, so the free map describes nothing: a failed [`load`] leaves its
        // own attempt's bitmap behind, and `free_extents()` is public.
        self.free.borrow_mut().reset(0);
    }

    /// Validates one catalog copy — the body CRC and every structural rule — and builds the free
    /// bitmap from the ranges as they go past. A failure leaves the caller free to try the next
    /// candidate; the bitmap is rebuilt from scratch each attempt.
    ///
    /// The free map is borrowed for the whole scan. That is safe only because this runs inside
    /// [`mount`](Self::mount), before the store exists for anyone else to reach.
    fn load(&self, copy: usize, gate: &Gate) -> Result<Loaded, StoreError> {
        let mut free = self.free.borrow_mut();
        free.reset(self.extents);
        // One window serves the header block and then the array. It is [`MOUNT_STREAM_WINDOW`] rather
        // than [`STREAM_WINDOW`] because this frame is also building the store.
        let mut window = [0u8; MOUNT_STREAM_WINDOW];
        read_blocks(&self.dev, CATALOG[copy], &mut window[..BLOCK])?;
        let header = Header::decode(&window[..BLOCK], &self.store).map_err(|_| StoreError::Invalid)?;
        if header.entry_count != gate.entry_count || header.sequence != gate.sequence {
            return Err(StoreError::Invalid);
        }
        let mut digest = Crc32::new();
        digest.update(&window[..BLOCK]);

        let mut structure = Structure::new(self.geometry);
        let mut loaded = Loaded { next_object: header.next_object, recording: None, exhausted: false };
        let mut done = 0usize;
        while done < header.entry_count as usize {
            // Only the blocks the live prefix occupies, so a short catalog is not read wider than it
            // is. A cost matter, not a safety one: it is the writer that must never reach the gate.
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
                free.claim(&entry.ranges).map_err(|_| StoreError::Invalid)?;
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

    /// Read the 16 slots and take the candidate with the greatest checkpoint sequence. The slot CRC
    /// is checked from the greatest sequence down, so the 16 KiB of tail bytes are only ever read for
    /// a slot that is about to be selected.
    ///
    /// A recording entry with no valid slot is the state a ride start leaves before its first
    /// checkpoint: the ride resumes at sequence 1 with nothing flushed.
    fn recover_ride(&self, entry: &Entry) {
        let mut candidates: [Option<Slot>; SLOTS] = [None; SLOTS];
        let mut block = [0u8; BLOCK];
        for (slot, candidate) in candidates.iter_mut().enumerate() {
            if read_blocks(&self.dev, slot_header_block(slot), &mut block).is_err() {
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
            tail_len: 0,
            payload_crc: 0,
            resume: [0; RIDE_RESUME_LEN],
            pending_proof: 0,
            pending_append_len: 0,
            pending_append_crc: 0,
        };
        while let Some(index) = candidates
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.filter(|slot| !slot.proof).map(|slot| (index, slot.sequence)))
            .max_by_key(|(_, sequence)| *sequence)
            .map(|(index, _)| index)
        {
            let slot = candidates[index].take().expect("the index came from a present slot");
            if !self.slot_intact(&slot) {
                continue;
            }
            // A logical slot at the end of the integer space cannot be continued and is not a state
            // this writer produces: `journal` preflights the following sequence before it touches
            // media.
            let Some(next_sequence) = slot.sequence.checked_add(1) else { continue };
            if slot.proof_sequence != 0 {
                let Some(proof) = candidates
                    .iter()
                    .flatten()
                    .copied()
                    .find(|proof| proof.proof && proof.sequence == slot.proof_sequence)
                else {
                    continue;
                };
                if proof.flushed.checked_add(PROGRAM_PAGE as u64) != Some(slot.flushed) || !self.slot_intact(&proof) {
                    continue;
                }
                // The logical gate is durable before this copy. A cut here leaves the same logical
                // gate and proof for the next boot to retry.
                if self.repair_rollover(entry.ranges, &proof).is_err() {
                    let mut served = self.served.get();
                    served.mode = Mode::CatalogUnreadable;
                    self.served.set(served);
                    self.ride.set(None);
                    self.recovered.set(None);
                    return;
                }
            }
            ride.flushed = slot.flushed;
            ride.next_sequence = next_sequence;
            ride.tail_len = slot.tail_len;
            ride.payload_crc = slot.payload_crc;
            ride.resume = slot.resume;
            self.recovered.set(Some(RideRecovery {
                id: slot.id,
                revision: slot.revision,
                checkpoint_sequence: slot.sequence,
                flushed: slot.flushed,
                tail_len: slot.tail_len,
                payload_crc: slot.payload_crc,
                resume: slot.resume,
                slot: slot.slot,
            }));
            break;
        }
        if self.recovered.get().is_none() {
            // Ride start is itself durable. Before its first checkpoint the logical recording is
            // exactly empty, and exposing that lets the board continue or discard it.
            self.recovered.set(Some(RideRecovery {
                id: entry.meta.id,
                revision: entry.meta.revision,
                checkpoint_sequence: 0,
                flushed: 0,
                tail_len: 0,
                payload_crc: 0,
                resume: [0; RIDE_RESUME_LEN],
                slot: u16::MAX,
            }));
        }
        self.ride.set(Some(ride));
    }

    /// Ensure a rollover's already-gated proof page is present byte-for-byte in the ride extent.
    /// An intact page is not rewritten. A torn page is repaired only from the immutable proof slot;
    /// cuts during repair are harmless because the same comparison and copy repeat on the next boot.
    fn repair_rollover(&self, ranges: Ranges, proof: &Slot) -> Result<(), StoreError> {
        let located = ranges
            .locate(self.geometry, proof.flushed)
            .filter(|located| located.offset == 0 && located.contiguous >= PROGRAM_PAGE as u64)
            .ok_or(StoreError::Invalid)?;
        let mut source = [0u8; BLOCK];
        let mut target = [0u8; BLOCK];
        let mut differs = false;
        for block in 0..SLOT_BLOCKS {
            read_blocks(&self.dev, slot_block(proof.slot as usize) + block, &mut source)?;
            read_blocks(&self.dev, located.block + block, &mut target)?;
            differs |= source != target;
        }
        if !differs {
            // Also closes the uncertainty window of a prior sync that returned an error: the page may
            // read back through volatile cache while still needing this retry's durability gate.
            return sync(&self.dev);
        }
        for block in 0..SLOT_BLOCKS {
            read_blocks(&self.dev, slot_block(proof.slot as usize) + block, &mut source)?;
            write_blocks(&self.dev, located.block + block, &source)?;
        }
        sync(&self.dev)
    }

    /// The other half of a slot's candidacy: the slot CRC over its header and full 16 KiB tail page.
    /// The 32 tail blocks are read in chunks, because this is a digest fold with nothing to decode
    /// block by block.
    fn slot_intact(&self, slot: &Slot) -> bool {
        let mut digest = journal::header_digest(&slot.header_bytes(&self.store));
        let base = slot_block(slot.slot as usize);
        let mut chunk = [0u8; ZERO_PAD.len()];
        let mut read = 0u64;
        while read < SLOT_BLOCKS {
            let blocks = (SLOT_BLOCKS - read).min(chunk.len() as u64 / BLOCK as u64) as usize;
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
        // One block, not a window: a binary search's probes are scattered.
        let served = self.served.get();
        let mut cursor = EntryCursor::new(served.copy, self.extents, served.entry_count);
        let mut probe = [0u8; BLOCK];
        let mut low = 0u16;
        let mut high = served.entry_count;
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
        for index in low..served.entry_count.min(low.saturating_add(2)) {
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

    /// Walks the new entry array in key order: the serving copy's entries with the batch applied.
    ///
    /// The cursor and its window are the caller's, so a commit's two passes share one of each. The two
    /// passes can never disagree about which copy they are reading, and the frame carries one window.
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
        // No borrow is held across this loop, and `emit` is called from inside it. The closures the
        // two passes pass in touch `Structure` and `BodyWriter`, never the store.
        for index in 0..self.served.get().entry_count {
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
                // `u64::MAX`, and the id cursor must end strictly greater than every id in the array.
                if meta.revision.0 == u64::MAX || meta.id.0 == u64::MAX {
                    return Err(StoreError::ReadOnly);
                }
                let (retained, head) = self.find(meta.id)?;
                let existing =
                    [retained, head].into_iter().flatten().find(|entry| entry.meta.revision == meta.revision);
                match source {
                    PutSource::Amend => {
                        let existing = existing.ok_or(StoreError::NotFound)?;
                        if meta.kind != existing.meta.kind
                            || (meta.flags.has(EntryFlags::ASSISTANT_ACCEPTED)
                                && (meta.payload_len != existing.meta.payload_len
                                    || meta.payload_crc != existing.meta.payload_crc))
                        {
                            return Err(StoreError::Invalid);
                        }
                        let mut entry = Entry { meta: *meta, ranges: existing.ranges };
                        entry.meta.added_at_utc = existing.meta.added_at_utc;
                        let freed = if meta.flags.holds_slack() {
                            Ranges::default()
                        } else {
                            entry.ranges.trim_to(self.geometry.extents_for(meta.payload_len) as u32)
                        };
                        if entry.ranges.is_empty() {
                            return Err(StoreError::Invalid);
                        }
                        Ok(Resolved { key: meta.key(), entry: Some(entry), creates: false, freed, reservation: None })
                    }
                    PutSource::Fresh(allocation) => {
                        if meta.flags.has(EntryFlags::ASSISTANT_ACCEPTED) {
                            return Err(StoreError::Invalid);
                        }
                        // The id cursor never rewinds, so an id below it named an object once and may
                        // never name another. The hold table keys on `(ObjectId, Revision)`, so a key
                        // re-created over different extents would serve a removed object's bytes to a
                        // reader that opened the live one. A create names `next_object_id()`.
                        if retained.is_none() && head.is_none() && meta.id.0 < self.served.get().next_object {
                            return Err(StoreError::Invalid);
                        }
                        // A revision that already exists is caught by the compare-and-swap below: the
                        // head is either this revision, in which case one past it is not it, or a
                        // greater one.
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
                        // The two facts this needs out of the row, copied out and the borrow dropped.
                        let rows = self.reservations.borrow();
                        let row = row_of(&rows, allocation).ok_or(StoreError::Invalid)?;
                        let (ranges, written) = (row.ranges, row.written);
                        drop(rows);
                        if !meta.flags.holds_slack() && meta.payload_len != written {
                            return Err(StoreError::Invalid);
                        }
                        let mut entry = Entry { meta: *meta, ranges };
                        if meta.kind == super::ObjectKind::Route
                            && !meta.flags.has(EntryFlags::RETAINED)
                            && meta.added_at_utc == 0
                        {
                            entry.meta.added_at_utc = self.route_added_at.get();
                        }
                        let freed = if meta.flags.holds_slack() {
                            Ranges::default()
                        } else {
                            entry.ranges.trim_to(self.geometry.extents_for(meta.payload_len) as u32)
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
    /// them — a RAM-only hold, because after a reboot the extents are free and no reader is left.
    fn release(&self, plan: &[Resolved]) {
        // Two cells at once and no card command between them. Admissible because neither `holds`' nor
        // `free`'s own methods call back into the store.
        let holds = self.holds.borrow();
        let mut free = self.free.borrow_mut();
        for resolved in plan {
            // Both branches ask, because both take extents away from a reader: a removal takes the
            // whole entry, and an amend that trims a reserve takes its tail.
            let held = holds.iter().flatten().any(|hold| (hold.id, hold.revision) == resolved.key);
            if !held {
                free.release(&resolved.freed);
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

    /// Reserves extents and takes a row. No card command runs here at all, so both borrows are the
    /// short kind.
    fn allocate(&self, bytes: u64) -> Result<Allocation, StoreError> {
        if !self.mode().writable() {
            return Err(StoreError::ReadOnly);
        }
        if bytes == 0 {
            return Err(StoreError::Invalid);
        }
        let extents = self.geometry.extents_for(bytes);
        let mut free = self.free.borrow_mut();
        if u64::from(free.free()) < extents {
            return Err(StoreError::NoSpace { required: bytes });
        }
        let ranges = free.first_fit(extents as u32).ok_or(StoreError::TooFragmented)?;
        // No free row is `busy` on the wire, not `invalidRequest`. Taken before the claim, so a
        // refusal here gives the map back nothing to undo.
        let mut rows = self.reservations.borrow_mut();
        let slot = rows.iter().position(Option::is_none).ok_or(StoreError::Invalid)?;
        free.claim(&ranges).map_err(|_| StoreError::Invalid)?;
        let nonce = self.nonce.get().wrapping_add(1);
        self.nonce.set(nonce);
        rows[slot] = Some(Reservation { nonce, ranges, reserved: bytes, written: 0, staging: [0; BLOCK] });
        Ok(Allocation { slot: slot as u8, nonce, reserved: bytes, written: 0 })
    }

    /// The reservation borrow is held across the card commands `fill` issues, because the row's
    /// staging block is what those commands write out of. Nothing on the read path wants this cell.
    fn write(&self, allocation: &mut Allocation, bytes: &[u8]) -> Result<(), StoreError> {
        if !self.mode().writable() {
            return Err(StoreError::ReadOnly);
        }
        let mut rows = self.reservations.borrow_mut();
        if row_of(&rows, allocation).is_none() {
            return Err(StoreError::Invalid);
        }
        if allocation.written + bytes.len() as u64 > allocation.reserved {
            return Err(StoreError::Invalid);
        }
        let dev = &self.dev;
        let geometry = self.geometry;
        let row = rows[allocation.slot as usize].as_mut().expect("the row was just validated");
        // A fragmented allocation is several writes, so one of them can fail with the others already
        // on the card. The row's cursor goes back where it was: it is the reservation's identity as
        // much as its position, so a cursor left ahead of the caller's would wedge the row and its
        // extents until the next mount, `cancel` included.
        let start = row.written;
        if let Err(error) = fill(dev, geometry, row, bytes) {
            row.written = start;
            return Err(error);
        }
        allocation.written = row.written;
        Ok(())
    }

    /// The only durable state transition an object ever undergoes. Payload bytes are written and
    /// synchronized before it begins, so a cut at any point before the gate leaves those bytes
    /// anonymous and their extents free at the next mount.
    ///
    /// The ~36 card commands below run with no cell borrowed. `served` is read out once as a `Copy`
    /// value, and the only borrows are the short no-I/O windows plus the staging flush.
    fn commit(&self, mutations: &[Mutation]) -> Result<u64, StoreError> {
        let served = self.served.get();
        if !served.mode.writable() {
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
        let mut count = served.entry_count as i32;
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

        // Validate the complete batch before any write. A structural refusal must preserve both
        // catalog copies: validating while writing would invalidate the inactive gate and spend that
        // redundancy. Publication errors later can instead require remount, because a failed final
        // gate write or sync can already be durable.
        //
        // There is nothing past `u64::MAX` to continue to, and the arithmetic is checked here because
        // a refusal is the only admissible answer and a panic is not one.
        let sequence = served.high_water.checked_add(1).ok_or(StoreError::ReadOnly)?;
        let header = Header {
            store: self.store,
            sequence,
            next_object: served.next_object.max(greatest_id + 1),
            entry_count: count as u16,
        };
        let mut structure = Structure::new(self.geometry);
        // The commit's two windows, owned here and lent out: see [`EntryCursor`].
        let mut scan = [0u8; STREAM_WINDOW];
        let mut cursor = EntryCursor::new(served.copy, self.extents, served.entry_count);
        let written =
            self.merge(&mut cursor, &mut scan, plan, |entry| structure.accept(entry).map_err(|_| StoreError::Invalid))?;
        structure.finish(&header).map_err(|_| StoreError::Invalid)?;
        if written != header.entry_count {
            return Err(StoreError::Invalid);
        }
        // A pending logical gate must be repaired before an amend can publish its bytes. Removal is
        // different: its catalog gate makes the reserve, proof and torn payload page unreachable.
        if self.ride.get().is_some_and(|ride| {
            ride.pending_proof != 0
                && plan.iter().any(|resolved| resolved.key == (ride.id, ride.revision) && resolved.entry.is_some())
        }) {
            return Err(StoreError::Invalid);
        }

        // The payload is durable before the commit begins: whatever a `write` left in a staging block
        // goes to the card now. The borrow spans these commands because releasing it between them
        // would lift a 512-byte staging block onto the frame this module measures.
        let mut staged = false;
        let geometry = self.geometry;
        {
            let mut rows = self.reservations.borrow_mut();
            for resolved in plan.iter() {
                let Some(slot) = resolved.reservation else { continue };
                let dev = &self.dev;
                let row = rows[slot as usize].as_mut().expect("resolve validated the reservation");
                let partial = (row.written % BLOCK as u64) as usize;
                if partial > 0 {
                    let located =
                        row.ranges.locate(geometry, row.written - partial as u64).ok_or(StoreError::Invalid)?;
                    row.staging[partial..].fill(0);
                    write_blocks(dev, located.block, &row.staging)?;
                }
                staged = true;
            }
        }
        // Ride end: the last checkpoint's tail is on the card in a journal slot, not in the ride's
        // extents, and this is the commit that gives those bytes a length and a CRC. It obeys the same
        // "payload synchronized before the commit begins" rule as the staging flush above.
        staged |= self.flush_ride_tail(plan)?;
        if staged {
            sync(&self.dev)?;
        }

        let target = 1 - served.copy;
        write_blocks(&self.dev, catalog_gate(target), &INVALIDATED)?;
        sync(&self.dev)?;

        // The body, header block first and the entries after it, through one window: the header is
        // block 0 of the body and the first bytes its CRC covers.
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
        if let Err(error) = write_blocks(&self.dev, catalog_gate(target), &gate.encode()).and_then(|()| sync(&self.dev))
        {
            // Either failure can follow a complete durable gate. Do not release or rewrite
            // allocations until a new mount selects the catalog that actually reached media.
            self.require_remount();
            return Err(error);
        }

        // The gate landed: `target` is the truth, and everything the batch displaced is free. One
        // `set` of the whole `Served` value, the resident mirror of the atomic transition the gate
        // write just made on the card. A store whose high-water mark has reached `u64::MAX` has no
        // sequence for the next commit, so this is the last one this card accepts.
        self.served.set(Served {
            mode: if header.sequence == u64::MAX { Mode::SequenceSpaceExhausted } else { served.mode },
            copy: target,
            sequence: header.sequence,
            high_water: header.sequence,
            next_object: header.next_object,
            entry_count: header.entry_count,
            body_crc,
        });
        self.release(plan);
        {
            let mut rows = self.reservations.borrow_mut();
            for resolved in plan.iter() {
                if let Some(slot) = resolved.reservation {
                    rows[slot as usize] = None;
                }
            }
        }
        self.settle_ride(plan);
        Ok(header.sequence)
    }

    fn open(&self, id: ObjectId, revision: Option<Revision>) -> Result<Handle, StoreError> {
        if !self.mode().readable() {
            return Err(StoreError::ReadOnly);
        }
        let (retained, head) = self.find(id)?;
        let entry = match revision {
            None => head,
            Some(revision) => [retained, head].into_iter().flatten().find(|entry| entry.meta.revision == revision),
        }
        .ok_or(StoreError::NotFound)?;
        // The store did not write a reserve's bytes, so there is nothing here to serve.
        if entry.meta.flags.has(EntryFlags::RESERVED) {
            return Err(StoreError::Invalid);
        }
        let mut holds = self.holds.borrow_mut();
        let key = (entry.meta.id, entry.meta.revision);
        if let Some(slot) = holds.iter().position(|hold| hold.is_some_and(|hold| (hold.id, hold.revision) == key)) {
            let hold = holds[slot].as_mut().expect("the row was just found");
            hold.readers += 1;
            // An amend keeps the key and changes the metadata, so a reader joining an existing row
            // takes the length just read — and only the length: the row keeps the extents the first
            // reader resolved, and an amend only ever trims, so the wider ranges serve every byte.
            // `max`, not assignment, because a later joiner must not shorten what an earlier one is
            // already serving.
            hold.payload_len = hold.payload_len.max(entry.meta.payload_len);
            return Ok(Handle { slot: slot as u8, id: entry.meta.id, revision: entry.meta.revision });
        }
        // A full table is transient: some other reader is holding every row, so the answer is to ask
        // again rather than to reject the request. A client that read this as `invalidRequest` would
        // stop retrying something that would have worked a second later.
        let slot = holds.iter().position(Option::is_none).ok_or(StoreError::Busy)?;
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
        self.read_ranges(&hold.ranges, hold.payload_len, offset, buf)
    }

    /// The listing snapshots the copy, the count and the commit sequence it was built against, and
    /// holds no cell borrow, which is what lets it coexist with a commit.
    ///
    /// It stops if the store moves off that sequence: two commits later the snapshotted copy has been
    /// rewritten under the cursor, and the walk would serve the new generation's entries mid-listing
    /// with [`entries_ok`](Self::entries_ok) still answering `true`.
    fn entries(&self) -> impl Iterator<Item = EntryMeta> + '_ {
        let served = self.served.get();
        self.listing_failed.set(!served.mode.readable());
        Entries {
            dev: &self.dev,
            cursor: EntryCursor::new(served.copy, self.extents, served.entry_count),
            buf: [0; BLOCK],
            index: 0,
            count: served.entry_count,
            sequence: served.sequence,
            served: &self.served,
            failed: &self.listing_failed,
        }
    }

    /// An ordinary checkpoint or a rare rollover. The caller lends only bytes added since its last
    /// successful checkpoint; storage reconstructs the next full logical tail snapshot by streaming
    /// the previous slot into the next one. A rollover gates its reconstructed full-page proof, then
    /// the advanced logical remainder, and only then copies the proof page to the payload extent, so
    /// no cut exposes the 16 KiB proof as a logical checkpoint.
    fn journal(&self, checkpoint: RideCheckpoint) -> Result<(), StoreError> {
        if !self.mode().writable() {
            return Err(StoreError::ReadOnly);
        }
        let Some(mut ride) = self.ride.get() else { return Err(StoreError::Invalid) };
        if (checkpoint.id, checkpoint.revision) != (ride.id, ride.revision) {
            return Err(StoreError::Invalid);
        }
        if ride.pending_proof != 0 {
            return self.finish_pending_rollover(ride, checkpoint);
        }
        // One bounded interval can cross at most one page. That bound is what makes one proof plus
        // one logical gate sufficient without a page-sized snapshot in the recorder.
        if checkpoint.append.len() > PROGRAM_PAGE {
            return Err(StoreError::Invalid);
        }
        // Continuing from the prior checksum verifies exactly the delta the caller says this
        // checkpoint adds. The durable tail is reread and CRC-verified as it is copied below.
        let mut expected = Crc32::from_checksum(ride.payload_crc);
        expected.update(checkpoint.append);
        if expected.finalize() != checkpoint.payload_crc {
            return Err(StoreError::Invalid);
        }
        let source = self.current_ride_slot(&ride)?;
        let combined = ride.tail_len as usize + checkpoint.append.len();

        if combined >= PROGRAM_PAGE {
            let proof_sequence = ride.next_sequence;
            let logical_sequence = proof_sequence.checked_add(1).ok_or(StoreError::ReadOnly)?;
            let following_sequence = logical_sequence.checked_add(1).ok_or(StoreError::ReadOnly)?;
            let mut page_crc = Crc32::from_checksum(ride.payload_crc);
            let into_page = PROGRAM_PAGE - ride.tail_len as usize;
            page_crc.update(&checkpoint.append[..into_page]);
            let proof = self.write_ride_slot(
                &ride,
                SlotWrite {
                    sequence: proof_sequence,
                    flushed: ride.flushed,
                    source,
                    append: &checkpoint.append[..into_page],
                    payload_crc: page_crc.finalize(),
                    proof: true,
                    proof_sequence: 0,
                    resume: &[0; RIDE_RESUME_LEN],
                },
            )?;
            let remainder = &checkpoint.append[into_page..];
            let advanced = ride.flushed + PROGRAM_PAGE as u64;
            self.write_ride_slot(
                &ride,
                SlotWrite {
                    sequence: logical_sequence,
                    flushed: advanced,
                    source: None,
                    append: remainder,
                    payload_crc: checkpoint.payload_crc,
                    proof: false,
                    proof_sequence,
                    resume: checkpoint.resume,
                },
            )?;
            ride.flushed = advanced;
            ride.next_sequence = following_sequence;
            ride.tail_len = remainder.len() as u32;
            ride.payload_crc = checkpoint.payload_crc;
            ride.resume = *checkpoint.resume;
            ride.pending_proof = proof_sequence;
            ride.pending_append_len = checkpoint.append.len() as u32;
            let mut append_digest = Crc32::new();
            append_digest.update(checkpoint.append);
            ride.pending_append_crc = append_digest.finalize();
            // Both gates are now authoritative. Publish the pending state before the fallible page
            // copy so a same-boot retry repairs from the proof without touching either gate.
            self.ride.set(Some(ride));
            self.repair_rollover(ride.ranges, &proof)?;
            ride.pending_proof = 0;
            ride.pending_append_len = 0;
            ride.pending_append_crc = 0;
        } else {
            let following_sequence = ride.next_sequence.checked_add(1).ok_or(StoreError::ReadOnly)?;
            self.write_ride_slot(
                &ride,
                SlotWrite {
                    sequence: ride.next_sequence,
                    flushed: ride.flushed,
                    source,
                    append: checkpoint.append,
                    payload_crc: checkpoint.payload_crc,
                    proof: false,
                    proof_sequence: 0,
                    resume: checkpoint.resume,
                },
            )?;
            ride.next_sequence = following_sequence;
            ride.tail_len = combined as u32;
        }
        ride.payload_crc = checkpoint.payload_crc;
        ride.resume = *checkpoint.resume;
        ride.pending_proof = 0;
        ride.pending_append_len = 0;
        ride.pending_append_crc = 0;

        self.ride.set(Some(ride));
        Ok(())
    }
}

impl<D: BlockDevice> FlatStore<D> {
    fn finish_pending_rollover(&self, mut ride: RideState, checkpoint: RideCheckpoint) -> Result<(), StoreError> {
        // Both gates already include the caller's delta, so applying `append` again would double it.
        // The retry proves it is asking for exactly that logical checkpoint through the full payload
        // CRC and resume anchor.
        let mut append_digest = Crc32::new();
        append_digest.update(checkpoint.append);
        if checkpoint.payload_crc != ride.payload_crc
            || checkpoint.resume != &ride.resume
            || checkpoint.append.len() != ride.pending_append_len as usize
            || append_digest.finalize() != ride.pending_append_crc
        {
            return Err(StoreError::Invalid);
        }
        let proof_index = (ride.pending_proof % SLOTS as u64) as usize;
        let mut header = [0u8; BLOCK];
        read_blocks(&self.dev, slot_header_block(proof_index), &mut header)?;
        let proof = Slot::decode(&header, proof_index, &self.store, self.extents).map_err(|_| StoreError::Invalid)?;
        if !proof.proof
            || proof.sequence != ride.pending_proof
            || proof.flushed + PROGRAM_PAGE as u64 != ride.flushed
            || proof.id != ride.id
            || proof.revision != ride.revision
            || proof.ranges != ride.ranges
            || !self.slot_intact(&proof)
        {
            return Err(StoreError::Invalid);
        }
        self.repair_rollover(ride.ranges, &proof)?;
        ride.pending_proof = 0;
        ride.pending_append_len = 0;
        ride.pending_append_crc = 0;
        self.ride.set(Some(ride));
        Ok(())
    }

    /// The newest logical slot `ride` names. Its tail CRC is checked while
    /// [`write_ride_slot`](Self::write_ride_slot) streams it, avoiding a second 16 KiB read pass.
    fn current_ride_slot(&self, ride: &RideState) -> Result<Option<Slot>, StoreError> {
        if ride.next_sequence == 1 {
            return if ride.flushed == 0 && ride.tail_len == 0 && ride.payload_crc == 0 {
                Ok(None)
            } else {
                Err(StoreError::Invalid)
            };
        }
        let index = ((ride.next_sequence - 1) % SLOTS as u64) as usize;
        let mut header = [0u8; BLOCK];
        read_blocks(&self.dev, slot_header_block(index), &mut header)?;
        let slot = Slot::decode(&header, index, &self.store, self.extents).map_err(|_| StoreError::Invalid)?;
        if slot.proof
            || slot.sequence.checked_add(1) != Some(ride.next_sequence)
            || (slot.id, slot.revision, slot.ranges) != (ride.id, ride.revision, ride.ranges)
            || slot.flushed != ride.flushed
            || slot.tail_len != ride.tail_len
            || slot.payload_crc != ride.payload_crc
            || slot.resume != ride.resume
        {
            return Err(StoreError::Invalid);
        }
        Ok(Some(slot))
    }

    /// Reconstruct, write, and gate one full tail-slot snapshot without a page-sized RAM buffer.
    /// The previous logical slot is copied in bounded 4 KiB chunks, `append` follows its logical tail,
    /// and the rest is zero. Both the source and target CRCs are folded during that one pass. A bad
    /// source or any cut leaves the target header absent/invalid, so recovery stays on the source.
    fn write_ride_slot(&self, ride: &RideState, write: SlotWrite<'_>) -> Result<Slot, StoreError> {
        let source_len = write.source.map_or(0, |slot| slot.tail_len as usize);
        let tail_len = source_len.checked_add(write.append.len()).ok_or(StoreError::Invalid)?;
        if tail_len > TAIL_CAPACITY {
            return Err(StoreError::Invalid);
        }
        let mut slot = Slot {
            slot: (write.sequence % SLOTS as u64) as u16,
            id: ride.id,
            revision: ride.revision,
            sequence: write.sequence,
            flushed: write.flushed,
            tail_len: tail_len as u32,
            payload_crc: write.payload_crc,
            resume: *write.resume,
            proof: write.proof,
            proof_sequence: write.proof_sequence,
            ranges: ride.ranges,
            slot_crc: 0,
        };
        let base = slot_block(slot.slot as usize);
        let mut target_digest = journal::header_digest(&slot.header_bytes(&self.store));
        let mut source_digest = write.source.map(|source| journal::header_digest(&source.header_bytes(&self.store)));
        let mut block = [0u8; ZERO_PAD.len()];
        let blocks_per_chunk = block.len() / BLOCK;
        for index in 0..SLOT_BLOCKS as usize / blocks_per_chunk {
            let block_start = index * block.len();
            if let Some(source) = write.source {
                read_blocks(
                    &self.dev,
                    slot_block(source.slot as usize) + (index * blocks_per_chunk) as u64,
                    &mut block,
                )?;
                source_digest.as_mut().expect("a source created its digest").update(&block);
            } else {
                block.fill(0);
            }
            if source_len < block_start + block.len() {
                block[source_len.saturating_sub(block_start)..].fill(0);
            }
            let append_start = source_len.max(block_start);
            let append_end = tail_len.min(block_start + block.len());
            if append_start < append_end {
                let from = append_start - source_len;
                let to = append_end - source_len;
                block[append_start - block_start..append_end - block_start].copy_from_slice(&write.append[from..to]);
            }
            target_digest.update(&block);
            write_blocks(&self.dev, base + (index * blocks_per_chunk) as u64, &block)?;
        }
        if let Some(source) = write.source {
            if source_digest.expect("a source created its digest").finalize() != source.slot_crc {
                return Err(StoreError::Invalid);
            }
        }
        sync(&self.dev)?;
        slot.slot_crc = target_digest.finalize();
        write_blocks(&self.dev, slot_header_block(slot.slot as usize), &slot.header_bytes(&self.store))?;
        sync(&self.dev)?;
        Ok(slot)
    }
    /// The entry a batch is finalising the live ride with: the `Put` that names the recording entry's
    /// key and clears `RECORDING`. A `Remove` of that key is not one — the object is going away, and
    /// so are its bytes.
    fn finalising(&self, plan: &[Resolved]) -> Option<(RideState, Entry)> {
        let ride = self.ride.get()?;
        let entry = plan
            .iter()
            .filter_map(|resolved| resolved.entry)
            .find(|entry| entry.meta.key() == (ride.id, ride.revision))?;
        (!entry.meta.flags.has(EntryFlags::RECORDING)).then_some((ride, entry))
    }

    /// Moves the bytes past `flushed` out of the newest journal slot and into the ride's own extents,
    /// so the length and CRC the finalising commit publishes describe bytes that are on the card.
    /// Reports whether anything was written. Everything up to `flushed` is already there, because a
    /// checkpoint only ever writes whole 16 KiB pages; the remainder is the partial page.
    fn flush_ride_tail(&self, plan: &[Resolved]) -> Result<bool, StoreError> {
        let Some((ride, entry)) = self.finalising(plan) else { return Ok(false) };
        let length = entry.meta.payload_len;
        if length == ride.flushed {
            if entry.meta.payload_crc != ride.payload_crc || ride.tail_len != 0 {
                return Err(StoreError::Invalid);
            }
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
        read_blocks(&self.dev, slot_header_block(slot_index), &mut block)?;
        let slot = Slot::decode(&block, slot_index, &self.store, self.extents).map_err(|_| StoreError::Invalid)?;
        // The slot has to be this ride's, at this flush point, holding exactly the tail the caller is
        // publishing a length for. The comparison is against the reserve the ride is recording into,
        // not the entry being written, whose ranges are already trimmed to the finalised payload.
        if (slot.id, slot.revision, slot.ranges) != (ride.id, ride.revision, ride.ranges)
            || slot.proof
            || slot.sequence.checked_add(1) != Some(ride.next_sequence)
            || slot.flushed != ride.flushed
            || u64::from(slot.tail_len) != tail_len
            || slot.payload_crc != ride.payload_crc
            || slot.payload_crc != entry.meta.payload_crc
            || !self.slot_intact(&slot)
        {
            return Err(StoreError::Invalid);
        }

        let base = slot_block(slot_index);
        let mut done = 0u64;
        let mut target = [0u8; BLOCK];
        let mut wrote = false;
        while done < tail_len {
            read_blocks(&self.dev, base + done / BLOCK as u64, &mut block)?;
            let located = ride.ranges.locate(self.geometry, ride.flushed + done).ok_or(StoreError::Invalid)?;
            // A whole block goes out even for a partial tail: the bytes past `payload_len` are slack
            // inside the ride's last extent, which nothing reads and no CRC covers.
            read_blocks(&self.dev, located.block, &mut target)?;
            if target != block {
                write_blocks(&self.dev, located.block, &block)?;
                wrote = true;
            }
            done += BLOCK as u64;
        }
        Ok(wrote)
    }

    /// The resident ride state after a commit that started, amended or ended the ride.
    ///
    /// Ride end zeroes the 16 slot headers, and this runs after the gate, so a media failure here
    /// cannot be reported: the commit already happened. A cut during that zeroing is harmless, because
    /// no entry carries `RECORDING` and mount never reads the slots.
    fn settle_ride(&self, plan: &[Resolved]) {
        let started =
            plan.iter().filter_map(|resolved| resolved.entry).find(|entry| entry.meta.flags.has(EntryFlags::RECORDING));
        if let Some(entry) = started {
            let same = self.ride.get().filter(|ride| (ride.id, ride.revision) == (entry.meta.id, entry.meta.revision));
            self.ride.set(Some(RideState {
                id: entry.meta.id,
                revision: entry.meta.revision,
                ranges: entry.ranges,
                flushed: same.map_or(0, |ride| ride.flushed),
                next_sequence: same.map_or(1, |ride| ride.next_sequence),
                tail_len: same.map_or(0, |ride| ride.tail_len),
                payload_crc: same.map_or(0, |ride| ride.payload_crc),
                resume: same.map_or([0; RIDE_RESUME_LEN], |ride| ride.resume),
                pending_proof: same.map_or(0, |ride| ride.pending_proof),
                pending_append_len: same.map_or(0, |ride| ride.pending_append_len),
                pending_append_crc: same.map_or(0, |ride| ride.pending_append_crc),
            }));
            // `recovered` is a mount-time offer, not a mirror of the active ride. Manufacturing a
            // zero-length recovery here would make a later recorder construction offer that new ride
            // as if it had survived a reset.
            return;
        }
        let Some(ride) = self.ride.get() else { return };
        if !plan.iter().any(|resolved| resolved.key == (ride.id, ride.revision)) {
            return;
        }
        self.ride.set(None);
        self.recovered.set(None);
        for slot in 0..SLOTS {
            if write_blocks(&self.dev, slot_header_block(slot), &INVALIDATED).is_err() {
                return;
            }
        }
        let _ = sync(&self.dev);
    }
}

/// The read-only catalog view: every entry, in the catalog's own `(ObjectId, Revision)` order.
struct Entries<'a, D> {
    dev: &'a D,
    cursor: EntryCursor,
    /// One block, not a window: widening it would grow the frame of every caller holding this
    /// iterator. Owned rather than lent, because this iterator outlives the call that built it.
    buf: [u8; BLOCK],
    index: u16,
    count: u16,
    /// The commit sequence this listing was built against. See [`Store::entries`] for why a listing
    /// that outlives it has to stop.
    sequence: u64,
    served: &'a Cell<Served>,
    failed: &'a Cell<bool>,
}

impl<D: BlockDevice> Iterator for Entries<'_, D> {
    type Item = EntryMeta;

    fn next(&mut self) -> Option<EntryMeta> {
        // A commit has landed since this listing was made, so the copy under the cursor is no longer
        // the one the store is serving and will be rewritten by the next commit. Reported through the
        // same channel a media failure is: this list is not the catalog.
        let served = self.served.get();
        if !served.mode.readable() || served.sequence != self.sequence {
            self.failed.set(true);
            self.index = self.count;
            return None;
        }
        if self.index >= self.count {
            return None;
        }
        // A read failure ends the listing, because the signature has nowhere to put an error — but not
        // silently: `entries_ok` is how the caller learns the list is short.
        let Ok(entry) = self.cursor.get(self.dev, &mut self.buf, self.index) else {
            self.failed.set(true);
            self.index = self.count;
            return None;
        };
        self.index += 1;
        Some(entry.meta)
    }
}
