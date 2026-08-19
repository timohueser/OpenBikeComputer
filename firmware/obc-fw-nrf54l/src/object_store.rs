//! The remaining legacy control-plane state: RRAM-backed settings and the FAT ride catalog.
//!
//! Route and trip ownership moved completely to [`crate::flat_store`]. This module stays only for
//! surfaces that have not moved yet: Config/bond state, locally recorded rides and their sync/delete
//! edges, and the on-glass DFU request hand-off. It owns no route/trip catalog, identity allocator,
//! upload receiver, or per-kind delete command.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embedded_sdmmc::ShortFileName;
use heapless::Vec;
use obc_app::settings::DeviceName;
use obc_app::Settings;
use obc_ports::SettingsStore;

use crate::SharedStore;

/// Store-movement edge for the app UI (epic #447): [`bump_revision`](ObjectStore::bump_revision) —
/// the single chokepoint for every commit/delete — increments this, the same edge that notifies the
/// phone's `storeChanged`. The map plane's ride loop drains it each pass via
/// [`take_store_changed`] and rings [`App::apply_event`](obc_app::App::apply_event),
/// so the on-device catalog can react (the live rescan is P3 #450). A counter, not a flag, so a
/// burst of commits between passes is never coalesced into a single missed edge.
///
/// It lives as a module static rather than a field because the `ObjectStore` lives behind the BLE
/// planes' `RefCell` while the app lives in the ride loop — this is the lock-free hand-off between
/// them, matching the `ble::state` publish pattern.
static STORE_CHANGED: AtomicU32 = AtomicU32::new(0);

/// Drain the count of store movements since the last call (epic #447). The ride loop calls this once
/// per pass and rings `App::apply_event` that many times. `0` = nothing moved.
pub fn take_store_changed() -> u32 {
    STORE_CHANGED.swap(0, Ordering::Relaxed)
}

/// Wakes the **event-driven** ride loop on a store movement (#450): a parked device (Home, GPS
/// asleep) otherwise dozes up to the watchdog-feed cap (~12 s) before its next pass would notice
/// [`STORE_CHANGED`] — an upload from the phone should hit the Route menu now, not "eventually". A
/// coalescing `Signal` (level, not queue), like the input plane's wake: the pass drains the counter
/// whole, so one wake covers a burst.
static STORE_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The ride loop's store-movement wake arm — resolves when a commit/delete lands after the last
/// pass. Folded into the loop's sensor-wake select (see `ride::wait_host_or_sensor_event`).
pub(crate) async fn wait_store_changed() {
    STORE_WAKE.wait().await
}

/// The bounded ride-delete request channel depth.
const DELETE_CHANNEL_CAP: usize = 8;

/// On-device **ride**-delete request (epic #447, P7 / #454) — the ride-namespace twin of
/// the removed route channel. The ride loop's Rides-menu hold posts a ride's durable object id; the BLE
/// plane drains it and runs [`ObjectStore::delete_ride`], so an on-device ride delete goes through
/// the same catalog + revision + `storeChanged` path a phone-initiated delete does.
/// A bounded [`Channel`] (finding #876-3): the ride retention sweep can
/// discover several synced+aged rides at once, and the old overwriting `Signal` lost all but the
/// newest. Same contract as the route channel: never an overwrite, but a full channel **drops** the
/// post — end-to-end losslessness rests on the app's retain-until-rescan retry, not on observed
/// backpressure. Shared by manual (Rides-menu hold) and retention ride deletes.
static RIDE_DELETE_REQ: Channel<CriticalSectionRawMutex, u16, DELETE_CHANNEL_CAP> = Channel::new();

/// Post a ride-delete request from the ride loop (epic #447, P7). Returns `false` when the channel
/// is full and the id was **dropped** — the caller warns, and the app's retained candidate retries.
pub(crate) fn request_ride_delete(id: u16) -> bool {
    RIDE_DELETE_REQ.try_send(id).is_ok()
}

/// The BLE plane's ride-delete arm: resolves with the next ride id to delete once the ride loop posts one.
pub(crate) async fn wait_ride_delete() -> u16 {
    RIDE_DELETE_REQ.receive().await
}

/// A locally-finished ride committed its `RD{id}.ORD` (the ride loop drained
/// [`Storage::take_ride_saved`](crate::sd::Storage::take_ride_saved)). The BLE plane drains this and
/// runs [`ObjectStore::adopt_saved_rides`] — the `/tracks` rescan + revision bump — so **one edge**
/// feeds every consumer exactly like an upload or delete does: the phone gets `storeChanged(ride)` +
/// the fresh digest, and the resulting [`STORE_CHANGED`] edge re-feeds the on-device Rides menu next
/// pass. Without it a finished ride was invisible everywhere until a reboot (the boot scan was the
/// only thing that ever read `/tracks` into either catalog). A coalescing `Signal<()>` — the drain
/// rescans the whole directory, so a burst of saves needs no queue and no payload.
static RIDE_SAVED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Post the saved-ride edge from the ride loop.
pub(crate) fn note_ride_saved() {
    RIDE_SAVED.signal(());
}

/// The BLE plane's saved-ride arm: resolves once a ride object lands after the last drain.
pub(crate) async fn wait_ride_saved() {
    RIDE_SAVED.wait().await
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
// Same lock-free module-static hand-off as [`STORE_CHANGED`]: the store lives behind the BLE plane's
// `RefCell`, the app in the ride loop. `Relaxed` suffices — both are cooperative futures on the one
// executor.

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

/// One ride-catalog slot: the object id and where its bytes live.
struct ObjectSlot {
    id: u16,
    file: ShortFileName,
}

/// Ride catalog capacity. Past it the newest rides stop being listed until the card is tidied.
pub const MAX_RIDES: usize = 128;

pub struct ObjectStore {
    /// The persisted settings, loaded once at boot — the config plane's read/modify cache. The SD
    /// card and the RRAM store themselves are **not** owned here: they live in the shared
    /// [`SharedStore`] both planes lock, which each storage/settings method takes as a `&mut` param
    /// (#270). Keeping only the catalog + this cache in `ObjectStore` lets the BLE planes hold it
    /// through a `RefCell` (never across an `await`) while the card is locked separately per call.
    settings: Settings,
    /// The stored rides: scanned at boot and re-scanned on the saved-ride edge ([`RIDE_SAVED`]) —
    /// since the de-split the `ble` build *is* the map build, so the ride loop records new rides
    /// mid-session and this catalog must follow (it feeds the `rideList` object the phone syncs
    /// against).
    rides: Vec<ObjectSlot, MAX_RIDES>,
    /// The ride store revision: monotonic per boot, bumped on every ride commit/delete.
    revision: u32,
    /// Full ride-catalog size before the [`MAX_RIDES`] cap — the `rideList` header's `total`.
    ride_total: u16,
}

/// The announce-time ceiling on a weather bundle's `total_len` (#1221 F6). A megabyte-scale
/// length — which no OBCW producer can mean — is refused before a byte streams, instead of being
/// streamed to the card for minutes and then failing validation.
///
impl ObjectStore {
    /// The empty store — no settings read, no card scan; [`hydrate`](Self::hydrate) does that,
    /// in place. Construction is split in two because this struct is ~13.5 KB by value: the old
    /// `new(shared)`-then-`RefCell::new` shape put **two** copies of it in `link::init_store`'s
    /// frame (the return slot + the wrapper's argument), the measured ~27.6 KB boot spike that
    /// overran the residual stack once EL7 grew the ride task's poll frame (STKOF HardFault at
    /// the `init_store` prologue, 2026-08-03). Since WX12 (#1197) the empty store is a **`const`**
    /// — a `.rodata` image the slot write copies from — because even the one by-value hop proved
    /// optimizer-fragile: a +96 B `Settings` growth was enough for rustc 1.96 to stop collapsing
    /// `RefCell::new(empty())` and stack the two ~13.6 KB temporaries again (the boot-chain guard
    /// caught it, as designed). A constant can't be duplicated onto the stack; everything that
    /// scans stays in [`hydrate`], operating on the slot directly.
    pub const EMPTY: ObjectStore =
        ObjectStore { settings: Settings::DEFAULT, rides: Vec::new(), revision: 1, ride_total: 0 };

    /// Mount-time fill of an [`EMPTY`](Self::EMPTY) store, **in place**: load settings and scan the
    /// legacy ride catalog. Route and trip objects are owned exclusively by the flat store.
    pub fn hydrate(&mut self, shared: &mut SharedStore) {
        self.settings = shared.settings.load().unwrap_or_default();
        self.rescan_rides(shared);
    }

    /// Scan `/tracks` for stored ride objects (`RD{id}.ORD`) whose id is durable in the filename.
    /// An interrupted save (the held-back version byte, exactly that signature) is swept; a merely
    /// unreadable file is kept off the catalog but never deleted. Ordered as the directory lists
    /// them; the app sorts by `start_time`.
    fn rescan_rides(&mut self, shared: &mut SharedStore) {
        self.rides.clear();
        self.ride_total = 0;
        let Some(storage) = &mut shared.storage else { return };
        let mut entries: Vec<(u16, ShortFileName), MAX_RIDES> = Vec::new();
        // Count the excess the cap drops (epic #632 item 7) rather than boolean-flagging it, so
        // the `rideList` header's `total` makes the truncation visible on the wire.
        let mut over_cap: u16 = 0;
        storage.for_each_ride_file(|id, n| {
            if entries.push((id, n.clone())).is_err() {
                over_cap = over_cap.saturating_add(1);
            }
        });
        if over_cap > 0 {
            defmt::warn!("store: more than {=usize} ride objects — {=u16} not listed", MAX_RIDES, over_cap);
        }
        for (id, name) in &entries {
            match storage.ride_object_info(name) {
                Some(_) => {
                    let _ = self.rides.push(ObjectSlot { id: *id, file: name.clone() });
                }
                None => {
                    if storage.is_aborted_ride_object(name) {
                        defmt::info!("store: sweeping interrupted ride save {}", defmt::Debug2Format(name));
                        let _ = storage.delete_ride_file(name);
                    }
                }
            }
        }
        self.ride_total = (self.rides.len() as u16).saturating_add(over_cap);
        defmt::info!("store: {=usize} ride object(s)", self.rides.len());
    }

    /// The current store revision — monotonic per boot, bumped on every commit/delete. The BLE plane
    /// stamps it into the `storeChanged` status message (protocol v2's sole change signal — the
    /// `objectStore` digest characteristic is retired).
    pub fn revision(&self) -> u32 {
        self.revision
    }

    fn bump_revision(&mut self) -> u32 {
        self.revision = self.revision.wrapping_add(1);
        // Signal the app UI (epic #447): every commit/delete funnels through here, so this is the one
        // spot that raises the store-changed edge the ride loop drains — the same movement that
        // notifies the phone's `storeChanged` — and wakes the loop if it's parked (#450).
        STORE_CHANGED.fetch_add(1, Ordering::Relaxed);
        STORE_WAKE.signal(());
        self.revision
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

    // ==================== delete ====================

    /// Delete a stored **ride** by object id (epic #447, P7 / #454) — the ride-namespace twin of
    /// [`delete_route`](Self::delete_route). Routes the delete through the store (revision bump +
    /// `storeChanged`) so the phone's device-rides reconcile; retires the ride's synced-set flag too.
    /// `true` = deleted. Ids never reuse, so the phone's synced/tombstone bookkeeping stays coherent.
    pub fn delete_ride(&mut self, shared: &mut SharedStore, id: u16) -> bool {
        let Some(idx) = self.rides.iter().position(|s| s.id == id) else { return false };
        let Some(storage) = &mut shared.storage else { return false };
        if !storage.delete_ride_file(&self.rides[idx].file) {
            return false;
        }
        // Retire the synced flag (belt-and-braces — ids never reuse) so the sidecar stays tidy.
        storage.forget_ride_synced(id);
        self.rides.remove(idx);
        self.ride_total = self.ride_total.saturating_sub(1);
        self.bump_revision();
        true
    }

    /// Reconcile the synced sidecar from the phone's possession ack (`ackRides`, spec §4.4 cmd 2):
    /// flag every acked id **the device still stores** as synced — the phone's library is the ground
    /// truth for "the phone has this ride", so this heals every divergence the download-completion
    /// event alone leaves permanent (rides downloaded before the sidecar existed, a sidecar lost
    /// with the card, an app reinstall). Monotonic: nothing is ever un-flagged here. One sidecar
    /// read-modify-write for the whole batch; a change bumps the revision once, so the ride loop's
    /// `STORE_CHANGED` rescan re-feeds the Rides menu with the freshened flags (same funnel as a
    /// download-completion mark). Returns the newly-flagged count (the `commandResult.detail`,
    /// saturating at 255).
    pub fn ack_rides(&mut self, shared: &mut SharedStore, ack: &obc_ble::AckRides) -> u8 {
        let Some(storage) = &mut shared.storage else { return 0 };
        let rides = &self.rides;
        // `synced_at = 0` (auto-expiry epic #638, S3): the BLE plane here has no trusted-clock
        // handle, so the ride is flagged synced now with an unset stamp. S2's `setClock` precedes
        // `ackRides` on every connect, so the clock is trusted in practice — and the app's **eager**
        // ride-stamp step (`RetentionRuntime::stamp_synced_rides`, run every trusted tick, *not*
        // recording-gated) stamps `synced_at = now` at ~ack-time once the store-changed rescan
        // re-feeds this flag. An old app that never sends `setClock` leaves the clock untrusted and
        // the stamp waits for the first trusted tick — the lazy fallback (invariant 5: a
        // synced-without-timestamp ride is never deleted on sight, its countdown just starts later).
        let added = storage.mark_rides_synced(ack.iter().filter(|id| rides.iter().any(|s| s.id == *id)), 0);
        if added > 0 {
            self.bump_revision();
        }
        added.min(u8::MAX as usize) as u8
    }

    /// Adopt locally-saved rides into the live catalog: re-scan `/tracks` and bump the revision, so
    /// the phone's `storeChanged(ride)` + digest and the ride loop's [`STORE_CHANGED`] edge (→ the
    /// Rides menu re-feed) all move from this one edge — the exact path an upload commit or a delete
    /// takes. Driven by [`wait_ride_saved`] in `ble::run`'s `ride_saved_task`.
    pub fn adopt_saved_rides(&mut self, shared: &mut SharedStore) {
        self.rescan_rides(shared);
        self.bump_revision();
    }

    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Purely presence; the full CRC scan is the on-device flow's.
    pub fn update_staged(&self, shared: &SharedStore) -> bool {
        shared.storage.as_ref().is_some_and(|s| s.has_update_bin())
    }
}
