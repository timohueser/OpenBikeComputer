//! The BLE stack (issue #270, epic #267): nrf-mpsl (Nordic's Multiprotocol Service Layer) +
//! nrf-sdc (the SoftDevice Controller, LL only) + trouble-host (the Rust BLE host), folded into
//! the real firmware behind the `ble` feature. The A1 spike (#269) proved this exact dependency
//! trio + config on the DK; this module is its production home — the throwaway `ble_spike` bin
//! is retired.
//!
//! A2's scope was deliberately small: advertise (S0 §2 name) + hold a connection + publish link
//! status. **A3 (issue #271) makes the link layer boring** — a lifecycle that never wedges:
//!
//! - **The loop has no terminal states.** [`advertise_lifecycle`] → [`serve_connection`] →
//!   re-advertise, forever; any disconnect (for any reason) drops straight back to advertising,
//!   and even an advertise *error* only pauses a beat before retrying (S0 §2 "always just works").
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
//!
//! ### GATT control plane + a minimal CoC accept (A4, issue #272)
//!
//! The GATT surface is now the **real** control plane the iOS app discovers on connect
//! (`obc-ble-interface-spec.md` §3, the S0 UUIDs): **DIS** (real firmware revision / board id /
//! FICR serial), **BAS** (battery, notify — fed from the [`FuelGauge`] seam via [`publish_battery`]),
//! and the custom **OBC Control** service ([`ObcControlService`]) with all eight characteristics.
//! Writes are answered with the S0-typed `status` envelope, never a hang or a bare ATT failure:
//! `command` → `commandResult`, `transfer_control` → `transferResult(error)` (no data plane yet),
//! `config` is validated + accepted (round-trips in-session; storage wiring is A6).
//!
//! A4 also stands up a **minimal L2CAP CoC**: the SPSM [`OBC_PSM`] is registered on the stack and
//! published in the `psm` characteristic, and [`serve_coc`] accepts the channel and drains/discards
//! its bytes. That is deliberately *just enough* for the app's `connect()` to complete — it gates
//! completion on the L2CAP channel opening — the framing crate + transfer state machine + real
//! object payloads are A5/A6, pairing/bonding A8.
//!
//! ### The CoC data plane + the echo loopback (A5, issue #273)
//!
//! A5 turns that drain into a **real bulk-transfer data plane**, driven by the host-tested
//! [`obc_ble`] crate (the S0 descriptor codecs + the whole-object transfer state machine). The
//! control plane and the data plane are separate futures that coordinate through one [`Signal`]:
//!
//! - **Control plane** ([`serve_connection`]): a `transfer_control` write is decoded with
//!   [`obc_ble::TransferControl`] and classified ([`classify_transfer`]). An `echo` **upload** is
//!   *armed* — its descriptor is signalled to the CoC task ([`TRANSFER_ARM`]) and answered later by
//!   the data plane; every other op/type (real routes/rides are A6/A7) gets an immediate S0-typed
//!   [`obc_ble::TransferResult`] on the `status` characteristic, and an `abort` an `aborted` result.
//! - **Data plane** ([`serve_coc`] → [`run_echo`]): the CoC carries **only the object's payload
//!   bytes** (no per-chunk framing, S0 §5). The echo path feeds them through an
//!   [`obc_ble::Receiver`] — a running CRC-32 with no reassembly buffer — and streams each SDU
//!   straight back over the same channel, verifying **one** whole-object CRC at the end and
//!   notifying `committed` / `crcMismatch`. This is the loopback that proves the data plane end to
//!   end with **zero storage involvement**. On the first transfer the
//!   link is asked for the fast [`conn_params`] set (throughput); the kB/s is logged over RTT.
//!
//! ### The route object plane (A6, issue #274)
//!
//! A6 wires the data plane to real storage through the [`ObjectStore`] (`object_store.rs`: SD
//! catalog + object ids + the RRAM settings), shared between both planes as a `RefCell` that is
//! **never borrowed across an `await`** (one thread-mode executor — the borrows are all inside
//! sync sections):
//!
//! - **Route upload** ([`run_upload`]): CoC bytes sink straight into an SD temp with the running
//!   CRC; commit validates (CRC + OBCR header) and atomically promotes (see `sd.rs` — the
//!   held-back-magic substitute for FatFs' missing `rename`). Uploads don't resume (S0 §1
//!   principle 4): a CoC drop, a link drop, or an `op=3` abort discards the partial and the app
//!   re-sends the object from the start.
//! - **Downloads** ([`run_download`]): `routeList` (+ the empty `rideList` until A7) from a
//!   store-built buffer, route detail streamed off the card — announce descriptor first, then
//!   raw chunks, one whole-object CRC.
//! - **`deleteObject`** ([`run_command`]) and every store movement notify `storeChanged` + the
//!   refreshed `objectStore` digest ([`publish_store_change`]).
//! - **Config ↔ settings** ([`config_blob`] / `apply_config`): the `config` characteristic
//!   round-trips through the persisted settings; a rename reaches the airwaves on the next
//!   advertise cycle ([`advertised_name`] is re-read per cycle).
//!
//! ### The ride object plane + diagnostics (A7, issue #275)
//!
//! The reverse direction reuses the download machinery wholesale, because the Finish-time save
//! (`sd.rs::write_ride_object`) already stored each ride as **exactly** the S0 §7.2 wire bytes
//! (`/tracks/RD{id}.ORD`, the durable ride object id in the filename like `RT{id}.OBR`):
//! `rideList` is built from the stored headers, a ride download streams the file verbatim, and
//! ride ids survive reboots — what the app's synced-set / tombstone model keys on ("sync pulls
//! only rides it hasn't landed; deleting a ride in the app never resurrects it"). Rides are
//! never mutated over the link: no ride delete (`notFound`, see [`run_command`]), no replace —
//! a tracked ride never changes after it's recorded, so there is no route-style up-to-date
//! reconciliation. The `diagnostics` object (§7.5) is an honest text blob — identity, the
//! RRAM boot counter, uptime, the A3 link counters, store counts — rendered on request.
//!
//! ## Interrupts / priorities (the A1 inventory, reconciled at A2)
//!
//! MPSL claims `RADIO_0` / `TIMER10` / `GRTC_3` at **P0** (timing-critical), `CLOCK_POWER` and
//! its low-prio scheduling on **`SWI00`** at default priority. `SWI00` is why `main.rs`'s
//! high-priority `InterruptExecutor` lives on **`SWI01`** (every build — one ladder, no per-build
//! vector swap). The full ladder is documented in `main.rs`'s module doc.
//!
//! ## RAM
//!
//! Everything resident is a named static below (the `MaybeUninit`-in-`.bss` pattern — see
//! [`crate::init_static`]; `StaticCell`'s one-shot flag can panic on this board's warm-reset
//! path). [`RESIDENT_BYTES`] sums them for the N3 budget assert in `main.rs`, so the BLE build's
//! fit on the 256 KB DK is enforced at compile time like the map build's.
//!
//! ## Clocking (inherited from the spike — see #269)
//!
//! MPSL requires the HF **crystal**; LFCLK runs the internal **RC** with MPSL calibration, *not*
//! the 32 k crystal: the nRF54L's XO internal load caps are never programmed by embassy-nrf 0.11
//! or nrf-mpsl, so the LFXO runs off-frequency and every connection dies at establishment with
//! HCI 0x3E (advertising works — the failure needs a sync anchor). INTCAP-then-xtal is a filed
//! follow-up. `main.rs` sets both knobs in its `ble`-build boot config.

use core::cell::{Cell, RefCell};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::{join, join3};
use embassy_futures::select::{select, Either};
use embassy_nrf::mode::Blocking;
use embassy_nrf::{bind_interrupts, cracen, peripherals, Peri};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant, Timer};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use obc_ble::{
    CommandResult, CommandStatus, Config, ObjectType, Op, Receiver, StatusMessage, StoreChanged, TransferControl,
    TransferResult, TransferStatus,
};
use trouble_host::prelude::*;

use crate::init_static;
use crate::object_store::ObjectStore;

bind_interrupts!(struct Irqs {
    SWI00 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO_0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER10 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    GRTC_3 => nrf_sdc::mpsl::HighPrioInterruptHandler;
});

/// Ship config: the phone is the only peer.
const CONNECTIONS_MAX: usize = 1;
/// Advertising sets — one legacy connectable set is all the S0 §2 policy needs.
const ADV_SETS_MAX: usize = 1;
/// Bonded peers stored in the host (A8, S0 §8): exactly one. A fresh pairing replaces it, so the
/// resolving list never holds more than the single phone (matches the app's single-peer model).
const BONDS_MAX: usize = 1;
/// L2CAP signal + ATT + the data-plane CoC (A4 stands up the accept side; A5 gives it semantics).
const L2CAP_CHANNELS_MAX: usize = 3;
/// Outgoing/incoming LL buffers per link (the TrouBLE nrf54 example's values).
const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;

/// The dynamic L2CAP SPSM the CoC server listens on (S0 §5), published in the `psm` characteristic.
/// A fixed value in the LE dynamic range (`0x0080..=0x00FF`) — the app reads whatever we advertise,
/// so a constant is simpler than negotiating one and equally correct.
const OBC_PSM: u16 = 0x0080;

/// The OBC Control service UUID (`3C920000-9916-4EBA-ABC2-342FE08F6B10`, S0 §3.3) as the raw
/// **little-endian** 16 bytes the advertising AD structure wants (reverse of the display order).
/// Advertised so the app's `scanForPeripherals(withServices:)` filter matches.
const OBC_SERVICE_UUID_LE: [u8; 16] =
    [0x10, 0x6B, 0x8F, 0xE0, 0x2F, 0x34, 0xC2, 0xAB, 0xBA, 0x4E, 0x16, 0x99, 0x00, 0x00, 0x92, 0x3C];

/// SDC memory block, sized to `Builder::required_memory()` for this exact config — measured on
/// glass 2026-07-02 (logged at boot; the SDC warns if the block is bigger than needed and
/// errors if smaller). Re-measure after any Builder/buffer_cfg change.
const SDC_MEM_SIZE: usize = 4704;

/// TrouBLE's host arena for this config (connection state + the DefaultPacketPool at MTU 251 + the
/// single-peer bond storage the `security` feature adds).
type Resources = HostResources<
    nrf_sdc::SoftdeviceController<'static>,
    DefaultPacketPool,
    CONNECTIONS_MAX,
    L2CAP_CHANNELS_MAX,
    ADV_SETS_MAX,
    BONDS_MAX,
>;

/// The BLE build's resident statics, summed for the N3 budget assert in `main.rs` (the `ble`
/// analogue of the map build's App/cache terms): the MPSL handle, the SDC memory block, TrouBLE's
/// [`Resources`] arena, TrouBLE's **global `DEFAULT_POOL`** packet pool (`DefaultPacketPool` is a
/// type alias for the concrete pool struct, so its `size_of` *is* the pool's ~4 KB — sized by the
/// `default-packet-pool-mtu-251` feature), and the CRACEN RNG the LL's crypto pulls from. The
/// SDC/host *stack* usage rides `main.rs`'s `STACK_RESERVE` like everything else.
pub const RESIDENT_BYTES: usize = core::mem::size_of::<MultiprotocolServiceLayer<'static>>()
    + SDC_MEM_SIZE
    + core::mem::size_of::<Resources>()
    + core::mem::size_of::<DefaultPacketPool>()
    + core::mem::size_of::<cracen::Cracen<'static, Blocking>>();

// The resident statics (see the module doc): written exactly once in [`run`], never aliased.
static mut MPSL: MaybeUninit<MultiprotocolServiceLayer<'static>> = MaybeUninit::uninit();
static mut RNG: MaybeUninit<cracen::Cracen<'static, Blocking>> = MaybeUninit::uninit();
static mut SDC_MEM: MaybeUninit<sdc::Mem<SDC_MEM_SIZE>> = MaybeUninit::uninit();
static mut RESOURCES: MaybeUninit<Resources> = MaybeUninit::uninit();

// ============================ Link status → the status UI ============================

/// The link state the status UI shows.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum LinkState {
    /// Stack still coming up (the boot instant, before the first advertise).
    Init,
    /// Advertising, connectable — the powered-and-unconnected steady state (S0 §2).
    Advertising,
    /// A central holds the (single) link.
    Connected,
}

/// One coherent snapshot of the link for the status UI — published by the BLE plumbing below,
/// drained by `run_status` in `main.rs`. `Copy` so it crosses the mutex as a value.
#[derive(Clone, Copy)]
pub struct Status {
    pub state: LinkState,
    /// The connected central's address (little-endian, as the wire carries it), while connected.
    pub peer: Option<[u8; 6]>,
    /// The live connection interval (ms), once the central negotiated one; 0 = not reported yet.
    pub conn_interval_ms: u32,
    /// The negotiated ATT MTU (S0 §3.4 target 247); 0 = not exchanged yet.
    pub att_mtu: u16,
    /// True once the link runs on the 2M PHY (S0 §3.4 target).
    pub phy_2m: bool,
    /// Lifetime counters — the soak's at-a-glance health line.
    pub connects: u32,
    pub disconnects: u32,
    /// The HCI reason (status) code of the most recent disconnect; 0 = none yet. Logged in full
    /// (named) over RTT on each disconnect — this is the at-a-glance byte for the status screen.
    pub last_disconnect_reason: u8,
    /// The 6-digit LESC passkey to show on glass while pairing (A8, S0 §8) — `Some` between
    /// `PassKeyDisplay` and pairing completing/failing, `None` otherwise. When set, the status
    /// screen becomes the big-font passkey card the rider types into the phone.
    pub passkey: Option<u32>,
    /// True once the link is encrypted (a fresh pairing or a resumed bond, A8) — the status
    /// screen's "secured" marker and the CoC's open-gate.
    pub secured: bool,
}

impl Status {
    const INIT: Status = Status {
        state: LinkState::Init,
        peer: None,
        conn_interval_ms: 0,
        att_mtu: 0,
        phy_2m: false,
        connects: 0,
        disconnects: 0,
        last_disconnect_reason: 0,
        passkey: None,
        secured: false,
    };
}

/// The published snapshot ([`Status`] is `Copy`, so a plain `Cell` under the blocking mutex).
static STATUS: BlockingMutex<CriticalSectionRawMutex, Cell<Status>> = BlockingMutex::new(Cell::new(Status::INIT));
/// Edge the status UI sleeps on: signalled on every [`publish`], consumed by [`wait_status_change`].
static STATUS_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Read-modify-write the published status + wake the status UI.
fn publish(f: impl FnOnce(&mut Status)) {
    STATUS.lock(|c| {
        let mut s = c.get();
        f(&mut s);
        c.set(s);
    });
    STATUS_EDGE.signal(());
}

/// The status UI's snapshot read (any time, non-blocking).
pub fn status() -> Status {
    STATUS.lock(|c| c.get())
}

/// Sleep until the next [`publish`] — the status UI's wake edge.
pub async fn wait_status_change() {
    STATUS_EDGE.wait().await
}

/// The latest battery percent for the BAS characteristic (S0 §3.2) — written by the status plane
/// ([`publish_battery`], which owns the [`FuelGauge`]) and read by [`battery_task`] to seed + notify.
/// Seeded to the `StubFuelGauge` default so a read before the first poll is still plausible.
static BATTERY: AtomicU8 = AtomicU8::new(75);

/// Publish the latest battery percent for BAS (called by `run_status` after each fuel poll).
pub fn publish_battery(pct: u8) {
    BATTERY.store(pct, Ordering::Relaxed);
}

/// The latest published battery percent (BAS seed + notify).
fn battery() -> u8 {
    BATTERY.load(Ordering::Relaxed)
}

// ============================ Data-plane arming (A5/A6, S0 §4.2 / §5) ============================

/// A transfer the control plane validated and handed to the data plane: the echo loopback (A5),
/// a route upload with its ready fresh [`Receiver`] (the store opened the temp), or a download
/// (the data plane opens the source itself; opening may be slow — a CRC pre-pass — and belongs
/// off the GATT reply path).
#[derive(Clone, Copy)]
enum Armed {
    Echo(TransferControl),
    Upload(TransferControl, Receiver),
    Download(TransferControl),
}

/// The control plane → data plane hand-off: [`serve_connection`] decodes a `transfer_control`
/// write, validates it against the [`ObjectStore`], and signals the [`Armed`] transfer here;
/// [`serve_coc`] wakes on it and drives the CoC. A `Signal` (latest-value) suffices because S0
/// allows exactly one transfer in flight at a time (§4.1) — [`TRANSFER_ACTIVE`] turns a second
/// open into a typed `busy` instead of a silent overwrite.
static TRANSFER_ARM: Signal<CriticalSectionRawMutex, Armed> = Signal::new();

/// One-transfer-at-a-time (S0 §4.1): set by the control plane when it arms, cleared by the data
/// plane when the transfer concludes (answered, aborted, or the channel dropped). While set,
/// another `transferControl` open is answered `busy`.
static TRANSFER_ACTIVE: AtomicBool = AtomicBool::new(false);

/// An abort (S0 §4.2 op 3) aimed at the in-flight transfer: the control plane signals, the data
/// plane consumes it at its next step (between SDUs / chunks), discards, and answers `aborted`
/// with the durable offset. Latched — an abort that races the transfer's own completion is
/// drained by [`serve_coc`] after each transfer, so it can't leak into the next one.
static TRANSFER_ABORT: Signal<CriticalSectionRawMutex, ()> = Signal::new();

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
pub fn device_name() -> heapless::String<8> {
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
fn advertised_name(store: &ObjectStore) -> heapless::String<48> {
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
fn gatt_str<const N: usize>(args: core::fmt::Arguments<'_>) -> heapless09::String<N> {
    let mut s = heapless09::String::new();
    let _ = core::fmt::write(&mut s, args);
    s
}

/// The DIS **Serial Number** string (S0 §3.1): the 64-bit FICR `DEVICEID` as 16 uppercase hex
/// digits, high word first — so its last four digits are [`device_name`]'s `XXXX`.
pub fn serial_string() -> heapless09::String<16> {
    let id0 = unsafe { FICR_INFO_DEVICEID0.read_volatile() };
    let id1 = unsafe { FICR_INFO_DEVICEID1.read_volatile() };
    gatt_str(format_args!("{:08X}{:08X}", id1, id0))
}

/// The DIS **Firmware Revision** string (S0 §3.1): crate semver + git short hash, e.g. `0.1.0+ca9b336`
/// (`OBC_FW_GIT` is emitted by `build.rs`; `unknown` when git wasn't reachable at build time).
fn firmware_revision() -> heapless09::String<24> {
    gatt_str(format_args!("{}+{}", env!("CARGO_PKG_VERSION"), env!("OBC_FW_GIT")))
}

/// The DIS **Hardware Revision** string (S0 §3.1): the board id. The DK today; the LM20 board crate
/// changes this const when it lands.
const HARDWARE_REVISION: &str = "nrf54l15-dk";

/// A **static random** address derived from the factory device id (top two bits must be `11` per
/// the spec), so every board advertises a stable, distinct address. Real identity management
/// (resolvable addresses, bonding) is A8.
fn device_address() -> Address {
    let id0 = unsafe { FICR_INFO_DEVICEID0.read_volatile() }.to_le_bytes();
    let id1 = unsafe { FICR_INFO_DEVICEID1.read_volatile() }.to_le_bytes();
    // 46 factory-id bits + the mandatory `11` top bits of a static random address.
    Address::random([id0[0], id0[1], id0[2], id0[3], id1[0], id1[1] | 0xC0])
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

/// Timeout on every per-connection host procedure ([`negotiate_link`]). Generous — these are LL
/// round-trips with the peer — but finite, so a peer that never answers can't wedge the task.
const HOST_OP_TIMEOUT: Duration = Duration::from_secs(5);

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
pub fn conn_params(transfer_active: bool) -> RequestedConnParams {
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

// ============================ The stack ============================

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

/// Peripheral-only SDC at the config we intend to ship: legacy adv, 1 peripheral link,
/// DLE on with LL payload 251 (ATT MTU 247 + 4 L2CAP header), 2M PHY supported.
fn build_sdc<'d, const N: usize>(
    p: nrf_sdc::Peripherals<'d>,
    rng: &'d mut cracen::Cracen<'static, Blocking>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<nrf_sdc::SoftdeviceController<'d>, nrf_sdc::Error> {
    sdc::Builder::new()?
        .support_adv()
        .support_peripheral()
        .support_dle_peripheral()
        .support_le_2m_phy()
        .support_phy_update_peripheral()
        .peripheral_count(1)?
        .buffer_cfg(DefaultPacketPool::MTU as u16, DefaultPacketPool::MTU as u16, L2CAP_TXQ, L2CAP_RXQ)?
        .build(p, rng, mpsl, mem)
}

// The GATT control plane (A4, S0 §3): the two SIG services + the custom OBC Control service. The
// attribute table is auto-sized by the derive (no `attribute_table_size`); runtime values (DIS
// strings, the Config default) are seeded via `server.set` after `new_with_config` in `run`.
#[gatt_server]
struct Server {
    dis: DeviceInformationService,
    bas: BatteryService,
    obc: ObcControlService,
}

/// Device Information Service (S0 §3.1). All read-only strings, seeded at boot; `value` can't hold
/// a runtime string, so the macro declares them empty and `run` fills them.
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
struct DeviceInformationService {
    #[characteristic(uuid = characteristic::FIRMWARE_REVISION_STRING, read)]
    firmware_revision: heapless09::String<24>,
    #[characteristic(uuid = characteristic::HARDWARE_REVISION_STRING, read)]
    hardware_revision: heapless09::String<16>,
    #[characteristic(uuid = characteristic::SERIAL_NUMBER_STRING, read)]
    serial_number: heapless09::String<16>,
}

/// Battery Service (S0 §3.2): the level, read + notify — fed from the [`FuelGauge`] seam.
#[gatt_service(uuid = service::BATTERY)]
struct BatteryService {
    #[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 75)]
    level: u8,
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
struct ObcControlService {
    /// Small imperative commands (§4.4). Write; answered by a `status` `commandResult`.
    #[characteristic(uuid = "3C920001-9916-4EBA-ABC2-342FE08F6B10", write, permissions(authenticated))]
    command: heapless09::Vec<u8, 8>,
    /// Typed device → app messages (§4.3). Notify-only.
    #[characteristic(uuid = "3C920002-9916-4EBA-ABC2-342FE08F6B10", notify, permissions(authenticated))]
    status: heapless09::Vec<u8, 8>,
    /// The store digest (§4.5): revision + object counts. Seeded from the [`ObjectStore`] at
    /// boot; re-set + notified on every commit/delete ([`publish_store_change`]).
    #[characteristic(uuid = "3C920003-9916-4EBA-ABC2-342FE08F6B10", read, notify, permissions(authenticated), value = [0u8; 10])]
    object_store: [u8; 10],
    /// The Config object (§7.3), whole-blob read + write — round-trips through the persisted
    /// settings (A6): seeded at boot, re-seeded canonical after every accepted write.
    #[characteristic(uuid = "3C920004-9916-4EBA-ABC2-342FE08F6B10", read, write, permissions(authenticated))]
    config: heapless09::Vec<u8, 128>,
    /// Open / abort a CoC transfer (§4.2). Write + notify (the notify carries a download's
    /// filled announce descriptor).
    #[characteristic(uuid = "3C920005-9916-4EBA-ABC2-342FE08F6B10", write, notify, permissions(authenticated), value = [0u8; 16])]
    transfer_control: [u8; 16],
    /// Reserved (§7.5 — diagnostics cross the CoC): reads return 0 bytes.
    #[characteristic(uuid = "3C920006-9916-4EBA-ABC2-342FE08F6B10", read, permissions(authenticated))]
    diagnostics: heapless09::Vec<u8, 1>,
    /// The L2CAP CoC PSM the app opens the channel on (§3.3).
    #[characteristic(uuid = "3C920007-9916-4EBA-ABC2-342FE08F6B10", read, permissions(authenticated), value = OBC_PSM)]
    psm: u16,
    /// `protocol_version` (§1) — read **without** encryption (the connect-time version check
    /// happens before pairing). `1` for this contract.
    #[characteristic(uuid = "3C920008-9916-4EBA-ABC2-342FE08F6B10", read, value = 1)]
    protocol_version: u16,
}

// ============================ S0 control-plane codecs (§4.3 / §7.3) ============================
//
// The wire layouts themselves live in `obc_ble` (the host-tested crate the shared `protocol-vectors/`
// fixtures pin); these helpers only bridge them to the board's GATT attribute types and policy.

/// The canonical Config blob (S0 §7.3, Config v1) from the persisted settings: the stored rename
/// (or the factory name when unset — what the device actually advertises) + the units. Served on
/// the `config` read; re-seeded after every accepted write so reads always return canonical bytes.
fn config_blob(store: &ObjectStore) -> heapless09::Vec<u8, 128> {
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

/// A `status` notification's bytes, ready to hand to `server.notify` (`&buf[..len]`). The board keeps
/// one small stack buffer per message rather than a heapless alloc — every S0 status message fits.
type StatusBytes = ([u8; StatusMessage::MAX_ENCODED_LEN], usize);

/// What a `command` write did: the `commandResult` to notify, plus whether the store changed
/// (→ the caller also notifies `storeChanged` + the digest characteristic).
struct CommandOutcome {
    result: StatusBytes,
    store_changed: bool,
}

/// Execute a `command` write (S0 §4.3/§4.4). `deleteObject` (cmd 1: `type u8 · object_id u16`)
/// deletes a stored route through the [`ObjectStore`]. Ride deletion is **deliberately not
/// implemented** (`notFound`): the device retains every tracked ride until a future device-side
/// management UI — the app hides synced rides locally (tombstones) rather than deleting them
/// here, so a re-sync can never resurrect them. Any other command byte is `unknownCommand`.
fn run_command(data: &[u8], store: &RefCell<ObjectStore>) -> CommandOutcome {
    let cmd = data.first().copied().unwrap_or(0);
    let (status, store_changed) = match (cmd, data) {
        (1, [_, ty, lo, hi, ..]) => {
            let id = u16::from_le_bytes([*lo, *hi]);
            match ObjectType::from_u8(*ty) {
                Ok(ObjectType::Route) => {
                    if store.borrow_mut().delete_route(id) {
                        info!("ble: [cmd] deleted route object {}", id);
                        (CommandStatus::Ok, true)
                    } else {
                        (CommandStatus::NotFound, false)
                    }
                }
                // Rides are never deleted over the link (see the fn doc); nothing else deletes.
                _ => (CommandStatus::NotFound, false),
            }
        }
        (1, _) => (CommandStatus::Error, false), // deleteObject with a truncated arg list
        _ => (CommandStatus::UnknownCommand, false),
    };
    CommandOutcome { result: StatusMessage::CommandResult(CommandResult::new(cmd, status)).encode(), store_changed }
}

/// How a decoded `transfer_control` write proceeds (S0 §4.2).
enum TransferDisposition {
    /// Validated — hand to the CoC task ([`serve_coc`]), which answers when the transfer ends.
    Arm(Armed),
    /// Answer immediately on `status` (a reject, or an abort with nothing in flight).
    Answer(StatusBytes),
    /// An abort aimed at the in-flight transfer — signal the data plane; *it* answers.
    AbortActive,
}

/// Decode + classify a `transfer_control` write against the store (S0 §4.2): echo uploads (A5),
/// route uploads (fresh or replace-by-id), and route / list downloads. Everything invalid —
/// malformed bytes, an unknown id (`notFound`), a non-zero upload offset or a second open
/// mid-transfer, an unsupported op/type combination — is answered immediately with the S0-typed
/// [`TransferResult`] (`error` / `notFound` / `busy`), never a hang or a bare ATT failure.
fn classify_transfer(data: &[u8], store: &RefCell<ObjectStore>) -> TransferDisposition {
    let Ok(desc) = TransferControl::decode(data) else {
        // A malformed descriptor — the app can't have meant a real transfer; report `error`.
        return TransferDisposition::Answer(transfer_result(0, TransferStatus::Error));
    };
    if desc.op == Op::Abort {
        if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
            return TransferDisposition::AbortActive;
        }
        // Nothing in flight: discard any stray temp and confirm the abort.
        store.borrow_mut().upload_discard();
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Aborted));
    }
    if TRANSFER_ACTIVE.load(Ordering::Relaxed) {
        return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Busy));
    }
    match (desc.op, desc.ty) {
        (Op::Upload, ObjectType::Echo) => TransferDisposition::Arm(Armed::Echo(desc)),
        (Op::Upload, ObjectType::Route) => match store.borrow_mut().upload_open(&desc) {
            Ok(rx) => TransferDisposition::Arm(Armed::Upload(desc, rx)),
            Err(status) => TransferDisposition::Answer(transfer_result(desc.object_id, status)),
        },
        (
            Op::Download,
            ObjectType::Route
            | ObjectType::Ride
            | ObjectType::RouteList
            | ObjectType::RideList
            | ObjectType::Diagnostics,
        ) => {
            // Cheap existence check here for the immediate `notFound`; the source itself (and
            // its CRC pre-pass) opens on the data plane, off the GATT reply path.
            let known = match desc.ty {
                ObjectType::Route => store.borrow().has_route(desc.object_id),
                ObjectType::Ride => store.borrow().has_ride(desc.object_id),
                _ => true,
            };
            if !known {
                return TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::NotFound));
            }
            TransferDisposition::Arm(Armed::Download(desc))
        }
        // Uploads of ride/list/config/diagnostics types are nonsensical.
        _ => TransferDisposition::Answer(transfer_result(desc.object_id, TransferStatus::Error)),
    }
}

/// A `transferResult` status message (S0 §4.3 `msg=1`) with a zero `committed_offset` — the shape
/// for every result the control plane answers directly (nothing durable is being reported, §4.2).
fn transfer_result(object_id: u16, status: TransferStatus) -> StatusBytes {
    transfer_result_at(object_id, status, 0)
}

/// A `transferResult` carrying a real durable byte count — a committed transfer reports its
/// `total_len` (S0 §4.3).
fn transfer_result_at(object_id: u16, status: TransferStatus, committed_offset: u32) -> StatusBytes {
    StatusMessage::TransferResult(TransferResult::new(object_id, status, committed_offset)).encode()
}

/// Bring the whole stack up and run it forever: MPSL (spawned — it must outlive everything),
/// SDC, the TrouBLE host, then the S0 advertise → connect → re-advertise loop, publishing every
/// link edge for the status UI. Joined against `run_status` on the thread-mode executor in
/// `main.rs`. The MPSL/SDC peripheral sets are built in `main` (where the `Peripherals` struct
/// is split) and handed in whole.
pub async fn run(
    spawner: Spawner,
    mpsl_p: mpsl::Peripherals<'static>,
    sdc_p: sdc::Peripherals<'static>,
    cracen_p: Peri<'static, peripherals::CRACEN>,
    store: ObjectStore,
) -> ! {
    // The object store (A6): SD catalog + RRAM settings behind one RefCell. The control plane
    // (GATT writes) and the data plane (CoC transfers) both borrow it, always synchronously —
    // never across an `await` — which is sound on the one thread-mode executor.
    let store = RefCell::new(store);
    // LFCLK = the internal RC at Nordic's recommended calibration cadence (calibrate every
    // 16×0.25 s = 4 s; temp-check every 2 intervals) — guarantees the ±500 ppm class the accuracy
    // field claims. NOT the 32 k crystal — see the module doc (unprogrammed XO INTCAPs → HCI 0x3E).
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: 500,
        skip_wait_lfclk_started: false,
    };
    // SAFETY (all four `init_static` calls): each slot is written exactly once here — `run` is
    // called once from `main` — and the returned `&'static mut` is the sole reference.
    let mpsl: &'static MultiprotocolServiceLayer = unsafe {
        init_static(
            core::ptr::addr_of_mut!(MPSL),
            unwrap!(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk_cfg)),
        )
    };
    spawner.spawn(unwrap!(mpsl_task(mpsl)));

    // The LL pulls its crypto randomness from CRACEN (the nRF54L has no legacy RNG peripheral).
    let rng = unsafe { init_static(core::ptr::addr_of_mut!(RNG), cracen::Cracen::new_blocking(cracen_p)) };

    // Log the exact SDC memory requirement for this config — the number `SDC_MEM_SIZE` pins.
    match sdc::Builder::new().and_then(|b| {
        b.support_adv()
            .support_peripheral()
            .support_dle_peripheral()
            .support_le_2m_phy()
            .support_phy_update_peripheral()
            .peripheral_count(1)?
            .buffer_cfg(DefaultPacketPool::MTU as u16, DefaultPacketPool::MTU as u16, L2CAP_TXQ, L2CAP_RXQ)?
            .required_memory()
    }) {
        Ok(required) => info!("ble: sdc required_memory = {} bytes (SDC_MEM_SIZE = {})", required, SDC_MEM_SIZE),
        Err(e) => warn!("ble: sdc required_memory failed: {:?}", e),
    }

    let sdc_mem = unsafe { init_static(core::ptr::addr_of_mut!(SDC_MEM), sdc::Mem::new()) };
    let sdc = unwrap!(build_sdc(sdc_p, rng, mpsl, sdc_mem));

    let resources = unsafe { init_static(core::ptr::addr_of_mut!(RESOURCES), HostResources::new()) };
    let address = device_address();
    // The GAP name is pinned at boot (the attribute borrows it for the server's life); the
    // *advertised* name is re-read from the store each advertise cycle, so a rename lands
    // without a reboot (S0 §7.3 Delta 1 — the Config characteristic is authoritative anyway).
    let name = advertised_name(&store.borrow());
    info!("ble: host up as '{}', address {:?}", name.as_str(), address);

    // Register the CoC SPSM up front (S0 §5) so `serve_coc` can accept on it once a link is up.
    // IO = DisplayOnly (A8, S0 §8): the device shows a 6-digit passkey, the phone (keyboard)
    // enters it → LESC passkey-entry pairing, MITM-protected. Keep the static-random address
    // (no device privacy) — the phone stores our stable identity for instant reconnect; we
    // resolve *its* rotating RPA from the stored peer IRK below.
    let stack = trouble_host::new(sdc, resources)
        .set_random_address(address)
        .set_io_capabilities(IoCapabilities::DisplayOnly)
        .register_l2cap_spsm(OBC_PSM)
        .build();

    // Re-establish the stored bond (A8, S0 §8): hand it to the host so the controller's resolving
    // list resolves the bonded phone's RPA on reconnect and re-encrypts with the stored LTK — no
    // dialog, no interaction ("it just works"). Absent/torn → open pairing.
    if let Some(bond) = store.borrow_mut().load_bond() {
        match stack.add_bond_information(bond) {
            Ok(()) => info!("ble: restored stored bond — bonded reconnect armed"),
            Err(e) => warn!("ble: add_bond_information failed: {:?}", defmt::Debug2Format(&e)),
        }
    }

    let runner = stack.runner();
    let mut peripheral = stack.peripheral();

    let server = unwrap!(Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: name.as_str(),
        appearance: &appearance::cycling::CYCLING_COMPUTER,
    })));

    // Seed the runtime attribute values the macro `value =` can't hold (DIS strings, the Config
    // blob from the persisted settings, the store digest). `server.set` writes the shared
    // attribute table once — no connection needed.
    let _ = server.set(&server.dis.firmware_revision, &firmware_revision());
    let _ = server.set(&server.dis.hardware_revision, &gatt_str::<16>(format_args!("{HARDWARE_REVISION}")));
    let _ = server.set(&server.dis.serial_number, &serial_string());
    let _ = server.set(&server.obc.config, &config_blob(&store.borrow()));
    let _ = server.set(&server.obc.object_store, &store.borrow().digest().encode());
    info!(
        "ble: DIS fw '{}' hw '{}' serial '{}'",
        firmware_revision().as_str(),
        HARDWARE_REVISION,
        serial_string().as_str()
    );

    // The lifecycle loop (A3): advertise → serve → re-advertise, forever, with no terminal state.
    join(host_task(runner), async {
        loop {
            publish(|s| {
                s.state = LinkState::Advertising;
                s.peer = None;
                s.conn_interval_ms = 0;
                s.att_mtu = 0;
                s.phy_2m = false;
                s.passkey = None;
                s.secured = false;
            });
            // Re-read the advertised name each cycle — a rename (Config write) takes effect on
            // the next advertising start, no reboot (S0 §2: the Config name replaces factory).
            let adv_name = advertised_name(&store.borrow());
            let conn = match advertise_lifecycle(adv_name.as_str(), &mut peripheral, &server).await {
                Ok(conn) => conn,
                Err(e) => {
                    // "Always just works" (S0 §2): an advertise error must not take the firmware
                    // down and must not wedge the loop — log it, wait a beat, and try again.
                    warn!("ble: advertise error: {:?} — retrying in 1 s", defmt::Debug2Format(&e));
                    Timer::after_secs(1).await;
                    continue;
                }
            };

            let peer = conn.raw().peer_address();
            let mut peer_bytes = [0u8; 6];
            peer_bytes.copy_from_slice(peer.addr.raw());
            publish(|s| {
                s.state = LinkState::Connected;
                s.peer = Some(peer_bytes);
                s.connects += 1;
            });

            // Allow this link to bond (A8): the trouble default is *not* bondable, so a passkey
            // pairing wouldn't persist keys without this. Set before the peer starts SMP.
            if let Err(e) = conn.raw().set_bondable(true) {
                warn!("ble: set_bondable failed: {:?}", defmt::Debug2Format(&e));
            }

            // Serve the link until the peer drops it. `serve_connection` pumps GATT + connection
            // events (so the phone's own MTU/PHY/DLE moves are serviced and our control-plane writes
            // are answered) and owns the exit — it returns the disconnect reason. The background set
            // (parameter negotiation, the CoC accept-and-drain, and the BAS battery notify) runs
            // concurrently and never returns, so `select` tears it all down the moment the link drops
            // (S0 §2: any disconnect drops straight back to advertising).
            let reason = match select(
                serve_connection(&stack, &server, &conn, &store),
                join3(
                    negotiate_link(&stack, &conn),
                    serve_coc(&stack, &server, &conn, &store),
                    battery_task(&stack, &server, &conn),
                ),
            )
            .await
            {
                Either::First(reason) => reason,
                Either::Second(_) => unreachable!("the background futures never return"),
            };
            // The drop may have cancelled the data plane mid-transfer (at an await): discard any
            // in-flight upload + release the store's open handles, clear the one-transfer gate, and
            // drain any latched arm/abort so the next connection starts clean (uploads restart).
            store.borrow_mut().link_reset();
            TRANSFER_ACTIVE.store(false, Ordering::Relaxed);
            TRANSFER_ARM.reset();
            TRANSFER_ABORT.reset();
            publish(|s| {
                s.disconnects += 1;
                s.last_disconnect_reason = reason;
            });
        }
    })
    .await;
    unreachable!()
}

/// The host's transport pump — must run forever alongside the advertise loop.
async fn host_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) -> ! {
    loop {
        if let Err(e) = runner.run().await {
            let e = defmt::Debug2Format(&e);
            defmt::panic!("ble: host runner error: {:?}", e);
        }
    }
}

/// Serve GATT + connection events until the peer drops the link. Returns the disconnect reason
/// (HCI status code); answers the OBC Control writes with the S0-typed `status` envelope, publishes
/// the link edges the status UI shows (conn interval, PHY) and logs the rest — including every
/// disconnect reason, named + numeric — for the `A9` soak's RTT trail. Concrete SDC/pool types
/// (like [`negotiate_link`]): the `status` notify needs the `stack`, and this only runs on the one
/// controller.
async fn serve_connection(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &RefCell<ObjectStore>,
) -> u8 {
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => break reason,
            GattConnectionEvent::Gatt { event } => {
                // Extract what a control-plane write needs answered *before* accepting (which
                // consumes the event), then notify the S0 `status` message(s) — never a hang /
                // bare ATT failure. A validated transfer instead arms the CoC data plane
                // ([`serve_coc`]), which answers when it ends. Store borrows stay inside the
                // sync `with_data` closures — never across an await.
                let mut status_msg: Option<StatusBytes> = None;
                let mut store_changed = false;
                let mut config_written = false;
                let reply = match event {
                    GattEvent::Write(e) => {
                        let handle = e.handle();
                        if handle == server.obc.command.handle {
                            let outcome = e.with_data(|_off, data| run_command(data, store));
                            status_msg = Some(outcome.result);
                            store_changed = outcome.store_changed;
                            info!("ble: [gatt] command write");
                            e.accept()
                        } else if handle == server.obc.transfer_control.handle {
                            match e.with_data(|_off, data| classify_transfer(data, store)) {
                                TransferDisposition::Arm(armed) => {
                                    info!("ble: [gatt] transfer_control: transfer armed");
                                    TRANSFER_ACTIVE.store(true, Ordering::Relaxed);
                                    TRANSFER_ARM.signal(armed);
                                }
                                TransferDisposition::AbortActive => {
                                    info!("ble: [gatt] transfer_control: abort → data plane");
                                    TRANSFER_ABORT.signal(());
                                }
                                TransferDisposition::Answer(bytes) => {
                                    info!("ble: [gatt] transfer_control: answered on status");
                                    status_msg = Some(bytes);
                                }
                            }
                            e.accept()
                        } else if handle == server.obc.config.handle {
                            // Validate + apply: units and rename persist to RRAM settings
                            // (S0 §7.3); the advertised name follows on the next adv cycle.
                            let applied = e.with_data(|_off, data| match Config::decode(data) {
                                Some(cfg) => match core::str::from_utf8(cfg.name) {
                                    Ok(name) => {
                                        store.borrow_mut().apply_config(name, cfg.units);
                                        true
                                    }
                                    Err(_) => false,
                                },
                                None => false,
                            });
                            if applied {
                                info!("ble: [gatt] config write applied + persisted");
                                config_written = true;
                                e.accept()
                            } else {
                                warn!("ble: [gatt] config write rejected (malformed)");
                                e.reject(AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH)
                            }
                        } else {
                            info!("ble: [gatt] write handle {}", handle);
                            e.accept()
                        }
                    }
                    GattEvent::Read(e) => {
                        info!("ble: [gatt] read handle {}", e.handle());
                        e.accept()
                    }
                    // Permission-violating request (e.g. a write to a read-only attribute): accepting
                    // lets the server send the proper ATT error response rather than dropping it.
                    GattEvent::NotAllowed(e) => e.accept(),
                    GattEvent::Other(e) => e.accept(),
                };
                match reply {
                    Ok(reply) => reply.send().await,
                    Err(e) => warn!("ble: [gatt] error sending response: {:?}", e),
                }
                if let Some((buf, len)) = status_msg {
                    if let Err(e) = server.notify(stack, server.obc.status.handle, &buf[..len]).await {
                        warn!("ble: [gatt] status notify failed: {:?}", defmt::Debug2Format(&e));
                    }
                }
                if store_changed {
                    publish_store_change(stack, server, store).await;
                }
                if config_written {
                    // Re-seed the characteristic with the canonical blob (what a read serves).
                    let _ = server.set(&server.obc.config, &config_blob(&store.borrow()));
                }
            }
            GattConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                info!("ble: [conn] PHY updated: tx {:?} rx {:?}", tx_phy, rx_phy);
                // "2M" on the status screen only when both directions made it (S0 §3.4 target).
                publish(|s| s.phy_2m = matches!(tx_phy, PhyKind::Le2M) && matches!(rx_phy, PhyKind::Le2M));
            }
            GattConnectionEvent::ConnectionParamsUpdated { conn_interval, peripheral_latency, supervision_timeout } => {
                info!(
                    "ble: [conn] params: interval {} ms latency {} timeout {} ms",
                    conn_interval.as_millis(),
                    peripheral_latency,
                    supervision_timeout.as_millis()
                );
                publish(|s| s.conn_interval_ms = conn_interval.as_millis() as u32);
            }
            GattConnectionEvent::DataLengthUpdated { max_tx_octets, max_rx_octets, .. } => {
                info!("ble: [conn] data length: tx {} rx {} octets", max_tx_octets, max_rx_octets);
            }

            // ---- Pairing / bonding lifecycle (A8, S0 §8) ----
            // The device is DisplayOnly: the phone drives passkey *entry*, so `PassKeyDisplay` is
            // the one we expect — show the 6-digit code big on the status screen; the rider types
            // it into the iOS dialog. (Confirm/Input are handled defensively for completeness.)
            GattConnectionEvent::PassKeyDisplay(passkey) => {
                info!("ble: [pair] display passkey {=u32:06}", passkey.value());
                publish(|s| s.passkey = Some(passkey.value()));
            }
            GattConnectionEvent::PassKeyConfirm(passkey) => {
                info!("ble: [pair] confirm passkey {=u32:06}", passkey.value());
                publish(|s| s.passkey = Some(passkey.value()));
            }
            GattConnectionEvent::PassKeyInput => {
                info!("ble: [pair] peer requests passkey input");
            }
            GattConnectionEvent::PairingComplete { security_level, bond } => {
                info!("ble: [pair] complete — level {:?}, bonded {}", security_level, bond.is_some());
                // Persist the single bond (S0 §8): a fresh pairing replaces whatever was stored.
                if let Some(bond) = bond {
                    store.borrow_mut().save_bond(&bond);
                }
                publish(|s| {
                    s.passkey = None;
                    s.secured = true;
                });
            }
            GattConnectionEvent::PairingFailed(e) => {
                warn!("ble: [pair] failed: {:?}", defmt::Debug2Format(&e));
                // The link usually drops on failure → the app lands on D5 and we re-advertise.
                publish(|s| s.passkey = None);
            }
            GattConnectionEvent::Encrypted { security_level, bond } => {
                // Fires for a resumed bonded session too (no pairing UI) — mark the link secured.
                info!("ble: [pair] encrypted — level {:?}, from bond {}", security_level, bond.is_some());
                publish(|s| {
                    s.passkey = None;
                    s.secured = true;
                });
            }
            GattConnectionEvent::BondLost => {
                // The peer paired again despite our stored bond ⇒ it lost its keys (the app/OS
                // "forgot" us). Drop our stale bond so this fresh pairing is the new one.
                warn!("ble: [pair] peer lost its bond — clearing stored bond");
                store.borrow_mut().clear_bond();
            }
            _ => {}
        }
    };
    info!("ble: [conn] disconnected, reason 0x{:02X} ({:?})", reason.into_inner(), reason);
    reason.into_inner()
}

/// The L2CAP CoC data plane (A5/A6): accept the app's channel on [`OBC_PSM`] and serve the
/// transfers [`serve_connection`] arms through [`TRANSFER_ARM`] — the echo loopback, route
/// uploads → SD, and route/list downloads ← SD. One armed transfer at a time (S0 §4.1); the
/// [`TRANSFER_ACTIVE`] gate is cleared here when each concludes, and a latched abort that raced
/// a completion is drained so it can't leak into the next transfer. A channel drop mid-transfer
/// breaks back to re-accept (the in-flight upload was discarded — uploads restart, S0 §1
/// principle 4); [`select`] in `run` cancels the whole task on disconnect. Never returns.
async fn serve_coc(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &RefCell<ObjectStore>,
) -> ! {
    let listener = L2capChannel::listen(stack, conn.raw());
    // The receive buffer must be ≥ the negotiated SDU MTU (defaults to the pool MTU − 6 = 245).
    let mut buf = [0u8; DefaultPacketPool::MTU];
    // Ask for the fast connection-parameter set (S0 §3.4) once, on the first transfer of the link —
    // the idle set is re-established on the next connect, so there's no per-transfer churn.
    let mut requested_fast = false;
    loop {
        let mut ch = match listener.accept(&L2capChannelConfig::default()).await {
            Ok(ch) => ch,
            Err(e) => {
                // A failed accept while the link is up shouldn't hot-spin — back off a beat. On a
                // real disconnect the `run` `select` has already dropped this future.
                warn!("ble: [coc] accept failed: {:?}", defmt::Debug2Format(&e));
                Timer::after_millis(200).await;
                continue;
            }
        };
        // The CoC requires an encrypted link (A8, S0 §8): opening it plaintext is refused. In
        // practice the app can't reach here unencrypted — `psm`/`transferControl` are both
        // `authenticated` — but a peer that guessed the SPSM must still be turned away.
        if !matches!(conn.raw().security_level(), Ok(level) if level.encrypted()) {
            warn!("ble: [coc] channel opened on an unencrypted link — refusing (S0 §8)");
            ch.disconnect();
            continue;
        }
        info!("ble: [coc] channel accepted (mtu {} mps {}) — data plane ready", ch.mtu(), ch.mps());
        loop {
            let armed = TRANSFER_ARM.wait().await;
            if !requested_fast {
                requested_fast = true;
                request_fast_conn_params(stack, conn).await;
            }
            let outcome = match armed {
                Armed::Echo(desc) => run_echo(stack, server, &mut ch, &desc, &mut buf).await,
                Armed::Upload(desc, rx) => run_upload(stack, server, store, &mut ch, &desc, rx, &mut buf).await,
                Armed::Download(desc) => run_download(stack, server, store, &mut ch, &desc, &mut buf).await,
            };
            // The transfer concluded (or the channel died): reopen the gate, and drain an abort
            // that raced the conclusion so it can't insta-abort the next transfer.
            TRANSFER_ACTIVE.store(false, Ordering::Relaxed);
            let _ = TRANSFER_ABORT.try_take();
            if let TransferOutcome::ChannelDropped = outcome {
                warn!("ble: [coc] channel dropped mid-transfer — re-accepting (uploads restart)");
                break;
            }
        }
    }
}

/// Whether a transfer runner answered on `status` or the CoC dropped under it (→ [`serve_coc`]
/// re-accepts; a re-upload arrives as a fresh arm, S0 §1 principle 4).
enum TransferOutcome {
    Answered,
    ChannelDropped,
}

/// Notify the store movement after a commit/delete: the `storeChanged` status message (which
/// store, new revision) + the refreshed `objectStore` digest characteristic (S0 §4.3/§4.5).
async fn publish_store_change(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
) {
    let digest = store.borrow().digest();
    let _ = server.set(&server.obc.object_store, &digest.encode());
    if let Err(e) = server.notify(stack, server.obc.object_store.handle, &digest.encode()).await {
        warn!("ble: [store] digest notify failed: {:?}", defmt::Debug2Format(&e));
    }
    let msg = StatusMessage::StoreChanged(StoreChanged { ty: ObjectType::Route, revision: digest.revision });
    notify_status(server, stack, msg.encode()).await;
}

/// A route upload (S0 §4.2 op 1, type 1): sink CoC bytes through the [`Receiver`] into the SD
/// temp, then commit — CRC verify, OBCR-header validate, atomic promote (see `sd.rs`) — and
/// answer with the assigned id. Uploads don't resume (S0 §1 principle 4): a channel drop or an
/// abort (op 3) discards the partial, and the app re-sends the object from the start.
async fn run_upload(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    mut rx: Receiver,
    buf: &mut [u8],
) -> TransferOutcome {
    info!("ble: [coc] route upload start: {} bytes", desc.total_len);
    while !rx.is_complete() {
        let n = match select(ch.receive(stack, buf), TRANSFER_ABORT.wait()).await {
            Either::First(Ok(n)) if n > 0 => n,
            Either::First(_) => {
                // Error or an empty SDU with bytes still expected — the channel is done for.
                // Discard the partial; the app re-uploads from the start.
                store.borrow_mut().upload_discard();
                info!("ble: [coc] upload interrupted — discarded (uploads restart)");
                notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::ChannelDropped;
            }
            Either::Second(()) => {
                // The app aborted (S0 §4.2 op 3): discard and confirm.
                store.borrow_mut().upload_discard();
                info!("ble: [coc] upload aborted by the app");
                notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Aborted)).await;
                return TransferOutcome::Answered;
            }
        };
        let consumed = rx.push(&buf[..n]);
        if !store.borrow_mut().upload_append(&buf[..consumed]) {
            store.borrow_mut().upload_discard();
            warn!("ble: [coc] SD append failed — upload rejected");
            notify_status(server, stack, transfer_result(rx.object_id(), TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
    }
    let (id, status) = store.borrow_mut().upload_finish(&rx);
    let committed = status == TransferStatus::Committed;
    info!("ble: [coc] upload finished: id {} -> {}", id, if committed { "committed" } else { "rejected" });
    let offset = if committed { rx.total_len() } else { 0 };
    notify_status(server, stack, transfer_result_at(id, status, offset)).await;
    if committed {
        publish_store_change(stack, server, store).await;
    }
    TransferOutcome::Answered
}

/// A download (S0 §4.2 op 2): open the source (`routeList` / `rideList` / diagnostics from the
/// store's built buffer, a route or ride detail straight off the card with its CRC pre-pass),
/// notify the filled announce descriptor, then stream the object in CoC chunks. An abort between
/// chunks stops cleanly; a send failure means the channel dropped (the app re-requests, S0 §4.2).
async fn run_download(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &RefCell<ObjectStore>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    buf: &mut [u8],
) -> TransferOutcome {
    // Bind the open's result before matching — a `match store.borrow_mut().…` scrutinee
    // temporary would keep the borrow alive through the error arm's await. Diagnostics render
    // from the link plane's own facts (S0 §7.5), everything else opens through the catalog.
    let opened = if desc.ty == ObjectType::Diagnostics {
        let fw = firmware_revision();
        let serial = serial_string();
        let s = status();
        let diag = crate::object_store::DiagInput {
            firmware: fw.as_str(),
            hardware: HARDWARE_REVISION,
            serial: serial.as_str(),
            uptime_s: Instant::now().as_secs() as u32,
            connects: s.connects,
            disconnects: s.disconnects,
            last_disconnect_reason: s.last_disconnect_reason,
        };
        store.borrow_mut().open_diagnostics_download(desc, &diag)
    } else {
        store.borrow_mut().download_open(desc)
    };
    let (mut tx, source) = match opened {
        Ok(open) => open,
        Err(status) => {
            notify_status(server, stack, transfer_result(desc.object_id, status)).await;
            return TransferOutcome::Answered;
        }
    };
    // Announce on `transferControl` (same 16 bytes, total_len + crc32 filled in), then stream.
    let announce = tx.announce();
    info!("ble: [coc] download start: {} bytes from offset {}", announce.total_len, announce.offset);
    if let Err(e) = server.notify(stack, server.obc.transfer_control.handle, &announce.encode()).await {
        warn!("ble: [coc] announce notify failed: {:?}", defmt::Debug2Format(&e));
        store.borrow_mut().download_close();
        return TransferOutcome::Answered;
    }
    while !tx.is_complete() {
        if TRANSFER_ABORT.try_take().is_some() {
            store.borrow_mut().download_close();
            info!("ble: [coc] download aborted by the app");
            notify_status(server, stack, transfer_result_at(desc.object_id, TransferStatus::Aborted, tx.position()))
                .await;
            return TransferOutcome::Answered;
        }
        let n = tx.next_chunk_len(CHUNK_LEN.min(buf.len()));
        if !store.borrow_mut().download_read(source, tx.position(), &mut buf[..n]) {
            store.borrow_mut().download_close();
            warn!("ble: [coc] SD read failed — download abandoned");
            notify_status(server, stack, transfer_result(desc.object_id, TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
        if let Err(e) = ch.send(stack, &buf[..n]).await {
            info!("ble: [coc] download send ended: {:?}", defmt::Debug2Format(&e));
            store.borrow_mut().download_close();
            return TransferOutcome::ChannelDropped;
        }
        tx.advance(n);
    }
    store.borrow_mut().download_close();
    let result = tx.outcome().unwrap(); // complete ⇒ Some
    info!("ble: [coc] download done: {} bytes", result.committed_offset);
    notify_status(server, stack, StatusMessage::TransferResult(result).encode()).await;
    TransferOutcome::Answered
}

/// One CoC SDU's worth of download payload (S0 §3.4: 244 rides one 251-byte PDU on a DLE link).
const CHUNK_LEN: usize = 244;

/// The A5 echo loopback (S0 object type 8): receive the announced object over the CoC and stream it
/// straight back, byte-for-byte, verifying **one** whole-object CRC-32 at the end (S0 §6) — the data
/// plane proven with zero storage. Sinks each SDU through an [`obc_ble::Receiver`] (a running CRC, no
/// reassembly buffer) and echoes exactly the consumed bytes; on completion notifies the S0
/// `transferResult` (`committed` / `crcMismatch`) and logs the throughput the issue asks for.
async fn run_echo(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    ch: &mut L2capChannel<'_, DefaultPacketPool>,
    desc: &TransferControl,
    buf: &mut [u8],
) -> TransferOutcome {
    let mut rx = match Receiver::new(desc) {
        Ok(rx) => rx,
        Err(_) => {
            // A nonsensical echo descriptor (e.g. offset past total_len) — answer error, leave the
            // channel untouched (no bytes were promised).
            notify_status(server, stack, transfer_result(desc.object_id, TransferStatus::Error)).await;
            return TransferOutcome::Answered;
        }
    };
    info!("ble: [coc] echo start: {} bytes", rx.total_len());
    let started = Instant::now();
    while !rx.is_complete() {
        let n = match ch.receive(stack, buf).await {
            Ok(0) => {
                // An empty SDU can't advance a transfer with bytes still expected — treat it as an
                // end-of-stream rather than spinning the receive loop.
                info!("ble: [coc] echo receive returned 0 bytes — ending");
                return TransferOutcome::ChannelDropped;
            }
            Ok(n) => n,
            Err(e) => {
                info!("ble: [coc] echo receive ended: {:?}", defmt::Debug2Format(&e));
                return TransferOutcome::ChannelDropped;
            }
        };
        let consumed = rx.push(&buf[..n]);
        if let Err(e) = ch.send(stack, &buf[..consumed]).await {
            info!("ble: [coc] echo send failed: {:?}", defmt::Debug2Format(&e));
            return TransferOutcome::ChannelDropped;
        }
    }
    let result = rx.outcome().unwrap(); // complete ⇒ Some
    let committed = result.status == TransferStatus::Committed;
    let elapsed_ms = (started.elapsed().as_millis()).max(1);
    // kB/s = bytes / seconds / 1024; kept in u64 (bytes × 1000 can't overflow a real object).
    let kbps = (rx.total_len() as u64) * 1000 / (elapsed_ms * 1024);
    info!(
        "ble: [coc] echo done: {} bytes in {} ms (~{} kB/s) -> {}",
        rx.total_len(),
        elapsed_ms,
        kbps,
        if committed { "committed" } else { "crcMismatch" }
    );
    notify_status(server, stack, StatusMessage::TransferResult(result).encode()).await;
    TransferOutcome::Answered
}

/// Notify one S0 `status` message (the CoC data plane's channel to the app).
async fn notify_status(
    server: &Server<'_>,
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    (buf, len): StatusBytes,
) {
    if let Err(e) = server.notify(stack, server.obc.status.handle, &buf[..len]).await {
        warn!("ble: [coc] status notify failed: {:?}", defmt::Debug2Format(&e));
    }
}

/// Request the fast connection-parameter set (S0 §3.4) for a transfer's throughput — best-effort and
/// timeout-bounded like [`negotiate_link`]'s requests (a peer that ignores it just runs slower).
async fn request_fast_conn_params(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) {
    let raw = conn.raw();
    if !raw.is_connected() {
        return;
    }
    let params = conn_params(true);
    match with_timeout(HOST_OP_TIMEOUT, raw.update_connection_params(stack, &params)).await {
        Ok(Ok(())) => info!("ble: [coc] requested fast conn params for transfer"),
        Ok(Err(e)) => warn!("ble: [coc] fast conn params failed: {:?}", defmt::Debug2Format(&e)),
        Err(_) => warn!("ble: [coc] fast conn params timed out"),
    }
}

/// Push the BAS battery level (S0 §3.2) to a subscribed central: seed on connect, then re-notify on
/// a slow cadence. The value comes from the [`FuelGauge`] seam via [`publish_battery`] (the status
/// plane owns the gauge). The stub is constant today — the notify *wiring* is what A4 proves.
/// Never returns; cancelled by [`select`] in `run` on disconnect.
async fn battery_task(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) -> ! {
    let level = server.bas.level;
    loop {
        let pct = battery();
        let _ = conn.set(&level, &pct); // keep the readable value in step with the notify
        if let Err(e) = server.notify(stack, level.handle, &pct).await {
            warn!("ble: [bas] battery notify failed: {:?}", defmt::Debug2Format(&e));
        }
        Timer::after_secs(30).await;
    }
}

/// Negotiate the S0 §3.4 link parameters, best-effort. Each step is a *preference* — the protocol
/// is correct at any negotiated MTU/PHY, just slower — and each is [`HOST_OP_TIMEOUT`]-bounded, so
/// a peer that ignores or stalls a procedure degrades the link (log + skip) but never wedges the
/// task. Runs concurrently with [`serve_connection`], which services the peer's own moves (and its
/// ATT MTU exchange) while these requests are in flight. Concrete SDC type: the extra command
/// bounds (`LeSetPhy` / `LeSetDataLength` / `LeReadLocalSupportedFeatures`) aren't in the
/// `trouble_host::Controller` bundle, and this only ever runs on the one controller.
async fn negotiate_link(
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
async fn advertise_lifecycle<'values, 'server, C: Controller>(
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
