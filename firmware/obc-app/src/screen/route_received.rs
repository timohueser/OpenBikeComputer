//! The route-upload popup cards (epic #447, P4) — the two **host-pushed** prompts for a route
//! arriving over BLE while the device is *not* mid-swap:
//!
//! - [`RouteReceivedScreen`] — the **idle** variant: "ROUTE RECEIVED", a route name + stats line,
//!   an optional mini elevation sparkline, and *View route* / *Dismiss*. View route opens the same
//!   Route overview pressing the route in the Routes list opens (#682) — where START RIDE is then
//!   one press away — so this card no longer starts a ride directly.
//! - [`RouteUpdatedScreen`] — the **active-route-replaced** variant: an info-only card. By the
//!   time it opens the new version is already adopted (matcher/profile dropped, geometry
//!   reopened by the host) — the card just says so; any press/Back dismisses it.
//!
//! The tracking variant reuses the parameterized [`RouteSwapScreen`](super::RouteSwapScreen).
//! All three share the popup rules [`App::apply_event`](crate::App::apply_event)
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
use crate::Msg;

use super::vocab::chrome::{card_check, title_frame, TITLE_BAR_H};
use super::vocab::list;
use super::vocab::rows::{draw_guarded_rows, GuardedRowsGeometry, MenuItem};
use super::{
    palette, Ctx, Render, RouteMenuScreen, RouteOverviewScreen, Screen, ScreenTick, Transition, UPLOAD_POPUP_TIMEOUT_MS,
};

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

/// The one-line route stats the received / swap cards share under the name — whole-unit distance +
/// climb straight off the catalog summary (`2 km, +76 m`), the format the idle card established.
/// Factored so every card in the family (idle received, mid-ride swap, active) reads identically.
pub(crate) fn route_stats(route: &crate::route::RouteSummary) -> heapless::String<24> {
    let mut s = heapless::String::new();
    let _ = write!(s, "{} km, +{} m", route.distance_km, route.climb_m);
    s
}

/// The two option rows (View route / Dismiss), neither guarded — the labels are looked up per
/// language at draw time (see [`RouteReceivedScreen::draw`]). The primary row opens the **Route
/// overview** (the same page pressing the route in the Routes list opens, where START RIDE is then
/// one press away): the card no longer starts a ride directly (#682, locked Q2).
const N_ITEMS: usize = 2;

const VIEW: usize = 0;

/// The mini elevation band's footprint (#682): ≈180 px wide, centred, sitting a little below the
/// stats line. Grown 32 → 52 px tall in owner review round 2 ("the one from the route overview is
/// bigger though, and it looks better") — the card's spare bottom air absorbs it, the option rows
/// keep their spacing below. [`SPARK_TOP`] is its top offset from the title bar's bottom.
const SPARK_W: i32 = 180;
const SPARK_H: i32 = 52;
const SPARK_TOP: i32 = 62;

/// The idle "ROUTE RECEIVED" prompt. Carries the received route as a remappable catalog index
/// (`None` once a rescan removed it) plus the highlighted option, its auto-close anchor, and the
/// route's mini elevation sparkline (`None` when the route has no elevation).
#[derive(Debug)]
pub struct RouteReceivedScreen {
    route: Option<usize>,
    selected: usize,
    /// Map-plane millis when the popup opened — the 30 s auto-close anchor.
    opened_ms: u32,
    /// The route's min–max-normalized elevation band ([`obc_route::elevation_sparkline`], 64
    /// `u8` buckets), built host-side from the committed OBCR (#682). `None` for a route with no
    /// elevation — the card then omits the band and lets the options move up (never a fake flat
    /// line).
    elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>,
}

impl RouteReceivedScreen {
    /// A prompt for catalog route `route`, opened at `now_ms` (the auto-close anchor), carrying the
    /// route's `elevation` sparkline (`None` when it has none).
    pub fn new(route: usize, now_ms: u32, elevation: Option<[u8; obc_route::SPARKLINE_BUCKETS]>) -> Self {
        RouteReceivedScreen { route: Some(route), selected: 0, opened_ms: now_ms, elevation }
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
            Gesture::Step(n) => list::on_step(&mut self.selected, n, N_ITEMS),
            // View route — open the Route overview exactly as pressing the route in the Routes list
            // does (same screen, same `active_route` data path, so the host streams it open behind
            // the page), validated against the current catalog: a route deleted while the popup was
            // up (`route` remapped to `None`, or the index out of range) dismisses instead of
            // opening a stranger. The advisory popup gives way to the overview (`Replace`), so
            // backing out returns to whatever the card covered, not the card.
            Gesture::Press if self.selected == VIEW => match self.route.filter(|&i| i < cx.routes.len()) {
                Some(i) => {
                    let prev = cx.activity.active_route.replace(i);
                    Transition::Replace(Screen::RouteOverview(RouteOverviewScreen::new(i, prev)))
                }
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

        title_frame(cv, w, h, rx.t(Msg::RouteReceivedTitle), "");
        // Whether the mini elevation band draws — only with a live route *and* an elevation array.
        // Without it the options move up into the band's slot (never a fake flat line).
        let mut drew_spark = false;
        match self.route.and_then(|i| rx.routes.get(i)) {
            Some(route) => {
                // Name first (names > metadata), one stats line under it.
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name = super::route_menu::fit_name(&route.name, max);
                cv.text(&name, Point::new(w / 2, TITLE_BAR_H + 14), Font::Body, TextAlign::Center, INK);
                let stats = route_stats(route);
                cv.text(&stats, Point::new(w / 2, TITLE_BAR_H + 44), Font::Label, TextAlign::Center, SUBTEXT);
                // The mini elevation sparkline, centred between the stats line and the options —
                // the Route-overview band's language (olive fill under a 2 px amber top stroke), no
                // labels, no axis.
                if let Some(elev) = &self.elevation {
                    let band_x = (w - SPARK_W) / 2;
                    draw_sparkline(cv, band_x, TITLE_BAR_H + SPARK_TOP, SPARK_W, SPARK_H, elev);
                    drew_spark = true;
                }
            }
            // Deleted from under the popup: say so — the View row will just dismiss.
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

        // With the band drawn the options sit below it; without it they move up into its slot.
        let rows_top = TITLE_BAR_H + if drew_spark { SPARK_TOP + SPARK_H + 10 } else { 78 };
        let geo = GuardedRowsGeometry::card(w, rows_top);
        let items = [
            MenuItem { label: rx.t(Msg::RouteReceivedViewRoute), guard: false },
            MenuItem { label: rx.t(Msg::RouteReceivedDismiss), guard: false },
        ];
        draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, AMBER, geo);
    }
}

/// Draw the mini elevation sparkline — the Route-overview band shrunk to the received card: an
/// olive [`PARCHMENT_SHADE`](palette::PARCHMENT_SHADE) fill under a 2 px
/// [`AMBER`](palette::AMBER) top stroke, no labels or axis. `elev` is the host-built
/// min–max-normalized band ([`obc_route::elevation_sparkline`], `0..=255` per bucket); each pixel
/// column reads a linearly-interpolated height so the coarse 64-bucket band draws as a smooth line.
/// The amber top connects to the previous column so steep steps stay solid (as the overview does).
fn draw_sparkline(cv: &mut impl Surface, x0: i32, y_top: i32, w_band: i32, h_band: i32, elev: &[u8]) {
    use palette::*;
    let last = elev.len().saturating_sub(1);
    let y_bot = y_top + h_band;
    let span_px = (w_band - 1).max(1) as f32;
    let mut prev_top: Option<i32> = None;
    for px in 0..w_band {
        // Fractional bucket for this column, linearly interpolated between the two nearest buckets.
        let fb = (px as f32 / span_px) * last as f32;
        let i = fb as usize;
        let frac = fb - i as f32;
        let a = elev[i] as f32;
        let b = elev[(i + 1).min(last)] as f32;
        let v = a + (b - a) * frac; // 0..=255
        let top_y = y_bot - (v / 255.0 * h_band as f32) as i32;
        let x = x0 + px;
        cv.vline(x, top_y, y_bot - top_y + 1, 1, PARCHMENT_SHADE);
        // Amber top, spanning this column's step from the previous so steep sections stay solid;
        // on a flat run it's the 2 px cap the overview draws.
        let (y0, y1) = prev_top.map_or((top_y, top_y), |p| (p.min(top_y), p.max(top_y)));
        cv.vline(x, y0 - 1, (y1 - y0) + 2, 1, AMBER);
        prev_top = Some(top_y);
    }
}

/// The "TRIP RECEIVED" prompt — the trip twin of [`RouteReceivedScreen`]. A committed trip object
/// always arrives **after** its member routes (it references their ids, so every client sends the
/// routes first), and the popup family's replace-not-stack rule means this card lands *over* — i.e.
/// replaces — the last per-route popup of the burst: the rider is left with one "TRIP RECEIVED"
/// card, not a parade. Same rules as the family: advisory (committed before the prompt), 30 s
/// auto-close = dismiss, passkey outranks, never lands mid-hold.
///
/// Carries the trip's **durable id**, not a catalog index: the trip catalog re-resolves in place on
/// a rescan (no index remap exists for trips, and none is needed) — a vanished trip turns *View
/// trip* into a self-dismiss and the card body into the removed notice.
#[derive(Debug)]
pub struct TripReceivedScreen {
    trip_id: crate::CatalogObjectId,
    selected: usize,
    /// Map-plane millis when the popup opened — the 30 s auto-close anchor.
    opened_ms: u32,
}

impl TripReceivedScreen {
    /// A prompt for the trip with durable id `trip_id`, opened at `now_ms` (the auto-close anchor).
    pub fn new(trip_id: crate::CatalogObjectId, now_ms: u32) -> Self {
        TripReceivedScreen { trip_id, selected: 0, opened_ms: now_ms }
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
            Gesture::Step(n) => list::on_step(&mut self.selected, n, N_ITEMS),
            // View trip — open the trip's folder exactly as pressing its row in the Route menu
            // does (the same durable-id-scoped stage list), validated against the live trip
            // catalog: a trip deleted while the popup was up dismisses instead of opening an empty
            // stranger. The advisory popup gives way to the folder (`Replace`), so backing out
            // returns to whatever the card covered, not the card.
            Gesture::Press if self.selected == VIEW => {
                if cx.trips.iter().any(|t| t.id == self.trip_id) {
                    Transition::Replace(Screen::RouteMenu(RouteMenuScreen::trip(self.trip_id)))
                } else {
                    Transition::Pop
                }
            }
            Gesture::Press => Transition::Pop, // Dismiss
            Gesture::Back => Transition::Pop,  // Back = Dismiss (advisory — the trip is in the menu)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::TripReceivedTitle), "");
        match rx.trips.iter().find(|t| t.id == self.trip_id) {
            Some(trip) => {
                // Name first (names > metadata), the summed stats line under it — the route card's
                // exact anatomy — then the member count, the "all N landed" confirmation the
                // per-route parade never gave.
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name = super::route_menu::fit_name(&trip.name, max);
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
            // Deleted from under the popup: say so — the View row will just dismiss.
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

        // The option rows sit under the three text lines — the route card's no-sparkline geometry,
        // shifted down by the extra count line.
        let geo = GuardedRowsGeometry::card(w, TITLE_BAR_H + 96);
        let items = [
            MenuItem { label: rx.t(Msg::TripReceivedViewTrip), guard: false },
            MenuItem { label: rx.t(Msg::TripReceivedDismiss), guard: false },
        ];
        draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, AMBER, geo);
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
    /// happened); steps are ignored.
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
        // The shared check in the glyph slot (dialog anatomy, #678 T1): the update already
        // succeeded — this card is the confirmation, so it carries the success mark.
        card_check(cv, Point::new(w / 2, TITLE_BAR_H + 40), 24);
        // The route's name, then the plain two-line statement of what already happened.
        let name_top = h * 35 / 100;
        match self.route.and_then(|i| rx.routes.get(i)) {
            Some(route) => {
                let max = (((w - 24) / Font::Body.char_width() as i32).max(6)) as usize;
                let name = super::route_menu::fit_name(&route.name, max);
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
