//! The "route already active" prompt — shown when a new route is picked mid-ride. Loading a route
//! while tracking is ambiguous: keep recording and re-navigate, or save and begin fresh. **Swap
//! route** (press) keeps the session and only changes the navigated route; **Save & new** (hold-
//! guarded) finalises the current track (the host's Save) and starts a new session; **Cancel** (back)
//! returns. Reached from [`RouteMenuScreen`](super::RouteMenuScreen) when a session is active and a
//! *different* route is chosen — or **host-pushed** by
//! [`App::notify_route_uploaded`](crate::App::notify_route_uploaded) when a route arrives over BLE
//! mid-ride (epic #447, P4): the [`received`](RouteSwapScreen::received) constructor retitles the
//! same screen ("ROUTE RECEIVED", named subtitle) and arms the popups' 30 s auto-close (timeout =
//! Cancel — advisory, the route is in the menu either way). Parameterized, not forked: the
//! keep-session vs. save-and-restart semantics are identical in both roles.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::{Mode, TrackAction};
use crate::input::Gesture;
use crate::Msg;

use super::route_received::{popup_expired, popup_tick};
use super::{list, palette, title_frame, Ctx, MapScreen, MenuItem, Render, Screen, ScreenTick, Transition};

/// Per-row guard flags (only *Save & new* is destructive). The labels are looked up per language at
/// draw time (see [`RouteSwapScreen::draw`]) — the old `const ITEMS` couldn't stay const.
const GUARDS: [bool; 3] = [false, true, false];

const SWAP: usize = 0;
const SAVE_NEW: usize = 1;
const CANCEL: usize = 2;

/// The prompt. Carries the route the rider picked (`pending`) plus the highlighted option.
/// `pending` is `None` once a live catalog rescan (#450) removed the picked route from under the
/// prompt — Swap / Save & new then cancel out instead of navigating whatever slid into its index.
#[derive(Debug)]
pub struct RouteSwapScreen {
    pending: Option<usize>,
    selected: usize,
    /// `Some(opened_ms)` when this prompt was **host-pushed** for a route received over BLE
    /// (epic #447, P4): retitled and auto-closing after
    /// [`UPLOAD_POPUP_TIMEOUT_MS`](super::UPLOAD_POPUP_TIMEOUT_MS). `None` for the manual,
    /// menu-opened prompt, which never times out.
    received_ms: Option<u32>,
}

impl RouteSwapScreen {
    /// The manual prompt — the rider picked `pending` from the Route menu mid-ride.
    pub fn new(pending: usize) -> Self {
        RouteSwapScreen { pending: Some(pending), selected: 0, received_ms: None }
    }

    /// The host-pushed variant for a route **received over BLE** mid-ride (P4), opened at
    /// `now_ms` (the auto-close anchor). Same options, same semantics — only the framing and the
    /// timeout differ.
    pub fn received(pending: usize, now_ms: u32) -> Self {
        RouteSwapScreen { pending: Some(pending), selected: 0, received_ms: Some(now_ms) }
    }

    /// Whether this is the host-pushed received-route popup (vs. the manual menu prompt) — the
    /// distinction the app's popup rules key on (auto-close, passkey replacement).
    pub(crate) fn is_received(&self) -> bool {
        self.received_ms.is_some()
    }

    /// Re-point the picked route after a live catalog rescan (#450): follow its identity to the
    /// new index, or mark it vanished (`None`) so a later fire can't swap onto the wrong route.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.pending = self.pending.and_then(remap);
    }

    /// Whether the received-popup auto-close deadline has passed. Always `false` for the manual
    /// prompt — it waits for the rider.
    pub(crate) fn expired(&self, now_ms: u32) -> bool {
        self.received_ms.is_some_and(|t| popup_expired(t, now_ms))
    }

    /// The received-popup's residual auto-close wake; idle for the manual prompt.
    pub(crate) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        match self.received_ms {
            Some(t) => popup_tick(t, now_ms),
            None => ScreenTick::idle(),
        }
    }

    /// True when the highlighted option needs a hold: its row fills with the live hold progress in
    /// `draw`, so [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) reports a charging
    /// hold as worth repainting here.
    pub fn selection_is_guarded(&self) -> bool {
        GUARDS[self.selected.min(GUARDS.len() - 1)]
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, GUARDS.len()),
            Gesture::Press => match self.selected {
                // Swap only: keep the session (no `start_session`), just re-navigate.
                SWAP => self.swap_route(cx),
                CANCEL => Transition::Pop,
                _ => Transition::None, // Save & new is guarded — press does nothing
            },
            Gesture::Hold if self.selected == SAVE_NEW => {
                // The picked route vanished under the prompt (rescan): cancel — don't finalise
                // the ride for a swap that can no longer happen.
                if self.pending.is_none() {
                    return Transition::Pop;
                }
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

    /// Point navigation at the picked route and drop onto the riding Map. A vanished/out-of-range
    /// pick (a rescan removed it) cancels instead — indexing by position here is exactly the
    /// "silently navigate a shifted route" bug the identity remap exists to prevent.
    fn swap_route(&self, cx: &mut Ctx) -> Transition {
        let Some(i) = self.pending.filter(|&i| i < cx.routes.len()) else {
            return Transition::Pop;
        };
        cx.state.enter_riding_view(cx.routes[i].start_lon, cx.routes[i].start_lat);
        cx.activity.mode = Mode::Riding;
        cx.activity.active_route = Some(i);
        Transition::Root(Screen::Map(MapScreen::new()))
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        // Opaque full-screen prompt (not an overlay): a one-line explanation and three options.
        // The received variant renames the frame and puts the *arriving route's name* in the
        // subtitle slot (the rider didn't pick it, so the screen must say what landed); the
        // manual prompt explains the state instead — the rider just picked the route themselves.
        let title =
            if self.is_received() { rx.t(Msg::RouteSwapReceivedTitle) } else { rx.t(Msg::RouteSwapActiveTitle) };
        title_frame(cv, w, h, title, "");
        let mut sub: heapless::String<64> = heapless::String::new();
        if self.is_received() {
            match self.pending.and_then(|i| rx.routes.get(i)) {
                Some(route) => {
                    let max = (((w - 24) / Font::Label.char_width() as i32).max(6)) as usize;
                    sub = super::route_menu::fit_name(&route.name, max);
                }
                None => {
                    let _ = sub.push_str(rx.t(Msg::RouteSwapRouteRemoved));
                }
            }
        } else {
            let _ = sub.push_str(rx.t(Msg::RouteSwapRecording));
        }
        cv.text(&sub, Point::new(w / 2, super::TITLE_BAR_H + 16), Font::Label, TextAlign::Center, SUBTEXT);

        // The picked / received route's stats line, directly under the subtitle — the same helper
        // the idle received card uses, so the whole card family reads identically (#682). No
        // sparkline here: three option rows + subtitle already fill the card (locked, idle-only).
        if let Some(route) = self.pending.and_then(|i| rx.routes.get(i)) {
            let stats = super::route_received::route_stats(route);
            cv.text(&stats, Point::new(w / 2, super::TITLE_BAR_H + 38), Font::Label, TextAlign::Center, SUBTEXT);
        }

        // Guarded rows fill amber (not warning-red — this confirms a save, it isn't destructive).
        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: super::TITLE_BAR_H + 64,
            row_h: 46,
            gap: 8,
            label_dx: 16,
            label_dy: 11,
        };
        let items = [
            MenuItem { label: rx.t(Msg::RouteSwapSwap), guard: GUARDS[0] },
            MenuItem { label: rx.t(Msg::RouteSwapSaveNew), guard: GUARDS[1] },
            MenuItem { label: rx.t(Msg::RouteSwapCancel), guard: GUARDS[2] },
        ];
        super::draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, AMBER, geo);
    }
}
