//! The BLE central sensor manager: scan, connect, GATT-subscribe, decode and dispatch for HR, power
//! and cadence sensors, on the same [`Stack`] as the phone-facing peripheral link. One central-role
//! task, joined into [`super::run`], driven by signal and latch state.
//!
//! The byte-to-struct half is `obc-ble` (host-tested, with no trouble-host type): the profile
//! parsers, the crank-to-rpm accumulator, the advertisement classifier and the cadence arbitration.
//! This file is the radio glue. It feeds scan-report and notification bytes into those, and pushes
//! decoded values through the hub's [`SampleInjector`](obc_platform::sensor_hub::SampleInjector) —
//! the same mailboxes the `debug-uart` injection path feeds (last writer wins), so the app cannot
//! tell a real strap from an injected line.
//!
//! trouble-host API notes:
//!
//! - Scan reports arrive through an [`EventHandler`], not `ScanSession`. `ScanSession` is only a
//!   guard that keeps the scan enabled; the `LeAdvReport`s are delivered synchronously through
//!   `Runner::run_with_handler`'s handler. So [`ScanEventHandler`] parses each report here, armed by
//!   [`SCAN_ARMED`], and [`super::host_task`] runs the host with it.
//! - Extended scan and extended connect ([`Scanner::scan_ext`] / [`Central::connect_ext`]), not the
//!   legacy commands, and not by choice of wire format. The nRF54L15 controller blob faults
//!   internally (`SoftdeviceController: 50:701`) the instant a legacy `LeCreateConn` initiator
//!   receives its target's advertisement, while the same connect as `LeExtCreateConn` works. Legacy
//!   and extended adv/scan/initiate commands are one mutually-exclusive HCI group, where first use
//!   latches the mode, so the advertiser rides the extended commands too. The PDUs on air stay
//!   legacy, and phones see no difference.
//! - The `GattClient` event task must be polled beside the notification loop. [`GattClient::task`]
//!   pumps the ATT rx; without it `subscribe`/`next` never complete. We `select` the two, plus a
//!   radio or request interrupt, so a disconnect tears the whole session down.
//! - No sensor bonding or SMP. Sensors are open GATT servers, connected by stored address through
//!   the controller filter-accept-list. `BONDS_MAX` stays 1, for the phone.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{select, select3, select4, Either3, Either4};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::{String, Vec};
use nrf_sdc::{self as sdc};
use obc_ble::SensorKind;
use obc_platform::sensor_hub::SampleInjector;
use trouble_host::prelude::*;

use super::gatt::Server;

/// The three fixed sensor quantities (HR, power, cadence), one saved slot each.
const QUANTITIES: usize = 3;
/// Deduped scan snapshot cap — enough discovered sensors to fill a scan list without unbounded RAM.
const MAX_SCAN_HITS: usize = 8;

/// One user-initiated scan window (active legacy scan). Reports drain into [`SCAN_HITS`] meanwhile.
const SCAN_SECS: u64 = 10;
/// Active-scan timing while connecting/scanning: ~60 ms interval, ~30 ms window (a 50 % duty).
const SCAN_INTERVAL: Duration = Duration::from_millis(60);
const SCAN_WINDOW: Duration = Duration::from_millis(30);
/// A single connection attempt is bounded — an absent/asleep sensor must not block the one link
/// forever. On timeout the connect future is dropped (its `OnDrop` issues `LeCreateConnCancel`).
const CONNECT_TIMEOUT_SECS: u64 = 20;
/// Backoff after a drop or a failed attempt before a retry, woken early by any radio or request
/// change.
const BACKOFF_SECS: u64 = 15;

/// Steady-state link parameters, applied by [`serve_link`] once subscribed; the connect itself uses
/// the fast params in [`run_link`]. A 250–500 ms interval keeps the sensor link cheap beside the
/// phone link, with a 5 s supervision timeout. The connection-event length is the radio timeslot the
/// SDC schedules per event, and a sensor exchange (notifications of 20 B or less) needs only a few
/// ms, so 30 ms is ample and matches the proven phone-link event length. A peer that later requests
/// its own preference still gets it, through the event pump in [`serve_link`].
const CRUISE_PARAMS: RequestedConnParams = RequestedConnParams {
    min_connection_interval: Duration::from_millis(250),
    max_connection_interval: Duration::from_millis(500),
    max_latency: 0,
    min_event_length: Duration::from_micros(0),
    max_event_length: Duration::from_millis(30),
    supervision_timeout: Duration::from_millis(5000),
};

/// A sensor discovered in a scan — one row of the Sensors screen's scan list.
#[derive(Clone)]
pub struct SensorScanHit {
    /// The advertiser address (little-endian, as the wire carries it).
    pub addr: [u8; 6],
    /// Whether the address is random and not public. Needed to reconnect by the same address.
    pub random: bool,
    pub kind: SensorKind,
    /// The advertised local name, truncated to 16 chars. Empty when the advert carried none.
    pub name: String<16>,
    /// Last-seen RSSI, in dBm.
    pub rssi: i8,
}

/// The live state of one sensor slot, for the app seam.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum SensorSlotState {
    /// No sensor saved, or saved but the radio is off — nothing to do.
    Idle,
    Connecting,
    /// Connected and subscribed; notifications are flowing.
    Connected,
}

/// A per-quantity status snapshot. The ride loop feeds it through the app the way
/// [`super::app_ble_status`] feeds the phone link.
#[derive(Clone, Copy)]
pub struct SensorSlotStatus {
    pub saved: bool,
    pub state: SensorSlotState,
    /// The sensor's last-read battery percent, set after a connect that reads 0x2A19.
    pub battery: Option<u8>,
    /// `Instant`-ms of the freshest decoded value; 0 means none yet.
    pub last_value_ms: u32,
}

impl SensorSlotStatus {
    const fn init() -> Self {
        Self { saved: false, state: SensorSlotState::Idle, battery: None, last_value_ms: 0 }
    }
}

/// A saved sensor: the stored address to reconnect by. The kind is implied by the slot index. No
/// name and no bond, because sensors are open GATT servers.
#[derive(Clone, Copy)]
struct SavedSensor {
    addr: [u8; 6],
    random: bool,
}

#[derive(Clone, Copy)]
struct SaveReq {
    quantity: usize,
    addr: [u8; 6],
    random: bool,
}

type ScanHitsCell = BlockingMutex<CriticalSectionRawMutex, RefCell<Vec<SensorScanHit, MAX_SCAN_HITS>>>;
type SlotStatusCell = BlockingMutex<CriticalSectionRawMutex, Cell<[SensorSlotStatus; QUANTITIES]>>;
type SavedCell = BlockingMutex<CriticalSectionRawMutex, Cell<[Option<SavedSensor>; QUANTITIES]>>;

/// The deduped scan snapshot, written by [`ScanEventHandler`] in the host rx path and read by the
/// app seam. The handler is synchronous, never across an `await`.
static SCAN_HITS: ScanHitsCell = BlockingMutex::new(RefCell::new(Vec::new()));
static SLOT_STATUS: SlotStatusCell =
    BlockingMutex::new(Cell::new([SensorSlotStatus::init(), SensorSlotStatus::init(), SensorSlotStatus::init()]));
/// The saved-sensor table, reconciled from the app's persisted settings by the ride loop's per-pass
/// diff. It starts empty and the ride loop seeds it on the first pass, so a saved sensor reconnects
/// across a reboot.
static SAVED: SavedCell = BlockingMutex::new(Cell::new([None, None, None]));

/// Whether a scan is armed — the [`ScanEventHandler`] only records reports while true, so stray
/// controller reports never pollute the snapshot.
static SCAN_ARMED: AtomicBool = AtomicBool::new(false);

/// The manager's wake edge: pulsed by every request below and by the radio switch, so the manager
/// reacts without polling. A burst coalesces into one wake, and the loop then re-reads the latched
/// requests and the radio level.
static WORK_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static SCAN_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static SAVE_REQUEST: Signal<CriticalSectionRawMutex, SaveReq> = Signal::new();
static FORGET_REQUEST: Signal<CriticalSectionRawMutex, usize> = Signal::new();

/// The manager's resident statics: the scan snapshot, the slot-status table, the saved table, every
/// request and wake `Signal`, and the scan-armed flag. The SDC and host central and scan buffers are
/// already counted by `SDC_MEM_SIZE` and `Resources`; this is the manager's own `.bss` on top.
///
/// Keep this in sync: every `static` added in this module must be summed here, because the resource
/// report publishes it. `const`s hold no runtime storage and are not counted.
pub const RESIDENT_BYTES: usize = core::mem::size_of::<ScanHitsCell>()
    + core::mem::size_of::<SlotStatusCell>()
    + core::mem::size_of::<SavedCell>()
    + core::mem::size_of::<AtomicBool>() // SCAN_ARMED
    + 2 * core::mem::size_of::<Signal<CriticalSectionRawMutex, ()>>() // WORK_EDGE + SCAN_REQUEST
    + core::mem::size_of::<Signal<CriticalSectionRawMutex, SaveReq>>() // SAVE_REQUEST
    + core::mem::size_of::<Signal<CriticalSectionRawMutex, usize>>(); // FORGET_REQUEST

pub fn sensor_scan_hits<R>(f: impl FnOnce(&[SensorScanHit]) -> R) -> R {
    SCAN_HITS.lock(|c| f(c.borrow().as_slice()))
}

/// The per-quantity status snapshot for the app seam. `quantity` is 0=HR, 1=power, 2=cadence.
pub fn sensor_slot_status(quantity: usize) -> SensorSlotStatus {
    SLOT_STATUS.lock(|c| c.get()[quantity.min(QUANTITIES - 1)])
}

/// Ring a one-shot scan request. The manager runs a ~10 s active scan and publishes the results
/// into [`sensor_scan_hits`].
pub fn request_scan() {
    SCAN_REQUEST.signal(());
    wake_work();
}

/// Drop a pending scan request that has not started yet. Rung by the ride loop on the falling edge
/// of the Sensors screen's scan mode. Without it, the latched request survives the pick and wins
/// over the fresh save at the loop top, so the save-from-scan-list flow sits through a useless 10 s
/// scan before it connects.
///
/// Deliberately no [`wake_work`] pulse: a pulse tears down whatever session the manager is in, so
/// cancelling after a Back with a healthy link up would bounce the link for nothing. A pick's save
/// request brings its own wake, and a Back lets a running window finish quietly.
pub fn cancel_scan() {
    SCAN_REQUEST.reset();
}

/// Save a sensor address to a quantity slot and (re)connect it. `quantity` is 0=HR, 1=power,
/// 2=cadence.
pub fn request_save_sensor(quantity: usize, addr: [u8; 6], random: bool) {
    SAVE_REQUEST.signal(SaveReq { quantity, addr, random });
    wake_work();
}

/// Forget the saved sensor for a quantity slot (drops any live link on the next loop pass).
pub fn request_forget_sensor(quantity: usize) {
    FORGET_REQUEST.signal(quantity);
    wake_work();
}

/// Pulse the manager's wake edge, so the manager reacts at once to a request or a radio-switch
/// change.
pub(crate) fn wake_work() {
    WORK_EDGE.signal(());
}

fn update_status(quantity: usize, f: impl FnOnce(&mut SensorSlotStatus)) {
    SLOT_STATUS.lock(|c| {
        let mut arr = c.get();
        f(&mut arr[quantity]);
        c.set(arr);
    });
}

fn saved_sensor(quantity: usize) -> Option<SavedSensor> {
    SAVED.lock(|c| c.get()[quantity])
}

fn has_dedicated_cadence_saved() -> bool {
    saved_sensor(quantity_of(SensorKind::Cadence)).is_some()
}

const fn quantity_of(kind: SensorKind) -> usize {
    match kind {
        SensorKind::HeartRate => 0,
        SensorKind::Power => 1,
        SensorKind::Cadence => 2,
    }
}

const fn kind_of(quantity: usize) -> SensorKind {
    match quantity {
        0 => SensorKind::HeartRate,
        1 => SensorKind::Power,
        _ => SensorKind::Cadence,
    }
}

/// The [`EventHandler`] that turns LE advertising reports into deduped [`SensorScanHit`]s — the only
/// path trouble-host offers for scan reports. Runs synchronously in the host rx task, and records
/// nothing unless [`SCAN_ARMED`] is set.
pub(crate) struct ScanEventHandler;

impl EventHandler for ScanEventHandler {
    // The trait methods exist only under trouble-host's `scan` feature, which the board's `ble`
    // feature always enables, and this module compiles only under `ble`. A board-crate
    // `#[cfg(feature = "scan")]` would read the board crate's feature set and drop every report.
    fn on_adv_reports(&self, reports: bt_hci::param::LeAdvReportsIter) {
        if !SCAN_ARMED.load(Ordering::Relaxed) {
            return;
        }
        for report in reports.flatten() {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(report.addr.raw());
            observe_report(addr, report.addr_kind.as_raw() & 1 == 1, report.data, report.rssi);
        }
    }

    // [`run_scan`] runs an extended scan, whose reports arrive on this event. A legacy ADV_IND from
    // a sensor is delivered here too, wrapped in `LeExtAdvReport`.
    fn on_ext_adv_reports(&self, reports: bt_hci::param::LeExtAdvReportsIter) {
        if !SCAN_ARMED.load(Ordering::Relaxed) {
            return;
        }
        for report in reports.flatten() {
            let mut addr = [0u8; 6];
            addr.copy_from_slice(report.addr.raw());
            observe_report(addr, report.addr_kind.as_raw() & 1 == 1, report.data, report.rssi);
        }
    }
}

/// Classify one advertisement and fold it into the deduped snapshot (replace-in-place by address).
fn observe_report(addr: [u8; 6], random: bool, data: &[u8], rssi: i8) {
    let Some(m) = obc_ble::classify_advertisement(data) else { return };
    let mut name = String::<16>::new();
    if let Some(n) = m.name {
        for ch in n.chars() {
            if name.push(ch).is_err() {
                break; // truncate at the 16-char cap on a char boundary
            }
        }
    }
    SCAN_HITS.lock(|c| {
        let mut hits = c.borrow_mut();
        if let Some(existing) = hits.iter_mut().find(|h| h.addr == addr) {
            existing.rssi = rssi;
            existing.kind = m.kind;
            existing.random = random;
            if !name.is_empty() {
                existing.name = name;
            }
        } else {
            let _ = hits.push(SensorScanHit { addr, random, kind: m.kind, name, rssi });
        }
    });
}

type SensorStack = Stack<'static, sdc::SoftdeviceController<'static>, DefaultPacketPool>;

/// The one central-role task, joined into [`super::run`]. Scan xor connect, never both at once, so
/// the controller never juggles a scan and a connect-initiate on one link. Never returns.
///
/// `server` is the shared GATT [`Server`]: every sensor connection attaches it, so the peer's own
/// GATT client gets answered — see [`run_link`] for why that is load-bearing.
pub async fn run(
    stack: &'static SensorStack,
    server: &'static Server<'static>,
    injector: SampleInjector<'static>,
) -> ! {
    info!("ble: [sensor] manager up (SENSOR_LINKS = {})", super::SENSOR_LINKS);

    // Hold the first pass until the host runner has finished its init sequence: this future is
    // polled before `host_task` in [`super::run`]'s join, so an immediate boot-seeded connect would
    // issue `LeCreateConn` mid host-init, and the init's resolving-list restore is spec-prohibited
    // while an initiator is active. One second covers the observed ~150 ms init.
    Timer::after_secs(1).await;

    loop {
        // Drain any stale wake pulse before reading the request latches: every request setter pulses
        // [`WORK_EDGE`], and a pulse left latched would instantly abort the very connect or scan it
        // requested, through the teardown selects below. The request latches survive the reset, and
        // every producer runs on this same thread-mode executor.
        WORK_EDGE.reset();
        apply_requests();

        // A user scan request wins — discovery is brief and interactive.
        if SCAN_REQUEST.try_take().is_some() {
            run_scan(stack).await;
            continue;
        }

        // Radio on and a sensor saved? Serve the link.
        if super::state::radio_enabled() {
            if let Some(quantity) = first_saved_quantity() {
                let interrupted = run_link(stack, server, quantity, injector).await;
                update_status(quantity, |s| s.state = SensorSlotState::Idle);
                // Back off before the next attempt, but only after a drop, failure or timeout. An
                // interrupt means a request or radio change is already waiting at the loop top, and
                // a Sensors-screen scan rung while connected must start now, not in 15 s.
                if !interrupted {
                    let _ = select(Timer::after_secs(BACKOFF_SECS), WORK_EDGE.wait()).await;
                }
                continue;
            }
        }

        // Nothing to do — park until a request or the radio switch pulses the work edge.
        WORK_EDGE.wait().await;
    }
}

fn apply_requests() {
    if let Some(req) = SAVE_REQUEST.try_take() {
        if req.quantity < QUANTITIES {
            SAVED.lock(|c| {
                let mut a = c.get();
                a[req.quantity] = Some(SavedSensor { addr: req.addr, random: req.random });
                c.set(a);
            });
            update_status(req.quantity, |s| s.saved = true);
            info!("ble: [sensor] saved quantity {} (random={})", req.quantity, req.random);
        }
    }
    if let Some(quantity) = FORGET_REQUEST.try_take() {
        if quantity < QUANTITIES {
            SAVED.lock(|c| {
                let mut a = c.get();
                a[quantity] = None;
                c.set(a);
            });
            update_status(quantity, |s| {
                s.saved = false;
                s.battery = None;
                s.state = SensorSlotState::Idle;
            });
            info!("ble: [sensor] forgot quantity {}", quantity);
        }
    }
}

/// The first saved quantity; the one sensor link serves it.
fn first_saved_quantity() -> Option<usize> {
    (0..QUANTITIES).find(|&q| saved_sensor(q).is_some())
}

/// Run one active-scan window, recording reports into [`SCAN_HITS`] via [`ScanEventHandler`].
async fn run_scan(stack: &'static SensorStack) {
    SCAN_HITS.lock(|c| c.borrow_mut().clear());
    SCAN_ARMED.store(true, Ordering::Relaxed);
    info!("ble: [sensor] scanning for {} s", SCAN_SECS);

    let mut scanner = Scanner::new(stack.central());
    let config = ScanConfig {
        active: true,
        interval: SCAN_INTERVAL,
        window: SCAN_WINDOW,
        timeout: Duration::from_secs(SCAN_SECS),
        ..Default::default()
    };
    match scanner.scan_ext(&config).await {
        Ok(_session) => {
            // The session keeps the scan enabled; reports flow through the handler. End the window
            // early if the radio switches off or a new request lands.
            let _ = select(Timer::after_secs(SCAN_SECS), WORK_EDGE.wait()).await;
            // `_session` drops here → the scan is cancelled.
        }
        Err(e) => warn!("ble: [sensor] scan start failed: {:?}", defmt::Debug2Format(&e)),
    }

    SCAN_ARMED.store(false, Ordering::Relaxed);
    // Let the scan-disable land before the loop moves on: the session's `Drop` above only queues the
    // cancel for the host runner, and the SDC refuses a create-connection while the scanner is still
    // enabled, so an immediate `connect_ext` bounces `Command Disallowed`.
    Timer::after_millis(200).await;
    let count = SCAN_HITS.lock(|c| c.borrow().len());
    info!("ble: [sensor] scan done — {} sensor(s) found", count);
}

/// Connect the saved sensor for `quantity` and serve it until it drops, or until the radio or a
/// request interrupts. The connect is bounded — dropped on timeout, which sends
/// `LeCreateConnCancel` — so an absent sensor cannot wedge the link. Returns `true` when the session
/// ended on a [`WORK_EDGE`] interrupt (skip the backoff), `false` on a drop, failure or timeout.
async fn run_link(
    stack: &'static SensorStack,
    server: &'static Server<'static>,
    quantity: usize,
    injector: SampleInjector<'static>,
) -> bool {
    let Some(saved) = saved_sensor(quantity) else { return false };
    let kind = kind_of(quantity);
    update_status(quantity, |s| s.state = SensorSlotState::Connecting);
    info!("ble: [sensor] connecting quantity {} (random={})", quantity, saved.random);

    let mut central = stack.central();
    let filter =
        [Address::new(if saved.random { AddrKind::RANDOM } else { AddrKind::PUBLIC }, BdAddr::new(saved.addr))];
    let config = ConnectConfig {
        scan_config: ScanConfig {
            active: true,
            filter_accept_list: &filter,
            interval: SCAN_INTERVAL,
            window: SCAN_WINDOW,
            ..Default::default()
        },
        // Connect fast, cruise slow: GATT runs about one ATT round trip per connection event, so
        // discovery, the battery read and the CCCD write at a relaxed interval are glacial — on
        // glass a 250–500 ms initial interval put connect-to-subscribed at 9.0 s. 30–60 ms makes the
        // chatty phase sub-second, and [`serve_link`] relaxes to [`CRUISE_PARAMS`] once subscribed.
        // The event length stays small, so the fast phase cannot starve the phone link.
        connect_params: RequestedConnParams {
            min_connection_interval: Duration::from_millis(30),
            max_connection_interval: Duration::from_millis(60),
            max_latency: 0,
            min_event_length: Duration::from_micros(0),
            max_event_length: Duration::from_millis(10),
            supervision_timeout: Duration::from_millis(5000),
        },
    };

    // `connect_ext`, NOT `connect`: the legacy `LeCreateConn` initiator faults the SDC blob the
    // moment the target's advert arrives (`SoftdeviceController: 50:701` — see the module doc).
    let conn =
        match select3(central.connect_ext(&config), Timer::after_secs(CONNECT_TIMEOUT_SECS), WORK_EDGE.wait()).await {
            Either3::First(Ok(conn)) => conn,
            Either3::First(Err(e)) => {
                warn!("ble: [sensor] connect failed: {:?}", defmt::Debug2Format(&e));
                return false;
            }
            // Timeout / interrupt: dropping the connect future cancels the create-connection.
            Either3::Second(()) => {
                info!("ble: [sensor] connect attempt timed out");
                return false;
            }
            Either3::Third(()) => {
                info!("ble: [sensor] connect attempt interrupted (radio/request)");
                return true;
            }
        };

    // Attach the shared GATT server, so the peer's own GATT client gets answered: a watch probes
    // whoever collects from it (GAP and DIS reads) right after subscribe. trouble queues inbound ATT
    // requests per connection, and only an attached attribute server drains them. With none, the
    // peer's ATT stalls for the spec's 30 s transaction timeout and the peer then hangs up. The
    // [`Server`]'s `connections_max` is sized for this.
    let conn = match conn.with_attribute_server(server) {
        Ok(conn) => conn,
        Err(e) => {
            warn!("ble: [sensor] attribute-server attach failed: {:?}", defmt::Debug2Format(&e));
            return false;
        }
    };

    // What the SDC actually granted: the interval should read 30–60 ms in the fast connect phase,
    // then flip to the cruise or peer values in the params-updated event below.
    info!("ble: [sensor] link up ({:?}), discovering", conn.raw().params());

    serve_link(stack, &conn, quantity, kind, injector).await
    // `conn` drops on return → the sensor link is disconnected.
}

/// Discover the service + measurement characteristic, read the battery once, subscribe, and pump
/// notifications — with the GATT client's rx task polled concurrently (required for notifications).
/// Returns `true` when the session ended on a [`WORK_EDGE`] interrupt (see [`run_link`]).
async fn serve_link(
    stack: &'static SensorStack,
    conn: &GattConnection<'static, 'static, DefaultPacketPool>,
    quantity: usize,
    kind: SensorKind,
    injector: SampleInjector<'static>,
) -> bool {
    let client = match GattClient::<_, _, 4>::new(stack, conn.raw()).await {
        Ok(client) => client,
        Err(e) => {
            warn!("ble: [sensor] GATT client init failed: {:?}", defmt::Debug2Format(&e));
            return false;
        }
    };

    // The discovery + notification loop. Returns `Err(())` on any GATT failure (already logged).
    let io = async {
        let service_uuid = Uuid::new_short(kind.service_uuid());
        let services = client
            .services_by_uuid(&service_uuid)
            .await
            .map_err(|e| warn!("ble: [sensor] service discovery failed: {:?}", defmt::Debug2Format(&e)))?;
        let Some(service) = services.first() else {
            warn!("ble: [sensor] service {:04x} not found on peer", kind.service_uuid());
            return Err(());
        };
        let measurement = client
            .characteristic_by_uuid::<[u8]>(service, &Uuid::new_short(kind.measurement_uuid()))
            .await
            .map_err(|e| warn!("ble: [sensor] characteristic lookup failed: {:?}", defmt::Debug2Format(&e)))?;

        // Battery Level (0x2A19) once, best-effort — a missing BAS is not an error.
        read_battery_once(&client, quantity).await;

        let mut listener = client
            .subscribe(&measurement, false)
            .await
            .map_err(|e| warn!("ble: [sensor] subscribe failed: {:?}", defmt::Debug2Format(&e)))?;

        update_status(quantity, |s| s.state = SensorSlotState::Connected);
        info!("ble: [sensor] quantity {} connected + subscribed", quantity);

        // The chatty phase is over — relax the link to the cruise cadence. Best-effort: on failure
        // the link stays fast, which is costlier but not broken, and a peer that pushes its own
        // preference through the event pump below overrides either way.
        if let Err(e) = conn.raw().update_connection_params(stack, &CRUISE_PARAMS).await {
            warn!("ble: [sensor] param relax failed: {:?}", defmt::Debug2Format(&e));
        }

        let mut cadence = obc_ble::CrankCadence::new();
        let mut notifications: u32 = 0;
        loop {
            let n = listener.next().await;
            notifications += 1;
            // Soak breadcrumbs: the first proves data flows at all, and every 32nd is dense enough
            // to bracket a drop without flooding RTT.
            if notifications == 1 || notifications.is_multiple_of(32) {
                info!("ble: [sensor] notification #{} ({} B)", notifications, n.as_ref().len());
            }
            decode_and_dispatch(kind, quantity, n.as_ref(), &mut cadence, injector);
        }
        // Unreachable, but pins the block's `Result<(), ()>` type for `?` above.
        #[allow(unreachable_code)]
        Ok(())
    };

    // The connection-event pump. Load-bearing, not bookkeeping: the sensor sends an L2CAP
    // connection-parameter-update request soon after connecting, and trouble only queues it as a
    // `ConnectionEvent`. Unanswered, Garmin-class peripherals give up and drop the link about 30 s
    // in, which reads on glass as a permanent connect/drop bounce. Accept with the peer's own
    // preferred parameters (`None`): a link of 20 B/s of notifications is happy at whatever cadence
    // the strap wants.
    let events = async {
        loop {
            match conn.next().await {
                // The peer's own GATT client, probing its collector. Serve reads from the shared
                // table and refuse writes, because the phone control plane is not commandable from a
                // sensor link. Answering something is the whole point: an unanswered request stalls
                // the peer's ATT for the spec's 30 s transaction timeout, and the peer then
                // terminates the link.
                GattConnectionEvent::Gatt { event } => {
                    let reply = match event {
                        GattEvent::Read(e) => {
                            info!("ble: [sensor] peer read, handle {}", e.handle());
                            e.accept()
                        }
                        GattEvent::Write(e) => {
                            warn!("ble: [sensor] peer write refused, handle {}", e.handle());
                            e.reject(AttErrorCode::WRITE_NOT_PERMITTED)
                        }
                        GattEvent::NotAllowed(e) => e.accept(),
                        GattEvent::Other(e) => e.accept(),
                    };
                    match reply {
                        Ok(reply) => reply.send().await,
                        Err(e) => warn!("ble: [sensor] gatt reply failed: {:?}", defmt::Debug2Format(&e)),
                    }
                }
                GattConnectionEvent::RequestConnectionParams(req) => {
                    // Log what the peer wants: if the link later drops with "remote user
                    // terminated", whether we honoured the peer's preference is the first question.
                    {
                        let p = req.params();
                        info!(
                            "ble: [sensor] peer requests conn params: interval {}..{} ms, latency {}, timeout {} ms",
                            p.min_connection_interval.as_millis(),
                            p.max_connection_interval.as_millis(),
                            p.max_latency,
                            p.supervision_timeout.as_millis()
                        );
                    }
                    if let Err(e) = req.accept(None, stack).await {
                        warn!("ble: [sensor] conn-param accept failed: {:?}", defmt::Debug2Format(&e));
                    } else {
                        info!("ble: [sensor] accepted the sensor's connection parameters");
                    }
                }
                // The on-air confirmation of any update — ours, an accept of the peer's request, or
                // a controller-initiated one. Logged so a soak capture shows the parameters the link
                // died under.
                GattConnectionEvent::ConnectionParamsUpdated {
                    conn_interval,
                    peripheral_latency,
                    supervision_timeout,
                } => {
                    info!(
                        "ble: [sensor] conn params now: interval {} ms, latency {}, timeout {} ms",
                        conn_interval.as_millis(),
                        peripheral_latency,
                        supervision_timeout.as_millis()
                    );
                }
                GattConnectionEvent::Disconnected { reason } => break reason,
                // Sensors are open GATT servers and we never pair, so any SMP activity here is a
                // peer expecting security we do not do: a prime suspect for a deliberate remote
                // disconnect.
                GattConnectionEvent::PairingFailed(e) => {
                    warn!("ble: [sensor] pairing FAILED on the sensor link: {:?}", defmt::Debug2Format(&e));
                }
                GattConnectionEvent::PairingComplete { security_level, .. } => {
                    warn!(
                        "ble: [sensor] unexpected pairing on the sensor link: {:?}",
                        defmt::Debug2Format(&security_level)
                    );
                }
                // PHY / data-length / passkey chatter — informational only on this link.
                _ => {}
            }
        }
    };

    // The GATT rx task returns `Err(Disconnected)` when the link drops; the IO block ends on a GATT
    // error; the event pump surfaces the disconnect reason; `WORK_EDGE` fires on radio-off / a new
    // request. Any of the four tears the session down.
    match select4(client.task(), io, events, WORK_EDGE.wait()).await {
        Either4::First(r) => {
            // The rx task usually wins the teardown race over the event pump, so log the actual
            // result rather than a bool. The HCI reason itself lands in trouble's own host log line.
            info!("ble: [sensor] gatt rx task ended: {:?}", defmt::Debug2Format(&r));
            false
        }
        Either4::Second(_) => false,
        Either4::Third(reason) => {
            info!("ble: [sensor] link disconnected: {:?}", defmt::Debug2Format(&reason));
            false
        }
        Either4::Fourth(()) => {
            info!("ble: [sensor] link interrupted (radio/request)");
            true
        }
    }
}

/// Read Battery Level (0x2A19) once into the slot status. Best-effort: a sensor without a BAS or a
/// failed read simply leaves `battery` unchanged.
async fn read_battery_once(
    client: &GattClient<'_, sdc::SoftdeviceController<'static>, DefaultPacketPool, 4>,
    quantity: usize,
) {
    let Ok(services) = client.services_by_uuid(&Uuid::new_short(obc_ble::UUID_BATTERY_SERVICE)).await else {
        return;
    };
    let Some(service) = services.first() else { return };
    let mut buf = [0u8; 4];
    if let Ok(len) =
        client.read_characteristic_by_uuid(service, &Uuid::new_short(obc_ble::UUID_BATTERY_LEVEL), &mut buf).await
    {
        if let Some(pct) = obc_ble::parse_battery_level(&buf[..len]) {
            update_status(quantity, |s| s.battery = Some(pct));
            info!("ble: [sensor] quantity {} battery {}%", quantity, pct);
        }
    }
}

/// Decode one measurement notification and dispatch it through the hub's [`SampleInjector`], the
/// same mailboxes the debug-uart injection path feeds. Cadence arbitration: a saved dedicated
/// cadence sensor owns cadence, otherwise the power meter's crank data fills it.
fn decode_and_dispatch(
    kind: SensorKind,
    quantity: usize,
    data: &[u8],
    cadence: &mut obc_ble::CrankCadence,
    injector: SampleInjector<'static>,
) {
    match kind {
        SensorKind::HeartRate => {
            if let Some(s) = obc_ble::parse_hr_measurement(data) {
                injector.dispatch_hr(s.bpm);
                note_value(quantity);
            }
        }
        SensorKind::Power => {
            if let Some(s) = obc_ble::parse_power_measurement(data) {
                // Signed meters can report negative (regen/coasting) — the mailbox is unsigned watts.
                injector.dispatch_power(s.watts.max(0) as u16);
                note_value(quantity);
                if let Some(crank) = s.crank {
                    if obc_ble::power_crank_feeds_cadence(has_dedicated_cadence_saved()) {
                        if let Some(rpm) = cadence.update(crank) {
                            injector.dispatch_cadence(rpm);
                            note_value(quantity_of(SensorKind::Cadence));
                        }
                    }
                }
            }
        }
        SensorKind::Cadence => {
            if let Some(s) = obc_ble::parse_csc_measurement(data) {
                if let Some(crank) = s.crank {
                    if let Some(rpm) = cadence.update(crank) {
                        injector.dispatch_cadence(rpm);
                        note_value(quantity);
                    }
                }
            }
        }
    }
}

fn note_value(quantity: usize) {
    let now = Instant::now().as_millis() as u32;
    update_status(quantity, |s| s.last_value_ms = now);
}
