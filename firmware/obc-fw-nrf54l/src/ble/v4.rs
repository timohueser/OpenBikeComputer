//! The **protocol-v4 BLE adapter** (`FLAT_Store_Protocol.md` §5.1) — FS7.5-c3a, epic #1256.
//!
//! An adapter "owns record boundaries, pacing, timeouts, drain, and nothing else. It never parses a
//! payload, never mints an identifier, and never originates a frame" (§5). That sentence is the
//! whole design brief for this module, and everything below is one of those four jobs.
//!
//! ## Where the engine is
//!
//! The engine is **not here**. It lives in [`crate::flat_store::storage_task`], because
//! `obc_link::flat::Store` is synchronous throughout and the card has exactly one writing execution
//! context (#1256's owner ruling). This module reaches it through
//! [`Writer::call`](crate::flat_store::Writer::call), one record at a time, and spends exactly one
//! [`Reply`] slot — [`ENGINE_REPLY`] — because there is exactly one driver and therefore never two
//! concurrently live calls. That is the contract `Writer::call` documents, honoured by construction
//! rather than by convention.
//!
//! ## §5's two obligations, and how each is actually met
//!
//! Neither is met by "one loop sees both channels" — that orders records already *visible* to the
//! loop, and §5 legislates about the *arrival* of an ATT write against the arrival of a CoC SDU,
//! which no loop can observe. Both are met by mechanism:
//!
//! - **Nothing consumed is ever dropped.** The channel is [`split`](L2capChannel::split) and the
//!   reader is its **own future** ([`reader_pump`]), a sibling of the driver rather than a branch of
//!   a `select` the driver re-enters every pass. The old shape raced `receive` against the control
//!   signal, so a `receive` that had already consumed a PDU into [`STREAM_RX`] and suspended inside
//!   flow control — routine when ACL TX grants are exhausted by sensor notifications — lost its
//!   bytes whenever the control side won. Now the reader is dropped only when the **driver returns**,
//!   and the driver returns only to tear the channel down: both split halves go out of scope
//!   together, the channel's refcount drops and the CoC is disconnected. So a frame undelivered at
//!   that moment belongs to a transfer that is over either way. (The weaker claim — "dropped between
//!   receives, with nothing in hand" — is **false**: several driver paths return while the reader is
//!   mid-`receive`. It is the channel's destruction, not the reader's timing, that makes the drop
//!   harmless, and stating it the wrong way would license a refactor that kept the channel alive.) §5: "MUST NOT deliver it,
//!   and MUST NOT drop it."
//! - **Credit is withheld while a frame is held.** [`reader_pump`] receives one record, posts it,
//!   and then waits for [`STREAM_TAKEN`] before receiving again. One frame in flight, never two, and
//!   the CoC's own credit flow control is what pushes back on the peer meanwhile.
//! - **The admission race is closed with a real hold.** A control write and a CoC SDU can genuinely
//!   arrive in either order — the GATT pump may be parked, so "no control record is pending" does
//!   not mean "none was written". When a stream frame arrives and
//!   [`Request::LiveTransfer`](crate::flat_store::Request::LiveTransfer) reports the engine idle,
//!   the frame is **held** for [`ADMISSION_WINDOW`] while the control channel is given its chance.
//!   The frame is not delivered early (the engine would discard it in silence and the upload would
//!   die at offset zero) and not dropped. That query is why `LiveTransfer` exists.
//!
//! ## Cancel stays bilateral during a transfer
//!
//! §3.8 makes cancel bilateral, so the driver checks for a control record **between pump
//! iterations** rather than only when idle. Without that a `CANCEL` sent during a multi-minute `GET`
//! would sit unread until the download it was cancelling had finished, and every control write
//! meanwhile would be refused at the ATT layer. `LIST` and `STATUS` are served mid-download for the
//! same reason: the engine answers them beside a live transfer, and the adapter must not be the
//! thing that does not.
//!
//! ## What a client must do, and the one c3a requirement worth ratifying
//!
//! §5.1's shape is unchanged: read `protocolVersion` (now two bytes, `4`), read `psm`, enable
//! indications on `objectControl`, then write control frames and open the L2CAP CoC for stream
//! records.
//!
//! **In c3a the CoC must be open before a control frame is accepted.** The driver owns the channel,
//! so until one is accepted there is no loop to answer a record — and rather than stage a record
//! that would be answered arbitrarily later, [`stage_control`] refuses the write outright while no
//! driver is live. §5.1 neither requires nor forbids this, and it is client-visible, so it is stated
//! here and in the PR body rather than left to be discovered. Lifting it means splitting the driver
//! from the channel owner; named follow-up.

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};
use nrf_sdc::{self as sdc};
use obc_link::flat::{Admission, Ceilings, Channel, Opcode, Reaction, RequestId};
use trouble_host::prelude::*;

use crate::flat_store::{Outcome, Reply, Request, Writer};

use super::gatt::Server;

/// One indication, bounded. §5.1 makes a response a *confirmed* indication, and trouble-host blocks
/// until the peer's `HandleValueConfirmation` for up to the 30 s ATT transaction timeout — far past
/// anything this device should wait on. The bound is the structural backstop the `status` notify has
/// had since #277: a peer that stops confirming must not park this task past the supervision timeout.
const INDICATE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the GATT pump waits for the driver to consume a staged record.
///
/// Deliberately **not** [`INDICATE_TIMEOUT`]: this bounds a hand-off between two of our own futures
/// on one executor, which is a scheduling latency, while that one bounds a peer's confirmation,
/// which is a radio round trip. They were one constant briefly and that was a coincidence of value,
/// not a shared meaning — so a change to either would silently have moved the other.
const CONTROL_TAKEN_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a stream frame is held while the control channel is given its chance to admit it.
///
/// §3.6 lets a client stream immediately, without waiting for an acceptance, so the first frame of a
/// `PUT` genuinely races its own control write.
///
/// **750 ms is a guess and is labelled as one.** It is meant to cover one GATT event pump pass plus
/// whatever the ride loop is holding the shared store for, and *neither of those has been measured*
/// — the board session re-pins it against the worst observed `shared.lock` hold. Measure, don't
/// theorize, is the standing law here, and a plausible-looking constant with no number behind it is
/// exactly what that law is about; this comment is the marker, not a justification.
///
/// The hold is deliberately **bounded**: a frame still unadmitted when the window closes is handed
/// over anyway, and §3.8's silent discard is then the correct answer, because it genuinely belongs
/// to no transfer the receiver can be sure of. Waiting forever on a `RequestId` that may never be
/// admitted would wedge the channel on a client that simply gave up.
const ADMISSION_WINDOW: Duration = Duration::from_millis(750);

// ══════════════════════════ the buffers ══════════════════════════

/// The reaction buffer, and the ceiling cap both channels are pinned under.
///
/// 256 rather than 245 so that the two §5.1 ceilings — a 244-byte control record at the preferred
/// 247-byte ATT MTU, and a CoC SDU of the packet pool's MTU − 6 — both fit with the slack a link
/// that negotiates *upward* would need. [`Ceilings::for_ble`] clamps to it either way, so the buffer
/// is the authority and not merely the usual case.
const OUT_LEN: usize = 256;

/// Where a reaction's bytes land. Lent to the engine for the length of one call; see [`Lane`].
static mut OUT: [u8; OUT_LEN] = [0; OUT_LEN];

/// One control record, copied out of the ATT write so it can cross the queue as `'static`.
static mut CONTROL_RX: [u8; OUT_LEN] = [0; OUT_LEN];

/// One stream record — **the frame §5's hold is about**. While it is occupied [`reader_pump`]
/// receives nothing further, which is the credit withholding §5 asks for.
static mut STREAM_RX: [u8; DefaultPacketPool::MTU] = [0; DefaultPacketPool::MTU];

/// BLE's one engine reply slot. One driver, one live call — see the module docs.
static ENGINE_REPLY: Reply = Signal::new();

/// A control record the GATT task has staged, as its length in [`CONTROL_RX`].
static CONTROL_IN: Signal<CriticalSectionRawMutex, usize> = Signal::new();

/// The engine has consumed [`CONTROL_RX`] and the GATT task may stage another.
static CONTROL_TAKEN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// A received stream record, as its length in [`STREAM_RX`].
static STREAM_IN: Signal<CriticalSectionRawMutex, usize> = Signal::new();

/// The driver has finished with [`STREAM_RX`] and [`reader_pump`] may receive again.
static STREAM_TAKEN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// True while a driver is running and able to answer a control record.
static DRIVER_READY: AtomicBool = AtomicBool::new(false);

/// The adapter's resident cost, for the budget table in `main.rs`.
pub(crate) const RESIDENT_BYTES: usize = OUT_LEN + OUT_LEN + DefaultPacketPool::MTU;

// ══════════════════════════ the control channel ══════════════════════════

/// What [`stage_control`] did with an `objectControl` write, in the terms the ATT layer answers in.
pub(crate) enum Staging {
    /// The record is staged for the driver.
    Taken,
    /// Longer than this link's record bound, or empty.
    BadLength,
    /// A previous record is still un-taken, or no driver is live to take one.
    Unavailable,
}

/// **Stage one `objectControl` write for the driver** — called from the GATT event pump, inside the
/// write's `with_data` closure.
///
/// It does **not** parse. The length check is a buffer bound, not a protocol opinion; every verdict
/// about these bytes is the engine's.
pub(crate) fn stage_control(record: &[u8]) -> Staging {
    if record.is_empty() || record.len() > OUT_LEN {
        warn!("ble: [v4] objectControl write is {} B — outside this link's record bound", record.len());
        return Staging::BadLength;
    }
    if !DRIVER_READY.load(Ordering::Relaxed) {
        // Staging a record no loop will answer would leave the client waiting on an indication that
        // arrives whenever a channel happens to open. Refusing now is the honest answer, and it is
        // the c3a requirement the module docs state: open the CoC first.
        warn!("ble: [v4] objectControl write before the stream channel is up — refused");
        return Staging::Unavailable;
    }
    if CONTROL_IN.signaled() {
        warn!("ble: [v4] a control record is still un-taken — refusing rather than overwriting it");
        return Staging::Unavailable;
    }
    // SAFETY: the driver has released `CONTROL_RX` — `CONTROL_IN` being clear is exactly that fact,
    // because the driver signals `CONTROL_TAKEN` only after the engine has consumed the record, and
    // it takes `CONTROL_IN` before that. Both this and the driver are cooperative futures on the one
    // thread-mode executor and neither holds the buffer across an `await`.
    unsafe {
        let staging = &mut *core::ptr::addr_of_mut!(CONTROL_RX);
        staging[..record.len()].copy_from_slice(record);
    }
    CONTROL_TAKEN.reset();
    CONTROL_IN.signal(record.len());
    Staging::Taken
}

/// Wait until the engine has consumed the staged record, so the next write cannot race it.
pub(crate) async fn control_taken() {
    if with_timeout(CONTROL_TAKEN_TIMEOUT, CONTROL_TAKEN.wait()).await.is_err() {
        warn!("ble: [v4] the driver did not take a staged control record in time");
    }
}

// ══════════════════════════ the lane ══════════════════════════

/// The adapter's half of one round trip to the engine: the buffer it lends the engine, and nothing
/// else.
///
/// The buffer is lent rather than copied — it crosses the queue inside the request and comes back
/// inside the answer — so a `None` here means a previous call's future was dropped between the send
/// and the answer, which a supervision timeout during a long finalizing commit can genuinely do.
/// [`Lane::reclaim`] is how that is recovered, and it is recovered *provably* rather than by
/// scheduling luck: see there.
pub(crate) struct Lane {
    out: Option<&'static mut [u8]>,
}

/// True once [`lane`] has built [`LANE`].
static LANE_BUILT: AtomicBool = AtomicBool::new(false);

/// The lane itself, in `.bss`.
static mut LANE: core::mem::MaybeUninit<Lane> = core::mem::MaybeUninit::uninit();

/// **The one lane, for the life of the image.**
///
/// Two properties, and both were bought by review findings rather than chosen up front:
///
/// - **The buffer is taken once.** A `Lane` built per connection could mint a second `&'static mut`
///   to [`OUT`] while a dropped call still had the first parked in [`ENGINE_REPLY`]. Here there is
///   one `Lane`, its state lives in `.bss`, and a connection that finds `out: None` calls
///   [`Lane::reclaim`] rather than conjuring a second reference.
/// - **`ble::run` holds nothing.** The lane is reached from inside [`serve_objects`], so the value
///   never becomes a local of the BLE task. That is not tidiness: carried as a local across that
///   task's awaits it cost the poll frame **8,628 B** — 1,036 → 9,664 — by changing the coroutine's
///   liveness enough that LLVM stopped sinking `init_resources`' and `init_server`'s construction
///   temporaries out of the frame. Eight bytes of value, three orders of magnitude of frame; the
///   #677/#1084 trap exactly, and the reason this returns a reference from a slot.
///
/// # Safety
/// One caller at a time: [`serve_objects`] is the only one and there is one BLE connection. The
/// `&'static mut` it hands out is re-derived per call, and no two live at once.
#[inline(never)]
pub(crate) fn lane() -> &'static mut Lane {
    // SAFETY: sole writer of `LANE`; the flag makes the build happen exactly once, before any driver
    // exists, and `Lane` has no `Drop`.
    unsafe {
        if !LANE_BUILT.swap(true, Ordering::Relaxed) {
            let out: Option<&'static mut [u8]> =
                Some(core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(OUT).cast::<u8>(), OUT_LEN));
            return crate::init_static(core::ptr::addr_of_mut!(LANE), Lane { out });
        }
        &mut *(*core::ptr::addr_of_mut!(LANE)).as_mut_ptr()
    }
}

impl Lane {
    /// **Recover the buffer from a call whose future was dropped.**
    ///
    /// Sound because of the queue's own ordering rather than a timing argument: requests are served
    /// FIFO by one consumer, so once *any* later call has been answered, every earlier job has run
    /// and an orphaned answer can only be sitting in [`ENGINE_REPLY`]. The caller therefore reclaims
    /// **before** issuing the next call — `Writer::call` discards a mismatched reply, which would
    /// throw the buffer away with it.
    ///
    /// **That inference needs one more thing than FIFO service: the later call must have been
    /// *enqueued* after the orphan.** It holds while BLE is the only sender, which is true today.
    /// FS8's ride journal and c3b's USB adapter both add senders, and each owes this argument a
    /// re-establishment — a second sender can interleave a job between the orphan and the reclaiming
    /// call, and then "a later answer arrived" no longer implies "the orphan has been served".
    fn reclaim(&mut self) {
        if self.out.is_some() {
            return;
        }
        match ENGINE_REPLY.try_take() {
            Some((_, Ok(Outcome::Reacted { out, .. }))) => {
                info!("ble: [v4] reclaimed the reaction buffer from an abandoned call");
                self.out = Some(out);
            }
            Some(_) => warn!("ble: [v4] an abandoned call left no buffer to reclaim"),
            None => warn!("ble: [v4] the reaction buffer is outstanding — this link cannot serve"),
        }
    }

    /// Hand one request to the engine and take the buffer back with its answer.
    async fn call(&mut self, writer: &Writer, make: impl FnOnce(&'static mut [u8]) -> Request) -> Option<Reaction> {
        let out = self.out.take()?;
        match writer.call(make(out), &ENGINE_REPLY).await {
            Ok(Outcome::Reacted { reaction, out }) => {
                self.out = Some(out);
                Some(reaction)
            }
            // `serve` answers these three requests with `Reacted` and nothing else, so the buffer is
            // gone only if that stopped being true. Report rather than panic: this is a radio task.
            _ => {
                warn!("ble: [v4] the engine answered a record with the wrong shape — lane closed");
                None
            }
        }
    }

    /// The bytes a [`Reaction::Send`] named.
    fn sent(&self, len: usize) -> &[u8] {
        match &self.out {
            Some(out) => &out[..len.min(out.len())],
            None => &[],
        }
    }
}

/// **Release whatever the engine still holds for a link that has gone away** (§3.8's third form of
/// cancel).
///
/// Called from the connection teardown in [`super::run`] rather than from the driver, because a peer
/// disconnect *drops* the driver: its own teardown is exactly the code that does not run when the
/// thing it cleans up after has happened.
pub(crate) async fn release_engine(writer: &Writer) {
    static TEARDOWN_REPLY: Reply = Signal::new();
    if writer.call(Request::LinkLost, &TEARDOWN_REPLY).await.is_err() {
        warn!("ble: [v4] the engine refused a link-lost teardown");
    }
    DRIVER_READY.store(false, Ordering::Relaxed);
}

// ══════════════════════════ the driver ══════════════════════════

/// **The engine driver.** Replaces the v1 `serve_coc`: the CoC no longer carries an unframed byte
/// pipe, it carries §3.8 stream records, one per SDU.
pub(crate) async fn serve_objects(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) -> ! {
    let Some(writer) = crate::flat_store::writer() else {
        // Unreachable since c3a spawns the storage task on every card, and reported rather than
        // unwrapped because a radio task must never be why the board panics.
        warn!("ble: [v4] the store's write half is not armed — object service is down this boot");
        core::future::pending().await
    };
    // Reached here rather than passed in: see `lane`. A local of `ble::run` costs that task's poll
    // frame 8,628 B.
    let lane = lane();
    let listener = L2capChannel::listen(stack, conn.raw());
    loop {
        let ch = match listener.accept(&L2capChannelConfig::default()).await {
            Ok(ch) => ch,
            Err(e) => {
                warn!("ble: [v4] accept failed: {:?}", defmt::Debug2Format(&e));
                Timer::after_millis(200).await;
                continue;
            }
        };
        // The CoC requires an encrypted link. In practice a peer cannot reach here unencrypted —
        // `psm` and `objectControl` are both `authenticated` — but one that guessed the SPSM must
        // still be turned away.
        if !matches!(conn.raw().security_level(), Ok(level) if level.encrypted()) {
            warn!("ble: [v4] channel opened on an unencrypted link — refusing (S0 §8)");
            let (mut w, _r) = ch.split();
            w.disconnect();
            continue;
        }
        let (mut writer_half, mut reader) = ch.split();
        // §5.1: "The control ceiling is `ATT_MTU - 3`"; the stream ceiling is the CoC SDU; both are
        // clamped to what this adapter can hold. The arithmetic is `obc-link`'s, where it is pinned
        // by tests, rather than re-derived here — and `None` is §5.1's refusal of a link below the
        // protocol floor.
        let att_mtu = usize::from(conn.raw().att_mtu());
        let Some(ceilings) = Ceilings::for_ble(att_mtu, usize::from(reader.mtu()), OUT_LEN) else {
            warn!("ble: [v4] link below the protocol floor (att {}, sdu {}) — closing", att_mtu, reader.mtu());
            writer_half.disconnect();
            continue;
        };
        info!("ble: [v4] channel up — control {} B, stream {} B", ceilings.control(), ceilings.stream());
        // A dropped call from the previous link parks the buffer in the reply slot; take it back
        // before the first call of this one, because `Writer::call` would discard it.
        lane.reclaim();
        if lane.call(&writer, |out| Request::Pump { out }).await.is_none() {
            // The lane has no buffer and cannot get one. Serving would mean answering nothing.
            warn!("ble: [v4] no reaction buffer — refusing this channel rather than half-serving it");
            writer_half.disconnect();
            continue;
        }
        if writer.call(Request::LinkUp { ceilings }, &ENGINE_REPLY).await.is_err() {
            warn!("ble: [v4] the engine refused the link — closing the channel");
            writer_half.disconnect();
            continue;
        }
        // Both signals are level state from the previous channel's point of view; clear them so a
        // stale length cannot be read against this channel's buffers.
        CONTROL_IN.reset();
        STREAM_IN.reset();
        STREAM_TAKEN.reset();
        DRIVER_READY.store(true, Ordering::Relaxed);
        // **The reader is a sibling, not a branch**, and that is what stops a consumed PDU being
        // thrown away. It is dropped only when the *driver* returns, and the driver returns only to
        // tear this channel down: both split halves fall out of scope together, so the channel is
        // disconnected in the same breath. A frame still in `STREAM_RX` then belongs to a transfer
        // that is over regardless. What must never happen again is the old shape — dropping the
        // reader while the channel lives on — which is why this is a `select` over two siblings and
        // not a `select` the driver re-enters every pass.
        let outcome =
            match select(driver(&writer, lane, stack, server, conn, &mut writer_half), reader_pump(stack, &mut reader))
                .await
            {
                Either::First(reason) => reason,
                Either::Second(never) => never,
            };
        // §3.8's third form of cancel — **through `release_engine`, on its own reply slot.** Calling
        // it on `ENGINE_REPLY` was a self-inflicted trap: the `select` above can drop the driver mid
        // `Lane::call`, so the orphaned answer — the one carrying the reaction buffer — is still in
        // that slot, and `Writer::call` discards a mismatched reply. This teardown would therefore
        // have thrown `OUT` away, `reclaim` would have found the slot empty, and every later CoC
        // accept would be refused for the rest of the boot. `TEARDOWN_REPLY` leaves the orphan where
        // `reclaim` can find it, and this round trip is itself the later-served call the FIFO
        // argument needs. It clears `DRIVER_READY` too.
        release_engine(&writer).await;
        info!("ble: [v4] channel down ({}) — engine released, re-accepting", outcome);
    }
}

/// Receive one stream record at a time and hand it over, waiting to be told the driver is done
/// before receiving another.
///
/// **This is where §5's credit withholding lives.** One record is outstanding at a time, so the
/// CoC's own credit flow control pushes back on the peer while a frame is held — there is no second
/// buffer for a second frame to go to, and there does not need to be.
async fn reader_pump(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    reader: &mut L2capChannelReader<'_, DefaultPacketPool>,
) -> &'static str {
    loop {
        // SAFETY: the borrow is taken fresh per receive and released before `STREAM_IN` is signalled;
        // the driver reads the buffer only between that signal and `STREAM_TAKEN`, which is exactly
        // the window in which this future holds no reference to it.
        let rx = unsafe { &mut *core::ptr::addr_of_mut!(STREAM_RX) };
        let received = match reader.receive(stack, rx).await {
            Ok(0) => continue,
            Ok(n) => n,
            Err(_) => return "channel",
        };
        STREAM_TAKEN.reset();
        STREAM_IN.signal(received);
        STREAM_TAKEN.wait().await;
    }
}

/// The driver loop: take whichever input is ready, hand it to the engine, and pump until quiet.
#[allow(clippy::too_many_arguments)]
async fn driver(
    writer: &Writer,
    lane: &mut Lane,
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    tx: &mut L2capChannelWriter<'_, DefaultPacketPool>,
) -> &'static str {
    // Owned by the driver, so it is per channel by construction rather than by a `reset` someone has
    // to remember. See `Admission` for why it remembers *which* transfer was admitted.
    let mut admission = Admission::new();
    loop {
        // Control first when both are ready: a `CANCEL` or a `LIST` must not queue behind a stream
        // frame the engine may be about to refuse anyway.
        let reaction = match select(CONTROL_IN.wait(), STREAM_IN.wait()).await {
            Either::First(len) => match control_record(writer, lane, &mut admission, len).await {
                Some(reaction) => reaction,
                None => return "lane",
            },
            Either::Second(len) => match stream_record(writer, lane, &mut admission, len).await {
                Some(reaction) => reaction,
                None => return "lane",
            },
        };
        if let Some(reason) = pump(writer, lane, &mut admission, stack, server, conn, tx, reaction).await {
            return reason;
        }
    }
}

/// Hand the staged control record to the engine, then release [`CONTROL_RX`].
async fn control_record(writer: &Writer, lane: &mut Lane, admission: &mut Admission, len: usize) -> Option<Reaction> {
    // SAFETY: `CONTROL_IN` has been taken, so `stage_control` will not write this buffer until
    // `CONTROL_TAKEN` is signalled below.
    let record: &'static [u8] =
        unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(CONTROL_RX).cast::<u8>(), len) };
    // PUT is the one control operation whose successful answer is deliberately deferred until its
    // stream commits. Latch that admission at the hand-off which created it, while the exact
    // control RequestId is still in hand. Waiting until the first CoC SDU to rediscover it makes
    // the hot path depend on a second queue round trip across the control/stream race; on glass that
    // left a successfully written PUT looking unadmitted and held every SDU for 750 ms until the
    // controller packet pool exhausted.
    let put_request = (len >= 16 && record.get(5).copied() == Some(Opcode::Put as u8))
        .then(|| RequestId(u32::from_le_bytes([record[12], record[13], record[14], record[15]])));
    let reaction = lane.call(writer, |out| Request::Control { record, out }).await;
    if let Some(request) = put_request {
        let live = live_transfer(writer).await;
        if admission.observed(request, live) {
            warn!(
                "ble: [v4] PUT request {} was not admitted (engine live request {})",
                request.0,
                live.map_or(0, |id| id.0)
            );
        } else {
            info!("ble: [v4] PUT request {} admitted for stream", request.0);
        }
    }
    // **Released here and not a statement earlier.** The engine consumes `CONTROL_RX` synchronously
    // inside the storage task's `serve`, which is over by the time this call answers — so this is
    // the first instant at which the GATT task may stage another record without writing under a
    // borrow the queue still holds.
    CONTROL_TAKEN.signal(());
    reaction
}

/// Hand the received stream record to the engine, holding it first if nothing is admitted yet.
async fn stream_record(writer: &Writer, lane: &mut Lane, admission: &mut Admission, len: usize) -> Option<Reaction> {
    // §5's admission hold. A stream frame that belongs to a `PUT` whose control write has not
    // reached the engine yet would be discarded in silence (§3.8) and the upload would die at offset
    // zero — and "no control record is pending" does *not* mean none was written, because the GATT
    // pump may be parked on the shared store. So: ask the engine, and if this frame is not admitted,
    // hold it while the control channel is given its window. The reader is already withholding
    // credit, so holding costs nothing but the wait.
    //
    // The query is skipped for a frame that continues the transfer the engine last confirmed —
    // `Admission` is keyed on the `RequestId` §3.8 puts in the frame header, so a steady-state
    // upload pays no round trips *and* the leading frame of the **next** transfer on this channel is
    // still queried. A plain "something was admitted" flag got the first half right and the second
    // half catastrophically wrong; `Admission`'s own tests carry that case.
    //
    // Reading four bytes of the §3.8 frame header is not "parsing a payload" (§5): it is the record
    // boundary information the binding is explicitly responsible for. A record too short to carry
    // one is not decoded here — it goes to the engine, which owns that refusal.
    let frame_id = (len >= 4).then(|| {
        // SAFETY: as below — the driver reads this buffer only between `STREAM_IN` and
        // `STREAM_TAKEN`, and `reader_pump` holds no reference across that window.
        let header = unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(STREAM_RX).cast::<u8>(), 4) };
        RequestId(u32::from_le_bytes([header[0], header[1], header[2], header[3]]))
    });
    if let Some(frame_id) = frame_id {
        if admission.needs_query(frame_id) {
            let live = live_transfer(writer).await;
            if admission.observed(frame_id, live) {
                if let Either::First(control_len) = select(CONTROL_IN.wait(), Timer::after(ADMISSION_WINDOW)).await {
                    let reaction = control_record(writer, lane, admission, control_len).await?;
                    // Admission answered; the held frame goes next round, still un-dropped.
                    STREAM_IN.signal(len);
                    return Some(reaction);
                }
                warn!(
                    "ble: [v4] stream request {} is not engine request {} — delivering after the hold window",
                    frame_id.0,
                    live.map_or(0, |id| id.0)
                );
            }
        }
    }
    // SAFETY: the driver reads this buffer only between `STREAM_IN` and `STREAM_TAKEN`, and
    // `reader_pump` holds no reference to it across that window.
    let record: &'static [u8] =
        unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(STREAM_RX).cast::<u8>(), len) };
    let reaction = lane.call(writer, |out| Request::Stream { record, out }).await;
    // The engine has consumed the bytes; the reader may take the next SDU.
    STREAM_TAKEN.signal(());
    reaction
}

/// Whether a transfer owns the engine right now.
///
/// Deliberately **not** a `Lane` call: it borrows no buffer, so it cannot be the thing that loses
/// one, and it is the only read on the write queue.
async fn live_transfer(writer: &Writer) -> Option<obc_link::flat::RequestId> {
    // Its own slot rather than `ENGINE_REPLY`: one slot per *concurrently live* call is the
    // contract, and although this query never overlaps a `Lane::call` today, sharing the slot would
    // make that a fact about call ordering rather than about the types. The engine answers without
    // touching the card, so the round trip is one executor hop.
    static LIVE_REPLY: Reply = Signal::new();
    match writer.call(Request::LiveTransfer, &LIVE_REPLY).await {
        Ok(Outcome::Live(live)) => live,
        _ => None,
    }
}

/// Send what the reaction names, then pump until the engine goes quiet — servicing control records
/// in between, because §3.8's cancel is bilateral and a download must not deafen the control channel.
///
/// Returns `Some(reason)` when the channel should be torn down, `None` when the engine went quiet.
#[allow(clippy::too_many_arguments)]
async fn pump(
    writer: &Writer,
    lane: &mut Lane,
    admission: &mut Admission,
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    tx: &mut L2capChannelWriter<'_, DefaultPacketPool>,
    first: Reaction,
) -> Option<&'static str> {
    let mut reaction = first;
    loop {
        match reaction {
            Reaction::Idle => return None,
            Reaction::Close(channel) => {
                // §3.1's unanswerable record: emit nothing and close that record stream.
                //
                // **Today both arms end the driver, and ending the driver drops both split halves —
                // so the stream channel goes down either way and only the log differs.** Honouring
                // the distinction for real means keeping the driver alive on a control-side close,
                // which needs a control channel that can be closed independently of the CoC; BLE has
                // no such thing (the ATT link *is* the connection). The match is kept because the
                // engine's answer carries the channel and discarding it here would hide that, but
                // the comment says what the code does rather than what the shape suggests.
                return match channel {
                    Channel::Control => {
                        warn!("ble: [v4] unanswerable control record — dropping the link");
                        Some("control-closed")
                    }
                    Channel::Stream => {
                        warn!("ble: [v4] unanswerable stream record — closing the stream channel");
                        tx.disconnect();
                        Some("stream-closed")
                    }
                };
            }
            Reaction::Send { channel, len } => {
                let ok = match channel {
                    // §5.1: one confirmed indication carries the response.
                    Channel::Control => {
                        match with_timeout(
                            INDICATE_TIMEOUT,
                            server.obc.object_control.indicate_raw(conn, lane.sent(len), false),
                        )
                        .await
                        {
                            Ok(Ok(())) => true,
                            Ok(Err(e)) => {
                                warn!("ble: [v4] indicate failed: {:?}", defmt::Debug2Format(&e));
                                false
                            }
                            Err(_) => {
                                warn!("ble: [v4] indicate timed out — abandoning");
                                false
                            }
                        }
                    }
                    Channel::Stream => tx.send(stack, lane.sent(len)).await.is_ok(),
                };
                if !ok {
                    return Some("send");
                }
            }
            Reaction::SendAndReboot { .. } => {
                // §4 step 4. `ARM` is refused by this build's policy (`flat_store::BoardPolicy`), so
                // the engine never reaches this reaction; it is answered rather than ignored so that
                // the slice which fills the policy finds a driver that already handles it.
                warn!("ble: [v4] the engine asked for a reboot, which this build cannot arm");
                return Some("arm");
            }
        }
        // **Between iterations, not only when idle.** A `GET` streams for as long as the object is
        // large, and §3.8's cancel is bilateral: a `CANCEL` written mid-download has to reach the
        // engine while there is still something to cancel.
        if let Some(len) = CONTROL_IN.try_take() {
            match control_record(writer, lane, admission, len).await {
                Some(next) => reaction = next,
                None => return Some("lane"),
            }
            continue;
        }
        match lane.call(writer, |out| Request::Pump { out }).await {
            Some(next) => reaction = next,
            None => return Some("lane"),
        }
    }
}
