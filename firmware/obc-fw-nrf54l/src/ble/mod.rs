//! The BLE stack: nrf-mpsl (Nordic's Multiprotocol Service Layer) + nrf-sdc (the SoftDevice
//! Controller, LL only) + trouble-host (the Rust BLE host), folded into the real firmware behind the
//! `ble` feature.
//!
//! ## Module map
//!
//! This file is the stack bring-up + the advertise → connect → re-advertise orchestration ([`run`]);
//! the four jobs the link does are split into focused submodules:
//!
//! - [`gatt`] — the `#[gatt_server]`/`#[gatt_service]` tables, DIS/BAS seeding, FICR
//!   serial/address identity, the Config codec, and the on-glass status screen.
//! - [`lifecycle`] — advertise (fast/slow policy), parameter negotiation, the conn-param sets.
//! - [`control`] — the per-connection GATT event pump: command/config/transfer writes + pairing.
//! - [`data_plane`] — the L2CAP CoC bulk-transfer plane (echo, route upload, download, ride).
//! - [`state`] — the shared link-status snapshot, BAS cell, and the one-transfer arming channel.
//!
//! ## Interrupts / priorities
//!
//! MPSL claims `RADIO_0` / `TIMER10` / `GRTC_3` at **P0** (timing-critical), `CLOCK_POWER` and its
//! low-prio scheduling on **`SWI00`** at default priority. `SWI00` is why `main.rs`'s high-priority
//! `InterruptExecutor` lives on **`SWI01`** (every build). The full ladder is documented in `main.rs`'s
//! module doc.
//!
//! ## RAM
//!
//! Everything resident is a named static below (the `MaybeUninit`-in-`.bss` pattern — see
//! [`crate::init_static`]). [`RESIDENT_BYTES`] sums them for the budget assert in `main.rs`, so the BLE
//! build's fit on the 256 KB DK is enforced at compile time like the map build's.
//!
//! ## Clocking
//!
//! MPSL requires the HF **crystal**; LFCLK runs the internal **RC** with MPSL calibration, *not* the
//! 32 k crystal: the nRF54L's XO internal load caps are never programmed by embassy-nrf 0.11 or
//! nrf-mpsl, so the LFXO runs off-frequency and every connection dies at establishment with HCI 0x3E
//! (advertising works — the failure needs a sync anchor). `main.rs` sets both knobs in its `ble`-build
//! boot config.

mod control;
mod data_plane;
mod gatt;
mod lifecycle;
mod state;

// The ride loop publishes its stack high-water mark here (#277/A9) so the diagnostics blob can post it
// over the link; the map plane owns the stackmeter, this is the one value that crosses into the BLE
// module tree.
pub use state::publish_stack_high_water;

// The app-facing link snapshot (epic #447): the ride loop feeds it into `App::set_ble_status` each
// pass. The only BLE state that crosses into the app seam, already distilled to `obc_app` vocabulary.
pub use state::app_ble_status;

use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::{join, join3};
use embassy_futures::select::{select, Either};
use embassy_nrf::mode::Blocking;
use embassy_nrf::{bind_interrupts, cracen, peripherals, Peri};
use embassy_time::Timer;
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use trouble_host::prelude::*;

use crate::init_static;
use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

use control::serve_connection;
use data_plane::{battery_task, serve_coc};
use gatt::{
    advertised_name, config_blob, device_address, firmware_revision, gatt_str, serial_string, Server,
    HARDWARE_REVISION, OBC_PSM,
};
use lifecycle::{advertise_lifecycle, negotiate_link};
use state::{publish, LinkState, TRANSFER_ABORT, TRANSFER_ACTIVE, TRANSFER_ARM};

bind_interrupts!(struct Irqs {
    SWI00 => nrf_sdc::mpsl::LowPrioInterruptHandler;
    CLOCK_POWER => nrf_sdc::mpsl::ClockInterruptHandler;
    RADIO_0 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    TIMER10 => nrf_sdc::mpsl::HighPrioInterruptHandler;
    GRTC_3 => nrf_sdc::mpsl::HighPrioInterruptHandler;
});

/// Ship config: the phone is the only peer.
const CONNECTIONS_MAX: usize = 1;
/// Advertising sets — one legacy connectable set is all the advertise policy needs.
const ADV_SETS_MAX: usize = 1;
/// Bonded peers stored in the host: exactly one. A fresh pairing replaces it, so the resolving list
/// never holds more than the single phone (matches the app's single-peer model).
const BONDS_MAX: usize = 1;
/// L2CAP signal + ATT + the data-plane CoC.
const L2CAP_CHANNELS_MAX: usize = 3;
/// Outgoing/incoming LL buffers per link (the TrouBLE nrf54 example's values).
const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;

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

/// The BLE build's resident statics, summed for the budget assert in `main.rs` (the `ble` analogue of
/// the map build's App/cache terms): the MPSL handle, the SDC memory block, TrouBLE's
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

/// Bring the whole stack up and run it forever: MPSL (spawned — it must outlive everything),
/// SDC, the TrouBLE host, then the S0 advertise → connect → re-advertise loop, publishing every
/// link edge for the status UI. Joined against `run_status` on the thread-mode executor in
/// `main.rs`. The MPSL/SDC peripheral sets are built in `main` (where the `Peripherals` struct
/// is split) and handed in whole.
/// An **embassy task**, not a plain future: its ~36 KB state machine must live in this task's
/// `.bss` pool static, built **in place** by the spawn. As a `join`-ed local future in `main` it
/// was a giant stack temporary inside `main`'s poll frame — which overflowed the combined build's
/// ~36 KB stack straight into `.bss` before the first frame rendered (#270; caught by a DWT
/// watchpoint — the corrupted task pool panicked "Busy" at the com_task spawn).
#[embassy_executor::task]
pub async fn run(
    spawner: Spawner,
    mpsl_p: mpsl::Peripherals<'static>,
    sdc_p: sdc::Peripherals<'static>,
    cracen_p: Peri<'static, peripherals::CRACEN>,
    shared: &'static SharedStoreMutex,
) -> ! {
    // The object store: the catalog/upload/digest semantics behind a RefCell — both BLE planes (GATT
    // control + CoC data) borrow it synchronously, never across an `await`. The SD card + RRAM
    // settings it operates on live in `shared` (the async mutex the ride loop shares), which each
    // plane locks per call and passes into the store method (#270). Built **here**, not in `main`:
    // the ~8 KB value then lives in this task's future (its pool static) and its construction
    // temporary lands on this task's shallow poll frame — in `main` both cost stack the combined
    // build doesn't have (see `spawn_ble_stack`).
    let store = {
        let mut guard = shared.lock().await;
        core::cell::RefCell::new(ObjectStore::new(&mut guard))
    };
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
    // The GAP name is pinned at boot (the attribute borrows it for the server's life); the *advertised*
    // name is re-read from the store each advertise cycle, so a rename lands without a reboot.
    let name = advertised_name(&store.borrow());
    info!("ble: host up as '{}', address {:?}", name.as_str(), address);

    // Register the CoC SPSM up front so `serve_coc` can accept on it once a link is up. IO =
    // DisplayOnly: the device shows a 6-digit passkey, the phone (keyboard) enters it → LESC
    // passkey-entry pairing, MITM-protected. Keep the static-random address (no device privacy) — the
    // phone stores our stable identity for instant reconnect; we resolve *its* rotating RPA from the
    // stored peer IRK below.
    let stack = trouble_host::new(sdc, resources)
        .set_random_address(address)
        .set_io_capabilities(IoCapabilities::DisplayOnly)
        .register_l2cap_spsm(OBC_PSM)
        .build();

    // Re-establish the stored bond: hand it to the host so the controller's resolving list resolves the
    // bonded phone's RPA on reconnect and re-encrypts with the stored LTK — no dialog, no interaction.
    // Absent/torn → open pairing.
    let stored_bond = {
        let mut guard = shared.lock().await;
        store.borrow_mut().load_bond(&mut guard)
    };
    if let Some(bond) = stored_bond {
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

    // The lifecycle loop: advertise → serve → re-advertise, forever, with no terminal state.
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
            // Re-read the advertised name each cycle — a rename (Config write) takes effect on the next
            // advertising start, no reboot. Refresh the config cache from RRAM first (a no-op unless
            // the ride loop flagged an on-device settings change, #456) so the cache stays coherent
            // with the persisted truth across an advertise cycle.
            {
                let mut guard = shared.lock().await;
                store.borrow_mut().refresh_settings_if_changed(&mut guard);
            }
            let adv_name = advertised_name(&store.borrow());
            let conn = match advertise_lifecycle(adv_name.as_str(), &mut peripheral, &server).await {
                Ok(conn) => conn,
                Err(e) => {
                    // An advertise error must not take the firmware down and must not wedge the loop —
                    // log it, wait a beat, and try again.
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

            // Allow this link to bond: the trouble default is *not* bondable, so a passkey pairing
            // wouldn't persist keys without this. Set before the peer starts SMP.
            if let Err(e) = conn.raw().set_bondable(true) {
                warn!("ble: set_bondable failed: {:?}", defmt::Debug2Format(&e));
            }

            // Serve the link until the peer drops it. `serve_connection` pumps GATT + connection
            // events (so the phone's own MTU/PHY/DLE moves are serviced and our control-plane writes
            // are answered) and owns the exit — it returns the disconnect reason. The background set
            // (parameter negotiation, the CoC accept-and-drain, and the BAS battery notify) runs
            // concurrently and never returns, so `select` tears it all down the moment the link drops
            // (any disconnect drops straight back to advertising).
            let reason = match select(
                serve_connection(&stack, &server, &conn, &store, shared),
                join3(
                    negotiate_link(&stack, &conn),
                    serve_coc(&stack, &server, &conn, &store, shared),
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
            {
                let mut guard = shared.lock().await;
                store.borrow_mut().link_reset(&mut guard);
            }
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
