//! The **Navigator** domain protocol: route planning, detour planning, preview and commit (#1436).
//!
//! Navigator owns the whole planning lifecycle — `Idle → Planning → PreviewReady → Committing →
//! Active` (or `Failed`) — and the rules a platform executor must never decide: when a plan is
//! cancelled, when a replacement supersedes an in-flight one, and when a late planner answer is too
//! old to matter. The executor is left with five bounded mechanisms: take the sources and the
//! workspace, run **one** planner step, commit a route, commit a detour, give the resources back.
//!
//! [`NavigatorMachine`] is that owner. It holds the rider's request until an executor takes it, the
//! [`OperationToken`] the answer must come back with, the per-family phase, and — since #1397 S2 —
//! the [`RerouteFreeze`] the planner's liveness drives. Nothing else may write any of them.
//!
//! Bulk stays out: the emitted OBCR bytes, the corridor blacklist and the detour preview *polyline*
//! never ride an effect or an outcome. What crosses is an identity, a bounded request, and the
//! preview *figures* the HUD prints.

use obc_route::nav::NavError;

use crate::activity::{DetourRequest, NavRequest};
use crate::device_core::{NavigatorTag, OperationToken, TokenSource};
use crate::host::DetourPreview;
use crate::reroute_freeze::{PlanFamily, RerouteFreeze};
use crate::CatalogObjectId;

/// What the rider (through `UiRuntime`) asks navigation to do. An intent is a *product request*:
/// Navigator decides whether it is admissible, what physical work it implies, and what the rider
/// then sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorIntent {
    /// Plan a route from the rider's fix to a chosen point.
    PlanRoute(NavRequest),
    /// Abandon the in-flight route plan. Navigator invalidates its token, so the planner's eventual
    /// answer is rejected rather than committing a route nobody is waiting for.
    CancelPlan,
    /// Plan a detour that rejoins the active route ahead.
    PlanDetour(DetourRequest),
    /// Abandon the in-flight detour plan **and** any planned-but-uncommitted detour.
    CancelDetour,
    /// Commit the previewed detour: splice it into the active route and make the result active.
    CommitDetour,
}

/// Which search the acquired workspace is for — the only thing the executor needs in order to open
/// the right sources. Bounded by construction: [`NavRequest`]'s name is a fixed inline buffer and
/// [`DetourRequest`] is four numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannerWork {
    /// A full route plan from a fix to a goal.
    Route(NavRequest),
    /// A detour around the span ahead of the rider.
    Detour(DetourRequest),
}

/// One bounded physical navigation operation. Every variant carries the
/// [`OperationToken`] Navigator issued, and the matching [`NavigatorOutcome`] carries it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorEffect {
    /// Open the map and route sources and claim the planner workspace for `work`.
    Acquire { token: OperationToken<NavigatorTag>, work: PlannerWork },
    /// Run **one** bounded planner step. Navigator paces the search: a step is a unit of work, not
    /// a whole search, so a plan never monopolises a pass.
    Step { token: OperationToken<NavigatorTag> },
    /// Write the finished search as a route object and commit it to the store.
    CommitRoute { token: OperationToken<NavigatorTag> },
    /// Splice the planned detour into the active route and commit the derived route.
    CommitDetour { token: OperationToken<NavigatorTag> },
    /// Release the workspace and the sources. Issued on success **and** on cancellation, so the
    /// executor never has to infer that the rider walked away.
    Release { token: OperationToken<NavigatorTag> },
}

impl NavigatorEffect {
    /// The operation this effect belongs to.
    pub fn token(&self) -> OperationToken<NavigatorTag> {
        match self {
            NavigatorEffect::Acquire { token, .. }
            | NavigatorEffect::Step { token }
            | NavigatorEffect::CommitRoute { token }
            | NavigatorEffect::CommitDetour { token }
            | NavigatorEffect::Release { token } => *token,
        }
    }
}

/// How far one [`Step`](NavigatorEffect::Step) got. Navigator — not the executor — decides what to
/// do next: keep stepping, or commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannerProgress {
    /// The frontier is still open; another step is worthwhile.
    Searching,
    /// The search reached its goal. The result is in the planner workspace, ready to commit.
    Reached,
}

/// Why a navigation operation failed, in Navigator's own vocabulary.
///
/// [`Plan`](NavigatorError::Plan) reuses the shared planner's [`NavError`] rather than restating
/// it: the same `obc-route` search runs on every platform (#1433 §7.2), so its two honest verdicts
/// are the same everywhere. An **unsupported** detour is not in this enum at all — that is a
/// missing [`NavigatorCapabilities::plan_detour`](crate::device_core::NavigatorCapabilities), and a
/// device without the planner must never report it as `NoPath`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorError {
    /// The search itself failed: no path, or the fixed scratch exhausted.
    Plan(NavError),
    /// The planner workspace or a source could not be claimed.
    Workspace,
    /// The store refused the commit; the previously active route is untouched.
    Store,
}

/// The result of one [`NavigatorEffect`] — success, a typed failure, or cancellation. Never
/// `Busy`: refusing to *start* work is an admission result the slot reports (see
/// [`device_core::slots`](crate::device_core::slots)), not an operation that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorOutcome {
    /// The workspace and sources are held; stepping may begin.
    Acquired { token: OperationToken<NavigatorTag> },
    /// One planner step ran.
    Stepped { token: OperationToken<NavigatorTag>, progress: PlannerProgress },
    /// The planned route was committed under `route`.
    PlanFinished { token: OperationToken<NavigatorTag>, route: CatalogObjectId },
    /// The detour search finished and its preview figures are ready. The preview *polyline* reaches
    /// the screens as a keyed derived input, never here.
    DetourFinished { token: OperationToken<NavigatorTag>, preview: DetourPreview },
    /// The spliced detour was committed under `route` — the re-adoption key.
    DetourCommitted { token: OperationToken<NavigatorTag>, route: CatalogObjectId },
    /// The workspace and sources are back with the executor.
    Released { token: OperationToken<NavigatorTag> },
    /// The operation failed.
    Failed { token: OperationToken<NavigatorTag>, error: NavigatorError },
    /// The executor abandoned the operation without completing it.
    Cancelled { token: OperationToken<NavigatorTag> },
}

impl NavigatorOutcome {
    /// The operation this outcome answers. Navigator accepts it only while the token is current.
    pub fn token(&self) -> OperationToken<NavigatorTag> {
        match self {
            NavigatorOutcome::Acquired { token }
            | NavigatorOutcome::Stepped { token, .. }
            | NavigatorOutcome::PlanFinished { token, .. }
            | NavigatorOutcome::DetourFinished { token, .. }
            | NavigatorOutcome::DetourCommitted { token, .. }
            | NavigatorOutcome::Released { token }
            | NavigatorOutcome::Failed { token, .. }
            | NavigatorOutcome::Cancelled { token } => *token,
        }
    }
}

// Layout tripwires: a navigation message is a request, an identity, or a handful of figures. The
// dominating effect variant is `Acquire`'s `PlannerWork` (a `NavRequest` with its fixed name
// buffer) and the dominating outcome is the four-figure `DetourPreview`.
const _: () = assert!(core::mem::size_of::<NavigatorIntent>() <= 48, "an intent is a bounded request");
const _: () = assert!(core::mem::size_of::<NavigatorEffect>() <= 56, "the planner request plus a token");
const _: () = assert!(core::mem::size_of::<NavigatorOutcome>() <= 24, "preview figures, never a polyline");
const _: () = assert!(core::mem::size_of::<NavigatorError>() <= 2, "a verdict, not a report");
const _: () = assert!(core::mem::size_of::<PlannerWork>() <= 48, "the largest planner request");
const _: () = assert!(core::mem::size_of::<PlannerProgress>() <= 1, "a two-state answer");

// ==================== the Navigator state machine (#1397 S2) ====================

/// Where one planning family is in the lifecycle Navigator owns.
///
/// Two families run this independently — a route search and a detour search take the same nav arm
/// but have their own commands, their own answers and their own failure tiers, and conflating them
/// is the #1146 regression (see [`PlanFamily`]). The route family never reaches
/// [`PreviewReady`](PlanPhase::PreviewReady) or [`Committing`](PlanPhase::Committing): a planned
/// route is adopted straight from its answer, while a detour is previewed and then spliced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlanPhase {
    /// Nothing asked for, nothing running.
    #[default]
    Idle,
    /// The rider asked; no executor has taken the work yet. A cancel here **annihilates** the
    /// request (#499) — the net intent is "no plan", so nothing is ever started.
    Requested,
    /// An executor holds the operation under the machine's current token.
    Planning,
    /// A detour search finished and its preview is what the rider is looking at — the phase a
    /// [`CommitDetour`](NavigatorIntent::CommitDetour) is pressed from, and the one a failed splice
    /// returns to.
    PreviewReady,
    /// The previewed detour is being spliced into the active route.
    Committing,
    /// The last operation was adopted: the planned route, or the spliced detour.
    Active,
    /// The last operation failed. Distinct from [`Idle`](PlanPhase::Idle) so "no path" is not read
    /// as "nothing was ever asked".
    Failed,
}

/// The domain that owns route planning, detour planning, preview and commit.
///
/// Everything one rider request passes through lives here and nowhere else: the undelivered
/// request, the cancel that annihilates it, the phase, the operation token, and the freeze the
/// planner's liveness engages. Both compositions reach it through the same three-method seam —
/// [`admit_intent`](Self::admit_intent), [`next_effect`](Self::next_effect),
/// `App::apply_navigator_outcome` — so the legacy drain and the pass cannot disagree about what
/// the rider asked for.
#[derive(Debug, Default)]
pub struct NavigatorMachine {
    /// The one operation token, and it is genuinely **one**:
    /// [`TokenSource::issue`](crate::device_core::TokenSource::issue) bumps a single generation, so
    /// only the newest operation is ever current *across both families*. A second concurrent
    /// operation would therefore make the first one's answer stale, `accepts` would refuse it, and
    /// its family's freeze flag would never be released — the "map that never redraws again"
    /// [`RerouteFreeze`] names. That is why [`next_plan_effect`](Self::next_plan_effect) and
    /// [`next_commit_effect`](Self::next_commit_effect) hand out **at most one operation at a
    /// time**: the constraint is enforced where the token is minted, not assumed.
    ///
    /// `RerouteFreeze`'s two flags are not the same question. They track *edges* — which family's
    /// terminal edge may release what — and they stay per-family (#1146) whatever the token layer
    /// allows.
    ops: TokenSource<NavigatorTag>,
    /// The family whose operation an executor is currently holding, if any. `Some` is what makes
    /// [`next_plan_effect`](Self::next_plan_effect) refuse a second one; it is cleared by the
    /// terminal answer, by a superseding intent, and by a cancellation reaching the executor.
    ///
    /// A [`Release`](NavigatorEffect::Release) never sets it: handing the workspace back owes no
    /// product answer, so it is not an operation anyone is waiting on.
    live: Option<PlanFamily>,
    /// The **Recalculating freeze** (#1146): a live planner run over a map base stops map redraws,
    /// pauses the matcher and raises the banner. Navigator is its only writer — the four scattered
    /// edge calls the drain used to make are the transitions below. S5 derives it from `CoreMode`
    /// and deletes the module.
    freeze: RerouteFreeze,
    /// The route family's phase.
    route: PlanPhase,
    /// The detour family's phase.
    detour: PlanPhase,
    /// The rider's route-plan request, until an executor takes it.
    route_request: Option<NavRequest>,
    /// The rider's detour-plan request, until an executor takes it.
    detour_request: Option<DetourRequest>,
    /// A route-plan cancellation the executor has not been told about yet.
    route_cancel: bool,
    /// A detour cancellation the executor has not been told about yet.
    detour_cancel: bool,
    /// The previewed detour's commit, until an executor takes it.
    detour_commit: bool,
}

impl NavigatorMachine {
    /// The boot state: nothing planned, nothing running, nothing frozen.
    pub(crate) const fn new() -> Self {
        NavigatorMachine {
            ops: TokenSource::new(),
            live: None,
            freeze: RerouteFreeze::new(),
            route: PlanPhase::Idle,
            detour: PlanPhase::Idle,
            route_request: None,
            detour_request: None,
            route_cancel: false,
            detour_cancel: false,
            detour_commit: false,
        }
    }

    // ---- the operation seam ----

    /// Admit one rider request. Navigator decides what it means; nothing here can fail, because
    /// every intent either supersedes what came before or annihilates it.
    ///
    /// Two rules the drain used to hide live here:
    ///
    /// - **Post-time annihilation** (#499): a cancel clears an undelivered request of its own
    ///   family, so a plan confirmed and cancelled inside one input batch nets "no plan" and no
    ///   executor is ever asked for a route nobody is waiting for.
    /// - **Late-answer refusal**: the operation's token stops being current the instant the rider
    ///   walks away, so the search's eventual answer commits nothing.
    ///
    /// The **freeze is not touched here**. A cancellation the executor has not been handed yet has
    /// not stopped anything: the search still owns the nav arm, and resuming the map plane on the
    /// rider's keypress is the arena race #1146 exists to prevent. It releases at
    /// [`note_cancel_delivered`](Self::note_cancel_delivered).
    pub(crate) fn admit_intent(&mut self, intent: NavigatorIntent) {
        match intent {
            NavigatorIntent::PlanRoute(request) => {
                self.supersede(PlanFamily::Route);
                self.route_request = Some(request);
                self.route = PlanPhase::Requested;
            }
            NavigatorIntent::CancelPlan => {
                self.route_request = None; // #499: the undelivered request nets out
                self.route_cancel = true;
                self.supersede(PlanFamily::Route);
                self.route = PlanPhase::Idle;
            }
            NavigatorIntent::PlanDetour(request) => {
                self.supersede(PlanFamily::Detour);
                self.detour_request = Some(request);
                self.detour = PlanPhase::Requested;
            }
            NavigatorIntent::CancelDetour => {
                self.detour_request = None;
                self.detour_commit = false;
                self.detour_cancel = true;
                self.supersede(PlanFamily::Detour);
                self.detour = PlanPhase::Idle;
            }
            // A commit is the rider pressing on figures they can see, so the preview screen is its
            // only producer — and a cancellation annihilates it exactly as it does a plan request,
            // which is what stops a splice of a detour that no longer exists.
            NavigatorIntent::CommitDetour => self.detour_commit = true,
        }
    }

    /// A cancellation or a replacement: `family`'s in-flight operation stops being the current one,
    /// so its answer will be refused when it lands.
    fn supersede(&mut self, family: PlanFamily) {
        if self.live == Some(family) {
            self.ops.invalidate();
            self.live = None;
        }
    }

    /// Which family the in-flight operation belongs to, if one is running.
    pub(crate) fn live_family(&self) -> Option<PlanFamily> {
        self.live
    }

    /// The next bounded navigation operation, or `None` when nothing is owed.
    ///
    /// Offered in the drain's order — cancellations before new work — so the pass and the legacy
    /// protocol ask the executor for the same thing in the same sequence.
    pub(crate) fn next_effect(&mut self) -> Option<NavigatorEffect> {
        self.next_release(PlanFamily::Route)
            .or_else(|| self.next_release(PlanFamily::Detour))
            .or_else(|| self.next_plan_effect(PlanFamily::Route))
            .or_else(|| self.next_plan_effect(PlanFamily::Detour))
            .or_else(|| self.next_commit_effect())
    }

    /// The workspace release a cancellation implies.
    ///
    /// Consumes the **same one-shot** [`take_cancel`](Self::take_cancel) does — the door
    /// `App::deliver_plan_cancel` uses for the legacy `CancelRoutePlan` / `CancelDetour` arms — so a
    /// cancellation reaches an executor exactly once however the two protocols are composed. Stage 8
    /// runs before `drain_host_commands`, so a composition that runs both consumes it *here*.
    ///
    /// **The known gap that leaves, stated rather than discovered:** under the pass *plus*
    /// [`LegacyAdapter`](crate::device_core::LegacyAdapter),
    /// [`navigator_row`](crate::device_core::compat::navigator_row) maps this `Release` to
    /// [`LegacyOwned::PlannerRelease`](crate::device_core::compat::LegacyOwned::PlannerRelease) —
    /// untranslatable by design, because a `Release` is issued on success too — so that composition
    /// never tells the executor to drop the planner. No shipped host runs both (nothing calls
    /// `App::run_pass` until #1397 S6, which is also what gives Navigator an executor that answers a
    /// `Release`), and the legacy hosts take the cancellation through the drain arm exactly as
    /// before.
    ///
    /// A release is offered only when this family's own operation was the one in flight. A cancel
    /// with nothing running is a host-side no-op — and minting a token for it while *another*
    /// family's search is live would supersede that search's answer, which is the failure
    /// [`ops`](Self::ops) describes.
    fn next_release(&mut self, family: PlanFamily) -> Option<NavigatorEffect> {
        if !self.take_cancel(family) {
            return None;
        }
        self.note_cancel_delivered(family);
        // `admit_intent` already invalidated this family's operation, so `live` is clear exactly
        // when the cancellation had something of its own to stop.
        self.live.is_none().then(|| NavigatorEffect::Release { token: self.ops.issue() })
    }

    /// Hand `family`'s undelivered request to an executor: the operation the search runs under, and
    /// the moment the freeze engages. **The engaging edge is here, not at admission** — a request
    /// the rider cancelled before anyone took it froze nothing, so nothing needs releasing.
    ///
    /// Refused while an executor already holds an operation, whichever family's. The request stays
    /// queued and goes out on a later pass — backpressure, never a loss, and the same shape
    /// [`CatalogState`](crate::catalog_state::CatalogState) uses. Handing out a second one would
    /// mint a token that supersedes the first, so the running search's genuine answer would be
    /// refused and its freeze flag would be stuck forever (see [`ops`](Self::ops)).
    pub(crate) fn next_plan_effect(&mut self, family: PlanFamily) -> Option<NavigatorEffect> {
        if self.live.is_some() {
            return None;
        }
        let work = match family {
            PlanFamily::Route => PlannerWork::Route(self.route_request.take()?),
            PlanFamily::Detour => PlannerWork::Detour(self.detour_request.take()?),
        };
        match family {
            PlanFamily::Route => self.route = PlanPhase::Planning,
            PlanFamily::Detour => self.detour = PlanPhase::Planning,
        }
        self.freeze.plan_started(family);
        self.live = Some(family);
        Some(NavigatorEffect::Acquire { token: self.ops.issue(), work })
    }

    /// Hand the previewed detour's splice to an executor. No freeze edge: a commit is a write, not
    /// a search, and it does not take the nav arm. Refused while another operation is in flight,
    /// for the same reason [`next_plan_effect`](Self::next_plan_effect) is.
    pub(crate) fn next_commit_effect(&mut self) -> Option<NavigatorEffect> {
        if self.live.is_some() {
            return None;
        }
        core::mem::take(&mut self.detour_commit).then(|| {
            self.detour = PlanPhase::Committing;
            self.live = Some(PlanFamily::Detour);
            NavigatorEffect::CommitDetour { token: self.ops.issue() }
        })
    }

    /// Whether `outcome` still answers the operation Navigator is waiting for. A cancelled or
    /// superseded plan refuses its own late answer here — the executor never has to know.
    pub(crate) fn accepts(&self, outcome: &NavigatorOutcome) -> bool {
        self.ops.is_current(outcome.token())
    }

    /// Record a terminal planner answer for `family`: the run is over, so the token stops being
    /// current and the freeze releases. Returns whether the freeze changed.
    ///
    /// Reached from both answer paths — a typed [`NavigatorOutcome`] at the pass's first stage, and
    /// a legacy `HostEvent` at [`App::apply_event`](crate::App::apply_event) — so there is one
    /// definition of "the run ended" whatever spoke.
    pub(crate) fn note_answer(&mut self, family: PlanFamily, phase: PlanPhase) -> bool {
        self.ops.invalidate();
        self.live = None;
        match family {
            PlanFamily::Route => self.route = phase,
            PlanFamily::Detour => self.detour = phase,
        }
        self.freeze.plan_ended(family)
    }

    /// Whether a detour plan exists at all — requested, running, previewed, committing or adopted.
    /// The falling edge of this is what drops the preview polyline drawn over the active route.
    pub(crate) fn detour_planned(&self) -> bool {
        self.detour != PlanPhase::Idle
    }

    /// Whether the in-flight detour operation is the splice rather than the search — the two have
    /// the same family and different answers.
    pub(crate) fn detour_committing(&self) -> bool {
        self.detour == PlanPhase::Committing
    }

    /// A detour commit answered. Success adopts the spliced route; a failure returns the rider to
    /// the preview they came from, which is what makes a failed commit retryable.
    pub(crate) fn note_commit(&mut self, committed: bool) {
        self.ops.invalidate();
        self.live = None;
        self.detour = if committed { PlanPhase::Active } else { PlanPhase::PreviewReady };
    }

    // ---- the legacy protocol's per-class doors (deleted at #1397 S6) ----

    /// Whether `family` has an undelivered plan request — the `PlanRoute` / `PlanDetour` peek.
    pub(crate) fn request_pending(&self, family: PlanFamily) -> bool {
        match family {
            PlanFamily::Route => self.route_request.is_some(),
            PlanFamily::Detour => self.detour_request.is_some(),
        }
    }

    /// Whether `family` has an undelivered cancellation — the `CancelRoutePlan` / `CancelDetour`
    /// peek.
    pub(crate) fn cancel_pending(&self, family: PlanFamily) -> bool {
        match family {
            PlanFamily::Route => self.route_cancel,
            PlanFamily::Detour => self.detour_cancel,
        }
    }

    /// Whether the previewed detour's commit is undelivered — the `CommitDetour` peek.
    pub(crate) fn commit_pending(&self) -> bool {
        self.detour_commit
    }

    /// Take an undelivered cancellation for `family`.
    ///
    /// The legacy protocol expresses a cancellation as its own command, while the new one expresses
    /// it as a [`Release`](NavigatorEffect::Release) that is *also* issued on success — which is
    /// why [`LegacyOwned::PlannerRelease`](crate::device_core::compat::LegacyOwned::PlannerRelease)
    /// refuses to translate one into the other, and why the drain asks for the cancel by name.
    pub(crate) fn take_cancel(&mut self, family: PlanFamily) -> bool {
        match family {
            PlanFamily::Route => core::mem::take(&mut self.route_cancel),
            PlanFamily::Detour => core::mem::take(&mut self.detour_cancel),
        }
    }

    /// The executor has been told to drop `family`'s search: **now** the run is over, so the freeze
    /// releases and the map plane may resume. Returns whether that changed the freeze, so the
    /// caller can repaint the frame that held still for it.
    ///
    /// **Per-family** (#1146): a detour's cancellation must never resume the map while a route
    /// search still holds the nav arm — the very next frame would claim the render arm out from
    /// under it.
    pub(crate) fn note_cancel_delivered(&mut self, family: PlanFamily) -> bool {
        self.freeze.plan_ended(family)
    }

    // ---- the freeze, read-only to everyone else ----

    /// Whether a planner run is live at all — the arena's "is the nav arm claimed?" fact.
    pub(crate) fn plan_live(&self) -> bool {
        self.freeze.plan_live()
    }

    /// Whether the freeze is engaged: a live plan **and** a base screen that would draw the map.
    pub(crate) fn freeze_active(&self, base_draws_map: bool) -> bool {
        self.freeze.active(base_draws_map)
    }

    /// The banner's repaint edge — see [`RerouteFreeze::take_engaged_edge`].
    pub(crate) fn take_freeze_edge(&mut self, base_draws_map: bool) -> bool {
        self.freeze.take_engaged_edge(base_draws_map)
    }

    /// Engage or release a `Route` run without a real planner — the simulator's `--freeze` flag and
    /// the snapshot harness. No production path reaches it.
    pub(crate) fn debug_set_plan_live(&mut self, live: bool) -> bool {
        if live {
            self.freeze.plan_started(PlanFamily::Route);
            self.route = PlanPhase::Planning;
            false
        } else {
            self.note_answer(PlanFamily::Route, PlanPhase::Idle)
        }
    }

    // ---- catalog identity ----

    /// Follow the undelivered detour request through a route-catalog rescan by durable identity; a
    /// vanished route drops it, exactly as it drops the caches keyed on that route.
    pub(crate) fn remap_detour_route(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.detour_request =
            self.detour_request.and_then(|req| remap(req.route).map(|route| DetourRequest { route, ..req }));
    }

    /// A fresh tracking session starts with no detour in flight.
    ///
    /// Dropping the pair cannot strand the freeze's `Detour` level: an **undelivered** request never
    /// engaged it (the effect is the engaging edge), and a dropped **cancel** only forfeits one of
    /// two release edges — the executor is still running the plan that cancel would have aborted,
    /// and it answers every plan it was given, so the answer's own release lands anyway.
    pub(crate) fn reset_detour(&mut self) {
        self.detour_request = None;
        self.detour_commit = false;
        self.detour_cancel = false;
        // The phase describes a plan that is gone, so it goes with it — otherwise `detour_planned`
        // and `detour_committing` would keep reporting a preview or a splice that no longer exists.
        // `live` and the token are deliberately left: an executor may still be holding this
        // family's operation, and its answer is what hands the workspace back.
        self.detour = PlanPhase::Idle;
    }

    /// The detour family's phase — the preview/commit gate, and what the tests read.
    #[cfg(test)]
    pub(crate) fn detour_phase(&self) -> PlanPhase {
        self.detour
    }

    /// The undelivered detour request itself — the durable-identity tests pin the remap through it
    /// without consuming the request.
    #[cfg(test)]
    pub(crate) fn pending_detour_request(&self) -> Option<DetourRequest> {
        self.detour_request
    }

    /// Assert the boot state, field by field. The destructure is exhaustive, so a field added here
    /// must state its boot value too.
    #[cfg(test)]
    pub(crate) fn assert_boot_state(&self) {
        let NavigatorMachine {
            ops,
            live,
            freeze,
            route,
            detour,
            route_request,
            detour_request,
            route_cancel,
            detour_cancel,
            detour_commit,
        } = self;
        assert_eq!(format!("{ops:?}"), "TokenSource(0)", "no navigation operation has been issued");
        assert!(live.is_none(), "no operation is in flight");
        assert!(!freeze.plan_live() && !freeze.active(true), "no planner running, no freeze banner");
        assert!(*route == PlanPhase::Idle && *detour == PlanPhase::Idle, "neither family has been asked");
        assert!(route_request.is_none() && detour_request.is_none(), "no request waiting");
        assert!(!*route_cancel && !*detour_cancel && !*detour_commit, "no one-shot latched");
    }
}

// Layout tripwire: two bounded requests, two phases, a token and a handful of one-shots — never a
// route, a polyline or a screen.
const _: () = assert!(core::mem::size_of::<NavigatorMachine>() <= 96, "two planner requests and their phases");

#[cfg(test)]
mod machine_tests {
    use super::*;
    use obc_route::nav::NavError;

    fn route_request(name: &str) -> NavRequest {
        NavRequest::new((0, 0), (1_000, 1_000), name)
    }

    fn detour_request() -> DetourRequest {
        DetourRequest { route: 0, from: (0, 0), progress_m: 1_000, target_m: 1_600 }
    }

    /// The name of what an effect asks for, so a test can say what it expects without matching on
    /// a token it never chose.
    fn acquired(effect: Option<NavigatorEffect>) -> Option<PlannerWork> {
        match effect {
            Some(NavigatorEffect::Acquire { work, .. }) => Some(work),
            _ => None,
        }
    }

    /// **#499, both families.** A cancel posted before its request reaches an executor nets "no
    /// plan": the request is annihilated at post time, so nothing is ever started, and the cancel
    /// still latches (a plan an executor already took is still aborted).
    #[test]
    fn a_cancel_before_delivery_nets_no_plan() {
        for (plan, cancel, family) in [
            (NavigatorIntent::PlanRoute(route_request("col")), NavigatorIntent::CancelPlan, PlanFamily::Route),
            (NavigatorIntent::PlanDetour(detour_request()), NavigatorIntent::CancelDetour, PlanFamily::Detour),
        ] {
            let mut nav = NavigatorMachine::new();
            nav.admit_intent(plan);
            nav.admit_intent(cancel);
            assert!(!nav.request_pending(family), "the undelivered request nets out");
            assert!(nav.next_plan_effect(family).is_none(), "so no executor is ever asked to plan it");
            assert!(nav.cancel_pending(family), "and the cancel still reaches one");
            assert!(!nav.plan_live(), "nothing froze the map for a plan that never started");
        }
    }

    /// **#1146.** A stray edge of one family must never release the freeze a *route* search is
    /// holding the nav arm behind, and the running search's own answer must still be accepted when
    /// it arrives. The regression is a map that never redraws again with an unfrozen matcher
    /// drifting under it.
    ///
    /// Driven through the real seam — `next_effect` and `accepts`, not `note_answer` — because the
    /// token layer is where this can go wrong: a second operation would mint a generation that
    /// supersedes the first, its genuine answer would be refused, and nothing would ever release
    /// its flag. The machine refuses the second operation instead, so both halves hold.
    #[test]
    fn a_detours_terminal_edge_never_releases_a_route_freeze() {
        let mut nav = NavigatorMachine::new();
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("col")));
        let route = nav.next_plan_effect(PlanFamily::Route).expect("the route search starts");
        assert!(nav.freeze_active(true), "a search over a map base is the freeze");

        // A detour cancellation while the route search runs. It is not this run's edge, and it
        // mints no token — a `Release` here would supersede the search that is still going.
        nav.admit_intent(NavigatorIntent::CancelDetour);
        assert!(nav.next_effect().is_none(), "a cancellation with nothing of its own to stop asks for no work");
        assert!(nav.freeze_active(true), "the route search still holds the nav arm");

        // A detour *request* while it runs: refused, and the request waits rather than being lost.
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        assert!(nav.next_effect().is_none(), "one navigation operation at a time");
        assert!(nav.request_pending(PlanFamily::Detour), "…and the rider's detour is still queued");

        // The route search's genuine answer is still this operation's — which is the half a
        // superseding second token would have destroyed.
        let answer = NavigatorOutcome::PlanFinished { token: route.token(), route: 9 };
        assert!(nav.accepts(&answer), "the running search's answer is accepted");
        assert!(nav.note_answer(PlanFamily::Route, PlanPhase::Active), "and it is what releases the freeze");
        assert!(!nav.freeze_active(true));

        // Only now does the queued detour go out, and it freezes on its own account.
        assert!(matches!(acquired(nav.next_effect()), Some(PlannerWork::Detour(_))), "nothing was lost");
        assert!(nav.freeze_active(true));
        assert!(nav.note_answer(PlanFamily::Detour, PlanPhase::PreviewReady), "released by its own edge");
        assert!(!nav.freeze_active(true));
    }

    /// The freeze's own two-flag rule, kept where the machine cannot reach it: two runs live at
    /// once is not a state today's UI can produce — and `next_plan_effect` now refuses to create
    /// one — but the arm is a single block, so if it ever becomes reachable the freeze must hold
    /// until the *last* run is done, not the first. `RerouteFreeze`'s own tests pin that;
    /// Navigator's contribution is that it never hands out the second operation that would strand
    /// the first one's answer.
    #[test]
    fn a_second_operation_is_refused_rather_than_superseding_the_first() {
        let mut nav = NavigatorMachine::new();
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        let detour = nav.next_plan_effect(PlanFamily::Detour).expect("the detour search starts");

        // A route plan and a commit, both while it runs.
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("col")));
        nav.admit_intent(NavigatorIntent::CommitDetour);
        assert!(nav.next_plan_effect(PlanFamily::Route).is_none(), "no second search");
        assert!(nav.next_commit_effect().is_none(), "and no splice beside one");
        assert!(nav.accepts(&NavigatorOutcome::Acquired { token: detour.token() }), "the first is still current");

        nav.note_answer(PlanFamily::Detour, PlanPhase::PreviewReady);
        assert!(nav.next_plan_effect(PlanFamily::Route).is_some(), "the queued plan goes out once it is free");
    }

    /// A plan answer that arrives after the rider cancelled changes nothing: the token stopped being
    /// current the instant they walked away, so the search's eventual result commits no route.
    #[test]
    fn an_answer_after_a_cancellation_is_refused() {
        let mut nav = NavigatorMachine::new();
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("col")));
        let effect = nav.next_plan_effect(PlanFamily::Route).expect("the search starts");

        nav.admit_intent(NavigatorIntent::CancelPlan);
        let answer = NavigatorOutcome::PlanFinished { token: effect.token(), route: 7 };
        assert!(!nav.accepts(&answer), "the cancelled operation does not accept its own late answer");
    }

    /// The same rule for a *replacement*: the newer request supersedes the older operation, and the
    /// older one's answer belongs to nothing.
    #[test]
    fn an_answer_after_a_replacement_is_refused() {
        let mut nav = NavigatorMachine::new();
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("first")));
        let first = nav.next_plan_effect(PlanFamily::Route).expect("the first search starts");

        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("second")));
        let second = nav.next_plan_effect(PlanFamily::Route).expect("the replacement starts");
        assert_ne!(first.token(), second.token(), "a new operation, a new token");
        assert!(!nav.accepts(&NavigatorOutcome::PlanFinished { token: first.token(), route: 7 }));
        assert!(nav.accepts(&NavigatorOutcome::PlanFinished { token: second.token(), route: 8 }));
        assert_eq!(
            acquired(Some(second))
                .map(|work| matches!(work, PlannerWork::Route(request) if request.name() == "second")),
            Some(true),
            "and it is the newer request that went out"
        );
    }

    /// A detour with no path is a **planning failure**, not the absence of the capability: the
    /// family lands in `Failed`, distinguishable from the `Idle` a device that never planned is in.
    /// A device without `NavigatorCapabilities::plan_detour` never reaches this path at all — the
    /// UI's Detour station is not offered, so no intent is ever admitted.
    #[test]
    fn a_detour_without_a_path_is_a_failure_and_not_an_absent_capability() {
        let mut nav = NavigatorMachine::new();
        assert_eq!(nav.detour_phase(), PlanPhase::Idle, "a device that never planned is idle");

        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        let effect = nav.next_plan_effect(PlanFamily::Detour).expect("the search starts");
        let answer = NavigatorOutcome::Failed { token: effect.token(), error: NavigatorError::Plan(NavError::NoPath) };
        assert!(nav.accepts(&answer));
        nav.note_answer(PlanFamily::Detour, PlanPhase::Failed);
        assert_eq!(nav.detour_phase(), PlanPhase::Failed, "…and one that tried and could not is not");
    }

    /// The lifecycle end to end, in the order the rider walks it: the detour is planned, previewed,
    /// committed, and adopted — and a commit that fails returns to the preview it was pressed from,
    /// which is what makes a failed commit retryable.
    #[test]
    fn the_detour_walks_plan_preview_commit_and_a_failure_returns_to_the_preview() {
        let mut nav = NavigatorMachine::new();
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        assert_eq!(nav.detour_phase(), PlanPhase::Requested);
        assert!(acquired(nav.next_plan_effect(PlanFamily::Detour)).is_some());
        assert_eq!(nav.detour_phase(), PlanPhase::Planning);

        nav.note_answer(PlanFamily::Detour, PlanPhase::PreviewReady);
        nav.admit_intent(NavigatorIntent::CommitDetour);
        assert!(nav.commit_pending());
        assert!(matches!(nav.next_commit_effect(), Some(NavigatorEffect::CommitDetour { .. })));
        assert_eq!(nav.detour_phase(), PlanPhase::Committing);
        assert!(!nav.plan_live(), "a splice is a write, not a search — it takes no nav arm");

        nav.note_commit(false);
        assert_eq!(nav.detour_phase(), PlanPhase::PreviewReady, "a failed commit can be retried");
        nav.admit_intent(NavigatorIntent::CommitDetour);
        nav.next_commit_effect().expect("…and the retry goes out");
        nav.note_commit(true);
        assert_eq!(nav.detour_phase(), PlanPhase::Active);
    }

    /// A fresh tracking session drops the detour, and nothing is left describing it: the phase goes
    /// with the one-shots, so `detour_planned` and `detour_committing` cannot keep reporting a
    /// preview or a splice the session reset just threw away.
    #[test]
    fn a_session_reset_leaves_nothing_describing_the_dropped_detour() {
        let mut nav = NavigatorMachine::new();
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        nav.next_plan_effect(PlanFamily::Detour).expect("the search starts");
        nav.note_answer(PlanFamily::Detour, PlanPhase::PreviewReady);
        nav.admit_intent(NavigatorIntent::CommitDetour);
        assert!(nav.detour_planned() && nav.commit_pending());

        nav.reset_detour();
        assert!(!nav.detour_planned(), "no plan");
        assert!(!nav.detour_committing(), "no splice");
        assert!(!nav.commit_pending() && !nav.request_pending(PlanFamily::Detour));
        assert!(!nav.cancel_pending(PlanFamily::Detour));
        assert_eq!(nav.detour_phase(), PlanPhase::Idle);
    }

    /// One stream, one order: the pass takes cancellations before new work, so both compositions ask
    /// an executor for the same thing in the same sequence.
    #[test]
    fn the_pass_offers_a_cancellation_before_new_work() {
        let mut nav = NavigatorMachine::new();
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        nav.next_plan_effect(PlanFamily::Detour).expect("a search is running");
        nav.admit_intent(NavigatorIntent::CancelDetour);
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("col")));

        assert!(matches!(nav.next_effect(), Some(NavigatorEffect::Release { .. })), "the cancellation first");
        assert!(!nav.freeze_active(true), "and delivering it is what releases the detour's freeze");
        assert!(matches!(acquired(nav.next_effect()), Some(PlannerWork::Route(_))), "then the new search");
        assert!(nav.next_effect().is_none(), "and nothing else is owed");
    }
}
