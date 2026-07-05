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
/// loaded when it opened (restored on `back`).
#[derive(Debug, Default)]
pub struct RouteOverviewScreen {
    route: usize,
    prev_active: Option<usize>,
}

impl RouteOverviewScreen {
    /// Preview catalog route `route`; `prev_active` is the `active_route` to restore on cancel.
    pub fn new(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active }
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
            Gesture::Press => super::start_ride(cx, self.route),
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
            title_frame(cv, w, h, "ROUTE", "");
            super::empty_state(cv, w, h, "No route", "Back to the list");
            return;
        };

        let name = super::route_menu::fit_name(&summary.name, ((w - 28) / Font::Body.char_width() as i32) as usize);
        title_frame(cv, w, h, &name, "");

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;

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
                "loading profile",
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
            ("DISTANCE", &dist, dist_unit, None),
            ("CLIMB", &climb, units.elev_label(), Some(true)),
            ("DESCENT", &desc, units.elev_label(), Some(false)),
        ];
        for (i, (caption, value, unit, arrow)) in rows.iter().enumerate() {
            let y = ROWS_TOP + i as i32 * ROW_PITCH;
            ledger_row(cv, w, y, caption, value, unit, *arrow);
            if i + 1 < rows.len() {
                cv.hline(16, y + ROW_PITCH - 4, w - 32, RULE);
            }
        }

        // START RIDE: the page's one action, so it draws armed (amber) with a play wedge.
        let by = h - 10 - BUTTON_H;
        cv.round(rect(chart_x, by, chart_w, BUTTON_H), 8, AMBER);
        let tx = w / 2 + 8;
        cv.text_vcentered("START RIDE", tx, (by, BUTTON_H), Font::Body, TextAlign::Center, INK);
        let px = tx - 5 * Font::Body.char_width() as i32 - 16;
        let mid = by + BUTTON_H / 2;
        cv.triangle(Point::new(px, mid - 7), Point::new(px, mid + 7), Point::new(px + 11, mid), INK);
    }
}
