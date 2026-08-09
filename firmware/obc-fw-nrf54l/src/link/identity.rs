//! Device identity and the small read-only blobs, as **plain bytes**.
//!
//! On BLE these are separately-addressed GATT attributes (three DIS strings, `config`,
//! `protocolVersion`); over USB they are the payloads of selector-routed control frames. Both are
//! the same bytes, so the codecs live here and each transport only decides how to address and
//! deliver them. `ble::gatt` wraps the results into trouble-host's heapless-0.9 attribute types;
//! `usb::control` copies them straight into a frame.

use core::cell::{Cell, RefCell};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use obc_ble::Config;
use obc_dfu::{ImageHeader, FW_VERSION_LEN};

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

/// The running image's version as the DFU boot-state page recorded it, captured once at boot by
/// [`seed_installed_version`]. The live prefix is `bytes[..len]`; `len == 0` means *no installed
/// record*, which is a fact (a probe-flashed device), not a missing read.
///
/// A `Cell` behind a critical section, like `ble::state`'s cells: written once from `main` before
/// anything else runs, then read from the BLE task, the USB task and the ride loop, none of which
/// may block for it.
static INSTALLED_VERSION: BlockingMutex<CriticalSectionRawMutex, Cell<([u8; FW_VERSION_LEN], u8)>> =
    BlockingMutex::new(Cell::new(([0; FW_VERSION_LEN], 0)));

/// Capture the running image's OBCU version for [`firmware_revision`] — called **once**, from
/// `main`, before the BLE/USB tasks are spawned (see `dfu::seed_firmware_revision`, which does the
/// boot-state read).
///
/// A snapshot is enough because the answer cannot change while this image runs: the states that
/// name a *different* running image ([`BootState::running_image`](obc_dfu::BootState::running_image))
/// are all resolved by a reboot, and the two page writes the app itself performs — the trial confirm
/// and the stray-arm downgrade — both carry this very header into `Idle { installed }`.
pub(crate) fn seed_installed_version(installed: Option<&ImageHeader>) {
    let version = installed.map(|h| h.fw_version_str()).unwrap_or("");
    let mut buf = [0u8; FW_VERSION_LEN];
    let n = version.len().min(FW_VERSION_LEN);
    buf[..n].copy_from_slice(&version.as_bytes()[..n]);
    INSTALLED_VERSION.lock(|cell| cell.set((buf, n as u8)));
}

/// **The** firmware-revision assembler: the one preference order, so every screen and every
/// transport says the same thing about the same device (#996, epic #773 U1).
///
/// 1. the **installed OBCU container's version**, verbatim (a release tag like `v1.3.0`: what the
///    image was wrapped with, and the only dialect a host can compare against a published release);
/// 2. otherwise `OBC_FW_GIT`, the build's bare git short hash (`unknown` when git wasn't reachable
///    at build time) — a probe-flashed device with no install history.
///
/// The fallback is deliberately **unparseable as a release version** (#773's locked dialect): a
/// host that cannot read a version never offers an auto-update, which is exactly what a dev build
/// should get. It is deliberately *not* `CARGO_PKG_VERSION+hash` — that parses as a release version
/// whose `+build` metadata a host ignores, so dev devices would be offered updates against whatever
/// the crate's semver happens to say.
fn revision_of(installed_version: &str) -> heapless::String<FW_VERSION_LEN> {
    let mut s = heapless::String::new();
    let _ = s.push_str(if installed_version.is_empty() { env!("OBC_FW_GIT") } else { installed_version });
    s
}

/// [`revision_of`] for a caller holding a live boot-state record — the DFU confirm screen, which
/// reads the page itself (it must show what the page says *now*, not the boot snapshot).
pub(crate) fn revision_from(installed: Option<&ImageHeader>) -> heapless::String<FW_VERSION_LEN> {
    revision_of(installed.map(|h| h.fw_version_str()).unwrap_or(""))
}

/// The **Firmware Revision** string of the running image, e.g. `v1.3.0` after an installed update
/// and `ca9b336` on a probe-flashed build — [`revision_of`] applied to the boot-state snapshot
/// [`seed_installed_version`] took. This is the *only* place the running image's version is
/// published: BLE DIS `0x2A26` and the USB `DEVICE_INFO_READ` frame are the same bytes, and it is
/// never duplicated into the Config object, where the two could disagree.
pub(crate) fn firmware_revision() -> heapless::String<FW_VERSION_LEN> {
    let (buf, len) = INSTALLED_VERSION.lock(|cell| cell.get());
    // The bytes came out of a decoded header's `fw_version_str`, so they are valid UTF-8; stay
    // total anyway — an identity read must never panic.
    revision_of(core::str::from_utf8(&buf[..len as usize]).unwrap_or(""))
}

/// The **Hardware Revision** string: the board id.
pub(crate) const HARDWARE_REVISION: &str = "nrf54lm20-dk";

/// The canonical Config blob (Config v1) from the persisted settings: the stored rename (or the
/// factory name when unset — what the device actually advertises) + the units. Returned as
/// `(buf, len)`; every caller copies out of it immediately.
pub(crate) fn config_bytes(store: &ObjectStore) -> ([u8; Config::MAX_ENCODED], usize) {
    let name = resolved_name(store);
    let units = if store.settings().units.is_imperial() { 1 } else { 0 };
    // The stored §11.8 interval, served verbatim (WX8, #1193). The persisted byte is always a
    // validated discriminant — every writer goes through `refresh_to_apply`, and the settings
    // codec sanitises corruption to the default — so this read never carries a value this build
    // could not have stored.
    let cfg =
        Config { name: name.as_bytes(), units, weather_refresh: Some(store.settings().weather_refresh as u8) };
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
                // §11.8's strict direction. A write naming an interval this build cannot honour is
                // refused whole rather than applied in part: the rename in the same blob is not a
                // consolation prize when the setting beside it was silently dropped.
                //
                // `Ok(None)` — the field absent — is *not* a request to reset anything, so nothing
                // is written for it. That is what keeps an old app's rename from resetting a rider
                // who deliberately chose `Off` back to 30-minute wakeups (§7.3's absent-on-write
                // rule); `apply_config` leaves the stored interval untouched for `None`.
                let Ok(refresh) = cfg.refresh_to_apply() else {
                    return false;
                };
                store.borrow_mut().apply_config(shared, name, cfg.units, refresh.map(|r| r.as_u8()));
                true
            }
            Err(_) => false,
        },
        None => false,
    }
}

/// The §1 identity read (V2 / #632; card-resident epoch #776; OBCM version E1 / #911; capability
/// word WX3 / #1188). With a store epoch (`Some`) it is the full 11-byte
/// [`VersionRead`](obc_ble::VersionRead) — `version u16 ·
/// store_epoch u32 · obcm_version u8 · feature_bits u32`. With **no store** (`None` — a no-card boot has no epoch, and
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
/// The capability word this build announces (§1, §11).
///
/// [`FEATURE_WEATHER`](obc_ble::FEATURE_WEATHER) is set **by WX8 (#1193), in the same change that
/// makes the contract true** — exactly the §11.7 rule: this branch routes type-`20` uploads into
/// the WX7 dual-slot store, persists + honours the §7.3 `weather_refresh` field, populates the real
/// request context, and runs the due scheduler that raises the advertising hint. A phone that reads
/// the bit and runs the fetch-build-upload loop now lands a bundle the rider actually sees.
///
/// Gated on the `ble` feature because the bit covers the **whole** §11 contract, and the service /
/// context read / advertising half of it lives in the radio: a radio-less build accepting the
/// layouts is not the contract, and §11.7 is explicit that announcing nothing is the only accurate
/// answer there. (Every shipping image carries `ble`; the gate exists for the constrained dev
/// profiles.)
const fn feature_bits() -> u32 {
    if cfg!(feature = "ble") {
        obc_ble::FEATURE_WEATHER
    } else {
        0
    }
}

pub(crate) fn version_read_bytes(store_epoch: Option<u32>) -> ([u8; obc_ble::VersionRead::ENCODED_LEN], usize) {
    let mut buf = [0u8; obc_ble::VersionRead::ENCODED_LEN];
    match store_epoch {
        Some(epoch) => {
            let vr = obc_ble::VersionRead {
                version: obc_ble::PROTOCOL_VERSION,
                store_epoch: epoch,
                obcm_version: Some(obc_formats::obcm::VERSION),
                feature_bits: Some(feature_bits()),
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
