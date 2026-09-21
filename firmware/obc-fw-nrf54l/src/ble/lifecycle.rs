//! The advertise and negotiate lifecycle — a link layer that never wedges.
//!
//! - The loop has no terminal states. Advertise, serve, re-advertise, forever; the loop itself lives
//!   in [`super::run`]. Any disconnect drops straight back to advertising, and even an advertise
//!   error only pauses a beat before the retry.
//! - Advertising interval policy: fast (40 ms) for [`FAST_ADV_WINDOW`] after boot and after every
//!   disconnect, then slow (1000 ms) indefinitely. Legacy connectable adv does not self-terminate,
//!   so the fast-to-slow switch is a host-side timer, not the HCI duration field.
//! - Parameter negotiation on connect ([`negotiate_link`]): 2M PHY, DLE (251-byte PDUs) and the idle
//!   connection-parameter set. Each is a preference — the protocol is correct at any negotiated MTU
//!   or PHY, only slower — so every step is timeout-bounded and best-effort. A failed or hung
//!   procedure is logged and skipped, never a reason to drop the link.
//!
//! The lifecycle is the structural first line of the watchdog policy: every host operation is
//! `with_timeout`-bounded, the serve loop exits only on a real disconnect event, and the outer loop
//! has no path that can block permanently, so a stuck procedure degrades to a reconnect rather than
//! a hang. Beneath it sits the hardware watchdog, which the ride loop feeds. It catches what the
//! structural layer cannot reach: a synchronous wedge that never yields to `with_timeout`, such as a
//! hanging SD access, which blocks the shared thread-mode executor and resets the board in ~24 s.

use defmt::{info, warn};
use embassy_futures::select::{select, Either};
use embassy_time::{with_timeout, Duration, Timer};
use nrf_sdc::{self as sdc};
use trouble_host::prelude::*;

use super::gatt::Server;
use super::state::publish;

/// The OBC Control service UUID as the raw little-endian 16 bytes the advertising AD structure
/// wants, which is the reverse of the display order. Advertised so the app's
/// `scanForPeripherals(withServices:)` filter matches.
const OBC_SERVICE_UUID_LE: [u8; 16] =
    [0x10, 0x6B, 0x8F, 0xE0, 0x2F, 0x34, 0xC2, 0xAB, 0xBA, 0x4E, 0x16, 0x99, 0x00, 0x00, 0x92, 0x3C];

/// How long the device advertises fast after boot and after every disconnect, before it drops to
/// the slow interval.
const FAST_ADV_WINDOW: Duration = Duration::from_secs(30);

fn fast_adv_params() -> AdvertisementParameters {
    AdvertisementParameters {
        interval_min: Duration::from_millis(40),
        interval_max: Duration::from_millis(40),
        ..Default::default()
    }
}

fn slow_adv_params() -> AdvertisementParameters {
    AdvertisementParameters {
        interval_min: Duration::from_millis(1000),
        interval_max: Duration::from_millis(1000),
        ..Default::default()
    }
}

/// Timeout on every per-connection host procedure. Generous, because these are LL round trips with
/// the peer, but finite, so a peer that never answers cannot wedge the task.
pub(crate) const HOST_OP_TIMEOUT: Duration = Duration::from_secs(5);

/// The connection-parameter set for the current link phase. The device requests; iOS accepts what
/// the OS allows. Apple's Accessory Design Guidelines constrain a peripheral's request — interval ≥
/// 15 ms, interval_max ≥ interval_min + 15 ms, latency ≤ 30, timeout ≤ 6 s, and interval_max ×
/// (latency + 1) × 3 < timeout — and both sets below satisfy them.
///
/// The idle set uses a relaxed interval and peripheral latency, so the radio, and the M33 it wakes,
/// mostly sleeps between the phone's keep-alives. The active set uses the tightest interval iOS
/// reliably grants, with no latency, for throughput. The data plane asks for it at transfer start
/// and reverts to the idle set when the CoC closes.
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

/// Negotiate the link parameters, best-effort. Each step is a preference and is
/// [`HOST_OP_TIMEOUT`]-bounded, so a peer that ignores or stalls a procedure degrades the link but
/// never wedges the task. Runs beside `serve_connection`, which services the peer's own moves and
/// its ATT MTU exchange meanwhile. Concrete SDC type: the extra command bounds (`LeSetPhy`,
/// `LeSetDataLength`, `LeReadLocalSupportedFeatures`) are not in the `trouble_host::Controller`
/// bundle, and this runs only on the one controller.
pub(crate) async fn negotiate_link(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) {
    let raw = conn.raw();

    // Each step guards on `is_connected` first: if the peer dropped mid-negotiation, bail instead of
    // issuing doomed commands, so the outer loop re-advertises sooner. `with_timeout` is the backstop
    // for a peer that stays connected but never answers.

    // 2M PHY — double the symbol rate for the object plane's bulk transfers.
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

/// Advertise per the interval policy and return the accepted connection: fast (40 ms) for
/// [`FAST_ADV_WINDOW`], then slow (1000 ms) indefinitely. Each phase is a fresh advertiser; when the
/// fast window elapses with no central, its advertiser is dropped, which stops adv, and the slow one
/// starts. Legacy connectable PDUs (ADV_IND) through the extended HCI commands (see the comment at
/// `adv_set` below). The primary PDU carries the AD flags and the 128-bit OBC Control service UUID,
/// so the app's scan filter matches, and the local name (`OBC-XXXX`) rides the scan response,
/// because it would crowd the 31-byte primary PDU beside the 18-byte UUID structure.
pub(crate) async fn advertise_lifecycle<'values, 'server>(
    // Copied into the local scan-response buffer below — deliberately *not* `'values`, so the caller
    // can pass a per-cycle name (the rename) without pinning it for the server's life.
    name: &str,
    // Concrete controller, as in [`super::sensors`]: `advertise_ext`'s command bounds are a zoo, and
    // the SDC is the only controller this crate ever runs.
    peripheral: &mut Peripheral<'values, nrf_sdc::SoftdeviceController<'static>, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<nrf_sdc::Error>> {
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

    // One legacy-PDU advertising set per phase, driven through `advertise_ext` — the extended HCI
    // commands, not the legacy `LeSetAdvParams`/`LeSetAdvEnable`. The wire format is unchanged: a
    // legacy-PDU set emits the same ADV_IND and SCAN_RSP, and every phone sees it as before. The
    // command class is what matters: the first legacy command latches the mode and would bounce
    // every sensor connect with `Command Disallowed` (see [`super::sensors`]' module doc).
    let adv_set = |params| {
        [AdvertisementSet {
            params,
            data: Advertisement::ConnectableScannableUndirected { adv_data, scan_data },
            address: None,
        }]
    };

    // Fast phase: 40 ms, abandoned after FAST_ADV_WINDOW. `select` drops the losing future, so on
    // timeout the advertiser (owned by `accept`) is dropped and its `Drop` stops advertising.
    let sets = adv_set(fast_adv_params());
    let mut handles = AdvertisementSet::handles(&sets);
    let advertiser = peripheral.advertise_ext(&sets, &mut handles).await?;
    info!("ble: advertising as '{}' (fast, 40 ms for {} s)", name, FAST_ADV_WINDOW.as_secs());
    if let Either::First(conn) = select(advertiser.accept(), Timer::after(FAST_ADV_WINDOW)).await {
        let conn = conn?.with_attribute_server(server)?;
        info!("ble: connection established (fast phase)");
        return Ok(conn);
    }
    info!("ble: fast-advertise window elapsed — dropping to slow advertising");

    let sets = adv_set(slow_adv_params());
    let mut handles = AdvertisementSet::handles(&sets);
    let advertiser = peripheral.advertise_ext(&sets, &mut handles).await?;
    info!("ble: advertising as '{}' (slow, 1000 ms)", name);
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    info!("ble: connection established (slow phase)");
    Ok(conn)
}
