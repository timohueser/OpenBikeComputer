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
use obc_app::{Retention, Settings, MAX_ROUTES};
use obc_ports::SettingsStore;
use obc_storage::route_name;

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
/// Published before the revision bump that follows it, so the pass the
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
/// *fresh* trip — a delivery — is announced. Published before the revision bump that follows it,
/// and drained by the ride loop strictly *after* the route event so the
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
}

/// Ride catalog capacity. Rides accumulate — the device keeps every tracked ride until a (future)
/// manual delete — so this is roomier than [`MAX_ROUTES`]; past it the newest rides stop being listed
/// (warned at scan) until the card is tidied.
pub const MAX_RIDES: usize = 128;

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
    /// The stored rides: scanned at boot and re-scanned on the saved-ride edge ([`RIDE_SAVED`]) —
    /// since the de-split the `ble` build *is* the map build, so the ride loop records new rides
    /// mid-session and this catalog must follow (it feeds the `rideList` object the phone syncs
    /// against).
    rides: Vec<ObjectSlot, MAX_RIDES>,
    /// The next fresh-upload object id (ids are never reused within a boot).
    next_id: u16,
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
    pub const EMPTY: ObjectStore = ObjectStore {
        settings: Settings::DEFAULT,
        routes: Vec::new(),
        rides: Vec::new(),
        next_id: 0,
        revision: 1,
        trip_revision: 1,
        route_total: 0,
        ride_total: 0,
    };

    /// Mount-time fill of an [`EMPTY`](Self::EMPTY) store, **in place**: load settings, scan
    /// `/routes` into the id table, and sweep aborted commits (files whose held-back magic never
    /// got patched — see `sd.rs`). Runs under a boot-time lock of the shared store (`shared`),
    /// which it borrows for the settings load + scans.
    pub fn hydrate(&mut self, shared: &mut SharedStore) {
        self.settings = shared.settings.load().unwrap_or_default();
        self.rescan(shared);
        self.rescan_rides(shared);
        // Seed the canonical trip repository for every link-store composition point. Normal boot
        // already scanned once to build the pre-link App, but map-recovery USB calls `init_store`
        // before App construction and depends on this scan for trip visibility and id recovery.
        // Both scans finish before a link plane is published; runtime App reloads stay scan-free.
        if let Some(storage) = &mut shared.storage {
            storage.trips().scan();
        }
        // The durable id floor (#450): fresh upload ids start at `max(scan_max + 1, stored floor)`,
        // so an id deleted last session can't be re-issued (the phone's persisted `deviceObjectID`s
        // key on it). A blank/torn line is "no floor" → exactly the old scan-derived start.
        if let Some(m) = shared.settings.load_id_marks() {
            self.next_id = self.next_id.max(m.next_route_id);
        }
        // **There is no trip-id floor any more.** It drew from its own RRAM line, written by the
        // `deviceObjectID`-minting path that arrived over the cable — and that writer went with the
        // v1 command surface (FS7.5-c3b). A read whose writer is deleted answers the blank line
        // forever, which is not a floor, it is a decode of nothing; the standing rule is that a
        // never-exercised capability goes rather than lingering as a call that always returns
        // `None`. When trips come back over a link that can carry them, so does the line.
        if let Some(storage) = &mut shared.storage {
            let trips = storage.trips();
            let len = trips.len();
            let candidate = trips.candidate().unwrap_or(SIDELOAD_ID_BASE);
            defmt::info!("store: {=usize} trip object(s), next trip id {=u16}", len, candidate);
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
                Some(_) => {
                    let id = match route_name::uploaded_id(name.base_name(), name.extension()) {
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
                    let _ = self.routes.push(ObjectSlot { id, file: name.clone() });
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

    /// Whether a trip object with this id exists (the control plane's cheap `notFound` check).
    pub fn has_trip(&self, shared: &mut SharedStore, id: u16) -> bool {
        shared.storage.as_mut().is_some_and(|storage| storage.trips().contains(id))
    }

    fn slot_index(&self, id: u16) -> Option<usize> {
        self.routes.iter().position(|s| s.id == id)
    }

    /// Whether a route object with this id exists (the control plane's cheap `notFound` check).
    pub fn has_route(&self, id: u16) -> bool {
        self.slot_index(id).is_some()
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
        let Some(storage) = &mut shared.storage else { return false };
        if !storage.trips().delete(id) {
            return false;
        }
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
        // Snapshot member ids before any route delete re-borrows Storage; the Trips view never
        // survives a nested repository mutation (and never crosses an await).
        let stages = shared.storage.as_mut().and_then(|storage| storage.trips().stage_ids(id));
        if stages.is_none() && !self.has_trip(shared, id) {
            return false;
        }
        if let Some(stages) = stages {
            for stage_id in stages {
                if let Ok(stage_id) = u16::try_from(stage_id) {
                    let _ = self.delete_route(shared, stage_id); // dangling → false, skipped
                }
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

    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Purely presence; the full CRC scan is the on-device flow's.
    pub fn update_staged(&self, shared: &SharedStore) -> bool {
        shared.storage.as_ref().is_some_and(|s| s.has_update_bin())
    }

    /// The active bundle's identity for the request context's bundle group (§11.4 validity bit 3)
    /// and the scheduler's age input — the boot/commit-refreshed slot selection, no card I/O.
    pub fn weather_active(&self, shared: &SharedStore) -> Option<obc_weather::Candidate> {
        shared.storage.as_ref().and_then(|s| s.weather_active())
    }
}
