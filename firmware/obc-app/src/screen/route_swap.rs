//! The "route already active" prompt — shown when a new route is picked mid-ride. Loading a route
//! while tracking is ambiguous: keep recording and re-navigate, or save and begin fresh. **Swap
//! route** (press) keeps the session and only changes the navigated route; **Save & new** (hold-
//! guarded) finalises the current track to a `.gpx` and starts a new session; **Cancel** (back)
//! returns. Reached from [`RouteMenuScreen`](super::RouteMenuScreen) when a session is active and a
//! *different* route is chosen.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::{Mode, TrackAction};
use crate::input::Gesture;

use super::{list, palette, title_frame, Ctx, MapScreen, MenuItem, Render, Screen, Transition};

const ITEMS: [MenuItem; 3] = [
    MenuItem { label: "Swap route", guard: false },
    MenuItem { label: "Save & new", guard: true },
    MenuItem { label: "Cancel", guard: false },
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

    /// True when the highlighted option needs a hold: its row fills with the live hold progress in
    /// `draw`, so [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) reports a charging
    /// hold as worth repainting here.
    pub fn selection_is_guarded(&self) -> bool {
        ITEMS[self.selected].guard
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, ITEMS.len()),
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

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // Opaque full-screen prompt (not an overlay): a one-line explanation and three options.
        title_frame(cv, w, h, "ROUTE ACTIVE", "");
        cv.text(
            "Recording a ride",
            Point::new(w / 2, super::TITLE_BAR_H + 16),
            Font::Label,
            TextAlign::Center,
            SUBTEXT,
        );

        // Guarded rows fill amber (not warning-red — this confirms a save, it isn't destructive).
        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: super::TITLE_BAR_H + 46,
            row_h: 46,
            gap: 8,
            label_dx: 16,
            label_dy: 11,
        };
        super::draw_guarded_rows(cv, &ITEMS, self.selected, rx.hold_progress, AMBER, geo);
    }
}
