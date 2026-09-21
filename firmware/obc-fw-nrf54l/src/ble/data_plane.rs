use defmt::warn;
use embassy_time::{with_timeout, Timer};
use nrf_sdc::{self as sdc};
use trouble_host::prelude::*;

use super::gatt::Server;
use super::lifecycle::HOST_OP_TIMEOUT;
use super::state::battery;

/// One host notify, [`HOST_OP_TIMEOUT`]-bounded, so a peer that stops draining its ATT queue cannot
/// stall a plane's task past the link's supervision timeout — the structural backstop beneath the
/// hardware watchdog (see `lifecycle`'s watchdog policy). A timeout or error is logged and
/// abandoned: a lost notification is the app's to recover by re-reading, never a reason to wedge the
/// link.
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
/// cadence. The value comes from the fuel-gauge seam through [`super::state::battery`], and the stub
/// is constant today. Never returns; `run`'s `select` cancels it on disconnect.
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
