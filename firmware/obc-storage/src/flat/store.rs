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
//!
//! ## Aliasing: the whole seam is `&self`
//!
//! Every operation takes `&self`, mutators included, and the resident state that moves lives behind
//! cells (#1256, the owner ruling of 2026-08-18; [`source`](super::source) has the argument for why).
//! Three rules hold it together, and they are what the rest of this module is arranged to obey.
//!
//! **1. Four kinds of field, and the split is deliberate.**
//!
//! | Field | Shape | Why |
//! |---|---|---|
//! | `dev` | plain, no cell | [`BlockDevice`] is `&self` throughout, so **the card is reachable with no borrow at all** — which is what makes rule 2 possible |
//! | `store`, `geometry`, `extents` | plain, no cell | settled by `bring_up` inside the constructor and never written again |
//! | [`Served`], `nonce`, `ride`, `recovered`, `listing_failed` | [`Cell`] | small and `Copy`; a `Cell` has no borrow flag, so these can never block anything and can never panic |
//! | `free`, `holds`, `reservations` | [`RefCell`] | 8 KiB and two tables of rows — too big to copy through a `Cell` |
//!
//! **2. Card commands run with the state unborrowed.** Every `RefCell` borrow in this module is
//! taken, used and dropped inside a window that issues no card command — with **two** exceptions, both
//! named below. A commit's ~36 write commands, a journal's page flushes and every `read` therefore run
//! holding **nothing**: the addresses they need are `Copy` values taken out of a cell first
//! ([`Hold`], [`RideState`], [`Served`]), and the device is reached through a plain `&`. That is
//! `FLAT_Store_Protocol.md`'s "per card command, never per commit" granularity, and it is the property
//! a storage task needs in order to interleave a render read into a commit's gaps.
//!
//! [`granularity`](super::granularity) is where this stops being a claim: it re-enters the store from
//! inside the block driver on every card command and pins what is served.
//!
//! **3. The first exception is `reservations`, and it is safe because no reader needs it.** A
//! [`write`](Store::write) streams the caller's bytes through a row's staging block, and a commit
//! flushes those blocks; both hold the reservations borrow across the card commands they issue, because
//! releasing it between them would mean copying a 512-byte staging block onto a frame this module
//! measures. Nothing on the read path — `open`, `read`, `entries`, `handle_len` — touches
//! `reservations`, so the only caller that borrow can reach is another writer. It does not *block* one
//! — a `RefCell` has no queue; it would **panic**, which on the device is a hard fault. The safety
//! therefore rests on writers being serialized by construction (`FLAT_Store_Protocol.md` §1 serves one
//! transfer at a time, and slice 3's storage task owns the write path), not on the cell arbitrating.
//! Measured cost: one command per reservation, not a phase.
//!
//! **4. The second exception is [`load`](FlatStore::load), and it is the constructor.** It holds the
//! free map across its whole catalog scan. Nothing else can reach a store that `mount` has not
//! returned yet, so there is no borrow to contend with — but it is an exception to rule 2 as stated,
//! and counting it as one is cheaper than explaining every time why it is not.
//!
//! **Re-entrancy is structurally impossible, which is why the borrows are `borrow_mut` and not
//! `try_borrow_mut`.** A borrow panic on the device is a hard fault, so the guarantee has to be
//! structural rather than handled: no cell borrow here spans a call to another `&self` method of the
//! store, and the one callback the module takes — [`merge`](FlatStore::merge)'s `emit` — is invoked with
//! no borrow held and is passed only closures that touch the catalog `Structure` and the
//! [`BodyWriter`], never the store. [`source`](super::source) carries the consumer-side half.

use core::cell::{Cell, RefCell};

use obc_crc::Crc32;

use super::bitmap::FreeMap;
use super::catalog::{Entry, Gate, Header, Structure, INVALIDATED};
use super::device::BlockDevice;
use super::error::StoreError;
use super::journal::{self, Slot, TAIL_CAPACITY, ZERO_PAD};
use super::layout::{
    catalog_gate, slot_block, slot_header_block, Geometry, Ranges, BLOCK, CATALOG, ENTRIES_PER_BLOCK, ENTRY_CAPACITY,
    ENTRY_STRIDE, MOUNT_STREAM_BLOCKS, MOUNT_STREAM_WINDOW, PROGRAM_PAGE, SLOTS, SLOT_BLOCKS, STREAM_WINDOW,
    SUPERBLOCK,
};
use super::seam::{
    Allocation, EntryFlags, EntryMeta, Mutation, ObjectId, PutSource, Revision, RideCheckpoint, Store, StoreId,
    RIDE_RESUME_LEN,
};
use super::superblock::Superblock;

/// Entry mutations one commit carries. Two is what the largest real batch needs — publish the new
/// head and retain or remove the displaced one — and four leaves margin without making the plan
/// arrays interesting.
pub const MAX_BATCH: usize = 4;
/// Reservations live at once: one transfer (`FLAT_Store_Protocol.md` §1) plus the ride reserve a
/// start allocates while one is in flight.
pub const MAX_RESERVATIONS: usize = 2;

/// Who holds an open object, and how many. The table is the whole argument for
/// [`MAX_OPEN_OBJECTS`]; the constant is just its sum.
///
/// **The rule: every session-long open needs a named row here.** A consumer that opens an object and
/// keeps it open across a render or a ride adds its row and raises [`MAX_OPEN_OBJECTS`] to match, or
/// the `const` assertion below fails the build. That is the point — the previous constant was `12`,
/// derived as "eleven shards plus one transfer", and it was short by three because terrain, the
/// active route and the weather bundle each hold one too and none of them was written down. A table
/// that has to be edited is harder to be wrong about than a sentence in a doc comment.
///
/// Rows are the *worst concurrent* case, not the typical one: a rider following a route, with
/// weather mounted, while an upload runs.
///
/// **The recording ride is deliberately not a row.** It is not an open object at all: it lives in
/// [`RideState`], reached through [`journal`](Store::journal) and its own reservation (see
/// [`MAX_RESERVATIONS`]), and it never takes a hold. A `RIDE` row here would be double-counting.
pub mod open_objects {
    /// **The map: one object, because a map is one file** (FS7.5, #1420).
    ///
    /// This row was `SET_SHARDS = 11` — the board's ceiling on a mounted volume set, inherited from
    /// `obc-fw-nrf54l`'s `SD_MAX_FILES - SD_RIDE_PEAK_FILES` because a set held every shard's handle
    /// open for the session. There are no shards, so there is no ceiling to inherit and nothing here
    /// derives from a board constant any more.
    pub const MAP: usize = 1;
    /// The active route's geometry, held from load until the ride ends.
    pub const ROUTE: usize = 1;
    /// The weather bundle, held for the session once mounted.
    pub const WEATHER: usize = 1;
    /// The one transfer `FLAT_Store_Protocol.md` §1 admits at a time, which may run mid-ride.
    pub const TRANSFER: usize = 1;
    /// One row that belongs to nobody, so a short-lived open — a menu reading a trip's header, a
    /// `STATUS` resolving an object — never has to wait for a session-long holder to let go.
    pub const SPARE: usize = 1;
    /// **The row a safe swap needs**: one extra hold so a session-long holder can acquire its
    /// replacement *before* releasing what it has.
    ///
    /// The rows above are a census of what is held at the worst moment, and they are right — but a
    /// census misses the moment a holder is being *changed*. Adopting a computed detour swaps the
    /// active route; adopting a freshly published bundle swaps the weather. Release-first makes
    /// those two operations fallible in the worst possible way: if the new open fails — a media
    /// glitch, a revision that moved — the rider is mid-ride with **no** route, and the object that
    /// was working a microsecond ago has already been let go. Acquire-before-release cannot lose
    /// what it has, and it needs one row that the census does not.
    ///
    /// **One row, not two.** A route swap and a weather swap overlapping is not budgeted: they are
    /// both rider- or link-driven and neither is on a timer, so the second one to start finds the
    /// table full and is refused [`Busy`](super::error::StoreError::Busy) — which is *ask again*,
    /// and the honest answer for a device that is momentarily out of rows. Budgeting for two would
    /// be provisioning for a coincidence nobody has measured.
    pub const SWAP: usize = 1;

    /// The sum every row above owes.
    pub const ACCOUNTED: usize = MAP + ROUTE + WEATHER + TRANSFER + SPARE + SWAP;
}

/// **Terrain is not a row, and its absence is the substantive half of this re-derivation.**
///
/// It used to be `TERRAIN = 1`: a `.obcd` sidecar mounted beside the map and held for the session,
/// a separate object because `SetManifest::shards()` excluded it. OBCM v14 §1.3 puts the OBCT
/// container **inside the map file**, so the board forms a byte window over the map's own source
/// and parses through it. Same handle, same hold, same refcount — a second row would be counting
/// one open twice, which is exactly the mistake the previous constant made in the other direction.
///
/// So `11 + 1 + 1 + 1 + 1 + 1 = 16` becomes `1 + 1 + 1 + 1 + 1 + 1 = 6`: five rows of census plus
/// [`SWAP`](open_objects::SWAP), the row acquire-before-release needs and a census cannot see.
///
/// **The 5-vs-6 choice, recorded because it reverses a saving.** The census alone is 5, and 5 is
/// provably the worst *legal concurrent* case — with zero margin. The sixth row buys back the safe
/// swap pattern, which under the old 16-row table was free and unstated; taking the table to its
/// exact census would have quietly made release-first the only affordable ordering, i.e. removed a
/// cross-slice invariant nobody had written down. 64 B is the right price for that, and the row's
/// own doc is where the invariant is now written down.
///
/// The ten rows that still go take **728 B** off a linked `FlatStore` on a part where every `.bss`
/// byte is a main-stack byte. A `Hold` is 64 B, so 640 of that is the rows and the rest is padding a
/// 16-row array carried — measured, because the arithmetic alone would have under-claimed it.
///
/// Open objects at once — the sum of [`open_objects`]'s rows, and nothing else.
pub const MAX_OPEN_OBJECTS: usize = 6;

// Deliberately an anonymous module-level `const`, not an associated one: an associated `const` is
// evaluated lazily, only when something names it, so a table that stopped adding up would compile
// silently until a test happened to touch it. This one is evaluated whenever the crate is.
const _: () = assert!(
    open_objects::ACCOUNTED == MAX_OPEN_OBJECTS,
    "MAX_OPEN_OBJECTS must equal the sum of `open_objects`'s rows: add a named row for the new \
     session-long open and raise the constant to match",
);
// `Handle::slot` is a `u8` and the holds array is indexed by it.
const _: () = assert!(MAX_OPEN_OBJECTS <= u8::MAX as usize);
// Journal snapshots are reconstructed one [`ZERO_PAD`] window at a time. A partial final window
// would require a second buffer shape and would make the fixed write census depend on arithmetic
// that the format already fixes exactly.
const _: () = assert!((SLOT_BLOCKS as usize * BLOCK).is_multiple_of(ZERO_PAD.len()));

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
    /// No catalog gate is well-formed, no candidate body validated, or required ride rollover
    /// repair failed. Evidence is preserved for the next mount; no incomplete recovery is exposed.
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
    /// Recorder-owned state CRC-covered by the selected logical checkpoint.
    pub resume: [u8; RIDE_RESUME_LEN],
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
    /// Tail bytes and payload CRC of the newest durable header. A caller may only append to that
    /// tail; these anchors let `journal` derive and verify rollover CRCs without rereading the ride.
    tail_len: u32,
    payload_crc: u32,
    resume: [u8; RIDE_RESUME_LEN],
    /// Nonzero after both rollover gates are durable but before their proof page is confirmed in
    /// the payload extent. A retry repairs from this proof without rewriting either gate.
    pending_proof: u64,
    /// Caller append identity already incorporated by the pending logical gate. Length plus the
    /// delta's own format CRC make the retry contract explicit instead of inferring it only from the
    /// cumulative payload CRC and resume anchor.
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

/// The catalog the store is serving, and the counters that move with it.
///
/// One `Copy` value in one [`Cell`] rather than six fields, because that is what they are: §5.5's
/// gate write is the instant all six become true together, and a commit publishes them in one
/// [`set`](Cell::set) on the far side of it. Six separate cells would have made the same transition
/// six independently observable steps.
#[derive(Debug, Clone, Copy)]
struct Served {
    mode: Mode,
    /// The copy the store is currently serving. §5.5's commit targets the other one.
    copy: usize,
    sequence: u64,
    /// The greatest commit sequence any **well-formed** gate carried, which is what a commit
    /// continues from — not the sequence of the copy that happened to validate.
    high_water: u64,
    next_object: u64,
    entry_count: u16,
}

/// The flat card store.
///
/// The field shapes are the module docs' table, and the rules that go with them are load bearing:
/// read them before moving a borrow.
pub struct FlatStore<D> {
    dev: D,
    store: StoreId,
    /// The card's own extent size, read from its superblock (§4) and the source of every address
    /// below. A store that is serving nothing keeps the default, which addresses nothing either.
    geometry: Geometry,
    extents: u32,
    served: Cell<Served>,
    free: RefCell<FreeMap>,
    holds: RefCell<[Option<Hold>; MAX_OPEN_OBJECTS]>,
    reservations: RefCell<[Option<Reservation>; MAX_RESERVATIONS]>,
    nonce: Cell<u32>,
    ride: Cell<Option<RideState>>,
    recovered: Cell<Option<RideRecovery>>,
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

/// The reservation an [`Allocation`] names, or `None` when the token names no live row: a stale slot,
/// a row that has been cancelled and re-taken, or a cursor that has moved out from under the caller.
///
/// A free function over the borrowed table rather than a method, because the table now lives in a
/// [`RefCell`] and a method could not lend a reference out of a borrow it had already dropped. Every
/// caller therefore takes the borrow itself, which is also what makes each one's extent visible.
fn row_of<'a>(rows: &'a [Option<Reservation>; MAX_RESERVATIONS], allocation: &Allocation) -> Option<&'a Reservation> {
    rows.get(allocation.slot as usize)?.as_ref().filter(|row| {
        (row.nonce, row.written, row.reserved) == (allocation.nonce, allocation.written, allocation.reserved)
    })
}

/// Appends `input` to a reservation, one contiguous run per pass: whole blocks straight out of the
/// caller's slice, and a partial one through the row's staging block. The cursor it advances belongs to
/// the caller's [`Allocation`] too, which is why [`Store::write`] rewinds it when this fails.
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
            // Bounded in `u64` and narrowed after — see [`Located::whole_blocks`], which is the one
            // place that order is decided. Narrowing first is a hang on the device and nothing at all
            // on a host.
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
    /// Patch bytes already appended to a live, unpublished allocation.
    ///
    /// Protocol uploads never need this: their CRC and header are known before the first payload
    /// byte. The on-device OBCR emitter is different — its streamed header is intentionally
    /// backfilled after geometry and index emission — so the board needs one bounded random write
    /// before publication. No committed object is addressable here, and a stale allocation token
    /// is refused by the same identity check as [`Store::write`].
    pub fn patch_allocation(&self, allocation: &Allocation, offset: u64, bytes: &[u8]) -> Result<(), StoreError> {
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

    /// CRC-32/IEEE of the bytes appended to a live allocation, including its unflushed tail.
    ///
    /// This is the on-device producer's final verification pass after its header patch. It reads in
    /// the same 4 KiB windows as catalog streaming, bounded on the stack and wide enough not to turn
    /// a normal route into hundreds of single-block commands.
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

    /// §5.6: superblock, gates, one body, the free bitmap, and the ride journal only when an entry
    /// says a ride was recording. There is no journal replay, no garbage collection and no recovery
    /// scan, so those five steps are the whole of mount.
    ///
    /// A card this cannot bring up mounts read-only rather than failing to exist: the seam has to be
    /// able to answer `readOnly`, and initialization is the only transition into this format.
    ///
    /// **The three `const` blocks below are load bearing, and the plain spelling costs 6,208 B of this
    /// frame.** `RefCell::new(x)` takes `x` *by value*, so `RefCell::new(FreeMap::BLANK)` materialises
    /// 8 KiB in a stack temporary and then copies it into the store being built — measured, on this
    /// symbol, as 14,016 B becoming 20,224 against a gate of 16,384. Wrapping each in `const { … }`
    /// makes it a constant expression instead: the blank table lands in `.rodata` and is copied once,
    /// straight into its field. `Default::default()` does *not* fix it — the temporary is inside the
    /// impl. This is the same failure `resource_guard.py frames` was written for, one layer down from
    /// #1359's return slots, and the reason `FreeMap::BLANK` exists at all.
    ///
    /// **`#[inline(never)]` is load bearing too, and it was added when the board first mounted one
    /// (FS7.5-c1).** Two things depend on this symbol existing:
    ///
    /// 1. **The frame gate can only see what it can name.** CI holds `obc_storage::flat` to 16,384 B
    ///    of frame. Inlined into its caller, this function's frame is charged to *that* caller's
    ///    symbol and the gate watching the store measures whatever is left — in the board image, the
    ///    largest `obc_storage::flat` frame read 6,336 B while the board helper that had absorbed
    ///    `mount` read 22,656. The gate was green and blind at the same time.
    /// 2. **A caller can place the store where it wants it.** A ~10.5 KB return value uses the
    ///    indirect-return ABI, so the caller hands in the destination and this function builds there.
    ///    That is what lets the board write straight into its `.bss` slot with no copy on the boot
    ///    frame; inlined, LLVM built the store as a local and memcpy'd it, which is the #1084 shape.
    ///
    /// The cost is one call and no duplicated body — this is a boot-path constructor, not a hot path.
    #[inline(never)]
    pub fn mount(dev: D) -> Self {
        let mut store = Self::blank(dev);
        store.bring_up();
        store
    }

    /// **[`mount`](Self::mount), into a slot the caller owns** — for a caller that cannot afford a
    /// ~10.5 KB value to exist on a stack even for the length of a move.
    ///
    /// `mount` returns by value, and a caller that then writes the result into a `static` gets two
    /// copies of the store on its own frame in practice, not one: LLVM builds the return value as a
    /// local and `memcpy`s it into the destination. Measured on the board when it first mounted one
    /// (FS7.5-c1): the helper doing `slot.write(FlatStore::mount(card))` carried a 10,688 B frame of
    /// its own, on top of `mount`'s 14,016 — a boot-chain cost of 25 KB against a residual main stack
    /// under 40. Through this constructor the caller's frame carries the pointer and nothing else.
    ///
    /// The shape is `obc2::Transaction::mount_in_place`'s, one layer down and for the same reason —
    /// and it is the shape `resource_guard.py`'s own failure text points a caller at.
    ///
    /// `#[inline(never)]` for both of `mount`'s reasons: a caller that inlined this would be back to
    /// building the store in its own frame, and the frame gate would stop being able to name it.
    #[inline(never)]
    pub fn mount_in_place(slot: &mut core::mem::MaybeUninit<Self>, dev: D) -> &mut Self {
        // `blank` is `#[inline(always)]`, so the literal is written **through** this pointer rather
        // than built beside it — the same reason the board's `init_static` is `inline(always)`.
        let store = slot.write(Self::blank(dev));
        store.bring_up();
        store
    }

    /// The struct literal, before [`bring_up`](Self::bring_up) has read a single block: an
    /// `Unformatted` store over `dev`, with an empty free map and no rows.
    ///
    /// Split out of [`mount`] so [`mount_in_place`](Self::mount_in_place) can place it, and
    /// `#[inline(always)]` so that placement is a write through the caller's pointer rather than a
    /// build-then-copy. The three `const` blocks are the frame-cost note above; they matter here
    /// rather than at either call site.
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
            }),
            free: const { RefCell::new(FreeMap::BLANK) },
            holds: const { RefCell::new([None; MAX_OPEN_OBJECTS]) },
            reservations: const { RefCell::new([None; MAX_RESERVATIONS]) },
            nonce: Cell::new(0),
            ride: Cell::new(None),
            recovered: Cell::new(None),
            listing_failed: Cell::new(false),
        }
    }

    /// §8: explicit, destructive, and the only transition into this format. The superblocks are
    /// destroyed first and written last, so a valid superblock implies a valid catalog
    /// unconditionally.
    ///
    /// This is also where the card's extent size is decided — §8's `max(1 MiB, card / 65,536)` rounded
    /// up to a power of two — and the superblock is the only place it is ever written. A card too large
    /// to express in 65,536 extents of the largest size (128 TiB, past every SD standard) is refused
    /// here rather than formatted into a store that would not mount.
    pub fn initialize(dev: D, store: StoreId) -> Result<Self, StoreError> {
        Self::write_empty_store(&dev, store)?;
        let store = Self::mount(dev);
        if store.mode().writable() {
            Ok(store)
        } else {
            Err(StoreError::Media)
        }
    }

    /// Destructively write a fresh empty flat store through this mounted store's device.
    ///
    /// This deliberately does not mutate resident state. The protocol command that calls it drains
    /// its response and immediately reboots; the next mount is the only point at which the new
    /// identity and geometry become observable. Keeping that transition boot-scoped avoids trying
    /// to replace a shared `FlatStore` while map, route, and weather readers still hold references.
    pub fn format_media(&self, store: StoreId) -> Result<(), StoreError> {
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

    /// Why this store refuses writes, if it does.
    pub fn mode(&self) -> Mode {
        self.served.get().mode
    }

    /// The card's identity. A client that has not seen it must treat its whole cache as void.
    pub fn store_id(&self) -> StoreId {
        self.store
    }

    /// The catalog commit sequence — the staleness hint a client compares its listing against.
    pub fn sequence(&self) -> u64 {
        self.served.get().sequence
    }

    /// Whether the current catalog has sequence space for `count` further commits.
    ///
    /// This deliberately checks the greatest well-formed gate rather than [`Self::sequence`]: a
    /// mount can fall back to the older catalog copy after the newer body's media read fails, but
    /// the next commit must still continue past the newer gate's sequence. Callers that publish an
    /// object which may need a compensating removal use `count == 2`; accepting that publication
    /// with only one sequence left would make the object impossible to retract.
    pub fn has_commit_capacity(&self, count: u64) -> bool {
        let served = self.served.get();
        served.mode.writable() && served.high_water.checked_add(count).is_some()
    }

    /// Entries the catalog holds.
    pub fn entry_count(&self) -> u16 {
        self.served.get().entry_count
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
        self.served.get().copy
    }

    /// The mark §5.5 step 2 continues from, which a fallback mount leaves above the served sequence.
    #[cfg(any(test, feature = "std"))]
    pub fn high_water(&self) -> u64 {
        self.served.get().high_water
    }

    /// Free extents, each of this card's recorded extent size (§4, §6).
    pub fn free_extents(&self) -> u32 {
        self.free.borrow().free()
    }

    /// That size, in bytes — the other half of what [`free_extents`](Self::free_extents) means. It is
    /// the card's, decided at initialization by §8's card-scaled rule, and a caller that wants free
    /// *bytes* rather than free extents needs both.
    pub fn extent_size(&self) -> u64 {
        self.geometry.extent_size()
    }

    /// The next `ObjectId` the cursor will hand out. A create names this in its `Put`; the commit
    /// advances the cursor past it and never rewinds.
    ///
    /// Reading it reserves nothing. Two creates that both read it before either commits would name the
    /// same id, and the second one's commit is refused as a duplicate key — acceptable only because
    /// `FLAT_Store_Protocol.md` §1 serves one transfer at a time.
    pub fn next_object_id(&self) -> ObjectId {
        ObjectId(self.served.get().next_object)
    }

    /// What §7.3 recovered, if a ride was recording when the card lost power.
    pub fn recovered_ride(&self) -> Option<RideRecovery> {
        self.recovered.get()
    }

    /// Random access over the checkpoint-durable bytes of the recording ride.
    ///
    /// This is deliberately separate from [`Store::open`]: an entry carrying `RECORDING` still has
    /// the catalog length from ride start and is not a normally served object. Recovery policy needs
    /// bounded reads of the first sample and a possible final footer, though, and either can straddle
    /// the boundary between write-once payload pages and the selected journal tail. This follows that
    /// one logical byte range without scanning the ride prefix.
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
    /// their extents are free again immediately — the same state the next mount would compute.
    ///
    /// A dropped `Allocation` releases nothing: the reservation row and its extents stay taken until
    /// this is called or the card is remounted, and there are only [`MAX_RESERVATIONS`] rows. Every
    /// path that abandons a transfer — a cancel, a refusal, a validator rejection, a lost link — has to
    /// come through here.
    pub fn cancel(&self, allocation: Allocation) {
        // Two short borrows of two different cells and no card command between them — rule 2.
        let mut rows = self.reservations.borrow_mut();
        let Some(row) = row_of(&rows, &allocation) else { return };
        let ranges = row.ranges;
        rows[allocation.slot as usize] = None;
        drop(rows);
        self.free.borrow_mut().release(&ranges);
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
    pub fn close(&self, handle: Handle) {
        let mut holds = self.holds.borrow_mut();
        let Some(hold) = holds[handle.slot as usize] else { return };
        if (hold.id, hold.revision) != (handle.id, handle.revision) {
            return;
        }
        // §2.1's teardown rule, and since the seam went `&self` it is the *only* thing standing
        // between a live [`StoreSource`](super::source::StoreSource) and a `close` that would pull the
        // extents out from under it: another reader still holds this row, so this close spends a
        // refcount and nothing else. The row, its ranges and its length survive untouched.
        if hold.readers > 1 {
            holds[handle.slot as usize] = Some(Hold { readers: hold.readers - 1, ..hold });
            return;
        }
        holds[handle.slot as usize] = None;
        // Dropped before the `find` below: rule 2 — no borrow across a card command.
        drop(holds);
        // What the entry still names, if it is still there at all. A media failure here leaves the
        // extents allocated until the next mount rebuilds the map from the catalog, which is the safe
        // direction: never hand out an extent an entry might name. A failed read is *not* evidence the
        // entry is gone, so it must not be read as one — freeing a live entry's extents would let the
        // next allocation overlap it, and an overlap is a rule only a mount checks.
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

    /// The payload length `handle` resolved, or `None` when it names a row that is no longer its own
    /// (a closed handle, or one whose slot has been reused).
    ///
    /// This is the length the handle keeps reading, not the entry's current one: §2.1 promises a
    /// handle serves the revision it opened, and an amend that trimmed the entry since does not
    /// shorten a reader that is already past it.
    pub fn handle_len(&self, handle: &Handle) -> Option<u64> {
        let holds = self.holds.borrow();
        holds[handle.slot as usize]
            .filter(|hold| (hold.id, hold.revision) == (handle.id, handle.revision))
            .map(|hold| hold.payload_len)
    }

    /// The device, for a bench or a harness that needs the card underneath. Nothing above the seam has
    /// any business with it.
    #[cfg(any(test, feature = "std"))]
    pub fn device(&self) -> &D {
        &self.dev
    }

    /// **Rule 2 broken on purpose**: one card command issued with the free map held.
    ///
    /// The positive control for [`granularity`](super::granularity), and it earns its five lines. That
    /// module enforces rule 2 by re-entering the store from inside the block driver and recording what
    /// is refused — but a probe that quietly stopped re-entering would report zero refusals forever and
    /// read as a pass. This gives it something it *must* catch. Nothing else calls it, and it is
    /// `cfg(test)`, so no device build has it.
    #[cfg(test)]
    pub(super) fn hold_free_across_a_command(&self) {
        let _free = self.free.borrow_mut();
        let mut block = [0u8; BLOCK];
        let _ = read_blocks(&self.dev, SUPERBLOCK[0], &mut block);
    }

    /// How many extent ranges the head revision of `id` holds — `None` when there is no such object.
    ///
    /// For the fixtures that need to *prove* an object is fragmented rather than assume it: an
    /// allocator change that quietly handed out one contiguous run would leave a straddling-read
    /// test passing while it had stopped straddling anything. Nothing else calls it, and it is
    /// `cfg(test)`, so no device build has it — the same shape as
    /// [`hold_free_across_a_command`](Self::hold_free_across_a_command) above.
    #[cfg(test)]
    pub(super) fn head_range_count(&self, id: ObjectId) -> Option<usize> {
        self.find(id).ok()?.1.map(|entry| entry.ranges.len())
    }

    /// The one method that takes `&mut self`, and the reason `store`, `geometry` and `extents` need
    /// no cell: it runs inside [`mount`](Self::mount) on a store nothing else can reach yet, and those
    /// three are never written again.
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
        // §6's count, at the extent size *this card* records — the decode above is what guarantees it
        // fits the entry's `u16` index, so nothing below has to clamp it.
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
        // well-formed gates contribute, so garbage in a dead gate's sequence field cannot poison the
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
                // A counter that has run out mounts read-only rather than wrapping: a revision no
                // commit can supersede (§3), or a gate sequence §5.5 step 2 cannot continue from.
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
        // No copy is being served, so the free map describes nothing: a failed [`load`] leaves its own
        // attempt's bitmap behind, and `free_extents()` is public.
        self.free.borrow_mut().reset(0);
    }

    /// §5.6 step 3 and 4 for one copy: the body CRC, every structural rule of §5.3, and the free
    /// bitmap built from the ranges as they go past. A failure leaves the caller free to try the
    /// next candidate — the bitmap is rebuilt from scratch each attempt.
    ///
    /// The free map is borrowed for the whole scan rather than per window, which is the module docs'
    /// **rule 4**: an exception to rule 2, and a safe one only because this runs inside
    /// [`mount`](Self::mount), before the store exists for anyone else to reach. It is the one place a
    /// borrow spans card commands where the reason is "nobody can be here" rather than "nobody who
    /// could be here wants this cell", which is why it is counted separately from rule 3's.
    fn load(&self, copy: usize, gate: &Gate) -> Result<Loaded, StoreError> {
        let mut free = self.free.borrow_mut();
        free.reset(self.extents);
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

        let mut structure = Structure::new(self.geometry);
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

    /// §7.3: read the 16 slots, take the candidate with the greatest checkpoint sequence. That is the
    /// whole mandatory decision. The slot CRC is checked from the greatest sequence down, so the 16
    /// KiB of tail bytes are only ever read for a slot that is about to be selected.
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
            // this writer ever produces: `journal` preflights the following sequence before it
            // touches media. Treat a hostile/restamped one like any other inadmissible candidate.
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
                // gate and proof for the next boot to retry; until repair succeeds no recovery is
                // exposed and `journal` has no resident ride state to append through.
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
            // exactly empty, and exposing that fact lets the board continue or discard it.
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
            // Also closes the uncertainty window of a prior sync that returned an error: the page
            // may read back through volatile cache while still needing this retry's durability gate.
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
        // One block, not a window: a binary search's probes are scattered, so a wide window would read
        // 4 KiB to look at 128 bytes of it.
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
        // No borrow is held across this loop, and `emit` is called from inside it: rule 2 and the
        // re-entrancy argument both live here. The closures the two passes pass in touch `Structure`
        // and `BodyWriter` and never the store.
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
                            entry.ranges.trim_to(self.geometry.extents_for(meta.payload_len) as u32)
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
                        if retained.is_none() && head.is_none() && meta.id.0 < self.served.get().next_object {
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
                        // The two facts this needs out of the row, copied out and the borrow dropped —
                        // the `find` above is already done, and nothing below touches the card.
                        let rows = self.reservations.borrow();
                        let row = row_of(&rows, allocation).ok_or(StoreError::Invalid)?;
                        let (ranges, written) = (row.ranges, row.written);
                        drop(rows);
                        if !meta.flags.holds_slack() && meta.payload_len != written {
                            return Err(StoreError::Invalid);
                        }
                        let mut entry = Entry { meta: *meta, ranges };
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
    /// them — a RAM-only hold that needs no durable record, because after a reboot the extents are
    /// free and there is no reader left to be surprised.
    fn release(&self, plan: &[Resolved]) {
        // Two cells at once and no card command between them — as in `allocate`, which holds `free`
        // and `reservations` together for the same reason. Admissible because the two are different
        // cells and neither `holds`' nor `free`'s own methods call back into the store, so there is
        // nothing here that could ask for either of them a second time.
        let holds = self.holds.borrow();
        let mut free = self.free.borrow_mut();
        for resolved in plan {
            // Both branches ask, because both take extents away from a reader: a removal takes the
            // whole entry, and an amend that trims a reserve takes its tail. `close` works out which
            // of a hold's extents the catalog has stopped naming and frees exactly those.
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
    /// short kind — and they are two, in order, because the free map and the reservation table are
    /// separate cells.
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
        // As with the hold table: no free row is `busy` on the wire, not `invalidRequest`. Taken
        // *before* the claim, so a refusal here gives the map back nothing to undo.
        let mut rows = self.reservations.borrow_mut();
        let slot = rows.iter().position(Option::is_none).ok_or(StoreError::Invalid)?;
        free.claim(&ranges).map_err(|_| StoreError::Invalid)?;
        let nonce = self.nonce.get().wrapping_add(1);
        self.nonce.set(nonce);
        rows[slot] = Some(Reservation { nonce, ranges, reserved: bytes, written: 0, staging: [0; BLOCK] });
        Ok(Allocation { slot: slot as u8, nonce, reserved: bytes, written: 0 })
    }

    /// Rule 3's first half: the reservation borrow is held across the card commands `fill` issues,
    /// because the row's staging block is what those commands write out of. Nothing on the read path
    /// wants this cell.
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
        // A fragmented allocation is several writes, so one of them can fail with the others already on
        // the card. The row's cursor goes back where it was: it is the reservation's identity as much as
        // its position — `row` matches an `Allocation` on it — so a cursor left ahead of the caller's
        // would make the reservation unnameable, which is a row and its extents wedged until the next
        // mount, `cancel` included. Rewinding costs nothing instead: the bytes already on the card are
        // the bytes the retry writes there.
        let start = row.written;
        if let Err(error) = fill(dev, geometry, row, bytes) {
            row.written = start;
            return Err(error);
        }
        allocation.written = row.written;
        Ok(())
    }

    /// §5.5, and the only durable state transition an object ever undergoes. Payload bytes are
    /// written and synchronized before it begins, so a cut at any point before the gate leaves those
    /// bytes anonymous and their extents free at the next mount.
    /// The granularity claim, made concrete: the ~36 card commands below — the two merge passes, the
    /// gate invalidation, the body stream, the gate write and their syncs — run with **no cell
    /// borrowed**. `served` is read out once as a `Copy` value, and the only borrows are the short
    /// no-I/O windows in `resolve` and `release` plus rule 3's staging flush.
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
        let sequence = served.high_water.checked_add(1).ok_or(StoreError::ReadOnly)?;
        let header = Header {
            store: self.store,
            sequence,
            next_object: served.next_object.max(greatest_id + 1),
            entry_count: count as u16,
        };
        let mut structure = Structure::new(self.geometry);
        // The commit's two windows, owned here and lent out: see [`EntryCursor`] for why they are not
        // owned by the reader and the writer that use them.
        let mut scan = [0u8; STREAM_WINDOW];
        let mut cursor = EntryCursor::new(served.copy, self.extents, served.entry_count);
        let written =
            self.merge(&mut cursor, &mut scan, plan, |entry| structure.accept(entry).map_err(|_| StoreError::Invalid))?;
        structure.finish(&header).map_err(|_| StoreError::Invalid)?;
        if written != header.entry_count {
            return Err(StoreError::Invalid);
        }
        // A pending logical gate must be repaired before an amend can publish its bytes. Removal is
        // deliberately different: its catalog gate makes the reserve, proof and torn payload page
        // unreachable, so discard neither needs nor should be blocked on failing media repair.
        if self.ride.get().is_some_and(|ride| {
            ride.pending_proof != 0
                && plan.iter().any(|resolved| resolved.key == (ride.id, ride.revision) && resolved.entry.is_some())
        }) {
            return Err(StoreError::Invalid);
        }

        // The payload is durable before the commit begins: whatever a `write` left in a staging
        // block goes to the card now.
        // Rule 3's second half, and the borrow is scoped to exactly this loop: at most four rows, one
        // card command each, and no reader wants this cell. Releasing it between the commands would
        // mean lifting a 512-byte staging block onto the frame this module measures.
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
        // §7.2's ride end: the last checkpoint's tail is on the card in a journal slot, not in the
        // ride's extents, and this is the commit that gives those bytes a length and a CRC. So the
        // partial page moves out of the slot and into the extents here — the same "payload
        // synchronized before the commit begins" rule the staging flush above obeys.
        staged |= self.flush_ride_tail(plan)?;
        if staged {
            sync(&self.dev)?;
        }

        let target = 1 - served.copy;
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

        // The gate landed: `target` is the truth, and everything the batch displaced is free. One
        // `set` of the whole `Served` value, which is the resident mirror of the atomic transition the
        // gate write just made on the card.
        //
        // The counter that ran out mid-session comes with it, from the same rule §5.6 applies at
        // mount: a store whose high-water mark has reached `u64::MAX` has no sequence for the next
        // commit, so this is the last one this card accepts. Read-only from here, reads still served.
        self.served.set(Served {
            mode: if header.sequence == u64::MAX { Mode::SequenceSpaceExhausted } else { served.mode },
            copy: target,
            sequence: header.sequence,
            high_water: header.sequence,
            next_object: header.next_object,
            entry_count: header.entry_count,
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
            //
            // **`max`, not assignment, and this PR is what made the difference matter.** The row is
            // shared by every reader of the key, so a plain assignment lets a *later* joiner shorten
            // what an *earlier* one is already serving. Before the seam went `&self` that sequence was
            // unreachable — an amend needed `&mut`, and a live source held `&` — so the only writer of
            // this field was a ride finalising, which only ever grows it. Now `source` → trimming
            // amend → second `open` is expressible, and assignment would give the original reader
            // `Err(Io)` at offsets below the `len()` it reported: a silent truncation, and exactly the
            // "never to *silent*" line `source`'s docs draw. Taking the maximum keeps the
            // adopt-the-longer intent and can never over-serve, because an amend only ever trims and
            // the row's ranges are the wider, first-resolved ones.
            hold.payload_len = hold.payload_len.max(entry.meta.payload_len);
            return Ok(Handle { slot: slot as u8, id: entry.meta.id, revision: entry.meta.revision });
        }
        // A full table is transient: some other reader is holding every row, and the answer is to
        // ask again rather than to reject the request. It now *says* so — `StoreError::Busy`, §3.9's
        // `busy` with detail `holds 2`, rather than the `Invalid` this returned while the two shared
        // a value. At `MAX_OPEN_OBJECTS = 16` the arm was unreachable and the difference was
        // theoretical; at 6 it is a state a rider can reach, and a client that reads it as
        // `invalidRequest` stops retrying something that would have worked a second later.
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
        if offset >= hold.payload_len {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(hold.payload_len - offset) as usize;
        let mut done = 0usize;
        let mut block = [0u8; BLOCK];
        while done < want {
            let located = hold.ranges.locate(self.geometry, offset + done as u64).ok_or(StoreError::Invalid)?;
            // The same narrowing rule as [`Located::whole_blocks`], one unit up: the bound is taken in
            // `u64` against a byte count that can exceed a device `usize`, and the result — never more
            // than what the caller asked for — is what narrows.
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

    /// The listing snapshots the copy and the count it was built against, and holds no cell borrow —
    /// which is what lets an iterator coexist with a commit now that both take `&self`.
    ///
    /// **It also snapshots the commit sequence, and stops if the store moves off it.** That case did
    /// not exist before the seam went `&self`: a listing could not outlive a commit, because the
    /// commit needed `&mut`. Now it can, and the untended version of this was genuinely unsafe to
    /// leave — one commit later the snapshotted copy is still intact, so the walk reads one commit
    /// stale and looks fine; **two** commits later that copy has been rewritten underneath the cursor,
    /// and the walk would serve the *new* generation's entries mid-listing with
    /// [`entries_ok`](Self::entries_ok) still answering `true`. A listing that silently splices two
    /// catalogs together and calls itself complete is exactly the failure this seam refuses
    /// everywhere else, so the sequence check turns it into the short listing it already knows how to
    /// report. Cost: two words on the iterator and one `Cell` read per entry.
    ///
    /// Every caller in the tree drains its listing inside the request that asked for it, so nothing
    /// observes this today; it is here so that nothing has to.
    fn entries(&self) -> impl Iterator<Item = EntryMeta> + '_ {
        self.listing_failed.set(false);
        let served = self.served.get();
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

    /// §7.2's ordinary checkpoint or rare rollover. The caller lends only bytes added since its last
    /// successful checkpoint; storage reconstructs the next full logical tail snapshot by streaming
    /// the previous slot into the next one. A rollover gates its reconstructed full-page proof, then
    /// gates the advanced logical remainder, and only then copies the proof page to the payload
    /// extent. Thus no cut exposes the 16 KiB proof as a logical checkpoint, and boot can complete
    /// the identical copy before exposing the remainder.
    ///
    /// The ride state is a `Cell`, so the page flushes and the slot write below hold no borrow at all
    /// — rule 2 — and the local `ride` this advances is the same local it always was.
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
        // One bounded interval can cross at most one page. Keeping that bound at the seam makes one
        // proof + one logical gate sufficient and reviewable without making the recorder hold a
        // page-sized snapshot.
        if checkpoint.append.len() > PROGRAM_PAGE {
            return Err(StoreError::Invalid);
        }
        // Continuing from the prior checksum verifies exactly the delta the caller says this
        // checkpoint adds. The durable tail itself is reread and CRC-verified while it is copied to
        // the next slot below.
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
        // Both gates already include the caller's delta. The retry proves it is asking for exactly
        // that logical checkpoint through the full payload CRC + resume anchor; applying `append`
        // again would double it. The recorder blocks sampling after an error and keeps the same
        // bytes until this repair succeeds.
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

    /// Moves the bytes past `flushed length` out of the newest journal slot and into the ride's own
    /// extents, so the length and CRC the finalising commit publishes describe bytes that are on the
    /// card. Reports whether anything was written.
    ///
    /// Everything up to `flushed length` is already there — §7.2 wrote it a page at a time — and the
    /// remainder is the partial page no checkpoint ever flushes, because a checkpoint only ever writes
    /// whole 16 KiB pages. The slot is where those bytes live, so the slot is where they come from:
    /// re-reading it costs at most 32 blocks once per ride and needs no buffer of its own.
    ///
    /// `&self`, which it always could have been: it moves bytes on the card and settles no resident
    /// state — that is [`settle_ride`](Self::settle_ride)'s job, after the gate. Saying so is now load
    /// bearing as well as honest, because the commit around this call holds an [`EntryReader`] over
    /// `self.dev` across it.
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
        // publishing a length for. Anything else and the commit would describe bytes it cannot produce.
        // The comparison is against the *reserve* the ride is recording into, not against the entry
        // being written — that one's ranges are already trimmed to the finalised payload.
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
    /// Ride end zeroes the 16 slot headers (§7.2), and this runs *after* the gate, so a media failure
    /// here cannot be reported: the commit already happened and `commit` promises that an `Err`
    /// changed nothing. §7.2 covers the cost of losing it — a cut during that zeroing is harmless,
    /// because no entry carries `RECORDING` and §5.6 never reads the slots.
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
            // `recovered` is a mount-time offer, not a mirror of the active ride. A fresh start is
            // already owned by its live recorder in this boot, so manufacturing a zero-length
            // recovery here would make a later recorder construction offer that new ride as if it
            // had survived a reset. If power is actually cut before its first journal slot, §7.3's
            // mount fallback synthesizes the required zero-length recovery then.
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
    /// The commit sequence this listing was built against. See [`Store::entries`] for why a listing
    /// that outlives it has to stop rather than carry on.
    sequence: u64,
    served: &'a Cell<Served>,
    failed: &'a Cell<bool>,
}

impl<D: BlockDevice> Iterator for Entries<'_, D> {
    type Item = EntryMeta;

    fn next(&mut self) -> Option<EntryMeta> {
        if self.index >= self.count {
            return None;
        }
        // A commit has landed since this listing was made, so the copy under the cursor is no longer
        // the one the store is serving and will be rewritten by the next commit. Reported through the
        // same channel a media failure is — the listing is short, and `entries_ok` says so — because
        // to the caller it is the same fact: this list is not the catalog.
        if self.served.get().sequence != self.sequence {
            self.failed.set(true);
            self.index = self.count;
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
