//! The POI **create-route** flow's two small screens (epic #116, R4):
//!
//! - [`NavConfirmScreen`] — the "Create a route?" confirm, reached by pressing a POI's
//!   [detail](super::PoiDetailScreen). *Create route* records a one-shot
//!   [`NavRequest`](crate::activity::NavRequest) (rider fix → POI coord + the POI's name) that the
//!   host drains via [`App::take_nav_request`](crate::App::take_nav_request), runs the on-device
//!   A* router on, and answers through [`App::notify_nav_result`](crate::App::notify_nav_result) —
//!   which swaps this screen for the computed-route
//!   [overview](super::RouteOverviewScreen) (success) or the [`NavFailScreen`] (failure).
//!   The confirm stays up while the host plans (the answer lands within the same host pass).
//! - [`NavFailScreen`] — the locked **two-tier failure** card: `Exhausted` → "Too far to route
//!   here" (there is no distance cap; the router's fixed table running out **is** the device's
//!   range limit), everything else → "Couldn't find a route." Info-only, like the
//!   [`RouteUpdated`](super::route_received::RouteUpdatedScreen) card: any press/Back dismisses,
//!   returning to the POI detail underneath.
//!
//! The request needs the rider's position; the POI browser already required a fix to snapshot, so
//! [`AppState::user_fix`](crate::AppState::user_fix) is essentially always present here. If it
//! genuinely isn't, the confirm degrades straight to the "Couldn't find a route." tier.

use embedded_graphics::prelude::Point;
use obc_reader::POI_NAME_MAX;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::NavRequest;
use crate::input::Gesture;

use super::{list, palette, title_frame, Ctx, MenuItem, Render, Screen, Transition};

const ITEMS: [MenuItem; 2] =
    [MenuItem { label: "Create route", guard: false }, MenuItem { label: "Cancel", guard: false }];

const CREATE: usize = 0;

/// The "Create a route?" confirm. Carries the target POI's coordinate and its display/route name
/// (the stored name, or the subtype fallback label — the list row's convention), plus the
/// highlighted option.
#[derive(Debug)]
pub struct NavConfirmScreen {
    /// The POI coordinate, `(lon, lat)` µdeg — the route's goal.
    to: (i32, i32),
    /// The route's name-to-be (what the emitted OBCR is titled and the catalog lists).
    name: heapless::String<POI_NAME_MAX>,
    selected: usize,
}

impl NavConfirmScreen {
    /// The confirm for a route to `to`, named `name` (truncated to the POI name cap).
    pub fn new(to: (i32, i32), name: &str) -> Self {
        let mut nm = heapless::String::new();
        for ch in name.chars() {
            if nm.push(ch).is_err() {
                break;
            }
        }
        NavConfirmScreen { to, name: nm, selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, ITEMS.len()),
            Gesture::Press if self.selected == CREATE => {
                // The request's start is the rider's fix. No position at all (never a fix this
                // session) can't be routed — degrade to the generic failure tier rather than
                // sending the host a garbage start.
                let Some(fix) = cx.state.user_fix else {
                    return Transition::Replace(Screen::NavFail(NavFailScreen::not_found()));
                };
                cx.activity.request_nav(NavRequest::new((fix.lon, fix.lat), self.to, &self.name));
                // Stay put: the host drains the request and answers within its pass, replacing
                // this screen with the overview or the failure card.
                Transition::None
            }
            Gesture::Press => Transition::Pop, // Cancel
            Gesture::Back => Transition::Pop,  // Back = Cancel (return to the POI detail)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, "CREATE ROUTE", "");
        // The destination's name — what the rider is routing to.
        let max = (((w - 24) / Font::Label.char_width() as i32).max(6)) as usize;
        let name = super::route_menu::fit_name(&self.name, max);
        cv.text(&name, Point::new(w / 2, super::TITLE_BAR_H + 16), Font::Label, TextAlign::Center, SUBTEXT);

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

/// The routing-failure card — the locked two-tier copy, info-only (any press/Back dismisses back
/// to the POI detail).
#[derive(Debug)]
pub struct NavFailScreen {
    /// `true` = the range tier ("Too far to route here"): the router's fixed table exhausted
    /// before the goal — with no distance cap, that **is** the device's range limit.
    /// `false` = every other failure ("Couldn't find a route.").
    too_far: bool,
}

impl NavFailScreen {
    /// The range tier: the search exhausted the fixed table — the target is beyond what the
    /// device can plan.
    pub fn too_far() -> Self {
        NavFailScreen { too_far: true }
    }

    /// The generic tier: no snap, no path, a host-aborted search, or any host I/O failure.
    pub fn not_found() -> Self {
        NavFailScreen { too_far: false }
    }

    /// Which tier the card shows (`true` = "Too far to route here") — lets the seam tests pin
    /// the error→tier mapping without reading pixels.
    pub fn shows_too_far(&self) -> bool {
        self.too_far
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, "CREATE ROUTE", "");
        // The two-tier message, wrapped onto two Body lines (either single line overruns the
        // 240 px panel).
        let (l1, l2) = if self.too_far { ("Too far to", "route here") } else { ("Couldn't find", "a route.") };
        let y = h * 35 / 100;
        cv.text(l1, Point::new(w / 2, y), Font::Body, TextAlign::Center, INK);
        cv.text(l2, Point::new(w / 2, y + Font::Body.line_height() as i32 + 6), Font::Body, TextAlign::Center, INK);
    }
}
