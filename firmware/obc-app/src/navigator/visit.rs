//! Visit requests and phase progress stay in Navigator. Geometry is accepted once.
use super::{review::AfterCheckpoint, NavigatorMachine, ReviewContext, ReviewPurpose, ReviewStatus};
use obc_formats::assistant::JourneyPhase;
use obc_formats::obcm::PoiMetadata;
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

fn departure_m(entry: u32, stop: u32) -> u32 {
    DEPARTURE_M.min(stop.saturating_sub(entry) / 2).max(1)
}

pub(super) struct VisitState {
    target: Option<(PoiMetadata, (i32, i32))>,
    latest_fix: Option<(i32, i32)>,
    departure_fix: Option<(i32, i32)>,
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
            departure_fix: None,
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
    pub(crate) fn reconcile_visit(&mut self, route: &RouteReader) -> bool {
        let Some(c) = self.review.checkpoint else {
            let changed = self.visit.phase.is_some() || self.visit.arrival;
            self.visit.phase = None;
            self.visit.arrival = false;
            return changed;
        };
        if self.review.status != ReviewStatus::Accepted || self.visit.phase == Some(c.phase) {
            return false;
        }
        let previous = self.visit.phase;
        let recovering = previous.is_none() && self.visit.target.is_none();
        if recovering && c.phase == JourneyPhase::Outbound {
            if let Ok(Some(descriptor)) = route.visit_descriptor() {
                let [entry, stop, _] = descriptor.accepted_anchors_m;
                self.visit.departure_fix = route.position_at(entry).map(|at| (at.lon, at.lat));
                self.visit.departed |= c.progress_m >= entry.saturating_add(departure_m(entry, stop));
            }
        }
        self.visit.phase = Some(c.phase);
        self.visit.target = None;
        if c.phase == JourneyPhase::Following {
            self.visit.arrival = false;
            return true;
        }
        if c.phase == JourneyPhase::AtStop && previous == Some(JourneyPhase::Outbound) && self.visit.departed {
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
        true
    }
    pub(super) fn retry_visit_phase(&mut self) {
        self.visit.phase = None;
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
        let departure_m = departure_m(entry, stop);
        let remaining = if stop == rejoin { route.total_distance_m.saturating_sub(stop) } else { rejoin - stop };
        let leave_m = if remaining == 0 { 0 } else { LEAVE_STOP_M.min(remaining / 2).max(1) };
        let return_arrival_m = ARRIVAL_M.min(rejoin.saturating_sub(stop) / 2).max(1);
        if current.phase == JourneyPhase::Outbound {
            let first = *self.visit.departure_fix.get_or_insert(fix);
            if progress >= entry.saturating_add(departure_m)
                && obc_map_scene::ground_dist_m(first, fix) >= departure_m as f32
            {
                self.visit.departed = true;
            }
        }
        self.review.latest_origin = Some(super::ReviewOrigin {
            fix,
            progress_m: progress,
            occurrence: self.route_match.occurrence(),
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
                next.upper_m = if stop == rejoin { route.total_distance_m } else { rejoin };
                next.progress_m = stop;
            }
            JourneyPhase::AtStop if progress >= stop.saturating_add(leave_m) && at_stop >= leave_m as f32 => {
                if stop == rejoin {
                    next.phase = JourneyPhase::Following;
                    next.original = None;
                    next.lower_m = rejoin;
                    next.upper_m = route.total_distance_m;
                } else {
                    next.phase = JourneyPhase::Returning;
                }
                next.progress_m = progress;
            }
            JourneyPhase::Returning if progress >= rejoin.saturating_sub(return_arrival_m) => {
                let Some(at) = route.position_at(rejoin) else {
                    return;
                };
                if obc_map_scene::ground_dist_m(fix, (at.lon, at.lat)) > return_arrival_m as f32 {
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
        let Some(occurrence) = self.route_match.occurrence_at(route, next.progress_m) else {
            return;
        };
        next.occurrence = occurrence;
        next.lon = fix.0;
        next.lat = fix.1;
        if next.valid() {
            self.review.change = Some(Some(next));
            self.review.after = AfterCheckpoint::Phase;
        }
    }
}

impl crate::App {
    /// Stage geometry from the exact published candidate after constructing its ReviewReady
    /// outcome. The buffer is shared with ordinary route overviews and remains hidden until ACK.
    pub fn set_assistant_preview_shape(
        &mut self,
        token: crate::device_core::OperationToken<crate::device_core::NavigatorTag>,
        source: obc_formats::assistant::PayloadFingerprint,
        points: &[(i32, i32)],
    ) -> bool {
        if !self.navigator.accepts(&super::NavigatorOutcome::ReviewReady { token })
            || self.assistant_preview().map(|preview| preview.source) != Some(source)
        {
            return false;
        }
        let Some(index) = self.route_ids().iter().position(|id| *id == source.object) else { return false };
        let Some(key) = self.catalogs.nav_preview_key(Some(index), true) else { return false };
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
        self.catalogs.nav_preview_for(self.catalogs.nav_preview_key(index, true))
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
        self.request_place_route(target, name, false)
    }
    /// Route to the place without retaining the current route's continuation.
    pub fn request_destination(&mut self, target: VisitTarget, name: &str) -> Result<(), VisitUnavailable> {
        self.request_place_route(target, name, true)
    }
    fn request_place_route(
        &mut self,
        target: VisitTarget,
        name: &str,
        destination: bool,
    ) -> Result<(), VisitUnavailable> {
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
        if !target.metadata.source.is_valid() || target.metadata.approach.is_some_and(|a| !a.source.is_valid()) {
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
            purpose: if original.is_some() && !destination { ReviewPurpose::Visit } else { ReviewPurpose::Destination },
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
    pub(crate) fn request_easier(
        &mut self,
        map: obc_formats::obcr::RouteSourceKey,
        objective: obc_route::nav::Objective,
        destination: (i32, i32),
    ) -> Result<(), VisitUnavailable> {
        let scope = self.catalogs.loaded_scope.ok_or(VisitUnavailable::SourceChanged)?;
        let origin = self.current_review_origin().ok_or(VisitUnavailable::NoFix)?;
        let original = self
            .active_route_index()
            .and_then(|i| self.route_ids().get(i))
            .copied()
            .ok_or(VisitUnavailable::Unmatched)?;
        if scope.store.bytes() != map.store {
            return Err(VisitUnavailable::SourceChanged);
        }
        let context = ReviewContext {
            purpose: ReviewPurpose::Easier(objective),
            map,
            store: scope.store,
            original: None,
            origin: origin.fix,
            progress_m: origin.progress_m,
            occurrence: origin.occurrence,
            required_anchors_m: [origin.progress_m; 3],
            profile: self.settings().bike_profile_idx,
            facts_policy: super::REVIEW_FACTS_POLICY,
            unresolved_avoidance: false,
        };
        self.navigator.visit = VisitState::new();
        self.navigator.visit.catalog_revision = scope.revision.raw();
        self.navigator.visit.requested_route = original;
        self.navigator.visit.needs_bind = true;
        self.plan_assistant(crate::NavRequest::new(origin.fix, destination, "Easier route"), context);
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
        original: Option<obc_formats::assistant::PayloadFingerprint>,
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
    /// Place queries supply a map-bound approach or an ordinary coordinate destination.
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
            || anchors[1] < context.progress_m
            || anchors[2] < anchors[1]
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
        self.ui.map_dirty |= self.navigator.visit.arrival;
        self.navigator.visit.arrival = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device_core::{MetadataTag, TokenSource};
    use crate::metadata::MetadataOutcome;
    use obc_formats::assistant::{NavigatorCheckpoint, PayloadFingerprint};
    use obc_formats::io::{ByteSink, Error, SliceSource};
    use obc_formats::obcr::{RouteSourceKey, VisitDescriptor};

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
        let gpx = b"<gpx><trk><trkseg><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0\" lat=\"0.001\"/><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0.002\" lat=\"0\"/></trkseg></trk></gpx>";
        visit_route(gpx, [0, 111, 222], 1000)
    }
    fn visit_route(gpx: &[u8], anchors: [u32; 3], target_lat: i32) -> Sink {
        let mut sink = Sink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx), "Visit", &mut sink).unwrap();
        let descriptor = VisitDescriptor {
            original: RouteSourceKey { store: [1; 16], object: 7, revision: 1 },
            original_anchors_m: [0; 3],
            accepted_anchors_m: anchors,
            target_id: 99,
            target_kind: 1,
            target_lon: 0,
            target_lat,
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
            selection: false,
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
    fn ack(app: &mut crate::App, tokens: &mut TokenSource<MetadataTag>, route: &RouteReader) {
        let token = tokens.issue();
        app.navigator.checkpoint_issued(token);
        assert!(app.assistant_checkpoint_submission(token));
        app.assistant_checkpoint_answer(MetadataOutcome::CheckpointWritten { token });
        app.navigator.reconcile_visit(route);
    }
    fn live_fix(app: &mut crate::App, route: Option<&RouteReader>, lon: i32, lat: i32) {
        struct Location(Option<obc_ports::Fix>);
        impl obc_ports::LocationSource for Location {
            fn poll(&mut self) -> Option<obc_ports::Fix> {
                self.0.take()
            }
        }
        app.tick(
            obc_ports::RideClock(0),
            obc_ports::Sensors::new(&mut Location(Some(obc_ports::Fix::at(lat, lon)))),
            route,
        );
    }
    fn open_current(app: &mut crate::App, route: &RouteReader) {
        assert!(app.apply_chord(crate::input::Chord::Assistant));
        app.apply_chord(crate::input::Chord::Context);
        app.apply_gesture(crate::Gesture::Press);
        app.prepare_find(None, Some(route));
        assert!(matches!(app.top_screen(), crate::screen::Screen::VisitReview(s) if s.accepted));
    }
    #[test]
    fn destination_request_keeps_original_authority_until_acceptance() {
        use crate::device_core::{Revision, StoreIdentity, StoreRevision};
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[route.summary()], &[7]);
        app.navigator.following.active_route = Some(0);
        let store = StoreIdentity::from_bytes([1; 16]);
        let scope = StoreRevision { store, revision: Revision::new(1) };
        app.catalogs.loaded_scope = Some(scope);
        live_fix(&mut app, Some(&route), 0, 0);
        let target = VisitTarget {
            map: RouteSourceKey { store: store.bytes(), object: 1, revision: 1 },
            display: (0, 1000),
            metadata: PoiMetadata { source: obc_formats::obcm::SourceId::osm(1, 99), approach: None },
        };
        app.request_destination(target, "Water").unwrap();
        let original = PayloadFingerprint { object: 7, revision: 1, length: 100, crc: 1 };
        assert!(!app.bind_visit_sources(scope, None, false));
        assert!(app.bind_visit_sources(scope, Some(original), false));
        let context = app.assistant_review_context().unwrap();
        assert_eq!(context.purpose, ReviewPurpose::Destination);
        assert_eq!(context.original, Some(original));
        assert_eq!(app.active_route_index(), Some(0));
        app.cancel_assistant();
        assert_eq!(app.active_route_index(), Some(0));
        assert!(app.assistant_checkpoint().is_none());
    }
    #[test]
    fn current_visit_cancel_waits_for_checkpoint_or_explicit_connector_acceptance() {
        use crate::{
            device_core::{Revision, StoreIdentity, StoreRevision},
            Gesture,
        };
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        for departed in [false, true] {
            let mut app = app();
            app.set_routes_with_ids(&[route.summary(), route.summary()], &[8, 7]);
            app.navigator.following.active_route = Some(0);
            app.catalogs.loaded_scope =
                Some(StoreRevision { store: StoreIdentity::from_bytes([1; 16]), revision: Revision::new(1) });
            app.bind_place_map(Some(RouteSourceKey { store: [1; 16], object: 1, revision: 1 }));
            live_fix(&mut app, Some(&route), 0, if departed { 600 } else { 0 });
            app.navigator.visit.departed = departed;
            let checkpoint = app.assistant_checkpoint();
            let recording = app.ride_session();
            open_current(&mut app, &route);
            app.apply_gesture(Gesture::Step(1));
            app.apply_gesture(Gesture::Press);
            app.prepare_find(None, Some(&route));
            assert_eq!(app.active_route_index(), Some(0));
            assert_eq!(app.assistant_checkpoint(), checkpoint);
            if departed {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Planning);
                assert_eq!(app.assistant_review_context().unwrap().purpose, ReviewPurpose::ReturnToRoute);
                assert!(app.assistant_preview().is_none());
                app.apply_gesture(Gesture::Back);
                assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
                assert_eq!(app.assistant_checkpoint(), checkpoint);
            } else {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Saving);
                app.apply_gesture(Gesture::Back);
                assert_eq!(app.navigator.review.change, Some(None), "Back cannot revoke the requested journey change");
                ack(&mut app, &mut TokenSource::new(), &route);
                assert_eq!(app.active_route_index(), Some(1));
                assert!(app.assistant_checkpoint().is_none());
            }
            assert_eq!(app.ride_session(), recording);
        }
    }
    #[test]
    fn arrival_card_dismissal_and_rejoin_leave_guidance_and_recording_alone() {
        use crate::{screen::Screen, Gesture};
        for dismiss in [false, true] {
            let bytes = route();
            let source = SliceSource(&bytes.0);
            let index = obc_route::RouteIndex::read(&source).unwrap();
            let route = RouteReader::new(&index, &source);
            let mut app = app();
            let recording = app.ride_session();
            for lat in [0, 300, 600, 900, 1000] {
                fix(&mut app, &route, 0, lat);
            }
            let mut tokens = TokenSource::new();
            ack(&mut app, &mut tokens, &route);
            app.advance_animations(obc_ports::InputClock(0));
            assert!(matches!(app.top_screen(), Screen::Journey(s) if !s.resume));
            let checkpoint = app.assistant_checkpoint();
            if dismiss {
                app.apply_gesture(Gesture::Back);
                assert!(!app.visit_arrival_pending());
                assert_eq!(app.assistant_checkpoint(), checkpoint);
                app.advance_animations(obc_ports::InputClock(0));
                assert!(!matches!(app.top_screen(), Screen::Journey(_)));
            }
            fix(&mut app, &route, 0, 700);
            ack(&mut app, &mut tokens, &route);
            for lat in [500, 200, 0] {
                fix(&mut app, &route, 0, lat);
            }
            ack(&mut app, &mut tokens, &route);
            app.advance_animations(obc_ports::InputClock(0));
            assert!(!matches!(app.top_screen(), Screen::Journey(_)));
            assert!(!app.active_visit());
            assert_eq!(app.ride_session(), recording);
        }
    }
    #[test]
    fn recording_recovery_preserves_the_separate_assistant_resume_decision() {
        use crate::{screen::Screen, Gesture, RideContinuation, RideDamage};
        let checkpoint = app().assistant_checkpoint();
        for assistant_first in [false, true] {
            for choice in 0..3 {
                let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
                let store = crate::device_core::StoreIdentity::from_bytes([1; 16]);
                if assistant_first {
                    app.offer_assistant_checkpoint(store, checkpoint);
                }
                // The recording offer must also suspend any ordinary guidance already selected.
                app.navigator.following.active_route = Some(0);
                if choice == 2 {
                    assert!(app.offer_damaged_ride(RideDamage::Payload));
                } else {
                    assert!(app.offer_recovered_ride(RideContinuation::default()));
                }
                if !assistant_first {
                    app.offer_assistant_checkpoint(store, checkpoint);
                }
                assert!(app.active_route_index().is_none());
                assert_eq!(app.assistant_checkpoint(), checkpoint);
                assert!(app.navigator.review.change.is_none(), "a recording offer cannot cancel a journey");
                assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));
                if choice == 1 {
                    app.apply_gesture(Gesture::Step(1));
                }
                app.apply_gesture(if choice == 0 { Gesture::Press } else { Gesture::Hold });
                app.advance_animations(obc_ports::InputClock(0));
                app.prepare_find(None, None);
                assert!(app.active_route_index().is_none(), "recording recovery cannot resume guidance");
                assert_eq!(app.assistant_checkpoint(), checkpoint);
                assert!(app.navigator.review.change.is_none(), "the recording choice cannot write journey metadata");
                assert_eq!(app.assistant_review_status(), ReviewStatus::ResumeAvailable);
                assert!(app.requested_assistant_resume().is_none());
                if choice == 0 {
                    assert!(matches!(app.top_screen(), Screen::Journey(s) if s.resume));
                    app.apply_gesture(Gesture::Press);
                    assert_eq!(app.requested_assistant_resume(), checkpoint.map(|c| c.route));
                    assert!(app.active_route_index().is_none(), "Resume still needs the exact-source save gate");
                }
            }
        }
    }
    #[test]
    fn recovery_card_requires_a_fresh_phase_match_and_explicit_press() {
        use crate::{screen::Screen, Gesture};
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let checkpoint = app().assistant_checkpoint();
        let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        app.offer_assistant_checkpoint(crate::device_core::StoreIdentity::from_bytes([1; 16]), checkpoint);
        app.advance_animations(obc_ports::InputClock(0));
        app.prepare_find(None, None);
        assert!(matches!(app.top_screen(), Screen::Journey(s) if s.resume));
        assert!(app.requested_assistant_resume().is_none());
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.requested_assistant_resume(), checkpoint.map(|c| c.route));
        app.prepare_assistant_resume(Some(&route));
        assert_eq!(app.assistant_review_status(), ReviewStatus::ResumeAvailable);
        assert!(app.active_route_index().is_none());
        live_fix(&mut app, None, 0, 0);
        app.apply_gesture(Gesture::Press);
        app.prepare_assistant_resume(Some(&route));
        assert_eq!(app.assistant_review_status(), ReviewStatus::Saving);
        assert_eq!(app.navigator.review.change, Some(checkpoint));
        assert!(app.active_route_index().is_none());
        assert!(!app.recording());
        assert!(!app.visit_arrival_pending());
    }
    #[test]
    fn resume_records_current_progress_inside_outbound_and_return_phases_and_retries_ambiguity() {
        use crate::{screen::Screen, Gesture};
        let bytes = visit_route(b"<gpx><trk><trkseg><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0\" lat=\"0.01\"/><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0.02\" lat=\"0\"/></trkseg></trk></gpx>", [0, 1111, 2222], 10000);
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        for (phase, lower_m, upper_m) in [
            (JourneyPhase::Outbound, 0, 1111),
            (JourneyPhase::Returning, 1111, 2222),
            (JourneyPhase::Following, 0, route.total_distance_m),
        ] {
            let checkpoint = NavigatorCheckpoint {
                phase,
                lower_m,
                upper_m,
                progress_m: lower_m,
                ..app().assistant_checkpoint().unwrap()
            };
            let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
            app.offer_assistant_checkpoint(crate::device_core::StoreIdentity::from_bytes([1; 16]), Some(checkpoint));
            live_fix(&mut app, None, 0, 5000);
            app.advance_animations(obc_ports::InputClock(0));
            app.apply_gesture(Gesture::Press);
            app.prepare_assistant_resume(Some(&route));
            if phase == JourneyPhase::Following {
                assert_eq!(app.assistant_review_status(), ReviewStatus::ResumeAvailable);
                assert!(
                    matches!(app.top_screen(), Screen::Journey(s) if s.error == Some(crate::screen::JourneyError::Unmatched))
                );
                assert!(app.navigator.review.change.is_none());
                live_fix(&mut app, None, 10000, 0);
                app.apply_gesture(Gesture::Press);
                app.prepare_assistant_resume(Some(&route));
            }
            assert_eq!(app.assistant_review_status(), ReviewStatus::Saving);
            let next = app.navigator.review.change.unwrap().unwrap();
            assert!(next.progress_m > checkpoint.progress_m + 50);
            assert_eq!(next.phase, checkpoint.phase);
            assert_eq!(next.occurrence, app.navigator.route_match.occurrence());
            assert!(app.active_route_index().is_none());
            assert!(!app.recording());
        }
    }
    #[test]
    fn outbound_resume_near_stop_keeps_proven_departure_and_completes_after_dwell() {
        use crate::Gesture;
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        for remaining in [5, 15] {
            let checkpoint = app().assistant_checkpoint().unwrap();
            let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
            app.set_routes_with_ids(&[route.summary()], &[8]);
            app.offer_assistant_checkpoint(crate::device_core::StoreIdentity::from_bytes([1; 16]), Some(checkpoint));
            let at = route.position_at(checkpoint.upper_m - remaining).unwrap();
            live_fix(&mut app, None, at.lon, at.lat);
            app.advance_animations(obc_ports::InputClock(0));
            app.apply_gesture(Gesture::Press);
            app.prepare_assistant_resume(Some(&route));
            assert_eq!(app.assistant_review_status(), ReviewStatus::Saving);
            let mut tokens = TokenSource::new();
            ack(&mut app, &mut tokens, &route);
            assert!(app.navigator.visit.departed);
            live_fix(&mut app, Some(&route), 0, 1000);
            live_fix(&mut app, Some(&route), 0, 1000);
            assert_eq!(app.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::AtStop);
            ack(&mut app, &mut tokens, &route);
            live_fix(&mut app, Some(&route), 0, 1000);
            assert_eq!(app.assistant_checkpoint().unwrap().phase, JourneyPhase::AtStop);
            live_fix(&mut app, Some(&route), 0, 700);
            assert_eq!(app.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::Returning);
            assert!(!app.recording());
        }
        let mut initial = app();
        initial.navigator.visit.phase = None;
        initial.navigator.visit.target = Some((PoiMetadata::default(), (0, 1000)));
        initial.navigator.reconcile_visit(&route);
        fix(&mut initial, &route, 0, 1000);
        fix(&mut initial, &route, 0, 1000);
        assert!(!initial.navigator.visit.departed);
        assert!(initial.navigator.review.change.is_none(), "fresh stationary acceptance still needs movement");
    }
    #[test]
    fn unpublished_phase_edits_keep_guidance_and_retry_latest_fix_without_another_sample() {
        use crate::metadata::MetadataError;
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        for uncertain in [false, true] {
            let mut app = app();
            let store = crate::device_core::StoreIdentity::from_bytes([1; 16]);
            app.navigator.review.store = Some(store);
            let mut tokens = TokenSource::new();
            let recording = app.ride_session();
            for (expected, fixes) in [
                (JourneyPhase::AtStop, &[0, 300, 600, 900, 1000][..]),
                (JourneyPhase::Returning, &[700][..]),
                (JourneyPhase::Following, &[500, 200, 0][..]),
            ] {
                for &lat in fixes {
                    fix(&mut app, &route, 0, lat);
                }
                let old = app.assistant_checkpoint();
                let pending = app.navigator.review.change;
                assert_eq!(pending.unwrap().unwrap().phase, expected);
                let latest = app.navigator.visit.latest_fix;
                let token = tokens.issue();
                app.navigator.checkpoint_issued(token);
                assert!(app.assistant_checkpoint_submission(token));
                app.assistant_checkpoint_answer(MetadataOutcome::Failed {
                    token,
                    error: if uncertain { MetadataError::RemountRequired } else { MetadataError::Busy },
                });
                if uncertain {
                    assert_eq!(app.assistant_review_status(), ReviewStatus::Unresolved);
                    assert!(!app.navigator.reconcile_visit(&route));
                    app.offer_assistant_checkpoint(store, old);
                }
                assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
                assert_eq!(app.assistant_checkpoint(), old);
                assert_eq!(app.active_route_index(), Some(0));
                assert_eq!(app.navigator.visit.latest_fix, latest);
                assert!(app.navigator.review.change.is_none());
                app.navigator.reconcile_visit(&route);
                assert_eq!(app.navigator.review.change, pending, "retry reuses the latest trustworthy fix");
                assert!(!app.visit_arrival_pending(), "retry cannot raise a dismissed card again");
                ack(&mut app, &mut tokens, &route);
                assert_eq!(app.assistant_checkpoint().unwrap().phase, expected);
                app.dismiss_visit_arrival();
                assert_eq!(app.ride_session(), recording);
            }
        }
    }
    #[test]
    fn refused_restore_keeps_the_visit_and_refused_initial_resume_stays_inactive() {
        use crate::{
            device_core::{Revision, StoreIdentity, StoreRevision},
            metadata::MetadataError,
            screen::Screen,
            Gesture,
        };
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let store = StoreIdentity::from_bytes([1; 16]);
        for (resume, uncertain) in [(false, false), (false, true), (true, false), (true, true)] {
            let original = app().assistant_checkpoint();
            let mut app = if resume { crate::App::new_idle(crate::AppState::new(0, 0, 1.0)) } else { app() };
            app.set_routes_with_ids(&[route.summary(), route.summary()], &[8, 7]);
            app.catalogs.loaded_scope = Some(StoreRevision { store, revision: Revision::new(1) });
            app.bind_place_map(Some(RouteSourceKey { store: store.bytes(), object: 1, revision: 1 }));
            let old = original;
            app.navigator.review.store = Some(store);
            if resume {
                app.offer_assistant_checkpoint(store, old);
                live_fix(&mut app, None, 0, 500);
                app.advance_animations(obc_ports::InputClock(0));
                app.apply_gesture(Gesture::Press);
                app.prepare_assistant_resume(Some(&route));
            } else {
                app.navigator.following.active_route = Some(0);
                live_fix(&mut app, Some(&route), 0, 0);
                open_current(&mut app, &route);
                app.apply_gesture(Gesture::Step(1));
                app.apply_gesture(Gesture::Press);
                app.prepare_find(None, Some(&route));
            }
            assert_eq!(app.assistant_review_status(), ReviewStatus::Saving);
            let mut tokens = TokenSource::new();
            let token = tokens.issue();
            app.navigator.checkpoint_issued(token);
            assert!(app.assistant_checkpoint_submission(token));
            let _ = app.take_dirty();
            app.assistant_checkpoint_answer(MetadataOutcome::Failed {
                token,
                error: if uncertain { MetadataError::RemountRequired } else { MetadataError::WriteFailed },
            });
            if uncertain {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Unresolved);
                app.offer_assistant_checkpoint(store, old);
            }
            assert!(app.take_dirty().map, "save refusal must repaint without another GPS sample");
            assert_eq!(app.assistant_checkpoint(), old);
            assert!(app.navigator.review.change.is_none());
            assert!(!app.recording());
            if resume {
                assert_eq!(app.assistant_review_status(), ReviewStatus::ResumeAvailable);
                assert!(app.active_route_index().is_none());
                assert!(matches!(app.top_screen(), Screen::Journey(s) if s.error.is_some()));
                app.advance_animations(obc_ports::InputClock(0));
                app.apply_gesture(Gesture::Press);
                app.prepare_assistant_resume(Some(&route));
                assert_eq!(app.assistant_review_status(), ReviewStatus::Saving);
            } else {
                assert_eq!(app.assistant_review_status(), ReviewStatus::Accepted);
                assert_eq!(app.active_route_index(), Some(0));
                app.prepare_find(None, Some(&route));
                assert!(matches!(app.top_screen(), Screen::VisitReview(s) if s.accepted && s.error.is_some()));
                app.apply_gesture(Gesture::Back);
                assert!(matches!(app.top_screen(), Screen::VisitReview(s) if s.accepted && s.error.is_none()));
            }
        }
    }
    #[test]
    fn accepted_visit_reopens_from_assistant_without_a_second_acceptance() {
        use crate::{input::Chord, screen::Screen, Gesture};
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut app = app();
        app.set_routes_with_ids(&[route.summary()], &[8]);
        app.navigator.following.active_route = Some(0);
        let checkpoint = app.assistant_checkpoint();
        let session = app.ride_session();
        assert!(app.apply_chord(Chord::Assistant));
        app.apply_gesture(Gesture::Press); // Explore another question first.
        assert!(matches!(app.top_screen(), Screen::FindPlace(_)));
        app.apply_gesture(Gesture::Back);
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        app.prepare_find(None, Some(&route));
        assert!(matches!(app.top_screen(), Screen::VisitReview(s) if s.accepted));
        assert_eq!(app.ui.find.review, ReviewStatus::Accepted);
        assert_eq!(app.ui.find.review_costs.unwrap().arrival_m, 111);
        assert_eq!(app.derived_needs().nav_preview.unwrap().route, 8);
        // Returning to Assistant lets the new question own preparation and geometry.
        assert!(app.apply_chord(Chord::Assistant));
        app.apply_gesture(Gesture::Press);
        app.prepare_find(None, Some(&route));
        assert!(matches!(app.top_screen(), Screen::FindPlace(_)));
        assert_ne!(app.find_place_state(), crate::find_place::State::Start);
        assert!(app.derived_needs().nav_preview.is_none());
        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Assistant(_)));
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        app.prepare_find(None, Some(&route));
        assert!(matches!(app.top_screen(), Screen::VisitReview(s) if s.accepted));
        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Assistant(_)));
        assert_eq!(app.assistant_checkpoint(), checkpoint);
        assert_eq!(app.ride_session(), session);
        assert!(app.navigator.review.change.is_none());
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        app.navigator.following.active_route = None;
        app.prepare_find(None, None);
        assert!(matches!(app.ui.find.review, ReviewStatus::Failed(_)));
        assert!(app.ui.find.review_costs.is_none());
        assert!(app.derived_needs().nav_preview.is_none());
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::VisitReview(_)));
        app.apply_gesture(Gesture::Back);
        assert_eq!(app.assistant_checkpoint(), checkpoint);
        app.navigator.following.active_route = Some(0);
        assert!(app.current_visit_index().is_some());
        app.on_route_uploaded(8, true, None);
        assert!(app.current_visit_index().is_none());
        assert_eq!(app.assistant_checkpoint(), checkpoint);
        assert!(app.navigator.review.change.is_none());
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
    fn on_route_stops_resume_the_tail_after_stationary_dwell() {
        use crate::navigator::{ReviewContext, ReviewOrigin, ReviewedRoute, REVIEW_FACTS_POLICY};
        for (stop, tail_lat) in [(0, 1000), (0, 90), (0, 0), (111, 1000), (111, 90), (111, 0)] {
            let lat = if stop == 0 { 0 } else { 1000 };
            let gpx = std::format!(
                "<gpx><trk><trkseg><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0\" lat=\"{}\"/></trkseg></trk></gpx>",
                (lat + tail_lat) as f64 / 1_000_000.0
            );
            let bytes = visit_route(gpx.as_bytes(), [0, stop, stop], lat);
            let source = SliceSource(&bytes.0);
            let index = obc_route::RouteIndex::read(&source).unwrap();
            let route = RouteReader::new(&index, &source);
            let mut app = app();
            app.set_routes_with_ids(&[route.summary(), route.summary()], &[8, 7]);
            let checkpoint = app.assistant_checkpoint().unwrap();
            app.navigator.review.context = Some(ReviewContext {
                purpose: ReviewPurpose::Visit,
                map: RouteSourceKey { store: [1; 16], object: 1, revision: 1 },
                store: crate::device_core::StoreIdentity::from_bytes([1; 16]),
                original: checkpoint.original,
                origin: (0, 0),
                progress_m: 0,
                occurrence: 0,
                required_anchors_m: [0; 3],
                profile: 0,
                facts_policy: REVIEW_FACTS_POLICY,
                unresolved_avoidance: false,
            });
            app.navigator.review.preview = Some(ReviewedRoute {
                source: checkpoint.route,
                distance_m: route.total_distance_m,
                ascent_m: 0,
                descent_m: 0,
                visit_anchors_m: Some([0, stop, stop]),
                visit_costs: None,
            });
            app.navigator.review.status = ReviewStatus::Preview;
            app.navigator.accept_review(
                ReviewOrigin { fix: (0, 0), progress_m: 0, occurrence: 0, lateral_m: 0, trustworthy: true },
                0,
            );
            let mut tokens = TokenSource::new();
            ack(&mut app, &mut tokens, &route);
            assert!(!app.visit_arrival_pending());
            if route.total_distance_m == 0 {
                assert!(!app.active_visit());
                assert!(app.assistant_checkpoint().unwrap().original.is_none());
                continue;
            }
            if stop != 0 {
                for lat in [0, 300, 600, 900, 1000] {
                    fix(&mut app, &route, 0, lat);
                }
                ack(&mut app, &mut tokens, &route);
                assert!(app.visit_arrival_pending());
            }
            assert_eq!(app.assistant_checkpoint().unwrap().phase, JourneyPhase::AtStop);
            assert_eq!(app.assistant_checkpoint().unwrap().upper_m, route.total_distance_m);
            if route.total_distance_m > stop {
                for _ in 0..3 {
                    fix(&mut app, &route, 0, lat);
                }
                assert!(app.navigator.review.change.is_none());
                let next = route.position_at(stop + (route.total_distance_m - stop).min(30)).unwrap();
                fix(&mut app, &route, next.lon, next.lat);
            }
            assert_eq!(app.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::Following);
            ack(&mut app, &mut tokens, &route);
            assert!(!app.active_visit());
            assert!(app.assistant_checkpoint().unwrap().original.is_none());
            assert!(!app.visit_arrival_pending());
        }
    }

    #[test]
    fn short_legs_require_movement_and_cross_durable_phase_acks() {
        let bytes=visit_route(b"<gpx><trk><trkseg><trkpt lon=\"0\" lat=\"0\"/><trkpt lon=\"0\" lat=\"0.00027\"/><trkpt lon=\"0\" lat=\"0.00018\"/><trkpt lon=\"0.002\" lat=\"0.00018\"/></trkseg></trk></gpx>",[0,30,40],270);
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut stationary = app();
        stationary.navigator.review.checkpoint.as_mut().unwrap().upper_m = 30;
        for _ in 0..3 {
            fix(&mut stationary, &route, 0, 270);
        }
        assert!(stationary.navigator.review.change.is_none());
        let mut moving = app();
        moving.navigator.review.checkpoint.as_mut().unwrap().upper_m = 30;
        let mut tokens = TokenSource::new();
        for lat in [0, 90, 180, 270] {
            fix(&mut moving, &route, 0, lat);
        }
        assert_eq!(moving.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::AtStop);
        ack(&mut moving, &mut tokens, &route);
        for _ in 0..3 {
            fix(&mut moving, &route, 0, 270);
        }
        assert!(moving.navigator.review.change.is_none());
        fix(&mut moving, &route, 0, 215);
        assert_eq!(moving.navigator.review.change.unwrap().unwrap().phase, JourneyPhase::Returning);
        ack(&mut moving, &mut tokens, &route);
        fix(&mut moving, &route, 0, 180);
        if moving.assistant_checkpoint().unwrap().phase != JourneyPhase::Following {
            ack(&mut moving, &mut tokens, &route);
        }
        assert_eq!(moving.assistant_checkpoint().unwrap().phase, JourneyPhase::Following);
    }

    #[test]
    fn phase_checkpoint_resumes_at_its_nonzero_repeated_coordinate_occurrence() {
        let bytes = route();
        let source = SliceSource(&bytes.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut app = app();
        let mut tokens = TokenSource::new();
        for lat in [0, 300, 600, 1000] {
            fix(&mut app, &route, 0, lat);
        }
        assert_eq!(app.latest_assistant_origin().unwrap().occurrence, app.navigator.route_match.occurrence());
        ack(&mut app, &mut tokens, &route);
        fix(&mut app, &route, 0, 700);
        ack(&mut app, &mut tokens, &route);
        for lat in [400, 0] {
            fix(&mut app, &route, 0, lat);
        }
        ack(&mut app, &mut tokens, &route);
        let checkpoint = app.assistant_checkpoint().unwrap();
        assert!(checkpoint.occurrence > 0);
        let mut matched = obc_route::RouteMatch::new();
        matched.set_progress_floor(&route, checkpoint.lower_m);
        let position = route.position_at(checkpoint.progress_m).unwrap();
        let m = matched.update_to(position.lon, position.lat, &route, checkpoint.upper_m);
        assert_eq!(matched.occurrence(), checkpoint.occurrence);
        let mut resumed = NavigatorMachine::new();
        resumed.offer_checkpoint(crate::device_core::StoreIdentity::from_bytes([1; 16]), Some(checkpoint));
        resumed.resume_review(super::super::ReviewOrigin {
            fix: (position.lon, position.lat),
            progress_m: m.progress_m,
            occurrence: matched.occurrence(),
            lateral_m: 0,
            trustworthy: true,
        });
        assert_eq!(resumed.review.status, ReviewStatus::Saving);
        assert_eq!(
            resumed.review.change,
            Some(Some(NavigatorCheckpoint { lon: position.lon, lat: position.lat, ..checkpoint }))
        );
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
