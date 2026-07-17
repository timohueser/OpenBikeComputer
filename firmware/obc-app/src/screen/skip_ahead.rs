//! Pure Skip-ahead chooser (#788): select a later point on the existing route, without synthesizing
//! a detour or modifying the OBCR. The screen streams only the highlighted interval, fits a local
//! north-up camera around the rider + whole selected stretch, and queues a durable matcher floor on
//! Press. Hold toggles a rejoin-inspection camera where Turn zooms around the candidate; Back returns
//! to the caller without touching navigation state.

use core::fmt::Write as _;
use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, Surface, Viewport,
};
use obc_route::BBox;

use crate::activity::Activity;
use crate::input::Gesture;
use crate::Msg;

use super::map::{draw_map_scene, SkipMapOverlay};
use super::{Ctx, Prepare, Render, Transition};

/// One encoder detent changes the requested along-route rejoin distance by 200 m. Skip ahead is for
/// nearby closures and trail problems, so finer control matters more than spanning many kilometres;
/// the route-end clamp still displays/commits the exact non-multiple remainder.
pub(crate) const SKIP_STEP_M: u32 = 200;
/// A shorter remainder has no useful later rejoin point and is guarded as "Route ends here".
pub(crate) const MIN_SKIP_M: u32 = 100;
/// Enter inspection at roughly 2.5× the overview scale: enough to resolve the candidate's local
/// junction without making the Hold transition visually disorienting.
const INSPECT_ENTRY_STEPS: u8 = 5;
const INSPECT_MAX_STEPS: u8 = 13;
const INSPECT_ZOOM_STEP: f32 = 1.2;
const HUD_H: i32 = 76;
const HUD_MARGIN: i32 = 10;
const FIT_MARGIN: f32 = 24.0;

#[derive(Debug, Clone, Copy)]
struct PreparedSkip {
    target_m: u32,
    candidate: (i32, i32),
    bounds: BBox,
}

/// Screen-local chooser state. No route geometry is retained: only the entry anchor, encoder step
/// count, compact inspection zoom state, and one prepared coordinate/bounds record live in the
/// screen stack.
#[derive(Debug)]
pub struct SkipAheadScreen {
    route: Option<usize>,
    start_m: u32,
    total_m: u32,
    steps: u16,
    /// `0` = overview/distance adjustment; `1..=INSPECT_MAX_STEPS` = rejoin inspection, with this
    /// many multiplicative zoom detents over the fitted overview.
    inspect_steps: u8,
    prepared: Option<PreparedSkip>,
}

impl SkipAheadScreen {
    pub fn new(activity: &Activity) -> Self {
        SkipAheadScreen {
            route: activity.active_route,
            start_m: activity.progress_m,
            total_m: activity.route_total_m,
            steps: 1,
            inspect_steps: 0,
            prepared: None,
        }
    }

    /// Re-point the chooser's held catalog slot after a live route rescan. A surviving route keeps
    /// the selection by identity; a vanished one becomes unavailable and cannot be committed.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = self.route.and_then(remap);
        self.prepared = None;
    }

    fn actual_skip_m(&self) -> Option<u32> {
        let remaining = self.total_m.saturating_sub(self.start_m);
        (remaining >= MIN_SKIP_M).then_some((self.steps as u32).saturating_mul(SKIP_STEP_M).min(remaining))
    }

    fn target_m(&self) -> Option<u32> {
        self.actual_skip_m().map(|d| self.start_m.saturating_add(d).min(self.total_m))
    }

    fn inspecting(&self) -> bool {
        self.inspect_steps != 0
    }

    fn inspect_zoom(&self) -> f32 {
        let mut zoom = 1.0;
        for _ in 0..self.inspect_steps {
            zoom *= INSPECT_ZOOM_STEP;
        }
        zoom
    }

    fn available(&self, activity: &Activity) -> bool {
        activity.is_tracking()
            && self.route.is_some()
            && activity.active_route == self.route
            && !activity.off_route
            && self.actual_skip_m().is_some()
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
        match g {
            Gesture::Turn(n) if self.available(cx.activity) && self.inspecting() => {
                let next = (self.inspect_steps as i32).saturating_add(n).clamp(1, INSPECT_MAX_STEPS as i32);
                self.inspect_steps = next as u8;
                Transition::None
            }
            Gesture::Turn(n) if self.available(cx.activity) => {
                let remaining = self.total_m.saturating_sub(self.start_m);
                let max_steps = remaining.saturating_add(SKIP_STEP_M - 1) / SKIP_STEP_M;
                let max_steps = max_steps.clamp(1, u16::MAX as u32) as i32;
                let next = (self.steps as i32).saturating_add(n).clamp(1, max_steps);
                self.steps = next as u16;
                self.prepared = None;
                Transition::None
            }
            Gesture::Press if self.available(cx.activity) => {
                // Derive from the current step count here — never from `prepared`, which can still
                // describe the previous frame when Turn and Press arrive in one input drain.
                if let (Some(route), Some(target)) = (self.route, self.target_m()) {
                    cx.activity.request_skip(route, target);
                    Transition::Pop
                } else {
                    Transition::None
                }
            }
            // The encoder hold is unused by the chooser otherwise. Toggle between the spatial
            // overview and a candidate-centred inspection camera without changing the selection.
            Gesture::Hold if self.available(cx.activity) => {
                self.inspect_steps = if self.inspecting() { 0 } else { INSPECT_ENTRY_STEPS };
                Transition::None
            }
            // Cancel consumes both chooser and ride-menu caller, restoring the riding view without
            // queueing a floor or changing progress/session/mode.
            Gesture::Back => Transition::Pop,
            Gesture::Turn(_) | Gesture::Press | Gesture::Hold | Gesture::BackHold => Transition::None,
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
        self.prepared = Some(PreparedSkip { target_m, candidate: (candidate.lon, candidate.lat), bounds });
    }

    pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut Render)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let selected = self.prepared.filter(|_| self.available(rx.activity));
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
        let overlay =
            selected.map(|p| SkipMapOverlay { start_m: self.start_m, end_m: p.target_m, candidate: p.candidate });
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
        let title = if self.inspecting() { rx.t(Msg::RideMenuInspectRejoin) } else { rx.t(Msg::RideMenuSkipAhead) };
        cv.text(title, Point::new(rx.w / 2, y + 7), Font::Label, TextAlign::Center, INK);

        let status = if self.route.is_none() || rx.activity.active_route != self.route {
            Err(rx.t(Msg::RideMenuNoRoute))
        } else if rx.activity.off_route {
            Err(rx.t(Msg::RideMenuOffRoute))
        } else if let Some(m) = self.actual_skip_m() {
            Ok(crate::stat_fields::fmt_dist_short(m, rx.settings.units))
        } else {
            Err(rx.t(Msg::RideMenuRouteEnd))
        };
        match status {
            Ok(dist) => {
                let mut readout = heapless::String::<24>::new();
                if self.inspecting() {
                    let _ = write!(readout, "{} {:.1}x", dist.as_str(), self.inspect_zoom());
                } else {
                    let _ = readout.push_str(dist.as_str());
                }
                cv.text("-", Point::new(x + 24, y + 36), Font::Display, TextAlign::Center, INK);
                cv.text(readout.as_str(), Point::new(rx.w / 2, y + 36), Font::Display, TextAlign::Center, WARNING);
                cv.text("+", Point::new(x + w - 24, y + 36), Font::Display, TextAlign::Center, INK);
            }
            Err(msg) => {
                cv.text(msg, Point::new(rx.w / 2, y + 40), Font::Label, TextAlign::Center, WARNING);
            }
        }
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

    fn with_ctx<T>(activity: &mut Activity, f: impl FnOnce(&mut Ctx) -> T) -> T {
        let mut state = AppState::new(0, 0, 1.0);
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
        let mut s = SkipAheadScreen::new(&a);
        assert_eq!(s.actual_skip_m(), Some(200));
        s.steps = 9;
        assert_eq!(s.actual_skip_m(), Some(1_150));
        assert_eq!(s.target_m(), Some(1_350));
    }

    #[test]
    fn near_end_has_no_rejoin_candidate() {
        let mut a = tracking_activity(950, 1_000);
        a.active_route = Some(0);
        assert_eq!(SkipAheadScreen::new(&a).actual_skip_m(), None);
    }

    #[test]
    fn turn_then_press_without_prepare_commits_the_current_requested_target() {
        let mut a = tracking_activity(1_000, 5_000);
        let session = a.session();
        let mode = a.mode;
        let mut s = SkipAheadScreen::new(&a);
        with_ctx(&mut a, |cx| assert!(matches!(s.handle(Gesture::Turn(2), cx), Transition::None)));
        let t = with_ctx(&mut a, |cx| s.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop));
        let req = a.pending_skip().expect("commit queued");
        assert_eq!((req.route, req.target_m), (2, 1_600), "three 200 m steps from the live anchor");
        assert_eq!(a.progress_m, 1_000, "visible progress waits for the atomic route-aware seek");
        assert_eq!(a.session(), session, "same tracking session");
        assert_eq!(a.mode, mode, "Mode is preserved");
    }

    #[test]
    fn back_cancels_to_map_without_any_navigation_change() {
        let mut a = tracking_activity(800, 4_000);
        let before = a;
        let mut s = SkipAheadScreen::new(&a);
        with_ctx(&mut a, |cx| {
            let _ = s.handle(Gesture::Turn(3), cx);
        });
        let t = with_ctx(&mut a, |cx| s.handle(Gesture::Back, cx));
        assert!(matches!(t, Transition::Pop));
        assert_eq!(a.progress_m, before.progress_m);
        assert_eq!(a.session(), before.session());
        assert_eq!(a.mode, before.mode);
        assert!(a.pending_skip().is_none());
    }

    #[test]
    fn route_less_off_route_and_near_end_press_are_guarded() {
        let mut route_less = tracking_activity(0, 0);
        route_less.active_route = None;
        let mut s = SkipAheadScreen::new(&route_less);
        assert!(matches!(with_ctx(&mut route_less, |cx| s.handle(Gesture::Press, cx)), Transition::None));

        let mut off = tracking_activity(100, 2_000);
        off.off_route = true;
        let mut s = SkipAheadScreen::new(&off);
        assert!(matches!(with_ctx(&mut off, |cx| s.handle(Gesture::Press, cx)), Transition::None));

        let mut end = tracking_activity(1_950, 2_000);
        let mut s = SkipAheadScreen::new(&end);
        assert!(matches!(with_ctx(&mut end, |cx| s.handle(Gesture::Press, cx)), Transition::None));
        assert!(route_less.pending_skip().is_none() && off.pending_skip().is_none() && end.pending_skip().is_none());
    }

    #[test]
    fn moving_while_open_advances_highlight_and_commit_anchor() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut s = SkipAheadScreen::new(&a);
        // Rider advances before a Turn; that input refreshes the live anchor and selects 400 m.
        a.progress_m = 1_200;
        with_ctx(&mut a, |cx| {
            let _ = s.handle(Gesture::Turn(1), cx);
        });
        assert_eq!((s.start_m, s.target_m()), (1_200, Some(1_600)));
        // Another 100 m before Press: commit is still a 400 m skip, now from 1.3 km.
        a.progress_m = 1_300;
        let t = with_ctx(&mut a, |cx| s.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop));
        assert_eq!(a.pending_skip().unwrap().target_m, 1_700);
    }

    #[test]
    fn hold_toggles_candidate_inspection_and_turn_changes_only_zoom() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut s = SkipAheadScreen::new(&a);
        let target = s.target_m();

        assert!(matches!(with_ctx(&mut a, |cx| s.handle(Gesture::Hold, cx)), Transition::None));
        assert!(s.inspecting());
        assert_eq!(s.inspect_steps, INSPECT_ENTRY_STEPS);
        let entry_zoom = s.inspect_zoom();

        let _ = with_ctx(&mut a, |cx| s.handle(Gesture::Turn(2), cx));
        assert!(s.inspect_zoom() > entry_zoom, "Turn zooms in while inspecting");
        assert_eq!(s.target_m(), target, "inspection never changes the selected rejoin point");

        let _ = with_ctx(&mut a, |cx| s.handle(Gesture::Hold, cx));
        assert!(!s.inspecting(), "a second Hold returns to the overview");
    }

    #[test]
    fn press_from_inspection_commits_the_unchanged_candidate() {
        let mut a = tracking_activity(1_000, 5_000);
        let mut s = SkipAheadScreen::new(&a);
        let target = s.target_m().unwrap();
        let _ = with_ctx(&mut a, |cx| s.handle(Gesture::Hold, cx));
        let _ = with_ctx(&mut a, |cx| s.handle(Gesture::Turn(3), cx));

        let t = with_ctx(&mut a, |cx| s.handle(Gesture::Press, cx));
        assert!(matches!(t, Transition::Pop));
        assert_eq!(a.pending_skip().unwrap().target_m, target);
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
