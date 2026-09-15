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
    #[cfg(test)]
    pub(super) fn assert_boot_state(&self) {
        assert!(self.target.is_none() && self.latest_fix.is_none() && self.phase.is_none());
        assert!(!self.departed && !self.arrival && !self.needs_bind);
        assert_eq!((self.catalog_revision, self.requested_route), (0, 0));
    }
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
    /// Current eligibility from Navigator's matcher and a fresh position, for frozen reviews.
    /// Stage geometry from the exact published candidate after constructing its ReviewReady
    /// outcome. The buffer is shared with ordinary route overviews and remains hidden until ACK.
    pub fn set_assistant_preview_shape(
        &mut self,
        token: crate::device_core::OperationToken<crate::device_core::NavigatorTag>,
        source: obc_formats::retention::PayloadFingerprint,
        points: &[(i32, i32)],
    ) -> bool {
        if !self.navigator.accepts(&super::NavigatorOutcome::ReviewReady { token })
            || self.assistant_preview().map(|preview| preview.source) != Some(source)
        {
            return false;
        }
        let Some(index) = self.route_ids().iter().position(|id| *id == source.object) else { return false };
        let Some(key) = self.catalogs.nav_preview_key(Some(index)) else { return false };
        self.catalogs.accept_nav_preview(Some(key), crate::device_core::DerivedInput::filled(key), points)
    }

    pub fn assistant_preview_shape(&self) -> &[(i32, i32)] {
        if !matches!(
            self.assistant_review_status(),
            ReviewStatus::Preview | ReviewStatus::Saving | ReviewStatus::Accepted
        ) {
            return &[];
        }
        let index =
            self.assistant_preview().and_then(|p| self.route_ids().iter().position(|id| *id == p.source.object));
        self.catalogs.nav_preview_for(self.catalogs.nav_preview_key(index))
    }

    pub fn current_review_origin(&self) -> Option<super::ReviewOrigin> {
        let fix = self.fresh_position()?;
        let active = self.active_route_index().is_some();
        Some(super::ReviewOrigin {
            fix: (fix.lon, fix.lat),
            progress_m: if active { self.navigator.following.progress_m } else { 0 },
            occurrence: if active { self.navigator.route_match.occurrence() } else { 0 },
            lateral_m: if active { self.navigator.following.dist_to_route_m } else { 0 },
            trustworthy: !active || (self.navigator.route_match.started() && !self.navigator.following.off_route),
        })
    }
    /// True only after the physical planner release acknowledgement.
    pub fn assistant_planner_released(&self) -> bool {
        self.navigator.live.is_none()
    }
    pub fn assistant_visit_costs(&self) -> Option<obc_route::visit::VisitCosts> {
        self.assistant_preview()?.visit_costs
    }
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
    /// Cancel at the original departure, or prepare a real connector to the accepted original
    /// tail for explicit review. `route` is the currently active accepted route supplied by the host.
    pub fn cancel_visit(
        &mut self,
        route: &RouteReader,
        map: obc_formats::obcr::RouteSourceKey,
    ) -> Result<(), VisitUnavailable> {
        let current = self.navigator.review.checkpoint.ok_or(VisitUnavailable::Busy)?;
        if !self.active_visit()
            || self.navigator.review.status != ReviewStatus::Accepted
            || self.navigator.review.change.is_some()
            || self.navigator.live.is_some()
        {
            return Err(VisitUnavailable::Busy);
        }
        let scope = self.catalogs.loaded_scope.ok_or(VisitUnavailable::SourceChanged)?;
        if scope.store.bytes() != map.store
            || self.active_route_index().and_then(|i| self.route_ids().get(i)).copied() != Some(current.route.object)
        {
            return Err(VisitUnavailable::SourceChanged);
        }
        let descriptor = route
            .visit_descriptor()
            .map_err(|_| VisitUnavailable::SourceChanged)?
            .ok_or(VisitUnavailable::SourceChanged)?;
        let original = current.original.ok_or(VisitUnavailable::SourceChanged)?;
        if descriptor.original.object != original.object
            || descriptor.original.revision != original.revision
            || descriptor.original.store != scope.store.bytes()
        {
            return Err(VisitUnavailable::SourceChanged);
        }
        let fix = self.fresh_position().ok_or(VisitUnavailable::NoFix)?;
        let origin = (fix.lon, fix.lat);
        let entry = route.position_at(descriptor.accepted_anchors_m[0]).ok_or(VisitUnavailable::SourceChanged)?;
        if current.phase == JourneyPhase::Outbound
            && !self.navigator.visit.departed
            && self.navigator.following.progress_m <= descriptor.accepted_anchors_m[0].saturating_add(ARRIVAL_LATERAL_M)
            && obc_map_scene::ground_dist_m(origin, (entry.lon, entry.lat)) <= ARRIVAL_LATERAL_M as f32
        {
            self.navigator.review.change = Some(None);
            self.navigator.review.after =
                AfterCheckpoint::Restore { route: original.object, progress_m: descriptor.original_anchors_m[0] };
            self.navigator.review.status = ReviewStatus::Saving;
        } else {
            let rejoin = descriptor.accepted_anchors_m[2];
            let at = route.position_at(rejoin).ok_or(VisitUnavailable::SourceChanged)?;
            let context = ReviewContext {
                purpose: ReviewPurpose::ReturnToRoute,
                map,
                store: scope.store,
                original: Some(current.route),
                origin,
                progress_m: self.navigator.following.progress_m,
                occurrence: self.navigator.route_match.occurrence(),
                required_anchors_m: [rejoin; 3],
                profile: self.settings().bike_profile_idx,
                facts_policy: super::REVIEW_FACTS_POLICY,
                unresolved_avoidance: false,
            };
            self.plan_assistant(crate::NavRequest::new(origin, (at.lon, at.lat), "Return to route"), context);
        }
        self.ui.map_dirty = true;
        Ok(())
    }
    /// Arrival is informational. Dismissing it has no effect on route guidance or recording.
    pub fn visit_arrival_pending(&self) -> bool {
        self.navigator.visit.arrival
    }
    pub fn dismiss_visit_arrival(&mut self) {
        self.navigator.visit.arrival = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_core::{RetentionTag, TokenSource};
    use crate::retention::RetentionOutcome;
    use obc_formats::io::{ByteSink, Error, SliceSource};
    use obc_formats::obcr::{RouteSourceKey, VisitDescriptor};
    use obc_formats::retention::{NavigatorCheckpoint, PayloadFingerprint};

    #[derive(Default)]
    struct Sink(std::vec::Vec<u8>);
    impl ByteSink for Sink {
        fn write(&mut self, b: &[u8]) -> Result<(), Error> {
            self.0.extend_from_slice(b);
            Ok(())
        }
        fn patch_at(&mut self, at: u32, b: &[u8]) -> Result<(), Error> {
            self.0[at as usize..at as usize + b.len()].copy_from_slice(b);
            Ok(())
        }
    }
    fn route() -> Sink {
        let mut sink = Sink::default();
        let gpx = b"<gpx><trk><trkseg><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0\" lat=\"0.001\"/><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0.002\" lat=\"0\"/></trkseg></trk></gpx>";
        obc_route::gpx_to_obcr(&SliceSource(gpx), "Visit", &mut sink).unwrap();
        let descriptor = VisitDescriptor {
            original: RouteSourceKey { store: [1; 16], object: 7, revision: 1 },
            original_anchors_m: [0; 3],
            accepted_anchors_m: [0, 111, 222],
            target_id: 99,
            target_kind: 1,
            target_lon: 0,
            target_lat: 1000,
        };
        let offset = sink.0.len() as u32;
        sink.write(&descriptor.encode().unwrap()).unwrap();
        sink.0[118] = 1;
        sink.0[120..124].copy_from_slice(&offset.to_le_bytes());
        sink.0[124..128].copy_from_slice(&80u32.to_le_bytes());
        sink
    }
    fn app() -> crate::App {
        let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        app.navigator.following.active_route = Some(0);
        app.navigator.review.checkpoint = Some(NavigatorCheckpoint {
            route: PayloadFingerprint { object: 8, revision: 1, length: 1, crc: 1 },
            original: Some(PayloadFingerprint { object: 7, revision: 1, length: 1, crc: 1 }),
            progress_m: 0,
            occurrence: 0,
            lon: 0,
            lat: 0,
            phase: JourneyPhase::Outbound,
            unresolved_avoidance: false,
            lower_m: 0,
            upper_m: 111,
        });
        app.navigator.review.status = ReviewStatus::Accepted;
        app.navigator.visit.phase = Some(JourneyPhase::Outbound);
        app
    }
    fn fix(app: &mut crate::App, route: &RouteReader, lon: i32, lat: i32) {
        app.navigator.match_fix(obc_ports::Fix::at(lat, lon), route);
    }
    fn ack(app: &mut crate::App, tokens: &mut TokenSource<RetentionTag>, route: &RouteReader) {
        let token = tokens.issue();
        app.navigator.checkpoint_issued(token);
        assert!(app.assistant_checkpoint_submission(token));
        app.assistant_checkpoint_answer(RetentionOutcome::CheckpointWritten { token });
        app.navigator.reconcile_visit(route);
    }
    #[test]
    fn phase_ack_replays_latest_fix_without_acceptance_or_dwell_arrival() {
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut stationary = app();
        fix(&mut stationary, &route, 0, 1000);
        fix(&mut stationary, &route, 0, 1000);
        assert!(stationary.navigator.review.change.is_none());
        assert!(!stationary.visit_arrival_pending());

        let mut app = app();
        let mut tokens = TokenSource::new();
        for lat in [0, 300, 600, 900, 1000] {
            fix(&mut app, &route, 0, lat);
        }
        assert_eq!(app.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::AtStop);
        // The rider leaves while the phase is being written. The outbound ceiling must not
        // pre-advance to a repeated coordinate on the return leg.
        fix(&mut app, &route, 0, 700);
        ack(&mut app, &mut tokens, &route);
        assert!(app.visit_arrival_pending());
        assert_eq!(app.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::Returning);
        ack(&mut app, &mut tokens, &route);
        for lat in [500, 200, 0] {
            fix(&mut app, &route, 0, lat);
        }
        assert_eq!(app.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::Following);
        ack(&mut app, &mut tokens, &route);
        assert!(!app.active_visit() && !app.visit_arrival_pending());
        assert!(app.assistant_checkpoint().unwrap().original.is_none());
        assert!(app.navigator.review.change.is_none());
    }
    #[test]
    fn parallel_pass_and_recovered_stop_do_not_show_arrival() {
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut app = app();
        for lat in [0, 300, 700, 1000] {
            fix(&mut app, &route, 100, lat);
        }
        assert!(app.navigator.review.change.is_none());
        app.navigator.review.checkpoint.as_mut().unwrap().phase = JourneyPhase::AtStop;
        app.navigator.review.checkpoint.as_mut().unwrap().lower_m = 111;
        app.navigator.review.checkpoint.as_mut().unwrap().upper_m = 222;
        app.navigator.visit.phase = None;
        app.navigator.reconcile_visit(&route);
        assert!(!app.visit_arrival_pending());
    }
}
