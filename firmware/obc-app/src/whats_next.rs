//! A bounded, paged line over the complete accepted route. Drawing reads prepared facts only.

use crate::corridor::{CorridorKey, UpAheadScope};
use heapless::{String, Vec};
use obc_formats::obcm::{settlement_class_of, SettlementClass, SourceId};
use obc_reader::{
    reader::places::{EncounterRange, HoursFilter, PlaceKey, PlaceQuery, PlaceWindow, QueryProgress},
    CorridorPoi, PoiCategory, PoiCategorySet, Reader,
};
use obc_route::{window::RouteWindow, RouteReader, Waypoint, WaypointCursor};

pub(crate) const ROWS: usize = 6;
pub(crate) const SERVICES: [PoiCategory; 7] = [
    PoiCategory::Water,
    PoiCategory::Resupply,
    PoiCategory::Restaurant,
    PoiCategory::Cafe,
    PoiCategory::BikeShop,
    PoiCategory::Train,
    PoiCategory::Fuel,
];
pub(crate) const DEFAULT_FILTER: PoiCategorySet = PoiCategorySet::only(PoiCategory::Water)
    .with(PoiCategory::Resupply)
    .with(PoiCategory::Restaurant)
    .with(PoiCategory::Cafe)
    .with(PoiCategory::BikeShop)
    .with(PoiCategory::Train)
    .with(PoiCategory::Fuel);
const GROUP_M: u32 = 1_000;
const SETTLEMENT_CORRIDOR_M: u16 = 1_150;
const CORE: [PoiCategory; 2] = [PoiCategory::Water, PoiCategory::Resupply];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key(pub u32, pub u8, pub u64);
impl Key {
    pub fn distance(self) -> u32 {
        self.0
    }
}
#[derive(Debug, Clone)]
pub(crate) enum Item {
    Waypoint(Waypoint),
    Settlement(CorridorPoi, SettlementClass),
    Place(CorridorPoi),
    End,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct StopFacts {
    pub ascent_m: Option<u32>,
    pub elevation_m: Option<i16>,
}
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub key: Key,
    pub item: Item,
    pub services: PoiCategorySet,
    pub facts: Option<StopFacts>,
}
impl Row {
    fn new(key: Key, item: Item) -> Self {
        Self { key, item, services: PoiCategorySet::EMPTY, facts: None }
    }
    pub(crate) fn name(&self) -> &str {
        match &self.item {
            Item::Waypoint(w) => &w.name,
            Item::Settlement(p, _) | Item::Place(p) => &p.poi.name,
            Item::End => "",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Timeline,
    Detail,
}

struct CachedOwner {
    source: SourceId,
    occurrence: u32,
    owner: Option<(SourceId, u32)>,
}

struct CachedSettlement {
    source: SourceId,
    encounter: Option<EncounterRange>,
}
struct OwnerCache {
    services: Vec<CachedOwner, 16>,
    settlements: Vec<CachedSettlement, 8>,
}
impl OwnerCache {
    const fn new() -> Self {
        Self { services: Vec::new(), settlements: Vec::new() }
    }
    fn clear(&mut self) {
        self.services.clear();
        self.settlements.clear();
    }
}

/// A source-free cursor and its current page; map and route bytes remain on the card.
struct Scan {
    query: PlaceQuery,
    hits: Vec<CorridorPoi, 8>,
    at: usize,
    status: QueryProgress,
}
impl Scan {
    fn new(query: PlaceQuery) -> Self {
        Self { query, hits: Vec::new(), at: 0, status: QueryProgress::Pending }
    }
    fn step(&mut self, reader: &Reader, route: &RouteReader) {
        for _ in 0..64 {
            if self.status != QueryProgress::Pending {
                break;
            }
            self.status = self.query.step(reader, Some(route), 0, &mut self.hits);
        }
    }
    fn take(&mut self, backwards: bool) -> Option<CorridorPoi> {
        if matches!(self.status, QueryProgress::Ready { .. }) {
            let index = if backwards { self.hits.len().checked_sub(self.at + 1)? } else { self.at };
            let hit = self.hits.get(index)?.clone();
            self.at += 1;
            Some(hit)
        } else {
            None
        }
    }
    fn advance(&mut self, backwards: bool) -> bool {
        if !matches!(self.status, QueryProgress::Ready { more: true, .. }) {
            return false;
        }
        let edge = if backwards { self.hits.first() } else { self.hits.last() };
        let Some(hit) = edge else {
            return false;
        };
        let key = self.query.key(hit);
        if backwards {
            self.query.previous_page(key);
        } else {
            self.query.next_page(key);
        }
        self.hits.clear();
        self.at = 0;
        self.status = QueryProgress::Pending;
        true
    }
}

pub struct AheadState {
    pub(crate) window: Option<RouteWindow>,
    pub(crate) page: Page,
    pub(crate) rows: Vec<Row, ROWS>,
    pub(crate) selected: usize,
    pub(crate) scroll: i32,
    pub(crate) status: QueryProgress,
    pub(crate) stale: bool,
    pub(crate) detail_rows: Vec<CorridorPoi, 8>,
    pub(crate) detail_selected: usize,
    pub(crate) detail_more: bool,
    anchor: u32,
    filter: PoiCategorySet,
    boundary: Option<Key>,
    backwards: bool,
    more: bool,
    dirty: bool,
    cursor: Option<WaypointCursor>,
    ordinal: u16,
    authored_done: bool,
    scan: Option<Scan>,
    work: Option<Scan>,
    candidate: Option<Row>,
    map_done: bool,
    detail_facts_dirty: bool,
    detail_dirty: bool,
    detail_boundary: Option<PlaceKey>,
    detail_backwards: bool,
    owners: OwnerCache,
    pub(crate) detail_previous: bool,
    pub(crate) route_name: String<64>,
}
impl AheadState {
    pub const fn new() -> Self {
        Self {
            window: None,
            page: Page::Timeline,
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            status: QueryProgress::Unavailable,
            stale: false,
            detail_rows: Vec::new(),
            detail_selected: 0,
            detail_more: false,
            anchor: 0,
            filter: PoiCategorySet::ALL,
            boundary: None,
            backwards: false,
            more: false,
            dirty: true,
            cursor: None,
            ordinal: 0,
            authored_done: false,
            scan: None,
            work: None,
            candidate: None,
            map_done: false,
            detail_facts_dirty: false,
            detail_dirty: false,
            detail_boundary: None,
            detail_backwards: false,
            owners: OwnerCache::new(),
            detail_previous: false,
            route_name: String::new(),
        }
    }
    pub(crate) fn open(&mut self, anchor: u32) {
        *self = Self::new();
        self.anchor = anchor;
    }
    pub(crate) fn refresh(&mut self, anchor: u32) {
        self.open(anchor);
    }
    pub(crate) fn invalidate(&mut self) {
        self.stale = true;
        self.window = None;
        self.rows.clear();
        self.scan = None;
        self.work = None;
        self.status = QueryProgress::Unavailable;
    }
    pub(crate) fn pending(&self) -> bool {
        !self.stale
            && (self.dirty || !self.authored_done || !self.map_done || self.detail_facts_dirty || self.detail_dirty)
    }
    pub(crate) fn request(&self, scope: UpAheadScope) -> Option<CorridorKey> {
        (self.pending() || self.filter != scope.filter).then_some(CorridorKey {
            filter: scope.filter,
            hours_filter: HoursFilter::All,
            anchor_m: self.anchor,
        })
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
        if let Some(row) = if backwards { self.rows.first() } else { self.rows.last() } {
            self.boundary = Some(row.key);
            self.backwards = backwards;
            self.dirty = true;
        }
    }
    pub(crate) fn open_detail(&mut self) {
        self.page = Page::Detail;
        self.detail_selected = 0;
        self.detail_boundary = None;
        self.detail_backwards = false;
        self.detail_previous = false;
        self.detail_rows.clear();
        self.detail_facts_dirty = self.rows.get(self.selected).is_some_and(|r| r.facts.is_none());
        self.detail_dirty = matches!(self.rows.get(self.selected).map(|r| &r.item), Some(Item::Settlement(..)));
        self.work = None;
    }
    pub(crate) fn detail_page(&mut self, backwards: bool) {
        if let Some(p) = if backwards { self.detail_rows.first() } else { self.detail_rows.last() } {
            self.detail_boundary = Some(place_key(p));
            self.detail_backwards = backwards;
            self.detail_dirty = true;
            self.work = None;
        }
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
}
impl Default for AheadState {
    fn default() -> Self {
        Self::new()
    }
}
pub(crate) fn place_key(p: &CorridorPoi) -> PlaceKey {
    PlaceKey { distance_m: p.poi.distance_m, source: p.poi.metadata.source, occurrence: p.dist_along_m }
}
impl crate::App {
    pub fn ahead_debug(&self, mut visit: impl FnMut(&str, u32, u64, i32, i32)) {
        for row in &self.ui.ahead.rows {
            if let Item::Settlement(p, _) | Item::Place(p) = &row.item {
                visit(row.name(), row.key.0, p.poi.metadata.source.0, p.poi.lon, p.poi.lat);
            }
        }
    }

    pub fn ahead_base_active(&self) -> bool {
        matches!(crate::screen::base_screen(&self.ui.stack), Some(crate::screen::Screen::WhatsNext(_)))
    }

    pub fn ahead_preparing(&self) -> bool {
        matches!(self.top_screen(), crate::screen::Screen::WhatsNext(_)) && self.ui.ahead.pending()
    }

    /// Advance the query without repainting an unchanged loading screen.
    pub fn prepare_ahead(&mut self, reader: Option<&Reader>, route: Option<&RouteReader>) {
        if self.ui.stack.iter().any(|s| matches!(s, crate::screen::Screen::WhatsNext(_))) {
            let scope = self.up_ahead_scope();
            self.ui.ahead.prepare(reader, route, scope, self.place_local_time());
            self.ui.reconcile_corridor(scope);
        }
    }

    pub fn open_whats_next(&mut self) {
        self.ui.ahead.open(self.navigator.route_state().progress_m);
        self.state.up_ahead_filter = DEFAULT_FILTER;
        crate::screen::apply(
            &mut self.ui.stack,
            crate::screen::Transition::Push(crate::screen::Screen::WhatsNext(crate::screen::WhatsNextScreen::new())),
        );
        self.ui.reconcile_corridor(self.up_ahead_scope());
        self.ui.map_dirty = true;
    }
}

fn service_on_route(p: &CorridorPoi) -> bool {
    obc_formats::obcm::poi_category_of(p.poi.subtype)
        .is_some_and(|c| p.offset_m.unsigned_abs() <= if CORE.contains(&c) { 100 } else { 150 })
}
fn settlement_area(pos: (i32, i32)) -> obc_map_scene::BBox {
    let pad_lat = 9_000;
    let pad_lon = (pad_lat as f32 / obc_map_scene::cos_lat(pos.1).max(0.01)) as i32;
    obc_map_scene::BBox {
        min_lon: pos.0.saturating_sub(pad_lon),
        max_lon: pos.0.saturating_add(pad_lon),
        min_lat: pos.1.saturating_sub(pad_lat),
        max_lat: pos.1.saturating_add(pad_lat),
    }
}

/// OSM can describe one settlement as both a place node and an area. Prefer its named node;
/// distinct place nodes and later route visits keep their own identities.
fn settlement_duplicate(
    reader: &Reader,
    id: SourceId,
    position: (i32, i32),
    name: &str,
    class: SettlementClass,
) -> Result<bool, obc_reader::Error> {
    if id.0 >> 62 == 1 {
        return Ok(false);
    }
    let cl = obc_map_scene::cos_lat(position.1);
    let mut duplicate = false;
    reader.visit_settlements_in(&settlement_area(position), |s| {
        duplicate |= s.source.0 >> 62 == 1
            && s.class == class
            && s.name == name
            && obc_map_scene::ground_dist_m_cl(position, (s.lon, s.lat), cl) <= GROUP_M as f32;
    })?;
    Ok(duplicate)
}

fn owner(
    cache: &mut OwnerCache,
    reader: &Reader,
    route: &RouteReader,
    p: &CorridorPoi,
    anchor: u32,
) -> Result<Option<(SourceId, u32)>, obc_reader::Error> {
    let key = (p.poi.metadata.source, p.dist_along_m);
    if let Some(hit) = cache.services.iter().find(|hit| (hit.source, hit.occurrence) == key) {
        return Ok(hit.owner);
    }
    use obc_map_scene::{cos_lat, ground_dist_m_cl};
    let pos = (p.poi.lon, p.poi.lat);
    let cl = cos_lat(pos.1);
    let area = settlement_area(pos);
    let mut error = None;
    let mut nearest: Option<(u32, SourceId, (i32, i32))> = None;
    reader.visit_settlements_in(&area, |s| {
        let d = ground_dist_m_cl(pos, (s.lon, s.lat), cl) as u32;
        let rank = (d, s.source, (s.lon, s.lat));
        if d <= GROUP_M && nearest.is_none_or(|old| rank < old) {
            match settlement_duplicate(reader, s.source, (s.lon, s.lat), &s.name, s.class) {
                Ok(false) => nearest = Some(rank),
                Ok(true) => {}
                Err(e) => error = Some(e),
            }
        }
    })?;
    if let Some(error) = error {
        return Err(error);
    }
    let owner = match nearest {
        Some((_, id, position)) => {
            let encounter = if let Some(hit) = cache
                .settlements
                .iter()
                .find(|hit| hit.source == id && hit.encounter.is_none_or(|e| e.contains(p.dist_along_m)))
            {
                hit.encounter
            } else {
                let encounter = reader.nearest_encounter(route, position, SETTLEMENT_CORRIDOR_M, p.dist_along_m)?;
                if cache.settlements.is_full() {
                    cache.settlements.remove(0);
                }
                let _ = cache.settlements.push(CachedSettlement { source: id, encounter });
                encounter
            };
            encounter.filter(|e| e.along_m >= anchor).map(|e| (id, e.along_m))
        }
        None => None,
    };
    if cache.services.is_full() {
        cache.services.remove(0);
    }
    let _ = cache.services.push(CachedOwner { source: key.0, occurrence: key.1, owner });
    Ok(owner)
}
fn query(filter: PoiCategorySet, from: u32, to: u32, width: u16, local: Option<(u8, u16)>) -> PlaceQuery {
    PlaceQuery::new(0, filter, PlaceWindow::Corridor { from_m: from, to_m: to, half_width_m: width }, local)
        .with_hours_filter(HoursFilter::All)
}
impl AheadState {
    fn fail(&mut self, status: QueryProgress) {
        self.status = status;
        self.scan = None;
        self.work = None;
        self.candidate = None;
        self.dirty = false;
        self.authored_done = true;
        self.map_done = true;
        self.detail_facts_dirty = false;
        self.detail_dirty = false;
    }
    fn services_query(&self, stop: &CorridorPoi, local: Option<(u8, u16)>) -> PlaceQuery {
        let window = self.window.unwrap();
        query(self.filter, window.start_m, window.end_m, 150, local).within((stop.poi.lon, stop.poi.lat), GROUP_M)
    }
    pub(crate) fn prepare(
        &mut self,
        reader: Option<&Reader>,
        route: Option<&RouteReader>,
        scope: UpAheadScope,
        local: Option<(u8, u16)>,
    ) {
        if self.stale {
            return;
        }
        let Some(route) = route else {
            self.fail(QueryProgress::Unavailable);
            return;
        };
        if self.window.is_some_and(|w| !w.matches(route)) {
            self.invalidate();
            return;
        }
        if self.filter != scope.filter {
            self.filter = scope.filter;
            self.boundary = None;
            self.backwards = false;
            self.dirty = true;
        }
        if self.dirty {
            self.owners.clear();
            self.rows.clear();
            self.selected = 0;
            self.scroll = 0;
            self.more = false;
            self.ordinal = 0;
            self.detail_facts_dirty = false;
            self.work = None;
            self.candidate = None;
            let window = RouteWindow {
                identity: route.identity(),
                start_m: self.anchor.min(route.total_distance_m),
                end_m: route.total_distance_m,
            };
            self.window = Some(window);
            self.route_name.clear();
            let _ = self.route_name.push_str(route.name());
            match route.waypoint_cursor() {
                Ok(cursor) => self.cursor = Some(cursor),
                Err(e) => {
                    self.fail(QueryProgress::Failed(obc_reader::Error::Source(e)));
                    return;
                }
            }
            self.authored_done = false;
            let standalone =
                CORE.into_iter().filter(|&c| self.filter.contains(c)).fold(PoiCategorySet::EMPTY, |s, c| s.with(c));
            let mut q =
                query(standalone, window.start_m, window.end_m, 100, local).with_settlements(SETTLEMENT_CORRIDOR_M);
            if let Some(b) = self.boundary {
                // At equal distance, authored waypoints precede map stops and the route end follows them.
                q = q.starting_after(
                    PlaceKey {
                        distance_m: b.0.saturating_sub(window.start_m),
                        source: SourceId(match b.1 {
                            0 => 0,
                            1 => b.2,
                            _ => u64::MAX,
                        }),
                        occurrence: if b.1 == 2 { u32::MAX } else { b.0 },
                    },
                    self.backwards,
                );
            }
            self.scan = reader.map(|_| Scan::new(q));
            self.map_done = false;
            self.status = if reader.is_some() { QueryProgress::Pending } else { QueryProgress::Unavailable };
            self.insert(Row::new(Key(window.end_m, 2, 0), Item::End));
            self.dirty = false;
        }
        if !self.authored_done {
            for _ in 0..16 {
                match route.next_waypoint(self.cursor.as_mut().unwrap()) {
                    Ok(Some(w)) => {
                        let index = self.ordinal;
                        self.ordinal += 1;
                        if self.window.unwrap().contains(w.dist_along_m) {
                            self.insert(Row::new(Key(w.dist_along_m, 0, u64::from(index)), Item::Waypoint(w)));
                        }
                    }
                    Ok(None) => {
                        self.authored_done = true;
                        break;
                    }
                    Err(e) => {
                        self.fail(QueryProgress::Failed(obc_reader::Error::Source(e)));
                        return;
                    }
                }
            }
            return;
        }
        if !self.map_done {
            if let Some(reader) = reader {
                if let Err(e) = self.prepare_map(reader, route, local) {
                    self.fail(QueryProgress::Failed(e));
                }
            } else {
                self.finish_map();
            }
            return;
        }
        if self.detail_facts_dirty {
            self.detail_facts_dirty = false;
            if let Err(e) = self.prepare_detail_facts(route) {
                self.fail(QueryProgress::Failed(obc_reader::Error::Source(e)));
            }
            return;
        }
        if self.detail_dirty {
            if let Some(reader) = reader {
                if let Err(e) = self.prepare_detail(reader, route, local) {
                    self.fail(QueryProgress::Failed(e));
                }
            } else {
                self.fail(QueryProgress::Unavailable);
            }
        }
    }
    fn prepare_map(
        &mut self,
        reader: &Reader,
        route: &RouteReader,
        local: Option<(u8, u16)>,
    ) -> Result<(), obc_reader::Error> {
        if self.candidate.is_some() {
            let work = self.work.as_mut().unwrap();
            work.step(reader, route);
            match work.status {
                QueryProgress::Pending => return Ok(()),
                QueryProgress::Failed(e) => return Err(e),
                _ => {}
            }
            while let Some(p) = work.take(false) {
                let row = self.candidate.as_mut().unwrap();
                let Item::Settlement(stop, _) = &row.item else { unreachable!() };
                if service_on_route(&p)
                    && owner(&mut self.owners, reader, route, &p, self.anchor)?
                        == Some((stop.poi.metadata.source, stop.dist_along_m))
                {
                    if let Some(cat) = obc_formats::obcm::poi_category_of(p.poi.subtype) {
                        row.services = row.services.with(cat);
                    }
                }
            }
            if work.advance(false) {
                return Ok(());
            }
            let row = self.candidate.take().unwrap();
            self.work = None;
            if !row.services.is_empty() {
                self.insert(row);
            }
            return Ok(());
        }
        let scan = self.scan.as_mut().unwrap();
        scan.step(reader, route);
        match scan.status {
            QueryProgress::Pending => return Ok(()),
            QueryProgress::Failed(e) => return Err(e),
            _ => {}
        }
        let hit = scan.take(self.backwards);
        if let Some(p) = hit {
            let key = Key(p.dist_along_m, 1, p.poi.metadata.source.0);
            if !self.keep(key) {
                return Ok(());
            }
            if self.rows.is_full()
                && if self.backwards {
                    key < self.rows.first().unwrap().key
                } else {
                    key > self.rows.last().unwrap().key
                }
            {
                self.more = true;
                self.finish_map();
                return Ok(());
            }
            if let Some(class) = settlement_class_of(p.poi.subtype) {
                if settlement_duplicate(reader, p.poi.metadata.source, (p.poi.lon, p.poi.lat), &p.poi.name, class)? {
                    return Ok(());
                }
                self.work = Some(Scan::new(self.services_query(&p, local)));
                self.candidate = Some(Row::new(key, Item::Settlement(p, class)));
            } else if service_on_route(&p) && owner(&mut self.owners, reader, route, &p, self.anchor)?.is_none() {
                if let Some(cat) = obc_formats::obcm::poi_category_of(p.poi.subtype).filter(|c| CORE.contains(c)) {
                    let mut row = Row::new(key, Item::Place(p));
                    row.services = PoiCategorySet::only(cat);
                    self.insert(row);
                }
            }
        } else if !scan.advance(self.backwards) {
            self.finish_map();
        }
        Ok(())
    }
    fn finish_map(&mut self) {
        self.status = self.scan.as_ref().map_or(QueryProgress::Unavailable, |s| s.status);
        self.scan = None;
        self.map_done = true;
        if self.backwards {
            if self.rows.is_empty() {
                self.boundary = None;
                self.backwards = false;
                self.dirty = true;
            } else {
                self.selected = self.rows.len() - 1;
                self.scroll = 320;
            }
        }
    }
    fn prepare_detail_facts(&mut self, route: &RouteReader) -> Result<(), obc_formats::io::Error> {
        let Some(row) = self.rows.get_mut(self.selected) else {
            return Ok(());
        };
        let facts = route.interval_facts(self.window.unwrap().start_m, row.key.0)?;
        row.facts = Some(StopFacts {
            ascent_m: facts.complete_elevation().then_some(facts.ascent_m),
            elevation_m: match &row.item {
                Item::Waypoint(w) if w.ele != i16::MIN => Some(w.ele),
                _ => route.elevation_at(row.key.0),
            },
        });
        Ok(())
    }
    fn prepare_detail(
        &mut self,
        reader: &Reader,
        route: &RouteReader,
        local: Option<(u8, u16)>,
    ) -> Result<(), obc_reader::Error> {
        let Some(Row { item: Item::Settlement(stop, _), .. }) = self.rows.get(self.selected) else {
            self.detail_dirty = false;
            return Ok(());
        };
        if self.work.is_none() {
            let mut q = self.services_query(stop, local);
            if let Some(b) = self.detail_boundary {
                q = q.starting_after(b, self.detail_backwards);
            }
            self.work = Some(Scan::new(q));
            self.detail_rows.clear();
        }
        let work = self.work.as_mut().unwrap();
        work.step(reader, route);
        match work.status {
            QueryProgress::Pending => return Ok(()),
            QueryProgress::Failed(e) => return Err(e),
            _ => {}
        }
        let mut more = false;
        while let Some(p) = work.take(self.detail_backwards) {
            if service_on_route(&p)
                && owner(&mut self.owners, reader, route, &p, self.anchor)?
                    == Some((stop.poi.metadata.source, stop.dist_along_m))
            {
                if self.detail_rows.is_full() {
                    more = true;
                    break;
                }
                if self.detail_backwards {
                    let _ = self.detail_rows.insert(0, p);
                } else {
                    let _ = self.detail_rows.push(p);
                }
            }
        }
        // Only an eligible service beyond this page proves that another page exists.
        if !more && work.advance(self.detail_backwards) {
            return Ok(());
        }
        self.detail_more = if self.detail_backwards { self.detail_boundary.is_some() } else { more };
        self.detail_previous = if self.detail_backwards { more } else { self.detail_boundary.is_some() };
        self.detail_selected = if self.detail_backwards { self.detail_rows.len().saturating_sub(1) } else { 0 };
        self.detail_dirty = false;
        self.work = None;
        Ok(())
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

    fn settle(a: &mut AheadState, map: Option<&Reader>, route: &RouteReader) {
        for _ in 0..10_000 {
            a.prepare(
                map,
                Some(route),
                UpAheadScope { filter: DEFAULT_FILTER, source: crate::settings::UpAheadSource::Both },
                None,
            );
            if !a.pending() {
                return;
            }
        }
        panic!("line did not settle");
    }
    #[test]
    fn full_route_pages_keep_every_authored_waypoint_and_return_in_reverse() {
        let bytes = route(false);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut a = AheadState::new();
        let mut pages = std::vec::Vec::new();
        loop {
            settle(&mut a, None, &route);
            let keys: std::vec::Vec<_> = a.rows.iter().map(|r| r.key).collect();
            assert!(!keys.is_empty());
            pages.push(keys);
            if !a.has_next() {
                break;
            }
            a.turn_page(false);
        }
        assert_eq!(pages.iter().map(|p| p.len()).sum::<usize>(), 41);
        assert!(matches!(a.rows.last().unwrap().item, Item::End));
        let all: std::vec::Vec<_> = pages.iter().flatten().copied().collect();
        assert!(all.windows(2).all(|p| p[0] < p[1]));
        let mut backwards = std::vec::Vec::new();
        backwards.extend(a.rows.iter().rev().map(|r| r.key));
        while a.has_previous() {
            a.turn_page(true);
            settle(&mut a, None, &route);
            backwards.extend(a.rows.iter().rev().map(|r| r.key));
        }
        backwards.reverse();
        assert_eq!(backwards, all);
        a.turn_page(false);
        settle(&mut a, None, &route);
        assert!(a.rows[0].key > all[0]);
    }
    fn spec(lon: i32, subtype: u8, name: &str) -> PoiSpec {
        PoiSpec { lat: 0, lon, subtype, name: name.into(), payload: u16::MAX }
    }
    #[test]
    fn settlement_nodes_replace_duplicate_areas_and_keep_distinct_same_name_places() {
        let route_bytes = route(false);
        let route_source = SliceSource(&route_bytes);
        let index = RouteIndex::read(&route_source).unwrap();
        let route = RouteReader::new(&index, &route_source);
        let mut bytes = build_poi_map(
            (-20_000, -20_000, 160_000, 20_000),
            512,
            &[
                (9, vec![spec(10_000, 22, "Waldkirch"), spec(12_000, 22, "Waldkirch"), spec(50_000, 22, "Waldkirch")]),
                (1, vec![spec(11_900, 1, "Water"), spec(50_100, 1, "Other water")]),
                (11, vec![spec(9_800, 26, "Cafe")]),
            ],
        );
        let area = obcm_testkit::pack_poi_record(0, 12_000, 22, "Waldkirch", u16::MAX);
        let offset = bytes.windows(area.len()).position(|record| record == area).unwrap();
        bytes[offset + 36..offset + 44].copy_from_slice(&SourceId::osm(2, 99).0.to_le_bytes());
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut state = AheadState::new();
        let mut towns = std::vec::Vec::new();
        loop {
            settle(&mut state, Some(&reader), &route);
            towns.extend(state.rows.iter().filter(|r| matches!(r.item, Item::Settlement(..))).cloned());
            if !state.has_next() {
                break;
            }
            state.turn_page(false);
        }
        assert_eq!(towns.len(), 2, "the area is redundant, the distant place node is distinct");
        assert!(towns[0].services.contains(PoiCategory::Water), "water nearest the area belongs to the town node");
        assert!(towns[0].services.contains(PoiCategory::Cafe));
        assert!(towns[1].services.contains(PoiCategory::Water));
        state.turn_page(true);
        settle(&mut state, Some(&reader), &route);
        assert!(state
            .rows
            .iter()
            .all(|r| !matches!(&r.item, Item::Settlement(p, _) if p.poi.metadata.source == SourceId::osm(2, 99))));
    }

    #[test]
    fn settlements_collect_only_near_route_services_and_hide_empty_places() {
        let bytes = route(false);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut far = spec(10_000, 25, "Far restaurant");
        far.lat = 2_000;
        let mut village = spec(10_000, 23, "Village");
        village.lat = 7_000;
        let bytes = build_poi_map(
            (-20_000, -20_000, 160_000, 20_000),
            512,
            &[
                (9, vec![village, spec(90_000, 24, "Empty")]),
                (1, vec![spec(10_100, 1, "Water"), spec(40_000, 1, "Fountain")]),
                (10, vec![spec(10_200, 25, "Restaurant"), far]),
                (11, vec![spec(10_300, 26, "Cafe")]),
                (12, vec![spec(10_400, 27, "Fuel")]),
            ],
        );
        let src = obc_reader::SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut app = crate::App::new(crate::AppState::new(0, 0, 1.0));
        app.set_routes_with_ids(&[route.summary()], &[7]);
        app.navigator.set_active_route(Some(0));
        app.navigator.sync_route_state(Some(&route));
        app.open_whats_next();
        let mut places = std::vec::Vec::new();
        loop {
            for _ in 0..20_000 {
                app.prepare_ahead(Some(&reader), Some(&route));
                if !app.ahead_preparing() {
                    break;
                }
            }
            assert!(!app.ahead_preparing(), "background work completes without drawing");
            let a = &mut app.ui.ahead;
            assert!(!matches!(a.status, QueryProgress::Failed(_)), "{:?}", a.status);
            for r in &a.rows {
                if matches!(r.item, Item::Settlement(..) | Item::Place(_)) {
                    places.push(r.clone());
                }
            }
            if !a.has_next() {
                break;
            }
            a.turn_page(false);
        }
        assert_eq!(places.len(), 2);
        let village = places.iter().find(|r| r.name() == "Village").unwrap();
        assert!(village.services.contains(PoiCategory::Water));
        assert!(village.services.contains(PoiCategory::Restaurant));
        assert!(village.services.contains(PoiCategory::Cafe));
        assert!(village.services.contains(PoiCategory::Fuel));
        assert!(!village.services.contains(PoiCategory::Resupply));
        assert_eq!(places[1].name(), "Fountain");
        let a = &mut app.ui.ahead;
        a.rows.clear();
        a.rows.push(village.clone()).unwrap();
        a.selected = 0;
        a.open_detail();
        settle(a, Some(&reader), &route);
        assert_eq!(a.detail_rows.len(), 4);
        assert!(a.detail_rows.iter().all(|p| p.poi.name != "Far restaurant"));
    }
    #[test]
    fn grouping_keeps_two_visits_to_the_same_village_separate() {
        let mut sink = Sink::default();
        let gpx = br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"/><trkpt lat="0" lon="0.01"/><trkpt lat="0" lon="0.04"/><trkpt lat="0" lon="0.01"/><trkpt lat="0" lon="0"/></trkseg></trk></gpx>"#;
        obc_route::gpx_to_obcr(&SliceSource(gpx), "Out and back", &mut sink).unwrap();
        let source = SliceSource(&sink.0);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let bytes = build_poi_map(
            (-20_000, -20_000, 60_000, 20_000),
            512,
            &[(9, vec![spec(10_000, 23, "Village")]), (1, vec![spec(10_100, 1, "Fountain")])],
        );
        let src = obc_reader::SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut a = AheadState::new();
        settle(&mut a, Some(&reader), &route);
        assert_eq!(a.rows.len(), 3);
        for selected in 0..2 {
            assert_eq!(a.rows[selected].name(), "Village");
            assert!(a.rows[selected].services.contains(PoiCategory::Water));
            a.selected = selected;
            a.open_detail();
            settle(&mut a, Some(&reader), &route);
            assert_eq!(a.detail_rows.len(), 1);
            assert!(a.detail_rows[0].dist_along_m.abs_diff(a.rows[selected].key.0) < 20);
        }
    }

    #[test]
    fn water_ahead_of_a_passed_settlement_remains_a_standalone_stop() {
        let bytes = route(false);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let bytes = build_poi_map(
            (-20_000, -20_000, 160_000, 20_000),
            512,
            &[(9, vec![spec(10_000, 23, "Passed village")]), (1, vec![spec(11_000, 1, "Water ahead")])],
        );
        let src = obc_reader::SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut a = AheadState::new();
        a.open(1_200);
        settle(&mut a, Some(&reader), &route);
        assert!(a.rows.iter().any(|r| r.name() == "Water ahead" && matches!(r.item, Item::Place(_))));
        assert!(!a.rows.iter().any(|r| matches!(r.item, Item::Settlement(..))));
    }

    #[test]
    fn settlement_detail_pages_skip_neighbor_services_in_both_directions() {
        let mut sink = Sink::default();
        let gpx = br#"<gpx><trk><trkseg><trkpt lat="0" lon="0"/><trkpt lat="0" lon="0.025"/></trkseg></trk></gpx>"#;
        obc_route::gpx_to_obcr(&SliceSource(gpx), "Three villages", &mut sink).unwrap();
        let source = SliceSource(&sink.0);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        for count in [1, 17] {
            let mut water: std::vec::Vec<_> = (0..8)
                .flat_map(|i| [spec(3_800 + i * 100, 1, "West water"), spec(15_800 + i * 100, 1, "East water")])
                .collect();
            water.extend((0..count).map(|i| spec(9_500 + i * 60, 1, &format!("Local water {i}"))));
            let bytes = build_poi_map(
                (-20_000, -20_000, 60_000, 20_000),
                512,
                &[
                    (9, vec![spec(4_000, 23, "West"), spec(10_000, 23, "Village"), spec(16_000, 23, "East")]),
                    (1, water),
                ],
            );
            let src = SliceSource(&bytes);
            let tables = MapTables::parse(&src).unwrap();
            let cache = MapCache::new();
            let reader = Reader::new(&src, &tables, &cache);
            let mut a = AheadState::new();
            settle(&mut a, Some(&reader), &route);
            a.selected = a.rows.iter().position(|r| r.name() == "Village").unwrap();
            a.open_detail();
            let mut forward = std::vec::Vec::new();
            loop {
                settle(&mut a, Some(&reader), &route);
                assert!(!a.detail_rows.is_empty());
                assert!(a.detail_rows.iter().all(|p| p.poi.name.starts_with("Local water")));
                forward.extend(a.detail_rows.iter().map(place_key));
                assert!(forward.len() <= count as usize);
                if !a.detail_more {
                    break;
                }
                assert_eq!(a.detail_rows.len(), 8);
                a.detail_page(false);
            }
            assert_eq!(forward.len(), count as usize);
            let mut backward: std::vec::Vec<_> = a.detail_rows.iter().rev().map(place_key).collect();
            while a.detail_previous {
                a.detail_page(true);
                settle(&mut a, Some(&reader), &route);
                assert!(!a.detail_rows.is_empty());
                backward.extend(a.detail_rows.iter().rev().map(place_key));
                assert!(backward.len() <= count as usize);
            }
            backward.reverse();
            assert_eq!(backward, forward);
        }
    }

    #[test]
    fn backward_pages_keep_services_at_the_route_end() {
        let mut sink = Sink::default();
        let gpx = br#"<gpx><wpt lat="0" lon="0.002"><name>Earlier</name></wpt><trk><trkseg><trkpt lat="0" lon="0"/><trkpt lat="0" lon="0.01"/></trkseg></trk></gpx>"#;
        obc_route::gpx_to_obcr(&SliceSource(gpx), "End services", &mut sink).unwrap();
        let source = SliceSource(&sink.0);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let water = (0..5)
            .map(|i| {
                let mut p = spec(10_000, 1, &format!("Water {i}"));
                p.lat = (i - 2) * 100;
                p
            })
            .collect();
        let bytes = build_poi_map((-20_000, -20_000, 60_000, 20_000), 512, &[(1, water)]);
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut a = AheadState::new();
        settle(&mut a, Some(&reader), &route);
        assert_eq!(a.rows.len(), 6);
        assert!(matches!(a.rows[0].item, Item::Waypoint(_)));
        assert!(a.rows[1..].iter().all(|r| matches!(r.item, Item::Place(_)) && r.key.0 == route.total_distance_m));
        let first: std::vec::Vec<_> = a.rows.iter().map(|r| r.key).collect();
        assert!(a.has_next());
        a.turn_page(false);
        settle(&mut a, Some(&reader), &route);
        assert_eq!(a.rows.len(), 1);
        assert!(matches!(a.rows[0].item, Item::End));
        assert!(a.has_previous());
        a.turn_page(true);
        settle(&mut a, Some(&reader), &route);
        assert_eq!(a.rows.iter().map(|r| r.key).collect::<std::vec::Vec<_>>(), first);
    }

    #[test]
    fn stop_facts_read_geometry_only_when_details_open_and_are_reused() {
        use core::cell::Cell;
        use obc_formats::io::ByteSource;
        struct Counted<'a> {
            bytes: &'a [u8],
            geometry: core::ops::Range<u64>,
            reads: Cell<usize>,
        }
        impl ByteSource for Counted<'_> {
            fn len(&self) -> u64 {
                self.bytes.len() as u64
            }
            fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
                if offset < self.geometry.end && offset + out.len() as u64 > self.geometry.start {
                    self.reads.set(self.reads.get() + 1);
                }
                SliceSource(self.bytes).read_at(offset, out)
            }
        }
        let bytes = route(false);
        let index = RouteIndex::read(&SliceSource(&bytes)).unwrap();
        let chunk = index.chunks()[0];
        let source = Counted {
            bytes: &bytes,
            geometry: u64::from(chunk.byte_offset)..u64::from(chunk.byte_offset + chunk.byte_len),
            reads: Cell::new(0),
        };
        let route = RouteReader::new(&index, &source);
        let mut a = AheadState::new();
        a.open(1_000);
        settle(&mut a, None, &route);
        assert_eq!(source.reads.get(), 0, "the timeline does not decode route geometry for elevation");
        assert!(a.rows.iter().all(|r| r.facts.is_none()));
        a.open_detail();
        assert!(a.pending());
        settle(&mut a, None, &route);
        assert!(source.reads.get() > 0);
        let row = &a.rows[a.selected];
        let expected = route.interval_facts(1_000, row.key.0).unwrap();
        assert_eq!(row.facts.unwrap().ascent_m, Some(expected.ascent_m));
        assert!(row.facts.unwrap().elevation_m.is_some());
        let reads = source.reads.get();
        a.open_detail();
        assert!(!a.pending());
        settle(&mut a, None, &route);
        assert_eq!(source.reads.get(), reads, "reopening details reuses the facts");
    }

    #[test]
    fn missing_elevation_stays_unknown_and_changed_routes_invalidate_the_line() {
        let bytes = route(true);
        let source = SliceSource(&bytes);
        let index = RouteIndex::read(&source).unwrap();
        let route = RouteReader::new(&index, &source);
        let mut a = AheadState::new();
        settle(&mut a, None, &route);
        while a.has_next() {
            a.turn_page(false);
            settle(&mut a, None, &route);
        }
        assert!(a.rows.iter().all(|r| r.facts.is_none()));
        a.selected = a.rows.len() - 1;
        a.open_detail();
        settle(&mut a, None, &route);
        assert_eq!(a.rows.last().unwrap().facts.unwrap().ascent_m, None);
        let replacement = RouteIndex::read(&source).unwrap();
        let replacement = RouteReader::new(&replacement, &source);
        settle(&mut a, None, &replacement);
        assert!(a.stale && a.window.is_none() && a.rows.is_empty());
    }
}
