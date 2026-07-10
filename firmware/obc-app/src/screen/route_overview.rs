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

use crate::activity::Activity;
use crate::input::Gesture;
use crate::route::RouteSummary;
use crate::screen::ScreenTick;
use crate::Msg;

use super::{ledger_row, palette, title_frame, Ctx, Render, Transition, LIST_TOP};

/// Chart band: below the title bar, deep enough to read the terrain, clear of the stat tiles.
const BAND_TOP: i32 = LIST_TOP + 8;
const BAND_BOT: i32 = 140;
const SIDE_MARGIN: i32 = 12;

/// The stat ledger. Making room for the Delete row (T3) turned the three ledger rows into a two-row
/// auto-flip pager (page 0 = DISTANCE + CLIMB, page 1 = DESCENT); [`ROW_PITCH`] is the row spacing
/// within a page. Placed between the band and the Delete row.
const ROWS_TOP: i32 = 150;
const ROW_PITCH: i32 = 42;

/// The guarded **Delete route** row, directly above the START button — the ride_control guarded-row
/// idiom, same base geometry (row height + a bottom gap to the button).
const DELETE_ROW_H: i32 = 38;
const DELETE_GAP: i32 = 8;

/// The START RIDE button bar at the bottom.
const BUTTON_H: i32 = 34;

/// The stat-ledger pager's dwell — a plain fixed constant (not user-configurable): each of the two
/// pages shows this long before the auto-flip (T3). Reuses the Statistics screen-tick machinery.
const PAGE_FLIP_MS: u32 = 5_000;

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
    /// Which stat-ledger page is showing (0 = DISTANCE + CLIMB, 1 = DESCENT); auto-flipped by
    /// [`tick_timers`](Self::tick_timers). Unused on the computed (length-only) page.
    page: usize,
    /// Instant of the last page flip (wrap-safe). `None` until the first tick anchors it, so the
    /// first page gets a full dwell on entry — mirrors the Statistics pager.
    last_flip_ms: Option<u32>,
}

impl RouteOverviewScreen {
    /// Preview catalog route `route`; `prev_active` is the `active_route` to restore on cancel.
    pub fn new(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: false, page: 0, last_flip_ms: None }
    }

    /// Preview a **computed** route (the router's output): length only — no elevation band, no
    /// climb/descent rows. Opened by [`App::notify_nav_result`](crate::App::notify_nav_result).
    pub fn computed(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: true, page: 0, last_flip_ms: None }
    }

    /// Whether this overview previews a **computed** route — the variant that wants the
    /// host-decimated shape preview (#685 §4). Read by
    /// [`App::nav_preview_missing`](crate::App::nav_preview_missing).
    pub(crate) fn is_computed(&self) -> bool {
        self.computed
    }

    /// Whether the guarded **Delete route** row is **live** — a real, non-computed catalog route
    /// that isn't the actively-navigated route of a running tracking session. This is the exact
    /// greying predicate the old Route-menu footer used, moved here (T3): deleting the file under an
    /// open geometry handle mid-ride would break navigation, so the row greys out (a hold does
    /// nothing) exactly while `is_tracking()` and this is the active route. Also the
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) predicate for this screen.
    pub(crate) fn delete_enabled(&self, activity: &Activity, routes: &[RouteSummary]) -> bool {
        !self.computed
            && self.route < routes.len()
            && !(activity.is_tracking() && activity.active_route == Some(self.route))
    }

    /// Stat-ledger pager tick: flip the two pages every [`PAGE_FLIP_MS`], reporting the residual
    /// dwell as the next wake (the Statistics auto-flip machinery). The computed (length-only) page
    /// has a single distance row and no pager, so it never flips.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        if self.computed {
            return ScreenTick::idle();
        }
        let last = *self.last_flip_ms.get_or_insert(now_ms);
        let changed = now_ms.wrapping_sub(last) >= PAGE_FLIP_MS;
        if changed {
            self.page ^= 1; // two pages
            self.last_flip_ms = Some(now_ms);
        }
        let anchor = self.last_flip_ms.unwrap_or(now_ms);
        let next = PAGE_FLIP_MS.saturating_sub(now_ms.wrapping_sub(anchor)).max(1);
        ScreenTick { changed, next_wake_ms: Some(next), region: None }
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
            // Delete: a completed hold over a live Delete row (the guarded hold is the confirmation,
            // no popup) records the delete by index. The host resolves it to the durable object id,
            // deletes the object — a created `_NAV.OBR` route the same way, no special casing — and
            // the store-changed rescan re-feeds the catalog. Restore the pre-preview active route and
            // pop to the refreshed Routes list. A hold while the route is in use (greyed row) never
            // reaches here.
            Gesture::Hold if self.delete_enabled(cx.activity, cx.routes) => {
                cx.activity.request_route_delete(self.route);
                cx.activity.active_route = self.prev_active;
                Transition::Pop
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

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;

        // A computed route (the on-device router's output) has no elevation data at all — the
        // locked "length only" page: a DISTANCE row (meter-resolution from the opened geometry,
        // where the whole-km catalog figure would read "0 km" on a short POI route), no elevation
        // band, no climb/descent — plus the shape preview (#685 §4). The START button is shared.
        if self.computed {
            // Static NEW ROUTE title; the destination name moves into the body as the first line
            // at full card width (#685 §4 — a title-bar name truncated to `Carrefour Mar..`).
            title_frame(cv, w, h, rx.t(Msg::RouteOverviewNewRoute), "");
            let x = 16;
            let name =
                super::route_menu::fit_name(&summary.name, ((w - 2 * x) / Font::Body.char_width() as i32) as usize);
            cv.text(&name, Point::new(x, LIST_TOP + 4), Font::Body, TextAlign::Left, INK);

            let units = rx.settings.units;
            let total_m = rx.route.map(|r| r.total_distance_m).unwrap_or(summary.distance_km * 1000);
            // Metres below 1 km (`600 m`, #685 §4 — `0.6 km` undersells a short POI route), the
            // one-decimal km above; imperial twin: whole feet below a mile, one-decimal miles.
            let mut dist: heapless::String<8> = heapless::String::new();
            let dist_unit = write_computed_distance(&mut dist, total_m, units);
            let rows_top = LIST_TOP + 34;
            ledger_row(cv, w, rows_top, rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None);
            // The bike profile the route was planned under (routing-v2 N5): the rider must be able to
            // tell a Road route from an MTB one they picked by accident. The name resolves against the
            // loaded map for the current selection — which is the profile the just-finished plan used,
            // since planning uses `bike_profile_idx` and the overview opens straight off it.
            draw_profile_label(cv, w, rx, rows_top + ROW_PITCH);
            // The route-shape preview fills the middle between the ledger and the START bar.
            draw_route_preview(cv, w, rows_top + 2 * ROW_PITCH, h - 10 - BUTTON_H, rx.nav_preview);
            draw_start_button(cv, w, h, rx.t(Msg::RouteOverviewStartRide));
            return;
        }

        let name = super::route_menu::fit_name(&summary.name, ((w - 28) / Font::Body.char_width() as i32) as usize);
        title_frame(cv, w, h, &name, "");

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

        // The three figures don't fit alongside the new Delete row + START bar with standard row
        // spacing, so they auto-flip as a two-row pager (T3): page 0 = DISTANCE + CLIMB, page 1 =
        // DESCENT alone. The flip itself is the affordance — no page dots.
        let entries: [(&str, &str, &str, Option<bool>); 3] = [
            (rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None),
            (rx.t(Msg::RouteOverviewClimb), &climb, units.elev_label(), Some(true)),
            (rx.t(Msg::RouteOverviewDescent), &desc, units.elev_label(), Some(false)),
        ];
        let page_rows: &[usize] = if self.page & 1 == 0 { &[0, 1] } else { &[2] };
        for (slot, &e) in page_rows.iter().enumerate() {
            let y = ROWS_TOP + slot as i32 * ROW_PITCH;
            let (caption, value, unit, arrow) = entries[e];
            ledger_row(cv, w, y, caption, value, unit, arrow);
            if slot + 1 < page_rows.len() {
                cv.hline(16, y + ROW_PITCH - 4, w - 32, RULE);
            }
        }

        // The guarded Delete-route row, greyed with an "In use" cue while this is the active ride's
        // route (a hold does nothing there).
        let in_use = rx.activity.is_tracking() && rx.activity.active_route == Some(self.route);
        draw_delete_row(cv, w, h, rx.t(Msg::RouteOverviewDelete), rx.t(Msg::RouteMenuInUse), !in_use, rx.hold_progress);
        draw_start_button(cv, w, h, rx.t(Msg::RouteOverviewStartRide));
    }
}

/// The bottom-anchored y of the Delete-route row, sitting a [`DELETE_GAP`] above the START button.
fn delete_row_y(h: i32) -> i32 {
    (h - 10 - BUTTON_H) - DELETE_GAP - DELETE_ROW_H
}

/// Draw the guarded **Delete route** row above the START button — the ride_control guarded-row
/// idiom (a `PARCHMENT_SHADE` base filling warning-red with the live `hold` under the "Delete route"
/// label). While the route is the active ride's (`enabled == false`) the row greys out with the old
/// footer's exact disabled treatment — a dim trash + the reused "In use" cue (the label + cue don't
/// share a 240 px line, so the cue takes the row, as the footer had it) — the same face the Ride
/// detail's `Recording` state wears (the Q1 sibling). No fill draws; a hold does nothing.
fn draw_delete_row(cv: &mut impl Surface, w: i32, h: i32, label: &str, in_use_cue: &str, enabled: bool, hold: f32) {
    use palette::*;
    let x = 14;
    let y = delete_row_y(h);
    if enabled {
        let row = rect(x, y, w - 2 * x, DELETE_ROW_H);
        super::confirm_row(cv, row, true, true, hold, WARNING, 6);
        cv.text_vcentered(label, x + 12, (y, DELETE_ROW_H), Font::Body, TextAlign::Left, INK);
    } else {
        draw_trash(cv, x + 16, y + DELETE_ROW_H / 2, RULE);
        cv.text_vcentered(in_use_cue, x + 36, (y, DELETE_ROW_H), Font::Label, TextAlign::Left, SUBTEXT);
    }
}

/// Draw a small trash-can glyph centred at `(cx, cy)` — the old Route-menu-footer glyph, carried
/// into the delete row's disabled state so "can't delete now" keeps its established face. The Ride
/// detail's twin — kept local so the sibling screens stay independent.
fn draw_trash(cv: &mut impl Surface, cx: i32, cy: i32, color: u16) {
    let (bw, bh) = (11, 12);
    let (bx, by) = (cx - bw / 2, cy - bh / 2 + 1);
    cv.round_outline(rect(bx, by, bw, bh), 2, color); // can body
    cv.hline(bx - 2, by - 2, bw + 4, color); // lid
    cv.hline(cx - 2, by - 4, 5, color); // handle
    cv.vline(cx - 2, by + 3, bh - 5, 1, color); // ribs
    cv.vline(cx + 2, by + 3, bh - 5, 1, color);
}

/// The "BIKE TYPE" ledger row: the profile name the computed route was planned under (routing-v2
/// N5), drawn at `y` under the DISTANCE row on the computed page in the same caption-left/
/// value-right shape. A stale/out-of-range index shows **profile 0's name** — the profile the
/// router actually fell back to for this plan (see [`NavProfiles::write_label`](crate::NavProfiles)).
fn draw_profile_label(cv: &mut impl Surface, w: i32, rx: &Render, y: i32) {
    let mut name: heapless::String<20> = heapless::String::new();
    rx.nav_profiles.write_label(rx.settings.bike_profile_idx, &mut name);
    ledger_row(cv, w, y, rx.t(Msg::RouteOverviewBikeType), &name, "", None);
}

/// Write the computed page's DISTANCE value into `s`, returning its unit suffix: whole metres
/// below 1 km (#685 §4), one-decimal km above — imperial twin: whole feet below a mile,
/// one-decimal miles (the same thresholds as every other compacting readout).
fn write_computed_distance(s: &mut heapless::String<8>, total_m: u32, units: crate::settings::Units) -> &'static str {
    use crate::settings::{FT_PER_M, FT_PER_MI};
    if units.is_imperial() {
        let ft = (total_m as f32 * FT_PER_M) as u32;
        if ft < FT_PER_MI {
            let _ = write!(s, "{ft}");
            "ft"
        } else {
            let _ = write!(s, "{:.1}", units.dist(total_m as f32 / 1000.0));
            "mi"
        }
    } else if total_m < 1000 {
        let _ = write!(s, "{total_m}");
        "m"
    } else {
        let _ = write!(s, "{:.1}", total_m as f32 / 1000.0);
        "km"
    }
}

/// The route-shape preview's box size (#685 §4): ≈212×90 px, horizontally centred, vertically
/// centred between the ledger rows and the START bar.
const PREVIEW_W: i32 = 212;
const PREVIEW_H: i32 = 90;

/// Draw the computed route's shape preview: the host-decimated polyline (≤ 64 points) normalized
/// and aspect-fit into the [`PREVIEW_W`]×[`PREVIEW_H`] box — lon scaled by cos(mid-lat) so the
/// shape keeps its ground aspect — stroked 2 px INK (the doubled-1-px idiom), with a 4 px filled
/// disc at the start and a 6 px hollow diamond at the destination. An empty/short slice (the
/// frame or two before the host hands the preview in, or a stale one) draws nothing — the box
/// just stays empty, like the full page's "loading profile" band footprint.
fn draw_route_preview(cv: &mut impl Surface, w: i32, top: i32, bot: i32, pts: &[(i32, i32)]) {
    use palette::*;
    if pts.len() < 2 {
        return;
    }
    let x0 = (w - PREVIEW_W) / 2;
    let y0 = top + ((bot - top - PREVIEW_H) / 2).max(0);
    let (mut min_lon, mut max_lon, mut min_lat, mut max_lat) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(lon, lat) in pts {
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
    }
    // Aspect-fit: one scale for both axes (the smaller of the two fits), the fitted shape
    // centred in the box. `max(1.0)` guards a degenerate straight north-south / east-west line.
    let clat = obc_route::cos_lat((min_lat / 2) + (max_lat / 2));
    let geo_w = ((max_lon - min_lon) as f32 * clat).max(1.0);
    let geo_h = ((max_lat - min_lat) as f32).max(1.0);
    let scale = (PREVIEW_W as f32 / geo_w).min(PREVIEW_H as f32 / geo_h);
    let ox = x0 as f32 + (PREVIEW_W as f32 - geo_w * scale) / 2.0;
    let oy = y0 as f32 + (PREVIEW_H as f32 - geo_h * scale) / 2.0;
    let project = |(lon, lat): (i32, i32)| {
        Point::new((ox + (lon - min_lon) as f32 * clat * scale) as i32, (oy + (max_lat - lat) as f32 * scale) as i32)
    };
    let mut prev = project(pts[0]);
    for &p in &pts[1..] {
        let cur = project(p);
        super::stroke2(cv, prev, cur, INK);
        prev = cur;
    }
    // Start: a 4 px filled disc. Destination: a 6 px hollow diamond (its four 1 px edges).
    cv.disc(project(pts[0]), 2, INK);
    let d = project(pts[pts.len() - 1]);
    let k = 3;
    cv.line(Point::new(d.x, d.y - k), Point::new(d.x + k, d.y), INK);
    cv.line(Point::new(d.x + k, d.y), Point::new(d.x, d.y + k), INK);
    cv.line(Point::new(d.x, d.y + k), Point::new(d.x - k, d.y), INK);
    cv.line(Point::new(d.x - k, d.y), Point::new(d.x, d.y - k), INK);
}

/// START RIDE: the page's one action, so it draws armed (amber) with a play wedge. Shared by the
/// full page and the computed-route variant — and by the POI detail's `Route here` footer (#685),
/// which is specified as exactly this bar, so the two can't drift.
pub(super) fn draw_start_button(cv: &mut impl Surface, w: i32, h: i32, label: &str) {
    use palette::*;
    let by = h - 10 - BUTTON_H;
    cv.round(rect(SIDE_MARGIN, by, w - 2 * SIDE_MARGIN, BUTTON_H), 8, AMBER);
    let tx = w / 2 + 8;
    cv.text_vcentered(label, tx, (by, BUTTON_H), Font::Body, TextAlign::Center, INK);
    // Play wedge just left of the centred label — from its real half-width, so a longer
    // translation (or the POI detail's `Route here`) can't run into it.
    let px = tx - label.chars().count() as i32 * Font::Body.char_width() as i32 / 2 - 16;
    let mid = by + BUTTON_H / 2;
    cv.triangle(Point::new(px, mid - 7), Point::new(px, mid + 7), Point::new(px + 11, mid), INK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::route::RouteSummary;
    use crate::screen::PoiScratch;
    use crate::settings::Settings;
    use crate::AppState;
    use obc_route::BBox;

    fn summary() -> RouteSummary {
        RouteSummary {
            name: heapless::String::try_from("A").unwrap(),
            distance_km: 10,
            climb_m: 100,
            bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            start_lon: 0,
            start_lat: 0,
        }
    }

    fn run(scr: &mut RouteOverviewScreen, act: &mut Activity, routes: &[RouteSummary], g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
            routes,
            rides: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// A completed hold over the live Delete row records the route's index, restores the pre-preview
    /// active route, and pops back to the Routes list.
    #[test]
    fn hold_deletes_the_previewed_route_and_pops() {
        let routes = [summary(), summary()];
        let mut act = Activity::new(Mode::Idle);
        act.active_route = Some(1); // the menu preview
        let mut scr = RouteOverviewScreen::new(1, Some(0)); // was previewing route 0 before
        assert!(scr.delete_enabled(&act, &routes), "an Idle preview is deletable");
        let t = run(&mut scr, &mut act, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "the delete pops back to the Routes list");
        assert_eq!(act.take_route_delete(), Some(1), "records the previewed route's index");
        assert_eq!(act.active_route, Some(0), "the pre-preview route is restored");
    }

    /// The Delete row is disabled — and a hold does nothing — while this route is the active route of
    /// a running tracking session (the greying predicate moved off the old Route-menu footer).
    #[test]
    fn hold_over_the_active_ride_route_is_a_no_op() {
        let routes = [summary(), summary()];
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // now tracking…
        act.active_route = Some(0); // …route 0
        let mut scr = RouteOverviewScreen::new(0, None);
        assert!(!scr.delete_enabled(&act, &routes), "the active ride's route can't be deleted");
        let t = run(&mut scr, &mut act, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold over the in-use route does nothing");
        assert_eq!(act.take_route_delete(), None);
    }

    /// A computed (length-only) overview has no Delete row, so it's never deletable and a hold is a
    /// no-op — the locked length-only page stays exactly as-is.
    #[test]
    fn computed_overview_has_no_delete() {
        let routes = [summary()];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RouteOverviewScreen::computed(0, None);
        assert!(!scr.delete_enabled(&act, &routes));
        run(&mut scr, &mut act, &routes, Gesture::Hold);
        assert_eq!(act.take_route_delete(), None);
    }

    /// The two-row pager flips exactly at the dwell deadline and only once, re-arming a fresh dwell —
    /// the Statistics auto-flip contract. The first poll anchors the dwell (page 0 gets a full one).
    #[test]
    fn pager_flips_once_at_the_deadline() {
        let mut scr = RouteOverviewScreen::new(0, None);
        assert_eq!(scr.page, 0);
        assert!(!scr.tick_timers(0).changed, "the first poll only anchors the dwell");
        assert!(!scr.tick_timers(PAGE_FLIP_MS - 1).changed, "still dwelling just before the deadline");
        assert_eq!(scr.page, 0);
        assert!(scr.tick_timers(PAGE_FLIP_MS).changed, "flips exactly at the deadline");
        assert_eq!(scr.page, 1, "now on the DESCENT page");
        assert!(!scr.tick_timers(PAGE_FLIP_MS + 1).changed, "and only once — a fresh dwell re-armed");
        assert!(scr.tick_timers(2 * PAGE_FLIP_MS).changed, "flips back at the next deadline");
        assert_eq!(scr.page, 0);
    }

    /// The computed page has a single distance row and no pager, so its tick never self-dirties.
    #[test]
    fn computed_overview_never_flips() {
        let mut scr = RouteOverviewScreen::computed(0, None);
        assert!(!scr.tick_timers(PAGE_FLIP_MS).changed);
        assert_eq!(scr.tick_timers(PAGE_FLIP_MS), ScreenTick::idle());
    }
}
