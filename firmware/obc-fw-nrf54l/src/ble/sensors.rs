//! The BLE **central** sensor manager (SE6, epic #707): scan → connect → GATT-subscribe → decode →
//! dispatch for HR / power / cadence sensors, running on the same [`Stack`] as the phone-facing
//! peripheral link. One central-role task, joined into [`super::run`], driven entirely by Signal /
//! latch state in the [`super::state`] style.
//!
//! ## Where the radio-free logic lives
//!
//! The byte→struct half is `obc-ble` (host-tested, no trouble-host type): the profile parsers
//! ([`obc_ble::parse_hr_measurement`] …), the crank→rpm accumulator ([`obc_ble::CrankCadence`]), the
//! advertisement classifier ([`obc_ble::classify_advertisement`]) and the cadence arbitration
//! ([`obc_ble::power_crank_feeds_cadence`]). This file is only the radio glue: it feeds scan-report
//! and notification bytes into those, and pushes decoded values through
//! [`obc_platform::sensor_values`]'s mailboxes — the same ones the `debug-uart` `H`/`P`/`R` injection
//! path feeds (last-writer-wins), so the app can't tell a real strap from an injected line.
//!
//! ## trouble-host 0.7 API notes (verified against the vendored source)
//!
//! - **Scan reports arrive via an [`EventHandler`], not `ScanSession`.** 0.7's `ScanSession` is only
//!   a guard that keeps the scan enabled (its `Drop` cancels); the actual `LeAdvReport`s are
//!   delivered synchronously through `Runner::run_with_handler`'s handler. So [`ScanEventHandler`]
//!   parses each report here (armed by [`SCAN_ARMED`]) and [`super::host_task`] runs the host with
//!   it. `run()`'s "call repeatedly" doc comment refers to the guard, not a report method.
//! - **Extended scan + extended connect** ([`Scanner::scan_ext`] / [`Central::connect_ext`]) —
//!   **not** the legacy commands, and not by choice of wire format (sensors advertise legacy
//!   ADV_IND, which an extended scanner/initiator receives fine): the nRF54L15 SDC blob
//!   (nrfxlib 3.3.0) **faults internally** (`SoftdeviceController: 50:701`) the instant a *legacy*
//!   `LeCreateConn` initiator receives its target's advertisement — 100 % reproducible in a
//!   minimal harness (`src/bin/ble_central_repro.rs`, 2026-07-12) — while the same connect issued
//!   as `LeExtCreateConn` works. Nordic's own central-role coverage runs through Zephyr, which
//!   uses the extended commands; the legacy initiator path is the untested one (reported
//!   upstream, #736). And because legacy and extended adv/scan/initiate commands are one
//!   mutually-exclusive HCI group (Core v6 Vol 4 E 3.1.1 — first use latches the mode, the other
//!   class then bounces `Command Disallowed`), the advertiser rides the extended commands too
//!   ([`super::lifecycle`]) — same legacy PDUs on air, phones see no difference.
//! - **The `GattClient` event task must be polled concurrently with the notification loop.**
//!   [`GattClient::task`] pumps the ATT rx; without it `subscribe`/`next` never complete. We
//!   `select` the two (plus a radio/-request interrupt), so a disconnect (task returns
//!   `Err(Disconnected)`) tears the whole session down.
//! - **No sensor bonding/SMP.** Sensors are open GATT servers; we connect by stored address via the
//!   controller filter-accept-list. `BONDS_MAX` stays 1 (the phone).

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicBool, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{select, select3, Either3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::{String, Vec};
use nrf_sdc::{self as sdc};
use obc_ble::SensorKind;
use obc_platform::sensor_values::{dispatch_cadence, dispatch_hr, dispatch_power};
use trouble_host::prelude::*;

/// The three fixed sensor quantities (HR / power / cadence), one saved slot each (epic #707).
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
/// Backoff after a drop/failed attempt before retrying (epic #707 runtime policy: ~15 s), woken
/// early by any radio/request change.
const BACKOFF_SECS: u64 = 15;

// ============================ App-facing seam types (SE7 consumes) ============================
//
// This whole seam — the scan-hit / slot-status snapshots and the scan/save/forget requests — is
// built now (SE6) for SE7 (#714, the Sensors screen + saved-sensor persistence) to consume. Until
// SE7 wires it into the ride loop it is legitimately unread, so the seam carries `#[allow(dead_code)]`
// (the board crate is linted `-D warnings`); the fields are all written by the manager below.

/// A sensor discovered in a scan — the scan-list row the Sensors screen (SE7) shows and, on select,
/// saves + connects.
#[derive(Clone)]
#[allow(dead_code)] // SE7 (#714) reads these fields via `sensor_scan_hits`.
pub struct SensorScanHit {
    /// The advertiser address (little-endian, as the wire carries it).
    pub addr: [u8; 6],
    /// Whether the address is random (vs public) — needed to reconnect by the same address.
    pub random: bool,
    /// Which quantity this sensor serves (from its advertised service UUID).
    pub kind: SensorKind,
    /// The advertised local name, truncated to 16 chars (empty when the advert carried none).
    pub name: String<16>,
    /// Last-seen RSSI (dBm) — the scan list's signal indicator.
    pub rssi: i8,
}

/// The live state of one sensor slot, for the app seam.
#[derive(Clone, Copy, PartialEq, Eq, defmt::Format)]
#[allow(dead_code)] // `Scanning` is set by SE7's per-quantity scan; the rest are set here.
pub enum SensorSlotState {
    /// No sensor saved, or saved but the radio is off — nothing to do.
    Idle,
    /// A user scan is running (discovery).
    Scanning,
    /// Connecting to / rediscovering the saved sensor.
    Connecting,
    /// Connected and subscribed — notifications are flowing.
    Connected,
}

/// A per-quantity status snapshot (SE7 renders it as the Sensors-screen row; the ride loop feeds it
/// through the app the way [`super::app_ble_status`] feeds the phone link).
#[derive(Clone, Copy)]
#[allow(dead_code)] // SE7 (#714) reads these fields via `sensor_slot_status`.
pub struct SensorSlotStatus {
    /// Which quantity this slot is (HR / Power / Cadence) — fixed per index.
    pub kind: SensorKind,
    /// Whether a sensor address is saved for this quantity.
    pub saved: bool,
    /// The connection state.
    pub state: SensorSlotState,
    /// The sensor's last-read battery percent (`Some` after a connect that read 0x2A19).
    pub battery: Option<u8>,
    /// `Instant`-ms of the freshest decoded value (0 = none yet) — the app's freshness tick.
    pub last_value_ms: u32,
}

impl SensorSlotStatus {
    const fn init(kind: SensorKind) -> Self {
        Self { kind, saved: false, state: SensorSlotState::Idle, battery: None, last_value_ms: 0 }
    }
}

/// A saved sensor: the stored address to reconnect by (kind is implied by the slot index). No name
/// or bond — sensors are open GATT servers.
#[derive(Clone, Copy)]
struct SavedSensor {
    addr: [u8; 6],
    random: bool,
}

/// A save request from the app seam (SE7): the quantity slot + the address to store.
#[derive(Clone, Copy)]
struct SaveReq {
    quantity: usize,
    addr: [u8; 6],
    random: bool,
}

// ============================ Resident state (summed into RESIDENT_BYTES) ============================

type ScanHitsCell = BlockingMutex<CriticalSectionRawMutex, RefCell<Vec<SensorScanHit, MAX_SCAN_HITS>>>;
type SlotStatusCell = BlockingMutex<CriticalSectionRawMutex, Cell<[SensorSlotStatus; QUANTITIES]>>;
type SavedCell = BlockingMutex<CriticalSectionRawMutex, Cell<[Option<SavedSensor>; QUANTITIES]>>;

/// The deduped scan snapshot, written by [`ScanEventHandler`] (in the host rx path) and read by the
/// app seam. `RefCell` under a critical-section mutex — the handler is synchronous, never across an
/// `await`.
static SCAN_HITS: ScanHitsCell = BlockingMutex::new(RefCell::new(Vec::new()));
/// The per-quantity status snapshot (Copy, so a plain `Cell`).
static SLOT_STATUS: SlotStatusCell = BlockingMutex::new(Cell::new([
    SensorSlotStatus::init(SensorKind::HeartRate),
    SensorSlotStatus::init(SensorKind::Power),
    SensorSlotStatus::init(SensorKind::Cadence),
]));
/// The saved-sensor table: reconciled from the app's persisted `Settings.saved_sensors` (SE7, #714)
/// via the ride loop's per-pass diff → [`request_save_sensor`] / [`request_forget_sensor`]. Starts
/// empty; the ride loop seeds it on its first pass, so a saved sensor auto-reconnects across a reboot.
static SAVED: SavedCell = BlockingMutex::new(Cell::new([None, None, None]));

/// Whether a scan is armed — the [`ScanEventHandler`] only records reports while true, so stray
/// controller reports never pollute the snapshot.
static SCAN_ARMED: AtomicBool = AtomicBool::new(false);

/// The manager's wake edge: pulsed by every request below **and** by the #455 radio switch
/// ([`super::state::set_radio_enabled`]), so the manager reacts immediately without polling. Level +
/// coalescing — a burst wakes once and the loop re-reads the latched requests / radio level.
static WORK_EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// One-shot scan request (Sensors screen → manager).
static SCAN_REQUEST: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// One-shot save request (drained at the loop top, FORGET_BOND-style).
static SAVE_REQUEST: Signal<CriticalSectionRawMutex, SaveReq> = Signal::new();
/// One-shot forget request carrying the quantity index.
static FORGET_REQUEST: Signal<CriticalSectionRawMutex, usize> = Signal::new();

/// The manager's resident statics (see [`super::RESIDENT_BYTES`]): the scan snapshot, the slot-status
/// table, the saved table, and every request/wake `Signal` + the scan-armed `AtomicBool`. The
/// SDC/host central+scan buffers are already counted via `SDC_MEM_SIZE` / `Resources`; this is the
/// manager's own `.bss` on top. The transient `GattClient` (~0.5 KB) + its 512 B `Notification` ride
/// [`run`]'s task future (they borrow the live connection, so they can't be `.bss` statics) —
/// re-measured on glass.
///
/// **Keep this in sync:** every `static` added in this module must be summed here — it feeds the
/// compile-time budget assert (`main.rs`) that guards against the #677 stack-overflow class, all the
/// more once the LM20 profile raises `SENSOR_LINKS` to 3. `const`s hold no runtime storage and are
/// not counted.
pub const RESIDENT_BYTES: usize = core::mem::size_of::<ScanHitsCell>()
    + core::mem::size_of::<SlotStatusCell>()
    + core::mem::size_of::<SavedCell>()
    + core::mem::size_of::<AtomicBool>() // SCAN_ARMED
    + 2 * core::mem::size_of::<Signal<CriticalSectionRawMutex, ()>>() // WORK_EDGE + SCAN_REQUEST
    + core::mem::size_of::<Signal<CriticalSectionRawMutex, SaveReq>>() // SAVE_REQUEST
    + core::mem::size_of::<Signal<CriticalSectionRawMutex, usize>>(); // FORGET_REQUEST

// ============================ App-facing accessors + requests ============================

/// Read the current scan snapshot (the deduped hits) under the lock, without copying it out.
#[allow(dead_code)] // SE7 (#714): the Sensors-screen scan list.
pub fn sensor_scan_hits<R>(f: impl FnOnce(&[SensorScanHit]) -> R) -> R {
    SCAN_HITS.lock(|c| f(c.borrow().as_slice()))
}

/// The per-quantity status snapshot for the app seam (SE7). `quantity` is 0=HR, 1=power, 2=cadence.
#[allow(dead_code)] // SE7 (#714): the Sensors-screen rows.
pub fn sensor_slot_status(quantity: usize) -> SensorSlotStatus {
    SLOT_STATUS.lock(|c| c.get()[quantity.min(QUANTITIES - 1)])
}

/// Ring a one-shot scan request (Sensors screen). The manager runs a ~10 s active scan and publishes
/// the results into [`sensor_scan_hits`].
#[allow(dead_code)] // SE7 (#714): the Sensors-screen "scan" action.
pub fn request_scan() {
    SCAN_REQUEST.signal(());
    wake_work();
}

/// Save a sensor address to a quantity slot and (re)connect it. `quantity` is 0=HR, 1=power,
/// 2=cadence.
#[allow(dead_code)] // SE7 (#714): saving a picked sensor + boot seed from settings.
pub fn request_save_sensor(quantity: usize, addr: [u8; 6], random: bool) {
    SAVE_REQUEST.signal(SaveReq { quantity, addr, random });
    wake_work();
}

/// Forget the saved sensor for a quantity slot (drops any live link on the next loop pass).
#[allow(dead_code)] // SE7 (#714): the Sensors-screen hold-to-forget footer.
pub fn request_forget_sensor(quantity: usize) {
    FORGET_REQUEST.signal(quantity);
    wake_work();
}

/// Pulse the manager's wake edge. Called by the request setters above and by
/// [`super::state::set_radio_enabled`] on a radio-switch change, so the manager reacts at once.
pub(crate) fn wake_work() {
    WORK_EDGE.signal(());
}

// ============================ Status helpers ============================

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

/// The fixed slot index for a quantity.
const fn quantity_of(kind: SensorKind) -> usize {
    match kind {
        SensorKind::HeartRate => 0,
        SensorKind::Power => 1,
        SensorKind::Cadence => 2,
    }
}

/// The quantity a slot index serves.
const fn kind_of(quantity: usize) -> SensorKind {
    match quantity {
        0 => SensorKind::HeartRate,
        1 => SensorKind::Power,
        _ => SensorKind::Cadence,
    }
}

// ============================ The scan event handler ============================

/// The trouble-host [`EventHandler`] that turns LE advertising reports into deduped [`SensorScanHit`]s
/// — the only path 0.7 offers for scan reports (see the module doc). Runs synchronously in the host
/// rx task; records nothing unless [`SCAN_ARMED`] is set.
pub(crate) struct ScanEventHandler;

impl EventHandler for ScanEventHandler {
    // The trait methods exist only under trouble-host's `scan` feature — which the board's `ble`
    // feature always enables, and this module only compiles under `ble`, so the overrides are
    // unconditional here (a board-crate `#[cfg(feature = "scan")]` would wrongly read the *board*
    // crate's feature set and drop every scan report).
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

    // The live path: [`run_scan`] runs an **extended** scan (see the module doc — the whole stack
    // is on the extended command set), whose reports arrive on the extended event. A legacy
    // ADV_IND from a sensor is delivered here too, wrapped in `LeExtAdvReport`.
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

// ============================ The manager task ============================

type SensorStack = Stack<'static, sdc::SoftdeviceController<'static>, DefaultPacketPool>;

/// The one central-role task, joined into [`super::run`]. Scan **xor** connect (never both at once,
/// so the controller never juggles a scan and a connect-initiate on the single DK link), driven by
/// [`WORK_EDGE`] + the request latches. Never returns.
pub async fn run(stack: &'static SensorStack) -> ! {
    // The saved table starts empty; the ride loop seeds it from `Settings.saved_sensors` on its first
    // pass and re-pushes every change through [`request_save_sensor`] / [`request_forget_sensor`] (SE7,
    // #714 — the `set_radio_enabled` shape). The SE6 hardcoded `SEED` hook is gone: the Sensors screen
    // is the source of saved addresses now.
    info!("ble: [sensor] manager up (SENSOR_LINKS = {})", super::SENSOR_LINKS);

    // Hold the first pass until the host runner has finished its init sequence: this future is
    // polled *before* `host_task` in [`super::run`]'s join, so an immediate boot-seeded connect
    // would issue `LeCreateConn` mid host-init — and the init's resolving-list restore (the phone
    // bond) is spec-prohibited while an initiator is active. One second covers the observed
    // ~150 ms init with room, and is invisible next to a strap's advertising cadence.
    Timer::after_secs(1).await;

    loop {
        // Drain any stale wake pulse *before* reading the request latches: every request setter
        // pulses [`WORK_EDGE`], and a pulse left latched (the boot seed lands before this task's
        // first poll) would instantly abort the very connect/scan it requested via the teardown
        // selects below. The request latches survive the reset, and all producers run on this
        // same thread-mode executor, so nothing can slip between the reset and `apply_requests`.
        WORK_EDGE.reset();
        apply_requests();

        // A user scan request wins — discovery is brief and interactive.
        if SCAN_REQUEST.try_take().is_some() {
            run_scan(stack).await;
            continue;
        }

        // Radio on and a sensor saved? Serve the one DK link. (LM20, SENSOR_LINKS = 3, would run
        // `connection_worker(1)` / `(2)` beside this for the other saved slots.)
        if super::state::radio_enabled() {
            if let Some(quantity) = first_saved_quantity() {
                let interrupted = run_link(stack, quantity).await;
                update_status(quantity, |s| s.state = SensorSlotState::Idle);
                // Backoff before the next attempt — but only after a drop / failure / timeout.
                // An interrupt means a request or radio change is already waiting at the loop top
                // (its wake pulse was consumed by the teardown select), so backing off here would
                // stall it: a Sensors-screen scan rung while connected must start now, not in 15 s.
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

/// Drain the save/forget request latches into the saved table (the loop-top reconcile).
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

/// The first saved quantity (the single DK link serves it). LM20 would iterate all `SENSOR_LINKS`.
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
    // Extended scan — the stack never issues a legacy scan/adv/initiate command (module doc).
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
    let count = SCAN_HITS.lock(|c| c.borrow().len());
    info!("ble: [sensor] scan done — {} sensor(s) found", count);
}

/// Connect the saved sensor for `quantity` and serve it until it drops (or the radio/-a request
/// interrupts). A bounded connect (dropped on timeout → `LeCreateConnCancel`) keeps an absent sensor
/// from wedging the link. Returns `true` when the attempt/session ended on a [`WORK_EDGE`] interrupt
/// (a request or radio change is waiting at the loop top — skip the backoff), `false` on a
/// drop / failure / timeout (back off before retrying).
async fn run_link(stack: &'static SensorStack, quantity: usize) -> bool {
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
        // ~250–500 ms interval keeps the sensor link cheap beside the phone; 5 s supervision. The
        // connection-event length (`max_event_length` → `LeCreateConn`'s `max_ce_len`) is the radio
        // timeslot the SDC schedules per event — keep it ≤ the connection interval and small: a
        // sensor exchange (discovery, then ≤ 20 B notifications) needs only a few ms, so 30 ms is
        // ample and matches the proven phone-link event length. (An earlier 500 ms value was once
        // suspected as the `SoftdeviceController: 50:701` fault; the real cause was the missing
        // `support_dle_central`/`support_phy_update_central` — see `build_sdc` — but the cap stays.)
        connect_params: RequestedConnParams {
            min_connection_interval: Duration::from_millis(250),
            max_connection_interval: Duration::from_millis(500),
            max_latency: 0,
            min_event_length: Duration::from_micros(0),
            max_event_length: Duration::from_millis(30),
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

    serve_link(stack, &conn, quantity, kind).await
    // `conn` drops on return → the sensor link is disconnected.
}

/// Discover the service + measurement characteristic, read the battery once, subscribe, and pump
/// notifications — with the GATT client's rx task polled concurrently (required for notifications).
/// Returns `true` when the session ended on a [`WORK_EDGE`] interrupt (see [`run_link`]).
async fn serve_link(
    stack: &'static SensorStack,
    conn: &Connection<'static, DefaultPacketPool>,
    quantity: usize,
    kind: SensorKind,
) -> bool {
    let client = match GattClient::<_, _, 4>::new(stack, conn).await {
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

        let mut cadence = obc_ble::CrankCadence::new();
        loop {
            let n = listener.next().await;
            decode_and_dispatch(kind, quantity, n.as_ref(), &mut cadence);
        }
        // Unreachable, but pins the block's `Result<(), ()>` type for `?` above.
        #[allow(unreachable_code)]
        Ok(())
    };

    // The GATT rx task returns `Err(Disconnected)` when the link drops; the IO block ends on a GATT
    // error; `WORK_EDGE` fires on radio-off / a new request. Any of the three tears the session down.
    match select3(client.task(), io, WORK_EDGE.wait()).await {
        Either3::First(r) => {
            info!("ble: [sensor] link dropped: {:?}", r.is_err());
            false
        }
        Either3::Second(_) => false,
        Either3::Third(()) => {
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

/// Decode one measurement notification (SE1 parsers) and dispatch it through the shared
/// [`obc_platform::sensor_values`] mailboxes. Cadence arbitration (epic #707): a saved dedicated
/// cadence sensor owns cadence; else the power meter's crank data fills it.
fn decode_and_dispatch(kind: SensorKind, quantity: usize, data: &[u8], cadence: &mut obc_ble::CrankCadence) {
    match kind {
        SensorKind::HeartRate => {
            if let Some(s) = obc_ble::parse_hr_measurement(data) {
                dispatch_hr(s.bpm);
                note_value(quantity);
            }
        }
        SensorKind::Power => {
            if let Some(s) = obc_ble::parse_power_measurement(data) {
                // Signed meters can report negative (regen/coasting) — the mailbox is unsigned watts.
                dispatch_power(s.watts.max(0) as u16);
                note_value(quantity);
                if let Some(crank) = s.crank {
                    if obc_ble::power_crank_feeds_cadence(has_dedicated_cadence_saved()) {
                        if let Some(rpm) = cadence.update(crank) {
                            dispatch_cadence(rpm);
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
                        dispatch_cadence(rpm);
                        note_value(quantity);
                    }
                }
            }
        }
    }
}

/// Stamp a slot's freshest-value tick (the app seam's freshness indicator).
fn note_value(quantity: usize) {
    let now = Instant::now().as_millis() as u32;
    update_status(quantity, |s| s.last_value_ms = now);
}
