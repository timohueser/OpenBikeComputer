//! The protocol-v4 USB adapter. An adapter owns record boundaries, pacing, timeouts and drain, and
//! nothing else: it never parses a payload, never mints an identifier and never originates a frame.
//! The record boundaries are [`super::records`]; this file is the rest.
//!
//! The engine is not here. It lives in [`crate::flat_store::storage_task`] beside the write half,
//! because `obc_link::flat::Store` is synchronous throughout and the card has exactly one writing
//! execution context. This module reaches it through [`Lane`](crate::flat_store::Lane), one record
//! at a time, over the same seam the BLE adapter uses.
//!
//! Two obligations shape what follows. Nothing consumed is dropped: each reader is its own future
//! ([`control_pump`], [`stream_pump`]) and not a branch of a `select` the driver re-enters, because
//! `RecordReader::next` is not cancellation-safe and may have moved bytes out of the endpoint
//! before it suspends. And credit is withheld while a frame is held: a pump receives one record,
//! posts it, and waits to be told the driver is done before it reads again, so the bulk OUT
//! endpoint NAKs and the host's own send loop is what stops.

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{select3, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use obc_link::flat::wire::StreamFrame;
use obc_link::flat::{Admission, Ceilings, Channel, Link, Reaction, RequestId};

use crate::flat_store::{Lane, Outcome, Reply, Request, Writer};

use super::records::{buffer_len, RecordEnd, RecordReader, RecordWriter};
use super::{EpIn, EpOut, BULK_BURST_LEN, MAX_PACKET};

/// The record ceiling: the 16-byte stream frame plus 8,192 payload bytes.
///
/// The payload width is the point. The engine writes a whole aligned prefix of a stream record
/// straight to the card, so a record of exactly eight 1 KiB blocks is one card command and not
/// eight, and on this media a command costs about the same whatever it carries. The same number is
/// the reaction buffer's size, the `LIST` page ceiling, and the bound both readers refuse a record
/// above.
pub(crate) const RECORD_CEILING: usize = 16 + 8_192;

/// This link's ceilings, resolved once rather than per cable. USB negotiates nothing, so `for_usb`'s
/// `Option` has a fixed answer here. Unwrapping it at a `static` makes a ceiling edited below the
/// protocol floor a bring-up panic, and not a link that silently refuses every cable.
static CEILINGS: Ceilings = match Ceilings::for_usb(RECORD_CEILING) {
    Some(ceilings) => ceilings,
    None => panic!("§5.2's record ceiling is below the protocol floor"),
};

/// The narrower bound on a host-to-device control record. The largest request is the 100-byte
/// `PUT`, so [`RECORD_CEILING`] here would be 8 KiB of `.bss` for a channel whose widest message is
/// a fraction of a packet. A longer record ends the record stream, as any over-ceiling length does.
pub(crate) const CONTROL_RECORD_CEILING: usize = 256;

/// How long a stream record is held while the control channel is given its chance to admit it.
///
/// A client may stream immediately, so the first record of a `PUT` races its own control record.
/// The hold is bounded: a record still unadmitted when the window closes is handed over anyway and
/// silently discarded, because it belongs to no transfer the receiver can be sure of. The 250 ms
/// covers one pass of the control pump, which on this link is a bulk read the driver already armed.
/// It is a guess with a reason, not a measurement on glass.
const ADMISSION_WINDOW: Duration = Duration::from_millis(250);

/// Where a reaction's bytes land, and the ceiling both channels are pinned under.
static mut OUT: [u8; RECORD_CEILING] = [0; RECORD_CEILING];

/// A word-aligned reassembly buffer. Every record spans a multiple of four bytes and places its
/// frame after a four-byte prefix, so this base alignment keeps every payload on memcpy's fast
/// path. It is a measured throughput property on the strict-align LM20 target.
#[repr(C, align(4))]
struct RxBuffer<const N: usize>([u8; N]);

static mut CONTROL_RX: RxBuffer<{ buffer_len(CONTROL_RECORD_CEILING, MAX_PACKET as usize) }> =
    RxBuffer([0; buffer_len(CONTROL_RECORD_CEILING, MAX_PACKET as usize)]);

/// The stream channel's reassembly buffer, and the largest static this plane owns. The
/// `+ BULK_BURST_LEN` term is what lets the burst-armed bulk OUT endpoint keep its arming: the
/// driver hands back everything the core absorbed while the CPU was busy, and a free tail shorter
/// than one burst would make it refuse the read.
static mut STREAM_RX: RxBuffer<{ buffer_len(RECORD_CEILING, BULK_BURST_LEN) }> =
    RxBuffer([0; buffer_len(RECORD_CEILING, BULK_BURST_LEN)]);

/// USB's engine reply slot. One driver, one live call: [`Writer::call`]'s contract, honoured by
/// there being exactly one caller.
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

/// The plane's resident cost, summed into the resource report.
pub(crate) const RESIDENT_BYTES: usize = RECORD_CEILING
    + buffer_len(CONTROL_RECORD_CEILING, MAX_PACKET as usize)
    + buffer_len(RECORD_CEILING, BULK_BURST_LEN);

/// The one lane, for the life of the image.
///
/// Built once and reached from inside the driver rather than carried as a local of the USB task: a
/// lane carried across a task's awaits changes that coroutine's liveness enough for LLVM to stop
/// sinking construction temporaries out of the poll frame, which on the BLE side cost 8,628 bytes
/// of frame for eight bytes of value.
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

/// Serve records over the cable until the device is unplugged, then wait for the next cable,
/// forever. The endpoints are owned for the life of the task and re-armed across an unplug rather
/// than rebuilt, which keeps this plane's `.bss` footprint a property of the image.
pub(crate) async fn serve_objects(ctrl_in: EpIn, ctrl_out: EpOut, bulk_in: EpIn, bulk_out: EpOut) -> ! {
    // Read once, before the loop: `flat_store::arm` runs at its spawn site in `main`, which is
    // several statements ahead of this task's first poll on every boot path.
    let writer = crate::flat_store::writer();
    // SAFETY: sole writer of each buffer; `serve_objects` is the body of a task spawned once.
    let control_buf = unsafe { &mut (*core::ptr::addr_of_mut!(CONTROL_RX)).0 };
    let stream_buf = unsafe { &mut (*core::ptr::addr_of_mut!(STREAM_RX)).0 };
    let mut control = RecordReader::new(ctrl_out, control_buf, CONTROL_RECORD_CEILING, MAX_PACKET as usize);
    let mut stream = RecordReader::new(bulk_out, stream_buf, RECORD_CEILING, BULK_BURST_LEN);
    let mut control_tx = RecordWriter::new(ctrl_in);
    let mut stream_tx = RecordWriter::new(bulk_in);
    // Reached here rather than passed in: see `lane`.
    let lane = lane();
    let ceilings = CEILINGS;

    loop {
        // Before configuration, and after an unplug, the endpoints are disabled. Parking here is
        // the idle state, woken by the host's SET_CONFIGURATION.
        control.wait_enabled().await;
        let Some(writer) = writer else {
            // Reported and not unwrapped, because a link task must never be why the board panics.
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
        // The two hand-off releases are level state from the previous cable. The record streams
        // themselves are reset on the way out of a cable, below.
        CONTROL_TAKEN.reset();
        STREAM_TAKEN.reset();

        // Three siblings, and not a `select` the driver re-enters: each reader may have moved bytes
        // out of its endpoint before it suspends, so a shape that dropped one of them per pass would
        // throw those bytes away.
        let reason = match select3(
            driver(&writer, lane, &mut control_tx, &mut stream_tx),
            control_pump(&mut control),
            stream_pump(&mut stream),
        )
        .await
        {
            Either3::First(reason) | Either3::Second(reason) | Either3::Third(reason) => reason,
        };
        // Reset the record streams, then report teardown, in that order: a bad record length resets
        // that record stream before teardown is reported to the engine. Both `select3` borrows end
        // at the statement above, which is what makes the resets reachable here.
        control.reset();
        stream.reset();
        CONTROL_IN.reset();
        STREAM_IN.reset();
        // On its own reply slot, so that an orphan the driver may have left in `ENGINE_REPLY` stays
        // where `Lane::reclaim` can find it.
        let joined = release_engine(&writer).await;
        release_joined_stage(joined);
        info!("usb: [v4] link down ({}) — engine released", reason);
        if reason != RecordEnd::LinkDown.reason() {
            // Not an unplug: a framing error, or a driver failure with the endpoints still up.
            // Back off before re-arming, or a persistent one starves the ride loop.
            Timer::after_millis(200).await;
        }
    }
}

/// Release whatever the engine still holds for a link that has gone away.
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

/// Read stream records and hand them over one at a time. This is where credit withholding lives on
/// this link: one record is outstanding at a time and nothing else reads the bulk OUT endpoint, so
/// while a record is held the endpoint NAKs and the host's send loop is what stops.
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
    // Owned by the driver, so it is per cable by construction.
    let mut admission = Admission::new();
    let mut staged_request: Option<RequestId> = None;
    let mut usb_stage: Option<UsbStage> = None;
    let mut staged_started: Option<Instant> = None;
    let mut staged_bytes = 0u64;
    loop {
        // Control first when both are ready: a `CANCEL` or a `LIST` must not queue behind a stream
        // record the engine may be about to refuse anyway.
        let reaction = match embassy_futures::select::select(CONTROL_IN.wait(), STREAM_IN.wait()).await {
            embassy_futures::select::Either::First(record) => {
                // A deferred batch owns the lane, so its answer comes first. A non-idle answer
                // ended the upload: it becomes this pass's reaction, and the control record goes
                // back where the next pass takes it.
                match collect_pending(writer, lane, &mut usb_stage).await {
                    Some(Reaction::Idle) => match control_record(writer, lane, record).await {
                        Some(reaction) => reaction,
                        None => return "lane",
                    },
                    Some(reaction) => {
                        CONTROL_IN.signal(record);
                        reaction
                    }
                    None => return "lane",
                }
            }
            embassy_futures::select::Either::Second(record) => {
                let result =
                    stream_record(writer, lane, &mut admission, record, &mut staged_request, &mut usb_stage).await;
                if usb_stage.is_some() {
                    staged_started.get_or_insert_with(Instant::now);
                    staged_bytes += record.len().saturating_sub(obc_link::flat::wire::STREAM_HEADER_LEN) as u64;
                }
                match result {
                    Some(reaction) => reaction,
                    None => return "lane",
                }
            }
        };
        // Most records are `Idle`; do not put another storage round trip in the 8 KiB hot path. A
        // terminal reaction can end the USB upload while the app-facing map projection stays
        // `Receiving`, if BLE admits another map at once. That edge needs the exact query below.
        let may_have_ended = reaction != Reaction::Idle;
        if let Some(reason) = pump(writer, lane, control_tx, stream_tx, reaction).await {
            if usb_stage.is_some() {
                let joined = finish_usb_stage(writer).await;
                release_joined_stage(joined);
                log_staged_rate(staged_started, staged_bytes);
            }
            return reason;
        }
        let projection_ended = !crate::link::map_transfer_state().is_some_and(|state| state.is_receiving());
        let staged_ended = if let Some(request) = staged_request {
            (projection_ended || may_have_ended) && usb_map_upload(writer, request).await.is_none()
        } else {
            false
        };
        if staged_ended {
            // A still-outstanding deferred batch owns the lane. Its answer must be taken before
            // the stage is released, and a non-idle one still owes the host its bytes.
            match collect_pending(writer, lane, &mut usb_stage).await {
                Some(Reaction::Idle) => {}
                Some(reaction) => {
                    if let Some(reason) = pump(writer, lane, control_tx, stream_tx, reaction).await {
                        if usb_stage.is_some() {
                            let joined = finish_usb_stage(writer).await;
                            release_joined_stage(joined);
                            log_staged_rate(staged_started, staged_bytes);
                        }
                        return reason;
                    }
                }
                None => return "lane",
            }
            if usb_stage.is_some() {
                let joined = finish_usb_stage(writer).await;
                release_joined_stage(joined);
                log_staged_rate(staged_started, staged_bytes);
            }
            staged_request = None;
            usb_stage = None;
            staged_started = None;
            staged_bytes = 0;
        }
    }
}

fn log_staged_rate(started: Option<Instant>, bytes: u64) {
    let Some(started) = started else { return };
    let us = started.elapsed().as_micros().max(1);
    info!(
        "usb: [v4] staged {=u64} B in {=u64} ms ({=u64} kB/s, cable-integrity map, card DMA)",
        bytes,
        us / 1_000,
        bytes.saturating_mul(1_000) / us
    );
}

/// Hand one control record to the engine, then release the pump's buffer.
async fn control_record(writer: &Writer, lane: &mut Lane, record: &'static [u8]) -> Option<Reaction> {
    let reaction = lane.call(writer, |out| Request::Control { link: Link::Usb, record, out }).await;
    // Released here and not a statement earlier: the engine consumes the record synchronously
    // inside the storage task's `serve`, which is over by the time this call answers.
    CONTROL_TAKEN.signal(());
    reaction
}

/// Cable-local cursor for the current 64 KiB arena batch.
struct UsbStage {
    request: RequestId,
    declared: u64,
    received: u64,
    bank: usize,
    fill: usize,
    /// A mid-upload batch handed to the storage owner whose answer has not been taken yet. The
    /// collect point is a correctness rule: the serve of batch N is what joins the other bank's
    /// previous card DMA, so the driver must collect this ticket before the first byte lands in a
    /// freshly swapped bank, and before any other use of the lane.
    pending: Option<crate::flat_store::Ticket>,
}

impl UsbStage {
    fn new(request: RequestId, declared: u64) -> Self {
        Self { request, declared, received: 0, bank: 0, fill: 0, pending: None }
    }
}

/// Take the deferred batch's answer if one is outstanding, restoring the lane.
///
/// `Some(Reaction::Idle)` is the ordinary mid-upload answer. Anything else means the engine ended
/// the upload at that batch, and the caller must treat it as the pass's reaction: its bytes are in
/// the lane, exactly as after `Lane::call`. `None` means the lane is gone and the link must die.
async fn collect_pending(writer: &Writer, lane: &mut Lane, usb_stage: &mut Option<UsbStage>) -> Option<Reaction> {
    let Some(ticket) = usb_stage.as_mut().and_then(|stage| stage.pending.take()) else {
        return Some(Reaction::Idle);
    };
    lane.collect(writer, ticket).await
}

/// Hand one stream record to the engine, holding it first if nothing is admitted yet.
async fn stream_record(
    writer: &Writer,
    lane: &mut Lane,
    admission: &mut Admission,
    record: &'static [u8],
    staged_request: &mut Option<RequestId>,
    usb_stage: &mut Option<UsbStage>,
) -> Option<Reaction> {
    // The admission hold. To read four bytes of the frame header is not to parse a payload: it is
    // the record boundary information the binding is responsible for. A record too short to carry
    // one goes to the engine, which owns that refusal.
    let frame_id =
        (record.len() >= 4).then(|| RequestId(u32::from_le_bytes([record[0], record[1], record[2], record[3]])));
    if let Some(frame_id) = frame_id {
        if admission.needs_query(frame_id) && admission.observed(frame_id, live_transfer(writer).await) {
            if let embassy_futures::select::Either::First(control) =
                embassy_futures::select::select(CONTROL_IN.wait(), Timer::after(ADMISSION_WINDOW)).await
            {
                let reaction = control_record(writer, lane, control).await?;
                // The held record is re-signalled on `STREAM_IN` instead of delivered here: a held
                // frame must not be delivered before its admission, and must not be dropped. The
                // pump is still not reading behind it, so nothing overwrites the buffer meanwhile.
                // It costs one extra trip round the driver loop, which is where control records win
                // ties; to deliver from here would jump that queue.
                STREAM_IN.signal(record);
                return Some(reaction);
            }
            warn!("usb: [v4] a stream record arrived unadmitted — delivering after the hold window");
        }
    }
    if staged_request.is_none() {
        if let Some(frame_id) = frame_id {
            if let Some(declared) = usb_map_upload(writer, frame_id).await {
                *staged_request = Some(frame_id);
                if crate::usb::request_stage().await {
                    *usb_stage = Some(UsbStage::new(frame_id, declared));
                }
            }
        }
    }

    if let Some(stage) = usb_stage {
        // The bank hand-back gate. The serve of the deferred batch is what joins this bank's
        // previous card DMA, so a freshly swapped bank may not take its first byte before that
        // answer is in. A non-idle answer ended the upload: it becomes this pass's reaction and the
        // held record goes back to the pump. `pending` can only be live on a bank's first record,
        // so this gate also restores the lane before any fallback `lane.call` below.
        if stage.fill == 0 {
            if let Some(ticket) = stage.pending.take() {
                match lane.collect(writer, ticket).await {
                    Some(Reaction::Idle) => {}
                    Some(reaction) => {
                        STREAM_IN.signal(record);
                        return Some(reaction);
                    }
                    None => return None,
                }
            }
        }
        let Some((frame, payload)) = StreamFrame::split(record) else {
            let reaction = lane.call(writer, |out| Request::Stream { link: Link::Usb, record, out }).await;
            STREAM_TAKEN.signal(());
            return reaction;
        };
        if frame.transfer != stage.request {
            STREAM_TAKEN.signal(());
            return Some(Reaction::Idle);
        }
        let remaining = crate::usb::STAGE_HALF_LEN - stage.fill;
        if frame.offset != stage.received
            || stage.received + payload.len() as u64 > stage.declared
            || payload.len() > remaining
        {
            let reaction = lane.call(writer, |out| Request::Stream { link: Link::Usb, record, out }).await;
            STREAM_TAKEN.signal(());
            return reaction;
        }
        let copied = crate::arena::with_usb_stage_bank(stage.bank, |bank| {
            bank[stage.fill..stage.fill + payload.len()].copy_from_slice(payload);
        })
        .is_some();
        if !copied {
            STREAM_TAKEN.signal(());
            return Some(Reaction::Close(Channel::Stream));
        }
        stage.fill += payload.len();
        stage.received += payload.len() as u64;
        // The record buffer is no longer borrowed. Let the pump receive the next record while the
        // one-per-bank storage request below starts the deferred card write.
        STREAM_TAKEN.signal(());
        if stage.fill < crate::usb::STAGE_HALF_LEN && stage.received < stage.declared {
            return Some(Reaction::Idle);
        }
        let offset = stage.received - stage.fill as u64;
        let len = stage.fill;
        let request = stage.request;
        if stage.received >= stage.declared {
            // The final batch produces the `PUT` answer, so it is awaited inline, on an idle
            // pipeline.
            let reaction = lane.call(writer, |out| Request::StreamStagedBatch { request, offset, len, out }).await;
            stage.bank ^= 1;
            stage.fill = 0;
            return reaction;
        }
        // A mid-upload batch is fire-now-collect-later: the storage owner writes this bank to the
        // card while the pump receives the next one. The answer is taken at the swapped bank's
        // first record, above.
        stage.pending =
            lane.call_deferred(writer, |out| Request::StreamStagedBatch { request, offset, len, out }).await;
        stage.pending?;
        stage.bank ^= 1;
        stage.fill = 0;
        return Some(Reaction::Idle);
    }

    let reaction = lane.call(writer, |out| Request::Stream { link: Link::Usb, record, out }).await;
    STREAM_TAKEN.signal(());
    reaction
}

/// Whether a transfer owns the engine right now: the query the admission hold is built on.
///
/// Not a `Lane` call, because it borrows no buffer and so cannot be what loses one. It has its own
/// reply slot, because one slot per concurrently live call is a property of the types here.
static LIVE_QUERY_REPLY: Reply = Signal::new();

async fn live_transfer(writer: &Writer) -> Option<RequestId> {
    match writer.call(Request::LiveTransfer, &LIVE_QUERY_REPLY).await {
        Ok(Outcome::Live(live)) => live,
        _ => None,
    }
}

/// The storage-owned admission proof for the cable-only scratch arm. The app's map progress state
/// deliberately omits link ownership and therefore cannot distinguish a BLE map PUT from USB.
async fn usb_map_upload(writer: &Writer, request: RequestId) -> Option<u64> {
    // Sequential with `live_transfer` in this one driver, so the slot is reused instead of paying
    // a second Signal for two mutually exclusive engine queries.
    match writer.call(Request::UsbMapUpload { request }, &LIVE_QUERY_REPLY).await {
        Ok(Outcome::UsbMap(declared)) => declared,
        _ => None,
    }
}

/// Send what the reaction names, then pump until the engine goes quiet, servicing control records
/// in between, because cancel is bilateral and a download must not deafen the control channel.
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
            // An unanswerable record: emit nothing and close that record stream. The two channels
            // are two endpoint pairs of one interface, which the host enables and disables
            // together, so to close one is to end the link; the log says which channel asked.
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
                // clean boot.
                if !control_tx.send(lane.sent(len)).await {
                    return Some("send-reboot");
                }
                info!("usb: [v4] terminal response sent — rebooting");
                Timer::after_millis(50).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
        // Between iterations, not only when idle: a `GET` streams for as long as the object is
        // large, and cancel is bilateral, so a `CANCEL` sent mid-download must reach the engine
        // while there is still something to cancel.
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
