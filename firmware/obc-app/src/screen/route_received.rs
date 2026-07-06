//! The route-upload popup cards (epic #447, P4) — the two **host-pushed** prompts for a route
//! arriving over BLE while the device is *not* mid-swap:
//!
//! - [`RouteReceivedScreen`] — the **idle** variant: "ROUTE RECEIVED", *Start navigation* /
//!   *Dismiss*. Start navigation is exactly the Route-menu start path ([`super::start_ride`]).
//! - [`RouteUpdatedScreen`] — the **active-route-replaced** variant: an info-only card. By the
//!   time it opens the new version is already adopted (matcher/profile dropped, geometry
//!   reopened by the host) — the card just says so; any press/Back dismisses it.
//!
//! The tracking variant reuses the parameterized [`RouteSwapScreen`](super::RouteSwapScreen).
//! All three share the popup rules [`App::notify_route_uploaded`](crate::App::notify_route_uploaded)
//! enforces: advisory (committed before the prompt), 30 s auto-close = dismiss
//! ([`UPLOAD_POPUP_TIMEOUT_MS`](super::UPLOAD_POPUP_TIMEOUT_MS)), replace-not-stack on consecutive
//! uploads, never landing mid-hold, and the passkey card outranking them. Both screens carry their
//! subject as a **remappable catalog index** — a live rescan re-points it by id, and a vanished
//! route turns actions into a self-dismiss (never "navigate whatever slid into the slot").

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;

use super::{list, palette, title_frame, Ctx, MenuItem, Render, ScreenTick, Transition, UPLOAD_POPUP_TIMEOUT_MS};

/// Whether a popup opened at `opened_ms` has outlived its auto-close window at `now_ms`.
/// Wrap-safe (boot-relative millis wrap after ~49 days). Shared by all three popup variants.
pub(crate) fn popup_expired(opened_ms: u32, now_ms: u32) -> bool {
    now_ms.wrapping_sub(opened_ms) >= UPLOAD_POPUP_TIMEOUT_MS
}

/// The residual timed-wake for a popup opened at `opened_ms`: the millis until its auto-close is
/// due, so the event-driven host arms a timer instead of polling — the timeout fires from warm
/// sleep. Once due, a short retry keeps the host awake for the removal sweep (which can be
/// hold-deferred a tick). Never reports a change itself: the removal in
/// [`App::advance_animations`](crate::App::advance_animations) dirties the repaint.
pub(crate) fn popup_tick(opened_ms: u32, now_ms: u32) -> ScreenTick {
    let elapsed = now_ms.wrapping_sub(opened_ms);
    let next = if elapsed >= UPLOAD_POPUP_TIMEOUT_MS { POPUP_RETRY_MS } else { UPLOAD_POPUP_TIMEOUT_MS - elapsed };
    ScreenTick { changed: false, next_wake_ms: Some(next), region: None }
}

/// Re-poll cadence once a popup is due but not yet removed (a hold deferred the sweep a tick).
const POPUP_RETRY_MS: u32 = 50;

// The start row is the epic's *Start navigation* action; the label is the Route overview's
// established "start ride" verb because the literal phrase doesn't fit the panel (16 glyphs
// × the Body row font's 14 px = 224 px, wider than the 200 px row interior).
const ITEMS: [MenuItem; 2] =
    [MenuItem { label: "Start ride", guard: false }, MenuItem { label: "Dismiss", guard: false }];

const START: usize = 0;

/// The idle "ROUTE RECEIVED" prompt. Carries the received route as a remappable catalog index
/// (`None` once a rescan removed it) plus the highlighted option and its auto-close anchor.
#[derive(Debug)]
pub struct RouteReceivedScreen {
    route: Option<usize>,
    selected: usize,
    /// Map-plane millis when the popup opened — the 30 s auto-close anchor.
    opened_ms: u32,
}

impl RouteReceivedScreen {
    /// A prompt for catalog route `route`, opened at `now_ms` (the auto-close anchor).
    pub fn new(route: usize, now_ms: u32) -> Self {
        RouteReceivedScreen { route: Some(route), selected: 0, opened_ms: now_ms }
    }

    /// Re-point the received route after a live catalog rescan (#450): follow its identity to the
    /// new index, or mark it vanished so *Start navigation* dismisses instead of misfiring.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = self.route.and_then(remap);
    }

    /// Whether the 30 s auto-close deadline has passed — polled by the app's popup sweep.
    pub(crate) fn expired(&self, now_ms: u32) -> bool {
        popup_expired(self.opened_ms, now_ms)
    }

    /// The auto-close deadline's residual wake (see [`popup_tick`]).
    pub(crate) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        popup_tick(self.opened_ms, now_ms)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, ITEMS.len()),
            // Start navigation — exactly the Route-menu start path, validated against the current
            // catalog: a route deleted while the popup was up (`route` remapped to `None`, or the
            // index out of range) dismisses instead of riding a stranger.
            Gesture::Press if self.selected == START => match self.route.filter(|&i| i < cx.routes.len()) {
                Some(i) => super::start_ride(cx, i),
                None => Transition::Pop,
            },
            Gesture::Press => Transition::Pop, // Dismiss
            Gesture::Back => Transition::Pop,  // Back = Dismiss (advisory — the route is in the menu)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, "ROUTE RECEIVED", "");
        match self.route.and_then(|i| rx.routes.get(i)) {
            Some(route) => {
                // Name first (names > metadata), one stats line under it.
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name = super::route_menu::fit_name(&route.name, max);
                cv.text(&name, Point::new(w / 2, super::TITLE_BAR_H + 14), Font::Body, TextAlign::Center, INK);
                let mut stats: heapless::String<24> = heapless::String::new();
                let _ = write!(stats, "{} km, +{} m", route.distance_km, route.climb_m);
                cv.text(&stats, Point::new(w / 2, super::TITLE_BAR_H + 44), Font::Label, TextAlign::Center, SUBTEXT);
            }
            // Deleted from under the popup: say so — the Start row will just dismiss.
            None => {
                cv.text(
                    "Route removed",
                    Point::new(w / 2, super::TITLE_BAR_H + 24),
                    Font::Label,
                    TextAlign::Center,
                    SUBTEXT,
                );
            }
        }

        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: super::TITLE_BAR_H + 78,
            row_h: 46,
            gap: 8,
            label_dx: 16,
            label_dy: 11,
        };
        super::draw_guarded_rows(cv, &ITEMS, self.selected, rx.hold_progress, AMBER, geo);
    }
}

/// The active-route-replaced info card. No options — adoption is not optional and already
/// happened; press/Back (or the auto-close) dismisses.
#[derive(Debug)]
pub struct RouteUpdatedScreen {
    route: Option<usize>,
    /// Map-plane millis when the card opened — the 30 s auto-close anchor.
    opened_ms: u32,
}

impl RouteUpdatedScreen {
    /// A card for catalog route `route` (the still-navigated, freshly-replaced one), opened at
    /// `now_ms`.
    pub fn new(route: usize, now_ms: u32) -> Self {
        RouteUpdatedScreen { route: Some(route), opened_ms: now_ms }
    }

    /// Re-point the subject after a live catalog rescan (#450); display-only here.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = self.route.and_then(remap);
    }

    /// Whether the 30 s auto-close deadline has passed — polled by the app's popup sweep.
    pub(crate) fn expired(&self, now_ms: u32) -> bool {
        popup_expired(self.opened_ms, now_ms)
    }

    /// The auto-close deadline's residual wake (see [`popup_tick`]).
    pub(crate) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        popup_tick(self.opened_ms, now_ms)
    }

    /// Info-only: any press or Back dismisses (nothing here to confirm — the swap already
    /// happened); turns are ignored.
    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, "ROUTE UPDATED", "");
        // The route's name, then the plain two-line statement of what already happened.
        let name_top = h * 35 / 100;
        match self.route.and_then(|i| rx.routes.get(i)) {
            Some(route) => {
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name = super::route_menu::fit_name(&route.name, max);
                cv.text(&name, Point::new(w / 2, name_top), Font::Body, TextAlign::Center, INK);
            }
            None => {
                cv.text("Active route", Point::new(w / 2, name_top), Font::Body, TextAlign::Center, INK);
            }
        }
        let line = Font::Label.line_height() as i32;
        let cap_top = name_top + Font::Body.line_height() as i32 + 14;
        cv.text("Navigation follows", Point::new(w / 2, cap_top), Font::Label, TextAlign::Center, SUBTEXT);
        cv.text("the new version", Point::new(w / 2, cap_top + line), Font::Label, TextAlign::Center, SUBTEXT);
    }
}
