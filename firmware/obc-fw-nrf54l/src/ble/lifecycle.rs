//! The advertise / negotiate lifecycle (A3, issue #271) — the link layer made *boring*: a
//! lifecycle that never wedges.
//!
//! - **The loop has no terminal states.** [`advertise_lifecycle`] → `serve_connection` →
//!   re-advertise, forever (the loop itself lives in [`super::run`]); any disconnect (for any
//!   reason) drops straight back to advertising, and even an advertise *error* only pauses a beat
//!   before retrying (S0 §2 "always just works").
//! - **Advertising interval policy (S0 §2)**: *fast* (40 ms) for [`FAST_ADV_WINDOW`] after boot and
//!   after every disconnect, then *slow* (1000 ms) indefinitely. Legacy connectable adv doesn't
//!   self-terminate, so the fast→slow switch is a host-side timer, not the HCI duration field.
//! - **Parameter negotiation on connect** ([`negotiate_link`], S0 §3.4): request 2M PHY, DLE
//!   (251-byte PDUs), and the idle connection-parameter set. Each is a *preference* — the protocol
//!   is correct at any negotiated MTU/PHY, just slower — so every step is timeout-bounded and
//!   best-effort: a failed or hung procedure is logged and skipped, never a reason to drop the link.
//! - **Telemetry**: connects / disconnects / last disconnect reason / negotiated MTU + PHY, both
//!   published for the status UI and logged over RTT — the raw material for the `A9` soak assertions.
//!
//! ### Watchdog policy (A3 decision)
//!
//! **No hardware WDT in the `ble` build (yet).** The lifecycle is a *structural* watchdog: every
//! host operation is `with_timeout`-bounded, the serve loop only ever exits on a real disconnect
//! event, and the outer loop has no path that can block permanently — a stuck procedure degrades to
//! a reconnect rather than a hang. A hardware `WDT` petted from the host runner is deferred to `A9`
//! (reliability hardening), where it can be co-designed with the whole-firmware idle/WFI wake
//! pattern rather than bolted onto one task. The firmware runs no watchdog today, so this build
//! doesn't regress that.

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_time::{with_timeout, Duration, Timer};
use nrf_sdc::{self as sdc};
use trouble_host::prelude::*;

use super::gatt::Server;
use super::state::publish;

/// The OBC Control service UUID (`3C920000-9916-4EBA-ABC2-342FE08F6B10`, S0 §3.3) as the raw
/// **little-endian** 16 bytes the advertising AD structure wants (reverse of the display order).
/// Advertised so the app's `scanForPeripherals(withServices:)` filter matches.
const OBC_SERVICE_UUID_LE: [u8; 16] =
    [0x10, 0x6B, 0x8F, 0xE0, 0x2F, 0x34, 0xC2, 0xAB, 0xBA, 0x4E, 0x16, 0x99, 0x00, 0x00, 0x92, 0x3C];

// ============================ Link-parameter policy (S0 §2 / §3.4, A3) ============================

/// How long the device advertises *fast* (S0 §2) after boot and after every disconnect before
/// dropping to the slow interval — snappy reconnection while the phone is nearby, then power-saving.
const FAST_ADV_WINDOW: Duration = Duration::from_secs(30);

/// Fast advertising: 40 ms interval (S0 §2). Legacy connectable, defaults otherwise.
fn fast_adv_params() -> AdvertisementParameters {
    AdvertisementParameters {
        interval_min: Duration::from_millis(40),
        interval_max: Duration::from_millis(40),
        ..Default::default()
    }
}

/// Slow advertising: 1000 ms interval (S0 §2) — the indefinite powered-and-unconnected steady state.
fn slow_adv_params() -> AdvertisementParameters {
    AdvertisementParameters {
        interval_min: Duration::from_millis(1000),
        interval_max: Duration::from_millis(1000),
        ..Default::default()
    }
}

/// Timeout on every per-connection host procedure ([`negotiate_link`], the data plane's fast-param
/// request). Generous — these are LL round-trips with the peer — but finite, so a peer that never
/// answers can't wedge the task.
pub(crate) const HOST_OP_TIMEOUT: Duration = Duration::from_secs(5);

/// The connection-parameter set for the current link phase (S0 §3.4). The device *requests*; iOS
/// accepts what the OS allows. Apple's Accessory Design Guidelines constrain a peripheral's request
/// — interval ≥ 15 ms, interval_max ≥ interval_min + 15 ms, latency ≤ 30, timeout ≤ 6 s, and
/// interval_max × (latency + 1) × 3 < timeout — and both sets below satisfy those, so a compliant
/// central can honour either.
///
/// - `transfer_active == false` → **idle**: a relaxed interval + peripheral latency so the radio
///   (and the M33 it wakes) mostly sleeps between the phone's keep-alives. A3 only ever runs this
///   set — there is no transfer yet.
/// - `transfer_active == true` → **fast**: the tightest interval iOS reliably grants, no latency,
///   for throughput. `A5`'s data plane calls `conn_params(true)` at transfer start and reverts to
///   the idle set when the CoC closes; pinned here so both live in one reviewed place.
pub(crate) fn conn_params(transfer_active: bool) -> RequestedConnParams {
    if transfer_active {
        RequestedConnParams {
            min_connection_interval: Duration::from_millis(15),
            max_connection_interval: Duration::from_millis(30),
            max_latency: 0,
            min_event_length: Duration::from_micros(0),
            max_event_length: Duration::from_millis(30),
            supervision_timeout: Duration::from_millis(4000),
        }
    } else {
        RequestedConnParams {
            min_connection_interval: Duration::from_millis(30),
            max_connection_interval: Duration::from_millis(45),
            max_latency: 4,
            min_event_length: Duration::from_micros(0),
            max_event_length: Duration::from_millis(45),
            supervision_timeout: Duration::from_millis(4000),
        }
    }
}

/// Negotiate the S0 §3.4 link parameters, best-effort. Each step is a *preference* — the protocol
/// is correct at any negotiated MTU/PHY, just slower — and each is [`HOST_OP_TIMEOUT`]-bounded, so
/// a peer that ignores or stalls a procedure degrades the link (log + skip) but never wedges the
/// task. Runs concurrently with `serve_connection`, which services the peer's own moves (and its
/// ATT MTU exchange) while these requests are in flight. Concrete SDC type: the extra command
/// bounds (`LeSetPhy` / `LeSetDataLength` / `LeReadLocalSupportedFeatures`) aren't in the
/// `trouble_host::Controller` bundle, and this only ever runs on the one controller.
pub(crate) async fn negotiate_link(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) {
    let raw = conn.raw();

    // Each step guards on `is_connected` first: if the peer dropped mid-negotiation (a
    // connect/disconnect storm, a walk-away), bail immediately instead of issuing doomed commands
    // — the outer loop re-advertises that much sooner. `with_timeout` is the backstop for a peer
    // that stays connected but never answers.

    // 2M PHY — double the symbol rate for the object plane's bulk transfers (A5+).
    if !raw.is_connected() {
        return;
    }
    match with_timeout(HOST_OP_TIMEOUT, raw.set_phy(stack, PhyKind::Le2M)).await {
        Ok(Ok(())) => info!("ble: [negotiate] requested 2M PHY"),
        Ok(Err(e)) => warn!("ble: [negotiate] set_phy failed: {:?}", defmt::Debug2Format(&e)),
        Err(_) => warn!("ble: [negotiate] set_phy timed out"),
    }

    // Data-length extension — 251-byte PDUs (max TX time 2120 µs is the 1M-PHY worst case, so it's
    // valid regardless of the negotiated PHY; the controller caps to what the link supports).
    if !raw.is_connected() {
        return;
    }
    match with_timeout(HOST_OP_TIMEOUT, raw.update_data_length(stack, 251, 2120)).await {
        Ok(Ok(())) => info!("ble: [negotiate] requested DLE (251-byte PDUs)"),
        Ok(Err(e)) => warn!("ble: [negotiate] update_data_length failed: {:?}", defmt::Debug2Format(&e)),
        Err(_) => warn!("ble: [negotiate] update_data_length timed out"),
    }

    // Let the central finish its own connection-setup procedures before asking it to relax the
    // interval — iOS drives PHY/DLE and the ATT MTU exchange right after connect and tends to
    // ignore a parameter request that lands mid-setup.
    Timer::after_millis(500).await;
    if !raw.is_connected() {
        return;
    }
    let params = conn_params(false);
    match with_timeout(HOST_OP_TIMEOUT, raw.update_connection_params(stack, &params)).await {
        Ok(Ok(())) => info!(
            "ble: [negotiate] requested idle conn params (interval {}-{} ms, latency {})",
            params.min_connection_interval.as_millis(),
            params.max_connection_interval.as_millis(),
            params.max_latency
        ),
        Ok(Err(e)) => warn!("ble: [negotiate] update_connection_params failed: {:?}", defmt::Debug2Format(&e)),
        Err(_) => warn!("ble: [negotiate] update_connection_params timed out"),
    }

    // The MTU is exchanged by the central (GATT client); log + publish what we settled on.
    let mtu = raw.att_mtu();
    info!("ble: [negotiate] ATT MTU = {}", mtu);
    publish(|s| s.att_mtu = mtu);
}

/// Advertise per the S0 §2 interval policy and return the accepted connection: **fast** (40 ms)
/// for [`FAST_ADV_WINDOW`], then **slow** (1000 ms) indefinitely. Each phase is a fresh advertiser;
/// when the fast window elapses with no central its advertiser is dropped (which stops adv) and the
/// slow one starts. Legacy connectable adv, S0 §2 shaped: the primary PDU carries AD Flags + the
/// 128-bit OBC Control service UUID (so the app's `scanForPeripherals(withServices:)` filter
/// matches), and the local name (`OBC-XXXX`) rides the scan response (S0 §2 allows this — the name
/// would crowd the 31-byte primary PDU alongside the 18-byte UUID structure).
pub(crate) async fn advertise_lifecycle<'values, 'server, C: Controller>(
    // Copied into the local scan-response buffer below — deliberately *not* `'values`, so the
    // caller can pass a per-cycle name (the A6 rename) without pinning it for the server's life.
    name: &str,
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv_data = [0u8; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteServiceUuids128(&[OBC_SERVICE_UUID_LE]),
        ],
        &mut adv_data[..],
    )?;
    let adv_data = &adv_data[..adv_len];

    let mut scan_data = [0u8; 31];
    let scan_len = AdStructure::encode_slice(&[AdStructure::CompleteLocalName(name.as_bytes())], &mut scan_data[..])?;
    let scan_data = &scan_data[..scan_len];

    let adv = || Advertisement::ConnectableScannableUndirected { adv_data, scan_data };

    // Fast phase: 40 ms, abandoned after FAST_ADV_WINDOW. `select` drops the losing future, so on
    // timeout the advertiser (owned by `accept`) is dropped and its `Drop` stops advertising.
    let advertiser = peripheral.advertise(&fast_adv_params(), adv()).await?;
    info!("ble: advertising as '{}' (fast, 40 ms for {} s)", name, FAST_ADV_WINDOW.as_secs());
    if let Either::First(conn) = select(advertiser.accept(), Timer::after(FAST_ADV_WINDOW)).await {
        let conn = conn?.with_attribute_server(server)?;
        info!("ble: connection established (fast phase)");
        return Ok(conn);
    }
    info!("ble: fast-advertise window elapsed — dropping to slow advertising");

    // Slow phase: 1000 ms, no timeout — the indefinite steady state.
    let advertiser = peripheral.advertise(&slow_adv_params(), adv()).await?;
    info!("ble: advertising as '{}' (slow, 1000 ms)", name);
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    info!("ble: connection established (slow phase)");
    Ok(conn)
}
