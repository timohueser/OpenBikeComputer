//! The Route menu: pick a route, or open a trip folder. Two levels in one screen. The top level
//! lists the trip folders first and then the unfiled routes; a folder press pushes a second menu
//! scoped to that trip. A route row is the same code path in both scopes, and never nests further.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::route::RouteSummary;
use crate::trip::TripSummary;
use crate::Msg;

use super::vocab::chrome::empty_state;
use super::vocab::list::{self, ListGeometry, Separators};
use super::vocab::marquee::{fit, MarqueeFrame};
use super::{
    palette, Ctx, MapScreen, Render, RouteOverviewScreen, RouteSwapScreen, Screen, Transition, TripDeleteScreen,
};

/// Per-route pane height (two lines: name + stats), sized so the routes fill the full list area.
const ROW_H: i32 = 66;

/// Text inset of the name/stats column from the row area's left edge. The distance on line 2 shares
/// this x, so the name and the distance form one left column.
const NAME_INSET: i32 = 12;

/// The list side inset: the row area's margin from the panel edge.
const SIDE_INSET: i32 = 12;

/// The stats line's second column, the climb group, as a fraction of the row's inner width, so the
/// climb figures align across every row whatever the width of the distance.
const CLIMB_COL_PCT: i32 = 55;

/// The count badge box height (px), also its minimum width, so one digit sits in a near-square pill
/// and two digits widen it symmetrically.
const BADGE_H: i32 = 24;
/// Horizontal padding inside the count badge (both sides together).
const BADGE_PAD: i32 = 14;

/// The upper bound on rows: every trip folder plus every route, when nothing is filed.
const ROW_CAP: usize = crate::trip::MAX_TRIPS + crate::route::MAX_ROUTES;

/// One row of the menu: a trip folder by trip-catalog index, or a route by route-catalog index.
/// The route index is a real catalog index in either scope.
#[derive(Clone, Copy)]
enum Row {
    Folder(usize),
    Route(usize),
}

/// The identity of the highlighted row, so the highlight can follow it across a live catalog
/// rescan. A route is pinned by its catalog index, a folder by its trip's durable id.
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
            Row::Route(ri) => Some(SelId::Route(ri)),
        }
    }

    fn is(self, id: SelId, trips: &[TripSummary]) -> bool {
        match (self, id) {
            (Row::Folder(ti), SelId::Folder(tid)) => trips.get(ti).is_some_and(|t| t.id == tid),
            (Row::Route(ri), SelId::Route(i)) => ri == i,
            _ => false,
        }
    }
}

/// What this menu instance lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteMenuScope {
    /// The top level: trip folders first, then the unfiled routes.
    TopLevel,
    /// Inside one trip's folder: that trip's member routes only. The durable id is resolved against
    /// the live trip catalog each frame, so a rescan that reorders the trips cannot mis-scope it.
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

    /// A stage list scoped to the trip with durable id `trip_id`.
    pub fn trip(trip_id: crate::CatalogObjectId) -> Self {
        RouteMenuScreen { selected: 0, sel_id: None, scope: RouteMenuScope::Trip { trip_id } }
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
                    for &idx in t.stage_indices.iter() {
                        if internal_routes & (1 << idx) == 0 {
                            let _ = out.push(Row::Route(idx as usize));
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
                Row::Folder(ti) => Transition::Push(Screen::RouteMenu(RouteMenuScreen::trip(cx.trips[ti].id))),
                Row::Route(ri) => self.press_route(ri, cx),
            },
            // A long-press on a folder opens the cascade-delete confirm; on a route row it does
            // nothing, because a route is deleted from the Route overview.
            Gesture::Hold if len > 0 => match rows[self.selected.min(len - 1)] {
                Row::Folder(ti) => {
                    let t = &cx.trips[ti];
                    Transition::Push(Screen::TripDelete(TripDeleteScreen::new(t.id, &t.name)))
                }
                Row::Route(_) => Transition::None,
            },
            Gesture::Back => Transition::Pop, // top level → Home/Menu; stage list → top level
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

        let geo = ListGeometry::below_title(w, h, ROW_H, 8, SIDE_INSET, Separators::Unselected);

        // The title is "ROUTES" at the top level and the trip's name inside a folder. `title_buf`
        // is filled only on the trip-name path, so it is declared uninitialized and borrowed there.
        let title_buf: heapless::String<64>;
        let title = match self.scope {
            RouteMenuScope::TopLevel => rx.t(Msg::RouteMenuTitle),
            RouteMenuScope::Trip { trip_id } => match trips.iter().find(|t| t.id == trip_id) {
                Some(t) => {
                    // Leave room for the scroll counter the title bar's right slot may show.
                    title_buf = fit(&t.name, w - 72, Font::Body);
                    &title_buf
                }
                None => rx.t(Msg::RouteMenuTitle),
            },
        };

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, title, pos, total, geo.visible);

        if total == 0 {
            let sub = match self.scope {
                RouteMenuScope::TopLevel => rx.t(Msg::RouteMenuNoRoutesSub),
                RouteMenuScope::Trip { .. } => rx.t(Msg::RouteMenuFolderEmptySub),
            };
            empty_state(cv, w, h, rx.t(Msg::RouteMenuNoRoutes), sub);
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let accent = if row.selected { INK } else { SUBTEXT };
            match rows[row.index] {
                Row::Folder(ti) => draw_folder_row(cv, &row, &rx.marquee, &trips[ti], w, accent),
                Row::Route(ri) => {
                    let unaccepted = rx.unaccepted_routes & (1 << ri) != 0;
                    draw_route_row(
                        cv,
                        &row,
                        &rx.marquee,
                        &routes[ri],
                        w,
                        accent,
                        unaccepted.then(|| rx.t(Msg::RouteMenuUnaccepted)),
                    );
                }
            }
        });
    }
}

/// The x of the climb column, from the row area's left edge.
fn climb_col_x(area_x: i32, w: i32) -> i32 {
    area_x + (w - 2 * SIDE_INSET) * CLIMB_COL_PCT / 100
}

/// A standard route row: the name on line 1, the distance under it, and the climb group at the
/// fixed second column.
fn draw_route_row(
    cv: &mut impl Surface,
    row: &list::RowCtx,
    marquee: &MarqueeFrame,
    route: &RouteSummary,
    w: i32,
    accent: u16,
    unavailable: Option<&str>,
) {
    use palette::*;
    let area = &row.area;
    let y = area.top_left.y;
    let name_x = area.top_left.x + NAME_INSET;
    let name = marquee.fit(&route.name, (w - 20) - name_x, Font::Body, row.scroll());
    cv.text(&name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, INK);

    let sy = y + 35;
    if let Some(label) = unavailable {
        cv.text(label, Point::new(name_x, sy), Font::Label, TextAlign::Left, SUBTEXT);
        return;
    }
    let mut dist: heapless::String<12> = heapless::String::new();
    let _ = write!(dist, "{} km", route.distance_km);
    cv.text(&dist, Point::new(name_x, sy), Font::Label, TextAlign::Left, accent);

    climb_group(cv, climb_col_x(area.top_left.x, w), sy, route.climb_m, accent);
}

/// Draw the climb group at `x` and return the x past its text. The triangle is drawn, because the
/// panel font has no `↑` glyph.
fn climb_group(cv: &mut impl Surface, x: i32, sy: i32, climb_m: u32, accent: u16) -> i32 {
    cv.triangle(Point::new(x, sy + 14), Point::new(x + 9, sy + 14), Point::new(x + 4, sy + 5), accent);
    let mut climb: heapless::String<12> = heapless::String::new();
    let _ = write!(climb, "{climb_m} m");
    cv.text(&climb, Point::new(x + 16, sy), Font::Label, TextAlign::Left, accent);
    x + 16 + text_width(&climb, Font::Label) as i32
}

/// A trip folder row. There is no folder pictogram: the count badge alone marks the trip, so the
/// name keeps the full remaining width. Line 2 uses the two columns of a route row, so the stats
/// align down the list.
fn draw_folder_row(
    cv: &mut impl Surface,
    row: &list::RowCtx,
    marquee: &MarqueeFrame,
    t: &TripSummary,
    w: i32,
    accent: u16,
) {
    use palette::*;
    let area = &row.area;
    let y = area.top_left.y;
    let n = t.stage_indices.len();
    let name_x = area.top_left.x + NAME_INSET;

    // The badge box is centred on the number and widens with the digit count.
    let mut nbuf: heapless::String<8> = heapless::String::new();
    let _ = write!(nbuf, "{n}");
    let badge_w = (text_width(&nbuf, Font::Label) as i32 + BADGE_PAD).max(BADGE_H);
    let badge_x = w - 20 - badge_w;
    let badge_y = y + 8; // box y+8..y+32; Label cap (18 px) at y+11 → 3 px margin above and below
    cv.round(rect(badge_x, badge_y, badge_w, BADGE_H), 6, WOOD);
    cv.text(&nbuf, Point::new(badge_x + badge_w / 2, badge_y + 3), Font::Label, TextAlign::Center, PARCHMENT);

    let name = marquee.fit(&t.name, (badge_x - 8) - name_x, Font::Body, row.scroll());
    cv.text(&name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, INK);

    let sy = y + 35;
    let mut dist: heapless::String<12> = heapless::String::new();
    let _ = write!(dist, "{} km", t.distance_km);
    cv.text(&dist, Point::new(name_x, sy), Font::Label, TextAlign::Left, accent);
    climb_group(cv, climb_col_x(area.top_left.x, w), sy, t.climb_m, accent);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
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
        TripSummary::resolve(&TripInput { id, name, stage_ids: stages }, catalog, &ids)
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
        RouteMenuScreen::trip(9).build_rows(&trips, routes.len(), 0b010, &mut rows);
        assert!(matches!(rows.as_slice(), [Row::Route(0), Row::Route(2)]));
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
    fn a_folder_stage_list_presses_the_member_route() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A"), summary("B"), summary("C")];
        let trips = [trip(9, "Trip", &[1, 2], &routes)]; // members: catalog indices 1, 2
        let mut scr = RouteMenuScreen::trip(9);
        let mut act = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let t = run_with_nav(&mut scr, &mut act, &mut rec, &mut navigator, &routes, &trips, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteOverview(_))));
        assert_eq!(navigator.route_state().active_route, Some(1), "the first stage is catalog route 1");
    }

    #[test]
    fn empty_folder_has_no_rows_and_backs_out() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary("A")];
        let trips = [trip(9, "Trip", &[99], &routes)]; // the only ref dangles
        assert!(trips[0].is_empty_folder());
        let mut scr = RouteMenuScreen::trip(9);
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &mut rec, &routes, &trips, Gesture::Back);
        assert!(matches!(t, Transition::Pop), "Back leaves the empty folder");
        let mut scr = RouteMenuScreen::trip(9);
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &mut rec, &routes, &trips, Gesture::Press);
        assert!(matches!(t, Transition::None), "a press in an empty folder does nothing");
    }
}
