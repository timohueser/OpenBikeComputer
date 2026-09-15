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
pub const SOURCE_LIMIT: usize = 8;
pub const PLAN_LIMIT: usize = SOURCE_LIMIT * 2;
pub const RESULT_LIMIT: usize = 4;
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
    Accept,
    Cancel,
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

pub struct FindState {
    pub state: State,
    pub(crate) action: Action,
    pub category: PoiCategory,
    pub map: Option<RouteSourceKey>,
    bound_map: Option<RouteSourceKey>,
    pub origin: (i32, i32),
    pub results: heapless::Vec<u8, RESULT_LIMIT>,
    costs: [Option<Costs>; PLAN_LIMIT],
    next: u8,
    context: Option<ReviewContext>,
    route: Option<u64>,
    remaining_m: Option<u32>,
    remaining_ascent: Option<u32>,
    clock: (bool, i16),
    profile: u8,
    local: Option<(u8, u16)>,
    selected_review: bool,
    pub review: ReviewStatus,
    pub review_costs: Option<Costs>,
}
impl FindState {
    pub const fn new() -> Self {
        Self {
            state: State::Idle,
            action: Action::None,
            category: PoiCategory::Water,
            map: None,
            bound_map: None,
            origin: (0, 0),
            results: heapless::Vec::new(),
            costs: [None; PLAN_LIMIT],
            next: 0,
            context: None,
            route: None,
            remaining_m: None,
            remaining_ascent: None,
            clock: (false, 0),
            profile: 0,
            local: None,
            selected_review: false,
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
        if id.is_multiple_of(2) { nearby.pois.get(i) } else { corridor.get(i) }.map(|hit| &hit.poi)
    }
    pub fn selected<'a>(
        &self,
        i: usize,
        nearby: &'a crate::screen::PoiScratch,
        corridor: &'a [obc_reader::CorridorPoi],
    ) -> Option<&'a Poi> {
        self.candidate(*self.results.get(i)?, nearby, corridor)
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
            if at < RESULT_LIMIT {
                if list.is_full() {
                    list.pop();
                }
                let _ = list.insert(at, i as u8);
            }
        }
        let slots = RESULT_LIMIT - usize::from(!alternatives.is_empty());
        for id in on_way.iter().take(slots).chain(alternatives.iter()).take(RESULT_LIMIT) {
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
        if self.ui.find.bound_map.is_some() && self.ui.find.bound_map != map {
            self.ui.poi_scratch.cancel();
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
    pub(crate) fn handle_find_action(&mut self) {
        match core::mem::replace(&mut self.ui.find.action, Action::None) {
            Action::Refresh => {
                self.cancel_assistant();
                self.ui.find.state = State::Start;
                self.ui.find.results.clear();
                self.ui.find.costs.fill(None);
                self.ui.find.context = None;
                self.ui.find.next = 0;
                self.ui.poi_scratch.invalidate();
                self.ui.corridor_scratch.disarm();
            }
            Action::More => {
                self.cancel_assistant();
                self.ui.find.state = State::Idle;
                self.ui.poi_scratch.invalidate();
                self.ui.corridor_scratch.disarm();
                crate::screen::apply(
                    &mut self.ui.stack,
                    crate::screen::Transition::Replace(Screen::PoiList(crate::screen::PoiListScreen::new(
                        self.ui.find.category,
                    ))),
                );
            }
            Action::Accept => {
                if self.ui.poi_scratch.detail_valid
                    && !self
                        .ui
                        .poi_scratch
                        .detail_schedule
                        .is_some_and(|s| s.status(self.place_local_time()) == OpeningStatus::Closed)
                {
                    if let Some(origin) = self.current_review_origin() {
                        self.accept_assistant(origin);
                    }
                }
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
            if self.assistant_review_status() != ReviewStatus::Accepted {
                self.cancel_assistant();
            }
        }
        let has_find = self.ui.stack.iter().any(|s| matches!(s, Screen::FindPlace(s) if s.choices()));
        if self.ui.find.owns_pages() && !has_find {
            if matches!(self.ui.find.state, State::Planning | State::Releasing) {
                self.cancel_assistant();
            }
            self.ui.find.state = State::Idle;
            self.ui.corridor_scratch.disarm();
        }
    }
    pub(crate) fn activate_place_detail(&mut self) -> bool {
        let Some(Screen::PoiDetail(detail)) = self.ui.stack.last() else { return false };
        let poi = detail.poi().clone();
        if !self.ui.poi_scratch.detail_valid
            || self
                .ui
                .poi_scratch
                .detail_schedule
                .is_some_and(|s| s.status(self.place_local_time()) == OpeningStatus::Closed)
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
                crate::screen::apply(
                    &mut self.ui.stack,
                    crate::screen::Transition::Push(Screen::VisitReview(VisitReviewScreen::new(name))),
                );
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
    pub(crate) fn prepare_find(&mut self, reader: Option<&Reader>, route: Option<&RouteReader>) {
        let local = self.place_local_time();
        self.handle_find_exit();
        self.ui.find.review = self.assistant_review_status();
        let review_screen = self.ui.stack.iter().any(|s| matches!(s, Screen::VisitReview(_)));
        if review_screen {
            if self.assistant_review_status() == ReviewStatus::Accepted {
                crate::screen::apply(
                    &mut self.ui.stack,
                    crate::screen::Transition::Root(Screen::Map(crate::screen::MapScreen::new())),
                );
                self.ui.find.state = State::Idle;
                self.ui.map_dirty = true;
                return;
            }
            if self.ui.poi_scratch.detail_schedule.is_some_and(|s| s.status(local) == OpeningStatus::Closed) {
                self.invalidate_assistant_preview();
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
            if !self.assistant_planner_released() {
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
            let scratch = &mut self.ui.poi_scratch;
            scratch.query = Some(PlaceQuery::new(
                scratch.generation,
                PoiCategorySet::only(self.ui.find.category),
                PlaceWindow::Nearby { position: self.ui.find.origin, radius_m: NEARBY_M },
                local,
            ));
            scratch.status = QueryProgress::Pending;
            if route.is_some() {
                self.ui.corridor_scratch.arm(crate::corridor::CorridorKey {
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
                self.ui.find.state = State::Planning;
            } else {
                self.ui.map_dirty = true;
                self.ui.next_wake_ms = Some(1);
                return;
            }
        }
        if self.ui.find.state == State::Releasing {
            if !self.assistant_planner_released() || self.assistant_review_status() != ReviewStatus::Idle {
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
            let Some(poi) = poi.filter(|p| p.opening != OpeningStatus::Closed) else {
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
                .is_none_or(|p| p.opening == OpeningStatus::Closed)
            {
                self.ui.find.costs[i] = None;
            }
        }
        self.ui.find.rank();
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
        Some(Costs {
            arrival_m,
            arrival_ascent_m: facts.arrival_elevation_complete.then_some(facts.arrival_ascent_m),
            added_m: context.original.and(self.ui.find.remaining_m).map(|m| p.distance_m.saturating_sub(m)),
            added_ascent_m: context
                .original
                .and(self.ui.find.remaining_ascent)
                .filter(|_| facts.complete_elevation)
                .map(|a| p.ascent_m.saturating_sub(a)),
        })
    }
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

    fn hours_map() -> std::vec::Vec<u8> {
        let mut open = [0; 29];
        for day in 0..7 {
            open[2 + day * 4] = 96;
        }
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
                    hours_ref: 0,
                }],
            )],
            &[open],
        )
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
            app.ui.corridor_scratch.arm(crate::corridor::CorridorKey { filter: PoiCategorySet::ALL, anchor_m: 0 });
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
