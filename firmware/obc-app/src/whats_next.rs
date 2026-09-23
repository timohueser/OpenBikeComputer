//! Prepared facts and a four-row view over one frozen accepted-route interval.
//! Map places remain in the existing corridor page. Authored records are streamed independently
//! of the riding waypoint cache, so browsing cannot change guidance or truncate the timeline.

use crate::{
    corridor::{quarter, CorridorKey, CorridorScratch, UpAheadScope},
    settings::UpAheadSource,
};
use core::cmp::Ordering;
use heapless::Vec;
use obc_formats::obcm::SourceId;
use obc_reader::{
    hours::OpeningStatus,
    reader::places::{PlaceKey, QueryProgress},
    CorridorPoi, PoiCategory, PoiCategorySet, Reader,
};
use obc_route::{
    window::{AheadRange, RouteWindow},
    ClimbSeg, Climbs, RouteReader, WaypointCursor, WptEntry,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Waypoint(u32, u16),
    Climb(u32, u8),
    Place(PlaceKey),
}
impl Key {
    pub fn distance(self) -> u32 {
        match self {
            Self::Waypoint(d, _) | Self::Climb(d, _) => d,
            Self::Place(k) => k.occurrence,
        }
    }
    fn order(self) -> (u32, u8, u64, u32) {
        match self {
            Self::Waypoint(d, i) => (d, 0, i.into(), 0),
            Self::Climb(d, i) => (d, 1, i.into(), 0),
            Self::Place(k) => (k.occurrence, 2, k.source.0, k.distance_m),
        }
    }
    fn place_boundary(self, anchor: u32) -> PlaceKey {
        match self {
            Self::Place(k) => k,
            _ => PlaceKey {
                distance_m: self.distance().saturating_sub(anchor),
                source: SourceId(0),
                occurrence: self.distance(),
            },
        }
    }
}
impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order().cmp(&other.order())
    }
}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
pub(crate) enum Item {
    Waypoint(WptEntry),
    Climb(ClimbSeg),
    Place(u8),
}
#[derive(Debug)]
pub(crate) struct Row {
    pub key: Key,
    pub item: Item,
    pub ascent_m: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Overview,
    Timeline,
    Detail,
}

pub struct AheadState {
    pub(crate) window: Option<RouteWindow>,
    pub(crate) range: AheadRange,
    pub(crate) page: Page,
    pub(crate) totals: Option<(u32, u32)>,
    pub(crate) next_waypoint: Option<WptEntry>,
    pub(crate) climb: Option<ClimbSeg>,
    pub(crate) water: Option<u32>,
    pub(crate) shop: Option<u32>,
    pub(crate) status: QueryProgress,
    pub(crate) rows: Vec<Row, 4>,
    pub(crate) selected: usize,
    anchor: u32,
    scope: Option<UpAheadScope>,
    boundary: Option<Key>,
    backwards: bool,
    /// The settled Overview's place status and the quarter hour it was taken at. It outlives a
    /// visit to the Timeline, so Back shows the Overview without a new query.
    overview: Option<(QueryProgress, Option<(u8, u16)>)>,
    cursor: Option<WaypointCursor>,
    ordinal: u16,
    authored_done: bool,
    places_done: bool,
    more: bool,
    dirty: bool,
    settled: bool,
    pub(crate) stale: bool,
}
impl AheadState {
    pub const fn new() -> Self {
        Self {
            window: None,
            range: AheadRange::TenKm,
            page: Page::Overview,
            totals: None,
            next_waypoint: None,
            climb: None,
            water: None,
            shop: None,
            status: QueryProgress::Unavailable,
            rows: Vec::new(),
            selected: 0,
            anchor: 0,
            scope: None,
            boundary: None,
            backwards: false,
            overview: None,
            cursor: None,
            ordinal: 0,
            authored_done: false,
            places_done: false,
            more: false,
            dirty: true,
            settled: false,
            stale: false,
        }
    }
    pub(crate) fn open(&mut self, anchor: u32) {
        *self = Self::new();
        self.anchor = anchor;
    }
    pub(crate) fn range(&mut self, range: AheadRange) {
        if self.range != range {
            self.range = range;
            self.window = None;
            self.dirty = true;
        }
    }
    pub(crate) fn explore(&mut self) {
        self.page = Page::Timeline;
        self.boundary = None;
        self.backwards = false;
        self.dirty = true;
    }
    pub(crate) fn back(&mut self) {
        self.page = Page::Overview;
        self.rows.clear();
        if let Some((status, _)) = self.overview {
            self.status = status;
            self.authored_done = true;
            self.places_done = true;
            self.settled = true;
        } else if !self.stale {
            self.dirty = true;
        }
    }
    pub(crate) fn refresh(&mut self, anchor: u32) {
        self.stale = false;
        self.anchor = anchor;
        self.window = None;
        self.boundary = None;
        self.dirty = true;
    }
    pub(crate) fn has_next(&self) -> bool {
        if self.backwards {
            self.boundary.is_some()
        } else {
            self.more
        }
    }
    pub(crate) fn has_previous(&self) -> bool {
        if self.backwards {
            self.more
        } else {
            self.boundary.is_some()
        }
    }
    pub(crate) fn turn_page(&mut self, backwards: bool) {
        let edge = if backwards { self.rows.first() } else { self.rows.last() };
        if let Some(row) = edge {
            self.boundary = Some(row.key);
            self.backwards = backwards;
            self.dirty = true;
        }
    }
    pub(crate) fn invalidate(&mut self) {
        self.stale = true;
        self.window = None;
        self.rows.clear();
        self.next_waypoint = None;
        self.totals = None;
        self.status = QueryProgress::Unavailable;
        self.authored_done = true;
        self.places_done = true;
        self.dirty = false;
    }
    pub(crate) fn pending(&self) -> bool {
        self.dirty || !self.authored_done || !self.places_done
    }
    pub(crate) fn request(&self, scope: UpAheadScope) -> Option<CorridorKey> {
        if self.stale || (self.page == Page::Overview && self.overview.is_some()) {
            return None;
        }
        let filter = if self.page == Page::Overview {
            PoiCategorySet::only(PoiCategory::Water).with(PoiCategory::Resupply)
        } else {
            scope.filter
        };
        (self.page == Page::Overview || scope.source.shows_pois()).then_some(CorridorKey {
            hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
            filter,
            anchor_m: self.window.map_or(self.anchor, |w| w.start_m),
        })
    }
    fn keep(&self, key: Key) -> bool {
        self.boundary.is_none_or(|b| if self.backwards { key < b } else { key > b })
    }
    fn insert(&mut self, row: Row) {
        if !self.keep(row.key) {
            return;
        }
        let i = self.rows.iter().position(|r| r.key > row.key).unwrap_or(self.rows.len());
        if self.rows.is_full() {
            self.more = true;
            if self.backwards {
                if i == 0 {
                    return;
                }
                self.rows.remove(0);
                let _ = self.rows.insert(i - 1, row);
            } else if i < self.rows.len() {
                self.rows.pop();
                let _ = self.rows.insert(i, row);
            }
        } else {
            let _ = self.rows.insert(i, row);
        }
    }
    pub(crate) fn prepare(
        &mut self,
        reader: Option<&Reader>,
        route: Option<&RouteReader>,
        climbs: &Climbs,
        scope: UpAheadScope,
        scratch: &mut CorridorScratch,
        local: Option<(u8, u16)>,
    ) {
        if self.stale {
            scratch.disarm();
            return;
        }
        if self.window.is_some_and(|w| route.is_none_or(|r| !w.matches(r))) {
            self.invalidate();
            scratch.disarm();
            return;
        }
        let Some(route) = route else {
            return self.fail(QueryProgress::Unavailable, scratch);
        };
        let window = RouteWindow::new(route, self.anchor, self.range);
        if self.window != Some(window) {
            self.boundary = None;
            self.backwards = false;
            self.overview = None;
            self.dirty = true;
            let facts = window.facts(route).and_then(|f| Ok((f, window.next_waypoint(route)?)));
            let (facts, next_waypoint) = match facts {
                Ok(facts) => facts,
                Err(e) => return self.fail(QueryProgress::Failed(obc_reader::Error::Source(e)), scratch),
            };
            self.window = Some(window);
            self.totals = facts.complete_elevation().then_some((facts.ascent_m, facts.descent_m));
            self.next_waypoint = next_waypoint.map(entry);
            self.climb = window.climb(climbs).copied();
        }
        if self.page == Page::Overview && self.overview.is_some_and(|(_, at)| at != quarter(local)) {
            self.dirty = true;
        }
        if self.settled
            && self.request(scope).is_some()
            && (scratch.pending() || matches!(scratch.status(), QueryProgress::Failed(_) | QueryProgress::Unavailable))
        {
            scratch.prepare_to(reader, Some(route), local, window.end_m);
            self.status = scratch.status();
            if matches!(self.status, QueryProgress::Failed(_) | QueryProgress::Unavailable) {
                self.rows.retain(|row| !matches!(row.item, Item::Place(_)));
                self.selected = self.selected.min(self.rows.len().saturating_sub(1));
            } else {
                let selected = self.rows.get(self.selected).map(|row| row.key);
                self.rows.retain(|row| selected == Some(row.key) || !matches!(row.item, Item::Place(i) if scratch.entries().get(i as usize).is_none_or(|p| p.poi.opening == OpeningStatus::Closed)));
                self.selected = selected.and_then(|key| self.rows.iter().position(|row| row.key == key)).unwrap_or(0);
            }
        }
        if self.scope != Some(scope) {
            self.scope = Some(scope);
            self.boundary = None;
            self.backwards = false;
            self.dirty = true;
        }
        if self.dirty {
            self.settled = false;
            self.rows.clear();
            self.selected = 0;
            self.more = false;
            self.ordinal = 0;
            // The Overview shows no authored rows, so only the Timeline walks them.
            let overview = self.page == Page::Overview;
            if overview {
                self.overview = None;
                self.water = None;
                self.shop = None;
            } else {
                match route.waypoint_cursor() {
                    Ok(cursor) => self.cursor = Some(cursor),
                    Err(e) => return self.fail(QueryProgress::Failed(obc_reader::Error::Source(e)), scratch),
                }
            }
            self.authored_done = overview;
            self.places_done = false;
            scratch.invalidate();
            if let Some(key) = self.request(scope) {
                scratch.arm(key);
                if let Some(boundary) = self.boundary {
                    scratch.start_after(boundary.place_boundary(window.start_m), self.backwards);
                }
            } else {
                scratch.disarm();
                self.places_done = true;
            }
            self.status = QueryProgress::Pending;
            if !overview && scope.source == UpAheadSource::Both && scope.filter == PoiCategorySet::ALL {
                for (i, c) in climbs.as_slice().iter().enumerate() {
                    if window.contains(c.start_m) {
                        self.insert(Row { key: Key::Climb(c.start_m, i as u8), item: Item::Climb(*c), ascent_m: None });
                    }
                }
            }
            self.dirty = false;
        }
        if !self.authored_done {
            for _ in 0..16 {
                match route.next_waypoint(self.cursor.as_mut().unwrap()) {
                    // Waypoints are sorted by distance, so the first one past the window ends the walk.
                    Ok(Some(w)) if w.dist_along_m <= window.end_m => {
                        let index = self.ordinal;
                        self.ordinal += 1;
                        if window.contains(w.dist_along_m)
                            && scope.source.shows_waypoints()
                            && w.category().map_or(scope.filter == PoiCategorySet::ALL, |c| scope.filter.contains(c))
                        {
                            self.insert(Row {
                                key: Key::Waypoint(w.dist_along_m, index),
                                item: Item::Waypoint(entry(w)),
                                ascent_m: None,
                            });
                        }
                    }
                    Ok(_) => {
                        self.authored_done = true;
                        break;
                    }
                    Err(e) => return self.fail(QueryProgress::Failed(obc_reader::Error::Source(e)), scratch),
                }
            }
        }
        if !self.places_done {
            if reader.is_none() {
                scratch.cancel();
                self.status = QueryProgress::Unavailable;
                self.places_done = true;
                return;
            }
            scratch.prepare_to(reader, Some(route), local, window.end_m);
            self.status = scratch.status();
            if let QueryProgress::Ready { more, .. } = self.status {
                if self.page == Page::Overview {
                    for p in scratch.entries().iter().filter(|p| p.poi.opening != OpeningStatus::Closed) {
                        match obc_formats::obcm::poi_category_of(p.poi.subtype) {
                            Some(PoiCategory::Water) if self.water.is_none() => self.water = Some(p.dist_along_m),
                            Some(PoiCategory::Resupply) if self.shop.is_none() => self.shop = Some(p.dist_along_m),
                            _ => {}
                        }
                    }
                    if more && (self.water.is_none() || self.shop.is_none()) {
                        if let Some(last) = scratch.entries().last() {
                            let key = place_key(last);
                            scratch.next_page(key);
                            self.status = QueryProgress::Pending;
                            return;
                        }
                    }
                } else {
                    for (i, p) in scratch.entries().iter().enumerate() {
                        if p.poi.opening != OpeningStatus::Closed {
                            self.insert(Row {
                                key: Key::Place(place_key(p)),
                                item: Item::Place(i as u8),
                                ascent_m: None,
                            });
                        }
                    }
                    self.more |= more;
                }
                self.places_done = true;
            } else if matches!(self.status, QueryProgress::Failed(_) | QueryProgress::Unavailable) {
                self.places_done = true;
            }
        }
        if self.authored_done && self.places_done && !self.settled {
            self.settled = true;
            if self.page == Page::Overview {
                self.overview = Some((self.status, quarter(local)));
                scratch.disarm();
                return;
            }
            if self.backwards && self.rows.is_empty() {
                self.boundary = None;
                self.backwards = false;
                self.dirty = true;
                return;
            }
            if self.backwards {
                self.selected = self.rows.len().saturating_sub(1);
            }
            let ends: Vec<u32, 4> = self.rows.iter().map(|row| row.key.distance()).collect();
            let facts = route.interval_facts_to::<4>(window.start_m, &ends);
            for (i, row) in self.rows.iter_mut().enumerate() {
                let facts = facts.as_ref().ok().and_then(|facts| facts.get(i));
                row.ascent_m = facts.filter(|f| f.complete_elevation()).map(|f| f.ascent_m);
            }
            if self.request(scope).is_none() {
                self.status = QueryProgress::Ready { more: self.more, coverage_complete: true };
            }
        }
    }
    /// A read failure or a missing route shows no window. The next prepare starts it again.
    fn fail(&mut self, status: QueryProgress, scratch: &mut CorridorScratch) {
        self.window = None;
        self.rows.clear();
        self.totals = None;
        self.next_waypoint = None;
        self.status = status;
        self.dirty = false;
        self.authored_done = true;
        self.places_done = true;
        scratch.disarm();
    }
}
impl Default for AheadState {
    fn default() -> Self {
        Self::new()
    }
}
fn entry(w: obc_route::Waypoint) -> WptEntry {
    let category = w.category();
    WptEntry {
        dist_along_m: w.dist_along_m,
        lon: w.lon,
        lat: w.lat,
        category,
        lateral_offset_m: w.lateral_offset_m,
        name: w.name,
    }
}
pub(crate) fn place_key(p: &CorridorPoi) -> PlaceKey {
    PlaceKey { distance_m: p.poi.distance_m, source: p.poi.metadata.source, occurrence: p.dist_along_m }
}

impl crate::App {
    /// Open the production overview at current accepted-route progress. Category is per-entry;
    /// the rider's persisted source preference remains unchanged.
    pub fn open_whats_next(&mut self) {
        self.ui.ahead.open(self.navigator.route_state().progress_m);
        self.state.up_ahead_filter = PoiCategorySet::ALL;
        crate::screen::apply(
            &mut self.ui.stack,
            crate::screen::Transition::Push(crate::screen::Screen::WhatsNext(crate::screen::WhatsNextScreen::new())),
        );
        self.ui.reconcile_corridor(self.up_ahead_scope());
        self.ui.map_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::{ByteSink, Error, SliceSource};
    use obc_reader::{MapCache, MapTables};
    use obc_route::RouteIndex;
    use obcm_testkit::{build_poi_map, PoiSpec};
    use std::fmt::Write;

    #[derive(Default)]
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
    fn route(missing: bool) -> std::vec::Vec<u8> {
        let mut gpx = std::string::String::from("<gpx>");
        for i in 1..=40 {
            let _ = write!(gpx, "<wpt lat=\"0\" lon=\"{}\"><name>Authored {i}</name></wpt>", i as f64 * 0.002);
        }
        gpx.push_str("<trk><trkseg>");
        for i in 0..=140 {
            let _ = write!(gpx, "<trkpt lat=\"0\" lon=\"{}\">", i as f64 * 0.001);
            if !missing || i != 40 {
                let _ = write!(gpx, "<ele>{}</ele>", (i % 20) * 3);
            }
            gpx.push_str("</trkpt>");
        }
        gpx.push_str("</trkseg></trk></gpx>");
        let mut sink = Sink::default();
        obc_route::gpx_to_obcr(&SliceSource(gpx.as_bytes()), "Window", &mut sink).unwrap();
        // The GPX converter's bounded placement set is smaller than the format's section. Extend
        // the stored section independently to exercise records beyond the resident riding table.
        use obc_formats::{
            io::{put_u16, put_u32, rd_u32},
            obcr::{HEADER_LEN, WAYPOINT_LEN},
        };
        let offset = rd_u32(&sink.0, HEADER_LEN) as usize;
        let last = sink.0[offset + 31 * WAYPOINT_LEN..offset + 32 * WAYPOINT_LEN].to_vec();
        let distance = rd_u32(&last, 0);
        for i in 1..=8 {
            let mut record = last.clone();
            put_u32(&mut record, 0, distance + i * 222);
            record[14] = PoiCategory::Train as u8;
            sink.0.extend_from_slice(&record);
        }
        put_u16(&mut sink.0, HEADER_LEN + 4, 40);
        sink.0
    }
    fn settle(
        a: &mut AheadState,
        map: Option<&Reader>,
        route: &RouteReader,
        scope: UpAheadScope,
        scratch: &mut CorridorScratch,
    ) {
        for _ in 0..1000 {
            a.prepare(map, Some(route), &Climbs::default(), scope, scratch, None);
            if !a.pending() {
                return;
            }
        }
        panic!("window did not settle");
    }
    fn scope(source: UpAheadSource) -> UpAheadScope {
        UpAheadScope { filter: PoiCategorySet::ALL, source }
    }

    #[test]
    fn real_route_window_is_frozen_clipped_and_distinguishes_missing_elevation() {
        for missing in [false, true] {
            let bytes = route(missing);
            let src = SliceSource(&bytes);
            let idx = RouteIndex::read(&src).unwrap();
            let route = RouteReader::new(&idx, &src);
            let mut a = AheadState::new();
            a.open(3_000);
            let mut scratch = CorridorScratch::new();
            settle(&mut a, None, &route, scope(UpAheadSource::Both), &mut scratch);
            assert_eq!(a.window.unwrap().start_m, 3_000);
            assert_eq!(a.window.unwrap().end_m, 13_000);
            assert_eq!(a.totals.is_some(), !missing);
            a.range(AheadRange::FiveKm);
            settle(&mut a, None, &route, scope(UpAheadSource::Both), &mut scratch);
            assert_eq!(a.window.unwrap().end_m, 8_000);
            a.refresh(15_000);
            settle(&mut a, None, &route, scope(UpAheadSource::Both), &mut scratch);
            assert_eq!(a.window.unwrap().end_m, route.total_distance_m);
            a.refresh(u32::MAX);
            settle(&mut a, None, &route, scope(UpAheadSource::Both), &mut scratch);
            assert_eq!(scratch.armed().unwrap().anchor_m, route.total_distance_m);
            assert_eq!(a.window.unwrap().start_m, route.total_distance_m);
            let replacement = RouteIndex::read(&src).unwrap();
            let replacement = RouteReader::new(&replacement, &src);
            a.prepare(None, Some(&replacement), &Climbs::default(), scope(UpAheadSource::Both), &mut scratch, None);
            assert!(a.stale);
            assert!(a.rows.is_empty());
            assert!(a.window.is_none());
        }
    }

    #[test]
    fn authored_and_map_pages_are_complete_and_bidirectional_without_touching_riding_cache() {
        let bytes = route(false);
        let src = SliceSource(&bytes);
        let idx = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&idx, &src);
        let pois = (1..=22)
            .map(|i| PoiSpec {
                lat: 100,
                lon: i * 3_000 + 1000,
                subtype: 1,
                name: format!("Water {i}"),
                payload: u16::MAX,
            })
            .collect();
        let train = vec![PoiSpec { lat: 100, lon: 65_000, subtype: 20, name: "Station".into(), payload: u16::MAX }];
        let map = build_poi_map((-1000, -1000, 150_000, 1000), 512, &[(1, pois), (8, train)]);
        let source = obc_reader::SliceSource(&map);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let map = Reader::new(&source, &tables, &cache);
        let mut a = AheadState::new();
        a.open(0);
        a.explore();
        let mut scratch = CorridorScratch::new();
        let mut keys = std::vec::Vec::new();
        let mut last_page = std::vec::Vec::new();
        loop {
            settle(&mut a, Some(&map), &route, scope(UpAheadSource::Both), &mut scratch);
            let page: std::vec::Vec<_> = a.rows.iter().map(|r| r.key).collect();
            assert!(!page.is_empty());
            if !keys.is_empty() {
                assert!(keys.last().unwrap() < &page[0]);
            }
            keys.extend_from_slice(&page);
            if !a.more {
                break;
            }
            last_page = page;
            a.turn_page(false);
        }
        assert_eq!(keys.iter().filter(|k| matches!(k, Key::Waypoint(..))).count(), 40);
        assert_eq!(keys.iter().filter(|k| matches!(k, Key::Place(_))).count(), 23);
        a.turn_page(true);
        settle(&mut a, Some(&map), &route, scope(UpAheadSource::Both), &mut scratch);
        assert_eq!(a.rows.iter().map(|r| r.key).collect::<std::vec::Vec<_>>(), last_page);
        let selected = a.rows[a.selected].key;
        a.page = Page::Detail;
        settle(&mut a, Some(&map), &route, scope(UpAheadSource::Both), &mut scratch);
        a.page = Page::Timeline;
        assert_eq!(a.rows[a.selected].key, selected);
        settle(&mut a, None, &route, scope(UpAheadSource::WaypointsOnly), &mut scratch);
        assert!(a.rows.iter().all(|r| matches!(r.item, Item::Waypoint(_))));
        assert_eq!(route.load_waypoints(0).entries.len(), obc_route::MAX_WAYPOINTS);
        let train = UpAheadScope { filter: PoiCategorySet::only(PoiCategory::Train), source: UpAheadSource::Both };
        settle(&mut a, Some(&map), &route, train, &mut scratch);
        assert!(a.rows.iter().all(|r| matches!(&r.item,Item::Waypoint(w) if w.category==Some(PoiCategory::Train))
            || matches!(r.item, Item::Place(_))));
    }
    #[test]
    fn ordinary_assistant_sources_and_filters_preserve_authored_rows_and_persist() {
        use crate::{input::Chord, screen::Screen, App, AppState, Gesture};
        let bytes = route(false);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.test_mount_store();
        app.set_routes_with_ids(&[route.summary()], &[7]);
        app.navigator.set_active_route(Some(0));
        app.navigator.sync_route_state(Some(&route));
        assert!(app.apply_chord(Chord::Assistant));
        app.apply_gesture(Gesture::Step(1));
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::WhatsNext(_)));
        app.apply_gesture(Gesture::Press);
        let mut now = 1_000;
        let act = |app: &mut App, now: &mut u32, gesture| {
            *now += 400;
            app.advance_animations(obc_ports::InputClock(*now));
            app.apply_gesture(gesture);
        };
        assert!(app.apply_chord(Chord::Context));
        act(&mut app, &mut now, Gesture::Step(1));
        act(&mut app, &mut now, Gesture::Press);
        act(&mut app, &mut now, Gesture::Step(1));
        assert_eq!(app.settings().up_ahead_source, UpAheadSource::Both);
        act(&mut app, &mut now, Gesture::Press);
        assert_eq!(app.settings().up_ahead_source, UpAheadSource::WaypointsOnly);
        assert!(!crate::harness::support::quiet_pass(&mut app, now).effects.settings.is_empty());
        act(&mut app, &mut now, Gesture::Back);
        let scope = app.up_ahead_scope();
        settle(&mut app.ui.ahead, None, &route, scope, &mut app.ui.corridor_scratch);
        assert!(app.ui.ahead.rows.iter().any(|r| matches!(r.item, Item::Waypoint(_))));
        assert!(app.ui.ahead.rows.iter().all(|r| matches!(r.item, Item::Waypoint(_))));
        assert!(app.ui.ahead.request(scope).is_none());
        app.apply_gesture(Gesture::Step(1));
        let selected = app.ui.ahead.rows[app.ui.ahead.selected].key;
        assert!(app.apply_chord(Chord::Context));
        act(&mut app, &mut now, Gesture::Press);
        act(&mut app, &mut now, Gesture::Step(1));
        act(&mut app, &mut now, Gesture::Back);
        assert_eq!(app.state.up_ahead_filter, PoiCategorySet::ALL);
        act(&mut app, &mut now, Gesture::Back);
        assert_eq!(app.ui.ahead.rows[app.ui.ahead.selected].key, selected);
        assert!(app.apply_chord(Chord::Context));
        act(&mut app, &mut now, Gesture::Press);
        act(&mut app, &mut now, Gesture::Step(1));
        act(&mut app, &mut now, Gesture::Press);
        act(&mut app, &mut now, Gesture::Back);
        assert_eq!(app.state.up_ahead_filter, PoiCategorySet::only(PoiCategory::Water));
        let scope = app.up_ahead_scope();
        settle(&mut app.ui.ahead, None, &route, scope, &mut app.ui.corridor_scratch);
        assert!(app
            .ui
            .ahead
            .rows
            .iter()
            .all(|r| matches!(&r.item, Item::Waypoint(w) if w.category == Some(PoiCategory::Water))));
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::Assistant(_)));
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.state.up_ahead_filter, PoiCategorySet::ALL);
        assert_eq!(app.settings().up_ahead_source, UpAheadSource::WaypointsOnly);
    }

    #[test]
    fn timeline_gestures_can_return_forward_after_reversing_to_the_first_page() {
        use crate::{App, AppState, Gesture, Settings};
        let mut bytes = route(false);
        obc_formats::io::put_u16(&mut bytes, obc_formats::obcr::HEADER_LEN + 4, 8);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_settings(Settings { up_ahead_source: UpAheadSource::WaypointsOnly, ..Settings::default() });
        app.set_routes_with_ids(&[route.summary()], &[7]);
        app.navigator.set_active_route(Some(0));
        app.navigator.sync_route_state(Some(&route));
        app.open_whats_next();
        app.apply_gesture(Gesture::Press);
        let scope = scope(UpAheadSource::WaypointsOnly);
        let mut scratch = CorridorScratch::new();
        settle(&mut app.ui.ahead, None, &route, scope, &mut scratch);
        let first: std::vec::Vec<_> = app.ui.ahead.rows.iter().map(|r| r.key).collect();
        assert_eq!(first.len(), 4);
        app.apply_gesture(Gesture::Step(-1));
        assert!(!app.ui.ahead.pending());
        for _ in 0..4 {
            app.apply_gesture(Gesture::Step(1));
        }
        settle(&mut app.ui.ahead, None, &route, scope, &mut scratch);
        let second: std::vec::Vec<_> = app.ui.ahead.rows.iter().map(|r| r.key).collect();
        assert_eq!(second.len(), 4);
        assert!(first[3] < second[0]);
        assert!(!app.ui.ahead.has_next());
        app.apply_gesture(Gesture::Step(-1));
        settle(&mut app.ui.ahead, None, &route, scope, &mut scratch);
        assert_eq!(app.ui.ahead.rows.iter().map(|r| r.key).collect::<std::vec::Vec<_>>(), first);
        assert!(!app.ui.ahead.has_previous());
        assert_eq!(app.ui.ahead.selected, 3);
        app.apply_gesture(Gesture::Step(1));
        settle(&mut app.ui.ahead, None, &route, scope, &mut scratch);
        assert_eq!(app.ui.ahead.rows.iter().map(|r| r.key).collect::<std::vec::Vec<_>>(), second);
        assert_eq!(app.navigator.route_state().active_route, Some(0));
    }
    #[test]
    fn clock_refresh_removes_closed_unselected_rows_without_moving_the_selection() {
        let bytes = route(false);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut morning = [0; 29];
        morning[2] = 4;
        let pois = (1..=3)
            .map(|i| PoiSpec {
                lat: 100,
                lon: i * 3000,
                subtype: 1,
                name: format!("Water {i}"),
                payload: if i == 3 { u16::MAX } else { 0 },
            })
            .collect();
        let bytes =
            obcm_testkit::build_poi_map_with_hours((-1000, -1000, 150_000, 1000), 512, &[(1, pois)], &[morning]);
        let source = obc_reader::SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let map = Reader::new(&source, &tables, &cache);
        let mut a = AheadState::new();
        a.explore();
        let mut scratch = CorridorScratch::new();
        let scope = scope(UpAheadSource::MapPoisOnly);
        scratch.clock_changed(Some((0, 30)), 0);
        for _ in 0..100 {
            a.prepare(Some(&map), Some(&route), &Climbs::default(), scope, &mut scratch, Some((0, 30)));
            if !a.pending() {
                break;
            }
        }
        assert_eq!(a.rows.len(), 3);
        let selected = a.rows[0].key;
        let survivor = a.rows[2].key;
        scratch.clock_changed(Some((0, 90)), 0);
        a.prepare(Some(&map), Some(&route), &Climbs::default(), scope, &mut scratch, Some((0, 90)));
        assert_eq!(a.rows.iter().map(|r| r.key).collect::<std::vec::Vec<_>>(), [selected, survivor]);
        assert_eq!(a.rows[a.selected].key, selected);
        assert_eq!(scratch.entries()[0].poi.opening, OpeningStatus::Closed);
    }

    #[test]
    fn back_keeps_the_overview_until_the_quarter_hour_changes() {
        let bytes = route(false);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut first_hour = [0; 29];
        first_hour[2] = 4;
        let pois = [(3_000, 0), (6_000, u16::MAX)]
            .into_iter()
            .map(|(lon, payload)| PoiSpec { lat: 100, lon, subtype: 1, name: format!("Water {lon}"), payload })
            .collect();
        let bytes =
            obcm_testkit::build_poi_map_with_hours((-1000, -1000, 150_000, 1000), 512, &[(1, pois)], &[first_hour]);
        let source = obc_reader::SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let map = Reader::new(&source, &tables, &cache);
        let scope = scope(UpAheadSource::Both);
        let mut a = AheadState::new();
        let mut scratch = CorridorScratch::new();
        let settle = |a: &mut AheadState, scratch: &mut CorridorScratch, local| {
            for _ in 0..100 {
                a.prepare(Some(&map), Some(&route), &Climbs::default(), scope, scratch, Some(local));
                if !a.pending() {
                    return;
                }
            }
            panic!("window did not settle");
        };
        settle(&mut a, &mut scratch, (0, 30));
        let (water, status) = (a.water, a.status);
        assert!(water.is_some_and(|d| d < 400), "the nearer water is open in the first hour");
        a.explore();
        settle(&mut a, &mut scratch, (0, 30));
        a.back();
        assert!(!a.pending() && a.request(scope).is_none(), "Back takes no new query");
        assert_eq!((a.water, a.status), (water, status));
        a.prepare(Some(&map), Some(&route), &Climbs::default(), scope, &mut scratch, Some((0, 44)));
        assert!(!a.pending(), "a new minute inside the quarter hour changes no opening status");
        settle(&mut a, &mut scratch, (0, 90));
        assert!(a.water.is_some_and(|d| d > 400), "the nearer water closed after the first hour");
    }

    #[test]
    fn failed_route_bytes_are_not_reported_as_a_flat_or_empty_window() {
        use obc_formats::io::ByteSource;
        struct Source<'a> {
            bytes: &'a [u8],
            failed: core::cell::Cell<bool>,
            geometry_end: u64,
        }
        impl ByteSource for Source<'_> {
            fn len(&self) -> u64 {
                self.bytes.len() as u64
            }
            fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
                if self.failed.get()
                    && offset >= obc_formats::obcr::HEADER_FULL_LEN as u64
                    && offset < self.geometry_end
                {
                    Err(Error::Io)
                } else {
                    SliceSource(self.bytes).read_at(offset, out)
                }
            }
        }
        let bytes = route(false);
        let source = Source {
            bytes: &bytes,
            failed: core::cell::Cell::new(false),
            geometry_end: obc_formats::io::rd_u32(&bytes, obc_formats::obcr::HEADER_LEN) as u64,
        };
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        source.failed.set(true);
        let mut a = AheadState::new();
        a.explore();
        let mut scratch = CorridorScratch::new();
        let mut cursor = route.waypoint_cursor().unwrap();
        assert!(route.next_waypoint(&mut cursor).unwrap().is_some(), "authored bytes remain readable");
        for _ in 0..3 {
            settle(&mut a, None, &route, scope(UpAheadSource::WaypointsOnly), &mut scratch);
            assert_eq!(a.status, QueryProgress::Failed(obc_reader::Error::Source(Error::Io)));
            assert!(a.totals.is_none());
            assert!(a.rows.is_empty());
            assert!(a.next_waypoint.is_none());
        }
        source.failed.set(false);
        settle(&mut a, None, &route, scope(UpAheadSource::WaypointsOnly), &mut scratch);
        assert!(matches!(a.status, QueryProgress::Ready { .. }));
        assert!(a.totals.is_some());
        assert!(a.next_waypoint.is_some());
    }
    #[test]
    fn production_frames_keep_the_active_route_and_back_selection() {
        use crate::harness::support::Buf;
        use crate::{App, AppState, Gesture};
        use embedded_graphics::{pixelcolor::Rgb888, prelude::RgbColor};
        let bytes = route(false);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let map = build_poi_map((-1000, -1000, 150_000, 1000), 512, &[]);
        let source = obc_reader::SliceSource(&map);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let map = Reader::new(&source, &tables, &cache);
        let mut app = App::new(AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[route.summary()], &[7]);
        app.navigator.set_active_route(Some(0));
        app.navigator.sync_route_state(Some(&route));
        app.open_whats_next();
        let mut frame = Buf::new(240, 320);
        for name in ["overview", "timeline"] {
            for _ in 0..100 {
                app.render_map(None, &mut frame, &map, Some(&route), 240.0, 320.0, |c| {
                    let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
                    Rgb888::new(r, g, b)
                });
                if !app.ui.ahead.pending() {
                    break;
                }
            }
            assert!(!app.ui.ahead.pending());
            assert_eq!(app.navigator.route_state().active_route, Some(0));
            let (r, g, b) = obc_reader::rgb565_to_rgb888(crate::screen::palette::AMBER);
            assert!(frame.count(Rgb888::new(r, g, b)) > 500);
            if let Ok(dir) = std::env::var("OBC_AHEAD_FRAME_DIR") {
                let mut bmp = vec![0u8; 54];
                bmp[..2].copy_from_slice(b"BM");
                let size = 54 + 240 * 320 * 3;
                bmp[2..6].copy_from_slice(&(size as u32).to_le_bytes());
                bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
                bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
                bmp[18..22].copy_from_slice(&240i32.to_le_bytes());
                bmp[22..26].copy_from_slice(&(-320i32).to_le_bytes());
                bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
                bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
                for p in &frame.px {
                    bmp.extend_from_slice(&[p.b(), p.g(), p.r()]);
                }
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(std::path::Path::new(&dir).join(format!("{name}.bmp")), bmp).unwrap();
            }
            if name == "overview" {
                app.apply_gesture(Gesture::Press);
            }
        }
        app.apply_gesture(Gesture::Step(1));
        let selected = app.ui.ahead.rows[app.ui.ahead.selected].key;
        app.apply_gesture(Gesture::Press);
        assert_eq!(app.ui.ahead.page, Page::Detail);
        app.apply_gesture(Gesture::Back);
        assert_eq!(app.ui.ahead.rows[app.ui.ahead.selected].key, selected);
        assert_eq!(app.navigator.route_state().active_route, Some(0));
    }
}
