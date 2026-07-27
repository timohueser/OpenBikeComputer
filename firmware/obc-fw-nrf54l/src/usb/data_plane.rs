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
use core::sync::atomic::Ordering;

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use embassy_usb::driver::{Endpoint as _, EndpointIn, EndpointOut};
use obc_ble::{ObjectType, Receiver, StatusMessage, TransferControl, TransferStatus};

use crate::link::identity;
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
    TRANSFER_ACTIVE.store(false, Ordering::Relaxed);
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
            let armed = match select(TRANSFER_ARM.wait(), ep_out.read(buf)).await {
                Either::First(armed) => armed,
                Either::Second(Ok(n)) if n > 0 => {
                    warn!("usb: [bulk] discarded {} unclaimed bytes while idle", n);
                    continue;
                }
                Either::Second(Ok(_)) => continue, // a zero-length packet is not data
                Either::Second(Err(e)) => {
                    info!("usb: [bulk] idle read ended: {:?} — re-arming", defmt::Debug2Format(&e));
                    break;
                }
            };
            let outcome = match armed {
                Armed::Echo(desc) => run_echo(tx, &mut ep_in, &mut ep_out, &desc, buf).await,
                Armed::Upload(desc, rx) => run_upload(tx, &mut ep_out, store, shared, &desc, rx, buf).await,
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
        {
            let mut guard = shared.lock().await;
            store.borrow_mut().link_reset(&mut guard);
        }
        TRANSFER_ACTIVE.store(false, Ordering::Relaxed);
        TRANSFER_ARM.reset();
        TRANSFER_ABORT.reset();
        // `wait_enabled` returns immediately while the endpoint is still up, so a *persistent*
        // driver-level error would hot-spin this loop — and on a cooperative executor that starves
        // the ride loop, freezing the map. Back off a beat, like the BLE CoC accept loop.
        Timer::after_millis(200).await;
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
    buf: &mut [u8],
) -> TransferOutcome {
    info!("usb: [bulk] upload start: {} bytes (type {})", desc.total_len, desc.ty.as_u8());
    // A **map** (#927) is the one type that does not stream into `/routes/UPLOAD.TMP`: at hundreds
    // of megabytes the temp-then-copy promote would double both the minutes of writing and the free
    // space required, so a map streams straight into its final `MP{id}.OBM` with its first four bytes
    // — the OBCM magic — withheld here and patched in at commit. `map_id` is the assigned object id,
    // carried in this frame because a map holds no slot in the store to remember it in.
    let is_map = desc.ty == ObjectType::Map;
    let mut held = obc_ble::HeldMagic::new();
    let mut map_id = 0u16;
    // Open the SD file here — at the first real byte — rather than when the control plane armed it:
    // a host that sends `transferControl` and then never writes holds no storage handle (it only
    // wedges its own one-transfer gate until it unplugs).
    let began = {
        let mut guard = shared.lock().await;
        if is_map {
            match store.borrow_mut().map_upload_begin(&mut guard) {
                Some(id) => {
                    map_id = id;
                    true
                }
                None => false,
            }
        } else {
            store.borrow_mut().upload_begin(&mut guard)
        }
    };
    if !began {
        warn!("usb: [bulk] cannot open the upload target — rejecting");
        if is_map {
            crate::link::map_transfer_storage_failed();
        }
        close_transfer();
        tx.send_status(transfer_result(rx.object_id(), TransferStatus::Error)).await;
        return TransferOutcome::Answered;
    }
    if is_map {
        // Raise the on-glass card now: from here the SD bus is saturated for minutes and the map
        // plane's own reads queue behind this transfer. Unexplained, that reads as a wedged device.
        crate::link::map_transfer_started(rx.total_len());
    }
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
                    discard_upload(&mut store.borrow_mut(), &mut guard, is_map);
                }
                info!("usb: [bulk] upload interrupted ({:?}) — discarded", defmt::Debug2Format(&e));
                close_transfer();
                return TransferOutcome::LinkDropped;
            }
            Either::Second(()) => {
                // The host aborted (op 3): discard and confirm.
                {
                    let mut guard = shared.lock().await;
                    discard_upload(&mut store.borrow_mut(), &mut guard, is_map);
                }
                info!("usb: [bulk] upload aborted by the host");
                close_transfer();
                tx.send_status(transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::Answered;
            }
        };
        let consumed = rx.push(&buf[..n]);
        // The receiver's CRC always sees every payload byte; only the *write* skips the held magic.
        let write = if is_map { held.feed(&buf[..consumed]) } else { &buf[..consumed] };
        let appended = write.is_empty() || {
            let mut guard = shared.lock().await;
            store.borrow_mut().upload_append(&mut guard, write)
        };
        if !appended {
            {
                let mut guard = shared.lock().await;
                discard_upload(&mut store.borrow_mut(), &mut guard, is_map);
            }
            warn!("usb: [bulk] SD append failed — upload rejected");
            if is_map {
                crate::link::map_transfer_storage_failed();
            }
            close_transfer();
            tx.send_status(transfer_result(rx.object_id(), TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
        if is_map {
            crate::link::map_transfer_progress(rx.committed_offset());
        }
    }
    // The commit target is the object type: a `fwImage` promotes to /UPDATE.BIN in the card root
    // (staging, no catalog id, no store-revision bump — spec §7.6); a `trip` commits into the trip
    // catalog as `TP{id}.OBT` (bumping the *trip* store, §4.3); a `map` patches the held magic into
    // `MP{id}.OBM` and becomes the selected map (#927); everything else is a route.
    let is_fwimage = desc.ty == ObjectType::FwImage;
    let is_trip = desc.ty == ObjectType::Trip;
    let (id, status) = {
        let mut guard = shared.lock().await;
        let mut st = store.borrow_mut();
        if is_map {
            // A map shorter than a magic can't reach here — the announce guard rejects anything
            // below a full OBCM header — but the codec is total, so answer `error` rather than
            // fabricate one.
            let status = match held.take() {
                Some(magic) => st.map_upload_finish(&mut guard, &rx, map_id, magic),
                None => TransferStatus::Error,
            };
            (map_id, status)
        } else if is_fwimage {
            (rx.object_id(), st.fwimage_finish(&mut guard, &rx))
        } else if is_trip {
            st.upload_finish_trip(&mut guard, &rx, desc.crc32)
        } else {
            st.upload_finish(&mut guard, &rx, desc.crc32)
        }
    };
    if is_map {
        crate::link::map_transfer_ended(Some(status));
    }
    let committed = status == TransferStatus::Committed;
    let elapsed_ms = started.elapsed().as_millis().max(1);
    if committed && is_map {
        info!("usb: [bulk] map {} is now the selected map — it loads on the next boot", id);
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
    if committed && !is_fwimage && !is_map {
        let ty = if is_trip { ObjectType::Trip } else { ObjectType::Route };
        tx.publish_store_change(store, ty).await;
    }
    TransferOutcome::Answered
}

/// Drop an in-flight upload's partial. A map's partial is its final file with the magic still
/// zeroed — inert to every catalog and reclaimed by the boot sweep — so it is *closed*, not deleted:
/// erasing hundreds of megabytes on the failure path would add minutes to a transfer that has
/// already failed, and a retry truncates the same file anyway (the id re-derives to the same value,
/// since the scan cannot see a zero-magic map). Every other type drops its `UPLOAD.TMP`.
fn discard_upload(store: &mut ObjectStore, shared: &mut crate::SharedStore, is_map: bool) {
    if is_map {
        crate::link::map_transfer_ended(None);
        if let Some(storage) = &mut shared.storage {
            storage.map_upload_abort();
        }
    } else {
        store.upload_discard(shared);
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
        let n = match ep_out.read(buf).await {
            Ok(0) => continue, // a zero-length packet is not data
            Ok(n) => n,
            Err(e) => {
                info!("usb: [bulk] echo receive ended: {:?}", defmt::Debug2Format(&e));
                close_transfer();
                return TransferOutcome::LinkDropped;
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
