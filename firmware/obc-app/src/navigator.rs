//! The Navigator domain: route following, route planning, detour planning, preview and commit.
//!
//! Navigator owns the whole planning lifecycle — `Idle → Planning → PreviewReady → Committing →
//! Active` (or `Failed`) — and the rules a platform executor must never decide: when a plan is
//! cancelled, when a replacement supersedes an in-flight one, and when a late planner answer is too
//! old to matter. The executor is left with five bounded mechanisms: take the sources and the
//! workspace, run one planner step, commit a route, commit a detour, give the resources back.
//!
//! [`NavigatorMachine`] is that owner, and the only writer of [`CoreMode`]'s two search levels, the
//! fact "the executor holds the nav arm".
//!
//! Bulk stays out: the emitted OBCR bytes, the corridor blacklist and the detour preview polyline
//! never ride an effect or an outcome. What crosses is an identity, a bounded request, and the
//! preview figures the HUD prints.

mod arrival;
mod following;
mod review;
mod visit;
use review::ReviewState;
pub use review::{
    CheckpointChange, ReviewContext, ReviewOrigin, ReviewPurpose, ReviewStatus, ReviewedRoute, RouteCheckpointSource,
    REVIEW_ALONG_TOLERANCE_M, REVIEW_FACTS_POLICY, REVIEW_LATERAL_TOLERANCE_M,
};
pub use visit::VisitUnavailable;

pub(crate) use arrival::Arrival;
pub use following::RouteState;

use obc_route::nav::NavError;
use obc_route::{ClimbProfile, Climbs, Profile, RouteMatch, Waypoints};

use crate::activity::{DetourRequest, NavRequest};
use crate::device_core::core_mode::CoreMode;
use crate::device_core::{NavigatorTag, OperationToken, TokenSource};
use crate::host::DetourPreview;
use crate::placement::define_placement_constructors;
use crate::CatalogObjectId;

/// The route and detour families share one physical workspace and keep separate product states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFamily {
    Route,
    Detour,
}

/// What the rider asks navigation to do. An intent is a product request: Navigator decides whether
/// it is admissible and what physical work it implies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorIntent {
    AcceptAssistant {
        origin: ReviewOrigin,
        profile: crate::settings::BikeType,
    },
    CancelAssistant,
    ResumeAssistant {
        origin: ReviewOrigin,
    },
    PlanRoute(NavRequest),
    /// Abandon the in-flight route plan. Navigator invalidates its token, so the planner's eventual
    /// answer is rejected rather than committing a route nobody is waiting for.
    CancelPlan,
    /// Plan a detour that rejoins the active route ahead.
    PlanDetour(DetourRequest),
    /// Abandon the in-flight detour plan and any planned-but-uncommitted detour.
    CancelDetour,
    /// Commit the previewed detour: splice it into the active route and make the result active.
    CommitDetour,
}

/// Which search the acquired workspace is for: the only thing the executor needs in order to open
/// the right sources. Bounded by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlannerWork {
    AssistantRoute(NavRequest),
    /// Plan the request as a leg and, for a visit, the leg back; report the figures, keep nothing.
    MeasureLegs(NavRequest),
    Route(NavRequest),
    Detour(DetourRequest),
}

/// One bounded physical navigation operation. Every variant carries the
/// [`OperationToken`] Navigator issued, and the matching [`NavigatorOutcome`] carries it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorEffect {
    /// Open the map and route sources and claim the planner workspace for `work`.
    Acquire { token: OperationToken<NavigatorTag>, work: PlannerWork },
    /// Run one bounded planner step. Navigator paces the search: a step is a unit of work, not
    /// a whole search, so a plan never monopolises a pass.
    Step { token: OperationToken<NavigatorTag> },
    /// Write the finished search as a route object and commit it to the store.
    CommitRoute { token: OperationToken<NavigatorTag> },
    /// Splice the planned detour into the active route and commit the derived route.
    CommitDetour { token: OperationToken<NavigatorTag> },
    /// Finish physical cleanup before acknowledging. Keep an adopted route or pending preview
    /// only when `retain_result` is true; otherwise retract this operation's exact publication.
    Release { token: OperationToken<NavigatorTag>, family: PlanFamily, retain_result: bool },
}

impl NavigatorEffect {
    pub fn token(&self) -> OperationToken<NavigatorTag> {
        match self {
            NavigatorEffect::Acquire { token, .. }
            | NavigatorEffect::Step { token }
            | NavigatorEffect::CommitRoute { token }
            | NavigatorEffect::CommitDetour { token }
            | NavigatorEffect::Release { token, .. } => *token,
        }
    }
}

/// How far one [`Step`](NavigatorEffect::Step) got. Navigator, not the executor, decides what to do
/// next: keep stepping, or commit.
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
/// it. An unsupported detour is not in this enum at all: that is a missing
/// [`NavigatorCapabilities::plan_detour`](crate::device_core::NavigatorCapabilities), and a device
/// without the planner must never report it as `NoPath`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorError {
    /// The search itself failed: no path, or the fixed scratch exhausted.
    Plan(NavError),
    /// The planner workspace or a source could not be claimed.
    Workspace,
    /// The store refused the commit; the previously active route is untouched.
    Store,
    /// An admitted map or original-route source changed.
    SourceChanged,
    Movement,
    Unavailable,
    DurabilityUnknown,
}

/// The result of one [`NavigatorEffect`] — success, a typed failure, or cancellation. Never
/// `Busy`: refusing to *start* work is an admission result the slot reports (see
/// [`device_core::slots`](crate::device_core::slots)), not an operation that failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigatorOutcome {
    ReleaseUnresolved {
        token: OperationToken<NavigatorTag>,
    },
    ReviewReady {
        token: OperationToken<NavigatorTag>,
    },
    /// The workspace and sources are held; stepping may begin.
    Acquired {
        token: OperationToken<NavigatorTag>,
    },
    Stepped {
        token: OperationToken<NavigatorTag>,
        progress: PlannerProgress,
    },
    /// The planned route was committed under `route`.
    PlanFinished {
        token: OperationToken<NavigatorTag>,
        route: CatalogObjectId,
    },
    /// The detour search finished and its preview figures are ready. The preview *polyline* reaches
    /// the screens as a keyed derived input, never here.
    DetourFinished {
        token: OperationToken<NavigatorTag>,
        preview: DetourPreview,
    },
    /// The spliced detour was committed under `route` — the re-adoption key.
    DetourCommitted {
        token: OperationToken<NavigatorTag>,
        route: CatalogObjectId,
    },
    /// The workspace and sources are back with the executor.
    Released {
        token: OperationToken<NavigatorTag>,
    },
    Failed {
        token: OperationToken<NavigatorTag>,
        error: NavigatorError,
    },
    /// The executor abandoned the operation without completing it.
    Cancelled {
        token: OperationToken<NavigatorTag>,
    },
}

impl NavigatorOutcome {
    /// The operation this outcome answers. Navigator accepts it only while the token is current.
    pub fn token(&self) -> OperationToken<NavigatorTag> {
        match self {
            NavigatorOutcome::ReleaseUnresolved { token }
            | NavigatorOutcome::ReviewReady { token, .. }
            | NavigatorOutcome::Acquired { token }
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

// Layout tripwires: a navigation message is a request, an identity, or a handful of figures.
const _: () = assert!(core::mem::size_of::<NavigatorIntent>() <= 48, "an intent is a bounded request");
const _: () = assert!(core::mem::size_of::<NavigatorEffect>() <= 56, "the planner request plus a token");
const _: () =
    assert!(core::mem::size_of::<NavigatorOutcome>() <= 40, "one bounded result; review figures stay with Navigator");
const _: () = assert!(core::mem::size_of::<NavigatorError>() <= 2, "a verdict, not a report");
const _: () = assert!(core::mem::size_of::<PlannerWork>() <= 48, "the largest planner request");
const _: () = assert!(core::mem::size_of::<PlannerProgress>() <= 1, "a two-state answer");

/// Where one planning family is in the lifecycle Navigator owns.
///
/// Two families run this independently: a route search and a detour search take the same nav arm but
/// have their own commands, answers and failure tiers. The route family never reaches
/// [`PreviewReady`](PlanPhase::PreviewReady) or [`Committing`](PlanPhase::Committing), because a
/// planned route is adopted straight from its answer while a detour is previewed and then spliced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PlanPhase {
    #[default]
    Idle,
    /// The rider asked; no executor has taken the work yet. A cancel here annihilates the request:
    /// the net intent is "no plan", so nothing is ever started.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum OperationPhase {
    Idle,
    Acquiring,
    NextStep,
    Stepping,
    NextCommit,
    Committing,
    NextRelease,
    Releasing,
}

// The phase and the cancellation mask must stay as small as the two booleans they encode.
const _: () = assert!(core::mem::size_of::<(OperationPhase, u8)>() == core::mem::size_of::<[bool; 2]>());
const _: () = assert!(core::mem::align_of::<(OperationPhase, u8)>() == core::mem::align_of::<[bool; 2]>());

/// A lead-in plan: its leg, and once the preview is in, the leg's length and where it joins the
/// route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LeadPlan {
    leg: obc_route::Leg,
    lead_m: u32,
    join_m: u32,
}

/// A route the device spliced in front of a stored route: Ride to start, or the rest of the day
/// before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeadIn {
    pub splice: crate::CatalogObjectId,
    pub route: crate::CatalogObjectId,
    /// Metres of the splice before it joins `route`, and metres into `route` where it joins.
    pub lead_m: u32,
    pub join_m: u32,
    /// Where the rest of the day before starts on that day's route. `None` for Ride to start.
    pub rest_from_m: Option<u32>,
}

impl LeadIn {
    /// Where splice metres `m` lie: `Ok` metres into the route, or `Err` metres into the day
    /// before, for a rest. Ride to start is not on the route yet, so its lead reads as the route
    /// start.
    pub fn position(&self, m: u32) -> Result<u32, u32> {
        match self.rest_from_m {
            Some(from_m) if m < self.lead_m => Err(from_m + m),
            _ => Ok(self.join_m + m.saturating_sub(self.lead_m)),
        }
    }
}

/// The domain that owns active-route following, route planning, detour planning, preview, and
/// commit.
///
/// Persistent route identity, match and guidance state, route caches, seam state, and travel heading
/// live here. Screens borrow [`RouteState`] directly; the short-lived
/// [`Prepare`](crate::screen::Prepare) context copies only the route facts one pre-draw pass needs.
///
/// Everything one rider request passes through lives here and nowhere else: the undelivered request,
/// the cancel that annihilates it, the phase, the operation token, and the [`CoreMode`] search level
/// the planner's liveness drives.
pub struct NavigatorMachine {
    /// The one operation token, and it is genuinely one:
    /// [`TokenSource::issue`](crate::device_core::TokenSource::issue) bumps a single generation, so
    /// only the newest operation is ever current across both families. A second concurrent operation
    /// would make the first one's answer stale, `accepts` would refuse it, and its family's search
    /// level would never be released. That is why [`next_plan_effect`](Self::next_plan_effect) and
    /// [`next_commit_effect`](Self::next_commit_effect) hand out at most one operation at a time.
    ///
    /// [`CoreMode`]'s two search levels are not the same question: they track which family's
    /// terminal edge may release what, and stay per-family whatever the token layer allows.
    ops: TokenSource<NavigatorTag>,
    review: ReviewState,
    visit: visit::VisitState,
    /// The family that owns physical work, through the acknowledged release.
    live: Option<PlanFamily>,
    route: PlanPhase,
    detour: PlanPhase,
    route_request: Option<NavRequest>,
    detour_request: Option<DetourRequest>,
    phase: OperationPhase,
    // Bits 0–1: family cleanup owed. Bit 2: outstanding release retains a preview.
    cancel_mask: u8,
    detour_commit: bool,
    /// The detour-family plan leads into a stored route: Ride to start, or the rest of the day before.
    /// It outlives the request, which leaves at Acquire.
    lead_plan: Option<LeadPlan>,
    /// The last adopted lead-in. A ride on its splice is saved under the route's name, and counts
    /// as a ride on that route.
    lead_in: Option<LeadIn>,
    following: RouteState,
    /// Resident per-route caches, each with its own build key.
    profile: Option<Profile>,
    profile_route: Option<usize>,
    climbs: Climbs,
    climbs_route: Option<usize>,
    waypoints: Waypoints,
    waypoints_route: Option<usize>,
    /// The loaded route whose bike type the settings do not have yet; see
    /// [`take_loaded_bike_type`](Self::take_loaded_bike_type).
    bike_type_owed: Option<usize>,
    climb_profile: ClimbProfile,
    #[cfg(test)]
    climb_fill_count: u32,
    /// The one route matcher and the active-route key it last locked to.
    route_match: RouteMatch,
    matched_route: Option<usize>,
    /// Where the next fresh ride joins the active route, instead of where its first fix locks.
    join_m: Option<u32>,
    /// Where the rider stood on the route at Finish, which unloads it before the store confirms the
    /// save.
    ride_end: Option<following::RideEnd>,
}

impl NavigatorMachine {
    define_placement_constructors!(
        pub(crate) fn new();
        /// Initialize the Navigator in place. Its route caches are too large for a device stack
        /// temporary, so the board and [`crate::App`] use this path.
        pub(crate) unsafe fn init_in_place;
        fields {
            ops: TokenSource::new(),
            review: ReviewState::new(),
            visit: visit::VisitState::new(),
            live: None,
            route: PlanPhase::Idle,
            detour: PlanPhase::Idle,
            route_request: None,
            detour_request: None,
            phase: OperationPhase::Idle,
            cancel_mask: 0,
            detour_commit: false,
            lead_plan: None,
            lead_in: None,
            following: RouteState::new(),
            profile: None,
            profile_route: None,
            climbs: Climbs::new(),
            climbs_route: None,
            waypoints: Waypoints::new(),
            waypoints_route: None,
            bike_type_owed: None,
            climb_profile: ClimbProfile::new(),
            #[cfg(test)]
            climb_fill_count: 0,
            route_match: RouteMatch::new(),
            matched_route: None,
            join_m: None,
            ride_end: None,
        }
    );

    /// Admit one rider request. Navigator decides what it means; nothing here can fail, because
    /// every intent either supersedes what came before or annihilates it.
    ///
    /// Cancellation clears undelivered work and invalidates its current answer. The search level
    /// stays engaged until the executor acknowledges release, including any pending publication.
    pub(crate) fn admit_intent(&mut self, intent: NavigatorIntent) {
        match intent {
            NavigatorIntent::AcceptAssistant { origin, profile } => self.accept_review(origin, profile),
            NavigatorIntent::CancelAssistant => self.cancel_review(),
            NavigatorIntent::ResumeAssistant { origin } => self.resume_review(origin),
            NavigatorIntent::PlanRoute(request) => {
                if !self.prepare_ordinary_route() {
                    return;
                }
                self.supersede(PlanFamily::Route);
                self.route_request = Some(request);
                self.route = PlanPhase::Requested;
            }
            NavigatorIntent::CancelPlan => {
                self.route_request = None;
                self.supersede(PlanFamily::Route);
                self.route = PlanPhase::Idle;
            }
            NavigatorIntent::PlanDetour(request) => {
                if self.active_visit() {
                    return;
                }
                self.supersede(PlanFamily::Detour);
                self.lead_plan = (request.leg != obc_route::Leg::Detour).then_some(LeadPlan {
                    leg: request.leg,
                    lead_m: 0,
                    join_m: 0,
                });
                self.detour_request = Some(request);
                self.detour = PlanPhase::Requested;
            }
            NavigatorIntent::CancelDetour => {
                self.detour_request = None;
                self.detour_commit = false;
                self.lead_plan = None;
                self.supersede(PlanFamily::Detour);
                self.detour = PlanPhase::Idle;
            }
            NavigatorIntent::CommitDetour => self.detour_commit = true,
        }
    }

    fn family_bit(family: PlanFamily) -> u8 {
        match family {
            PlanFamily::Route => 1,
            PlanFamily::Detour => 2,
        }
    }

    fn supersede(&mut self, family: PlanFamily) {
        if self.live == Some(family)
            || match family {
                PlanFamily::Route => self.route == PlanPhase::PreviewReady,
                PlanFamily::Detour => self.detour == PlanPhase::PreviewReady,
            }
        {
            self.cancel_mask |= Self::family_bit(family);
            if self.live.is_none() {
                self.live = Some(family);
            }
            if self.live == Some(family) && self.phase != OperationPhase::Releasing {
                self.ops.invalidate();
                self.phase = OperationPhase::NextRelease;
            }
        }
    }

    pub(crate) fn live_family(&self) -> Option<PlanFamily> {
        self.live
    }

    /// One physical operation at a time, including the release that permits replacement work.
    pub(crate) fn next_effect(&mut self, mode: &mut CoreMode) -> Option<NavigatorEffect> {
        if let Some(family) = self.live {
            return match self.phase {
                OperationPhase::NextRelease => self.next_release(family, mode),
                OperationPhase::NextStep => {
                    self.phase = OperationPhase::Stepping;
                    Some(NavigatorEffect::Step { token: self.ops.issue() })
                }
                OperationPhase::NextCommit => {
                    self.phase = OperationPhase::Committing;
                    Some(NavigatorEffect::CommitRoute { token: self.ops.issue() })
                }
                _ => None,
            };
        }
        if self.cancel_mask & 3 != 0 {
            let family = if self.cancel_mask & 1 != 0 { PlanFamily::Route } else { PlanFamily::Detour };
            self.live = Some(family);
            self.phase = OperationPhase::NextRelease;
            return self.next_release(family, mode);
        }
        self.next_plan_effect(PlanFamily::Route, mode)
            .or_else(|| self.next_plan_effect(PlanFamily::Detour, mode))
            .or_else(|| self.next_commit_effect())
    }

    pub(crate) fn next_release(&mut self, family: PlanFamily, _mode: &mut CoreMode) -> Option<NavigatorEffect> {
        if self.live != Some(family) || self.phase != OperationPhase::NextRelease {
            return None;
        }
        self.phase = OperationPhase::Releasing;
        let retain_result = match family {
            PlanFamily::Route => {
                matches!(self.route, PlanPhase::Active | PlanPhase::PreviewReady)
                    || self.review.status == ReviewStatus::Unresolved
            }
            PlanFamily::Detour => matches!(self.detour, PlanPhase::PreviewReady | PlanPhase::Active),
        } && self.cancel_mask & Self::family_bit(family) == 0;
        self.cancel_mask = (self.cancel_mask & 3)
            | if retain_result
                && match family {
                    PlanFamily::Route => self.route == PlanPhase::PreviewReady,
                    PlanFamily::Detour => self.detour == PlanPhase::PreviewReady,
                }
            {
                4
            } else {
                0
            };
        Some(NavigatorEffect::Release { token: self.ops.issue(), family, retain_result })
    }

    pub(crate) fn next_plan_effect(&mut self, family: PlanFamily, mode: &mut CoreMode) -> Option<NavigatorEffect> {
        if self.live.is_some() {
            return None;
        }
        let work = match family {
            PlanFamily::Route => {
                if self.review.change.is_some() || self.review.status == ReviewStatus::Unresolved {
                    return None;
                }
                let request = self.route_request.take()?;
                if self.review.measure {
                    PlannerWork::MeasureLegs(request)
                } else if self.review.status == ReviewStatus::Planning {
                    PlannerWork::AssistantRoute(request)
                } else {
                    PlannerWork::Route(request)
                }
            }
            PlanFamily::Detour => PlannerWork::Detour(self.detour_request.take()?),
        };
        match family {
            PlanFamily::Route => self.route = PlanPhase::Planning,
            PlanFamily::Detour => self.detour = PlanPhase::Planning,
        }
        mode.search_started(family);
        self.live = Some(family);
        self.phase = OperationPhase::Acquiring;
        Some(NavigatorEffect::Acquire { token: self.ops.issue(), work })
    }

    pub(crate) fn next_commit_effect(&mut self) -> Option<NavigatorEffect> {
        if self.live.is_some() {
            return None;
        }
        core::mem::take(&mut self.detour_commit).then(|| {
            self.detour = PlanPhase::Committing;
            self.live = Some(PlanFamily::Detour);
            self.phase = OperationPhase::Committing;
            NavigatorEffect::CommitDetour { token: self.ops.issue() }
        })
    }

    pub(crate) fn accepts(&self, outcome: &NavigatorOutcome) -> bool {
        if !self.ops.is_current(outcome.token()) {
            return false;
        }
        match outcome {
            NavigatorOutcome::Acquired { .. } => self.phase == OperationPhase::Acquiring,
            NavigatorOutcome::Stepped { progress, .. } => {
                self.phase == OperationPhase::Stepping
                    && (*progress == PlannerProgress::Searching || self.live == Some(PlanFamily::Route))
            }
            NavigatorOutcome::ReviewReady { .. } => {
                (self.phase == OperationPhase::Committing
                    || self.phase == OperationPhase::Stepping && self.review.measure)
                    && self.live == Some(PlanFamily::Route)
                    && self.review.context.is_some()
            }
            NavigatorOutcome::PlanFinished { .. } => {
                self.phase == OperationPhase::Committing && self.live == Some(PlanFamily::Route)
            }
            NavigatorOutcome::DetourFinished { .. } => {
                self.phase == OperationPhase::Stepping && self.live == Some(PlanFamily::Detour)
            }
            NavigatorOutcome::DetourCommitted { .. } => {
                self.phase == OperationPhase::Committing && self.live == Some(PlanFamily::Detour)
            }
            NavigatorOutcome::Released { .. } | NavigatorOutcome::ReleaseUnresolved { .. } => {
                self.phase == OperationPhase::Releasing
            }
            NavigatorOutcome::Failed { .. } | NavigatorOutcome::Cancelled { .. } => {
                matches!(self.phase, OperationPhase::Acquiring | OperationPhase::Stepping | OperationPhase::Committing)
            }
        }
    }

    pub(crate) fn progressed(&mut self, progress: PlannerProgress) {
        self.ops.invalidate();
        self.phase = match progress {
            PlannerProgress::Searching => OperationPhase::NextStep,
            PlannerProgress::Reached => OperationPhase::NextCommit,
        };
    }

    /// Product completion keeps the physical workspace held until its release is acknowledged.
    pub(crate) fn note_answer(&mut self, family: PlanFamily, phase: PlanPhase, mode: &mut CoreMode) -> bool {
        self.ops.invalidate();
        match family {
            PlanFamily::Route => self.route = phase,
            PlanFamily::Detour => self.detour = phase,
        }
        if self.live == Some(family) {
            self.phase = OperationPhase::NextRelease;
            false
        } else {
            mode.search_ended(family)
        }
    }

    pub(crate) fn released(&mut self, mode: &mut CoreMode) -> bool {
        self.ops.invalidate();
        let Some(family) = self.live else { return false };
        if self.cancel_mask & 4 != 0 && self.cancel_mask & Self::family_bit(family) != 0 {
            self.phase = OperationPhase::NextRelease;
            return false;
        }
        self.phase = OperationPhase::Idle;
        self.live = None;
        self.cancel_mask &= 3;
        self.cancel_mask &= !Self::family_bit(family);
        mode.search_ended(family)
    }

    /// Whether a detour plan exists at all — requested, running, previewed, committing or adopted.
    /// The falling edge of this is what drops the preview polyline drawn over the active route.
    pub(crate) fn detour_planned(&self) -> bool {
        self.detour != PlanPhase::Idle
    }

    /// What the detour-family plan leads into a route with: Ride to start's approach, or the rest of
    /// the day before. `None` for a detour.
    pub(crate) fn lead_leg(&self) -> Option<obc_route::Leg> {
        self.lead_plan.map(|plan| plan.leg)
    }

    /// The planned lead's length, and where it joins the route, from the plan's preview.
    pub(crate) fn note_lead_preview(&mut self, preview: &crate::host::DetourPreview) {
        if let Some(plan) = &mut self.lead_plan {
            plan.lead_m = preview.total_distance_m;
            plan.join_m = preview.rejoin_m;
        }
    }

    /// The planned lead is spliced as `splice` in front of `route`.
    pub(crate) fn adopt_lead_in(&mut self, splice: crate::CatalogObjectId, route: crate::CatalogObjectId) {
        self.lead_in = self.lead_plan.map(|plan| LeadIn {
            splice,
            route,
            lead_m: plan.lead_m,
            join_m: plan.join_m,
            rest_from_m: match plan.leg {
                obc_route::Leg::Rest { from_m, .. } => Some(from_m),
                _ => None,
            },
        });
    }

    pub(crate) fn lead_in(&self) -> Option<LeadIn> {
        self.lead_in
    }

    /// Whether the in-flight detour operation is the splice rather than the search — the two have
    /// the same family and different answers.
    pub(crate) fn detour_committing(&self) -> bool {
        self.detour == PlanPhase::Committing
    }

    /// Whether this family holds a plan that is still a question put to the rider, so a screen
    /// could still drop it. [`Active`](PlanPhase::Active) is not: the result is adopted, and the
    /// flow truncates its own screens off once the rider has it. Nor is
    /// [`Committing`](PlanPhase::Committing), a splice already under way with no answer to give.
    pub(crate) fn plan_awaits_rider(&self, family: PlanFamily) -> bool {
        let phase = match family {
            PlanFamily::Route => self.route,
            PlanFamily::Detour => self.detour,
        };
        matches!(phase, PlanPhase::Requested | PlanPhase::Planning | PlanPhase::PreviewReady | PlanPhase::Failed)
    }

    /// Whether an ordinary route search is out with the planner: the phases the planning spinner
    /// is up for, and the only ones the spinner's Back can cancel. A route preview or failure is
    /// the Assistant's review, which has its own screens and its own release.
    pub(crate) fn route_search_running(&self) -> bool {
        matches!(self.route, PlanPhase::Requested | PlanPhase::Planning)
    }

    /// A detour commit answered. Success adopts the spliced route; a failure returns the rider to
    /// the preview they came from, which is what makes a failed commit retryable.
    pub(crate) fn note_commit(&mut self, committed: bool) {
        self.ops.invalidate();
        self.phase = OperationPhase::NextRelease;
        self.detour = if committed { PlanPhase::Active } else { PlanPhase::PreviewReady };
    }

    #[cfg(test)]
    pub(crate) fn cancel_pending(&self, family: PlanFamily) -> bool {
        match family {
            PlanFamily::Route => self.cancel_mask & 1 != 0,
            PlanFamily::Detour => self.cancel_mask & 2 != 0,
        }
    }

    /// Engage or release a `Route` run without a real planner — the simulator's `--freeze` flag and
    /// the snapshot harness. No production path reaches it.
    pub(crate) fn debug_set_plan_live(&mut self, live: bool, mode: &mut CoreMode) -> bool {
        if live {
            mode.search_started(PlanFamily::Route);
            self.route = PlanPhase::Planning;
            false
        } else {
            self.note_answer(PlanFamily::Route, PlanPhase::Idle, mode)
        }
    }

    /// Follow the undelivered detour request through a route-catalog rescan by durable identity. A
    /// vanished route drops it, exactly as it drops the caches keyed on that route.
    pub(crate) fn remap_detour_route(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.detour_request =
            self.detour_request.and_then(|req| remap(req.route).map(|route| DetourRequest { route, ..req }));
    }

    /// A fresh tracking session drops the product and requests cleanup of any held preview.
    pub(crate) fn reset_detour(&mut self) {
        self.detour_request = None;
        self.detour_commit = false;
        // An adopted splice is the route being ridden, and a ride can open while its release is
        // still out. Cancelling that release would retract the route.
        if self.detour == PlanPhase::Active {
            return;
        }
        self.lead_plan = None;
        self.supersede(PlanFamily::Detour);
        self.detour = PlanPhase::Idle;
    }

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
            review,
            visit,
            live,
            route,
            detour,
            route_request,
            detour_request,
            phase,
            cancel_mask,
            detour_commit,
            lead_plan,
            lead_in,
            following,
            profile,
            profile_route,
            climbs,
            climbs_route,
            waypoints,
            waypoints_route,
            bike_type_owed,
            climb_profile,
            climb_fill_count,
            route_match,
            matched_route,
            join_m,
            ride_end,
        } = self;
        assert_eq!(review.status, ReviewStatus::Idle);
        visit.assert_boot_state();
        assert_eq!(format!("{ops:?}"), "TokenSource(0)", "no navigation operation has been issued");
        assert!(live.is_none(), "no operation is in flight");
        assert!(*route == PlanPhase::Idle && *detour == PlanPhase::Idle, "neither family has been asked");
        assert!(route_request.is_none() && detour_request.is_none(), "no request waiting");
        assert!(*phase == OperationPhase::Idle && *cancel_mask == 0 && !*detour_commit, "no physical work pending");
        assert!(lead_plan.is_none() && lead_in.is_none(), "no lead-in is planned or adopted");
        following.assert_boot_state();
        assert!(profile.is_none() && profile_route.is_none(), "no elevation profile cached");
        assert!(climbs.is_empty() && climbs_route.is_none(), "no climbs before a route loads");
        assert!(waypoints.is_empty() && waypoints_route.is_none(), "no waypoints before a route loads");
        assert!(bike_type_owed.is_none(), "no route has been loaded");
        assert!(climb_profile.cols().iter().all(|&column| column == 0), "the climb detail starts flat");
        assert_eq!(*climb_fill_count, 0, "the climb detail has not been filled");
        assert!(!route_match.started() && matched_route.is_none(), "the matcher is unlocked");
        assert!(join_m.is_none() && ride_end.is_none(), "no ride waits to join or has ended");
    }
}

impl Default for NavigatorMachine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod machine_tests {
    use super::*;

    /// Navigator writes the search levels; `App` owns the `CoreMode` they live in. These tests own
    /// one directly so the pair can be driven without an `App`.
    struct Nav {
        machine: NavigatorMachine,
        mode: CoreMode,
    }

    impl Nav {
        fn new() -> Nav {
            Nav { machine: NavigatorMachine::new(), mode: CoreMode::new() }
        }

        fn admit_intent(&mut self, intent: NavigatorIntent) {
            self.machine.admit_intent(intent);
        }

        fn next_effect(&mut self) -> Option<NavigatorEffect> {
            self.machine.next_effect(&mut self.mode)
        }

        fn note_answer(&mut self, family: PlanFamily, phase: PlanPhase) -> bool {
            self.machine.note_answer(family, phase, &mut self.mode)
        }

        fn frozen(&self) -> bool {
            self.mode.frozen(true)
        }

        fn searching(&self) -> bool {
            self.mode.searching()
        }
    }

    impl core::ops::Deref for Nav {
        type Target = NavigatorMachine;

        fn deref(&self) -> &NavigatorMachine {
            &self.machine
        }
    }

    impl core::ops::DerefMut for Nav {
        fn deref_mut(&mut self) -> &mut NavigatorMachine {
            &mut self.machine
        }
    }

    fn route_request(name: &str) -> NavRequest {
        NavRequest::new((0, 0), (1_000, 1_000), name)
    }

    fn detour_request() -> DetourRequest {
        DetourRequest { route: 0, from: (0, 0), progress_m: 1_000, target_m: 1_600, leg: obc_route::Leg::Detour }
    }

    /// The name of what an effect asks for, so a test can say what it expects without matching on a
    /// token it never chose.
    fn acquired(effect: Option<NavigatorEffect>) -> Option<PlannerWork> {
        match effect {
            Some(NavigatorEffect::Acquire { work, .. }) => Some(work),
            _ => None,
        }
    }

    fn release(nav: &mut Nav) -> NavigatorEffect {
        let effect = nav.next_effect().expect("release owed");
        assert!(matches!(effect, NavigatorEffect::Release { .. }));
        assert!(nav.next_effect().is_none(), "wait for acknowledgment");
        assert!(nav.accepts(&NavigatorOutcome::Released { token: effect.token() }));
        nav.machine.released(&mut nav.mode);
        effect
    }

    #[test]
    fn each_phase_requires_its_own_answer_and_fresh_token() {
        let mut nav = Nav::new();
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("col")));
        let acquire = nav.next_effect().unwrap();
        assert!(matches!(acquire, NavigatorEffect::Acquire { .. }));
        assert!(!nav.accepts(&NavigatorOutcome::PlanFinished { token: acquire.token(), route: 9 }));
        assert!(nav.next_effect().is_none());
        assert!(nav.accepts(&NavigatorOutcome::Acquired { token: acquire.token() }));
        nav.progressed(PlannerProgress::Searching);
        let first = nav.next_effect().unwrap();
        assert!(matches!(first, NavigatorEffect::Step { .. }));
        assert_ne!(first.token(), acquire.token());
        let answer = NavigatorOutcome::Stepped { token: first.token(), progress: PlannerProgress::Searching };
        assert!(nav.accepts(&answer));
        nav.progressed(PlannerProgress::Searching);
        let second = nav.next_effect().unwrap();
        assert_ne!(first.token(), second.token());
        assert!(!nav.accepts(&answer), "duplicate cannot advance a later step");
        nav.progressed(PlannerProgress::Reached);
        let commit = nav.next_effect().unwrap();
        assert!(matches!(commit, NavigatorEffect::CommitRoute { .. }));
        assert!(!nav.accepts(&NavigatorOutcome::Acquired { token: commit.token() }));
        assert!(nav.accepts(&NavigatorOutcome::PlanFinished { token: commit.token(), route: 9 }));
        nav.note_answer(PlanFamily::Route, PlanPhase::Active);
        assert!(nav.frozen(), "product completion does not release the arena");
        release(&mut nav);
        assert!(!nav.frozen());
        assert_eq!(nav.route, PlanPhase::Active);
        assert!(nav.next_effect().is_none());
    }

    #[test]
    fn cancellation_and_replacement_wait_for_release_acknowledgment() {
        let mut nav = Nav::new();
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("first")));
        let first = nav.next_effect().unwrap();
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("second")));
        assert!(!nav.accepts(&NavigatorOutcome::Acquired { token: first.token() }));
        assert!(nav.frozen());
        release(&mut nav);
        let replacement = nav.next_effect().unwrap();
        assert!(matches!(acquired(Some(replacement)), Some(PlannerWork::Route(req)) if req.name() == "second"));
        nav.admit_intent(NavigatorIntent::CancelPlan);
        release(&mut nav);
        assert!(!nav.searching());
        assert!(nav.next_effect().is_none());
    }

    #[test]
    fn undelivered_cancel_and_other_family_edges_do_not_start_or_release_work() {
        let mut nav = Nav::new();
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        nav.admit_intent(NavigatorIntent::CancelDetour);
        assert!(nav.next_effect().is_none());
        nav.admit_intent(NavigatorIntent::PlanRoute(route_request("col")));
        let first = nav.next_effect().unwrap();
        nav.admit_intent(NavigatorIntent::CancelDetour);
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        assert!(nav.next_effect().is_none());
        assert!(nav.accepts(&NavigatorOutcome::Acquired { token: first.token() }));
        assert!(nav.frozen());
        nav.note_answer(PlanFamily::Route, PlanPhase::Failed);
        release(&mut nav);
        assert!(matches!(acquired(nav.next_effect()), Some(PlannerWork::Detour(_))));
    }

    #[test]
    fn detour_preview_and_failed_commit_survive_release_until_explicit_cancel() {
        let mut nav = Nav::new();
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        nav.next_effect().unwrap();
        nav.progressed(PlannerProgress::Searching);
        let step = nav.next_effect().unwrap();
        assert!(!nav.accepts(&NavigatorOutcome::Stepped { token: step.token(), progress: PlannerProgress::Reached }));
        nav.note_answer(PlanFamily::Detour, PlanPhase::PreviewReady);
        assert!(matches!(release(&mut nav), NavigatorEffect::Release { retain_result: true, .. }));
        nav.admit_intent(NavigatorIntent::CommitDetour);
        let commit = nav.next_effect().unwrap();
        assert!(matches!(commit, NavigatorEffect::CommitDetour { .. }));
        nav.note_commit(false);
        release(&mut nav);
        assert_eq!(nav.detour, PlanPhase::PreviewReady);
        nav.admit_intent(NavigatorIntent::CommitDetour);
        assert!(matches!(nav.next_effect(), Some(NavigatorEffect::CommitDetour { .. })));
        nav.note_commit(true);
        assert!(matches!(release(&mut nav), NavigatorEffect::Release { retain_result: true, .. }));
        assert_eq!(nav.detour, PlanPhase::Active);
    }

    #[test]
    fn cancellation_during_preview_release_requires_final_lease_cleanup() {
        let mut nav = Nav::new();
        nav.admit_intent(NavigatorIntent::PlanDetour(detour_request()));
        nav.next_effect().unwrap();
        nav.note_answer(PlanFamily::Detour, PlanPhase::PreviewReady);
        let first = nav.next_effect().unwrap();
        assert!(matches!(first, NavigatorEffect::Release { retain_result: true, .. }));
        nav.reset_detour();
        assert!(nav.accepts(&NavigatorOutcome::Released { token: first.token() }));
        nav.machine.released(&mut nav.mode);
        assert!(nav.frozen());
        assert!(matches!(release(&mut nav), NavigatorEffect::Release { retain_result: false, .. }));
        assert!(!nav.frozen());
        assert_eq!(nav.detour, PlanPhase::Idle);
    }
}
