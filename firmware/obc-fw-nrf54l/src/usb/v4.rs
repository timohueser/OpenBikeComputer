//! The **protocol-v4 USB adapter** (`FLAT_Store_Protocol.md` §5.2) — FS7.5-c3b, epic #1256.
//!
//! §5's brief is the same one the radio's adapter works to: "an adapter owns record boundaries,
//! pacing, timeouts, drain, and nothing else. It never parses a payload, never mints an identifier,
//! and never originates a frame." The record boundaries are [`super::records`]; this file is the
//! rest.
//!
//! ## What changed on this link, and what did not
//!
//! The engine is not here. It lives in [`crate::flat_store::storage_task`] beside the write half,
//! because `obc_link::flat::Store` is synchronous throughout and the card has exactly one writing
//! execution context (#1256's owner ruling). This module reaches it through
//! [`Lane`](crate::flat_store::Lane), one record at a time — the same seam the BLE adapter uses,
//! deliberately the *same code*, so the two links cannot drift into two answers to the same
//! question.
//!
//! What is genuinely different is everything below the records:
//!
//! - **Two endpoint pairs, both byte streams.** BLE's control channel is an ATT write and an
//!   indication, which are messages; USB's is a bulk pipe, which is not. §5.2's length prefix is
//!   what makes it one, and [`super::records`] owns that.
//! - **No `psm`, no accept, no MTU.** A link comes up when the host sets a configuration and goes
//!   away when the cable does. `Ceilings::for_usb` is a constant of the binding rather than
//!   something read off a negotiation, which is why there is no floor refusal path here.
//! - **One writer per endpoint.** The v1 plane shared its control IN endpoint between two futures
//!   behind a mutex; here the driver owns both IN endpoints and nothing else writes them.
//!
//! ## §5's two obligations
//!
//! Both are met the way c3a met them, because both are properties of the shape rather than of the
//! transport:
//!
//! - **Nothing consumed is dropped.** Each reader is its own future ([`control_pump`],
//!   [`stream_pump`]), a sibling of the driver rather than a branch of a `select` the driver
//!   re-enters. `RecordReader::next` is not cancellation-safe — it may have moved bytes out of the
//!   endpoint into its buffer before it suspends — so a `select` that dropped it mid-record would
//!   lose exactly those bytes. The readers are dropped only when the driver returns, which it does
//!   only to tear the link down.
//! - **Credit is withheld while a frame is held.** A pump receives one record, posts it, and waits
//!   to be told the driver is done before reading again. Nothing reads the bulk OUT endpoint
//!   meanwhile, so it NAKs — which is §5's "ceasing to accept stream records on USB", in the only
//!   terms this transport has.
//!
//! **Cross-channel ordering** is the same `Admission` latch, from `obc-link`, for the same reason it
//! is a type there rather than a flag in a binding: a `PUT`'s first stream record genuinely races
//! its own control record, and a latch that only remembered *that* something was admitted would wave
//! the second transfer of a session straight into an idle engine.

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use obc_link::flat::{Admission, Ceilings, Channel, Link, Reaction, RequestId};

use crate::flat_store::{Lane, Outcome, Reply, Request, Writer};

use super::records::{buffer_len, RecordEnd, RecordReader, RecordWriter};
use super::{EpIn, EpOut, BULK_BURST_LEN, MAX_PACKET};

// ══════════════════════════ the link's constants ══════════════════════════

/// §5.2's record ceiling: the 16-byte stream frame plus 4,096 payload bytes.
///
/// The payload width is the point. `obc_link`'s engine writes a whole aligned prefix of a stream
/// record straight to the card, so a record of exactly four 1 KiB blocks — eight 512-byte card
/// blocks — is **one** card command rather than eight, and on this media a command costs about the
/// same whatever it carries (`FLAT_Store_Format.md` §5.5). Everything else about this number follows
/// from that one: it is the reaction buffer's size, the `LIST` page ceiling (46 entries), and the
/// bound both readers refuse a record above.
pub(crate) const RECORD_CEILING: usize = 16 + 4_096;

/// §5.2's ceilings for this link, resolved once rather than per cable.
///
/// `for_usb` answers `Option` because §5.1's floor refusal is a real outcome on a link that
/// *negotiates*; USB negotiates nothing, so on this binding the answer is fixed. Unwrapping it here
/// — at a `static`, not inside a task — means a ceiling edited below the floor is a bring-up panic
/// with a legible message rather than a link that silently refuses every cable.
static CEILINGS: Ceilings = match Ceilings::for_usb(RECORD_CEILING) {
    Some(ceilings) => ceilings,
    None => panic!("§5.2's record ceiling is below the protocol floor"),
};

/// §5.2's narrower bound on a **host → device control** record.
///
/// §3's largest request is the 100-byte `PUT`. Sizing this buffer to [`RECORD_CEILING`] would be
/// sizing it to nothing — 4 KiB of `.bss` for a channel whose widest message is a tenth of a
/// packet — so the binding states the narrowing instead, and a longer record ends the record stream
/// exactly as §5.2 says a length above the ceiling does.
pub(crate) const CONTROL_RECORD_CEILING: usize = 256;

/// How long a stream record is held while the control channel is given its chance to admit it.
///
/// §3.6 lets a client stream immediately, so the first record of a `PUT` races its own control
/// record. The hold is bounded: a record still unadmitted when the window closes is handed over
/// anyway, and §3.8's silent discard is then the right answer, because it genuinely belongs to no
/// transfer the receiver can be sure of.
///
/// **250 ms, and it is a guess with a reason rather than a measurement.** It is meant to cover one
/// pass of the control pump, which on this link is a bulk read already armed by the driver — there
/// is no GATT event pump to wait on and no shared-store lock in the path, which is why it is
/// shorter than the radio's 750 ms rather than copied from it. Neither number has been measured on
/// glass; this comment is the marker, and the board session re-pins both.
const ADMISSION_WINDOW: Duration = Duration::from_millis(250);

// ══════════════════════════ the buffers ══════════════════════════

/// Where a reaction's bytes land, and the ceiling both channels are pinned under.
static mut OUT: [u8; RECORD_CEILING] = [0; RECORD_CEILING];

/// The control channel's reassembly buffer.
static mut CONTROL_RX: [u8; buffer_len(CONTROL_RECORD_CEILING, MAX_PACKET as usize)] =
    [0; buffer_len(CONTROL_RECORD_CEILING, MAX_PACKET as usize)];

/// The stream channel's reassembly buffer. It is the largest static this plane owns, and the
/// `+ BULK_BURST_LEN` term is what lets the burst-armed bulk OUT endpoint (#1173) keep its arming:
/// the driver hands back everything the core absorbed while the CPU was busy, and a free tail
/// shorter than one burst would have it refuse the read.
static mut STREAM_RX: [u8; buffer_len(RECORD_CEILING, BULK_BURST_LEN)] =
    [0; buffer_len(RECORD_CEILING, BULK_BURST_LEN)];

/// USB's engine reply slot. One driver, one live call — [`Writer::call`]'s contract, honoured by
/// there being exactly one caller rather than by call ordering.
static ENGINE_REPLY: Reply = Signal::new();

/// This link's lane, in `.bss` — see [`lane`].
static mut LANE: core::mem::MaybeUninit<Lane> = core::mem::MaybeUninit::uninit();
static LANE_BUILT: AtomicBool = AtomicBool::new(false);

/// A control record the control pump has read, and the driver's release of it.
static CONTROL_IN: Signal<CriticalSectionRawMutex, &'static [u8]> = Signal::new();
static CONTROL_TAKEN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// A stream record the stream pump has read, and the driver's release of it.
static STREAM_IN: Signal<CriticalSectionRawMutex, &'static [u8]> = Signal::new();
static STREAM_TAKEN: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The plane's resident cost, for the budget table in `main.rs`.
pub(crate) const RESIDENT_BYTES: usize = RECORD_CEILING
    + buffer_len(CONTROL_RECORD_CEILING, MAX_PACKET as usize)
    + buffer_len(RECORD_CEILING, BULK_BURST_LEN);

/// **The one lane, for the life of the image.**
///
/// Built once and reached from inside the driver rather than carried as a local of the USB task,
/// for the reason the radio's twin records: a lane carried across a task's awaits changes that
/// coroutine's liveness enough for LLVM to stop sinking construction temporaries out of the poll
/// frame, and on the BLE side that cost 8,628 bytes of frame for eight bytes of value (#677/#1084).
///
/// # Safety
/// One caller: [`serve_objects`], which is the body of the one USB task.
#[inline(never)]
fn lane() -> &'static mut Lane {
    // SAFETY: sole writer of `LANE`; the flag makes the build happen exactly once, before any
    // driver exists, and `Lane` has no `Drop`.
    unsafe {
        if !LANE_BUILT.swap(true, Ordering::Relaxed) {
            let out: &'static mut [u8] =
                core::slice::from_raw_parts_mut(core::ptr::addr_of_mut!(OUT).cast::<u8>(), RECORD_CEILING);
            return crate::init_static(core::ptr::addr_of_mut!(LANE), Lane::new(out, &ENGINE_REPLY, "usb"));
        }
        &mut *(*core::ptr::addr_of_mut!(LANE)).as_mut_ptr()
    }
}

// ══════════════════════════ the driver ══════════════════════════

/// **Serve v4 records over the cable until the device is unplugged, then wait for the next cable —
/// forever.**
///
/// The endpoints are owned for the life of the task and re-armed across an unplug rather than
/// rebuilt, which is what keeps this plane's `.bss` footprint a property of the image instead of the
/// cable cycle.
pub(crate) async fn serve_objects(ctrl_in: EpIn, ctrl_out: EpOut, bulk_in: EpIn, bulk_out: EpOut) -> ! {
    // Read once, before the loop: `flat_store::arm` runs at its spawn site in `main`, which is
    // several statements ahead of this task's first poll on every boot path.
    let writer = crate::flat_store::writer();
    // SAFETY: sole writer of each buffer; `serve_objects` is the body of a task spawned once.
    let control_buf = unsafe { &mut *core::ptr::addr_of_mut!(CONTROL_RX) };
    let stream_buf = unsafe { &mut *core::ptr::addr_of_mut!(STREAM_RX) };
    let mut control = RecordReader::new(ctrl_out, control_buf, CONTROL_RECORD_CEILING, MAX_PACKET as usize);
    let mut stream = RecordReader::new(bulk_out, stream_buf, RECORD_CEILING, BULK_BURST_LEN);
    let mut control_tx = RecordWriter::new(ctrl_in);
    let mut stream_tx = RecordWriter::new(bulk_in);
    // Reached here rather than passed in: see `lane`.
    let lane = lane();
    // Not an `expect` in a link task: §5.2's ceiling is a *constant*, so "is it above the protocol
    // floor" is a question with one answer for the life of the image and a task is the wrong place
    // to discover it. `CEILINGS` is that answer, computed once.
    let ceilings = CEILINGS;

    loop {
        // Before configuration (and after an unplug) the endpoints are disabled; parking here is the
        // idle state, woken by the host's SET_CONFIGURATION.
        control.wait_enabled().await;
        let Some(writer) = writer else {
            // Unreachable since c3a spawns the storage task on every card, and reported rather than
            // unwrapped because a link task must never be why the board panics.
            warn!("usb: [v4] the store's write half is not armed — object service is down this boot");
            core::future::pending::<()>().await;
            continue;
        };
        info!("usb: [v4] endpoints enabled — control {} B, stream {} B", ceilings.control(), ceilings.stream());

        // A dropped call from the previous cable parks the buffer in the reply slot; take it back
        // before the first call of this one, because `Writer::call` would discard it.
        lane.reclaim().await;
        if lane.call(&writer, |out| Request::Pump { link: Link::Usb, out }).await.is_none() {
            warn!("usb: [v4] no reaction buffer — refusing this cable rather than half-serving it");
            Timer::after_millis(500).await;
            continue;
        }
        if writer.call(Request::LinkUp { link: Link::Usb, ceilings }, &ENGINE_REPLY).await.is_err() {
            warn!("usb: [v4] the engine refused the link");
            Timer::after_millis(500).await;
            continue;
        }
        // The two hand-off releases are level state from the previous cable's point of view; the
        // record streams themselves were reset on the way *out* of that cable, where §5.2 wants
        // them (see below).
        CONTROL_TAKEN.reset();
        STREAM_TAKEN.reset();

        // **Three siblings, not a `select` the driver re-enters.** Each reader may have moved bytes
        // out of its endpoint before it suspends, so a shape that dropped one of them per pass would
        // throw those bytes away — the failure c3a's review found on the radio, in the one form this
        // transport can also produce.
        let reason = match select3(
            driver(&writer, lane, &mut control_tx, &mut stream_tx),
            control_pump(&mut control),
            stream_pump(&mut stream),
        )
        .await
        {
            Either3::First(reason) | Either3::Second(reason) | Either3::Third(reason) => reason,
        };
        // **Reset the record streams, then report teardown — in that order**, because §5.2 puts them
        // in it: a bad record length "is `invalidFrame` and resets that record stream *before*
        // teardown is reported to the engine". A peer that has lost a record boundary cannot be
        // re-synchronised by guessing where the next one starts, so what is buffered is dropped and
        // the link goes with it. Both `select3` borrows end at the statement above, which is what
        // makes the resets reachable here rather than at the top of the next pass.
        control.reset();
        stream.reset();
        CONTROL_IN.reset();
        STREAM_IN.reset();
        // §3.8's third form of cancel. On its own reply slot, so that an orphan the driver may have
        // left in `ENGINE_REPLY` stays where `Lane::reclaim` can find it.
        let joined = release_engine(&writer).await;
        release_joined_stage(joined);
        info!("usb: [v4] link down ({}) — engine released", reason);
        if reason != RecordEnd::LinkDown.reason() {
            // Not an unplug: a framing error or a driver failure with the endpoints still up. Back
            // off before re-arming, or a persistent one hot-spins and starves the ride loop on this
            // cooperative executor.
            Timer::after_millis(200).await;
        }
    }
}

/// **Release whatever the engine still holds for a link that has gone away** (§3.8's third form of
/// cancel).
async fn release_engine(writer: &Writer) -> JoinedUsbStage {
    static TEARDOWN_REPLY: Reply = Signal::new();
    if writer.call(Request::LinkLost { link: Link::Usb }, &TEARDOWN_REPLY).await.is_err() {
        warn!("usb: [v4] the engine refused a link-lost teardown");
    }
    finish_usb_stage(writer).await
}

/// Proof that the storage task answered `FinishUsbStage`. Its constructor is private to the await
/// below, so no granted-stage terminal path can compile a release before the DMA join.
struct JoinedUsbStage;

/// Join a deferred card write before the ride loop may hand the arena to render or navigation.
async fn finish_usb_stage(writer: &Writer) -> JoinedUsbStage {
    static FINISH_REPLY: Reply = Signal::new();
    if writer.call(Request::FinishUsbStage, &FINISH_REPLY).await.is_err() {
        warn!("usb: [v4] could not join the final staged card write");
    }
    // Even an error answer is produced only after `finish_write_blocks` returns: the transfer has
    // stopped borrowing the source, although the card operation itself failed.
    JoinedUsbStage
}

fn release_joined_stage(_joined: JoinedUsbStage) {
    crate::usb::STAGE_REQ.store(false, core::sync::atomic::Ordering::Relaxed);
    crate::usb::STAGE_WAKE.signal(());
}

/// Read control records and hand them over one at a time.
async fn control_pump(reader: &mut RecordReader) -> &'static str {
    loop {
        let record = match reader.next().await {
            Ok(record) => record,
            Err(end) => return end.reason(),
        };
        CONTROL_TAKEN.reset();
        CONTROL_IN.signal(record);
        CONTROL_TAKEN.wait().await;
    }
}

/// Read stream records and hand them over one at a time.
///
/// **This is where §5's credit withholding lives on this link.** One record is outstanding at a
/// time and nothing else reads the bulk OUT endpoint, so while a record is held the endpoint NAKs
/// and the host's send loop is what stops. There is no second buffer for a second record to go to,
/// and there does not need to be.
async fn stream_pump(reader: &mut RecordReader) -> &'static str {
    loop {
        let record = match reader.next().await {
            Ok(record) => record,
            Err(end) => return end.reason(),
        };
        STREAM_TAKEN.reset();
        STREAM_IN.signal(record);
        STREAM_TAKEN.wait().await;
    }
}

/// The driver loop: take whichever record is ready, hand it to the engine, and pump until quiet.
async fn driver(
    writer: &Writer,
    lane: &mut Lane,
    control_tx: &mut RecordWriter,
    stream_tx: &mut RecordWriter,
) -> &'static str {
    // Owned by the driver, so it is per cable by construction rather than by a `reset` someone has
    // to remember.
    let mut admission = Admission::new();
    let mut staged_request: Option<RequestId> = None;
    let mut stage_granted = false;
    let mut staged_started: Option<Instant> = None;
    let mut staged_bytes = 0u64;
    loop {
        // Control first when both are ready: a `CANCEL` or a `LIST` must not queue behind a stream
        // record the engine may be about to refuse anyway.
        let reaction = match embassy_futures::select::select(CONTROL_IN.wait(), STREAM_IN.wait()).await {
            embassy_futures::select::Either::First(record) => match control_record(writer, lane, record).await {
                Some(reaction) => reaction,
                None => return "lane",
            },
            embassy_futures::select::Either::Second(record) => {
                let result =
                    stream_record(writer, lane, &mut admission, record, &mut staged_request, &mut stage_granted).await;
                if stage_granted {
                    staged_started.get_or_insert_with(Instant::now);
                    staged_bytes += record.len().saturating_sub(obc_link::flat::wire::STREAM_HEADER_LEN) as u64;
                }
                match result {
                    Some(reaction) => reaction,
                    None => return "lane",
                }
            }
        };
        // Most records are `Idle`; do not put another storage round trip in the 4 KiB hot path.
        // A terminal/control reaction can end the USB upload, while the app-facing map projection
        // may remain `Receiving` if BLE immediately admits another map. That edge requires the
        // exact owner query below.
        let may_have_ended = reaction != Reaction::Idle;
        if let Some(reason) = pump(writer, lane, control_tx, stream_tx, reaction).await {
            if stage_granted {
                let joined = finish_usb_stage(writer).await;
                release_joined_stage(joined);
                log_staged_rate(staged_started, staged_bytes);
            }
            return reason;
        }
        let projection_ended = !crate::link::map_transfer_state().is_some_and(|state| state.is_receiving());
        let staged_ended = if let Some(request) = staged_request {
            (projection_ended || may_have_ended) && !usb_owns_map_upload(writer, request).await
        } else {
            false
        };
        if staged_ended {
            if stage_granted {
                let joined = finish_usb_stage(writer).await;
                release_joined_stage(joined);
                log_staged_rate(staged_started, staged_bytes);
            }
            staged_request = None;
            stage_granted = false;
            staged_started = None;
            staged_bytes = 0;
        }
    }
}

fn log_staged_rate(started: Option<Instant>, bytes: u64) {
    let Some(started) = started else { return };
    let us = started.elapsed().as_micros().max(1);
    info!(
        "usb: [v4] staged {=u64} B in {=u64} ms ({=u64} kB/s, full CRC + card DMA)",
        bytes,
        us / 1_000,
        bytes.saturating_mul(1_000) / us
    );
}

/// Hand one control record to the engine, then release the pump's buffer.
async fn control_record(writer: &Writer, lane: &mut Lane, record: &'static [u8]) -> Option<Reaction> {
    let reaction = lane.call(writer, |out| Request::Control { link: Link::Usb, record, out }).await;
    // **Released here and not a statement earlier.** The engine consumes the record synchronously
    // inside the storage task's `serve`, which is over by the time this call answers — so this is
    // the first instant at which the pump may read over bytes the queue still borrowed.
    CONTROL_TAKEN.signal(());
    reaction
}

/// Hand one stream record to the engine, holding it first if nothing is admitted yet.
async fn stream_record(
    writer: &Writer,
    lane: &mut Lane,
    admission: &mut Admission,
    record: &'static [u8],
    staged_request: &mut Option<RequestId>,
    stage_granted: &mut bool,
) -> Option<Reaction> {
    // §5's admission hold. Reading four bytes of the §3.8 frame header is not "parsing a payload":
    // it is the record boundary information the binding is explicitly responsible for, and a record
    // too short to carry one is not decoded here — it goes to the engine, which owns that refusal.
    let frame_id =
        (record.len() >= 4).then(|| RequestId(u32::from_le_bytes([record[0], record[1], record[2], record[3]])));
    if let Some(frame_id) = frame_id {
        if admission.needs_query(frame_id) && admission.observed(frame_id, live_transfer(writer).await) {
            if let embassy_futures::select::Either::First(control) =
                embassy_futures::select::select(CONTROL_IN.wait(), Timer::after(ADMISSION_WINDOW)).await
            {
                let reaction = control_record(writer, lane, control).await?;
                // **Re-signalling `STREAM_IN` with the record we are holding, rather than delivering
                // it here.** §5 says a held frame must not be delivered before its admission and
                // must not be dropped; this is the third option — put it back where the driver's
                // next `select` will take it, having spent this pass on the control record that may
                // be its admission. The pump is still not reading behind it (`STREAM_TAKEN` is
                // un-signalled), so nothing overwrites the buffer meanwhile and §5's credit
                // withholding continues to hold.
                //
                // It costs one extra trip round the driver loop, and it is deliberately not
                // optimised into a direct call: the loop is where control records win ties, and
                // delivering from here would jump that queue with a record that has just been told
                // to wait.
                STREAM_IN.signal(record);
                return Some(reaction);
            }
            warn!("usb: [v4] a stream record arrived unadmitted — delivering after the hold window");
        }
    }
    if staged_request.is_none() {
        if let Some(frame_id) = frame_id {
            if usb_owns_map_upload(writer, frame_id).await {
                *staged_request = Some(frame_id);
                *stage_granted = crate::usb::request_stage().await;
            }
        }
    }
    let reaction = if *stage_granted {
        lane.call(writer, |out| Request::StreamStaged { record, out }).await
    } else {
        lane.call(writer, |out| Request::Stream { link: Link::Usb, record, out }).await
    };
    STREAM_TAKEN.signal(());
    reaction
}

/// Whether a transfer owns the engine right now — the query §5's hold is built on.
///
/// Deliberately **not** a `Lane` call: it borrows no buffer, so it cannot be the thing that loses
/// one. Its own reply slot for the reason the contract states — one slot per concurrently live
/// call is a property of the types here rather than of call ordering.
static LIVE_QUERY_REPLY: Reply = Signal::new();

async fn live_transfer(writer: &Writer) -> Option<RequestId> {
    match writer.call(Request::LiveTransfer, &LIVE_QUERY_REPLY).await {
        Ok(Outcome::Live(live)) => live,
        _ => None,
    }
}

/// The storage-owned admission proof for the cable-only scratch arm. The app's map progress state
/// deliberately omits link ownership and therefore cannot distinguish a BLE map PUT from USB.
async fn usb_owns_map_upload(writer: &Writer, request: RequestId) -> bool {
    // Sequential in this one USB driver with `live_transfer`; reusing the slot avoids paying a
    // second Signal (72 linked resident bytes) for two mutually-exclusive engine queries.
    match writer.call(Request::UsbMapUpload { request }, &LIVE_QUERY_REPLY).await {
        Ok(Outcome::UsbMap(owns)) => owns,
        _ => false,
    }
}

/// Send what the reaction names, then pump until the engine goes quiet — servicing control records
/// in between, because §3.8's cancel is bilateral and a download must not deafen the control channel.
async fn pump(
    writer: &Writer,
    lane: &mut Lane,
    control_tx: &mut RecordWriter,
    stream_tx: &mut RecordWriter,
    first: Reaction,
) -> Option<&'static str> {
    let mut reaction = first;
    loop {
        match reaction {
            Reaction::Idle => return None,
            // §3.1's unanswerable record: emit nothing and close that record stream. On this link
            // the two channels are two endpoint pairs of one interface, which the host enables and
            // disables together, so closing one means ending the link — and the log says which
            // channel asked rather than pretending the distinction was honoured.
            Reaction::Close(channel) => {
                return Some(match channel {
                    Channel::Control => "control-closed",
                    Channel::Stream => "stream-closed",
                })
            }
            Reaction::Send { channel, len } => {
                let tx = match channel {
                    Channel::Control => &mut *control_tx,
                    Channel::Stream => &mut *stream_tx,
                };
                if !tx.send(lane.sent(len)).await {
                    return Some("send");
                }
            }
            Reaction::SendAndReboot { len } => {
                // FORMAT invalidates the mounted store before it answers. Complete the USB record,
                // give the controller one short drain beat, then remount the new empty store from a
                // clean boot. ARM uses this same terminal reaction once board policy enables it.
                if !control_tx.send(lane.sent(len)).await {
                    return Some("send-reboot");
                }
                info!("usb: [v4] terminal response sent — rebooting");
                Timer::after_millis(50).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
        // **Between iterations, not only when idle.** A `GET` streams for as long as the object is
        // large, and §3.8's cancel is bilateral: a `CANCEL` sent mid-download has to reach the
        // engine while there is still something to cancel.
        if let Some(record) = CONTROL_IN.try_take() {
            match control_record(writer, lane, record).await {
                Some(next) => reaction = next,
                None => return Some("lane"),
            }
            continue;
        }
        match lane.call(writer, |out| Request::Pump { link: Link::Usb, out }).await {
            Some(next) => reaction = next,
            None => return Some("lane"),
        }
    }
}
