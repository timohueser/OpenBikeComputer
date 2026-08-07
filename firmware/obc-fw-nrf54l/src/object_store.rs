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
//! - **Store revision**: bumped on every commit/delete; the BLE plane stamps it into the
//!   `storeChanged` status message — protocol v2's sole change signal (the `objectStore` digest
//!   characteristic is retired).
//! - **The upload state machine**: descriptor → [`Receiver`] (+ temp-file sink) → commit. Uploads are
//!   not resumable: an interrupted upload (a drop or an `op=3` abort) is discarded and the app re-sends
//!   the object from the start.
//! - **Downloads**: the `routeList` / `rideList` objects are built into a resident buffer; a route or
//!   ride detail is served straight off the card (CRC pre-pass, then chunk reads — a stored
//!   `RD{id}.ORD` *is* the wire object, so a ride download is verbatim).
//! - **Rides are read-only over the link**: recorded by the ride loop (which posts the saved-ride
//!   edge so this catalog follows mid-session) and deleted only from the device's Rides screen
//!   (#454) — never by a phone command; the app hides synced rides locally instead of deleting them.
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
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embedded_sdmmc::ShortFileName;
use heapless::Vec;
use obc_app::settings::DeviceName;
use obc_app::{Retention, Settings, MAX_ROUTES, MAX_TRIPS};
use obc_ble::{
    Crc32, ListHeader, ObjectType, Receiver, RideListEntry, RouteListEntry, StreamSender, TransferControl,
    TransferStatus, TripListEntry,
};
use obc_ports::SettingsStore;

use crate::sd::Storage;
use crate::SharedStore;

/// The outcome of a `setRouteRetention` command (finding #876-5) — replaces the old `Option<bool>`
/// so a durable-write failure is distinguishable from success. The BLE handler maps it to a
/// [`CommandStatus`](obc_ble::CommandStatus): `Changed`/`Unchanged` → `Ok` (bump only on `Changed`),
/// `NotFound` → `NotFound`, `WriteFailed` → `Error` — never a false `ok` ahead of durability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetRetentionResult {
    /// No stored route has this id (or the card is absent) — the handler answers `notFound`.
    NotFound,
    /// The route already had this level — `ok`, **no** revision bump (the idempotence pin).
    Unchanged,
    /// A real change persisted durably — `ok`, revision bumped, `storeChanged(route)` fires.
    Changed,
    /// The route exists but the retention sidecar rewrite did not reach the card — `Error`, no bump.
    WriteFailed,
}

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

/// The latest **committed route upload** for the app UI (epic #447, P4): which durable id landed
/// and whether it replaced a stored slot — what the store-changed edge alone can't say, and what
/// the upload popups key their variant on. Packed `present-bit | replaced-bit | id` (`0` = none);
/// deliberately a **single latest-wins slot**, which *is* the locked popup rule (consecutive
/// uploads replace the prompt, most recent wins), so a burst between passes needs no queue.
///
/// Published by [`ObjectStore::upload_finish`] **before** its revision bump, so the pass the
/// [`STORE_WAKE`] pulls out of warm sleep sees the rescan edge and this event together; the ride
/// loop drains the rescan **first** (the id must resolve against the fresh catalog), then rings
/// [`App::apply_event`](obc_app::App::apply_event). Same module-static
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

/// The latest **committed trip upload** for the app UI — the trip twin of [`UPLOAD_EVENT`], packed
/// identically (`present-bit | replaced-bit | id`). The replaced-bit matters more here than for
/// routes: the desktop app *edits* a trip exclusively by replace-at-same-id (rename, add/remove/
/// move stage — one upload per click), so the app **suppresses the popup on a replace** and only a
/// *fresh* trip — a delivery — is announced. Published by [`ObjectStore::upload_finish_trip`]
/// **before** its revision bump, drained by the ride loop strictly *after* the route event so the
/// pass's popup order matches the wire's routes-then-trip commit order — the trip popup then wins
/// the app's single most-recent-wins prompt slot, which is exactly what collapses a trip
/// transfer's per-route popup parade into one "TRIP RECEIVED" card. Same latest-wins single-slot +
/// `Relaxed` hand-off rationale as [`UPLOAD_EVENT`]: of two fresh trips committing between passes,
/// only the newest pops — the same policy routes have always had.
static TRIP_UPLOAD_EVENT: AtomicU32 = AtomicU32::new(0);

/// Drain the latest committed trip upload since the last call: `(trip_id, replaced_existing)`, or
/// `None` when none landed. The ride loop calls this once per pass, after [`take_store_changed`]
/// (the id must resolve against the freshly re-fed trip catalog) and after [`take_route_uploaded`].
pub(crate) fn take_trip_uploaded() -> Option<(u16, bool)> {
    let v = TRIP_UPLOAD_EVENT.swap(0, Ordering::Relaxed);
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
/// hold-to-delete footer asked to remove, or the auto-expiry sweep's next expired route. The **ride
/// loop** posts it, and the **BLE plane** — the sole owner of the `RefCell<ObjectStore>` — executes
/// it via [`ObjectStore::delete_route`], so the delete goes through the same catalog + revision +
/// `storeChanged` path a phone-initiated delete does (never raw SD).
///
/// It lives as a module static, like [`STORE_CHANGED`], because the `ObjectStore` is trapped behind
/// the BLE task's `RefCell` while the app lives in the ride loop — this is the lock-free hand-off
/// from the loop that owns the *intent* to the plane that owns the *store*.
///
/// A bounded **[`Channel`]**, not a coalescing `Signal` (finding #876-3): the old overwriting
/// `Signal` held only the newest id, so a retention batch dispatched faster than the SD delete task
/// drained silently lost the earlier ids until a later sweep. The channel never *overwrites* a
/// queued id — but be precise about what it does and does not guarantee: a full channel drops the
/// posted id ([`request_route_delete`] reports `false`), and the app's dispatch bookkeeping ran
/// *before* this post, so a drop is **not observed backpressure**. End-to-end losslessness rests on
/// the app's **retain-until-rescan** ownership instead: a retention delete candidate stays queued in
/// `obc-app` until the store rescan confirms the id gone, so a dropped post is simply re-dispatched
/// after the bounded backoff. With one retention delete in flight per kind plus at most one manual
/// delete, depth [`DELETE_CHANNEL_CAP`] means a full channel is effectively unreachable — and when
/// it isn't, the cost is a delay, never a lost delete. Manual (UI hold-to-delete) and retention
/// deletes share this one executor.
static ROUTE_DELETE_REQ: Channel<CriticalSectionRawMutex, u16, DELETE_CHANNEL_CAP> = Channel::new();

/// The bounded delete-request channel depth. The app dispatches **one retention delete per kind in
/// flight** (retained until the store confirms it gone) plus at most one manual delete, so at most a
/// couple of ids are ever outstanding; a small depth is ample. A full channel drops the post (the
/// caller warns) and the app's retained candidate re-dispatches it after the backoff — a delay,
/// never a lost delete (see [`ROUTE_DELETE_REQ`]).
const DELETE_CHANNEL_CAP: usize = 8;

/// Post a route-delete request from the ride loop (epic #447, P6) — the BLE plane drains it and runs
/// the `ObjectStore` delete. Returns `false` when the channel is full and the id was **dropped**;
/// the caller must surface that (a warn), and recovery is the app's retain-until-rescan retry — the
/// candidate is still owned app-side and re-dispatches after its bounded backoff (finding #876-3).
pub(crate) fn request_route_delete(id: u16) -> bool {
    ROUTE_DELETE_REQ.try_send(id).is_ok()
}

/// The BLE plane's route-delete arm: resolves with the next id to delete once the ride loop posts
/// one. Folded into the BLE lifetime `join` so it drains whether the phone is connected or the device
/// is parked advertising (see `ble::run`).
pub(crate) async fn wait_route_delete() -> u16 {
    ROUTE_DELETE_REQ.receive().await
}

/// On-device **ride**-delete request (epic #447, P7 / #454) — the ride-namespace twin of
/// [`ROUTE_DELETE_REQ`]. The ride loop's Rides-menu hold posts a ride's durable object id; the BLE
/// plane drains it and runs [`ObjectStore::delete_ride`], so an on-device ride delete goes through
/// the same catalog + revision + `storeChanged` path a phone-initiated delete does.
/// A bounded [`Channel`] like [`ROUTE_DELETE_REQ`] (finding #876-3): the ride retention sweep can
/// discover several synced+aged rides at once, and the old overwriting `Signal` lost all but the
/// newest. Same contract as the route channel: never an overwrite, but a full channel **drops** the
/// post — end-to-end losslessness rests on the app's retain-until-rescan retry, not on observed
/// backpressure. Shared by manual (Rides-menu hold) and retention ride deletes.
static RIDE_DELETE_REQ: Channel<CriticalSectionRawMutex, u16, DELETE_CHANNEL_CAP> = Channel::new();

/// Post a ride-delete request from the ride loop (epic #447, P7). Returns `false` when the channel
/// is full and the id was **dropped** — the caller warns, and the app's retained candidate
/// re-dispatches after its bounded backoff (see [`request_route_delete`]).
pub(crate) fn request_ride_delete(id: u16) -> bool {
    RIDE_DELETE_REQ.try_send(id).is_ok()
}

/// The BLE plane's ride-delete arm: resolves with the next ride id to delete once the ride loop posts one.
pub(crate) async fn wait_ride_delete() -> u16 {
    RIDE_DELETE_REQ.receive().await
}

/// On-device trip **cascade**-delete request (epic #526, TR3/TR4) — the trip-namespace sibling of
/// [`ROUTE_DELETE_REQ`]. The Route menu's long-press → confirm posts the trip's durable object id
/// (`App::drain_host_commands` in the ride loop); the BLE plane drains it and runs
/// [`ObjectStore::delete_trip_cascade`] — member routes first, then the trip object — so both the
/// route and the trip store revisions move and the phone gets **both** `storeChanged` edges (§4.3).
static TRIP_CASCADE_REQ: Signal<CriticalSectionRawMutex, u16> = Signal::new();

/// Post a trip cascade-delete from the ride loop (epic #526, TR3). Overwrites any un-drained request
/// (one delete in flight at a time — the confirm dialog fires one and the drain runs promptly).
pub(crate) fn request_trip_cascade(id: u16) {
    TRIP_CASCADE_REQ.signal(id);
}

/// The BLE plane's trip-cascade arm: resolves with the trip id once the ride loop posts one.
pub(crate) async fn wait_trip_cascade() -> u16 {
    TRIP_CASCADE_REQ.wait().await
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

/// The list-object buffer: header + one entry per slot of whichever catalog encodes **larger** (both
/// lists stream from the same scratch — one transfer at a time). Protocol v2 gives the two lists
/// different entry sizes (`routeList` 76 B with its content CRC, `rideList` 72 B), so the larger
/// object isn't simply the larger count — compare both encoded sizes.
const LIST_BUF_LEN: usize = {
    let route = ListHeader::object_len(MAX_ROUTES, RouteListEntry::ENTRY_LEN);
    let ride = ListHeader::object_len(MAX_RIDES, RideListEntry::ENTRY_LEN);
    let trip = ListHeader::object_len(MAX_TRIPS, TripListEntry::ENTRY_LEN);
    let a = if route > ride { route } else { ride };
    if a > trip {
        a
    } else {
        trip
    }
};

// The side-load id band base lives in `sd.rs` beside the session registry both scanners share
// (the ride loop's catalog scan assigns the *same* session ids — see `Storage::sideload_id`).
use crate::sd::SIDELOAD_ID_BASE;

/// The one place a volume-set refusal becomes a wire status (issue #1039). `obc_app::set_upload`
/// names the *reason* so its rules can be tested without a wire vocabulary; this maps each onto the
/// §4.3 status a host already knows how to read:
///
/// - a part field that names no file, or a manifest with no set in flight, is a client error about
///   an object that does not exist → `notFound` / `error`, the same pair a bad id gets;
/// - a set past this board's shard ceiling is a **catalog** it cannot index another entry in, which
///   is exactly what `storageFull` means in §4.3 (the route cap, the trip cap) — and like those, it
///   is refused at descriptor-open before a byte streams, so the host can say "this device cannot
///   take a map that large" instead of failing at the end of a multi-gigabyte upload;
/// - a manifest sent before its shards is a protocol-order error, not a storage one → `error`.
const fn set_reject_status(reject: obc_app::SetReject) -> TransferStatus {
    match reject {
        obc_app::SetReject::Part => TransferStatus::NotFound,
        obc_app::SetReject::Shards => TransferStatus::StorageFull,
        obc_app::SetReject::Mismatch | obc_app::SetReject::ManifestEarly | obc_app::SetReject::Length => {
            TransferStatus::Error
        }
    }
}

pub struct ObjectStore {
    /// The persisted settings, loaded once at boot — the config plane's read/modify cache. The SD
    /// card and the RRAM store themselves are **not** owned here: they live in the shared
    /// [`SharedStore`] both planes lock, which each storage/settings method takes as a `&mut` param
    /// (#270). Keeping only the catalog + this cache in `ObjectStore` lets the BLE planes hold it
    /// through a `RefCell` (never across an `await`) while the card is locked separately per call.
    settings: Settings,
    routes: Vec<ObjectSlot, MAX_ROUTES>,
    /// The stored rides: scanned at boot and re-scanned on the saved-ride edge ([`RIDE_SAVED`]) —
    /// since the de-split the `ble` build *is* the map build, so the ride loop records new rides
    /// mid-session and this catalog must follow (it feeds the `rideList` object the phone syncs
    /// against).
    rides: Vec<ObjectSlot, MAX_RIDES>,
    /// The trip catalog (epic #526 TR4): `TP{id}.OBT` files scanned at boot + on every trip
    /// commit/delete, the wire-facing twin of the app's trip folders. Each slot's `byte_len` is the
    /// stored trip-object size; the `tripList` build reads each file's stages fresh (like routes read
    /// their header) to sum resolvable-stage stats.
    trips: Vec<ObjectSlot, MAX_TRIPS>,
    /// The next fresh-upload object id (ids are never reused within a boot).
    next_id: u16,
    /// The next fresh **trip** id — a device counter separate from routes/rides (spec §4.1), floored
    /// by its own RRAM high-water line so a deleted trip id is never re-issued across a reboot.
    next_trip_id: u16,
    /// The store revision: monotonic per boot, bumped on every route/ride commit/delete.
    revision: u32,
    /// The **trip** store revision — monotonic per boot, its own counter (spec §4.3: a trip
    /// commit/delete bumps the trip store, never the route store). Stamped into `storeChanged(trip)`.
    trip_revision: u32,
    /// Full route-catalog size **before** the [`MAX_ROUTES`] cap — the `routeList` header's `total`
    /// (epic #632 item 7). Equal to `routes.len()` when the card fits the cap; greater when the scan
    /// dropped the excess, which the app surfaces as a truncation warning.
    route_total: u16,
    /// Full ride-catalog size before the [`MAX_RIDES`] cap — the `rideList` header's `total`.
    ride_total: u16,
    /// Full trip-catalog size before the [`MAX_TRIPS`] cap — the `tripList` header's `total`.
    trip_total: u16,
    /// The built list / diagnostics object a download streams from.
    list_buf: [u8; LIST_BUF_LEN],
    /// The **volume set** being received, if any (issue #1039). A set is several transfers, so
    /// unlike every other object type it needs state that outlives one descriptor: which set id
    /// this device minted, how many shards it will have, and which of them have committed. That is
    /// what makes `OBCA_Spec.md` §5.4's manifest-last rule enforceable rather than merely
    /// documented — see `obc_app::set_upload`. Eight bytes.
    ///
    /// Deliberately **not** dropped by [`link_reset`](Self::link_reset), which runs on either
    /// transport's teardown. It is closed by [`set_manifest_finish`](Self::set_manifest_finish)
    /// (the set committed, or its manifest was refused and the set deleted with it) and by
    /// [`set_upload_abort`](Self::set_upload_abort) — the cable's own teardown, and the `op=3`
    /// abort that reaches it.
    set_upload: Option<obc_app::SetUpload>,
}

impl ObjectStore {
    /// The empty store — no settings read, no card scan; [`hydrate`](Self::hydrate) does that,
    /// in place. Construction is split in two because this struct is ~13.5 KB by value: the old
    /// `new(shared)`-then-`RefCell::new` shape put **two** copies of it in `link::init_store`'s
    /// frame (the return slot + the wrapper's argument), the measured ~27.6 KB boot spike that
    /// overran the residual stack once EL7 grew the ride task's poll frame (STKOF HardFault at
    /// the `init_store` prologue, 2026-08-03). One by-value hop into the `.bss` slot is the floor
    /// for safe code; everything that scans stays in [`hydrate`], operating on the slot directly.
    pub fn empty() -> Self {
        ObjectStore {
            settings: Settings::default(),
            routes: Vec::new(),
            rides: Vec::new(),
            trips: Vec::new(),
            next_id: 0,
            next_trip_id: 0,
            revision: 1,
            trip_revision: 1,
            route_total: 0,
            ride_total: 0,
            trip_total: 0,
            list_buf: [0; LIST_BUF_LEN],
            set_upload: None,
        }
    }

    /// Mount-time fill of an [`empty`](Self::empty) store, **in place**: load settings, scan
    /// `/routes` into the id table, and sweep aborted commits (files whose held-back magic never
    /// got patched — see `sd.rs`). Runs under a boot-time lock of the shared store (`shared`),
    /// which it borrows for the settings load + scans.
    pub fn hydrate(&mut self, shared: &mut SharedStore) {
        self.settings = shared.settings.load().unwrap_or_default();
        self.rescan(shared);
        self.rescan_rides(shared);
        self.rescan_trips(shared);
        // The durable id floor (#450): fresh upload ids start at `max(scan_max + 1, stored floor)`,
        // so an id deleted last session can't be re-issued (the phone's persisted `deviceObjectID`s
        // key on it). A blank/torn line is "no floor" → exactly the old scan-derived start.
        if let Some(m) = shared.settings.load_id_marks() {
            self.next_id = self.next_id.max(m.next_route_id);
        }
        // The trip-id floor draws from its own RRAM line (spec §4.1 — a separate counter).
        if let Some(floor) = shared.settings.load_trip_mark() {
            self.next_trip_id = self.next_trip_id.max(floor);
        }
    }

    /// (Re)build the id table from the card. Uploaded files carry their **durable id in the
    /// filename** (`RT{id}.OBR`); side-loaded `.obcr` files get session ids from the
    /// [`SIDELOAD_ID_BASE`] band. `next_id` resumes past the highest stored upload id, so a
    /// fresh upload can't alias a stored object across reboots.
    fn rescan(&mut self, shared: &mut SharedStore) {
        self.routes.clear();
        self.route_total = 0;
        let Some(storage) = &mut shared.storage else { return };
        let mut names: Vec<ShortFileName, MAX_ROUTES> = Vec::new();
        // Count how many route files the cap dropped (epic #632 item 7): `route_total` = listed +
        // dropped, so the `routeList` header's `total` exceeds `count` exactly when the scan
        // truncated — the app then warns instead of silently answering "up to date".
        let mut over_cap: u16 = 0;
        storage.for_each_route_file(|n| {
            if names.push(n.clone()).is_err() {
                over_cap = over_cap.saturating_add(1);
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
        // `total` = listed entries + those the cap dropped (see `over_cap` above).
        self.route_total = (self.routes.len() as u16).saturating_add(over_cap);
        defmt::info!("store: {=usize} route object(s), next id {=u16}", self.routes.len(), self.next_id);
    }

    /// Scan `/tracks` for stored ride objects (`RD{id}.ORD`) — the id is durable in the filename, like
    /// the routes'. An interrupted save (the held-back version byte, exactly
    /// that signature) is swept; a merely unreadable file is kept off the catalog but never
    /// deleted. Ordered as the directory lists them; the app sorts by `start_time`.
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
        self.ride_total = (self.rides.len() as u16).saturating_add(over_cap);
        defmt::info!("store: {=usize} ride object(s)", self.rides.len());
    }

    /// (Re)build the **trip** catalog from the card (epic #526 TR4) — the trip twin of [`rescan`]:
    /// scan `TP{id}.OBT` (durable id in the name) + side-loaded `.obt` (session id), record each
    /// slot's byte length, and resume `next_trip_id` past the highest stored upload id. A trip whose
    /// header doesn't validate is skipped; a torn commit (held-back zero version) is swept like an
    /// aborted route commit (same zeroed-first-bytes signature, same `/routes` dir).
    fn rescan_trips(&mut self, shared: &mut SharedStore) {
        self.trips.clear();
        self.trip_total = 0;
        let Some(storage) = &mut shared.storage else { return };
        let mut names: Vec<ShortFileName, MAX_TRIPS> = Vec::new();
        let mut over_cap: u16 = 0;
        storage.for_each_trip_file(|n| {
            if names.push(n.clone()).is_err() {
                over_cap = over_cap.saturating_add(1);
            }
        });
        for name in &names {
            match storage.read_trip(name) {
                Some((byte_len, _meta, _stage_count)) => {
                    let id = match crate::sd::uploaded_trip_id(name) {
                        Some(id) => {
                            self.next_trip_id = self.next_trip_id.max(id.saturating_add(1));
                            id
                        }
                        None => match storage.sideload_id(name) {
                            Some(id) => id,
                            None => continue,
                        },
                    };
                    let _ = self.trips.push(ObjectSlot { id, file: name.clone(), byte_len });
                }
                None => {
                    if storage.is_aborted_commit(name) {
                        defmt::info!("store: sweeping aborted trip commit {}", defmt::Debug2Format(name));
                        let _ = storage.delete_trip_file(name);
                    }
                }
            }
        }
        self.trip_total = (self.trips.len() as u16).saturating_add(over_cap);
        defmt::info!("store: {=usize} trip object(s), next trip id {=u16}", self.trips.len(), self.next_trip_id);
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

    /// Bump the **trip** store's own revision (epic #526 TR4; spec §4.3 — separate from the route
    /// revision) and raise the same app-side rescan edge a route/ride move does (the ride loop rescans
    /// routes + rides + trips off one `STORE_CHANGED` counter).
    fn bump_trip_revision(&mut self) -> u32 {
        self.trip_revision = self.trip_revision.wrapping_add(1);
        STORE_CHANGED.fetch_add(1, Ordering::Relaxed);
        STORE_WAKE.signal(());
        self.trip_revision
    }

    /// The trip store's revision — stamped into `storeChanged(trip)` (spec §4.3).
    pub fn trip_revision(&self) -> u32 {
        self.trip_revision
    }

    fn trip_index(&self, id: u16) -> Option<usize> {
        self.trips.iter().position(|s| s.id == id)
    }

    /// Whether a trip object with this id exists (the control plane's cheap `notFound` check).
    pub fn has_trip(&self, id: u16) -> bool {
        self.trip_index(id).is_some()
    }

    fn slot_index(&self, id: u16) -> Option<usize> {
        self.routes.iter().position(|s| s.id == id)
    }

    /// The stored **route** holding exactly this content — same byte length AND the same
    /// whole-object CRC in the `/routes` sidecar — or `None`. The fresh-upload dedup lookup
    /// (`upload_finish`): content identity is the CRC (epic #632), the length check is free
    /// belt-and-braces. A route with no sidecar entry yet (side-loaded, not yet listed) simply
    /// never matches — the safe direction (worst case a true duplicate of a side-load, never a
    /// wrong id).
    fn find_route_by_content(&self, shared: &SharedStore, crc: u32, byte_len: u32) -> Option<u16> {
        let storage = shared.storage.as_ref()?;
        let crcs = storage.load_route_crcs();
        self.routes.iter().find(|s| s.byte_len == byte_len && crcs.get(s.id) == Some(crc)).map(|s| s.id)
    }

    /// The trip twin of [`find_route_by_content`](Self::find_route_by_content), against the
    /// trip-CRC sidecar (`upload_finish_trip`'s dedup lookup).
    fn find_trip_by_content(&self, shared: &SharedStore, crc: u32, byte_len: u32) -> Option<u16> {
        let storage = shared.storage.as_ref()?;
        let crcs = storage.load_trip_crcs();
        self.trips.iter().find(|s| s.byte_len == byte_len && crcs.get(s.id) == Some(crc)).map(|s| s.id)
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
        // The BLE plane persists its owned fields directly (best-effort): the store logs a write
        // failure internally, and the phone re-asserts config on reconnect, so there is no App-side
        // revision to retry here (the device-edit path owns the acknowledged handshake — #810).
        let _ = shared.settings.save(&self.settings);
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
        // Retire the route's content-CRC sidecar entry (epic #632 item 6) — ids never reuse, so this
        // is belt-and-braces tidiness that keeps the sidecar from carrying a stale fingerprint.
        storage.forget_route_crc(id);
        // …and its retention entry (auto-expiry epic #638, S3), same never-reuse tidiness.
        storage.forget_route_retention(id);
        self.routes.remove(idx);
        self.route_total = self.route_total.saturating_sub(1);
        self.bump_revision();
        true
    }

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

    /// Delete a stored **trip** object by id (epic #526 TR4) — the `deleteObject` trip type (spec
    /// §4.4). **Non-cascading**: removes only the trip object; its member routes become top-level
    /// routes (spec §7.7). Retires the trip's CRC sidecar entry and bumps the **trip** store revision
    /// (never the route store, §4.3). `true` = deleted; an unknown id → `false` (the handler answers
    /// `notFound`).
    pub fn delete_trip(&mut self, shared: &mut SharedStore, id: u16) -> bool {
        let Some(idx) = self.trip_index(id) else { return false };
        let Some(storage) = &mut shared.storage else { return false };
        if !storage.delete_trip_file(&self.trips[idx].file) {
            return false;
        }
        storage.forget_trip_crc(id);
        self.trips.remove(idx);
        self.trip_total = self.trip_total.saturating_sub(1);
        self.bump_trip_revision();
        true
    }

    /// The on-device long-press **cascade** delete (epic #526 TR3/TR4): the trip object **and** its
    /// member route objects — the "delete trip & routes" the device's Route-folder hold composes. The
    /// wire protocol's `deleteObject` is non-cascading ([`delete_trip`](Self::delete_trip)); this
    /// composes it, exactly as the app expresses the same intent as individual deletes (spec §7.7).
    /// Each member route is deleted through [`delete_route`](Self::delete_route) so the **route** store
    /// revision + `storeChanged(route)` move, then the trip through [`delete_trip`](Self::delete_trip)
    /// so the **trip** store revision + `storeChanged(trip)` move — both edges emitted, as §4.3
    /// requires. A dangling stage id (already-deleted member) is skipped. `true` = the trip was deleted.
    ///
    /// Driven by the ride loop's TR3 drain: `App::drain_host_commands` → [`request_trip_cascade`] →
    /// the BLE plane's `trip_cascade_task`, mirroring the `request_route_delete` →
    /// [`delete_route`](Self::delete_route) seam.
    pub fn delete_trip_cascade(&mut self, shared: &mut SharedStore, id: u16) -> bool {
        let Some(idx) = self.trip_index(id) else { return false };
        // Resolve the member stage ids from the stored trip file before deleting anything.
        let file = self.trips[idx].file.clone();
        let stages = shared.storage.as_ref().and_then(|s| s.read_trip(&file)).map(|(_, meta, _)| meta.stage_ids);
        if let Some(stages) = stages {
            for stage_id in stages {
                let _ = self.delete_route(shared, stage_id); // dangling → false, skipped
            }
        }
        self.delete_trip(shared, id)
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

    /// Set a stored route's retention level from the `setRouteRetention` command (§4.4 cmd 6 /
    /// epic #638 S4). Writes the level through the S3 route-retention sidecar **without touching
    /// `last_used`** (changing retention never resets the usage clock). Returns:
    ///
    /// - `None` — the id names no stored route → the handler answers `notFound` (2). No sidecar write.
    /// - `Some(true)` — a real change; the sidecar was rewritten and the **route** store revision
    ///   bumped, so the phone's `storeChanged(route)` fires and the ride loop's `STORE_CHANGED` rescan
    ///   re-feeds `set_routes_with_meta` with the fresh retention (the app displays device truth).
    /// - `Some(false)` — the value was already that level; `ok` with **no** revision bump (the
    ///   idempotence pin: only a real change moves the store).
    ///
    /// A missing card is treated as "no such route" ([`NotFound`](SetRetentionResult::NotFound)) —
    /// nothing to write. A sidecar write that does not reach the card is
    /// [`WriteFailed`](SetRetentionResult::WriteFailed) — the revision is **not** bumped and the
    /// handler replies `command` `Error`, never a false `ok` (finding #876-5).
    pub fn set_route_retention(
        &mut self,
        shared: &mut SharedStore,
        id: u16,
        retention: Retention,
    ) -> SetRetentionResult {
        if !self.has_route(id) {
            return SetRetentionResult::NotFound;
        }
        let Some(storage) = shared.storage.as_mut() else {
            return SetRetentionResult::NotFound;
        };
        match storage.set_route_retention_level(id, retention) {
            Ok(false) => SetRetentionResult::Unchanged, // already that level — `ok`, no bump (idempotence pin)
            Ok(true) => {
                // Durable success only: bump the route revision so `storeChanged(route)` + the ride
                // loop's rescan re-feed the fresh retention.
                self.bump_revision();
                SetRetentionResult::Changed
            }
            Err(_) => SetRetentionResult::WriteFailed, // torn persist — no bump, surfaced as `Error`
        }
    }

    /// Adopt locally-saved rides into the live catalog: re-scan `/tracks` and bump the revision, so
    /// the phone's `storeChanged(ride)` + digest and the ride loop's [`STORE_CHANGED`] edge (→ the
    /// Rides menu re-feed) all move from this one edge — the exact path an upload commit or a delete
    /// takes. Driven by [`wait_ride_saved`] in `ble::run`'s `ride_saved_task`.
    pub fn adopt_saved_rides(&mut self, shared: &mut SharedStore) {
        self.rescan_rides(shared);
        self.bump_revision();
    }

    /// Mark a ride as synced (epic #447, P7): set the `/tracks` synced-set flag when a ride download
    /// **completes**, so the Rides screen's delete footer drops the "not synced" cue. A revision bump
    /// funnels the change to the ride loop (via `STORE_CHANGED`) so its live rescan re-feeds the Rides
    /// menu with the freshened flag — without changing the ride *count* the phone reconciles on.
    /// A no-op (no bump) when the ride was already flagged.
    pub fn mark_ride_synced(&mut self, shared: &mut SharedStore, id: u16) {
        let Some(storage) = &mut shared.storage else { return };
        // `synced_at = 0` — the sweep stamps the real countdown anchor once trusted (see `ack_rides`).
        if storage.mark_ride_synced(id, 0) {
            self.bump_revision();
        }
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

    /// Validate a fresh **trip** upload from its descriptor (epic #526 TR4) — the trip twin of
    /// [`upload_open`](Self::upload_open). The same descriptor-open reject rule against the *trip*
    /// catalog: a new trip past [`MAX_TRIPS`] (or the exhausted id band) → `StorageFull` before any
    /// byte streams; a replace-by-id of an existing trip is exempt. The reference cap is 16 trips
    /// (spec §4.2) — here [`MAX_TRIPS`], the resident cap on this memory profile.
    pub fn upload_open_trip(
        &mut self,
        shared: &SharedStore,
        desc: &TransferControl,
    ) -> Result<Receiver, TransferStatus> {
        let catalog_full = self.trips.is_full() || self.next_trip_id >= SIDELOAD_ID_BASE;
        let id_known = self.trip_index(desc.object_id).is_some();
        if let Some(status) = TransferStatus::upload_open_reject(desc.object_id, id_known, catalog_full) {
            return Err(status);
        }
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

    /// Reserve the announced object's cluster chain before the first payload byte, so the FAT's
    /// per-cluster writes leave the streaming path. Advisory: `false` only means the upload runs at
    /// the old pace. See [`Storage::upload_reserve`](crate::sd::Storage::upload_reserve).
    pub fn upload_reserve(&mut self, shared: &mut SharedStore, total_len: u32) -> bool {
        shared.storage.as_mut().is_some_and(|s| s.upload_reserve(total_len))
    }

    /// Sink one CoC chunk: append to the temp. False = storage failure (the caller aborts).
    pub fn upload_append(&mut self, shared: &mut SharedStore, bytes: &[u8]) -> bool {
        shared.storage.as_mut().is_some_and(|s| s.upload_append(bytes))
    }

    /// The whole link dropped, or the CoC dropped mid-upload, or the app aborted (op 3): discard
    /// the partial upload and release any open storage handles a cancelled future couldn't.
    /// Uploads don't resume, so nothing is kept — the app re-sends from the start.
    ///
    /// **This runs on *either* transport's teardown**, which is the fact everything below turns on:
    /// a phone walking out of range must not disturb a transfer the cable is running. Two things
    /// follow, and both are load-bearing rather than tidy (issue #1039):
    ///
    /// - A volume set is **not** torn down here, for the same reason `map_upload_abort` is not: it
    ///   is gigabytes, and only the cable's own teardown knows the cable went away. The USB data
    ///   plane owns that cleanup at the two points that know it — `discard_upload` mid-transfer,
    ///   and `set_upload_abort` beside this call on endpoint disable.
    /// - `upload_discard` no longer *means* "close whatever file is open". The storage handle
    ///   carries its owner (`sd::UploadOwner`), so this closes the temp and only the temp; a map or
    ///   a set streaming on the other wire keeps its handle. Before that, this call closed the
    ///   cable's file and the next append failed into a discard that deleted the whole upload —
    ///   the same bug the set teardown above was moved out of here to avoid, one layer down.
    pub fn link_reset(&mut self, shared: &mut SharedStore) {
        self.upload_discard(shared);
        if let Some(storage) = &mut shared.storage {
            storage.close_object();
        }
    }

    /// Abort/interrupt: discard the in-flight **temp**. A map or set stream owns its own handle and
    /// its own teardown (`sd::UploadOwner`) — see [`link_reset`](Self::link_reset).
    pub fn upload_discard(&mut self, shared: &mut SharedStore) {
        if let Some(storage) = &mut shared.storage {
            storage.upload_abort();
        }
    }

    /// All bytes arrived: verify + commit. On a CRC match the temp is promoted (fresh id assigned /
    /// replaced file swapped), the revision bumps, and the result carries the assigned id; on a mismatch
    /// nothing is committed and the temp is dropped. Returns `(object_id, status)` for the
    /// `transferResult`.
    ///
    /// `whole_crc` is the verified whole-object CRC-32 from the upload descriptor (the [`Receiver`]
    /// only reaches [`TransferStatus::Committed`] when the streamed bytes matched it), persisted into
    /// the `/routes` content-CRC sidecar under the committed id in the **same movement** (epic #632
    /// item 6) so the next `routeList` carries the route's fingerprint without a lazy re-read.
    pub fn upload_finish(&mut self, shared: &mut SharedStore, rx: &Receiver, whole_crc: u32) -> (u16, TransferStatus) {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return (rx.object_id(), TransferStatus::Error), // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            self.upload_discard(shared);
            return (rx.object_id(), outcome.status);
        }
        let fresh = rx.object_id() == TransferControl::NEW_OBJECT_ID;
        // Fresh-upload dedup (§4.2): a retry of an upload whose commit ack was lost (the link died
        // between the device's commit and the phone's `transferResult`) re-sends the identical bytes
        // as a *new* object — before this check, that minted a silent twin. Content identity IS the
        // whole-object CRC (epic #632), so a fresh upload whose verified CRC + length match a stored
        // route answers `committed` with the **existing** id and stores nothing; the phone links to
        // that id exactly as if the first ack had arrived. Checked before the storage-full backstop:
        // a dedup hit consumes no slot, so a full catalog must not fail it.
        if fresh {
            if let Some(id) = self.find_route_by_content(shared, whole_crc, rx.total_len()) {
                self.upload_discard(shared);
                return (id, TransferStatus::Committed);
            }
        }
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
                        // A fresh route grows the catalog (a replace reuses its slot) — keep `total`
                        // in step so the next `routeList` header's count == total (untruncated).
                        self.route_total = self.route_total.saturating_add(1);
                        id
                    }
                };
                // Persist the verified whole-object CRC into the `/routes` content-CRC sidecar in the
                // same movement (epic #632 item 6) — a replace upserts the id's fingerprint, a fresh
                // upload records it — so the route's `routeList` entry serves its CRC immediately,
                // never lazily. (A side-loaded route, which never passes through here, fills lazily.)
                storage.set_route_crc(id, whole_crc);
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
                        self.route_total = self.route_total.saturating_sub(1);
                        self.bump_revision();
                    }
                }
                (rx.object_id(), TransferStatus::Error)
            }
        }
    }

    /// All bytes arrived: verify + commit a **trip** upload (epic #526 TR4) — the trip twin of
    /// [`upload_finish`](Self::upload_finish). On a CRC match the temp is promoted (fresh trip id
    /// assigned — and its RRAM floor advanced — or the replaced file swapped), the whole-object CRC is
    /// persisted into the trip-CRC sidecar in the same movement, and the **trip** store revision bumps
    /// (never the route store, spec §4.3). Dangling stage refs are stored verbatim (validation is the
    /// app's job, spec §7.7). Returns `(object_id, status)` for the `transferResult`.
    pub fn upload_finish_trip(
        &mut self,
        shared: &mut SharedStore,
        rx: &Receiver,
        whole_crc: u32,
    ) -> (u16, TransferStatus) {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return (rx.object_id(), TransferStatus::Error),
        };
        if outcome.status != TransferStatus::Committed {
            self.upload_discard(shared);
            return (rx.object_id(), outcome.status);
        }
        let fresh = rx.object_id() == TransferControl::NEW_OBJECT_ID;
        // Fresh-upload dedup, the trip twin of the route rule in `upload_finish` (§4.2): a retry
        // whose first commit ack was lost must converge on the stored trip — same-name twin folders
        // were exactly the on-glass duplicate-trip bug. Before the storage-full backstop for the same
        // reason (a dedup hit consumes no trip slot).
        if fresh {
            if let Some(id) = self.find_trip_by_content(shared, whole_crc, rx.total_len()) {
                self.upload_discard(shared);
                return (id, TransferStatus::Committed);
            }
        }
        if fresh && (self.trips.is_full() || self.next_trip_id >= SIDELOAD_ID_BASE) {
            // Storage-full backstop: the catalog filled during the transfer (upload_open_trip already
            // rejects at descriptor-open). Same typed status either way.
            self.upload_discard(shared);
            return (rx.object_id(), TransferStatus::StorageFull);
        }
        let replace_idx = if fresh { None } else { self.trip_index(rx.object_id()) };
        let Some(storage) = &mut shared.storage else { return (rx.object_id(), TransferStatus::Error) };
        let replace_file = replace_idx.map(|i| self.trips[i].file.clone());
        match storage.upload_commit_trip(replace_file.as_ref(), self.next_trip_id) {
            Some((file, byte_len)) => {
                let id = match replace_idx {
                    Some(i) => {
                        self.trips[i].byte_len = byte_len;
                        self.trips[i].file = file;
                        self.trips[i].id
                    }
                    None => {
                        let id = self.next_trip_id;
                        self.next_trip_id += 1;
                        // Advance the persisted trip-id floor (its own RRAM line, spec §4.1) so this id
                        // stays reserved across deletes + reboots.
                        shared.settings.save_trip_mark(self.next_trip_id);
                        let _ = self.trips.push(ObjectSlot { id, file, byte_len });
                        self.trip_total = self.trip_total.saturating_add(1);
                        id
                    }
                };
                // Persist the verified whole-object CRC into the trip-CRC sidecar in the same movement,
                // so the trip's `tripList` entry serves its fingerprint immediately (never lazily).
                storage.set_trip_crc(id, whole_crc);
                // The app-UI trip upload event: the committed id + fresh-vs-replace, published
                // before the revision bump so the STORE_WAKE'd pass sees the rescan edge and this
                // event together. Only a *fresh* trip pops the "TRIP RECEIVED" card — the app
                // suppresses the replace case, a host-side edit (see [`TRIP_UPLOAD_EVENT`]).
                TRIP_UPLOAD_EVENT.store(
                    UPLOAD_EVENT_PRESENT | ((replace_idx.is_some() as u32) << 16) | id as u32,
                    Ordering::Relaxed,
                );
                self.bump_trip_revision();
                (id, TransferStatus::Committed)
            }
            None => {
                if let Some(i) = replace_idx {
                    let gone = shared.storage.as_ref().is_none_or(|s| s.read_trip(&self.trips[i].file).is_none());
                    if gone {
                        self.trips.remove(i);
                        self.trip_total = self.trip_total.saturating_sub(1);
                        self.bump_trip_revision();
                    }
                }
                (rx.object_id(), TransferStatus::Error)
            }
        }
    }

    // ==================== fwImage staging (epic #615 S6, #621) ====================

    /// Validate + arm a `fwImage` upload (spec §4.2 / §7.6): the announce-time size guard — reject an
    /// object past [`MAX_IMAGE_LEN`](obc_dfu::MAX_IMAGE_LEN) with `error` **before any byte streams**
    /// (a ~900 KB update must not transfer only to fail at commit) — then a fresh [`Receiver`]. The SD
    /// temp opens on the first CoC byte via [`upload_begin`](Self::upload_begin), exactly like a route,
    /// so an armed-but-never-opened transfer holds no handle. A `fwImage` carries no object id and no
    /// catalog slot: [`fwimage_finish`](Self::fwimage_finish) promotes it to `/UPDATE.BIN` in the card
    /// root, not into the route catalog.
    pub fn fwimage_open(&mut self, shared: &SharedStore, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        // `total_len` is the whole OBCU container (64-byte header + raw image + — since v2, #997 —
        // the 64-byte signature trailer), so the ceiling must be container-sized too. Gating at the
        // bare `MAX_IMAGE_LEN` would spuriously reject a raw image the armer/engine (which gate the
        // raw `image_len` only) would happily flash (DR5, #733); `MAX_CONTAINER_LEN` carries the
        // arithmetic so the two can't drift apart again.
        if let Some(status) = TransferStatus::fwimage_announce_reject(desc.total_len, obc_dfu::MAX_CONTAINER_LEN) {
            return Err(status);
        }
        // No card ⇒ nowhere to stage; answer now rather than after the CoC opens.
        if shared.storage.is_none() {
            return Err(TransferStatus::Error);
        }
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    /// All bytes arrived: verify the whole-object CRC (via the [`Receiver`] outcome, same as a route)
    /// and, on a match, promote the staged temp to `/UPDATE.BIN` in the card root, **overwriting any
    /// existing one** ([`Storage::commit_fwimage`]). A CRC mismatch discards the temp and leaves no
    /// `/UPDATE.BIN` — nothing durable. Returns the wire [`TransferStatus`] for the `transferResult`.
    ///
    /// Deliberately does **not** bump the store revision or notify `storeChanged`: `/UPDATE.BIN` is not
    /// a listed object, and staging is not installing — the install is armed later by the
    /// `installFw` command's on-glass-confirmed request, never by this commit (spec §7.6).
    pub fn fwimage_finish(&mut self, shared: &mut SharedStore, rx: &Receiver) -> TransferStatus {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return TransferStatus::Error, // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            self.upload_discard(shared); // CRC mismatch / not committed ⇒ no UPDATE.BIN
            return outcome.status;
        }
        let Some(storage) = &mut shared.storage else { return TransferStatus::Error };
        match storage.commit_fwimage() {
            Some(_len) => TransferStatus::Committed,
            None => TransferStatus::Error, // invalid OBCU container or a torn promote (temp dropped)
        }
    }

    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Purely presence; the full CRC scan is the on-device flow's.
    pub fn update_staged(&self, shared: &SharedStore) -> bool {
        shared.storage.as_ref().is_some_and(|s| s.has_update_bin())
    }

    // ==================== the map upload (issue #927) ====================

    /// Validate + arm a **map** upload (spec §4.2 / §10). Three refusals, all before a byte streams,
    /// because a map runs for minutes and a late failure costs the rider all of them:
    /// a named object id (maps are **new-only** — see `Storage`'s map section for why the device
    /// never rewrites a stored map in place), an object too short to be an OBCM at all, and one the
    /// card cannot hold with [`MAP_FREE_HEADROOM`](crate::sd::MAP_FREE_HEADROOM) left over. The rule
    /// itself is the host-tested [`TransferStatus::map_announce_reject`]; the constants are the
    /// board's, exactly as `fwimage_open` passes its own ceiling in.
    ///
    /// Like every other upload, the SD file is **not** opened here — the data plane opens it at the
    /// first streamed byte ([`map_upload_begin`](Self::map_upload_begin)), so an armed transfer whose
    /// stream never starts leaves nothing on the card.
    ///
    /// A map carries no catalog slot in this store: there is no `mapList` on the wire, and the card's
    /// map catalog is derived by a directory scan whenever it is wanted
    /// ([`Storage::scan_maps_into`](crate::sd::Storage::scan_maps_into)) rather than held resident.
    pub fn map_upload_open(
        &mut self,
        shared: &SharedStore,
        desc: &TransferControl,
    ) -> Result<Receiver, TransferStatus> {
        let Some(storage) = shared.storage.as_ref() else { return Err(TransferStatus::Error) };
        let free = storage.card_free_bytes();
        if let Some(status) = TransferStatus::map_announce_reject(
            desc.object_id,
            desc.total_len,
            obc_formats::obcm::HEADER_LEN as u32,
            free,
            crate::sd::MAP_FREE_HEADROOM,
        ) {
            return Err(status);
        }
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    /// Allocate the fresh map object id and open `MP{id}.OBM` for the stream — called by the data
    /// plane at the first byte. Returns the id, which the data plane carries to
    /// [`map_upload_finish`](Self::map_upload_finish) (a map holds no slot in this store, so there is
    /// nothing here to remember it in — and nothing to leak if the transfer dies).
    ///
    /// The id is `max(card scan + 1, RRAM floor)`, the durable-id rule of spec §4.1. The **floor is
    /// bumped here, not at commit**, so an id handed to a transfer is spent even if that transfer
    /// fails: re-issuing it would be safe today (the failed upload stored nothing) but the invariant
    /// worth keeping is the simple one — an id this device has ever named an object with is never
    /// handed out twice within a store epoch.
    pub fn map_upload_begin(&mut self, shared: &mut SharedStore) -> Option<u16> {
        let scan_next = shared.storage.as_ref()?.next_map_id_from_scan();
        let floor = shared.settings.load_map_mark().unwrap_or(0);
        let id = floor.max(scan_next);
        if id == u16::MAX {
            defmt::warn!("store: map id space exhausted — refusing the upload");
            return None;
        }
        shared.settings.save_map_mark(id.saturating_add(1));
        shared.storage.as_mut()?.map_upload_begin(id).then_some(id)
    }

    /// All bytes arrived: verify the whole-object CRC (the [`Receiver`] outcome, as for every other
    /// type) and, on a match, patch the held-back magic into `MP{id}.OBM` — the commit point — then
    /// record the map as the card's **selected** one, so the map a rider just sent is what comes up
    /// on the next boot without a second step.
    ///
    /// Deliberately does **not** bump the store revision or notify `storeChanged`: a map is not a
    /// listed object (there is no `mapList`), so there is no catalog for a peer to re-read. It also
    /// does not touch the running session — the map plane keeps streaming from the map it opened at
    /// boot, because `MapTables` is parsed once into a `.bss` slot the whole ride loop borrows. That
    /// is what the card's "restart to use it" line is telling the rider.
    pub fn map_upload_finish(
        &mut self,
        shared: &mut SharedStore,
        rx: &Receiver,
        id: u16,
        magic: [u8; 4],
    ) -> TransferStatus {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return TransferStatus::Error, // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            // A CRC mismatch: the partial never had its magic patched in, so nothing durable was
            // created — drop it rather than leave its clusters to the next boot's sweep (the retry
            // gets a fresh id and a fresh filename, so it would not reuse this one).
            if let Some(storage) = &mut shared.storage {
                storage.map_upload_abort(id);
            }
            return outcome.status;
        }
        let Some(storage) = &mut shared.storage else { return TransferStatus::Error };
        let Some(_len) = storage.map_upload_commit(id, magic) else { return TransferStatus::Error };
        if let Some(name) = crate::sd::map_file_name_for(id) {
            storage.save_selected_map(&name);
        }
        TransferStatus::Committed
    }

    // ==================== the volume-set upload (issue #1039) ====================
    //
    // The set path is the map path with one thing added: memory between transfers. Each file — every
    // shard, then the manifest — streams exactly as a single map does (straight into its final 8.3
    // name, format magic held back, patched in at commit). What the session holds is the *order*,
    // because `OBCA_Spec.md` §5.4's rule is about order and a device that only saw one transfer at a
    // time could not check it.

    /// Validate + arm a **shard** upload (`OBCA_Spec.md` §5.1). The map guards run unchanged —
    /// long enough to be an OBCM, and a card with room to spare — plus the three the session owns
    /// (`obc_app::shard_announce`): a part field that names a real file of a set, a shard count
    /// inside this board's [`SD_SET_MAX_SHARDS`](crate::sd::SD_SET_MAX_SHARDS) ceiling, and
    /// agreement with the set already in flight.
    ///
    /// The free-space guard is necessarily **per file**: the device is not told the set's total
    /// until the manifest, which by §5.4 is last. A host has the whole projection before it starts
    /// (§5.7 makes that mandatory) and is the right place to refuse a set that cannot fit; this is
    /// the backstop, and it is the same backstop a single map gets.
    ///
    /// Note what is *not* checked: `object_id` is not an object id here (see `obc_ble::SetPart`),
    /// so `map_announce_reject`'s new-only clause would be nonsense and is deliberately not reused.
    /// A shard has no id to target, and the set it belongs to is the one in flight.
    pub fn set_shard_open(
        &self,
        shared: &SharedStore,
        desc: &TransferControl,
    ) -> Result<(Receiver, obc_ble::SetPart), TransferStatus> {
        let Some(storage) = shared.storage.as_ref() else { return Err(TransferStatus::Error) };
        let Some(part) = obc_ble::SetPart::decode(desc.object_id) else {
            return Err(TransferStatus::NotFound);
        };
        let fresh = obc_app::shard_announce(
            self.set_upload.as_ref(),
            part.shard_count,
            part.index,
            crate::sd::SD_SET_MAX_SHARDS as u8,
        )
        .map_err(set_reject_status)?;
        // A set with no id left to mint is refused **here**, at the announce, rather than at the
        // first byte: the answer is the same `storageFull` the shard ceiling gets — a catalog that
        // cannot take another entry, §4.3's own meaning — and the host is told before it opens the
        // pipe instead of after a red storage-failed card. `set_shard_begin` keeps the check as a
        // backstop, because the id is minted there and the card can change under a slow host.
        if fresh && storage.next_set_id_from_scan() > obc_formats::obcs::MAX_SET_ID {
            return Err(TransferStatus::StorageFull);
        }
        if desc.total_len < obc_formats::obcm::HEADER_LEN as u32 {
            return Err(TransferStatus::Error);
        }
        if let Some(free) = storage.card_free_bytes() {
            if desc.total_len as u64 + crate::sd::MAP_FREE_HEADROOM > free {
                return Err(TransferStatus::StorageFull);
            }
        }
        Receiver::new(desc).map(|rx| (rx, part)).map_err(|_| TransferStatus::Error)
    }

    /// Validate + arm the set's **terrain shard** upload (#1044) — the raster that carries the
    /// set's elevation (`OBCA_Spec.md` §5.1's `terrain` role, an OBCT container).
    ///
    /// New-only, like the manifest: there is at most one terrain shard per set, so a named
    /// `object_id` is `notFound` — there is nothing for an id to select. What the session owns is
    /// the ordering (`obc_app::terrain_announce`): a raster with no set in flight names no set at
    /// all, because the set id is minted by the first OBCM shard.
    ///
    /// The free-space and minimum-length guards are the shard path's, against an OBCT header
    /// instead of an OBCM one.
    pub fn set_terrain_open(&self, shared: &SharedStore, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        let Some(storage) = shared.storage.as_ref() else { return Err(TransferStatus::Error) };
        if desc.object_id != TransferControl::NEW_OBJECT_ID {
            return Err(TransferStatus::NotFound);
        }
        obc_app::terrain_announce(self.set_upload.as_ref()).map_err(set_reject_status)?;
        if desc.total_len < obc_formats::obct::HEADER_LEN as u32 {
            return Err(TransferStatus::Error);
        }
        if let Some(free) = storage.card_free_bytes() {
            if desc.total_len as u64 + crate::sd::MAP_FREE_HEADROOM > free {
                return Err(TransferStatus::StorageFull);
            }
        }
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    /// Open the card file for the armed terrain shard — `MS{id}.OBD` under the open session's id.
    /// Unlike a shard this never mints a set: `set_terrain_open` already refused a raster with no
    /// session, and a terrain shard is not something a set can begin with.
    pub fn set_terrain_begin(&mut self, shared: &mut SharedStore) -> Option<u16> {
        let id = self.set_upload.as_ref()?.id();
        if shared.storage.as_mut().is_some_and(|storage| storage.set_terrain_begin(id)) {
            return Some(id);
        }
        // As for a shard: the open truncates, so a failure here has already destroyed any raster a
        // previous attempt committed, and nothing downstream runs to notice.
        self.set_upload.as_mut()?.clear_terrain();
        None
    }

    /// The terrain shard's bytes have all arrived: verify the whole-object CRC, patch the held-back
    /// `OBCT` magic in, and record the raster in the session — the fact
    /// `obc_app::manifest_announce` reads to know the manifest is one record longer.
    ///
    /// A failed raster leaves the **session** open but stops it counting the record, exactly as a
    /// failed shard does: it is one independent file, and the honest recovery is to re-send it
    /// rather than the gigabytes beside it.
    pub fn set_terrain_finish(
        &mut self,
        shared: &mut SharedStore,
        rx: &Receiver,
        id: u16,
        magic: [u8; 4],
    ) -> TransferStatus {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return TransferStatus::Error, // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            if let Some(storage) = &mut shared.storage {
                storage.set_terrain_discard(id);
            }
            self.forget_terrain();
            return outcome.status;
        }
        let Some(storage) = &mut shared.storage else { return TransferStatus::Error };
        if storage.set_terrain_commit(id, magic).is_none() {
            // The commit either deleted `MS{id}.OBD` (it is not a readable OBCT — a wrong file, or
            // a container version this firmware does not read) or left it zero-magic. Either way
            // the card has no raster this set can mount, so the session must stop naming one.
            self.forget_terrain();
            return TransferStatus::Error;
        }
        if let Some(session) = &mut self.set_upload {
            session.mark_terrain();
        }
        TransferStatus::Committed
    }

    /// The session stops counting this set's raster: the card no longer holds a readable
    /// `MS{id}.OBD` (#1044). The terrain twin of [`forget_shard`](Self::forget_shard), and the same
    /// rule — a session never claims a file the card cannot supply.
    fn forget_terrain(&mut self) {
        if let Some(session) = &mut self.set_upload {
            session.clear_terrain();
        }
    }

    /// Validate + arm the **manifest** upload — `OBCA_Spec.md` §5.4's manifest-last rule, enforced
    /// before a byte streams (`obc_app::manifest_announce`). New-only like a map, so a named
    /// `object_id` is `notFound`: the manifest is the set's identity, not a slot to write into.
    pub fn set_manifest_open(&self, shared: &SharedStore, desc: &TransferControl) -> Result<Receiver, TransferStatus> {
        if shared.storage.is_none() {
            return Err(TransferStatus::Error);
        }
        if desc.object_id != TransferControl::NEW_OBJECT_ID {
            return Err(TransferStatus::NotFound);
        }
        obc_app::manifest_announce(self.set_upload.as_ref(), desc.total_len).map_err(set_reject_status)?;
        Receiver::new(desc).map_err(|_| TransferStatus::Error)
    }

    /// Open the card file for one armed shard — called by the data plane at the first streamed
    /// byte, exactly like [`map_upload_begin`](Self::map_upload_begin). Mints the set id and writes
    /// the in-flight token on the *first* shard of a set; joins the open session otherwise.
    /// Returns the set id, which the data plane carries to the commit.
    pub fn set_shard_begin(&mut self, shared: &mut SharedStore, part: obc_ble::SetPart) -> Option<u16> {
        let id = match self.set_upload {
            Some(session) => session.id(),
            None => {
                let id = shared.storage.as_ref()?.next_set_id_from_scan();
                // The backstop behind `set_shard_open`'s announce-time refusal: the host was
                // already told `storageFull` before the pipe opened, so reaching this means the
                // card changed under a slow host.
                if id > obc_formats::obcs::MAX_SET_ID {
                    defmt::warn!("store: volume-set id space exhausted — refusing the upload");
                    return None;
                }
                if !shared.storage.as_mut()?.set_upload_begin(id) {
                    return None;
                }
                self.set_upload = Some(obc_app::SetUpload::new(id, part.shard_count));
                id
            }
        };
        if shared.storage.as_mut().is_some_and(|storage| storage.set_shard_begin(id, part.index as usize)) {
            return Some(id);
        }
        // The open is `ReadWriteCreateOrTruncate`, so by the time it can fail the shard that was
        // under this name is **already gone**. Nothing streams and `set_shard_finish` is never
        // reached, so this is the only place that can keep the session honest about it (#1044).
        self.set_upload.as_mut()?.clear(part.index);
        None
    }

    /// One shard's bytes have all arrived: verify the whole-object CRC, patch the held-back OBCM
    /// magic in, and record the shard as committed in the session — the fact
    /// `obc_app::manifest_announce` later reads to decide whether the manifest may be sent.
    ///
    /// A failed shard leaves the **session** open but stops it counting this shard. Shards are
    /// independent files (§5.4), so the honest recovery is for the host to re-send this one rather
    /// than the gigabytes beside it — and for that to work the session has to admit the shard is
    /// gone, or the manifest sails through its announce and dies at the set-deleting commit.
    pub fn set_shard_finish(
        &mut self,
        shared: &mut SharedStore,
        rx: &Receiver,
        id: u16,
        part: obc_ble::SetPart,
        magic: [u8; 4],
    ) -> TransferStatus {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return TransferStatus::Error, // caller bug: not complete
        };
        if outcome.status != TransferStatus::Committed {
            if let Some(storage) = &mut shared.storage {
                storage.set_shard_discard(id, part.index as usize);
            }
            self.forget_shard(part.index);
            return outcome.status;
        }
        let Some(storage) = &mut shared.storage else { return TransferStatus::Error };
        if storage.set_shard_commit(id, part.index as usize, magic).is_none() {
            // The commit either deleted the shard (its header is not a readable OBCM) or left it
            // zero-magic, which no reader accepts either. Both are "there is no shard here now".
            self.forget_shard(part.index);
            return TransferStatus::Error;
        }
        if let Some(session) = &mut self.set_upload {
            session.mark(part.index);
        }
        TransferStatus::Committed
    }

    /// The session stops counting shard `index`: the card no longer holds a readable one under
    /// that name, whatever the reason (#1044).
    ///
    /// Every failure path of a shard transfer routes through here rather than each remembering to
    /// clear the bit, because the one that did not was the bug: a session claiming a file the card
    /// cannot supply passes the manifest's announce-length check and dies at the *commit*, which
    /// deletes the whole set. Cleared, the same host is refused at the announce with
    /// `manifestEarly` — the answer that says which file to send again.
    fn forget_shard(&mut self, index: u8) {
        if let Some(session) = &mut self.set_upload {
            session.clear(index);
        }
    }

    /// Open the card file for the armed manifest — the same `MS{id}.OBS` the session's token
    /// already occupies, truncated back to its four zero bytes.
    pub fn set_manifest_begin(&mut self, shared: &mut SharedStore) -> Option<u16> {
        let id = self.set_upload.as_ref()?.id();
        shared.storage.as_mut()?.set_manifest_begin(id).then_some(id)
    }

    /// **The set's commit point.** Verify the whole-object CRC, then hand the held-back `OBCS`
    /// magic to the card, which re-reads the manifest, validates it against §5.3 *and* against the
    /// shards actually present, and only then writes those four bytes. On success the set becomes
    /// the card's selected map, exactly as a committed single map does.
    ///
    /// Either way the session closes: a set that committed is finished, and one whose manifest was
    /// refused has already been deleted whole — leaving it half-present is the state §5.4 exists to
    /// make impossible.
    pub fn set_manifest_finish(
        &mut self,
        shared: &mut SharedStore,
        rx: &Receiver,
        id: u16,
        magic: [u8; 4],
    ) -> TransferStatus {
        let outcome = match rx.outcome() {
            Some(o) => o,
            None => return TransferStatus::Error, // caller bug: not complete
        };
        self.set_upload = None;
        if outcome.status != TransferStatus::Committed {
            if let Some(storage) = &mut shared.storage {
                storage.set_upload_abort(id);
            }
            return outcome.status;
        }
        let Some(storage) = &mut shared.storage else { return TransferStatus::Error };
        if storage.set_manifest_commit(id, magic).is_none() {
            return TransferStatus::Error;
        }
        if let Some(name) = obc_formats::obcs::manifest_name(id) {
            if let Ok(short) = ShortFileName::create_from_str(name.as_str()) {
                storage.save_selected_map(&short);
            }
        }
        TransferStatus::Committed
    }

    /// Whether a volume-set upload session is open — i.e. whether a refusal here lands *mid-set*,
    /// with a previous file's outcome already on the glass (#1044).
    pub fn set_upload_active(&self) -> bool {
        self.set_upload.is_some()
    }

    /// Abandon the set in flight: close the session and delete every file of it, token first
    /// (`OBCA_Spec.md` §5.4's ordering, executed by `obc_formats::obcs::delete_plan`). A no-op when
    /// no set is open, so the link-reset path can call it unconditionally.
    pub fn set_upload_abort(&mut self, shared: &mut SharedStore) {
        let Some(session) = self.set_upload.take() else { return };
        if let Some(storage) = &mut shared.storage {
            storage.set_upload_abort(session.id());
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
            ObjectType::RouteList | ObjectType::RideList | ObjectType::TripList => {
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
            // A trip detail download is the same verbatim stream as a route — the stored `TP{id}.OBT`
            // *is* the wire object — out of `/routes` (`ride = false`).
            ObjectType::Trip => {
                let Some(idx) = self.trip_index(desc.object_id) else {
                    return Err(TransferStatus::NotFound);
                };
                let file = self.trips[idx].file.clone();
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
                .is_some_and(|src| obc_formats::io::ByteSource::read_at(&src, offset, buf).is_ok()),
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
    /// committed list as authoritative (it reconciles its on-device link set off it), and a short
    /// list would make it drop a still-present object. `None` → the caller answers a typed `error`
    /// and the app retries, keeping its links.
    ///
    /// The v2 header carries `total` (the pre-cap catalog size, from [`Self::route_total`] /
    /// [`Self::ride_total`]) beside `count`, so a `MAX_ROUTES`/`MAX_RIDES` truncation is visible on
    /// the wire. `routeList` entries also carry the content CRC (see [`Self::build_route_list`]).
    fn build_list(&mut self, shared: &mut SharedStore, ty: ObjectType) -> Option<usize> {
        let (body_len, count, total, entry_len) = match ty {
            ObjectType::RouteList => self.build_route_list(shared)?,
            ObjectType::RideList => self.build_ride_list(shared)?,
            ObjectType::TripList => self.build_trip_list(shared)?,
            // A non-list type never reaches here (`download_open` only calls this for the list
            // types); an empty header keeps the arm total.
            _ => (ListHeader::ENCODED_LEN, 0, 0, RideListEntry::ENTRY_LEN as u8),
        };
        self.list_buf[..ListHeader::ENCODED_LEN].copy_from_slice(&ListHeader { count, total }.encode(entry_len));
        Some(body_len)
    }

    /// Build the `routeList` into [`Self::list_buf`] — the v2 path that also serves each entry's
    /// content CRC-32 (epic #632 item 6). The CRC comes from the `/routes` sidecar; a route with no
    /// entry (side-loaded, or a pre-v2 stock card) is **lazily filled** here — one streaming
    /// whole-object CRC pass, then the whole sidecar is persisted once. A transient CRC-read failure
    /// serves `0 = unknown` (the sidecar stays unfilled, so the next build retries); a genuine CRC of
    /// `0` is a legal value served as `0`, never special-cased. Returns `(body_len, count, total,
    /// entry_len)`. `None` on a transient header-read failure (fails the whole list — see
    /// [`Self::build_list`]).
    fn build_route_list(&mut self, shared: &mut SharedStore) -> Option<(usize, u16, u16, u8)> {
        let Some(storage) = shared.storage.as_mut() else {
            return Some((ListHeader::ENCODED_LEN, 0, 0, RouteListEntry::ENTRY_LEN as u8));
        };
        let mut crcs = storage.load_route_crcs();
        // The per-route retention state (auto-expiry epic #638 S4): one sidecar read for the whole
        // build. Each entry's `expires_at` is device-computed (`last_used + retention days`, `0` for
        // Never or an unstarted clock) — **volatile** state, so it rides the entry tail *after* the
        // content crc32, never conflated with the route-content fingerprint.
        let retention = storage.load_route_retention();
        let mut crcs_dirty = false;
        let mut off = ListHeader::ENCODED_LEN;
        let mut count: u16 = 0;
        for i in 0..self.routes.len() {
            let id = self.routes[i].id;
            let file = self.routes[i].file.clone();
            let (byte_len, info) = storage.route_object_info(&file)?; // transient → fail whole list
            let crc = match crcs.get(id) {
                Some(c) => c,
                // Lazy fill: stream the whole file once through the detail-download handle slot
                // (idle during a list build) to compute the CRC, then remember it as dirty.
                None => match storage.open_object(&file).and_then(|len| {
                    let computed = object_crc(storage, len);
                    storage.close_object();
                    computed
                }) {
                    Some(c) => {
                        if crcs.insert(id, c) {
                            crcs_dirty = true;
                        }
                        c
                    }
                    None => RouteListEntry::CRC_UNKNOWN, // transient read failure → 0 = unknown
                },
            };
            let meta = retention.get(id);
            let entry = RouteListEntry {
                object_id: id,
                byte_len,
                distance_m: info.distance_m,
                ascent_m: info.ascent_m,
                point_count: info.point_count,
                waypoint_count: info.waypoint_count,
                name: info.name.as_bytes(),
                crc32: crc,
                // Auto-expiry tail: `expires_at = last_used + retention days` (0 = Never / unstarted).
                expires_at: meta.expires_at().unwrap_or(0),
                retention: meta.retention.as_u8(),
            }
            .encode();
            self.list_buf[off..off + RouteListEntry::ENTRY_LEN].copy_from_slice(&entry);
            off += RouteListEntry::ENTRY_LEN;
            count += 1;
        }
        // Persist any lazy fills in one write (skipped when every route already had a sidecar entry).
        if crcs_dirty {
            storage.write_route_crcs(&crcs);
        }
        Some((off, count, self.route_total, RouteListEntry::ENTRY_LEN as u8))
    }

    /// Build the `rideList` into [`Self::list_buf`] — unchanged 72-byte entries (no content CRC).
    /// Returns `(body_len, count, total, entry_len)`; `None` on a transient header-read failure.
    fn build_ride_list(&mut self, shared: &SharedStore) -> Option<(usize, u16, u16, u8)> {
        let Some(storage) = shared.storage.as_ref() else {
            return Some((ListHeader::ENCODED_LEN, 0, 0, RideListEntry::ENTRY_LEN as u8));
        };
        let mut off = ListHeader::ENCODED_LEN;
        let mut count: u16 = 0;
        for slot in &self.rides {
            let (byte_len, info) = storage.ride_object_info(&slot.file)?; // transient → fail whole list
            let entry = RideListEntry {
                object_id: slot.id,
                byte_len,
                start_time: info.start_time,
                distance_m: info.distance_m,
                moving_time_s: info.moving_time_s,
                avg_speed_cms: info.avg_speed_cms,
                climb_m: info.climb_m,
                name: info.name.as_bytes(),
            }
            .encode();
            self.list_buf[off..off + RideListEntry::ENTRY_LEN].copy_from_slice(&entry);
            off += RideListEntry::ENTRY_LEN;
            count += 1;
        }
        Some((off, count, self.ride_total, RideListEntry::ENTRY_LEN as u8))
    }

    /// Build the `tripList` into [`Self::list_buf`] (spec §7.4) — the trip twin of
    /// [`build_route_list`](Self::build_route_list). Each entry's totals are summed over the trip's
    /// **resolvable** stages (each stage route id looked up in the route catalog; a dangling ref
    /// contributes nothing), while `stage_count` is the trip's stored count — so `stage_count` can
    /// exceed the number of stages the totals drew from. The trailing content `crc32` comes from the
    /// `/routes` trip-CRC sidecar; a trip with no entry (side-loaded) is **lazily filled** here (one
    /// whole-object CRC pass), then the sidecar is persisted once. A transient CRC-read failure serves
    /// `0 = unknown`. Returns `(body_len, count, total, entry_len)`; `None` on a transient trip-header
    /// read failure (fails the whole list — see [`build_list`](Self::build_list)).
    fn build_trip_list(&mut self, shared: &mut SharedStore) -> Option<(usize, u16, u16, u8)> {
        let Some(storage) = shared.storage.as_mut() else {
            return Some((ListHeader::ENCODED_LEN, 0, 0, TripListEntry::ENTRY_LEN as u8));
        };
        let mut crcs = storage.load_trip_crcs();
        let mut crcs_dirty = false;
        let mut off = ListHeader::ENCODED_LEN;
        let mut count: u16 = 0;
        for i in 0..self.trips.len() {
            let id = self.trips[i].id;
            let file = self.trips[i].file.clone();
            let (byte_len, meta, stage_count) = storage.read_trip(&file)?; // transient → fail whole list
                                                                           // Sum distance/ascent over the trip's resolvable stages; a dangling stage id (no route
                                                                           // with it) is skipped, contributing nothing.
            let mut total_distance_m: u32 = 0;
            let mut total_ascent_m: u32 = 0;
            for stage_id in &meta.stage_ids {
                if let Some(route_file) = self.routes.iter().find(|s| s.id == *stage_id).map(|s| s.file.clone()) {
                    if let Some((_, info)) = storage.route_object_info(&route_file) {
                        total_distance_m = total_distance_m.saturating_add(info.distance_m);
                        total_ascent_m = total_ascent_m.saturating_add(info.ascent_m);
                    }
                }
            }
            let crc = match crcs.get(id) {
                Some(c) => c,
                // Lazy fill: stream the whole trip file once to compute its CRC (the tiny file is a
                // fast pass), then remember it as dirty for the single persist below.
                None => match storage.open_object(&file).and_then(|len| {
                    let computed = object_crc(storage, len);
                    storage.close_object();
                    computed
                }) {
                    Some(c) => {
                        if crcs.insert(id, c) {
                            crcs_dirty = true;
                        }
                        c
                    }
                    None => TripListEntry::CRC_UNKNOWN,
                },
            };
            let entry = TripListEntry {
                object_id: id,
                byte_len,
                total_distance_m,
                total_ascent_m,
                stage_count,
                name: meta.name.as_bytes(),
                crc32: crc,
            }
            .encode();
            self.list_buf[off..off + TripListEntry::ENTRY_LEN].copy_from_slice(&entry);
            off += TripListEntry::ENTRY_LEN;
            count += 1;
        }
        if crcs_dirty {
            storage.write_trip_crcs(&crcs);
        }
        Some((off, count, self.trip_total, TripListEntry::ENTRY_LEN as u8))
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
        obc_formats::io::ByteSource::read_at(&src, off, &mut buf[..n]).ok()?;
        crc.update(&buf[..n]);
        off += n as u32;
    }
    Some(crc.finalize())
}
