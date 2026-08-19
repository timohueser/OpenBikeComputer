//! The flat store on the board: the card binding, the boot mount, and the one storage task
//! (FS7.5-c1, epic #1256).
//!
//! This is the first slice that puts `obc_storage::flat` into the **shipping image**. Until now the
//! store existed on glass only inside `bin/flat_store_bench.rs`, which owns its own `Semmc` and
//! measures the store beside the app rather than under it. Three things move here:
//!
//! 1. **[`FlatCard`]** — the bench's `Card` generalized out of its private `static mut SEMMC` onto
//!    the app's one host, through [`flpr_mux::with_storage`](crate::flpr_mux::with_storage). Same
//!    shape as [`sd::SemmcCard`](crate::sd): a zero-sized handle, because all the state is the mux's.
//! 2. **[`mount_at_boot`]** — `FlatStore::mount` into a `.bss` slot behind `#[inline(never)]`, so a
//!    ~10.5 KB store never becomes a permanent poll-frame slot and its ~14 KB constructor frame
//!    never becomes part of the boot task's (issues #677, #1084, #1108).
//! 3. **[`storage_task`]** — the owner ruling of 2026-08-18 on #1256, in code: **reads go direct**
//!    through `&'static FlatStore` + `StoreSource`; **writes serialize through one task**, callers
//!    sending async messages and blocking only while they await confirmation.
//!
//! ## One card is one store *or* one filesystem — never both
//!
//! The flat store owns the **raw card from LBA 0** (`FLAT_Store_Format.md` §2): no partition table,
//! no boot record, no filesystem. FAT is a filesystem *on* a card. There is no LBA at which the two
//! can be laid out side by side, and any arrangement that tried would have `sd.rs`'s
//! `VolumeManager` and this store writing the same blocks with different meanings.
//!
//! So the dev window's "coexistence" is **coexistence of two code paths in one image, over
//! whichever card is in the slot** — not of two structures on one card. Boot classifies the card and
//! takes exactly one path, and §5.6 step 1 is already precisely that test:
//!
//! > *Read superblock A block 0; on failure read superblock B. Neither valid ⇒ the card is not a
//! > flat store.*
//!
//! `FlatStore::mount` **is** the probe — it never fails, it classifies — so the board does not need a
//! second, board-private superblock reader that could disagree with the store's own rule. On a FAT
//! card the whole probe is two block reads and [`Mode::Unformatted`]; the v1 stack then mounts
//! exactly as it did before this slice, and the store sits in its slot inert.
//!
//! ## Why no card can answer to both classifiers
//!
//! The ordering above would still be a coin toss if a card could satisfy both tests, so it is worth
//! writing down that **neither classifier can accept the other's card**, and that this holds in both
//! directions from facts already in the tree rather than from the order boot happens to ask in:
//!
//! - **A flat card can never FAT-mount.** `FLAT_Store_Format.md` §2 makes block 0 *deliberately not
//!   an MBR*: its bytes `510..511` are zero — the superblock CRC sits elsewhere precisely so that
//!   footer can stay zero — and `superblock.rs`'s encoder asserts it. The vendored `embedded-sdmmc`
//!   fork requires the `0xAA55` boot signature there before it will read a partition table or a BPB,
//!   so it refuses a flat card at its first block.
//! - **A FAT card can never flat-mount.** §5.6 step 1 validates a superblock magic and CRC at two
//!   fixed blocks; an MBR or a volume boot record carries neither, so `mount` returns
//!   [`Mode::Unformatted`].
//!
//! So the two are disjoint by construction, and the classification is a fact about the card rather
//! than a policy of this module. What the *ordering* buys is only honest reporting — see
//! [`crate::sd::bring_up_card`] for why FAT must not be tried first.
//!
//! ## What c1 does not do
//!
//! The renderer and the router still read through `sd.rs` (that is c2), and every transport still
//! writes through the v1 object store (that is c3). So a **flat card boots to a fault screen** with
//! the truth on RTT: the store mounted, the catalog is there, and this build cannot yet draw from
//! it. See [`boot_fault_for`] for why that fault is the honest one rather than a new screen.

use core::mem::MaybeUninit;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;

use obc_link::flat::{Ceilings, Engine, Policy, Reaction, RequestId};
use obc_storage::flat::store::MAX_BATCH;
use obc_storage::flat::{
    Allocation, BlockDevice, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectKind, RideCheckpoint, Store as _,
    StoreError,
};

use crate::semmc::{SemmcError, BLOCK_BYTES};

// ══════════════════════════════ the card ══════════════════════════════

// **The flat binding places no alignment buffer of its own, and that is a stack decision.** The
// sEMMC firmware's DMA wants 32-bit alignment; the store hands the device `[u8; 512]` frame locals
// and its 2 KiB/4 KiB streaming windows, none of which carries an alignment attribute, so whether a
// given call is aligned is a codegen accident and the binding has to be correct either way. It
// borrows `sd`'s 4-block buffer for that (`sd::with_bounce`) rather than placing one.
//
// A second buffer sized to §5.5's 8-block commit window would have cost 4 KiB — and on this part
// every `.bss` byte is a main-stack byte (`_stack_start − __euninit`), taken out of the deep-ride
// path's headroom, for a case that may never occur. What the shared 4-block buffer costs when it
// *does* fire is one extra card command per commit-body window; a mount's 2 KiB window is one chunk
// either way, so the figure c1 measures is unaffected. `sd::warn_bounce`'s one-shot line is how a run
// says which side of the alignment accident this build fell on — and if it fires and a measurement
// says the commit cost matters, the flat binding places its own buffer then, with the number in hand.

/// **The card, as `obc_storage::flat` wants it.**
///
/// Zero-sized on purpose, and that is the whole generalization this slice performs: the bench's
/// `Card` reached a `static mut SEMMC` of its own through a `with` helper documented as
/// *"the caller must not be inside another `with` — this binary never is"*, which is a claim a
/// single-threaded bench can make and an app with a ride loop, a BLE plane and a USB plane cannot.
/// Here every method is one [`flpr_mux::with_storage`](crate::flpr_mux::with_storage) call, so the
/// FLPR mode and the driver borrow are taken together, the re-entrancy assertion is the mux's, and
/// this type owns nothing that a second instance could duplicate.
///
/// `BlockDevice` takes `&self` throughout (`flat::device`), which — as that module's docs say — is
/// the fact that makes per-card-command borrow granularity implementable at all: the store reaches
/// the card holding none of its own cells.
#[derive(Clone, Copy)]
pub(crate) struct FlatCard;

impl FlatCard {
    /// The store addresses blocks in a `u64`; the card in a `u32`.
    fn lba(lba: u64) -> Result<u32, SemmcError> {
        u32::try_from(lba).map_err(|_| SemmcError::OutOfRange)
    }
}

impl BlockDevice for FlatCard {
    type Error = SemmcError;

    fn block_count(&self) -> Result<u64, SemmcError> {
        crate::flpr_mux::with_storage(|sd| sd.num_blocks())?.map(u64::from)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), SemmcError> {
        let start = FlatCard::lba(lba)?;
        let addr = buf.as_ptr() as usize;
        crate::flpr_mux::with_storage(|sd| {
            if addr.is_multiple_of(4) {
                return sd.read_blocks(start, buf);
            }
            // SAFETY: we are inside `with_storage`, which is where `with_bounce` requires its
            // caller to be, and nothing in this closure reaches another bounce user.
            unsafe {
                crate::sd::with_bounce(addr, |bounce| {
                    let mut done = 0usize;
                    while done < buf.len() {
                        let take = (buf.len() - done).min(bounce.len());
                        sd.read_blocks(start + (done / BLOCK_BYTES) as u32, &mut bounce[..take])?;
                        buf[done..done + take].copy_from_slice(&bounce[..take]);
                        done += take;
                    }
                    Ok(())
                })
            }
        })?
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), SemmcError> {
        let start = FlatCard::lba(lba)?;
        let addr = buf.as_ptr() as usize;
        crate::flpr_mux::with_storage(|sd| {
            if addr.is_multiple_of(4) {
                return sd.write_blocks(start, buf);
            }
            // SAFETY: as in `read`.
            unsafe {
                crate::sd::with_bounce(addr, |bounce| {
                    let mut done = 0usize;
                    while done < buf.len() {
                        let take = (buf.len() - done).min(bounce.len());
                        bounce[..take].copy_from_slice(&buf[done..done + take]);
                        sd.write_blocks(start + (done / BLOCK_BYTES) as u32, &bounce[..take])?;
                        done += take;
                    }
                    Ok(())
                })
            }
        })?
    }

    /// **Free on this transport, and the reason §5.5's budget reads the way it does.**
    ///
    /// `Semmc::write_blocks` polls CMD13 until the card has left `prg`, so the program cycle *is*
    /// the completion signal and every write is already durable when the store's next statement
    /// runs. §5.5 calls its three synchronizations the dominant term; on this card they cost
    /// nothing, and a commit's whole cost is its block writes plus the M33's per-entry work
    /// (`obc_storage::flat::cost`). A transport with a write-back cache would move that cost back
    /// here, and every commit figure would move with it.
    fn sync(&self) -> Result<(), SemmcError> {
        Ok(())
    }
}

// ══════════════════════════ the boot mount ══════════════════════════

/// The mounted store, resident for the life of the image.
///
/// `.bss`, and written **in place**: `FlatStore` is ~10.5 KB, most of it §6.2's 8 KiB free bitmap,
/// and this board's rules about values that size are not stylistic. See [`mount_at_boot`].
static mut FLAT_STORE: MaybeUninit<FlatStore<FlatCard>> = MaybeUninit::uninit();

/// What this layer costs the resident budget: the store and the write queue. The alignment bounce
/// is `sd`'s and is already counted there — see the note above [`FLAT_BOUNCE_WARNED`].
///
/// The recording caller's 32,256-byte ride tail (§7.1) is **not** here — no ride records to the flat
/// store until FS8 (#1390), and a budget row for a buffer nothing allocates would be a lie in the other
/// direction. It joins this sum in the slice that starts recording.
pub(crate) const RESIDENT_BYTES: usize =
    core::mem::size_of::<FlatStore<FlatCard>>() + REQUEST_QUEUE_BYTES + MAP_READ_BYTES + ENGINE_BYTES;

/// **Everything the read cutover keeps resident on this arm** (FS7.5-c2): the session-long
/// [`MAP_SOURCE`] *and* the [`MAP_NAME`] the same boot step captures.
///
/// Both, because this budget's discipline is that every resident byte is named — an itemization
/// that quietly omits 28 B is worse than one that admits it, since the next reader has no way to
/// know which of the two it is. (The first version of this constant counted only the source and
/// still called itself the whole cost; the review caught it.)
pub(crate) const MAP_READ_BYTES: usize = core::mem::size_of::<obc_storage::flat::StoreSource<'static, FlatCard>>()
    + core::mem::size_of::<heapless::String<24>>();

/// The store is the free bitmap plus its rows; if that ever stops being true the budget note above
/// is wrong before anything else notices.
const _: () = assert!(core::mem::size_of::<FlatStore<FlatCard>>() > 8 * 1_024);

/// **Mount the card's flat store into the `.bss` slot, and hand back the one `&'static` to it.**
///
/// `#[inline(never)]` is load-bearing twice over, and neither reason is style:
///
/// - **The store must not become a poll-frame slot.** A `FlatStore` built by-value inside the boot
///   task's async block is a permanent ~10.5 KB slot in that task's poll frame, allocated at entry
///   on every poll — the #1084/#1108 mechanism exactly, which boot-bricked develop for five days.
///   The `.bss` slot plus this out-of-line helper is the pattern `mount_terrain` and
///   `ObjectStore::empty`/`hydrate` already established for the same reason.
/// - **The constructor's own frame must stay transient, and must not be paid twice.** `mount` links
///   at ~14 KB (CI's `--match 'obc_storage::flat' --limit 16384` gate): `load` streams the catalog in
///   the frame that is building the store, which is why `MOUNT_STREAM_BLOCKS` is half a commit's
///   window. In an ordinary call that frame pops on return; inlined into the boot task's coroutine it
///   would be permanent. Called from here it is one sibling step of the boot chain, which is what
///   `resource_guard.py board`'s `boot_chain_roots` measures — and this helper is baselined as one of
///   those roots, so the gate sees it.
///
///   It goes through [`FlatStore::mount_in_place`] rather than `mount`, and the difference is
///   measured, not stylistic: `slot.write(FlatStore::mount(card))` builds the store as a local of
///   *this* function and `memcpy`s it into the slot, which put a second 10,688 B frame on the boot
///   chain beside `mount`'s own 14,016. Placed, this frame carries a pointer.
///
/// **This is `mount`'s only caller and it runs exactly once**, before anything is spawned, so the
/// slot is written once per boot and the `&'static` handed out is the only reference. A warm reset
/// re-enters and overwrites in place; `FlatStore` has no `Drop`, which is the `init_static`
/// contract.
///
/// The card must already be up — [`crate::sd::bring_up_card`] first, or every probe read is
/// `SemmcError::NoBoot` and the store classifies a perfectly good card as unformatted.
#[inline(never)]
pub(crate) fn mount_at_boot() -> &'static FlatStore<FlatCard> {
    // SAFETY: sole writer of FLAT_STORE; `mount_at_boot` runs once per boot on the one thread-mode
    // executor, before any task that could hold a reference exists, so this `&mut` is the only live
    // borrow of the slot. The write is unconditional — no `StaticCell` one-shot flag a warm reset
    // could find already set — and `FlatStore` has no `Drop`, which is the `init_static` contract.
    let store = unsafe { FlatStore::mount_in_place(&mut *core::ptr::addr_of_mut!(FLAT_STORE), FlatCard) };
    &*store
}

/// **What the probe found, in the terms boot has to act on.** The three outcomes are the three
/// different things a rider's card can be; `Mode`'s six variants collapse onto them here so `main`
/// never has to re-derive the mapping.
pub(crate) enum Card {
    /// A flat store this build can read (`Mode::readable()` — read-write, or read-only because a
    /// revision or sequence space is exhausted). The flat path takes the card.
    Flat,
    /// §5.6 step 1: neither superblock is valid, so **this is not a flat store**. Two block reads
    /// and out; the v1 FAT stack owns the card exactly as it did before this slice.
    NotFlat,
    /// A card that *is* a flat store and will not serve one: `CatalogUnreadable` (no well-formed
    /// gate, or no candidate body validated — §5.6 steps 2–3 call this media damage, since no state
    /// the store can produce leaves both gates ill-formed) or `CardTooSmall` (the card in the slot
    /// is smaller than the superblock on it describes).
    ///
    /// **Not a fall-through to FAT.** The superblock says a flat store was written here, so a FAT
    /// mount would fail anyway and report the failure of the stack that was never on this card.
    /// `StorageFault` is the honest superset — *"something below the filesystem broke, and it was
    /// not the card's absence"* — and it is what [`crate::sd::mount_fat`] already reports for the
    /// mirror case, a card whose FAT volume will not mount.
    FlatBroken(obc_app::BootFault),
}

/// Collapse §5.6's classification onto the three cards above, logging the reason for the one that
/// is neither a working store nor a plain FAT card.
pub(crate) fn classify(store: &FlatStore<FlatCard>) -> Card {
    let mode = store.mode();
    if mode.readable() {
        return Card::Flat;
    }
    if mode == Mode::Unformatted {
        return Card::NotFlat;
    }
    defmt::error!(
        "flat: the card carries a flat store that will not serve ({}) — STORAGE FAULT, and no FAT fall-through: a superblock is on this card",
        defmt::Debug2Format(&mode)
    );
    Card::FlatBroken(obc_app::BootFault::StorageFault)
}

/// **Which boot fault a card carrying a flat store earns in c1** — the honesty rule, *bound* here
/// rather than decided here.
///
/// The decision is `obc_app::flat_boot_fault`, beside the FAT scan's `boot_fault` and tested where
/// tests run: the board crate is bare metal and CI runs none in it, so a rule written at this call
/// site would be a rule nothing checks. All this does is reduce the catalog to the two facts that
/// rule takes — how many entries are map objects, and whether the walk finished.
///
/// It takes the counts rather than the store, because [`report`] has already walked the catalog once
/// and a second walk is not free: a listing re-reads the live prefix off the card — at 1,027 entries
/// that is ~69 read commands and about a tenth of a second of the boot this slice exists to measure.
/// One walk, both consumers.
///
/// Not a new `BootFault` variant, and that is a decision rather than an omission: the screen would
/// have to say "this firmware cannot read this card yet", which is true for exactly the length of the
/// dev window and would then be a dead string with a translation, a repertoire-test row and a
/// `copy()` arm to delete. The truth that *is* durable — mode, entry count, sequence, free extents,
/// what the mount cost — goes to RTT, where a dev-window fact belongs.
pub(crate) fn boot_fault_for(catalog: Catalog) -> obc_app::BootFault {
    obc_app::flat_boot_fault(catalog.maps, catalog.listing_complete)
}

/// What one walk of the catalog found. [`report`] produces it and [`boot_fault_for`] consumes it, so
/// the mount's listing is paid for once.
#[derive(Clone, Copy)]
pub(crate) struct Catalog {
    /// Entries whose kind is a map (§3.1's `MapShard` / `MapSetManifest`).
    pub(crate) maps: usize,
    /// False when the walk stopped short because a commit moved the catalog under its cursor
    /// (`flat::source`'s stale-listing rule) — the flat twin of the FAT scan's `unlistable`, and
    /// evidence of a map rather than of an empty card.
    pub(crate) listing_complete: bool,
}

// ══════════════════════════ the storage task ══════════════════════════

/// Write requests queued at once.
///
/// **Two, and the small number is the point.** Writes are serialized by construction, so queue
/// depth buys nothing but latency hiding: one slot is in service and one is waiting, and a third
/// sender simply awaits its turn in `Sender::send` rather than being refused. A deeper queue would
/// only park more messages — and a `Job` is not small (see [`Request`]), so depth is `.bss`.
const REQUEST_QUEUE: usize = 2;

/// The queue's resident cost, for the budget table in `main.rs`. Named rather than left anonymous
/// because it is the one part of this layer whose size is a *design* choice rather than a
/// consequence: it is `REQUEST_QUEUE` times a `Job`, and a `Job` is as large as the largest request.
pub(crate) const REQUEST_QUEUE_BYTES: usize =
    core::mem::size_of::<Channel<CriticalSectionRawMutex, Job, REQUEST_QUEUE>>();

/// One unit of write work, exactly as the seam spells it.
///
/// **Reads are not here, and that is the design.** `ByteSource::read_at` is synchronous and
/// latency-bound; routing a render's reads through a channel would add a scheduler round trip to
/// every one of them and buy nothing, because the store already serves a reader with **no borrow
/// held** at every card command of a running write (`flat::store`'s rule 2, pinned by
/// `flat::granularity`). The ruling's word for this shape is *hybrid*.
///
/// Everything carried here is owned or `'static` on purpose: a request outlives the statement that
/// sent it, so a borrowed payload would need a lifetime the channel cannot express. `Allocation` is
/// `Copy` RAM state, `Handle` is a row token, a batch is at most [`MAX_BATCH`] mutations, and a
/// write's bytes come from a `'static` staging buffer — which is what c3's transports already have
/// (the USB plane stages into the scratch arena).
///
/// **The whole enum is dead until c3, and linked as nothing.** c1 stands the write half up and
/// measures it; the shipping callers — the transports, the ride journal — arrive in c3. With no
/// caller the linker keeps none of it, so the cost of the surface existing early is zero bytes, and
/// the benefit is that c2/c3 plug into a serialization that has already been on glass. Even under
/// `flat-exercise`, `Journal` and `Close` have no caller: the ride journal is c3's, and nothing c1
/// opens outlives its `with_source` scope. They are here because the seam is six operations, and a
/// task that served four of them would not be the write half.
///
/// The variants also differ in size by an order of magnitude — a `Commit` carries up to
/// [`MAX_BATCH`] `Mutation`s and each embeds an `EntryMeta` with §9's 48-byte display name, so the
/// enum is as large as a batch. That is inherent, not accidental: `no_std` with no allocator, so the
/// lint's advice (box the large field) is not available, and the two alternatives — a `'static`
/// batch slot per caller, or a commit carrying one mutation and giving up §5.5's atomicity across a
/// batch — are both worse than the ~1 KB [`REQUEST_QUEUE_BYTES`] accounts for.
#[allow(dead_code, clippy::large_enum_variant)]
pub(crate) enum Request {
    /// §6's extent reservation.
    Allocate { bytes: u64 },
    /// Append to a reservation. Replies with the advanced [`Allocation`], because the seam takes it
    /// by `&mut` and a channel cannot lend one.
    Write { allocation: Allocation, bytes: &'static [u8] },
    /// §5.5's atomic batch. Replies with the commit sequence.
    Commit { batch: heapless::Vec<Mutation, MAX_BATCH> },
    /// §7.2's ride checkpoint.
    Journal { checkpoint: RideCheckpoint<'static> },
    /// Give a reservation back without publishing it.
    Cancel { allocation: Allocation },
    /// Return a hold row. Refused rather than obeyed while another reader holds it (`flat::source`).
    Close { handle: Handle },

    // ── the protocol-v4 engine (FS7.5-c3a) ──────────────────────────────────────────────────────
    //
    // The engine runs *here*, inside the one task that writes, and the transports are pure record
    // shippers. That is not a convenience: `obc_link::flat::Store` is synchronous throughout — the
    // mutators included — so an engine driven from a transport task would have to reach the card
    // from a second execution context, which is exactly what the #1256 owner ruling forbids. Sitting
    // it behind this queue makes "one engine, one owner" (`FLAT_Store_Protocol.md` §1) a property of
    // the type rather than a convention, and it is what `Writer::call`'s one-slot-per-concurrently-
    // live-call contract was written for.
    /// One whole control record (§3.1), and the buffer the reaction's bytes land in.
    ///
    /// `out` is the **caller's** `'static` buffer and rides back in [`Outcome::Reacted`]. It is a
    /// borrow rather than a copy for the same reason [`Request::Write`]'s bytes are: a request
    /// outlives the statement that sent it, and a `LIST` page or a stream record is up to a link
    /// ceiling of bytes that would otherwise be memcpy'd twice per record.
    Control { record: &'static [u8], out: &'static mut [u8] },
    /// One whole stream record (§3.8): the 16-byte frame followed by exactly its payload.
    Stream { record: &'static [u8], out: &'static mut [u8] },
    /// Pump the engine once — a live `GET`'s next record, or an error owed to a dropped transfer.
    /// An adapter repeats this until the reaction is [`Reaction::Idle`]; a driver that stops pumping
    /// stalls a download.
    Pump { out: &'static mut [u8] },
    /// A link came up with these record ceilings (§5.1, §5.2). Releases whatever the previous link
    /// held and re-pins the engine to what *this* link negotiated. `Err(StoreError::Invalid)` is
    /// §5.1's "a link below the protocol floor is refused rather than truncated".
    LinkUp { control: usize, stream: usize },
    /// §3.8's third form of cancel: the link went away. Answers nobody, because there is nobody
    /// left to answer.
    LinkLost,
    /// The live transfer's `RequestId`, if one owns the engine.
    ///
    /// The one *read* on this queue, and it earns its place: §5's cross-channel ordering makes an
    /// adapter hold a stream frame for a `RequestId` it has not yet seen admitted, and "has this
    /// been admitted" is a question only the engine can answer. One round trip, and only inside the
    /// race window.
    LiveTransfer,
}

/// What one [`Request`] produced.
///
/// **Dead in the default build until c3, and linked as nothing.** c1 stands the write half up and
/// measures it; the shipping callers — the transports, the ride journal — arrive in c3. With no
/// caller the linker keeps none of this, so the cost of the surface existing early is zero bytes and
/// the benefit is that c2/c3 plug into a serialization that has already been on glass. The
/// `flat-exercise` build is what exercises it today.
#[allow(dead_code)]
pub(crate) enum Outcome {
    Allocated(Allocation),
    /// The allocation, advanced by the bytes written.
    Wrote(Allocation),
    /// §5.5's commit sequence.
    Committed(u64),
    /// Nothing to hand back: `journal`, `cancel`, `close`.
    Done,
    /// What the engine wants done, and the caller's buffer back with the bytes in it.
    Reacted {
        reaction: Reaction,
        out: &'static mut [u8],
    },
    /// The live transfer's `RequestId`, or `None` when the engine is idle.
    Live(Option<RequestId>),
}

/// The caller's half of one round trip: the answer, **tagged with the request it answers**.
///
/// The tag is what makes [`Writer::call`] cancellation-safe, and without it this seam is not.
/// `call` is an `async fn`, so its future can be dropped between the send and the wait — by a
/// `select`, a timeout, or an early `return` in a caller that is racing something else. The task
/// has no idea; it serves the request and signals the slot anyway. The **next** caller to use that
/// same slot would then wake on a value that answers a request it never made, and take a stale
/// `Allocation` or a stale commit sequence for its own. Every transport in c3 is built on this call,
/// so the failure would be a transfer publishing against another transfer's reservation.
///
/// A `Signal` rather than a channel is still right — exactly one value per round trip, and a
/// dropped caller must not leave a *queued* reply behind either — but "the value in the slot is
/// mine" has to be checked rather than assumed.
pub(crate) type Reply = Signal<CriticalSectionRawMutex, (u32, Result<Outcome, StoreError>)>;

/// Hands out [`Job::tag`]s. Monotonic and never reused in any window that matters: a collision needs
/// 2^32 intervening calls *and* the same reply slot, on a device that issues a few writes a second.
static NEXT_TAG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// A request, where its answer goes, and which request that answer is for.
pub(crate) struct Job {
    request: Request,
    reply: &'static Reply,
    /// Matched by [`Writer::call`] against its own — see [`Reply`].
    tag: u32,
}

/// The queue itself. One producer-agnostic channel: any task may send, exactly one task receives.
static REQUESTS: Channel<CriticalSectionRawMutex, Job, REQUEST_QUEUE> = Channel::new();

/// **The write half's front door**, handed to every plane that mutates the store.
///
/// `Copy`, so it costs a caller nothing to hold, and it carries no store reference at all — which is
/// what makes "there is exactly one execution context that writes" a property of the type rather
/// than a convention.
///
/// Like [`Request`], it has no shipping caller until c3; the `flat-exercise` build is what drives it
/// today, and the linker keeps none of it in a build with neither.
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "flat-exercise"), allow(dead_code))]
pub(crate) struct Writer {
    requests: Sender<'static, CriticalSectionRawMutex, Job, REQUEST_QUEUE>,
}

#[cfg_attr(not(feature = "flat-exercise"), allow(dead_code))]
impl Writer {
    /// Send `request` and wait for the store's answer.
    ///
    /// **The caller blocks here and nowhere else**, which is the ruling's "callers block only if
    /// they await confirmation" — and it blocks on its own `Signal`, not on the store: the queue slot
    /// is released the moment the task takes the job. A caller that does not need the answer wants a
    /// fire-and-forget variant, and c3 adds one when it has such a caller; c1 has none, so there is
    /// none to be dead.
    ///
    /// **Cancellation-safe**: dropping this future between the send and the answer is legitimate
    /// (a `select` lost, a caller that stopped caring), and the answer that arrives afterwards is
    /// discarded by the *next* caller rather than mistaken for its own — see [`Reply`] for why that
    /// is the whole reason a tag exists. The slot is never `reset`, only advanced past.
    ///
    /// `reply` is a `'static` slot the caller owns, and the contract on it is **one slot per
    /// concurrently live call**. A slot may be reused freely *across time* — that is what the tag is
    /// for — but it must never be awaited by two callers at once: a `Signal` holds **one value and
    /// one waker**, so two live waiters on one slot lose an answer (the second `signal` overwrites
    /// the first before either polls) and wake each other instead of themselves, which on this board
    /// is an executor-starving ready-loop and then a watchdog reset. The `debug_assert` above encodes
    /// the same sequential-only contract from the other side.
    pub(crate) async fn call(&self, request: Request, reply: &'static Reply) -> Result<Outcome, StoreError> {
        let tag = NEXT_TAG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.requests.send(Job { request, reply, tag }).await;
        loop {
            let (answered, outcome) = reply.wait().await;
            if answered == tag {
                return outcome;
            }
            // Someone else's answer, left in this slot by a `call` whose future was dropped before
            // it collected. Consuming it is the point — `Signal::wait` takes the value, so the slot
            // is now empty and the next wait is for ours. Deliberately not a `warn!`: a dropped
            // caller is a legitimate outcome of a `select`, not a fault.
            debug_assert!(answered < tag, "a reply slot answered a tag that has not been issued");
        }
    }
}

/// True once [`arm`] has handed the receive end to the storage task.
static ARMED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// **The [`Writer`], if there is anything on the other end** — `None` on a card that is not a flat
/// store.
///
/// The `Option` is not defensive typing, it is the difference between an error and a hang. The
/// queue is a `static`, so `REQUESTS.sender()` succeeds whether or not a task is draining it; on a
/// FAT card no storage task is ever spawned, and a c3 caller that sent into that channel would fill
/// two slots and then wait **forever** in `Sender::send` — no timeout, no error, no log. A write
/// path that cannot run should say so at the first call, not wedge the plane that made it.
#[cfg_attr(not(feature = "flat-exercise"), allow(dead_code))]
pub(crate) fn writer() -> Option<Writer> {
    ARMED.load(core::sync::atomic::Ordering::Relaxed).then(|| Writer { requests: REQUESTS.sender() })
}

/// **The one task that writes.**
///
/// It owns nothing: the store is `&'static` and every reader has one too. What it owns is the
/// *right to call the mutators*, and that is the whole of the ruling's serialization — no lock, no
/// `Mutex`, no `RefCell` at this layer. The store's own cells are what make a concurrent reader
/// safe, and this task is what makes concurrent *writers* impossible instead of merely refused.
///
/// **What it deliberately does not have is a lock around a commit.** The ruling's granularity law —
/// *per card command, never per commit* — is a property of `obc_storage::flat`, pinned by
/// `flat::granularity` (a re-entrant probe device asserts that reads, listings and free-space
/// queries are served at every card command of a write and of a commit). This layer's obligation is
/// not to take that away, and the way it would have taken it away is a coarse lock held across
/// `commit`. There is none: a render's `read_at` never touches this channel.
///
/// **The remaining stall is the executor's, not the store's, and c1 measures it rather than
/// claiming otherwise.** `Store::commit` is synchronous — ~36 card commands over ~250 ms at 1,024
/// entries — and this is one cooperative thread-mode executor, so between its first and last command
/// no other task is polled. The borrow granularity that lets a reader *be served* mid-commit is
/// necessary and, on a single-threaded executor, not sufficient: the store would also have to hand
/// control back between commands. Closing that needs a yield seam in `obc-storage` — a resumable
/// commit, an async device, or a per-command callback — which is not this slice's and is not
/// smuggled in as a lock here. **The follow-up is recorded on #1420**, with the reasoning and the
/// measurement that should size it; `--features flat-exercise` is what produces that measurement.
#[embassy_executor::task]
pub(crate) async fn storage_task(
    store: &'static FlatStore<FlatCard>,
    requests: Receiver<'static, CriticalSectionRawMutex, Job, REQUEST_QUEUE>,
) -> ! {
    defmt::info!("flat: storage task up — the write half and the v4 engine are serialized here, reads stay direct");
    // The engine and its policy live in `.bss`, built out of line: see `engine_slot`.
    let engine = engine_slot();
    let mut policy = BoardPolicy;
    loop {
        let job = requests.receive().await;
        // The FLPR is switched per card command by `flpr_mux::with_storage`, so nothing is held
        // across this call and there is no mode session to acquire around the batch.
        let outcome = serve(store, engine, &mut policy, job.request);
        // The tag rides back with the answer: the caller may be gone, and the next user of this slot
        // has to be able to tell that this value is not theirs. See `Reply`.
        job.reply.signal((job.tag, outcome));
    }
}

/// One request against the store. **Synchronous and out of line**: it is the whole write surface, so
/// its frame is measured as its own symbol rather than folded into the task's poll frame — the same
/// reason `mount_at_boot` is `#[inline(never)]`.
#[inline(never)]
fn serve(
    store: &FlatStore<FlatCard>,
    engine: &mut BoardEngine,
    policy: &mut BoardPolicy,
    request: Request,
) -> Result<Outcome, StoreError> {
    match request {
        Request::Allocate { bytes } => store.allocate(bytes).map(Outcome::Allocated),
        Request::Write { mut allocation, bytes } => {
            store.write(&mut allocation, bytes)?;
            Ok(Outcome::Wrote(allocation))
        }
        Request::Commit { batch } => store.commit(&batch).map(Outcome::Committed),
        Request::Journal { checkpoint } => store.journal(checkpoint).map(|()| Outcome::Done),
        Request::Cancel { allocation } => {
            store.cancel(allocation);
            Ok(Outcome::Done)
        }
        Request::Close { handle } => {
            store.close(handle);
            Ok(Outcome::Done)
        }
        Request::Control { record, out } => {
            let reaction = engine.on_control(store, policy, record, out);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::Stream { record, out } => {
            let reaction = engine.on_stream(store, policy, record, out);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::Pump { out } => {
            let reaction = engine.poll(store, out);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::LinkUp { control, stream } => {
            // §5.1: a link below the protocol floor cannot carry this protocol and the adapter
            // refuses the connection rather than truncating. `Ceilings::new` returning `None` *is*
            // that refusal, and it reaches the adapter as the one error it can act on.
            let Some(ceilings) = Ceilings::new(control, stream) else {
                defmt::warn!(
                    "flat/v4: link offers control {=usize} B / stream {=usize} B — below the protocol floor, refused",
                    control,
                    stream
                );
                return Err(StoreError::Invalid);
            };
            // A new link has no live transfer, and the old one's holds must go back before the
            // engine forgets them: `on_link_lost` is the release, and the rebuild is what re-pins
            // the ceilings a *negotiated* ATT MTU decides. Rebuilding in place rather than mutating
            // is deliberate — `Ceilings` has no setter, and an engine whose ceilings changed under a
            // live transfer would frame the rest of it against a link that no longer exists.
            engine.on_link_lost(store);
            *engine = Engine::new(ceilings);
            defmt::info!("flat/v4: link up — control {=usize} B, stream {=usize} B", control, stream);
            Ok(Outcome::Done)
        }
        Request::LinkLost => {
            engine.on_link_lost(store);
            Ok(Outcome::Done)
        }
        Request::LiveTransfer => Ok(Outcome::Live(engine.live_transfer())),
    }
}

// ══════════════════════════ the protocol-v4 engine ══════════════════════════

/// The engine's staging buffer, in bytes.
///
/// **512 — the minimum the engine's own `const` assertion allows — and that is a c3a decision with a
/// c3b sequel.** The stage exists to turn a burst of small link records into few large card writes,
/// and on BLE there is no burst to turn: a CoC SDU is 245 bytes and the radio delivers a handful per
/// connection interval, so a 4 KiB stage would batch writes the link cannot feed it fast enough to
/// fill. What it *would* cost is real — [`Engine`] embeds the buffer, and a 4 KiB engine built by
/// value is 4 KiB of transient frame at a depth this board measures (#1084/#1108).
///
/// USB is the case the default was written for, and c3b raises this with that transport's measured
/// number in hand rather than inheriting a guess from the radio.
const ENGINE_STAGE: usize = 512;

/// The one engine, bound to this board's store.
pub(crate) type BoardEngine = Engine<FlatStore<FlatCard>, ENGINE_STAGE>;

/// `.bss`, for the reason every value this size on this board is: an engine built by value inside
/// [`storage_task`]'s async block is a permanent slot in that task's poll frame, allocated at entry
/// on every poll (#677, #1084, #1108).
static mut ENGINE: MaybeUninit<BoardEngine> = MaybeUninit::uninit();

/// The engine's resident cost. Named for the budget table in `main.rs`.
pub(crate) const ENGINE_BYTES: usize = core::mem::size_of::<BoardEngine>();

/// **Build the engine into its slot and hand back the one `&'static mut`.**
///
/// `#[inline(never)]` so the constructor's frame is a transient sibling rather than part of the
/// task's poll frame. It is small — [`ENGINE_STAGE`] is 512 B and the rest is a live-transfer record
/// — but the rule is about *where a value is built*, not how big it is, and c3b raises the stage.
///
/// The initial ceilings are the device's **preferred** BLE link (§5.1: `ATT_MTU - 3` at the
/// preferred 247-byte MTU, and a CoC SDU of the packet pool's MTU − 6). They are re-pinned per
/// connection by [`Request::LinkUp`], which is what makes a link that negotiated something smaller
/// correct rather than merely usual.
///
/// # Safety
/// Sole writer of [`ENGINE`]; called exactly once, from [`storage_task`], which is spawned once.
#[inline(never)]
fn engine_slot() -> &'static mut BoardEngine {
    let ceilings = Ceilings::new(PREFERRED_CONTROL_CEILING, PREFERRED_STREAM_CEILING)
        .expect("the device's preferred link is above the protocol floor");
    // SAFETY: sole writer; `storage_task` is spawned exactly once and nothing else names this slot.
    unsafe { crate::init_static(core::ptr::addr_of_mut!(ENGINE), Engine::new(ceilings)) }
}

/// §5.1's control ceiling at the device's preferred 247-byte ATT MTU.
pub(crate) const PREFERRED_CONTROL_CEILING: usize = 244;
/// The CoC SDU the packet pool yields (`DefaultPacketPool::MTU - 6`).
pub(crate) const PREFERRED_STREAM_CEILING: usize = 245;

/// The two decisions the engine cannot make for itself (`FLAT_Store_Protocol.md` §3.6, §4).
///
/// **Both are unfilled in c3a, and the defaults are the honest answers rather than placeholders.**
///
/// - `accept` is §3.6's "runs the kind's validator". The hook the seam offers is
///   `(kind, payload_len)` — the payload itself is in an uncommitted allocation the engine cannot
///   re-read, and `open` resolves committed entries only. So a real OBCR/OBCW/OBCM magic check
///   needs a read hook `obc_link::flat::Policy` does not have, and inventing a length-only
///   "validator" here would be a check that passes everything while reading as though it did not.
///   What *is* enforced is the whole-payload CRC-32 the engine verifies before this is called, which
///   is what catches a damaged transfer; what is not enforced is a well-formed transfer of the wrong
///   bytes. Filling this is a named follow-up on #1420.
/// - `validate_package` and `hand_off` are §4's arm. They need `obc-dfu`, the RRAM boot page and a
///   reboot, and the default refuses — which is correct for a build that cannot arm: a device that
///   committed a rollback reserve it could never hand off would strand extents no client can free
///   (§3.7 refuses to `REMOVE` a `RESERVED` entry). `ARM` therefore answers `rejected`.
pub(crate) struct BoardPolicy;

impl Policy for BoardPolicy {}

/// **Take the receive end and arm the write half**, for the one `spawn` in `main`.
///
/// One function rather than two because the two facts are the same fact: there is a consumer, and
/// therefore [`writer`] may hand out senders. Splitting them would allow an arming that never
/// spawned (senders that wedge) or a spawn that never armed (a live task no one can reach), and
/// both are silent.
///
/// Call it exactly once, at the spawn site. A second call would make a second consumer and the
/// serialization this whole module exists for would be gone; the `debug_assert` is what says so on
/// the host, and the single call site is what makes it true on the device.
pub(crate) fn arm() -> Receiver<'static, CriticalSectionRawMutex, Job, REQUEST_QUEUE> {
    let already = ARMED.swap(true, core::sync::atomic::Ordering::Relaxed);
    debug_assert!(!already, "the flat store's write half was armed twice — that is a second consumer");
    REQUESTS.receiver()
}

// ══════════════════════════ the boot report ══════════════════════════

/// What the mount found, on RTT. **This is where a flat card's truth lives in c1** — the glass gets
/// the honest fault screen ([`boot_fault_for`]) and no dev-window prose.
///
/// It returns the [`Catalog`] its walk produced, so `boot_fault_for` decides from this listing
/// rather than taking a second one off the card.
pub(crate) fn report(store: &FlatStore<FlatCard>, mount_us: u64) -> Catalog {
    defmt::info!(
        "flat: {} at sequence {=u64} — {=u16} entries, {=u32} free extents of {=u64} B, mount {=u64} us",
        defmt::Debug2Format(&store.mode()),
        store.sequence(),
        store.entry_count(),
        store.free_extents(),
        store.extent_size(),
        mount_us,
    );
    // §5.6's cost is stated for a card with no ride in progress; a mount that also read the 16 slot
    // headers and CRC'd a 32 KiB slot did more than that figure covers, so it is reported apart.
    if let Some(recovered) = store.recovered_ride() {
        defmt::info!(
            "flat: §7.3 recovered a ride — object {=u64} revision {=u64}, checkpoint {=u64}, {=u64} B flushed + {=u32} B tail",
            recovered.id.0,
            recovered.revision.0,
            recovered.checkpoint_sequence,
            recovered.flushed,
            recovered.tail_len,
        );
    }
    let mut maps = 0u16;
    let mut routes = 0u16;
    let mut rides = 0u16;
    let mut other = 0u16;
    for entry in store.entries() {
        match entry.kind {
            ObjectKind::MapShard | ObjectKind::MapSetManifest => maps += 1,
            ObjectKind::Route => routes += 1,
            ObjectKind::Ride => rides += 1,
            _ => other += 1,
        }
    }
    // Read once, after the walk: `entries_ok` reports whether the listing *just taken* crossed a
    // commit, so reading it before the loop would answer about the previous one.
    let listing_complete = store.entries_ok();
    defmt::info!(
        "flat: catalog holds {=u16} map object(s), {=u16} route(s), {=u16} ride(s), {=u16} other — listing complete: {=bool}",
        maps,
        routes,
        rides,
        other,
        listing_complete,
    );
    defmt::warn!("flat: c1 mounts and does not render — the renderer and the transports cut over in c2/c3");
    Catalog { maps: usize::from(maps), listing_complete }
}

/// The metadata of the first object of `kind`, or `None`. The one catalog helper the read path
/// needs: [`open_map`] resolves the map with it, and the interleave exercise picks a victim to read
/// while a commit runs.
pub(crate) fn first_of(store: &FlatStore<FlatCard>, kind: ObjectKind) -> Option<EntryMeta> {
    store.entries().find(|entry| entry.kind == kind)
}

// ══════════════════════════ the map, as bytes ══════════════════════════

/// The session-long source over the mounted card's map object.
///
/// `.bss`, and **session-long by construction**: `flat::source`'s two shapes are a scoped
/// `with_source` and a `StoreSource` released by hand, and a map that is read from boot to power-off
/// across a hundred `await`s cannot be a scope. So this is the second shape, and the hold row it
/// spends is never given back — which is correct rather than a leak: §6.2's row is what keeps the
/// revision the renderer is drawing from alive while an upload commits over it, and this image has
/// no state in which the map stops being needed.
static mut MAP_SOURCE: MaybeUninit<obc_storage::flat::StoreSource<'static, FlatCard>> = MaybeUninit::uninit();

/// The open map's §9 display name, truncated to what the System-settings row shows.
///
/// Captured in [`open_map`] because the alternative is a **second catalog walk** — at 1,027 entries
/// that is ~69 read commands and a tenth of a second, for a string. Same reasoning as
/// [`boot_fault_for`]'s: one walk, both consumers.
static mut MAP_NAME: heapless::String<24> = heapless::String::new();

/// The open map's display name, or `""` before [`open_map`] has run / on a card with no map.
pub(crate) fn map_name() -> &'static str {
    // SAFETY: written once by `open_map` before anything is spawned; read-only afterwards.
    unsafe { (*core::ptr::addr_of!(MAP_NAME)).as_str() }
}

/// **Open the card's map object and hand back a `'static` [`ByteSource`] over it** — the read
/// cutover's one new boot step (FS7.5-c2, #1420).
///
/// `None` when the catalog holds no map object at all, which is the flat twin of a FAT card with no
/// `.obcm` in the root: `flat_boot_fault` then says NO MAP, and it is the *only* input that makes it
/// say so. Everything else — an object that will not open, a map whose header will not parse — is
/// MAP UNREADABLE, decided further up exactly as it is on the FAT arm.
///
/// **The first map object wins, and there is deliberately no selection rule yet.** The FAT arm has
/// one (`MAP.SEL`, then the newest upload, then anything readable) because a FAT card accumulates
/// maps; nothing writes a second map to a flat store until c3's transports exist, so a rule here
/// would be a policy with no case to decide. c3 brings the objects and the rule together.
///
/// `#[inline(never)]` for the reason every constructor on this boot path is: a `StoreSource` built
/// by value inside the boot task's async block is a permanent slot in that task's poll frame
/// (#1084/#1108). It is small — the store reference, a hold row token and a length — but the rule is
/// about *where a value is built*, not how big it is, and the next thing to grow this type would do
/// it silently.
#[inline(never)]
pub(crate) fn open_map(store: &'static FlatStore<FlatCard>) -> Option<&'static dyn obc_formats::io::ByteSource> {
    let meta = first_of(store, ObjectKind::MapShard)?;
    match store.source(meta.id, None) {
        Ok(source) => {
            defmt::info!(
                "flat: map object {=u64} revision {=u64} open — {=u64} B, read direct (no channel)",
                meta.id.0,
                meta.revision.0,
                meta.payload_len,
            );
            // SAFETY: sole writer of MAP_NAME; same once-per-boot argument as MAP_SOURCE below.
            unsafe {
                let name = &mut *core::ptr::addr_of_mut!(MAP_NAME);
                name.clear();
                for ch in meta.name.as_str().unwrap_or("").chars() {
                    if name.push(ch).is_err() {
                        break;
                    }
                }
            }
            // SAFETY: sole writer of MAP_SOURCE; `open_map` runs once per boot on the one
            // thread-mode executor, before any task that could hold a reference exists. The write is
            // unconditional (no `StaticCell` one-shot flag a warm reset could find set), and the
            // `&'static` handed out is the only reference. `StoreSource`'s `Drop` is a
            // `debug_assert` that never runs here: the value is never dropped.
            Some(unsafe { crate::init_static(core::ptr::addr_of_mut!(MAP_SOURCE), source) })
        }
        Err(error) => {
            defmt::error!(
                "flat: the catalog names map object {=u64} and it will not open ({}) — MAP UNREADABLE",
                meta.id.0,
                defmt::Debug2Format(&error)
            );
            None
        }
    }
}

// ══════════════════════ the on-glass interleave exercise ══════════════════════

/// The write-path exercise and the interleaving measurement (`--features flat-exercise`).
///
/// **Never in a shipping image.** It writes to the card in the slot and it is a measurement, so it
/// sits behind a feature that is off by default and absent from both profiles
/// `resource_guard.py board` gates. It is also not a second bench: `bin/flat_store_bench.rs` already
/// measures the store's own costs far better than this can, and deliberately measures them with
/// *nothing else running*. This exercises the one thing a bench structurally cannot — the store
/// under a **real scheduler**, with a reader task and the storage task competing for one executor.
///
/// ## What it asks, and why it is asked this way
///
/// The ruling says a commit's ~36 card commands should let a render read interleave into their gaps,
/// so that the worst render stall is one command (10–20 ms) rather than one commit (~250 ms at 1,024
/// entries). That claim has two halves and they can disagree:
///
/// - **The store's half** — the state borrow is per card command, so a reader *may* be served
///   between any two of them. Proven off-board and pinned by `flat::granularity`; nothing on glass
///   is needed for it and nothing on glass could improve on it.
/// - **The scheduler's half** — the reader has to actually *run* between two of those commands.
///   Only glass answers that, because it is a fact about this executor and this task set.
///
/// So the measurement is deliberately the crudest honest one: a reader on a fixed cadence, and the
/// figure is the **longest gap between two of its completed reads** while one commit runs. A gap
/// near one card command means the halves agree; a gap near the whole commit means they do not, and
/// the difference is the executor's, not the store's.
///
/// The counters live in statics rather than in the reader's own frame **on purpose**: `select` drops
/// the losing branch, and the losing branch here is always the reader — a figure that only survived
/// when the measurement failed would be no figure at all.
#[cfg(feature = "flat-exercise")]
mod exercise {
    use core::sync::atomic::{AtomicU32, Ordering};

    // `u32` microseconds throughout: this part has no 64-bit atomics, and the widest figure the
    // exercise can produce is one commit — ~250 ms at 1,024 entries, four orders of magnitude below
    // a `u32`'s 71 minutes. A run that overflowed one would have failed long before reporting.

    /// Longest gap between two completed probe reads, microseconds. **The figure.**
    pub(super) static WORST_GAP_US: AtomicU32 = AtomicU32::new(0);
    /// Longest single probe read, microseconds — the control: a gap is only interesting if it is
    /// much larger than a read.
    pub(super) static WORST_READ_US: AtomicU32 = AtomicU32::new(0);
    /// Probe reads completed while the commit ran.
    pub(super) static READS: AtomicU32 = AtomicU32::new(0);

    pub(super) fn note(gap_us: u64, read_us: u64) {
        WORST_GAP_US.fetch_max(gap_us.try_into().unwrap_or(u32::MAX), Ordering::Relaxed);
        WORST_READ_US.fetch_max(read_us.try_into().unwrap_or(u32::MAX), Ordering::Relaxed);
        READS.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn taken() -> (u32, u32, u32) {
        (READS.load(Ordering::Relaxed), WORST_GAP_US.load(Ordering::Relaxed), WORST_READ_US.load(Ordering::Relaxed))
    }
}

#[cfg(feature = "flat-exercise")]
#[embassy_executor::task]
pub(crate) async fn interleave_exercise(store: &'static FlatStore<FlatCard>, writer: Writer) {
    use embassy_futures::select::{select, Either};
    use embassy_time::{Instant, Timer};
    use obc_formats::io::ByteSource as _;
    use obc_storage::flat::{DisplayName, EntryFlags, PutSource, Revision};

    /// The reader's cadence. Fine enough that a stall is measured rather than inferred, coarse
    /// enough that the reads are not themselves what keeps the card busy.
    const READ_PERIOD_MS: u64 = 2;
    /// One probe read: a single block, the smallest thing the card can be asked for — so the figure
    /// is a scheduling measurement and not a transfer-size one.
    const PROBE_LEN: usize = 512;
    /// The exercise's payload. `'static` because a write request outlives the statement that sent it
    /// (see [`Request`]), which is the same reason c3's transports will hand over arena slices.
    static PAYLOAD: [u8; 512] = [0xC1; 512];
    /// This call site's reply slot. One per site, so two callers can never collect each other's.
    static REPLY: Reply = Signal::new();

    // Read whatever the card already holds. A store with no objects still exercises the commit path;
    // it just cannot exercise a reader beside it, and the report says so rather than inventing one.
    let subject = first_of(store, ObjectKind::MapShard)
        .or_else(|| first_of(store, ObjectKind::Route))
        .or_else(|| store.entries().next());
    let Some(subject) = subject else {
        defmt::warn!("flat/exercise: the catalog is empty — no object to read, so no interleaving figure");
        return;
    };
    let probe = PROBE_LEN.min(subject.payload_len as usize).max(1);
    defmt::info!(
        "flat/exercise: probing object {=u64} with {=usize} B reads every {=u64} ms while one commit runs at {=u16} entries",
        subject.id.0,
        probe,
        READ_PERIOD_MS,
        store.entry_count(),
    );

    // The reader. The figure is the gap between two *completed* reads, not a read's own duration: a
    // stall shows up as one long gap whether the reader was blocked before its call or inside it,
    // which is exactly the property under test.
    let reader = async {
        let mut buf = [0u8; PROBE_LEN];
        let mut last = Instant::now();
        loop {
            let started = Instant::now();
            let outcome = store.with_source(subject.id, None, |source| source.read_at(0, &mut buf[..probe]));
            let now = Instant::now();
            // Both halves, and the distinction is the useful part: the outer `Err` is the open being
            // refused (no hold row, or the object gone), the inner one is the read itself. A gap
            // measured across a read that never happened would be a measurement of nothing.
            match outcome {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    defmt::warn!(
                        "flat/exercise: the probe read failed ({}) — the reader stops here",
                        defmt::Debug2Format(&error)
                    );
                    return;
                }
                Err(error) => {
                    defmt::warn!(
                        "flat/exercise: the probe object would not open ({}) — the reader stops here",
                        defmt::Debug2Format(&error)
                    );
                    return;
                }
            }
            exercise::note((now - last).as_micros(), (now - started).as_micros());
            last = now;
            Timer::after_millis(READ_PERIOD_MS).await;
        }
    };

    // The writer: one publication through the task — allocate, write, commit — with only the commit
    // on the clock, because that is the step §5.5 puts a figure on and the one the reader races.
    let commit = async {
        let allocation = match writer.call(Request::Allocate { bytes: PAYLOAD.len() as u64 }, &REPLY).await {
            Ok(Outcome::Allocated(allocation)) => allocation,
            Err(error) => {
                defmt::error!("flat/exercise: allocate refused ({})", defmt::Debug2Format(&error));
                return None;
            }
            Ok(_) => return None,
        };
        let allocation = match writer.call(Request::Write { allocation, bytes: &PAYLOAD }, &REPLY).await {
            Ok(Outcome::Wrote(allocation)) => allocation,
            other => {
                defmt::error!("flat/exercise: write refused ({})", defmt::Debug2Format(&other.err()));
                let _ = writer.call(Request::Cancel { allocation }, &REPLY).await;
                return None;
            }
        };
        let meta = EntryMeta {
            id: store.next_object_id(),
            revision: Revision(1),
            kind: ObjectKind::Route,
            flags: EntryFlags::NONE,
            payload_len: PAYLOAD.len() as u64,
            payload_crc: obc_crc::crc32(&PAYLOAD),
            name: DisplayName::new("c1-exercise").unwrap_or_default(),
        };
        let mut batch: heapless::Vec<Mutation, MAX_BATCH> = heapless::Vec::new();
        let _ = batch.push(Mutation::Put { meta, source: PutSource::Fresh(allocation) });
        let started = Instant::now();
        let outcome = writer.call(Request::Commit { batch }, &REPLY).await;
        let commit_us = started.elapsed().as_micros();
        match outcome {
            Ok(Outcome::Committed(sequence)) => Some((sequence, commit_us)),
            other => {
                defmt::error!("flat/exercise: commit refused ({})", defmt::Debug2Format(&other.err()));
                None
            }
        }
    };

    // The reader loops forever, so `select` resolves when the commit does — and the counters are in
    // statics, so the reader's figure survives being dropped.
    let entries = store.entry_count();
    let landed = match select(reader, commit).await {
        Either::First(()) => {
            defmt::error!("flat/exercise: the reader stopped before the commit did — the figure below is not a stall");
            None
        }
        Either::Second(landed) => landed,
    };
    let (reads, worst_gap_us, worst_read_us) = exercise::taken();
    match landed {
        Some((sequence, commit_us)) => defmt::info!(
            "flat/exercise: commit {=u64} at {=u16} entries took {=u64} us; the reader completed {=u32} probe(s) across it",
            sequence,
            entries,
            commit_us,
            reads,
        ),
        None => defmt::error!("flat/exercise: the commit did not land — the figures below are of an incomplete run"),
    }
    defmt::info!(
        "flat/exercise: INTERLEAVING — worst gap between two completed reads {=u32} us, worst single read {=u32} us",
        worst_gap_us,
        worst_read_us,
    );
    defmt::info!(
        "flat/exercise: read that against §5.5's ~1.5 ms write command and ~340 us read command. A gap near one command means the store's per-command granularity reached the scheduler; a gap near the whole commit means it did not, and the remaining stall is this executor's — `Store::commit` is synchronous and hands nothing back between its commands"
    );
}
