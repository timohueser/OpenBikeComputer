//! BLE controller spike (issue #269, epic #267) — **throwaway**, retired when A2 (#270) folds
//! the stack into `main.rs` (the `ls021_bringup` precedent).
//!
//! Proves the Rust BLE stack on our hardware with our dependency pins: `nrf-mpsl` (Nordic's
//! Multiprotocol Service Layer) + `nrf-sdc` (the closed-source SoftDevice Controller, LL only)
//! + `trouble-host` (the Rust BLE host) on the nRF54L15-DK. Advertises as `OBC-SPIKE` with a
//! stub battery service so a phone's service discovery has something to walk; logs the whole
//! event flow over defmt/RTT for the ≥10 min soak.
//!
//!     cargo run --release --no-default-features --features ble-spike --bin ble_spike
//!
//! (`--no-default-features` swaps the critical-section impl from `cortex-m`'s global-irq-disable
//! one to MPSL's — mandatory with the radio running; see `cs-single-core` in Cargo.toml.)
//!
//! ## What A2 needs from this file (the spike's real deliverables)
//!
//! **Interrupt inventory** — everything MPSL/SDC claim on the nRF54L, vs. what `main.rs` uses:
//!
//! | vector        | owner (this bin)                  | prio          | `main.rs` conflict?        |
//! |---------------|-----------------------------------|---------------|----------------------------|
//! | `RADIO_0`     | MPSL high-prio (timing-critical)  | P0 (highest)  | free                       |
//! | `TIMER10`     | MPSL high-prio                    | P0            | free                       |
//! | `GRTC_3`      | MPSL high-prio (its GRTC IRQ lane)| P0            | embassy time driver is on GRTC_0 — separate vector, no clash |
//! | `CLOCK_POWER` | MPSL clock handler                | default (P1)  | free                       |
//! | `SWI00`       | MPSL **low-prio scheduling**      | default (P1)  | **CLASH — `main.rs` runs the input-plane `InterruptExecutor` on SWI00@P3.** A2 must move one of them (SWI01 is free). |
//!
//! **Peripheral inventory** — owned outright by MPSL/SDC (`mpsl::Peripherals` / `sdc::Peripherals`):
//! GRTC channels **7–11** (embassy's GRTC time driver allocates from CH0 up — no overlap, but the
//! budget shrinks), `TIMER10`, `TIMER20`, `TEMP`, `PPI10_CH0..11`, `PPI00_CH1/3`, `PPI20_CH1`,
//! `PPIB00_CH1..3`, `PPIB10_CH1..3`, `PPIB11_CH0`, `PPIB21_CH0`, and `CRACEN` (the RNG the LL's
//! crypto pulls from). None of these are used by `main.rs` today.
//!
//! **Clocking** — MPSL *requires* the HF crystal (`HfclkSource::ExternalXtal`); `main.rs` currently
//! boots on the internal RC, so that config change folds into `main.rs` at A2. LFCLK = the internal
//! **RC** with MPSL calibration, NOT the 32 k xtal — see the comment at `lfclk_source` below for
//! the on-glass failure (HCI 0x3E on every connect) that forced this. CK128 kept, same as the app.
//!
//! **RAM** — the SDC memory block requirement at the ship config (peripheral-only, 1 conn,
//! ATT MTU 247 / LL 251, DLE, 2M PHY) is logged at boot (`sdc required_memory`); the static block
//! here (`SDC_MEM`), TrouBLE's `HostResources`, and the stack numbers come out of the map file.
#![no_std]
#![no_main]

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::select;
use embassy_nrf::mode::Blocking;
use embassy_nrf::{bind_interrupts, config, cracen};
use embassy_time::Timer;
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use static_cell::StaticCell;
use trouble_host::prelude::*;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    SWI00 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO_0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER10 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    GRTC_3 => nrf_sdc::mpsl::HighPrioInterruptHandler;
});

/// Ship config (mirrors what A2 will run): the phone is the only peer.
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

// GATT server: a stub battery service so nRF Connect's discovery walk has real attributes.
#[gatt_server]
struct Server {
    battery_service: BatteryService,
}

#[gatt_service(uuid = service::BATTERY)]
struct BatteryService {
    #[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 75)]
    level: u8,
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config: config::Config = Default::default();
    // CK128 like main.rs; the HF **crystal** is an MPSL hard requirement (radio timing), the
    // LF crystal is the DK's 32.768 kHz / 50 ppm one — both a delta vs. main.rs's boot config
    // that A2 inherits.
    config.clock_speed = config::ClockSpeed::CK128;
    config.hfclk_source = config::HfclkSource::ExternalXtal;
    // LFCLK = internal RC, MPSL-calibrated (rc_ctiv), NOT the 32k crystal: with the LF xtal
    // selected, every connection died at establishment with reason 0x3E (sync timeout) — the
    // nRF54L needs its *internal load capacitors* programmed (Nordic's DK config: LFXO 15.5 pF,
    // HFXO 15 pF) and neither embassy-nrf 0.11 (nRF5340-only knob) nor nrf-mpsl does that, so
    // the LFXO runs off-frequency and the peripheral misses every anchor point. RC + periodic
    // calibration is solid (500 ppm class); moving back to the xtal once the caps are
    // programmable is an A2 follow-up. See issue #269.
    config.lfclk_source = config::LfclkSource::InternalRC;
    let p = embassy_nrf::init(config);

    info!("ble_spike: MPSL init");
    let mpsl_p = mpsl::Peripherals::new(
        p.GRTC_CH7,
        p.GRTC_CH8,
        p.GRTC_CH9,
        p.GRTC_CH10,
        p.GRTC_CH11,
        p.TIMER10,
        p.TIMER20,
        p.TEMP,
        p.PPI10_CH0,
        p.PPI20_CH1,
        p.PPIB11_CH0,
        p.PPIB21_CH0,
    );
    // RC source at Nordic's recommended calibration cadence (calibrate every 16×0.25 s = 4 s;
    // temp-check every 2 intervals) — guarantees the ±500 ppm class the accuracy field claims.
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: 500,
        skip_wait_lfclk_started: false,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk_cfg)));
    // embassy-executor 0.10: the task macro returns a Result'd spawn token.
    spawner.spawn(unwrap!(mpsl_task(&*mpsl)));

    let sdc_p = sdc::Peripherals::new(
        p.PPI00_CH1,
        p.PPI00_CH3,
        p.PPI10_CH1,
        p.PPI10_CH2,
        p.PPI10_CH3,
        p.PPI10_CH4,
        p.PPI10_CH5,
        p.PPI10_CH6,
        p.PPI10_CH7,
        p.PPI10_CH8,
        p.PPI10_CH9,
        p.PPI10_CH10,
        p.PPI10_CH11,
        p.PPIB00_CH1,
        p.PPIB00_CH2,
        p.PPIB00_CH3,
        p.PPIB10_CH1,
        p.PPIB10_CH2,
        p.PPIB10_CH3,
    );

    // The LL pulls its crypto randomness from CRACEN (the nRF54L has no legacy RNG peripheral).
    // Static because the SDC handle (and thus this borrow) must be `'static` for the
    // `HostResources` static below.
    static RNG: StaticCell<cracen::Cracen<'static, Blocking>> = StaticCell::new();
    let rng = RNG.init(cracen::Cracen::new_blocking(p.CRACEN));

    // Log the exact SDC memory requirement for this config — the number A2's RAM budget uses.
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
        Ok(required) => info!("sdc required_memory = {} bytes (SDC_MEM_SIZE = {})", required, SDC_MEM_SIZE),
        Err(e) => warn!("sdc required_memory failed: {:?}", e),
    }

    static SDC_MEM: StaticCell<sdc::Mem<SDC_MEM_SIZE>> = StaticCell::new();
    let sdc = unwrap!(build_sdc(sdc_p, rng, mpsl, SDC_MEM.init(sdc::Mem::new())));
    info!("ble_spike: SDC up, starting host");

    // Static so the block shows up named in the map file (the RAM-table measurement).
    static RESOURCES: StaticCell<
        HostResources<nrf_sdc::SoftdeviceController<'static>, DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX>,
    > = StaticCell::new();
    // Fixed random address — fine for the spike; A8 owns real identity/bonding.
    let address: Address = Address::random([0x0b, 0xc0, 0x51, 0x14, 0xe5, 0xff]);
    info!("ble_spike: address = {:?}", address);

    let stack = trouble_host::new(sdc, RESOURCES.init(HostResources::new())).set_random_address(address).build();
    let runner = stack.runner();
    let mut peripheral = stack.peripheral();

    let server = unwrap!(Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "OBC-SPIKE",
        appearance: &appearance::cycling::CYCLING_COMPUTER,
    })));

    join(ble_task(runner), async {
        loop {
            match advertise("OBC-SPIKE", &mut peripheral, &server).await {
                Ok(conn) => {
                    // Both end when the central drops the link; then we re-advertise.
                    select(gatt_events_task(&server, &conn), soak_task(&server, &conn, &stack)).await;
                }
                Err(e) => {
                    let e = defmt::Debug2Format(&e);
                    defmt::panic!("[adv] error: {:?}", e);
                }
            }
        }
    })
    .await;
}

/// The host's transport pump — must run forever alongside everything else.
async fn ble_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    loop {
        if let Err(e) = runner.run().await {
            let e = defmt::Debug2Format(&e);
            defmt::panic!("[ble_task] error: {:?}", e);
        }
    }
}

/// Log every GATT event until the connection closes (the "event flow" the soak watches).
async fn gatt_events_task<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) -> Result<(), Error> {
    let level = server.battery_service.level;
    let reason = loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => break reason,
            GattConnectionEvent::Gatt { event } => {
                match &event {
                    GattEvent::Read(e) if e.handle() == level.handle => {
                        info!("[gatt] read battery level -> {:?}", conn.get(&level));
                    }
                    GattEvent::Read(e) => info!("[gatt] read handle {}", e.handle()),
                    GattEvent::Write(e) => info!("[gatt] write handle {}", e.handle()),
                    _ => info!("[gatt] other event"),
                }
                match event.accept() {
                    Ok(reply) => reply.send().await,
                    Err(e) => warn!("[gatt] error sending response: {:?}", e),
                }
            }
            GattConnectionEvent::PhyUpdated { tx_phy, rx_phy } => {
                info!("[conn] PHY updated: tx {:?} rx {:?}", tx_phy, rx_phy);
            }
            GattConnectionEvent::ConnectionParamsUpdated { conn_interval, peripheral_latency, supervision_timeout } => {
                info!(
                    "[conn] params: interval {} ms latency {} timeout {} ms",
                    conn_interval.as_millis(),
                    peripheral_latency,
                    supervision_timeout.as_millis()
                );
            }
            _ => {}
        }
    };
    info!("[gatt] disconnected: {:?}", reason);
    Ok(())
}

/// Legacy connectable adv with the name in the adv payload (what nRF Connect lists).
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
    info!("[adv] advertising as '{}'", name);
    let conn = advertiser.accept().await?.with_attribute_server(server)?;
    info!("[adv] connection established");
    Ok(conn)
}

/// Soak heartbeat while connected: notify a sweeping battery level + read RSSI every 10 s,
/// so the RTT log shows continuous two-way traffic for the ≥10 min connected soak.
async fn soak_task<C: Controller, P: PacketPool>(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, P>,
    stack: &Stack<'_, C, P>,
) {
    let level = server.battery_service.level;
    let mut tick: u8 = 0;
    loop {
        tick = tick.wrapping_add(1);
        let value = 100 - (tick % 100);
        if level.notify(conn, &value, true).await.is_err() {
            warn!("[soak] notify failed (not subscribed yet, or link gone)");
        }
        match conn.raw().rssi(stack).await {
            Ok(rssi) => info!("[soak] tick {} battery {} RSSI {}", tick, value, rssi),
            Err(_) => {
                info!("[soak] RSSI read failed — link closing");
                break;
            }
        }
        Timer::after_secs(10).await;
    }
}
