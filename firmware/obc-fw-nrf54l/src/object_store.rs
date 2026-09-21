//! The remaining legacy control-plane state: RRAM-backed settings and the bond and config hand-off.
//!
//! Route, trip, ride, transfer and catalog ownership all live in [`crate::flat_store`]. This module
//! stays only for the surfaces that have not moved: config and bond state, and the on-glass DFU
//! request hand-off.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_app::settings::DeviceName;
use obc_app::Settings;
use obc_ports::SettingsStore;

use crate::SharedStore;

/// Wakes the event-driven ride loop for the remaining BLE control-plane hand-offs. Catalog movement
/// has its own flat-store commit edge.
static STORE_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The ride loop's store-movement wake arm: it resolves when a commit or delete lands after the
/// last pass.
pub(crate) async fn wait_store_changed() {
    STORE_WAKE.wait().await
}

// The BLE-to-ride-loop half of the DFU seam. The `installFw` command handler posts an install
// request here, and the ride loop drains it into the on-glass flow: push the "Checking card..."
// wait and post `DfuAction::Scan`, which is the System menu's press arriving over the air, and never
// `DfuAction::Install` — the phone can request, only the rider installs. The drain is a deferral,
// not a take-and-hope: the flag is consumed only once the flow actually opened, so a request landing
// while the passkey card is up stays pending, which also keeps [`dfu_install_pending`]'s `busy`
// answer accurate while it waits. Posting records intent only.
//
// `Relaxed` is enough: both sides are cooperative futures on the one executor.

/// A BLE `installFw` request, posted but not yet drained by the ride loop.
static DFU_INSTALL_REQ: AtomicBool = AtomicBool::new(false);

/// Post a BLE-initiated install request. It wakes a parked ride loop through [`STORE_WAKE`], so the
/// check and confirm flow appears without waiting for an unrelated event.
pub(crate) fn request_dfu_install_ble() {
    DFU_INSTALL_REQ.store(true, Ordering::Relaxed);
    STORE_WAKE.signal(());
}

/// Whether a BLE install request is posted but undrained: the `installFw` busy gate's "an install
/// request is already pending" input. It stays `true` through a deferral, so a second `installFw`
/// while the first waits is answered busy rather than double-queued.
pub(crate) fn dfu_install_pending() -> bool {
    DFU_INSTALL_REQ.load(Ordering::Relaxed)
}

/// Consume the pending BLE install request. The ride loop calls it after the on-glass flow opened;
/// on a deferral the flag is left set and the drain retries next pass.
pub(crate) fn take_dfu_install_ble() -> bool {
    DFU_INSTALL_REQ.swap(false, Ordering::Relaxed)
}

// Settings are the one thing both thread-mode planes edit: the ride loop, through the on-device
// Settings screens, and the BLE Config write. The RRAM blob behind `SharedStore.settings` is the
// single source of truth, and these two flags carry a change across the plane boundary so neither
// cache goes stale, and so the ride loop's change-detection save cannot clobber a BLE write.
//
// `Relaxed` is enough: both planes are cooperative futures on the one executor, and a settings
// change is idempotent, so the worst case is a flag observed one pass late.

/// Raised by a BLE Config write ([`ObjectStore::apply_config`]); the ride loop drains it and
/// reloads the BLE-owned fields (units + name) into the live `App` settings **before** its next
/// change-detection save, so the phone's write reaches the UI same-session and is never clobbered.
static BLE_CONFIG_WRITTEN: AtomicBool = AtomicBool::new(false);

/// Raised by the ride loop after it persists an on-device settings change; the BLE plane drains it
/// and refreshes the [`ObjectStore`] config cache from RRAM before serving a Config read (or the
/// advertised name), so a read after an on-device units change is fresh without a reboot.
static DEVICE_SETTINGS_CHANGED: AtomicBool = AtomicBool::new(false);

/// The ride loop's cue to reload BLE-written settings before its next save (see
/// [`BLE_CONFIG_WRITTEN`]). `true` at most once per BLE Config write; drains on read.
pub(crate) fn take_ble_config_written() -> bool {
    BLE_CONFIG_WRITTEN.swap(false, Ordering::Relaxed)
}

/// The ride loop signals that it persisted an on-device settings edit, so the BLE plane's config
/// cache is now stale (see [`DEVICE_SETTINGS_CHANGED`]). Cheap: one relaxed store per settings save.
pub(crate) fn mark_device_settings_changed() {
    DEVICE_SETTINGS_CHANGED.store(true, Ordering::Relaxed);
}

// A `setClock` command must not touch `App` from the BLE plane. Like the other crossings, the
// validated `(utc_unix, offset_min)` is stashed here and the ride loop drains it into
// `App::stamp_clock_ble`, which sets and persists the offset and marks the clock trusted. A
// data-carrying `Signal` is level and last-write-wins, so a reconnect that re-sends before the loop
// drains supersedes the pending value, and the payload rides in the signal itself, so there is no
// torn read of two separate atomics.

/// A validated BLE `setClock`, posted by the command handler and drained once by the ride loop. It
/// coalesces: a second connect's clock supersedes an undrained one.
static BLE_CLOCK_SET: Signal<CriticalSectionRawMutex, (u32, i16)> = Signal::new();

/// Post a validated `setClock`. It wakes a parked ride loop through [`STORE_WAKE`], so the home
/// screen's clock jumps promptly.
pub(crate) fn post_ble_clock(utc: u32, offset_min: i16) {
    BLE_CLOCK_SET.signal((utc, offset_min));
    STORE_WAKE.signal(());
}

/// The ride loop's cue to stamp the wall clock from a BLE `setClock`, or `None` when none is pending
/// (drains on read).
pub(crate) fn take_ble_clock() -> Option<(u32, i16)> {
    BLE_CLOCK_SET.try_take()
}

pub struct ObjectStore {
    /// The persisted settings, loaded once at boot: the config plane's read and modify cache. The
    /// card and the RRAM store are not owned here — they live in the shared [`SharedStore`] both
    /// planes lock, which each method takes as a parameter. Keeping only this cache here lets the
    /// BLE planes hold it through a `RefCell`, never across an `await`, while the card is locked
    /// separately per call.
    settings: Settings,
}

impl ObjectStore {
    /// The empty control-plane cache, with no settings read; [`hydrate`](Self::hydrate) fills it in
    /// place. A const initializer keeps the boot path allocation-free.
    pub const EMPTY: ObjectStore = ObjectStore { settings: Settings::DEFAULT };

    /// Mount-time fill of an [`EMPTY`](Self::EMPTY) store, in place: load settings only.
    pub fn hydrate(&mut self, shared: &mut SharedStore) {
        self.settings = shared.settings.load().unwrap_or_default();
    }


    /// The current settings (the config read + the advertised-name source).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Apply a validated Config write: persist the name and units through the RRAM store. The name
    /// is stored verbatim, and an empty name clears back to the factory name.
    ///
    /// The cache is updated from the fresh RRAM blob rather than from its two edited fields, because
    /// an on-device change may have landed since this cache was last synced. Then
    /// [`BLE_CONFIG_WRITTEN`] is raised, so the ride loop reloads the units and name into the live
    /// `App` copy before its next save and the phone's write cannot be clobbered.
    pub fn apply_config(&mut self, shared: &mut SharedStore, name: &str, units: u8) {
        // Start from the current persisted truth so an on-device edit racing this write isn't dropped.
        self.settings = shared.settings.load().unwrap_or_default();
        self.settings.device_name = DeviceName::from_str_lossy(name);
        self.settings.units = if units == 1 { obc_app::Units::Imperial } else { obc_app::Units::Metric };

        // The BLE plane persists its owned fields directly, best-effort: the store logs a write
        // failure internally and the phone re-asserts config on reconnect, so there is no App-side
        // revision to retry here.
        let _ = shared.settings.save(&self.settings);
        BLE_CONFIG_WRITTEN.store(true, Ordering::Relaxed);
    }

    /// Refresh the config cache from RRAM if the ride loop flagged an on-device settings change:
    /// the device-to-phone half of coherence. The BLE plane calls it before it reads the config
    /// cache, so a read after an on-device change serves fresh values without a reboot. It is one
    /// relaxed load when nothing changed.
    pub fn refresh_settings_if_changed(&mut self, shared: &mut SharedStore) {
        if DEVICE_SETTINGS_CHANGED.swap(false, Ordering::Relaxed) {
            self.settings = shared.settings.load().unwrap_or_default();
        }
    }

    // The single bonded peer lives in the same RRAM settings carve as the config. These delegate to
    // the store, so the BLE plane reaches the bond through the one `RefCell<ObjectStore>` it holds.

    /// The stored bond (LTK + peer identity/IRK), or `None` for open pairing.
    pub fn load_bond(&mut self, shared: &mut SharedStore) -> Option<trouble_host::prelude::BondInformation> {
        shared.settings.load_bond()
    }

    /// Persist the single bond — a fresh pairing replaces it (single-peer policy).
    pub fn save_bond(&mut self, shared: &mut SharedStore, bond: &trouble_host::prelude::BondInformation) {
        shared.settings.save_bond(bond);
    }

    /// Forget the stored bond, because the peer signalled it lost its keys, so the next contact
    /// re-pairs.
    pub fn clear_bond(&mut self, shared: &mut SharedStore) -> Result<(), obc_app::ble::BondError> {
        shared.settings.clear_bond()
    }

    /// Whether a staged `/UPDATE.BIN` exists in the card root: the `installFw` cheap existence
    /// check. Presence only; the full CRC scan is the on-device flow's.
    pub fn update_staged(&self, shared: &SharedStore) -> bool {
        shared.storage.as_ref().is_some_and(|s| s.has_update_bin())
    }
}
