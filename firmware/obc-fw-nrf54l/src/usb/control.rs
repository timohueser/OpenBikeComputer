//! The USB **control envelope** and the frame loop that serves it.
//!
//! ## The one thing USB adds to the interface spec
//!
//! BLE's control plane is GATT: seven separately-addressed characteristics, where *which
//! characteristic* is carried by the transport rather than by any byte of ours. USB has one
//! endpoint pair, so that routing has to become a byte on the wire — and one byte is all it
//! becomes.
//!
//! **Every control frame is `selector u8 · payload`, where the payload is the exact bytes the
//! corresponding GATT characteristic carries** — the same bytes `protocol-vectors/` pins and that
//! the firmware and iOS already encode. Nothing about the object model, the descriptors, the status
//! envelope, the commands or the CRC changes.
//!
//! This envelope was proposed by C3 (#902, merged) and built against a loopback pipe before this
//! device side existed; #889 owns the decision, and **it is hereby ratified unchanged**. The
//! alternatives were considered and rejected:
//!
//! - *A separate endpoint pair per characteristic* — seven pairs is 14 endpoints against the
//!   core's 16, for no gain: the selector byte costs one byte per control message, on a channel
//!   that carries a handful of messages per session.
//! - *CDC-ACM + Web Serial* — binds automatically on every OS with no MS OS descriptors and no
//!   udev rules, which is a real advantage. But CDC is a *stream*: it has no message boundaries, so
//!   the unframed bulk plane would need framing invented for it — exactly the property principle #2
//!   is built on. It also hands the device a modem abstraction (line coding, DTR/RTS) that means
//!   nothing here, and on macOS it spawns a `/dev/cu.*` that unrelated software probes. A vendor
//!   interface keeps the byte pipe raw; the Windows cost is solved by the MS OS 2.0 descriptors in
//!   [`super`], and the Linux cost is a udev rule for the desktop app (#909) that the browser does
//!   not need.
//!
//! ## Frame boundaries
//!
//! One frame is exactly one USB transfer, in both directions. That is why a frame must be strictly
//! shorter than the endpoint's max packet: at exactly the packet size the peer could not tell the
//! frame had ended without a zero-length packet. The longest frame the protocol produces is a
//! `config` write at ~130 bytes, against a 512-byte endpoint.
//!
//! Device → host, the [`DeviceFrame::Status`] selector carries the §4.3 status envelope verbatim,
//! discriminator byte included, and it is the **sole unsolicited channel** — exactly as on BLE,
//! where every device → app control message shares the one `status` CCCD. One ordering domain.

use core::cell::RefCell;

use defmt::{info, warn};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{with_timeout, Duration};
use embassy_usb::driver::{Endpoint as _, EndpointError, EndpointIn, EndpointOut};
use obc_ble::{ObjectType, StatusMessage, StoreChanged};

use crate::link::command::run_command;
use crate::link::identity;
use crate::link::transfer::{classify_transfer, TransferDisposition};
use crate::link::{StatusBytes, TRANSFER_ACTIVE};
use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

use super::data_plane::{TRANSFER_ABORT, TRANSFER_ARM};
use super::{EpIn, EpOut};

// ============================ The envelope ============================

/// Host → device selectors. Each names the GATT characteristic the payload would have been written
/// to (spec §3.3).
mod host_frame {
    /// `command` — a §4.4 imperative.
    pub const COMMAND: u8 = 1;
    /// `transferControl` — the §4.2 12-byte descriptor.
    pub const TRANSFER_CONTROL: u8 = 2;
    /// `config` write — the §7.3 blob.
    pub const CONFIG_WRITE: u8 = 3;
    /// `protocolVersion` read (§1). No payload.
    pub const IDENTITY_READ: u8 = 4;
    /// The Device Information Service strings (§3.1). No payload.
    pub const DEVICE_INFO_READ: u8 = 5;
    /// `config` read (§7.3). No payload.
    pub const CONFIG_READ: u8 = 6;
}

/// Device → host selectors.
pub(crate) mod device_frame {
    /// `status` — the §4.3 envelope verbatim. The sole unsolicited channel.
    pub const STATUS: u8 = 1;
    /// The answer to an identity read: the §1 bytes, 6 with a store, 2 without.
    pub const IDENTITY: u8 = 2;
    /// The answer to a device-info read: `len u8 · UTF-8`, three times, firmware · hardware · serial.
    pub const DEVICE_INFO: u8 = 3;
    /// The answer to a config read: the §7.3 blob.
    pub const CONFIG: u8 = 4;
}

/// Timeout on one control-frame send. A USB IN transfer completes when the host reads it, so a host
/// that stops draining this endpoint — a closed tab that never released the interface — would
/// otherwise park the sender forever and wedge whichever plane holds the mutex. The BLE side bounds
/// its notifies for the same reason; a lost control message is the host's to recover from by
/// re-asking, never a reason to wedge the link.
const HOST_OP_TIMEOUT: Duration = Duration::from_secs(5);

// ============================ The device → host writer ============================

/// The control IN endpoint, shared by the control loop (replies) and the data plane (download
/// announces, terminal transfer results, `storeChanged`).
///
/// `NoopRawMutex` suffices: both users are cooperative futures on the one thread-mode executor and
/// no ISR sends frames. Holding the mutex across a send is what keeps a frame atomic — a half-written
/// frame interleaved with another's would be unparseable, since the selector is the only framing.
pub(crate) struct ControlTx {
    ep: Mutex<NoopRawMutex, EpIn>,
}

impl ControlTx {
    pub(crate) fn new(ep: EpIn) -> Self {
        Self { ep: Mutex::new(ep) }
    }

    /// Send one `selector · payload` frame, [`HOST_OP_TIMEOUT`]-bounded. Errors and timeouts are
    /// logged and abandoned — the caller's state machine moves on.
    pub(crate) async fn send(&self, selector: u8, payload: &[u8], what: &str) {
        // One frame, one packet, one transfer: the frame buffer is a small stack local rather than
        // a static because it is bounded by the largest control payload, not by the endpoint.
        let mut frame = [0u8; 1 + MAX_CONTROL_PAYLOAD];
        if payload.len() > MAX_CONTROL_PAYLOAD {
            warn!("usb: [ctl] {} payload is {} B, over the frame cap — dropping", what, payload.len());
            return;
        }
        frame[0] = selector;
        frame[1..1 + payload.len()].copy_from_slice(payload);
        let mut ep = self.ep.lock().await;
        match with_timeout(HOST_OP_TIMEOUT, ep.write(&frame[..1 + payload.len()])).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => warn!("usb: [ctl] {} send failed: {:?}", what, defmt::Debug2Format(&e)),
            Err(_) => warn!("usb: [ctl] {} send timed out — abandoning", what),
        }
    }

    /// Send one §4.3 status message (the transport-agnostic [`StatusBytes`]).
    pub(crate) async fn send_status(&self, (buf, len): StatusBytes) {
        self.send(device_frame::STATUS, &buf[..len], "status").await;
    }

    /// Notify a store movement after a commit/delete: the `storeChanged` status message (which
    /// store, new revision) — protocol v2's sole change signal, identical on both transports.
    pub(crate) async fn publish_store_change(&self, store: &RefCell<ObjectStore>, ty: ObjectType) {
        // Each store keeps its own monotonic-per-boot revision (spec §4.3): a trip move stamps the
        // trip store's counter, a route/ride move the shared route/ride counter.
        let revision = if ty == ObjectType::Trip { store.borrow().trip_revision() } else { store.borrow().revision() };
        self.send_status(StatusMessage::StoreChanged(StoreChanged { ty, revision }).encode()).await;
    }
}

/// The largest control payload any frame carries.
///
/// Taken from the spec's own ceiling rather than from today's values: `Config::MAX_ENCODED` is the
/// widest thing §7.3 permits, and it dominates the device-info triple (3 length bytes + 24 + 16 + 16
/// = 59 B), a status message (13 B) and the identity read (6 B). Sizing to the *rule* means a longer
/// device name or a new status message can't silently start truncating replies. It is comfortably
/// under [`MAX_PACKET`](super::MAX_PACKET), which the frame-per-transfer contract requires.
const MAX_CONTROL_PAYLOAD: usize = obc_ble::Config::MAX_ENCODED;
const _: () = assert!(
    1 + MAX_CONTROL_PAYLOAD < super::MAX_PACKET as usize,
    "a control frame must be strictly shorter than one max packet — at exactly the packet size the \
     peer cannot tell the frame ended without a zero-length packet"
);

// ============================ The frame loop ============================

/// Serve host → device control frames until the device is unplugged, then wait to be plugged in
/// again — forever.
///
/// Each iteration is one frame. The dispatch mirrors [`crate::ble::control::serve_connection`]'s
/// handle routing exactly, and calls the same shared decision functions: the store work is
/// synchronous inside a locked scope, the `RefCell` borrow never spans an `await`, and the reply
/// goes out after the shared-store guard is dropped.
pub(crate) async fn run(
    tx: &ControlTx,
    mut ep: EpOut,
    buf: &'static mut [u8],
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
    store_epoch: Option<u32>,
) -> ! {
    loop {
        // Before configuration (and after an unplug) the endpoint is disabled; parking here is the
        // idle state, woken by the host's SET_CONFIGURATION.
        ep.wait_enabled().await;
        info!("usb: [ctl] endpoint enabled — serving control frames");
        loop {
            let n = match ep.read(buf).await {
                Ok(n) => n,
                Err(EndpointError::Disabled) => {
                    info!("usb: [ctl] endpoint disabled — waiting for the next configuration");
                    break;
                }
                Err(e) => {
                    warn!("usb: [ctl] read failed: {:?} — re-arming", defmt::Debug2Format(&e));
                    break;
                }
            };
            if n == 0 {
                // A zero-length packet is a USB-level marker, not a frame.
                continue;
            }
            serve_frame(tx, &buf[..n], store, shared, store_epoch).await;
        }
    }
}

/// Dispatch one decoded frame.
async fn serve_frame(
    tx: &ControlTx,
    frame: &[u8],
    store: &RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
    store_epoch: Option<u32>,
) {
    let (selector, payload) = (frame[0], &frame[1..]);
    // Lock the shared store for this frame's synchronous store work (a delete / upload check /
    // config persist), then drop the guard before the async sends below. The ride loop's map render
    // can hold the same lock, so a control frame may wait a frame for it — harmless.
    let mut guard = shared.lock().await;

    // Settings coherence, device → host (#456): if the ride loop persisted an on-device settings
    // change since the last frame, its RRAM blob moved and our config cache is stale. Refresh it (a
    // no-op when nothing changed) so a `config` read this session serves the fresh units/name.
    store.borrow_mut().refresh_settings_if_changed(&mut guard);

    let mut status_msg: Option<StatusBytes> = None;
    let mut store_changed: Option<ObjectType> = None;
    let mut forget_after_ack = false;
    // A reply whose payload is assembled under the lock but sent after it is released.
    let mut reply: Option<(u8, [u8; MAX_CONTROL_PAYLOAD], usize)> = None;

    match selector {
        host_frame::COMMAND => {
            let outcome = run_command(payload, store, &mut guard);
            status_msg = Some(outcome.result);
            store_changed = outcome.store_changed;
            forget_after_ack = outcome.forget_bond;
            info!("usb: [ctl] command frame ({} B)", payload.len());
        }
        host_frame::TRANSFER_CONTROL => match classify_transfer(payload, store, &mut guard) {
            TransferDisposition::Arm(armed) => {
                info!("usb: [ctl] transfer armed");
                // Set the gate *before* the data plane can observe the arm, exactly as the GATT
                // path does: the cross-transport `TRANSFER_ACTIVE` is what turns a second open —
                // from either wire — into a typed `busy`.
                TRANSFER_ACTIVE.store(true, core::sync::atomic::Ordering::Relaxed);
                TRANSFER_ARM.signal(armed);
            }
            TransferDisposition::AbortActive => {
                info!("usb: [ctl] abort → data plane");
                TRANSFER_ABORT.signal(());
            }
            TransferDisposition::Answer(bytes) => {
                info!("usb: [ctl] transfer answered immediately");
                status_msg = Some(bytes);
            }
        },
        host_frame::CONFIG_WRITE => {
            // Applied + persisted, or rejected. A rejection has no ATT error code to ride on here,
            // so it is reported the way every other typed failure is: the caller re-reads the
            // config and sees its write did not take. Logged loudly either way.
            if identity::apply_config_write(payload, store, &mut guard) {
                info!("usb: [ctl] config write applied + persisted");
            } else {
                warn!("usb: [ctl] config write rejected (malformed)");
            }
        }
        host_frame::IDENTITY_READ => {
            let (bytes, len) = identity::version_read_bytes(store_epoch);
            let mut out = [0u8; MAX_CONTROL_PAYLOAD];
            out[..len].copy_from_slice(&bytes[..len]);
            reply = Some((device_frame::IDENTITY, out, len));
        }
        host_frame::DEVICE_INFO_READ => {
            let (out, len) = device_info_bytes();
            reply = Some((device_frame::DEVICE_INFO, out, len));
        }
        host_frame::CONFIG_READ => {
            // `config_bytes` yields at most `Config::MAX_ENCODED`, which is exactly
            // `MAX_CONTROL_PAYLOAD` — so this copy cannot truncate, by construction rather than by
            // clamping.
            let (bytes, len) = identity::config_bytes(&store.borrow());
            let mut out = [0u8; MAX_CONTROL_PAYLOAD];
            out[..len].copy_from_slice(&bytes[..len]);
            reply = Some((device_frame::CONFIG, out, len));
        }
        other => warn!("usb: [ctl] unknown selector {} ({} B) — ignored", other, payload.len()),
    }

    // Store work for this frame is done; release the shared lock before the async sends. (The
    // `RefCell` borrows above all ended with their expressions.)
    drop(guard);

    if let Some((sel, buf, len)) = reply {
        tx.send(sel, &buf[..len], "read reply").await;
    }
    if let Some(bytes) = status_msg {
        tx.send_status(bytes).await;
    }
    if forget_after_ack {
        // Same ordering the spec pins for BLE: the `commandResult(ok)` ack is out, so it is now
        // safe to trigger the forget. The bond is the *radio's*, and clearing it also drops a live
        // BLE connection — which is exactly what a rider asking to forget their phone wants,
        // whichever cable they asked over.
        crate::ble::request_forget_bond();
    }
    if let Some(ty) = store_changed {
        tx.publish_store_change(store, ty).await;
    }
}

/// The Device Information Service strings, which have no binary layout of their own because on BLE
/// they are three separate characteristics: `len u8 · UTF-8`, three times, in the order firmware ·
/// hardware · serial. The firmware revision is the load-bearing one — it is what "an update is
/// available" compares against, and the spec is explicit that the *running* image's version lives
/// there and nowhere else.
fn device_info_bytes() -> ([u8; MAX_CONTROL_PAYLOAD], usize) {
    let fw = identity::firmware_revision();
    let serial = identity::serial_string();
    let mut out = [0u8; MAX_CONTROL_PAYLOAD];
    let mut at = 0;
    for s in [fw.as_str(), identity::HARDWARE_REVISION, serial.as_str()] {
        let bytes = s.as_bytes();
        // Every source is a fixed-capacity string well under 255 B and under the frame cap by
        // construction; clamp rather than truncate mid-frame if a revision string ever grows.
        let n = bytes.len().min(MAX_CONTROL_PAYLOAD - at - 1);
        out[at] = n as u8;
        at += 1;
        out[at..at + n].copy_from_slice(&bytes[..n]);
        at += n;
    }
    (out, at)
}
