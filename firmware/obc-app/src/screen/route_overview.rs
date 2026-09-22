//! The Route overview: the look-before-you-ride page between picking a route and tracking it.
//!
//! It shows the route's name and a content-paired pager, where the media band and the stat rows
//! flip together: page A is the track-shape preview over DISTANCE and EST TIME, page B the full
//! elevation profile over CLIMB and DESCENT. The band is not interactive: no cursor, no zoom, no
//! live shading. Below the pager is a START RIDE row and, when the route is deletable, the guarded
//! Delete-route row under it. Entry selects START RIDE; up and down toggle the two rows; press
//! starts the session only from the START row; hold charges the delete only from the Delete row;
//! back cancels and returns to the Route menu.
//!
//! Entering the overview sets Navigator's active route, because hosts key geometry loading on it,
//! so the route streams open and the profile builds while the rider is still looking at the page.
//! It starts no session, and the previous `active_route` is restored on `back`, so browsing routes
//! never clobbers a loaded one.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use super::vocab::band::{ElevationBand, PeakLabel};
use super::vocab::chrome::{empty_state, stroke2, title_frame, LIST_TOP, TITLE_BAR_H};
use super::vocab::fmt::{duration_hms, write_distance_split};
use super::vocab::pager::ContentPager;
use super::vocab::rows::{draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use crate::input::Gesture;
use crate::navigator::RouteState;
use crate::route::RouteSummary;
use crate::screen::ScreenTick;
use crate::Msg;

use super::{palette, Ctx, Render, Transition};

/// Chart band: below the title bar, deep enough to read the terrain, clear of the stat tiles.
const BAND_TOP: i32 = LIST_TOP + 8;
const BAND_BOT: i32 = 140;
const SIDE_MARGIN: i32 = 12;

/// The stat ledger under the media band: page A carries DISTANCE and EST TIME, page B CLIMB and
/// DESCENT. [`ROW_PITCH`] is the row spacing within a page.
const ROWS_TOP: i32 = 146;
const ROW_PITCH: i32 = 42;

/// The two action rows, in the Pause-menu row family's geometry: START RIDE over the guarded
/// Delete-route row, because the destructive row ranks under the primary action. START keeps its
/// two-row position even with Delete hidden, so nothing jumps when the row re-arms.
const OPTION_ROW_H: i32 = 38;
const OPTION_GAP: i32 = 8;

/// The START RIDE button bar of the computed, length-only page: the screen-bottom anchor shared
/// with the POI detail's `Route here` footer.
const BUTTON_H: i32 = 34;

/// The two action rows the cursor walks: START RIDE and, when the route is deletable, the guarded
/// Delete-route row under it.
const START: usize = 0;
const DELETE: usize = 1;

/// The Route overview. State is which catalog route it previews, the `active_route` that was
/// loaded when it opened, and whether the route is a computed one, which has no elevation data and
/// therefore shows length only.
#[derive(Debug, Default)]
pub struct RouteOverviewScreen {
    route: usize,
    prev_active: Option<usize>,
    /// The previewed route came from the on-device router, whose points carry no elevation, so the
    /// page omits the elevation band and the climb and descent rows rather than showing a flat band
    /// and "+0 m".
    computed: bool,
    /// The content-paired pager. Unused on the computed page, which never flips.
    pager: ContentPager,
    /// The action-row cursor. Entry selects START, and with the Delete row hidden the cursor pins
    /// there.
    selected: usize,
}

impl RouteOverviewScreen {
    /// Preview catalog route `route`; `prev_active` is the `active_route` to restore on cancel.
    pub fn new(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: false, pager: ContentPager::default(), selected: START }
    }

    /// Preview a computed route: length only, with no elevation band and no climb or descent
    /// rows.
    pub fn computed(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: true, pager: ContentPager::default(), selected: START }
    }

    /// Whether the guarded Delete-route row exists: a real, non-computed catalog route that is not
    /// the actively-navigated route of a running tracking session. Deleting the file under an open
    /// geometry handle mid-ride would break navigation. The row is hidden entirely while
    /// disallowed, and this guard keeps a hold a no-op regardless.
    pub(crate) fn delete_enabled(&self, navigation: &RouteState, recording: bool, routes: &[RouteSummary]) -> bool {
        !self.computed && self.route < routes.len() && !(recording && navigation.active_route == Some(self.route))
    }

    /// True while a hold would charge the Delete row: it exists and the cursor is on it. The
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) predicate for this screen.
    pub(crate) fn selection_is_guarded(
        &self,
        navigation: &RouteState,
        recording: bool,
        routes: &[RouteSummary],
    ) -> bool {
        self.selected == DELETE && self.delete_enabled(navigation, recording, routes)
    }

    /// Flip the two pages on the shared dwell. The computed page has a single fixed layout and no
    /// pager, so it never flips.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        if self.computed {
            return ScreenTick::idle();
        }
        self.pager.tick(now_ms)
    }

    /// Re-point both held indices after a live catalog rescan. A vanished preview subject becomes
    /// an out-of-range index, which is the missing-summary path `draw` and `handle` already have; a
    /// vanished `prev_active` restores to `None` on cancel.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = remap(self.route).unwrap_or(usize::MAX);
        self.prev_active = self.prev_active.and_then(remap);
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // A step toggles START and Delete. With the Delete row hidden there is one row and the
            // step is a no-op; the cursor also clamps back to START first, in case the row vanished
            // under it.
            Gesture::Step(n) => {
                let len = if self.delete_enabled(cx.navigator.route_state(), cx.recorder.recording(), cx.routes) {
                    2
                } else {
                    1
                };
                self.selected = self.selected.min(len - 1);
                self.selected = super::vocab::list::step_selection(self.selected, n, len);
                Transition::None
            }
            // Start, only from the selected START row: a press on the Delete row does nothing,
            // because that row is hold-guarded. Mid-ride, accepting is ambiguous the same way
            // picking a route from the menu is, so it opens the same save-or-swap prompt instead of
            // silently restarting the session.
            Gesture::Press if self.selected == START => {
                if cx.recorder.recording() {
                    return Transition::Push(super::Screen::RouteSwap(super::RouteSwapScreen::new(self.route)));
                }
                super::start_ride(cx, self.route)
            }
            // The guarded hold is the confirmation, so there is no popup. It records the delete by
            // index; the host resolves it to the durable object id, deletes the object, and the
            // store-changed rescan re-feeds the catalog.
            Gesture::Hold
                if self.selection_is_guarded(cx.navigator.route_state(), cx.recorder.recording(), cx.routes) =>
            {
                cx.activity.request_route_delete(self.route);
                cx.navigator.set_active_route(self.prev_active);
                Transition::Pop
            }
            // Cancel: put back whatever route was loaded before the preview.
            Gesture::Back => {
                cx.navigator.set_active_route(self.prev_active);
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
            empty_state(cv, w, h, rx.t(Msg::RouteOverviewNoRoute), rx.t(Msg::RouteOverviewNoRouteSub));
            return;
        };

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;

        // A computed route has no elevation data, so the page is length only. DISTANCE reads at
        // metre resolution from the opened geometry, because the whole-kilometre catalog figure
        // would read "0 km" on a short route.
        if self.computed {
            // The title is static, and the destination name moves into the body at full card
            // width, where it is not truncated.
            title_frame(cv, w, h, rx.t(Msg::RouteOverviewNewRoute), "");
            let x = 16;
            let name_row = rect(x, LIST_TOP + 4, w - 2 * x, Font::Body.line_height() as i32);
            let name = rx.marquee.fit(&summary.name, w - 2 * x, Font::Body, Some(name_row));
            cv.text(&name, Point::new(x, LIST_TOP + 4), Font::Body, TextAlign::Left, INK);

            let units = rx.settings.units;
            let total_m = rx.route.map(|r| r.total_distance_m).unwrap_or(summary.distance_km * 1000);
            // Metres below a kilometre, because "0.6 km" undersells a short route, and one-decimal
            // kilometres above. Imperial does the same with feet and miles.
            let mut dist: heapless::String<8> = heapless::String::new();
            let dist_unit = write_distance_split(&mut dist, total_m, units);
            let rows_top = LIST_TOP + 34;
            ledger_row(cv, w, rows_top, rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None);
            // A computed route's points carry no elevation, so the model's ascent term is zero and
            // this reads `distance / v_flat`. The BIKE TYPE row under it names the profile the
            // figure is keyed to.
            let est = est_time_value(total_m, route_ascent_m(rx, summary), rx.settings.bike_profile_idx);
            ledger_row(cv, w, rows_top + ROW_PITCH, rx.t(Msg::RouteOverviewEstTime), &est, "h", None);
            // The profile the route was planned under, so the rider can tell a Road route from an
            // MTB one. The name resolves against the loaded map for the current selection, which is
            // the profile the just-finished plan used.
            draw_profile_label(cv, w, rx, rows_top + 2 * ROW_PITCH);
            // The shape preview fills the middle between the ledger and the START bar.
            draw_route_preview(cv, w, rows_top + 3 * ROW_PITCH, h - 10 - BUTTON_H, rx.nav_preview);
            draw_start_button(cv, w, h, rx.t(Msg::RouteOverviewStartRide));
            return;
        }

        let name = rx.marquee.fit(&summary.name, w - 28, Font::Body, Some(rect(0, 0, w, TITLE_BAR_H)));
        title_frame(cv, w, h, &name, "");

        let band_top = BAND_TOP;

        // The auto-flip swaps this band with the stat rows below. Both pages draw in the same
        // slot, so nothing jumps on the flip.
        let page_b = self.pager.on_second_page();
        if !page_b {
            // An empty slice, for the frame or two before the host hands the preview in, leaves
            // the slot blank.
            draw_route_preview(cv, w, band_top, BAND_BOT, rx.nav_preview);
        } else if let Some(profile) = rx.profile {
            // The shared full-route elevation band, without any of Statistics' live layers. A peak
            // label over the apex gives the vertical scale meaning.
            let band = ElevationBand::whole_route(profile, rect(chart_x, band_top, chart_w, BAND_BOT - band_top + 1));
            band.fill(cv, PARCHMENT_SHADE);
            band.stroke(cv, AMBER);
            band.peak_label(cv, rx.settings.units, PeakLabel::OverPeak);
        } else {
            // While the route still streams open, keep the band's footprint so the page does not
            // jump.
            cv.text(
                rx.t(Msg::RouteOverviewLoadingProfile),
                Point::new(w / 2, (band_top + BAND_BOT) / 2 - 9),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
        }
        cv.hline(chart_x, BAND_BOT + 1, chart_w, RULE); // baseline marks the band slot on both pages

        // A ledger rather than the riding grid's panes, which read as live data and swallow space
        // this page does not need. Distance and climb come from the catalog summary, which is
        // always present; descent needs the opened route.
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

        // The gradient-aware estimate for the whole route, keyed to the rider's bike profile.
        // Totals come from the opened route once it has streamed in, and from the catalog summary
        // before that, so the row never has to show a placeholder.
        let est = est_time_value(route_total_m(rx, summary), route_ascent_m(rx, summary), rx.settings.bike_profile_idx);

        // The stats pair with their media: DISTANCE and EST TIME with the track shape, CLIMB and
        // DESCENT with the elevation band. The flip itself is the affordance, so there are no page
        // dots.
        let entries: [(&str, &str, &str, Option<bool>); 4] = [
            (rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None),
            (rx.t(Msg::RouteOverviewClimb), &climb, units.elev_label(), Some(true)),
            (rx.t(Msg::RouteOverviewDescent), &desc, units.elev_label(), Some(false)),
            (rx.t(Msg::RouteOverviewEstTime), &est, "h", None),
        ];
        let page_rows: &[usize] = if page_b { &[1, 2] } else { &[0, 3] };
        for (slot, &e) in page_rows.iter().enumerate() {
            let y = ROWS_TOP + slot as i32 * ROW_PITCH;
            let (caption, value, unit, arrow) = entries[e];
            ledger_row(cv, w, y, caption, value, unit, arrow);
            if slot + 1 < page_rows.len() {
                cv.hline(16, y + ROW_PITCH - 4, w - 32, RULE);
            }
        }

        // The Pause-menu row family: plain labels, the selected row in the amber fill, and the
        // guarded Delete row with its shaded base and warning hold-fill only while selected. While
        // the route is the active ride's the Delete row is not drawn at all, because a state that
        // cannot act does not show. START keeps the top slot either way, so nothing jumps when the
        // Delete row re-arms.
        let geo = GuardedRowsGeometry::panel(w, action_rows_top(h), OPTION_ROW_H, OPTION_GAP);
        let items = [
            MenuItem { label: rx.t(Msg::RouteOverviewStartRide), guard: false },
            MenuItem { label: rx.t(Msg::RouteOverviewDelete), guard: true },
        ];
        let n = if self.delete_enabled(rx.navigation, rx.recording, rx.routes) { 2 } else { 1 };
        draw_guarded_rows(cv, &items[..n], self.selected.min(n - 1), rx.hold_progress, WARNING, geo);
    }
}

/// Top of the two-row action block. Fixed at the two-row position whether or not Delete is drawn.
fn action_rows_top(h: i32) -> i32 {
    h - 10 - 2 * OPTION_ROW_H - OPTION_GAP
}

fn route_total_m(rx: &Render, summary: &RouteSummary) -> u32 {
    rx.route.map_or(summary.distance_km * 1000, |r| r.total_distance_m)
}

/// The route's total ascent in metres for the time model: the opened route's header figure, or the
/// catalog summary's. A route with no elevation reports `0`, which is what makes [`est_time_value`]
/// fall back to `distance / v_flat` with no branch of its own.
fn route_ascent_m(rx: &Render, summary: &RouteSummary) -> u32 {
    rx.route.map_or(summary.climb_m, |r| r.total_ascent_m)
}

/// The EST TIME ledger value: the whole route through the gradient-aware model
/// ([`obc_route::eta`]) as `H:MM`, the same duration shape the RIDE tile and the ride ledger use.
/// Not localised and not unit-dependent.
fn est_time_value(total_m: u32, ascent_m: u32, bike_profile_idx: u8) -> heapless::String<8> {
    duration_hms(obc_route::route_time_s(total_m, ascent_m, bike_profile_idx) as f32)
}

/// The BIKE TYPE ledger row: the profile name the computed route was planned under, in the same
/// caption-left, value-right shape as the rows above. A stale index shows profile 0's name, which
/// is the profile the router fell back to for this plan.
fn draw_profile_label(cv: &mut impl Surface, w: i32, rx: &Render, y: i32) {
    let mut name: heapless::String<20> = heapless::String::new();
    rx.nav_profiles.write_label(rx.settings.bike_profile_idx, &mut name);
    ledger_row(cv, w, y, rx.t(Msg::RouteOverviewBikeType), &name, "", None);
}

/// The track-shape preview's box size. Centred horizontally, and vertically inside whatever slot
/// the caller hands it.
const PREVIEW_W: i32 = 212;
const PREVIEW_H: i32 = 90;

/// Draw a track's shape preview: the host-decimated polyline, aspect-fit into the
/// [`PREVIEW_W`]×[`PREVIEW_H`] box with the longitude scaled by cos(mid-latitude) so the shape
/// keeps its ground aspect. A filled disc marks the start and a hollow diamond the end. An empty or
/// short slice draws nothing and leaves the box empty. Shared with the Ride detail's recorded-track
/// page, so the two sketches cannot drift.
pub(super) fn draw_route_preview(cv: &mut impl Surface, w: i32, top: i32, bot: i32, pts: &[(i32, i32)]) {
    use palette::*;
    if pts.len() < 2 {
        return;
    }
    // The box clamps to the caller's slot, and the fit insets by the end markers' reach, so a disc
    // or diamond on an extreme point cannot spill past the slot into the rows around it.
    const MARK: i32 = 4;
    let box_h = PREVIEW_H.min(bot - top);
    let (fit_w, fit_h) = (PREVIEW_W - 2 * MARK, box_h - 2 * MARK);
    let x0 = (w - PREVIEW_W) / 2 + MARK;
    let y0 = top + ((bot - top - box_h) / 2).max(0) + MARK;
    let (mut min_lon, mut max_lon, mut min_lat, mut max_lat) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(lon, lat) in pts {
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
    }
    // One scale for both axes, and the fitted shape centred in the box. `max(1.0)` guards a
    // degenerate straight line.
    let clat = obc_map_scene::cos_lat((min_lat / 2) + (max_lat / 2));
    let geo_w = ((max_lon - min_lon) as f32 * clat).max(1.0);
    let geo_h = ((max_lat - min_lat) as f32).max(1.0);
    let scale = (fit_w as f32 / geo_w).min(fit_h as f32 / geo_h);
    let ox = x0 as f32 + (fit_w as f32 - geo_w * scale) / 2.0;
    let oy = y0 as f32 + (fit_h as f32 - geo_h * scale) / 2.0;
    let project = |(lon, lat): (i32, i32)| {
        Point::new((ox + (lon - min_lon) as f32 * clat * scale) as i32, (oy + (max_lat - lat) as f32 * scale) as i32)
    };
    let mut prev = project(pts[0]);
    for &p in &pts[1..] {
        let cur = project(p);
        stroke2(cv, prev, cur, INK);
        prev = cur;
    }
    // The start is a filled disc and the destination a hollow diamond.
    cv.disc(project(pts[0]), 2, INK);
    let d = project(pts[pts.len() - 1]);
    let k = 3;
    cv.line(Point::new(d.x, d.y - k), Point::new(d.x + k, d.y), INK);
    cv.line(Point::new(d.x + k, d.y), Point::new(d.x, d.y + k), INK);
    cv.line(Point::new(d.x, d.y + k), Point::new(d.x - k, d.y), INK);
    cv.line(Point::new(d.x - k, d.y), Point::new(d.x, d.y - k), INK);
}

/// START RIDE at the screen-bottom anchor: the computed-route variant and the POI detail's
/// `Route here` footer are exactly this bar, so the two cannot drift. Always armed, because these
/// pages have a single action and no cursor.
pub(super) fn draw_start_button(cv: &mut impl Surface, w: i32, h: i32, label: &str) {
    use palette::*;
    let by = h - 10 - BUTTON_H;
    let bar = rect(SIDE_MARGIN, by, w - 2 * SIDE_MARGIN, BUTTON_H);
    cv.round(bar, 8, AMBER);
    let tx = w / 2 + 8;
    cv.text_vcentered(label, tx, (by, BUTTON_H), Font::Body, TextAlign::Center, INK);
    // The play wedge sits left of the centred label, measured from its real half-width, so a
    // longer translation cannot run into it.
    let px = tx - label.chars().count() as i32 * Font::Body.char_width() as i32 / 2 - 16;
    let mid = by + BUTTON_H / 2;
    cv.triangle(Point::new(px, mid - 7), Point::new(px, mid + 7), Point::new(px + 11, mid), INK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::route::RouteSummary;
    use crate::screen::test_ctx;
    use crate::settings::Settings;
    use crate::{Activity, AppState};
    use obc_map_scene::BBox;

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

    fn run(
        scr: &mut RouteOverviewScreen,
        act: &mut Activity,
        navigation: &mut RouteState,
        rec: &mut crate::RecorderMachine,
        routes: &[RouteSummary],
        g: Gesture,
    ) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut navigator = crate::navigator::NavigatorMachine::new();
        *navigator.route_state_mut() = *navigation;
        let result = {
            let mut cx =
                Ctx { routes, recorder: rec, navigator: &mut navigator, ..test_ctx(&mut st, act, &mut settings) };
            scr.handle(g, &mut cx)
        };
        *navigation = *navigator.route_state();
        result
    }

    /// Entry selects START and a hold there does nothing. Deleting takes a step onto the Delete
    /// row first; the completed hold then records the route's index, restores the pre-preview
    /// active route, and pops back to the Routes list.
    #[test]
    fn hold_deletes_only_from_the_selected_delete_row() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary(), summary()];
        let mut act = Activity::new(Mode::Idle);
        let mut navigation = RouteState::new();
        navigation.active_route = Some(1); // the menu preview
        let mut scr = RouteOverviewScreen::new(1, Some(0)); // was previewing route 0 before
        assert!(scr.delete_enabled(&navigation, rec.recording(), &routes), "an Idle preview is deletable");
        assert!(
            !scr.selection_is_guarded(&navigation, rec.recording(), &routes),
            "entry selects START — nothing armed"
        );
        let t = run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold with START selected does not delete");
        assert_eq!(act.take_route_delete(), None);

        run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Step(1)); // → the Delete row
        assert!(
            scr.selection_is_guarded(&navigation, rec.recording(), &routes),
            "the hold fill is live on the Delete row"
        );
        let t = run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "the delete pops back to the Routes list");
        assert_eq!(act.take_route_delete(), Some(1), "records the previewed route's index");
        assert_eq!(navigation.active_route, Some(0), "the pre-preview route is restored");
    }

    /// A press fires START only from the START row. With the cursor on Delete a press does
    /// nothing, because that row is hold-guarded.
    #[test]
    fn press_on_the_delete_row_is_a_no_op() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary()];
        let mut act = Activity::new(Mode::Idle);
        let mut navigation = RouteState::new();
        let mut scr = RouteOverviewScreen::new(0, None);
        run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Step(1)); // → Delete
        let t = run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Press);
        assert!(matches!(t, Transition::None), "press on the Delete row starts nothing");
        assert!(!rec.recording(), "no session began");
        run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Step(1)); // wrap back → START
        let t = run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Press);
        assert!(!matches!(t, Transition::None), "press on START starts the ride");
    }

    /// While this route is the active route of a running tracking session the Delete row is
    /// hidden, so a step has nothing to select and a hold does nothing.
    #[test]
    fn hold_over_the_active_ride_route_is_a_no_op() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary(), summary()];
        let mut act = Activity::new(Mode::Riding);
        let mut navigation = RouteState::new();
        rec.test_open(); // now tracking…
        navigation.active_route = Some(0); // …route 0
        let mut scr = RouteOverviewScreen::new(0, None);
        assert!(!scr.delete_enabled(&navigation, rec.recording(), &routes), "the active ride's route can't be deleted");
        run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Step(1));
        assert_eq!(scr.selected, START, "with the Delete row hidden there is nothing to toggle");
        let t = run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold over the in-use route does nothing");
        assert_eq!(act.take_route_delete(), None);
    }

    /// A computed overview has no Delete row, so a step stays on START and a hold is a no-op.
    #[test]
    fn computed_overview_has_no_delete() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary()];
        let mut act = Activity::new(Mode::Idle);
        let mut navigation = RouteState::new();
        let mut scr = RouteOverviewScreen::computed(0, None);
        assert!(!scr.delete_enabled(&navigation, rec.recording(), &routes));
        run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Step(1));
        assert_eq!(scr.selected, START, "no Delete row — the step is a no-op");
        run(&mut scr, &mut act, &mut navigation, &mut rec, &routes, Gesture::Hold);
        assert_eq!(act.take_route_delete(), None);
    }

    /// The page the pager selects is the page the content is paired with. The flip timing is the
    /// shared pager's; this pins that the screen delegates it and reads it back.
    #[test]
    fn the_pager_drives_this_screen_s_paired_pages() {
        use super::super::vocab::pager::PAGE_FLIP_MS;
        let mut scr = RouteOverviewScreen::new(0, None);
        assert!(!scr.tick_timers(0).changed, "the first poll only anchors the dwell");
        assert!(!scr.pager.on_second_page(), "entry shows the track shape + DISTANCE page");
        assert!(scr.tick_timers(PAGE_FLIP_MS).changed, "the dwell flips the page");
        assert!(scr.pager.on_second_page(), "now on the elevation (CLIMB + DESCENT) page");
    }

    /// The computed page has one fixed layout and no pager, so its tick never self-dirties.
    #[test]
    fn computed_overview_never_flips() {
        use super::super::vocab::pager::PAGE_FLIP_MS;
        let mut scr = RouteOverviewScreen::computed(0, None);
        assert!(!scr.tick_timers(PAGE_FLIP_MS).changed);
        assert_eq!(scr.tick_timers(PAGE_FLIP_MS), ScreenTick::idle());
    }

    /// `H:MM` from the gradient-aware model, keyed by the rider's bike profile. A zero-ascent
    /// route degrades to plain `distance / v_flat`, so the row never needs a "no elevation" branch
    /// or a `--`.
    #[test]
    fn est_time_value_is_the_gradient_aware_estimate() {
        // The road profile at 22 km/h flat: 44 km with no climbing is two hours.
        assert_eq!(est_time_value(44_000, 0, 0).as_str(), "2:00", "a flat route is distance / v_flat");
        // The same 44 km over a 1000 m col costs 1000 × 1.6 s more, so 2:26.
        assert_eq!(est_time_value(44_000, 1_000, 0).as_str(), "2:26", "the col adds its climb term");
        // The same route on the MTB profile, at 16 km/h and 2.3 s/m: 2:45 flat plus 38:20
        // climbing, so 3:23.
        assert_eq!(est_time_value(44_000, 1_000, 2).as_str(), "3:23", "a slower bike, a longer day");
        // A stale index falls back to profile 0, which is the router's own rule.
        assert_eq!(est_time_value(44_000, 1_000, 99), est_time_value(44_000, 1_000, 0));
        // Degenerate inputs read as zero rather than as a placeholder.
        assert_eq!(est_time_value(0, 0, 0).as_str(), "0:00");
    }
}
