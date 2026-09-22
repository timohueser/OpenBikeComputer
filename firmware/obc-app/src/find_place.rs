//! Bounded place acquisition and sequential, measured visit comparisons.
use crate::{
    navigator::{ReviewContext, ReviewStatus},
    screen::{FindPlaceScreen, Screen, VisitReviewScreen},
};
use obc_formats::obcr::RouteSourceKey;
use obc_reader::{
    hours::OpeningStatus,
    reader::places::{PlaceQuery, PlaceWindow, QueryProgress},
    Poi, PoiCategory, PoiCategorySet, Reader,
};
use obc_route::{visit::VisitTarget, RouteReader};

pub const NEARBY_M: u32 = 10_000;
pub const FORWARD_M: u32 = 20_000;
pub const SOURCE_LIMIT: usize = 6;
pub const PLAN_LIMIT: usize = SOURCE_LIMIT * 2;
pub const RESULT_LIMIT: usize = SOURCE_LIMIT;
pub const ON_WAY_M: u32 = 400;
const QUERY_STEPS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Start,
    Querying,
    Planning,
    Releasing,
    Ready,
    Empty,
    NoFix,
    NoMap,
    NoAccess,
    Failed,
    Stale,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    None,
    Refresh,
    More,
    Preview(u8),
    Accept,
    RouteMode(bool),
    Cancel,
    OpenAccepted,
    CancelVisit,
    Resume,
    DismissArrival,
    DismissResume,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Costs {
    pub arrival_m: u32,
    pub arrival_ascent_m: Option<u32>,
    pub added_m: Option<u32>,
    pub added_ascent_m: Option<u32>,
}
impl Costs {
    pub fn on_way(self) -> bool {
        self.added_m.is_some_and(|m| m <= ON_WAY_M)
    }
    fn dominates(self, other: Self) -> bool {
        self.arrival_m <= other.arrival_m
            && matches!((self.arrival_ascent_m, other.arrival_ascent_m), (Some(a), Some(b)) if a <= b)
            && matches!((self.added_m, other.added_m), (Some(a), Some(b)) if a <= b)
            && matches!((self.added_ascent_m, other.added_ascent_m), (Some(a), Some(b)) if a <= b)
    }
}

#[derive(Clone, Copy, Default)]
struct RetainedReview {
    object: u64,
    revision: u64,
    rejoin_m: u32,
}
impl RetainedReview {
    const NONE: Self = Self { object: 0, revision: 0, rejoin_m: 0 };
    fn source(self, store: crate::device_core::StoreIdentity) -> RouteSourceKey {
        RouteSourceKey { store: store.bytes(), object: self.object, revision: self.revision }
    }
}

pub struct FindState {
    pub state: State,
    pub(crate) action: Action,
    pub category: PoiCategory,
    limit: usize,
    hours_filter: obc_reader::reader::places::HoursFilter,
    pub map: Option<RouteSourceKey>,
    bound_map: Option<RouteSourceKey>,
    pub origin: (i32, i32),
    pub results: heapless::Vec<u8, RESULT_LIMIT>,
    places: heapless::Vec<Poi, RESULT_LIMIT>,
    costs: [Option<Costs>; PLAN_LIMIT],
    corridor_candidates: [u8; SOURCE_LIMIT],
    retained: [RetainedReview; PLAN_LIMIT],
    retained_store: Option<crate::device_core::StoreIdentity>,
    next: u8,
    context: Option<ReviewContext>,
    route: Option<u64>,
    remaining_m: Option<u32>,
    remaining_ascent: Option<u32>,
    clock: (bool, i16),
    profile: u8,
    local: Option<(u8, u16)>,
    selected_review: bool,
    invalid_visit: Option<obc_formats::assistant::PayloadFingerprint>,
    pub(crate) resume_offer: bool,
    /// The catalog row the standing checkpoint names, so the Resume card can say which route.
    pub(crate) resume_route: Option<u8>,
    pub review: ReviewStatus,
    pub review_costs: Option<Costs>,
}
impl FindState {
    pub const fn new() -> Self {
        Self {
            state: State::Idle,
            action: Action::None,
            category: PoiCategory::Water,
            limit: 4,
            hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
            map: None,
            bound_map: None,
            origin: (0, 0),
            results: heapless::Vec::new(),
            places: heapless::Vec::new(),
            costs: [None; PLAN_LIMIT],
            corridor_candidates: [0, 1, 2, 3, 4, 5],
            retained: [RetainedReview::NONE; PLAN_LIMIT],
            retained_store: None,
            next: 0,
            context: None,
            route: None,
            remaining_m: None,
            remaining_ascent: None,
            clock: (false, 0),
            profile: 0,
            local: None,
            selected_review: false,
            invalid_visit: None,
            resume_offer: false,
            resume_route: None,
            review: ReviewStatus::Idle,
            review_costs: None,
        }
    }
    pub(crate) fn owns_pages(&self) -> bool {
        self.state != State::Idle
    }
    pub fn costs(&self, i: usize) -> Option<Costs> {
        self.results.get(i).and_then(|i| self.costs[*i as usize])
    }
    pub fn candidate<'a>(
        &self,
        id: u8,
        nearby: &'a crate::screen::PoiScratch,
        corridor: &'a [obc_reader::CorridorPoi],
    ) -> Option<&'a Poi> {
        let i = id as usize / 2;
        if i >= self.limit {
            return None;
        }
        if id.is_multiple_of(2) { nearby.pois.get(i) } else { corridor.get(*self.corridor_candidates.get(i)? as usize) }
            .map(|hit| &hit.poi)
    }
    pub fn selected<'a>(
        &'a self,
        i: usize,
        nearby: &'a crate::screen::PoiScratch,
        corridor: &'a [obc_reader::CorridorPoi],
    ) -> Option<&'a Poi> {
        self.places.get(i).or_else(|| self.candidate(*self.results.get(i)?, nearby, corridor))
    }
    fn rank(&mut self) {
        self.results.clear();
        let mut on_way = heapless::Vec::<u8, RESULT_LIMIT>::new();
        let mut alternatives = heapless::Vec::<u8, RESULT_LIMIT>::new();
        for (i, value) in self.costs.iter().enumerate() {
            let Some(cost) = value else { continue };
            let list = if cost.on_way() {
                &mut on_way
            } else {
                if self.costs.iter().flatten().any(|other| other.on_way() && other.dominates(*cost)) {
                    continue;
                }
                &mut alternatives
            };
            let key = |id: u8| {
                let c = self.costs[id as usize].unwrap();
                (c.arrival_m, c.arrival_ascent_m.unwrap_or(u32::MAX), id)
            };
            let at = list.iter().position(|id| key(i as u8) < key(*id)).unwrap_or(list.len());
            if at < self.limit {
                if list.is_full() {
                    list.pop();
                }
                let _ = list.insert(at, i as u8);
            }
        }
        let slots = self.limit - usize::from(!alternatives.is_empty());
        for id in on_way.iter().take(slots).chain(alternatives.iter()).take(self.limit) {
            let _ = self.results.push(*id);
        }
    }
}
impl Default for FindState {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::App {
    /// The host binds the exact map object used by its reader and planner.
    pub fn bind_place_map(&mut self, map: Option<RouteSourceKey>) {
        if self.ui.find.bound_map != map && self.ui.stack.iter().any(|s| matches!(s, Screen::WhatsNext(_))) {
            self.ui.ahead.invalidate();
            self.ui.map_dirty = true;
        }
        if self.ui.find.bound_map.is_some() && self.ui.find.bound_map != map {
            self.ui.poi_scratch.cancel();
            self.ui.landmarks.invalidate();
            self.state.peak_view_peak_count = 0;
            for screen in &mut self.ui.stack {
                if let Screen::PeakView(peak) = screen {
                    peak.invalidate_map();
                }
            }
            for screen in &mut self.ui.stack {
                if let Screen::LandmarkPhoto(photo) = screen {
                    photo.invalidate_source();
                }
            }
            self.ui.corridor_scratch.cancel();
            for screen in &mut self.ui.stack {
                if let Screen::PoiDetail(detail) = screen {
                    detail.invalidate_source();
                }
            }
            if self.ui.find.owns_pages() || self.ui.stack.iter().any(|s| matches!(s, Screen::VisitReview(_))) {
                self.cancel_assistant();
            }
        }
        self.ui.find.bound_map = map;
        if self.ui.find.owns_pages() && self.ui.find.map.is_some() && self.ui.find.map != map {
            self.ui.find.state = State::Stale;
            self.cancel_assistant();
            self.ui.map_dirty = true;
        }
    }
    pub(crate) fn current_visit_index(&self) -> Option<usize> {
        if !self.active_visit() || self.assistant_review_status() != ReviewStatus::Accepted {
            return None;
        }
        let source = self.assistant_checkpoint()?.route;
        if self.ui.find.invalid_visit == Some(source) {
            return None;
        }
        let index = self.active_route_index()?;
        (self.route_ids().get(index).copied() == Some(source.object)).then_some(index)
    }
    pub(crate) fn invalidate_current_visit(&mut self, id: crate::CatalogObjectId) {
        if let Some(source) = self.assistant_checkpoint().map(|c| c.route).filter(|s| s.object == id) {
            self.ui.find.invalid_visit = Some(source);
        }
    }
    pub(crate) fn place_map_key(&self) -> Option<RouteSourceKey> {
        self.ui.find.bound_map
    }
    pub fn open_find_place(&mut self) {
        crate::screen::apply(
            &mut self.ui.stack,
            crate::screen::Transition::Push(Screen::FindPlace(FindPlaceScreen::new())),
        );
        self.ui.map_dirty = true;
    }
    pub fn find_place_state(&self) -> State {
        self.ui.find.state
    }
    pub fn find_place_result_count(&self) -> usize {
        self.ui.find.results.len()
    }
    pub fn retains_find_review(&self, source: RouteSourceKey) -> bool {
        self.ui.find.retained_store.is_some_and(|store| {
            self.ui.find.retained.iter().any(|retained| retained.object != 0 && retained.source(store) == source)
        })
    }
    pub(crate) fn find_review_removed(&mut self, source: RouteSourceKey) {
        if let Some(store) = self.ui.find.retained_store {
            for retained in &mut self.ui.find.retained {
                if retained.source(store) == source {
                    *retained = RetainedReview::NONE;
                }
            }
            if self.ui.find.retained.iter().all(|retained| retained.object == 0) {
                self.ui.find.retained_store = None;
            }
        }
    }
    /// A recovered, idle session can reclaim unaccepted candidates found by the catalog reader.
    pub fn can_reconcile_reviews(&self) -> bool {
        matches!(self.ui.find.state, State::Idle | State::Start)
            && !self.ui.find.selected_review
            && self.assistant_planner_released()
            && !self.assistant_needs_recovery()
            && matches!(
                self.assistant_review_status(),
                ReviewStatus::Idle | ReviewStatus::ResumeAvailable | ReviewStatus::Accepted
            )
            && self.ui.find.retained.iter().any(|retained| retained.object == 0)
    }
    /// The feeder supplies only validated, unaccepted Assistant candidates from this card.
    pub fn reconcile_review_candidate(&mut self, source: RouteSourceKey) {
        let store = crate::device_core::StoreIdentity::from_bytes(source.store);
        if !self.can_reconcile_reviews()
            || !self.assistant_store_matches(store)
            || self.ui.find.retained_store.is_some_and(|retained_store| retained_store != store)
            || self.retains_find_review(source)
            || self.active_route_index().and_then(|index| self.route_ids().get(index)).copied() == Some(source.object)
            || self.assistant_checkpoint().is_some_and(|checkpoint| {
                [Some(checkpoint.route), checkpoint.original]
                    .into_iter()
                    .flatten()
                    .any(|protected| protected.object == source.object && protected.revision == source.revision)
            })
        {
            return;
        }
        if let Some(retained) = self.ui.find.retained.iter_mut().find(|retained| retained.object == 0) {
            *retained = RetainedReview { object: source.object, revision: source.revision, rejoin_m: 0 };
            self.ui.find.retained_store = Some(store);
            self.ui.map_dirty = true;
            self.ui.next_wake_ms = Some(1);
        }
    }

    fn cleanup_find_reviews(&mut self) {
        let Some(store) = self.ui.find.retained_store else { return };
        let accepted = self.assistant_checkpoint().filter(|_| {
            self.assistant_review_status() == ReviewStatus::Accepted && self.assistant_store_matches(store)
        });
        if let Some(checkpoint) = accepted {
            self.find_review_removed(RouteSourceKey {
                store: store.bytes(),
                object: checkpoint.route.object,
                revision: checkpoint.route.revision,
            });
        }
        if self.ui.find.selected_review
            || matches!(self.assistant_review_status(), ReviewStatus::Saving | ReviewStatus::Unresolved)
            || matches!(self.ui.find.state, State::Querying | State::Planning | State::Releasing)
            || !self.catalogs.can_admit_intent()
        {
            return;
        }
        let keep_choices = self.ui.find.state == State::Ready;
        if let Some(retained) = self.ui.find.retained.iter().enumerate().find_map(|(index, retained)| {
            (retained.object != 0 && !(keep_choices && self.ui.find.results.contains(&(index as u8))))
                .then_some(*retained)
        }) {
            let _ = self
                .catalogs
                .admit_intent(crate::catalog_state::CatalogIntent::RemoveReview { source: retained.source(store) });
            self.ui.next_wake_ms = Some(1);
        }
    }
    pub(crate) fn sync_find_preferences(&mut self) {
        let limit = self.settings().find_results.limit();
        let hours_filter = self.settings().find_hours_filter();
        if (self.ui.find.limit, self.ui.find.hours_filter) == (limit, hours_filter) {
            return;
        }
        if self.ui.find.owns_pages() {
            self.cancel_assistant();
            self.ui.corridor_scratch.disarm();
        }
        self.ui.find.limit = limit;
        self.ui.find.hours_filter = hours_filter;
        self.ui.find.state = State::Idle;
        self.ui.find.action = Action::None;
        self.ui.find.results.clear();
        self.ui.find.places.clear();
        self.ui.poi_scratch.invalidate();
        self.ui.poi_scratch.hours_filter = hours_filter;
        for screen in &mut self.ui.stack {
            match screen {
                Screen::FindPlace(s) => s.refresh_selection(),
                Screen::PoiList(s) => s.refresh(),
                _ => {}
            }
        }
    }

    pub(crate) fn handle_find_action(&mut self) {
        if matches!(self.ui.find.action, Action::CancelVisit | Action::Resume | Action::Preview(_)) {
            return;
        }
        match core::mem::replace(&mut self.ui.find.action, Action::None) {
            Action::Refresh => {
                self.cancel_assistant();
                self.ui.find.state = State::Start;
                self.ui.find.results.clear();
                self.ui.find.places.clear();
                self.ui.find.costs.fill(None);
                self.ui.find.context = None;
                self.ui.find.next = 0;
                self.ui.poi_scratch.invalidate();
                self.ui.corridor_scratch.disarm();
            }
            Action::More => {
                self.cancel_assistant();
                if self.ui.find.state != State::Ready {
                    self.ui.find.state = State::Stale;
                }
                self.ui.poi_scratch.invalidate();
                self.ui.corridor_scratch.disarm();
                crate::screen::apply(
                    &mut self.ui.stack,
                    crate::screen::Transition::Push(Screen::PoiList(crate::screen::PoiListScreen::new(
                        self.ui.find.category,
                    ))),
                );
            }
            Action::Accept => {
                if self
                    .assistant_review_context()
                    .is_some_and(|c| c.purpose == crate::navigator::ReviewPurpose::ReturnToRoute)
                    || self.ui.poi_scratch.detail_valid
                {
                    if let Some(origin) = self.current_review_origin() {
                        self.accept_assistant(origin);
                    }
                }
            }
            Action::RouteMode(destination) => {
                if self.assistant_review_status() == ReviewStatus::Preview && self.assistant_planner_released() {
                    if let Some(target) = self.assistant_visit_target() {
                        self.cancel_assistant();
                        if let Some(Screen::VisitReview(screen)) = self.ui.stack.last_mut() {
                            screen.destination = destination;
                            screen.pending_target = Some(target);
                        }
                        self.ui.find.review_costs = None;
                        self.ui.find.review = ReviewStatus::Planning;
                    }
                }
            }
            Action::OpenAccepted => {
                if let Some(index) = self.current_visit_index() {
                    let screen = VisitReviewScreen::accepted(self.routes()[index].name.as_str());
                    self.ui.find.selected_review = false;
                    self.ui.find.review_costs = None;
                    crate::screen::apply(
                        &mut self.ui.stack,
                        crate::screen::Transition::Push(Screen::VisitReview(screen)),
                    );
                }
            }
            Action::CancelVisit | Action::Resume | Action::Preview(_) => return,
            Action::DismissArrival => self.dismiss_visit_arrival(),
            Action::DismissResume => {
                self.ui.find.resume_offer = false;
            }
            Action::Cancel => self.cancel_assistant(),
            Action::None => {}
        }
        self.handle_find_exit();
        self.ui.map_dirty = true;
    }
    fn handle_find_exit(&mut self) {
        if self.ui.find.selected_review && !self.ui.stack.iter().any(|s| matches!(s, Screen::VisitReview(_))) {
            self.ui.find.selected_review = false;
            if !matches!(
                self.assistant_review_status(),
                ReviewStatus::Accepted | ReviewStatus::Saving | ReviewStatus::Unresolved
            ) {
                self.cancel_assistant();
            }
        }
        let has_find = self.ui.stack.iter().any(|s| match s {
            Screen::Assistant(_) | Screen::FindPlace(_) if self.ui.find.state == State::Ready => true,
            Screen::FindPlace(s) => s.choices(),
            _ => false,
        });
        if self.ui.find.owns_pages() && !has_find {
            if matches!(self.ui.find.state, State::Planning | State::Releasing) {
                self.cancel_assistant();
            }
            self.ui.find.state = State::Idle;
            self.ui.find.places.clear();
            self.ui.corridor_scratch.disarm();
        }
    }
    pub(crate) fn activate_place_detail(&mut self) -> bool {
        let Some(Screen::PoiDetail(detail)) = self.ui.stack.last() else { return false };
        let poi = detail.poi().clone();
        if detail.visit_error == Some(crate::navigator::VisitUnavailable::SourceChanged)
            || (detail.is_landmark() && poi.metadata.approach.is_none())
            || detail.hours_pending(&self.ui.poi_scratch)
            || !self.ui.poi_scratch.detail_valid
        {
            return true;
        }
        let Some(map) = self.ui.find.bound_map else { return true };
        let name = if poi.name.is_empty() {
            obc_formats::obcm::poi_label_of(poi.subtype).unwrap_or("Place")
        } else {
            poi.name.as_str()
        };
        match self.request_visit(VisitTarget { map, metadata: poi.metadata, display: (poi.lon, poi.lat) }, name) {
            Ok(()) => {
                self.ui.find.selected_review = true;
                self.ui.find.review_costs = None;
                self.ui.find.review = ReviewStatus::Planning;
                let screen = VisitReviewScreen::new(name).route_choices(self.active_route_index().is_some());
                crate::screen::apply(&mut self.ui.stack, crate::screen::Transition::Push(Screen::VisitReview(screen)));
                self.ui.map_dirty = true;
                true
            }
            Err(error) => {
                if let Some(Screen::PoiDetail(detail)) = self.ui.stack.last_mut() {
                    detail.visit_error = Some(error);
                }
                self.ui.map_dirty = true;
                true
            }
        }
    }
    fn preview_find_result(&mut self, reader: &Reader, selected: usize) {
        if !self.catalogs.can_admit_intent() {
            self.ui.next_wake_ms = Some(1);
            return;
        }
        self.ui.find.action = Action::None;
        if self.ui.find.state != State::Ready
            || !matches!(self.ui.stack.last(), Some(Screen::FindPlace(screen)) if screen.choices())
        {
            return;
        }
        let local = self.place_local_time();
        if self.ui.find.map != self.place_map_key()
            || self.ui.find.profile != self.settings().bike_profile_idx
            || self.ui.find.clock != (local.is_some(), self.settings().utc_offset_min)
            || self.ui.find.route != self.active_route_index().and_then(|i| self.route_ids().get(i)).copied()
        {
            self.ui.find.state = State::Stale;
            self.cancel_assistant();
            return;
        }
        let Some(poi) =
            self.ui.find.selected(selected, &self.ui.poi_scratch, self.ui.corridor_scratch.entries()).cloned()
        else {
            return;
        };
        let schedule = reader.try_poi_hours(poi.hours_ref);
        self.ui.poi_scratch.detail_source = poi.metadata.source.0;
        self.ui.poi_scratch.detail_valid = schedule.is_ok();
        self.ui.poi_scratch.detail_schedule = schedule.ok().flatten();
        if !self.ui.poi_scratch.detail_valid {
            return;
        }
        let Some(map) = self.ui.find.map else { return };
        let name = if poi.name.is_empty() {
            obc_formats::obcm::poi_label_of(poi.subtype).unwrap_or("Place")
        } else {
            poi.name.as_str()
        };
        let retained = self.ui.find.results.get(selected).map(|index| self.ui.find.retained[*index as usize]);
        let error = match retained.zip(self.ui.find.context).filter(|(retained, _)| retained.object != 0) {
            Some((retained, mut context)) => {
                context.required_anchors_m[1] = retained.rejoin_m;
                context.required_anchors_m[2] = retained.rejoin_m;
                if self
                    .current_review_origin()
                    .is_none_or(|origin| !context.accepts_origin(self.settings().bike_profile_idx, origin))
                {
                    Some(crate::navigator::VisitUnavailable::Unmatched)
                } else if self.restore_visit(
                    VisitTarget { map, metadata: poi.metadata, display: (poi.lon, poi.lat) },
                    context,
                    retained.source(context.store),
                ) {
                    None
                } else {
                    Some(crate::navigator::VisitUnavailable::Busy)
                }
            }
            None => Some(crate::navigator::VisitUnavailable::SourceChanged),
        };
        self.ui.find.selected_review = true;
        self.ui.find.review_costs = None;
        self.ui.find.review = self.assistant_review_status();
        let mut screen = VisitReviewScreen::new(name).route_choices(self.active_route_index().is_some());
        screen.error = error;
        crate::screen::apply(&mut self.ui.stack, crate::screen::Transition::Push(Screen::VisitReview(screen)));
        self.ui.map_dirty = true;
    }
    /// Advance place queries and candidate ownership while streamed readers are available.
    pub fn prepare_find(&mut self, reader: Option<&Reader>, route: Option<&RouteReader>) {
        self.sync_find_preferences();
        if self.ui.find.state == State::Idle
            && matches!(self.ui.stack.iter().rev().find(|s| !s.is_overlay()), Some(Screen::FindPlace(s)) if s.choices())
        {
            self.ui.find.action = Action::Refresh;
            self.handle_find_action();
        }
        let local = self.place_local_time();
        if let Some(Screen::VisitReview(screen)) = self.ui.stack.last() {
            if let Some(target) = screen.pending_target {
                let status = self.assistant_review_status();
                if matches!(status, ReviewStatus::Failed(_) | ReviewStatus::Unresolved | ReviewStatus::ResumeAvailable)
                {
                    if let Some(Screen::VisitReview(screen)) = self.ui.stack.last_mut() {
                        screen.pending_target = None;
                    }
                    self.ui.find.review = status;
                    return;
                }
                if !self.assistant_planner_released()
                    || !self.catalogs.can_admit_intent()
                    || !matches!(status, ReviewStatus::Idle | ReviewStatus::Accepted)
                {
                    return;
                }
                let destination = screen.destination;
                let result = if destination {
                    self.request_destination(target, "Route to place")
                } else {
                    self.request_visit(target, "Visit")
                };
                if let Some(Screen::VisitReview(screen)) = self.ui.stack.last_mut() {
                    screen.pending_target = None;
                    screen.error = result.err();
                }
            }
        }
        if let Action::Preview(selected) = self.ui.find.action {
            if let Some(reader) = reader {
                self.preview_find_result(reader, selected as usize);
            }
        }
        if self.ui.find.action == Action::CancelVisit {
            self.ui.find.action = Action::None;
            let result = self
                .current_visit_index()
                .ok_or(crate::navigator::VisitUnavailable::SourceChanged)
                .and_then(|_| route.zip(self.place_map_key()).ok_or(crate::navigator::VisitUnavailable::SourceChanged))
                .and_then(|(route, map)| self.cancel_visit(route, map));
            if let Some(Screen::VisitReview(screen)) = self.ui.stack.last_mut() {
                screen.error = result.err();
                if result.is_ok() {
                    screen.accepted = false;
                    screen.returning = true;
                    self.ui.find.selected_review = true;
                    self.ui.find.review_costs = None;
                }
            }
        }
        self.handle_find_exit();
        self.cleanup_find_reviews();
        self.ui.find.review = self.assistant_review_status();
        if self.assistant_review_status() == ReviewStatus::Accepted && self.active_visit() {
            if let Some(Screen::VisitReview(screen)) = self.ui.stack.last_mut() {
                if screen.returning && !screen.accepted {
                    screen.accepted = true;
                    screen.error = Some(crate::navigator::VisitUnavailable::Busy);
                    self.ui.find.selected_review = false;
                }
            }
        }
        let base = self.ui.stack.iter().rev().find(|s| !s.is_overlay());
        if let Some(Screen::VisitReview(screen)) = base {
            if screen.accepted {
                let current = self.current_visit_index().and_then(|_| self.assistant_checkpoint());
                self.ui.find.review = if current.is_some() {
                    ReviewStatus::Accepted
                } else {
                    ReviewStatus::Failed(crate::navigator::NavigatorError::SourceChanged)
                };
                self.ui.find.review_costs = current.map(|c| Costs {
                    arrival_m: c.upper_m.saturating_sub(self.progress_m()),
                    arrival_ascent_m: None,
                    added_m: None,
                    added_ascent_m: None,
                });
                return;
            }
            if self.assistant_review_status() == ReviewStatus::Accepted
                || (screen.returning && self.assistant_review_status() == ReviewStatus::Idle)
            {
                crate::screen::apply(
                    &mut self.ui.stack,
                    crate::screen::Transition::Root(Screen::Map(crate::screen::MapScreen::new())),
                );
                self.ui.find.state = State::Idle;
                self.ui.map_dirty = true;
                return;
            }
            self.ui.find.review = self.assistant_review_status();
            // Exact candidate metrics are copied before cancellation retracts its publication.
            if self.ui.find.review_costs.is_none() && self.assistant_preview().is_some() {
                if let Some(context) = self.assistant_review_context() {
                    self.capture_place_baseline(route, context.progress_m);
                }
                self.ui.find.review_costs = self.measured_place_costs();
            }
            return;
        }
        if matches!(
            self.ui.find.state,
            State::Idle | State::NoFix | State::NoMap | State::NoAccess | State::Failed | State::Stale
        ) {
            return;
        }
        if self.ui.find.state == State::Start {
            if self.ui.find.retained.iter().any(|retained| retained.object != 0)
                || !self.assistant_planner_released()
                || !self.catalogs.can_admit_intent()
            {
                return;
            }
            let Some(fix) = self.fresh_position() else {
                self.ui.find.state = State::NoFix;
                return;
            };
            let Some(map) = self.ui.find.bound_map else {
                self.ui.find.state = State::NoMap;
                return;
            };
            let Some(reader) = reader else { return };
            if reader.nav_directory().is_empty() {
                self.ui.find.state = State::NoAccess;
                return;
            }
            let progress = self.navigator.route_state().progress_m;
            self.ui.find.map = Some(map);
            self.ui.find.origin = (fix.lon, fix.lat);
            self.ui.find.route = self.active_route_index().and_then(|i| self.route_ids().get(i)).copied();
            self.ui.find.remaining_m = route.map(|r| r.total_distance_m.saturating_sub(progress));
            self.ui.find.remaining_ascent = route
                .and_then(|r| r.interval_facts(progress, r.total_distance_m).ok())
                .filter(|f| f.complete_elevation())
                .map(|f| f.ascent_m);
            self.ui.find.clock = (local.is_some(), self.settings().utc_offset_min);
            self.ui.find.profile = self.settings().bike_profile_idx;
            self.ui.find.local = local;
            self.ui.find.limit = self.settings().find_results.limit();
            self.ui.find.hours_filter = self.settings().find_hours_filter();
            let scratch = &mut self.ui.poi_scratch;
            scratch.query = Some(
                PlaceQuery::new(
                    scratch.generation,
                    PoiCategorySet::only(self.ui.find.category),
                    PlaceWindow::Nearby { position: self.ui.find.origin, radius_m: NEARBY_M },
                    local,
                )
                .with_hours_filter(self.ui.find.hours_filter),
            );
            scratch.status = QueryProgress::Pending;
            if route.is_some() {
                self.ui.corridor_scratch.arm(crate::corridor::CorridorKey {
                    hours_filter: self.ui.find.hours_filter,
                    filter: PoiCategorySet::only(self.ui.find.category),
                    anchor_m: progress,
                });
            }
            self.ui.find.state = State::Querying;
        }
        if self.ui.find.profile != self.settings().bike_profile_idx
            || self.ui.find.clock != (local.is_some(), self.settings().utc_offset_min)
            || self.ui.find.route != self.active_route_index().and_then(|i| self.route_ids().get(i)).copied()
        {
            self.ui.find.state = State::Stale;
            self.cancel_assistant();
            return;
        }
        if self.ui.find.state == State::Ready && !self.ui.find.places.is_empty() {
            if self.ui.find.context.is_some_and(|context| {
                self.current_review_origin()
                    .is_none_or(|origin| !context.accepts_origin(self.settings().bike_profile_idx, origin))
            }) {
                self.ui.find.state = State::Stale;
                self.cancel_assistant();
                return;
            }
            if self.ui.find.local != local {
                let Some(reader) = reader else { return };
                for poi in &mut self.ui.find.places {
                    let Ok(schedule) = reader.try_poi_hours(poi.hours_ref) else {
                        self.ui.find.state = State::Failed;
                        self.ui.find.results.clear();
                        return;
                    };
                    poi.opening = schedule.map_or(OpeningStatus::Unknown, |schedule| schedule.status(local));
                }
                self.ui.find.local = local;
            }
            return;
        }
        if self.ui.find.local != local {
            let Some(reader) = reader else { return };
            if reader.refresh_place_hours(&mut self.ui.poi_scratch.pois, local).is_err() {
                self.ui.find.state = State::Failed;
                self.cancel_assistant();
                return;
            }
            self.ui.corridor_scratch.clock_changed(local, self.settings().utc_offset_min);
            self.ui.find.local = local;
            let end = self.ui.corridor_scratch.armed().map_or(0, |key| key.anchor_m.saturating_add(FORWARD_M));
            self.ui.corridor_scratch.prepare_to(Some(reader), route, local, end);
            if matches!(self.ui.corridor_scratch.status(), QueryProgress::Failed(_)) {
                self.ui.find.state = State::Failed;
                self.ui.find.results.clear();
                self.cancel_assistant();
                return;
            }
        }
        if self.ui.find.state == State::Querying {
            let Some(reader) = reader else { return };
            let scratch = &mut self.ui.poi_scratch;
            if let Some(query) = &mut scratch.query {
                for _ in 0..QUERY_STEPS {
                    if scratch.status != QueryProgress::Pending {
                        break;
                    }
                    scratch.status = query.step(reader, None, scratch.generation, &mut scratch.pois);
                }
            }
            let end = self.ui.corridor_scratch.armed().map_or(0, |key| key.anchor_m.saturating_add(FORWARD_M));
            self.ui.corridor_scratch.prepare_to(Some(reader), route, local, end);
            if matches!(scratch.status, QueryProgress::Failed(_) | QueryProgress::Unavailable)
                || matches!(self.ui.corridor_scratch.status(), QueryProgress::Failed(_))
            {
                self.ui.find.state = State::Failed;
                return;
            }
            if matches!(scratch.status, QueryProgress::Ready { .. }) && !self.ui.corridor_scratch.pending() {
                if scratch.pois.is_empty() && self.ui.corridor_scratch.entries().is_empty() {
                    self.ui.find.state = State::Empty;
                    return;
                }
                if let (Some(route), Some(key), Some(map)) = (route, self.ui.corridor_scratch.armed(), self.ui.find.map)
                {
                    self.ui.find.corridor_candidates = corridor_candidates(
                        route,
                        self.ui.corridor_scratch.entries(),
                        key.anchor_m,
                        map,
                        self.ui.find.profile,
                    );
                }
                self.ui.find.state = State::Planning;
            } else {
                self.ui.map_dirty = true;
                self.ui.next_wake_ms = Some(1);
                return;
            }
        }
        if self.ui.find.state == State::Releasing {
            if !self.assistant_planner_released()
                || !matches!(self.assistant_review_status(), ReviewStatus::Idle | ReviewStatus::Accepted)
            {
                return;
            }
            self.ui.find.next += 1;
            self.ui.find.state = State::Planning;
        }
        if self.ui.find.state != State::Planning {
            return;
        }
        if let Some(context) = self.ui.find.context {
            if self
                .current_review_origin()
                .is_none_or(|origin| !context.accepts_origin(self.settings().bike_profile_idx, origin))
            {
                self.ui.find.state = State::Stale;
                self.cancel_assistant();
                return;
            }
        }
        if self.assistant_review_status() == ReviewStatus::Preview {
            if let Some(context) = self.assistant_review_context() {
                if self.ui.find.context.is_some_and(|old| {
                    ReviewContext { required_anchors_m: context.required_anchors_m, ..old } != context
                }) {
                    self.ui.find.state = State::Stale;
                    self.cancel_assistant();
                    return;
                }
                self.ui.find.context = Some(context);
            }
            self.ui.find.costs[self.ui.find.next as usize] = self.measured_place_costs();
            if let (Some(preview), Some(context)) = (self.assistant_preview(), self.assistant_review_context()) {
                self.ui.find.retained_store = Some(context.store);
                self.ui.find.retained[self.ui.find.next as usize] = RetainedReview {
                    object: preview.source.object,
                    revision: preview.source.revision,
                    rejoin_m: context.required_anchors_m[2],
                };
            }
            self.cancel_assistant();
            self.ui.find.state = State::Releasing;
            return;
        }
        if let ReviewStatus::Failed(error) = self.assistant_review_status() {
            self.cancel_assistant();
            self.ui.find.state = match error {
                crate::navigator::NavigatorError::Plan(_) | crate::navigator::NavigatorError::Unavailable => {
                    State::Releasing
                }
                crate::navigator::NavigatorError::Movement | crate::navigator::NavigatorError::SourceChanged => {
                    State::Stale
                }
                _ => State::Failed,
            };
            return;
        }
        if !self.assistant_planner_released() || self.assistant_review_status() == ReviewStatus::Planning {
            return;
        }
        while (self.ui.find.next as usize) < PLAN_LIMIT {
            let i = self.ui.find.next;
            let poi = self.ui.find.candidate(i, &self.ui.poi_scratch, self.ui.corridor_scratch.entries()).cloned();
            let Some(poi) = poi.filter(|p| self.ui.find.hours_filter.includes(p.opening)) else {
                self.ui.find.next += 1;
                continue;
            };
            if (0..i).any(|n| {
                self.ui
                    .find
                    .candidate(n, &self.ui.poi_scratch, self.ui.corridor_scratch.entries())
                    .is_some_and(|other| other.metadata.source == poi.metadata.source)
            }) {
                self.ui.find.next += 1;
                continue;
            }
            let target =
                VisitTarget { map: self.ui.find.map.unwrap(), metadata: poi.metadata, display: (poi.lon, poi.lat) };
            let result = if let Some(context) = self.ui.find.context {
                self.plan_visit(target, context)
            } else {
                self.request_visit(target, &poi.name).is_ok()
            };
            if result {
                if self.ui.find.context.is_none() {
                    if let Some(context) = self.assistant_review_context() {
                        self.ui.find.origin = context.origin;
                        self.capture_place_baseline(route, context.progress_m);
                    }
                }
                return;
            }
            self.ui.find.next += 1;
        }
        for i in 0..PLAN_LIMIT {
            if self
                .ui
                .find
                .candidate(i as u8, &self.ui.poi_scratch, self.ui.corridor_scratch.entries())
                .is_none_or(|p| !self.ui.find.hours_filter.includes(p.opening))
            {
                self.ui.find.costs[i] = None;
            }
        }
        self.ui.find.rank();
        for &id in &self.ui.find.results {
            if let Some(poi) = self.ui.find.candidate(id, &self.ui.poi_scratch, self.ui.corridor_scratch.entries()) {
                let poi = poi.clone();
                let _ = self.ui.find.places.push(poi);
            }
        }
        self.ui.find.state = State::Ready;
        self.ui.map_dirty = true;
    }
    fn capture_place_baseline(&mut self, route: Option<&RouteReader>, progress: u32) {
        self.ui.find.remaining_m = route.map(|r| r.total_distance_m.saturating_sub(progress));
        self.ui.find.remaining_ascent = route
            .and_then(|r| r.interval_facts(progress, r.total_distance_m).ok())
            .filter(|f| f.complete_elevation())
            .map(|f| f.ascent_m);
    }
    fn measured_place_costs(&self) -> Option<Costs> {
        let p = self.assistant_preview()?;
        let context = self.assistant_review_context()?;
        let arrival_m = p.visit_anchors_m.map_or(p.distance_m, |a| a[1].saturating_sub(a[0]));
        let facts = self.assistant_visit_costs()?;
        let original = context.original.filter(|_| context.purpose != crate::navigator::ReviewPurpose::Destination);
        Some(Costs {
            arrival_m,
            arrival_ascent_m: facts.arrival_elevation_complete.then_some(facts.arrival_ascent_m),
            added_m: original.and(self.ui.find.remaining_m).map(|m| p.distance_m.saturating_sub(m)),
            added_ascent_m: original
                .and(self.ui.find.remaining_ascent)
                .filter(|_| facts.complete_elevation)
                .map(|a| p.ascent_m.saturating_sub(a)),
        })
    }
}

/// Rank the completed Find page by the same future occurrence the visit planner will use.
/// Keys are read once; the shared corridor page keeps its chronological order.
fn corridor_candidates(
    route: &RouteReader,
    places: &[obc_reader::CorridorPoi],
    progress_m: u32,
    map: RouteSourceKey,
    profile: u8,
) -> [u8; SOURCE_LIMIT] {
    let mut ranked = [(u32::MAX, u8::MAX); obc_reader::reader::places::PLACE_PAGE_SIZE];
    for (i, place) in places.iter().take(ranked.len()).enumerate() {
        let poi = &place.poi;
        let target = VisitTarget { map, metadata: poi.metadata, display: (poi.lon, poi.lat) };
        let estimate = target.approach(map, profile).and_then(|point| {
            let anchor = obc_route::visit::visit_anchor(route, progress_m, point).ok()?;
            let at = route.position_at(anchor)?;
            Some(
                anchor
                    .saturating_sub(progress_m)
                    .saturating_add(obc_map_scene::ground_dist_m((at.lon, at.lat), point) as u32),
            )
        });
        if let Some(estimate) = estimate {
            ranked[i] = (estimate, i as u8);
        }
    }
    ranked.sort_unstable();
    core::array::from_fn(|i| ranked[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::{ByteSink, ByteSource, Error, SliceSource};
    use obc_reader::{MapCache, MapTables};

    struct Sink(std::vec::Vec<u8>);
    impl ByteSink for Sink {
        fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
            self.0.extend_from_slice(bytes);
            Ok(())
        }
        fn patch_at(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Error> {
            self.0[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
            Ok(())
        }
    }

    #[test]
    fn mode_change_surfaces_release_failure_without_starting_another_route() {
        use crate::navigator::NavigatorError;
        for error in [NavigatorError::Store, NavigatorError::DurabilityUnknown] {
            let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
            let mut screen = VisitReviewScreen::new("Water").route_choices(true);
            screen.pending_target = Some(VisitTarget {
                map: RouteSourceKey { store: [1; 16], object: 1, revision: 1 },
                metadata: obc_formats::obcm::PoiMetadata {
                    source: obc_formats::obcm::SourceId::osm(1, 1),
                    approach: None,
                },
                display: (0, 0),
            });
            assert!(app.ui.stack.push(Screen::VisitReview(screen)).is_ok());
            app.ui.find.review = ReviewStatus::Planning;
            app.advance_animations(obc_ports::InputClock(0));
            assert!(app.assistant_route_pending());
            assert_eq!(app.ms_until_next_wake(0), Some(1));
            assert!(app.reroute_banner_rows(320.0).is_some());
            assert!(!app.reroute_freeze_active(), "waiting for release does not claim the planner arena");
            app.navigator.review_failed(error);
            let expected = app.assistant_review_status();
            app.prepare_find(None, None);
            assert_eq!(app.ui.find.review, expected);
            assert!(!app.assistant_route_pending());
            assert!(app.reroute_banner_rows(320.0).is_none());
            assert!(matches!(app.top_screen(), Screen::VisitReview(screen) if screen.pending_target.is_none()));
            assert!(app.assistant_planner_released());
            assert!(app.assistant_review_context().is_none());
        }
    }

    #[test]
    fn find_shortlist_uses_visit_occurrence_without_reordering_corridor() {
        use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
        let mut sink = Sink(std::vec::Vec::new());
        obc_route::gpx_to_obcr(
            &SliceSource(
                br#"<gpx><trk><trkseg>
                <trkpt lon="0" lat="0"/><trkpt lon="0.01" lat="0"/>
                <trkpt lon="0.01" lat="0.01"/><trkpt lon="0" lat="0.01"/>
                <trkpt lon="0" lat="0.001"/><trkpt lon="0.01" lat="0.001"/>
                </trkseg></trk></gpx>"#,
            ),
            "Loop",
            &mut sink,
        )
        .unwrap();
        let source = SliceSource(&sink.0);
        let index = obc_route::RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let map = RouteSourceKey { store: [1; 16], object: 1, revision: 1 };
        let points = [(4000, 1000), (6000, 0), (8000, 0), (10_000, 2000), (10_000, 4000)];
        let mut places: std::vec::Vec<_> = points
            .into_iter()
            .enumerate()
            .map(|(i, (lon, lat))| obc_reader::CorridorPoi {
                poi: Poi {
                    opening: OpeningStatus::Unknown,
                    metadata: PoiMetadata { source: SourceId::osm(1, i as u64 + 1), approach: None },
                    lat,
                    lon,
                    subtype: 1,
                    name: Default::default(),
                    hours_ref: 0,
                    distance_m: i as u32,
                },
                dist_along_m: i as u32,
                offset_m: 0,
            })
            .collect();
        // The first corridor hit is near the outbound pass, but the planner visits it on the return.
        assert!(obc_route::visit::visit_anchor(&route, 0, points[0]).unwrap() > 4000);
        assert_eq!(corridor_candidates(&route, &places, 0, map, 0), [1, 2, 3, 4, 0, u8::MAX]);
        assert_eq!(places.iter().map(|p| p.dist_along_m).collect::<std::vec::Vec<_>>(), [0, 1, 2, 3, 4]);
        places[1].poi.metadata.approach =
            Some(PoiApproach { source: SourceId::osm(1, 10), lon: 6000, lat: 0, profile_mask: 2 });
        assert_eq!(corridor_candidates(&route, &places, 0, map, 0), [2, 3, 4, 0, u8::MAX, u8::MAX]);
        assert_eq!(corridor_candidates(&route, &places, route.total_distance_m + 1, map, 0), [u8::MAX; SOURCE_LIMIT]);
    }

    #[test]
    fn find_drawer_preferences_are_global_and_refresh_both_browsers() {
        use crate::{input::Chord, settings::FindResults, Gesture};
        use obc_ports::InputClock;
        let mut app = crate::App::new_idle(crate::AppState::new(50_000, 50_000, 1.0));
        app.open_find_place();
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        assert!(!app.settings().find_hide_closed);
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        app.advance_animations(InputClock(1000));
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.settings().find_results, FindResults::Six);
        app.advance_animations(InputClock(2000));
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.ui.find.limit, 6);
        assert_eq!(app.ui.find.hours_filter, obc_reader::reader::places::HoursFilter::All);
        app.ui.find.state = State::Ready;
        app.ui.find.results.push(0).unwrap();
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        assert!(app.settings().find_hide_closed);
        assert!(app.ui.find.results.is_empty());
        app.apply_gesture(Gesture::Back);
        app.prepare_find(None, None);
        assert_ne!(app.ui.find.state, State::Ready);
        app.ui.find.action = Action::More;
        app.handle_find_action();
        assert!(matches!(app.top_screen(), Screen::PoiList(_)));
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        assert!(!app.settings().find_hide_closed);
        assert!(app.ui.poi_scratch.query.is_none());
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.ui.find.category, PoiCategory::Campsite);
        let saved = crate::settings::decode(&crate::settings::encode(app.settings())).unwrap();
        let mut rebooted = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        rebooted.set_settings(saved);
        rebooted.open_find_place();
        rebooted.apply_gesture(Gesture::Press);
        assert_eq!(rebooted.ui.find.limit, 6);
        assert_eq!(rebooted.ui.find.hours_filter, obc_reader::reader::places::HoursFilter::All);
    }

    #[test]
    fn result_setting_bounds_both_candidate_sources_and_ranked_choices() {
        let mut nearby = crate::screen::PoiScratch::new();
        let poi = Poi {
            opening: OpeningStatus::Unknown,
            metadata: Default::default(),
            lat: 0,
            lon: 0,
            subtype: 1,
            name: Default::default(),
            hours_ref: 0xffff,
            distance_m: 0,
        };
        for _ in 0..8 {
            nearby.pois.push(obc_reader::CorridorPoi { poi: poi.clone(), dist_along_m: 0, offset_m: 0 }).unwrap();
        }
        for limit in [2, 4, 6] {
            let mut find = FindState::new();
            find.limit = limit;
            assert_eq!(
                (0..PLAN_LIMIT as u8).filter(|&id| find.candidate(id, &nearby, &nearby.pois).is_some()).count(),
                limit * 2
            );
            assert_eq!((0..PLAN_LIMIT as u8).filter(|&id| find.candidate(id, &nearby, &[]).is_some()).count(), limit);
            for i in 0..limit * 2 {
                find.costs[i] = cost(10_000 - i as u32 * 100, Some(100), 100);
            }
            find.rank();
            assert_eq!(
                find.results.as_slice(),
                (limit..limit * 2).rev().map(|i| i as u8).collect::<std::vec::Vec<_>>()
            );
        }
    }

    fn hours_map() -> std::vec::Vec<u8> {
        let mut open = [0; 29];
        for day in 0..7 {
            open[2 + day * 4] = 96;
        }
        map_with_hours(open)
    }

    fn map_with_hours(schedule: [u8; 29]) -> std::vec::Vec<u8> {
        obcm_testkit::build_poi_map_with_hours(
            (0, 0, 100_000, 100_000),
            512,
            &[(
                1,
                std::vec![obcm_testkit::PoiSpec {
                    lat: 50_000,
                    lon: 50_000,
                    subtype: 1,
                    name: "Water".into(),
                    payload: 0,
                }],
            )],
            &[schedule],
        )
    }

    #[test]
    fn last_category_survives_more_places_and_back_until_assistant_closes() {
        let bytes = hours_map();
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut app = crate::App::new_idle(crate::AppState::new(50_000, 50_000, 1.0));
        app.apply_chord(crate::input::Chord::Assistant);
        app.apply_gesture(crate::Gesture::Press);
        app.apply_gesture(crate::Gesture::Press);
        let mut query = PlaceQuery::new(
            0,
            PoiCategorySet::ALL,
            PlaceWindow::Nearby { position: (50_000, 50_000), radius_m: 1000 },
            None,
        );
        while query.step(&reader, None, 0, &mut app.ui.poi_scratch.pois) == QueryProgress::Pending {}
        let poi = app.ui.poi_scratch.pois[0].poi.clone();
        app.ui.find.places.push(poi.clone()).unwrap();
        app.ui.find.results.push(0).unwrap();
        app.ui.find.costs[0] = cost(200, Some(5), 0);
        let stored = RouteSourceKey { store: [1; 16], object: 7, revision: 2 };
        app.ui.find.retained[0] = RetainedReview { object: stored.object, revision: stored.revision, rejoin_m: 0 };
        app.ui.find.retained_store = Some(crate::device_core::StoreIdentity::from_bytes(stored.store));
        app.ui.find.state = State::Ready;
        app.ui.find.clock = (false, app.settings().utc_offset_min);
        app.ui.find.profile = app.settings().bike_profile_idx;
        app.apply_gesture(crate::Gesture::Step(1));
        app.apply_gesture(crate::Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::PoiList(_)));
        assert!(app.ui.poi_scratch.pois.is_empty());
        app.prepare_find(Some(&reader), None);
        assert!(app.retains_find_review(stored));
        assert!(app.catalogs.can_admit_intent(), "the cached route is not queued for removal");
        app.apply_gesture(crate::Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::FindPlace(screen) if screen.choices()));
        assert_eq!(app.ui.find.selected(0, &app.ui.poi_scratch, &[]), Some(&poi));
        app.apply_gesture(crate::Gesture::Back);
        app.apply_gesture(crate::Gesture::Press);
        assert_eq!(app.find_place_state(), State::Ready);
        assert_eq!(app.ui.find.action, Action::None);
        app.apply_gesture(crate::Gesture::Back);
        app.apply_gesture(crate::Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Assistant(_)));
        assert_eq!(app.find_place_state(), State::Ready);
        app.apply_gesture(crate::Gesture::Back);
        assert_eq!(app.find_place_state(), State::Idle);
        assert!(app.ui.find.places.is_empty());
        app.prepare_find(Some(&reader), None);
        assert!(!app.catalogs.can_admit_intent(), "closing the assistant releases its stored route");
    }

    #[test]
    fn direct_find_preview_resolves_hours_and_rejects_stale_or_abandoned_selection() {
        for closed in [false, true] {
            let bytes = if closed { map_with_hours([0; 29]) } else { hours_map() };
            let source = SliceSource(&bytes);
            let tables = MapTables::parse(&source).unwrap();
            let cache = MapCache::new();
            let reader = Reader::new(&source, &tables, &cache);
            for invalid in 0..4 {
                let mut app = crate::App::new_idle(crate::AppState::new(50_000, 50_000, 1.0));
                let map = RouteSourceKey { store: [1; 16], object: 1, revision: 1 };
                app.bind_place_map(Some(map));
                app.stamp_clock_ble(1_727_000_000, 0);
                app.open_find_place();
                app.apply_gesture(crate::Gesture::Press);
                let mut query = PlaceQuery::new(
                    0,
                    PoiCategorySet::ALL,
                    PlaceWindow::Nearby { position: (50_000, 50_000), radius_m: 1000 },
                    None,
                );
                while query.step(&reader, None, 0, &mut app.ui.poi_scratch.pois) == QueryProgress::Pending {}
                app.ui.find.results.push(0).unwrap();
                app.ui.find.state = State::Ready;
                app.ui.find.map = Some(map);
                app.ui.find.profile = app.settings().bike_profile_idx;
                app.ui.find.clock = (true, app.settings().utc_offset_min);
                match invalid {
                    1 => app.ui.find.map = Some(RouteSourceKey { revision: 2, ..map }),
                    2 => app.ui.find.profile = app.settings().bike_profile_idx.wrapping_add(1),
                    3 => app.apply_gesture(crate::Gesture::Back),
                    _ => {}
                }
                if invalid == 0 {
                    if closed {
                        app.ui.poi_scratch.pois[0].poi.opening = OpeningStatus::Closed;
                    }
                    app.apply_gesture(crate::Gesture::Press);
                    assert_eq!(app.ui.find.action, Action::Preview(0));
                } else {
                    app.ui.find.action = Action::Preview(0);
                }
                app.prepare_find(Some(&reader), None);
                assert_eq!(app.ui.find.action, Action::None);
                if invalid == 0 {
                    assert!(matches!(app.top_screen(), Screen::VisitReview(_)));
                    assert!(app.ui.poi_scratch.detail_valid);
                    assert_eq!(app.ui.poi_scratch.detail_source, app.ui.poi_scratch.pois[0].poi.metadata.source.0);
                    assert!(!app.ui.stack.iter().any(|screen| matches!(screen, Screen::PoiDetail(_))));
                } else {
                    assert!(matches!(app.top_screen(), Screen::FindPlace(_)));
                    assert!(!app.ui.poi_scratch.detail_valid);
                    assert!(matches!(app.ui.find.state, State::Stale | State::Ready));
                }
            }
        }
    }

    #[test]
    fn map_replacement_before_first_detail_prepare_cannot_revalidate_old_metadata() {
        let bytes = hours_map();
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut page = heapless::Vec::<_, 8>::new();
        let mut query = PlaceQuery::new(
            0,
            PoiCategorySet::ALL,
            PlaceWindow::Nearby { position: (50_000, 50_000), radius_m: 1000 },
            None,
        );
        while query.step(&reader, None, 0, &mut page) == QueryProgress::Pending {}
        let mut app = crate::App::new_idle(crate::AppState::new(50_000, 50_000, 1.0));
        let key = RouteSourceKey { store: [1; 16], object: 1, revision: 1 };
        app.bind_place_map(Some(key));
        assert!(app.ui.stack.push(Screen::PoiDetail(crate::screen::PoiDetailScreen::new(page[0].poi.clone()))).is_ok());
        app.bind_place_map(Some(RouteSourceKey { revision: 2, ..key }));
        let replacement = obcm_testkit::build_poi_map_with_hours((0, 0, 100_000, 100_000), 512, &[], &[[0; 29]]);
        let source = SliceSource(&replacement);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut frame = crate::harness::support::Buf::new(240, 320);
        app.render_frame(None, &mut frame, &reader, None, 240.0, 320.0, |color| {
            let (r, g, b) = obc_reader::rgb565_to_rgb888(color);
            embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
        });
        assert!(!app.ui.poi_scratch.detail_valid, "a successful hours read cannot restore old source identity");
        app.apply_gesture(crate::Gesture::Press);
        assert_eq!(app.assistant_review_status(), ReviewStatus::Idle);

        // A retained detail can sit below the menu while another detail uses the shared cache.
        app.apply_gesture(crate::Gesture::BackHold);
        assert!(app.ui.stack.push(Screen::PoiDetail(crate::screen::PoiDetailScreen::new(page[0].poi.clone()))).is_ok());
        app.render_frame(None, &mut frame, &reader, None, 240.0, 320.0, |color| {
            let (r, g, b) = obc_reader::rgb565_to_rgb888(color);
            embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
        });
        assert!(app.ui.poi_scratch.detail_valid);
        app.apply_gesture(crate::Gesture::Back);
        app.apply_gesture(crate::Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::PoiDetail(_)));
        app.apply_gesture(crate::Gesture::Press);
        assert!(
            matches!(app.top_screen(), Screen::PoiDetail(detail) if detail.visit_error == Some(crate::navigator::VisitUnavailable::SourceChanged))
        );
        assert_eq!(app.assistant_review_status(), ReviewStatus::Idle);
    }

    #[test]
    fn retained_detail_reloads_its_own_schedule_after_another_place() {
        let mut open = [0; 29];
        open[2] = 96;
        let bytes = obcm_testkit::build_poi_map_with_hours(
            (0, 0, 100_000, 100_000),
            512,
            &[(
                1,
                (0..2)
                    .map(|i| obcm_testkit::PoiSpec {
                        lat: 50_000,
                        lon: 50_000 + i * 100,
                        subtype: 1,
                        name: format!("Water {i}"),
                        payload: i as u16,
                    })
                    .collect(),
            )],
            &[[0; 29], open],
        );
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut page = heapless::Vec::<_, 8>::new();
        let mut query = PlaceQuery::new(
            0,
            PoiCategorySet::ALL,
            PlaceWindow::Nearby { position: (50_000, 50_000), radius_m: 1000 },
            None,
        );
        while query.step(&reader, None, 0, &mut page) == QueryProgress::Pending {}
        assert_eq!(page.len(), 2);
        let mut app = crate::App::new_idle(crate::AppState::new(50_000, 50_000, 1.0));
        app.bind_place_map(Some(RouteSourceKey { store: [1; 16], object: 1, revision: 1 }));
        let mut frame = crate::harness::support::Buf::new(240, 320);
        let draw = |app: &mut crate::App, frame: &mut crate::harness::support::Buf| {
            app.render_frame(None, frame, &reader, None, 240.0, 320.0, |color| {
                let (r, g, b) = obc_reader::rgb565_to_rgb888(color);
                embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
            });
        };
        assert!(app.ui.stack.push(Screen::PoiDetail(crate::screen::PoiDetailScreen::new(page[0].poi.clone()))).is_ok());
        draw(&mut app, &mut frame);
        assert_eq!(app.ui.poi_scratch.detail_schedule.unwrap().status(Some((0, 30))), OpeningStatus::Closed);
        app.apply_gesture(crate::Gesture::BackHold);
        assert!(app.ui.stack.push(Screen::PoiDetail(crate::screen::PoiDetailScreen::new(page[1].poi.clone()))).is_ok());
        draw(&mut app, &mut frame);
        assert_eq!(app.ui.poi_scratch.detail_schedule.unwrap().status(Some((0, 30))), OpeningStatus::Open);
        app.apply_gesture(crate::Gesture::Back);
        app.apply_gesture(crate::Gesture::Back);
        assert!(app.base_needs_reader());
        app.apply_gesture(crate::Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::PoiDetail(detail) if detail.visit_error.is_none()));
        draw(&mut app, &mut frame);
        assert_eq!(app.ui.poi_scratch.detail_source, page[0].poi.metadata.source.0);
        assert_eq!(app.ui.poi_scratch.detail_schedule.unwrap().status(Some((0, 30))), OpeningStatus::Closed);
    }

    #[test]
    fn corridor_hours_failure_is_not_an_empty_result_during_or_after_probes() {
        struct FailingSource {
            bytes: std::vec::Vec<u8>,
            fail: core::cell::Cell<bool>,
        }
        impl ByteSource for FailingSource {
            fn len(&self) -> u64 {
                self.bytes.len() as u64
            }
            fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
                if self.fail.get() {
                    return Err(Error::Io);
                }
                SliceSource(&self.bytes).read_at(offset, out)
            }
        }
        let source = FailingSource { bytes: hours_map(), fail: core::cell::Cell::new(false) };
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut sink = Sink(std::vec::Vec::new());
        obc_route::gpx_to_obcr(&SliceSource(br#"<gpx><trk><trkseg><trkpt lon="0.04" lat="0.05"/><trkpt lon="0.06" lat="0.05"/></trkseg></trk></gpx>"#), "Route", &mut sink).unwrap();
        let route_source = SliceSource(&sink.0);
        let index = obc_route::RouteIndex::read(&route_source).unwrap();
        let route = RouteReader::new(&index, &route_source);
        for state in [State::Planning, State::Ready] {
            source.fail.set(false);
            let mut app = crate::App::new_idle(crate::AppState::new(50_000, 50_000, 1.0));
            app.stamp_clock_ble(1_727_000_000, 0);
            app.open_find_place();
            app.apply_gesture(crate::Gesture::Press);
            let local = app.place_local_time();
            app.ui.corridor_scratch.arm(crate::corridor::CorridorKey {
                hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
                filter: PoiCategorySet::ALL,
                anchor_m: 0,
            });
            app.ui.corridor_scratch.clock_changed(local, 0);
            while app.ui.corridor_scratch.pending() {
                app.ui.corridor_scratch.prepare_to(Some(&reader), Some(&route), local, FORWARD_M);
            }
            assert_eq!(app.ui.corridor_scratch.len(), 1);
            app.ui.find.state = state;
            app.ui.find.clock = (true, 0);
            app.ui.find.local = local;
            app.ui.find.results.push(1).unwrap();
            source.fail.set(true);
            app.stamp_clock_ble(1_727_000_060, 0);
            app.prepare_find(Some(&reader), Some(&route));
            assert_eq!(app.find_place_state(), State::Failed);
            assert!(app.ui.find.results.is_empty());
            assert!(matches!(app.ui.corridor_scratch.status(), QueryProgress::Failed(_)));
        }
    }
    fn cost(arrival: u32, ascent: Option<u32>, added: u32) -> Option<Costs> {
        Some(Costs { arrival_m: arrival, arrival_ascent_m: ascent, added_m: Some(added), added_ascent_m: ascent })
    }
    #[test]
    fn useful_measured_choices_keep_on_way_stops_and_a_nearer_alternative() {
        let mut f = FindState::new();
        f.costs[0] = cost(2000, Some(100), 100);
        f.costs[1] = cost(3000, Some(100), 200);
        f.costs[2] = cost(4000, Some(100), 300);
        f.costs[3] = cost(5000, Some(100), 100);
        f.costs[4] = cost(1000, Some(200), 900);
        f.costs[5] = cost(4000, Some(200), 1000);
        f.rank();
        assert_eq!(f.results.as_slice(), &[0, 1, 2, 4]);
        f.costs.fill(None);
        f.costs[0] = cost(100, None, 100);
        f.costs[1] = cost(200, Some(5), 900);
        f.rank();
        assert_eq!(f.results.as_slice(), &[0, 1], "unknown elevation cannot dominate a measured alternative");
        f.costs.fill(None);
        f.rank();
        assert!(f.results.is_empty());
        f.costs[2] = cost(500, Some(10), 1000);
        f.rank();
        assert_eq!(f.results.as_slice(), &[2]);
    }
    #[test]
    fn no_fix_and_no_map_settle_until_explicit_refresh_and_replacement_invalidates_details() {
        let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        app.open_find_place();
        app.apply_gesture(crate::Gesture::Press);
        app.prepare_find(None, None);
        assert_eq!(app.find_place_state(), State::NoFix);
        app.ui.find.profile = 1; // No request context was captured for this unavailable state.
        app.prepare_find(None, None);
        assert_eq!(app.find_place_state(), State::NoFix);
        let map = RouteSourceKey { store: [1; 16], object: 1, revision: 1 };
        app.bind_place_map(Some(map));
        app.ui.find.state = State::Ready;
        app.ui.find.map = Some(map);
        app.ui.poi_scratch.detail_valid = true;
        app.bind_place_map(Some(RouteSourceKey { revision: 2, ..map }));
        assert_eq!(app.find_place_state(), State::Stale);
        assert!(!app.ui.poi_scratch.detail_valid);
    }
}
