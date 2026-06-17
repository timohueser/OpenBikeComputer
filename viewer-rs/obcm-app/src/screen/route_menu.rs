//! The Route menu — pick a route to load. Same chrome as the main [`Menu`](super::MenuScreen)
//! (via [`list_frame`]), but with taller panes that show each route's total
//! distance and climb. Reached from Home (`press`) and from the main Menu's
//! Routes item; `press` loads the selected route (starts riding) and opens the
//! Map, `back` returns to the caller.
//!
//! Routes come from [`crate::route::routes`] (mock data for now). Loading sets
//! [`Activity::active_route`](crate::Activity::active_route); the geometry/camera
//! handoff (centering the Map on the route) joins when route loading is real.

use core::fmt::Write;

use embedded_graphics::prelude::{DrawTarget, Point};
use obcm_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::activity::Mode;
use crate::input::Gesture;
use crate::route::routes;

use super::{list_frame, palette, scrollbar, window_start, Ctx, MapScreen, Render, Screen, Transition, LIST_TOP};

/// Per-route pane height (two lines: name + stats).
const ROW_H: i32 = 58;

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
        match g {
            Gesture::Turn(n) => {
                let len = routes().len() as i32;
                self.selected = (self.selected as i32 + n).rem_euclid(len) as usize;
                Transition::None
            }
            Gesture::Press => {
                // Load the selected route: tracking starts and the Map opens.
                cx.activity.mode = Mode::Riding;
                cx.activity.active_route = Some(self.selected);
                Transition::Replace(Screen::Map(MapScreen::new()))
            }
            Gesture::Back => Transition::Pop, // return to caller (Home / Menu)
            Gesture::Hold => Transition::None,
            Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let routes = routes();
        let total = routes.len();
        let mut cv = Canvas::new(target, color_fn);

        list_frame(&mut cv, w, h, "ROUTES", self.selected + 1, total);

        // Window the list to the rows that fit, scrolling to keep the selection
        // visible, and show a scrollbar when there are more routes than fit.
        let list_h = h - LIST_TOP - 6;
        let visible = (list_h / ROW_H).max(1) as usize;
        let first = window_start(self.selected, visible, total);

        for slot in 0..visible {
            let i = first + slot;
            if i >= total {
                break;
            }
            let route = &routes[i];
            let y = LIST_TOP + slot as i32 * ROW_H;
            let selected = i == self.selected;

            if selected {
                cv.round(rect(12, y, w - 24, ROW_H - 8), 6, AMBER);
            }

            // Pointer bullet + route name on the first line.
            let accent = if selected { INK } else { SUBTEXT };
            let name_mid = y + 16;
            cv.triangle(Point::new(24, name_mid - 6), Point::new(24, name_mid + 6), Point::new(33, name_mid), accent);
            cv.text(route.name, Point::new(42, y + 8), Font::Body, TextAlign::Left, INK);

            // Stats line: "NNN km" then an up-triangle + "NNNN m" of climb.
            let sy = y + 32;
            let mut dist: heapless::String<12> = heapless::String::new();
            let _ = write!(dist, "{} km", route.distance_km);
            cv.text(&dist, Point::new(42, sy), Font::Label, TextAlign::Left, accent);

            let cx0 = 132;
            cv.triangle(Point::new(cx0, sy + 9), Point::new(cx0 + 8, sy + 9), Point::new(cx0 + 4, sy), accent);
            let mut climb: heapless::String<12> = heapless::String::new();
            let _ = write!(climb, "{} m", route.climb_m);
            cv.text(&climb, Point::new(cx0 + 14, sy), Font::Label, TextAlign::Left, accent);

            // Separator below a row when the next visible row is also drawn.
            if !selected && slot + 1 < visible && i + 1 < total {
                cv.hline(16, y + ROW_H - 4, w - 32, RULE);
            }
        }

        scrollbar(&mut cv, w - 8, LIST_TOP, visible as i32 * ROW_H, total, first, visible);
        RenderStats::default()
    }
}
