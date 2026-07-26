//! The **Detour** flow (#882, superseding the pure Skip-ahead of #788): pick a rejoin point on
//! the active route, let the host plan an A* detour around the skipped span (its corridor
//! blacklisted on the nav graph), preview the detour's shape + distance cost, and commit a
//! full-splice derived route.
//!
//! Two screens live here:
//! - [`DetourScreen`] — the Up/Down-stepped rejoin chooser (unchanged mechanics from #788: the
//!   screen streams only the highlighted interval, fits a local north-up camera, Hold toggles a
//!   rejoin-inspection camera). **Press now posts a plan request** and pushes the shared planning
//!   spinner; there is no pure skip.
//! - [`DetourPreviewScreen`] — the planned detour drawn over the map (skipped span in warning
//!   ink, the detour in blue) with a signed "±X km" cost line. Press commits the splice;
//!   Back cancels back to the chooser with steps intact.
//!
//! The chooser holds **no route geometry** — only the entry anchor, Up/Down step count, compact
//! inspection zoom state, and one prepared coordinate/bounds record; the preview additionally
//! reads the host-fed decimated detour polyline through [`Render::detour_preview`].

use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface, Viewport,
};
use obc_route::BBox;

use crate::activity::{Activity, DetourRequest};
use crate::host::DetourPreview;
use crate::input::Gesture;
use crate::Msg;

use super::map::{draw_map_scene, DetourMapOverlay};
use super::nav_route::NavPlanningScreen;
use super::{Ctx, Prepare, Render, Screen, Transition};

/// One Up/Down step changes the requested along-route rejoin distance by 100 m. A detour is for
/// nearby closures and trail problems, so finer control matters more than spanning many
/// kilometres; the route-end clamp still displays/commits the exact non-multiple remainder.
pub(crate) const DETOUR_STEP_M: u32 = 100;
/// The minimum rejoin distance — [`obc_route::MIN_DETOUR_SPAN_M`]: below it the corridor's
/// endpoint-exemption discs swallow the whole span and a "detour" would just re-follow the route,
/// so the chooser refuses to select shorter spans (and a shorter remainder is "Route ends here").
pub(crate) const MIN_DETOUR_M: u32 = obc_route::MIN_DETOUR_SPAN_M;
/// The chooser's lower step bound (`MIN_DETOUR_M` expressed in steps).
const MIN_STEPS: u16 = MIN_DETOUR_M.div_ceil(DETOUR_STEP_M) as u16;
/// Enter inspection at roughly 2.5× the overview scale: enough to resolve the candidate's local
/// junction without making the Hold transition visually disorienting.
const INSPECT_MIN_EXP: i8 = -2;
const INSPECT_ENTRY_EXP: i8 = 5;
const INSPECT_MAX_EXP: i8 = 13;
const INSPECT_ENTRY_LEVEL: u8 = (INSPECT_ENTRY_EXP - INSPECT_MIN_EXP + 1) as u8;
const INSPECT_MAX_LEVEL: u8 = (INSPECT_MAX_EXP - INSPECT_MIN_EXP + 1) as u8;
const INSPECT_ZOOM_STEP: f32 = 1.2;
const HUD_H: i32 = 76;
const HUD_MARGIN: i32 = 10;
const FIT_MARGIN: f32 = 24.0;

#[derive(Debug, Clone, Copy)]
struct PreparedDetour {
    target_m: u32,
    candidate: (i32, i32),
    bounds: BBox,
}

/// Screen-local chooser state. No route geometry is retained: only the entry anchor, Up/Down step
/// count, compact inspection zoom state, and one prepared coordinate/bounds record live in the
/// screen stack. `Copy`, so the planned-answer router can lift the request context into the
/// preview screen without a back-channel.
#[derive(Debug, Clone, Copy)]
pub struct DetourScreen {
    route: Option<usize>,
    start_m: u32,
    total_m: u32,
    steps: u16,
    /// `0` = overview/distance adjustment; `1..=INSPECT_MAX_LEVEL` = rejoin inspection. Level one
    /// starts two zoom steps wider than the fitted overview; higher levels move progressively in.
    inspect_level: u8,
    prepared: Option<PreparedDetour>,
}

impl DetourScreen {
    pub fn new(activity: &Activity) -> Self {
        DetourScreen {
            route: activity.active_route,
            start_m: activity.progress_m,
            total_m: activity.route_total_m,
            steps: MIN_STEPS,
            inspect_level: 0,
            prepared: None,
        }
    }

    /// Re-point the chooser's held catalog slot after a live route rescan. A surviving route keeps
    /// the selection by identity; a vanished one becomes unavailable and cannot start a plan.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = self.route.and_then(remap);
        self.prepared = None;
    }

    fn actual_detour_m(&self) -> Option<u32> {
        let remaining = self.total_m.saturating_sub(self.start_m);
        (remaining >= MIN_DETOUR_M).then_some((self.steps as u32).saturating_mul(DETOUR_STEP_M).min(remaining))
    }

    fn target_m(&self) -> Option<u32> {
        self.actual_detour_m().map(|d| self.start_m.saturating_add(d).min(self.total_m))
    }

    fn inspecting(&self) -> bool {
        self.inspect_level != 0
    }

    fn inspect_zoom(&self) -> f32 {
        let exponent = INSPECT_MIN_EXP + self.inspect_level.saturating_sub(1) as i8;
        let mut zoom = 1.0;
        if exponent >= 0 {
            for _ in 0..exponent {
                zoom *= INSPECT_ZOOM_STEP;
            }
        } else {
            for _ in exponent..0 {
                zoom /= INSPECT_ZOOM_STEP;
            }
        }
        zoom
    }

    fn available(&self, activity: &Activity, has_nav_graph: bool) -> bool {
        activity.is_tracking()
            && has_nav_graph
            && self.route.is_some()
            && activity.active_route == self.route
            && !activity.off_route
            && self.actual_detour_m().is_some()
    }

    /// Move the selected stretch's start to the latest matched rider progress while preserving the
    /// requested step count. This keeps HUD distance, prepared ink and Press semantics aligned even
    /// when the rider keeps moving with the chooser open.
    fn refresh_anchor(&mut self, activity: &Activity) {
        if activity.active_route == self.route
            && (activity.progress_m != self.start_m || activity.route_total_m != self.total_m)
        {
            self.start_m = activity.progress_m;
            self.total_m = activity.route_total_m;
            self.prepared = None;
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        self.refresh_anchor(cx.activity);
        let available = self.available(cx.activity, cx.state.has_nav_graph);
        match g {
            Gesture::Step(n) if available && self.inspecting() => {
                let next = (self.inspect_level as i32).saturating_add(n).clamp(1, INSPECT_MAX_LEVEL as i32);
                self.inspect_level = next as u8;
                Transition::None
            }
            Gesture::Step(n) if available => {
                let remaining = self.total_m.saturating_sub(self.start_m);
                let max_steps = remaining.saturating_add(DETOUR_STEP_M - 1) / DETOUR_STEP_M;
                let max_steps = max_steps.clamp(MIN_STEPS as u32, u16::MAX as u32) as i32;
                let next = (self.steps as i32).saturating_add(n).clamp(MIN_STEPS as i32, max_steps);
                self.steps = next as u16;
                self.prepared = None;
                Transition::None
            }
            Gesture::Press if available => {
                // Derive from the current step count here — never from `prepared`, which can still
                // describe the previous frame when Step and Press arrive in one input drain. The
                // request freezes the corridor/prefix anchor at this instant; the host resolves
                // the rejoin coordinate itself (it owns the RouteReader).
                if let (Some(route), Some(target), Some(fix)) = (self.route, self.target_m(), cx.state.user_fix) {
                    cx.activity.request_detour(DetourRequest {
                        route,
                        from: (fix.lon, fix.lat),
                        progress_m: self.start_m,
                        target_m: target,
                    });
                    // Push (not Replace): Back from the planning spinner or the preview returns
                    // here with steps intact.
                    Transition::Push(Screen::NavPlanning(NavPlanningScreen::detour()))
                } else {
                    Transition::None
                }
            }
            // The Select hold is unused by the chooser otherwise. Toggle between the spatial
            // overview and a candidate-centred inspection camera without changing the selection.
            Gesture::Hold if available => {
                self.inspect_level = if self.inspecting() { 0 } else { INSPECT_ENTRY_LEVEL };
                Transition::None
            }
            // Cancel consumes both chooser and ride-menu caller, restoring the riding view without
            // planning anything or changing progress/session/mode.
            Gesture::Back => Transition::Pop,
            Gesture::Step(_) | Gesture::Press | Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn prepare(&mut self, px: &mut Prepare) {
        if px.active_route == self.route && (px.progress_m != self.start_m || px.route_total_m != self.total_m) {
            self.start_m = px.progress_m;
            self.total_m = px.route_total_m;
            self.prepared = None;
        }
        let Some(target_m) = self.target_m() else {
            self.prepared = None;
            return;
        };
        if self.prepared.is_some_and(|p| p.target_m == target_m) {
            return;
        }
        let Some(route) = px.route else {
            self.prepared = None;
            return;
        };
        let Some(candidate) = route.position_at(target_m) else {
            self.prepared = None;
            return;
        };
        let mut bounds =
            BBox { min_lon: candidate.lon, min_lat: candidate.lat, max_lon: candidate.lon, max_lat: candidate.lat };
        route.visit_points_between(self.start_m, target_m, |pts| {
            for &(lon, lat) in pts {
                extend_bounds(&mut bounds, lon, lat);
            }
        });
        if let Some(fix) = px.user_fix {
            extend_bounds(&mut bounds, fix.lon, fix.lat);
        }
        self.prepared = Some(PreparedDetour { target_m, candidate: (candidate.lon, candidate.lat), bounds });
    }

    pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut Render)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let selected = self.prepared.filter(|_| self.available(rx.activity, rx.state.has_nav_graph));
        let vp = selected.map_or_else(
            || rx.state.viewport(rx.w as f32, rx.h as f32),
            |p| {
                let overview = fit_viewport(rx.w, rx.h, p.bounds);
                if self.inspecting() {
                    inspect_viewport(rx.w, rx.h, p.candidate, overview.zoom, self.inspect_zoom())
                } else {
                    overview
                }
            },
        );
        let overlay = selected.map(|p| DetourMapOverlay {
            start_m: self.start_m,
            end_m: p.target_m,
            candidate: p.candidate,
            detour: &[],
        });
        let _ = draw_map_scene(cv, rx, &vp, overlay);
        self.draw_hud(cv, rx);
    }

    fn draw_hud(&self, cv: &mut impl Surface, rx: &Render) {
        use super::palette::*;
        let x = HUD_MARGIN;
        let y = rx.h - HUD_H - HUD_MARGIN;
        let w = rx.w - 2 * HUD_MARGIN;
        cv.round(rect(x, y, w, HUD_H), 11, PARCHMENT);
        cv.round_outline(rect(x, y, w, HUD_H), 11, INK);
        let title = if self.inspecting() { rx.t(Msg::RideMenuInspectRejoin) } else { rx.t(Msg::RideMenuDetour) };
        cv.text(title, Point::new(rx.w / 2, y + 7), Font::Label, TextAlign::Center, INK);

        let status = if self.route.is_none() || rx.activity.active_route != self.route {
            Err(rx.t(Msg::RideMenuNoRoute))
        } else if !rx.state.has_nav_graph {
            Err(rx.t(Msg::RideMenuNoNav))
        } else if rx.activity.off_route {
            Err(rx.t(Msg::RideMenuOffRoute))
        } else if let Some(m) = self.actual_detour_m() {
            Ok(crate::stat_fields::fmt_dist_short(m, rx.settings.units))
        } else {
            Err(rx.t(Msg::RideMenuRouteEnd))
        };
        match status {
            Ok(dist) => {
                cv.text("-", Point::new(x + 24, y + 36), Font::Display, TextAlign::Center, INK);
                cv.text(dist.as_str(), Point::new(rx.w / 2, y + 36), Font::Display, TextAlign::Center, WARNING);
                cv.text("+", Point::new(x + w - 24, y + 36), Font::Display, TextAlign::Center, INK);
            }
            Err(msg) => {
                cv.text(msg, Point::new(rx.w / 2, y + 40), Font::Label, TextAlign::Center, WARNING);
            }
        }
    }
}

/// The planned-detour preview (#882): the skipped span and the detour's decimated polyline over a
/// fitted map, with a signed distance-cost line. **Press commits** the splice (the host answers
/// `DetourCommitted` and the app lands back on the riding view); **Back cancels** to the chooser.
/// The cost figure and polyline describe the plan frozen at the chooser's Press — the anchor is
/// deliberately not re-derived here.
#[derive(Debug, Clone, Copy)]
pub struct DetourPreviewScreen {
    route: Option<usize>,
    /// The frozen prefix/corridor anchor (the chooser's `start_m` at Press) — the splice seam the
    /// commit handler re-anchors the matcher at.
    anchor_m: u32,
    /// The chosen rejoin distance (for the skipped-span overlay + staleness checks).
    target_m: u32,
    /// The plan's cost delta, meters, signed (see [`DetourPreview::cost_delta_m`]).
    cost_delta_m: i32,
    /// Press posted the commit; further Presses are no-ops while the host works.
    committing: bool,
    /// The commit failed host-side — the old route is untouched; shown inline on the HUD.
    error: bool,
    prepared: Option<PreparedDetour>,
}

impl DetourPreviewScreen {
    /// The preview for a completed plan: request context lifted from the chooser (still on the
    /// stack below), figures from the host's [`DetourPreview`] answer.
    pub(crate) fn new(chooser: &DetourScreen, preview: DetourPreview) -> Self {
        DetourPreviewScreen {
            route: chooser.route,
            anchor_m: chooser.start_m,
            target_m: chooser.target_m().unwrap_or(chooser.total_m),
            cost_delta_m: preview.cost_delta_m,
            committing: false,
            error: false,
            prepared: None,
        }
    }

    /// The frozen splice-seam anchor — what the commit handler queues the matcher re-anchor at.
    pub(crate) fn anchor_m(&self) -> u32 {
        self.anchor_m
    }

    /// Mark the commit failed ([`HostEvent::DetourCommitted`](crate::host::HostEvent) `Err` arm):
    /// stay up, show the inline error, allow another Press or Back.
    pub(crate) fn set_commit_failed(&mut self) {
        self.committing = false;
        self.error = true;
    }

    /// Re-point the held catalog slot after a live route rescan (vanished route → the staleness
    /// guard in [`handle`](Self::handle) cancels out).
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = self.route.and_then(remap);
        self.prepared = None;
    }

    /// The plan went stale under the preview: its route vanished/swapped, the rider rode past the
    /// rejoin point, or went off-route. Checked on every gesture — the commit itself is safe
    /// under drift (the splice uses the frozen anchor and the matcher re-locks from the live
    /// fix), so staleness only gates *starting* one.
    fn stale(&self, activity: &Activity) -> bool {
        self.route.is_none()
            || activity.active_route != self.route
            || activity.off_route
            || activity.progress_m >= self.target_m
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.stale(cx.activity) {
            // Cancel out to the chooser, which re-anchors to live progress; the host drops the
            // held detour bytes.
            cx.activity.request_detour_cancel();
            return Transition::Pop;
        }
        match g {
            Gesture::Press if !self.committing => {
                cx.activity.request_detour_commit();
                self.committing = true;
                self.error = false;
                Transition::None
            }
            Gesture::Back => {
                cx.activity.request_detour_cancel();
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    pub fn prepare(&mut self, px: &mut Prepare) {
        if self.prepared.is_some() {
            return;
        }
        let Some(route) = px.route else { return };
        let Some(candidate) = route.position_at(self.target_m) else { return };
        let mut bounds =
            BBox { min_lon: candidate.lon, min_lat: candidate.lat, max_lon: candidate.lon, max_lat: candidate.lat };
        route.visit_points_between(self.anchor_m, self.target_m, |pts| {
            for &(lon, lat) in pts {
                extend_bounds(&mut bounds, lon, lat);
            }
        });
        for &(lon, lat) in px.detour_preview {
            extend_bounds(&mut bounds, lon, lat);
        }
        if let Some(fix) = px.user_fix {
            extend_bounds(&mut bounds, fix.lon, fix.lat);
        }
        self.prepared =
            Some(PreparedDetour { target_m: self.target_m, candidate: (candidate.lon, candidate.lat), bounds });
    }

    pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut Render)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let vp = self
            .prepared
            .map_or_else(|| rx.state.viewport(rx.w as f32, rx.h as f32), |p| fit_viewport(rx.w, rx.h, p.bounds));
        let overlay = self.prepared.map(|p| DetourMapOverlay {
            start_m: self.anchor_m,
            end_m: p.target_m,
            candidate: p.candidate,
            detour: rx.detour_preview,
        });
        let _ = draw_map_scene(cv, rx, &vp, overlay);
        self.draw_hud(cv, rx);
    }

    fn draw_hud(&self, cv: &mut impl Surface, rx: &Render) {
        use super::palette::*;
        let x = HUD_MARGIN;
        let y = rx.h - HUD_H - HUD_MARGIN;
        let w = rx.w - 2 * HUD_MARGIN;
        cv.round(rect(x, y, w, HUD_H), 11, PARCHMENT);
        cv.round_outline(rect(x, y, w, HUD_H), 11, INK);
        cv.text(rx.t(Msg::DetourTitle), Point::new(rx.w / 2, y + 7), Font::Label, TextAlign::Center, INK);

        if self.error {
            cv.text(
                rx.t(Msg::DetourCommitFailed),
                Point::new(rx.w / 2, y + 40),
                Font::Label,
                TextAlign::Center,
                WARNING,
            );
            return;
        }
        // The signed cost line: "+4.2 km" (or "-", for a detour that shortcuts a wandering span).
        let sign = if self.cost_delta_m < 0 { "-" } else { "+" };
        let dist = crate::stat_fields::fmt_dist_short(self.cost_delta_m.unsigned_abs(), rx.settings.units);
        let mut line: heapless::String<12> = heapless::String::new();
        let _ = line.push_str(sign);
        let _ = line.push_str(dist.as_str());
        cv.text(line.as_str(), Point::new(rx.w / 2, y + 36), Font::Display, TextAlign::Center, WARNING);
    }
}

fn extend_bounds(b: &mut BBox, lon: i32, lat: i32) {
    b.min_lon = b.min_lon.min(lon);
    b.min_lat = b.min_lat.min(lat);
    b.max_lon = b.max_lon.max(lon);
    b.max_lat = b.max_lat.max(lat);
}

/// North-up camera fitting the whole rider→candidate selected path above the bottom HUD with a
/// fixed pixel margin. The camera is shifted south so the bounds centre lands in the usable map
/// region rather than behind the panel.
fn fit_viewport(w: i32, h: i32, b: BBox) -> Viewport {
    let cam_lon = b.min_lon + (b.max_lon - b.min_lon) / 2;
    let centre_lat = b.min_lat + (b.max_lat - b.min_lat) / 2;
    let aspect = obc_route::cos_lat(centre_lat).abs().max(0.01);
    let span_x = (b.max_lon - b.min_lon).unsigned_abs() as f32 * aspect;
    let span_y = (b.max_lat - b.min_lat).unsigned_abs() as f32;
    let usable_w = (w as f32 - 2.0 * FIT_MARGIN).max(1.0);
    let usable_bottom = (h - HUD_H - 2 * HUD_MARGIN) as f32;
    let usable_h = (usable_bottom - FIT_MARGIN).max(1.0);
    let zx = if span_x > 0.0 { usable_w / span_x } else { super::map::MAX_ZOOM };
    let zy = if span_y > 0.0 { usable_h / span_y } else { super::map::MAX_ZOOM };
    let zoom = zx.min(zy).clamp(super::map::MIN_ZOOM, super::map::MAX_ZOOM);
    let desired_y = FIT_MARGIN + usable_h / 2.0;
    let cam_lat = centre_lat - ((h as f32 / 2.0 - desired_y) / zoom) as i32;
    Viewport::new(w as f32, h as f32, cam_lon, cam_lat, zoom)
}

/// Candidate-centred north-up camera for the inspection sub-mode. The ring sits at the same usable
/// map-area centre as the overview bounds, never behind the floating HUD.
fn inspect_viewport(w: i32, h: i32, candidate: (i32, i32), overview_zoom: f32, factor: f32) -> Viewport {
    let zoom = (overview_zoom * factor).clamp(super::map::MIN_ZOOM, super::map::MAX_ZOOM);
    let usable_bottom = (h - HUD_H - 2 * HUD_MARGIN) as f32;
    let usable_h = (usable_bottom - FIT_MARGIN).max(1.0);
    let desired_y = FIT_MARGIN + usable_h / 2.0;
    let cam_lat = candidate.1 - ((h as f32 / 2.0 - desired_y) / zoom) as i32;
    Viewport::new(w as f32, h as f32, candidate.0, cam_lat, zoom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppState, NavProfiles, Settings};
    use obc_ports::Fix;

    fn with_ctx<T>(activity: &mut Activity, f: impl FnOnce(&mut Ctx) -> T) -> T {
        with_state_ctx(activity, AppState::new(0, 0, 1.0), f)
    }

    /// A `Ctx` whose `AppState` has a nav graph, a fix, and whatever the test staged.
    fn with_state_ctx<T>(activity: &mut Activity, mut state: AppState, f: impl FnOnce(&mut Ctx) -> T) -> T {
        let mut settings = Settings::default();
        let scratch = super::super::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut state,
            activity,
            settings: &mut settings,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        f(&mut cx)
    }

    fn nav_state() -> AppState {
        let mut s = AppState::new(0, 0, 1.0);
        s.has_nav_graph = true;
        s.user_fix = Some(Fix { lon: 7_800_000, lat: 48_000_000, course: None, speed_mps: None });
        s
    }

    fn tracking_activity(progress: u32, total: u32) -> Activity {
        let mut a = Activity::new(crate::activity::Mode::Riding);
        a.active_route = Some(2);
        a.progress_m = progress;
        a.route_total_m = total;
        a.start_session();
        a
    }

    #[test]
    fn candidate_steps_clamp_to_actual_route_remainder() {
        let a = tracking_activity(200, 1_350);
        let mut s = DetourScreen::new(&a);
        assert_eq!(s.actual_detour_m(), Some(600), "opens at the minimum non-degenerate span");
        s.steps = 99;
        assert_eq!(s.actual_detour_m(), Some(1_150));
        assert_eq!(s.target_m(), Some(1_350));
    }

    #[test]
    fn near_end_has_no_rejoin_candidate() {
        // Below MIN_DETOUR_M of remaining route there is no non-degenerate detour.
        let mut a = tracking_activity(950, 1_500);
        a.active_route = Some(0);
        assert_eq!(DetourScreen::new(&a).actual_detour_m(), None);
    }

    #[test]
    fn turn_then_press_posts_a_plan_request_and_pushes_planning() {
        let mut a = tracking_activity(1_000, 5_000);
        let session = a.session();
        let mode = a.mode;
        let mut s = DetourScreen::new(&a);
        with_state_ctx(&mut a, nav_state(), |cx| assert!(matches!(s.handle(Gesture::Step(2), cx), Transition::None)));
        let t = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Push(Screen::NavPlanning(_))), "Press starts the plan flow");
        let req = a.take_detour_request().expect("plan request queued");
        assert_eq!(
            (req.route, req.progress_m, req.target_m),
            (2, 1_000, 1_800),
            "minimum six steps plus two, from the live anchor"
        );
        assert_eq!(req.from, (7_800_000, 48_000_000), "the rider's fix rides the request");
        assert!(a.pending_seam().is_none(), "no floor/seam is installed at Press — only at commit");
        assert_eq!(a.progress_m, 1_000, "progress untouched until the commit re-adopts");
        assert_eq!(a.session(), session, "same tracking session");
        assert_eq!(a.mode, mode, "Mode is preserved");
    }

    #[test]
    fn press_without_nav_graph_or_fix_is_guarded() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut s = DetourScreen::new(&a);
        // No nav graph: unavailable outright.
        assert!(matches!(with_ctx(&mut a, |cx| s.handle(Gesture::Press, cx)), Transition::None));
        assert!(a.take_detour_request().is_none());
        // Graph but no fix: available() passes, the Press guard refuses to send a garbage start.
        let mut state = nav_state();
        state.user_fix = None;
        assert!(matches!(with_state_ctx(&mut a, state, |cx| s.handle(Gesture::Press, cx)), Transition::None));
        assert!(a.take_detour_request().is_none());
    }

    #[test]
    fn back_cancels_to_map_without_any_navigation_change() {
        let mut a = tracking_activity(800, 4_000);
        let before = a;
        let mut s = DetourScreen::new(&a);
        with_state_ctx(&mut a, nav_state(), |cx| {
            let _ = s.handle(Gesture::Step(3), cx);
        });
        let t = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));
        assert_eq!(a.progress_m, before.progress_m);
        assert_eq!(a.session(), before.session());
        assert_eq!(a.mode, before.mode);
        assert!(a.take_detour_request().is_none());
    }

    #[test]
    fn route_less_off_route_and_near_end_press_are_guarded() {
        let mut route_less = tracking_activity(0, 0);
        route_less.active_route = None;
        let mut s = DetourScreen::new(&route_less);
        assert!(matches!(
            with_state_ctx(&mut route_less, nav_state(), |cx| s.handle(Gesture::Press, cx)),
            Transition::None
        ));

        let mut off = tracking_activity(100, 2_000);
        off.off_route = true;
        let mut s = DetourScreen::new(&off);
        assert!(matches!(with_state_ctx(&mut off, nav_state(), |cx| s.handle(Gesture::Press, cx)), Transition::None));

        let mut end = tracking_activity(1_950, 2_000);
        let mut s = DetourScreen::new(&end);
        assert!(matches!(with_state_ctx(&mut end, nav_state(), |cx| s.handle(Gesture::Press, cx)), Transition::None));
        assert!(
            route_less.take_detour_request().is_none()
                && off.take_detour_request().is_none()
                && end.take_detour_request().is_none()
        );
    }

    #[test]
    fn moving_while_open_advances_highlight_and_plan_anchor() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut s = DetourScreen::new(&a);
        // Rider advances before a Step; that input refreshes the live anchor and adds 100 m.
        a.progress_m = 1_200;
        with_state_ctx(&mut a, nav_state(), |cx| {
            let _ = s.handle(Gesture::Step(1), cx);
        });
        assert_eq!((s.start_m, s.target_m()), (1_200, Some(1_900)));
        // Another 100 m before Press: the request is still a 700 m span, now from 1.3 km.
        a.progress_m = 1_300;
        let t = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Push(Screen::NavPlanning(_))));
        let req = a.take_detour_request().unwrap();
        assert_eq!((req.progress_m, req.target_m), (1_300, 2_000));
    }

    #[test]
    fn hold_toggles_candidate_inspection_and_turn_changes_only_zoom() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut s = DetourScreen::new(&a);
        let target = s.target_m();

        assert!(matches!(with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Hold, cx)), Transition::None));
        assert!(s.inspecting());
        assert_eq!(s.inspect_level, INSPECT_ENTRY_LEVEL);
        let entry_zoom = s.inspect_zoom();

        let _ = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Step(-20), cx));
        assert!(s.inspect_zoom() < 1.0, "inspection can zoom a little wider than the fitted overview");
        assert_eq!(s.target_m(), target, "inspection never changes the selected rejoin point");

        let _ = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Step(20), cx));
        assert!(s.inspect_zoom() > entry_zoom, "a Step zooms back in while inspecting");

        let _ = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Hold, cx));
        assert!(!s.inspecting(), "a second Hold returns to the overview");
    }

    #[test]
    fn press_from_inspection_plans_the_unchanged_candidate() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut s = DetourScreen::new(&a);
        let target = s.target_m().unwrap();
        let _ = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Hold, cx));
        let _ = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Step(3), cx));

        let t = with_state_ctx(&mut a, nav_state(), |cx| s.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Push(Screen::NavPlanning(_))));
        assert_eq!(a.take_detour_request().unwrap().target_m, target);
    }

    // ---- the preview screen ----

    fn preview_for(a: &Activity) -> DetourPreviewScreen {
        let chooser = DetourScreen::new(a);
        DetourPreviewScreen::new(&chooser, DetourPreview { cost_delta_m: 4_200, total_distance_m: 5_000 })
    }

    #[test]
    fn preview_press_commits_once_and_back_cancels() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut p = preview_for(&a);
        assert_eq!(p.anchor_m(), 1_000);

        let t = with_state_ctx(&mut a, nav_state(), |cx| p.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::None), "commit keeps the preview up until the host answers");
        assert!(a.take_detour_commit(), "the commit one-shot is queued");
        // A second Press while committing is a no-op.
        let _ = with_state_ctx(&mut a, nav_state(), |cx| p.handle(Gesture::Press, cx));
        assert!(!a.take_detour_commit(), "no double commit");

        let mut b = tracking_activity(1_000, 5_000);
        let mut p2 = preview_for(&b);
        let t = with_state_ctx(&mut b, nav_state(), |cx| p2.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));
        assert!(b.take_detour_cancel(), "Back rings the host to drop the held detour");
    }

    #[test]
    fn preview_commit_failure_reopens_press() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut p = preview_for(&a);
        let _ = with_state_ctx(&mut a, nav_state(), |cx| p.handle(Gesture::Press, cx));
        assert!(a.take_detour_commit());
        p.set_commit_failed();
        let _ = with_state_ctx(&mut a, nav_state(), |cx| p.handle(Gesture::Press, cx));
        assert!(a.take_detour_commit(), "a failed commit can be retried");
    }

    #[test]
    fn preview_goes_stale_when_the_rider_passes_the_rejoin_or_route_vanishes() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut p = preview_for(&a);
        a.progress_m = p.target_m; // rode past the rejoin during the preview
        let t = with_state_ctx(&mut a, nav_state(), |cx| p.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop), "stale preview cancels out instead of committing");
        assert!(a.take_detour_cancel());
        assert!(!a.take_detour_commit());

        let mut b = tracking_activity(1_000, 5_000);
        let mut p = preview_for(&b);
        p.remap_routes(&|_| None); // the planned route vanished in a rescan
        let t = with_state_ctx(&mut b, nav_state(), |cx| p.handle(Gesture::Step(1), cx));
        assert!(matches!(t, Transition::Pop));
        assert!(b.take_detour_cancel());
    }

    #[test]
    fn fitted_bounds_land_above_the_hud_with_margin() {
        let b = BBox { min_lon: 7_800_000, min_lat: 48_000_000, max_lon: 7_810_000, max_lat: 48_010_000 };
        let vp = fit_viewport(240, 320, b);
        for (lon, lat) in [(b.min_lon, b.min_lat), (b.max_lon, b.max_lat)] {
            let (x, y) = vp.to_screen(lon, lat);
            assert!(x >= FIT_MARGIN as i32 - 1 && x <= 240 - FIT_MARGIN as i32 + 1);
            assert!(y >= FIT_MARGIN as i32 - 1 && y <= 320 - HUD_H - 2 * HUD_MARGIN + 1);
        }
    }

    #[test]
    fn inspection_centres_the_candidate_above_the_hud_at_a_tighter_zoom() {
        let candidate = (7_805_000, 48_005_000);
        let vp = inspect_viewport(240, 320, candidate, 0.01, 2.5);
        let (x, y) = vp.to_screen(candidate.0, candidate.1);
        assert_eq!(x, 120);
        assert!(y >= FIT_MARGIN as i32 && y < 320 - HUD_H - 2 * HUD_MARGIN);
        assert!((vp.zoom - 0.025).abs() < 1e-6);
    }
}
