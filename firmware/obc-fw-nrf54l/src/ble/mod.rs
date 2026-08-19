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
//! Radio peripheral/interrupt selection lives in [`crate::board`]; this module owns MPSL/SDC runtime policy.
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
//! (advertising works — the failure needs a sync anchor). [`crate::board::init!`] sets both knobs in
//! its `ble`-build boot config.

mod control;
mod data_plane;
mod gatt;
mod lifecycle;
mod sensors;
mod state;
mod v4;
mod weather;

// The app-facing link snapshot (epic #447): the ride loop feeds it into `App::set_ble_status` each
// pass. The only BLE state that crosses into the app seam, already distilled to `obc_app` vocabulary.
pub use state::app_ble_status;

// The link-edge wake (epic #447, P2): the ride loop's event-driven sleep selects on this so a link
// change — connect/disconnect, and the pairing passkey — pulls it out of warm sleep to feed the seam
// and render the passkey card.
pub use state::wait_status_change;

// The settings→radio controls (#455): the ride loop pushes the persisted Bluetooth switch each pass
// and rings the Bluetooth screen's Forget-phone request; the lifecycle loop below honours both.
pub(crate) use state::set_usb_radio_inhibited;
pub use state::{request_forget_bond, set_radio_enabled};

// The weather due plane's seams (WX8, #1193): the ride loop pushes the app-side context snapshot
// each pass and raises the urgent request when WX11's dashboard opens; the store's commit/config
// paths poke the two crate-internal edges.
pub use weather::refresh_in_flight as weather_refresh_in_flight;
pub use weather::request_weather_now;
pub use weather::set_weather_inputs;
pub(crate) use weather::{
    note_commit as weather_committed, note_settings_changed as weather_settings_changed,
    note_unchanged as weather_unchanged,
};

// The radio link's lifetime counters, for the §7.5 diagnostics blob any transport can serve.
pub(crate) use state::link_counters;

// The BLE sensor manager's app-facing seam (SE6, epic #707): the per-quantity status snapshot the
// ride loop feeds the Sensors screen, and the scan/save/forget one-shot requests flowing back — the
// central-role analogue of the phone link's `app_ble_status` + `request_forget_bond`. SE7 (the
// Sensors screen + saved-sensor persistence, #714) consumes these.
pub use sensors::{
    cancel_scan, request_forget_sensor, request_save_sensor, request_scan, sensor_scan_hits, sensor_slot_status,
    SensorSlotState,
};

use core::mem::MaybeUninit;

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::join::{join, join3, join4, join5};
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_nrf::mode::Blocking;
use embassy_nrf::{cracen, peripherals, Peri};
use embassy_time::{with_timeout, Duration, Timer};
use nrf_sdc::mpsl::MultiprotocolServiceLayer;
use nrf_sdc::{self as sdc, mpsl};
use trouble_host::prelude::*;

use crate::init_static;
use crate::link::identity;
use crate::object_store::ObjectStore;
use crate::SharedStoreMutex;

use control::serve_connection;
use data_plane::battery_task;
use gatt::{
    advertised_name, config_blob, device_address, dis_firmware_revision, dis_hardware_revision, dis_serial_number,
    Server, OBC_PSM,
};
use lifecycle::{advertise_lifecycle, negotiate_link, AdvertisingIntent};
use state::{publish, LinkState, FORGET_BOND};

/// Concurrent **sensor** links the central role holds (SE6, epic #707) — the HR/power/cadence
/// straps the manager connects to beside the phone link. 3 on the LM20 — HR, power, and cadence
/// all live at once. Every central link costs ~2.3 KB of SDC memory (see [`SDC_MEM_SIZE`]) plus
/// host arena; one const here, arbitrated by the `main.rs` budget assert.
const SENSOR_LINKS: usize = 3;
/// Total ACL links the host tracks: the one phone (peripheral role) + [`SENSOR_LINKS`] sensors
/// (central role). One `Stack` runs both roles concurrently (trouble-host 0.7).
const CONNECTIONS_MAX: usize = 1 + SENSOR_LINKS;
/// Advertising sets — one legacy connectable set is all the advertise policy needs.
const ADV_SETS_MAX: usize = 1;
/// Bonded peers stored in the host: exactly one — the **phone**. Sensors are **not** bonded (open
/// GATT servers connected by stored address, no SMP), so they never consume a bond slot. While the
/// phone slot is occupied new pairings are rejected (#455) and only Forget phone clears it, so the
/// resolving list never holds more than the single phone (matches the app's single-peer model).
const BONDS_MAX: usize = 1;
/// L2CAP channels: the phone link's 3 (signal + ATT + the data-plane CoC) plus **2 per sensor link**
/// (the fixed L2CAP signalling channel + the ATT bearer the GATT client rides). No CoC on a sensor
/// link — sensors are notification-only GATT servers.
const L2CAP_CHANNELS_MAX: usize = 3 + 2 * SENSOR_LINKS;
/// Outgoing/incoming LL buffers per link (the TrouBLE nrf54 example's values).
const L2CAP_TXQ: u8 = 3;
const L2CAP_RXQ: u8 = 3;

/// SDC memory block, sized to `Builder::required_memory()` for this exact config (logged at boot;
/// the SDC **warns** if the block is bigger than needed and **errors out of `build()`** if smaller —
/// so this must be ≥ the real requirement). Re-measure after any Builder/buffer_cfg change.
///
/// Peripheral-only was 4704 B (measured on glass 2026-07-02). Adding the central role + scan (SE6,
/// epic #707) grows it by the SDC header math — `SDC_MEM_PER_CENTRAL_LINK(251,251,3,3)` ≈ 2310 B ×
/// [`SENSOR_LINKS`], plus `SDC_MEM_SCAN(3)` ≈ 720 B, plus ~21 B shared → 4704 + 2310 + 720 + 21 ≈
/// **MEASURED ON GLASS 2026-07-24** (LM20-DK, 3 sensor links): the boot RTT's
/// `ble: sdc required_memory = 11336 bytes` — pinned exactly, so the SDC neither errors ("Memory
/// buffer too small") nor warns ("Memory buffer too big", which the earlier 13 312 estimate did).
/// Re-read that line and re-pin after any Builder/buffer_cfg/[`SENSOR_LINKS`] change.
pub(crate) const SDC_MEM_SIZE: usize = 11336;

/// TrouBLE's host arena for this config (connection state + the DefaultPacketPool at MTU 251 + the
/// single-peer bond storage the `security` feature adds).
pub(crate) type Resources = HostResources<
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
/// `default-packet-pool-mtu-251` feature), the CRACEN RNG the LL's crypto pulls from, and the
/// #677 evictions: the `RefCell<ObjectStore>` (~12.8 KB), the GATT [`Server`] (~2.2 KB) and its
/// pinned GAP name — all formerly `ble::run` locals whose construction temporaries gave the task's
/// poll function a 30.5 KB entry frame (see [`run`]'s doc). Moving them here is ~zero-sum RAM-wise:
/// the task's future (its `POOL` static) shrinks by what these statics grow. The SDC/host *stack*
/// usage rides `main.rs`'s `STACK_RESERVE` like everything else.
/// The sensor manager's own resident statics (SE6, epic #707): the deduped scan-hit snapshot, the
/// per-quantity slot-status table, and the saved-sensor table — the small [`sensors`] cells the scan
/// event handler + the app seam read/write. `Resources`/`SDC_MEM_SIZE` above already absorbed the
/// central-role + scan buffers via [`CONNECTIONS_MAX`]/[`L2CAP_CHANNELS_MAX`]/[`SDC_MEM_SIZE`]; this
/// is the manager's plain `.bss` state on top. The transient `GattClient` + its 512 B `Notification`
/// live in `run`'s task future per the #677 rule (bounded, not resident — re-measured on glass).
pub(crate) const MPSL_BYTES: usize = core::mem::size_of::<MultiprotocolServiceLayer<'static>>();
pub(crate) const HOST_RESOURCES_BYTES: usize = core::mem::size_of::<Resources>();
pub(crate) const PACKET_POOL_BYTES: usize = core::mem::size_of::<DefaultPacketPool>();
pub(crate) const CRACEN_BYTES: usize = core::mem::size_of::<cracen::Cracen<'static, Blocking>>();
/// The one shared [`ObjectStore`] now lives in [`crate::link`] (every transport drives the same
/// card), but it is still reported under the historical `ble_object_store` name so the pinned
/// resource baseline keeps its meaning.
pub(crate) const OBJECT_STORE_BYTES: usize = crate::link::OBJECT_STORE_BYTES;
pub(crate) const SERVER_BYTES: usize = core::mem::size_of::<Server<'static>>();
pub(crate) const GAP_NAME_BYTES: usize = core::mem::size_of::<heapless::String<48>>();
pub(crate) const SENSOR_MANAGER_BYTES: usize = sensors::RESIDENT_BYTES;
/// The protocol-v4 adapter's record and reaction buffers (FS7.5-c3a).
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

// The resident statics (see the module doc): written exactly once in [`run`], never aliased.
static mut MPSL: MaybeUninit<MultiprotocolServiceLayer<'static>> = MaybeUninit::uninit();
static mut RNG: MaybeUninit<cracen::Cracen<'static, Blocking>> = MaybeUninit::uninit();
static mut SDC_MEM: MaybeUninit<sdc::Mem<SDC_MEM_SIZE>> = MaybeUninit::uninit();
static mut RESOURCES: MaybeUninit<Resources> = MaybeUninit::uninit();
// The #677 evictions (see [`run`]'s doc): the GATT server and the GAP name the server's attribute
// table borrows for its (now `'static`) life. Formerly `run` locals — their construction temporaries
// lived in the task's steady-state poll frame. (The object store was the third; it now lives in
// `crate::link` because every transport shares one.)
static mut GAP_NAME: MaybeUninit<heapless::String<48>> = MaybeUninit::uninit();
static mut SERVER: MaybeUninit<Server<'static>> = MaybeUninit::uninit();
static mut STACK: MaybeUninit<Stack<'static, nrf_sdc::SoftdeviceController<'static>, DefaultPacketPool>> =
    MaybeUninit::uninit();

/// Build the SDC memory block into `.bss` ([`SDC_MEM`]) off the poll frame — `#[inline(never)]` is
/// load-bearing on this and the two init fns below: the construction temporary must land in **this**
/// transient frame (popped before steady state, at boot's shallow depth), not in `run`'s poll frame.
/// Inlined (or via the `#[inline(always)]` `init_static` directly), LLVM reserves the temporary's
/// slot in the poll frame **at entry, on every poll**, which is exactly the #677 overflow.
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

/// Pin the boot-time GAP name ([`GAP_NAME`]) and build the GATT server into `.bss` ([`SERVER`]) off
/// the poll frame (see [`init_sdc_mem`]). The server's attribute table borrows the name for its
/// `'static` life; the *advertised* name is still re-read each advertise cycle, so a rename lands
/// without a reboot (the GAP characteristic keeps the boot value — Config, not GAP, is
/// authoritative). SAFETY: sole writer of `GAP_NAME`/`SERVER`, called once from [`run`].
#[inline(never)]
fn init_server(store: &core::cell::RefCell<ObjectStore>) -> &'static Server<'static> {
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

/// The SDC at the config we ship: 1 peripheral link (the phone) + the central role and scanner for
/// the sensor manager ([`SENSOR_LINKS`] central links), DLE + PHY-update on **both** roles
/// (LL payload 251 = ATT MTU 247 + 4 L2CAP header, 2M PHY; the headers *require* both halves when
/// both roles are supported), and the **extended** adv/scan/central command set on top.
///
/// The `_ext_` trio is load-bearing, not an upgrade: the nRF54L15 blob (nrfxlib 3.3.0) **faults
/// internally** (`SoftdeviceController: 50:701`) when a *legacy* `LeCreateConn` initiator receives
/// its target's advertisement — pinned on glass with the minimal harness
/// (`src/bin/ble_central_repro.rs`, 2026-07-12; reported upstream, #736) — while the same connect
/// as `LeExtCreateConn` works. And since legacy and extended adv/scan/initiate commands are one
/// mutually-exclusive HCI group (first use latches the mode), the *whole host* speaks extended:
/// advertising ([`lifecycle`] — same legacy PDUs on air), scanning, and connecting ([`sensors`]).
/// `support_adv()`/`support_scan()` stay compiled in as a safety net for any residual legacy
/// command path in trouble-host (a follow-up can cull them once soaked).
///
/// Kept in lockstep with the `required_memory` probe in [`run`] — change one, change both.
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

/// Bring the whole stack up and run it forever: MPSL (spawned — it must outlive everything),
/// SDC, the TrouBLE host, then the S0 advertise → connect → re-advertise loop, publishing every
/// link edge for the status UI. Joined against `run_status` on the thread-mode executor in
/// `main.rs`. The board-owned MPSL/SDC peripheral sets are expanded at the composition point and
/// handed in whole.
/// An **embassy task**, not a plain future: its state machine must live in this task's
/// `.bss` pool static, built **in place** by the spawn. As a `join`-ed local future in `main` it
/// was a giant stack temporary inside `main`'s poll frame — which overflowed the combined build's
/// ~36 KB stack straight into `.bss` before the first frame rendered (#270; caught by a DWT
/// watchpoint — the corrupted task pool panicked "Busy" at the com_task spawn).
///
/// **#677 — keep the big values out of this function's body.** The task pool solves where the
/// future's *state* lives, but every sizeable value constructed inline in the async body also gets
/// a **construction-temporary slot in the generated poll function's stack frame** — and LLVM
/// reserves all those slots at frame entry, on **every** poll, forever. With the object store
/// (~12.8 KB), the GATT server (~2.2 KB), the SDC memory block (4.7 KB) and TrouBLE's arena
/// (2.9 KB) built inline, the poll frame was **30,464 B** (`sub.w sp, sp, #0x7700`) of the ble
/// build's ~38 KB stack region — and SMP's synchronous software-P256 pairing chain (~5–7 KB of
/// frames, run inside `host_task`'s rx path on this same stack) overflowed the bottom into
/// `defmt_rtt::BUFFER` on every pairing attempt. The rule: anything bigger than a few hundred
/// bytes is built into a `.bss` static by a dedicated `#[inline(never)]` init fn (transient frame,
/// boot-time depth), and this body only ever holds `&'static` handles.
#[embassy_executor::task]
pub async fn run(
    spawner: Spawner,
    mpsl_p: mpsl::Peripherals<'static>,
    sdc_p: sdc::Peripherals<'static>,
    cracen_p: Peri<'static, peripherals::CRACEN>,
    // The SD/settings mutex, the one shared object store, and the boot store-epoch. The store used
    // to be built here; with USB as a second transport (#889) two independently-constructed stores
    // would each keep their own catalog and upload temp over the *same* SD card, so `main` builds
    // it once and every plane is composed with the same handle.
    stores: crate::link::LinkStores,
    // The sensor hub's HR/power/cadence injector (#808), threaded from `main`'s `static SensorHub`
    // through `spawn_ble_stack`: the central manager (SE6) decodes notifications and publishes
    // through it into the same mailboxes the debug-uart path feeds (last-writer-wins). Ownership is
    // visible at composition rather than reached through a global.
    sensor_injector: obc_platform::sensor_hub::SampleInjector<'static>,
) -> ! {
    let crate::link::LinkStores { shared, objects: store, epoch: _store_epoch } = stores;
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
    // DisplayOnly: the device shows a 6-digit passkey, the phone (keyboard) enters it → LESC
    // passkey-entry pairing, MITM-protected. Keep the static-random address (no device privacy) — the
    // phone stores our stable identity for instant reconnect; we resolve *its* rotating RPA from the
    // stored peer IRK below. In a static (it's all of 8 B — two references) because the `'static`
    // [`SERVER`]'s advertise path needs a `Peripheral<'static>`, which only a `'static` stack lends.
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
    #[cfg(feature = "ble-weather-request")]
    {
        state::arm_weather_request(embassy_time::Duration::from_secs(obc_ble::WEATHER_REQUEST_WINDOW_S));
        info!("ble: Weather Request harness armed (60 s maximum advertising window)");
    }

    let runner = stack.runner();
    let mut peripheral = stack.peripheral();

    // The GAP name is pinned at boot (the attribute borrows it for the server's `'static` life);
    // the *advertised* name is re-read from the store each advertise cycle, so a rename lands
    // without a reboot.
    let server: &'static Server<'static> = init_server(store);
    info!("ble: host up as '{}', address {:?}", advertised_name(&store.borrow()).as_str(), address);

    // Seed the runtime attribute values the macro `value =` can't hold (DIS strings, the Config
    // blob from the persisted settings, the widened `protocolVersion` read). `server.set` writes the
    // shared attribute table once — no connection needed.
    let _ = server.set(&server.dis.firmware_revision, &dis_firmware_revision());
    let _ = server.set(&server.dis.hardware_revision, &dis_hardware_revision());
    let _ = server.set(&server.dis.serial_number, &dis_serial_number());
    let _ = server.set(&server.obc.config, &config_blob(&store.borrow()));
    // `protocolVersion` (V2 / #632; card-resident epoch #776): the pre-pairing read. `store_epoch` is
    // the boot mint pass's outcome, threaded in (never re-read here) — the epoch lives on the card
    // now, and a card swap must not silently change what this task serves. `Some(epoch)` → the full
    // `protocolVersion` needs no boot seed since c3a: §5.1 makes it two fixed bytes (`u16` = 4),
    // declared as the characteristic's `value`. Protocol v2's store-epoch blob is retired — a v4
    // client learns the card's identity from the `StoreId` every `LIST` page carries.
    // The resting value: a structurally valid "nothing is due" rather than a zeroed buffer, so a
    // peer that reads the characteristic out of turn cannot mistake it for a request at 0°N 0°E.
    // Seeded with the **stored** refresh byte, not `EMPTY`'s compile-time default (#1221 F2):
    // §11.8's byte reports the rider's own setting, and a rider who persisted Off/15/60/120 must
    // not read back "30 min" between boot and the first raise. The weather plane re-asserts the
    // resting value whenever the stored setting moves.
    let _ = server.set(
        &server.weather_request.context,
        &obc_ble::WeatherRequestContext::resting(store.borrow().settings().weather_refresh as u8).encode(),
    );
    info!(
        "ble: DIS fw '{}' hw '{}' serial '{}'",
        identity::firmware_revision().as_str(),
        identity::HARDWARE_REVISION,
        identity::serial_string().as_str()
    );

    // The lifecycle loop: advertise → serve → re-advertise, forever, with no terminal state — and,
    // since #455, a parked **Off** state the rider's Bluetooth switch gates. The route-delete task
    // runs beside it for the whole lifetime (epic #447, P6) so an on-device delete executes whether
    // the phone is connected, the device is parked advertising, or the radio is off; the ride-saved
    // task likewise, so a ride finished with the radio off still reaches the catalog + Rides menu.
    // The **sensor manager** (SE6, epic #707) rides beside them (`join` — `embassy_futures` tops out
    // at `join5`): its one central-role task scans / connects / subscribes / dispatches HR/power/
    // cadence, gated by the same #455 radio switch as the peripheral link.
    join(
        sensors::run(stack, server, sensor_injector),
        join5(
            host_task(runner),
            // The trip cascade + the weather due plane ride the route-delete slot (`join5` is
            // embassy's ceiling): all three are rare, edge/deadline-driven arms, so sharing a slot
            // costs nothing.
            join3(
                route_delete_task(stack, server, store, shared),
                trip_cascade_task(stack, server, store, shared),
                weather::run(server, store, shared),
            ),
            ride_delete_task(stack, server, store, shared),
            ride_saved_task(stack, server, store, shared),
            async {
                loop {
                    // A Forget-phone request latched between phases: honour it before the next advertise,
                    // so the freshly-open pairing window never races a stale bond.
                    if FORGET_BOND.try_take().is_some() {
                        forget_bond(stack, store, shared).await;
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
                    // Re-read the advertised name each cycle — a rename (Config write) takes effect on the next
                    // advertising start, no reboot. Refresh the config cache from RRAM first (a no-op unless
                    // the ride loop flagged an on-device settings change, #456) so the cache stays coherent
                    // with the persisted truth across an advertise cycle.
                    {
                        let mut guard = shared.lock().await;
                        store.borrow_mut().refresh_settings_if_changed(&mut guard);
                    }
                    let adv_name = advertised_name(&store.borrow());
                    let advertising_intent = if state::weather_request_pending() {
                        AdvertisingIntent::WeatherRequest
                    } else {
                        AdvertisingIntent::Control
                    };
                    // Advertise until a central connects — or the radio switch flips off (dropping the
                    // advertiser future stops advertising), or a Forget request lands (handled, then this
                    // phase restarts — a moment of re-advertising is harmless).
                    let conn = match select3(
                        advertise_lifecycle(adv_name.as_str(), advertising_intent, &mut peripheral, server),
                        state::radio_disabled(),
                        select(FORGET_BOND.wait(), weather_request_policy_change()),
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
                        Either3::Third(Either::First(())) => {
                            forget_bond(stack, store, shared).await;
                            continue;
                        }
                        Either3::Third(Either::Second(())) => continue,
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
                    // link control — radio-off / Forget end in a local disconnect; a Weather
                    // Request first takes the live notify/read fast path and disconnects only for
                    // the advertisement fallback) runs
                    // concurrently and never returns before the teardown, so `select` tears it all down the
                    // moment the link drops (any disconnect drops straight back to the loop top).
                    let reason = match select(
                        serve_connection(stack, server, &conn, store, shared),
                        join4(
                            negotiate_link(stack, &conn),
                            v4::serve_objects(stack, server, &conn),
                            battery_task(stack, server, &conn),
                            link_control(stack, server, &conn, store, shared, advertising_intent),
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
                    //
                    // Everything here is scoped to **this** wire, and none of it is incidental
                    // (issue #1039): the cable can be uploading while the phone drops off. The gate
                    // is released only if the radio was the one holding it, `TRANSFER_ARM` /
                    // `TRANSFER_ABORT` are this plane's own signals (USB has its own pair), and
                    // `link_reset` closes the temp handle only if the temp is what is open.
                    {
                        let mut guard = shared.lock().await;
                        store.borrow_mut().link_reset(&mut guard);
                    }
                    publish(|s| {
                        s.disconnects += 1;
                        s.last_disconnect_reason = reason;
                    });
                }
            },
        ),
    )
    .await;
    unreachable!()
}

/// Wake the advertiser when the secondary intent changes. While it is pending, the same future
/// also enforces the exact bounded window; the timeout lowers the intent before returning so the
/// loop's next advertisement is OBC Control. A latched edge may cause one harmless immediate
/// restart before the timer begins (the level remains authoritative).
async fn weather_request_policy_change() {
    if state::weather_request_pending() {
        let Some(remaining) = state::weather_request_remaining() else {
            state::clear_weather_request();
            return;
        };
        if remaining.as_ticks() == 0 {
            state::clear_weather_request();
            info!("ble: Weather Request total budget already elapsed — returning to Control");
            return;
        }
        match select(state::weather_request_changed(), Timer::after(remaining)).await {
            Either::First(()) => {}
            Either::Second(()) => {
                state::clear_weather_request();
                info!("ble: Weather Request advertising budget elapsed — returning to Control");
            }
        }
    } else {
        state::weather_request_changed().await;
    }
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
/// and waits for the radio switch flipping **off**, a **Forget phone** request, or a newly raised
/// **Weather Request**. Radio/Forget end in a local disconnect. A request on an authenticated
/// Control link is notified and acknowledged by its read without churn; disconnect + secondary
/// advertising remains the fallback when the live delivery cannot be proven.
///
/// The advertisement path remains load-bearing for a disconnected/background phone. The live-link
/// fast path is intentionally only an optimisation of that same read receipt, not another protocol.
async fn link_control(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    store: &core::cell::RefCell<ObjectStore>,
    shared: &SharedStoreMutex,
    advertised_as: AdvertisingIntent,
) {
    loop {
        match select3(state::radio_disabled(), FORGET_BOND.wait(), weather_request_raised(advertised_as)).await {
            Either3::First(()) => {
                info!("ble: radio switched off — dropping the live connection");
                break;
            }
            Either3::Second(()) => {
                // Forget with a live link: clear the bond first, then drop the connection (locked
                // decision). The single-peer model means the peer is the bonded phone in every real
                // flow; an unbonded peer that happens to hold the link just reconnects.
                forget_bond(stack, store, shared).await;
                info!("ble: forget phone — dropping the live connection");
                break;
            }
            Either3::Third(()) => {
                // A link accepted from the secondary advertisement lives only for the request it
                // advertised. Either the authenticated read consumed it or its original radio
                // budget expired; in both cases return to Control rather than letting an
                // unauthenticated/buggy central monopolise the sole phone link indefinitely.
                if advertised_as == AdvertisingIntent::WeatherRequest && !state::weather_request_pending() {
                    info!("ble: Weather Request connection finished — returning to Control");
                    break;
                }
                // A foreground companion already has the authenticated Control link and subscribes
                // to the status characteristic. Hand it the request directly: tearing down a healthy link
                // merely to rediscover the same GATT database made Fetch now depend on iOS seeing a
                // short-lived secondary advertisement. If the subscription is absent (old app,
                // background/suspended peer, or an ATT failure), retain the original advertisement
                // path as the lossless fallback.
                let notified = if state::status().secured {
                    let (bytes, len) = obc_ble::StatusMessage::WeatherRequest.encode();
                    data_plane::notify_bounded(
                        stack,
                        server,
                        server.obc.status.handle,
                        &bytes[..len],
                        "weather request",
                    )
                    .await
                } else {
                    false
                };
                if notified {
                    // The notify is only a wake-up/hint. The authenticated read remains the receipt:
                    // it is the one place `serve_connection` clears the request, so merely queueing a
                    // notification cannot lose work if iOS suspends before consuming it.
                    let consumed = with_timeout(Duration::from_secs(3), async {
                        // The arm edge may still be latched because the level-aware waiter returned
                        // immediately above. Ignore such stale edges until the read lowers the level.
                        while state::weather_request_pending() {
                            state::weather_request_changed().await;
                        }
                    })
                    .await
                    .is_ok();
                    if consumed {
                        info!("ble: Weather Request consumed on live Control link — keeping connection");
                        continue;
                    }
                }
                info!("ble: Weather Request live notify unavailable — reconnecting through weather advertisement");
                break;
            }
        }
    }
    conn.raw().disconnect();
}

/// Wait for the *raised* level. A link accepted from a Weather Request advertisement already owns
/// the current request and must be allowed to read it; first wait for that level to clear. A link
/// accepted from OBC Control must drop immediately even if the raise raced the connect boundary.
/// `WEATHER_REQUEST_EDGE` signals both directions, so the level is always the authority.
async fn weather_request_raised(advertised_as: AdvertisingIntent) {
    if advertised_as == AdvertisingIntent::WeatherRequest {
        while state::weather_request_pending() {
            let Some(remaining) = state::weather_request_remaining() else {
                state::clear_weather_request();
                return;
            };
            match select(state::weather_request_changed(), Timer::after(remaining)).await {
                Either::First(()) => {}
                Either::Second(()) => {
                    state::clear_weather_request();
                    info!("ble: Weather Request budget expired while connected — returning to Control");
                    return;
                }
            }
        }
    } else if state::weather_request_pending() {
        return;
    }
    loop {
        state::weather_request_changed().await;
        if state::weather_request_pending() {
            return;
        }
    }
}

/// The host's transport pump — must run forever alongside the advertise loop. Runs **with the
/// sensor manager's scan event handler** (SE6): trouble-host 0.7 delivers LE advertising reports only
/// through an [`EventHandler`] on the rx runner (there is no report method on `ScanSession`), so the
/// manager's [`sensors::ScanEventHandler`] parses each report into the deduped scan snapshot here.
/// The handler is inert unless the manager has a scan armed, so this costs nothing on the steady
/// advertise/serve path.
async fn host_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) -> ! {
    loop {
        if let Err(e) = runner.run_with_handler(&sensors::ScanEventHandler).await {
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
/// phone's `storeChanged`; when disconnected the notify fails harmlessly (no subscriber), and the
/// revision bump makes the next `storeChanged`/reconnect reflect the deletion. The `ObjectStore`'s
/// own `bump_revision` rings the `STORE_CHANGED` edge the ride loop drains for the live catalog
/// rescan + P3 remap.
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
            // Notify a connected phone's `storeChanged` (harmless no-op when disconnected).
            data_plane::publish_store_change(stack, server, store, obc_ble::ObjectType::Route).await;
        } else {
            warn!("ble: [delete] on-device delete of route object {} found nothing", id);
        }
    }
}

/// Drain on-device trip **cascade**-delete requests (epic #526, TR3/TR4) for the whole `ble::run`
/// lifetime — the trip sibling of [`route_delete_task`]. The Route menu's long-press → confirm posts
/// the trip's durable id ([`request_trip_cascade`](crate::object_store::request_trip_cascade)); this
/// executes [`ObjectStore::delete_trip_cascade`] — each member route through `delete_route`, then the
/// trip object — and notifies **both** `storeChanged` edges (§4.3): the member deletes moved the
/// route store, the trip delete its own store. The same borrow discipline as the route arm: the
/// `RefCell` borrow ends before any `await`.
async fn trip_cascade_task(
    stack: &Stack<'_, sdc::SoftdeviceController<'_>, DefaultPacketPool>,
    server: &Server<'_>,
    store: &core::cell::RefCell<ObjectStore>,
    shared: &'static SharedStoreMutex,
) -> ! {
    loop {
        let id = crate::object_store::wait_trip_cascade().await;
        let deleted = {
            let mut guard = shared.lock().await;
            store.borrow_mut().delete_trip_cascade(&mut guard, id)
        };
        if deleted {
            info!("ble: [delete] on-device cascade delete of trip object {}", id);
            // Both edges (§4.3): member routes moved the route store, the trip its own. Harmless
            // no-ops when disconnected; the revision bumps carry the change to the next connect.
            data_plane::publish_store_change(stack, server, store, obc_ble::ObjectType::Route).await;
            data_plane::publish_store_change(stack, server, store, obc_ble::ObjectType::Trip).await;
        } else {
            warn!("ble: [delete] on-device cascade delete of trip object {} found nothing", id);
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
/// (its next `listRides` includes the ride), and the `STORE_CHANGED` edge re-feeds
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
