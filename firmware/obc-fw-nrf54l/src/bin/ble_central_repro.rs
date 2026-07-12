//! Minimal repro for the `SoftdeviceController: 50:701` assert on a central connect (epic #707).
//!
//! The full firmware faults the SDC the instant its initiator hears the saved HR strap and fires
//! the connect — with or without the advertiser running, with or without a stored bond. This bin
//! is the isolation harness (the `ble_spike` precedent, resurrected): **MPSL + SDC + trouble-host
//! only** — no FLPR, no SD, no ride loop, no interrupt executor — driving the exact ship
//! configuration:
//!
//! - the ship SDC builder (adv + peripheral + central + scan, DLE + PHY-update both roles, 2M PHY,
//!   1 peripheral + 1 central link, 251-byte buffers),
//! - the ship connect parameters (60/30 ms initiator scan, 250–500 ms interval, 30 ms CE, 5 s
//!   supervision).
//!
//! Flow: active-scan until an advertiser carrying the HR service (0x180D) appears → connect →
//! discover 0x180D/0x2A37 → subscribe → log bpm. Turn the strap on once "scanning…" shows.
//!
//!     cargo run --release --no-default-features --features ble --bin ble_central_repro
//!
//! If this faults at the connect: the bug is in the nrf-sdc/SDC/trouble stack itself → file
//! upstream with this file as the repro. If it does NOT fault: the trigger is something in the
//! full firmware's environment (executors, FLPR, SD, GRTC use) → bisect from here.
//!
//! [`ADVERTISE_TOO`] flips the ship-shaped concurrency back on: run once with `false` (pure
//! central — the cleanest upstream repro if it faults), once with `true` (advertiser beside the
//! initiator, the shipping topology).
#![no_std]
#![no_main]

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_futures::select::select;
use embassy_nrf::mode::Blocking;
use embassy_nrf::{bind_interrupts, config, cracen};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use static_cell::StaticCell;
use trouble_host::prelude::*;
use {defmt_rtt as _, panic_probe as _};

/// Run the legacy advertiser beside the initiator (the shipping topology). `false` = pure
/// central-only — the cleanest possible repro if the fault still fires.
///
/// **On-glass result 2026-07-12 (`false`)**: still faults — `SoftdeviceController: 50:701` ~200 ms
/// after `LeCreateConn`, with nothing but the initiator on the radio and no bond/resolving-list
/// state. The legacy central connect is broken at the stack level on this DK.
const ADVERTISE_TOO: bool = false;

/// Use `LeExtCreateConn` (`Central::connect_ext`) instead of the legacy `LeCreateConn`. Nordic's
/// own SDC central coverage runs through Zephyr, which issues the **extended** command on the SDC
/// (it detects ext-command support) — so the legacy initiator path this firmware used is the
/// undertested one. An extended initiator on 1M connects to legacy advertisers (the strap) fine.
/// If this works where `false` faults, the ship fix is `connect_ext` + `support_ext_central()`.
const EXT_CONNECT: bool = true;

bind_interrupts!(struct Irqs {
    SWI00 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO_0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER10 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    GRTC_3 => nrf_sdc::mpsl::HighPrioInterruptHandler;
});

// The ship dimensions (`src/ble/mod.rs`): 1 phone + 1 sensor link, one adv set, one bond slot,
// the phone's 3 L2CAP channels + 2 for the sensor link.
const CONNECTIONS_MAX: usize = 2;
const L2CAP_CHANNELS_MAX: usize = 5;
const ADV_SETS_MAX: usize = 1;
const BONDS_MAX: usize = 1;
const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;
const SDC_MEM_SIZE: usize = 8704;

/// First HR-service advertiser seen by the scan (address, is-random) — the connect target.
static FOUND: Signal<CriticalSectionRawMutex, ([u8; 6], bool)> = Signal::new();

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

/// The exact ship SDC config (`src/ble/mod.rs::build_sdc`).
fn build_sdc<'d, const N: usize>(
    p: nrf_sdc::Peripherals<'d>,
    rng: &'d mut cracen::Cracen<'static, Blocking>,
    mpsl: &'d MultiprotocolServiceLayer,
    mem: &'d mut sdc::Mem<N>,
) -> Result<nrf_sdc::SoftdeviceController<'d>, nrf_sdc::Error> {
    sdc::Builder::new()?
        .support_adv()
        .support_peripheral()
        .support_central()
        .support_scan()
        .support_dle_central()
        .support_dle_peripheral()
        .support_le_2m_phy()
        .support_phy_update_central()
        .support_phy_update_peripheral()
        // The extended initiator ([`EXT_CONNECT`]) — present in both modes so the SDC image is
        // identical and only the HCI command under test differs.
        .support_ext_central()
        .peripheral_count(1)?
        .central_count(1)?
        .buffer_cfg(DefaultPacketPool::MTU as u16, DefaultPacketPool::MTU as u16, L2CAP_TXQ, L2CAP_RXQ)?
        .build(p, rng, mpsl, mem)
}

/// Scan-report tap: classify each advertisement (via the same `obc_ble` classifier the firmware
/// uses) and signal the first HR-service advertiser.
struct HrScanHandler;

impl EventHandler for HrScanHandler {
    fn on_adv_reports(&self, reports: bt_hci::param::LeAdvReportsIter) {
        for report in reports {
            let Ok(report) = report else { continue };
            let Some(m) = obc_ble::classify_advertisement(report.data) else { continue };
            if matches!(m.kind, obc_ble::SensorKind::HeartRate) {
                let mut addr = [0u8; 6];
                addr.copy_from_slice(report.addr.raw());
                FOUND.signal((addr, report.addr_kind.as_raw() & 1 == 1));
            }
        }
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config: config::Config = Default::default();
    // The ship clocking (see `src/ble/mod.rs`): HF crystal (MPSL hard requirement), LFCLK =
    // internal RC with MPSL calibration (the DK's LFXO runs off-frequency — unprogrammed INTCAPs).
    config.clock_speed = config::ClockSpeed::CK128;
    config.hfclk_source = config::HfclkSource::ExternalXtal;
    config.lfclk_source = config::LfclkSource::InternalRC;
    let p = embassy_nrf::init(config);

    info!("ble_central_repro: MPSL init (ADVERTISE_TOO = {})", ADVERTISE_TOO);
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
    let lfclk_cfg = mpsl::raw::mpsl_clock_lfclk_cfg_t {
        source: mpsl::raw::MPSL_CLOCK_LF_SRC_RC as u8,
        rc_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_CTIV as u8,
        rc_temp_ctiv: mpsl::raw::MPSL_RECOMMENDED_RC_TEMP_CTIV as u8,
        accuracy_ppm: 500,
        skip_wait_lfclk_started: false,
    };
    static MPSL: StaticCell<MultiprotocolServiceLayer> = StaticCell::new();
    let mpsl = MPSL.init(unwrap!(mpsl::MultiprotocolServiceLayer::new(mpsl_p, Irqs, lfclk_cfg)));
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

    static RNG: StaticCell<cracen::Cracen<'static, Blocking>> = StaticCell::new();
    let rng = RNG.init(cracen::Cracen::new_blocking(p.CRACEN));

    static SDC_MEM: StaticCell<sdc::Mem<SDC_MEM_SIZE>> = StaticCell::new();
    let sdc = unwrap!(build_sdc(sdc_p, rng, mpsl, SDC_MEM.init(sdc::Mem::new())));
    info!("ble_central_repro: SDC up, starting host");

    static RESOURCES: StaticCell<
        HostResources<
            nrf_sdc::SoftdeviceController<'static>,
            DefaultPacketPool,
            CONNECTIONS_MAX,
            L2CAP_CHANNELS_MAX,
            ADV_SETS_MAX,
            BONDS_MAX,
        >,
    > = StaticCell::new();
    let address: Address = Address::random([0x0b, 0xc0, 0x51, 0x14, 0xe5, 0xff]);
    let stack = trouble_host::new(sdc, RESOURCES.init(HostResources::new())).set_random_address(address).build();
    let runner = stack.runner();

    join3(host_task(runner), advertise_task(&stack), central_task(&stack)).await;
}

/// The host's transport pump, with the scan-report tap attached.
async fn host_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
    loop {
        if let Err(e) = runner.run_with_handler(&HrScanHandler).await {
            let e = defmt::Debug2Format(&e);
            defmt::panic!("[host] error: {:?}", e);
        }
    }
}

/// The shipping topology's other half: a legacy connectable advertiser running the whole time
/// ([`ADVERTISE_TOO`]). Nobody needs to connect to it — its radio activity is the variable.
async fn advertise_task<C: Controller>(stack: &Stack<'_, C, DefaultPacketPool>) {
    if !ADVERTISE_TOO {
        core::future::pending::<()>().await;
    }
    let mut peripheral = stack.peripheral();
    let mut adv_data = [0; 31];
    let len = unwrap!(AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(b"OBC-REPRO"),
        ],
        &mut adv_data[..],
    ));
    let _advertiser = unwrap!(
        peripheral
            .advertise(
                &Default::default(),
                Advertisement::ConnectableScannableUndirected { adv_data: &adv_data[..len], scan_data: &[] },
            )
            .await
    );
    info!("[adv] advertising as 'OBC-REPRO' (held open, never accepted)");
    core::future::pending::<()>().await;
}

/// The repro proper: scan for the strap, connect with the ship parameters, subscribe, stream HR.
/// Concrete controller type (the ship manager's `SensorStack` shape) — no trait-bound zoo.
async fn central_task(stack: &Stack<'_, nrf_sdc::SoftdeviceController<'_>, DefaultPacketPool>) {
    // Give the host runner its first polls before the first command (the ship manager's 1 s hold).
    Timer::after_secs(1).await;

    // Active scan (the Sensors-screen shape) until an HR-service advertiser shows.
    info!("[central] scanning for an HR sensor — turn the strap on now");
    let (addr, random) = {
        let mut scanner = Scanner::new(stack.central());
        let config = ScanConfig {
            active: true,
            interval: Duration::from_millis(60),
            window: Duration::from_millis(30),
            ..Default::default()
        };
        let _session = unwrap!(scanner.scan(&config).await);
        FOUND.wait().await
        // `_session` drops → scan off.
    };
    info!("[central] found HR sensor {:02x} (random={}) — connecting with ship params", addr, random);
    Timer::after_millis(200).await;

    // The exact ship connect (`src/ble/sensors.rs::run_link`).
    let mut central = stack.central();
    let filter = [Address::new(if random { AddrKind::RANDOM } else { AddrKind::PUBLIC }, BdAddr::new(addr))];
    let config = ConnectConfig {
        scan_config: ScanConfig {
            active: true,
            filter_accept_list: &filter,
            interval: Duration::from_millis(60),
            window: Duration::from_millis(30),
            ..Default::default()
        },
        connect_params: RequestedConnParams {
            min_connection_interval: Duration::from_millis(250),
            max_connection_interval: Duration::from_millis(500),
            max_latency: 0,
            min_event_length: Duration::from_micros(0),
            max_event_length: Duration::from_millis(30),
            supervision_timeout: Duration::from_millis(5000),
        },
    };
    // ← the legacy path (`EXT_CONNECT = false`) faults ('SoftdeviceController: 50:701') in here.
    let conn = if EXT_CONNECT {
        info!("[central] using LeExtCreateConn (extended initiator)");
        unwrap!(central.connect_ext(&config).await)
    } else {
        info!("[central] using LeCreateConn (legacy initiator)");
        unwrap!(central.connect(&config).await)
    };
    info!("[central] CONNECTED — no fault. Discovering HR service…");

    let client = unwrap!(GattClient::<_, _, 4>::new(stack, &conn).await);
    select(client.task(), async {
        let services = unwrap!(client.services_by_uuid(&Uuid::new_short(obc_ble::UUID_HEART_RATE_SERVICE)).await);
        let service = unwrap!(services.first()).clone();
        let hr = unwrap!(
            client.characteristic_by_uuid::<[u8]>(&service, &Uuid::new_short(obc_ble::UUID_HR_MEASUREMENT)).await
        );
        let mut listener = unwrap!(client.subscribe(&hr, false).await);
        info!("[central] subscribed — streaming HR");
        loop {
            let n = listener.next().await;
            if let Some(s) = obc_ble::parse_hr_measurement(n.as_ref()) {
                info!("[central] HR {} bpm", s.bpm);
            }
        }
    })
    .await;
    warn!("[central] link dropped — repro over (reset to rerun)");
    core::future::pending::<()>().await;
}
