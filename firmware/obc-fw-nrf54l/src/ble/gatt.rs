//! The GATT control-plane surface + device identity.
//!
//! The GATT table is the **real** control plane the iOS app discovers on connect (see
//! `obc-ble-interface-spec.md`): **DIS** (real firmware revision / board id / FICR serial), **BAS**
//! (battery, notify — fed from the `FuelGauge` seam via [`super::state::publish_battery`]), and the
//! custom **OBC Control** service ([`ObcControlService`]) with its six characteristics (protocol v2
//! retired the `objectStore` digest and reserved `diagnostics` characteristics). This module owns
//! the `#[gatt_server]`/`#[gatt_service]` tables, the FICR-derived identity (serial, advertising
//! name, static-random address), the Config-blob codec, and the on-glass status screen. The writes
//! themselves are answered by [`super::control`].

use obc_ble::Config;
use trouble_host::prelude::*;

use crate::object_store::ObjectStore;

/// The dynamic L2CAP SPSM the CoC server listens on, published in the `psm` characteristic. A fixed
/// value in the LE dynamic range (`0x0080..=0x00FF`) — the app reads whatever we advertise, so a
/// constant is simpler than negotiating one and equally correct.
pub(crate) const OBC_PSM: u16 = 0x0080;

// The GATT control plane: the two SIG services + the custom OBC Control service. The attribute table
// is auto-sized by the derive; runtime values (DIS strings, the Config default) are seeded via
// `server.set` after `new_with_config` in `run`.
//
// `connections_max` (default 1): the server's per-connection (CCCD) table serves the phone **and**
// every sensor link (epic #744 SR1). A sensor we connect to runs its own GATT client against us —
// a Garmin watch probes its collector right after subscribe — and an unanswered inbound request
// stalls the peer's ATT for the spec's 30 s transaction timeout, after which the peer terminates
// the link. So the sensor manager attaches this same server to its central connections, and the
// table must hold phone + sensors.
#[gatt_server(connections_max = crate::ble::CONNECTIONS_MAX)]
pub(crate) struct Server {
    pub dis: DeviceInformationService,
    pub bas: BatteryService,
    pub obc: ObcControlService,
}

/// Device Information Service. All read-only strings, seeded at boot; `value` can't hold a runtime
/// string, so the macro declares them empty and `run` fills them.
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
pub(crate) struct DeviceInformationService {
    #[characteristic(uuid = characteristic::FIRMWARE_REVISION_STRING, read)]
    pub firmware_revision: heapless09::String<24>,
    #[characteristic(uuid = characteristic::HARDWARE_REVISION_STRING, read)]
    pub hardware_revision: heapless09::String<16>,
    #[characteristic(uuid = characteristic::SERIAL_NUMBER_STRING, read)]
    pub serial_number: heapless09::String<16>,
}

/// Battery Service: the level, read + notify — fed from the `FuelGauge` seam.
#[gatt_service(uuid = service::BATTERY)]
pub(crate) struct BatteryService {
    #[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 75)]
    pub level: u8,
}

/// OBC Control service: the custom `3C92XXXX-…` base, the 16-bit block selecting the entity.
///
/// **Security:** every characteristic here is `permissions(authenticated)` — access requires an
/// encrypted, LESC-authenticated (MITM) link — **except `protocol_version`**, which stays open so the
/// app can version-check before pairing. DIS/BAS are open too (their own services). An unbonded
/// stranger discovers the service but gets Insufficient-Authentication on every gated
/// read/write/subscribe.
#[gatt_service(uuid = "3C920000-9916-4EBA-ABC2-342FE08F6B10")]
pub(crate) struct ObcControlService {
    /// Small imperative commands. Write; answered by a `status` `commandResult`. 64 bytes fits the
    /// biggest write: an `ackRides` chunk of 31 ids (`2 + 31 × 2`) — the app splits longer
    /// possession lists across writes (the command is idempotent and order-free).
    #[characteristic(uuid = "3C920001-9916-4EBA-ABC2-342FE08F6B10", write, permissions(authenticated))]
    pub command: heapless09::Vec<u8, 64>,
    /// Typed device → app messages. Notify-only — protocol v2's **sole** device → app control channel,
    /// so it also carries a download's announce (`downloadAnnounce`, `msg = 4`). Sized to
    /// [`StatusMessage::MAX_ENCODED_LEN`](obc_ble::StatusMessage::MAX_ENCODED_LEN) (13 bytes, the
    /// announce) so any message fits one notify.
    #[characteristic(uuid = "3C920002-9916-4EBA-ABC2-342FE08F6B10", notify, permissions(authenticated))]
    pub status: heapless09::Vec<u8, { obc_ble::StatusMessage::MAX_ENCODED_LEN }>,
    // `…0003` (the `objectStore` digest) is **retired** in protocol v2 — `storeChanged` (status
    // msg 2) is the sole change signal. The UUID block is not reassigned.
    /// The Config object, whole-blob read + write — round-trips through the persisted settings: seeded
    /// at boot, re-seeded canonical after every accepted write.
    #[characteristic(uuid = "3C920004-9916-4EBA-ABC2-342FE08F6B10", read, write, permissions(authenticated))]
    pub config: heapless09::Vec<u8, 128>,
    /// Open / abort a CoC transfer — **write-only** in protocol v2 (no CCCD): a download's announce
    /// now rides the `status` envelope (`downloadAnnounce`), so all device → app control traffic is
    /// one notify characteristic. The written descriptor is 12 bytes (v2 dropped the offset field).
    #[characteristic(uuid = "3C920005-9916-4EBA-ABC2-342FE08F6B10", write, permissions(authenticated), value = [0u8; 12])]
    pub transfer_control: [u8; 12],
    // `…0006` (the reserved `diagnostics` characteristic) is **retired** in protocol v2 — diagnostics
    // cross the CoC as object type 4. The UUID block is not reassigned.
    /// The L2CAP CoC PSM the app opens the channel on.
    #[characteristic(uuid = "3C920007-9916-4EBA-ABC2-342FE08F6B10", read, permissions(authenticated), value = OBC_PSM)]
    pub psm: u16,
    /// `protocol_version` — read **without** encryption (the connect-time version check happens before
    /// pairing). Protocol v2 widens it to the 6-byte [`VersionRead`](obc_ble::VersionRead): `version
    /// u16 · store_epoch u32`. **Variable-length** (a `Vec`, like `status`/`config`): with a mounted
    /// store the boot seed sets the full 6 bytes; with **no store** (card-resident epoch #776 — no
    /// card ⇒ no epoch) it sets the 2-byte **version-only** form, which the app decodes as
    /// `storeEpoch = nil` → ack fail-closed. Empty until the seed runs (before advertising). See
    /// [`version_read_blob`].
    #[characteristic(uuid = "3C920008-9916-4EBA-ABC2-342FE08F6B10", read)]
    pub protocol_version: heapless09::Vec<u8, { obc_ble::VersionRead::ENCODED_LEN }>,
}

// ============================ Identity ============================

/// `FICR.INFO.DEVICEID[0]` (nRF54L15: FICR `0x00FF_C000` + INFO `0x300` + DEVICEID `0x04`) — the low
/// word of the 64-bit factory device id. Read raw: embassy-nrf's `pac` re-export is `pub(crate)`
/// without its `unstable-pac` feature, and one always-readable FICR word doesn't justify enabling
/// that. The full 16-hex-digit serial is built by [`serial_string`].
const FICR_INFO_DEVICEID0: *const u32 = 0x00FF_C304 as *const u32;
/// `FICR.INFO.DEVICEID[1]` — the high word (the address derivation below uses both).
const FICR_INFO_DEVICEID1: *const u32 = 0x00FF_C308 as *const u32;

/// The factory advertising name: `OBC-XXXX`, the last four uppercase hex digits of the serial number
/// — i.e. the low 16 bits of `DEVICEID[0]`, the tail of the serial's hex string. The default whenever
/// no user rename is stored (the Config object's name).
pub(crate) fn device_name() -> heapless::String<8> {
    let id = unsafe { FICR_INFO_DEVICEID0.read_volatile() };
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("OBC-{:04X}", id & 0xFFFF));
    s
}

/// How many bytes of the advertised name fit the 31-byte scan-response PDU beside the AD
/// structure overhead (length + type = 2 bytes).
const ADV_NAME_MAX: usize = 29;

/// The device's current name: the stored rename, or the factory `OBC-XXXX` when unset. The single
/// source both [`advertised_name`] and [`config_blob`] resolve from, so a change to how
/// a cleared name falls back can't make the advertised name and the Config read disagree about what
/// the device is called.
fn resolved_name(store: &ObjectStore) -> heapless::String<48> {
    let stored = store.settings().device_name;
    let mut s: heapless::String<48> = heapless::String::new();
    if stored.is_empty() {
        let _ = s.push_str(device_name().as_str());
    } else {
        let _ = s.push_str(stored.as_str());
    }
    s
}

/// The name the device advertises **right now**: the [`resolved_name`], re-read by every advertise
/// cycle so a rename lands in the airwaves on the next advertising start (the
/// current connection's GAP name keeps the boot value — the Config characteristic, not GAP, is
/// authoritative). Truncated to the scan-response budget on a char boundary; the full name still
/// serves on the `config` read.
pub(crate) fn advertised_name(store: &ObjectStore) -> heapless::String<48> {
    let full = resolved_name(store);
    let name = full.as_str();
    let mut end = name.len().min(ADV_NAME_MAX);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let mut s: heapless::String<48> = heapless::String::new();
    let _ = s.push_str(&name[..end]);
    s
}

/// A GATT-typed string (trouble-host's heapless 0.9) from `format_args!` — the DIS values live in
/// the attribute table, which is 0.9. Truncates to `N` on overflow (all callers fit by construction).
pub(crate) fn gatt_str<const N: usize>(args: core::fmt::Arguments<'_>) -> heapless09::String<N> {
    let mut s = heapless09::String::new();
    let _ = core::fmt::write(&mut s, args);
    s
}

/// The DIS **Serial Number** string: the 64-bit FICR `DEVICEID` as 16 uppercase hex digits, high word
/// first — so its last four digits are [`device_name`]'s `XXXX`.
pub(crate) fn serial_string() -> heapless09::String<16> {
    let id0 = unsafe { FICR_INFO_DEVICEID0.read_volatile() };
    let id1 = unsafe { FICR_INFO_DEVICEID1.read_volatile() };
    gatt_str(format_args!("{:08X}{:08X}", id1, id0))
}

/// The DIS **Firmware Revision** string: crate semver + git short hash, e.g. `0.1.0+ca9b336`
/// (`OBC_FW_GIT` is emitted by `build.rs`; `unknown` when git wasn't reachable at build time).
pub(crate) fn firmware_revision() -> heapless09::String<24> {
    gatt_str(format_args!("{}+{}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT")))
}

/// The DIS **Hardware Revision** string: the board id. The DK today; the LM20 board crate changes this
/// const when it lands.
pub(crate) const HARDWARE_REVISION: &str = "nrf54l15-dk";

/// A **static random** address derived from the factory device id (top two bits must be `11` per the
/// spec), so every board advertises a stable, distinct address.
pub(crate) fn device_address() -> Address {
    let id0 = unsafe { FICR_INFO_DEVICEID0.read_volatile() }.to_le_bytes();
    let id1 = unsafe { FICR_INFO_DEVICEID1.read_volatile() }.to_le_bytes();
    // 46 factory-id bits + the mandatory `11` top bits of a static random address.
    Address::random([id0[0], id0[1], id0[2], id0[3], id1[0], id1[1] | 0xC0])
}

// ============================ Config codec ============================
//
// The wire layouts themselves live in `obc_ble` (the host-tested crate); this helper only bridges them
// to the board's GATT attribute types and policy.

/// The canonical Config blob (Config v1) from the persisted settings: the stored rename (or the
/// factory name when unset — what the device actually advertises) + the units. Served on the `config`
/// read; re-seeded after every accepted write so reads always return canonical bytes.
pub(crate) fn config_blob(store: &ObjectStore) -> heapless09::Vec<u8, 128> {
    let name = resolved_name(store);
    let units = if store.settings().units.is_imperial() { 1 } else { 0 };
    let cfg = Config { name: name.as_bytes(), units };
    let mut buf = [0u8; Config::MAX_ENCODED];
    let len = cfg.encode(&mut buf).unwrap_or(0); // both name sources are ≤ 48 by construction
    let mut v = heapless09::Vec::new();
    let _ = v.extend_from_slice(&buf[..len]);
    v
}

/// The `protocolVersion` read blob for the boot seed (V2 / #632; card-resident epoch #776). With a
/// store epoch (`Some`) it is the full 6-byte [`VersionRead`](obc_ble::VersionRead) — `version u16 ·
/// store_epoch u32`. With **no store** (`None` — a no-card boot has no epoch, and 0 is a *legal*
/// epoch we must never fabricate) it is the 2-byte **version-only** form (`PROTOCOL_VERSION` LE): the
/// app reads a short attribute, decodes `storeEpoch = nil`, and fail-closes the ack. The attribute is
/// a variable-length `Vec`, so the read serves whichever length was seeded.
pub(crate) fn version_read_blob(
    store_epoch: Option<u32>,
) -> heapless09::Vec<u8, { obc_ble::VersionRead::ENCODED_LEN }> {
    let mut v = heapless09::Vec::new();
    match store_epoch {
        Some(epoch) => {
            let vr = obc_ble::VersionRead { version: obc_ble::PROTOCOL_VERSION, store_epoch: epoch };
            let _ = v.extend_from_slice(&vr.encode());
        }
        None => {
            let _ = v.extend_from_slice(&obc_ble::PROTOCOL_VERSION.to_le_bytes());
        }
    }
    v
}
