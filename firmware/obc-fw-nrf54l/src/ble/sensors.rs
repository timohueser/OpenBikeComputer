//! The BLE central sensor manager: scan, connect, GATT-subscribe, decode and dispatch for HR, power
//! and cadence sensors, on the same [`Stack`] as the phone-facing peripheral link. Joined into
//! [`super::run`]: one initiator that scans and connects, and one link future per quantity that
//! serves its sensor, so all three sensors stay connected at the same time.
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
//! - One connect at a time, and never during a scan. The SDC refuses a create-connection while the
//!   scanner is enabled, and trouble-host allows one connect: a second `connect_ext` waits in a
//!   queue, and dropping it there cancels the active one. So only [`initiate`] scans or connects.
//! - The `GattClient` event task must be polled beside the notification loop. [`GattClient::task`]
//!   pumps the ATT rx; without it `subscribe`/`next` never complete. We `select` the two, plus a
//!   radio or saved-slot change, so a disconnect tears the whole session down.
//! - No sensor bonding or SMP. Sensors are open GATT servers, connected by stored address through
//!   the controller filter-accept-list. `BONDS_MAX` stays 1, for the phone.

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_futures::join::join4;
use embassy_futures::select::{select, select3, select4, Either3, Either4};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
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
/// The services a link looks up: its sensor service and the battery service. The GATT client
/// caches each one it finds, and every live link carries that cache. A second instance of either
/// service overflows it; the battery read is best-effort, so the link survives.
const LINK_SERVICES: usize = 2;
/// Deduped scan snapshot cap — enough discovered sensors to fill a scan list without unbounded RAM.
const MAX_SCAN_HITS: usize = 8;

/// One user-initiated scan window (active legacy scan). Reports drain into [`SCAN_HITS`] meanwhile.
const SCAN_SECS: u64 = 10;
/// Active-scan timing while connecting/scanning: ~60 ms interval, ~30 ms window (a 50 % duty).
const SCAN_INTERVAL: Duration = Duration::from_millis(60);
const SCAN_WINDOW: Duration = Duration::from_millis(30);
/// A single connection attempt is bounded, so the initiator duty-cycles while every missing sensor
/// is absent or asleep. On timeout the connect future is dropped (its `OnDrop` issues
/// `LeCreateConnCancel`).
const CONNECT_TIMEOUT_SECS: u64 = 20;
/// Backoff after a failed attempt, or after a link drops, before that sensor is retried.
const BACKOFF_SECS: u64 = 15;

/// Steady-state link parameters, applied by [`serve_link`] once subscribed; the connect itself uses
/// the fast params in [`connect`]. A 250–500 ms interval keeps the sensor link cheap beside the
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
#[derive(Clone, Copy, PartialEq, Eq)]
struct SavedSensor {
    addr: [u8; 6],
    random: bool,
}

impl SavedSensor {
    fn address(self) -> Address {
        Address::new(if self.random { AddrKind::RANDOM } else { AddrKind::PUBLIC }, BdAddr::new(self.addr))
    }
}

type ScanHitsCell = BlockingMutex<CriticalSectionRawMutex, RefCell<Vec<SensorScanHit, MAX_SCAN_HITS>>>;
type SlotStatusCell = BlockingMutex<CriticalSectionRawMutex, Cell<[SensorSlotStatus; QUANTITIES]>>;
type SavedCell = BlockingMutex<CriticalSectionRawMutex, Cell<[Option<SavedSensor>; QUANTITIES]>>;

/// The deduped scan snapshot, written by [`ScanEventHandler`] in the host rx path and read by the
/// app seam. The handler is synchronous, never across an `await`.
static SCAN_HITS: ScanHitsCell = BlockingMutex::new(RefCell::new(Vec::new()));
static SLOT_STATUS: SlotStatusCell =
    BlockingMutex::new(Cell::new([SensorSlotStatus::init(), SensorSlotStatus::init(), SensorSlotStatus::init()]));
/// The saved-sensor table, written by [`request_save_sensor`] and [`request_forget_sensor`] from the
/// ride loop's per-pass diff of the persisted settings. It starts empty and the ride loop seeds it
/// on the first pass, so a saved sensor reconnects across a reboot.
static SAVED: SavedCell = BlockingMutex::new(Cell::new([None, None, None]));

/// Whether a scan is armed — the [`ScanEventHandler`] only records reports while true, so stray
/// controller reports never pollute the snapshot.
static SCAN_ARMED: AtomicBool = AtomicBool::new(false);

/// The wake edges, pulsed together by every request below and by the radio switch, so the manager
/// reacts without polling. [`WORK_EDGE`] wakes [`initiate`] and [`LINK_EDGE`] wakes each link
/// future. A burst coalesces into one wake, and each future then re-reads the level it cares about:
/// the scan latch, the saved table and the radio switch.
static WORK_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static LINK_EDGE: [Signal<CriticalSectionRawMutex, ()>; QUANTITIES] = [const { Signal::new() }; QUANTITIES];
static SCAN_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();

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
    + (2 + QUANTITIES) * core::mem::size_of::<Signal<CriticalSectionRawMutex, ()>>(); // WORK_EDGE, SCAN_REQUEST, LINK_EDGE

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
/// over the fresh save at the initiator's loop top, so the save-from-scan-list flow sits through a
/// useless 10 s scan before it connects.
///
/// No [`wake_work`] pulse: a pick's save brings its own wake, and a Back lets a running window
/// finish quietly.
pub fn cancel_scan() {
    SCAN_REQUEST.reset();
}

/// Save a sensor address to a quantity slot and (re)connect it. `quantity` is 0=HR, 1=power,
/// 2=cadence.
pub fn request_save_sensor(quantity: usize, addr: [u8; 6], random: bool) {
    if quantity >= QUANTITIES {
        return;
    }
    set_saved(quantity, Some(SavedSensor { addr, random }));
    update_status(quantity, |s| s.saved = true);
    info!("ble: [sensor] saved quantity {} (random={})", quantity, random);
    wake_work();
}

/// Forget the saved sensor for a quantity slot, which drops its live link.
pub fn request_forget_sensor(quantity: usize) {
    if quantity >= QUANTITIES {
        return;
    }
    set_saved(quantity, None);
    update_status(quantity, |s| {
        s.saved = false;
        s.battery = None;
    });
    info!("ble: [sensor] forgot quantity {}", quantity);
    wake_work();
}

/// Pulse every wake edge, so the manager reacts at once to a request or a radio-switch change.
pub(crate) fn wake_work() {
    WORK_EDGE.signal(());
    for edge in &LINK_EDGE {
        edge.signal(());
    }
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

fn set_saved(quantity: usize, saved: Option<SavedSensor>) {
    SAVED.lock(|c| {
        let mut a = c.get();
        a[quantity] = saved;
        c.set(a);
    });
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

/// One quantity's link, shared by [`initiate`] and that quantity's [`hold_link`]. Local to [`run`]:
/// a `Connection` borrows the host, so it cannot sit in a `static`.
struct Slot {
    /// A connection [`initiate`] made for this quantity, with the saved entry it matched.
    handoff: Signal<NoopRawMutex, (Connection<'static, DefaultPacketPool>, SavedSensor)>,
    /// Set by [`initiate`] at the handoff, and cleared by [`hold_link`] once the link has ended and
    /// its backoff has run. While set, the initiator leaves this quantity out of its connect.
    busy: Cell<bool>,
}

/// The central role, joined into [`super::run`]: [`initiate`] beside one [`hold_link`] per
/// quantity. Never returns.
///
/// `server` is the shared GATT [`Server`]: every sensor connection attaches it, so the peer's own
/// GATT client gets answered — see [`hold_link`] for why that is load-bearing.
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

    let slots = [const { Slot { handoff: Signal::new(), busy: Cell::new(false) } }; QUANTITIES];
    join4(
        initiate(stack, &slots),
        hold_link(stack, server, &slots[0], 0, injector),
        hold_link(stack, server, &slots[1], 1, injector),
        hold_link(stack, server, &slots[2], 2, injector),
    )
    .await
    .0
}

/// The one initiator: scan xor connect, never both at once. A scan request wins, because discovery
/// is brief and interactive. Otherwise it connects every saved sensor that has no link, all in one
/// filter accept list, so the controller takes whichever sensor advertises first and an absent
/// sensor never holds up a present one. Live links stay up through both.
async fn initiate(stack: &'static SensorStack, slots: &[Slot; QUANTITIES]) -> ! {
    loop {
        // Drain any stale wake pulse before reading the levels: every request setter pulses
        // [`WORK_EDGE`], and a pulse left latched would instantly abort the very connect or scan it
        // requested, through the selects below.
        WORK_EDGE.reset();

        if SCAN_REQUEST.try_take().is_some() {
            run_scan(stack).await;
            continue;
        }

        let mut wanted: Vec<(usize, SavedSensor), QUANTITIES> = Vec::new();
        if super::state::radio_enabled() {
            for (quantity, slot) in slots.iter().enumerate() {
                if let (false, Some(saved)) = (slot.busy.get(), saved_sensor(quantity)) {
                    let _ = wanted.push((quantity, saved));
                }
            }
        }
        if wanted.is_empty() {
            // Nothing to do — park until a request, the radio switch or a free slot pulses the edge.
            WORK_EDGE.wait().await;
            continue;
        }

        for &(quantity, _) in &wanted {
            update_status(quantity, |s| s.state = SensorSlotState::Connecting);
        }
        let backoff = match connect(stack, &wanted).await {
            Some(Ok((conn, quantity, saved))) => {
                slots[quantity].busy.set(true);
                slots[quantity].handoff.signal((conn, saved));
                false
            }
            // An interrupt means a request, the radio switch or a free slot is already waiting at the
            // loop top, and a Sensors-screen scan must start now, not in 15 s.
            None => false,
            Some(Err(())) => true,
        };
        for &(quantity, _) in &wanted {
            if !slots[quantity].busy.get() {
                update_status(quantity, |s| s.state = SensorSlotState::Idle);
            }
        }
        if backoff {
            let _ = select(Timer::after_secs(BACKOFF_SECS), WORK_EDGE.wait()).await;
        }
    }
}

/// Connect the first of `wanted` that advertises, and match it back to its quantity. The connect is
/// bounded — dropped on timeout, which sends `LeCreateConnCancel`. Returns `None` on a
/// [`WORK_EDGE`] interrupt, and `Err` on a failure or a timeout.
async fn connect(
    stack: &'static SensorStack,
    wanted: &[(usize, SavedSensor)],
) -> Option<Result<(Connection<'static, DefaultPacketPool>, usize, SavedSensor), ()>> {
    let filter: Vec<Address, QUANTITIES> = wanted.iter().map(|&(_, saved)| saved.address()).collect();
    info!("ble: [sensor] connecting {} sensor(s)", filter.len());

    let mut central = stack.central();
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
                return Some(Err(()));
            }
            // Timeout / interrupt: dropping the connect future cancels the create-connection.
            Either3::Second(()) => {
                info!("ble: [sensor] connect attempt timed out");
                return Some(Err(()));
            }
            Either3::Third(()) => {
                info!("ble: [sensor] connect attempt interrupted (radio/request)");
                return None;
            }
        };

    // The accept list admits only `wanted`, but a save or forget can land while the connect runs;
    // a peer no longer wanted drops here, and its wake is already pending.
    let peer = conn.peer_address();
    let &(quantity, saved) = wanted
        .iter()
        .find(|&&(quantity, saved)| saved.addr == *peer.addr.raw() && saved_sensor(quantity) == Some(saved))?;
    Some(Ok((conn, quantity, saved)))
}

/// Serve `quantity`'s link for each connection [`initiate`] hands over, until it drops or the slot
/// is released: radio off, or the saved sensor forgotten or replaced. After a drop it backs off
/// before it frees the slot, so a peer that connects and then fails at once cannot hot-loop.
async fn hold_link(
    stack: &'static SensorStack,
    server: &'static Server<'static>,
    slot: &Slot,
    quantity: usize,
    injector: SampleInjector<'static>,
) -> ! {
    loop {
        let (conn, saved) = slot.handoff.wait().await;
        // Attach the shared GATT server, so the peer's own GATT client gets answered: a watch probes
        // whoever collects from it (GAP and DIS reads) right after subscribe. trouble queues inbound
        // ATT requests per connection, and only an attached attribute server drains them. With none,
        // the peer's ATT stalls for the spec's 30 s transaction timeout and the peer then hangs up.
        // The [`Server`]'s `connections_max` is sized for this.
        let released = match conn.with_attribute_server(server) {
            Ok(conn) => {
                // What the SDC actually granted: the interval should read 30–60 ms in the fast
                // connect phase, then flip to the cruise or peer values in the params-updated event.
                info!(
                    "ble: [sensor] quantity {} link up ({:?}), discovering",
                    quantity,
                    defmt::Debug2Format(&conn.raw().params())
                );
                serve_link(stack, &conn, quantity, saved, injector).await
                // `conn` drops here → the sensor link is disconnected.
            }
            Err(e) => {
                warn!("ble: [sensor] attribute-server attach failed: {:?}", defmt::Debug2Format(&e));
                false
            }
        };
        update_status(quantity, |s| s.state = SensorSlotState::Idle);
        if !released {
            let _ = select(Timer::after_secs(BACKOFF_SECS), released_wait(quantity, saved)).await;
        }
        slot.busy.set(false);
        WORK_EDGE.signal(());
    }
}

/// Resolves once `quantity` must let go of `saved`: the radio is off, or the slot no longer holds it.
async fn released_wait(quantity: usize, saved: SavedSensor) {
    while super::state::radio_enabled() && saved_sensor(quantity) == Some(saved) {
        LINK_EDGE[quantity].wait().await;
    }
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

/// Discover the service + measurement characteristic, read the battery once, subscribe, and pump
/// notifications — with the GATT client's rx task polled concurrently (required for notifications).
/// Returns `true` when the slot was released (skip the backoff), `false` on a drop or a failure.
async fn serve_link(
    stack: &'static SensorStack,
    conn: &GattConnection<'static, 'static, DefaultPacketPool>,
    quantity: usize,
    saved: SavedSensor,
    injector: SampleInjector<'static>,
) -> bool {
    let kind = kind_of(quantity);
    let client = match GattClient::<_, _, LINK_SERVICES>::new(stack, conn.raw()).await {
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
                info!("ble: [sensor] quantity {} notification #{} ({} B)", quantity, notifications, n.as_ref().len());
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
    // error; the event pump surfaces the disconnect reason; the slot is released on radio-off or a
    // forget or replace. Any of the four tears the session down.
    match select4(client.task(), io, events, released_wait(quantity, saved)).await {
        Either4::First(r) => {
            // The rx task usually wins the teardown race over the event pump, so log the actual
            // result rather than a bool. The HCI reason itself lands in trouble's own host log line.
            info!("ble: [sensor] quantity {} gatt rx task ended: {:?}", quantity, defmt::Debug2Format(&r));
            false
        }
        Either4::Second(_) => false,
        Either4::Third(reason) => {
            info!("ble: [sensor] quantity {} link disconnected: {:?}", quantity, defmt::Debug2Format(&reason));
            false
        }
        Either4::Fourth(()) => {
            info!("ble: [sensor] quantity {} link released (radio/forget/replace)", quantity);
            true
        }
    }
}

/// Read Battery Level (0x2A19) once into the slot status. Best-effort: a sensor without a BAS or a
/// failed read simply leaves `battery` unchanged.
async fn read_battery_once(
    client: &GattClient<'_, sdc::SoftdeviceController<'static>, DefaultPacketPool, LINK_SERVICES>,
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
