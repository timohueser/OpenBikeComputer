//! The remaining legacy control-plane state: RRAM-backed settings and bond/config hand-off.
//!
//! Route and trip ownership moved completely to [`crate::flat_store`]. This module stays only for
//! surfaces that have not moved yet: Config/bond state and the on-glass DFU request hand-off. Ride,
//! route, trip, transfer, and catalog ownership all live in [`crate::flat_store`].

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use obc_app::settings::DeviceName;
use obc_app::Settings;
use obc_ports::SettingsStore;

use crate::SharedStore;

/// Wakes the event-driven ride loop for the remaining BLE control-plane hand-offs (remote DFU and
/// clock stamps). Catalog movement has its own flat-store commit edge.
static STORE_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The ride loop's store-movement wake arm — resolves when a commit/delete lands after the last
/// pass. Folded into the loop's sensor-wake select (see `ride::wait_host_or_sensor_event`).
pub(crate) async fn wait_store_changed() {
    STORE_WAKE.wait().await
}

// ==================== BLE-initiated DFU install request (S6, #621) ====================
//
// The BLE→ride-loop half of the DFU seam: the `installFw` command handler ([`ble::control`]) posts an
// install request here; the ride loop drains it into the **on-glass flow** via
// [`App::open_remote_dfu_check`](obc_app::App::open_remote_dfu_check) — push the "Checking card..."
// wait + post `DfuAction::Scan`, the System menu's press arriving over the air, **never**
// `DfuAction::Install` (spec §4.4: the phone can request, only the rider installs; direct Install
// stays the physical debug link's and the confirm screen's). The drain is a *deferral*, not a
// take-and-hope: the flag is consumed only once the flow actually opened, so a request landing while
// the passkey card is up (or a DFU screen is already showing) stays pending — which also keeps
// [`dfu_install_pending`]'s `busy` answer accurate while it waits — and opens at the next drainable
// pass. Posting records **intent only**; the BLE command never waits for the human and never installs
// on its own (spec §4.4 security posture).
//
// Same lock-free module-static hand-off as the other BLE/ride-loop signals: the store lives behind
// the BLE plane's `RefCell`, the app in the ride loop. `Relaxed` suffices — both are cooperative
// futures on the one executor.

/// A BLE `installFw` request, posted but not yet drained by the ride loop.
static DFU_INSTALL_REQ: AtomicBool = AtomicBool::new(false);

/// Post a BLE-initiated install request (the `installFw` command handler). Wakes a parked ride loop
/// through the store wake ([`STORE_WAKE`]) so it drains promptly and the check → confirm flow appears
/// without waiting for an unrelated event (the #450 rescan-wake pattern — an `installFw` commit is a
/// store movement in spirit even though `/UPDATE.BIN` isn't a listed object, so it never bumps the
/// revision).
pub(crate) fn request_dfu_install_ble() {
    DFU_INSTALL_REQ.store(true, Ordering::Relaxed);
    STORE_WAKE.signal(());
}

/// Whether a BLE install request is posted but undrained — the `installFw` `busy` gate's "an install
/// request is already pending" input (read at the BLE edge, spec §4.4). Stays `true` through a
/// deferral (the ride loop consumes only once the on-glass flow opens), so a second `installFw`
/// while the first waits behind e.g. the passkey card is answered `busy`, not double-queued.
pub(crate) fn dfu_install_pending() -> bool {
    DFU_INSTALL_REQ.load(Ordering::Relaxed)
}

/// Consume the pending BLE install request — called by the ride loop **after**
/// [`App::open_remote_dfu_check`](obc_app::App::open_remote_dfu_check) returned `true` (the flow
/// opened); on a deferral the flag is left set and the drain retries next pass.
pub(crate) fn take_dfu_install_ble() -> bool {
    DFU_INSTALL_REQ.swap(false, Ordering::Relaxed)
}

// ==================== settings-coherence signals (#456) ====================
//
// Settings are the one thing both thread-mode planes edit: the ride loop (the on-device Settings
// screens) and the BLE Config write. The RRAM blob behind `SharedStore.settings` is the single
// source of truth; these two flags carry a *change* across the plane boundary so neither cache goes
// stale (and, crucially, so the ride loop's change-detection save can't clobber a BLE write).
//
// Kept at the board-crate level (a plain `AtomicBool`, host→app style like `App::set_routes`)
// deliberately independent of the P1 BLE→app event seam (#448 / obc-app) so this PR lands on its
// own; a merge-time consolidation onto that seam is fine if it ships first. `Relaxed` is enough:
// both planes are cooperative futures on the one executor, and a settings change is idempotent
// (worst case a flag is observed one pass late — a reload/refresh that changes nothing).

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

// ==================== BLE setClock → ride loop (auto-expiry epic #638 S2, #642) ====================
//
// A `setClock` command (spec §4.4 cmd 5) must not touch `App` from the BLE plane. Like the other
// crossings, the validated `(utc_unix, offset_min)` is stashed here and the ride loop drains it into
// `App::stamp_clock_ble`, which sets + persists the offset and marks the clock trusted (`ClockTrust::
// Ble`). A data-carrying `Signal` (level, last write wins — a reconnect that re-sends before the loop
// drains simply supersedes the pending value, which is exactly right) mirrors the `*_DELETE_REQ`
// idiom; the payload rides in the signal itself, so there is no torn read of two separate atomics.
// Persistence + the device→phone config-cache refresh (`DEVICE_SETTINGS_CHANGED`) then happen through
// the normal #810 settings-save path stamp_clock arms, so a Config read soon after a setClock serves
// the fresh offset — no extra coherence wiring here.

/// A validated BLE `setClock`: `(utc unix seconds, offset_min)`, posted by the command handler and
/// drained once by the ride loop. Level-coalescing — a second connect's clock supersedes an undrained
/// one (the newest phone time is the one to stamp).
static BLE_CLOCK_SET: Signal<CriticalSectionRawMutex, (u32, i16)> = Signal::new();

/// Post a validated `setClock` (the `command` handler, spec §4.4 cmd 5). Wakes a parked ride loop
/// through [`STORE_WAKE`] so the home-screen clock jumps promptly (the same wake `installFw` uses —
/// the clock is trusted device state moving, even though it is not a listed object and bumps no
/// revision).
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
    /// The persisted settings, loaded once at boot — the config plane's read/modify cache. The SD
    /// card and the RRAM store themselves are **not** owned here: they live in the shared
    /// [`SharedStore`] both planes lock, which each storage/settings method takes as a `&mut` param
    /// (#270). Keeping only this cache in `ObjectStore` lets the BLE planes hold it
    /// through a `RefCell` (never across an `await`) while the card is locked separately per call.
    settings: Settings,
}

/// The announce-time ceiling on a weather bundle's `total_len` (#1221 F6). A megabyte-scale
/// length — which no OBCW producer can mean — is refused before a byte streams, instead of being
/// streamed to the card for minutes and then failing validation.
///
impl ObjectStore {
    /// The empty control-plane cache — no settings read; [`hydrate`](Self::hydrate) fills it in
    /// place. The former catalog arrays disappeared with flat-store ownership; a const initializer
    /// keeps the remaining boot path allocation-free.
    pub const EMPTY: ObjectStore = ObjectStore { settings: Settings::DEFAULT };

    /// Mount-time fill of an [`EMPTY`](Self::EMPTY) store, **in place**: load settings only.
    pub fn hydrate(&mut self, shared: &mut SharedStore) {
        self.settings = shared.settings.load().unwrap_or_default();
    }

    // ==================== config ↔ settings ====================

    /// The current settings (the config read + the advertised-name source).
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Apply a validated Config write: persist name + units through the RRAM store. The name is stored
    /// verbatim; an empty name clears back to factory (the factory `OBC-XXXX` returns to the
    /// advertisement).
    ///
    /// The cache is updated *from the fresh RRAM blob*, not just its two edited fields: an on-device
    /// change may have landed since this cache was last synced, so re-seeding from the store keeps
    /// every field coherent (not only units + name). Then [`BLE_CONFIG_WRITTEN`] is raised so the
    /// ride loop reloads the units + name into the live `App` copy before its next save — the phone's
    /// write reaches the UI and can't be clobbered by the app's change-detection save (#456).
    pub fn apply_config(&mut self, shared: &mut SharedStore, name: &str, units: u8, weather_refresh: Option<u8>) {
        // Start from the current persisted truth so an on-device edit racing this write isn't dropped.
        self.settings = shared.settings.load().unwrap_or_default();
        self.settings.device_name = DeviceName::from_str_lossy(name);
        self.settings.units = if units == 1 { obc_app::Units::Imperial } else { obc_app::Units::Metric };
        // The §7.3 weather-refresh interval (WX8, #1193). `None` = the writer never mentioned the
        // field — *leave the stored value untouched* (spec §7.3's absent-on-write rule; this is
        // what keeps an old app's rename from resetting a rider who chose `Off`). The caller
        // already validated the byte through the wire enum's strict write direction.
        if let Some(refresh) = weather_refresh {
            // Wire-validated upstream (obc-ble's strict write direction); this converts the §11.8
            // byte into obc-app's typed representation (#1221/#1224 merge resolution).
            //
            // That conversion is only sound because the two enums agree byte-for-byte, and this
            // crate is the only one that depends on both — so the parity check lives here, as a
            // compile-time assert per interval. `COUNT` catches an interval appended on one side
            // and not the other: a new value fails the board build until it gets its row.
            #[cfg(feature = "ble")]
            const _: () = {
                use obc_app::WeatherRefresh as Stored;
                use obc_ble::WeatherRefresh as Wire;
                assert!(Stored::Off as u8 == Wire::Off.as_u8());
                assert!(Stored::Every15 as u8 == Wire::Every15.as_u8());
                assert!(Stored::Every30 as u8 == Wire::Every30.as_u8());
                assert!(Stored::Every60 as u8 == Wire::Every60.as_u8());
                assert!(Stored::Every120 as u8 == Wire::Every120.as_u8());
                assert!(Stored::COUNT == 5, "a new refresh interval needs a §11.8 parity row above");
            };
            self.settings.weather_refresh = obc_app::WeatherRefresh::from_byte(refresh);
        }
        // The BLE plane persists its owned fields directly (best-effort): the store logs a write
        // failure internally, and the phone re-asserts config on reconnect, so there is no App-side
        // revision to retry here (the device-edit path owns the acknowledged handshake — #810).
        let _ = shared.settings.save(&self.settings);
        BLE_CONFIG_WRITTEN.store(true, Ordering::Relaxed);
        // The due scheduler keys its cadence off this setting — wake it so an interval change
        // lands now rather than at the next unrelated edge (WX8).
        #[cfg(feature = "ble")]
        crate::ble::weather_settings_changed();
    }

    /// Refresh the config cache from RRAM **if** the ride loop flagged an on-device settings change
    /// ([`DEVICE_SETTINGS_CHANGED`]) — the *device → phone* half of coherence. Called by the BLE
    /// plane before it reads the config cache (a Config read, the advertised name) so a read after an
    /// on-device units/clock change serves fresh values without a reboot. Cheap when nothing changed
    /// (one relaxed load); one RRAM slice read + decode when it did.
    pub fn refresh_settings_if_changed(&mut self, shared: &mut SharedStore) {
        if DEVICE_SETTINGS_CHANGED.swap(false, Ordering::Relaxed) {
            self.settings = shared.settings.load().unwrap_or_default();
        }
    }

    // ==================== BLE bond ↔ RRAM ====================
    // The single bonded peer lives in the same RRAM settings carve as the config; these delegate
    // to the store so `ble.rs` reaches the bond through the one `RefCell<ObjectStore>` it holds.

    /// The stored bond (LTK + peer identity/IRK), or `None` for open pairing.
    pub fn load_bond(&mut self, shared: &mut SharedStore) -> Option<trouble_host::prelude::BondInformation> {
        shared.settings.load_bond()
    }

    /// Persist the single bond — a fresh pairing replaces it (single-peer policy).
    pub fn save_bond(&mut self, shared: &mut SharedStore, bond: &trouble_host::prelude::BondInformation) {
        shared.settings.save_bond(bond);
    }

    /// Forget the stored bond (the peer signalled it lost its keys) → next contact re-pairs.
    pub fn clear_bond(&mut self, shared: &mut SharedStore) {
        shared.settings.clear_bond();
    }

    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Purely presence; the full CRC scan is the on-device flow's.
    pub fn update_staged(&self, shared: &SharedStore) -> bool {
        shared.storage.as_ref().is_some_and(|s| s.has_update_bin())
    }
}
