//! Visit requests and phase progress stay in Navigator. Geometry is accepted once.
use super::{review::AfterCheckpoint, NavigatorMachine, ReviewContext, ReviewPurpose, ReviewStatus};
use obc_formats::obcm::PoiMetadata;
use obc_formats::retention::JourneyPhase;
use obc_route::visit::VisitTarget;
use obc_route::RouteReader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisitUnavailable {
    NoFix,
    NoMappedAccess,
    Profile,
    SourceChanged,
    Busy,
    Avoidance,
    Unmatched,
}

const DEPARTURE_M: u32 = 20;
const ARRIVAL_M: u32 = 10;
const ARRIVAL_LATERAL_M: u32 = 5;
const LEAVE_STOP_M: u32 = 15;

pub(super) struct VisitState {
    target: Option<(PoiMetadata, (i32, i32))>,
    latest_fix: Option<(i32, i32)>,
    phase: Option<JourneyPhase>,
    departed: bool,
    arrival: bool,
    catalog_revision: u64,
    requested_route: u64,
    needs_bind: bool,
}
impl VisitState {
    pub const fn new() -> Self {
        Self {
            target: None,
            latest_fix: None,
            phase: None,
            departed: false,
            arrival: false,
            catalog_revision: 0,
            requested_route: 0,
            needs_bind: false,
        }
    }
}

impl NavigatorMachine {
    pub(crate) fn active_visit(&self) -> bool {
        self.review.checkpoint.is_some_and(|c| c.phase != JourneyPhase::Following)
    }
    /// Reconcile the newest raw fix when a durable phase acknowledgement changes the matching
    /// window. This does not feed Recorder, and is a no-op during unchanged dwell frames.
    pub(crate) fn reconcile_visit(&mut self, route: &RouteReader) {
        let Some(c) = self.review.checkpoint else {
            self.visit.phase = None;
            self.visit.arrival = false;
            return;
        };
        if self.review.status != ReviewStatus::Accepted || self.visit.phase == Some(c.phase) {
            return;
        }
        let recovering = self.visit.phase.is_none() && self.visit.target.is_none();
        self.visit.phase = Some(c.phase);
        self.visit.target = None;
        if c.phase == JourneyPhase::Following {
            self.visit.arrival = false;
            return;
        }
        if c.phase == JourneyPhase::AtStop && !recovering {
            self.visit.arrival = true;
        }
        if c.phase != JourneyPhase::Outbound {
            self.route_match.set_progress_floor(route, c.lower_m);
        }
        if let Some((lon, lat)) = self.visit.latest_fix {
            let matched = self.route_match.update_to(lon, lat, route, c.upper_m);
            self.following.apply_match(matched);
            self.advance_visit((lon, lat), route);
        }
    }
    pub(crate) fn visit_ceiling(&self) -> Option<u32> {
        self.review.checkpoint.filter(|c| c.phase != JourneyPhase::Following).map(|c| c.upper_m)
    }
    pub(crate) fn advance_visit(&mut self, fix: (i32, i32), route: &RouteReader) {
        self.visit.latest_fix = Some(fix);
        let Some(current) = self.review.checkpoint else {
            return;
        };
        if self.review.status != ReviewStatus::Accepted || current.phase == JourneyPhase::Following {
            return;
        }
        let Ok(Some(descriptor)) = route.visit_descriptor() else {
            return;
        };
        let [entry, stop, rejoin] = descriptor.accepted_anchors_m;
        let progress = self.following.progress_m;
        let trustworthy = !self.following.off_route && self.following.dist_to_route_m <= ARRIVAL_LATERAL_M;
        if !trustworthy {
            return;
        }
        let at_stop = obc_map_scene::ground_dist_m(fix, (descriptor.target_lon, descriptor.target_lat));
        if current.phase == JourneyPhase::Outbound
            && progress >= entry.saturating_add(DEPARTURE_M)
            && at_stop > ARRIVAL_M as f32 * 2.0
        {
            self.visit.departed = true;
        }
        self.review.latest_origin = Some(super::ReviewOrigin {
            fix,
            progress_m: progress,
            occurrence: current.occurrence,
            lateral_m: self.following.dist_to_route_m,
            trustworthy: true,
        });
        if self.review.change.is_some() {
            return;
        }
        let mut next = current;
        match current.phase {
            JourneyPhase::Outbound
                if self.visit.departed && progress >= stop.saturating_sub(ARRIVAL_M) && at_stop <= ARRIVAL_M as f32 =>
            {
                next.phase = JourneyPhase::AtStop;
                next.lower_m = stop;
                next.upper_m = rejoin;
                next.progress_m = stop;
            }
            JourneyPhase::AtStop if progress >= stop.saturating_add(LEAVE_STOP_M) && at_stop >= LEAVE_STOP_M as f32 => {
                next.phase = JourneyPhase::Returning;
                next.progress_m = progress;
            }
            JourneyPhase::Returning if progress >= rejoin.saturating_sub(ARRIVAL_M) => {
                let Some(at) = route.position_at(rejoin) else {
                    return;
                };
                if obc_map_scene::ground_dist_m(fix, (at.lon, at.lat)) > ARRIVAL_M as f32 {
                    return;
                }
                next.phase = JourneyPhase::Following;
                next.original = None;
                next.lower_m = rejoin;
                next.upper_m = route.total_distance_m;
                next.progress_m = rejoin;
            }
            _ => return,
        }
        next.lon = fix.0;
        next.lat = fix.1;
        if next.valid() {
            self.review.change = Some(Some(next));
            self.review.after = AfterCheckpoint::Phase;
        }
    }
}

impl crate::App {
    /// UI entry. Capture the live origin and catalog epoch here; Acquire binds the original
    /// fingerprint from that exact epoch before any planner runs.
    pub fn request_visit(&mut self, target: VisitTarget, name: &str) -> Result<(), VisitUnavailable> {
        if self.active_visit()
            || matches!(
                self.assistant_review_status(),
                ReviewStatus::Planning | ReviewStatus::Saving | ReviewStatus::Preview | ReviewStatus::Unresolved
            )
            || self.navigator.live.is_some()
        {
            return Err(VisitUnavailable::Busy);
        }
        let scope = self.catalogs.loaded_scope.ok_or(VisitUnavailable::SourceChanged)?;
        if scope.store.bytes() != target.map.store {
            return Err(VisitUnavailable::SourceChanged);
        }
        let fix = self.fresh_position().ok_or(VisitUnavailable::NoFix)?;
        let a = target.metadata.approach.ok_or(VisitUnavailable::NoMappedAccess)?;
        if !a.source.is_valid() || !target.metadata.source.is_valid() {
            return Err(VisitUnavailable::NoMappedAccess);
        }
        let profile = self.settings().bike_profile_idx;
        let approach = target.approach(target.map, profile).ok_or(VisitUnavailable::Profile)?;
        let original = self.active_route_index().and_then(|i| self.route_ids().get(i)).copied();
        if original.is_some() && (!self.navigator.route_match.started() || self.navigator.following.off_route) {
            return Err(VisitUnavailable::Unmatched);
        }
        let progress = self.navigator.following.progress_m;
        let context = ReviewContext {
            purpose: if original.is_some() { ReviewPurpose::Visit } else { ReviewPurpose::Destination },
            map: target.map,
            store: scope.store,
            original: None,
            origin: (fix.lon, fix.lat),
            progress_m: progress,
            occurrence: self.navigator.route_match.occurrence(),
            required_anchors_m: [progress; 3],
            profile,
            facts_policy: super::REVIEW_FACTS_POLICY,
            unresolved_avoidance: false,
        };
        self.navigator.visit = VisitState::new();
        self.navigator.visit.target = Some((target.metadata, target.display));
        self.navigator.visit.catalog_revision = scope.revision.raw();
        self.navigator.visit.requested_route = original.unwrap_or(0);
        self.navigator.visit.needs_bind = true;
        self.plan_assistant(crate::NavRequest::new(context.origin, approach, name), context);
        if self.assistant_review_status() == ReviewStatus::Planning {
            Ok(())
        } else {
            Err(VisitUnavailable::Busy)
        }
    }
    /// Source binding is accepted only under the epoch captured at the UI request.
    pub fn bind_visit_sources(
        &mut self,
        scope: crate::device_core::StoreRevision,
        original: Option<obc_formats::retention::PayloadFingerprint>,
        avoidance: bool,
    ) -> bool {
        if !self.navigator.visit.needs_bind {
            return true;
        }
        let Some(context) = self.navigator.review.context.as_mut() else {
            return false;
        };
        if self.navigator.review.status != ReviewStatus::Planning
            || scope.store != context.store
            || scope.revision.raw() != self.navigator.visit.catalog_revision
            || original.map_or(0, |p| p.object) != self.navigator.visit.requested_route
            || avoidance
        {
            return false;
        }
        context.original = original;
        self.navigator.visit.needs_bind = false;
        true
    }
    /// Place queries supply a map-bound explicit approach; missing access remains information-only.
    /// No active route means a direct destination, with no implied continuation.
    pub fn plan_visit(&mut self, target: VisitTarget, mut context: ReviewContext) -> bool {
        if self.navigator.active_visit()
            || self.navigator.review.status == ReviewStatus::Unresolved
            || self.navigator.review.status == ReviewStatus::Planning
            || self.navigator.live.is_some()
            || self.navigator.review.preview.is_some()
            || self.navigator.review.change.is_some()
        {
            return false;
        }
        let Some(approach) = target.approach(context.map, context.profile) else {
            return false;
        };
        context.purpose = if context.original.is_some() { ReviewPurpose::Visit } else { ReviewPurpose::Destination };
        context.required_anchors_m = [context.progress_m; 3];
        self.navigator.visit = VisitState::new();
        self.navigator.visit.target = Some((target.metadata, target.display));
        self.plan_assistant(crate::NavRequest::new(context.origin, approach, "Visit"), context);
        self.assistant_review_status() == ReviewStatus::Planning
    }
    pub fn assistant_visit_target(&self) -> Option<VisitTarget> {
        let (metadata, display) = self.navigator.visit.target?;
        Some(VisitTarget { map: self.navigator.review.context?.map, metadata, display })
    }
    /// The executor calls this under the final step token after choosing complete immutable bytes.
    /// Source stamps stay frozen; only the selected variant's original rejoin anchor is resolved.
    pub fn assistant_visit_variant(
        &mut self,
        token: crate::device_core::OperationToken<crate::device_core::NavigatorTag>,
        anchors: [u32; 3],
    ) -> bool {
        if !self
            .navigator
            .accepts(&super::NavigatorOutcome::Stepped { token, progress: super::PlannerProgress::Reached })
        {
            return false;
        }
        let Some(context) = self.navigator.review.context.as_mut() else {
            return false;
        };
        if context.purpose != ReviewPurpose::Visit
            || anchors[0] != context.progress_m
            || anchors[1] != context.progress_m
            || anchors[2] < context.progress_m
        {
            return false;
        }
        context.required_anchors_m = anchors;
        true
    }
    pub fn active_visit(&self) -> bool {
        self.navigator.active_visit()
    }
    /// Arrival is informational. Dismissing it has no effect on route guidance or recording.
    pub fn visit_arrival_pending(&self) -> bool {
        self.navigator.visit.arrival
    }
    pub fn dismiss_visit_arrival(&mut self) {
        self.navigator.visit.arrival = false;
    }
}
