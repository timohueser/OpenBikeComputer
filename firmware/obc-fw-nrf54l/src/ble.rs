//! The BLE stack (issue #270, epic #267): nrf-mpsl (Nordic's Multiprotocol Service Layer) +
//! nrf-sdc (the SoftDevice Controller, LL only) + trouble-host (the Rust BLE host), folded into
//! the real firmware behind the `ble` feature. The A1 spike (#269) proved this exact dependency
//! trio + config on the DK; this module is its production home — the throwaway `ble_spike` bin
//! is retired.
//!
//! A2's scope is deliberately small: **advertise (S0 §2 name) + hold a connection + publish link
//! status** for the text status UI in `main.rs` (`run_status`). The GATT surface is a stub battery
//! service so a phone's discovery walk has real attributes — the real control plane (DIS + BAS +
//! OBC Control) is A4, the CoC data plane A5, pairing/bonding A8.
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
use embassy_nrf::mode::Blocking;
use embassy_nrf::{bind_interrupts, cracen, peripherals, Peri};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
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
    /// Lifetime counters — the soak's at-a-glance health line.
    pub connects: u32,
    pub disconnects: u32,
}

impl Status {
    const INIT: Status =
        Status { state: LinkState::Init, peer: None, conn_interval_ms: 0, connects: 0, disconnects: 0 };
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
    let mut line: heapless::String<20> = heapless::String::new();
    let _ = core::fmt::write(&mut line, format_args!("int  {} ms", s.conn_interval_ms));
    if s.conn_interval_ms > 0 {
        row(&mut dev, line.as_str());
    }
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("batt {}%", battery_pct));
    row(&mut dev, line.as_str());
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("sd   {}", if sd_ok { "ok" } else { "--" }));
    row(&mut dev, line.as_str());
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("link {} / {}", s.connects, s.disconnects));
    row(&mut dev, line.as_str());
    line.clear();
    let _ = core::fmt::write(&mut line, format_args!("in   {}", inputs));
    row(&mut dev, line.as_str());
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

    join(host_task(runner), async {
        loop {
            publish(|s| {
                s.state = LinkState::Advertising;
                s.peer = None;
                s.conn_interval_ms = 0;
            });
            match advertise(name.as_str(), &mut peripheral, &server).await {
                Ok(conn) => {
                    let peer = conn.raw().peer_address();
                    let mut peer_bytes = [0u8; 6];
                    peer_bytes.copy_from_slice(peer.addr.raw());
                    publish(|s| {
                        s.state = LinkState::Connected;
                        s.peer = Some(peer_bytes);
                        s.connects += 1;
                    });
                    gatt_events(&server, &conn).await;
                    publish(|s| s.disconnects += 1);
                }
                Err(e) => {
                    // "Always just works" (S0 §2): an advertise error must not take the firmware
                    // down — log it and retry after a beat.
                    warn!("ble: advertise error: {:?} — retrying in 1 s", defmt::Debug2Format(&e));
                    Timer::after_secs(1).await;
                }
            }
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

/// Serve GATT + connection events until the central drops the link, publishing the edges the
/// status UI shows (conn params) and logging the rest (the soak's RTT trail).
async fn gatt_events<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) {
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
            _ => {}
        }
    };
    info!("ble: [gatt] disconnected: {:?}", reason);
}

/// Legacy connectable adv, S0 §2 shaped: AD Flags + the complete local name (`OBC-XXXX`). The
/// 128-bit OBC Control service UUID joins the payload at A4 (the service doesn't exist yet);
/// fast/slow interval switching is A3's lifecycle work — default intervals until then.
async fn advertise<'values, 'server, C: Controller>(
    name: &'values str,
    peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
    server: &'server Server<'values>,
) -> Result<GattConnection<'values, 'server, DefaultPacketPool>, BleHostError<C::Error>> {
    let mut adv_data = [0; 31];
    let len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(name.as_bytes()),
        ],
        &mut adv_data[..],
    )?;
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected { adv_data: &adv_data[..len], scan_data: &[] },
        )
        .await?;
    info!("ble: advertising as '{}'", name);
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    info!("ble: connection established");
    Ok(conn)
}
