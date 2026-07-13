//! The typed app ↔ host coordination protocol (FAR-07, #800).
//!
//! `App` asks its host to do exactly one kind of thing — run a bounded piece of I/O it cannot do
//! itself (store scans, file deletes, the router, the DFU armer, settings persistence) — and the
//! host answers with facts. Historically that conversation grew as one independent `take_*` /
//! `has_*` / `notify_*` latch per feature; this module names the whole vocabulary in two bounded
//! enums so every host speaks the same protocol instead of re-deriving it latch by latch:
//!
//! - [`HostCommand`] — everything the app can ask of a host, drained through
//!   [`App::drain_host_commands`](crate::App::drain_host_commands) into a caller-owned
//!   [`HostMailbox`].
//! - [`HostEvent`] — every host answer/fact, applied through
//!   [`App::apply_event`](crate::App::apply_event).
//!
//! The legacy per-latch methods remain as **compatibility adapters** over this protocol (each one
//! drains/feeds the same single pending state — there is deliberately no second copy anywhere);
//! their removal is owned by #812 once #801 has moved the hosts onto the typed drain.
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

use crate::activity::{DfuAction, NavRequest, TrackAction};
use crate::dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport, Version};
use crate::screen::WarningFlags;

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
    /// Compat adapter: [`take_store_changed`](crate::App::take_store_changed).
    RescanStore { commits: u32 },
    /// Abort the in-flight route plan and discard the partial nav file (#499). Answers nothing —
    /// the planning screen is already gone. One-shot; drained before [`PlanRoute`] so a cancel and
    /// a fresh request posted in the same input batch resolve in order.
    /// Compat adapter: [`take_nav_cancel`](crate::App::take_nav_cancel).
    ///
    /// [`PlanRoute`]: HostCommand::PlanRoute
    CancelRoutePlan,
    /// Delete the route with durable object id `id` (epic #447, P6); the store-changed edge then
    /// re-feeds the catalog. The pending state is the menu's catalog *index*, resolved to the id
    /// at drain against the live catalog — a request whose route vanished drains to nothing.
    /// One-shot, modal-flow-guarded (hold-to-delete → per-pass drain).
    /// Compat adapter: [`take_route_delete`](crate::App::take_route_delete).
    DeleteRoute { id: u16 },
    /// Cascade-delete the trip with durable object id `id` **and every member route** (epic #526,
    /// TR3). Already id-shaped (a trip id is durable); a vanished trip is a host-side no-op.
    /// One-shot, modal-flow-guarded.
    /// Compat adapter: [`take_trip_delete`](crate::App::take_trip_delete).
    DeleteTrip { id: u16 },
    /// Delete the ride with durable object id `id` (epic #447, P7) — the ride-namespace twin of
    /// [`DeleteRoute`](HostCommand::DeleteRoute), index-resolved at drain the same way.
    /// One-shot, modal-flow-guarded.
    /// Compat adapter: [`take_ride_delete`](crate::App::take_ride_delete).
    DeleteRide { id: u16 },
    /// Close the open ride log: finalise it to the host's saved-ride artifact
    /// ([`TrackAction::Save`]) or throw it away ([`TrackAction::Discard`]). Persistence-critical
    /// one-shot; the host reads [`ride_stats`](crate::App::ride_stats) in the same pass so the
    /// wall-clock anchor pairs with the log's last points.
    /// Compat adapter: [`Activity::take_track_action`](crate::Activity::take_track_action).
    FinishTrack(TrackAction),
    /// Run the on-device router from `from` to `to` (epic #116, R4): write the emitted OBCR to the
    /// reserved nav route, rescan, and answer with [`HostEvent::NavPlanned`]. One-shot; the
    /// confirm-screen flow guarantees at most one plan is posted per drain.
    /// Compat adapter: [`take_nav_request`](crate::App::take_nav_request).
    PlanRoute(NavRequest),
    /// Run a DFU phase (epic #615): validate `UPDATE.BIN` ([`DfuAction::Scan`], answered by
    /// [`HostEvent::DfuScanned`]) or arm-and-reboot ([`DfuAction::Install`], which either never
    /// returns or answers [`HostEvent::DfuInstallFailed`]). Single slot, **most-recent-wins by
    /// design**: there is never more than one DFU phase in flight, and a later rider post
    /// supersedes an undrained earlier one (the remote BLE door defers instead — see
    /// [`open_remote_dfu_check`](crate::App::open_remote_dfu_check)).
    /// Compat adapter: [`take_dfu_request`](crate::App::take_dfu_request).
    Dfu(DfuAction),
    /// Forget the paired phone (epic #447, P8): clear the bond store and drop the bonded
    /// connection. One-shot, guarded-hold-posted.
    /// Compat adapter: [`take_ble_forget`](crate::App::take_ble_forget).
    ForgetBond,
    /// Persist the live [`settings`](crate::App::settings). Emitted once when an edited settings
    /// value leaves the settings subtree (the save is debounced to screen exit, not fired per
    /// detent). The acknowledged/retryable revision protocol is #810's; until then this remains
    /// fire-and-forget like the latch it replaces.
    /// Compat adapter: [`take_settings_dirty`](crate::App::take_settings_dirty).
    PersistSettings,
    /// Run the FAT free-cluster scan and answer with [`HostEvent::CardScanned`] (T8 item 6).
    /// One-shot per System-screen entry; idempotent refresh.
    /// Compat adapter: [`take_card_scan_request`](crate::App::take_card_scan_request).
    ScanCardFree,
    /// Stream ride `id`'s recorded track once and answer with
    /// [`set_ride_profile`](crate::App::set_ride_profile) /
    /// [`set_ride_preview`](crate::App::set_ride_preview) (#680). **Derived level, not a stored
    /// one-shot**: re-emitted on every drain while the open Ride detail's viewed ride is
    /// unanswered, and gone the moment the answer (even a failure's `None`) parks under the viewed
    /// key — so a missed pass re-asks and a dead file never grinds.
    /// Compat adapter: [`take_ride_track_request`](crate::App::take_ride_track_request).
    LoadRideTrack { id: u16 },
    /// Decimate the active route's shape polyline and hand it to
    /// [`set_nav_preview`](crate::App::set_nav_preview) (#685 §4). **Derived level** like
    /// [`LoadRideTrack`](HostCommand::LoadRideTrack): re-emitted while a Route overview is up
    /// without its preview.
    /// Compat adapter: [`nav_preview_missing`](crate::App::nav_preview_missing).
    RefreshNavPreview,
}

/// One command class per [`HostCommand`] variant — the drain iterates these; the discriminant
/// doubles as the canonical drain order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostCommandClass {
    RescanStore,
    CancelRoutePlan,
    DeleteRoute,
    DeleteTrip,
    DeleteRide,
    FinishTrack,
    PlanRoute,
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
    /// typed host observes; the per-class latches never encoded a cross-class arrival order to
    /// begin with (every legacy host imposed its own).
    pub(crate) const DRAIN_ORDER: [HostCommandClass; 13] = [
        HostCommandClass::RescanStore,
        HostCommandClass::CancelRoutePlan,
        HostCommandClass::DeleteRoute,
        HostCommandClass::DeleteTrip,
        HostCommandClass::DeleteRide,
        HostCommandClass::FinishTrack,
        HostCommandClass::PlanRoute,
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
            HostCommand::DeleteRoute { .. } => HostCommandClass::DeleteRoute,
            HostCommand::DeleteTrip { .. } => HostCommandClass::DeleteTrip,
            HostCommand::DeleteRide { .. } => HostCommandClass::DeleteRide,
            HostCommand::FinishTrack(_) => HostCommandClass::FinishTrack,
            HostCommand::PlanRoute(_) => HostCommandClass::PlanRoute,
            HostCommand::Dfu(_) => HostCommandClass::Dfu,
            HostCommand::ForgetBond => HostCommandClass::ForgetBond,
            HostCommand::PersistSettings => HostCommandClass::PersistSettings,
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
    /// [`HostCommand::RescanStore`] cue. Compat adapter:
    /// [`notify_store_changed`](crate::App::notify_store_changed).
    StoreChanged,
    /// A route upload committed to the store (epic #447, P4): `id` is the durable object id
    /// (resolved against the **already rescanned** catalog — the rescan-then-resolve ordering
    /// contract), `replaced` says the bytes of a stored route were swapped, `elevation` is the
    /// commit-time mini sparkline for the idle prompt. The advisory prompt keeps its documented
    /// single-slot **most-recent-wins** delivery. Compat adapter:
    /// [`notify_route_uploaded`](crate::App::notify_route_uploaded).
    RouteUploaded { id: u16, replaced: bool, elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]> },
    /// One or more device warnings were discovered (issue #504); flags accumulate onto the single
    /// dismissable card, each surfaced once per boot. Compat adapter:
    /// [`notify_warning`](crate::App::notify_warning).
    Warning(WarningFlags),
    /// The answer to [`HostCommand::PlanRoute`]: the committed nav route's durable id, or the
    /// typed failure. Lands in the planning screen; dropped if the rider already cancelled.
    /// Compat adapter: [`notify_nav_result`](crate::App::notify_nav_result).
    NavPlanned(Result<u16, obc_route::nav::NavError>),
    /// The answer to [`HostCommand::ScanCardFree`]: free bytes, or `None` when the scan
    /// failed/is unavailable. Compat adapter: [`set_card_free`](crate::App::set_card_free).
    CardScanned { free_bytes: Option<u64> },
    /// The answer to a drained [`DfuAction::Scan`]. Compat adapter:
    /// [`notify_dfu_scan_result`](crate::App::notify_dfu_scan_result).
    DfuScanned(Result<DfuScanReport, DfuScanError>),
    /// A drained [`DfuAction::Install`] refused or failed to arm without rebooting (issue #755).
    /// Compat adapter: [`notify_dfu_install_failed`](crate::App::notify_dfu_install_failed).
    DfuInstallFailed(DfuInstallError),
    /// The install drain's guards passed and the arm + reboot is imminent — swap in the terminal
    /// "Installing update" card the panel holds through the bootloader. Compat adapter:
    /// [`show_dfu_installing`](crate::App::show_dfu_installing).
    DfuInstallBegan,
    /// This boot confirmed a freshly-installed firmware update (S4, #619): the running image's
    /// version. Compat adapter: [`notify_update_confirmed`](crate::App::notify_update_confirmed).
    UpdateConfirmed(Version),
    /// This boot detected a failed firmware update: the typed verdict plus the staged version if
    /// the arm marker survived. Compat adapter:
    /// [`notify_update_failed`](crate::App::notify_update_failed).
    UpdateFailed { why: DfuFailure, staged: Option<Version> },
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

    /// How many commands are queued.
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

    /// Iterate the queued commands in drain order without consuming them.
    pub fn iter(&self) -> impl Iterator<Item = &HostCommand> {
        self.q.iter()
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
// ceilings needs an explicit re-baseline, not an accident. The mailbox itself is caller-owned, so
// `App` grows by none of this.
const _: () = assert!(core::mem::size_of::<HostCommand>() <= 48, "HostCommand grew — re-check the payload budget");
const _: () = assert!(core::mem::size_of::<HostEvent>() <= 88, "HostEvent grew — re-check the payload budget");
