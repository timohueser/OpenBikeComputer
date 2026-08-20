//! What is left of the BLE data plane after the protocol-v4 cutover (FS7.5-c3a, epic #1256).
//!
//! **The CoC transfer machinery is gone from here.** The L2CAP channel now carries
//! `FLAT_Store_Protocol.md` §3.8 stream records rather than an unframed byte pipe, and it is driven
//! by [`super::v4`] — which owns the channel, the engine round trips and the record boundaries. The
//! v1 runners this module used to hold (echo loopback, route upload, download, weather upload), the
//! `TRANSFER_ARM`/`TRANSFER_ABORT` signals that armed them and the one-transfer gate they claimed all
//! went with that wire: protocol v4 has no descriptor to classify and no per-runner shape, because a
//! transfer is a `PUT` or a `GET` and the engine is generic over object kinds.
//!
//! [`battery_task`] stayed because BAS level push is a SIG service and no business of the object
//! protocol.
//!
//! [`notify_bounded`] is the shared send both use: one host notify, [`HOST_OP_TIMEOUT`]-bounded so a
//! peer that stops draining its ATT queue cannot stall a plane past the link's supervision timeout.

use defmt::warn;
use embassy_time::{with_timeout, Timer};
use nrf_sdc::{self as sdc};
use trouble_host::prelude::*;

use super::gatt::Server;
use super::lifecycle::HOST_OP_TIMEOUT;
use super::state::battery;

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
