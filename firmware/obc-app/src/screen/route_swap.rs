//! The "route already active" prompt, shown when a new route is picked mid-ride. Loading a route
//! while tracking is ambiguous: keep recording and re-navigate, or save and begin fresh. Swap
//! route keeps the session and changes only the navigated route, Finish & new saves the current
//! track and starts a new session, and Cancel returns.
//!
//! The same screen serves the route that arrives over BLE mid-ride: the `received` constructor
//! retitles it and arms the auto-close of the popups. The two roles have identical semantics, so
//! the screen is parameterized instead of forked.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::Mode;
use crate::input::Gesture;
use crate::Msg;

use super::route_received::{popup_expired, popup_tick};
use super::vocab::card::{ActionRows, CardEvent};
use super::vocab::chrome::{title_frame, TITLE_BAR_H};
use super::vocab::rows::{GuardedRowsGeometry, MenuItem};
use super::{palette, Ctx, MapScreen, Render, Screen, ScreenTick, Transition};

/// Per-row guard flags. Only Finish & new is destructive.
const GUARDS: [bool; 3] = [false, true, false];

const SWAP: usize = 0;
const FINISH_NEW: usize = 1;
const CANCEL: usize = 2;

/// The prompt. `pending` is `None` when a catalog rescan removed the picked route from below the
/// prompt, and both actions then cancel instead of navigating the route that took its index.
#[derive(Debug)]
pub struct RouteSwapScreen {
    pending: Option<usize>,
    actions: ActionRows,
    /// `Some(opened_ms)` for the host-pushed prompt, which auto-closes. `None` for the manual
    /// prompt, which waits for the rider.
    received_ms: Option<u32>,
}

impl RouteSwapScreen {
    /// The manual prompt: the rider picked `pending` from the Route menu mid-ride.
    pub fn new(pending: usize) -> Self {
        RouteSwapScreen { pending: Some(pending), actions: ActionRows::new(0), received_ms: None }
    }

    /// The host-pushed prompt for a route that arrived over BLE mid-ride, opened at `now_ms`.
    /// Only the framing and the timeout differ from the manual prompt.
    pub fn received(pending: usize, now_ms: u32) -> Self {
        RouteSwapScreen { pending: Some(pending), actions: ActionRows::new(0), received_ms: Some(now_ms) }
    }

    /// True for the host-pushed popup, which the app's popup rules treat differently.
    pub(crate) fn is_received(&self) -> bool {
        self.received_ms.is_some()
    }

    /// Re-point the picked route after a catalog rescan: follow its identity to the new index, or
    /// mark it vanished, so a later fire cannot swap onto the wrong route.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.pending = self.pending.and_then(remap);
    }

    /// Always `false` for the manual prompt, which waits for the rider.
    pub(crate) fn expired(&self, now_ms: u32) -> bool {
        self.received_ms.is_some_and(|t| popup_expired(t, now_ms))
    }

    pub(crate) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        match self.received_ms {
            Some(t) => popup_tick(t, now_ms),
            None => ScreenTick::idle(),
        }
    }

    /// True when the highlighted row fills for a hold, which makes the app repaint that fill.
    pub fn selection_is_guarded(&self) -> bool {
        self.actions.selection_is_guarded(&GUARDS)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self.actions.handle(g, &GUARDS) {
            // Swap keeps the session: nothing is named to Recorder.
            CardEvent::Activate(SWAP) => self.swap_route(cx),
            CardEvent::Activate(FINISH_NEW) => {
                // The picked route vanished, so do not finalise the ride for a swap that cannot
                // happen any more.
                if self.pending.is_none() {
                    return Transition::Pop;
                }
                // Recorder opens the new session only after the store answered for the old one.
                cx.recorder.save_and_restart();
                self.swap_route(cx)
            }
            CardEvent::Activate(CANCEL) | CardEvent::Dismiss => Transition::Pop,
            CardEvent::Activate(_) | CardEvent::None => Transition::None,
        }
    }

    /// Point navigation at the picked route and drop onto the riding Map. A pick that vanished or
    /// is out of range cancels, so the screen never navigates the route that took its index.
    fn swap_route(&self, cx: &mut Ctx) -> Transition {
        let Some(i) = self.pending.filter(|&i| i < cx.routes.len()) else {
            return Transition::Pop;
        };
        cx.state.enter_riding_view(cx.routes[i].start_lon, cx.routes[i].start_lat);
        cx.activity.mode = Mode::Riding;
        cx.navigator.load_route(i);
        Transition::Root(Screen::Map(MapScreen::new()))
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // The received prompt puts the name of the arriving route in the subtitle, because the
        // rider did not pick it. The manual prompt explains the state instead.
        let title =
            if self.is_received() { rx.t(Msg::RouteSwapReceivedTitle) } else { rx.t(Msg::RouteSwapActiveTitle) };
        title_frame(cv, w, h, title, "");
        let mut sub: heapless::String<64> = heapless::String::new();
        if self.is_received() {
            match self.pending.and_then(|i| rx.routes.get(i)) {
                Some(route) => {
                    let name_row = rect(12, TITLE_BAR_H + 16, w - 24, Font::Label.line_height() as i32);
                    sub = rx.marquee.fit(&route.name, w - 24, Font::Label, Some(name_row));
                }
                None => {
                    let _ = sub.push_str(rx.t(Msg::RouteSwapRouteRemoved));
                }
            }
        } else {
            let _ = sub.push_str(rx.t(Msg::RouteSwapRecording));
        }
        cv.text(&sub, Point::new(w / 2, TITLE_BAR_H + 16), Font::Label, TextAlign::Center, SUBTEXT);

        if let Some(route) = self.pending.and_then(|i| rx.routes.get(i)) {
            let stats = super::route_received::route_stats(route);
            cv.text(&stats, Point::new(w / 2, TITLE_BAR_H + 38), Font::Label, TextAlign::Center, SUBTEXT);
        }

        // The guarded row fills amber, not warning red: it confirms a save, not a deletion.
        let geo = GuardedRowsGeometry::card(w, TITLE_BAR_H + 64);
        let items = [
            MenuItem { label: rx.t(Msg::RouteSwapSwap), guard: GUARDS[0] },
            MenuItem { label: rx.t(Msg::RouteSwapFinishNew), guard: GUARDS[1] },
            MenuItem { label: rx.t(Msg::RouteSwapCancel), guard: GUARDS[2] },
        ];
        self.actions.draw(cv, &items, rx.hold_progress, AMBER, geo);
    }
}
