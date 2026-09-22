//! Paged nearby places. Geometry, membership and order are frozen for each browsing generation.
//! The shared query advances during prepare; current opening status refreshes without moving
//! the selected identity. The App owns the one page buffer so screen-stack slots stay small.

use embedded_graphics::prelude::Point;
use obc_formats::obcm::poi_label_of;
use obc_map_scene::cos_lat;
use obc_reader::reader::places::{PlaceKey, PlaceQuery, PlaceWindow, QueryProgress, PLACE_PAGE_SIZE};
use obc_reader::{Poi, PoiCategory};
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::Units;
use crate::Msg;
use obc_ports::Fix;

use super::vocab::chrome::{empty_state, stroke2};
use super::vocab::fmt::write_distance_coarse;
use super::vocab::list::{self, ListGeometry, Separators};
use super::vocab::marquee::MarqueeFrame;
use super::{palette, Ctx, PoiDetailScreen, Render, Screen, Transition};

/// Per-POI nominal row height: two lines, the name above the bearing arrow and the distance. The
/// drawn pitch stretches from this, so the rows fill the whole viewport.
const ROW_H: i32 = 64;

/// The App owns one bounded nearby page, outside the screen stack so each stack slot stays small.
pub struct PoiScratch {
    pub(crate) query: Option<PlaceQuery>,
    pub(crate) hours_filter: obc_reader::reader::places::HoursFilter,
    pub(crate) detail_valid: bool,
    pub(crate) detail_source: u64,
    pub(crate) detail_schedule: Option<obc_reader::WeeklySchedule>,
    clock_key: Option<(bool, i16)>,
    local: Option<(u8, u16)>,
    pub(crate) recheck: bool,
    page: Option<(PlaceKey, bool)>,
    pub(crate) status: QueryProgress,
    pub(crate) generation: u32,
    /// The category the current snapshot is for. It stays `Some` on an empty result, so the screen
    /// can tell an empty category from a category not yet queried.
    pub(crate) taken_for: Option<PoiCategory>,
    /// The current page for [`taken_for`](PoiScratch::taken_for), ascending by distance.
    pub(crate) pois: heapless::Vec<obc_reader::CorridorPoi, PLACE_PAGE_SIZE>,
}

impl PoiScratch {
    pub const fn new() -> Self {
        PoiScratch {
            taken_for: None,
            pois: heapless::Vec::new(),
            query: None,
            hours_filter: obc_reader::reader::places::HoursFilter::HideClosed,
            detail_valid: false,
            detail_source: 0,
            detail_schedule: None,
            clock_key: None,
            local: None,
            recheck: false,
            page: None,
            generation: 0,
            status: QueryProgress::Unavailable,
        }
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(query) = &mut self.query {
            query.cancel();
        }
        self.status = QueryProgress::Unavailable;
        self.detail_valid = false;
        self.detail_source = 0;
    }

    pub(crate) fn clock_changed(&mut self, local: Option<(u8, u16)>, offset: i16) -> bool {
        let authority = (local.is_some(), offset);
        if self.clock_key.is_some_and(|key| key != authority) {
            if let Some(query) = &mut self.query {
                query.cancel();
            }
            self.status = QueryProgress::Unavailable;
            self.detail_valid = false;
            self.detail_source = 0;
        }
        let changed = self.local != local;
        self.recheck |= changed;
        self.local = local;
        self.clock_key = Some(authority);
        changed
    }

    /// Drop any snapshot, so the next POI-list draw re-queries at the current fix.
    pub fn invalidate(&mut self) {
        self.taken_for = None;
        self.pois.clear();
        self.query = None;
        self.recheck = false;
        self.page = None;
        self.generation = self.generation.wrapping_add(1);
        self.status = QueryProgress::Unavailable;
    }

    /// Whether a query for `category` has already run. An empty result still counts.
    pub(crate) fn holds(&self, category: PoiCategory) -> bool {
        self.taken_for == Some(category)
    }

    pub(crate) fn len(&self) -> usize {
        self.pois.len()
    }

    /// The snapshotted POI at `index`, or `None` past the end and while the query is not ready.
    pub(crate) fn get(&self, index: usize) -> Option<&Poi> {
        self.pois.get(index).filter(|_| matches!(self.status, QueryProgress::Ready { .. })).map(|hit| &hit.poi)
    }
}

impl Default for PoiScratch {
    fn default() -> Self {
        PoiScratch::new()
    }
}

/// The POI list. The snapshot itself lives in the [`App`](crate::App)-owned [`PoiScratch`].
#[derive(Debug)]
pub struct PoiListScreen {
    category: PoiCategory,
    selected: usize,
    page: Option<(PlaceKey, bool)>,
}

impl PoiListScreen {
    /// Open the list for `category`. The caller also [invalidates](PoiScratch::invalidate) the App
    /// scratch, so the first draw re-queries even on a re-entry to the same category.
    pub fn new(category: PoiCategory) -> Self {
        PoiListScreen { category, selected: 0, page: None }
    }

    pub(crate) fn refresh(&mut self) {
        self.page = None;
        self.selected = usize::MAX;
    }

    pub(crate) fn pending(&self, scratch: &PoiScratch) -> bool {
        if scratch.recheck {
            return true;
        }
        if scratch.query.is_some() && matches!(scratch.status, QueryProgress::Failed(_) | QueryProgress::Unavailable) {
            return false;
        }
        !scratch.holds(self.category) || self.page != scratch.page
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Wrap over the real row count, not the page capacity: a wrap over the cap makes the
            // cursor walk empty slots. With no snapshot yet the list is empty and a step does nothing.
            Gesture::Step(n) => {
                let scratch = cx.poi_scratch;
                let QueryProgress::Ready { more, .. } = scratch.status else { return Transition::None };
                if !scratch.holds(self.category) || self.page != scratch.page || n == 0 {
                    return Transition::None;
                }
                let visible: heapless::Vec<usize, PLACE_PAGE_SIZE> = scratch
                    .pois
                    .iter()
                    .enumerate()
                    .filter(|(_, hit)| scratch.hours_filter.includes(hit.poi.opening))
                    .map(|(i, _)| i)
                    .collect();
                let index = visible
                    .iter()
                    .position(|i| *i == self.selected)
                    .map(|i| i as i64)
                    .unwrap_or_else(|| visible.partition_point(|i| *i < self.selected) as i64 - i64::from(n > 0));
                let target = index + i64::from(n);
                let backwards = n < 0;
                let crossed = visible.is_empty() || target < 0 || target >= visible.len() as i64;
                let reverse_page = scratch.page.is_some_and(|(_, reverse)| reverse);
                let can_page = if backwards {
                    if reverse_page {
                        more
                    } else {
                        scratch.page.is_some()
                    }
                } else {
                    reverse_page || more
                };
                if crossed && can_page {
                    let boundary = if backwards { scratch.pois.first() } else { scratch.pois.last() };
                    if let (Some(query), Some(hit)) = (scratch.query.as_ref(), boundary) {
                        self.page = Some((query.key(hit), backwards));
                        self.selected = usize::MAX;
                        return Transition::None;
                    }
                }
                if !visible.is_empty() {
                    self.selected = visible[target.rem_euclid(visible.len() as i64) as usize];
                }
                Transition::None
            }
            // An empty scratch (never drawn, or no fix) gives `None`: there is nothing to open.
            Gesture::Press => match cx.poi_scratch.get(self.selected) {
                Some(poi) => Transition::Push(Screen::PoiDetail(PoiDetailScreen::new(poi.clone()))),
                None => Transition::None,
            },
            Gesture::Back => Transition::Pop, // return to the category list
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        // The pre-draw `prepare` pass takes the snapshot, so draw only reads the frozen scratch.
        let (w, h) = (rx.w, rx.h);
        let queried =
            rx.poi_scratch.holds(self.category) && matches!(rx.poi_scratch.status, QueryProgress::Ready { .. });
        let pois: &[obc_reader::CorridorPoi] = if queried { &rx.poi_scratch.pois } else { &[] };
        let visible: heapless::Vec<usize, PLACE_PAGE_SIZE> = pois
            .iter()
            .enumerate()
            .filter(|(i, p)| *i == self.selected || rx.poi_scratch.hours_filter.includes(p.poi.opening))
            .map(|(i, _)| i)
            .collect();
        let total = visible.len();

        let geo = ListGeometry::filling_below_title(w, h, ROW_H, 6, 14, Separators::All);
        let pos = visible.iter().position(|i| *i == self.selected).map_or(0, |i| i + 1);
        let title = rx.t(super::poi_menu::category_msg(self.category));
        list::list_frame(cv, w, h, title, pos, total, geo.visible);

        if total == 0 {
            // Before the first query there is nothing to say yet, so draw nothing: it lasts a frame.
            if rx.state.user_fix.is_none() {
                empty_state(cv, w, h, rx.t(Msg::PoiListNoPosition), rx.t(Msg::PoiListNoPositionSub));
            } else if matches!(rx.poi_scratch.status, QueryProgress::Failed(_) | QueryProgress::Unavailable) {
                empty_state(cv, w, h, rx.t(Msg::PoiListUnavailable), "");
            } else if queried && matches!(rx.poi_scratch.status, QueryProgress::Ready { coverage_complete: false, .. })
            {
                empty_state(cv, w, h, rx.t(Msg::PoiListNoPois), rx.t(Msg::PoiListCoverage));
            } else if queried {
                empty_state(cv, w, h, rx.t(Msg::PoiListNoPois), rx.t(Msg::PoiListNoPoisSub));
            }
            return;
        }

        // The heading reference for the bearing arrows. `None` hides them, rather than point wrong.
        let heading = rx.state.effective_heading_deg();
        let fix = rx.state.user_fix; // present here (a snapshot exists ⇒ there was a fix)
        let units = rx.settings.units;

        let sel = visible.iter().position(|i| *i == self.selected).unwrap_or(0);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            draw_poi_row(
                cv,
                &pois[visible[row.index]].poi,
                &row,
                &rx.marquee,
                w,
                fix,
                heading,
                units,
                rx.t(Msg::PoiDetailClosed),
            );
        });
    }

    /// Fill the [`App`](crate::App)-owned scratch with this category's nearest page, on the first
    /// prepare pass that has both a `Reader` and a fix. Prepare is the one place the side-effectful
    /// query runs; [`draw`](Self::draw) then reads the frozen snapshot.
    pub(crate) fn prepare(&mut self, px: &mut super::Prepare) {
        if !self.pending(px.poi_scratch) {
            return;
        }
        let (Some(reader), Some(fix)) = (px.reader, px.user_fix) else { return };
        let scratch = &mut px.poi_scratch;
        if scratch.recheck {
            if let Err(error) = reader.refresh_place_hours(&mut scratch.pois, px.place_local) {
                scratch.status = QueryProgress::Failed(error);
                scratch.pois.clear();
                scratch.recheck = false;
                return;
            }
            scratch.recheck = false;
            if scratch.holds(self.category) && self.page == scratch.page {
                return;
            }
        }
        if scratch.query.is_some() && matches!(scratch.status, QueryProgress::Failed(_) | QueryProgress::Unavailable) {
            return;
        }
        if scratch.query.is_none() {
            self.selected = usize::MAX;
            scratch.query = Some(
                PlaceQuery::new(
                    scratch.generation,
                    obc_reader::PoiCategorySet::only(self.category),
                    PlaceWindow::Nearby { position: (fix.lon, fix.lat), radius_m: 50_000 },
                    px.place_local,
                )
                .with_hours_filter(scratch.hours_filter),
            );
            scratch.status = QueryProgress::Pending;
        }
        if self.page != scratch.page {
            if let Some((key, backwards)) = self.page {
                if let Some(query) = &mut scratch.query {
                    if backwards {
                        query.previous_page(key);
                    } else {
                        query.next_page(key);
                    }
                    scratch.pois.clear();
                    scratch.taken_for = None;
                }
            }
            scratch.page = self.page;
        }
        let query = scratch.query.as_mut().expect("query initialized");
        for _ in 0..64 {
            scratch.status = query.step(reader, None, scratch.generation, &mut scratch.pois);
            if scratch.status != QueryProgress::Pending {
                if let Err(error) = reader.refresh_place_hours(&mut scratch.pois, px.place_local) {
                    scratch.status = QueryProgress::Failed(error);
                    scratch.pois.clear();
                }
                scratch.taken_for = Some(self.category);
                if self.selected == usize::MAX {
                    let mut visible = scratch
                        .pois
                        .iter()
                        .enumerate()
                        .filter(|(_, hit)| scratch.hours_filter.includes(hit.poi.opening));
                    self.selected = if scratch.page.is_some_and(|(_, reverse)| reverse) {
                        visible.next_back()
                    } else {
                        visible.next()
                    }
                    .map_or(usize::MAX, |(i, _)| i);
                }
                break;
            }
        }
    }
}

/// Draw one POI row on two lines: the name, or its subtype label, above the live bearing arrow and
/// the distance. The name gets its own full-width line, so most names fit whole.
#[allow(clippy::too_many_arguments)]
fn draw_poi_row(
    cv: &mut impl Surface,
    poi: &Poi,
    row: &list::RowCtx,
    marquee: &MarqueeFrame,
    w: i32,
    fix: Option<Fix>,
    heading: Option<f32>,
    units: Units,
    closed: &str,
) {
    use palette::*;
    let x = row.area.top_left.x + 8;
    let top = row.area.top_left.y;

    let name = if poi.name.is_empty() { poi_label_of(poi.subtype).unwrap_or("POI") } else { poi.name.as_str() };
    let name_top = top + 6;
    let name = marquee.fit(name, w - x - 12, Font::Body, row.scroll());
    cv.text(&name, Point::new(x, name_top), Font::Body, TextAlign::Left, INK);

    let line2_top = name_top + Font::Body.cap_bottom() as i32 + 4;
    if poi.opening == obc_reader::hours::OpeningStatus::Closed {
        cv.text(closed, Point::new(w - 20, line2_top), Font::Label, TextAlign::Right, WARNING);
    }
    let mut dist: heapless::String<12> = heapless::String::new();
    write_distance_coarse(&mut dist, "", poi.distance_m, units);
    let mut text_x = x;
    if let (Some(fix), Some(heading)) = (fix, heading) {
        let arrow_mid = line2_top + Font::Label.cap_mid() as i32;
        draw_bearing_arrow(
            cv,
            Point::new(x + ARROW_R, arrow_mid),
            ARROW_R,
            (fix.lon, fix.lat),
            (poi.lon, poi.lat),
            heading,
        );
        text_x = x + 2 * ARROW_R + 8;
    }
    cv.text(&dist, Point::new(text_x, line2_top), Font::Label, TextAlign::Left, SUBTEXT);
}

/// Half-size (px) of the bearing-arrow glyph in the list rows. The
/// [detail screen](super::PoiDetailScreen) passes its own radius.
pub(super) const ARROW_R: i32 = 7;

/// The on-screen direction from `pos` to the POI, relative to the rider's heading, quantized to 8
/// octants: 0 = straight ahead, 1 = up-right, and on clockwise in 45° steps. At glyph size a
/// degree-true arrow only smudges. `pos` and `poi` are `(lon, lat)` µdeg, `heading_deg` is CW from
/// north, and the east component is scaled by `cos_lat` to match the reader's distance metric.
pub(super) fn bearing_octant(pos: (i32, i32), poi: (i32, i32), heading_deg: f32) -> usize {
    let dlat = (poi.1 - pos.1) as f32;
    let dlon = (poi.0 - pos.0) as f32;
    let east = dlon * cos_lat(pos.1);
    // atan2(east, north): 0 = due north, +east (clockwise) positive — same convention as `heading`.
    let bearing = libm::atan2f(east, dlat);
    let theta = bearing - heading_deg.to_radians();
    (libm::roundf(theta / core::f32::consts::FRAC_PI_4) as i32).rem_euclid(8) as usize
}

/// Draw the bearing arrow centred at `c` with half-size `r`. The doubled 1 px stroke keeps it bold
/// at glyph size. The direction comes from [`bearing_octant`], so it is one of the 8 compass steps.
pub(super) fn draw_bearing_arrow(
    cv: &mut impl Surface,
    c: Point,
    r: i32,
    pos: (i32, i32),
    poi: (i32, i32),
    heading_deg: f32,
) {
    use core::f32::consts::FRAC_PI_4;
    let theta = bearing_octant(pos, poi, heading_deg) as f32 * FRAC_PI_4;
    let rf = r as f32;
    let end = |from: Point, ang: f32, len: f32| {
        Point::new(
            from.x + libm::roundf(libm::sinf(ang) * len) as i32,
            from.y - libm::roundf(libm::cosf(ang) * len) as i32,
        )
    };
    let tip = end(c, theta, rf);
    let tail = end(c, theta + core::f32::consts::PI, rf);
    stroke2(cv, tail, tip, palette::WOOD);
    for da in [3.0 * FRAC_PI_4, -3.0 * FRAC_PI_4] {
        stroke2(cv, tip, end(tip, theta + da, rf * 0.75), palette::WOOD);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    /// A scratch holding `n` snapshotted Water POIs.
    fn scratch_with(n: usize) -> PoiScratch {
        let mut scratch = PoiScratch::new();
        scratch.taken_for = Some(PoiCategory::Water);
        scratch.status = QueryProgress::Ready { more: false, coverage_complete: true };
        for i in 0..n {
            let _ = scratch.pois.push(obc_reader::CorridorPoi {
                dist_along_m: 0,
                offset_m: 0,
                poi: Poi {
                    opening: Default::default(),
                    metadata: Default::default(),
                    lat: 43_000_000 + i as i32,
                    lon: 7_000_000,
                    subtype: 1,
                    name: heapless::String::new(),
                    hours_ref: 0xFFFF,
                    distance_m: i as u32,
                },
            });
        }
        scratch
    }

    fn step(scr: &mut PoiListScreen, scratch: &PoiScratch, n: i32) {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut cx = Ctx { poi_scratch: scratch, ..test_ctx(&mut st, &mut act, &mut settings) };
        scr.handle(Gesture::Step(n), &mut cx);
    }

    #[test]
    fn paging_uses_visible_boundaries_and_does_not_revive_cancelled_work() {
        use obc_reader::{MapCache, MapTables, Reader, SliceSource};
        use obcm_testkit::{build_poi_map, PoiSpec};
        let records = (0..40)
            .map(|i| PoiSpec {
                lat: 43_500_000 + i,
                lon: 7_500_000,
                subtype: 1,
                name: std::format!("P{i}"),
                payload: 0xffff,
            })
            .collect();
        let bytes = build_poi_map((7_000_000, 43_000_000, 8_000_000, 44_000_000), 512, &[(1, records)]);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut scratch = PoiScratch::new();
        let mut screen = PoiListScreen::new(PoiCategory::Water);
        let finish = |screen: &mut PoiListScreen, scratch: &mut PoiScratch| {
            for _ in 0..100 {
                screen.prepare(&mut super::super::Prepare {
                    reader: Some(&reader),
                    route: None,
                    user_fix: Some(Fix::at(43_500_000, 7_500_000)),
                    poi_scratch: scratch,
                    active_route: None,
                    progress_m: 0,
                    route_total_m: 0,
                    detour_preview: &[],
                    place_local: None,
                });
                if !screen.pending(scratch) {
                    return;
                }
            }
            panic!("bounded query did not finish");
        };
        finish(&mut screen, &mut scratch);
        let first_boundary = scratch.query.as_ref().unwrap().key(scratch.pois.last().unwrap());
        scratch.pois[PLACE_PAGE_SIZE - 1].poi.opening = obc_reader::hours::OpeningStatus::Closed;
        screen.selected = PLACE_PAGE_SIZE - 2;
        step(&mut screen, &scratch, 1);
        assert_eq!(screen.page, Some((first_boundary, false)), "closed final row must not trap the page");
        finish(&mut screen, &mut scratch);
        assert!(scratch.query.as_ref().unwrap().key(&scratch.pois[0]) > first_boundary);
        screen.selected = 0;
        step(&mut screen, &scratch, -1);
        finish(&mut screen, &mut scratch);
        assert!(matches!(scratch.status, QueryProgress::Ready { more: false, .. }));
        let first_page = screen.page;
        screen.selected = 0;
        step(&mut screen, &scratch, -1);
        assert_eq!(screen.page, first_page, "a reverse first page has no preceding page");
        assert_eq!(screen.selected, PLACE_PAGE_SIZE - 1);
        for hit in &mut scratch.pois {
            hit.poi.opening = obc_reader::hours::OpeningStatus::Closed;
        }
        step(&mut screen, &scratch, 1);
        finish(&mut screen, &mut scratch);
        assert!(
            scratch.query.as_ref().unwrap().key(&scratch.pois[0]) > first_boundary,
            "an entirely hidden page still has immutable continuation boundaries"
        );
        scratch.clock_changed(None, 0);
        scratch.clock_changed(Some((0, 600)), 60);
        let cancelled_page = screen.page;
        screen.selected = 15;
        step(&mut screen, &scratch, 1);
        assert_eq!(screen.page, cancelled_page);
        finish(&mut screen, &mut scratch);
        assert_eq!(scratch.status, QueryProgress::Unavailable);
        assert_eq!(scratch.query.as_ref().unwrap().progress(), QueryProgress::Unavailable);
    }

    #[test]
    fn closed_status_preserves_selection_and_allows_activation() {
        let mut scratch = scratch_with(3);
        let mut screen = PoiListScreen::new(PoiCategory::Water);
        screen.selected = 1;
        scratch.pois[0].poi.opening = obc_reader::hours::OpeningStatus::Closed;
        scratch.pois[1].poi.opening = obc_reader::hours::OpeningStatus::Closed;
        assert_eq!(screen.selected, 1);
        assert!(scratch.get(screen.selected).is_some());
        step(&mut screen, &scratch, 1);
        assert_eq!(screen.selected, 2, "turning moves to the next eligible identity");
    }

    #[test]
    fn turn_wraps_over_the_real_count_not_the_cap() {
        let scratch = scratch_with(5);
        let mut scr = PoiListScreen::new(PoiCategory::Water);
        for (i, expected) in [1, 2, 3, 4, 0, 1, 2, 3, 4, 0].into_iter().enumerate() {
            step(&mut scr, &scratch, 1);
            assert_eq!(scr.selected, expected, "step {} lands on row {}", i + 1, expected);
        }
        step(&mut scr, &scratch, -1);
        assert_eq!(scr.selected, 4, "up from the top lands on the last real row");
    }

    #[test]
    fn turn_before_the_snapshot_is_a_noop() {
        let empty = PoiScratch::new();
        let mut scr = PoiListScreen::new(PoiCategory::Water);
        step(&mut scr, &empty, 1);
        assert_eq!(scr.selected, 0, "no snapshot — the cursor stays put");
        let other = scratch_with(3); // a stale snapshot for a different category
        let mut scr = PoiListScreen::new(PoiCategory::Pharmacy);
        step(&mut scr, &other, 1);
        assert_eq!(scr.selected, 0, "another category's snapshot doesn't count");
    }

    #[test]
    fn due_north_north_course_points_up() {
        let pos = (7_000_000, 43_000_000);
        let north = (7_000_000, 43_010_000);
        assert_eq!(bearing_octant(pos, north, 0.0), 0);
    }

    #[test]
    fn arrow_rotates_with_heading() {
        let pos = (7_000_000, 43_000_000);
        let north = (7_000_000, 43_010_000);
        assert_eq!(bearing_octant(pos, north, 90.0), 6); // heading east ⇒ north is on the left
        assert_eq!(bearing_octant(pos, north, 180.0), 4); // heading south ⇒ north is behind (down)
        assert_eq!(bearing_octant(pos, north, 270.0), 2); // heading west ⇒ north is on the right
    }

    #[test]
    fn due_east_north_course_points_right() {
        let pos = (7_000_000, 43_000_000);
        let east = (7_010_000, 43_000_000);
        assert_eq!(bearing_octant(pos, east, 0.0), 2);
    }

    #[test]
    fn bearing_quantizes_to_the_nearest_octant() {
        let pos = (7_000_000, 43_000_000);
        // cos_lat(43°) ≈ 0.731 scales the east component, so the lon offsets set the angle.
        let ne = (7_007_895, 43_010_000); // 30° east of north
        assert_eq!(bearing_octant(pos, ne, 0.0), 1);
        let n10 = (7_002_413, 43_010_000); // 10° east of north
        assert_eq!(bearing_octant(pos, n10, 0.0), 0);
        let w30 = (6_992_105, 43_010_000); // 30° west of north
        assert_eq!(bearing_octant(pos, w30, 0.0), 7);
        // Heading 90° puts the same POI 60° to the left, so the nearest step is octant 7.
        assert_eq!(bearing_octant(pos, ne, 90.0), 7);
    }
}
