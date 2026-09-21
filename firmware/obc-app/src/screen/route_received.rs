//! The popup cards for a route or a trip that arrives over BLE: the idle "ROUTE RECEIVED" prompt,
//! the "TRIP RECEIVED" twin, and the info-only card for a replaced active route. All are advisory —
//! the object is committed before the prompt — and all auto-close after
//! [`UPLOAD_POPUP_TIMEOUT_MS`](super::UPLOAD_POPUP_TIMEOUT_MS). An object that a rescan removed
//! turns its action row into a self-dismiss, so a card never opens a stranger.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::vocab::band::TopStroke;
use super::vocab::card::{ActionRows, CardEvent};
use super::vocab::chrome::{card_check, title_frame, TITLE_BAR_H};
use super::vocab::rows::{GuardedRowsGeometry, MenuItem};
use super::{
    palette, Ctx, Render, RouteMenuScreen, RouteOverviewScreen, Screen, ScreenTick, Transition, UPLOAD_POPUP_TIMEOUT_MS,
};

/// Whether a popup opened at `opened_ms` has outlived its auto-close window. Wrap-safe.
pub(crate) fn popup_expired(opened_ms: u32, now_ms: u32) -> bool {
    now_ms.wrapping_sub(opened_ms) >= UPLOAD_POPUP_TIMEOUT_MS
}

/// The millis until a popup's auto-close is due, so the host arms a timer instead of polling and
/// the timeout fires from warm sleep. Once due, a short retry keeps the host awake for the removal
/// sweep, which a hold can defer a tick.
pub(crate) fn popup_tick(opened_ms: u32, now_ms: u32) -> ScreenTick {
    let elapsed = now_ms.wrapping_sub(opened_ms);
    let next = if elapsed >= UPLOAD_POPUP_TIMEOUT_MS { POPUP_RETRY_MS } else { UPLOAD_POPUP_TIMEOUT_MS - elapsed };
    ScreenTick { changed: false, next_wake_ms: Some(next), region: None }
}

/// Re-poll cadence once a popup is due but not yet removed (a hold deferred the sweep a tick).
const POPUP_RETRY_MS: u32 = 50;

/// The one-line route stats every card in the family shows under the name (`2 km, +76 m`).
pub(crate) fn route_stats(route: &crate::route::RouteSummary) -> heapless::String<24> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{} km, +{} m", route.distance_km, route.climb_m);
    s
}

/// The two option rows (View route / Dismiss); neither is guarded. The primary row opens the Route
/// overview, from where START RIDE is one press away.
const ACTION_GUARDS: [bool; 2] = [false; 2];

const VIEW: usize = 0;

/// The footprint of the mini elevation band, centred below the stats line. [`SPARK_TOP`] is its
/// top offset from the bottom of the title bar.
const SPARK_W: i32 = 180;
const SPARK_H: i32 = 52;
const SPARK_TOP: i32 = 62;

/// The idle "ROUTE RECEIVED" prompt. The route is a remappable catalog index, `None` once a rescan
/// removed it.
#[derive(Debug)]
pub struct RouteReceivedScreen {
    route: Option<usize>,
    actions: ActionRows,
    /// Map-plane millis when the popup opened: the auto-close anchor.
    opened_ms: u32,
    /// The route's min-max-normalized elevation band, built host-side. `None` for a route with no
    /// elevation; the card then omits the band, rather than draw a flat line.
    elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
}

impl RouteReceivedScreen {
    /// A prompt for catalog route `route`, opened at `now_ms`.
    pub fn new(route: usize, now_ms: u32, elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>) -> Self {
        RouteReceivedScreen { route: Some(route), actions: ActionRows::new(0), opened_ms: now_ms, elevation }
    }

    /// Re-point the received route after a live catalog rescan, or mark it vanished.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = self.route.and_then(remap);
    }

    pub(crate) fn expired(&self, now_ms: u32) -> bool {
        popup_expired(self.opened_ms, now_ms)
    }

    pub(crate) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        popup_tick(self.opened_ms, now_ms)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self.actions.handle(g, &ACTION_GUARDS) {
            // A route deleted while the popup was up dismisses, instead of opening a stranger. The
            // advisory popup gives way to the overview, so Back returns to what the card covered.
            CardEvent::Activate(VIEW) => match self.route.filter(|&i| i < cx.routes.len()) {
                Some(i) => {
                    let prev = cx.navigator.replace_active_route(i);
                    Transition::Replace(Screen::RouteOverview(RouteOverviewScreen::new(i, prev)))
                }
                None => Transition::Pop,
            },
            CardEvent::Activate(_) | CardEvent::Dismiss => Transition::Pop,
            CardEvent::None => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::RouteReceivedTitle), "");
        let mut drew_spark = false;
        match self.route.and_then(|i| rx.routes.get(i)) {
            Some(route) => {
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name_row = rect(12, TITLE_BAR_H + 14, w - 24, Font::Body.line_height() as i32);
                let name = rx.marquee.fit(&route.name, max, Some(name_row));
                cv.text(&name, Point::new(w / 2, TITLE_BAR_H + 14), Font::Body, TextAlign::Center, INK);
                let stats = route_stats(route);
                cv.text(&stats, Point::new(w / 2, TITLE_BAR_H + 44), Font::Label, TextAlign::Center, SUBTEXT);
                if let Some(elev) = &self.elevation {
                    let band_x = (w - SPARK_W) / 2;
                    draw_sparkline(cv, band_x, TITLE_BAR_H + SPARK_TOP, SPARK_W, SPARK_H, elev);
                    drew_spark = true;
                }
            }
            None => {
                cv.text(
                    rx.t(Msg::RouteReceivedRouteRemoved),
                    Point::new(w / 2, TITLE_BAR_H + 24),
                    Font::Label,
                    TextAlign::Center,
                    SUBTEXT,
                );
            }
        }

        // Without the band the options move up into its slot.
        let rows_top = TITLE_BAR_H + if drew_spark { SPARK_TOP + SPARK_H + 10 } else { 78 };
        let geo = GuardedRowsGeometry::card(w, rows_top);
        let items = [
            MenuItem { label: rx.t(Msg::RouteReceivedViewRoute), guard: ACTION_GUARDS[0] },
            MenuItem { label: rx.t(Msg::RouteReceivedDismiss), guard: ACTION_GUARDS[1] },
        ];
        self.actions.draw(cv, &items, rx.hold_progress, AMBER, geo);
    }
}

/// Draw the mini elevation sparkline: an olive fill under an amber top stroke, no labels or axis.
/// Each pixel column interpolates between buckets, so the coarse band draws as a smooth line. The
/// card has no `Profile` for [`ElevationBand`](super::vocab::band::ElevationBand), so it draws its
/// own columns, but the top line is the shared [`TopStroke`].
fn draw_sparkline(cv: &mut impl Surface, x0: i32, y_top: i32, w_band: i32, h_band: i32, elev: &[u8]) {
    use palette::*;
    let last = elev.len().saturating_sub(1);
    let y_bot = y_top + h_band;
    let span_px = (w_band - 1).max(1) as f32;
    let mut stroke = TopStroke::default();
    for px in 0..w_band {
        let fb = (px as f32 / span_px) * last as f32;
        let i = fb as usize;
        let frac = fb - i as f32;
        let a = elev[i] as f32;
        let b = elev[(i + 1).min(last)] as f32;
        let v = a + (b - a) * frac; // 0..=255
        let top_y = y_bot - (v / 255.0 * h_band as f32) as i32;
        let x = x0 + px;
        cv.vline(x, top_y, y_bot - top_y + 1, 1, PARCHMENT_SHADE);
        stroke.column(cv, x, top_y, AMBER);
    }
}

/// The "TRIP RECEIVED" prompt, the trip twin of [`RouteReceivedScreen`]. A trip always arrives
/// after its member routes, so the replace-not-stack rule leaves the rider this one card instead of
/// a per-route parade. It carries the trip's durable id, because the trip catalog re-resolves in
/// place on a rescan.
#[derive(Debug)]
pub struct TripReceivedScreen {
    trip_id: crate::CatalogObjectId,
    actions: ActionRows,
    /// Map-plane millis when the popup opened: the auto-close anchor.
    opened_ms: u32,
}

impl TripReceivedScreen {
    /// A prompt for the trip with durable id `trip_id`, opened at `now_ms`.
    pub fn new(trip_id: crate::CatalogObjectId, now_ms: u32) -> Self {
        TripReceivedScreen { trip_id, actions: ActionRows::new(0), opened_ms: now_ms }
    }

    pub(crate) fn expired(&self, now_ms: u32) -> bool {
        popup_expired(self.opened_ms, now_ms)
    }

    pub(crate) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        popup_tick(self.opened_ms, now_ms)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self.actions.handle(g, &ACTION_GUARDS) {
            // A trip deleted while the popup was up dismisses, instead of opening an empty
            // stranger. The card gives way to the folder, so Back returns to what it covered.
            CardEvent::Activate(VIEW) => {
                if cx.trips.iter().any(|t| t.id == self.trip_id) {
                    Transition::Replace(Screen::RouteMenu(RouteMenuScreen::trip(self.trip_id)))
                } else {
                    Transition::Pop
                }
            }
            CardEvent::Activate(_) | CardEvent::Dismiss => Transition::Pop,
            CardEvent::None => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::TripReceivedTitle), "");
        match rx.trips.iter().find(|t| t.id == self.trip_id) {
            Some(trip) => {
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name_row = rect(12, TITLE_BAR_H + 14, w - 24, Font::Body.line_height() as i32);
                let name = rx.marquee.fit(&trip.name, max, Some(name_row));
                cv.text(&name, Point::new(w / 2, TITLE_BAR_H + 14), Font::Body, TextAlign::Center, INK);
                let mut stats: heapless::String<24> = heapless::String::new();
                let _ = write!(stats, "{} km, +{} m", trip.distance_km, trip.climb_m);
                cv.text(&stats, Point::new(w / 2, TITLE_BAR_H + 44), Font::Label, TextAlign::Center, SUBTEXT);
                let n = trip.stage_indices.len();
                let word = if n == 1 { rx.t(Msg::TripReceivedRouteOne) } else { rx.t(Msg::TripReceivedRoutes) };
                let mut count: heapless::String<24> = heapless::String::new();
                let _ = write!(count, "{n} {word}");
                cv.text(&count, Point::new(w / 2, TITLE_BAR_H + 68), Font::Label, TextAlign::Center, SUBTEXT);
            }
            None => {
                cv.text(
                    rx.t(Msg::TripReceivedTripRemoved),
                    Point::new(w / 2, TITLE_BAR_H + 24),
                    Font::Label,
                    TextAlign::Center,
                    SUBTEXT,
                );
            }
        }

        let geo = GuardedRowsGeometry::card(w, TITLE_BAR_H + 96);
        let items = [
            MenuItem { label: rx.t(Msg::TripReceivedViewTrip), guard: ACTION_GUARDS[0] },
            MenuItem { label: rx.t(Msg::TripReceivedDismiss), guard: ACTION_GUARDS[1] },
        ];
        self.actions.draw(cv, &items, rx.hold_progress, AMBER, geo);
    }
}

/// The active-route-replaced info card. It has no options: the new version is already adopted.
#[derive(Debug)]
pub struct RouteUpdatedScreen {
    route: Option<usize>,
    /// Map-plane millis when the card opened: the auto-close anchor.
    opened_ms: u32,
}

impl RouteUpdatedScreen {
    /// A card for the still-navigated catalog route `route`, opened at `now_ms`.
    pub fn new(route: usize, now_ms: u32) -> Self {
        RouteUpdatedScreen { route: Some(route), opened_ms: now_ms }
    }

    /// Re-point the subject after a live catalog rescan; display-only here.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = self.route.and_then(remap);
    }

    pub(crate) fn expired(&self, now_ms: u32) -> bool {
        popup_expired(self.opened_ms, now_ms)
    }

    pub(crate) fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        popup_tick(self.opened_ms, now_ms)
    }

    /// Info-only: any press or Back dismisses the card, and steps are ignored.
    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::RouteReceivedUpdatedTitle), "");
        card_check(cv, Point::new(w / 2, TITLE_BAR_H + 40), 24);
        let name_top = h * 35 / 100;
        match self.route.and_then(|i| rx.routes.get(i)) {
            Some(route) => {
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name_row = rect(12, name_top, w - 24, Font::Body.line_height() as i32);
                let name = rx.marquee.fit(&route.name, max, Some(name_row));
                cv.text(&name, Point::new(w / 2, name_top), Font::Body, TextAlign::Center, INK);
            }
            None => {
                cv.text(
                    rx.t(Msg::RouteReceivedActiveRoute),
                    Point::new(w / 2, name_top),
                    Font::Body,
                    TextAlign::Center,
                    INK,
                );
            }
        }
        let line = Font::Label.line_height() as i32;
        let cap_top = name_top + Font::Body.line_height() as i32 + 14;
        cv.text(
            rx.t(Msg::RouteReceivedNavFollows),
            Point::new(w / 2, cap_top),
            Font::Label,
            TextAlign::Center,
            SUBTEXT,
        );
        cv.text(
            rx.t(Msg::RouteReceivedNewVersion),
            Point::new(w / 2, cap_top + line),
            Font::Label,
            TextAlign::Center,
            SUBTEXT,
        );
    }
}
