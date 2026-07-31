//! The L2CAP CoC data plane: the bulk-transfer channel the control plane ([`super::control`]) arms
//! through [`super::state::TRANSFER_ARM`].
//!
//! The CoC carries **only the object's payload bytes** (no per-chunk framing); the whole transfer state
//! machine + CRC codecs live in the host-tested [`obc_ble`] crate. One transfer at a time: the
//! [`super::state::TRANSFER_ACTIVE`] gate is cleared immediately before each terminal result is
//! notified, and a latched abort that raced completion is drained at that same boundary.
//!
//! - **Echo loopback** ([`run_echo`]): stream each SDU straight back through an [`obc_ble::Receiver`]
//!   (a running CRC-32, no reassembly buffer), verify **one** whole-object CRC — the data plane proven
//!   end to end with **zero storage**.
//! - **Route uploads** ([`run_upload`]): CoC bytes sink through the [`Receiver`] into an SD temp;
//!   commit validates (CRC + OBCR header) and atomically promotes (see `sd.rs`). Uploads don't resume:
//!   a CoC drop, a link drop, or an `op=3` abort discards the partial and the app re-sends from the
//!   start.
//! - **Downloads** ([`run_download`]): `routeList` / `rideList` / diagnostics from a store-built
//!   buffer, a route or ride detail streamed straight off the card — the announce rides the `status`
//!   envelope (`downloadAnnounce`, protocol v2) first, then raw chunks, one whole-object CRC. Rides
//!   reuse the machinery wholesale because the Finish-time save already stored each as **exactly** the
//!   wire bytes (`sd.rs`), and the diagnostics object is rendered from the link plane's own facts.
//! - Every store movement notifies `storeChanged` (status msg 2) — protocol v2's sole change signal
//!   ([`publish_store_change`]).
//!
//! On the first transfer the link is asked for the fast [`conn_params`] set (throughput); the store is
//! shared with the control plane as a `RefCell` that is **never borrowed across an `await`**.

use core::cell::RefCell;

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_time::{with_timeout, Instant, Timer};
use nrf_sdc::{self as sdc};
use obc_ble::{ObjectType, Receiver, StatusMessage, StoreChanged, TransferControl, TransferStatus};
use trouble_host::prelude::*;

use crate::link::identity;
use crate::link::{transfer_result, transfer_result_at, Armed, StatusBytes, TRANSFER_ACTIVE};
use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

use super::gatt::Server;
use super::lifecycle::{conn_params, HOST_OP_TIMEOUT};
use super::state::{battery, TRANSFER_ABORT, TRANSFER_ARM};

/// The L2CAP CoC data plane: accept the app's channel on the OBC SPSM and serve the transfers
/// [`super::control::serve_connection`] arms through [`TRANSFER_ARM`] — the echo loopback, route
/// uploads → SD, and route/list downloads ← SD. One armed transfer at a time; the [`TRANSFER_ACTIVE`]
/// gate is cleared immediately before its terminal answer, and a latched abort that raced completion
/// is drained at that same boundary. A channel drop mid-transfer breaks back to re-accept (the
/// in-flight upload was discarded — uploads restart); `select` in `run` cancels the whole task on
/// disconnect. Never returns.
pub(crate) async fn serve_coc(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
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
            // Watch the byte pipe even while no descriptor is armed. An upload
            // reject is asynchronous relative to the sender, so raw bytes may
            // already be queued; discard those unclaimed bytes. Likewise, an
            // app-side reset after a reject/cancellation must make us re-accept
            // now, not only after sacrificing the next armed transfer to the
            // stale closed channel. A valid sender waits for its GATT write ack;
            // the control task signals TRANSFER_ARM before accepting that write,
            // so its descriptor wins before its first payload can be observed.
            let armed = match select(TRANSFER_ARM.wait(), ch.receive(stack, &mut buf)).await {
                Either::First(armed) => armed,
                Either::Second(Ok(n)) if n > 0 => {
                    warn!("ble: [coc] discarded {} unclaimed bytes while idle", n);
                    continue;
                }
                Either::Second(_) => {
                    info!("ble: [coc] idle channel closed — re-accepting");
                    break;
                }
            };
            if !requested_fast {
                requested_fast = true;
                request_fast_conn_params(stack, conn).await;
            }
            let outcome = match armed {
                Armed::Echo(desc) => run_echo(stack, server, &mut ch, &desc, &mut buf).await,
                Armed::Upload(desc, rx) => run_upload(stack, server, store, shared, &mut ch, &desc, rx, &mut buf).await,
                Armed::Download(desc) => run_download(stack, server, store, shared, &mut ch, &desc, &mut buf).await,
                // Unreachable by construction: `classify_transfer` refuses every map-payload type
                // on the radio (spec §10 — a DACH-shaped volume set is 7.6–8.9 GiB), so a set
                // descriptor never arms a BLE data plane. Answered rather than `unreachable!()`,
                // because a panic here would be a reset on a link a peer can drive.
                Armed::SetShard(desc, ..) | Armed::SetManifest(desc, ..) => {
                    warn!("ble: [coc] a volume-set transfer reached the radio — refusing (spec §10)");
                    close_transfer();
                    notify_status(server, stack, transfer_result(desc.object_id, TransferStatus::Error)).await;
                    TransferOutcome::Answered
                }
            };
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

/// Close the current descriptor's ownership before publishing its terminal
/// answer. Receipt of `transferResult` is the app's permission to send the next
/// descriptor, so keeping the gate set until after the notify returned created a
/// real `busy` race. Drain only the old transfer's latched abort first; an abort
/// arriving after the clear belongs to the next armed descriptor.
fn close_transfer() {
    let _ = TRANSFER_ABORT.try_take();
    TRANSFER_ACTIVE.release(crate::link::gate_owner(crate::link::Transport::Ble));
}

/// Notify the store movement after a commit/delete: the `storeChanged` status message (which store,
/// new revision). Protocol v2 retired the `objectStore` digest characteristic — `storeChanged`
/// (status msg 2) is the sole change signal.
pub(crate) async fn publish_store_change(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
    ty: ObjectType,
) {
    // Each store keeps its own monotonic-per-boot revision (spec §4.3): a trip move stamps the trip
    // store's counter, a route/ride move the shared route/ride counter.
    let revision = if ty == ObjectType::Trip { store.borrow().trip_revision() } else { store.borrow().revision() };
    let msg = StatusMessage::StoreChanged(StoreChanged { ty, revision });
    notify_status(server, stack, msg.encode()).await;
}

/// A route upload: sink CoC bytes through the [`Receiver`] into the SD temp, then commit — CRC verify,
/// OBCR-header validate, atomic promote (see `sd.rs`) — and answer with the assigned id. Uploads don't
/// resume: a channel drop or an abort (op 3) discards the partial, and the app re-sends the object from
/// the start.
#[allow(clippy::too_many_arguments)]
async fn run_upload(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    mut rx: Receiver,
    buf: &mut [u8],
) -> TransferOutcome {
    info!("ble: [coc] upload start: {} bytes (type {})", desc.total_len, desc.ty.as_u8());
    // Open the SD temp here — at the first real byte of the transfer — rather than when the
    // control plane armed it: a peer that writes `transferControl` but never opens the CoC then
    // holds no storage handle (it only wedges its own link's one-transfer gate until it drops).
    // Each store call locks the shared card just for its own duration, then releases before the next
    // `ch.receive`/`ch.send` await — so the ride loop's map render interleaves between chunks (#270).
    // The guard is always bound *before* `store.borrow_mut()` so the RefCell borrow never spans the
    // lock's `.await`.
    let began = {
        let mut guard = shared.lock().await;
        store.borrow_mut().upload_begin(&mut guard)
    };
    if !began {
        warn!("ble: [coc] cannot open upload temp — rejecting");
        close_transfer();
        notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Error)).await;
        return TransferOutcome::Answered;
    }
    while !rx.is_complete() {
        let n = match select(ch.receive(stack, buf), TRANSFER_ABORT.wait()).await {
            Either::First(Ok(n)) if n > 0 => n,
            Either::First(_) => {
                // Error or an empty SDU with bytes still expected — the channel is done for.
                // Discard the partial; the app re-uploads from the start.
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().upload_discard(&mut guard);
                }
                info!("ble: [coc] upload interrupted — discarded (uploads restart)");
                // The peer closed this CoC to reset the unframed stream. There
                // is no live exchange left to answer, and a late `aborted`
                // could be consumed as the next descriptor's result.
                close_transfer();
                return TransferOutcome::ChannelDropped;
            }
            Either::Second(()) => {
                // The app aborted (op 3): discard and confirm.
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().upload_discard(&mut guard);
                }
                info!("ble: [coc] upload aborted by the app");
                close_transfer();
                notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::Answered;
            }
        };
        let consumed = rx.push(&buf[..n]);
        let appended = {
            let mut guard = shared.lock().await;
            store.borrow_mut().upload_append(&mut guard, &buf[..consumed])
        };
        if !appended {
            {
                let mut guard = shared.lock().await;
                store.borrow_mut().upload_discard(&mut guard);
            }
            warn!("ble: [coc] SD append failed — upload rejected");
            close_transfer();
            notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
    }
    // The commit target is the object type: a `fwImage` promotes to /UPDATE.BIN in the card root
    // (staging, no catalog id, no store-revision bump — spec §7.6); a `trip` commits into the trip
    // catalog as `TP{id}.OBT` (bumping the *trip* store, §4.3); everything else is a route into the
    // route catalog. The CoC streaming above is identical for all three.
    let is_fwimage = desc.ty == ObjectType::FwImage;
    let is_trip = desc.ty == ObjectType::Trip;
    let (id, status) = {
        let mut guard = shared.lock().await;
        let mut st = store.borrow_mut();
        if is_fwimage {
            (rx.object_id(), st.fwimage_finish(&mut guard, &rx))
        } else if is_trip {
            // `desc.crc32` is the whole-object CRC the Receiver just verified — persist it into the
            // trip content-CRC sidecar in the same commit (epic #526 TR4).
            st.upload_finish_trip(&mut guard, &rx, desc.crc32)
        } else {
            // `desc.crc32` is the whole-object CRC the Receiver just verified — persist it into the
            // route content-CRC sidecar in the same commit (epic #632 item 6).
            st.upload_finish(&mut guard, &rx, desc.crc32)
        }
    };
    let committed = status == TransferStatus::Committed;
    info!("ble: [coc] upload finished: id {} -> {}", id, if committed { "committed" } else { "rejected" });
    let offset = if committed { rx.total_len() } else { 0 };
    close_transfer();
    notify_status(server, stack, transfer_result_at(id, status, offset)).await;
    // A committed route/trip moves its object store (`storeChanged`, typed accordingly); a `fwImage`
    // stage does not — /UPDATE.BIN is not a listed object, and the install is armed later by the
    // confirmed `installFw`.
    if committed && !is_fwimage {
        let ty = if is_trip { ObjectType::Trip } else { ObjectType::Route };
        publish_store_change(stack, server, store, ty).await;
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
    shared: &SharedStoreMutex,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    buf: &mut [u8],
) -> TransferOutcome {
    // Assemble the link-plane facts the diagnostics blob renders; `download_open` only reads them for a
    // `Diagnostics` request and opens everything else through the catalog, so the
    // runner has one open path. Bind the open's result before matching — a
    // `match store.borrow_mut().…` scrutinee temporary would keep the borrow alive through the
    // error arm's await.
    let fw = identity::firmware_revision();
    let serial = identity::serial_string();
    let diag = crate::link::diag_input(fw.as_str(), serial.as_str(), Instant::now().as_secs() as u32);
    let opened = {
        let mut guard = shared.lock().await;
        store.borrow_mut().download_open(&mut guard, desc, &diag)
    };
    let (mut tx, source) = match opened {
        Ok(open) => open,
        Err(status) => {
            close_transfer();
            notify_status(server, stack, transfer_result(desc.object_id, status)).await;
            return TransferOutcome::Answered;
        }
    };
    // Announce as a `downloadAnnounce` status message (protocol v2): the 12-byte descriptor with
    // `total_len` + `crc32` filled in, wrapped in the `status` envelope (`msg = 4`), then stream.
    // v2 folds the announce off `transferControl` and onto `status`, so the whole device → app
    // control channel is one CCCD — the split-CCCD failure mode (announce on one CCCD, result on
    // another) is gone, and with it the recovery notify that used to answer it.
    let announce = tx.announce();
    info!("ble: [coc] download start: {} bytes", announce.total_len);
    let (announce_buf, announce_len) = StatusMessage::DownloadAnnounce(announce).encode();
    // The announce carries the size + CRC the app streams against — `HOST_OP_TIMEOUT`-bounded like every
    // host op, and a timeout is treated exactly like a failure (the app never sees a stream start).
    let announced = matches!(
        with_timeout(HOST_OP_TIMEOUT, server.notify(stack, server.obc.status.handle, &announce_buf[..announce_len]))
            .await,
        Ok(Ok(()))
    );
    if !announced {
        // The announce rides the same `status` CCCD every other result does, so if it can't land,
        // a follow-up notify on that characteristic can't either — close and abandon; the app's own
        // download request times out (no cross-CCCD recovery notify to send).
        warn!("ble: [coc] download announce notify failed/timed out — abandoning download");
        {
            let mut guard = shared.lock().await;
            store.borrow_mut().download_close(&mut guard);
        }
        close_transfer();
        return TransferOutcome::Answered;
    }
    while !tx.is_complete() {
        if TRANSFER_ABORT.try_take().is_some() {
            {
                let mut guard = shared.lock().await;
                store.borrow_mut().download_close(&mut guard);
            }
            info!("ble: [coc] download aborted by the app");
            close_transfer();
            notify_status(server, stack, transfer_result_at(desc.object_id, TransferStatus::Aborted, tx.position()))
                .await;
            return TransferOutcome::Answered;
        }
        let n = tx.next_chunk_len(CHUNK_LEN.min(buf.len()));
        let read_ok = {
            let guard = shared.lock().await;
            store.borrow().download_read(&guard, source, tx.position(), &mut buf[..n])
        };
        if !read_ok {
            {
                let mut guard = shared.lock().await;
                store.borrow_mut().download_close(&mut guard);
            }
            warn!("ble: [coc] SD read failed — download abandoned");
            close_transfer();
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
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().download_close(&mut guard);
                }
                close_transfer();
                return TransferOutcome::ChannelDropped;
            }
            Either::Second(()) => {
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().download_close(&mut guard);
                }
                info!("ble: [coc] download aborted by the app (mid-send)");
                close_transfer();
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
    {
        let mut guard = shared.lock().await;
        let mut st = store.borrow_mut();
        st.download_close(&mut guard);
        // A **ride** download that reached completion is the unsynced-guard's commit point (epic #447
        // P7 / #454): flag this ride id as "downloaded at least once" in the `/tracks` synced sidecar
        // so the Rides screen drops its "not synced" delete cue. A no-op if already flagged; when it
        // flips it bumps the store revision, and the ride loop's rescan re-feeds the freshened flag.
        if desc.ty == ObjectType::Ride {
            st.mark_ride_synced(&mut guard, desc.object_id);
        }
    }
    let result = tx.outcome().unwrap(); // complete ⇒ Some
    info!("ble: [coc] download done: {} bytes", result.committed_offset);
    close_transfer();
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
            // A nonsensical echo descriptor (the wrong op — echo restarts, never resumes; v2 has no
            // offset to reject) — answer error, leave the channel untouched (no bytes were promised).
            close_transfer();
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
                close_transfer();
                return TransferOutcome::ChannelDropped;
            }
            Ok(n) => n,
            Err(e) => {
                info!("ble: [coc] echo receive ended: {:?}", defmt::Debug2Format(&e));
                close_transfer();
                return TransferOutcome::ChannelDropped;
            }
        };
        let consumed = rx.push(&buf[..n]);
        if let Err(e) = ch.send(stack, &buf[..consumed]).await {
            info!("ble: [coc] echo send failed: {:?}", defmt::Debug2Format(&e));
            close_transfer();
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
    close_transfer();
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
