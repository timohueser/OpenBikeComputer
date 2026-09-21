//! The protocol-v4 BLE adapter. An adapter owns record boundaries, pacing, timeouts and drain. It
//! never parses a payload, never mints an identifier and never originates a frame.
//!
//! The engine is not here. It lives in [`crate::flat_store::storage_task`], because
//! `obc_link::flat::Store` is synchronous and the card has exactly one writing execution context.
//! This module reaches it through [`Writer::call`](crate::flat_store::Writer::call), one record at a
//! time, and spends one [`Reply`] slot ([`ENGINE_REPLY`]), because one driver can never have two
//! calls live at once.
//!
//! Three mechanisms carry the protocol's ordering rules:
//!
//! - Nothing consumed is dropped. The channel is [`split`](L2capChannel::split) and the reader is
//!   its own future ([`reader_pump`]), a sibling of the driver, so a `receive` that already consumed
//!   a PDU into [`STREAM_RX`] cannot lose it to a control-side wake. The reader is dropped only when
//!   the driver returns, and the driver returns only to tear the channel down.
//! - Byte-stream fragments are reassembled before delivery. CoreBluetooth's output write can accept
//!   fewer bytes than requested, so the reader uses the record header length to recover a complete
//!   record, and waits for [`STREAM_TAKEN`] before it assembles another.
//! - The admission race is closed with a hold. A control write and a CoC SDU can arrive in either
//!   order, and the GATT pump may be parked, so "no control record is pending" does not mean none
//!   was written. When a stream frame arrives and the engine reports itself idle, the frame is held
//!   for [`ADMISSION_WINDOW`] rather than delivered early, which would discard it in silence, or
//!   dropped.
//!
//! Cancel is bilateral, so the driver looks for a control record between pump iterations and not
//! only when it is idle. `LIST` and `STATUS` are served mid-download for the same reason.
//!
//! A client reads `protocolVersion`, reads `psm`, enables indications on `objectControl`, then
//! writes control frames and opens the L2CAP CoC for stream records. The CoC must be open before a
//! control frame is accepted, because the driver owns the channel: until one is accepted there is no
//! loop to answer a record, and [`stage_control`] refuses the write.

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};
use nrf_sdc::{self as sdc};
use obc_link::flat::wire::{StreamAssembly, StreamRecordAssembler};
use obc_link::flat::{Admission, Ceilings, Channel, Link, Reaction, RequestId};
use trouble_host::prelude::*;

use crate::flat_store::{Lane, Outcome, Reply, Request, Writer};

use super::gatt::Server;

/// One indication, bounded. A response is a confirmed indication, and trouble-host blocks until the
/// peer's `HandleValueConfirmation` for up to the 30 s ATT transaction timeout. A peer that stops
/// confirming must not park this task past the supervision timeout.
const INDICATE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long the GATT pump waits for the driver to consume a staged record. Not
/// [`INDICATE_TIMEOUT`]: this bounds a hand-off between two of our own futures on one executor,
/// while that one bounds a peer's confirmation, which is a radio round trip.
const CONTROL_TAKEN_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a stream frame is held while the control channel is given its chance to admit it. A
/// client may stream immediately, without waiting for an acceptance, so the first frame of a `PUT`
/// races its own control write.
///
/// 750 ms is a guess and is labelled as one: it must cover one GATT event pump pass plus whatever
/// the ride loop holds the shared store for, and neither is measured yet. The hold is bounded on
/// purpose, because waiting forever on a `RequestId` that may never be admitted would wedge the
/// channel on a client that simply gave up.
const ADMISSION_WINDOW: Duration = Duration::from_millis(750);

/// The reaction buffer, and the ceiling cap both channels are pinned under. 256 rather than 245, so
/// that a 244-byte control record at the preferred 247-byte ATT MTU and a CoC SDU of the packet
/// pool's MTU − 6 both fit, with slack for a link that negotiates upward. [`Ceilings::for_ble`]
/// clamps to it either way, so the buffer is the authority.
const OUT_LEN: usize = 256;

/// Where a reaction's bytes land. Lent to the engine for the length of one call; see [`Lane`].
static mut OUT: [u8; OUT_LEN] = [0; OUT_LEN];

/// One control record, copied out of the ATT write so it can cross the queue as `'static`.
static mut CONTROL_RX: [u8; OUT_LEN] = [0; OUT_LEN];

/// One stream record. While it is occupied [`reader_pump`] receives nothing further, which is the
/// credit withholding the protocol asks for.
static mut STREAM_RX: [u8; DefaultPacketPool::MTU] = [0; DefaultPacketPool::MTU];

/// BLE's one engine reply slot. One driver, one live call.
static ENGINE_REPLY: Reply = Signal::new();

/// A control record the GATT task has staged, as its length in [`CONTROL_RX`].
static CONTROL_IN: Signal<CriticalSectionRawMutex, usize> = Signal::new();

/// The engine has consumed [`CONTROL_RX`] and the GATT task may stage another.
static CONTROL_TAKEN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// True from the instant the GATT task starts writing [`CONTROL_RX`] until the driver has finished
/// the engine call that borrows it. [`CONTROL_IN`] cannot carry this by itself: the driver clears
/// that signal when it takes the length, before the engine has consumed the bytes, so a second ATT
/// write in that interval would overwrite a live borrow. Only consumption or a FIFO-ordered link
/// teardown releases this gate.
static CONTROL_BUSY: AtomicBool = AtomicBool::new(false);

/// A received stream record, as its length in [`STREAM_RX`].
static STREAM_IN: Signal<CriticalSectionRawMutex, usize> = Signal::new();

/// The driver has finished with [`STREAM_RX`] and [`reader_pump`] may receive again.
static STREAM_TAKEN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

static DRIVER_READY: AtomicBool = AtomicBool::new(false);

/// The adapter's resident cost, for the resource report in `main.rs`.
pub(crate) const RESIDENT_BYTES: usize = OUT_LEN + OUT_LEN + DefaultPacketPool::MTU;

/// What [`stage_control`] did with an `objectControl` write, in the terms the ATT layer answers in.
pub(crate) enum Staging {
    Taken,
    /// Longer than this link's record bound, or empty.
    BadLength,
    /// A previous record is still un-taken, or no driver is live to take one.
    Unavailable,
}

/// Stage one `objectControl` write for the driver, called from the GATT event pump inside the
/// write's `with_data` closure. It does not parse: the length check is a buffer bound, not a
/// protocol opinion, and every verdict about these bytes is the engine's.
pub(crate) fn stage_control(record: &[u8]) -> Staging {
    if record.is_empty() || record.len() > OUT_LEN {
        warn!("ble: [v4] objectControl write is {} B — outside this link's record bound", record.len());
        return Staging::BadLength;
    }
    if !DRIVER_READY.load(Ordering::Relaxed) {
        // Staging a record no loop will answer would leave the client waiting on an indication that
        // arrives whenever a channel happens to open. Refusing now is the honest answer.
        warn!("ble: [v4] objectControl write before the stream channel is up — refused");
        return Staging::Unavailable;
    }
    if CONTROL_BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        warn!("ble: [v4] a control record is still owned by the driver — refusing rather than overwriting it");
        return Staging::Unavailable;
    }
    // SAFETY: the successful `CONTROL_BUSY` transition owns `CONTROL_RX` until `control_record`
    // releases it after the engine call. The atomic is the explicit ownership authority, so taking
    // `CONTROL_IN` cannot make the buffer appear free early.
    unsafe {
        let staging = &mut *core::ptr::addr_of_mut!(CONTROL_RX);
        staging[..record.len()].copy_from_slice(record);
    }
    CONTROL_TAKEN.reset();
    CONTROL_IN.signal(record.len());
    Staging::Taken
}

/// Wait until the engine has consumed the staged record. A timeout releases only this GATT task's
/// wait, not [`CONTROL_BUSY`], so the next write is refused until the driver really consumes the
/// record or link teardown retires it.
pub(crate) async fn control_taken() {
    if with_timeout(CONTROL_TAKEN_TIMEOUT, CONTROL_TAKEN.wait()).await.is_err() {
        warn!("ble: [v4] the driver did not take a staged control record in time");
    }
}

/// The one lane, for the life of the image. The type, the buffer lending and the orphan recovery
/// are [`crate::flat_store::Lane`]'s, shared with the cable's adapter. What is this link's is the
/// buffer ([`OUT`], sized to the BLE ceilings) and the reply slot ([`ENGINE_REPLY`]).
///
/// Reached from inside [`serve_objects`] rather than passed in: carried as a local across
/// `ble::run`'s awaits it cost that task's poll frame 8,628 B, because the changed coroutine
/// liveness stopped LLVM sinking `init_resources`' and `init_server`'s construction temporaries out
/// of the frame.
///
/// # Safety
/// One caller: [`serve_objects`], and there is one BLE connection.
#[inline(never)]
pub(crate) fn lane() -> &'static mut Lane {
    // SAFETY: sole writer of `LANE`; the flag makes the build happen exactly once, before any driver
    // exists, and `Lane` has no `Drop`.
    unsafe {
        if !LANE_BUILT.swap(true, Ordering::Relaxed) {
            let out: &'static mut [u8] =
                core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(OUT).cast::<u8>(), OUT_LEN);
            return crate::init_static(core::ptr::addr_of_mut!(LANE), Lane::new(out, &ENGINE_REPLY, "ble"));
        }
        &mut *(*core::ptr::addr_of_mut!(LANE)).as_mut_ptr()
    }
}

static LANE_BUILT: AtomicBool = AtomicBool::new(false);

static mut LANE: core::mem::MaybeUninit<Lane> = core::mem::MaybeUninit::uninit();

/// Release whatever the engine still holds for a link that has gone away. Called from the connection
/// teardown in [`super::run`] rather than from the driver, because a peer disconnect drops the
/// driver: its own teardown is exactly the code that does not run.
pub(crate) async fn release_engine(writer: &Writer) {
    static TEARDOWN_REPLY: Reply = Signal::new();
    // Close admission before yielding. A GATT write must not enter while the FIFO barrier below is
    // waiting to retire work from the old channel.
    DRIVER_READY.store(false, Ordering::Release);
    if writer.call(Request::LinkLost { link: Link::Ble }, &TEARDOWN_REPLY).await.is_err() {
        warn!("ble: [v4] the engine refused a link-lost teardown");
    }
    // `Writer` is FIFO: once LinkLost answers, every earlier control request has completed or been
    // retired, so no engine borrow of `CONTROL_RX` can remain. Discard a length the dropped driver
    // never took, then wake a GATT task that may still be waiting.
    CONTROL_IN.reset();
    CONTROL_BUSY.store(false, Ordering::Release);
    CONTROL_TAKEN.signal(());
}

/// The engine driver. The CoC carries the byte stream formed by consecutive stream records.
pub(crate) async fn serve_objects(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) -> ! {
    let Some(writer) = crate::flat_store::writer() else {
        // Unreachable, since the storage task is spawned on every card, and reported rather than
        // unwrapped because a radio task must never be why the board panics.
        warn!("ble: [v4] the store's write half is not armed — object service is down this boot");
        core::future::pending().await
    };
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
        // The CoC requires an encrypted link. A peer cannot normally reach here unencrypted, because
        // `psm` and `objectControl` are both `authenticated`, but one that guessed the SPSM must
        // still be turned away.
        if !matches!(conn.raw().security_level(), Ok(level) if level.encrypted()) {
            warn!("ble: [v4] channel opened on an unencrypted link — refusing (S0 §8)");
            let (mut w, _r) = ch.split();
            w.disconnect();
            continue;
        }
        let (mut writer_half, mut reader) = ch.split();
        // The control ceiling is `ATT_MTU - 3` and the stream ceiling is the CoC SDU, both clamped
        // to what this adapter can hold. The arithmetic is `obc-link`'s, where tests pin it. `None`
        // refuses a link below the protocol floor.
        let att_mtu = usize::from(conn.raw().att_mtu());
        let Some(ceilings) = Ceilings::for_ble(att_mtu, usize::from(reader.mtu()), OUT_LEN) else {
            warn!("ble: [v4] link below the protocol floor (att {}, sdu {}) — closing", att_mtu, reader.mtu());
            writer_half.disconnect();
            continue;
        };
        info!("ble: [v4] channel up — control {} B, stream {} B", ceilings.control(), ceilings.stream());
        // A dropped call from the previous link parks the buffer in the reply slot; take it back
        // before the first call of this one, because `Writer::call` would discard it.
        lane.reclaim().await;
        if lane.call(&writer, |out| Request::Pump { link: Link::Ble, out }).await.is_none() {
            // The lane has no buffer and cannot get one. Serving would mean answering nothing.
            warn!("ble: [v4] no reaction buffer — refusing this channel rather than half-serving it");
            writer_half.disconnect();
            continue;
        }
        if writer.call(Request::LinkUp { link: Link::Ble, ceilings }, &ENGINE_REPLY).await.is_err() {
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
        // The reader is a sibling, not a branch: it is dropped only when the driver returns, and the
        // driver returns only to tear this channel down, so both split halves fall out of scope
        // together and the channel is disconnected in the same breath.
        let outcome =
            match select(driver(&writer, lane, stack, server, conn, &mut writer_half), reader_pump(stack, &mut reader))
                .await
            {
                Either::First(reason) => reason,
                Either::Second(never) => never,
            };
        // Teardown goes on its own reply slot. The `select` above can drop the driver mid
        // `Lane::call`, so the orphaned answer — the one carrying the reaction buffer — is still in
        // `ENGINE_REPLY`, and `Writer::call` discards a mismatched reply. Teardown on that slot would
        // throw `OUT` away, `reclaim` would find the slot empty, and every later CoC accept would be
        // refused for the rest of the boot. `TEARDOWN_REPLY` leaves the orphan where `reclaim` finds
        // it. It clears `DRIVER_READY` too.
        release_engine(&writer).await;
        info!("ble: [v4] channel down ({}) — engine released, re-accepting", outcome);
    }
}

/// Recover stream records from the CoC byte stream and hand them over one at a time.
///
/// CoreBluetooth's `OutputStream.write` can accept fewer bytes than the complete record the app
/// supplied, and those pieces arrive here as separate SDUs. The record header length makes the byte
/// stream self-framing, so [`StreamRecordAssembler`] joins pieces in [`STREAM_RX`] and also handles
/// two records sharing one SDU. One record is outstanding at a time, which is where the credit
/// withholding lives.
async fn reader_pump(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    reader: &mut L2capChannelReader<'_, DefaultPacketPool>,
) -> &'static str {
    let mut assembler = StreamRecordAssembler::new();
    loop {
        let sdu = match reader.receive_sdu(stack).await {
            Ok(sdu) if sdu.is_empty() => continue,
            Ok(sdu) => sdu,
            Err(_) => return "channel",
        };
        let bytes = sdu.as_ref();
        let mut consumed = 0;
        while consumed < bytes.len() {
            // SAFETY: the mutable borrow ends before a complete record is signalled. The driver
            // reads this buffer only between `STREAM_IN` and `STREAM_TAKEN`.
            let rx = unsafe { &mut *core::ptr::addr_of_mut!(STREAM_RX) };
            let (used, state) = assembler.push(rx, &bytes[consumed..]);
            consumed += used;
            match state {
                StreamAssembly::NeedMore => {}
                StreamAssembly::Complete(len) => {
                    STREAM_TAKEN.reset();
                    STREAM_IN.signal(len);
                    STREAM_TAKEN.wait().await;
                    assembler.reset();
                }
                StreamAssembly::TooLarge(len) => {
                    warn!("ble: [v4] stream record is {} B — above the adapter buffer", len);
                    return "stream-framing";
                }
            }
        }
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
    // Owned by the driver, so it is per channel by construction. See `Admission` for why it
    // remembers which transfer was admitted.
    let mut admission = Admission::new();
    loop {
        // Control first when both are ready: a `CANCEL` or a `LIST` must not queue behind a stream
        // frame the engine may be about to refuse anyway.
        let reaction = match select(CONTROL_IN.wait(), STREAM_IN.wait()).await {
            Either::First(len) => match control_record(writer, lane, len).await {
                Some(reaction) => reaction,
                None => return "lane",
            },
            Either::Second(len) => match stream_record(writer, lane, &mut admission, len).await {
                Some(reaction) => reaction,
                None => return "lane",
            },
        };
        if let Some(reason) = pump(writer, lane, stack, server, conn, tx, reaction).await {
            return reason;
        }
    }
}

/// Hand the staged control record to the engine, then release [`CONTROL_RX`].
async fn control_record(writer: &Writer, lane: &mut Lane, len: usize) -> Option<Reaction> {
    // SAFETY: `CONTROL_BUSY` stays set after `CONTROL_IN` is taken, so `stage_control` cannot write
    // this buffer until the engine call below has finished consuming it.
    let record: &'static [u8] =
        unsafe { core::slice::from_raw_parts(core::ptr::addr_of!(CONTROL_RX).cast::<u8>(), len) };
    let reaction = lane.call(writer, |out| Request::Control { link: Link::Ble, record, out }).await;
    // Released here and not a statement earlier: the engine consumes `CONTROL_RX` synchronously
    // inside the storage task, which is over by the time this call answers, so this is the first
    // instant at which the GATT task may stage another record.
    CONTROL_BUSY.store(false, Ordering::Release);
    CONTROL_TAKEN.signal(());
    reaction
}

/// Hand the received stream record to the engine, holding it first if nothing is admitted yet.
async fn stream_record(writer: &Writer, lane: &mut Lane, admission: &mut Admission, len: usize) -> Option<Reaction> {
    // The admission hold. A stream frame whose `PUT` control write has not reached the engine yet
    // would be discarded in silence and the upload would die at offset zero, and "no control record
    // is pending" does not mean none was written, because the GATT pump may be parked on the shared
    // store. So ask the engine, and hold an unadmitted frame while the control channel gets its
    // window. The reader is already withholding credit, so the hold costs only the wait.
    //
    // The query is skipped for a frame that continues the transfer the engine last confirmed:
    // `Admission` is keyed on the `RequestId` in the frame header, so a steady-state upload pays no
    // round trips and the leading frame of the next transfer is still queried.
    //
    // Reading four bytes of the frame header is not parsing a payload: it is the record boundary
    // information this binding owns. A record too short to carry one goes to the engine, which owns
    // that refusal.
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
                    let reaction = control_record(writer, lane, control_len).await?;
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
    let reaction = lane.call(writer, |out| Request::Stream { link: Link::Ble, record, out }).await;
    // The engine has consumed the bytes; the reader may take the next SDU.
    STREAM_TAKEN.signal(());
    reaction
}

/// Whether a transfer owns the engine right now. Deliberately not a `Lane` call: it borrows no
/// buffer, so it cannot be the thing that loses one.
async fn live_transfer(writer: &Writer) -> Option<obc_link::flat::RequestId> {
    // Its own slot rather than `ENGINE_REPLY`: one slot per concurrently live call is the contract.
    // The engine answers without touching the card, so the round trip is one executor hop.
    static LIVE_REPLY: Reply = Signal::new();
    match writer.call(Request::LiveTransfer, &LIVE_REPLY).await {
        Ok(Outcome::Live(live)) => live,
        _ => None,
    }
}

/// Send what the reaction names, then pump until the engine goes quiet, servicing control records in
/// between, because cancel is bilateral and a download must not deafen the control channel.
///
/// Returns `Some(reason)` when the channel should be torn down, `None` when the engine went quiet.
#[allow(clippy::too_many_arguments)]
async fn pump(
    writer: &Writer,
    lane: &mut Lane,
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
                // An unanswerable record: emit nothing and close that record stream. Both arms end
                // the driver, and that drops both split halves, so the stream channel goes down
                // either way and only the log differs. BLE has no control channel that can close
                // independently of the CoC — the ATT link is the connection.
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
                    // One confirmed indication carries the response.
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
            Reaction::SendAndReboot { len } => {
                // The terminal answer is still a confirmed indication: the client must know that
                // FORMAT reached durable media before the link disappears. A brief beat lets the
                // controller finish the confirmation exchange.
                match with_timeout(
                    INDICATE_TIMEOUT,
                    server.obc.object_control.indicate_raw(conn, lane.sent(len), false),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        warn!("ble: [v4] terminal indication failed: {:?}", defmt::Debug2Format(&e));
                    }
                    Err(_) => warn!("ble: [v4] terminal indication timed out"),
                }
                info!("ble: [v4] terminal response complete — rebooting");
                Timer::after_millis(50).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
        // Between iterations, not only when idle. A `GET` streams for as long as the object is
        // large, and cancel is bilateral: a `CANCEL` written mid-download has to reach the engine
        // while there is still something to cancel.
        if let Some(len) = CONTROL_IN.try_take() {
            match control_record(writer, lane, len).await {
                Some(next) => reaction = next,
                None => return Some("lane"),
            }
            continue;
        }
        match lane.call(writer, |out| Request::Pump { link: Link::Ble, out }).await {
            Some(next) => reaction = next,
            None => return Some("lane"),
        }
    }
}
