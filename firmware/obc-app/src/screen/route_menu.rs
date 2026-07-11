//! The Route menu — pick a route. The shared list chrome with taller panes showing each route's
//! distance and climb. Reached from the main Menu's Routes station (Home → Menu → Routes); `press`
//! opens the [`Route overview`](super::RouteOverviewScreen) (or, mid-ride, resumes / opens the
//! swap flow), `back` returns.
//!
//! Routes come from the app's catalog ([`Render::routes`]/[`Ctx::routes`]), populated by the host
//! from its store. Picking one sets [`Activity::active_route`](crate::Activity::active_route); the
//! host keys geometry loading on it, so the route is streaming open while the overview shows.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::list::{self, ListGeometry, Separators};
use super::{palette, Ctx, MapScreen, Render, RouteOverviewScreen, RouteSwapScreen, Screen, Transition};

/// Per-route pane height (two lines: name + stats), sized so the routes fill the full list area
/// (the hold-to-delete footer is gone — deleting a route now lives on the Route overview, T3).
const ROW_H: i32 = 66;

/// Text inset of the name/stats column from the row area's left edge. The per-row `▶` triangle is
/// gone (T3); the name column shifts left to this inset and gains the triangle's width. Distance on
/// line 2 shares this x, so name and distance form one left column.
const NAME_INSET: i32 = 12;

/// The stats line's second column — the climb group (`▲` + metres) — as a fraction of the row's
/// inner width, so the climb figures line up across every row regardless of distance width (T3).
const CLIMB_COL_PCT: i32 = 55;

/// The route list. State is the highlighted route.
#[derive(Debug, Default)]
pub struct RouteMenuScreen {
    selected: usize,
}

impl RouteMenuScreen {
    pub fn new() -> Self {
        RouteMenuScreen { selected: 0 }
    }

    /// Re-point the highlight after a live catalog rescan (#450): the selection follows the
    /// previously-highlighted route's *identity* to its new index; if that route vanished it falls
    /// back to the nearest row (clamped near its old position — never a dangling index). The list
    /// itself refreshes in place via the shared catalog.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>, new_len: usize) {
        self.selected = remap(self.selected).unwrap_or_else(|| self.selected.min(new_len.saturating_sub(1)));
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.routes.len();
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, len),
            Gesture::Press if len > 0 => {
                let i = self.selected.min(len - 1);
                // With a session running, picking a *different* route asks whether to swap
                // navigation only or save-and-start-fresh; re-picking the active route just rides it.
                if cx.activity.is_tracking() {
                    if cx.activity.active_route == Some(i) {
                        return Transition::Root(Screen::Map(MapScreen::new()));
                    }
                    return Transition::Push(Screen::RouteSwap(RouteSwapScreen::new(i)));
                }
                // No session (picking from Idle): open the Route overview. Setting `active_route`
                // makes the host stream the route open (and the profile build) behind the page;
                // the overview's `press` starts the session, its `back` restores the previous one.
                let prev = cx.activity.active_route.replace(i);
                Transition::Push(Screen::RouteOverview(RouteOverviewScreen::new(i, prev)))
            }
            Gesture::Back => Transition::Pop, // return to caller (Home / Menu)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let routes = rx.routes;
        let total = routes.len();
        // The list fills the frame below the title bar — no footer to reserve (delete moved to the
        // Route overview, T3).
        let geo = ListGeometry::below_title(w, h, ROW_H, 8, 12, Separators::Unselected);

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, rx.t(Msg::RouteMenuTitle), pos, total, geo.visible);

        if total == 0 {
            super::empty_state(cv, w, h, rx.t(Msg::RouteMenuNoRoutes), rx.t(Msg::RouteMenuNoRoutesSub));
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let route = &routes[row.index];
            let y = row.area.top_left.y;
            let accent = if row.selected { INK } else { SUBTEXT };

            // Name at the row inset — the `▶` triangle is gone, so the name column starts here and
            // gains that width. Truncated with ".." when it overruns (no ellipsis glyph).
            let name_x = row.area.top_left.x + NAME_INSET;
            let name_max = (((w - 20) - name_x) / Font::Body.char_width() as i32).max(6) as usize;
            let name = fit_name(&route.name, name_max);
            cv.text(&name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, INK);

            // Stats line, two aligned columns: distance under the name (its x), then the climb group
            // (an up-triangle + "NNNN m") at a fixed second column so climb figures line up row-to-row.
            let sy = y + 35;
            let mut dist: heapless::String<12> = heapless::String::new();
            let _ = write!(dist, "{} km", route.distance_km);
            cv.text(&dist, Point::new(name_x, sy), Font::Label, TextAlign::Left, accent);

            let cx0 = row.area.top_left.x + row.area.size.width as i32 * CLIMB_COL_PCT / 100;
            // The climb arrow, vertically centred on the Label cap (cap 18 px from `sy`, arrow 9).
            cv.triangle(Point::new(cx0, sy + 14), Point::new(cx0 + 9, sy + 14), Point::new(cx0 + 4, sy + 5), accent);
            let mut climb: heapless::String<12> = heapless::String::new();
            let _ = write!(climb, "{} m", route.climb_m);
            cv.text(&climb, Point::new(cx0 + 16, sy), Font::Label, TextAlign::Left, accent);
        });
    }
}

/// Fit a route name into `max_chars`, appending ".." when truncated (no ellipsis glyph).
/// Truncates on a char boundary. Shared with the Route overview's title.
pub(crate) fn fit_name(name: &str, max_chars: usize) -> heapless::String<64> {
    let mut s = heapless::String::new();
    if name.chars().count() <= max_chars {
        let _ = s.push_str(name);
    } else {
        for c in name.chars().take(max_chars.saturating_sub(2)) {
            let _ = s.push(c);
        }
        let _ = s.push_str("..");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::route::RouteSummary;
    use crate::{AppState, Settings};
    use obc_route::BBox;

    /// A minimal named route summary — the Route menu only reads `name` in `handle`.
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

    fn run(scr: &mut RouteMenuScreen, act: &mut Activity, routes: &[RouteSummary], g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
            routes,
            rides: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Picking a route **mid route-less ride** (a running session with no `active_route`) goes
    /// through the guarded swap flow — the "ROUTE ACTIVE" card — exactly as picking a *different*
    /// route mid-ride does. There's no route yet to re-navigate, but a session is live, so the
    /// keep-or-restart choice still applies; it must never silently start navigating.
    #[test]
    fn picking_a_route_during_a_route_less_ride_opens_the_swap() {
        let routes = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // tracking, but route-less…
        assert_eq!(act.active_route, None, "…so no route is navigated");
        let mut scr = RouteMenuScreen::new(); // highlight row 0
        let t = run(&mut scr, &mut act, &routes, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteSwap(_))), "the guarded swap card opens");
        assert_eq!(act.active_route, None, "picking alone doesn't attach the route — the card decides");
    }
}
