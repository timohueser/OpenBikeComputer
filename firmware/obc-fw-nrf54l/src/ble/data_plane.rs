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

use defmt::warn;
use embassy_time::{with_timeout, Timer};
use nrf_sdc::{self as sdc};
use obc_ble::{ObjectType, StatusMessage, StoreChanged};
use trouble_host::prelude::*;

use crate::object_store::ObjectStore;

use super::gatt::Server;
use super::lifecycle::HOST_OP_TIMEOUT;
use super::state::battery;

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
    let (buf, len) = msg.encode();
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
) -> bool {
    match with_timeout(HOST_OP_TIMEOUT, server.notify(stack, handle, bytes)).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            warn!("ble: [coc] {} notify failed: {:?}", what, defmt::Debug2Format(&e));
            false
        }
        Err(_) => {
            warn!("ble: [coc] {} notify timed out — abandoning", what);
            false
        }
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
