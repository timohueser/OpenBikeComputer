//! The USB **bulk data plane**: the object stream the control plane ([`super::control`]) arms
//! through [`TRANSFER_ARM`].
//!
//! This is a near line-for-line twin of [`crate::ble::data_plane`], and that is the point — the two
//! differ only in `ch.receive`/`ch.send` becoming `ep.read`/`ep.write`. The bulk endpoints carry
//! **only the object's payload bytes** (no per-chunk framing); the whole transfer state machine and
//! the CRC codecs are the host-tested [`obc_ble`] crate, unchanged and unforked:
//!
//! - **Echo loopback** ([`run_echo`]): stream each packet straight back through an
//!   [`obc_ble::Receiver`] (a running CRC, no reassembly buffer), verify **one** whole-object
//!   CRC — the data plane proven end to end with zero storage.
//! - **Uploads** ([`run_upload`]): bytes sink through the [`Receiver`] into an SD temp; commit
//!   validates (CRC + header) and atomically promotes. Uploads don't resume: an unplug, a stall or
//!   an `op=3` abort discards the partial and the host re-sends from the start.
//! - **Downloads** ([`run_download`]): the announce rides the control plane's `status` envelope
//!   (`downloadAnnounce`) first, then raw chunks, one whole-object CRC.
//!
//! **The app keeps running underneath.** Each store call locks the shared SD + settings mutex for
//! its own duration only and releases before the next endpoint `await`, so the ride loop's map
//! render interleaves between chunks. This is the property Mass Storage could not have offered.

use core::cell::RefCell;

use defmt::{info, warn};
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use embassy_usb::driver::{Endpoint as _, EndpointIn, EndpointOut};
use obc_ble::{ObjectType, Receiver, StatusMessage, TransferControl, TransferStatus};

use crate::link::identity;
use crate::link::stage::Stage;
use crate::link::{transfer_result, transfer_result_at, Armed, TRANSFER_ACTIVE};
use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

use super::control::ControlTx;
use super::{EpIn, EpOut, MAX_PACKET};

/// The control plane → data plane hand-off, the USB twin of [`crate::ble::state::TRANSFER_ARM`]:
/// per-transport, because each data plane waits on its own. The *gate* it coordinates with
/// ([`TRANSFER_ACTIVE`]) is shared across transports.
pub(crate) static TRANSFER_ARM: Signal<CriticalSectionRawMutex, Armed> = Signal::new();

/// An abort aimed at the in-flight USB transfer. Latched — an abort that races the transfer's own
/// completion is drained at the same boundary that clears [`TRANSFER_ACTIVE`], before the terminal
/// result goes out, so it can't leak into the next transfer.
pub(crate) static TRANSFER_ABORT: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// One chunk of the object stream. A full max packet, so a multi-megabyte map moves in the fewest
/// transfers the endpoint allows; the SD card is the real ceiling either way (#889).
const CHUNK_LEN: usize = MAX_PACKET as usize;

/// How long the bulk OUT endpoint must stay silent before [`drain_bulk_out`] calls it empty.
///
/// Generous next to a high-speed microframe (125 µs): once the endpoint stops NAKing, a host's
/// queued transfers are delivered back to back, so a gap this long means there is nothing left
/// rather than that the next one is slow.
const DRAIN_QUIET_MS: u64 = 20;

/// Ceiling on one drain.
///
/// **The budget has to fit inside the peer's abort-ack wait, and it is not the only thing in
/// there.** The host gives an abort 2 s (`ABORT_ACK_TIMEOUT_MS`, `builder/app/src/lib/usb/
/// client.ts`) and this drain is one term; the other is whatever the abort's own cleanup costs,
/// which for a **volume set** is deleting up to 32 shard files. Both run before the answer goes
/// out. The drain is therefore sequenced *first* (see the abort arm in [`run_upload`]), so the
/// endpoint goes quiet while the deletes are still running rather than after them, and the two do
/// not stack in front of the same deadline.
///
/// Overrunning is survivable rather than harmless: the host's `sendAbort` swallows the timeout and
/// its busy latch still holds the slot, so the answer arriving late costs a retry some seconds
/// rather than correctness. (It is **not** `statuses.drain()` that saves it — that runs when a
/// transfer slot is *taken*, so a result landing after this one was given up on is still in the
/// mailbox when the next upload starts. What keeps it harmless is that the host ignores a late
/// `aborted` specifically: `checkUploadOpen` only stops a send on a terminal *failure*.)
const DRAIN_BUDGET_MS: u64 = 750;

/// Control plane → data plane: "drain the bulk pipe before I answer this idle abort."
///
/// The bulk OUT endpoint belongs to this task, so the control plane cannot read it; it asks, waits
/// for [`DRAIN_DONE`], and answers. See
/// [`TransferDisposition::AnswerIdleAbort`](crate::link::transfer::TransferDisposition) for why that
/// moment and no other.
static DRAIN_REQ: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Data plane → control plane: the pipe is quiet (or the drain gave up), answer now.
///
/// Carries the [`DRAIN_GEN`] value it answers, so a *late* completion cannot satisfy a later
/// request's wait. Without that, a request that timed out and then completed would leave a `DONE`
/// standing, and the next abort's `wait()` would return before its drain had run at all — an answer
/// racing ahead of the emptying it is supposed to follow.
static DRAIN_DONE: Signal<CriticalSectionRawMutex, u32> = Signal::new();

/// Which drain request is current. Bumped by every request, echoed by the completion.
static DRAIN_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// How long the control plane waits for the data plane to finish draining before answering anyway.
/// A little over [`DRAIN_BUDGET_MS`], since that is what it is waiting on.
const DRAIN_ACK_TIMEOUT_MS: u64 = DRAIN_BUDGET_MS + 250;

/// Ask the data plane to empty the bulk pipe, and wait for it. Called from the control plane when it
/// is about to answer an abort that found nothing in flight.
///
/// Bounded, and a timeout is not an error: the worst case is the stray bytes this exists to prevent,
/// which the whole-object CRC still catches. **A timeout must withdraw the request**, though — a
/// `DRAIN_REQ` left standing is the first arm of the data plane's idle `select`, so it would fire
/// against the *next* transfer and eat up to a full drain window of its opening payload.
pub(crate) async fn drain_before_idle_abort() {
    let generation = DRAIN_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed).wrapping_add(1);
    DRAIN_DONE.reset();
    DRAIN_REQ.signal(());
    let deadline = embassy_time::Duration::from_millis(DRAIN_ACK_TIMEOUT_MS);
    match embassy_time::with_timeout(deadline, DRAIN_DONE.wait()).await {
        Ok(answered) if answered == generation => {}
        Ok(_) => warn!("usb: [bulk] discarded a stale idle-abort drain completion"),
        Err(_) => {
            // Withdraw the request. The data plane may still be about to observe it — that is
            // harmless (it drains an already-quiet pipe and its completion is discarded by the
            // generation check above), whereas leaving it latched is not.
            DRAIN_REQ.reset();
            warn!("usb: [bulk] idle-abort drain did not answer in time — answering the abort anyway");
        }
    }
}

/// **Read and discard whatever the peer still had queued, before this exchange's answer goes out.**
///
/// The host does not wait for the device between chunks — the bulk channel is unframed and
/// unacknowledged, which is what lets an upload keep several `transferOut`s on the wire at once
/// (`UPLOAD_WINDOW` × `DEFAULT_CHUNK_SIZE`, 256 KiB today) — and WebUSB cannot cancel a submitted
/// transfer. So a transfer that ends early leaves bytes still arriving, and the idle loop's discard
/// does not save us: it deliberately lets `TRANSFER_ARM` win its `select`, so a retry's descriptor
/// can arm while the leftovers are still coming and they become its opening payload.
///
/// # Where this is allowed to run, and why it is only two places
///
/// **Draining only works where the peer has stopped pumping**, and there is exactly one such moment
/// in the protocol: the **abort handshake**. The host has thrown out of its send loop, it is holding
/// an `op = 3` open, and the spec has it wait for `transferResult(aborted)` before it does anything
/// else. Both call sites are that moment — an abort against a live transfer (the arm in
/// [`run_upload`]) and an abort that found nothing armed (the control plane, through
/// [`drain_before_idle_abort`]).
///
/// **It is deliberately *not* run on a device-originated termination** — a rejected descriptor, a
/// card that refused an append, a failed final flush. That reads like the same situation and is the
/// opposite one: the host has not been told anything yet, so it refills the window exactly as fast
/// as this discards, and the thing that would make it stop — the terminal `transferResult` — is
/// sitting *behind* the drain. An earlier cut of this branch did drain there, and all it bought was
/// up to [`DRAIN_BUDGET_MS`] of delay before the host could learn to stop, plus a warn on every
/// large upload that hit a storage failure.
///
/// Those paths need no drain of their own: the host's send loop settles every outstanding write
/// before it unwinds (`pumpChunks`), and the bytes it is waiting on are consumed by this module's
/// idle loop, which is reading the whole time. Where that guarantee does *not* hold — a rider's
/// cancel, where the write promises reject while their transfers stay on the wire — the host
/// follows up with the abort that brings us back to the handshake above.
#[inline(never)]
async fn drain_bulk_out(ep: &mut EpOut, buf: &mut [u8]) {
    let deadline = Instant::now() + embassy_time::Duration::from_millis(DRAIN_BUDGET_MS);
    let mut dropped = 0usize;
    loop {
        match select(ep.read(buf), Timer::after_millis(DRAIN_QUIET_MS)).await {
            Either::First(Ok(n)) => dropped += n,
            // The endpoint went away — an unplug drains it far more thoroughly than we can.
            Either::First(Err(_)) => break,
            // Quiet for a whole window: the peer has nothing more queued.
            Either::Second(()) => break,
        }
        if Instant::now() >= deadline {
            warn!("usb: [bulk] still receiving {} ms into an abort drain — answering anyway", DRAIN_BUDGET_MS);
            break;
        }
    }
    if dropped > 0 {
        info!("usb: [bulk] drained {} stray bytes the host had already queued", dropped);
    }
}

/// Whether a transfer runner answered, or the endpoint went away under it.
enum TransferOutcome {
    Answered,
    LinkDropped,
}

/// Close the current descriptor's ownership before publishing its terminal answer. Receipt of
/// `transferResult` is the host's permission to send the next descriptor, so keeping the gate set
/// until after the send returned would create a real `busy` race. Drain only the *old* transfer's
/// latched abort first; an abort arriving after the clear belongs to the next descriptor.
fn close_transfer() {
    let _ = TRANSFER_ABORT.try_take();
    // Hand the scratch arena's staging arm back before the gate opens (#1146 P2): the ride loop
    // reclaims off this level, and every terminal path in this module funnels through here, so
    // "which way did the transfer end" is not a fact the arena has to know.
    super::release_stage();
    TRANSFER_ACTIVE.release(crate::link::gate_owner(crate::link::Transport::Usb));
}

/// Serve the armed transfers forever. Parks on `wait_enabled` before configuration and after an
/// unplug; on any endpoint failure it resets the link state — discarding an in-flight upload and
/// releasing the store's open handles — exactly as `ble::run` does after a disconnect.
pub(crate) async fn run(
    tx: &ControlTx,
    mut ep_in: EpIn,
    mut ep_out: EpOut,
    buf: &'static mut [u8],
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
) -> ! {
    loop {
        ep_out.wait_enabled().await;
        info!("usb: [bulk] endpoint enabled — data plane ready");
        loop {
            // Watch the byte pipe even while no descriptor is armed. A reject is asynchronous
            // relative to the sender, so raw bytes may already be queued; discard those unclaimed
            // bytes rather than letting them be read as the next transfer's first chunk. A valid
            // sender waits for its control-frame reply, and the control plane signals TRANSFER_ARM
            // before sending that reply, so its descriptor always wins the race.
            // The first arm is the control plane asking for a drain before it answers an abort
            // that found nothing in flight (see `drain_before_idle_abort`). It lives here rather
            // than in the control plane because this task owns the endpoint, and it sits ahead of
            // the arm branch because the whole point is to finish before the peer's next descriptor.
            let armed = match select3(DRAIN_REQ.wait(), TRANSFER_ARM.wait(), ep_out.read(buf)).await {
                Either3::First(()) => {
                    let generation = DRAIN_GEN.load(core::sync::atomic::Ordering::Relaxed);
                    drain_bulk_out(&mut ep_out, buf).await;
                    DRAIN_DONE.signal(generation);
                    continue;
                }
                Either3::Second(armed) => armed,
                Either3::Third(Ok(n)) if n > 0 => {
                    warn!("usb: [bulk] discarded {} unclaimed bytes while idle", n);
                    continue;
                }
                Either3::Third(Ok(_)) => continue, // a zero-length packet is not data
                Either3::Third(Err(e)) => {
                    info!("usb: [bulk] idle read ended: {:?} — re-arming", defmt::Debug2Format(&e));
                    break;
                }
            };
            let outcome = match armed {
                Armed::Echo(desc) => run_echo(tx, &mut ep_in, &mut ep_out, &desc, buf).await,
                Armed::Upload(desc, rx) => {
                    let target = if desc.ty == ObjectType::Map { MapTarget::Map } else { MapTarget::Object };
                    run_upload(tx, &mut ep_out, store, shared, &desc, rx, target, buf).await
                }
                Armed::SetShard(desc, rx, part) => {
                    let target = MapTarget::Shard(part);
                    run_upload(tx, &mut ep_out, store, shared, &desc, rx, target, buf).await
                }
                Armed::SetManifest(desc, rx) => {
                    let target = MapTarget::Manifest;
                    run_upload(tx, &mut ep_out, store, shared, &desc, rx, target, buf).await
                }
                Armed::Download(desc) => run_download(tx, &mut ep_in, store, shared, &desc, buf).await,
            };
            if let TransferOutcome::LinkDropped = outcome {
                warn!("usb: [bulk] link dropped mid-transfer — re-arming (uploads restart)");
                break;
            }
        }
        // The endpoint went away (an unplug, or the host re-configured). Discard any in-flight
        // upload, release the store's open handles, clear the one-transfer gate, and drain any
        // latched arm/abort so the next enumeration starts clean.
        //
        // The set teardown is **here** rather than inside `link_reset`, because this is the one
        // place that knows the *cable* is what went away: `link_reset` also runs on a BLE
        // disconnect, and a phone walking out of range must not delete gigabytes the cable is
        // mid-way through writing. Nothing survives an unplug — the set has no manifest, so it is
        // not a map, and there is no way to resume it on the next enumeration (spec §1 principle 4).
        {
            let mut guard = shared.lock().await;
            store.borrow_mut().link_reset(&mut guard);
            store.borrow_mut().set_upload_abort(&mut guard);
        }
        super::release_stage(); // the arena's staging arm, if this teardown interrupted a transfer
        TRANSFER_ACTIVE.release(crate::link::gate_owner(crate::link::Transport::Usb));
        TRANSFER_ARM.reset();
        TRANSFER_ABORT.reset();
        // The drain handshake too, and for a sharper reason than tidiness: `DRAIN_REQ` is the
        // **first** arm of the idle `select` above, so one left standing across an unplug — signalled
        // by a control plane whose wait then died with the cable — fires against the next
        // enumeration's first transfer and eats up to a drain window of its opening payload.
        DRAIN_REQ.reset();
        DRAIN_DONE.reset();
        // `wait_enabled` returns immediately while the endpoint is still up, so a *persistent*
        // driver-level error would hot-spin this loop — and on a cooperative executor that starves
        // the ride loop, freezing the map. Back off a beat, like the BLE CoC accept loop.
        Timer::after_millis(200).await;
    }
}

/// Which final file a map-shaped upload streams into. All three of these hold their format magic
/// back and stream straight into the file they will commit as; everything else stages through
/// `/routes/UPLOAD.TMP` and is copied.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MapTarget {
    /// Not a map at all: the ordinary temp-then-promote path.
    Object,
    /// A single map — `MP{id}.OBM`, id minted at the first byte (#927).
    Map,
    /// One shard of the volume set in flight — `MS{id}S{kk}.OBM` (#1039). The part says which.
    Shard(obc_ble::SetPart),
    /// The set manifest — `MS{id}.OBS`, and the commit point of the whole set (`OBCA_Spec.md` §5.4).
    Manifest,
}

impl MapTarget {
    /// Whether this target withholds its leading four magic bytes (and therefore whether the
    /// stream's file offset starts at [`obc_ble::MAGIC_LEN`] rather than 0).
    fn holds_magic(self) -> bool {
        !matches!(self, MapTarget::Object)
    }
}

/// An upload: sink bulk bytes through the [`Receiver`] into the SD temp, then commit — CRC verify,
/// header validate, atomic promote — and answer with the assigned id.
#[allow(clippy::too_many_arguments)]
async fn run_upload(
    tx: &ControlTx,
    ep: &mut EpOut,
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
    desc: &TransferControl,
    mut rx: Receiver,
    target: MapTarget,
    buf: &mut [u8],
) -> TransferOutcome {
    info!("usb: [bulk] upload start: {} bytes (type {})", desc.total_len, desc.ty.as_u8());
    // A **map** (#927) is the one type that does not stream into `/routes/UPLOAD.TMP`: at hundreds
    // of megabytes the temp-then-copy promote would double both the minutes of writing and the free
    // space required, so a map streams straight into its final `MP{id}.OBM` with its first four bytes
    // — the OBCM magic — withheld here and patched in at commit. `map_id` is the assigned object id,
    // carried in this frame because a map holds no slot in the store to remember it in.
    //
    // A volume set (#1039) is the same shape N+1 times over: every shard and the manifest stream
    // into their own final name with their own magic held back, and `map_id` carries the **set** id
    // the store minted at the first shard. The one difference that matters is *between* the
    // transfers, and it lives in the store's session, not here.
    let holds_magic = target.holds_magic();
    let mut held = obc_ble::HeldMagic::new();
    let mut map_id = 0u16;
    // Open the SD file here — at the first real byte — rather than when the control plane armed it:
    // a host that sends `transferControl` and then never writes holds no storage handle (it only
    // wedges its own one-transfer gate until it unplugs).
    let began = {
        let mut guard = shared.lock().await;
        let opened: Option<u16> = match target {
            // The temp path has no id to hand back; `0` stands in and is never reported (the
            // commit's own `upload_finish` returns the assigned one).
            MapTarget::Object => store.borrow_mut().upload_begin(&mut guard).then_some(0),
            MapTarget::Map => store.borrow_mut().map_upload_begin(&mut guard),
            MapTarget::Shard(part) => store.borrow_mut().set_shard_begin(&mut guard, part),
            MapTarget::Manifest => store.borrow_mut().set_manifest_begin(&mut guard),
        };
        match opened {
            Some(id) => {
                map_id = id;
                // Reserve the whole chain now that the length is known and the file is open, under
                // the lock that opened it. Advisory — a refusal costs throughput, never correctness
                // — and the point is *when* it runs: every cluster it books here is four
                // single-block FAT writes that would otherwise land between the staged bursts.
                store.borrow_mut().upload_reserve(&mut guard, rx.total_len());
                true
            }
            None => false,
        }
    };
    if !began {
        warn!("usb: [bulk] cannot open the upload target — rejecting");
        if holds_magic {
            crate::link::map_transfer_storage_failed();
        }
        // No drain here, and that is the design rather than an omission - see `drain_bulk_out`.
        // The host has not been told anything yet, so it would refill the window as fast as we
        // emptied it, and the answer that makes it stop is the very thing we would be delaying. Its
        // own send loop settles what it queued, and this module's idle loop consumes those bytes.
        close_transfer();
        tx.send_status(transfer_result(rx.object_id(), TransferStatus::Error)).await;
        return TransferOutcome::Answered;
    }
    if holds_magic {
        // Raise the on-glass card now: from here the SD bus is saturated for minutes and the map
        // plane's own reads queue behind this transfer. Unexplained, that reads as a wedged device.
        // A set raises it once per file rather than once per set — the card tracks the transfer in
        // flight, and per-set aggregation needs a UI that knows a set is one map (P4d).
        crate::link::map_transfer_started(rx.total_len());
    }
    // Ask the ride loop for the scratch arena's staging arm (#1146 P2), **after** the card above has
    // been published: the loop's precondition is that the transfer screen is up, and the card is
    // what puts it there, so asking in this order makes the answer available on the loop's very next
    // pass. A refusal is not a failure — the stage degrades to unstaged appends (see [`Stage`]).
    //
    // Only a map-shaped payload asks. Everything else (a route, a trip, a firmware image) is
    // megabytes at most, raises no card, and therefore could never satisfy the `render ⊥ usb`
    // precondition — the staging dial was always about the map upload that saturates the bus for
    // minutes (see [`STAGE_LEN`](crate::usb::STAGE_LEN)'s own note), and asking for the arm while
    // the rider may still be browsing the map would be asking for the wrong thing.
    let staged = holds_magic && crate::usb::request_stage().await;
    // A map's placeholder magic is already on the card (`map_upload_begin` and its set twins), so
    // its payload starts at file offset 4 — the stage needs that or every flush lands misaligned.
    let mut stage = Stage::new(staged, if holds_magic { obc_ble::MAGIC_LEN } else { 0 });
    let started = Instant::now();
    while !rx.is_complete() {
        let n = match select(ep.read(buf), TRANSFER_ABORT.wait()).await {
            Either::First(Ok(n)) if n > 0 => n,
            Either::First(Ok(_)) => continue, // a zero-length packet advances nothing
            Either::First(Err(e)) => {
                // The endpoint failed or was disabled with bytes still expected. Discard the
                // partial; the host re-uploads from the start. There is no live exchange left to
                // answer, and a late `aborted` could be consumed as the next descriptor's result.
                {
                    let mut guard = shared.lock().await;
                    discard_upload(&mut store.borrow_mut(), &mut guard, target, map_id);
                }
                info!("usb: [bulk] upload interrupted ({:?}) — discarded", defmt::Debug2Format(&e));
                close_transfer();
                return TransferOutcome::LinkDropped;
            }
            Either::Second(()) => {
                // The host aborted (op 3). **Drain before anything else**: this is the one moment
                // the host is provably quiet (see `drain_bulk_out`), and the discard below can be a
                // whole set's worth of shard deletes — running those first would spend the host's
                // abort-ack budget before the endpoint had even started emptying. (The arena's
                // staging arm is given back by `close_transfer` either way, so the order does not
                // change how long it is held.)
                drain_bulk_out(ep, buf).await;
                {
                    let mut guard = shared.lock().await;
                    discard_upload(&mut store.borrow_mut(), &mut guard, target, map_id);
                }
                info!("usb: [bulk] upload aborted by the host");
                close_transfer();
                tx.send_status(transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::Answered;
            }
        };
        let consumed = rx.push(&buf[..n]);
        // The receiver's CRC always sees every payload byte; only the *write* skips the held magic.
        let write = if holds_magic { held.feed(&buf[..consumed]) } else { &buf[..consumed] };
        // Into RAM, not onto the card: `stage` appends a batch at a time so the card gets one
        // multi-block burst instead of a CMD24 per 512 B. It is what makes the fork worth having.
        let appended = stage.push(write, store, shared).await;
        if !appended {
            {
                let mut guard = shared.lock().await;
                discard_upload(&mut store.borrow_mut(), &mut guard, target, map_id);
            }
            warn!("usb: [bulk] SD append failed — upload rejected");
            if holds_magic {
                crate::link::map_transfer_storage_failed();
            }
            close_transfer();
            tx.send_status(transfer_result(rx.object_id(), TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
        if holds_magic {
            // Received, not durable: up to one staging half (32 KiB) is still in RAM, and the host
            // may be another `UPLOAD_WINDOW` of chunks ahead of that. The card the rider sees is a
            // liveness indicator, not a commit count — the commit is the terminal result.
            crate::link::map_transfer_progress(rx.committed_offset());
        }
    }
    // The tail. An object almost never ends on a batch boundary, so the last flush is short by
    // definition — and until it lands, those bytes exist only in RAM.
    if !stage.flush(store, shared).await {
        {
            let mut guard = shared.lock().await;
            discard_upload(&mut store.borrow_mut(), &mut guard, target, map_id);
        }
        warn!("usb: [bulk] SD append failed on the final flush — upload rejected");
        if holds_magic {
            crate::link::map_transfer_storage_failed();
        }
        close_transfer();
        tx.send_status(transfer_result(rx.object_id(), TransferStatus::Error)).await;
        return TransferOutcome::Answered;
    }
    // The commit target is the object type: a `fwImage` promotes to /UPDATE.BIN in the card root
    // (staging, no catalog id, no store-revision bump — spec §7.6); a `trip` commits into the trip
    // catalog as `TP{id}.OBT` (bumping the *trip* store, §4.3); a `map` patches the held magic into
    // `MP{id}.OBM` and becomes the selected map (#927); a set's shard and manifest patch theirs into
    // `MS{id}S{kk}.OBM` / `MS{id}.OBS`, the manifest last and only after every shard has landed
    // (#1039); everything else is a route.
    let is_fwimage = desc.ty == ObjectType::FwImage;
    let is_trip = desc.ty == ObjectType::Trip;
    let (id, status) = {
        let mut guard = shared.lock().await;
        let mut st = store.borrow_mut();
        match target {
            // A map shorter than a magic can't reach here — the announce guard rejects anything
            // below a full OBCM header — but the codec is total, so answer `error` rather than
            // fabricate one.
            MapTarget::Map => {
                let status = match held.take() {
                    Some(magic) => st.map_upload_finish(&mut guard, &rx, map_id, magic),
                    None => TransferStatus::Error,
                };
                (map_id, status)
            }
            // A shard's result echoes its **part**, not the set id: that is what the host correlates
            // its slot against (§4.1's "a correlated close"), and it is what says *which* file of
            // the set just committed.
            MapTarget::Shard(part) => {
                let status = match held.take() {
                    Some(magic) => st.set_shard_finish(&mut guard, &rx, map_id, part, magic),
                    None => TransferStatus::Error,
                };
                (part.encode(), status)
            }
            // The manifest's result carries the **assigned set id** — the one moment the set's
            // identity crosses the wire, and the answer to "what did my upload become".
            MapTarget::Manifest => {
                let status = match held.take() {
                    Some(magic) => st.set_manifest_finish(&mut guard, &rx, map_id, magic),
                    None => TransferStatus::Error,
                };
                (map_id, status)
            }
            MapTarget::Object if is_fwimage => (rx.object_id(), st.fwimage_finish(&mut guard, &rx)),
            MapTarget::Object if is_trip => st.upload_finish_trip(&mut guard, &rx, desc.crc32),
            MapTarget::Object => st.upload_finish(&mut guard, &rx, desc.crc32),
        }
    };
    if holds_magic {
        crate::link::map_transfer_ended(Some(status));
    }
    let committed = status == TransferStatus::Committed;
    let elapsed_ms = started.elapsed().as_millis().max(1);
    if committed && target == MapTarget::Map {
        info!("usb: [bulk] map {} is now the selected map — it loads on the next boot", id);
    }
    if committed && target == MapTarget::Manifest {
        info!("usb: [bulk] volume set MS{} committed — it loads on the next boot", id);
    }
    info!(
        "usb: [bulk] upload finished: id {} -> {} ({} bytes in {} ms, ~{} kB/s)",
        id,
        if committed { "committed" } else { "rejected" },
        rx.total_len(),
        elapsed_ms,
        (rx.total_len() as u64) * 1000 / (elapsed_ms * 1024)
    );
    let offset = if committed { rx.total_len() } else { 0 };
    close_transfer();
    tx.send_status(transfer_result_at(id, status, offset)).await;
    // A `fwImage` is a staging slot and a `map` is not a listed object (there is no `mapList`), so
    // neither has a catalog for a peer to re-read — only routes and trips raise `storeChanged`.
    if committed && !is_fwimage && target == MapTarget::Object {
        let ty = if is_trip { ObjectType::Trip } else { ObjectType::Route };
        tx.publish_store_change(store, ty).await;
    }
    TransferOutcome::Answered
}

/// Drop an in-flight upload's partial. Every type but a map drops its `UPLOAD.TMP`; a map's partial
/// **is** its final file with the magic still zeroed, so it is deleted by name — see
/// [`Storage::map_upload_abort`](crate::sd::Storage::map_upload_abort) for why waiting for the boot
/// sweep is not good enough. Clearing the published transfer state closes the on-glass card without
/// a red outcome: an abort or an unplug is something the rider did.
///
/// A **volume set** takes the same argument one level up, and further: an interrupted set is
/// gigabytes, and `OBCA_Spec.md` §5.4 makes a half-set unmountable anyway, so the whole set goes —
/// shards already committed included. The alternative would be a mountable-looking pile of files
/// the next upload could not reuse and no surface could explain. What that costs is resume across a
/// disconnect, and the protocol never offered that (§1 principle 4: transfers restart, never
/// resume); what it does *not* cost is resume within a session, because a failed shard drops only
/// itself (`ObjectStore::set_shard_finish`).
fn discard_upload(store: &mut ObjectStore, shared: &mut crate::SharedStore, target: MapTarget, map_id: u16) {
    match target {
        MapTarget::Object => store.upload_discard(shared),
        MapTarget::Map => {
            crate::link::map_transfer_ended(None);
            if let Some(storage) = &mut shared.storage {
                storage.map_upload_abort(map_id);
            }
        }
        MapTarget::Shard(_) | MapTarget::Manifest => {
            crate::link::map_transfer_ended(None);
            store.set_upload_abort(shared);
        }
    }
}

/// A download: open the source, send the filled announce descriptor on the control plane, then
/// stream the object in max-packet chunks. An abort between (or during) chunks stops cleanly.
async fn run_download(
    tx: &ControlTx,
    ep: &mut EpIn,
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
    desc: &TransferControl,
    buf: &mut [u8],
) -> TransferOutcome {
    // Bind the open's result before matching — a `match store.borrow_mut().…` scrutinee temporary
    // would keep the borrow alive through the error arm's await.
    let fw = identity::firmware_revision();
    let serial = identity::serial_string();
    let diag = crate::link::diag_input(fw.as_str(), serial.as_str(), Instant::now().as_secs() as u32);
    let opened = {
        let mut guard = shared.lock().await;
        store.borrow_mut().download_open(&mut guard, desc, &diag)
    };
    let (mut sender, source) = match opened {
        Ok(open) => open,
        Err(status) => {
            close_transfer();
            tx.send_status(transfer_result(desc.object_id, status)).await;
            return TransferOutcome::Answered;
        }
    };
    // Announce on the control plane as a `downloadAnnounce` status message (protocol v2): the
    // 12-byte descriptor with `total_len` + `crc32` filled in, wrapped in the status envelope, then
    // the bytes flow on the bulk endpoint. Same split as BLE's status-CCCD-then-CoC.
    let announce = sender.announce();
    let total_len = announce.total_len;
    info!("usb: [bulk] download start: {} bytes", total_len);
    tx.send_status(StatusMessage::DownloadAnnounce(announce).encode()).await;

    while !sender.is_complete() {
        if TRANSFER_ABORT.try_take().is_some() {
            {
                let mut guard = shared.lock().await;
                store.borrow_mut().download_close(&mut guard);
            }
            info!("usb: [bulk] download aborted by the host");
            close_transfer();
            tx.send_status(transfer_result_at(desc.object_id, TransferStatus::Aborted, sender.position())).await;
            return TransferOutcome::Answered;
        }
        let n = sender.next_chunk_len(CHUNK_LEN.min(buf.len()));
        let read_ok = {
            let guard = shared.lock().await;
            store.borrow().download_read(&guard, source, sender.position(), &mut buf[..n])
        };
        if !read_ok {
            {
                let mut guard = shared.lock().await;
                store.borrow_mut().download_close(&mut guard);
            }
            warn!("usb: [bulk] SD read failed — download abandoned");
            close_transfer();
            tx.send_status(transfer_result(desc.object_id, TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
        // Race the send against an abort so a host that stops draining the endpoint can still be
        // cancelled promptly. Cancel-safe: the driver's `write` only awaits *before* it pushes into
        // the TX FIFO, so a dropped future has written nothing.
        match select(ep.write(&buf[..n]), TRANSFER_ABORT.wait()).await {
            Either::First(Ok(())) => {}
            Either::First(Err(e)) => {
                info!("usb: [bulk] download send ended: {:?}", defmt::Debug2Format(&e));
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().download_close(&mut guard);
                }
                close_transfer();
                return TransferOutcome::LinkDropped;
            }
            Either::Second(()) => {
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().download_close(&mut guard);
                }
                info!("usb: [bulk] download aborted by the host (mid-send)");
                close_transfer();
                tx.send_status(transfer_result_at(desc.object_id, TransferStatus::Aborted, sender.position())).await;
                return TransferOutcome::Answered;
            }
        }
        sender.advance(n);
    }
    // A USB IN transfer ends on a **short packet**, so an object whose length is an exact multiple
    // of the max packet needs an explicit zero-length packet to mark its end. Today's host reads
    // exactly one max packet per transfer and so never depends on this — but sending it costs one
    // empty transfer and is what lets the host raise its read size later (the throughput lever
    // C3 flagged) without a firmware change. A host that doesn't need it absorbs a ZLP as
    // "not data" and reads on.
    if total_len > 0 && total_len % CHUNK_LEN as u32 == 0 {
        if let Err(e) = ep.write(&[]).await {
            info!("usb: [bulk] terminating ZLP failed: {:?}", defmt::Debug2Format(&e));
        }
    }
    {
        let mut guard = shared.lock().await;
        let mut st = store.borrow_mut();
        st.download_close(&mut guard);
        // A **ride** download that reached completion is the unsynced-guard's commit point (epic
        // #447 P7 / #454). Note this only clears the device's "not synced" delete cue; the durable
        // `synced` **ack** is a separate `ackRides` command the host issues *after* its own fsync,
        // and the browser deliberately never issues it (#894).
        if desc.ty == ObjectType::Ride {
            st.mark_ride_synced(&mut guard, desc.object_id);
        }
    }
    let result = sender.outcome().unwrap(); // complete ⇒ Some
    info!("usb: [bulk] download done: {} bytes", result.committed_offset);
    close_transfer();
    tx.send_status(StatusMessage::TransferResult(result).encode()).await;
    TransferOutcome::Answered
}

/// The echo loopback: receive the announced object and stream it straight back, byte for byte,
/// verifying **one** whole-object CRC-32 at the end — the data plane proven with zero storage. The
/// same harness the BLE side uses for bring-up, which makes it the first thing to run on glass.
async fn run_echo(
    tx: &ControlTx,
    ep_in: &mut EpIn,
    ep_out: &mut EpOut,
    desc: &TransferControl,
    buf: &mut [u8],
) -> TransferOutcome {
    let mut rx = match Receiver::new(desc) {
        Ok(rx) => rx,
        Err(_) => {
            // A nonsensical echo descriptor (the wrong op) — answer error, leave the pipe untouched
            // (no bytes were promised).
            close_transfer();
            tx.send_status(transfer_result(desc.object_id, TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
    };
    info!("usb: [bulk] echo start: {} bytes", rx.total_len());
    let started = Instant::now();
    while !rx.is_complete() {
        // Racing the abort matters more than it looks: the host now follows *every* failed exchange
        // with an `op = 3` and waits for the answer, so an echo with no abort arm would make the
        // host sit out its whole abort budget before it could retry anything.
        let n = match select(ep_out.read(buf), TRANSFER_ABORT.wait()).await {
            Either::First(Ok(0)) => continue, // a zero-length packet is not data
            Either::First(Ok(n)) => n,
            Either::First(Err(e)) => {
                info!("usb: [bulk] echo receive ended: {:?}", defmt::Debug2Format(&e));
                close_transfer();
                return TransferOutcome::LinkDropped;
            }
            Either::Second(()) => {
                info!("usb: [bulk] echo aborted by the host");
                drain_bulk_out(ep_out, buf).await;
                close_transfer();
                tx.send_status(transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::Answered;
            }
        };
        let consumed = rx.push(&buf[..n]);
        if let Err(e) = ep_in.write(&buf[..consumed]).await {
            info!("usb: [bulk] echo send failed: {:?}", defmt::Debug2Format(&e));
            close_transfer();
            return TransferOutcome::LinkDropped;
        }
    }
    // Same end-of-object rule as a download (see `run_download`): the echoed stream is delimited by
    // a short packet, so an object that is an exact multiple of the max packet gets an explicit
    // zero-length one. Keeping both device → host streams delimited identically is what lets the
    // host raise its read size once, for both.
    if rx.total_len() > 0 && rx.total_len() % CHUNK_LEN as u32 == 0 {
        if let Err(e) = ep_in.write(&[]).await {
            info!("usb: [bulk] echo terminating ZLP failed: {:?}", defmt::Debug2Format(&e));
        }
    }
    let result = rx.outcome().unwrap(); // complete ⇒ Some
    let committed = result.status == TransferStatus::Committed;
    let elapsed_ms = started.elapsed().as_millis().max(1);
    info!(
        "usb: [bulk] echo done: {} bytes in {} ms (~{} kB/s) -> {}",
        rx.total_len(),
        elapsed_ms,
        (rx.total_len() as u64) * 1000 / (elapsed_ms * 1024),
        if committed { "committed" } else { "crcMismatch" }
    );
    close_transfer();
    tx.send_status(StatusMessage::TransferResult(result).encode()).await;
    TransferOutcome::Answered
}
