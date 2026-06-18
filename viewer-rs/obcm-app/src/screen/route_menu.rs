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
use crate::input::Gesture;

use super::{
    list_frame, palette, scrollbar, window_start, Ctx, MapScreen, Render, RouteSwapScreen, Screen,
    Transition, LIST_TOP,
};

/// Per-route pane height (two lines: name + stats). Sized so exactly four routes fill the
/// list area below the title bar, with the two-line content centred in each pane.
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
            Gesture::Turn(n) => {
                self.selected = super::step_selection(self.selected, n, len);
                Transition::None
            }
            Gesture::Press if len > 0 => {
                let i = self.selected.min(len - 1);
                // A session already running changes the meaning of "load": picking a
                // *different* route asks whether to swap navigation only or save the ride and
                // start fresh; re-picking the active route just returns to riding it.
                if cx.activity.is_tracking() {
                    if cx.activity.active_route == Some(i) {
                        return Transition::Root(Screen::Map(MapScreen::new()));
                    }
                    return Transition::Push(Screen::RouteSwap(RouteSwapScreen::new(i)));
                }
                // No session (loading from Idle): tracking starts, the camera drops into the
                // riding view (follow, heading-up, zoomed in at the start), and the Map opens.
                // The host opens the geometry + the ride log on the index/session change.
                cx.state.enter_riding_view(cx.routes[i].start_lon, cx.routes[i].start_lat);
                cx.activity.mode = Mode::Riding;
                cx.activity.active_route = Some(i);
                cx.activity.start_session();
                Transition::Root(Screen::Map(MapScreen::new()))
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
            super::empty_state(&mut cv, w, h, "No routes yet", "Import a GPX file");
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

            // Pointer bullet (vertically centred on the whole two-line entry) + name.
            // The name is truncated with ".." when it would overrun the pane (the panel
            // font has no ellipsis glyph).
            let accent = if selected { INK } else { SUBTEXT };
            let row_mid = y + 33;
            cv.triangle(Point::new(24, row_mid - 8), Point::new(24, row_mid + 8), Point::new(36, row_mid), accent);
            let name_max = (((w - 20) - 44) / Font::Body.char_width() as i32).max(6) as usize;
            let name = fit_name(&route.name, name_max);
            cv.text(&name, Point::new(44, y + 9), Font::Body, TextAlign::Left, INK);

            // Stats line: "NNN km" then an up-triangle + "NNNN m" of climb. The climb column
            // sits at a fixed x with room for 5-digit metres (10 000 m+) inside the frame.
            let sy = y + 35;
            let mut dist: heapless::String<12> = heapless::String::new();
            let _ = write!(dist, "{} km", route.distance_km);
            cv.text(&dist, Point::new(44, sy), Font::Label, TextAlign::Left, accent);

            let cx0 = 126;
            cv.triangle(Point::new(cx0, sy + 9), Point::new(cx0 + 9, sy + 9), Point::new(cx0 + 4, sy), accent);
            let mut climb: heapless::String<12> = heapless::String::new();
            let _ = write!(climb, "{} m", route.climb_m);
            cv.text(&climb, Point::new(cx0 + 16, sy), Font::Label, TextAlign::Left, accent);

            // Separator below a row when the next visible row is also drawn.
            if !selected && slot + 1 < visible && i + 1 < total {
                cv.hline(16, y + ROW_H - 4, w - 32, RULE);
            }
        }

        scrollbar(&mut cv, w - 8, LIST_TOP, visible as i32 * ROW_H, total, first, visible);
        RenderStats::default()
    }
}

/// Fit a route name into `max_chars`, appending ".." when it has to be truncated (the
/// panel font has no ellipsis glyph). Truncates on a char boundary; the buffer comfortably
/// holds a full `NAME_CAP` (48-char) name for the no-truncation case.
fn fit_name(name: &str, max_chars: usize) -> heapless::String<64> {
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
