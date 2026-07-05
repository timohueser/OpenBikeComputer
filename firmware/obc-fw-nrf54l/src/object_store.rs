//! The device object store — the board half that turns the object plane into SD files and RRAM
//! settings. `obc-ble` owns the wire (descriptors, CRC, transfer sequencing); [`crate::sd::Storage`]
//! owns FatFs; this module owns the **catalog semantics** between them:
//!
//! - **Object ids**: `u16`, **durable for uploaded objects** — the id is encoded in the SD filename
//!   (`RT{id}.OBR`, see `sd.rs`), recovered at the mount scan, and fresh ids continue monotonically
//!   past `max(highest stored one + 1, the persisted RRAM high-water floor)` (#450) — so an id is
//!   **never reused**, even after a delete drops the highest stored file and the device reboots.
//!   Durability matters because the phone persists the id an upload
//!   commits under (`PlannedRouteRecord.deviceObjectID`) and uses it to badge-reconcile and
//!   replace-in-place across device reboots. Side-loaded `.obcr` files carry no id in their name and
//!   get a *session-scoped* one from the reserved [`SIDELOAD_ID_BASE`] band, handed out by the
//!   registry in `sd.rs` that the ride loop's catalog scan shares (identical ids in both tables) —
//!   the app never persists those.
//! - **Store revision + digest**: bumped on every commit/delete; the BLE plane notifies `storeChanged`
//!   + the digest characteristic from it.
//! - **The upload state machine**: descriptor → [`Receiver`] (+ temp-file sink) → commit. Uploads are
//!   not resumable: an interrupted upload (a drop or an `op=3` abort) is discarded and the app re-sends
//!   the object from the start.
//! - **Downloads**: the `routeList` / `rideList` objects are built into a resident buffer; a route or
//!   ride detail is served straight off the card (CRC pre-pass, then chunk reads — a stored
//!   `RD{id}.ORD` *is* the wire object, so a ride download is verbatim).
//! - **Rides are read-only here**: recorded by the map build's ride loop, scanned once at boot, never
//!   mutated over the link — the device retains them until a future device-side delete, and the app
//!   hides synced rides locally instead of deleting them.
//! - **Config ↔ settings**: the Config blob reads from / writes through the persisted [`Settings`]
//!   (`device_name` + `units`), so a rename survives a power cycle and feeds the advertised name.
//!
//! Everything here is synchronous SD I/O. The SD card + RRAM store are **not** owned here — they
//! live in the shared [`crate::SharedStore`] (the async mutex the map plane's ride loop also locks,
//! #270), passed as a `&mut SharedStore` into each storage/settings method; a BLE plane locks it per
//! call and drops the guard before its next `await`. `ObjectStore` itself (catalog + settings cache)
//! stays behind a `RefCell` the BLE planes borrow **never across an `await`** (single executor).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embedded_sdmmc::ShortFileName;
use heapless::Vec;
use obc_app::settings::DeviceName;
use obc_app::{Settings, SettingsStore, MAX_ROUTES};
use obc_ble::{
    Crc32, ListHeader, ObjectStoreDigest, ObjectType, Receiver, RideListEntry, RouteListEntry, StreamSender,
    TransferControl, TransferStatus,
};

use crate::sd::Storage;
use crate::SharedStore;

/// Store-movement edge for the app UI (epic #447): [`bump_revision`](ObjectStore::bump_revision) —
/// the single chokepoint for every commit/delete — increments this, the same edge that notifies the
/// phone's `storeChanged`. The map plane's ride loop drains it each pass via
/// [`take_store_changed`] and rings [`App::notify_store_changed`](obc_app::App::notify_store_changed),
/// so the on-device catalog can react (the live rescan is P3 #450). A counter, not a flag, so a
/// burst of commits between passes is never coalesced into a single missed edge.
///
/// It lives as a module static rather than a field because the `ObjectStore` lives behind the BLE
/// planes' `RefCell` while the app lives in the ride loop — this is the lock-free hand-off between
/// them, matching the `ble::state` publish pattern.
static STORE_CHANGED: AtomicU32 = AtomicU32::new(0);

/// Drain the count of store movements since the last call (epic #447). The ride loop calls this once
/// per pass and rings `App::notify_store_changed` that many times. `0` = nothing moved.
pub fn take_store_changed() -> u32 {
    STORE_CHANGED.swap(0, Ordering::Relaxed)
}

/// The latest **committed route upload** for the app UI (epic #447, P4): which durable id landed
/// and whether it replaced a stored slot — what the store-changed edge alone can't say, and what
/// the upload popups key their variant on. Packed `present-bit | replaced-bit | id` (`0` = none);
/// deliberately a **single latest-wins slot**, which *is* the locked popup rule (consecutive
/// uploads replace the prompt, most recent wins), so a burst between passes needs no queue.
///
/// Published by [`ObjectStore::upload_finish`] **before** its revision bump, so the pass the
/// [`STORE_WAKE`] pulls out of warm sleep sees the rescan edge and this event together; the ride
/// loop drains the rescan **first** (the id must resolve against the fresh catalog), then rings
/// [`App::notify_route_uploaded`](obc_app::App::notify_route_uploaded). Same module-static
/// hand-off pattern as [`STORE_CHANGED`] — the store lives behind the BLE planes' `RefCell`, the
/// app in the ride loop; both are cooperative tasks on the one executor, so `Relaxed` suffices.
static UPLOAD_EVENT: AtomicU32 = AtomicU32::new(0);
const UPLOAD_EVENT_PRESENT: u32 = 1 << 17;
const UPLOAD_EVENT_REPLACED: u32 = 1 << 16;

/// Drain the latest committed route upload since the last call: `(object_id, replaced_existing)`,
/// or `None` when nothing landed. The ride loop calls this once per pass, strictly *after* it has
/// serviced [`take_store_changed`] (see [`UPLOAD_EVENT`]).
pub(crate) fn take_route_uploaded() -> Option<(u16, bool)> {
    let v = UPLOAD_EVENT.swap(0, Ordering::Relaxed);
    (v & UPLOAD_EVENT_PRESENT != 0).then_some((v as u16, v & UPLOAD_EVENT_REPLACED != 0))
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

/// On-device route-delete request (epic #447, P6): the durable object id of a route the Route menu's
/// hold-to-delete footer asked to remove. The **ride loop** posts it (it drains the app's
/// `take_route_delete`), and the **BLE plane** — the sole owner of the `RefCell<ObjectStore>` —
/// executes it via [`ObjectStore::delete_route`], so the delete goes through the same catalog +
/// revision + `storeChanged` path a phone-initiated delete does (never raw SD). A coalescing
/// `Signal` (level, one id in flight) rather than a queue: the footer fires one delete at a time and
/// the drain runs promptly, so a second request can't stack up behind the first.
///
/// It lives as a module static, like [`STORE_CHANGED`], because the `ObjectStore` is trapped behind
/// the BLE task's `RefCell` while the app lives in the ride loop — this is the lock-free hand-off
/// from the loop that owns the *intent* to the plane that owns the *store*.
static ROUTE_DELETE_REQ: Signal<CriticalSectionRawMutex, u16> = Signal::new();

/// Post a route-delete request from the ride loop (epic #447, P6) — the BLE plane drains it and runs
/// the `ObjectStore` delete. Overwrites any un-drained request (one delete in flight at a time).
pub(crate) fn request_route_delete(id: u16) {
    ROUTE_DELETE_REQ.signal(id);
}

/// The BLE plane's route-delete arm: resolves with the id to delete once the ride loop posts one.
/// Folded into the BLE lifetime `join` so it drains whether the phone is connected or the device is
/// parked advertising (see `ble::run`).
pub(crate) async fn wait_route_delete() -> u16 {
    ROUTE_DELETE_REQ.wait().await
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

/// One catalog slot: the object id and where its bytes live (routes and rides alike).
struct ObjectSlot {
    id: u16,
    file: ShortFileName,
    byte_len: u32,
}

/// Ride catalog capacity. Rides accumulate — the device keeps every tracked ride until a (future)
/// manual delete — so this is roomier than [`MAX_ROUTES`]; past it the newest rides stop being listed
/// (warned at scan) until the card is tidied.
pub const MAX_RIDES: usize = 128;

/// The list-object buffer: header + one entry per slot of the **larger** catalog (both lists stream
/// from the same scratch — one transfer at a time).
const LIST_BUF_LEN: usize = ListHeader::object_len(if MAX_RIDES > MAX_ROUTES { MAX_RIDES } else { MAX_ROUTES });

// The side-load id band base lives in `sd.rs` beside the session registry both scanners share
// (the ride loop's catalog scan assigns the *same* session ids — see `Storage::sideload_id`).
use crate::sd::SIDELOAD_ID_BASE;

pub struct ObjectStore {
    /// The persisted settings, loaded once at boot — the config plane's read/modify cache. The SD
    /// card and the RRAM store themselves are **not** owned here: they live in the shared
    /// [`SharedStore`] both planes lock, which each storage/settings method takes as a `&mut` param
    /// (#270). Keeping only the catalog + this cache in `ObjectStore` lets the BLE planes hold it
    /// through a `RefCell` (never across an `await`) while the card is locked separately per call.
    settings: Settings,
    routes: Vec<ObjectSlot, MAX_ROUTES>,
    /// The stored rides, scanned once at boot: the `ble` build has no ride loop, so the catalog can't
    /// change while it runs (rides are recorded by the map build, then served here after a reflash —
    /// same card).
    rides: Vec<ObjectSlot, MAX_RIDES>,
    /// The next fresh-upload object id (ids are never reused within a boot).
    next_id: u16,
    /// The store revision: monotonic per boot, bumped on every commit/delete.
    revision: u32,
    /// The built list / diagnostics object a download streams from.
    list_buf: [u8; LIST_BUF_LEN],
}

impl ObjectStore {
    /// Mount-time construction: load settings, scan `/routes` into the id table, and sweep
    /// aborted commits (files whose held-back magic never got patched — see `sd.rs`). Runs under a
    /// boot-time lock of the shared store (`shared`), which it borrows for the settings load + scans.
    pub fn new(shared: &mut SharedStore) -> Self {
        let settings = shared.settings.load().unwrap_or_default();
        let mut store = ObjectStore {
            settings,
            routes: Vec::new(),
            rides: Vec::new(),
            next_id: 0,
            revision: 1,
            list_buf: [0; LIST_BUF_LEN],
        };
        store.rescan(shared);
        store.rescan_rides(shared);
        // The durable id floor (#450): fresh upload ids start at `max(scan_max + 1, stored floor)`,
        // so an id deleted last session can't be re-issued (the phone's persisted `deviceObjectID`s
        // key on it). A blank/torn line is "no floor" → exactly the old scan-derived start.
        if let Some(m) = shared.settings.load_id_marks() {
            store.next_id = store.next_id.max(m.next_route_id);
        }
        store
    }

    /// (Re)build the id table from the card. Uploaded files carry their **durable id in the
    /// filename** (`RT{id}.OBR`); side-loaded `.obcr` files get session ids from the
    /// [`SIDELOAD_ID_BASE`] band. `next_id` resumes past the highest stored upload id, so a
    /// fresh upload can't alias a stored object across reboots.
    fn rescan(&mut self, shared: &mut SharedStore) {
        self.routes.clear();
        let Some(storage) = &mut shared.storage else { return };
        let mut names: Vec<ShortFileName, MAX_ROUTES> = Vec::new();
        storage.for_each_route_file(|n| {
            if !names.is_full() {
                let _ = names.push(n.clone());
            }
        });
        for name in &names {
            match storage.route_object_info(name) {
                Some((byte_len, _)) => {
                    let id = match crate::sd::uploaded_route_id(name) {
                        Some(id) => {
                            self.next_id = self.next_id.max(id.saturating_add(1));
                            id
                        }
                        // Side-loads draw from the session registry shared with the ride loop's
                        // catalog scan (`Storage::sideload_id`) so both tables carry identical
                        // ids; `None` = band/registry exhausted → not listed rather than aliased.
                        None => match storage.sideload_id(name) {
                            Some(id) => id,
                            None => continue,
                        },
                    };
                    let _ = self.routes.push(ObjectSlot { id, file: name.clone(), byte_len });
                }
                // Unreadable: reclaim it only if it carries the aborted-commit signature (the
                // zeroed magic) — transiently unreadable real routes must be kept.
                None => {
                    if storage.is_aborted_commit(name) {
                        defmt::info!("store: sweeping aborted commit {}", defmt::Debug2Format(name));
                        let _ = storage.delete_route_file(name);
                    }
                }
            }
        }
        defmt::info!("store: {=usize} route object(s), next id {=u16}", self.routes.len(), self.next_id);
    }

    /// Scan `/tracks` for stored ride objects (`RD{id}.ORD`) — the id is durable in the filename, like
    /// the routes'. An interrupted save (the held-back version byte, exactly
    /// that signature) is swept; a merely unreadable file is kept off the catalog but never
    /// deleted. Ordered as the directory lists them; the app sorts by `start_time`.
    fn rescan_rides(&mut self, shared: &mut SharedStore) {
        self.rides.clear();
        let Some(storage) = &mut shared.storage else { return };
        let mut entries: Vec<(u16, ShortFileName), MAX_RIDES> = Vec::new();
        let mut overflow = false;
        storage.for_each_ride_file(|id, n| {
            if entries.push((id, n.clone())).is_err() {
                overflow = true;
            }
        });
        if overflow {
            defmt::warn!("store: more than {=usize} ride objects — the excess is not listed", MAX_RIDES);
        }
        for (id, name) in &entries {
            match storage.ride_object_info(name) {
                Some((byte_len, _)) => {
                    let _ = self.rides.push(ObjectSlot { id: *id, file: name.clone(), byte_len });
                }
                None => {
                    if storage.is_aborted_ride_object(name) {
                        defmt::info!("store: sweeping interrupted ride save {}", defmt::Debug2Format(name));
                        let _ = storage.delete_ride_file(name);
                    }
                }
            }
        }
        defmt::info!("store: {=usize} ride object(s)", self.rides.len());
    }

    /// The store digest.
    pub fn digest(&self) -> ObjectStoreDigest {
        ObjectStoreDigest {
            revision: self.revision,
            route_count: self.routes.len() as u16,
            ride_count: self.rides.len() as u16,
        }
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

    fn slot_index(&self, id: u16) -> Option<usize> {
        self.routes.iter().position(|s| s.id == id)
    }

    /// Whether a route object with this id exists (the control plane's cheap `notFound` check).
    pub fn has_route(&self, id: u16) -> bool {
        self.slot_index(id).is_some()
    }

    /// Whether a ride object with this id exists (the download-request `notFound` check).
    pub fn has_ride(&self, id: u16) -> bool {
        self.rides.iter().any(|s| s.id == id)
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
    pub fn apply_config(&mut self, shared: &mut SharedStore, name: &str, units: u8) {
        // Start from the current persisted truth so an on-device edit racing this write isn't dropped.
        self.settings = shared.settings.load().unwrap_or_default();
        self.settings.device_name = DeviceName::from_str_lossy(name);
        self.settings.units = if units == 1 { obc_app::Units::Imperial } else { obc_app::Units::Metric };
        shared.settings.save(&self.settings);
        BLE_CONFIG_WRITTEN.store(true, Ordering::Relaxed);
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

    /// Delete a stored route by object id. `true` = deleted (revision bumped).
    pub fn delete_route(&mut self, shared: &mut SharedStore, id: u16) -> bool {
        let Some(idx) = self.slot_index(id) else { return false };
        let Some(storage) = &mut shared.storage else { return false };
        if !storage.delete_route_file(&self.routes[idx].file) {
            return false;
        }
        self.routes.remove(idx);
        self.bump_revision();
        true
    }

    // ==================== upload ====================

    /// Validate a fresh upload from its descriptor (uploads restart, not resume): return the
    /// [`Receiver`] to drive, or the typed status to answer immediately. A non-zero
    /// offset is rejected (`Receiver::new`) — the app always sends 0. The SD temp is **not** opened
    /// here: the data plane opens it via [`upload_begin`](Self::upload_begin) at the first CoC byte,
    /// so an armed transfer whose CoC never opens holds no storage handle.
    pub fn upload_open(&mut self, shared: &SharedStore, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        // Descriptor-open reject, before any byte streams (issue #452): a *new* upload (id 0xFFFF or a
        // named id we don't hold) is refused when the catalog can't grow — StorageFull if the route
        // table is at MAX_ROUTES or the durable-id space is exhausted, NotFound for a named-but-unknown
        // id with room to spare. A replace-by-id of an existing route reuses its slot and is exempt
        // (updating the actively-navigated route must never hit the cap). The wire crate owns the rule.
        let catalog_full = self.routes.is_full() || self.next_id >= SIDELOAD_ID_BASE;
        let id_known = self.slot_index(desc.object_id).is_some();
        if let Some(status) = TransferStatus::upload_open_reject(desc.object_id, id_known, catalog_full) {
            return Err(status);
        }
        // No card ⇒ no upload; answer now rather than after the CoC opens.
        if shared.storage.is_none() {
            return Err(TransferStatus::Error);
        }
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    /// Open (truncating) the SD upload temp — called by the data plane when the transfer's bytes
    /// actually start flowing (see [`upload_open`](Self::upload_open)). False = no card / open
    /// failure (the caller answers `error`).
    pub fn upload_begin(&mut self, shared: &mut SharedStore) -> bool {
        shared.storage.as_mut().is_some_and(|s| s.upload_begin())
    }

    /// Sink one CoC chunk: append to the temp. False = storage failure (the caller aborts).
    pub fn upload_append(&mut self, shared: &mut SharedStore, bytes: &[u8]) -> bool {
        shared.storage.as_mut().is_some_and(|s| s.upload_append(bytes))
    }

    /// The whole link dropped, or the CoC dropped mid-upload, or the app aborted (op 3): discard
    /// the partial upload and release any open storage handles a cancelled future couldn't.
    /// Uploads don't resume, so nothing is kept — the app re-sends from the start.
    pub fn link_reset(&mut self, shared: &mut SharedStore) {
        self.upload_discard(shared);
        if let Some(storage) = &mut shared.storage {
            storage.close_object();
        }
    }

    /// Abort/interrupt: discard the in-flight temp.
    pub fn upload_discard(&mut self, shared: &mut SharedStore) {
        if let Some(storage) = &mut shared.storage {
            storage.upload_abort();
        }
    }

    /// All bytes arrived: verify + commit. On a CRC match the temp is promoted (fresh id assigned /
    /// replaced file swapped), the revision bumps, and the result carries the assigned id; on a mismatch
    /// nothing is committed and the temp is dropped. Returns `(object_id, status)` for the
    /// `transferResult`.
    pub fn upload_finish(&mut self, shared: &mut SharedStore, rx: &Receiver) -> (u16, TransferStatus) {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return (rx.object_id(), TransferStatus::Error), // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            self.upload_discard(shared);
            return (rx.object_id(), outcome.status);
        }
        let fresh = rx.object_id() == TransferControl::NEW_OBJECT_ID;
        if fresh && (self.routes.is_full() || self.next_id >= SIDELOAD_ID_BASE) {
            // Storage-full backstop: `upload_open` already rejects new uploads at descriptor-open
            // time (before any byte streams), so reaching here means the catalog filled *during* the
            // transfer. Same typed status, so the phone's handling is identical either way.
            self.upload_discard(shared);
            return (rx.object_id(), TransferStatus::StorageFull);
        }
        let replace_idx = if fresh { None } else { self.slot_index(rx.object_id()) };
        let Some(storage) = &mut shared.storage else { return (rx.object_id(), TransferStatus::Error) };
        let replace_file = replace_idx.map(|i| self.routes[i].file.clone());
        match storage.upload_commit(replace_file.as_ref(), self.next_id) {
            Some((file, byte_len, _info)) => {
                let id = match replace_idx {
                    Some(i) => {
                        self.routes[i].byte_len = byte_len;
                        self.routes[i].file = file;
                        self.routes[i].id
                    }
                    None => {
                        let id = self.next_id;
                        self.next_id += 1;
                        // Bump the persisted high-water past the assignment (#450) — one 16-byte
                        // RRAM line per fresh upload — so this id stays reserved across deletes
                        // and reboots. The ride floor in the same line is untouched.
                        let mut m = shared.settings.load_id_marks().unwrap_or_default();
                        m.next_route_id = m.next_route_id.max(self.next_id);
                        shared.settings.save_id_marks(&m);
                        let _ = self.routes.push(ObjectSlot { id, file, byte_len });
                        id
                    }
                };
                // The app-UI upload event (#451): the committed id + fresh-vs-replace, published
                // before the revision bump so the STORE_WAKE'd pass sees both edges together.
                UPLOAD_EVENT.store(
                    UPLOAD_EVENT_PRESENT | ((replace_idx.is_some() as u32) << 16) | id as u32,
                    Ordering::Relaxed,
                );
                self.bump_revision();
                (id, TransferStatus::Committed)
            }
            None => {
                // Validation/copy failed. If this was a replace, the old file may already be
                // gone (deleted after validation, before the copy landed) — re-check it and
                // drop its slot if so, so the catalog matches the card.
                if let Some(i) = replace_idx {
                    let gone =
                        shared.storage.as_ref().is_none_or(|s| s.route_object_info(&self.routes[i].file).is_none());
                    if gone {
                        self.routes.remove(i);
                        self.bump_revision();
                    }
                }
                (rx.object_id(), TransferStatus::Error)
            }
        }
    }

    // ==================== downloads ====================

    /// Open a download: build the list / diagnostics object, or open the stored route/ride (with its
    /// CRC pre-pass — the whole-object CRC the announce carries). Returns the sender to drive plus which
    /// source [`Self::download_read`] serves from. `diag` supplies the link-plane
    /// facts the diagnostics blob renders (unused by the other types); the runner builds it once.
    pub fn download_open(
        &mut self,
        shared: &mut SharedStore,
        desc: &TransferControl,
        diag: &DiagInput<'_>,
    ) -> Result<(StreamSender, DownloadSource), TransferStatus> {
        match desc.ty {
            ObjectType::RouteList | ObjectType::RideList => {
                // No card ≠ no objects: an empty *success* here would let one flaky mount
                // masquerade as "the device holds nothing" — the app takes a committed list
                // as authoritative and durably clears its on-device links off it. Answer the
                // typed error instead; the app keeps its links and retries later.
                if shared.storage.is_none() {
                    return Err(TransferStatus::Error);
                }
                let Some(len) = self.build_list(shared, desc.ty) else {
                    return Err(TransferStatus::Error);
                };
                let crc = Crc32::checksum(&self.list_buf[..len]);
                let tx = StreamSender::new(desc, len as u32, crc).map_err(|_| TransferStatus::Error)?;
                Ok((tx, DownloadSource::List))
            }
            ObjectType::Route => {
                let Some(idx) = self.slot_index(desc.object_id) else {
                    return Err(TransferStatus::NotFound);
                };
                let file = self.routes[idx].file.clone();
                self.open_object_download(shared, desc, &file, false)
            }
            // A ride download is the same verbatim stream — the stored `RD{id}.ORD` *is* the wire
            // object — just out of `/tracks`.
            ObjectType::Ride => {
                let Some(slot) = self.rides.iter().find(|s| s.id == desc.object_id) else {
                    return Err(TransferStatus::NotFound);
                };
                let file = slot.file.clone();
                self.open_object_download(shared, desc, &file, true)
            }
            // Diagnostics: render the text blob into the object buffer and stream it like a list.
            // Deliberately **card-independent** — diagnostics must be readable exactly
            // when things are broken, so no `storage` gate here (the store counts then honestly read
            // 0 with `sd: --`).
            ObjectType::Diagnostics => {
                let len = self.build_diagnostics(shared, diag);
                let crc = Crc32::checksum(&self.list_buf[..len]);
                let tx = StreamSender::new(desc, len as u32, crc).map_err(|_| TransferStatus::Error)?;
                Ok((tx, DownloadSource::List))
            }
            _ => Err(TransferStatus::NotFound),
        }
    }

    /// Render the diagnostics text (an opaque, human-readable UTF-8 blob, **not** an API) into
    /// [`Self::list_buf`], returning its byte length: identity, the persisted boot counter, uptime, the
    /// link counters, and the store's view of the card.
    fn build_diagnostics(&mut self, shared: &SharedStore, link: &DiagInput<'_>) -> usize {
        let mut w = BufWriter { buf: &mut self.list_buf, len: 0 };
        let _ = core::fmt::write(
            &mut w,
            format_args!(
                "obc diagnostics\nfw: {}\nhw: {}\nserial: {}\nprotocol: {}\nboot_count: {}\nuptime_s: {}\n\
                 link_connects: {}\nlink_disconnects: {}\nlink_last_reason: 0x{:02X}\n\
                 routes: {}\nrides: {}\nsd: {}\nstack_hw: {}\nstack_total: {}\n",
                link.firmware,
                link.hardware,
                link.serial,
                obc_ble::PROTOCOL_VERSION,
                shared.settings.boot_count(),
                link.uptime_s,
                link.connects,
                link.disconnects,
                link.last_disconnect_reason,
                self.routes.len(),
                self.rides.len(),
                if shared.storage.is_some() { "ok" } else { "--" },
                // The A9 soak-rig health numbers: the deepest stack use the ride loop has painted
                // (0 until the first scan) against the total usable stack — the "stack high-water + RAM
                // numbers posted" DoD, readable over the link with no RTT.
                link.stack_hw,
                crate::stackmeter::total(),
            ),
        );
        w.len
    }

    /// Open a stored object file for a verbatim download: the handle, the CRC pre-pass (the
    /// whole-object CRC the announce carries), the [`StreamSender`].
    fn open_object_download(
        &mut self,
        shared: &mut SharedStore,
        desc: &TransferControl,
        file: &ShortFileName,
        ride: bool,
    ) -> Result<(StreamSender, DownloadSource), TransferStatus> {
        let Some(storage) = &mut shared.storage else { return Err(TransferStatus::Error) };
        let opened = if ride { storage.open_ride_object(file) } else { storage.open_object(file) };
        let Some(len) = opened else {
            return Err(TransferStatus::Error);
        };
        let Some(crc) = object_crc(storage, len) else {
            storage.close_object();
            return Err(TransferStatus::Error);
        };
        let tx = StreamSender::new(desc, len, crc).map_err(|_| TransferStatus::Error)?;
        Ok((tx, DownloadSource::Object))
    }

    /// Read the chunk at `offset` into `buf` from the opened download source. False = read
    /// failure (the caller answers `error`).
    pub fn download_read(&self, shared: &SharedStore, source: DownloadSource, offset: u32, buf: &mut [u8]) -> bool {
        match source {
            DownloadSource::List => {
                let (start, end) = (offset as usize, offset as usize + buf.len());
                if end > self.list_buf.len() {
                    return false;
                }
                buf.copy_from_slice(&self.list_buf[start..end]);
                true
            }
            DownloadSource::Object => shared
                .storage
                .as_ref()
                .and_then(|s| s.object_source())
                .is_some_and(|src| obc_route::ByteSource::read_at(&src, offset, buf).is_ok()),
        }
    }

    /// Close the download's storage handle (done, dropped, or superseded).
    pub fn download_close(&mut self, shared: &mut SharedStore) {
        if let Some(storage) = &mut shared.storage {
            storage.close_object();
        }
    }

    /// Build the list object for `ty` into [`Self::list_buf`], returning its byte length — or
    /// `None` if a cataloged slot can't be read *now*. Entries come from each stored file's header
    /// (one read per object — a full catalog is ~a hundred header reads, tens of ms, done once per
    /// download).
    ///
    /// A cataloged slot was readable at the mount scan, so a read failure here is a transient
    /// glitch. Fail the **whole** list rather than silently omit the entry: the app takes a
    /// committed list as authoritative (it reconciles its on-device link set off it), and a list
    /// shorter than the `objectStore` digest's count would make it drop a still-present route.
    /// `None` → the caller answers a typed `error` and the app retries, keeping its links.
    fn build_list(&mut self, shared: &SharedStore, ty: ObjectType) -> Option<usize> {
        // Each arm only supplies its per-entry field mapping (slot → encoded entry, `None` on a
        // failed read); `encode_list` owns the shared header-offset arithmetic both share.
        let (len, count) = match (&shared.storage, ty) {
            (Some(storage), ObjectType::RouteList) => Self::encode_list(
                &mut self.list_buf,
                self.routes.iter().map(|slot| {
                    let (byte_len, info) = storage.route_object_info(&slot.file)?;
                    Some(
                        RouteListEntry {
                            object_id: slot.id,
                            byte_len,
                            distance_m: info.distance_m,
                            ascent_m: info.ascent_m,
                            point_count: info.point_count,
                            waypoint_count: info.waypoint_count,
                            name: info.name.as_bytes(),
                        }
                        .encode(),
                    )
                }),
            )?,
            (Some(storage), ObjectType::RideList) => Self::encode_list(
                &mut self.list_buf,
                self.rides.iter().map(|slot| {
                    let (byte_len, info) = storage.ride_object_info(&slot.file)?;
                    Some(
                        RideListEntry {
                            object_id: slot.id,
                            byte_len,
                            start_time: info.start_time,
                            distance_m: info.distance_m,
                            moving_time_s: info.moving_time_s,
                            avg_speed_cms: info.avg_speed_cms,
                            climb_m: info.climb_m,
                            name: info.name.as_bytes(),
                        }
                        .encode(),
                    )
                }),
            )?,
            // No card, or a non-list type: an empty list — just the header.
            _ => (ListHeader::ENCODED_LEN, 0),
        };
        self.list_buf[..ListHeader::ENCODED_LEN].copy_from_slice(&ListHeader { count }.encode());
        Some(len)
    }

    /// Encode `entries` into `buf` after the [`ListHeader`], returning the total byte length + entry
    /// count. Each item is the encoded entry, or `None` for a slot that couldn't be read *now* —
    /// which fails the whole list (see [`Self::build_list`]) by propagating out through `?`.
    fn encode_list(
        buf: &mut [u8],
        entries: impl Iterator<Item = Option<[u8; obc_ble::LIST_ENTRY_LEN]>>,
    ) -> Option<(usize, u16)> {
        let mut off = ListHeader::ENCODED_LEN;
        let mut count: u16 = 0;
        for entry in entries {
            let entry = entry?;
            buf[off..off + obc_ble::LIST_ENTRY_LEN].copy_from_slice(&entry);
            off += obc_ble::LIST_ENTRY_LEN;
            count += 1;
        }
        Some((off, count))
    }
}

/// The link-plane facts the diagnostics blob renders — assembled by the `ble` module,
/// which owns the identity strings and the live BLE link-status counters; the store adds what
/// *it* owns (boot counter, catalog counts, the card).
pub struct DiagInput<'a> {
    pub firmware: &'a str,
    pub hardware: &'a str,
    pub serial: &'a str,
    pub uptime_s: u32,
    pub connects: u32,
    pub disconnects: u32,
    pub last_disconnect_reason: u8,
    /// The ride loop's deepest painted stack use (bytes); 0 before the first scan.
    pub stack_hw: u32,
}

/// `core::fmt::Write` into a fixed byte buffer, silently truncating on overflow (the
/// diagnostics text is a few hundred bytes against the multi-KB list buffer).
struct BufWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for BufWriter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Which source an open download streams from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DownloadSource {
    /// The built list / diagnostics object in [`ObjectStore::list_buf`].
    List,
    /// The open route / ride file on the card.
    Object,
}

/// The whole-object CRC pre-pass over the open detail-download file: one sequential read of
/// `len` bytes in card-block-sized chunks. Synchronous (the caller yields between GATT events,
/// not mid-CRC) — ~0.5 s/MB at the 8 MHz bus, and a route object is typically well under one.
fn object_crc(storage: &Storage, len: u32) -> Option<u32> {
    let src = storage.object_source()?;
    let mut crc = Crc32::new();
    let mut buf = [0u8; 512];
    let mut off = 0u32;
    while off < len {
        let n = ((len - off) as usize).min(buf.len());
        obc_route::ByteSource::read_at(&src, off, &mut buf[..n]).ok()?;
        crc.update(&buf[..n]);
        off += n as u32;
    }
    Some(crc.finalize())
}
