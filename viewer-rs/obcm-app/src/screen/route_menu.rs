//! The Route menu — pick a route to load. Same chrome as the main [`Menu`](super::MenuScreen)
//! (via [`list_frame`]), but with taller panes that show each route's total distance
//! and climb. Reached from Home (`press`) and from the main Menu's Routes item;
//! `press` loads the selected route (starts riding, frames it on the Map) and opens the
//! Map, `back` returns to the caller.
//!
//! Routes come from the app's catalog ([`Render::routes`]/[`Ctx::routes`]), populated by
//! the host from its store (the sim's folder of `.obcr` files, the device's SD card).
//! Loading sets [`Activity::active_route`](crate::Activity::active_route) and centers
//! the camera on the route's bbox; the host then opens the geometry for the Map.

use core::fmt::Write;

use embedded_graphics::prelude::{DrawTarget, Point};
use obcm_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::activity::Mode;
use crate::app::AppState;
use crate::input::Gesture;
use crate::route::RouteSummary;

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
        let len = cx.routes.len();
        match g {
            Gesture::Turn(n) if len > 0 => {
                self.selected = (self.selected as i32 + n).rem_euclid(len as i32) as usize;
                Transition::None
            }
            Gesture::Press if len > 0 => {
                // Load the selected route: tracking starts, the camera frames the
                // route, and the Map opens. The host opens the geometry on the index
                // change (no I/O here — the summary's bbox is enough to center).
                let i = self.selected.min(len - 1);
                center_camera(cx.state, &cx.routes[i]);
                cx.activity.mode = Mode::Riding;
                cx.activity.active_route = Some(i);
                Transition::Replace(Screen::Map(MapScreen::new()))
            }
            Gesture::Back => Transition::Pop, // return to caller (Home / Menu)
            _ => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let routes = rx.routes;
        let total = routes.len();
        let mut cv = Canvas::new(target, color_fn);

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list_frame(&mut cv, w, h, "ROUTES", pos, total);

        // Empty catalog: prompt the rider to add a route rather than show a blank list.
        if total == 0 {
            cv.text("No routes yet", Point::new(w / 2, h / 2 - 10), Font::Body, TextAlign::Center, INK);
            cv.text("Import a GPX file", Point::new(w / 2, h / 2 + 12), Font::Label, TextAlign::Center, SUBTEXT);
            return RenderStats::default();
        }

        // Window the list to the rows that fit, scrolling to keep the selection
        // visible, and show a scrollbar when there are more routes than fit.
        let sel = self.selected.min(total - 1);
        let list_h = h - LIST_TOP - 6;
        let visible = (list_h / ROW_H).max(1) as usize;
        let first = window_start(sel, visible, total);

        for slot in 0..visible {
            let i = first + slot;
            if i >= total {
                break;
            }
            let route = &routes[i];
            let y = LIST_TOP + slot as i32 * ROW_H;
            let selected = i == sel;

            if selected {
                cv.round(rect(12, y, w - 24, ROW_H - 8), 6, AMBER);
            }

            // Pointer bullet + route name on the first line.
            let accent = if selected { INK } else { SUBTEXT };
            let name_mid = y + 16;
            cv.triangle(Point::new(24, name_mid - 6), Point::new(24, name_mid + 6), Point::new(33, name_mid), accent);
            cv.text(&route.name, Point::new(42, y + 8), Font::Body, TextAlign::Left, INK);

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

/// Frame `route` on the Map: center the camera on its bbox and zoom so the whole route
/// fits the device panel. Using the larger bbox span against the panel's short edge
/// guarantees the route is fully visible (a touch conservative — the rider can zoom
/// from there). The simulator window may differ in size; this targets the real panel.
fn center_camera(state: &mut AppState, route: &RouteSummary) {
    let b = route.bbox;
    state.cam_lon = ((b.min_lon as i64 + b.max_lon as i64) / 2) as i32;
    state.cam_lat = ((b.min_lat as i64 + b.max_lat as i64) / 2) as i32;
    // zoom is pixels per microdegree-of-latitude; the projection narrows longitude by
    // cos(lat), so fitting the larger raw span to the 240 px short edge fits both axes.
    const PANEL_SHORT: f32 = 240.0;
    let span = (b.max_lon - b.min_lon).max(b.max_lat - b.min_lat).max(1) as f32;
    state.zoom = (PANEL_SHORT * 0.85 / span).clamp(1e-6, 1e4);
}
