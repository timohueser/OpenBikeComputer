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
//! The GATT surface is still a stub battery service so a phone's discovery walk has real
//! attributes — the real control plane (DIS + BAS + OBC Control) is A4, the CoC data plane A5,
//! pairing/bonding A8.
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

use core::cell::Cell;
use core::mem::MaybeUninit;

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{select, Either};
use embassy_nrf::mode::Blocking;
use embassy_nrf::{bind_interrupts, cracen, peripherals, Peri};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Timer};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use trouble_host::prelude::*;

use crate::init_static;

bind_interrupts!(struct Irqs {
    SWI00 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO_0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER10 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    GRTC_3 => nrf_sdc::mpsl::HighPrioInterruptHandler;
});

/// Ship config: the phone is the only peer.
const CONNECTIONS_MAX: usize = 1;
/// L2CAP signal + ATT; the data-plane CoC (A5) adds one more later.
const L2CAP_CHANNELS_MAX: usize = 2;
/// Outgoing/incoming LL buffers per link (the TrouBLE nrf54 example's values).
const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;

/// SDC memory block, sized to `Builder::required_memory()` for this exact config — measured on
/// glass 2026-07-02 (logged at boot; the SDC warns if the block is bigger than needed and
/// errors if smaller). Re-measure after any Builder/buffer_cfg change.
const SDC_MEM_SIZE: usize = 4704;

/// TrouBLE's host arena for this config (connection state + the DefaultPacketPool at MTU 251).
type Resources =
    HostResources<nrf_sdc::SoftdeviceController<'static>, DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX>;

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

// ============================ Identity (S0 §2 / §3.1) ============================

/// `FICR.INFO.DEVICEID[0]` (nRF54L15: FICR `0x00FF_C000` + INFO `0x300` + DEVICEID `0x04`) — the
/// low word of the 64-bit factory device id. Read raw: embassy-nrf's `pac` re-export is
/// `pub(crate)` without its `unstable-pac` feature, and one always-readable FICR word doesn't
/// justify enabling that. The full 16-hex-digit serial (S0 §3.1) is A4's DIS characteristic.
const FICR_INFO_DEVICEID0: *const u32 = 0x00FF_C304 as *const u32;
/// `FICR.INFO.DEVICEID[1]` — the high word (the address derivation below uses both).
const FICR_INFO_DEVICEID1: *const u32 = 0x00FF_C308 as *const u32;

/// The factory advertising name (S0 §2): `OBC-XXXX`, the last four uppercase hex digits of the
/// serial number — i.e. the low 16 bits of `DEVICEID[0]`, the tail of the serial's hex string.
/// (The user-facing rename lives in the Config object at A6; factory name until then.)
pub fn device_name() -> heapless::String<8> {
    let id = unsafe { FICR_INFO_DEVICEID0.read_volatile() };
    let mut s = heapless::String::new();
    let _ = core::fmt::write(&mut s, format_args!("OBC-{:04X}", id & 0xFFFF));
    s
}

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
/// lands on glass. The A8 passkey joins this screen later.
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

    draw_text(&mut dev, name.as_str(), Point::new(cx, 28), Font::Display, TextAlign::Center, ink);
    let state = match s.state {
        LinkState::Init => "starting",
        LinkState::Advertising => "advertising",
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

    // The detail rows: one label-value line each, Body font, fixed left edge.
    let x = 20;
    let mut y = 160;
    let mut row = |dev: &mut obc_platform::FbDevice64<'_>, text: &str| {
        draw_text(dev, text, Point::new(x, y), Font::Body, TextAlign::Left, ink);
        y += 36;
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

// GATT server: a stub battery service so a discovery walk has real attributes until A4 lands
// the real control plane (DIS + BAS + OBC Control). The fixed 75 matches `StubFuelGauge`.
#[gatt_server]
struct Server {
    battery_service: BatteryService,
}

#[gatt_service(uuid = service::BATTERY)]
struct BatteryService {
    #[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 75)]
    level: u8,
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
) -> ! {
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
    let name = device_name();
    info!("ble: host up as '{}', address {:?}", name.as_str(), address);

    let stack = trouble_host::new(sdc, resources).set_random_address(address).build();
    let runner = stack.runner();
    let mut peripheral = stack.peripheral();

    let server = unwrap!(Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: name.as_str(),
        appearance: &appearance::cycling::CYCLING_COMPUTER,
    })));

    // The lifecycle loop (A3): advertise → serve → re-advertise, forever, with no terminal state.
    join(host_task(runner), async {
        loop {
            publish(|s| {
                s.state = LinkState::Advertising;
                s.peer = None;
                s.conn_interval_ms = 0;
                s.att_mtu = 0;
                s.phy_2m = false;
            });
            let conn = match advertise_lifecycle(name.as_str(), &mut peripheral, &server).await {
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

            // Serve the link until the peer drops it, negotiating the S0 §3.4 parameters
            // concurrently: `serve_connection` pumps GATT + connection events (so the phone's own
            // MTU/PHY/DLE moves are serviced) while `negotiate_link` issues our requests. `join`
            // returns when `serve_connection` does — on disconnect — by which point `negotiate_link`
            // has long finished. Any disconnect reason drops straight back to advertising.
            let (reason, ()) = join(serve_connection(&server, &conn), negotiate_link(&stack, &conn)).await;
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
/// (HCI status code); publishes the link edges the status UI shows (conn interval, PHY) and logs
/// the rest — including every disconnect reason, named + numeric — for the `A9` soak's RTT trail.
async fn serve_connection<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) -> u8 {
    let level = server.battery_service.level;
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => break reason,
            GattConnectionEvent::Gatt { event } => {
                match &event {
                    GattEvent::Read(e) if e.handle() == level.handle => {
                        info!("ble: [gatt] read battery level -> {:?}", conn.get(&level));
                    }
                    GattEvent::Read(e) => info!("ble: [gatt] read handle {}", e.handle()),
                    GattEvent::Write(e) => info!("ble: [gatt] write handle {}", e.handle()),
                    _ => info!("ble: [gatt] other event"),
                }
                match event.accept() {
                    Ok(reply) => reply.send().await,
                    Err(e) => warn!("ble: [gatt] error sending response: {:?}", e),
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
            _ => {}
        }
    };
    info!("ble: [conn] disconnected, reason 0x{:02X} ({:?})", reason.into_inner(), reason);
    reason.into_inner()
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
/// slow one starts. Legacy connectable adv, S0 §2 shaped: AD Flags + the complete local name
/// (`OBC-XXXX`). The 128-bit OBC Control service UUID joins the payload at A4 (the service doesn't
/// exist yet).
async fn advertise_lifecycle<'values, 'server, C: Controller>(
    name: &'values str,
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv_data = [0u8; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut adv_data[..],
    )?;
    let adv_data = &adv_data[..len];
    let adv = || Advertisement::ConnectableScannableUndirected { adv_data, scan_data: &[] };

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
