//! The **Navigator** domain protocol: route planning, detour planning, preview and commit (#1436).
//!
//! Navigator owns the whole planning lifecycle — `Idle → Planning → PreviewReady → Committing →
//! Active` (or `Failed`) — and the rules a platform executor must never decide: when a plan is
//! cancelled, when a replacement supersedes an in-flight one, and when a late planner answer is too
//! old to matter. The executor is left with five bounded mechanisms: take the sources and the
//! workspace, run **one** planner step, commit a route, commit a detour, give the resources back.
//!
//! This module currently holds only the vocabulary. The state machine, and the cutover from
//! [`HostCommand::PlanRoute`](crate::HostCommand) and friends, arrive in later slices of #1433.
//!
//! Bulk stays out: the emitted OBCR bytes, the corridor blacklist and the detour preview *polyline*
//! never ride an effect or an outcome. What crosses is an identity, a bounded request, and the
//! preview *figures* the HUD prints.

use obc_route::nav::NavError;

use crate::activity::{DetourRequest, NavRequest};
use crate::device_core::{NavigatorTag, OperationToken};
use crate::host::DetourPreview;
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
