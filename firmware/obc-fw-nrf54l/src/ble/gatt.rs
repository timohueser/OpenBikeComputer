//! The GATT control-plane surface: the attribute table the radio exposes.
//!
//! The GATT table is the **real** control plane the iOS app discovers on connect (see
//! `obc-ble-interface-spec.md`): **DIS** (real firmware revision / board id / FICR serial), **BAS**
//! (battery, notify — fed from the `FuelGauge` seam), the custom **OBC Control** service
//! ([`ObcControlService`]) with its six characteristics (protocol v2 retired the `objectStore`
//! digest and reserved `diagnostics` characteristics), and the **Weather Request** service
//! ([`WeatherRequestService`], spec §11) with its one authenticated read. This module owns the
//! `#[gatt_server]`/`#[gatt_service]` tables and the BLE static-random address. The writes
//! themselves are answered by [`super::control`].
//!
//! The identity strings and blob codecs are **not** here: they are the same bytes on any transport
//! and live in [`crate::link::identity`]. What is left below is the thin adaptation into
//! trouble-host's attribute-value types — the `#[gatt_service]` derive impls `AsGatt` for *its*
//! heapless (0.9) `String`/`Vec`, so a shared heapless-0.8 value has to be re-packed here.

use trouble_host::prelude::*;

use crate::link::identity;
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
    pub weather_request: WeatherRequestService,
}

/// Device Information Service. All read-only strings, seeded at boot; `value` can't hold a runtime
/// string, so the macro declares them empty and `run` fills them.
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
pub(crate) struct DeviceInformationService {
    // 32 = the OBCU container's `fw_version` field width, which is what the value now carries
    // (#996) — a release tag verbatim, or the build's git hash on a dev device.
    #[characteristic(uuid = characteristic::FIRMWARE_REVISION_STRING, read)]
    pub firmware_revision: heapless09::String<32>,
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
    /// pairing). Protocol v2 widens it to the [`VersionRead`](obc_ble::VersionRead): `version u16 ·
    /// store_epoch u32 · obcm_version u8` (the last byte E1 / #911). **Variable-length** (a `Vec`,
    /// like `status`/`config`): with a mounted store the boot seed sets the full 7 bytes; with **no
    /// store** (card-resident epoch #776 — no card ⇒ no epoch) it sets the 2-byte **version-only**
    /// form, which the app decodes as `storeEpoch = nil` → ack fail-closed. Empty until the seed
    /// runs (before advertising). See [`version_read_blob`].
    #[characteristic(uuid = "3C920008-9916-4EBA-ABC2-342FE08F6B10", read)]
    pub protocol_version: heapless09::Vec<u8, { obc_ble::VersionRead::ENCODED_LEN }>,
}

/// The **Weather Request** service (spec §11): the dedicated UUID the device advertises *instead of*
/// OBC Control while a weather refresh is due, so a disconnected peripheral can wake the companion.
///
/// It lives in the GATT table of **every** BLE build, unconditionally — not only when the harness
/// feature is on. That is not defensive padding: advertising a service the connected database does
/// not contain is precisely the trap #1188 forbids, and a table that holds the advertised service
/// only in some builds is a table that works only in some builds.
///
/// The one characteristic is an [`obc_ble::WeatherRequestContext`] — 52 little-endian bytes
/// describing the request and the rider. `authenticated` because the value carries the rider's
/// coordinates: an unbonded peer that connects to the advertisement gets an ATT security error and,
/// per [`super::control`], does not consume the pending request either.
#[gatt_service(uuid = "B3B60000-33B4-4F02-A5FF-E5954D54B5AA")]
pub(crate) struct WeatherRequestService {
    #[characteristic(
        uuid = "B3B60001-33B4-4F02-A5FF-E5954D54B5AA",
        read,
        permissions(authenticated),
        value = [0u8; obc_ble::WeatherRequestContext::ENCODED_LEN]
    )]
    pub context: [u8; obc_ble::WeatherRequestContext::ENCODED_LEN],
}

// ============================ Radio identity ============================

/// How many bytes of the advertised name fit the 31-byte scan-response PDU beside the AD
/// structure overhead (length + type = 2 bytes).
const ADV_NAME_MAX: usize = 29;

/// The name the device advertises **right now**: [`identity::resolved_name`], re-read by every
/// advertise cycle so a rename lands in the airwaves on the next advertising start (the current
/// connection's GAP name keeps the boot value — the Config characteristic, not GAP, is
/// authoritative). Truncated to the scan-response budget on a char boundary; the full name still
/// serves on the `config` read.
pub(crate) fn advertised_name(store: &ObjectStore) -> heapless::String<48> {
    let full = identity::resolved_name(store);
    let name = full.as_str();
    let mut end = name.len().min(ADV_NAME_MAX);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let mut s: heapless::String<48> = heapless::String::new();
    let _ = s.push_str(&name[..end]);
    s
}

/// A GATT-typed string (trouble-host's heapless 0.9) from a shared heapless-0.8 one — the DIS values
/// live in the attribute table, which is 0.9. Truncates to `N` on overflow (all callers fit by
/// construction).
fn gatt_str<const N: usize>(s: &str) -> heapless09::String<N> {
    let mut out = heapless09::String::new();
    let _ = out.push_str(&s[..s.len().min(N)]);
    out
}

/// A GATT-typed blob (heapless 0.9) from a shared byte slice.
fn gatt_vec<const N: usize>(bytes: &[u8]) -> heapless09::Vec<u8, N> {
    let mut v = heapless09::Vec::new();
    let _ = v.extend_from_slice(&bytes[..bytes.len().min(N)]);
    v
}

/// The DIS **Firmware Revision** attribute value.
pub(crate) fn dis_firmware_revision() -> heapless09::String<32> {
    gatt_str(identity::firmware_revision().as_str())
}

/// The DIS **Hardware Revision** attribute value.
pub(crate) fn dis_hardware_revision() -> heapless09::String<16> {
    gatt_str(identity::HARDWARE_REVISION)
}

/// The DIS **Serial Number** attribute value.
pub(crate) fn dis_serial_number() -> heapless09::String<16> {
    gatt_str(identity::serial_string().as_str())
}

/// A **static random** address derived from the factory device id (top two bits must be `11` per the
/// spec), so every board advertises a stable, distinct address.
pub(crate) fn device_address() -> Address {
    let (id0, id1) = identity::device_id_words();
    let (id0, id1) = (id0.to_le_bytes(), id1.to_le_bytes());
    // 46 factory-id bits + the mandatory `11` top bits of a static random address.
    Address::random([id0[0], id0[1], id0[2], id0[3], id1[0], id1[1] | 0xC0])
}

/// The canonical Config blob as a GATT attribute value. Served on the `config` read; re-seeded after
/// every accepted write so reads always return canonical bytes.
pub(crate) fn config_blob(store: &ObjectStore) -> heapless09::Vec<u8, 128> {
    let (buf, len) = identity::config_bytes(store);
    gatt_vec(&buf[..len])
}

/// The `protocolVersion` read blob for the boot seed. The attribute is a variable-length `Vec`, so
/// the read serves whichever length [`identity::version_read_bytes`] produced (7 with a mounted
/// store, the 2-byte version-only form without).
pub(crate) fn version_read_blob(
    store_epoch: Option<u32>,
) -> heapless09::Vec<u8, { obc_ble::VersionRead::ENCODED_LEN }> {
    let (buf, len) = identity::version_read_bytes(store_epoch);
    gatt_vec(&buf[..len])
}
