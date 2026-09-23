//! The Route menu: pick a route, or open a trip. Two levels in one screen. The top level lists the
//! trips first and then the unfiled routes; a trip press pushes its day list, a second menu scoped
//! to that trip. A day row opens its route as a route row does, and never nests further.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::route::RouteSummary;
use crate::settings::Language;
use crate::trip::{TripProgress, TripSummary};
use crate::{t as tr, Msg};

use super::vocab::chrome::{empty_state, row_check, title_frame, ROW_CHECK_HALF};
use super::vocab::list;
use super::vocab::marquee::{fit, MarqueeFrame};
use super::vocab::two_line::{self, line2_right, LINE2, LINE2_FONT};
use super::{
    palette, Ctx, MapScreen, Render, RouteOverviewScreen, RouteSwapScreen, Screen, Transition, TripDeleteScreen,
};

/// The stats line's second column, the climb group, as a fraction of the row's inner width, so the
/// climb figures align across every row whatever the width of the distance.
const CLIMB_COL_PCT: i32 = 55;

/// The climb triangle's side, and the triangle plus its gap to the climb figure.
const CLIMB_TRI: i32 = 10;
const CLIMB_GLYPH_W: i32 = 14;

/// The indent a ridden day's tick takes. Only a ticked row moves its text right by it.
const TICK_W: i32 = 16;

/// The dim olive of a ridden day's text.
const RIDDEN: u16 = palette::PARCHMENT_SHADE;

/// The upper bound on rows: every trip folder plus every route, when nothing is filed.
const ROW_CAP: usize = crate::trip::MAX_TRIPS + crate::route::MAX_ROUTES;

/// Weekday catalog keys, Monday-first.
const WEEKDAYS: [Msg; 7] = [
    Msg::WeekdayMon,
    Msg::WeekdayTue,
    Msg::WeekdayWed,
    Msg::WeekdayThu,
    Msg::WeekdayFri,
    Msg::WeekdaySat,
    Msg::WeekdaySun,
];

/// One row of the menu: a trip folder by trip-catalog index, a route by route-catalog index, or a
/// day of the scoped trip with its route's catalog index.
#[derive(Clone, Copy)]
enum Row {
    Folder(usize),
    Route(usize),
    Day { day: u16, route: usize },
}

/// The identity of the highlighted row, so the highlight can follow it across a live catalog
/// rescan. A route or a day is pinned by its catalog index, a folder by its trip's durable id.
#[derive(Debug, Clone, Copy)]
enum SelId {
    Folder(crate::CatalogObjectId),
    Route(usize),
}

impl Row {
    /// This row's [`SelId`] — a folder resolves to its trip's durable id, a route to its index.
    fn identity(self, trips: &[TripSummary]) -> Option<SelId> {
        match self {
            Row::Folder(ti) => trips.get(ti).map(|t| SelId::Folder(t.id)),
            Row::Route(ri) | Row::Day { route: ri, .. } => Some(SelId::Route(ri)),
        }
    }

    fn is(self, id: SelId, trips: &[TripSummary]) -> bool {
        match (self, id) {
            (Row::Folder(ti), SelId::Folder(tid)) => trips.get(ti).is_some_and(|t| t.id == tid),
            (Row::Route(ri) | Row::Day { route: ri, .. }, SelId::Route(i)) => ri == i,
            _ => false,
        }
    }
}

/// What this menu instance lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteMenuScope {
    /// The top level: trip folders first, then the unfiled routes.
    TopLevel,
    /// One trip's day list. The durable id is resolved against the live trip catalog each frame,
    /// so a rescan that reorders the trips cannot mis-scope it.
    Trip { trip_id: crate::CatalogObjectId },
}

#[derive(Debug)]
pub struct RouteMenuScreen {
    selected: usize,
    /// Pinned each `handle` and `draw`, so a live rescan can follow the highlight to its new row.
    /// `None` before the first frame; a rescan then only clamps.
    sel_id: Option<SelId>,
    scope: RouteMenuScope,
}

impl Default for RouteMenuScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteMenuScreen {
    /// The top-level menu (trip folders + unfiled routes).
    pub fn new() -> Self {
        RouteMenuScreen { selected: 0, sel_id: None, scope: RouteMenuScope::TopLevel }
    }

    /// The day list of trip `t`, with the cursor on the next day. A done trip opens on its first
    /// day.
    pub fn trip(t: &TripSummary, progress: Option<&TripProgress>) -> Self {
        let selected = t.next_day(progress).map_or(0, |next| t.days().take_while(|&(day, _)| day < next).count());
        RouteMenuScreen { selected, sel_id: None, scope: RouteMenuScope::Trip { trip_id: t.id } }
    }
    /// Re-point the highlight after a live catalog rescan: map the pinned identity across the
    /// rescan and find it again in the rebuilt list. A route that vanished clamps near its old
    /// position, never to a dangling index.
    pub(crate) fn remap_routes(
        &mut self,
        remap: &dyn Fn(usize) -> Option<usize>,
        trips: &[TripSummary],
        routes_len: usize,
        internal_routes: u64,
    ) {
        let mapped = self.sel_id.and_then(|id| match id {
            SelId::Route(i) => remap(i).map(SelId::Route),
            SelId::Folder(tid) => Some(SelId::Folder(tid)),
        });
        let mut rows: heapless::Vec<Row, ROW_CAP> = heapless::Vec::new();
        self.build_rows(trips, routes_len, internal_routes, &mut rows);
        self.selected = match mapped.and_then(|id| rows.iter().position(|r| r.is(id, trips))) {
            Some(row) => row,
            None => self.selected.min(rows.len().saturating_sub(1)),
        };
        self.sel_id = rows.get(self.selected).and_then(|r| r.identity(trips));
    }

    /// Pin the identity of the current selection, so the next rescan-remap can follow it.
    fn pin(&mut self, rows: &[Row], trips: &[TripSummary]) {
        self.sel_id = rows.get(self.selected).and_then(|r| r.identity(trips));
    }

    /// Build the current scope's rows into `out`. Internal Assistant routes are omitted, but the
    /// catalog indices used for navigation do not change.
    fn build_rows(
        &self,
        trips: &[TripSummary],
        routes_len: usize,
        internal_routes: u64,
        out: &mut heapless::Vec<Row, ROW_CAP>,
    ) {
        out.clear();
        match self.scope {
            RouteMenuScope::TopLevel => {
                for ti in 0..trips.len() {
                    let _ = out.push(Row::Folder(ti));
                }
                for ri in 0..routes_len {
                    // A filed route shows only inside its folder — skip it at the top level.
                    let filed = trips.iter().any(|t| t.stage_indices.contains(&(ri as u16)));
                    if !filed && internal_routes & (1 << ri) == 0 {
                        let _ = out.push(Row::Route(ri));
                    }
                }
            }
            RouteMenuScope::Trip { trip_id } => {
                if let Some(t) = trips.iter().find(|t| t.id == trip_id) {
                    for (day, idx) in t.days() {
                        if internal_routes & (1 << idx) == 0 {
                            let _ = out.push(Row::Day { day, route: usize::from(idx) });
                        }
                    }
                }
            }
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let mut rows: heapless::Vec<Row, ROW_CAP> = heapless::Vec::new();
        self.build_rows(cx.trips, cx.routes.len(), cx.navigator.internal_routes(), &mut rows);
        let len = rows.len();
        if len > 0 {
            self.selected = self.selected.min(len - 1);
        }
        self.pin(&rows, cx.trips);
        match g {
            Gesture::Step(n) => {
                let t = list::on_step(&mut self.selected, n, len);
                self.pin(&rows, cx.trips);
                t
            }
            Gesture::Press if len > 0 => match rows[self.selected.min(len - 1)] {
                Row::Folder(ti) => {
                    let t = &cx.trips[ti];
                    Transition::Push(Screen::RouteMenu(RouteMenuScreen::trip(t, t.progress_in(cx.trip_progress))))
                }
                Row::Route(ri) | Row::Day { route: ri, .. } => self.press_route(ri, cx),
            },
            // A long-press on a folder opens the cascade-delete confirm; on a route or a day it
            // does nothing, because a route is deleted from the Route overview.
            Gesture::Hold if len > 0 => match rows[self.selected.min(len - 1)] {
                Row::Folder(ti) => {
                    let t = &cx.trips[ti];
                    Transition::Push(Screen::TripDelete(TripDeleteScreen::new(t.id, &t.name)))
                }
                Row::Route(_) | Row::Day { .. } => Transition::None,
            },
            Gesture::Back => Transition::Pop, // top level → Home/Menu; day list → top level
            _ => Transition::None,
        }
    }

    /// The route-row press flow, shared by both scopes: mid-ride it asks whether to swap or
    /// re-ride; from Idle it opens the Route overview.
    fn press_route(&self, i: usize, cx: &mut Ctx) -> Transition {
        if cx.navigator.route_unaccepted(i) {
            return Transition::None;
        }
        if cx.recorder.recording() {
            if cx.navigator.route_state().active_route == Some(i) {
                return Transition::Root(Screen::Map(MapScreen::new()));
            }
            return Transition::Push(Screen::RouteSwap(RouteSwapScreen::new(i)));
        }
        let prev = cx.navigator.replace_active_route(i);
        Transition::Push(Screen::RouteOverview(RouteOverviewScreen::new(i, prev)))
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let routes = rx.routes;
        let trips = rx.trips;

        let mut rows: heapless::Vec<Row, ROW_CAP> = heapless::Vec::new();
        self.build_rows(trips, routes.len(), rx.internal_routes, &mut rows);
        let total = rows.len();

        let geo = two_line::geometry(w, h);

        // The title is "ROUTES" at the top level and the trip's name in a day list. `title_buf`
        // is filled only on the trip-name path, so it is declared uninitialized and borrowed there.
        let title_buf: heapless::String<64>;
        let trip = match self.scope {
            RouteMenuScope::TopLevel => None,
            RouteMenuScope::Trip { trip_id } => trips.iter().find(|t| t.id == trip_id),
        };
        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        let counter = list::counter(pos, total, geo.visible);
        let title = match trip {
            Some(t) => {
                // The title and the counter both keep 14 px from the bar's ends, and 8 px apart.
                let counter_w = if counter.is_empty() { 0 } else { text_width(&counter, Font::Label) as i32 + 8 };
                title_buf = fit(&t.name, w - 28 - counter_w, Font::Body);
                &title_buf
            }
            None => rx.t(Msg::RouteMenuTitle),
        };
        title_frame(cv, w, h, title, &counter);

        if total == 0 {
            let sub = match self.scope {
                RouteMenuScope::TopLevel => rx.t(Msg::RouteMenuNoRoutesSub),
                RouteMenuScope::Trip { .. } => rx.t(Msg::RouteMenuFolderEmptySub),
            };
            empty_state(cv, w, h, rx.t(Msg::RouteMenuNoRoutes), sub);
            return;
        }

        let lang = rx.settings.language;
        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let unaccepted =
                |ri: usize| (rx.unaccepted_routes & (1 << ri) != 0).then(|| rx.t(Msg::RouteMenuUnaccepted));
            match rows[row.index] {
                Row::Folder(ti) => {
                    let t = &trips[ti];
                    let x = name_line(cv, &row, &rx.marquee, &t.name, None, INK);
                    let meta = trip_meta(t, t.progress_in(rx.trip_progress), lang, line2_right(&row) - x);
                    cv.text(
                        &meta,
                        Point::new(x, two_line::line2_y(&row)),
                        LINE2_FONT,
                        TextAlign::Left,
                        two_line::row_color(&row, LINE2),
                    );
                }
                Row::Route(ri) => draw_route_row(cv, &row, &rx.marquee, &routes[ri], unaccepted(ri)),
                Row::Day { day, route } => {
                    let Some(t) = trip else { return };
                    let progress = t.progress_in(rx.trip_progress);
                    // A ridden day is dim, except under the cursor, where dim olive on amber is
                    // unreadable.
                    let (ink, line2, tick) = match (t.is_ticked(day, progress), row.selected) {
                        (false, _) => (INK, LINE2, None),
                        (true, false) => (RIDDEN, RIDDEN, Some(LINE2)),
                        (true, true) => (INK, LINE2, Some(LINE2)),
                    };
                    let r = &routes[route];
                    let x = name_line(cv, &row, &rx.marquee, &r.name, tick, ink);
                    let sy = two_line::line2_y(&row);
                    if let Some(label) = unaccepted(route) {
                        cv.text(
                            label,
                            Point::new(x, sy),
                            LINE2_FONT,
                            TextAlign::Left,
                            two_line::row_color(&row, LINE2),
                        );
                        return;
                    }
                    let mut dist: heapless::String<24> = heapless::String::new();
                    if let Some(date) = t.day_date(day, progress) {
                        let _ = write!(dist, "{} · ", tr(weekday(date), lang));
                    }
                    let _ = write!(dist, "{} km", r.distance_km);
                    cv.text(&dist, Point::new(x, sy), LINE2_FONT, TextAlign::Left, two_line::row_color(&row, line2));
                    // The climb follows the distance, because the weekday leaves no room for a
                    // column. It drops whole when the row is too narrow.
                    let climb_x = x + text_width(&dist, LINE2_FONT) as i32 + 8;
                    let climb = climb_label(r.climb_m);
                    if climb_x + CLIMB_GLYPH_W + text_width(&climb, LINE2_FONT) as i32 <= line2_right(&row) {
                        climb_group(cv, climb_x, sy, &climb, two_line::row_color(&row, line2));
                    }
                }
            }
        });
    }
}

/// The x of the climb column.
fn climb_col_x(row: &list::RowCtx) -> i32 {
    row.area.top_left.x + row.area.size.width as i32 * CLIMB_COL_PCT / 100
}

/// Line 1 of a route-menu row: the name, cut or scrolled to the row. `tick` draws the ridden tick in
/// that colour in the text column's indent and moves the text right by [`TICK_W`]. Returns the x
/// where line 2 starts.
fn name_line(
    cv: &mut impl Surface,
    row: &list::RowCtx,
    marquee: &MarqueeFrame,
    name: &str,
    tick: Option<u16>,
    color: u16,
) -> i32 {
    let mut x = two_line::text_x(row);
    if let Some(tick_color) = tick {
        row_check(cv, two_line::mark_at(row, x + ROW_CHECK_HALF), two_line::row_color(row, tick_color));
        x += TICK_W;
    }
    two_line::name_line(cv, row, marquee, name, (x, two_line::name_right(row)), color);
    x
}

/// A standard route row: the name on line 1, the distance under it, and the climb group at the
/// fixed second column.
fn draw_route_row(
    cv: &mut impl Surface,
    row: &list::RowCtx,
    marquee: &MarqueeFrame,
    route: &RouteSummary,
    unavailable: Option<&str>,
) {
    use palette::*;
    let name_x = name_line(cv, row, marquee, &route.name, None, INK);
    let sy = two_line::line2_y(row);
    if let Some(label) = unavailable {
        cv.text(label, Point::new(name_x, sy), LINE2_FONT, TextAlign::Left, two_line::row_color(row, LINE2));
        return;
    }
    let mut dist: heapless::String<12> = heapless::String::new();
    let _ = write!(dist, "{} km", route.distance_km);
    cv.text(&dist, Point::new(name_x, sy), LINE2_FONT, TextAlign::Left, two_line::row_color(row, LINE2));

    climb_group(cv, climb_col_x(row), sy, &climb_label(route.climb_m), two_line::row_color(row, LINE2));
}

fn climb_label(climb_m: u32) -> heapless::String<12> {
    let mut climb = heapless::String::new();
    let _ = write!(climb, "{climb_m} m");
    climb
}

/// Draw the climb group at `x`: the triangle, then `climb`. The triangle is drawn, because the
/// panel font has no `↑` glyph.
pub(super) fn climb_group(cv: &mut impl Surface, x: i32, sy: i32, climb: &str, color: u16) {
    // The base sits on the baseline, so the triangle reads as a capital.
    let base = sy + LINE2_FONT.cap_bottom() as i32 - 1;
    cv.triangle(
        Point::new(x, base),
        Point::new(x + CLIMB_TRI, base),
        Point::new(x + CLIMB_TRI / 2, base - CLIMB_TRI),
        color,
    );
    cv.text(climb, Point::new(x + CLIMB_GLYPH_W, sy), LINE2_FONT, TextAlign::Left, color);
}

/// The weekday of `date`, in days since 1970-01-01, which was a Thursday.
fn weekday(date: u16) -> Msg {
    WEEKDAYS[(usize::from(date) + 3) % 7]
}

/// Line 2 of a trip row: the next day, or done, then the day count when it fits `budget_px`. A
/// trip without a day to pick has neither.
fn trip_meta(t: &TripSummary, progress: Option<&TripProgress>, lang: Language, budget_px: i32) -> heapless::String<48> {
    let mut s = heapless::String::new();
    if t.is_empty_folder() {
        let _ = s.push_str(tr(Msg::RouteMenuNoDays, lang));
        return s;
    }
    match t.next_day(progress) {
        Some(k) => {
            let _ = write!(s, "{} {} {}", tr(Msg::RouteMenuDay, lang), k + 1, tr(Msg::RouteMenuDayNext, lang));
        }
        None => {
            let _ = s.push_str(tr(Msg::RouteMenuTripDone, lang));
        }
    }
    let n = t.stage_ids.len();
    let word = tr(if n == 1 { Msg::RouteMenuDayOne } else { Msg::RouteMenuDays }, lang);
    let mut count: heapless::String<24> = heapless::String::new();
    let _ = write!(count, " · {n} {word}");
    let chars = s.chars().count() + count.chars().count();
    if chars as i32 * LINE2_FONT.char_width() as i32 <= budget_px {
        let _ = s.push_str(&count);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
    use crate::settings::Language;
    use crate::trip::TripInput;
    use crate::{AppState, Settings};
    use obc_map_scene::BBox;

    /// A minimal named route summary; `handle` only reads the name.
    fn summary(name: &str) -> RouteSummary {
        RouteSummary {
            name: heapless::String::try_from(name).unwrap(),
            distance_km: 10,
            climb_m: 100,
            bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            start_lon: 0,
            start_lat: 0,
        }
    }

    /// A resolved trip over the given catalog indices, against a catalog whose ids equal indices.
    fn trip(
        id: crate::CatalogObjectId,
        name: &str,
        stages: &[crate::CatalogObjectId],
        catalog: &[RouteSummary],
    ) -> TripSummary {
        let ids: heapless::Vec<crate::CatalogObjectId, { crate::route::MAX_ROUTES }> =
            (0..catalog.len() as crate::CatalogObjectId).collect();
        TripSummary::resolve(&TripInput { id, key: 1, name, start_date: 0, stage_ids: stages }, catalog, &ids)
    }

    fn run(
        scr: &mut RouteMenuScreen,
        act: &mut Activity,
        rec: &mut crate::RecorderMachine,
        routes: &[RouteSummary],
        trips: &[TripSummary],
        g: Gesture,
    ) -> Transition {
        let mut navigator = crate::navigator::NavigatorMachine::new();
        run_with_nav(scr, act, rec, &mut navigator, routes, trips, g)
    }

    fn run_with_nav(
        scr: &mut RouteMenuScreen,
        act: &mut Activity,
        rec: &mut crate::RecorderMachine,
        navigator: &mut crate::navigator::NavigatorMachine,
        routes: &[RouteSummary],
        trips: &[TripSummary],
        g: Gesture,
    ) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { routes, trips, recorder: rec, navigator, ..test_ctx(&mut st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    #[test]
    fn picking_a_route_during_a_route_less_ride_opens_the_swap() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Riding);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        rec.test_open();
        assert_eq!(navigator.route_state().active_route, None);
        let mut scr = RouteMenuScreen::new();
        let t = run_with_nav(&mut scr, &mut act, &mut rec, &mut navigator, &routes, &[], Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteSwap(_))), "the guarded swap card opens");
        assert_eq!(navigator.route_state().active_route, None);
    }

    #[test]
    fn top_level_lists_folders_then_unfiled_routes() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A"), summary("B"), summary("C")];
        let trips = [trip(9, "Trip", &[0, 1], &routes)];
        // Row 0 is the folder; row 1 is the one unfiled route, catalog index 2.
        let mut scr = RouteMenuScreen::new();
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, &mut rec, &routes, &trips, Gesture::Press);
        assert!(
            matches!(t, Transition::Push(Screen::RouteMenu(_))),
            "pressing a folder opens its stage list (a scoped Route menu)"
        );
        let mut scr = RouteMenuScreen::new();
        run(&mut scr, &mut Activity::new(Mode::Idle), &mut rec, &routes, &trips, Gesture::Step(1)); // → row 1
        let mut act = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let t = run_with_nav(&mut scr, &mut act, &mut rec, &mut navigator, &routes, &trips, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteOverview(_))), "the loose route opens its overview");
        assert_eq!(
            navigator.route_state().active_route,
            Some(2),
            "and it's the unfiled catalog route (index 2), not a filed one"
        );
    }

    #[test]
    fn internal_routes_are_absent_from_both_scopes_and_selection_follows_the_saved_route() {
        let routes = [summary("Saved A"), summary("Visit"), summary("Saved B")];
        let mut rows = heapless::Vec::new();
        let mut menu = RouteMenuScreen::new();
        menu.build_rows(&[], routes.len(), 0b010, &mut rows);
        assert!(matches!(rows.as_slice(), [Row::Route(0), Row::Route(2)]));
        menu.selected = 1;
        menu.pin(&rows, &[]);
        menu.remap_routes(&|index| Some(index + 1), &[], 4, 0b0011);
        assert_eq!(menu.selected, 1);
        assert!(matches!(menu.sel_id, Some(SelId::Route(3))));

        let trips = [trip(9, "Tour", &[0, 1, 2], &routes)];
        RouteMenuScreen::trip(&trips[0], None).build_rows(&trips, routes.len(), 0b010, &mut rows);
        assert!(matches!(rows.as_slice(), [Row::Day { day: 0, route: 0 }, Row::Day { day: 2, route: 2 }]));
        RouteMenuScreen::new().build_rows(&[], 1, 1, &mut rows);
        assert!(rows.is_empty(), "an internal-only catalog has no saved routes");
    }

    #[test]
    fn long_press_folder_opens_the_delete_confirm() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A"), summary("B")];
        let trips = [trip(42, "Trip", &[0, 1], &routes)];
        let mut scr = RouteMenuScreen::new();
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &mut rec, &routes, &trips, Gesture::Hold);
        assert!(matches!(t, Transition::Push(Screen::TripDelete(_))), "a folder long-press confirms a cascade delete");
    }

    #[test]
    fn long_press_route_does_nothing() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A")];
        let mut scr = RouteMenuScreen::new();
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &mut rec, &routes, &[], Gesture::Hold);
        assert!(matches!(t, Transition::None));
    }

    #[test]
    fn a_day_press_opens_the_day_route() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A"), summary("B"), summary("C")];
        let trips = [trip(9, "Trip", &[1, 2], &routes)]; // members: catalog indices 1, 2
        let mut scr = RouteMenuScreen::trip(&trips[0], None);
        let mut act = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let t = run_with_nav(&mut scr, &mut act, &mut rec, &mut navigator, &routes, &trips, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteOverview(_))));
        assert_eq!(navigator.route_state().active_route, Some(1), "Day 1 is catalog route 1");
    }

    #[test]
    fn empty_folder_has_no_rows_and_backs_out() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A")];
        let trips = [trip(9, "Trip", &[99], &routes)]; // the only ref dangles
        assert!(trips[0].is_empty_folder());
        let mut scr = RouteMenuScreen::trip(&trips[0], None);
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &mut rec, &routes, &trips, Gesture::Back);
        assert!(matches!(t, Transition::Pop), "Back leaves the empty folder");
        let mut scr = RouteMenuScreen::trip(&trips[0], None);
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &mut rec, &routes, &trips, Gesture::Press);
        assert!(matches!(t, Transition::None), "a press in an empty folder does nothing");
    }

    /// Three days on catalog routes 0, 1 and 2, keyed 1, starting Monday 2025-09-29.
    fn three_days(routes: &[RouteSummary]) -> TripSummary {
        let ids = [0, 1, 2];
        TripSummary::resolve(&TripInput { id: 9, key: 1, name: "Alps", start_date: MON, stage_ids: &ids }, routes, &ids)
    }

    const MON: u16 = 20_360;

    fn finished(last: u16) -> TripProgress {
        TripProgress {
            key: 1,
            day: last,
            day_route: crate::trip::RouteVersion { id: u64::from(last), revision: 1 },
            metres: 0,
            last_finished: Some(last),
            dates: [0; obc_route::MAX_TRIP_DAYS],
        }
    }

    #[test]
    fn trip_row_says_what_is_next() {
        let t = three_days(&[]);
        let meta = |p: Option<&TripProgress>| trip_meta(&t, p, Language::En, 200);
        assert_eq!(meta(None), "Day 1 next · 3 days");
        assert_eq!(meta(Some(&finished(0))), "Day 2 next · 3 days");
        assert_eq!(meta(Some(&finished(2))), "Done · 3 days");
        assert_eq!(trip_meta(&t, None, Language::En, 180), "Day 1 next", "the count drops whole");
    }

    #[test]
    fn every_language_keeps_the_day_count_on_a_panel_wide_row() {
        let budget = 240 - 2 * two_line::SIDE_INSET - 4 - two_line::NAME_INSET;
        let t = three_days(&[]);
        for lang in Language::ALL {
            for p in [None, Some(&finished(2))] {
                let meta = trip_meta(&t, p, lang, budget);
                assert!(meta.ends_with(tr(Msg::RouteMenuDays, lang)), "{lang:?}: {meta}");
            }
        }
    }

    #[test]
    fn a_trip_without_a_day_route_has_no_days() {
        let trip = |stage_ids| {
            TripSummary::resolve(&TripInput { id: 9, key: 1, name: "Alps", start_date: 0, stage_ids }, &[], &[])
        };
        assert_eq!(trip_meta(&trip(&[]), None, Language::En, 200), "No days", "not \"Done · 0 days\"");
        // No day route is in the catalog.
        let dangling = trip(&[0, 1, 2]);
        assert_eq!(trip_meta(&dangling, None, Language::En, 200), "No days");
        let mut rows = heapless::Vec::new();
        RouteMenuScreen::trip(&dangling, None).build_rows(std::slice::from_ref(&dangling), 0, 0, &mut rows);
        assert!(rows.is_empty(), "the day list has nothing to pick");
    }

    #[test]
    fn the_day_list_opens_on_the_next_day() {
        let routes = [summary("A"), summary("B"), summary("C")];
        let t = three_days(&routes);
        assert_eq!(RouteMenuScreen::trip(&t, None).selected, 0);
        assert_eq!(RouteMenuScreen::trip(&t, Some(&finished(1))).selected, 2);
        assert_eq!(RouteMenuScreen::trip(&t, Some(&finished(2))).selected, 0, "a done trip opens on Day 1");
    }

    #[test]
    fn weekdays_count_from_a_thursday_epoch() {
        assert!(matches!(weekday(0), Msg::WeekdayThu));
        assert!(matches!(weekday(MON), Msg::WeekdayMon));
        assert!(matches!(weekday(MON + 6), Msg::WeekdaySun));
    }
}
