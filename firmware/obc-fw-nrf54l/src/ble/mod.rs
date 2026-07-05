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

// The link-edge wake (epic #447, P2): the ride loop's event-driven sleep selects on this so a link
// change — connect/disconnect, and the pairing passkey — pulls it out of warm sleep to feed the seam
// and render the passkey card.
pub use state::wait_status_change;

// The settings→radio controls (#455): the ride loop pushes the persisted Bluetooth switch each pass
// and rings the Bluetooth screen's Forget-phone request; the lifecycle loop below honours both.
pub use state::{request_forget_bond, set_radio_enabled};

use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::{join4, join5};
use embassy_futures::select::{select, select3, Either, Either3};
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
use state::{publish, LinkState, FORGET_BOND, TRANSFER_ABORT, TRANSFER_ACTIVE, TRANSFER_ARM};

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
/// Bonded peers stored in the host: exactly one. While it is occupied new pairings are rejected
/// (#455) and only Forget phone clears it, so the resolving list never holds more than the single
/// phone (matches the app's single-peer model).
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
    // The paired flag drives both the app's "Paired" row and the reject-when-bonded pairing policy
    // (S0 §8 as amended by #455) — seed it before the first advertise.
    publish(|s| s.paired = stored_bond.is_some());
    if let Some(bond) = stored_bond {
        match stack.add_bond_information(bond) {
            Ok(()) => info!("ble: restored stored bond — bonded reconnect armed"),
            Err(e) => warn!("ble: add_bond_information failed: {:?}", defmt::Debug2Format(&e)),
        }
    }

    // Seed the radio switch from the persisted settings (#455): a device toggled off stays off
    // across a reboot, before the first advertise. The ride loop re-pushes the live value each pass.
    state::seed_radio_enabled(store.borrow().settings().ble_enabled);

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

    // The lifecycle loop: advertise → serve → re-advertise, forever, with no terminal state — and,
    // since #455, a parked **Off** state the rider's Bluetooth switch gates. The route-delete task
    // runs beside it for the whole lifetime (epic #447, P6) so an on-device delete executes whether
    // the phone is connected, the device is parked advertising, or the radio is off; the ride-saved
    // task likewise, so a ride finished with the radio off still reaches the catalog + Rides menu.
    join5(
        host_task(runner),
        route_delete_task(&stack, &server, &store, shared),
        ride_delete_task(&stack, &server, &store, shared),
        ride_saved_task(&stack, &server, &store, shared),
        async {
            loop {
                // A Forget-phone request latched between phases: honour it before the next advertise,
                // so the freshly-open pairing window never races a stale bond.
                if FORGET_BOND.try_take().is_some() {
                    forget_bond(&stack, &store, shared).await;
                }

                // The radio switch (#455): while off, publish Off and park — the advertiser is not
                // running (dropped with the previous phase), so the device vanishes from scans. Forget
                // is honoured while parked too (the rider clears the bond with the radio down).
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
                        forget_bond(&stack, &store, shared).await;
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
                // Re-read the advertised name each cycle — a rename (Config write) takes effect on the next
                // advertising start, no reboot. Refresh the config cache from RRAM first (a no-op unless
                // the ride loop flagged an on-device settings change, #456) so the cache stays coherent
                // with the persisted truth across an advertise cycle.
                {
                    let mut guard = shared.lock().await;
                    store.borrow_mut().refresh_settings_if_changed(&mut guard);
                }
                let adv_name = advertised_name(&store.borrow());
                // Advertise until a central connects — or the radio switch flips off (dropping the
                // advertiser future stops advertising), or a Forget request lands (handled, then this
                // phase restarts — a moment of re-advertising is harmless).
                let conn = match select3(
                    advertise_lifecycle(adv_name.as_str(), &mut peripheral, &server),
                    state::radio_disabled(),
                    FORGET_BOND.wait(),
                )
                .await
                {
                    Either3::First(Ok(conn)) => conn,
                    Either3::First(Err(e)) => {
                        // An advertise error must not take the firmware down and must not wedge the loop —
                        // log it, wait a beat, and try again.
                        warn!("ble: advertise error: {:?} — retrying in 1 s", defmt::Debug2Format(&e));
                        Timer::after_secs(1).await;
                        continue;
                    }
                    Either3::Second(()) => continue, // radio off — park at the loop top
                    Either3::Third(()) => {
                        forget_bond(&stack, &store, shared).await;
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

                // Bonding policy (S0 §8, amended by #455): the link is bondable only while **no** bond
                // is stored. With a bond present the link stays at trouble's not-bondable default and
                // the control plane rejects the pairing attempt outright (see `serve_connection`) —
                // a stranger can never mint a replacement bond; Forget phone is the only re-pair path.
                // The bonded phone's silent reconnect is encryption resumption, not pairing, so it is
                // untouched by either knob.
                let open_pairing = !state::status().paired;
                if let Err(e) = conn.raw().set_bondable(open_pairing) {
                    warn!("ble: set_bondable failed: {:?}", defmt::Debug2Format(&e));
                }
                if !open_pairing {
                    info!("ble: bond stored — new pairing attempts on this link will be rejected");
                }

                // Serve the link until the peer drops it. `serve_connection` pumps GATT + connection
                // events (so the phone's own MTU/PHY/DLE moves are serviced and our control-plane writes
                // are answered) and owns the exit — it returns the disconnect reason. The background set
                // (parameter negotiation, the CoC accept-and-drain, the BAS battery notify, and the
                // #455 link control — radio-off / Forget both end in a local disconnect) runs
                // concurrently and never returns before the teardown, so `select` tears it all down the
                // moment the link drops (any disconnect drops straight back to the loop top).
                let reason = match select(
                    serve_connection(&stack, &server, &conn, &store, shared),
                    join4(
                        negotiate_link(&stack, &conn),
                        serve_coc(&stack, &server, &conn, &store, shared),
                        battery_task(&stack, &server, &conn),
                        link_control(&stack, &conn, &store, shared),
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
        },
    )
    .await;
    unreachable!()
}

/// Forget the bonded phone (#455): zero the RRAM bond slot (a reboot lands in open pairing), drop
/// the bond from the host's table + the controller resolving list (the forgotten phone can't
/// silently re-encrypt this session), and lower the paired flag — which re-opens pairing on the
/// next connection and reads "Paired: no" on the Bluetooth screen.
async fn forget_bond(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    store: &core::cell::RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
) {
    {
        let mut guard = shared.lock().await;
        store.borrow_mut().clear_bond(&mut guard);
    }
    let identity = stack.with_bond_information(|bonds| bonds.first().map(|b| b.identity));
    if let Some(identity) = identity {
        if let Err(e) = stack.remove_bond_information(identity) {
            warn!("ble: remove_bond_information failed: {:?}", defmt::Debug2Format(&e));
        }
    }
    publish(|s| s.paired = false);
    info!("ble: bond forgotten — open pairing re-armed");
}

/// The per-connection control watcher (#455): rides the background `join4` beside the serve loop
/// and waits for either the radio switch flipping **off** or a **Forget phone** request. Both end
/// in a local disconnect; the `Disconnected` event then unwinds `serve_connection` and the outer
/// loop lands back at its top (parked Off, or re-advertising with pairing open). Returns after the
/// disconnect request — the enclosing `join4` keeps waiting on its never-returning siblings, so the
/// teardown still comes from the one `select` exit path.
async fn link_control(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &core::cell::RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
) {
    match select(state::radio_disabled(), FORGET_BOND.wait()).await {
        Either::First(()) => info!("ble: radio switched off — dropping the live connection"),
        Either::Second(()) => {
            // Forget with a live link: clear the bond first, then drop the connection (locked
            // decision). The single-peer model means the peer is the bonded phone in every real
            // flow; an unbonded peer that happens to hold the link just reconnects.
            forget_bond(stack, store, shared).await;
            info!("ble: forget phone — dropping the live connection");
        }
    }
    conn.raw().disconnect();
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

/// Drain on-device route-delete requests (epic #447, P6) for the whole `ble::run` lifetime — folded
/// into the top-level `join`, so it runs whether the phone is connected or the device is parked
/// advertising. The ride loop's Route-menu hold posts a route's durable id
/// ([`request_route_delete`](crate::object_store::request_route_delete)); this executes it through
/// [`ObjectStore::delete_route`] — the same catalog + revision + `storeChanged` path a phone
/// `deleteObject` command takes — so the on-device delete is coherent with the wire.
///
/// The `RefCell<ObjectStore>` borrow never spans an `await` (it ends before `publish_store_change`),
/// matching the store's single-executor discipline. `publish_store_change` notifies a *connected*
/// phone's `storeChanged` + digest; when disconnected its notifies fail harmlessly (no subscriber),
/// and the re-seeded digest attribute + the revision bump make the next read/reconnect reflect the
/// deletion. The `ObjectStore`'s own `bump_revision` rings the `STORE_CHANGED` edge the ride loop
/// drains for the live catalog rescan + P3 remap.
async fn route_delete_task(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &core::cell::RefCell<ObjectStore>,
    shared: &'static SharedStoreMutex,
) -> ! {
    loop {
        let id = crate::object_store::wait_route_delete().await;
        let deleted = {
            let mut guard = shared.lock().await;
            store.borrow_mut().delete_route(&mut guard, id)
        };
        if deleted {
            info!("ble: [delete] on-device delete of route object {}", id);
            // Notify a connected phone (harmless no-op when disconnected) and re-seed the digest.
            data_plane::publish_store_change(stack, server, store, obc_ble::ObjectType::Route).await;
        } else {
            warn!("ble: [delete] on-device delete of route object {} found nothing", id);
        }
    }
}

/// Drain on-device **ride**-delete requests (epic #447, P7 / #454) for the whole `ble::run` lifetime
/// — the ride-namespace twin of [`route_delete_task`]. The ride loop's Rides-menu hold posts a ride's
/// durable id ([`request_ride_delete`](crate::object_store::request_ride_delete)); this executes it
/// through [`ObjectStore::delete_ride`] — the same catalog + revision + `storeChanged` path a phone
/// `deleteObject` command takes — so the on-device ride delete is coherent with the wire (the phone's
/// device-rides reconcile; its own library copy is untouched).
async fn ride_delete_task(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &core::cell::RefCell<ObjectStore>,
    shared: &'static SharedStoreMutex,
) -> ! {
    loop {
        let id = crate::object_store::wait_ride_delete().await;
        let deleted = {
            let mut guard = shared.lock().await;
            store.borrow_mut().delete_ride(&mut guard, id)
        };
        if deleted {
            info!("ble: [delete] on-device delete of ride object {}", id);
            data_plane::publish_store_change(stack, server, store, obc_ble::ObjectType::Ride).await;
        } else {
            warn!("ble: [delete] on-device delete of ride object {} found nothing", id);
        }
    }
}

/// Drain the ride loop's **saved-ride** edge for the whole `ble::run` lifetime: a locally-finished
/// ride committed its `RD{id}.ORD` (`Storage::run_pending_save`), so re-scan `/tracks` into the
/// [`ObjectStore`] catalog and bump the revision ([`ObjectStore::adopt_saved_rides`]). That one edge
/// then feeds everyone the way an upload commit does: a connected phone gets `storeChanged(ride)` +
/// the fresh digest (its next `listRides` includes the ride), and the `STORE_CHANGED` edge re-feeds
/// the on-device Rides menu next pass. Before this task existed, both catalogs were boot-scans only
/// and a freshly-finished ride was invisible everywhere until a reboot.
async fn ride_saved_task(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &core::cell::RefCell<ObjectStore>,
    shared: &'static SharedStoreMutex,
) -> ! {
    loop {
        crate::object_store::wait_ride_saved().await;
        {
            let mut guard = shared.lock().await;
            store.borrow_mut().adopt_saved_rides(&mut guard);
        }
        info!("ble: [store] adopted freshly-saved ride(s) into the catalog");
        data_plane::publish_store_change(stack, server, store, obc_ble::ObjectType::Ride).await;
    }
}
