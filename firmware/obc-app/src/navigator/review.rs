//! Immutable Assistant review and durable checkpoint policy owned by Navigator.
use super::NavigatorError;
use crate::device_core::{MetadataTag, OperationToken, StoreIdentity};
use obc_formats::assistant::{NavigatorCheckpoint, PayloadFingerprint};
use obc_formats::obcr::RouteSourceKey;

pub const REVIEW_ALONG_TOLERANCE_M: u32 = 50;
pub const REVIEW_LATERAL_TOLERANCE_M: u32 = 30;
pub const REVIEW_FACTS_POLICY: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPurpose {
    Destination,
    Visit,
    ReturnToRoute,
    Easier(obc_route::nav::Objective),
}

/// Frozen request inputs. Exact sources must still match before candidate publication and acceptance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewContext {
    pub purpose: ReviewPurpose,
    pub map: RouteSourceKey,
    pub store: StoreIdentity,
    pub original: Option<PayloadFingerprint>,
    pub origin: (i32, i32),
    pub progress_m: u32,
    pub occurrence: u32,
    pub required_anchors_m: [u32; 3],
    pub profile: u8,
    pub facts_policy: u16,
    pub unresolved_avoidance: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewOrigin {
    pub fix: (i32, i32),
    pub progress_m: u32,
    pub occurrence: u32,
    pub lateral_m: u32,
    pub trustworthy: bool,
}

impl ReviewContext {
    /// Both executors use the persisted original, rather than trusting a caller's avoidance flag.
    pub fn accepts_original(self, active: Option<u64>, current: Option<PayloadFingerprint>, avoidance: bool) -> bool {
        active == self.original.map(|source| source.object) && current == self.original && !avoidance
    }

    pub fn accepts_origin(self, profile: u8, current: ReviewOrigin) -> bool {
        current.trustworthy
            && self.profile == profile
            && self.facts_policy == REVIEW_FACTS_POLICY
            && self.occurrence == current.occurrence
            && self.progress_m.abs_diff(current.progress_m) <= REVIEW_ALONG_TOLERANCE_M
            && current.lateral_m <= REVIEW_LATERAL_TOLERANCE_M
            && (self.original.is_some()
                || obc_map_scene::ground_dist_m(self.origin, current.fix) <= REVIEW_ALONG_TOLERANCE_M as f32)
    }
}

/// Figures and fingerprint from the exact complete Route that the executor published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewedRoute {
    pub source: PayloadFingerprint,
    pub distance_m: u32,
    pub ascent_m: u32,
    pub descent_m: u32,
    pub visit_anchors_m: Option<[u32; 3]>,
    pub visit_costs: Option<obc_route::visit::VisitCosts>,
}

impl ReviewedRoute {
    /// Derive the preview only from the exact published candidate's validated bytes.
    pub fn read(
        source: PayloadFingerprint,
        bytes: &dyn obc_formats::io::ByteSource,
        context: ReviewContext,
    ) -> Result<Self, NavigatorError> {
        let info = obc_route::RouteObjectInfo::read(bytes).map_err(|_| NavigatorError::Unavailable)?;
        if !info.assistant_candidate
            || bytes.len() != source.length
            || info.attribution_map != Some(context.map)
            || info.unresolved_avoidance != context.unresolved_avoidance
        {
            return Err(NavigatorError::SourceChanged);
        }
        let visit_anchors_m = if context.purpose == ReviewPurpose::Visit {
            let descriptor = info.visit.ok_or(NavigatorError::Unavailable)?;
            let original = context.original.ok_or(NavigatorError::Unavailable)?;
            if descriptor.original.store != context.store.bytes()
                || descriptor.original.object != original.object
                || descriptor.original.revision != original.revision
                || descriptor.original_anchors_m != context.required_anchors_m
            {
                return Err(NavigatorError::SourceChanged);
            }
            Some(descriptor.accepted_anchors_m)
        } else {
            if info.visit.is_some() {
                return Err(NavigatorError::Unavailable);
            }
            None
        };
        let visit_costs = if matches!(
            context.purpose,
            ReviewPurpose::Visit | ReviewPurpose::Destination | ReviewPurpose::ReturnToRoute | ReviewPurpose::Easier(_)
        ) {
            let arrival = visit_anchors_m.map_or([0, info.distance_m], |a| [a[0], a[1]]);
            Some(obc_route::visit::VisitCosts::read(bytes, arrival).map_err(|_| NavigatorError::Unavailable)?)
        } else {
            None
        };
        Ok(Self {
            source,
            distance_m: info.distance_m,
            ascent_m: info.ascent_m,
            descent_m: info.descent_m,
            visit_anchors_m,
            visit_costs,
        })
    }
}

/// Exact identity and shape of the selected ordinary route, read from the bytes the executor holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteCheckpointSource {
    pub route: PayloadFingerprint,
    pub distance_m: u32,
    pub unresolved_avoidance: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStatus {
    Idle,
    Planning,
    Preview,
    Saving,
    Accepted,
    ResumeAvailable,
    Unresolved,
    Failed(NavigatorError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointChange {
    pub expected: Option<NavigatorCheckpoint>,
    pub next: Option<NavigatorCheckpoint>,
}

/// What follows a committed checkpoint write. `Activate` records a ride only for an accepted plan,
/// because recovery restores guidance and never a recording, and `Selected` follows the rider's own
/// route selection, which is already active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AfterCheckpoint {
    Activate { route: u64, record: bool },
    Select(Option<usize>),
    Restore { route: u64, progress_m: u32 },
    Phase,
    Selected,
}

pub(super) struct ReviewState {
    pub store: Option<StoreIdentity>,
    pub context: Option<ReviewContext>,
    pub restore: Option<RouteSourceKey>,
    pub preview: Option<ReviewedRoute>,
    pub preview_index: Option<usize>,
    pub unaccepted: u64,
    pub internal_routes: u64,
    pub checkpoint: Option<NavigatorCheckpoint>,
    pub change: Option<Option<NavigatorCheckpoint>>,
    pub token: Option<OperationToken<MetadataTag>>,
    pub submitted: bool,
    pub cancel_after: bool,
    pub status: ReviewStatus,
    pub after: AfterCheckpoint,
    pub recovery_seen: bool,
    pub owed: bool,
    pub latest_origin: Option<ReviewOrigin>,
}
impl ReviewState {
    pub const fn new() -> Self {
        Self {
            store: None,
            context: None,
            restore: None,
            preview: None,
            preview_index: None,
            unaccepted: 0,
            internal_routes: 0,
            checkpoint: None,
            change: None,
            token: None,
            submitted: false,
            cancel_after: false,
            status: ReviewStatus::Idle,
            after: AfterCheckpoint::Phase,
            recovery_seen: false,
            owed: false,
            latest_origin: None,
        }
    }
}

use super::{NavigatorMachine, PlanFamily, PlanPhase};
use crate::metadata::{MetadataError, MetadataOutcome};
use obc_formats::assistant::JourneyPhase;

impl NavigatorMachine {
    pub(crate) fn request_review(&mut self, request: crate::activity::NavRequest, context: ReviewContext) {
        if (self.active_visit() && context.purpose != ReviewPurpose::ReturnToRoute)
            || self.review.change.is_some()
            || self.review.preview.is_some()
            || self.live.is_some()
            || self.review.status == ReviewStatus::Unresolved
        {
            return;
        }
        if context.facts_policy != REVIEW_FACTS_POLICY || context.unresolved_avoidance || request.from != context.origin
        {
            self.review.status = ReviewStatus::Failed(NavigatorError::Unavailable);
            return;
        }
        self.review.store = Some(context.store);
        self.review.context = Some(context);
        self.review.restore = None;
        self.review.status = ReviewStatus::Planning;
        self.route_request = Some(request);
        self.route = PlanPhase::Requested;
    }

    pub(crate) fn unaccepted_routes(&self) -> u64 {
        self.review.unaccepted
    }

    pub fn route_unaccepted(&self, index: usize) -> bool {
        index < 64 && self.review.unaccepted & (1 << index) != 0
    }
    /// Generated Assistant routes stay internal even after they are accepted.
    pub fn internal_routes(&self) -> u64 {
        self.review.internal_routes
    }
    pub(crate) fn set_internal_routes(&mut self, mask: u64) {
        self.review.internal_routes = mask;
    }
    pub(crate) fn set_unaccepted_routes(&mut self, mask: u64) {
        self.review.unaccepted = mask;
    }
    pub(crate) fn review_index(&mut self, index: Option<usize>) {
        self.review.preview_index = index;
    }
    /// Take the selection the rider made. An ordinary route then owes a checkpoint of its own, so
    /// a restart can offer Resume for it exactly as it does for an accepted Assistant plan.
    pub(super) fn select_now(&mut self, index: Option<usize>) {
        self.following.active_route = index;
        self.review.owed = index.is_some();
    }
    pub(crate) fn select_after_checkpoint(&mut self, index: Option<usize>) -> bool {
        if self.review.status == ReviewStatus::Unresolved {
            if self.review.change.is_some() {
                self.review.cancel_after = true;
                self.review.after = AfterCheckpoint::Select(index);
            }
            return false;
        }
        if index.is_some_and(|index| self.route_unaccepted(index))
            || (index.is_some() && index == self.review.preview_index)
        {
            return false;
        }
        if self.review.submitted {
            self.review.cancel_after = true;
            self.review.after = AfterCheckpoint::Select(index);
            return false;
        }
        if self.review.checkpoint.is_some() {
            if self.review.checkpoint.is_some_and(|standing| standing.selection) {
                // A plain selection is replaced by the next one, or cleared behind it. Only an
                // accepted plan holds guidance up until the card says it is no longer accepted.
                if index.is_none() {
                    self.review.change = Some(None);
                    self.review.after = AfterCheckpoint::Phase;
                }
                return true;
            }
            self.review.change = Some(None);
            self.review.after = AfterCheckpoint::Select(index);
            return false;
        }
        true
    }
    pub(crate) fn remap_review_keys(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        for mask in [&mut self.review.unaccepted, &mut self.review.internal_routes] {
            let mut next = 0;
            for old in 0..64 {
                if *mask & (1 << old) != 0 {
                    if let Some(new) = remap(old).filter(|&index| index < 64) {
                        next |= 1 << new;
                    }
                }
            }
            *mask = next;
        }
        self.review.preview_index = self.review.preview_index.and_then(remap);
        if let AfterCheckpoint::Select(index) = &mut self.review.after {
            *index = index.and_then(remap);
        }
    }

    pub(crate) fn released_unresolved(&mut self, mode: &mut crate::device_core::core_mode::CoreMode) {
        self.ops.invalidate();
        self.phase = super::OperationPhase::Idle;
        if let Some(family) = self.live.take() {
            mode.search_ended(family);
        }
        self.cancel_mask = 0;
    }

    pub(crate) fn reviewed(&mut self, preview: ReviewedRoute) {
        self.review.preview = Some(preview);
        self.review.status = ReviewStatus::Preview;
    }

    pub(crate) fn review_failed(&mut self, error: NavigatorError) {
        self.review.status = if error == NavigatorError::DurabilityUnknown {
            ReviewStatus::Unresolved
        } else {
            ReviewStatus::Failed(error)
        };
    }

    pub(super) fn accept_review(&mut self, origin: ReviewOrigin, profile: u8) {
        if self.review.status != ReviewStatus::Preview || self.review.change.is_some() || self.live.is_some() {
            return;
        }
        let (Some(context), Some(preview)) = (self.review.context, self.review.preview) else {
            return;
        };
        if !context.accepts_origin(profile, origin) {
            self.review.status = ReviewStatus::Failed(NavigatorError::Movement);
            return;
        }
        let (progress_m, upper_m, phase) = if context.purpose == ReviewPurpose::Visit {
            let Some([entry, stop, rejoin]) = preview.visit_anchors_m else {
                self.review.status = ReviewStatus::Failed(NavigatorError::Unavailable);
                return;
            };
            if entry > stop || stop > rejoin || rejoin > preview.distance_m {
                self.review.status = ReviewStatus::Failed(NavigatorError::Unavailable);
                return;
            }
            if entry == stop && stop == rejoin && rejoin == preview.distance_m {
                (entry, rejoin, JourneyPhase::Following)
            } else if entry == stop {
                (entry, if stop == rejoin { preview.distance_m } else { rejoin }, JourneyPhase::AtStop)
            } else {
                (entry, stop, JourneyPhase::Outbound)
            }
        } else {
            (0, preview.distance_m, JourneyPhase::Following)
        };
        // The measured candidate axis is authoritative for its accepted phase.
        let next = NavigatorCheckpoint {
            route: preview.source,
            original: if matches!(context.purpose, ReviewPurpose::Destination | ReviewPurpose::ReturnToRoute)
                || context.purpose == ReviewPurpose::Visit && phase == JourneyPhase::Following
            {
                None
            } else {
                context.original
            },
            progress_m,
            occurrence: 0,
            lon: context.origin.0,
            lat: context.origin.1,
            phase,
            unresolved_avoidance: context.unresolved_avoidance,
            selection: false,
            lower_m: progress_m,
            upper_m,
        };
        if !next.valid() {
            self.review.status = ReviewStatus::Failed(NavigatorError::Unavailable);
            return;
        }
        self.review.change = Some(Some(next));
        self.review.after = AfterCheckpoint::Activate { route: preview.source.object, record: true };
        self.review.latest_origin = Some(origin);
        self.review.status = ReviewStatus::Saving;
    }

    pub(super) fn cancel_review(&mut self) {
        if self.review.status == ReviewStatus::Accepted && self.review.preview.is_none() {
            return;
        }
        if self.review.status == ReviewStatus::Unresolved {
            self.review.cancel_after |= self.review.change.is_some();
            return;
        }
        if self.review.submitted {
            self.review.cancel_after = true;
            return;
        }
        if self
            .review
            .preview
            .is_some_and(|preview| self.review.checkpoint.is_some_and(|checkpoint| checkpoint.route == preview.source))
        {
            self.review.change = Some(None);
            self.review.status = ReviewStatus::Saving;
            self.review.after = AfterCheckpoint::Phase;
            return;
        }
        self.review.change = None;
        // An emitted but unsubmitted Metadata effect is answered Cancelled by its executor.
        self.review.status = if self.review.checkpoint.is_some() { ReviewStatus::Accepted } else { ReviewStatus::Idle };
        self.route_request = None;
        self.supersede(PlanFamily::Route);
        self.route = PlanPhase::Idle;
        self.review.preview = None;
        self.review.preview_index = None;
        self.review.context = None;
        self.review.restore = None;
    }

    pub(super) fn prepare_ordinary_route(&mut self) -> bool {
        if self.review.status == ReviewStatus::Unresolved || self.review.submitted {
            return false;
        }
        self.cancel_review();
        if self.review.checkpoint.is_some() {
            self.review.change = Some(None);
            self.review.after = AfterCheckpoint::Phase;
        }
        true
    }

    pub(crate) fn review_store_changed(&mut self, store: StoreIdentity) {
        if self.review.store.is_none_or(|bound| bound == store) {
            return;
        }
        if self.review.change.is_some()
            || self.review.preview.is_some()
            || self.live.is_some()
            || self.review.status == ReviewStatus::Unresolved
        {
            self.review.status = ReviewStatus::Unresolved;
        } else {
            self.review = ReviewState::new();
        }
    }

    /// Called only after a complete read and CRC verification on the current card.
    pub(crate) fn offer_checkpoint(&mut self, store: StoreIdentity, checkpoint: Option<NavigatorCheckpoint>) {
        if self.review.recovery_seen || self.review.change.is_some() {
            return;
        }
        self.review.store = Some(store);
        self.review.recovery_seen = true;
        self.review.checkpoint = checkpoint;
        if checkpoint.is_some() {
            // Guidance that is already following a route has nothing to resume.
            self.review.status = if self.following.active_route.is_some() {
                ReviewStatus::Accepted
            } else {
                ReviewStatus::ResumeAvailable
            };
        }
    }

    /// Give up the checkpoint so a removal the store would otherwise refuse can go out.
    pub(crate) fn clear_checkpoint_for_removal(&mut self) {
        if self.review.checkpoint.is_some() && self.review.change.is_none() && !self.review.submitted {
            self.review.change = Some(None);
            self.review.after = AfterCheckpoint::Phase;
        }
    }

    pub(super) fn resume_review(&mut self, origin: ReviewOrigin) {
        if self.review.status != ReviewStatus::ResumeAvailable {
            return;
        }
        let Some(checkpoint) = self.review.checkpoint else {
            return;
        };
        if !origin.trustworthy
            || origin.progress_m < checkpoint.lower_m
            || origin.progress_m > checkpoint.upper_m
            || origin.lateral_m > REVIEW_LATERAL_TOLERANCE_M
        {
            self.review.status = ReviewStatus::Failed(NavigatorError::Movement);
            return;
        }
        // Even explicit Resume rechecks the exact payloads through the same serialized operation.
        self.review.change = Some(Some(NavigatorCheckpoint {
            progress_m: origin.progress_m,
            occurrence: origin.occurrence,
            lon: origin.fix.0,
            lat: origin.fix.1,
            ..checkpoint
        }));
        self.review.after = AfterCheckpoint::Activate { route: checkpoint.route.object, record: false };
        self.review.latest_origin = Some(origin);
        self.review.status = ReviewStatus::Saving;
    }

    pub(crate) fn checkpoint_change(&self) -> Option<CheckpointChange> {
        if self.review.token.is_some() || self.review.status == ReviewStatus::Unresolved {
            None
        } else {
            self.review.change.map(|next| CheckpointChange { expected: self.review.checkpoint, next })
        }
    }

    pub(crate) fn checkpoint_issued(&mut self, token: OperationToken<MetadataTag>) {
        self.review.token = Some(token);
    }

    pub(crate) fn checkpoint_submission(&mut self, token: OperationToken<MetadataTag>) -> bool {
        if self.review.token != Some(token) || self.review.change.is_none() {
            return false;
        }
        self.review.submitted = true;
        true
    }

    fn checkpoint_committed(&mut self) -> Option<AfterCheckpoint> {
        self.review.status = ReviewStatus::Saving;
        let change = CheckpointChange { expected: self.review.checkpoint, next: self.review.change.take()? };
        self.review.checkpoint = change.next;
        if core::mem::take(&mut self.review.cancel_after) {
            // Resolve the submitted acceptance first, then durably clear before retirement.
            self.review.change = Some(None);
            if !matches!(self.review.after, AfterCheckpoint::Select(_)) {
                self.review.after = AfterCheckpoint::Phase;
            }
            return None;
        }
        if change.next.is_none() {
            if self.review.preview.is_some() {
                self.cancel_review();
            } else {
                self.review.status = ReviewStatus::Idle;
            }
            return Some(self.review.after);
        }
        self.review.status = ReviewStatus::Accepted;
        let selection = self.review.after == AfterCheckpoint::Selected;
        // Only a confirmed write settles the debt: a refused one leaves the previous route on the
        // card, which a restart must not offer for a route the rider is no longer following.
        self.review.owed &= !selection;
        self.review.preview = None;
        self.review.preview_index = None;
        self.review.context = None;
        if !selection {
            self.route = PlanPhase::Active;
        }
        Some(self.review.after)
    }

    fn recover_checkpoint(
        &mut self,
        store: StoreIdentity,
        checkpoint: Option<NavigatorCheckpoint>,
    ) -> Result<Option<AfterCheckpoint>, ()> {
        if self.review.store != Some(store) || self.review.status != ReviewStatus::Unresolved {
            return Err(());
        }
        let next = self.review.change.ok_or(())?;
        if checkpoint != next && checkpoint != self.review.checkpoint {
            return Err(());
        }
        self.review.token = None;
        self.review.submitted = false;
        if checkpoint == next {
            return Ok(self.checkpoint_committed());
        }
        // The complete recovered head proves the pending edit did not commit.
        self.checkpoint_unpublished();
        Ok(None)
    }

    fn checkpoint_unpublished(&mut self) {
        let retry_phase =
            self.review.after == AfterCheckpoint::Phase && self.review.change.is_some_and(|next| next.is_some());
        self.review.change = None;
        self.review.status = if self.review.preview.is_some() {
            ReviewStatus::Preview
        } else if self.review.checkpoint.is_some() {
            if self.following.active_route.is_some() {
                ReviewStatus::Accepted
            } else {
                ReviewStatus::ResumeAvailable
            }
        } else if self.review.after == AfterCheckpoint::Selected {
            // A refused selection checkpoint leaves the selection alone; nothing is owed again.
            ReviewStatus::Idle
        } else {
            ReviewStatus::Failed(NavigatorError::Store)
        };
        if retry_phase && self.review.status == ReviewStatus::Accepted {
            self.retry_visit_phase();
        }
        if core::mem::take(&mut self.review.cancel_after) {
            self.cancel_review();
        }
    }

    fn checkpoint_answer(&mut self, outcome: MetadataOutcome) -> Option<AfterCheckpoint> {
        if self.review.token != Some(outcome.token()) {
            return None;
        }
        self.review.token = None;
        self.review.submitted = false;
        match outcome {
            MetadataOutcome::CheckpointWritten { .. } => self.checkpoint_committed(),
            MetadataOutcome::Failed { error: MetadataError::RemountRequired, .. } => {
                self.review.status = ReviewStatus::Unresolved;
                None
            }
            MetadataOutcome::Failed { .. } => {
                self.checkpoint_unpublished();
                None
            }
            MetadataOutcome::Cancelled { .. } => None,
        }
    }
}

impl crate::App {
    /// Stage one exact publication under the current commit token. The outcome exposes the preview.
    pub fn assistant_preview_outcome(
        &mut self,
        token: OperationToken<crate::device_core::NavigatorTag>,
        preview: ReviewedRoute,
    ) -> super::NavigatorOutcome {
        let outcome = super::NavigatorOutcome::ReviewReady { token };
        if !self.navigator.accepts(&outcome) || self.navigator.review.preview.is_some() {
            return super::NavigatorOutcome::Failed { token, error: NavigatorError::SourceChanged };
        }
        self.navigator.review.preview = Some(preview);
        outcome
    }
    pub fn plan_assistant(&mut self, request: crate::activity::NavRequest, context: ReviewContext) {
        self.navigator.request_review(request, context);
        self.ui.map_dirty = true;
    }
    pub fn accept_assistant(&mut self, origin: ReviewOrigin) {
        self.admit_navigator_intent(super::NavigatorIntent::AcceptAssistant {
            origin,
            profile: self.settings().bike_profile_idx,
        });
    }
    pub fn cancel_assistant(&mut self) {
        self.admit_navigator_intent(super::NavigatorIntent::CancelAssistant);
    }
    pub fn resume_assistant(&mut self, origin: ReviewOrigin) {
        self.admit_navigator_intent(super::NavigatorIntent::ResumeAssistant { origin });
    }

    /// Whether the standing checkpoint names an object a pending catalog removal would take. The
    /// store refuses such a removal, and a refused removal is never retried, so the rider's
    /// confirmed delete has to wait for the checkpoint to go rather than be dropped by it.
    pub(crate) fn checkpoint_blocks_removal(&self) -> bool {
        self.assistant_checkpoint().is_some_and(|checkpoint| {
            [Some(checkpoint.route), checkpoint.original]
                .into_iter()
                .flatten()
                .any(|protected| self.catalogs.pending_removes(protected.object))
        })
    }

    /// The active ordinary route that still owes a durable checkpoint, so a restart can offer
    /// Resume for it. An executor answers it once per selection with
    /// [`offer_route_checkpoint`](App::offer_route_checkpoint), because reading the exact payload
    /// identity walks the card's entry table.
    pub fn requested_route_checkpoint(&self) -> Option<crate::CatalogObjectId> {
        let review = &self.navigator.review;
        if !review.owed
            || !review.recovery_seen
            || review.change.is_some()
            || review.token.is_some()
            || !matches!(review.status, ReviewStatus::Idle | ReviewStatus::Accepted)
        {
            return None;
        }
        if self.mode() == crate::activity::Mode::Idle {
            // The Route overview makes a route active to preview it. Guidance is not running, so
            // nothing is being followed and nothing is owed.
            return None;
        }
        let id = self.route_ids().get(self.active_route_index()?).copied()?;
        // A route the rider has just asked to delete is not worth a new checkpoint, and writing one
        // would put the removal back behind another clear.
        if self.catalogs.pending_removes(id) {
            return None;
        }
        (review.checkpoint.is_none_or(|standing| standing.route.object != id)).then_some(id)
    }

    /// Stage the selected route's checkpoint from the exact bytes the executor holds. An ordinary
    /// route has no phase, so its window is the whole route and recovery derives progress from a
    /// fresh fix. A source the executor cannot name leaves the selection without an offer.
    pub fn offer_route_checkpoint(&mut self, id: crate::CatalogObjectId, source: Option<RouteCheckpointSource>) {
        if self.requested_route_checkpoint() != Some(id) {
            return;
        }
        let (Some(source), Some(summary)) = (source, self.active_route_index().and_then(|i| self.routes().get(i)))
        else {
            // Nothing is payable, so nothing stays owed.
            self.navigator.review.owed = false;
            return;
        };
        let next = NavigatorCheckpoint {
            route: source.route,
            original: None,
            progress_m: 0,
            occurrence: 0,
            lon: summary.start_lon,
            lat: summary.start_lat,
            phase: JourneyPhase::Following,
            unresolved_avoidance: source.unresolved_avoidance,
            selection: true,
            lower_m: 0,
            upper_m: source.distance_m,
        };
        if !next.valid() {
            self.navigator.review.owed = false;
            return;
        }
        self.navigator.review.change = Some(Some(next));
        self.navigator.review.after = AfterCheckpoint::Selected;
    }

    /// A rider-requested recovery read. Executors reuse their existing route index for this read.
    pub fn requested_assistant_resume(&self) -> Option<PayloadFingerprint> {
        (self.ui.find.action == crate::find_place::Action::Resume
            && self.assistant_review_status() == ReviewStatus::ResumeAvailable
            && self.active_route_index().is_none())
        .then(|| self.assistant_checkpoint().map(|c| c.route))
        .flatten()
    }

    /// Match only the accepted phase window before naming the existing durable Resume operation.
    pub fn prepare_assistant_resume(&mut self, route: Option<&obc_route::RouteReader>) {
        if self.requested_assistant_resume().is_none() {
            return;
        }
        self.ui.find.action = crate::find_place::Action::None;
        self.ui.map_dirty = true;
        let fix = self.fresh_position();
        if let Some(crate::screen::Screen::Journey(screen)) = self.ui.stack.last_mut() {
            screen.error = fix.is_none().then_some(crate::screen::JourneyError::NoFix);
        }
        let Some(fix) = fix else {
            return;
        };
        let Some(route) = route else {
            if let Some(crate::screen::Screen::Journey(screen)) = self.ui.stack.last_mut() {
                screen.error = Some(crate::screen::JourneyError::SourceChanged);
            }
            return;
        };
        let checkpoint = self.assistant_checkpoint().unwrap();
        let matcher = &mut self.navigator.route_match;
        let Some(matched) = matcher.recover(fix.lon, fix.lat, route, checkpoint.lower_m, checkpoint.upper_m) else {
            if let Some(crate::screen::Screen::Journey(screen)) = self.ui.stack.last_mut() {
                screen.error = Some(crate::screen::JourneyError::Unmatched);
            }
            return;
        };
        let origin = ReviewOrigin {
            fix: (fix.lon, fix.lat),
            progress_m: matched.progress_m,
            occurrence: matcher.occurrence(),
            lateral_m: matched.dist_m,
            trustworthy: !matched.off_route,
        };
        self.resume_assistant(origin);
    }

    pub fn assistant_needs_recovery(&self) -> bool {
        (!self.navigator.review.recovery_seen && self.navigator.review.status == ReviewStatus::Idle)
            || (self.navigator.review.status == ReviewStatus::Unresolved && self.navigator.review.change.is_some())
    }

    /// Keep the newest trustworthy sample while a phase write is pending; the phase owner replays it after ACK.
    pub fn observe_assistant_origin(&mut self, origin: ReviewOrigin) {
        if origin.trustworthy {
            self.navigator.review.latest_origin = Some(origin);
        }
    }
    pub fn latest_assistant_origin(&self) -> Option<ReviewOrigin> {
        self.navigator.review.latest_origin
    }
    pub fn checkpoint_assistant_phase(&mut self, next: NavigatorCheckpoint) -> bool {
        let review = &mut self.navigator.review;
        let Some(current) = review.checkpoint else {
            return false;
        };
        if review.status != ReviewStatus::Accepted
            || review.change.is_some()
            || !next.valid()
            || next.route != current.route
            || (next.original != current.original
                && !(next.phase == JourneyPhase::Following && next.original.is_none()))
            || next.unresolved_avoidance != current.unresolved_avoidance
        {
            return false;
        }
        review.change = Some(Some(next));
        review.after = AfterCheckpoint::Phase;
        true
    }
    /// A recovered preview cannot be accepted when its frozen sources no longer match.
    pub fn invalidate_assistant_preview(&mut self) {
        if self.navigator.review.preview.is_some() && self.navigator.review.change.is_none() {
            self.navigator.review_failed(NavigatorError::SourceChanged);
        }
    }
    pub fn assistant_review_status(&self) -> ReviewStatus {
        self.navigator.review.status
    }
    pub fn assistant_review_context(&self) -> Option<ReviewContext> {
        self.navigator.review.context
    }
    pub fn requested_assistant_restore(&self) -> Option<RouteSourceKey> {
        self.navigator.review.restore
    }
    pub fn restore_visit(
        &mut self,
        target: obc_route::visit::VisitTarget,
        context: ReviewContext,
        source: RouteSourceKey,
    ) -> bool {
        if !self.plan_visit(target, context) {
            return false;
        }
        self.navigator.review.context = Some(context);
        self.navigator.review.restore = Some(source);
        true
    }
    pub fn assistant_preview(&self) -> Option<ReviewedRoute> {
        self.navigator.review.preview
    }
    pub fn assistant_checkpoint(&self) -> Option<NavigatorCheckpoint> {
        self.navigator.review.checkpoint
    }
    pub fn offer_assistant_checkpoint(&mut self, store: StoreIdentity, checkpoint: Option<NavigatorCheckpoint>) {
        if self.navigator.review.status == ReviewStatus::Unresolved {
            if let Ok(after) = self.navigator.recover_checkpoint(store, checkpoint) {
                self.metadata.reset_store();
                self.catalogs.remount_required = false;
                self.catalogs.loaded_scope = None;
                self.catalogs.note_store_moved();
                self.apply_assistant_checkpoint_action(after);
                self.ui.map_dirty = true;
                self.note_resume_save_refusal();
            }
        } else {
            let first = !self.navigator.review.recovery_seen;
            self.navigator.offer_checkpoint(store, checkpoint);
            let offer = first && self.assistant_review_status() == ReviewStatus::ResumeAvailable;
            if first && self.navigator.review.recovery_seen {
                self.catalogs.note_store_moved();
            }
            self.ui.find.resume_offer |= offer;
            self.ui.map_dirty |= offer;
        }
    }
    /// A transient immutable edit, available only to the current MetadataMachine token.
    pub fn assistant_checkpoint_payload(&self, token: OperationToken<MetadataTag>) -> Option<CheckpointChange> {
        (self.navigator.review.token == Some(token)).then_some(())?;
        self.navigator.review.change.map(|next| CheckpointChange { expected: self.navigator.review.checkpoint, next })
    }
    pub fn assistant_store_matches(&self, store: StoreIdentity) -> bool {
        self.navigator.review.store == Some(store) && self.navigator.review.status != ReviewStatus::Unresolved
    }
    /// The executor calls this immediately before physical submission, after admitted cancellations.
    pub fn assistant_checkpoint_submission(&mut self, token: OperationToken<MetadataTag>) -> bool {
        self.navigator.checkpoint_submission(token)
    }
    pub(crate) fn assistant_checkpoint_answer(&mut self, outcome: MetadataOutcome) {
        let before = self.assistant_review_status();
        let after = self.navigator.checkpoint_answer(outcome);
        self.apply_assistant_checkpoint_action(after);
        self.ui.map_dirty |= self.assistant_review_status() != before;
        self.note_resume_save_refusal();
    }
    fn note_resume_save_refusal(&mut self) {
        if self.assistant_review_status() == ReviewStatus::ResumeAvailable {
            if let Some(crate::screen::Screen::Journey(screen)) = self.ui.stack.last_mut() {
                screen.error = Some(crate::screen::JourneyError::SourceChanged);
            }
        }
    }
    fn apply_assistant_checkpoint_action(&mut self, after: Option<AfterCheckpoint>) {
        match after {
            Some(AfterCheckpoint::Activate { route: id, record }) => {
                if let Some(index) = self.route_ids().iter().position(|&candidate| candidate == id) {
                    self.navigator.review.unaccepted &= !(1u64 << index);
                    self.navigator.following.active_route = Some(index);
                    if let Some(checkpoint) = self.navigator.review.checkpoint {
                        self.navigator.request_seam(index, checkpoint.progress_m);
                    }
                    if self.recorder.session().is_none() {
                        // A route is active, so the mode and the camera follow whatever opened it.
                        // Only the ride itself is the accepted plan's, never recovery's.
                        if record {
                            self.recorder.request(crate::RecorderIntent::Start);
                        }
                        self.activity.mode = crate::activity::Mode::Riding;
                        let position = self.state.user_fix.map(|fix| (fix.lon, fix.lat));
                        if let Some((lon, lat)) = position {
                            self.state.enter_riding_view(lon, lat);
                        }
                    }
                    self.ui.map_dirty = true;
                } else {
                    self.navigator.review_failed(NavigatorError::SourceChanged);
                }
            }
            Some(AfterCheckpoint::Restore { route, progress_m }) => {
                if let Some(index) = self.route_ids().iter().position(|&id| id == route) {
                    self.navigator.following.active_route = Some(index);
                    self.navigator.request_seam(index, progress_m);
                    self.ui.map_dirty = true;
                } else {
                    self.navigator.review_failed(NavigatorError::SourceChanged);
                }
            }
            Some(AfterCheckpoint::Select(index)) => {
                self.navigator.select_now(index);
                self.ui.map_dirty = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_core::{core_mode::CoreMode, TokenSource};
    fn source(id: u64) -> PayloadFingerprint {
        PayloadFingerprint { object: id, revision: 1, length: 1000, crc: id as u32 }
    }
    fn context() -> ReviewContext {
        ReviewContext {
            purpose: ReviewPurpose::Destination,
            map: RouteSourceKey { store: [1; 16], object: 2, revision: 3 },
            store: StoreIdentity::from_bytes([1; 16]),
            original: Some(source(4)),
            origin: (8_000_000, 47_000_000),
            progress_m: 100,
            occurrence: 2,
            required_anchors_m: [0; 3],
            profile: 0,
            facts_policy: REVIEW_FACTS_POLICY,
            unresolved_avoidance: false,
        }
    }
    fn origin() -> ReviewOrigin {
        ReviewOrigin { fix: context().origin, progress_m: 100, occurrence: 2, lateral_m: 0, trustworthy: true }
    }
    fn preview() -> NavigatorMachine {
        let mut nav = NavigatorMachine::new();
        nav.following.active_route = Some(0);
        nav.review.store = Some(context().store);
        nav.review.context = Some(context());
        nav.reviewed(ReviewedRoute {
            source: source(5),
            distance_m: 500,
            ascent_m: 10,
            descent_m: 3,
            visit_anchors_m: None,
            visit_costs: None,
        });
        nav.review_index(Some(1));
        nav.route = PlanPhase::PreviewReady;
        nav
    }
    fn issued(nav: &mut NavigatorMachine, tokens: &mut TokenSource<MetadataTag>) -> OperationToken<MetadataTag> {
        let token = tokens.issue();
        nav.checkpoint_issued(token);
        token
    }
    #[test]
    fn accepted_destination_starts_only_a_missing_recording_and_keeps_navigation() {
        use crate::activity::Mode;
        for mode in [Mode::Idle, Mode::Riding, Mode::Paused] {
            for committed in [false, true] {
                let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
                app.test_mount_store();
                if mode != Mode::Idle {
                    app.test_start_ride();
                }
                app.activity.mode = mode;
                let session = app.recorder.session();
                let summary = obc_route::RouteSummary {
                    name: heapless::String::new(),
                    distance_km: 1,
                    climb_m: 0,
                    bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 },
                    start_lon: 0,
                    start_lat: 0,
                };
                app.set_routes_with_ids(&[summary.clone(), summary], &[4, 5]);
                app.navigator = preview();
                if mode == Mode::Idle {
                    app.navigator.following.active_route = None;
                    app.navigator.review.context.as_mut().unwrap().original = None;
                }
                app.navigator.accept_review(origin(), 0);
                let effect = app.metadata.next_checkpoint_effect().unwrap();
                let token = effect.token();
                app.navigator.checkpoint_issued(token);
                assert!(app.navigator.checkpoint_submission(token));
                assert_eq!(app.recorder.session(), session, "preview and saving do not start recording");
                let outcome = if committed {
                    MetadataOutcome::CheckpointWritten { token }
                } else {
                    MetadataOutcome::Failed { token, error: MetadataError::Busy }
                };
                assert!(app.metadata.apply_outcome(outcome));
                app.assistant_checkpoint_answer(outcome);
                app.advance_recorder_session();
                if committed {
                    let recording = app.recorder.session().expect("accepted navigation records a ride");
                    if let Some(session) = session {
                        assert_eq!(recording, session);
                    }
                    assert_eq!(app.active_route_index(), Some(1));
                    assert!(app.navigator.pending_seam(), "session initialization must keep the accepted route seam");
                    assert_eq!(app.mode(), if mode == Mode::Idle { Mode::Riding } else { mode });
                    app.advance_recorder_session();
                    assert_eq!(app.recorder.session(), Some(recording));
                } else {
                    assert_eq!(app.recorder.session(), session);
                    assert_eq!(app.mode(), mode);
                    assert_eq!(app.active_route_index(), if mode == Mode::Idle { None } else { Some(0) });
                }
            }
        }
    }
    #[test]
    fn easier_review_refuses_movement_and_preserves_uncertain_acceptance_until_recovery() {
        use crate::easier::Phase;
        for recovery in [None, Some(false), Some(true)] {
            let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 10.0));
            app.navigator = preview();
            let mut easier = context();
            easier.purpose = ReviewPurpose::Easier(obc_route::nav::Objective::Profile);
            app.navigator.review.context = Some(easier);
            app.easier.context = Some(easier);
            app.easier.phase = Phase::Ready;
            app.easier.review = true;
            assert!(app.ui.stack.push(crate::screen::Screen::Assistant(crate::screen::AssistantScreen::new())).is_ok());
            assert!(app.ui.stack.push(crate::screen::Screen::Easier(crate::screen::EasierScreen::new())).is_ok());
            if recovery.is_some() {
                app.navigator.accept_review(origin(), 0);
                let mut tokens = TokenSource::new();
                let token = issued(&mut app.navigator, &mut tokens);
                assert!(app.navigator.checkpoint_submission(token));
                app.advance_easier();
                assert!(app.easier.phase == Phase::Ready, "Saving must remain visible");
                app.navigator
                    .checkpoint_answer(MetadataOutcome::Failed { token, error: MetadataError::RemountRequired });
            } else {
                let mut moved = origin();
                moved.progress_m += REVIEW_ALONG_TOLERANCE_M + 1;
                app.navigator.accept_review(moved, 0);
                assert_eq!(app.assistant_review_status(), ReviewStatus::Failed(NavigatorError::Movement));
            }
            app.advance_easier();
            assert!(app.easier.phase == if recovery.is_some() { Phase::Ready } else { Phase::Unavailable });
            app.apply_gesture(crate::Gesture::Press);
            assert_ne!(app.assistant_review_status(), ReviewStatus::Saving);
            assert_eq!(app.active_route_index(), Some(0));
            if let Some(committed) = recovery {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Unresolved);
                assert!(app.navigator.checkpoint_change().is_none());
                assert!(!app.navigator.review.cancel_after, "an uncertain write is not a cancel request");
                let checkpoint =
                    if committed { app.navigator.review.change.unwrap() } else { app.navigator.review.checkpoint };
                let after = app.navigator.recover_checkpoint(context().store, checkpoint).unwrap();
                assert!(app.navigator.review.change.is_none(), "recovery must not schedule a clear");
                if committed {
                    assert_eq!(after, Some(AfterCheckpoint::Activate { route: 5, record: true }));
                    assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
                    app.advance_easier();
                    assert!(app.easier.phase == Phase::Idle);
                    assert!(matches!(app.top_screen(), crate::screen::Screen::Map(_)));
                    assert!(!app.ui.stack.iter().any(|s| matches!(s, crate::screen::Screen::Assistant(_))));
                } else {
                    assert_eq!(after, None);
                    assert_eq!(app.assistant_review_status(), ReviewStatus::Preview);
                    assert!(app.assistant_preview().is_some());
                }
            }
        }
    }

    #[test]
    fn easier_failed_entry_preserves_another_review_and_its_uncertain_save() {
        use crate::{input::Chord, screen::Screen, Gesture};
        for unresolved in [false, true] {
            let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 10.0));
            app.navigator = preview();
            app.bind_place_map(Some(context().map));
            if unresolved {
                app.navigator.accept_review(origin(), 0);
                let token = issued(&mut app.navigator, &mut TokenSource::new());
                assert!(app.navigator.checkpoint_submission(token));
                app.navigator
                    .checkpoint_answer(MetadataOutcome::Failed { token, error: MetadataError::RemountRequired });
            }
            let status = app.assistant_review_status();
            let change = app.navigator.review.change;
            assert!(app.apply_chord(Chord::Assistant));
            app.apply_gesture(Gesture::Step(2));
            app.apply_gesture(Gesture::Press);
            assert!(matches!(app.top_screen(), Screen::Assistant(_)));
            app.advance_easier();
            app.apply_gesture(Gesture::Back);
            assert_eq!(app.assistant_review_status(), status);
            assert_eq!(app.navigator.review.change, change);
            assert!(!app.navigator.review.cancel_after);
            assert!(app.assistant_preview().is_some());
        }
    }

    #[test]
    fn changing_cards_cannot_submit_or_recover_an_old_checkpoint() {
        let other = StoreIdentity::from_bytes([9; 16]);
        let mut nav = preview();
        nav.accept_review(origin(), 0);
        nav.review_store_changed(other);
        assert_eq!(nav.review.status, ReviewStatus::Unresolved);
        assert!(nav.checkpoint_change().is_none());
        let mut nav = NavigatorMachine::new();
        nav.offer_checkpoint(context().store, None);
        nav.review_store_changed(other);
        assert!(!nav.review.recovery_seen);
        nav.offer_checkpoint(other, None);
        assert_eq!(nav.review.store, Some(other));
    }

    #[test]
    fn immutable_preview_requires_fresh_origin_and_verified_ack_before_activation() {
        let mut nav = preview();
        assert_eq!(nav.following.active_route, Some(0));
        nav.set_active_route(Some(1));
        assert_eq!(nav.following.active_route, Some(0), "ordinary route selection cannot accept the preview");
        let mut moved = origin();
        moved.progress_m += REVIEW_ALONG_TOLERANCE_M + 1;
        nav.accept_review(moved, 0);
        assert_eq!(nav.review.status, ReviewStatus::Failed(NavigatorError::Movement));
        assert!(nav.review.change.is_none());
        let mut nav = preview();
        nav.accept_review(origin(), 0);
        let mut tokens = TokenSource::new();
        let token = issued(&mut nav, &mut tokens);
        assert!(nav.checkpoint_submission(token));
        assert_eq!(nav.following.active_route, Some(0));
        assert_eq!(
            nav.checkpoint_answer(MetadataOutcome::CheckpointWritten { token }),
            Some(AfterCheckpoint::Activate { route: 5, record: true })
        );
        assert_eq!(nav.review.checkpoint.unwrap().route, source(5));
        assert_eq!(nav.review.checkpoint.unwrap().original, None, "a destination replaces the previous goal");
        assert_eq!(nav.review.status, ReviewStatus::Accepted);
        assert!(nav.checkpoint_answer(MetadataOutcome::CheckpointWritten { token }).is_none());
        assert_eq!(nav.review.preview_index, None);
        assert!(!nav.select_after_checkpoint(None));
        let token = issued(&mut nav, &mut tokens);
        nav.checkpoint_submission(token);
        assert_eq!(
            nav.checkpoint_answer(MetadataOutcome::CheckpointWritten { token }),
            Some(AfterCheckpoint::Select(None))
        );
        assert!(nav.select_after_checkpoint(Some(1)), "accepted route can be selected again after stop");
    }
    #[test]
    fn cancel_before_submission_annihilates_and_cancel_after_submission_waits_for_clear_ack() {
        let mut tokens = TokenSource::new();
        let mut nav = preview();
        nav.accept_review(origin(), 0);
        let token = issued(&mut nav, &mut tokens);
        nav.cancel_review();
        assert!(!nav.checkpoint_submission(token));
        nav.checkpoint_answer(MetadataOutcome::Cancelled { token });
        assert!(nav.review.checkpoint.is_none());
        let mut nav = preview();
        nav.accept_review(origin(), 0);
        let token = issued(&mut nav, &mut tokens);
        assert!(nav.checkpoint_submission(token));
        nav.cancel_review();
        assert!(nav.checkpoint_answer(MetadataOutcome::CheckpointWritten { token }).is_none());
        assert_eq!(nav.review.change.unwrap(), None);
        assert!(nav.review.preview.is_some(), "do not retire a durably accepted candidate before durable clear");
        let token = issued(&mut nav, &mut tokens);
        nav.checkpoint_submission(token);
        nav.checkpoint_answer(MetadataOutcome::CheckpointWritten { token });
        assert!(nav.review.checkpoint.is_none());
        assert_eq!(nav.following.active_route, Some(0));
        assert!(matches!(
            nav.next_effect(&mut CoreMode::new()),
            Some(super::super::NavigatorEffect::Release { retain_result: false, .. })
        ));
    }
    #[test]
    fn unknown_submission_fences_cancel_and_known_failure_keeps_exact_preview_retryable() {
        let mut nav = preview();
        nav.accept_review(origin(), 0);
        let original_preview = nav.review.preview;
        let mut tokens = TokenSource::new();
        let token = issued(&mut nav, &mut tokens);
        nav.checkpoint_submission(token);
        nav.checkpoint_answer(MetadataOutcome::Failed { token, error: MetadataError::Busy });
        assert_eq!(nav.review.preview, original_preview);
        assert_eq!(nav.review.status, ReviewStatus::Preview);
        nav.accept_review(origin(), 0);
        let token = issued(&mut nav, &mut tokens);
        nav.checkpoint_submission(token);
        nav.checkpoint_answer(MetadataOutcome::Failed { token, error: MetadataError::RemountRequired });
        nav.cancel_review();
        nav.set_active_route(None);
        assert_eq!(nav.review.status, ReviewStatus::Unresolved);
        assert_eq!(nav.review.preview, original_preview);
        assert_eq!(nav.following.active_route, Some(0));
        assert!(nav.checkpoint_change().is_none());
    }
    #[test]
    fn same_card_recovery_resolves_old_or_new_head_and_queued_cancel_in_the_live_app() {
        for committed in [false, true] {
            for cancel in [false, true] {
                let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 10.0));
                let summary = obc_route::RouteSummary {
                    name: heapless::String::new(),
                    distance_km: 1,
                    climb_m: 0,
                    bbox: obc_map_scene::BBox { min_lon: 0, min_lat: 0, max_lon: 1, max_lat: 1 },
                    start_lon: 0,
                    start_lat: 0,
                };
                app.set_routes_with_ids(&[summary.clone(), summary], &[4, 5]);
                app.navigator = preview();
                app.navigator.review.recovery_seen = true;
                app.navigator.accept_review(origin(), 0);
                let next = app.navigator.review.change.unwrap();
                let effect = app.metadata.next_checkpoint_effect().unwrap();
                let token = effect.token();
                app.navigator.checkpoint_issued(token);
                app.navigator.checkpoint_submission(token);
                let failed = MetadataOutcome::Failed { token, error: MetadataError::RemountRequired };
                assert!(app.metadata.apply_outcome(failed));
                app.assistant_checkpoint_answer(failed);
                app.catalogs.remount_required = true;
                if cancel {
                    app.navigator.cancel_review();
                }
                assert!(app.assistant_needs_recovery());
                assert!(app.metadata.next_checkpoint_effect().is_none());
                let recovered = if committed { next } else { None };
                app.offer_assistant_checkpoint(context().store, recovered);
                assert!(!app.catalogs.remount_required);
                assert_eq!(app.navigator.review.checkpoint, recovered);
                assert_ne!(app.assistant_review_status(), ReviewStatus::Unresolved);
                let next_effect =
                    app.metadata.next_checkpoint_effect().expect("same owner permits work after verified recovery");
                if cancel && committed {
                    assert_eq!(app.navigator.review.change, Some(None));
                    assert!(app.assistant_preview().is_some(), "clear is still owed before retirement");
                    app.navigator.checkpoint_issued(next_effect.token());
                    app.navigator.checkpoint_submission(next_effect.token());
                    let outcome = MetadataOutcome::CheckpointWritten { token: next_effect.token() };
                    assert!(app.metadata.apply_outcome(outcome));
                    app.assistant_checkpoint_answer(outcome);
                    assert!(app.assistant_checkpoint().is_none());
                    assert!(app.assistant_preview().is_none());
                    assert_eq!(app.active_route_index(), Some(0));
                } else if cancel {
                    assert!(app.assistant_preview().is_none());
                    assert_eq!(app.assistant_review_status(), ReviewStatus::Idle);
                } else if !committed {
                    assert_eq!(app.assistant_review_status(), ReviewStatus::Preview);
                } else {
                    assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
                    assert_eq!(app.active_route_index(), Some(1));
                }
            }
        }
    }

    /// An imported route bytes, plus the exact fingerprint an executor would report for it.
    fn ordinary_route() -> (std::vec::Vec<u8>, PayloadFingerprint) {
        use obc_formats::io::{ByteSink, SliceSource};
        #[derive(Default)]
        struct Sink(std::vec::Vec<u8>);
        impl ByteSink for Sink {
            fn write(&mut self, b: &[u8]) -> Result<(), obc_formats::io::Error> {
                self.0.extend_from_slice(b);
                Ok(())
            }
            fn patch_at(&mut self, at: u32, b: &[u8]) -> Result<(), obc_formats::io::Error> {
                self.0[at as usize..at as usize + b.len()].copy_from_slice(b);
                Ok(())
            }
        }
        let gpx = br#"<gpx><trk><trkseg><trkpt lon="0" lat="0"/><trkpt lon="0" lat="0.02"/><trkpt lon="0.02" lat="0.02"/></trkseg></trk></gpx>"#;
        let mut sink = Sink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx), "Imported", &mut sink).unwrap();
        let source = PayloadFingerprint { object: 7, revision: 3, length: sink.0.len() as u64, crc: 0x1234 };
        (sink.0, source)
    }
    fn mounted(summary: &obc_route::RouteSummary, id: u64) -> crate::App {
        let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        app.test_mount_store();
        app.set_routes_with_ids(core::slice::from_ref(summary), &[id]);
        app
    }
    /// A mounted app with guidance running, which is what owes a checkpoint.
    fn riding(summary: &obc_route::RouteSummary, id: u64) -> crate::App {
        let mut app = mounted(summary, id);
        app.activity.mode = crate::activity::Mode::Riding;
        app
    }
    fn ordinary_store() -> StoreIdentity {
        StoreIdentity::new(1)
    }
    fn ack_checkpoint(app: &mut crate::App) {
        let token = app.metadata.next_checkpoint_effect().expect("one bounded metadata operation").token();
        app.navigator.checkpoint_issued(token);
        assert!(app.assistant_checkpoint_submission(token));
        let outcome = MetadataOutcome::CheckpointWritten { token };
        assert!(app.metadata.apply_outcome(outcome));
        app.assistant_checkpoint_answer(outcome);
    }
    fn fix_at(app: &mut crate::App, lon: i32, lat: i32) {
        struct Location(Option<obc_ports::Fix>);
        impl obc_ports::LocationSource for Location {
            fn poll(&mut self) -> Option<obc_ports::Fix> {
                self.0.take()
            }
        }
        app.tick(
            obc_ports::RideClock(0),
            obc_ports::Sensors::new(&mut Location(Some(obc_ports::Fix::at(lat, lon)))),
            None,
        );
    }

    /// Only a route the rider is following owes a checkpoint. A preview from the Route overview
    /// runs no guidance, so it owes nothing and stays deletable. An unnamed source leaves no offer,
    /// and a refused write stays owed rather than leaving the previous route on the card.
    #[test]
    fn a_preview_owes_nothing_and_a_refused_selection_write_stays_owed() {
        use obc_formats::io::SliceSource;
        let (bytes, source) = ordinary_route();
        let held = SliceSource(&bytes);
        let index = obc_route::RouteIndex::read(&held).unwrap();
        let route = obc_route::RouteReader::new(&index, &held);
        let facts =
            RouteCheckpointSource { route: source, distance_m: route.total_distance_m, unresolved_avoidance: false };

        // A preview: the Route overview makes the route active from Idle to show it.
        let mut app = mounted(&route.summary(), source.object);
        app.offer_assistant_checkpoint(ordinary_store(), None);
        app.activate_route(0);
        assert_eq!(app.active_route_index(), Some(0));
        assert!(app.requested_route_checkpoint().is_none(), "a preview is not a route being followed");
        assert!(app.navigator.checkpoint_change().is_none());

        // Guidance running: the same selection now owes exactly one checkpoint.
        for named in [false, true] {
            let mut app = riding(&route.summary(), source.object);
            app.activate_route(0);
            assert!(app.requested_route_checkpoint().is_none(), "the card's own checkpoint is not read yet");
            app.offer_assistant_checkpoint(ordinary_store(), None);
            assert_eq!(app.requested_route_checkpoint(), Some(source.object));
            app.offer_route_checkpoint(source.object, named.then_some(facts));
            if !named {
                assert!(app.navigator.checkpoint_change().is_none(), "nothing is owed to the card");
                assert!(app.requested_route_checkpoint().is_none(), "and an unpayable debt is not re-asked");
                assert_eq!(app.active_route_index(), Some(0), "a refused offer keeps the selection");
                continue;
            }
            assert!(app.requested_route_checkpoint().is_none(), "one write at a time");

            // A refused write keeps the debt, so the card never names a route nobody follows.
            let token = app.metadata.next_checkpoint_effect().unwrap().token();
            app.navigator.checkpoint_issued(token);
            assert!(app.assistant_checkpoint_submission(token));
            let failed = MetadataOutcome::Failed { token, error: MetadataError::Busy };
            assert!(app.metadata.apply_outcome(failed));
            app.assistant_checkpoint_answer(failed);
            assert!(app.assistant_checkpoint().is_none());
            assert_eq!(app.requested_route_checkpoint(), Some(source.object), "the debt survives a refusal");
            app.offer_route_checkpoint(source.object, Some(facts));
            ack_checkpoint(&mut app);

            let checkpoint = app.assistant_checkpoint().expect("the selection is durable");
            assert_eq!(checkpoint.route, source);
            assert_eq!((checkpoint.original, checkpoint.phase), (None, JourneyPhase::Following));
            assert_eq!((checkpoint.lower_m, checkpoint.upper_m), (0, route.total_distance_m));
            assert!(!app.recording(), "selecting a route is not starting a ride");
            assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
            assert!(app.requested_route_checkpoint().is_none(), "a confirmed write settles the debt");
            // A selection records no progress, so a fresh ride is not dragged back to the start.
            app.test_start_ride();
            assert!(!app.navigator.pending_seam(), "a plain selection is not a measured anchor");
            // Stopping guidance clears it again, so a restart offers nothing.
            app.activate_route(usize::MAX);
            ack_checkpoint(&mut app);
            assert!(app.assistant_checkpoint().is_none());
            assert_eq!(app.active_route_index(), None);
        }
    }

    /// The restart offer for an ordinary route: an explicit press, a fresh position on the saved
    /// bytes, and no recording. Declining leaves guidance inactive and the checkpoint standing.
    #[test]
    fn an_ordinary_route_resumes_only_on_an_explicit_press_and_never_starts_a_recording() {
        use crate::{screen::Screen, Gesture};
        use obc_formats::io::SliceSource;
        let (bytes, source) = ordinary_route();
        let held = SliceSource(&bytes);
        let index = obc_route::RouteIndex::read(&held).unwrap();
        let route = obc_route::RouteReader::new(&index, &held);
        let checkpoint = NavigatorCheckpoint {
            route: source,
            original: None,
            progress_m: 0,
            occurrence: 0,
            lon: 0,
            lat: 0,
            phase: JourneyPhase::Following,
            unresolved_avoidance: false,
            selection: false,
            lower_m: 0,
            upper_m: route.total_distance_m,
        };
        for accept in [false, true] {
            let mut app = mounted(&route.summary(), source.object);
            app.offer_assistant_checkpoint(ordinary_store(), Some(checkpoint));
            assert_eq!(app.assistant_review_status(), ReviewStatus::ResumeAvailable);
            assert!(app.active_route_index().is_none(), "a restart never restores guidance by itself");
            app.advance_animations(obc_ports::InputClock(0));
            app.prepare_find(None, None);
            assert!(matches!(app.top_screen(), Screen::Journey(s) if s.resume));
            if !accept {
                app.apply_gesture(Gesture::Back);
                app.prepare_find(None, None);
                assert!(app.active_route_index().is_none(), "declining leaves guidance inactive");
                assert_eq!(app.assistant_checkpoint(), Some(checkpoint), "and keeps the saved route");
                continue;
            }
            fix_at(&mut app, 0, 10_000);
            app.apply_gesture(Gesture::Press);
            assert_eq!(app.requested_assistant_resume(), Some(source));
            app.prepare_assistant_resume(None);
            assert!(
                matches!(app.top_screen(), Screen::Journey(s) if s.error == Some(crate::screen::JourneyError::SourceChanged))
            );
            assert!(app.navigator.review.change.is_none(), "a source the card cannot produce restores nothing");
            app.apply_gesture(Gesture::Press);
            app.prepare_assistant_resume(Some(&route));
            assert_eq!(app.assistant_review_status(), ReviewStatus::Saving);
            ack_checkpoint(&mut app);
            assert_eq!(app.active_route_index(), Some(0), "the exact saved route is what guidance returns to");
            assert_eq!(app.assistant_checkpoint().map(|c| c.route), Some(source));
            assert!(app.navigator.pending_seam(), "guidance rejoins at the recovered progress");
            assert!(!app.recording(), "navigation recovery is not recording recovery");
            assert_eq!(app.mode(), crate::activity::Mode::Riding, "an active route is never Idle");
            assert_eq!(
                app.state.mode,
                crate::app::CameraMode::Follow,
                "and recovery returns to the riding view, not browse"
            );
        }
    }

    #[test]
    fn reboot_only_offers_resume_and_requires_same_phase_occurrence() {
        let checkpoint = NavigatorCheckpoint {
            route: source(5),
            original: Some(source(4)),
            progress_m: 100,
            occurrence: 2,
            lon: 8_000_000,
            lat: 47_000_000,
            phase: JourneyPhase::Outbound,
            unresolved_avoidance: false,
            selection: false,
            lower_m: 0,
            upper_m: 500,
        };
        let mut nav = NavigatorMachine::new();
        nav.offer_checkpoint(context().store, Some(checkpoint));
        assert!(nav.following.active_route.is_none());
        let mut wrong = origin();
        wrong.progress_m = checkpoint.upper_m + 1;
        nav.resume_review(wrong);
        assert!(nav.review.change.is_none());
        let mut nav = NavigatorMachine::new();
        nav.offer_checkpoint(context().store, Some(checkpoint));
        nav.resume_review(origin());
        assert_eq!(nav.review.change, Some(Some(checkpoint)));
        assert!(nav.following.active_route.is_none());
    }
}
