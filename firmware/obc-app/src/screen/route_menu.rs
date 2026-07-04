//! The Route menu — pick a route. The shared list chrome with taller panes showing each route's
//! distance and climb. Reached from Home (`press`) and the main Menu's Routes station; `press`
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

use super::list::{self, ListGeometry, Separators};
use super::{palette, Ctx, MapScreen, Render, RouteOverviewScreen, RouteSwapScreen, Screen, Transition};

/// Per-route pane height (two lines: name + stats), sized so four routes fill the list area.
const ROW_H: i32 = 66;

/// The route list. State is the highlighted route.
#[derive(Debug, Default)]
pub struct RouteMenuScreen {
    selected: usize,
}

impl RouteMenuScreen {
    pub fn new() -> Self {
        RouteMenuScreen { selected: 0 }
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
        let geo = ListGeometry::below_title(w, h, ROW_H, 8, 12, Separators::Unselected);

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, "ROUTES", pos, total, geo.visible);

        if total == 0 {
            super::empty_state(cv, w, h, "No routes yet", "Import a GPX file");
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let route = &routes[row.index];
            let y = row.area.top_left.y;

            // Pointer bullet + name, truncated with ".." when it overruns (no ellipsis glyph).
            let accent = if row.selected { INK } else { SUBTEXT };
            let row_mid = y + 33;
            cv.triangle(Point::new(24, row_mid - 8), Point::new(24, row_mid + 8), Point::new(36, row_mid), accent);
            let name_max = (((w - 20) - 44) / Font::Body.char_width() as i32).max(6) as usize;
            let name = fit_name(&route.name, name_max);
            cv.text(&name, Point::new(44, y + 9), Font::Body, TextAlign::Left, INK);

            // Stats line: "NNN km" then an up-triangle + "NNNN m" of climb. The climb column sits at
            // a fixed x with room for 5-digit metres.
            let sy = y + 35;
            let mut dist: heapless::String<12> = heapless::String::new();
            let _ = write!(dist, "{} km", route.distance_km);
            cv.text(&dist, Point::new(44, sy), Font::Label, TextAlign::Left, accent);

            let cx0 = 126;
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
