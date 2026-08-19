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
//! ## What FS7.5 finished here
//!
//! c1 mounted and stopped; c2 pointed the renderer at [`open_map`]; c3a and c3b put both links'
//! protocol-v4 engine inside [`storage_task`], which is why the engine is a field of the one task
//! that writes rather than a value a transport owns. What is still owed is FS8's ride journal
//! (#1390) — nothing records to a flat card yet — and FS11's (#1393) retirement of the FAT read
//! path.

use core::mem::MaybeUninit;

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;

use obc_link::flat::{Ceilings, Engine, Link, Policy, Reaction, RequestId};
use obc_storage::flat::store::MAX_BATCH;
use obc_storage::flat::{
    Allocation, BlockDevice, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectId, ObjectKind,
    RideCheckpoint, Store as _, StoreError,
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
static FLAT_STORE_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// What this layer costs the resident budget: the store and the write queue. The alignment bounce
/// is `sd`'s and is already counted there — see the note above [`FLAT_BOUNCE_WARNED`].
///
/// The recording caller's 32,256-byte ride tail (§7.1) is **not** here — no ride records to the flat
/// store until FS8 (#1390), and a budget row for a buffer nothing allocates would be a lie in the other
/// direction. It joins this sum in the slice that starts recording.
pub(crate) const RESIDENT_BYTES: usize = core::mem::size_of::<FlatStore<FlatCard>>()
    + REQUEST_QUEUE_BYTES
    + MAP_READ_BYTES
    + ROUTE_READ_BYTES
    + ENGINE_BYTES;

/// **Everything the read cutover keeps resident on this arm** (FS7.5-c2): the session-long
/// [`MAP_SOURCE`] *and* the [`MAP_NAME`] the same boot step captures.
///
/// Both, because this budget's discipline is that every resident byte is named — an itemization
/// that quietly omits 28 B is worse than one that admits it, since the next reader has no way to
/// know which of the two it is. (The first version of this constant counted only the source and
/// still called itself the whole cost; the review caught it.)
pub(crate) const MAP_READ_BYTES: usize = core::mem::size_of::<obc_storage::flat::StoreSource<'static, FlatCard>>()
    + core::mem::size_of::<heapless::String<24>>();

/// The active route's one held revision. It is released on selection or revision changes.
pub(crate) const ROUTE_READ_BYTES: usize =
    core::mem::size_of::<Option<obc_storage::flat::StoreSource<'static, FlatCard>>>();

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
    FLAT_STORE_READY.store(true, core::sync::atomic::Ordering::Release);
    &*store
}

/// The mounted store after boot initialization. Read-only callers use this instead of inventing a
/// second global reference; all mutation still goes through [`storage_task`].
pub(crate) fn mounted() -> Option<&'static FlatStore<FlatCard>> {
    FLAT_STORE_READY.load(core::sync::atomic::Ordering::Acquire).then(|| unsafe {
        // SAFETY: `Release` is stored only after `mount_in_place` fully initialized this slot; it is
        // never overwritten during the boot session.
        &*core::ptr::addr_of!(FLAT_STORE).cast::<FlatStore<FlatCard>>()
    })
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

/// **How many tasks hold a [`Writer`]** — the BLE v4 adapter and the USB v4 adapter.
///
/// A census, not a guess, and [`REQUEST_QUEUE`] is derived from it. FS8's ride journal is the next
/// entry.
const SENDERS: usize = 2;

/// Write requests queued at once.
///
/// **Two per sender, and that number is load-bearing rather than generous.** Each link has at most
/// two jobs on this queue at one instant: the one its lane is awaiting, and at most one *orphan* — a
/// job whose caller's future was dropped between the send and the answer, which a link teardown
/// during a long finalizing commit genuinely does. Nothing produces a third, because a lane holds
/// one buffer and cannot issue a second call without it.
///
/// Sizing to that census is what keeps the queue from ever filling, and a queue that cannot fill is
/// the difference between a recoverable orphan and a lost one: `Sender::send` on a full queue
/// *parks*, and a `Writer::call` future dropped while parked never enqueues its job at all — taking
/// the `&'static mut` reaction buffer inside it with it, permanently, since nothing may re-derive
/// one. [`Lane::reclaim`] rests on this; see there.
///
/// c3a's two slots were sized for one sender and said so. A `Job` is not small (see [`Request`]), so
/// this is `.bss` and the growth is priced in the resource baseline rather than waved through.
const REQUEST_QUEUE: usize = 2 * SENDERS;

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
/// **`Journal` and `Close` still have no caller, and are here anyway.** The ride journal is FS8's
/// (#1390) and nothing this image opens outlives its `with_source` scope. They stay because the
/// seam is six operations and a task that served four of them would not be the write half; with no
/// caller the linker keeps neither, so the cost is zero bytes.
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
    Control { link: Link, record: &'static [u8], out: &'static mut [u8] },
    /// One whole stream record (§3.8): the 16-byte frame followed by exactly its payload.
    Stream { link: Link, record: &'static [u8], out: &'static mut [u8] },
    /// Pump the engine once — a live `GET`'s next record, or an error owed to a dropped transfer.
    /// An adapter repeats this until the reaction is [`Reaction::Idle`]; a driver that stops pumping
    /// stalls a download.
    Pump { link: Link, out: &'static mut [u8] },
    /// **This** link came up with these record ceilings (§5.1, §5.2).
    ///
    /// It re-pins `link`'s ceilings and releases `link`'s transfer if it had one — and touches
    /// nothing belonging to the other link, which is the point. See the arm in [`serve`] for the
    /// bug that shape exists to prevent.
    ///
    /// It carries a **validated** [`Ceilings`], not two numbers, so §5.1's floor refusal never
    /// reaches this queue: [`Ceilings::for_ble`] is where a link is judged, the adapter closes the
    /// channel on `None`, and nothing here has to answer a transport verdict with a `StoreError`.
    LinkUp { link: Link, ceilings: Ceilings },
    /// §3.8's third form of cancel: **this** link went away. Answers nobody, because there is nobody
    /// left to answer — and releases only what that link held, so an unplugged cable is not a reason
    /// to kill a phone's download.
    LinkLost { link: Link },
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
/// Every variant has a caller since c3a except the two [`Request`] names above, and the linker keeps
/// only what is reached.
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

/// A menu-originated removal. Kept outside the protocol engine so the device UI does not have to
/// impersonate a link, but consumed by the same one writer task.
#[derive(Clone, Copy)]
enum MenuDelete {
    Route(ObjectId),
    TripCascade(ObjectId),
}

static MENU_DELETES: Channel<CriticalSectionRawMutex, MenuDelete, 8> = Channel::new();

pub(crate) fn request_route_delete(id: u64) -> bool {
    MENU_DELETES.try_send(MenuDelete::Route(ObjectId(id))).is_ok()
}

pub(crate) fn request_trip_cascade(id: u64) -> bool {
    MENU_DELETES.try_send(MenuDelete::TripCascade(ObjectId(id))).is_ok()
}

/// **The write half's front door**, handed to every plane that mutates the store.
///
/// `Copy`, so it costs a caller nothing to hold, and it carries no store reference at all — which is
/// what makes "there is exactly one execution context that writes" a property of the type rather
/// than a convention.
///
#[derive(Clone, Copy)]
pub(crate) struct Writer {
    requests: Sender<'static, CriticalSectionRawMutex, Job, REQUEST_QUEUE>,
}

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

// ══════════════════════════ the lane ══════════════════════════

/// **One link's half of a round trip to the engine**: the buffer it lends, the slot the answer comes
/// back in, and nothing else.
///
/// One lane per link, because two links are live at once — a phone in a pocket and a cable in J3 —
/// and every part of a round trip is per-caller: the reaction buffer (§5's ceilings differ by two
/// orders of magnitude between them), the reply slot ([`Writer::call`]'s one-slot-per-concurrently-
/// live-call contract), and the recovery below.
///
/// The buffer is *lent* rather than copied — it crosses the queue inside the request and comes back
/// inside the answer, which is what stops a `LIST` page being memcpy'd twice per record — so a
/// `None` here means a previous call's future was dropped between the send and the answer.
/// [`Lane::reclaim`] is how that is recovered.
///
/// Shared between the two adapters rather than written twice, so that the argument at `reclaim` has
/// one home and cannot drift into two versions that disagree.
pub(crate) struct Lane {
    out: Option<&'static mut [u8]>,
    reply: &'static Reply,
    /// Which link this is, for the log lines. The adapters are otherwise indistinguishable here.
    who: &'static str,
}

/// How long [`Lane::reclaim`] waits for an orphaned answer before giving the link up.
///
/// It is waiting on the storage task to finish jobs already in the queue, so the bound is a
/// *scheduling* one — and the longest single thing that task does is a commit, ~250 ms at 1,024
/// entries (`storage_task`'s own note). Two seconds is that with room; a link that has not been
/// answered by then is not going to be, and refusing it is better than parking a transport forever.
const RECLAIM_TIMEOUT: embassy_time::Duration = embassy_time::Duration::from_secs(2);

impl Lane {
    /// Build a lane over a caller-owned buffer and reply slot. Called once per link, per image.
    pub(crate) fn new(out: &'static mut [u8], reply: &'static Reply, who: &'static str) -> Self {
        Lane { out: Some(out), reply, who }
    }

    /// **Recover the buffer from a call whose future was dropped.**
    ///
    /// c3a inferred this from the queue's service order: requests are served FIFO by one consumer,
    /// so once *any* later call had been answered, every earlier job had run and an orphan could
    /// only be sitting in the reply slot. That was an argument about the **queue**, and it named
    /// this slice as owing its re-establishment, because a second sender can interleave a job
    /// between the orphan and the reclaiming call.
    ///
    /// It is re-established by not needing it. Three facts, each local to one lane:
    ///
    /// 1. **A reply slot has exactly one caller.** Each is a `static` private to its adapter and is
    ///    named by that adapter alone, so whatever arrives in this slot is *this* lane's orphan and
    ///    no other link's. The buffers are disjoint statics too, so a mis-reclaim could not even
    ///    type-check into the wrong lane. The other link's activity is invisible here, which is the
    ///    whole property the queue argument could not supply once the queue had two senders.
    /// 2. **The orphan is always in the queue.** [`REQUEST_QUEUE`] is sized to the sender census, so
    ///    `Sender::send` never parks, so a dropped `Writer::call` future is always dropped *after*
    ///    its job was enqueued. A job in the queue is a job that will be served and answered.
    /// 3. **So this is an observation, not an inference.** It waits on its own slot rather than
    ///    reasoning about when someone else's call proves the orphan ran. (1) says what arrives is
    ///    ours; (2) says something arrives.
    ///
    /// The wait is bounded anyway: (2) is an argument about a constant two files can change
    /// independently, and a transport that parked forever on it would be a watchdog reset rather
    /// than a log line.
    pub(crate) async fn reclaim(&mut self) {
        if self.out.is_some() {
            return;
        }
        match embassy_time::with_timeout(RECLAIM_TIMEOUT, self.reply.wait()).await {
            Ok((_, Ok(Outcome::Reacted { out, .. }))) => {
                defmt::info!("flat/v4: [{}] reclaimed the reaction buffer from an abandoned call", self.who);
                self.out = Some(out);
            }
            Ok(_) => defmt::warn!("flat/v4: [{}] an abandoned call left no buffer to reclaim", self.who),
            Err(_) => {
                defmt::warn!("flat/v4: [{}] no orphaned answer arrived — this link cannot serve", self.who)
            }
        }
    }

    /// Hand one request to the engine and take the buffer back with its answer.
    pub(crate) async fn call(
        &mut self,
        writer: &Writer,
        make: impl FnOnce(&'static mut [u8]) -> Request,
    ) -> Option<Reaction> {
        let out = self.out.take()?;
        match writer.call(make(out), self.reply).await {
            Ok(Outcome::Reacted { reaction, out }) => {
                self.out = Some(out);
                Some(reaction)
            }
            // `serve` answers these three requests with `Reacted` and nothing else, so the buffer is
            // gone only if that stopped being true. Report rather than panic: these are link tasks.
            _ => {
                defmt::warn!("flat/v4: [{}] the engine answered a record with the wrong shape — lane closed", self.who);
                None
            }
        }
    }

    /// The bytes a [`Reaction::Send`] named.
    pub(crate) fn sent(&self, len: usize) -> &[u8] {
        match &self.out {
            Some(out) => &out[..len.min(out.len())],
            None => &[],
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
pub(crate) fn writer() -> Option<Writer> {
    ARMED.load(core::sync::atomic::Ordering::Relaxed).then(|| Writer { requests: REQUESTS.sender() })
}

static CATALOG_COMMITS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static CATALOG_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn note_catalog_commit() {
    CATALOG_COMMITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    CATALOG_WAKE.signal(());
}

pub(crate) fn take_catalog_commits() -> u32 {
    CATALOG_COMMITS.swap(0, core::sync::atomic::Ordering::Relaxed)
}

pub(crate) async fn wait_catalog_commit() {
    CATALOG_WAKE.wait().await
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
/// smuggled in as a lock here. **That follow-up is closed, not built** (#1420, item I2): the board
/// session found no felt stall on glass, and the speculative-capability rule says a mechanism bought
/// against a cost nobody has measured is a mechanism that should not exist. The harness that would
/// have measured it — an `interleave_exercise` task behind a `flat-exercise` feature — is deleted
/// with the question, per the owner's rule that scaffolding is stripped when its purpose is served
/// rather than kept as a permanent fixture. If a stall ever shows up on a real ride, both come back
/// out of git history with the reasoning intact.
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
        match select(requests.receive(), MENU_DELETES.receive()).await {
            Either::First(job) => {
                let before = store.sequence();
                // The FLPR is switched per card command by `flpr_mux::with_storage`, so nothing is held
                // across this call and there is no mode session to acquire around the batch.
                let outcome = serve(store, engine, &mut policy, job.request);
                if store.sequence() != before {
                    note_catalog_commit();
                }
                // The tag rides back with the answer: the caller may be gone, and the next user of this slot
                // has to be able to tell that this value is not theirs. See `Reply`.
                job.reply.signal((job.tag, outcome));
            }
            Either::Second(delete) => serve_menu_delete(store, delete),
        }
    }
}

#[inline(never)]
fn serve_menu_delete(store: &FlatStore<FlatCard>, delete: MenuDelete) {
    let before = store.sequence();
    match delete {
        MenuDelete::Route(id) => remove_head(store, id, ObjectKind::Route),
        MenuDelete::TripCascade(id) => {
            let stages = store
                .with_source(id, None, |source| obc_route::TripMeta::read(source).ok().map(|m| m.stage_ids))
                .ok()
                .flatten();
            if let Some(stages) = stages {
                for stage in stages {
                    remove_head(store, ObjectId(stage), ObjectKind::Route);
                }
            }
            remove_head(store, id, ObjectKind::Trip);
        }
    }
    if store.sequence() != before {
        note_catalog_commit();
    }
}

fn remove_head(store: &FlatStore<FlatCard>, id: ObjectId, kind: ObjectKind) {
    let Some(meta) = store.entries().find(|entry| entry.id == id && entry.kind == kind) else {
        return;
    };
    if let Err(error) = store.commit(&[Mutation::Remove { id, revision: meta.revision }]) {
        defmt::warn!(
            "flat: menu removal of {} object {=u64} revision {=u64} failed: {}",
            defmt::Debug2Format(&kind),
            id.0,
            meta.revision.0,
            defmt::Debug2Format(&error)
        );
    }
}

/// One request against the store. **Synchronous and out of line**: it is the whole write surface, so
/// its frame is measured as its own symbol rather than folded into the task's poll frame — the same
/// reason `mount_at_boot` is `#[inline(never)]`.
///
/// # ⚠️ Synchronous is a contract here, not an implementation detail
///
/// The v4 adapters hand this function `&'static` borrows of buffers **they** own — a staged control
/// record, a received stream record, the reaction buffer — and they are free to reuse those buffers
/// the instant the answer to their call arrives. That is sound for exactly one reason: `serve` never
/// yields, so "the answer arrived" and "the engine is done with the bytes" are the same instant.
///
/// **The stepped-commit follow-up recorded on #1420 would break that.** A resumable commit, an async
/// block device, or a per-command yield seam inside `Store::commit` all turn this into a function
/// that can be suspended with an adapter's buffer borrowed — at which point the adapter may stage
/// the next record over bytes the engine has not finished reading. Whoever lands that seam owes this
/// module a different ownership story (a copy at the boundary, or a per-record token the adapter
/// waits on), and this paragraph is the note that says so at the site rather than in an issue.
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
        Request::Control { link, record, out } => {
            let reaction = engine.on_control(link, store, policy, record, out);
            publish_upload(engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::Stream { link, record, out } => {
            let reaction = engine.on_stream(link, store, policy, record, out);
            publish_upload(engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::Pump { link, out } => {
            let reaction = engine.poll(link, store, out);
            publish_upload(engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::LinkUp { link, ceilings } => {
            // **Scoped to `link`, and that is the whole of FS7.5-c3b's P1 fix.** This used to
            // release the live transfer and rebuild the engine outright, which was right while one
            // link existed: there was nothing else for it to disturb. With two — a phone in a pocket
            // and a cable in J3, both spawned side by side in `main` — it meant a reconnecting
            // radio destroyed a cable's twenty-minute map upload with no answer to the client
            // sending it, *and* re-pinned the shared stream ceiling to the radio's 245 bytes so the
            // cable's next 4,112-byte record died as over-ceiling. The reverse direction broke the
            // radio's framing the same way.
            //
            // `Engine::on_link_up` now touches only this link's ceilings and only this link's
            // transfer, and every transfer carries the ceilings it was admitted under. The
            // newcomer's own `PUT` then meets §1's one-at-a-time rule the ordinary way — `busy`,
            // with the live `RequestId` as context, whichever wire asked, which is exactly what the
            // spec commit's §10 sentence promises.
            engine.on_link_up(link, store, ceilings);
            defmt::info!(
                "flat/v4: link up ({}) — control {=usize} B, stream {=usize} B",
                match link {
                    Link::Ble => "ble",
                    Link::Usb => "usb",
                },
                ceilings.control(),
                ceilings.stream()
            );
            Ok(Outcome::Done)
        }
        Request::LinkLost { link } => {
            engine.on_link_lost(link, store);
            publish_upload(engine);
            Ok(Outcome::Done)
        }
        Request::LiveTransfer => Ok(Outcome::Live(engine.live_transfer())),
    }
}

/// **Push what the engine knows about a live upload to the glass** — issue #927's progress card,
/// re-sourced.
///
/// It runs here, beside the engine, rather than in an adapter, and both halves of that are
/// deliberate. Beside the engine, because this is the one execution context that holds one, so the
/// read is a field access rather than a round trip on the very queue a multi-megabyte upload is
/// saturating. Not in an adapter, because §5 says an adapter "never parses a payload" and the kind
/// and the declared length are payload — the engine is the layer entitled to know them.
///
/// A rider sees a card for a **map** and nothing else, which is not a filter but the truth: a route
/// lands in a second and a weather bundle is invisible by design, so a progress bar for either would
/// be a flicker asking to be dismissed. `crate::link` owns the mapping from these facts to a screen.
fn publish_upload(engine: &mut BoardEngine) {
    crate::link::publish_map_transfer(engine.live_upload(), engine.take_upload_end());
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
/// — but the rule is about *where a value is built*, not how big it is.
///
/// **It comes up with no link up and therefore no ceilings**, which is the honest starting state of
/// a device nobody has connected to: each adapter announces itself with [`Request::LinkUp`] and is
/// served only while it has. The engine used to be built with the radio's preferred numbers, which
/// was a guess that happened to be right for one link and wrong for the other the moment USB
/// arrived.
///
/// # Safety
/// Sole writer of [`ENGINE`]; called exactly once, from [`storage_task`], which is spawned once.
#[inline(never)]
fn engine_slot() -> &'static mut BoardEngine {
    // SAFETY: sole writer; `storage_task` is spawned exactly once and nothing else names this slot.
    unsafe { crate::init_static(core::ptr::addr_of_mut!(ENGINE), Engine::new()) }
}

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

/// What the mount found, on RTT. The same catalog drives the boot fault and the flat map reader, so
/// the log, glass and served object all describe one mount result.
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
    Catalog { maps: usize::from(maps), listing_complete }
}

/// The metadata of the first object of `kind`, or `None`. The one catalog helper the read path
/// needs: [`open_map`] resolves the map with it.
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
/// **The active map is the lowest-`ObjectId` `MapShard`.** Catalog iteration is ordered by
/// `(ObjectId, Revision)`, and `first_of` resolves that object's head, so selection is deterministic
/// even on a card that already contains several maps. Companion map sends follow the same rule:
/// replace this object using its listed revision, and create only when no map exists.
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

// ══════════════════════════ route + trip menus ══════════════════════════

/// The active route's held revision. One route is streamed by the matcher/renderer at a time; this
/// single slot replaces FAT's open file handle and spends one of the store's bounded hold rows.
static mut ROUTE_SOURCE: Option<obc_storage::flat::StoreSource<'static, FlatCard>> = None;

/// Reconcile the held route revision to the app's selected flat `ObjectId` and return its source.
/// A replace at the same id reopens because the catalog revision is part of the key.
#[inline(never)]
pub(crate) fn reconcile_route(
    store: &'static FlatStore<FlatCard>,
    wanted: Option<u64>,
) -> Option<&'static dyn obc_formats::io::ByteSource> {
    let wanted = wanted.and_then(|id| {
        store
            .entries()
            .find(|entry| entry.id == ObjectId(id) && entry.kind == ObjectKind::Route)
            .map(|entry| (entry.id, entry.revision))
    });
    let slot = core::ptr::addr_of_mut!(ROUTE_SOURCE);
    // SAFETY: the ride loop is the only caller and executes synchronously on thread mode. The
    // returned shared source is consumed only until the next loop pass; reconciliation never runs
    // while a reader from the previous pass is live.
    unsafe {
        let current = (*slot).as_ref().map(|source| (source.id(), source.revision()));
        if current != wanted {
            let old = core::ptr::read(slot);
            core::ptr::write(slot, None);
            if let Some(source) = old {
                store.close(source.release());
            }
            if let Some((id, revision)) = wanted {
                match store.source(id, Some(revision)) {
                    Ok(source) => core::ptr::write(slot, Some(source)),
                    Err(error) => defmt::warn!(
                        "flat: route object {=u64} revision {=u64} would not open: {}",
                        id.0,
                        revision.0,
                        defmt::Debug2Format(&error)
                    ),
                }
            }
        }
        (*slot).as_ref().map(|source| source as &dyn obc_formats::io::ByteSource)
    }
}

fn retain_newest<const N: usize>(entries: &mut heapless::Vec<EntryMeta, N>, entry: EntryMeta) {
    let at = entries.iter().position(|old| entry.id > old.id).unwrap_or(entries.len());
    if at >= N {
        return;
    }
    if entries.is_full() {
        let _ = entries.pop();
    }
    let _ = entries.insert(at, entry);
}

/// Rebuild the Route menu from one bounded catalog snapshot. The newest `MAX_ROUTES` ids win, so a
/// fresh upload remains visible even on a benchmark card carrying hundreds of old ladder objects.
#[inline(never)]
pub(crate) fn load_routes(store: &'static FlatStore<FlatCard>, app: &mut obc_app::App) -> bool {
    let mut heads: heapless::Vec<EntryMeta, { obc_app::MAX_ROUTES }> = heapless::Vec::new();
    for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Route) {
        retain_newest(&mut heads, entry);
    }
    if !store.entries_ok() {
        defmt::warn!("flat: route catalog listing failed — keeping the prior menu snapshot");
        return false;
    }

    let mut routes: heapless::Vec<obc_route::RouteSummary, { obc_app::MAX_ROUTES }> = heapless::Vec::new();
    let mut ids: heapless::Vec<u64, { obc_app::MAX_ROUTES }> = heapless::Vec::new();
    for entry in heads {
        let decoded = store
            .with_source(entry.id, Some(entry.revision), |source| obc_route::RouteSummary::read(source))
            .ok()
            .and_then(Result::ok);
        match decoded {
            Some(summary) => {
                let _ = routes.push(summary);
                let _ = ids.push(entry.id.0);
            }
            None => defmt::warn!(
                "flat: route object {=u64} revision {=u64} is malformed or unreadable — omitted from menu",
                entry.id.0,
                entry.revision.0
            ),
        }
    }
    app.set_routes_with_ids(&routes, &ids);
    defmt::info!("flat: Route menu loaded {=usize} route(s)", routes.len());
    true
}

/// Decode the newest bounded trip objects and resolve their full-width stage `ObjectId`s against
/// the route snapshot already fed to the app.
#[inline(never)]
pub(crate) fn load_trips(store: &'static FlatStore<FlatCard>, app: &mut obc_app::App) -> bool {
    let mut heads: heapless::Vec<EntryMeta, { obc_app::MAX_TRIPS }> = heapless::Vec::new();
    for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Trip) {
        retain_newest(&mut heads, entry);
    }
    if !store.entries_ok() {
        defmt::warn!("flat: trip catalog listing failed — keeping the prior menu snapshot");
        return false;
    }

    let mut metas: heapless::Vec<obc_route::TripMeta, { obc_app::MAX_TRIPS }> = heapless::Vec::new();
    let mut ids: heapless::Vec<u64, { obc_app::MAX_TRIPS }> = heapless::Vec::new();
    for entry in heads {
        let decoded = store
            .with_source(entry.id, Some(entry.revision), |source| obc_route::TripMeta::read(source))
            .ok()
            .and_then(Result::ok);
        match decoded {
            Some(meta) => {
                let _ = metas.push(meta);
                let _ = ids.push(entry.id.0);
            }
            None => defmt::warn!(
                "flat: trip object {=u64} revision {=u64} is malformed or unreadable — omitted from menu",
                entry.id.0,
                entry.revision.0
            ),
        }
    }
    let mut inputs: heapless::Vec<obc_app::TripInput<'_>, { obc_app::MAX_TRIPS }> = heapless::Vec::new();
    for (id, meta) in ids.iter().copied().zip(metas.iter()) {
        let _ = inputs.push(obc_app::TripInput { id, name: meta.name.as_str(), stage_ids: &meta.stage_ids });
    }
    app.set_trips(&inputs);
    defmt::info!("flat: Route menu loaded {=usize} trip folder(s)", inputs.len());
    true
}
