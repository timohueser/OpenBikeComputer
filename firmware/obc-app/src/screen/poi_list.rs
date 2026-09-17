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

/// Per-POI **nominal** row height — two lines (name above, bearing arrow + distance below) with
/// margin to keep the distance clear of the row separator / selected-row fill. Places
/// page through the list widget. The drawn pitch stretches from this so the rows consume
/// the whole viewport (owner review round 3: no dead band under the last row) — on the 320 px
/// panel that's 68 px rows, four flush to the bottom margin.
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
    /// The category the current snapshot is for once a query has run — `Some` even when the result
    /// is empty (so the screen can tell "queried, empty category" from "not queried yet"). `None`
    /// on a fresh/invalidated scratch.
    pub(crate) taken_for: Option<PoiCategory>,
    /// The current page for [`taken_for`](PoiScratch::taken_for), ascending by distance. Frozen once
    /// filled; the query owns the ordering.
    pub(crate) pois: heapless::Vec<obc_reader::CorridorPoi, PLACE_PAGE_SIZE>,
}

impl PoiScratch {
    /// An empty scratch — no snapshot taken yet.
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

    /// Drop any snapshot so the next POI-list draw re-queries. Called when a POI list screen opens,
    /// so re-entering a category always takes a fresh snapshot at the current fix.
    pub fn invalidate(&mut self) {
        self.taken_for = None;
        self.pois.clear();
        self.query = None;
        self.recheck = false;
        self.page = None;
        self.generation = self.generation.wrapping_add(1);
        self.status = QueryProgress::Unavailable;
    }

    /// Whether a query for `category` has already run (a snapshot is present — possibly empty).
    /// Read by the host reader-build seam ([`App::base_needs_reader`](crate::App::base_needs_reader))
    /// as well as the screen's own draw.
    pub(crate) fn holds(&self, category: PoiCategory) -> bool {
        self.taken_for == Some(category)
    }

    /// Number of POIs currently snapshotted (0 when none taken). Introspection for
    /// [`App::poi_snapshot_len`](crate::App::poi_snapshot_len).
    pub(crate) fn len(&self) -> usize {
        self.pois.len()
    }

    /// The snapshotted POI at `index` (ascending by distance), or `None` past the end. The POI
    /// list's `Gesture::Press` reads it through [`Ctx`](super::Ctx) to hand the selected `Poi` to the
    /// detail screen — the one place `handle` reaches the draw-taken snapshot (the query itself still
    /// only runs at draw).
    pub(crate) fn get(&self, index: usize) -> Option<&Poi> {
        self.pois.get(index).filter(|_| matches!(self.status, QueryProgress::Ready { .. })).map(|hit| &hit.poi)
    }
}

impl Default for PoiScratch {
    fn default() -> Self {
        PoiScratch::new()
    }
}

/// The POI list. State is the browsed category and the highlighted row; the snapshot itself lives
/// in the [`App`](crate::App)-owned [`PoiScratch`], keyed by category and taken lazily on draw.
#[derive(Debug)]
pub struct PoiListScreen {
    category: PoiCategory,
    selected: usize,
    page: Option<(PlaceKey, bool)>,
}

impl PoiListScreen {
    /// Open the list for `category`. The caller (the Find a place screen) also
    /// [invalidates](PoiScratch::invalidate) the App scratch on this transition, so the first draw
    /// re-queries even when re-entering the same category.
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
            // Wrap over the **real** row count — the draw-taken snapshot lives in the App-owned
            // scratch `cx` carries, so once the query has run the count is known right here (it
            // wasn't when this screen was born, and wrapping over the 16-record cap left the
            // cursor walking phantom rows: on a shorter list the highlight sat "stuck" on the
            // last row for the missing steps before wrapping — owner review round 2). Before
            // the snapshot lands (first draw hasn't run / no fix yet) the list is empty and a
            // step is a no-op.
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
            // Open the detail screen for the highlighted POI (epic #439 P4 #444). The snapshot is
            // taken at draw, so it lives in the App-owned scratch `cx` carries read-only. An empty scratch
            // (never drawn / no fix) ⇒ `get` is `None` ⇒ nothing to open, stay put.
            Gesture::Press => match cx.poi_scratch.get(self.selected) {
                Some(poi) => Transition::Push(Screen::PoiDetail(PoiDetailScreen::new(poi.clone()))),
                None => Transition::None,
            },
            Gesture::Back => Transition::Pop, // return to the category list
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        // The snapshot was taken by the pre-draw `prepare` pass (#803), so draw is side-effect-free:
        // it reads the frozen scratch read-only. Before `prepare` has landed a snapshot (no fix /
        // no reader yet) the scratch simply doesn't hold this category and the empty state covers it.
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
            // "No position" when a snapshot could never be taken (no fix ever); once a query has run
            // against a fix, "No POIs in this map" for a genuinely empty category. Before the first
            // query (no fix yet, not queried) draw nothing — a transient one-frame state.
            if rx.state.user_fix.is_none() {
                empty_state(cv, w, h, rx.t(Msg::PoiListNoPosition), rx.t(Msg::PoiListNoPositionSub));
            } else if matches!(rx.poi_scratch.status, QueryProgress::Failed(_) | QueryProgress::Unavailable) {
                empty_state(cv, w, h, rx.t(Msg::PoiListUnavailable), "");
            } else if queried && matches!(rx.poi_scratch.status, QueryProgress::Ready { coverage_complete: false, .. })
            {
                empty_state(cv, w, h, rx.t(Msg::PoiListNoPois), rx.t(Msg::PoiListCoverage));
            } else if queried {
                // Body title fits ~16 chars on the 240 px panel — keep it short; the hint carries
                // the "in this map" scope the epic's wording wants.
                empty_state(cv, w, h, rx.t(Msg::PoiListNoPois), rx.t(Msg::PoiListNoPoisSub));
            }
            return;
        }

        // The heading reference for the bearing arrows: GPS course while moving, compass while
        // stopped (the #231 seam). `None` ⇒ hide the arrows rather than point wrong.
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

    /// Fill the [`App`](crate::App)-owned scratch with this category's nearest-16 on the first
    /// **prepare** pass that has both a `Reader` and a fix — then never again (the scratch already
    /// `holds` the category). A re-entry invalidated the scratch in
    /// the Find a place screen, so it re-queries.
    ///
    /// Runs in the pre-draw [`prepare`](super::Screen::prepare) pass (#803) — the one place the
    /// side-effectful `Reader` query lives — writing solely to the shared [`Prepare::poi_scratch`];
    /// [`draw`](Self::draw) then reads the frozen snapshot. On a host that skips building the
    /// `Reader` on a non-map frame, [`base_needs_reader`](crate::App::base_needs_reader) keeps the
    /// `Reader` built and passed here until the snapshot lands.
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

/// Draw one POI row on two lines: the name (or its subtype fallback label) on top, in the row's
/// prominent Body type across the full width; the live bearing arrow + the distance below it, in
/// muted Label type. Giving the name its own line is what lets a real POI name fit instead of a
/// truncated stub. `fix`/`heading` drive the live arrow; `units` scales the distance.
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

    // Line 1 — the name, the row's primary element, now on its own full-width line so most names
    // fit whole (only a genuinely long one still gets the ".." from `fit`).
    let name = if poi.name.is_empty() { poi_label_of(poi.subtype).unwrap_or("POI") } else { poi.name.as_str() };
    let name_top = top + 6;
    let name_max = ((w - x - 12) / Font::Body.char_width() as i32).max(6) as usize;
    cv.text(&marquee.fit(name, name_max, row.scroll()), Point::new(x, name_top), Font::Body, TextAlign::Left, INK);

    // Line 2 — bearing arrow + distance, secondary (smaller, muted), stacked under the name.
    let line2_top = name_top + Font::Body.cap_bottom() as i32 + 4;
    if poi.opening == obc_reader::hours::OpeningStatus::Closed {
        cv.text(closed, Point::new(w - 20, line2_top), Font::Label, TextAlign::Right, WARNING);
    }
    let mut dist: heapless::String<12> = heapless::String::new();
    write_distance_coarse(&mut dist, "", poi.distance_m, units);
    let mut text_x = x;
    // Arrow at the left of line 2 (only when a heading reference exists — else hidden), distance
    // just to its right.
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

/// Half-size of the list rows' bearing-arrow glyph (px) — an ≈15×15 box in the same slot before
/// the distance (grown from 5 in owner review round 3, with the taller rows). The
/// [detail screen](super::PoiDetailScreen) draws the same arrow at Body-line size by passing its
/// own radius.
pub(super) const ARROW_R: i32 = 7;

/// The 8-way quantized on-screen direction of the bearing from `pos` to the POI **relative to the
/// rider's heading** — octant 0 = straight ahead (up), 1 = up-right, … clockwise in 45° steps
/// (#685 §1: at glyph size a degree-true arrow just smudges; the 8 snapped directions read
/// without focusing). `pos`/`poi` are `(lon, lat)` µdeg; `heading_deg` is CW from north.
///
/// Geometry: the true bearing to the POI is `atan2(east, north)` (CW from north) with the east
/// component scaled by `cos_lat` (local-equirectangular, matching the reader's distance metric);
/// subtracting the heading gives the on-screen angle, rounded to the nearest 45° step.
pub(super) fn bearing_octant(pos: (i32, i32), poi: (i32, i32), heading_deg: f32) -> usize {
    let dlat = (poi.1 - pos.1) as f32;
    let dlon = (poi.0 - pos.0) as f32;
    let east = dlon * cos_lat(pos.1);
    // atan2(east, north): 0 = due north, +east (clockwise) positive — same convention as `heading`.
    let bearing = libm::atan2f(east, dlat);
    let theta = bearing - heading_deg.to_radians();
    (libm::roundf(theta / core::f32::consts::FRAC_PI_4) as i32).rem_euclid(8) as usize
}

/// Draw the 8-way bearing arrow centred at `c` with half-size `r` — a full arrow (shaft + two
/// 135° barbs) in the doubled-1-px stroke idiom (the menu bezel ticks / passkey phone), so it
/// reads bold at glyph size where the old filled chevron was a smudge. Direction comes from
/// [`bearing_octant`], so the drawn angles are exactly the 8 compass steps.
///
/// Shared with the [detail screen](super::PoiDetailScreen), which draws it at Body-line size.
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
    // Barbs off the tip at ±135° from the direction, ~3/4 of the half-size long.
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

    /// A scratch holding `n` snapshotted Water POIs — the state after the first draw's query.
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
                hours_ref: 0xffff,
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

    /// The step wraps over the **real** snapshot count, not the page capacity: on a 5-result list
    /// every step moves exactly one real row and the wrap is immediate at both ends — no dead
    /// steps on phantom rows past the last item (the pre-#678 bug the owner hit: the cursor
    /// walked the cap's empty slots and looked stuck on the last row).
    #[test]
    fn turn_wraps_over_the_real_count_not_the_cap() {
        let scratch = scratch_with(5);
        let mut scr = PoiListScreen::new(PoiCategory::Water);
        // Ten forward steps from row 0 walk the 5 rows exactly twice — position after each is
        // deterministic, with the wrap firing straight past the last row.
        for (i, expected) in [1, 2, 3, 4, 0, 1, 2, 3, 4, 0].into_iter().enumerate() {
            step(&mut scr, &scratch, 1);
            assert_eq!(scr.selected, expected, "step {} lands on row {}", i + 1, expected);
        }
        // Backward off the top wraps to the last *real* row, not slot 15.
        step(&mut scr, &scratch, -1);
        assert_eq!(scr.selected, 4, "up from the top lands on the last real row");
    }

    /// Before the snapshot lands (no query yet, or the scratch holds another category) the list is
    /// empty — a step is a no-op, never a walk over the cap.
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

    /// A POI due north with a north-facing heading points straight up (octant 0).
    #[test]
    fn due_north_north_course_points_up() {
        let pos = (7_000_000, 43_000_000);
        let north = (7_000_000, 43_010_000);
        assert_eq!(bearing_octant(pos, north, 0.0), 0);
    }

    /// The arrow rotates with the heading: same POI due north, but facing east ⇒ it points to the
    /// rider's left (octant 6), facing south ⇒ down (octant 4), facing west ⇒ right (octant 2).
    #[test]
    fn arrow_rotates_with_heading() {
        let pos = (7_000_000, 43_000_000);
        let north = (7_000_000, 43_010_000);
        assert_eq!(bearing_octant(pos, north, 90.0), 6); // heading east ⇒ north is on the left
        assert_eq!(bearing_octant(pos, north, 180.0), 4); // heading south ⇒ north is behind (down)
        assert_eq!(bearing_octant(pos, north, 270.0), 2); // heading west ⇒ north is on the right
    }

    /// A POI due east with a north heading points right; the cos_lat scaling doesn't rotate a
    /// pure-east or pure-north bearing (only mixes the diagonal).
    #[test]
    fn due_east_north_course_points_right() {
        let pos = (7_000_000, 43_000_000);
        let east = (7_010_000, 43_000_000);
        assert_eq!(bearing_octant(pos, east, 0.0), 2);
    }

    /// Quantization snaps to the **nearest** 45° step in both directions: a bearing a hair past
    /// the 22.5° boundary rounds up to the diagonal, a hair before it stays cardinal, and a
    /// slightly-west-of-north bearing wraps to octant 7 (up-left), not 0.
    #[test]
    fn bearing_quantizes_to_the_nearest_octant() {
        let pos = (7_000_000, 43_000_000);
        // cos_lat(43°) ≈ 0.731 — pick lon offsets whose *scaled* east component sets the angle.
        // 30° east of north: east/north = tan(30°) = 0.577 ⇒ dlon = 0.577/0.731 × dlat.
        let ne = (7_007_895, 43_010_000); // atan(0.7895 × 0.731 / 1.0) ≈ 30° ⇒ nearest is 45° (oct 1)
        assert_eq!(bearing_octant(pos, ne, 0.0), 1);
        // 10° east of north stays cardinal (oct 0): dlon = tan(10°)/0.731 × dlat ≈ 0.2413 × dlat.
        let n10 = (7_002_413, 43_010_000);
        assert_eq!(bearing_octant(pos, n10, 0.0), 0);
        // 10° WEST of north wraps to… still octant 0 (nearest); 30° west snaps to octant 7.
        let w30 = (6_992_105, 43_010_000);
        assert_eq!(bearing_octant(pos, w30, 0.0), 7);
        // A heading change walks the octants: the 30°-east POI seen while heading 90° (east) sits
        // 60° to the left ⇒ nearest step is −45° ⇒ octant 7.
        assert_eq!(bearing_octant(pos, ne, 90.0), 7);
    }
}
