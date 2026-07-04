//! The L2CAP CoC data plane: the bulk-transfer channel the control plane ([`super::control`]) arms
//! through [`super::state::TRANSFER_ARM`].
//!
//! The CoC carries **only the object's payload bytes** (no per-chunk framing); the whole transfer state
//! machine + CRC codecs live in the host-tested [`obc_ble`] crate. One transfer at a time: the
//! [`super::state::TRANSFER_ACTIVE`] gate is cleared here when each concludes, and a latched abort that
//! raced a completion is drained so it can't leak into the next transfer.
//!
//! - **Echo loopback** ([`run_echo`]): stream each SDU straight back through an [`obc_ble::Receiver`]
//!   (a running CRC-32, no reassembly buffer), verify **one** whole-object CRC — the data plane proven
//!   end to end with **zero storage**.
//! - **Route uploads** ([`run_upload`]): CoC bytes sink through the [`Receiver`] into an SD temp;
//!   commit validates (CRC + OBCR header) and atomically promotes (see `sd.rs`). Uploads don't resume:
//!   a CoC drop, a link drop, or an `op=3` abort discards the partial and the app re-sends from the
//!   start.
//! - **Downloads** ([`run_download`]): `routeList` / `rideList` / diagnostics from a store-built
//!   buffer, a route or ride detail streamed straight off the card — announce descriptor first, then
//!   raw chunks, one whole-object CRC. Rides reuse the machinery wholesale because the Finish-time save
//!   already stored each as **exactly** the wire bytes (`sd.rs`), and the diagnostics object is
//!   rendered from the link plane's own facts.
//! - Every store movement notifies `storeChanged` + the refreshed `objectStore` digest
//!   ([`publish_store_change`]).
//!
//! On the first transfer the link is asked for the fast [`conn_params`] set (throughput); the store is
//! shared with the control plane as a `RefCell` that is **never borrowed across an `await`**.

use core::cell::RefCell;
use core::sync::atomic::Ordering;

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_time::{with_timeout, Instant, Timer};
use nrf_sdc::{self as sdc};
use obc_ble::{ObjectType, Receiver, StatusMessage, StoreChanged, TransferControl, TransferStatus};
use trouble_host::prelude::*;

use crate::object_store::{DiagInput, ObjectStore};

use super::gatt::{firmware_revision, serial_string, Server, HARDWARE_REVISION};
use super::lifecycle::{conn_params, HOST_OP_TIMEOUT};
use super::state::{
    battery, stack_high_water, status, transfer_result, transfer_result_at, Armed, StatusBytes, TRANSFER_ABORT,
    TRANSFER_ACTIVE, TRANSFER_ARM,
};

/// The L2CAP CoC data plane: accept the app's channel on the OBC SPSM and serve the transfers
/// [`super::control::serve_connection`] arms through [`TRANSFER_ARM`] — the echo loopback, route
/// uploads → SD, and route/list downloads ← SD. One armed transfer at a time; the [`TRANSFER_ACTIVE`]
/// gate is cleared here when each concludes, and a latched abort that raced a completion is drained so
/// it can't leak into the next transfer. A channel drop mid-transfer breaks back to re-accept (the
/// in-flight upload was discarded — uploads restart); `select` in `run` cancels the whole task on
/// disconnect. Never returns.
pub(crate) async fn serve_coc(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &RefCell<ObjectStore>,
) -> ! {
    let listener = L2capChannel::listen(stack, conn.raw());
    // The receive buffer must be ≥ the negotiated SDU MTU (defaults to the pool MTU − 6 = 245).
    let mut buf = [0u8; DefaultPacketPool::MTU];
    // Ask for the fast connection-parameter set once, on the first transfer of the link — the idle set
    // is re-established on the next connect, so there's no per-transfer churn.
    let mut requested_fast = false;
    loop {
        let mut ch = match listener.accept(&L2capChannelConfig::default()).await {
            Ok(ch) => ch,
            Err(e) => {
                // A failed accept while the link is up shouldn't hot-spin — back off a beat. On a
                // real disconnect the `run` `select` has already dropped this future.
                warn!("ble: [coc] accept failed: {:?}", defmt::Debug2Format(&e));
                Timer::after_millis(200).await;
                continue;
            }
        };
        // The CoC requires an encrypted link: opening it plaintext is refused. In practice the app
        // can't reach here unencrypted — `psm`/`transferControl` are both `authenticated` — but a peer
        // that guessed the SPSM must still be turned away.
        if !matches!(conn.raw().security_level(), Ok(level) if level.encrypted()) {
            warn!("ble: [coc] channel opened on an unencrypted link — refusing (S0 §8)");
            ch.disconnect();
            continue;
        }
        info!("ble: [coc] channel accepted (mtu {} mps {}) — data plane ready", ch.mtu(), ch.mps());
        loop {
            let armed = TRANSFER_ARM.wait().await;
            if !requested_fast {
                requested_fast = true;
                request_fast_conn_params(stack, conn).await;
            }
            let outcome = match armed {
                Armed::Echo(desc) => run_echo(stack, server, &mut ch, &desc, &mut buf).await,
                Armed::Upload(desc, rx) => run_upload(stack, server, store, &mut ch, &desc, rx, &mut buf).await,
                Armed::Download(desc) => run_download(stack, server, store, &mut ch, &desc, &mut buf).await,
            };
            // The transfer concluded (or the channel died): reopen the gate, and drain an abort
            // that raced the conclusion so it can't insta-abort the next transfer.
            TRANSFER_ACTIVE.store(false, Ordering::Relaxed);
            let _ = TRANSFER_ABORT.try_take();
            if let TransferOutcome::ChannelDropped = outcome {
                warn!("ble: [coc] channel dropped mid-transfer — re-accepting (uploads restart)");
                break;
            }
        }
    }
}

/// Whether a transfer runner answered on `status` or the CoC dropped under it (→ [`serve_coc`]
/// re-accepts; a re-upload arrives as a fresh arm).
enum TransferOutcome {
    Answered,
    ChannelDropped,
}

/// Notify the store movement after a commit/delete: the `storeChanged` status message (which store,
/// new revision) + the refreshed `objectStore` digest characteristic.
pub(crate) async fn publish_store_change(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
) {
    let digest = store.borrow().digest();
    let bytes = digest.encode();
    let _ = server.set(&server.obc.object_store, &bytes);
    notify_bounded(stack, server, server.obc.object_store.handle, &bytes, "digest").await;
    let msg = StatusMessage::StoreChanged(StoreChanged { ty: ObjectType::Route, revision: digest.revision });
    notify_status(server, stack, msg.encode()).await;
}

/// A route upload: sink CoC bytes through the [`Receiver`] into the SD temp, then commit — CRC verify,
/// OBCR-header validate, atomic promote (see `sd.rs`) — and answer with the assigned id. Uploads don't
/// resume: a channel drop or an abort (op 3) discards the partial, and the app re-sends the object from
/// the start.
async fn run_upload(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    mut rx: Receiver,
    buf: &mut [u8],
) -> TransferOutcome {
    info!("ble: [coc] route upload start: {} bytes", desc.total_len);
    // Open the SD temp here — at the first real byte of the transfer — rather than when the
    // control plane armed it: a peer that writes `transferControl` but never opens the CoC then
    // holds no storage handle (it only wedges its own link's one-transfer gate until it drops).
    if !store.borrow_mut().upload_begin() {
        warn!("ble: [coc] cannot open upload temp — rejecting");
        notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Error)).await;
        return TransferOutcome::Answered;
    }
    while !rx.is_complete() {
        let n = match select(ch.receive(stack, buf), TRANSFER_ABORT.wait()).await {
            Either::First(Ok(n)) if n > 0 => n,
            Either::First(_) => {
                // Error or an empty SDU with bytes still expected — the channel is done for.
                // Discard the partial; the app re-uploads from the start.
                store.borrow_mut().upload_discard();
                info!("ble: [coc] upload interrupted — discarded (uploads restart)");
                notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::ChannelDropped;
            }
            Either::Second(()) => {
                // The app aborted (op 3): discard and confirm.
                store.borrow_mut().upload_discard();
                info!("ble: [coc] upload aborted by the app");
                notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::Answered;
            }
        };
        let consumed = rx.push(&buf[..n]);
        if !store.borrow_mut().upload_append(&buf[..consumed]) {
            store.borrow_mut().upload_discard();
            warn!("ble: [coc] SD append failed — upload rejected");
            notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
    }
    let (id, status) = store.borrow_mut().upload_finish(&rx);
    let committed = status == TransferStatus::Committed;
    info!("ble: [coc] upload finished: id {} -> {}", id, if committed { "committed" } else { "rejected" });
    let offset = if committed { rx.total_len() } else { 0 };
    notify_status(server, stack, transfer_result_at(id, status, offset)).await;
    if committed {
        publish_store_change(stack, server, store).await;
    }
    TransferOutcome::Answered
}

/// A download: open the source (`routeList` / `rideList` / diagnostics from the store's built buffer, a
/// route or ride detail straight off the card with its CRC pre-pass), notify the filled announce
/// descriptor, then stream the object in CoC chunks. An abort between chunks stops cleanly; a send
/// failure means the channel dropped (the app re-requests).
async fn run_download(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    buf: &mut [u8],
) -> TransferOutcome {
    // Assemble the link-plane facts the diagnostics blob renders; `download_open` only reads them for a
    // `Diagnostics` request and opens everything else through the catalog, so the
    // runner has one open path. Bind the open's result before matching — a
    // `match store.borrow_mut().…` scrutinee temporary would keep the borrow alive through the
    // error arm's await.
    let fw = firmware_revision();
    let serial = serial_string();
    let s = status();
    let diag = DiagInput {
        firmware: fw.as_str(),
        hardware: HARDWARE_REVISION,
        serial: serial.as_str(),
        uptime_s: Instant::now().as_secs() as u32,
        connects: s.connects,
        disconnects: s.disconnects,
        last_disconnect_reason: s.last_disconnect_reason,
        stack_hw: stack_high_water(),
    };
    let opened = store.borrow_mut().download_open(desc, &diag);
    let (mut tx, source) = match opened {
        Ok(open) => open,
        Err(status) => {
            notify_status(server, stack, transfer_result(desc.object_id, status)).await;
            return TransferOutcome::Answered;
        }
    };
    // Announce on `transferControl` (same 16 bytes, total_len + crc32 filled in), then stream.
    let announce = tx.announce();
    info!("ble: [coc] download start: {} bytes from offset {}", announce.total_len, announce.offset);
    // The announce carries the size + CRC the app streams against — `HOST_OP_TIMEOUT`-bounded like every
    // host op, and a timeout is treated exactly like a failure (the app never sees a stream start).
    let announced = matches!(
        with_timeout(HOST_OP_TIMEOUT, server.notify(stack, server.obc.transfer_control.handle, &announce.encode()))
            .await,
        Ok(Ok(()))
    );
    if !announced {
        warn!("ble: [coc] announce notify failed/timed out — abandoning download");
        store.borrow_mut().download_close();
        // Still answer the transfer the app opened — a `status` notify can land even when the
        // `transferControl` notify didn't (different CCCD), so the app isn't left waiting for a
        // stream that will never start.
        notify_status(server, stack, transfer_result(desc.object_id, TransferStatus::Error)).await;
        return TransferOutcome::Answered;
    }
    while !tx.is_complete() {
        if TRANSFER_ABORT.try_take().is_some() {
            store.borrow_mut().download_close();
            info!("ble: [coc] download aborted by the app");
            notify_status(server, stack, transfer_result_at(desc.object_id, TransferStatus::Aborted, tx.position()))
                .await;
            return TransferOutcome::Answered;
        }
        let n = tx.next_chunk_len(CHUNK_LEN.min(buf.len()));
        if !store.borrow_mut().download_read(source, tx.position(), &mut buf[..n]) {
            store.borrow_mut().download_close();
            warn!("ble: [coc] SD read failed — download abandoned");
            notify_status(server, stack, transfer_result(desc.object_id, TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
        // Race the send against an abort so a backpressured send (peer stops crediting the CoC)
        // can still be cancelled promptly, not just between chunks — the receive side already
        // selects on the abort, so this keeps both directions honouring op=3 mid-SDU.
        match select(ch.send(stack, &buf[..n]), TRANSFER_ABORT.wait()).await {
            Either::First(Ok(())) => {}
            Either::First(Err(e)) => {
                info!("ble: [coc] download send ended: {:?}", defmt::Debug2Format(&e));
                store.borrow_mut().download_close();
                return TransferOutcome::ChannelDropped;
            }
            Either::Second(()) => {
                store.borrow_mut().download_close();
                info!("ble: [coc] download aborted by the app (mid-send)");
                notify_status(
                    server,
                    stack,
                    transfer_result_at(desc.object_id, TransferStatus::Aborted, tx.position()),
                )
                .await;
                return TransferOutcome::Answered;
            }
        }
        tx.advance(n);
    }
    store.borrow_mut().download_close();
    let result = tx.outcome().unwrap(); // complete ⇒ Some
    info!("ble: [coc] download done: {} bytes", result.committed_offset);
    notify_status(server, stack, StatusMessage::TransferResult(result).encode()).await;
    TransferOutcome::Answered
}

/// One CoC SDU's worth of download payload (244 rides one 251-byte PDU on a DLE link).
const CHUNK_LEN: usize = 244;

/// The echo loopback: receive the announced object over the CoC and stream it straight back,
/// byte-for-byte, verifying **one** whole-object CRC-32 at the end — the data plane proven with zero
/// storage. Sinks each SDU through an [`obc_ble::Receiver`] (a running CRC, no reassembly buffer) and
/// echoes exactly the consumed bytes; on completion notifies the `transferResult` (`committed` /
/// `crcMismatch`) and logs the throughput.
async fn run_echo(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    buf: &mut [u8],
) -> TransferOutcome {
    let mut rx = match Receiver::new(desc) {
        Ok(rx) => rx,
        Err(_) => {
            // A nonsensical echo descriptor (e.g. offset past total_len) — answer error, leave the
            // channel untouched (no bytes were promised).
            notify_status(server, stack, transfer_result(desc.object_id, TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
    };
    info!("ble: [coc] echo start: {} bytes", rx.total_len());
    let started = Instant::now();
    while !rx.is_complete() {
        let n = match ch.receive(stack, buf).await {
            Ok(0) => {
                // An empty SDU can't advance a transfer with bytes still expected — treat it as an
                // end-of-stream rather than spinning the receive loop.
                info!("ble: [coc] echo receive returned 0 bytes — ending");
                return TransferOutcome::ChannelDropped;
            }
            Ok(n) => n,
            Err(e) => {
                info!("ble: [coc] echo receive ended: {:?}", defmt::Debug2Format(&e));
                return TransferOutcome::ChannelDropped;
            }
        };
        let consumed = rx.push(&buf[..n]);
        if let Err(e) = ch.send(stack, &buf[..consumed]).await {
            info!("ble: [coc] echo send failed: {:?}", defmt::Debug2Format(&e));
            return TransferOutcome::ChannelDropped;
        }
    }
    let result = rx.outcome().unwrap(); // complete ⇒ Some
    let committed = result.status == TransferStatus::Committed;
    let elapsed_ms = (started.elapsed().as_millis()).max(1);
    // kB/s = bytes / seconds / 1024; kept in u64 (bytes × 1000 can't overflow a real object).
    let kbps = (rx.total_len() as u64) * 1000 / (elapsed_ms * 1024);
    info!(
        "ble: [coc] echo done: {} bytes in {} ms (~{} kB/s) -> {}",
        rx.total_len(),
        elapsed_ms,
        kbps,
        if committed { "committed" } else { "crcMismatch" }
    );
    notify_status(server, stack, StatusMessage::TransferResult(result).encode()).await;
    TransferOutcome::Answered
}

/// Notify one `status` message (the CoC data plane's channel to the app), `HOST_OP_TIMEOUT`-bounded.
async fn notify_status(
    server: &Server<'_>,
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    (buf, len): StatusBytes,
) {
    notify_bounded(stack, server, server.obc.status.handle, &buf[..len], "status").await;
}

/// One host notify, [`HOST_OP_TIMEOUT`]-bounded so a peer that stops draining its ATT queue can't stall
/// a plane's task past the link's supervision timeout — the structural backstop beneath the hardware
/// watchdog (see `lifecycle`'s watchdog policy; #277/A9). A timeout or error is logged and abandoned:
/// the caller's state machine moves on, since a lost notification is the app's to recover by re-reading,
/// never a reason to wedge the link.
pub(crate) async fn notify_bounded(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    handle: u16,
    bytes: &[u8],
    what: &str,
) {
    match with_timeout(HOST_OP_TIMEOUT, server.notify(stack, handle, bytes)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("ble: [coc] {} notify failed: {:?}", what, defmt::Debug2Format(&e)),
        Err(_) => warn!("ble: [coc] {} notify timed out — abandoning", what),
    }
}

/// Request the fast connection-parameter set for a transfer's throughput — best-effort and
/// timeout-bounded like [`super::lifecycle::negotiate_link`]'s requests (a peer that ignores it just
/// runs slower).
async fn request_fast_conn_params(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) {
    let raw = conn.raw();
    if !raw.is_connected() {
        return;
    }
    let params = conn_params(true);
    match with_timeout(HOST_OP_TIMEOUT, raw.update_connection_params(stack, &params)).await {
        Ok(Ok(())) => info!("ble: [coc] requested fast conn params for transfer"),
        Ok(Err(e)) => warn!("ble: [coc] fast conn params failed: {:?}", defmt::Debug2Format(&e)),
        Err(_) => warn!("ble: [coc] fast conn params timed out"),
    }
}

/// Push the BAS battery level to a subscribed central: seed on connect, then re-notify on a slow
/// cadence. The value comes from the `FuelGauge` seam via [`super::state::publish_battery`] (the status
/// plane owns the gauge; the stub is constant today). Never returns; cancelled by `select` in `run` on
/// disconnect.
pub(crate) async fn battery_task(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) -> ! {
    let level = server.bas.level;
    loop {
        let pct = battery();
        let _ = conn.set(&level, &pct); // keep the readable value in step with the notify
        notify_bounded(stack, server, level.handle, &[pct], "battery").await;
        Timer::after_secs(30).await;
    }
}
