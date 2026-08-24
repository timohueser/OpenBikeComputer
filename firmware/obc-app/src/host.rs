//! The typed app ↔ host coordination protocol (FAR-07, #800).
//!
//! `App` asks its host to do exactly one kind of thing — run a bounded piece of I/O it cannot do
//! itself (store scans, file deletes, the router, the DFU armer, settings persistence) — and the
//! host answers with facts. Two bounded enums define that shared vocabulary:
//!
//! - [`HostCommand`] — everything the app can ask of a host, drained through
//!   [`App::drain_host_commands`](crate::App::drain_host_commands) into a caller-owned
//!   [`HostMailbox`].
//! - [`HostEvent`] — every host answer/fact, applied through
//!   [`App::apply_event`](crate::App::apply_event).
//!
//! ## What is deliberately *not* in the protocol
//!
//! - **The input/overlay plane and repaint scheduling.** `take_dirty` and `take_hold_cancel` are
//!   the render/input loop's own high-priority coordination — routing button edges or bulge frames
//!   through a per-pass mailbox would insert the map plane into the preemptive hold path (an
//!   epic-level prohibition). They stay latch-shaped by design.
//! - **Bulk resident data.** Catalogs, profiles, previews, sensor snapshots, and settings blobs
//!   keep their `set_*` feeder methods: queue elements carry bounded ids/revisions/small results,
//!   never catalogs or profiles.
//! - **Levels.** `sensor_scan_active` (scan mode on/off, the `set_radio_enabled` shape) is
//!   continuous desired state re-read every pass, not a consumable message. The two *derived fill
//!   cues* — the ride-track request and the missing nav preview — are levels too, but they are
//!   surfaced as re-emitted commands ([`HostCommand::LoadRideTrack`] /
//!   [`HostCommand::RefreshNavPreview`]) so a typed host still sees them; they re-appear on every
//!   drain until the matching `set_*` answer lands.
//!
//! ## Per-class pending state, saturation, and coalescing
//!
//! Every command class has **exactly one** pending instance inside `App` (a typed slot, a counter,
//! or a derived predicate) — there is no internal queue to exhaust, and no allocation. Posting
//! policy per class is documented on each variant; the notable ones:
//!
//! - *Counted*: [`RescanStore`](HostCommand::RescanStore) carries the number of store commits since
//!   the last drain — a burst is never coalesced into a lost rescan.
//! - *Most-recent-wins*: a later same-class post replaces an undrained earlier one where the flow
//!   makes that intentional (the DFU phase slot; the advisory upload prompt is app-internal and
//!   keeps the same rule). Destructive classes share the mechanics but are modal-flow-guarded: the
//!   UI cannot post a second delete/plan before the per-pass drain, so replacement is unreachable
//!   in practice.
//! - *Backpressure, never loss*: [`App::drain_host_commands`](crate::App::drain_host_commands)
//!   moves a command into the mailbox **only if room exists**; a full mailbox leaves the remaining
//!   classes latched (and returns [`DrainStatus::MailboxFull`]) so a destructive or persistence
//!   command is never silently dropped. A mailbox with `N >= HOST_COMMAND_CLASSES` (compile-time
//!   asserted) always completes in one drain.

use obc_ports::SettingsSaveError;

use crate::activity::{DetourRequest, DfuAction, NavRequest, TrackAction};
use crate::dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport, Version};
use crate::screen::WarningFlags;

/// What the board host must do when a computed-route publication answers. Cancellation can arrive
/// while the synchronous store task is committing, so the answer is not automatically a success:
/// the just-published revision must be removed before the host reports the cancellation complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavPublishDisposition {
    Activate(crate::CatalogObjectId),
    Compensate(crate::CatalogObjectId),
}

pub const fn nav_publish_disposition(cancel_requested: bool, id: crate::CatalogObjectId) -> NavPublishDisposition {
    if cancel_requested {
        NavPublishDisposition::Compensate(id)
    } else {
        NavPublishDisposition::Activate(id)
    }
}

/// Store-task result categories relevant to retracting a route whose publication raced cancel.
/// Kept independent of a concrete store error type so the app/host state machine remains portable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavCompensationStatus {
    /// The exact published revision was removed.
    Removed,
    /// The exact revision is already absent (for example a later replacement removed it first).
    Absent,
    /// Media or scheduling failure that can succeed on a later pass.
    Retry,
    /// A permanent store refusal. The host must release its planner resources rather than spin
    /// forever; the board logs this as a violated publish-capacity invariant.
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavCompensationDisposition {
    Cancelled,
    Retry,
    CancelledAfterTerminalFailure,
}

pub const fn nav_compensation_disposition(status: NavCompensationStatus) -> NavCompensationDisposition {
    match status {
        NavCompensationStatus::Removed | NavCompensationStatus::Absent => NavCompensationDisposition::Cancelled,
        NavCompensationStatus::Retry => NavCompensationDisposition::Retry,
        NavCompensationStatus::Terminal => NavCompensationDisposition::CancelledAfterTerminalFailure,
    }
}

/// Everything the app can ask its host to do — the typed successor of the per-feature `take_*`
/// latches. Drained in the fixed [`HostCommand::DRAIN_ORDER`] by
/// [`App::drain_host_commands`](crate::App::drain_host_commands); each variant documents its
/// pending-state shape and posting policy.
///
/// Payloads are bounded by construction: durable `u16` object ids, the `Copy` [`NavRequest`]
/// (fixed 24-byte name buffer), and small `Copy` enums. No catalog, profile, or geometry ever
/// rides in a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostCommand {
    /// Rescan the object store and re-feed the catalogs
    /// ([`set_routes_with_ids`](crate::App::set_routes_with_ids) + trips/rides): `commits` store
    /// commits/deletes landed since the last drain. **Counted** — a burst between drains raises
    /// the count (saturating), never a lost edge; one rescan covers them all.
    RescanStore { commits: u32 },
    /// Abort the in-flight route plan and discard the partial nav file (#499). Answers nothing —
    /// the planning screen is already gone. One-shot, drained before [`PlanRoute`]; the two slots
    /// interact by **post-time annihilation**: posting a cancel clears a still-undrained plan
    /// request (a confirm→Back inside one input batch nets "no plan" — the host must never
    /// execute a plan whose spinner the rider already dismissed and commit a ghost route), so a
    /// drained cancel always refers to a plan the host already holds, and any `PlanRoute` behind
    /// it in the mailbox was posted **after** the cancel.
    ///
    /// [`PlanRoute`]: HostCommand::PlanRoute
    CancelRoutePlan,
    /// Abort the in-flight detour plan **and** drop any planned-but-uncommitted detour bytes the
    /// host still holds (#882) — Back on the detour planning or preview screen. Answers nothing.
    /// One-shot with the same **post-time annihilation** as
    /// [`CancelRoutePlan`](HostCommand::CancelRoutePlan): posting a cancel clears a still-undrained
    /// [`PlanDetour`](HostCommand::PlanDetour) request, so a drained cancel always refers to work
    /// the host actually holds.
    CancelDetour,
    /// Delete the route with durable object id `id` (epic #447, P6); the store-changed edge then
    /// re-feeds the catalog. The pending state is the menu's catalog *index*, resolved to the id
    /// at drain against the live catalog — a request whose route vanished drains to nothing.
    /// One-shot, modal-flow-guarded (hold-to-delete → per-pass drain).
    DeleteRoute { id: crate::CatalogObjectId },
    /// Cascade-delete the trip with durable object id `id` **and every member route** (epic #526,
    /// TR3). Already id-shaped (a trip id is durable); a vanished trip is a host-side no-op.
    /// One-shot, modal-flow-guarded.
    DeleteTrip { id: crate::CatalogObjectId },
    /// Delete the ride with durable object id `id` (epic #447, P7) — the ride-namespace twin of
    /// [`DeleteRoute`](HostCommand::DeleteRoute), index-resolved at drain the same way.
    /// One-shot, modal-flow-guarded. Also the auto-expiry sweep's ride-delete (epic #638, S3):
    /// a synced-and-aged-out ride leaves through this exact command, so the host deletes it through
    /// the same store path (revision bump + `storeChanged`) and a connected phone reconciles it like
    /// any other delete.
    DeleteRide { id: crate::CatalogObjectId },
    /// Stamp route `id`'s `last_used` to `utc` in the SD route-retention sidecar (auto-expiry epic
    /// #638, S3) — the sweep's "start the clock on an unknown stamp" and "re-stamp the active route
    /// so it never expires under a ride" writes, and the once-per-activation stamp. **Not** a store
    /// delete: it is a device-local sidecar write, no revision bump. Drained from the sweep queue,
    /// one per pass; the host applies it and the next scan re-feeds the app the fresh meta.
    StampRouteUsed { id: crate::CatalogObjectId, utc: u32 },
    /// Stamp ride `id`'s `synced_at` to `utc` in the extended synced-set sidecar (auto-expiry epic
    /// #638, S3) — the sweep's "start the countdown on a legacy synced-without-stamp ride" write.
    /// Only ever fills a `0` stamp (the host never re-stamps). Sidecar write, no revision bump.
    StampRideSynced { id: crate::CatalogObjectId, utc: u32 },
    /// Close the open ride log: finalise it to the host's saved-ride artifact
    /// ([`TrackAction::Save`]) or throw it away ([`TrackAction::Discard`]). Persistence-critical
    /// one-shot; the host reads [`ride_stats`](crate::App::ride_stats) in the same pass so the
    /// wall-clock anchor pairs with the log's last points.
    FinishTrack(TrackAction),
    /// Run the on-device router from `from` to `to` (epic #116, R4): write the emitted OBCR to the
    /// reserved nav route, rescan, and answer with [`HostEvent::NavPlanned`]. One-shot; the
    /// confirm-screen flow guarantees at most one plan is posted per drain, and a
    /// [`CancelRoutePlan`](HostCommand::CancelRoutePlan) posted while this request is still
    /// undrained **annihilates it** (see that variant) — a request the host receives was never
    /// cancelled before it left the app.
    PlanRoute(NavRequest),
    /// Plan a routed detour (#882): resolve the rejoin coordinate at `target_m` on the active
    /// route, build the corridor blacklist over `[progress_m, target_m]`, and run the detour A*
    /// from `from` into a host-held detour OBCR; answer with [`HostEvent::DetourPlanned`] (and
    /// feed the preview polyline via [`set_detour_preview`](crate::App::set_detour_preview)).
    /// One-shot; a [`CancelDetour`](HostCommand::CancelDetour) posted while this is undrained
    /// annihilates it, exactly like the [`PlanRoute`](HostCommand::PlanRoute) pair.
    PlanDetour(DetourRequest),
    /// Commit the planned detour (#882): splice the held detour bytes into the active route
    /// (Phase B), write the derived OBCR to the reserved computed-route slot, rescan, and answer
    /// with [`HostEvent::DetourCommitted`]. Persistence-critical one-shot, modal-flow-guarded
    /// (only the preview screen's Press posts it, once).
    CommitDetour,
    /// Run a DFU phase (epic #615): validate `UPDATE.BIN` ([`DfuAction::Scan`], answered by
    /// [`HostEvent::DfuScanned`]) or arm-and-reboot ([`DfuAction::Install`], which either never
    /// returns or answers [`HostEvent::DfuInstallFailed`]). Single slot, **most-recent-wins by
    /// design**: there is never more than one DFU phase in flight, and a later rider post
    /// supersedes an undrained earlier one (the remote BLE door defers instead — see
    /// [`open_remote_dfu_check`](crate::App::open_remote_dfu_check)).
    Dfu(DfuAction),
    /// Forget the paired phone (epic #447, P8): clear the bond store and drop the bonded
    /// connection. One-shot, guarded-hold-posted.
    ForgetBond,
    /// Persist the live [`settings`](crate::App::settings) at revision `revision` (#810). Emitted
    /// once when an edited settings value leaves the settings subtree (the save is debounced to
    /// screen exit, not fired per step) and **not re-emitted while its ack is outstanding**, so a
    /// slow host is never spammed with RRAM writes. The command carries the app's current settings
    /// revision; under the **snapshot-at-drain rule** the host reads [`settings`](crate::App::settings)
    /// in the same pass it drains this command (no `Settings` copy ever rides in the queue) and later
    /// answers with [`HostEvent::SettingsPersisted`] or [`HostEvent::SettingsPersistFailed`] carrying
    /// the same `revision`. A failed write keeps the revision dirty and retryable; a newer edit bumps
    /// the revision and supersedes an older pending one (a stale ack cannot clear the newer state).
    /// A host that drains this command but never acks it (the web demo has no persistent store)
    /// leaves the app parked in Awaiting terminally — by design: harmless (edits stay live in RAM
    /// and keep superseding), honest (nothing pretends the write landed), no re-emission.
    PersistSettings { revision: u16 },
    /// Run the FAT free-cluster scan and answer with [`HostEvent::CardScanned`] (T8 item 6).
    /// One-shot per System-screen entry; idempotent refresh.
    ScanCardFree,
    /// Stream ride `id`'s recorded track once and answer with
    /// [`set_ride_profile`](crate::App::set_ride_profile) /
    /// [`set_ride_preview`](crate::App::set_ride_preview) (#680). **Derived level, not a stored
    /// one-shot**: re-emitted on every drain while the open Ride detail's viewed ride is
    /// unanswered, and gone the moment the answer (even a failure's `None`) parks under the viewed
    /// key — so a missed pass re-asks and a dead file never grinds.
    LoadRideTrack { id: crate::CatalogObjectId },
    /// Decimate the active route's shape polyline and hand it to
    /// [`set_nav_preview`](crate::App::set_nav_preview) (#685 §4). **Derived level** like
    /// [`LoadRideTrack`](HostCommand::LoadRideTrack): re-emitted while a Route overview is up
    /// without its preview.
    RefreshNavPreview,
}

/// One command class per [`HostCommand`] variant — the drain iterates these; the discriminant
/// doubles as the canonical drain order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostCommandClass {
    RescanStore,
    CancelRoutePlan,
    CancelDetour,
    DeleteRoute,
    DeleteTrip,
    DeleteRide,
    StampRouteUsed,
    StampRideSynced,
    FinishTrack,
    PlanRoute,
    PlanDetour,
    CommitDetour,
    Dfu,
    ForgetBond,
    PersistSettings,
    ScanCardFree,
    LoadRideTrack,
    RefreshNavPreview,
}

/// How many command classes exist — the [`HostMailbox`] capacity at which one
/// [`drain_host_commands`](crate::App::drain_host_commands) call is guaranteed to complete
/// (compile-time asserted there).
pub const HOST_COMMAND_CLASSES: usize = HostCommand::DRAIN_ORDER.len();

impl HostCommand {
    /// The canonical drain order, fixed and documented: refresh state first (`RescanStore`),
    /// cancellations before new work (`CancelRoutePlan` strictly before `PlanRoute`), then
    /// destructive/persistence one-shots, then new work, then idempotent refreshes, then the
    /// derived fill cues. Within one input batch this order — not gesture arrival — is what a
    /// typed host observes. The one arrival-order fact that matters — a plan confirmed and
    /// cancelled inside the same batch must net "no plan" — is
    /// enforced at **post time**, not here: the cancel annihilates the undrained request (see
    /// [`HostCommand::CancelRoutePlan`]), so this order alone never hands the host a
    /// dead-on-arrival plan.
    pub(crate) const DRAIN_ORDER: [HostCommandClass; 18] = [
        HostCommandClass::RescanStore,
        HostCommandClass::CancelRoutePlan,
        HostCommandClass::CancelDetour,
        HostCommandClass::DeleteRoute,
        HostCommandClass::DeleteTrip,
        HostCommandClass::DeleteRide,
        HostCommandClass::StampRouteUsed,
        HostCommandClass::StampRideSynced,
        HostCommandClass::FinishTrack,
        HostCommandClass::PlanRoute,
        HostCommandClass::PlanDetour,
        HostCommandClass::CommitDetour,
        HostCommandClass::Dfu,
        HostCommandClass::ForgetBond,
        HostCommandClass::PersistSettings,
        HostCommandClass::ScanCardFree,
        HostCommandClass::LoadRideTrack,
        HostCommandClass::RefreshNavPreview,
    ];

    /// This command's class (used for mailbox coalescing).
    pub(crate) fn class(&self) -> HostCommandClass {
        match self {
            HostCommand::RescanStore { .. } => HostCommandClass::RescanStore,
            HostCommand::CancelRoutePlan => HostCommandClass::CancelRoutePlan,
            HostCommand::CancelDetour => HostCommandClass::CancelDetour,
            HostCommand::DeleteRoute { .. } => HostCommandClass::DeleteRoute,
            HostCommand::DeleteTrip { .. } => HostCommandClass::DeleteTrip,
            HostCommand::DeleteRide { .. } => HostCommandClass::DeleteRide,
            HostCommand::StampRouteUsed { .. } => HostCommandClass::StampRouteUsed,
            HostCommand::StampRideSynced { .. } => HostCommandClass::StampRideSynced,
            HostCommand::FinishTrack(_) => HostCommandClass::FinishTrack,
            HostCommand::PlanRoute(_) => HostCommandClass::PlanRoute,
            HostCommand::PlanDetour(_) => HostCommandClass::PlanDetour,
            HostCommand::CommitDetour => HostCommandClass::CommitDetour,
            HostCommand::Dfu(_) => HostCommandClass::Dfu,
            HostCommand::ForgetBond => HostCommandClass::ForgetBond,
            HostCommand::PersistSettings { .. } => HostCommandClass::PersistSettings,
            HostCommand::ScanCardFree => HostCommandClass::ScanCardFree,
            HostCommand::LoadRideTrack { .. } => HostCommandClass::LoadRideTrack,
            HostCommand::RefreshNavPreview => HostCommandClass::RefreshNavPreview,
        }
    }
}

/// Every fact or answer a host can hand the app — the typed successor of the `notify_*` latches,
/// applied through [`App::apply_event`](crate::App::apply_event). Events are **owned** values (no
/// borrow into `App` or the host), so a host can build one whenever its asynchronous work
/// completes and apply it on a later pass. Payloads are bounded: ids, small `Copy` enums, fixed
/// version strings, and the ≤ 64-byte upload sparkline — bulk answers (profiles, previews,
/// catalogs) stay on their `set_*` feeders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// The object store committed or deleted an object (epic #447) — raises the counted
    /// [`HostCommand::RescanStore`] cue.
    StoreChanged,
    /// A route upload committed to the store (epic #447, P4): `id` is the durable object id
    /// (resolved against the **already rescanned** catalog — the rescan-then-resolve ordering
    /// contract), `replaced` says the bytes of a stored route were swapped, `elevation` is the
    /// commit-time mini sparkline for the idle prompt. The advisory prompt keeps its documented
    /// single-slot **most-recent-wins** delivery.
    RouteUploaded { id: crate::CatalogObjectId, replaced: bool, elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]> },
    /// A **trip** upload committed to the store (epic #526): `id` is the trip's durable object id,
    /// resolved against the **already re-fed** trip catalog (the same rescan-then-resolve ordering
    /// contract as [`RouteUploaded`](HostEvent::RouteUploaded)). A **fresh** trip (`replaced ==
    /// false`) raises the "TRIP RECEIVED" popup — and, because a trip object always arrives *after*
    /// its member routes, replaces the last per-route popup of the upload burst (the single-slot
    /// most-recent-wins delivery). A **replace** (`replaced == true`) is silent: hosts edit a trip
    /// exclusively by replace-at-same-id (the desktop's rename / add / remove / reorder is one
    /// upload *per click*), so announcing each would be a popup parade — the user just made the
    /// change and needs no card. Deliberately not the route family's behavior: a route replace can
    /// force adoption mid-ride, a trip replace changes no navigation state at all.
    TripUploaded { id: crate::CatalogObjectId, replaced: bool },
    /// One or more device warnings were discovered (issue #504); flags accumulate onto the single
    /// dismissable card, each surfaced once per boot.
    Warning(WarningFlags),
    /// The answer to [`HostCommand::PlanRoute`]: the committed nav route's durable id, or the
    /// typed failure. Lands in the planning screen; dropped if the rider already cancelled.
    NavPlanned(Result<crate::CatalogObjectId, obc_route::nav::NavError>),
    /// The answer to [`HostCommand::PlanDetour`] (#882): the preview figures, or the typed
    /// failure (mapped to the "try a farther rejoin" presentation). The preview *polyline* rides
    /// its own [`set_detour_preview`](crate::App::set_detour_preview) feeder, never the event.
    DetourPlanned(Result<DetourPreview, obc_route::nav::NavError>),
    /// The answer to [`HostCommand::CommitDetour`] (#882): the spliced route's durable id (the
    /// re-adoption key), or the typed failure (the old route stays untouched).
    DetourCommitted(Result<crate::CatalogObjectId, obc_route::nav::NavError>),
    /// The answer to [`HostCommand::ScanCardFree`]: free bytes, or `None` when the scan
    /// failed/is unavailable.
    CardScanned { free_bytes: Option<u64> },
    /// The answer to a drained [`DfuAction::Scan`].
    DfuScanned(Result<DfuScanReport, DfuScanError>),
    /// A drained [`DfuAction::Install`] refused or failed to arm without rebooting (issue #755).
    DfuInstallFailed(DfuInstallError),
    /// The install drain's guards passed and the arm + reboot is imminent — swap in the terminal
    /// "Installing update" card the panel holds through the bootloader.
    DfuInstallBegan,
    /// This boot confirmed a freshly-installed firmware update (S4, #619): the running image's
    /// version.
    UpdateConfirmed(Version),
    /// This boot detected a failed firmware update: the typed verdict plus the staged version if
    /// the arm marker survived.
    UpdateFailed { why: DfuFailure, staged: Option<Version> },
    /// The answer to a drained [`HostCommand::PersistSettings`]: the host wrote `revision` to durable
    /// storage (#810). Clears the app's dirty state **iff `revision` is still the latest** — a stale
    /// ack (a newer edit already bumped the revision) leaves the newer content pending.
    SettingsPersisted { revision: u16 },
    /// The answer to a drained [`HostCommand::PersistSettings`] whose write failed (#810): the app
    /// keeps `revision` dirty and re-arms a bounded backoff retry, and surfaces the failure on the
    /// shared advisory warning card. `error` is the bounded reason.
    SettingsPersistFailed { revision: u16, error: SettingsSaveError },
}

/// A planned detour's preview figures (#882), carried on [`HostEvent::DetourPlanned`]: the cost
/// delta the HUD line shows (`detour length − skipped span length`, signed — a detour around a
/// wandering span *can* be shorter), the detour's own length, and — since #1091 — its own climb,
/// which the preview turns into the second signed figure beside the distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetourPreview {
    /// `detour_total − (rejoin_m − progress_m)`, meters.
    pub cost_delta_m: i32,
    /// The planned detour's honest length (summed raw edge meters).
    pub total_distance_m: u32,
    /// Where the plan actually rejoins the route — the chooser's `target_m`, or farther when the
    /// approach was trimmed to its first sustained tail contact. The replaced span the climb
    /// figure subtracts is `[anchor_m, rejoin_m]`, so it describes the same swap
    /// [`cost_delta_m`](Self::cost_delta_m) already prices.
    pub rejoin_m: u32,
    /// The planned detour's own dead-banded ascent (m), or `None` when **no terrain sample
    /// resolved** for it — the producer's explicit
    /// [`RouteStats::has_elevation`](obc_route::RouteStats), never a guess at the values, because a
    /// genuinely flat detour is `Some(0)` and must still show a figure.
    pub ascent_m: Option<u32>,
}

/// What [`App::drain_host_commands`](crate::App::drain_host_commands) reports about a drain pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum DrainStatus {
    /// Every pending command was moved into the mailbox.
    Complete,
    /// The mailbox filled before every class was drained. **Nothing was lost**: the remaining
    /// classes stay latched in the app and come out of the next drain — the saturation policy is
    /// backpressure, never a silent drop. Unreachable when the mailbox is empty and sized
    /// `N >= HOST_COMMAND_CLASSES`.
    MailboxFull,
}

/// A caller-owned, compile-time-bounded FIFO of drained [`HostCommand`]s. The host allocates it
/// (stack or its own static — `App` never grows by it), fills it once per pass via
/// [`App::drain_host_commands`](crate::App::drain_host_commands), and pops in canonical order.
///
/// Coalescing on push (so re-drains without processing cannot duplicate work):
/// - a [`RescanStore`](HostCommand::RescanStore) folds into a queued one by summing `commits`;
/// - the derived cues ([`LoadRideTrack`](HostCommand::LoadRideTrack) /
///   [`RefreshNavPreview`](HostCommand::RefreshNavPreview)) are skipped when the same class is
///   already queued.
///
/// One-shot classes are never coalesced — each drained instance is a distinct request.
#[derive(Debug)]
pub struct HostMailbox<const N: usize = HOST_COMMAND_CLASSES> {
    q: heapless::Deque<HostCommand, N>,
}

impl<const N: usize> HostMailbox<N> {
    /// An empty mailbox.
    pub const fn new() -> Self {
        HostMailbox { q: heapless::Deque::new() }
    }

    /// Pop the next command in canonical order, or `None` when empty.
    pub fn pop(&mut self) -> Option<HostCommand> {
        self.q.pop_front()
    }

    /// How many commands are queued (clippy pairs it with [`is_empty`](Self::is_empty); the
    /// protocol tests assert exact batch sizes through it).
    pub fn len(&self) -> usize {
        self.q.len()
    }

    /// Whether the mailbox is empty.
    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    /// Whether the mailbox is full — the drain's backpressure signal.
    pub fn is_full(&self) -> bool {
        self.q.is_full()
    }

    /// Push with the documented per-class coalescing. Returns `false` — leaving the command with
    /// the caller — only when the mailbox is full (the drain checks room first, so its pushes
    /// never fail).
    pub(crate) fn push_coalesced(&mut self, cmd: HostCommand) -> bool {
        match cmd {
            HostCommand::RescanStore { commits } => {
                for queued in self.q.iter_mut() {
                    if let HostCommand::RescanStore { commits: have } = queued {
                        *have = have.saturating_add(commits);
                        return true;
                    }
                }
            }
            HostCommand::LoadRideTrack { .. } | HostCommand::RefreshNavPreview => {
                let class = cmd.class();
                if self.q.iter().any(|queued| queued.class() == class) {
                    return true; // the level cue is already queued — re-emission coalesces
                }
            }
            _ => {}
        }
        self.q.push_back(cmd).is_ok()
    }
}

impl<const N: usize> Default for HostMailbox<N> {
    fn default() -> Self {
        Self::new()
    }
}

// Layout tripwires (FAR-07, #800): the message payloads stay pocket-sized — bounded ids/enums and
// the fixed-name `NavRequest` / version strings, never a catalog or profile. Measured 48 / 80 B on
// the 32-bit target (48 / 88 B on a 64-bit host); a variant that inflates either enum past these
// ceilings needs an explicit re-baseline, not an accident. #810's `PersistSettings { revision }` and
// the two `SettingsPersisted/Failed` events add only a `u16` (and a byte-sized `SettingsSaveError`),
// smaller than each enum's dominating variant, so both ceilings are unchanged. The mailbox itself is
// caller-owned, so `App` grows by none of this.
const _: () = assert!(core::mem::size_of::<HostCommand>() <= 48, "HostCommand grew — re-check the payload budget");
const _: () = assert!(core::mem::size_of::<HostEvent>() <= 88, "HostEvent grew — re-check the payload budget");

// ==================== The app-side pending protocol state (FAR-09, #802) ====================
//
// What is left here is the counted store-changed cue. The #810 settings-persistence handshake moved
// to [`SettingsMachine`](crate::settings::SettingsMachine) with #1397 S2 — the domain that owns the
// values owns the write that makes them durable — and the counter itself lives on `App` beside the
// other one-field protocol levels.
