//! Device identity and the small read-only blobs, as **plain bytes**.
//!
//! On BLE these are separately-addressed GATT attributes (three DIS strings, `config`,
//! `protocolVersion`); over USB they are the payloads of selector-routed control frames. Both are
//! the same bytes, so the codecs live here and each transport only decides how to address and
//! deliver them. `ble::gatt` wraps the results into trouble-host's heapless-0.9 attribute types;
//! `usb::control` copies them straight into a frame.

use core::cell::RefCell;

use obc_ble::Config;

use crate::object_store::ObjectStore;
use crate::SharedStore;

/// `FICR.INFO.DEVICEID[0]` (nRF54L: FICR `0x00FF_C000` + INFO `0x300` + DEVICEID `0x04`) — the low
/// word of the 64-bit factory device id. Read raw: embassy-nrf's `pac` re-export does not expose
/// FICR, and one always-readable word doesn't justify more. The full 16-hex-digit serial is built by
/// [`serial_string`].
const FICR_INFO_DEVICEID0: *const u32 = 0x00FF_C304 as *const u32;
/// `FICR.INFO.DEVICEID[1]` — the high word (the BLE static-random address derivation uses both).
const FICR_INFO_DEVICEID1: *const u32 = 0x00FF_C308 as *const u32;

/// The two factory device-id words, for callers that derive something else from them (the BLE
/// static-random address).
pub(crate) fn device_id_words() -> (u32, u32) {
    unsafe { (FICR_INFO_DEVICEID0.read_volatile(), FICR_INFO_DEVICEID1.read_volatile()) }
}

/// The factory device name: `OBC-XXXX`, the last four uppercase hex digits of the serial number —
/// i.e. the low 16 bits of `DEVICEID[0]`, the tail of the serial's hex string. The default whenever
/// no user rename is stored (the Config object's name), and the BLE advertising name.
pub(crate) fn device_name() -> heapless::String<8> {
    let (id0, _) = device_id_words();
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("OBC-{:04X}", id0 & 0xFFFF));
    s
}

/// The device's current name: the stored rename, or the factory `OBC-XXXX` when unset. The single
/// source the BLE advertised name and every `config` read resolve from, so a change to how a cleared
/// name falls back can't make the two disagree about what the device is called.
pub(crate) fn resolved_name(store: &ObjectStore) -> heapless::String<48> {
    let stored = store.settings().device_name;
    let mut s: heapless::String<48> = heapless::String::new();
    if stored.is_empty() {
        let _ = s.push_str(device_name().as_str());
    } else {
        let _ = s.push_str(stored.as_str());
    }
    s
}

/// The **Serial Number** string: the 64-bit FICR `DEVICEID` as 16 uppercase hex digits, high word
/// first — so its last four digits are [`device_name`]'s `XXXX`. Also the USB `iSerialNumber`, which
/// is what makes a plugged-in device distinguishable in the browser's chooser.
pub(crate) fn serial_string() -> heapless::String<16> {
    let (id0, id1) = device_id_words();
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("{:08X}{:08X}", id1, id0));
    s
}

/// The **Firmware Revision** string: crate semver + git short hash, e.g. `0.1.0+ca9b336`
/// (`OBC_FW_GIT` is emitted by `build.rs`; `unknown` when git wasn't reachable at build time). This
/// is the *only* place the running image's version is published — never duplicated into the Config
/// object, where the two could disagree.
pub(crate) fn firmware_revision() -> heapless::String<24> {
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("{}+{}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT")));
    s
}

/// The **Hardware Revision** string: the board id.
pub(crate) const HARDWARE_REVISION: &str = "nrf54lm20-dk";

/// The canonical Config blob (Config v1) from the persisted settings: the stored rename (or the
/// factory name when unset — what the device actually advertises) + the units. Returned as
/// `(buf, len)`; every caller copies out of it immediately.
pub(crate) fn config_bytes(store: &ObjectStore) -> ([u8; Config::MAX_ENCODED], usize) {
    let name = resolved_name(store);
    let units = if store.settings().units.is_imperial() { 1 } else { 0 };
    let cfg = Config { name: name.as_bytes(), units };
    let mut buf = [0u8; Config::MAX_ENCODED];
    let len = cfg.encode(&mut buf).unwrap_or(0); // both name sources are ≤ 48 by construction
    (buf, len)
}

/// Validate + apply a `config` write: units and a rename persist to the RRAM settings. Returns
/// whether the blob was accepted — a malformed blob or a non-UTF-8 name changes nothing, and the
/// caller reports the rejection in its own transport's vocabulary (an ATT error / a `status` reply).
pub(crate) fn apply_config_write(data: &[u8], store: &RefCell<ObjectStore>, shared: &mut SharedStore) -> bool {
    match Config::decode(data) {
        Some(cfg) => match core::str::from_utf8(cfg.name) {
            Ok(name) => {
                store.borrow_mut().apply_config(shared, name, cfg.units);
                true
            }
            Err(_) => false,
        },
        None => false,
    }
}

/// The §1 identity read (V2 / #632; card-resident epoch #776; OBCM version E1 / #911). With a store
/// epoch (`Some`) it is the full 7-byte [`VersionRead`](obc_ble::VersionRead) — `version u16 ·
/// store_epoch u32 · obcm_version u8`. With **no store** (`None` — a no-card boot has no epoch, and
/// 0 is a *legal* epoch we must never fabricate) it is the 2-byte **version-only** form
/// (`PROTOCOL_VERSION` LE): the peer reads a short value, decodes `storeEpoch = nil`, and
/// fail-closes the ack. Returned as `(buf, len)` so the caller serves whichever length applies.
///
/// `obcm_version` is read straight off [`obc_formats::obcm::VERSION`] — the same constant the map
/// reader validates every `.obcm` header against — so what the device *claims* to read and what it
/// *does* read cannot drift: a format bump moves both or neither. It is deliberately not a
/// hand-kept number here, and not derived from the firmware-revision string, which maps to a format
/// version only through a table that exists nowhere. A host uses it for `OBCC_Spec.md` §6(c): don't
/// offer a map artifact this device can't read.
///
/// The store-less read carries no `obcm_version` even though the device knows it — the fields are
/// positional, `store_epoch` has no absent encoding, and a device with no card has nowhere to put a
/// map anyway (see the [`VersionRead`](obc_ble::VersionRead) docs).
pub(crate) fn version_read_bytes(store_epoch: Option<u32>) -> ([u8; obc_ble::VersionRead::ENCODED_LEN], usize) {
    let mut buf = [0u8; obc_ble::VersionRead::ENCODED_LEN];
    match store_epoch {
        Some(epoch) => {
            let vr = obc_ble::VersionRead {
                version: obc_ble::PROTOCOL_VERSION,
                store_epoch: epoch,
                obcm_version: Some(obc_formats::obcm::VERSION),
            };
            let (encoded, len) = vr.encode();
            buf.copy_from_slice(&encoded);
            (buf, len)
        }
        None => {
            let v = obc_ble::PROTOCOL_VERSION.to_le_bytes();
            buf[..v.len()].copy_from_slice(&v);
            (buf, v.len())
        }
    }
}
