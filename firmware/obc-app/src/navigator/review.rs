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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AfterCheckpoint {
    Activate(u64),
    Select(Option<usize>),
    Restore { route: u64, progress_m: u32 },
    Phase,
}

pub(super) struct ReviewState {
    pub store: Option<StoreIdentity>,
    pub context: Option<ReviewContext>,
    pub preview: Option<ReviewedRoute>,
    pub preview_index: Option<usize>,
    pub unaccepted: u64,
    pub checkpoint: Option<NavigatorCheckpoint>,
    pub change: Option<Option<NavigatorCheckpoint>>,
    pub token: Option<OperationToken<MetadataTag>>,
    pub submitted: bool,
    pub cancel_after: bool,
    pub status: ReviewStatus,
    pub after: AfterCheckpoint,
    pub recovery_seen: bool,
    pub latest_origin: Option<ReviewOrigin>,
}
impl ReviewState {
    pub const fn new() -> Self {
        Self {
            store: None,
            context: None,
            preview: None,
            preview_index: None,
            unaccepted: 0,
            checkpoint: None,
            change: None,
            token: None,
            submitted: false,
            cancel_after: false,
            status: ReviewStatus::Idle,
            after: AfterCheckpoint::Phase,
            recovery_seen: false,
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
    pub(crate) fn set_unaccepted_routes(&mut self, mask: u64) {
        self.review.unaccepted = mask;
    }
    pub(crate) fn review_index(&mut self, index: Option<usize>) {
        self.review.preview_index = index;
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
            self.review.change = Some(None);
            self.review.after = AfterCheckpoint::Select(index);
            return false;
        }
        true
    }
    pub(crate) fn remap_review_keys(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        let mut mask = 0;
        for old in 0..64 {
            if self.route_unaccepted(old) {
                if let Some(new) = remap(old).filter(|&index| index < 64) {
                    mask |= 1 << new;
                }
            }
        }
        self.review.unaccepted = mask;
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
        let (progress_m, upper_m) = if context.purpose == ReviewPurpose::Visit {
            let Some([entry, stop, rejoin]) = preview.visit_anchors_m else {
                self.review.status = ReviewStatus::Failed(NavigatorError::Unavailable);
                return;
            };
            if entry > stop || stop > rejoin || rejoin > preview.distance_m {
                self.review.status = ReviewStatus::Failed(NavigatorError::Unavailable);
                return;
            }
            (entry, stop)
        } else {
            (0, preview.distance_m)
        };
        // The measured candidate axis is authoritative for its accepted phase.
        let next = NavigatorCheckpoint {
            route: preview.source,
            original: if context.purpose == ReviewPurpose::ReturnToRoute { None } else { context.original },
            progress_m,
            occurrence: 0,
            lon: context.origin.0,
            lat: context.origin.1,
            phase: if context.purpose == ReviewPurpose::Visit {
                JourneyPhase::Outbound
            } else {
                JourneyPhase::Following
            },
            unresolved_avoidance: context.unresolved_avoidance,
            lower_m: progress_m,
            upper_m,
        };
        if !next.valid() {
            self.review.status = ReviewStatus::Failed(NavigatorError::Unavailable);
            return;
        }
        self.review.change = Some(Some(next));
        self.review.after = AfterCheckpoint::Activate(preview.source.object);
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
            self.review.status = ReviewStatus::ResumeAvailable;
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
            || origin.occurrence != checkpoint.occurrence
            || origin.progress_m < checkpoint.lower_m
            || origin.progress_m > checkpoint.upper_m
            || origin.progress_m.abs_diff(checkpoint.progress_m) > REVIEW_ALONG_TOLERANCE_M
            || origin.lateral_m > REVIEW_LATERAL_TOLERANCE_M
        {
            self.review.status = ReviewStatus::Failed(NavigatorError::Movement);
            return;
        }
        // Even explicit Resume rechecks the exact payloads through the same serialized operation.
        self.review.change = Some(Some(checkpoint));
        self.review.after = AfterCheckpoint::Activate(checkpoint.route.object);
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
        self.review.preview = None;
        self.review.preview_index = None;
        self.review.context = None;
        self.route = PlanPhase::Active;
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
        self.review.change = None;
        self.review.status = if self.review.preview.is_some() {
            ReviewStatus::Preview
        } else {
            ReviewStatus::Failed(NavigatorError::Store)
        };
        if core::mem::take(&mut self.review.cancel_after) {
            self.cancel_review();
        }
        Ok(None)
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
                self.review.change = None;
                self.review.status = if self.review.preview.is_some() {
                    ReviewStatus::Preview
                } else {
                    ReviewStatus::Failed(NavigatorError::Store)
                };
                if core::mem::take(&mut self.review.cancel_after) {
                    self.cancel_review();
                }
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
            }
        } else {
            self.navigator.offer_checkpoint(store, checkpoint);
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
        let after = self.navigator.checkpoint_answer(outcome);
        self.apply_assistant_checkpoint_action(after);
    }
    fn apply_assistant_checkpoint_action(&mut self, after: Option<AfterCheckpoint>) {
        match after {
            Some(AfterCheckpoint::Activate(id)) => {
                if let Some(index) = self.route_ids().iter().position(|&candidate| candidate == id) {
                    self.navigator.review.unaccepted &= !(1u64 << index);
                    self.navigator.following.active_route = Some(index);
                    if let Some(checkpoint) = self.navigator.review.checkpoint {
                        self.navigator.request_seam(index, checkpoint.progress_m);
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
                self.navigator.following.active_route = index;
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
            Some(AfterCheckpoint::Activate(5))
        );
        assert_eq!(nav.review.checkpoint.unwrap().route, source(5));
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
            lower_m: 0,
            upper_m: 500,
        };
        let mut nav = NavigatorMachine::new();
        nav.offer_checkpoint(context().store, Some(checkpoint));
        assert!(nav.following.active_route.is_none());
        let mut wrong = origin();
        wrong.occurrence += 1;
        nav.resume_review(wrong);
        assert!(nav.review.change.is_none());
        let mut nav = NavigatorMachine::new();
        nav.offer_checkpoint(context().store, Some(checkpoint));
        nav.resume_review(origin());
        assert_eq!(nav.review.change, Some(Some(checkpoint)));
        assert!(nav.following.active_route.is_none());
    }
}
