use trouble_host::prelude::*;

use crate::link::identity;
use crate::object_store::ObjectStore;

/// The dynamic L2CAP SPSM the CoC server listens on, published in the `psm` characteristic. A fixed
/// value in the LE dynamic range (`0x0080..=0x00FF`): the app reads whatever we advertise, so a
/// constant is simpler than negotiating one.
pub(crate) const OBC_PSM: u16 = 0x0080;

// The GATT control plane: the two SIG services plus the custom OBC Control service. The attribute
// table is auto-sized by the derive; runtime values (the DIS strings, the Config default) are seeded
// with `server.set` in `run`.
//
// `connections_max` covers the phone and every sensor link. A sensor we connect to runs its own GATT
// client against us, and an unanswered inbound request stalls the peer's ATT for the spec's 30 s
// transaction timeout, after which the peer terminates the link. So the sensor manager attaches this
// same server to its central connections.
#[gatt_server(connections_max = crate::ble::CONNECTIONS_MAX)]
pub(crate) struct Server {
    pub dis: DeviceInformationService,
    pub bas: BatteryService,
    pub obc: ObcControlService,
}

/// Device Information Service. All read-only strings, seeded at boot, because `value` cannot hold a
/// runtime string.
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
pub(crate) struct DeviceInformationService {
    // 32 = the OBCU container's `fw_version` field width, which is what the value carries: a release
    // tag verbatim, or the build's git hash on a dev device.
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

/// OBC Control service: the custom `3C92XXXX-…` base, with the 16-bit block selecting the entity.
///
/// Every characteristic here is `permissions(authenticated)`, so access needs an encrypted,
/// LESC-authenticated link, except `protocol_version`, which stays open so the app can version-check
/// before pairing. DIS and BAS are open too. An unbonded stranger discovers the service but gets
/// Insufficient-Authentication on every gated read, write or subscribe.
#[gatt_service(uuid = "3C920000-9916-4EBA-ABC2-342FE08F6B10")]
pub(crate) struct ObcControlService {
    /// Small imperative commands. Write; answered by a `status` `commandResult`. 64 bytes is far
    /// more than the widest live command, a 7-byte `setClock`.
    #[characteristic(uuid = "3C920001-9916-4EBA-ABC2-342FE08F6B10", write, permissions(authenticated))]
    pub command: heapless09::Vec<u8, 64>,
    #[characteristic(uuid = "3C920002-9916-4EBA-ABC2-342FE08F6B10", notify, permissions(authenticated))]
    pub status: heapless09::Vec<u8, { obc_ble::StatusMessage::MAX_ENCODED_LEN }>,
    // The retired `…0003`, `…0005` and `…0006` UUID blocks are never reassigned.
    /// The Config object, whole-blob read + write — round-trips through the persisted settings: seeded
    /// at boot, re-seeded canonical after every accepted write.
    #[characteristic(uuid = "3C920004-9916-4EBA-ABC2-342FE08F6B10", read, write, permissions(authenticated))]
    pub config: heapless09::Vec<u8, 128>,
    /// The L2CAP CoC PSM the app opens the channel on — protocol v4's stream channel, one complete
    /// stream frame per SDU.
    #[characteristic(uuid = "3C920007-9916-4EBA-ABC2-342FE08F6B10", read, permissions(authenticated), value = OBC_PSM)]
    pub psm: u16,
    /// `protocolVersion` — read without encryption, because a peer reads the transport version
    /// before it can send a frame it would otherwise have to misparse, and that check happens before
    /// pairing.
    ///
    /// Two bytes, `u16` = 4. A client learns the card's identity, and the freshness of its cache,
    /// from the `StoreId` every `LIST` page carries, so nothing card-dependent is left here and a
    /// fixed `value` is enough.
    #[characteristic(uuid = "3C920008-9916-4EBA-ABC2-342FE08F6B10", read, value = obc_link::flat::WIRE_MAJOR as u16)]
    pub protocol_version: u16,
    /// The protocol-v4 control channel: one Write Request value carries one complete control frame,
    /// and one confirmed indication carries its response.
    ///
    /// 244 bytes is `ATT_MTU - 3` at the device's preferred 247-byte MTU, which is the control
    /// ceiling. The largest fixed message is the 100-byte `PUT`, and a `LIST` page carries as many
    /// 88-byte entries as the ceiling allows.
    #[characteristic(uuid = "3C920009-9916-4EBA-ABC2-342FE08F6B10", write, indicate, permissions(authenticated))]
    pub object_control: heapless09::Vec<u8, 244>,
}

/// How many bytes of the advertised name fit the 31-byte scan-response PDU, beside the 2-byte AD
/// structure overhead.
const ADV_NAME_MAX: usize = 29;

/// The name the device advertises right now, re-read by every advertise cycle so a rename lands on
/// the next advertising start. The current connection's GAP name keeps the boot value, because the
/// Config characteristic, not GAP, is authoritative. Truncated to the scan-response budget on a char
/// boundary; the full name still serves on the `config` read.
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

/// A GATT-typed string (trouble-host's heapless 0.9) from a shared heapless-0.8 one, because the
/// attribute table is 0.9. Truncates to `N` on overflow; all callers fit by construction.
fn gatt_str<const N: usize>(s: &str) -> heapless09::String<N> {
    let mut out = heapless09::String::new();
    let _ = out.push_str(&s[..s.len().min(N)]);
    out
}

fn gatt_vec<const N: usize>(bytes: &[u8]) -> heapless09::Vec<u8, N> {
    let mut v = heapless09::Vec::new();
    let _ = v.extend_from_slice(&bytes[..bytes.len().min(N)]);
    v
}

pub(crate) fn dis_firmware_revision() -> heapless09::String<32> {
    gatt_str(identity::firmware_revision().as_str())
}

pub(crate) fn dis_hardware_revision() -> heapless09::String<16> {
    gatt_str(identity::HARDWARE_REVISION)
}

pub(crate) fn dis_serial_number() -> heapless09::String<16> {
    gatt_str(identity::serial_string().as_str())
}

/// A static random address derived from the factory device id, so every board advertises a stable,
/// distinct address. The top two bits must be `11`.
pub(crate) fn device_address() -> Address {
    let (id0, id1) = identity::device_id_words();
    let (id0, id1) = (id0.to_le_bytes(), id1.to_le_bytes());
    // 46 factory-id bits + the mandatory `11` top bits of a static random address.
    Address::random([id0[0], id0[1], id0[2], id0[3], id1[0], id1[1] | 0xC0])
}

/// The canonical Config blob as a GATT attribute value. Served on the `config` read and re-seeded
/// after every accepted write, so reads always return canonical bytes.
pub(crate) fn config_blob(store: &ObjectStore) -> heapless09::Vec<u8, 128> {
    let (buf, len) = identity::config_bytes(store);
    gatt_vec(&buf[..len])
}
