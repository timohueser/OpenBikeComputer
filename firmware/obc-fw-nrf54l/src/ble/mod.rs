//! The BLE stack: nrf-mpsl, nrf-sdc (link layer only) and trouble-host, in every build.
//! This file brings the stack up and runs the advertise, connect, re-advertise loop. The submodules
//! hold the GATT tables, the lifecycle policy, the control plane, the bulk-transfer plane and the
//! shared link state. Radio peripheral and interrupt selection lives in [`crate::board`].
//!
//! Everything resident is a named static below; [`RESIDENT_BYTES`] sums them for the resource report.
//!
//! MPSL needs the HF crystal. LFCLK runs the internal RC with MPSL calibration, not the 32 k crystal:
//! the nRF54L XO internal load caps are never programmed by embassy-nrf or nrf-mpsl, so the LFXO runs
//! off-frequency and every connection dies at establishment with HCI 0x3E. Advertising still works,
//! because the failure needs a sync anchor. [`crate::board::init!`] sets both knobs.

mod control;
mod data_plane;
mod gatt;
mod lifecycle;
mod sensors;
mod state;
mod v4;

pub use state::app_ble_status;

pub use state::wait_status_change;

pub(crate) use state::set_usb_radio_inhibited;
pub use state::{set_radio_enabled, take_bond_outcome, try_forget_bond};

pub use sensors::{
    cancel_scan, request_forget_sensor, request_save_sensor, request_scan, sensor_scan_hits, sensor_slot_status,
    SensorSlotState,
};

use core::mem::MaybeUninit;

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::{join, join4};
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::mode::Blocking;
use embassy_nrf::{cracen, peripherals, Peri};
use embassy_time::Timer;
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use trouble_host::prelude::*;

use crate::init_static;
use crate::link::identity;
use crate::link_control::LinkControl;
use crate::SharedSettingsMutex;

use control::serve_connection;
use data_plane::battery_task;
use gatt::{
    advertised_name, config_blob, device_address, dis_firmware_revision, dis_hardware_revision, dis_serial_number,
    Server, OBC_PSM,
};
use lifecycle::{advertise_lifecycle, negotiate_link};
use state::{publish, LinkState, FORGET_BOND};

/// Concurrent sensor links the central role holds: HR, power and cadence all live at once. Each
/// central link costs about 2.3 KB of SDC memory (see [`SDC_MEM_SIZE`]) plus host arena.
const SENSOR_LINKS: usize = 3;
/// The phone link (peripheral role) plus [`SENSOR_LINKS`] sensor links (central role). One `Stack`
/// runs both roles at the same time.
const CONNECTIONS_MAX: usize = 1 + SENSOR_LINKS;
const ADV_SETS_MAX: usize = 1;
/// Only the phone is bonded. Sensors are open GATT servers connected by stored address, so they
/// use no bond slot. While the phone slot is full, new pairings are rejected.
const BONDS_MAX: usize = 1;
/// The phone link's 3 (signalling, ATT, and the data-plane CoC) plus 2 per sensor link (signalling
/// and the ATT bearer). Sensor links have no CoC.
const L2CAP_CHANNELS_MAX: usize = 3 + 2 * SENSOR_LINKS;
/// Outgoing and incoming LL buffers per link.
const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;

/// The SDC memory block for this exact config. It must be at least `Builder::required_memory()`:
/// the SDC errors out of `build()` if the block is smaller and warns if it is bigger. The value is
/// the `ble: sdc required_memory` line the boot log prints. Re-read that line and re-pin after any
/// change to the builder, the buffer config or [`SENSOR_LINKS`].
pub(crate) const SDC_MEM_SIZE: usize = 11336;

/// TrouBLE's host arena: connection state, the packet pool at MTU 251, and the single-peer bond
/// storage the `security` feature adds.
pub(crate) type Resources = HostResources<
    nrf_sdc::SoftdeviceController<'static>,
    DefaultPacketPool,
    CONNECTIONS_MAX,
    L2CAP_CHANNELS_MAX,
    ADV_SETS_MAX,
    BONDS_MAX,
>;

/// The radio's resident statics, summed into [`RESIDENT_BYTES`] for the resource report. Large
/// values live in `.bss` statics instead of `ble::run` locals, because a value built in the async
/// body also takes a permanent slot in the task's poll frame (see [`run`]).
pub(crate) const MPSL_BYTES: usize = core::mem::size_of::<MultiprotocolServiceLayer<'static>>();
pub(crate) const HOST_RESOURCES_BYTES: usize = core::mem::size_of::<Resources>();
pub(crate) const PACKET_POOL_BYTES: usize = core::mem::size_of::<DefaultPacketPool>();
pub(crate) const CRACEN_BYTES: usize = core::mem::size_of::<cracen::Cracen<'static, Blocking>>();
/// The shared [`LinkControl`] lives in [`crate::link`], because every transport uses the same
/// settings and hand-offs. The report keeps the `ble_object_store` metric key for baseline
/// compatibility.
pub(crate) const OBJECT_STORE_BYTES: usize = crate::link::OBJECT_STORE_BYTES;
pub(crate) const SERVER_BYTES: usize = core::mem::size_of::<Server<'static>>();
pub(crate) const GAP_NAME_BYTES: usize = core::mem::size_of::<heapless::String<48>>();
pub(crate) const SENSOR_MANAGER_BYTES: usize = sensors::RESIDENT_BYTES;
/// The protocol-v4 adapter's record and reaction buffers.
pub(crate) const V4_ADAPTER_BYTES: usize = v4::RESIDENT_BYTES;

pub const RESIDENT_BYTES: usize = MPSL_BYTES
    + SDC_MEM_SIZE
    + HOST_RESOURCES_BYTES
    + PACKET_POOL_BYTES
    + CRACEN_BYTES
    + OBJECT_STORE_BYTES
    + SERVER_BYTES
    + GAP_NAME_BYTES
    + SENSOR_MANAGER_BYTES
    + V4_ADAPTER_BYTES;

// Written exactly once in [`run`], never aliased.
static mut MPSL: MaybeUninit<MultiprotocolServiceLayer<'static>> = MaybeUninit::uninit();
static mut RNG: MaybeUninit<cracen::Cracen<'static, Blocking>> = MaybeUninit::uninit();
static mut SDC_MEM: MaybeUninit<sdc::Mem<SDC_MEM_SIZE>> = MaybeUninit::uninit();
static mut RESOURCES: MaybeUninit<Resources> = MaybeUninit::uninit();
static mut GAP_NAME: MaybeUninit<heapless::String<48>> = MaybeUninit::uninit();
static mut SERVER: MaybeUninit<Server<'static>> = MaybeUninit::uninit();
static mut STACK: MaybeUninit<Stack<'static, nrf_sdc::SoftdeviceController<'static>, DefaultPacketPool>> =
    MaybeUninit::uninit();

/// Build the SDC memory block into `.bss` off the poll frame. `#[inline(never)]` is load-bearing on
/// this and the two init fns below: inlined, LLVM reserves the construction temporary's slot in
/// `run`'s poll frame at entry, on every poll.
/// SAFETY: sole writer of `SDC_MEM`, called once from [`run`].
#[inline(never)]
fn init_sdc_mem() -> &'static mut sdc::Mem<SDC_MEM_SIZE> {
    unsafe { init_static(core::ptr::addr_of_mut!(SDC_MEM), sdc::Mem::new()) }
}

/// Build TrouBLE's host arena into `.bss` ([`RESOURCES`]) off the poll frame (see [`init_sdc_mem`]).
/// SAFETY: sole writer of `RESOURCES`, called once from [`run`].
#[inline(never)]
fn init_resources() -> &'static mut Resources {
    unsafe { init_static(core::ptr::addr_of_mut!(RESOURCES), HostResources::new()) }
}

/// Pin the boot-time GAP name and build the GATT server into `.bss` off the poll frame (see
/// [`init_sdc_mem`]). The server's attribute table borrows the name for its `'static` life. The
/// advertised name is re-read each advertise cycle, so a rename lands without a reboot.
/// SAFETY: sole writer of `GAP_NAME`/`SERVER`, called once from [`run`].
#[inline(never)]
fn init_server(store: &core::cell::RefCell<LinkControl>) -> &'static Server<'static> {
    let name: &'static str =
        unsafe { init_static(core::ptr::addr_of_mut!(GAP_NAME), advertised_name(&store.borrow())) }.as_str();
    unsafe {
        init_static(
            core::ptr::addr_of_mut!(SERVER),
            unwrap!(Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
                name,
                appearance: &appearance::cycling::CYCLING_COMPUTER,
            }))),
        )
    }
}

#[embassy_executor::task]
async fn mpsl_task(mpsl: &'static MultiprotocolServiceLayer<'static>) -> ! {
    mpsl.run().await
}

/// The SDC at the config we ship: one peripheral link (the phone), the central role and scanner for
/// the sensor manager, DLE and PHY update on both roles, and the extended adv/scan/central commands.
/// The extended trio is load-bearing, not an upgrade: the whole host must speak extended, because a
/// legacy initiator faults the controller blob (see [`sensors`]' module doc).
///
/// Keep this in lockstep with the `required_memory` probe in [`run`].
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
        .support_ext_adv()
        .support_ext_scan()
        .support_ext_central()
        .support_dle_central()
        .support_dle_peripheral()
        .support_le_2m_phy()
        .support_phy_update_central()
        .support_phy_update_peripheral()
        .peripheral_count(1)?
        .central_count(SENSOR_LINKS as u8)?
        .buffer_cfg(DefaultPacketPool::MTU as u16, DefaultPacketPool::MTU as u16, L2CAP_TXQ, L2CAP_RXQ)?
        .build(p, rng, mpsl, mem)
}

/// Bring the whole stack up and run it forever: MPSL (spawned, it must outlive everything), SDC,
/// the TrouBLE host, then the advertise, connect, re-advertise loop, publishing every link edge for
/// the status UI.
///
/// An embassy task, not a plain future: its state machine must live in this task's `.bss` pool.
/// Keep the big values out of this body. A value built inline in an async body also gets a
/// construction-temporary slot in the generated poll frame, which LLVM reserves at frame entry on
/// every poll. With the object store, the GATT server, the SDC block and the host arena built
/// inline, the frame was 30,464 B of the ~38 KB stack, and the software-P256 pairing chain ran off
/// the bottom of it. Anything bigger than a few hundred bytes is built into a `.bss` static by a
/// dedicated `#[inline(never)]` init fn; this body holds only `&'static` handles.
#[embassy_executor::task]
pub async fn run(
    spawner: Spawner,
    mpsl_p: mpsl::Peripherals<'static>,
    sdc_p: sdc::Peripherals<'static>,
    cracen_p: Peri<'static, peripherals::CRACEN>,
    // The settings mutex and one shared link-control state. `main` builds the state once, so every
    // transport reads and writes one settings cache.
    state: crate::link::LinkState,
    // The sensor hub's HR/power/cadence injector: the central manager decodes notifications and
    // publishes through it into the same mailboxes the debug-uart path feeds (last writer wins).
    sensor_injector: obc_platform::sensor_hub::SampleInjector<'static>,
) -> ! {
    let crate::link::LinkState { shared, control: store } = state;
    // LFCLK = the internal RC at Nordic's recommended calibration cadence, which holds the ±500 ppm
    // class the accuracy field claims. Not the 32 k crystal — see the module doc.
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
            unwrap!(mpsl::MultiprotocolServiceLayer::new(mpsl_p, crate::board::BleIrqs, lfclk_cfg)),
        )
    };
    spawner.spawn(unwrap!(mpsl_task(mpsl)));

    // The LL pulls its crypto randomness from CRACEN (the nRF54L has no legacy RNG peripheral).
    let rng = unsafe { init_static(core::ptr::addr_of_mut!(RNG), cracen::Cracen::new_blocking(cracen_p)) };

    // Log the exact SDC memory requirement for this config — the number `SDC_MEM_SIZE` pins.
    match sdc::Builder::new().and_then(|b| {
        b.support_adv()
            .support_peripheral()
            .support_central()
            .support_scan()
            .support_ext_adv()
            .support_ext_scan()
            .support_ext_central()
            .support_dle_central()
            .support_dle_peripheral()
            .support_le_2m_phy()
            .support_phy_update_central()
            .support_phy_update_peripheral()
            .peripheral_count(1)?
            .central_count(SENSOR_LINKS as u8)?
            .buffer_cfg(DefaultPacketPool::MTU as u16, DefaultPacketPool::MTU as u16, L2CAP_TXQ, L2CAP_RXQ)?
            .required_memory()
    }) {
        Ok(required) => info!("ble: sdc required_memory = {} bytes (SDC_MEM_SIZE = {})", required, SDC_MEM_SIZE),
        Err(e) => warn!("ble: sdc required_memory failed: {:?}", e),
    }

    let sdc_mem = init_sdc_mem();
    let sdc = unwrap!(build_sdc(sdc_p, rng, mpsl, sdc_mem));

    let resources = init_resources();
    let address = device_address();

    // Register the CoC SPSM up front so `serve_coc` can accept on it once a link is up. IO =
    // DisplayOnly gives LESC passkey-entry pairing with MITM protection: the device shows a 6-digit
    // passkey and the phone enters it. The static random address stays, so the phone reconnects to a
    // stable identity; we resolve its rotating RPA from the stored peer IRK. A static, because the
    // `'static` [`SERVER`]'s advertise path needs a `Peripheral<'static>`.
    // SAFETY: sole writer of `STACK`; `run` is called once from `main`.
    let stack: &'static Stack<'static, nrf_sdc::SoftdeviceController<'static>, DefaultPacketPool> = unsafe {
        init_static(
            core::ptr::addr_of_mut!(STACK),
            trouble_host::new(sdc, resources)
                .set_random_address(address)
                .set_io_capabilities(IoCapabilities::DisplayOnly)
                .register_l2cap_spsm(OBC_PSM)
                .build(),
        )
    };

    // Re-establish the stored bond so the controller's resolving list resolves the bonded phone's
    // RPA on reconnect and re-encrypts with the stored LTK. Absent or torn gives open pairing.
    let stored_bond = {
        let mut guard = shared.lock().await;
        store.borrow_mut().load_bond(&mut guard)
    };
    // The paired flag drives the app's "Paired" row and the reject-when-bonded pairing policy.
    publish(|s| s.paired = stored_bond.is_some());
    if let Some(bond) = stored_bond {
        match stack.add_bond_information(bond) {
            Ok(()) => info!("ble: restored stored bond — bonded reconnect armed"),
            Err(e) => warn!("ble: add_bond_information failed: {:?}", defmt::Debug2Format(&e)),
        }
    }

    // Seed the radio switch from the persisted settings: a device switched off stays off across a
    // reboot. The ride loop re-pushes the live value each pass.
    state::seed_radio_enabled(store.borrow().settings().ble_enabled);
    {}

    let runner = stack.runner();
    let mut peripheral = stack.peripheral();

    let server: &'static Server<'static> = init_server(store);
    info!(
        "ble: host up as '{}', address {:?}",
        advertised_name(&store.borrow()).as_str(),
        defmt::Debug2Format(&address)
    );

    // Seed the runtime attribute values the macro `value =` cannot hold. `server.set` writes the
    // shared attribute table once, with no connection.
    let _ = server.set(&server.dis.firmware_revision, &dis_firmware_revision());
    let _ = server.set(&server.dis.hardware_revision, &dis_hardware_revision());
    let _ = server.set(&server.dis.serial_number, &dis_serial_number());
    let _ = server.set(&server.obc.config, &config_blob(&store.borrow()));

    info!(
        "ble: DIS fw '{}' hw '{}' serial '{}'",
        identity::firmware_revision().as_str(),
        identity::HARDWARE_REVISION,
        identity::serial_string().as_str()
    );

    // Advertise → serve → re-advertise, forever, with a parked Off state the rider's Bluetooth
    // switch gates. The sensor manager's one central-role task rides beside them, gated by the same
    // switch.
    join(
        sensors::run(stack, server, sensor_injector),
        join(host_task(runner), async {
            loop {
                // A Forget-phone request latched between phases: honour it before the next
                // advertise, so the freshly-open pairing window never races a stale bond.
                if FORGET_BOND.try_take().is_some() {
                    forget_bond(stack, store, shared).await;
                }

                // While off, publish Off and park. The advertiser is not running, so the device
                // vanishes from scans. Forget is honoured while parked too.
                if !state::radio_enabled() {
                    info!("ble: radio off — parked until re-enabled");
                    publish(|s| {
                        s.state = LinkState::Off;
                        s.peer = None;
                        s.conn_interval_ms = 0;
                        s.att_mtu = 0;
                        s.phy_2m = false;
                        s.passkey = None;
                        s.secured = false;
                    });
                    if let Either::Second(()) = select(state::radio_enabled_wait(), FORGET_BOND.wait()).await {
                        forget_bond(stack, store, shared).await;
                    }
                    continue;
                }

                publish(|s| {
                    s.state = LinkState::Advertising;
                    s.peer = None;
                    s.conn_interval_ms = 0;
                    s.att_mtu = 0;
                    s.phy_2m = false;
                    s.passkey = None;
                    s.secured = false;
                });
                // Re-read the advertised name each cycle, so a rename takes effect on the next
                // advertising start. Refresh the config cache from RRAM first, so the cache stays
                // coherent with the persisted truth across an advertise cycle.
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().refresh_settings_if_changed(&mut guard);
                }
                let adv_name = advertised_name(&store.borrow());

                let conn = match select3(
                    advertise_lifecycle(adv_name.as_str(), &mut peripheral, server),
                    state::radio_disabled(),
                    FORGET_BOND.wait(),
                )
                .await
                {
                    Either3::First(Ok(conn)) => conn,
                    Either3::First(Err(e)) => {
                        // An advertise error must not take the firmware down and must not wedge the
                        // loop.
                        warn!("ble: advertise error: {:?} — retrying in 1 s", defmt::Debug2Format(&e));
                        Timer::after_secs(1).await;
                        continue;
                    }
                    Either3::Second(()) => continue, // radio off — park at the loop top
                    Either3::Third(()) => {
                        forget_bond(stack, store, shared).await;
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

                // The link is bondable only while no bond is stored. With a bond present the control
                // plane rejects the pairing attempt outright, so a stranger can never mint a
                // replacement bond; Forget phone is the only re-pair path. A bonded phone's silent
                // reconnect is encryption resumption, not pairing, so neither knob touches it.
                let open_pairing = !state::status().paired;
                if let Err(e) = conn.raw().set_bondable(open_pairing) {
                    warn!("ble: set_bondable failed: {:?}", defmt::Debug2Format(&e));
                }
                if !open_pairing {
                    info!("ble: bond stored — new pairing attempts on this link will be rejected");
                }

                let reason = match select(
                    serve_connection(stack, server, &conn, store, shared),
                    join4(
                        negotiate_link(stack, &conn),
                        v4::serve_objects(stack, server, &conn),
                        battery_task(stack, server, &conn),
                        link_control(stack, &conn, store, shared),
                    ),
                )
                .await
                {
                    Either::First(reason) => reason,
                    Either::Second(_) => unreachable!("the background futures never return"),
                };
                // A peer disconnect resolves the `select` above and drops `serve_objects`, so the
                // driver's own teardown never runs: a live PUT would keep its reservation and a live
                // GET its hold, pinning a revision the store cannot free. Release from this arm.
                if let Some(writer) = crate::flat_store::writer() {
                    v4::release_engine(&writer).await;
                }
                publish(|s| {
                    s.disconnects += 1;
                    s.last_disconnect_reason = reason;
                });
            }
        }),
    )
    .await;
    unreachable!()
}

/// A disconnect can cancel the mutex wait. Preserve the consumed request wake for the next phase.
struct ForgetWakeGuard(bool);

impl Drop for ForgetWakeGuard {
    fn drop(&mut self) {
        if self.0 {
            FORGET_BOND.signal(());
        }
    }
}

/// Remove durable keys before host keys. Controller updates have no receipt in trouble-host.
async fn forget_bond(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    store: &core::cell::RefCell<LinkControl>,
    shared: &SharedSettingsMutex,
) {
    let mut wake = ForgetWakeGuard(true);
    let mut guard = shared.lock().await;
    // No await after admission: disconnect cannot interrupt either key removal or its receipt.
    let request = state::begin_bond_removal();
    let result = store.borrow_mut().clear_bond(&mut guard);
    drop(guard);
    let result = result.and_then(|()| {
        let identity = stack.with_bond_information(|bonds| bonds.first().map(|b| b.identity));
        if let Some(identity) = identity {
            stack.remove_bond_information(identity).map_err(|e| {
                warn!("ble: host key removal failed: {:?}", defmt::Debug2Format(&e));
                obc_app::ble::BondError::HostKeysRemoveFailed
            })?;
        }
        publish(|s| s.paired = false);
        Ok(obc_app::ble::ControllerClearance::Unconfirmed)
    });
    if let Err(error) = result {
        warn!("ble: bond removal incomplete: {:?}", defmt::Debug2Format(&error));
    }
    if let Some(effect) = request {
        let outcome = obc_app::ble::BondOutcome::from_result(effect.token(), result);
        info!("ble: bond result {:?}", defmt::Debug2Format(&outcome));
        state::finish_bond_removal(outcome);
    }
    wake.0 = false;
}

async fn link_control(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,

    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &core::cell::RefCell<LinkControl>,
    shared: &SharedSettingsMutex,
) {
    match select(state::radio_disabled(), FORGET_BOND.wait()).await {
        Either::First(()) => info!("ble: radio switched off — dropping the live connection"),
        Either::Second(()) => {
            forget_bond(stack, store, shared).await;
            info!("ble: forget phone — dropping the live connection");
        }
    }
    conn.raw().disconnect();
}

/// The host's transport pump — it must run forever alongside the advertise loop. trouble-host
/// delivers LE advertising reports only through an [`EventHandler`] on the rx runner, so the sensor
/// manager's [`sensors::ScanEventHandler`] parses each report here. The handler is inert unless the
/// manager has a scan armed.
async fn host_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) -> ! {
    loop {
        if let Err(e) = runner.run_with_handler(&sensors::ScanEventHandler).await {
            let e = defmt::Debug2Format(&e);
            defmt::panic!("ble: host runner error: {:?}", e);
        }
    }
}
