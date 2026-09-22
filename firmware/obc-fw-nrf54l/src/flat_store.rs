//! The flat store on the board: the card binding, the boot mount, and the one storage task.
//!
//! Reads go direct, through `&'static FlatStore` and `StoreSource`. Writes serialize through
//! [`storage_task`]: callers send async messages and block only while they await confirmation.
//!
//! A card is a flat store or a filesystem, never both. The flat store owns the raw card from LBA 0,
//! so there is no layout in which the two could sit side by side. `FlatStore::mount` is the probe —
//! it never fails, it classifies — so the board runs no second superblock reader that could
//! disagree with it. The two classifiers are disjoint by construction: a flat card's block 0 is
//! deliberately not an MBR, so the FAT reader refuses it at the first block, and a FAT card carries
//! neither superblock magic nor CRC, so `mount` returns [`Mode::Unformatted`].

use core::{cell::RefCell, mem::MaybeUninit};

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::Deque;

use obc_link::flat::{
    Ceilings, Engine, Link, Policy, Reaction, RequestId, Stall, StreamBuffers, UsbMapBatch, STALL_TIMEOUT_MS,
};
use obc_storage::flat::store::MAX_BATCH;
use obc_storage::flat::{
    Allocation, BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectId,
    ObjectKind, PutSource, Revision, RideCheckpoint, Store as _, StoreError,
};

use crate::semmc::{SemmcError, BLOCK_BYTES};

// The flat binding places no alignment buffer of its own, and that is a stack decision. The sEMMC
// firmware's DMA wants 32-bit alignment, and the store hands the device frame locals and streaming
// windows that carry no alignment attribute, so whether a call is aligned is a codegen accident and
// the binding must be correct either way. It borrows the raw-card layer's 4-block buffer
// (`card_io::with_bounce`) instead of placing one, because on this part every `.bss` byte comes out
// of the deep-ride path's stack headroom. What the shared buffer costs when it fires is one extra
// card command per commit-body window.

/// The card, as `obc_storage::flat` wants it.
///
/// Zero-sized: every method is one [`flpr_mux::with_storage`](crate::flpr_mux::with_storage) call,
/// so the FLPR mode and the driver borrow are taken together and this type owns nothing a second
/// instance could duplicate. `BlockDevice` takes `&self` throughout, which is what makes
/// per-card-command borrow granularity implementable: the store reaches the card holding none of
/// its own cells.
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
        crate::flpr_mux::with_storage(|sd| sd.num_blocks()).map(u64::from)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), SemmcError> {
        let start = FlatCard::lba(lba)?;
        let addr = buf.as_ptr() as usize;
        #[cfg(feature = "sd-bench")]
        let blocks = buf.len() / BLOCK_BYTES;
        #[cfg(feature = "sd-bench")]
        let bench_started = embassy_time::Instant::now();
        let result = crate::flpr_mux::with_storage(|sd| {
            // A staged upload may have left the previous arena half in FLPR DMA while USB filled
            // the other one. No read may pass it; joining here preserves block-device ordering.
            sd.finish_write_blocks()?;
            if addr.is_multiple_of(4) {
                return sd.read_blocks(start, buf);
            }
            // SAFETY: we are inside `with_storage`, which is where `with_bounce` requires its
            // caller to be, and nothing in this closure reaches another bounce user.
            unsafe {
                crate::card_io::with_bounce(addr, |bounce| {
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
        });
        #[cfg(feature = "sd-bench")]
        crate::card_io::note_read_perf(bench_started, addr, blocks);
        if let Err(error) = result {
            defmt::warn!("SD: read at block {=u64}, {=usize} bytes failed: {}", lba, buf.len(), error);
        }
        result
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), SemmcError> {
        let start = FlatCard::lba(lba)?;
        let addr = buf.as_ptr() as usize;
        crate::flpr_mux::with_storage(|sd| {
            // Two arena halves give the USB task an owned, aligned DMA source. Join the older half,
            // start this one, and return while the card runs, so the engine can receive, CRC and
            // fill the disjoint half. Generic callers stay synchronous.
            if addr.is_multiple_of(4) && crate::arena::usb_stage_contains(addr, buf.len()) {
                sd.finish_write_blocks()?;
                // SAFETY: the arena gate retains both halves for the transfer, and the engine does
                // not reuse this half until the next staged write has joined it here.
                return unsafe { sd.start_write_blocks(start, buf) };
            }
            sd.finish_write_blocks()?;
            if addr.is_multiple_of(4) {
                return sd.write_blocks(start, buf);
            }
            // SAFETY: as in `read`.
            unsafe {
                crate::card_io::with_bounce(addr, |bounce| {
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
        })
    }

    /// Synchronous callers have nothing to flush. The staged USB path uses this as the explicit
    /// join seam before its arena grant is released, so a deferred card DMA can never outlive the
    /// buffer it borrows.
    ///
    /// `Semmc::write_blocks` polls CMD13 until the card has left `prg`, so the program cycle is the
    /// completion signal and every write is already durable when the store's next statement runs. A
    /// transport with a write-back cache would move that cost back here.
    fn sync(&self) -> Result<(), SemmcError> {
        crate::flpr_mux::with_storage(|sd| sd.finish_write_blocks())
    }
}

/// The mounted store, resident for the life of the image.
///
/// `.bss`, and written in place: `FlatStore` is about 10.5 KB, most of it the 8 KiB free bitmap.
/// See [`mount_at_boot`].
static mut FLAT_STORE: MaybeUninit<FlatStore<FlatCard>> = MaybeUninit::uninit();
static FLAT_STORE_READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// What this layer costs the resident budget: the store and the write queue. The alignment bounce
/// is `sd`'s and is counted there. The recording caller's bounded append buffer is
/// [`crate::flat_ride`]'s, because the ride loop lends its bytes.
pub(crate) const RESIDENT_BYTES: usize = core::mem::size_of::<FlatStore<FlatCard>>()
    + REQUEST_QUEUE_BYTES
    + CATALOG_UPLOAD_BYTES
    + MAP_READ_BYTES
    + ROUTE_READ_BYTES
    + ENGINE_BYTES;

/// Everything the read path keeps resident: the session-long [`MAP_SOURCE`] and the [`MAP_NAME`]
/// the same boot step captures.
pub(crate) const MAP_READ_BYTES: usize = core::mem::size_of::<obc_storage::flat::StoreSource<'static, FlatCard>>()
    + core::mem::size_of::<heapless::String<24>>();

/// The active route's one held revision. It is released on selection or revision changes.
pub(crate) const ROUTE_READ_BYTES: usize =
    core::mem::size_of::<Option<obc_storage::flat::StoreSource<'static, FlatCard>>>();

/// The store is the free bitmap plus its rows; if that stops being true, the budget note above is
/// wrong before anything else notices.
const _: () = assert!(core::mem::size_of::<FlatStore<FlatCard>>() > 8 * 1_024);

/// Mount the card's flat store into the `.bss` slot, and hand back the one `&'static` to it.
///
/// `#[inline(never)]` is load-bearing twice over. A `FlatStore` built by value inside the boot
/// task's async block is a permanent 10.5 KB slot in that task's poll frame, allocated at the entry
/// of every poll. And `mount` links at about 14 KB, because `load` streams the catalog in the frame
/// that is building the store; called from here that frame is one transient sibling step of the
/// boot chain, which is what `resource_guard.py board` measures.
///
/// It goes through [`FlatStore::mount_in_place`] rather than `mount`, and the difference is
/// measured: `slot.write(FlatStore::mount(card))` builds the store as a local of this function and
/// copies it into the slot, which puts a second 10,688 B frame on the boot chain. Placed, this
/// frame carries a pointer.
///
/// This is `mount`'s only caller and it runs exactly once, before anything is spawned. A warm reset
/// re-enters and overwrites in place; `FlatStore` has no `Drop`, which is the `init_static`
/// contract.
///
/// The card must already be up — [`crate::sd::bring_up_card`] first — or every probe read is
/// `SemmcError::NoBoot` and the store classifies a good card as unformatted.
#[inline(never)]
pub(crate) fn mount_at_boot() -> &'static FlatStore<FlatCard> {
    // SAFETY: sole writer of FLAT_STORE. `mount_at_boot` runs once per boot on the one thread-mode
    // executor, before any task that could hold a reference exists, so this `&mut` is the only live
    // borrow. The write is unconditional and `FlatStore` has no `Drop`.
    let store = unsafe { FlatStore::mount_in_place(&mut *core::ptr::addr_of_mut!(FLAT_STORE), FlatCard) };
    FLAT_STORE_READY.store(true, core::sync::atomic::Ordering::Release);
    &*store
}

/// What the probe found, in the terms boot has to act on.
pub(crate) enum Card {
    /// A flat store this build can read: read-write, or read-only because a revision or sequence
    /// space is exhausted.
    Flat,
    /// Neither superblock is valid, so this is not a supported flat store. Two block reads and out.
    NotFlat,
    /// A card that is a flat store and will not serve one: no well-formed gate and no validated
    /// candidate body, which is media damage, or a card smaller than the superblock on it describes.
    /// This is not a fall-through to FAT: the superblock says a flat store was written here, so a
    /// FAT mount would report the failure of a stack that was never on this card.
    FlatBroken(obc_app::BootFault),
}

/// Collapse the classification onto the three cards above, and log the reason for the one that is
/// neither a working store nor a plain FAT card.
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

/// Classify a missing or unreadable map from the catalog facts [`report`] already collected, so the
/// listing is paid for once.
pub(crate) fn boot_fault_for(catalog: Catalog) -> obc_app::BootFault {
    obc_app::flat_boot_fault(catalog.maps, catalog.listing_complete)
}

/// What one walk of the catalog found.
#[derive(Clone, Copy)]
pub(crate) struct Catalog {
    /// Entries whose kind is a map (§3.1's `MapShard` / `MapSetManifest`).
    pub(crate) maps: usize,
    /// False when a commit moved the catalog under the listing cursor, so the walk cannot prove that
    /// the card is empty.
    pub(crate) listing_complete: bool,
}

/// How many tasks hold a [`Writer`]: the BLE v4 adapter, the USB v4 adapter, and the ride loop.
///
/// The ride loop's nav ticket and its recorder call share one two-job allowance, because the task
/// cannot be polled in two places at once. [`REQUEST_QUEUE`] is derived from this census.
const SENDERS: usize = 3;

/// Write requests queued at once.
///
/// Two per sender, and that number is load-bearing rather than generous. Each link has at most two
/// jobs on this queue at one instant: the one its lane is awaiting, and at most one orphan, whose
/// caller's future was dropped between the send and the answer. Nothing produces a third, because a
/// lane holds one buffer and cannot issue a second call without it.
///
/// Sizing to that census is what keeps the queue from ever filling, and a queue that cannot fill is
/// the difference between a recoverable orphan and a lost one: `Sender::send` on a full queue
/// parks, and a `Writer::call` future dropped while parked never enqueues its job at all, taking
/// the `&'static mut` reaction buffer with it permanently. [`Lane::reclaim`] rests on this.
const REQUEST_QUEUE: usize = 2 * SENDERS;

/// The queue's resident cost, named rather than left anonymous because it is the one part of this
/// layer whose size is a design choice: `REQUEST_QUEUE` times a `Job`, and a `Job` is as large as
/// the largest request.
pub(crate) const REQUEST_QUEUE_BYTES: usize =
    core::mem::size_of::<Channel<CriticalSectionRawMutex, Job, REQUEST_QUEUE>>();

/// One unit of write work.
///
/// Reads are not here, and that is the design. `ByteSource::read_at` is synchronous and
/// latency-bound, and routing a render's reads through a channel would add a scheduler round trip
/// to every one of them for nothing: the store already serves a reader with no borrow held at every
/// card command of a running write.
///
/// Everything carried here is owned or `'static`, because a request outlives the statement that
/// sent it and a borrowed payload would need a lifetime the channel cannot express. A write's bytes
/// come from a `'static` staging buffer.
///
/// The variants differ in size by an order of magnitude: a `Commit` carries up to [`MAX_BATCH`]
/// mutations and each embeds a 48-byte display name, so the enum is as large as a batch. That is
/// inherent. There is no allocator, so the lint's advice to box the large field is not available,
/// and the alternatives — a `'static` batch slot per caller, or a commit of one mutation that gives
/// up atomicity across a batch — are both worse than the roughly 1 KB this costs.
#[allow(dead_code, clippy::large_enum_variant)]
pub(crate) enum Request {
    WriteCheckpoint {
        scope: obc_app::device_core::StoreRevision,
        change: obc_app::navigator::CheckpointChange,
    },
    ReconcileMetadata,
    CleanupRoute {
        before_utc: u32,
        store: obc_app::device_core::StoreIdentity,
        active: Option<ObjectId>,
    },
    Allocate {
        bytes: u64,
    },
    /// Append a staged planner step and optionally backfill its completed OBCR header. Replies with
    /// the advanced allocation. Patch-first ordering keeps the caller's old token cancellable if the
    /// append fails.
    WriteComputedRoute {
        allocation: Allocation,
        bytes: &'static [u8],
        header: &'static [u8],
    },
    /// Append the rollback container to a fresh allocation: the unsigned OBCU header, then the
    /// running image straight out of the memory-mapped app slot. Replies with the advanced
    /// allocation, which the arm's commit publishes as the rollback reserve.
    WriteRollback {
        allocation: Allocation,
        header: [u8; obc_dfu::HEADER_LEN],
        image: &'static [u8],
    },
    Seal {
        allocation: Allocation,
        out: &'static mut Option<obc_storage::flat::SealedAllocation<'static>>,
    },
    ReleaseSealed {
        sealed: obc_storage::flat::SealedAllocation<'static>,
    },
    /// Publish a freshly generated route under the store's next id.
    PublishComputedRoute {
        allocation: Allocation,
        name: DisplayName,
        original: Option<(ObjectId, Revision)>,
    },
    /// Compensate a cancellation that raced the synchronous publish. The exact revision is carried,
    /// so this can never remove a later replacement that happens to share the object id.
    RemoveComputedRoute {
        id: ObjectId,
        revision: Revision,
    },
    /// Remove the current head only if it belongs to the requested catalog family.
    RemoveObject {
        id: ObjectId,
        kind: obc_app::catalog_state::CatalogObjectKind,
    },
    /// One atomic batch. Replies with the commit sequence.
    Commit {
        batch: heapless::Vec<Mutation, MAX_BATCH>,
    },
    Journal {
        checkpoint: RideCheckpoint<'static>,
    },
    /// Give a reservation back without publishing it.
    Cancel {
        allocation: Allocation,
    },
    /// Return a hold row. Refused rather than obeyed while another reader holds it.
    Close {
        handle: Handle,
    },

    // The engine runs here, inside the one task that writes, and the transports are pure record
    // shippers. `obc_link::flat::Store` is synchronous throughout, including the mutators, so an
    // engine driven from a transport task would have to reach the card from a second execution
    // context. Sitting it behind this queue makes "one engine, one owner" a property of the type.
    /// One whole control record (§3.1), and the buffer the reaction's bytes land in.
    /// `out` is the caller's `'static` buffer and rides back in [`Outcome::Reacted`]. It is a
    /// borrow rather than a copy for the same reason [`Request::Write`]'s bytes are: a request
    /// outlives the statement that sent it, and a page or stream record would otherwise be copied
    /// twice per record.
    Control {
        link: Link,
        record: &'static [u8],
        out: &'static mut [u8],
    },
    /// One whole stream record: the 16-byte frame followed by exactly its payload.
    Stream {
        link: Link,
        record: &'static [u8],
        out: &'static mut [u8],
    },
    /// A USB stream record whose upload owns the arena's double 64 KiB stage.
    StreamStaged {
        record: &'static [u8],
        out: &'static mut [u8],
    },
    /// One arena-local USB map batch. Records were validated and packed by the cable adapter; the
    /// engine rechecks ownership and continuity before issuing the single media write.
    StreamStagedBatch {
        request: RequestId,
        offset: u64,
        len: usize,
        out: &'static mut [u8],
    },
    /// Join any card DMA that still borrows the USB arena before its guard is released.
    FinishUsbStage,
    /// Pump the engine once: a live `GET`'s next record, or an error owed to a dropped transfer. An
    /// adapter repeats this until the reaction is [`Reaction::Idle`]; a driver that stops pumping
    /// stalls a download.
    Pump {
        link: Link,
        out: &'static mut [u8],
    },
    /// This link came up with these record ceilings.
    ///
    /// It re-pins this link's ceilings and releases this link's transfer if it had one, and touches
    /// nothing belonging to the other link. It carries a validated [`Ceilings`], not two numbers, so
    /// a floor refusal never reaches this queue: the adapter closes the channel on `None`.
    LinkUp {
        link: Link,
        ceilings: Ceilings,
    },
    /// This link went away. It answers nobody, because there is nobody left to answer, and releases
    /// only what that link held, so an unplugged cable is not a reason to kill a phone's download.
    LinkLost {
        link: Link,
    },
    /// The live transfer's `RequestId`, if one owns the engine.
    ///
    /// The one read on this queue, and it earns its place: cross-channel ordering makes an adapter
    /// hold a stream frame for a `RequestId` it has not yet seen admitted, and only the engine can
    /// answer whether it was. One round trip, and only inside the race window.
    LiveTransfer,
    /// Whether this exact request is a map upload owned by USB. This is the admission proof for the
    /// cable-only arena arm; app-facing map progress intentionally carries no link identity.
    UsbMapUpload {
        request: RequestId,
    },
}

/// What one [`Request`] produced.
#[allow(dead_code)]
pub(crate) enum Outcome {
    CleanedRoute(Option<ObjectId>),
    Metadata(Result<(), obc_app::metadata::MetadataError>),
    Allocated(Allocation),
    /// The allocation, advanced by the bytes written.
    Wrote(Allocation),
    /// Object id assigned to a board-generated route.
    Published(ObjectId),
    /// §5.5's commit sequence.
    Committed(u64),
    /// A [`Request::RemoveObject`] ran. `false` means the entry was already absent, which the
    /// catalog domain reads as a success.
    Removed {
        existed: bool,
    },
    Done,
    /// What the engine wants done, and the caller's buffer back with the bytes in it.
    Reacted {
        reaction: Reaction,
        out: &'static mut [u8],
    },
    /// The live transfer's `RequestId`, or `None` when the engine is idle.
    Live(Option<RequestId>),
    /// Declared payload length when [`Request::UsbMapUpload`] names the live cable map upload.
    UsbMap(Option<u64>),
}

/// The caller's half of one round trip: the answer, tagged with the request it answers.
///
/// The tag is what makes [`Writer::call`] cancellation-safe. `call` is an `async fn`, so its future
/// can be dropped between the send and the wait. The task serves the request and signals the slot
/// anyway, and the next caller to use that slot would otherwise wake on a value that answers a
/// request it never made, and take a stale `Allocation` or commit sequence for its own.
///
/// A `Signal` rather than a channel is right — exactly one value per round trip, and a dropped
/// caller must not leave a queued reply behind — but "the value in the slot is mine" has to be
/// checked.
pub(crate) type Reply = Signal<CriticalSectionRawMutex, (u32, Result<Outcome, StoreError>)>;

/// Hands out [`Job::tag`]s. Monotonic, and never reused in any window that matters: a collision
/// needs 2^32 intervening calls and the same reply slot.
static NEXT_TAG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// A request, where its answer goes, and which request that answer is for.
pub(crate) struct Job {
    request: Request,
    reply: &'static Reply,
    /// Matched by [`Writer::call`] against its own — see [`Reply`].
    tag: u32,
}

/// The queue itself: any task may send, exactly one task receives.
static REQUESTS: Channel<CriticalSectionRawMutex, Job, REQUEST_QUEUE> = Channel::new();

/// The write half's front door, handed to every plane that mutates the store.
///
/// `Copy`, and it carries no store reference at all, which is what makes "there is exactly one
/// execution context that writes" a property of the type rather than a convention.
///
#[derive(Clone, Copy)]
pub(crate) struct Writer {
    requests: Sender<'static, CriticalSectionRawMutex, Job, REQUEST_QUEUE>,
}

#[derive(Clone, Copy)]
pub(crate) struct Ticket(u32);

impl Writer {
    /// Enqueue one call without parking the ride loop. A full queue is retried by the caller on a
    /// later pass; a successful ticket is polled with [`Writer::try_result`].
    pub(crate) fn try_call(&self, request: Request, reply: &'static Reply) -> Result<Ticket, ()> {
        let tag = NEXT_TAG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.requests.try_send(Job { request, reply, tag }).map(|()| Ticket(tag)).map_err(|_| ())
    }

    // A full bounded queue must return the cleanup owner without a heap allocation.
    #[allow(clippy::result_large_err)]
    pub(crate) fn try_call_owned(&self, request: Request, reply: &'static Reply) -> Result<Ticket, Request> {
        let tag = NEXT_TAG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.requests.try_send(Job { request, reply, tag }).map(|()| Ticket(tag)).map_err(|error| match error {
            embassy_sync::channel::TrySendError::Full(job) => job.request,
        })
    }

    /// Take this ticket's answer without parking. Older orphaned answers are discarded exactly as
    /// [`Writer::call`] does; `None` means the storage task has not answered yet.
    pub(crate) fn try_result(&self, ticket: Ticket, reply: &'static Reply) -> Option<Result<Outcome, StoreError>> {
        loop {
            let (answered, outcome) = reply.try_take()?;
            if answered == ticket.0 {
                return Some(outcome);
            }
            debug_assert!(answered < ticket.0, "a reply slot answered a tag that has not been issued");
        }
    }

    /// Send `request` and wait for the store's answer.
    ///
    /// The caller blocks here and nowhere else, and it blocks on its own `Signal`, not on the store:
    /// the queue slot is released the moment the task takes the job.
    ///
    /// Cancellation-safe: dropping this future between the send and the answer is legitimate, and
    /// the answer that arrives afterwards is discarded by the next caller rather than mistaken for
    /// its own. The slot is never `reset`, only advanced past.
    ///
    /// `reply` is a `'static` slot the caller owns, and the contract on it is one slot per
    /// concurrently live call. A slot may be reused across time, but it must never be awaited by two
    /// callers at once: a `Signal` holds one value and one waker, so two live waiters lose an answer
    /// and wake each other instead of themselves, which on this board is an executor-starving loop
    /// and then a watchdog reset.
    pub(crate) async fn call(&self, request: Request, reply: &'static Reply) -> Result<Outcome, StoreError> {
        let tag = NEXT_TAG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.requests.send(Job { request, reply, tag }).await;
        self.result(tag, reply).await
    }

    /// Enqueue one call and return without waiting for the answer. The awaited half is
    /// [`Writer::finish_call`]; between the two, the reply slot and whatever the request borrows are
    /// committed to this call.
    pub(crate) async fn begin_call(&self, request: Request, reply: &'static Reply) -> Ticket {
        let tag = NEXT_TAG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        self.requests.send(Job { request, reply, tag }).await;
        Ticket(tag)
    }

    pub(crate) async fn finish_call(&self, ticket: Ticket, reply: &'static Reply) -> Result<Outcome, StoreError> {
        self.result(ticket.0, reply).await
    }

    async fn result(&self, tag: u32, reply: &'static Reply) -> Result<Outcome, StoreError> {
        loop {
            let (answered, outcome) = reply.wait().await;
            if answered == tag {
                return outcome;
            }
            // Someone else's answer, left in this slot by a `call` whose future was dropped before
            // it collected. Consuming it is the point: `Signal::wait` takes the value, so the next
            // wait is for ours. A dropped caller is a legitimate outcome of a `select`, not a fault.
            debug_assert!(answered < tag, "a reply slot answered a tag that has not been issued");
        }
    }
}

/// One link's half of a round trip to the engine: the buffer it lends, the slot the answer comes
/// back in, and nothing else.
///
/// One lane per link, because two links are live at once and every part of a round trip is
/// per-caller: the reaction buffer, the reply slot, and the recovery below. The buffer is lent
/// rather than copied — it crosses the queue inside the request and comes back inside the answer —
/// so a `None` here means a previous call's future was dropped between the send and the answer.
/// [`Lane::reclaim`] is how that is recovered.
pub(crate) struct Lane {
    out: Option<&'static mut [u8]>,
    reply: &'static Reply,
    /// Which link this is, for the log lines. The adapters are otherwise indistinguishable here.
    who: &'static str,
}

/// How long [`Lane::reclaim`] waits for an orphaned answer before giving the link up.
///
/// It waits on the storage task to finish jobs already in the queue, so the bound is a scheduling
/// one, and the longest single thing that task does is a commit of about 250 ms. A link that has
/// not been answered by then is not going to be.
const RECLAIM_TIMEOUT: embassy_time::Duration = embassy_time::Duration::from_secs(2);

impl Lane {
    /// Build a lane over a caller-owned buffer and reply slot. Called once per link, per image.
    pub(crate) fn new(out: &'static mut [u8], reply: &'static Reply, who: &'static str) -> Self {
        Lane { out: Some(out), reply, who }
    }

    /// Recover the buffer from a call whose future was dropped.
    ///
    /// It waits on its own slot rather than reasoning about when someone else's call proves the
    /// orphan ran. Two facts, each local to one lane, make that an observation:
    ///
    /// 1. A reply slot has exactly one caller. Each is a `static` private to its adapter, so
    ///    whatever arrives in this slot is this lane's orphan and no other link's.
    /// 2. The orphan is always in the queue. [`REQUEST_QUEUE`] is sized to the sender census, so
    ///    `Sender::send` never parks and a dropped future is always dropped after its job was
    ///    enqueued. A job in the queue is a job that will be served and answered.
    ///
    /// The wait is bounded anyway, because (2) rests on a constant two files can change
    /// independently.
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

    /// Hand one request to the engine without waiting for its answer.
    ///
    /// The buffer travels with the request, so until [`Lane::collect`] takes it back this lane can
    /// make no other call, and the caller owns that sequencing. The enqueue never blocks in
    /// practice, so the await here is queue admission, not storage work.
    pub(crate) async fn call_deferred(
        &mut self,
        writer: &Writer,
        make: impl FnOnce(&'static mut [u8]) -> Request,
    ) -> Option<Ticket> {
        let out = self.out.take()?;
        Some(writer.begin_call(make(out), self.reply).await)
    }

    /// Take a deferred call's answer and the buffer back. The reaction's bytes are in this lane's
    /// buffer exactly as after [`Lane::call`].
    pub(crate) async fn collect(&mut self, writer: &Writer, ticket: Ticket) -> Option<Reaction> {
        match writer.finish_call(ticket, self.reply).await {
            Ok(Outcome::Reacted { reaction, out }) => {
                self.out = Some(out);
                Some(reaction)
            }
            _ => {
                defmt::warn!("flat/v4: [{}] the engine answered a batch with the wrong shape — lane closed", self.who);
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

/// The [`Writer`], if there is anything on the other end; `None` on a card that is not a flat
/// store.
///
/// The `Option` is the difference between an error and a hang. The queue is a `static`, so
/// `REQUESTS.sender()` succeeds whether or not a task is draining it, and a caller that sent into
/// an undrained channel would fill two slots and then wait forever in `Sender::send`.
pub(crate) fn writer() -> Option<Writer> {
    ARMED.load(core::sync::atomic::Ordering::Relaxed).then(|| Writer { requests: REQUESTS.sender() })
}

/// The ride loop's wake source on a catalog movement. It is not a level: the level is
/// [`FlatStore::sequence`], read straight off the store.
static CATALOG_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

static LIVE_TRANSFER: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub(crate) fn transfer_active() -> bool {
    LIVE_TRANSFER.load(core::sync::atomic::Ordering::Relaxed)
}

/// One successful protocol-v4 route/trip upload waiting for the app's post-rescan event seam.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CatalogUpload {
    id: [u8; 8],
    flags: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogUploadKind {
    Route,
    Trip,
}

impl CatalogUpload {
    fn new(kind: CatalogUploadKind, id: u64, replaced: bool) -> Self {
        let flags = (kind == CatalogUploadKind::Trip) as u8 | ((replaced as u8) << 1);
        Self { id: id.to_le_bytes(), flags }
    }

    pub(crate) const fn kind(self) -> CatalogUploadKind {
        if self.flags & 1 == 0 {
            CatalogUploadKind::Route
        } else {
            CatalogUploadKind::Trip
        }
    }

    pub(crate) const fn id(self) -> u64 {
        u64::from_le_bytes(self.id)
    }

    pub(crate) const fn replaced(self) -> bool {
        self.flags & 2 != 0
    }

    fn same_object(self, other: Self) -> bool {
        (self.flags & 1) == (other.flags & 1) && self.id == other.id
    }
}

const _: () = assert!(core::mem::size_of::<CatalogUpload>() == 9);

/// A complete UI catalog's worth of upload facts. The engine serializes transfers, and the ride
/// task drains these after the catalog rescan the same commits caused. Bounding this to the menus'
/// combined identity capacity keeps the resident cost explicit. Same-object commits coalesce, and
/// churn beyond one full snapshot retains the newest facts and raises the conservative
/// active-route refresh below.
const UPLOAD_EVENTS_CAP: usize = obc_app::MAX_ROUTES + obc_app::MAX_TRIPS;
static UPLOAD_EVENTS: Mutex<CriticalSectionRawMutex, RefCell<Deque<CatalogUpload, UPLOAD_EVENTS_CAP>>> =
    Mutex::new(RefCell::new(Deque::new()));
static UPLOAD_EVENTS_LOSS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The bounded route/trip commit handoff plus its one-bit conservative loss signal.
pub(crate) const CATALOG_UPLOAD_BYTES: usize = core::mem::size_of::<
    Mutex<CriticalSectionRawMutex, RefCell<Deque<CatalogUpload, UPLOAD_EVENTS_CAP>>>,
>() + core::mem::size_of::<core::sync::atomic::AtomicBool>();

/// Insert the latest fact for one object at the back of the queue. Repeated replaces must not spend
/// another slot: remove the older fact, preserve every other fact's order, then append the final
/// `replaced` value at its true commit position. Returns whether a distinct oldest fact had to be
/// evicted because catalog churn left more queued identities than the UI can simultaneously hold.
fn queue_catalog_upload(events: &mut Deque<CatalogUpload, UPLOAD_EVENTS_CAP>, upload: CatalogUpload) -> bool {
    let queued = events.len();
    let mut coalesced = false;
    for _ in 0..queued {
        if let Some(prior) = events.pop_front() {
            if prior.same_object(upload) {
                coalesced = true;
            } else {
                let _ = events.push_back(prior);
            }
        }
    }
    if coalesced {
        let _ = events.push_back(upload);
        return false;
    }
    if let Err(upload) = events.push_back(upload) {
        let _ = events.pop_front();
        let _ = events.push_back(upload);
        return true;
    }
    false
}

fn note_catalog_upload(upload: CatalogUpload) {
    UPLOAD_EVENTS.lock(|events| {
        if queue_catalog_upload(&mut events.borrow_mut(), upload) {
            UPLOAD_EVENTS_LOSS.store(true, core::sync::atomic::Ordering::Relaxed);
            defmt::warn!("flat: upload-event queue saturated — oldest fact replaced; forcing active-route refresh");
        }
    });
}

pub(crate) fn take_catalog_upload() -> Option<CatalogUpload> {
    UPLOAD_EVENTS.lock(|events| events.borrow_mut().pop_front())
}

pub(crate) fn take_catalog_upload_loss() -> bool {
    UPLOAD_EVENTS_LOSS.swap(false, core::sync::atomic::Ordering::Relaxed)
}

fn note_catalog_commit() {
    CATALOG_WAKE.signal(());
}

pub(crate) async fn wait_catalog_commit() {
    CATALOG_WAKE.wait().await
}

/// The one task that writes.
///
/// It owns nothing: the store is `&'static` and every reader has one too. What it owns is the right
/// to call the mutators, and that is the whole of the serialization — no lock and no `RefCell` at
/// this layer. The store's own cells are what make a concurrent reader safe, and this task is what
/// makes concurrent writers impossible instead of merely refused.
///
/// What it deliberately does not have is a lock around a commit. The granularity law — per card
/// command, never per commit — is a property of `obc_storage::flat`, and this layer's obligation is
/// not to take it away. A render's `read_at` never touches this channel.
///
/// The remaining stall is the executor's, not the store's. `Store::commit` is synchronous, about 36
/// card commands over 250 ms at 1,024 entries, and this is one cooperative executor, so between its
/// first and last command no other task is polled. Closing that needs a yield seam in
/// `obc-storage`, which is not smuggled in as a lock here: no felt stall was found on glass.
#[embassy_executor::task]
pub(crate) async fn storage_task(
    store: &'static FlatStore<FlatCard>,
    requests: Receiver<'static, CriticalSectionRawMutex, Job, REQUEST_QUEUE>,
) -> ! {
    defmt::info!("flat: storage task up — the write half and the v4 engine are serialized here, reads stay direct");
    // The engine and its policy live in `.bss`, built out of line: see `engine_slot`.
    let engine = engine_slot();
    let mut policy = BoardPolicy;
    // When the stall watchdog next looks. `None` while no transfer is live, and then this arm never
    // fires at all. A free-running ticker would wake a parked device every second.
    let mut next_look: Option<Instant> = None;
    loop {
        let watch = async {
            match next_look {
                Some(at) => Timer::at(at).await,
                None => core::future::pending().await,
            }
        };
        match select(requests.receive(), watch).await {
            Either::First(job) => {
                let before = store.sequence();
                // The FLPR is switched per card command by `flpr_mux::with_storage`, so nothing is
                // held across this call and there is no mode session to acquire around the batch.
                let outcome = serve(store, engine, &mut policy, job.request);
                if store.sequence() != before {
                    note_catalog_commit();
                }
                // The tag rides back with the answer: the caller may be gone, and the next user of
                // this slot has to be able to tell that the value is not theirs. See `Reply`.
                job.reply.signal((job.tag, outcome));
            }
            // The deadline came round. Nothing to do here: the watchdog runs below on every pass,
            // and this arm exists only so a wedged peer's silence still produces one.
            Either::Second(()) => {}
        }
        // The stall watchdog, on every pass: after a served request, so byte progress re-anchors the
        // deadline, and after the timer, so silence expires it. It runs here rather than in the timer
        // arm because the requests this task serves are not all the transfer's, and a ride
        // journalling every few seconds would keep resetting a wedged transfer's clock.
        let now = Instant::now();
        next_look = match engine.watch_stall(store, now.as_millis() as u32) {
            Stall::Idle => None,
            Stall::Watching { again_in_ms } => Some(now + Duration::from_millis(again_in_ms.into())),
            Stall::Abandoned(request) => {
                defmt::error!(
                    "flat/v4: transfer {=u32} moved no bytes for {=u32} s — abandoned, the store is released",
                    request.0,
                    STALL_TIMEOUT_MS / 1_000
                );
                // The level the ride loop reads is published from here, so a plan the rider asks for
                // next is no longer refused by a peer that left.
                publish_upload(store, engine);
                None
            }
        };
    }
}

/// Remove the head revision of `id`.
///
/// `Ok(false)` means there was nothing at `id`, so the goal state already holds, which
/// [`Request::RemoveObject`] answers as a success. `Err` means the store refused or failed the
/// commit.
///
/// A listing that stopped early is a failure, never an absent object: a cascade advances past a
/// member on `existed: false`, so a media error that truncated the walk before it reached `id`
/// would orphan a route that is still stored.
fn remove_head(
    store: &FlatStore<FlatCard>,
    id: ObjectId,
    kind: obc_app::catalog_state::CatalogObjectKind,
) -> Result<bool, StoreError> {
    use obc_app::catalog_state::CatalogObjectKind;
    let expected = match kind {
        CatalogObjectKind::Route => ObjectKind::Route,
        CatalogObjectKind::Ride => ObjectKind::Ride,
        CatalogObjectKind::Trip => ObjectKind::Trip,
    };
    let found = store.entries().find(|entry| {
        entry.id == id
            && (entry.flags == EntryFlags::NONE || (entry.kind == ObjectKind::Route && entry.flags.is_route_head()))
    });
    if !store.entries_ok() {
        return Err(StoreError::Media);
    }
    let Some(meta) = found else {
        return Ok(false);
    };
    if meta.kind != expected {
        return Err(StoreError::Invalid);
    }
    check_route_change(store, id)?;
    match store.commit(&[Mutation::Remove { id, revision: meta.revision }]) {
        Ok(_) => Ok(true),
        Err(error) => {
            defmt::warn!(
                "flat: removal of {} object {=u64} revision {=u64} failed: {}",
                defmt::Debug2Format(&meta.kind),
                id.0,
                meta.revision.0,
                defmt::Debug2Format(&error)
            );
            Err(error)
        }
    }
}

/// One request against the store. Synchronous and out of line: it is the whole write surface, so
/// its frame is measured as its own symbol rather than folded into the task's poll frame.
///
/// Synchronous is a contract here, not an implementation detail. The v4 adapters hand this function
/// `&'static` borrows of buffers they own, and they are free to reuse those buffers the instant the
/// answer arrives. That is sound for exactly one reason: `serve` never yields, so "the answer
/// arrived" and "the engine is done with the bytes" are the same instant. A resumable commit, an
/// async block device or a per-command yield seam inside `Store::commit` would all break it, and
/// whoever lands one owes this module a different ownership story.
#[inline(never)]
fn serve(
    store: &'static FlatStore<FlatCard>,
    engine: &mut BoardEngine,
    policy: &mut BoardPolicy,
    request: Request,
) -> Result<Outcome, StoreError> {
    match request {
        Request::WriteCheckpoint { scope, change } => Ok(Outcome::Metadata(
            obc_storage::flat::metadata::write_checkpoint(
                store,
                obc_storage::flat::StoreId(scope.store.bytes()),
                scope.revision.raw(),
                change.expected,
                change.next,
            )
            .map_err(metadata_error),
        )),
        Request::ReconcileMetadata => {
            Ok(Outcome::Metadata(obc_storage::flat::metadata::reconcile(store).map_err(metadata_error)))
        }
        Request::Allocate { bytes } => store.allocate(bytes).map(Outcome::Allocated),
        Request::WriteComputedRoute { mut allocation, bytes, header } => {
            if !header.is_empty() {
                store.patch_allocation(&allocation, 0, header)?;
            }
            store.write(&mut allocation, bytes)?;
            Ok(Outcome::Wrote(allocation))
        }
        Request::WriteRollback { mut allocation, header, image } => {
            store.write(&mut allocation, &header)?;
            store.write(&mut allocation, image)?;
            Ok(Outcome::Wrote(allocation))
        }
        Request::Seal { allocation, out } => {
            *out = Some(store.seal(allocation)?);
            Ok(Outcome::Done)
        }
        Request::ReleaseSealed { sealed } => {
            store.release_sealed(sealed).map_err(|_| StoreError::Invalid)?;
            Ok(Outcome::Done)
        }
        Request::PublishComputedRoute { allocation, name, original } => {
            if let Some((id, revision)) = original {
                if store.current_revision(id)? != Some(revision) {
                    return Err(StoreError::NotFound);
                }
            }
            #[cfg(has_nav)]
            if !planner_map_current() {
                return Err(StoreError::NotFound);
            }
            // Publishing can race a queued cancellation. Reserve one further catalog sequence for
            // the exact-revision compensating remove before making the route visible; otherwise a
            // publish at the last sequence would leave a ghost that no later commit can retract.
            if !store.has_commit_capacity(2) {
                return Err(StoreError::ReadOnly);
            }
            let id = store.next_object_id();
            let payload_crc = store.allocation_crc(&allocation)?;
            let meta = EntryMeta {
                added_at_utc: 0,
                id,
                revision: Revision(1),
                kind: ObjectKind::Route,
                flags: EntryFlags::NONE,
                payload_len: allocation.written_bytes(),
                payload_crc,
                name,
            };
            store
                .commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }])
                .map(|_| Outcome::Published(id))
        }
        Request::RemoveComputedRoute { id, revision } => {
            check_route_change(store, id)?;
            store.commit(&[Mutation::Remove { id, revision }]).map(|_| Outcome::Done)
        }
        Request::CleanupRoute { before_utc, store: identity, active } => {
            if catalog_scope(store).store != identity {
                return Err(StoreError::Invalid);
            }
            let Some((id, batch)) = obc_storage::flat::route_cleanup::next(store, before_utc, active)? else {
                return Ok(Outcome::CleanedRoute(None));
            };
            store.commit(&batch)?;
            Ok(Outcome::CleanedRoute(Some(id)))
        }
        Request::RemoveObject { id, kind } => remove_head(store, id, kind).map(|existed| Outcome::Removed { existed }),
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
            publish_upload(store, engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::Stream { link, record, out } => {
            let reaction = engine.on_stream(link, store, policy, record, out);
            publish_upload(store, engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::StreamStaged { record, out } => {
            let reaction = engine
                .upload_stage_bank()
                .and_then(|bank| {
                    crate::arena::with_usb_stage_bank(bank, |stage| {
                        engine.on_stream_staged(
                            Link::Usb,
                            store,
                            policy,
                            StreamBuffers::new(record, &mut *out),
                            bank,
                            stage,
                        )
                    })
                })
                .unwrap_or_else(|| {
                    // Losing the arm invalidates the staged prefix. Cancel rather than switching to
                    // the resident buffer and writing unrelated bytes under the same cursor.
                    engine.on_link_lost(Link::Usb, store);
                    Reaction::Close(obc_link::flat::Channel::Stream)
                });
            publish_upload(store, engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::StreamStagedBatch { request, offset, len, out } => {
            let reaction = engine
                .upload_stage_bank()
                .and_then(|bank| {
                    crate::arena::with_usb_stage_bank(bank, |stage| {
                        engine.on_usb_map_batch(
                            store,
                            policy,
                            UsbMapBatch::new(request, offset, len, bank, stage),
                            &mut *out,
                        )
                    })
                })
                .unwrap_or_else(|| {
                    engine.on_link_lost(Link::Usb, store);
                    Reaction::Close(obc_link::flat::Channel::Stream)
                });
            publish_upload(store, engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::FinishUsbStage => {
            crate::flpr_mux::with_storage(|sd| sd.finish_write_blocks()).map_err(|_| StoreError::Media)?;
            Ok(Outcome::Done)
        }
        Request::Pump { link, out } => {
            let reaction = engine.poll(link, store, out);
            publish_upload(store, engine);
            Ok(Outcome::Reacted { reaction, out })
        }
        Request::LinkUp { link, ceilings } => {
            // Scoped to `link`, and that scoping is the whole point. Releasing the live transfer
            // and rebuilding the engine outright was right while one link existed. With two, a
            // reconnecting radio destroyed a cable's long map upload with no answer to the client
            // sending it, and re-pinned the shared stream ceiling to the radio's 245 bytes so the
            // cable's next record died as over-ceiling. `Engine::on_link_up` now touches only this
            // link's ceilings and transfer, and every transfer carries the ceilings it was admitted
            // under. The newcomer's own `PUT` then meets the one-at-a-time rule the ordinary way.
            engine.on_link_up(link, store, ceilings);
            // The reconnecting link may have owned the live transfer, and `on_link_up` abandons it,
            // so the engine's transfer state moves here exactly as it does on a `LinkLost`. Without
            // this the level the ride loop reads would stay `true` until some later engine request
            // happened to run, withdrawing heavy-operation admission in between.
            publish_upload(store, engine);
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
            publish_upload(store, engine);
            Ok(Outcome::Done)
        }
        Request::LiveTransfer => Ok(Outcome::Live(engine.live_transfer())),
        Request::UsbMapUpload { request } => {
            Ok(Outcome::UsbMap(engine.upload_declared_len(Link::Usb, request, obc_link::flat::ObjectKind::MapShard)))
        }
    }
}

static ROUTE_STORAGE_FULL: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub(crate) fn take_route_storage_full() -> bool {
    ROUTE_STORAGE_FULL.swap(false, core::sync::atomic::Ordering::Relaxed)
}

/// A committed map is not yet a readable map. The USB map path skips the whole-payload CRC, so
/// without this the first code to look at these bytes would be the next boot. Re-open the object
/// and parse its tables: about 3 KB of structure in some forty small reads, tens of milliseconds
/// against a transfer that took minutes.
///
/// It proves the map mounts. It does not prove the bytes are undamaged: nothing here reads a
/// geometry chunk, and OBCM carries no checksum that would make reading them cheap.
///
/// `#[inline(never)]` because `MapTables` and its parse scratch are a few KiB, and a value built
/// inside an async block is a permanent slot in that task's poll frame.
#[inline(never)]
fn check_committed_map(store: &'static FlatStore<FlatCard>, id: ObjectId) -> Option<crate::link::MapVerifyFault> {
    use crate::link::MapVerifyFault;
    match store.with_source(id, None, |source| obc_reader::MapTables::parse(source).map(|_tables| ())) {
        Ok(Ok(())) => None,
        // A read that failed part way through the parse says nothing about the map: the card is the
        // fault, and the rider's fix is a different card rather than a different builder.
        Ok(Err(obc_reader::Error::Source(_) | obc_reader::Error::CacheBusy)) | Err(_) => {
            defmt::error!(
                "flat: map object {=u64} committed and could not be read back — reporting a card fault",
                id.0
            );
            Some(MapVerifyFault::Storage)
        }
        Ok(Err(error)) => {
            defmt::error!(
                "flat: map object {=u64} committed and will not parse ({}) — reporting the transfer as unreadable",
                id.0,
                defmt::Debug2Format(&error)
            );
            Some(MapVerifyFault::NotAMap)
        }
    }
}

fn publish_upload(store: &'static FlatStore<FlatCard>, engine: &mut BoardEngine) {
    let live = engine.live_upload();
    let ended = engine.take_upload_end();
    if matches!(
        ended,
        Some((
            obc_link::flat::ObjectKind::Route,
            obc_link::flat::UploadEnd::Refused(obc_link::flat::ErrorCode::NoSpace)
        ))
    ) {
        ROUTE_STORAGE_FULL.store(true, core::sync::atomic::Ordering::Relaxed);
        CATALOG_WAKE.signal(());
    }
    // The transfer level, every kind and not just the map the card shows. See [`LIVE_TRANSFER`].
    LIVE_TRANSFER.store(engine.live_transfer().is_some(), core::sync::atomic::Ordering::Relaxed);
    // A map's structure is checked here, after the commit, because the store has no read seam over
    // an uncommitted upload. The card says installed only if the bytes parse.
    let mut fault = None;
    if let Some((kind, obc_link::flat::UploadEnd::Committed { id, replaced })) = ended {
        match kind {
            obc_link::flat::ObjectKind::Route => {
                note_catalog_upload(CatalogUpload::new(CatalogUploadKind::Route, id.0, replaced))
            }
            obc_link::flat::ObjectKind::Trip => {
                note_catalog_upload(CatalogUpload::new(CatalogUploadKind::Trip, id.0, replaced))
            }
            // The link and the store each name objects with their own newtype over the same u64;
            // the seam between them is this crate's job, as everywhere else here.
            obc_link::flat::ObjectKind::MapShard => fault = check_committed_map(store, ObjectId(id.0)),
            _ => {}
        }
    }
    crate::link::publish_map_transfer(live, ended);
    if let Some(fault) = fault {
        crate::link::publish_map_verify_failure(fault);
    }
}

/// The engine's staging buffer, in bytes.
///
/// 512, the minimum the engine's own assertion allows. The stage exists to turn a burst of small
/// link records into few large card writes, and on BLE there is no burst to turn: a CoC SDU is 245
/// bytes, so a 4 KiB stage would batch writes the link cannot fill it with. What it would cost is
/// real: [`Engine`] embeds the buffer, so a 4 KiB engine built by value is 4 KiB of transient frame
/// at a depth this board measures.
const ENGINE_STAGE: usize = 512;

pub(crate) type BoardEngine = Engine<FlatStore<FlatCard>, ENGINE_STAGE>;

/// `.bss`, because an engine built by value inside [`storage_task`]'s async block is a permanent
/// slot in that task's poll frame, allocated at the entry of every poll.
static mut ENGINE: MaybeUninit<BoardEngine> = MaybeUninit::uninit();

/// The engine's resident cost, named for the resource report.
pub(crate) const ENGINE_BYTES: usize = core::mem::size_of::<BoardEngine>();

/// Build the engine into its slot and hand back the one `&'static mut`.
///
/// `#[inline(never)]` so the constructor's frame is a transient sibling rather than part of the
/// task's poll frame. It is small, but the rule is about where a value is built, not how big it is.
///
/// It comes up with no link up and therefore no ceilings, which is the honest starting state of a
/// device nobody has connected to: each adapter announces itself with [`Request::LinkUp`] and is
/// served only while it has.
///
/// # Safety
/// Sole writer of [`ENGINE`]; called exactly once, from [`storage_task`], which is spawned once.
#[inline(never)]
fn engine_slot() -> &'static mut BoardEngine {
    // SAFETY: sole writer; `storage_task` is spawned exactly once and nothing else names this slot.
    unsafe { crate::init_static(core::ptr::addr_of_mut!(ENGINE), Engine::new()) }
}

pub(crate) struct BoardPolicy;

impl Policy for BoardPolicy {}

/// Take the receive end and arm the write half, for the one `spawn` in `main`.
///
/// One function rather than two, because the two facts are the same fact: there is a consumer, and
/// therefore [`writer`] may hand out senders. Splitting them would allow an arming that never
/// spawned, whose senders wedge, or a spawn that never armed, whose task no one can reach.
///
/// Call it exactly once. A second call would make a second consumer, and the serialization this
/// module exists for would be gone.
pub(crate) fn arm() -> Receiver<'static, CriticalSectionRawMutex, Job, REQUEST_QUEUE> {
    let already = ARMED.swap(true, core::sync::atomic::Ordering::Relaxed);
    debug_assert!(!already, "the flat store's write half was armed twice — that is a second consumer");
    REQUESTS.receiver()
}

/// What the mount found, on the log. It returns the [`Catalog`] its walk produced, so
/// `boot_fault_for` decides from this listing rather than taking a second one off the card.
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
    // The mount cost is stated for a card with no ride in progress, so a mount that also read the
    // slot headers and CRC'd a slot did more than that figure covers and is reported apart.
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
    // Read once, after the walk: `entries_ok` reports whether the listing just taken crossed a
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

pub(crate) fn first_of(store: &FlatStore<FlatCard>, kind: ObjectKind) -> Option<EntryMeta> {
    store.entries().find(|entry| entry.kind == kind)
}

/// `debug-uart` only: print the whole catalog, one line per entry, plus the entry count, whether
/// the listing ran to the end, the free extents, the commit sequence and every durable archive
/// proof. Comparing two of these proves a repair removed exactly one object and left every other
/// one byte-identical, because `EntryMeta` carries the per-object CRC; comparing the proof lines
/// against the receipt a client got proves the same identity and stamp survived a remount.
#[cfg(feature = "debug-uart")]
pub(crate) fn debug_census(store: &FlatStore<FlatCard>) {
    for entry in store.entries() {
        defmt::info!(
            "store census: id={=u64} rev={=u64} kind={=u16} flags={=u16} len={=u64} crc={=u32} name={=str}",
            entry.id.0,
            entry.revision.0,
            entry.kind as u16,
            entry.flags.bits(),
            entry.payload_len,
            entry.payload_crc,
            entry.name.as_str().unwrap_or("<invalid>")
        );
    }
    defmt::info!(
        "store census: entry_count={=u16} listing_ok={=bool} free_extents={=u32} sequence={=u64}",
        store.entry_count(),
        store.entries_ok(),
        store.free_extents(),
        store.sequence()
    );
    // The archive proof the card holds, not the resident overlay: a session compares this line
    // with the receipt the client got, before and after a remount.
    match obc_storage::flat::metadata::census(store, |row| {
        defmt::info!(
            "proof census: id={=u64} rev={=u64} len={=u64} crc={=u32} stamp={=u32}",
            row.id.0,
            row.revision.0,
            row.payload_len,
            row.payload_crc,
            row.timestamp
        );
    }) {
        Ok(identity) => defmt::info!("proof census: store={=[u8; 16]:x}", identity.0),
        Err(_) => defmt::warn!("proof census: card metadata unreadable"),
    }
}

/// The session-long source over the mounted card's map object.
///
/// `.bss`, and session-long by construction: a map that is read from boot to power-off across a
/// hundred `await`s cannot be a scope, so this is the by-hand shape. The hold row it spends is
/// never given back, which is correct rather than a leak: that row is what keeps the revision the
/// renderer is drawing from alive while an upload commits over it.
static mut MAP_SOURCE: MaybeUninit<obc_storage::flat::StoreSource<'static, FlatCard>> = MaybeUninit::uninit();

/// Called only by the ride task after a map was opened successfully at boot.
#[cfg(has_nav)]
pub(crate) fn planner_map_current() -> bool {
    // SAFETY: run_app starts only with the initialized, session-long map source.
    unsafe { (&*core::ptr::addr_of!(MAP_SOURCE)).assume_init_ref().is_current() }
}

#[cfg(has_nav)]
pub(crate) fn planner_map_key(store: &FlatStore<FlatCard>) -> obc_formats::obcr::RouteSourceKey {
    // SAFETY: the ride task runs only after the session-long source is initialized.
    let source = unsafe { (&*core::ptr::addr_of!(MAP_SOURCE)).assume_init_ref() };
    obc_formats::obcr::RouteSourceKey {
        store: store.store_id().0,
        object: source.id().0,
        revision: source.revision().0,
    }
}

fn check_route_change(store: &FlatStore<FlatCard>, id: ObjectId) -> Result<(), StoreError> {
    obc_storage::flat::metadata::check_route_change(store, id).map_err(|error| match error {
        obc_storage::flat::metadata::Error::Store(error) => error,
        _ => StoreError::Media,
    })
}

#[cfg(has_nav)]
pub(crate) fn route_fingerprint(
    store: &FlatStore<FlatCard>,
    id: u64,
) -> Option<obc_formats::assistant::PayloadFingerprint> {
    let meta =
        store.entries().find(|meta| meta.kind == ObjectKind::Route && meta.id.0 == id && meta.flags.is_route_head());
    if !store.entries_ok() {
        return None;
    }
    meta.map(obc_storage::flat::metadata::fingerprint)
}

/// The open map's display name, truncated to what the System settings row shows.
///
/// It is captured in [`open_map`] because the alternative is a second catalog walk: at 1,027
/// entries that is about 69 read commands and a tenth of a second, for a string.
static mut MAP_NAME: heapless::String<24> = heapless::String::new();

/// The open map's display name, or `""` before [`open_map`] has run / on a card with no map.
pub(crate) fn map_name() -> &'static str {
    // SAFETY: written once by `open_map` before anything is spawned; read-only afterwards.
    unsafe { (*core::ptr::addr_of!(MAP_NAME)).as_str() }
}

/// Open the card's map object and hand back a `'static` [`ByteSource`] over it.
///
/// `None` when no map object can be opened. [`boot_fault_for`] distinguishes an empty, complete
/// catalog from an unreadable map or an incomplete listing.
///
/// The active map is the lowest-`ObjectId` `MapShard`. Catalog iteration is ordered by
/// `(ObjectId, Revision)` and `first_of` resolves that object's head, so selection is deterministic
/// even on a card that holds several maps. Companion map sends follow the same rule: replace this
/// object using its listed revision, and create only when no map exists.
///
/// `#[inline(never)]` because a `StoreSource` built by value inside the boot task's async block is
/// a permanent slot in that task's poll frame. It is small, but the rule is about where a value is
/// built, and the next thing to grow this type would do it silently.
#[inline(never)]
pub(crate) fn open_map(store: &'static FlatStore<FlatCard>) -> Option<&'static dyn obc_formats::io::ByteSource> {
    #[cfg(feature = "peak-view-demo")]
    let requested =
        option_env!("OBC_TEST_MAP_OBJECT_ID").map(|value| value.parse::<u64>().map(ObjectId)).transpose().ok()?;
    #[cfg(not(feature = "peak-view-demo"))]
    let requested: Option<ObjectId> = None;
    let meta = match requested {
        Some(id) => store.entries().find(|entry| entry.id == id && entry.kind == ObjectKind::MapShard),
        None => first_of(store, ObjectKind::MapShard),
    }?;
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
            // SAFETY: sole writer of MAP_SOURCE. `open_map` runs once per boot on the one
            // thread-mode executor, before any task that could hold a reference exists, the write is
            // unconditional, and the `&'static` handed out is the only reference. `StoreSource`'s
            // `Drop` never runs here, because the value is never dropped.
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

/// The active route's held revision. One route is streamed by the matcher/renderer at a time; this
/// single slot replaces FAT's open file handle and spends one of the store's bounded hold rows.
static mut ROUTE_SOURCE: Option<obc_storage::flat::StoreSource<'static, FlatCard>> = None;

/// The exact revision held for the ride loop's cached route index.
pub(crate) fn route_source_key() -> Option<(ObjectId, obc_storage::flat::Revision)> {
    // SAFETY: only the ride loop owns or reconciles this source, synchronously between frames.
    unsafe { (*core::ptr::addr_of!(ROUTE_SOURCE)).as_ref().map(|source| (source.id(), source.revision())) }
}

/// Take another reader of the exact active route, never a replacement found through a stale menu.
#[cfg(has_nav)]
pub(crate) fn planner_original(
    store: &'static FlatStore<FlatCard>,
    id: ObjectId,
) -> Result<obc_storage::flat::StoreSource<'static, FlatCard>, StoreError> {
    let source = unsafe { &*core::ptr::addr_of!(ROUTE_SOURCE) };
    let source =
        source.as_ref().filter(|source| source.id() == id && source.is_current()).ok_or(StoreError::NotFound)?;
    store.source(id, Some(source.revision()))
}

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

/// The only catalog fields a later object open needs. Keeping the full [`EntryMeta`] here retained
/// a 48-byte display name, flags, length and CRC for every menu slot at once, which at 64 routes
/// made `load_routes` an 18 KiB frame. This 16-byte key preserves the same selection and open
/// contract.
#[derive(Clone, Copy)]
struct CatalogHead {
    id: ObjectId,
    revision: Revision,
}

const _: () = assert!(core::mem::size_of::<CatalogHead>() <= 16);

fn retain_newest<const N: usize>(entries: &mut heapless::Vec<CatalogHead, N>, entry: CatalogHead) {
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
    let mut heads: heapless::Vec<CatalogHead, { obc_app::MAX_ROUTES }> = heapless::Vec::new();
    for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Route && entry.flags.is_route_head()) {
        retain_newest(&mut heads, CatalogHead { id: entry.id, revision: entry.revision });
    }
    if !store.entries_ok() {
        defmt::warn!("flat: route catalog listing failed — keeping the prior menu snapshot");
        return false;
    }

    let mut accepted = 0u64;
    for meta in store.entries().filter(|meta| meta.flags.has(EntryFlags::ASSISTANT_ACCEPTED)) {
        if let Some(index) = heads.iter().position(|head| head.id == meta.id && head.revision == meta.revision) {
            accepted |= 1 << index;
        }
    }
    if !store.entries_ok() {
        return false;
    }

    let mut routes: heapless::Vec<obc_route::RouteSummary, { obc_app::MAX_ROUTES }> = heapless::Vec::new();
    let mut ids: heapless::Vec<u64, { obc_app::MAX_ROUTES }> = heapless::Vec::new();
    let mut candidates = 0u64;
    let mut internal_routes = 0u64;
    for (index, entry) in heads.into_iter().enumerate() {
        match store
            .with_source(entry.id, Some(entry.revision), |source| obc_route::RouteSummary::read_with_candidate(source))
        {
            Ok(Ok((summary, candidate))) => {
                if candidate {
                    internal_routes |= 1 << routes.len();
                }
                if candidate && accepted & (1 << index) == 0 {
                    candidates |= 1 << routes.len();
                    let source = obc_formats::obcr::RouteSourceKey {
                        store: store.store_id().0,
                        object: entry.id.0,
                        revision: entry.revision.0,
                    };
                    if app.can_reconcile_reviews()
                        && !app.retains_find_review(source)
                        && store
                            .with_source(entry.id, Some(entry.revision), |bytes| {
                                obc_route::RouteObjectInfo::read(bytes).is_ok_and(|info| {
                                    info.assistant_candidate
                                        && info.attribution_map.is_some_and(|map| map.store == source.store)
                                })
                            })
                            .unwrap_or(false)
                    {
                        app.reconcile_review_candidate(source);
                    }
                }
                let _ = routes.push(summary);
                let _ = ids.push(entry.id.0);
            }
            Ok(Err(obc_formats::io::Error::Io)) | Err(_) => {
                defmt::warn!(
                    "flat: route object {=u64} revision {=u64} hit transient media I/O — keeping the prior menu snapshot",
                    entry.id.0,
                    entry.revision.0
                );
                return false;
            }
            Ok(Err(_)) => defmt::warn!(
                "flat: route object {=u64} revision {=u64} is malformed — omitted from menu",
                entry.id.0,
                entry.revision.0
            ),
        }
    }
    app.set_routes_with_ids(&routes, &ids);
    app.set_internal_routes(internal_routes);
    app.set_unaccepted_routes(candidates);
    if app.assistant_needs_recovery() {
        if let Ok(checkpoint) = obc_storage::flat::metadata::read_checkpoint(store) {
            app.offer_assistant_checkpoint(catalog_scope(store).store, checkpoint);
        }
    }
    defmt::info!("flat: Route menu loaded {=usize} route(s)", routes.len());
    true
}

/// Decode the newest bounded trip objects and resolve their full-width stage `ObjectId`s against
/// the route snapshot already fed to the app.
#[inline(never)]
pub(crate) fn load_trips(store: &'static FlatStore<FlatCard>, app: &mut obc_app::App) -> bool {
    let mut heads: heapless::Vec<CatalogHead, { obc_app::MAX_TRIPS }> = heapless::Vec::new();
    for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Trip) {
        retain_newest(&mut heads, CatalogHead { id: entry.id, revision: entry.revision });
    }
    if !store.entries_ok() {
        defmt::warn!("flat: trip catalog listing failed — keeping the prior menu snapshot");
        return false;
    }

    let mut metas: heapless::Vec<obc_route::TripMeta, { obc_app::MAX_TRIPS }> = heapless::Vec::new();
    let mut ids: heapless::Vec<u64, { obc_app::MAX_TRIPS }> = heapless::Vec::new();
    for entry in heads {
        match store.with_source(entry.id, Some(entry.revision), |source| obc_route::TripMeta::read(source)) {
            Ok(Ok(meta)) => {
                let _ = metas.push(meta);
                let _ = ids.push(entry.id.0);
            }
            Ok(Err(obc_formats::io::Error::Io)) | Err(_) => {
                defmt::warn!(
                    "flat: trip object {=u64} revision {=u64} hit transient media I/O — keeping the prior menu snapshot",
                    entry.id.0,
                    entry.revision.0
                );
                return false;
            }
            Ok(Err(_)) => defmt::warn!(
                "flat: trip object {=u64} revision {=u64} is malformed — omitted from menu",
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

#[inline(never)]
pub(crate) fn load_rides(store: &'static FlatStore<FlatCard>, app: &mut obc_app::App) -> bool {
    let mut rides = obc_app::RideCatalog::new();
    for entry in store.entries().filter(|entry| entry.kind == ObjectKind::Ride && entry.flags == EntryFlags::NONE) {
        let Ok(Ok(info)) =
            store.with_source(entry.id, Some(entry.revision), |source| obc_route::RideInfo::read(source))
        else {
            defmt::warn!("flat: incomplete ride catalog — keeping the prior menu snapshot");
            return false;
        };
        let position = rides.iter().position(|ride| ride.id < entry.id.0).unwrap_or(rides.len());
        if position < obc_app::UI_RIDES_CAP {
            if rides.is_full() {
                rides.pop();
            }
            let _ = rides.insert(
                position,
                obc_app::RideEntry { id: entry.id.0, summary: obc_app::RideSummary::from_info(&info, false, 0) },
            );
        }
    }
    if !store.entries_ok() {
        return false;
    }
    app.set_rides(&rides);
    defmt::info!("flat: Rides menu loaded {=usize} finished ride(s)", rides.len());
    true
}

/// Answer one keyed ride-track derived need from one immutable flat object revision: the elevation
/// profile, filled in place into the app's resident buffer, and the decimated track shape, into the
/// caller's stack buffer.
///
/// The polyline goes to the caller rather than into the app because it reaches DeviceCore beside
/// the key that guards it, and because at `NAV_PREVIEW_MAX` it is 512 B that must not become
/// resident. Both outputs come from one sample pass; a failure clears the preview and leaves the
/// profile unpublished.
#[inline(never)]
pub(crate) fn fill_ride_track(
    store: &'static FlatStore<FlatCard>,
    app: &mut obc_app::App,
    ride: u64,
    preview: &mut heapless::Vec<(i32, i32), { obc_app::NAV_PREVIEW_MAX }>,
) -> bool {
    let profile = app.begin_ride_profile_fill();
    let valid = matches!(
        store.with_source(ObjectId(ride), None, |source| obc_route::ride_track_into(source, profile, preview)),
        Ok(Ok(()))
    );
    if !valid {
        preview.clear();
        defmt::warn!("flat: ride track fill for object {=u64} failed", ride);
    }
    valid
}

/// Exact physical catalog identity, also used for admitted policy work.
pub(crate) fn catalog_scope(store: &FlatStore<FlatCard>) -> obc_app::device_core::StoreRevision {
    obc_app::device_core::StoreRevision {
        store: obc_app::device_core::StoreIdentity::from_bytes(store.store_id().0),
        revision: obc_app::device_core::Revision::new(store.sequence()),
    }
}

fn metadata_error(error: obc_storage::flat::metadata::Error) -> obc_app::metadata::MetadataError {
    use obc_app::metadata::MetadataError as E;
    use obc_storage::flat::metadata::Error;
    match error {
        Error::Stale | Error::WrongStore => E::Stale,
        Error::RemountRequired => E::RemountRequired,
        Error::Store(StoreError::Busy) => E::Busy,
        _ => E::WriteFailed,
    }
}

#[inline(never)]
pub(crate) fn load_metadata(
    store: &FlatStore<FlatCard>,
    app: &mut obc_app::App,
) -> Result<(), obc_app::metadata::MetadataError> {
    obc_storage::flat::metadata::read_rows(store, |row| app.set_ride_archive_proof(row.id.0, row.timestamp))
        .map_err(metadata_error)?;
    Ok(())
}
