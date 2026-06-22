//! The "route already active" prompt — shown when a new route is picked mid-ride.
//!
//! Loading a route while a tracking session is running is ambiguous: keep recording the same
//! ride and just re-navigate, or save this ride and begin a fresh one. This opaque prompt
//! asks which. **Swap route** (press) keeps the session and only changes the navigated route;
//! **Save & new** (hold-guarded, since it ends a session) finalises the current track to a
//! `.gpx` and starts a new session on the picked route; **Cancel** (back) returns to the
//! route list. Reached from [`RouteMenuScreen`](super::RouteMenuScreen) when a session is
//! active and a *different* route is chosen.

use embedded_graphics::prelude::{DrawTarget, Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::activity::{Mode, TrackAction};
use crate::input::Gesture;

use super::{palette, title_frame, Ctx, MapScreen, Render, Screen, Transition};

/// One prompt option. `guard` = ends the current session → hold-to-confirm.
struct Item {
    label: &'static str,
    guard: bool,
}

const ITEMS: [Item; 3] = [
    Item { label: "Swap route", guard: false },
    Item { label: "Save & new", guard: true },
    Item { label: "Cancel", guard: false },
];

const SWAP: usize = 0;
const SAVE_NEW: usize = 1;
const CANCEL: usize = 2;

/// The prompt. Carries the route the rider picked (`pending`) plus the highlighted option.
#[derive(Debug)]
pub struct RouteSwapScreen {
    pending: usize,
    selected: usize,
}

impl RouteSwapScreen {
    pub fn new(pending: usize) -> Self {
        RouteSwapScreen { pending, selected: 0 }
    }

    /// True when the highlighted option needs a hold — the host fills the confirm bar.
    pub fn selection_is_guarded(&self) -> bool {
        ITEMS[self.selected].guard
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                self.selected = super::step_selection(self.selected, n, ITEMS.len());
                Transition::None
            }
            Gesture::Press => match self.selected {
                // Swap only: keep the session (no `start_session`), just re-navigate.
                SWAP => self.swap_route(cx),
                CANCEL => Transition::Pop,
                _ => Transition::None, // Save & new is guarded — press does nothing
            },
            Gesture::Hold if self.selected == SAVE_NEW => {
                // Save the current ride, then begin a fresh session on the picked route. The
                // host drains the Save (finalising the old log) before it opens the new one.
                cx.activity.request_track(TrackAction::Save);
                cx.activity.start_session();
                self.swap_route(cx)
            }
            Gesture::Back => Transition::Pop, // back = Cancel (keep riding the current route)
            _ => Transition::None,
        }
    }

    /// Point navigation at the picked route and drop onto the riding Map.
    fn swap_route(&self, cx: &mut Ctx) -> Transition {
        if cx.routes.is_empty() {
            return Transition::Pop;
        }
        let i = self.pending.min(cx.routes.len() - 1);
        cx.state.enter_riding_view(cx.routes[i].start_lon, cx.routes[i].start_lat);
        cx.activity.mode = Mode::Riding;
        cx.activity.active_route = Some(i);
        Transition::Root(Screen::Map(MapScreen::new()))
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let mut cv = Canvas::new(target, color_fn);

        // Opaque full-screen prompt (not an overlay): header clears the screen, then a one-
        // line explanation and the three options.
        title_frame(&mut cv, w, h, "ROUTE ACTIVE", "");
        cv.text(
            "Recording a ride",
            Point::new(w / 2, super::TITLE_BAR_H + 16),
            Font::Label,
            TextAlign::Center,
            SUBTEXT,
        );

        let (row_h, gap) = (46, 8);
        let first = super::TITLE_BAR_H + 46;
        for (i, item) in ITEMS.iter().enumerate() {
            let y = first + i as i32 * (row_h + gap);
            let row = rect(12, y, w - 24, row_h);
            if i == self.selected {
                if item.guard {
                    // Guarded: a shade base that fills amber with the hold progress (amber,
                    // not warning-red — this confirms a save, it isn't destructive).
                    cv.round(row, 6, PARCHMENT_SHADE);
                    let fill_w = ((w - 24) as f32 * rx.hold_progress.clamp(0.0, 1.0)) as i32;
                    if fill_w > 0 {
                        cv.round(rect(12, y, fill_w, row_h), 6, AMBER);
                    }
                } else {
                    cv.round(row, 6, AMBER);
                }
            }
            cv.text(item.label, Point::new(28, y + 11), Font::Body, TextAlign::Left, INK);
        }
        RenderStats::default()
    }
}
