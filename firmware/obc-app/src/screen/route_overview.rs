//! The Route overview — the look-before-you-ride page between picking a route and tracking it.
//! Shows the route's name, its full elevation profile (the Statistics band, **non-interactive**:
//! no cursor, no zoom, no live shading), the headline stats (distance, climb, descent), and a
//! START RIDE button. *Press* starts the session and drops into the riding Map — exactly what
//! picking a route used to do directly; *back* cancels and returns to the Route menu.
//!
//! Entering the overview sets [`Activity::active_route`](crate::Activity::active_route) — the
//! hosts key geometry loading on it, so the route streams open and the profile builds while the
//! rider is still looking at the page — but starts **no** session; the previous `active_route`
//! is remembered and restored on `back`, so browsing routes never clobbers a loaded one. The
//! descent figure comes from the opened route (`--` for the frame or two before it streams in).

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::{ledger_row, palette, title_frame, Ctx, Render, Transition, LIST_TOP};

/// Chart band: below the title bar, deep enough to read the terrain, clear of the stat tiles.
const BAND_TOP: i32 = LIST_TOP + 8;
const BAND_BOT: i32 = 140;
const SIDE_MARGIN: i32 = 12;

/// The stat ledger: three caption/value rows between the band and the START button.
const ROWS_TOP: i32 = 150;
const ROW_PITCH: i32 = 42;

/// The START RIDE button bar at the bottom.
const BUTTON_H: i32 = 34;

/// The Route overview. State is which catalog route it previews, plus the `active_route` that was
/// loaded when it opened (restored on `back`), and whether the route is a **computed** one (the
/// on-device router's output, epic #116 R4) — which has no elevation data, so the page shows
/// length only.
#[derive(Debug, Default)]
pub struct RouteOverviewScreen {
    route: usize,
    prev_active: Option<usize>,
    /// The previewed route came from the on-device router (`/routes/_nav.obcr`): OSM highways
    /// carry no elevation and there is no DEM, so its per-point elevation is all zero — the page
    /// omits the elevation band and the climb/descent rows rather than showing a flat band and
    /// "+0 m" (the locked "length only" overview).
    computed: bool,
}

impl RouteOverviewScreen {
    /// Preview catalog route `route`; `prev_active` is the `active_route` to restore on cancel.
    pub fn new(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: false }
    }

    /// Preview a **computed** route (the router's output): length only — no elevation band, no
    /// climb/descent rows. Opened by [`App::notify_nav_result`](crate::App::notify_nav_result).
    pub fn computed(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: true }
    }

    /// Re-point both held indices after a live catalog rescan (#450). A vanished preview subject
    /// becomes an out-of-range index — exactly the missing-summary path `draw`/`handle` already
    /// have ("No route" + `press` pops); a vanished `prev_active` restores to `None` on cancel.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = remap(self.route).unwrap_or(usize::MAX);
        self.prev_active = self.prev_active.and_then(remap);
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Start: the session begin that Route-menu `press` used to do — riding camera on the
            // route's start, tracking on, and a clean [Home, Map] stack. The shared
            // [`start_ride`](super::start_ride) path, also the upload popup's *Start navigation*.
            //
            // Mid-ride (a computed-route overview can open while tracking — the POI flow, epic
            // #116 R4), accepting is ambiguous the same way picking a route from the menu is, so
            // it opens the **same** save/swap prompt instead of silently restarting the session;
            // the Route menu's tracking arm never reaches an overview, so this arm fires only on
            // that flow today.
            Gesture::Press => {
                if cx.activity.is_tracking() {
                    return Transition::Push(super::Screen::RouteSwap(super::RouteSwapScreen::new(self.route)));
                }
                super::start_ride(cx, self.route)
            }
            // Cancel: put back whatever route was loaded before the preview.
            Gesture::Back => {
                cx.activity.active_route = self.prev_active;
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let Some(summary) = rx.routes.get(self.route) else {
            title_frame(cv, w, h, rx.t(Msg::RouteOverviewTitle), "");
            super::empty_state(cv, w, h, rx.t(Msg::RouteOverviewNoRoute), rx.t(Msg::RouteOverviewNoRouteSub));
            return;
        };

        let name = super::route_menu::fit_name(&summary.name, ((w - 28) / Font::Body.char_width() as i32) as usize);
        title_frame(cv, w, h, &name, "");

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;

        // A computed route (the on-device router's output) has no elevation data at all — the
        // locked "length only" page: one DISTANCE row (meter-resolution from the opened geometry,
        // where the whole-km catalog figure would read "0 km" on a short POI route), no elevation
        // band, no climb/descent. The START button below is shared.
        if self.computed {
            let units = rx.settings.units;
            let total_m = rx.route.map(|r| r.total_distance_m).unwrap_or(summary.distance_km * 1000);
            let mut dist: heapless::String<8> = heapless::String::new();
            let _ = write!(dist, "{:.1}", units.dist(total_m as f32 / 1000.0));
            let dist_unit = if units.is_imperial() { "mi" } else { "km" };
            ledger_row(cv, w, LIST_TOP + 16, rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None);
            // The bike profile the route was planned under (routing-v2 N5): the rider must be able to
            // tell a Road route from an MTB one they picked by accident. The name resolves against the
            // loaded map for the current selection — which is the profile the just-finished plan used,
            // since planning uses `bike_profile_idx` and the overview opens straight off it.
            draw_profile_label(cv, w, rx);
            draw_start_button(cv, w, h, rx.t(Msg::RouteOverviewStartRide));
            return;
        }

        // The full-route elevation band — the Statistics silhouette without any of its live
        // layers (no traveled shading, no cursor, no progress bar). A small peak label gives the
        // vertical scale meaning.
        if let Some(profile) = rx.profile {
            let win = profile.window(0.5, 1.0, chart_w.max(1) as u32);
            let span = (win.hi_frac - win.lo_frac).max(1e-6);
            let span_ele = (profile.max_ele_m - profile.min_ele_m).max(1) as f32;
            let ele_to_y = |e: i16| -> i32 {
                let t = ((e - profile.min_ele_m) as f32 / span_ele).clamp(0.0, 1.0);
                BAND_BOT - (t * (BAND_BOT - BAND_TOP) as f32) as i32
            };
            let mut prev_top: Option<i32> = None;
            for px in 0..chart_w {
                let f = win.lo_frac + span * (px as f32 / chart_w as f32);
                let top_y = ele_to_y(profile.sample(win.level, f).1);
                let x = chart_x + px;
                cv.vline(x, top_y, BAND_BOT - top_y + 1, 1, PARCHMENT_SHADE);
                // Amber top line, connected to the previous column so steep sections stay solid.
                let (y0, y1) = prev_top.map_or((top_y, top_y), |p| (p.min(top_y), p.max(top_y)));
                cv.vline(x, y0 - 1, (y1 - y0) + 2, 1, AMBER);
                prev_top = Some(top_y);
            }
            // Peak elevation label, kept inside the band edges.
            let units = rx.settings.units;
            let mut peak: heapless::String<10> = heapless::String::new();
            let _ = write!(peak, "{} {}", units.elev(profile.peak_ele_m() as f32) as i32, units.elev_label());
            let peak_x =
                (chart_x + (profile.peak_frac() * chart_w as f32) as i32).clamp(chart_x + 30, chart_x + chart_w - 30);
            let peak_y = (ele_to_y(profile.peak_ele_m()) - 22).max(BAND_TOP - 2);
            cv.text(&peak, Point::new(peak_x, peak_y), Font::Label, TextAlign::Center, SUBTEXT);
        } else {
            // Route still streaming open: keep the band's footprint so the page doesn't jump.
            cv.text(
                rx.t(Msg::RouteOverviewLoadingProfile),
                Point::new(w / 2, (BAND_TOP + BAND_BOT) / 2 - 9),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
        }
        cv.hline(chart_x, BAND_BOT + 1, chart_w, RULE); // baseline under the band

        // Headline stats as a ledger — olive caption left, big ink value right with a small unit
        // suffix, hairline rules between rows. Organized without the riding grid's panes, which
        // read as "live data" and swallow space this page doesn't need. Distance and climb come
        // from the catalog summary (always present); descent needs the opened route.
        let units = rx.settings.units;
        let mut dist: heapless::String<8> = heapless::String::new();
        let _ = write!(dist, "{}", (units.dist(summary.distance_km as f32) + 0.5) as u32);
        let dist_unit = if units.is_imperial() { "mi" } else { "km" };

        let mut climb: heapless::String<8> = heapless::String::new();
        let _ = write!(climb, "{}", (units.elev(summary.climb_m as f32) + 0.5) as u32);

        let mut desc: heapless::String<8> = heapless::String::new();
        match rx.route {
            Some(r) => {
                let _ = write!(desc, "{}", (units.elev(r.total_descent_m as f32) + 0.5) as u32);
            }
            None => {
                let _ = desc.push_str("--");
            }
        }

        let rows: [(&str, &str, &str, Option<bool>); 3] = [
            (rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None),
            (rx.t(Msg::RouteOverviewClimb), &climb, units.elev_label(), Some(true)),
            (rx.t(Msg::RouteOverviewDescent), &desc, units.elev_label(), Some(false)),
        ];
        for (i, (caption, value, unit, arrow)) in rows.iter().enumerate() {
            let y = ROWS_TOP + i as i32 * ROW_PITCH;
            ledger_row(cv, w, y, caption, value, unit, *arrow);
            if i + 1 < rows.len() {
                cv.hline(16, y + ROW_PITCH - 4, w - 32, RULE);
            }
        }

        draw_start_button(cv, w, h, rx.t(Msg::RouteOverviewStartRide));
    }
}

/// The "BIKE TYPE" ledger row: the profile name the computed route was planned under (routing-v2
/// N5), drawn under the DISTANCE row on the length-only page in the same caption-left/value-right
/// shape. A stale/out-of-range index shows **profile 0's name** — the profile the router actually
/// fell back to for this plan (see [`NavProfiles::write_label`](crate::NavProfiles)).
fn draw_profile_label(cv: &mut impl Surface, w: i32, rx: &Render) {
    let mut name: heapless::String<20> = heapless::String::new();
    rx.nav_profiles.write_label(rx.settings.bike_profile_idx, &mut name);
    ledger_row(cv, w, LIST_TOP + 16 + ROW_PITCH, rx.t(Msg::RouteOverviewBikeType), &name, "", None);
}

/// START RIDE: the page's one action, so it draws armed (amber) with a play wedge. Shared by the
/// full page and the computed-route (length-only) variant.
fn draw_start_button(cv: &mut impl Surface, w: i32, h: i32, label: &str) {
    use palette::*;
    let by = h - 10 - BUTTON_H;
    cv.round(rect(SIDE_MARGIN, by, w - 2 * SIDE_MARGIN, BUTTON_H), 8, AMBER);
    let tx = w / 2 + 8;
    cv.text_vcentered(label, tx, (by, BUTTON_H), Font::Body, TextAlign::Center, INK);
    let px = tx - 5 * Font::Body.char_width() as i32 - 16;
    let mid = by + BUTTON_H / 2;
    cv.triangle(Point::new(px, mid - 7), Point::new(px, mid + 7), Point::new(px + 11, mid), INK);
}
