//! The GATT control-plane surface + device identity (A4, issue #272; S0 §3).
//!
//! The GATT table is the **real** control plane the iOS app discovers on connect
//! (`obc-ble-interface-spec.md` §3, the S0 UUIDs): **DIS** (real firmware revision / board id /
//! FICR serial), **BAS** (battery, notify — fed from the `FuelGauge` seam via
//! [`super::state::publish_battery`]), and the custom **OBC Control** service
//! ([`ObcControlService`]) with all eight characteristics. This module owns the
//! `#[gatt_server]`/`#[gatt_service]` tables (mirroring the spec one-to-one), the FICR-derived
//! identity (serial, advertising name, static-random address), the Config-blob codec, and the
//! on-glass status screen. The writes themselves are answered by [`super::control`].

use obc_ble::Config;
use trouble_host::prelude::*;

use crate::object_store::ObjectStore;

use super::state::{status, LinkState};

/// The dynamic L2CAP SPSM the CoC server listens on (S0 §5), published in the `psm` characteristic.
/// A fixed value in the LE dynamic range (`0x0080..=0x00FF`) — the app reads whatever we advertise,
/// so a constant is simpler than negotiating one and equally correct.
pub(crate) const OBC_PSM: u16 = 0x0080;

// The GATT control plane (A4, S0 §3): the two SIG services + the custom OBC Control service. The
// attribute table is auto-sized by the derive (no `attribute_table_size`); runtime values (DIS
// strings, the Config default) are seeded via `server.set` after `new_with_config` in `run`.
#[gatt_server]
pub(crate) struct Server {
    pub dis: DeviceInformationService,
    pub bas: BatteryService,
    pub obc: ObcControlService,
}

/// Device Information Service (S0 §3.1). All read-only strings, seeded at boot; `value` can't hold
/// a runtime string, so the macro declares them empty and `run` fills them.
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
pub(crate) struct DeviceInformationService {
    #[characteristic(uuid = characteristic::FIRMWARE_REVISION_STRING, read)]
    pub firmware_revision: heapless09::String<24>,
    #[characteristic(uuid = characteristic::HARDWARE_REVISION_STRING, read)]
    pub hardware_revision: heapless09::String<16>,
    #[characteristic(uuid = characteristic::SERIAL_NUMBER_STRING, read)]
    pub serial_number: heapless09::String<16>,
}

/// Battery Service (S0 §3.2): the level, read + notify — fed from the `FuelGauge` seam.
#[gatt_service(uuid = service::BATTERY)]
pub(crate) struct BatteryService {
    #[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 75)]
    pub level: u8,
}

/// OBC Control service (S0 §3.3): the custom `3C92XXXX-…` base, the 16-bit block selecting the
/// entity. This table mirrors the spec section one-to-one — one place to diff against S0.
///
/// **Security (A8, S0 §8):** every characteristic here is `permissions(authenticated)` — access
/// requires an encrypted, LESC-authenticated (MITM) link — **except `protocol_version`**, which
/// stays open so the app can version-check before pairing (S0 §1). DIS/BAS are open too (their own
/// services). An unbonded stranger discovers the service but gets Insufficient-Authentication on
/// every gated read/write/subscribe.
#[gatt_service(uuid = "3C920000-9916-4EBA-ABC2-342FE08F6B10")]
pub(crate) struct ObcControlService {
    /// Small imperative commands (§4.4). Write; answered by a `status` `commandResult`.
    #[characteristic(uuid = "3C920001-9916-4EBA-ABC2-342FE08F6B10", write, permissions(authenticated))]
    pub command: heapless09::Vec<u8, 8>,
    /// Typed device → app messages (§4.3). Notify-only.
    #[characteristic(uuid = "3C920002-9916-4EBA-ABC2-342FE08F6B10", notify, permissions(authenticated))]
    pub status: heapless09::Vec<u8, 8>,
    /// The store digest (§4.5): revision + object counts. Seeded from the [`ObjectStore`] at
    /// boot; re-set + notified on every commit/delete (`publish_store_change`).
    #[characteristic(uuid = "3C920003-9916-4EBA-ABC2-342FE08F6B10", read, notify, permissions(authenticated), value = [0u8; 10])]
    pub object_store: [u8; 10],
    /// The Config object (§7.3), whole-blob read + write — round-trips through the persisted
    /// settings (A6): seeded at boot, re-seeded canonical after every accepted write.
    #[characteristic(uuid = "3C920004-9916-4EBA-ABC2-342FE08F6B10", read, write, permissions(authenticated))]
    pub config: heapless09::Vec<u8, 128>,
    /// Open / abort a CoC transfer (§4.2). Write + notify (the notify carries a download's
    /// filled announce descriptor).
    #[characteristic(uuid = "3C920005-9916-4EBA-ABC2-342FE08F6B10", write, notify, permissions(authenticated), value = [0u8; 16])]
    pub transfer_control: [u8; 16],
    /// Reserved (§7.5 — diagnostics cross the CoC): reads return 0 bytes.
    #[characteristic(uuid = "3C920006-9916-4EBA-ABC2-342FE08F6B10", read, permissions(authenticated))]
    pub diagnostics: heapless09::Vec<u8, 1>,
    /// The L2CAP CoC PSM the app opens the channel on (§3.3).
    #[characteristic(uuid = "3C920007-9916-4EBA-ABC2-342FE08F6B10", read, permissions(authenticated), value = OBC_PSM)]
    pub psm: u16,
    /// `protocol_version` (§1) — read **without** encryption (the connect-time version check
    /// happens before pairing). `1` for this contract.
    #[characteristic(uuid = "3C920008-9916-4EBA-ABC2-342FE08F6B10", read, value = 1)]
    pub protocol_version: u16,
}

// ============================ Identity (S0 §2 / §3.1) ============================

/// `FICR.INFO.DEVICEID[0]` (nRF54L15: FICR `0x00FF_C000` + INFO `0x300` + DEVICEID `0x04`) — the
/// low word of the 64-bit factory device id. Read raw: embassy-nrf's `pac` re-export is
/// `pub(crate)` without its `unstable-pac` feature, and one always-readable FICR word doesn't
/// justify enabling that. The full 16-hex-digit serial (S0 §3.1) is built by [`serial_string`].
const FICR_INFO_DEVICEID0: *const u32 = 0x00FF_C304 as *const u32;
/// `FICR.INFO.DEVICEID[1]` — the high word (the address derivation below uses both).
const FICR_INFO_DEVICEID1: *const u32 = 0x00FF_C308 as *const u32;

/// The factory advertising name (S0 §2): `OBC-XXXX`, the last four uppercase hex digits of the
/// serial number — i.e. the low 16 bits of `DEVICEID[0]`, the tail of the serial's hex string.
/// The default whenever no user rename is stored (A6: the Config object's name, S0 §7.3).
pub(crate) fn device_name() -> heapless::String<8> {
    let id = unsafe { FICR_INFO_DEVICEID0.read_volatile() };
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("OBC-{:04X}", id & 0xFFFF));
    s
}

/// How many bytes of the advertised name fit the 31-byte scan-response PDU beside the AD
/// structure overhead (length + type = 2 bytes).
const ADV_NAME_MAX: usize = 29;

/// The name the device advertises **right now** (S0 §2/§7.3): the stored rename, or the factory
/// name when none is set. Re-read by every advertise cycle, so a rename lands in the airwaves on
/// the next advertising start (the current connection's GAP name keeps the boot value — the
/// Config characteristic, not GAP, is authoritative). Truncated to the scan-response budget on a
/// char boundary; the full name still serves on the `config` read.
pub(crate) fn advertised_name(store: &ObjectStore) -> heapless::String<48> {
    let mut s: heapless::String<48> = heapless::String::new();
    let stored = store.settings().device_name;
    if stored.is_empty() {
        let _ = s.push_str(device_name().as_str());
        return s;
    }
    let name = stored.as_str();
    let mut end = name.len().min(ADV_NAME_MAX);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
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

/// The DIS **Serial Number** string (S0 §3.1): the 64-bit FICR `DEVICEID` as 16 uppercase hex
/// digits, high word first — so its last four digits are [`device_name`]'s `XXXX`.
pub(crate) fn serial_string() -> heapless09::String<16> {
    let id0 = unsafe { FICR_INFO_DEVICEID0.read_volatile() };
    let id1 = unsafe { FICR_INFO_DEVICEID1.read_volatile() };
    gatt_str(format_args!("{:08X}{:08X}", id1, id0))
}

/// The DIS **Firmware Revision** string (S0 §3.1): crate semver + git short hash, e.g. `0.1.0+ca9b336`
/// (`OBC_FW_GIT` is emitted by `build.rs`; `unknown` when git wasn't reachable at build time).
pub(crate) fn firmware_revision() -> heapless09::String<24> {
    gatt_str(format_args!("{}+{}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT")))
}

/// The DIS **Hardware Revision** string (S0 §3.1): the board id. The DK today; the LM20 board crate
/// changes this const when it lands.
pub(crate) const HARDWARE_REVISION: &str = "nrf54l15-dk";

/// A **static random** address derived from the factory device id (top two bits must be `11` per
/// the spec), so every board advertises a stable, distinct address. Real identity management
/// (resolvable addresses, bonding) is A8.
pub(crate) fn device_address() -> Address {
    let id0 = unsafe { FICR_INFO_DEVICEID0.read_volatile() }.to_le_bytes();
    let id1 = unsafe { FICR_INFO_DEVICEID1.read_volatile() }.to_le_bytes();
    // 46 factory-id bits + the mandatory `11` top bits of a static random address.
    Address::random([id0[0], id0[1], id0[2], id0[3], id1[0], id1[1] | 0xC0])
}

// ============================ S0 Config codec (§7.3) ============================
//
// The wire layouts themselves live in `obc_ble` (the host-tested crate the shared `protocol-vectors/`
// fixtures pin); this helper only bridges them to the board's GATT attribute types and policy.

/// The canonical Config blob (S0 §7.3, Config v1) from the persisted settings: the stored rename
/// (or the factory name when unset — what the device actually advertises) + the units. Served on
/// the `config` read; re-seeded after every accepted write so reads always return canonical bytes.
pub(crate) fn config_blob(store: &ObjectStore) -> heapless09::Vec<u8, 128> {
    let stored = store.settings().device_name;
    let factory = device_name();
    let name = if stored.is_empty() { factory.as_str() } else { stored.as_str() };
    let units = if store.settings().units.is_imperial() { 1 } else { 0 };
    let cfg = Config { name: name.as_bytes(), units };
    let mut buf = [0u8; Config::MAX_ENCODED];
    let len = cfg.encode(&mut buf).unwrap_or(0); // both name sources are ≤ 48 by construction
    let mut v = heapless09::Vec::new();
    let _ = v.extend_from_slice(&buf[..len]);
    v
}

// ============================ The status screen ============================

/// Paint the whole BLE status screen into the resident RGB222 framebuffer (`run_status` presents
/// it through the `DisplayDriver` seam; RowDiff makes the re-present cheap). Deliberately dumb —
/// a white card of text: the factory name, the link state, the peer + negotiated interval while
/// connected, battery / SD / lifetime counters, and an input counter so a button press visibly
/// lands on glass. While pairing (A8) it becomes the big-font passkey card instead.
pub fn draw_status_screen(fb: &mut [u8], battery_pct: u8, sd_ok: bool, inputs: u32) {
    use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
    use embedded_graphics::prelude::Point;
    use obc_render::{draw_text, Font, TextAlign};

    let s = status();
    let name = device_name();

    fb.fill(0x3F); // device-64 white — the reflective panel's paper backdrop
    let mut dev = obc_platform::FbDevice64::new(fb, crate::st7789::WIDTH as u32, crate::st7789::HEIGHT as u32);
    let ink = Rgb565::BLACK;
    let cx = crate::st7789::WIDTH as i32 / 2;

    // Pairing (A8, S0 §8): the screen's marquee moment — the 6-digit passkey, huge, that the rider
    // types into the phone's pairing dialog. Takes over the whole card until pairing resolves.
    if let Some(code) = s.passkey {
        draw_text(&mut dev, "Pairing", Point::new(cx, 60), Font::Display, TextAlign::Center, ink);
        let mut line: heapless::String<8> = heapless::String::new();
        let _ = core::fmt::write(&mut line, format_args!("{:06}", code));
        draw_text(&mut dev, line.as_str(), Point::new(cx, 150), Font::Huge, TextAlign::Center, ink);
        draw_text(&mut dev, "enter this code", Point::new(cx, 232), Font::Body, TextAlign::Center, ink);
        draw_text(&mut dev, "on your phone", Point::new(cx, 262), Font::Body, TextAlign::Center, ink);
        return;
    }

    draw_text(&mut dev, name.as_str(), Point::new(cx, 28), Font::Display, TextAlign::Center, ink);
    let state = match s.state {
        LinkState::Init => "starting",
        LinkState::Advertising => "advertising",
        // "secured" once the link is encrypted (bonded, A8); plain "connected" before pairing.
        LinkState::Connected if s.secured => "secured",
        LinkState::Connected => "connected",
    };
    draw_text(&mut dev, state, Point::new(cx, 76), Font::Body, TextAlign::Center, ink);

    // The peer's address while connected (display order, MSB first) — Label so 17 chars fit.
    if let Some(p) = s.peer {
        let mut line: heapless::String<20> = heapless::String::new();
        let _ = core::fmt::write(
            &mut line,
            format_args!("peer {:02X}{:02X}{:02X}{:02X}{:02X}{:02X}", p[5], p[4], p[3], p[2], p[1], p[0]),
        );
        draw_text(&mut dev, line.as_str(), Point::new(cx, 112), Font::Label, TextAlign::Center, ink);
    }

    // The detail rows: one label-value line each, Body font, fixed left edge. Start + step are
    // sized so the deepest layout (5 rows when connected) clears the 320 px panel: the last row
    // tops out at 150 + 4×34 = 286, and a Body cell is 28 px tall → 314, inside 320.
    let x = 20;
    let mut y = 150;
    let mut row = |dev: &mut obc_platform::FbDevice64<'_>, text: &str| {
        draw_text(dev, text, Point::new(x, y), Font::Body, TextAlign::Left, ink);
        y += 34;
    };
    let mut line: heapless::String<24> = heapless::String::new();
    // While connected, the negotiated link parameters (A3): interval · PHY · MTU on one line.
    if s.state == LinkState::Connected {
        let _ = core::fmt::write(
            &mut line,
            format_args!("{}ms {} m{}", s.conn_interval_ms, if s.phy_2m { "2M" } else { "1M" }, s.att_mtu),
        );
        row(&mut dev, line.as_str());
        line.clear();
    }
    let _ = core::fmt::write(&mut line, format_args!("batt {}%", battery_pct));
    row(&mut dev, line.as_str());
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("sd   {}", if sd_ok { "ok" } else { "--" }));
    row(&mut dev, line.as_str());
    line.clear();
    // Lifetime connect/disconnect counters + the last drop's reason byte (the soak health line).
    if s.disconnects > 0 {
        let _ = core::fmt::write(
            &mut line,
            format_args!("link {}/{} x{:02X}", s.connects, s.disconnects, s.last_disconnect_reason),
        );
    } else {
        let _ = core::fmt::write(&mut line, format_args!("link {}/{}", s.connects, s.disconnects));
    }
    row(&mut dev, line.as_str());
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("in   {}", inputs));
    row(&mut dev, line.as_str());
}
