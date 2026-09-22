//! Device identity and the small read-only blobs, as plain bytes.
//!
//! On BLE these are separately-addressed GATT attributes (three DIS strings and `config`). Over USB
//! the device strings are the payload of one EP0 vendor request and `config` is not carried. Both
//! links serve the same identity bytes, so the codecs live here and each transport decides only how
//! to address and deliver them.

use core::cell::{Cell, RefCell};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use obc_ble::Config;
use obc_dfu::{ImageHeader, FW_VERSION_LEN};

use crate::link_control::LinkControl;
use crate::SharedSettings;

/// `FICR.INFO.DEVICEID[0]`, the low word of the 64-bit factory device id. Read raw, because
/// embassy-nrf's `pac` re-export does not expose FICR. [`serial_string`] builds the full
/// 16-hex-digit serial from both words.
const FICR_INFO_DEVICEID0: *const u32 = 0x00FF_C304 as *const u32;
/// `FICR.INFO.DEVICEID[1]` — the high word (the BLE static-random address derivation uses both).
const FICR_INFO_DEVICEID1: *const u32 = 0x00FF_C308 as *const u32;

/// The two factory device-id words, for callers that derive something else from them.
pub(crate) fn device_id_words() -> (u32, u32) {
    unsafe { (FICR_INFO_DEVICEID0.read_volatile(), FICR_INFO_DEVICEID1.read_volatile()) }
}

/// The factory device name `OBC-XXXX`: the last four uppercase hex digits of the serial, which are
/// the low 16 bits of `DEVICEID[0]`. The default whenever no rename is stored, and the BLE
/// advertising name.
pub(crate) fn device_name() -> heapless::String<8> {
    let (id0, _) = device_id_words();
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("OBC-{:04X}", id0 & 0xFFFF));
    s
}

/// The device's current name: the stored rename, or the factory `OBC-XXXX` when unset. The one
/// source the advertised name and every `config` read resolve from, so the two cannot disagree about
/// what the device is called.
pub(crate) fn resolved_name(store: &LinkControl) -> heapless::String<48> {
    let stored = store.settings().device_name;
    let mut s: heapless::String<48> = heapless::String::new();
    if stored.is_empty() {
        let _ = s.push_str(device_name().as_str());
    } else {
        let _ = s.push_str(stored.as_str());
    }
    s
}

/// The serial number string: the 64-bit FICR `DEVICEID` as 16 uppercase hex digits, high word
/// first, so its last four digits are [`device_name`]'s `XXXX`. Also the USB `iSerialNumber`, which
/// is what makes a plugged-in device distinguishable in the browser's chooser.
pub(crate) fn serial_string() -> heapless::String<16> {
    let (id0, id1) = device_id_words();
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("{:08X}{:08X}", id1, id0));
    s
}

/// The running image's version as the DFU boot-state page recorded it, captured once at boot by
/// [`seed_installed_version`]. The live prefix is `bytes[..len]`; `len == 0` means no installed
/// record, which is a fact on a probe-flashed device, not a missing read.
///
/// A `Cell` behind a critical section: written once from `main` before anything else runs, then read
/// from the BLE task, the USB task and the ride loop, none of which may block for it.
static INSTALLED_VERSION: BlockingMutex<CriticalSectionRawMutex, Cell<([u8; FW_VERSION_LEN], u8)>> =
    BlockingMutex::new(Cell::new(([0; FW_VERSION_LEN], 0)));

/// Capture the running image's OBCU version for [`firmware_revision`] — called once, from `main`,
/// before the BLE and USB tasks are spawned.
///
/// A snapshot is enough, because the answer cannot change while this image runs: the states that
/// name a different running image are all resolved by a reboot, and the two page writes the app
/// performs, the trial confirm and the stray-arm downgrade, both carry this header into
/// `Idle { installed }`.
pub(crate) fn seed_installed_version(installed: Option<&ImageHeader>) {
    let version = installed.map(|h| h.fw_version_str()).unwrap_or("");
    let mut buf = [0u8; FW_VERSION_LEN];
    let n = version.len().min(FW_VERSION_LEN);
    buf[..n].copy_from_slice(&version.as_bytes()[..n]);
    INSTALLED_VERSION.lock(|cell| cell.set((buf, n as u8)));
}

/// The firmware-revision assembler: one preference order, so every screen and every transport says
/// the same thing about the same device. First the installed OBCU container's version, verbatim (a
/// release tag like `v1.3.0`, the only dialect a host can compare against a published release);
/// otherwise `OBC_FW_GIT`, the build's bare git short hash, or `unknown` when git was not reachable
/// at build time.
///
/// The fallback is deliberately unparseable as a release version: a host that cannot read a version
/// never offers an auto-update, which is what a dev build should get. `CARGO_PKG_VERSION+hash` would
/// parse as a release version whose `+build` metadata a host ignores, so dev devices would be
/// offered updates against whatever the crate's semver says.
fn revision_of(installed_version: &str) -> heapless::String<FW_VERSION_LEN> {
    let mut s = heapless::String::new();
    let _ = s.push_str(if installed_version.is_empty() { env!("OBC_FW_GIT") } else { installed_version });
    s
}

/// [`revision_of`] for a caller holding a live boot-state record: the DFU confirm screen, which
/// reads the page itself, because it must show what the page says now, not the boot snapshot.
pub(crate) fn revision_from(installed: Option<&ImageHeader>) -> heapless::String<FW_VERSION_LEN> {
    revision_of(installed.map(|h| h.fw_version_str()).unwrap_or(""))
}

/// The firmware revision string of the running image, such as `v1.3.0` after an installed update or
/// `ca9b336` on a probe-flashed build. The only place the running image's version is published: the
/// BLE DIS string and the USB EP0 vendor request are the same bytes, and it is never duplicated into
/// the Config object, where the two could disagree.
pub(crate) fn firmware_revision() -> heapless::String<FW_VERSION_LEN> {
    let (buf, len) = INSTALLED_VERSION.lock(|cell| cell.get());
    // The bytes came out of a decoded header's `fw_version_str`, so they are valid UTF-8. Stay total
    // anyway: an identity read must never panic.
    revision_of(core::str::from_utf8(&buf[..len as usize]).unwrap_or(""))
}

pub(crate) const HARDWARE_REVISION: &str = "nrf54lm20-dk";

/// The canonical Config blob from the persisted settings: the stored rename, or the factory name
/// when unset, plus the units. Every caller copies out of the returned buffer immediately.
pub(crate) fn config_bytes(store: &LinkControl) -> ([u8; Config::MAX_ENCODED], usize) {
    let name = resolved_name(store);
    let units = if store.settings().units.is_imperial() { 1 } else { 0 };
    let cfg = Config { name: name.as_bytes(), units };
    let mut buf = [0u8; Config::MAX_ENCODED];
    let len = cfg.encode(&mut buf).unwrap_or(0); // both name sources are ≤ 48 by construction
    (buf, len)
}

/// Validate and apply a `config` write: units and a rename persist to the RRAM settings. Returns
/// whether the blob was accepted; a malformed blob or a non-UTF-8 name changes nothing, and the
/// caller reports the rejection in its own transport's vocabulary.
pub(crate) fn apply_config_write(data: &[u8], store: &RefCell<LinkControl>, shared: &mut SharedSettings) -> bool {
    match Config::decode(data) {
        Some(cfg) => match core::str::from_utf8(cfg.name) {
            Ok(name) => {
                // A write naming an interval this build cannot honour is refused whole rather than
                // applied in part: the rename in the same blob is not a consolation prize when the
                // setting beside it was silently dropped.
                //
                // An absent field is not a request to reset anything, so nothing is written for it.
                // That is what keeps an old app's rename from resetting a rider who deliberately
                // chose `Off` back to 30-minute wakeups; `apply_config` leaves the stored interval
                // untouched.
                store.borrow_mut().apply_config(shared, name, cfg.units);
                true
            }
            Err(_) => false,
        },
        None => false,
    }
}
