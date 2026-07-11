//! The host→app **sensor seam** (BLE sensors epic #707, SE7): the small app-vocabulary snapshot the
//! host feeds the Sensors settings screen each pass — a per-slot connection status and, while a scan
//! runs, the live scan-hit list.
//!
//! Like the BLE link seam ([`crate::ble`]), `obc-app` stays oblivious to the radio: **no `obc-ble` or
//! board type crosses this boundary**. The board's central manager distils its per-quantity link into
//! a [`SensorStatus`] and pushes it through [`App::set_sensor_status`](crate::App::set_sensor_status);
//! a running scan's discovered sensors arrive through
//! [`App::set_sensor_scan_hits`](crate::App::set_sensor_scan_hits). The simulator feeds the same seam
//! from a fake manager, so the Sensors screen is fully sim-drivable.
//!
//! Slot indices are the fixed quantities — **0 HR · 1 Power · 2 Cadence** (see
//! [`SENSOR_SLOTS`](crate::settings::SENSOR_SLOTS)); the slot *is* the kind, so no kind enum crosses
//! the seam. A scan hit carries the slot the board resolved from the advertised service UUID, so the
//! scan-list screen filters to the quantity it is pairing.

use heapless::{String, Vec};

/// The advertised-name cap for a scan-list row — matches the board manager's own name truncation.
pub const SCAN_NAME_MAX: usize = 16;
/// The most scan hits the app holds at once — the board caps its scan snapshot the same, so the whole
/// list always fits.
pub const SCAN_HITS_MAX: usize = 8;

/// One sensor slot's connection phase, in app vocabulary — exactly the four states the Sensors
/// screen's status line renders. The host maps the board's richer link state onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SensorPhase {
    /// No sensor is saved for this quantity — the row reads `Not set`.
    #[default]
    NotSet,
    /// A sensor is saved and the manager is scanning/reconnecting for it — `Searching`.
    Searching,
    /// Saved, found, mid connect / GATT discovery — `Connecting`.
    Connecting,
    /// Connected and subscribed, values flowing — `Connected` (with the battery when known).
    Connected,
}

/// One sensor slot's live status (SE7) — the Sensors-screen row. Stored app-side per slot like the
/// BLE link's [`BleStatus`](crate::ble::BleStatus), fed each pass by the host. `Copy + Eq` so the
/// per-slot array compares in one `==` for the screen's repaint gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SensorStatus {
    /// The connection phase — also encodes "no sensor saved" as [`SensorPhase::NotSet`].
    pub phase: SensorPhase,
    /// The sensor's last-read battery percent, when known (a connected sensor that exposed the
    /// Battery Level characteristic 0x2A19). Rendered as `Connected · 78%`; absent → just `Connected`.
    pub battery: Option<u8>,
    /// Boot-relative millis of the freshest decoded value (`0` = none yet) — carried through from the
    /// board snapshot for parity; the screen can pulse a live tick on it.
    pub last_value_ms: u32,
}

impl SensorStatus {
    /// Whether a sensor is saved for this slot (any phase but [`NotSet`](SensorPhase::NotSet)) — gates
    /// the row's hold-to-forget footer.
    pub fn saved(&self) -> bool {
        self.phase != SensorPhase::NotSet
    }
}

/// A sensor discovered in a scan (SE7) — one scan-list row. App vocabulary: `slot` is the fixed
/// quantity index (0 HR · 1 Power · 2 Cadence) the board resolved from the advertised service UUID, so
/// the screen filters the list to the row it is pairing without an `obc-ble` kind crossing the seam.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensorScanHit {
    /// The quantity this sensor serves (0 HR · 1 Power · 2 Cadence).
    pub slot: u8,
    /// The advertiser address kind: `0` public / `1` random — stored so the manager reconnects by the
    /// same address kind (a broadcast Garmin watch advertises `RANDOM`).
    pub addr_kind: u8,
    /// The 6-byte advertising address, little-endian as the wire carries it.
    pub addr: [u8; 6],
    /// The advertised local name (empty → the screen shows the address instead).
    pub name: String<SCAN_NAME_MAX>,
    /// Last-seen RSSI (dBm) — the row's signal readout.
    pub rssi: i8,
}

impl SensorScanHit {
    /// Build a hit from a `&str` name (truncated to [`SCAN_NAME_MAX`] on a char boundary) — the host
    /// constructor, so a board / sim needn't touch the inner `heapless::String` (or match its version).
    pub fn new(slot: u8, addr_kind: u8, addr: [u8; 6], name: &str, rssi: i8) -> SensorScanHit {
        let mut n = String::new();
        for c in name.chars() {
            if n.push(c).is_err() {
                break;
            }
        }
        SensorScanHit { slot, addr_kind, addr, name: n, rssi }
    }
}

/// The app-resident scan-hit list — a bounded `Vec`, replaced each pass while a scan runs.
pub type SensorScanHits = Vec<SensorScanHit, SCAN_HITS_MAX>;
