//! The Map screen, the Riding view. The camera lives in [`AppState`](crate::AppState); the screen
//! itself holds only a [`MinuteTicker`] for the clock overlay and the browse-map start hint. `draw`
//! renders the base map plus the route, travel chevrons, breadcrumb, user marker, and the map
//! chrome: the clock digits, one bottom chip slot, a bottom-left scale bar, a low-battery cue, the
//! effort band along the bottom edge and the pan HUD.
//!
//! Bindings depend on whether a ride is being tracked. While tracking, `press` pauses into Ride
//! control and `back` swaps to the sibling Statistics view. On the route-less browse map, `press`
//! opens the start card and `back` pops to the Menu, because there is no Statistics sibling without
//! a ride. Off-route chrome cannot fire without a route, so the browse map shows only the clock,
//! the scale bar and the low-battery cue.

use core::fmt::Write as _;

use embedded_graphics::{
    draw_target::DrawTarget,
    prelude::{Point, Size},
    primitives::Rectangle,
};
use obc_render::{
    rect, round_coord,
    text::{text_width, Font, TextAlign},
    Canvas, Surface, Viewport,
};
use obc_route::WptEntry;

use crate::app::{step_zoom, Pan, PanBasis, PanTool, MAX_ZOOM, MIN_ZOOM};
use crate::effort::Gauge;
use crate::input::Gesture;
use crate::settings::{DateTime, Units, WaypointMode};
use crate::wall_clock::MinuteTicker;
use crate::Msg;
use obc_ports::Fix;

use super::vocab::marquee::fit;
use super::{Ctx, RenderFrame, Screen, ScreenTick, StatisticsScreen, Transition};

pub(crate) mod placement;

use placement::PointPlacement;

/// Fallback backdrop when a map carries no backdrop style.
const DEFAULT_BG_RGB565: u16 = 0x2104;

/// Stroke width (px) of the active-route overlay. Bold enough to out-weigh the heaviest base road,
/// and sized so a direction chevron sits inside the line at riding zoom.
pub const ROUTE_WEIGHT: u32 = 11;
/// Narrower stroke painted inside the route line for the chooser's to-be-skipped stretch, so the
/// magenta edges stay visible around the warning-orange interval.
const SKIPPED_WEIGHT: u32 = 7;

/// Colour of the route direction chevrons — white, for contrast over the magenta route line. Drawn
/// only at riding zoom (see [`CHEVRON_MAX_MPP`]).
const ARROW_COLOR: u16 = super::palette::PARCHMENT;

/// Zoom threshold (ground metres per pixel) at or below which the chevrons draw. A scale gate,
/// independent of the map's LOD pyramid.
const CHEVRON_MAX_MPP: f32 = 4.0;

/// Stroke width (px) of the breadcrumb. Thinner than the route, so the route stays dominant where
/// the two coincide.
const BREADCRUMB_WEIGHT: u32 = 3;

/// The recorded track stays blue in both map presentations, but the dark map uses the bright
/// device-blue step so it remains distinct from the black backdrop and the magenta route.
fn breadcrumb_color(theme: crate::settings::Theme) -> u16 {
    match theme {
        crate::settings::Theme::Light => super::palette::BREADCRUMB,
        crate::settings::Theme::Dark => super::palette::rgb565(0, 170, 255),
    }
}

fn waypoint_color(theme: crate::settings::Theme) -> u16 {
    match theme {
        crate::settings::Theme::Light => super::palette::INK,
        crate::settings::Theme::Dark => super::palette::PARCHMENT,
    }
}

/// Half-diagonal (px) of a waypoint diamond. No zoom gate, unlike the chevrons: the resident table
/// is at most `MAX_WAYPOINTS`, so even a wide overview shows only a handful of anchors.
const WAYPOINT_DIAMOND_R: i32 = 4;

/// The live map, or Follow view. The camera is the shared [`AppState`](crate::AppState).
#[derive(Debug, Default)]
pub struct MapScreen {
    /// Fires a region-clipped repaint of the clock digits each minute the wall clock rolls over, so
    /// `HH:MM` advances without a full map redraw.
    ticker: MinuteTicker,
    /// The route-less browse map's one-shot "press to start a ride" hint.
    hint: BrowseHint,
}

/// How long the browse-map start hint stays up after entry, in milliseconds.
const HINT_MS: u32 = 4_000;

/// The browse map's one-shot start-hint chip state. A fresh `MapScreen` starts
/// [`Fresh`](BrowseHint::Fresh) and its first [`tick`](BrowseHint::tick) classifies it: a browse map
/// starts the timer, a riding map goes straight to [`Done`](BrowseHint::Done). Re-entering the
/// browse map is a fresh `MapScreen`, so the hint returns; a pop back from the start card is the
/// same instance, so it does not.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum BrowseHint {
    /// Not yet classified (a just-constructed screen).
    #[default]
    Fresh,
    /// Up, armed at this boot-relative millisecond.
    Showing(u32),
    /// Expired or suppressed (a riding map) — never shows again on this instance.
    Done,
}

impl BrowseHint {
    /// Advance the hint one poll against `now_ms`. Arms on the first poll of a browse map,
    /// suppresses on a riding map, and expires [`HINT_MS`] after arming. `changed` is the single
    /// repaint that clears the chip; the residual wake keeps an event-driven host armed to fire it.
    fn tick(&mut self, now_ms: u32, tracking: bool) -> (bool, Option<u32>) {
        match *self {
            BrowseHint::Fresh if tracking => {
                *self = BrowseHint::Done;
                (false, None)
            }
            BrowseHint::Fresh => {
                *self = BrowseHint::Showing(now_ms);
                (false, Some(HINT_MS))
            }
            BrowseHint::Showing(since) => {
                let elapsed = now_ms.wrapping_sub(since);
                if elapsed >= HINT_MS {
                    *self = BrowseHint::Done;
                    (true, None)
                } else {
                    (false, Some(HINT_MS - elapsed))
                }
            }
            BrowseHint::Done => (false, None),
        }
    }

    /// Whether the chip draws this frame. The caller still gates on tracking, panning and the
    /// higher-priority chips.
    fn chip_up(self) -> bool {
        matches!(self, BrowseHint::Fresh | BrowseHint::Showing(_))
    }
}

impl MapScreen {
    pub fn new() -> Self {
        MapScreen::default()
    }

    /// Poll the Map's two timed tenants: the clock overlay's minute tick and the browse hint. With
    /// the clock visible, a minute rollover self-dirties just the [`clock_region`], so the host
    /// clips the repaint to it and the map plane is not re-rendered. With the pill hidden the
    /// minute wake is not armed at all, so a parked map is not woken to no purpose.
    #[allow(clippy::too_many_arguments)] // two timed tenants in one poll
    pub fn tick_timers(
        &mut self,
        now_ms: u32,
        now: DateTime,
        ms_to_next_minute: u32,
        w: i32,
        pan_active: bool,
        map_clock: bool,
        tracking: bool,
    ) -> ScreenTick {
        // Hidden, the minute is still observed, so a later show does not fire a stale rollover.
        let (clk_changed, clk_wake, clk_region) = if !clock_cue(map_clock, pan_active) || w == 0 {
            let _ = self.ticker.changed(now);
            (false, None, None)
        } else {
            (self.ticker.changed(now), Some(ms_to_next_minute), Some(clock_region(w)))
        };
        // The hint's expiry repaints the chip band and steps the scale bar back down, so it is a
        // full-frame change, unlike the clock's region-clipped digit tick.
        let (hint_changed, hint_wake) = self.hint.tick(now_ms, tracking);
        let next_wake_ms = match (clk_wake, hint_wake) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        ScreenTick {
            changed: clk_changed || hint_changed,
            next_wake_ms,
            region: if hint_changed { None } else { clk_region },
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        // Pan mode is a sub-mode of the Map: while the shared camera holds a `pan`, Select and Back
        // drive panning instead of the Follow bindings below.
        if cx.state.pan.is_some() {
            return handle_pan(g, cx);
        }
        match g {
            Gesture::Step(n) => {
                cx.state.zoom = step_zoom(cx.state.zoom, n, MIN_ZOOM, MAX_ZOOM);
                Transition::None
            }
            // Enter Pan mode: the camera detaches and the pan HUD appears.
            Gesture::Hold => {
                let navigation = cx.navigator.route_state();
                cx.state.enter_pan(navigation.active_route.is_some(), navigation.progress_m);
                Transition::None
            }
            // The Map and Statistics ring only exists mid-ride, so on the browse map `back` has no
            // sibling to swap to and pops to the Menu instead.
            Gesture::Back if cx.recorder.recording() => {
                Transition::Replace(Screen::Statistics(StatisticsScreen::new()))
            }
            Gesture::Back => Transition::Pop,
            Gesture::Press if !cx.recorder.recording() => {
                Transition::Push(Screen::RideStart(super::RideStartScreen::new()))
            }
            Gesture::Press => super::riding_common(g, cx),
            Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, '_>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let vp = rx.state.viewport(rx.w as f32, rx.h as f32);
        let panning = rx.state.pan.is_some();
        // The effort band takes the bottom rows, so the bottom chrome stands on top of it.
        let gauge = gauge_cue(rx.recorder, rx.settings.effort_limits(), panning, rx.w);
        let h = if gauge.is_some() { rx.h - GAUGE_H } else { rx.h };

        // Which bottom chip is up, and therefore where the scale bar sits, is decided before the
        // scene draws: the settlement names inside it reserve the bar's live box, not every box it
        // could take. The pills themselves still draw below, over the map.
        let warning_up = !panning && (rx.no_fix || rx.navigation.off_route);
        let wpt_chip = waypoint_chip(
            rx.settings.waypoint_mode,
            panning,
            rx.no_fix,
            rx.navigation.off_route,
            rx.navigation.next_waypoint,
            rx.waypoints.as_slice(),
            rx.navigation.progress_m,
        );
        let hint_up = !rx.recording && !panning && !warning_up && wpt_chip.is_none() && self.hint.chip_up();
        let chip_band = if warning_up || wpt_chip.is_some() {
            CHIP_H
        } else if hint_up {
            hint_chip_height(rx.t(Msg::MapPressToStart))
        } else {
            0
        };
        let scale_bar = if rx.settings.map_scale_bar {
            ScaleBar::new(h, chip_band, vp.meters_per_pixel(), rx.settings.units)
        } else {
            None
        };

        // What this frame inks over the map after the names. Each piece is gated here by the same
        // condition that draws it below, so the box that is reserved and the pixels that are drawn
        // cannot drift.
        let clock_up = clock_cue(rx.settings.map_clock, panning);
        let low_battery = low_battery_cue(rx.state.device.battery_pct);
        let pan_hud = pan_hud_boxes(rx.w, rx.h, rx.state.pan, &vp, rx.state.user_fix);
        let band = gauge.map(|_| gauge_region(rx.w, rx.h));
        let chrome = map_chrome(rx.w, h, chip_band, scale_bar.as_ref(), &pan_hud, clock_up, low_battery, band);
        let Some(marker565) = draw_map_scene(cv, rx, &vp, None, &chrome) else {
            // A frame with no map is the opaque effort band's own repaint.
            if let Some(g) = gauge {
                draw_gauge(cv, rx.w, rx.h, g);
            }
            return;
        };

        // The remaining chrome draws in the palette vocabulary, back through the canvas. The
        // low-battery cue shows in pan mode too, because the top centre belongs to the pan HUD.
        if low_battery {
            draw_low_battery(cv);
        }

        if clock_up {
            draw_clock(cv, rx.w, rx.now);
        }

        // "No GPS Fix" takes priority over off-route: with no fix the match is stale, so the
        // cross-track distance is meaningless.
        if warning_up {
            if rx.no_fix {
                draw_status_chip(cv, rx.w, h, rx.t(Msg::MapNoGpsFix));
            } else if rx.navigation.dist_to_route_m == u32::MAX {
                draw_status_chip(cv, rx.w, h, rx.t(Msg::MapOffRoute).trim_end());
            } else {
                let mut s: heapless::String<20> = heapless::String::new();
                super::vocab::fmt::write_distance_coarse(
                    &mut s,
                    rx.t(Msg::MapOffRoute),
                    rx.navigation.dist_to_route_m,
                    rx.settings.units,
                );
                draw_status_chip(cv, rx.w, h, &s);
            }
        }

        // The warning chip keeps the bottom slot: `waypoint_chip` reports a chip only when the
        // warning chip is down, so the two never collide.
        if let Some((k, dist_to_go)) = wpt_chip {
            let dist = super::vocab::fmt::distance_short(dist_to_go, rx.settings.units);
            draw_waypoint_chip(cv, rx.w, h, rx.waypoints.as_slice()[k].name.as_str(), &dist);
        }

        // The lowest-priority bottom chip: dropped whenever a warning or waypoint chip wants the
        // slot. Its own timer drives the auto-hide; this only reads its state.
        if hint_up {
            draw_hint_chip(cv, rx.w, h, rx.t(Msg::MapPressToStart));
        }

        // The bar steps above the chip band while a bottom chip is up, so a wide chip never runs
        // under it. Visible in pan mode too: the pan HUD's bottom chevron is centred, clear of the
        // corner.
        if let Some(bar) = &scale_bar {
            draw_scale_bar(cv, bar);
        }

        if let Some(g) = gauge {
            draw_gauge(cv, rx.w, rx.h, g);
        }

        // Drawn last, so the HUD sits over the map and the marker.
        if let Some(pan) = rx.state.pan {
            draw_pan_hud(cv, (rx.w as f32, rx.h as f32), pan, rx.state.user_fix, marker565, &vp);
        }
    }
}

/// Optional detour ink added to the shared map scene: the skipped-span interval and rejoin
/// candidate the chooser owns, plus the planned detour's decimated polyline on the preview screen.
/// The normal Map passes `None` and pays no extra route decode.
#[derive(Clone, Copy)]
pub(crate) struct DetourMapOverlay<'a> {
    pub start_m: u32,
    pub end_m: u32,
    pub candidate: (i32, i32),
    /// The planned detour's decimated polyline. Empty on the chooser, where nothing is planned.
    pub detour: &'a [(i32, i32)],
}

/// Draw the reusable map scene (base map, full route, optional skipped-range + detour ink,
/// breadcrumb, waypoints, rider and candidate). Map chrome stays in [`MapScreen::draw`]; the
/// Detour chooser/preview add their own floating HUD after this returns.
///
/// `chrome` is what this screen will ink over the map — its header, its bottom panel or pill, and
/// the scale bar ([`ScaleBar::ink`]) — so a settlement name keeps off it. A screen that draws no
/// chrome passes an empty slice and the names get the whole panel.
pub(crate) fn draw_map_scene<D, F>(
    cv: &mut Canvas<D, F>,
    rx: &mut RenderFrame<'_, '_>,
    vp: &Viewport,
    skip: Option<DetourMapOverlay<'_>>,
    chrome: &[Rectangle],
) -> Option<u16>
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    // Map styles and their labels are authored RGB565. They may equal a UI role by chance, so the
    // UI theme must not reinterpret them. Overlays drawn after this function use the theme again.
    let scene = rx.scene?;
    cv.set_color_policy_enabled(false);
    let rx = &mut rx.render;
    // The only reader of the host's lent scratch, which is why every other screen may be drawn
    // with `None`. Reaching here without one means the host called a render entry point with `None`
    // under a map-drawing base, so skip the map rather than invent one.
    let Some(scratch) = rx.scratch.as_deref_mut() else {
        debug_assert!(false, "a map-drawing base needs the host's RenderScratch, but none was lent");
        cv.set_color_policy_enabled(true);
        return None;
    };
    let mut stats = render_base(cv, scene, scratch, vp, rx.settings.map_contours, rx.clock);
    // One occupancy list for the frame. The icons take their space here and the settlement names
    // take what is left, which is the whole priority rule between the two overlays.
    let reserved = label_reserved(vp, rx.state.user_fix, rx.waypoints.as_slice(), chrome);
    let mut place = PointPlacement::new(&reserved);
    rx.map_icons.draw(cv, vp, &mut place);
    let (target, color_fn) = cv.split();
    let arrows_at = (skip.is_none() && vp.meters_per_pixel() <= CHEVRON_MAX_MPP).then_some(rx.navigation.progress_m);

    if let Some(route) = rx.route {
        let (route_chunks, route_points, route_points_drawn) = scratch.draw_route(
            target,
            vp,
            &crate::route::RouteOverlay(route),
            color_fn(super::palette::ROUTE),
            ROUTE_WEIGHT,
            color_fn(ARROW_COLOR),
            arrows_at,
        );
        stats.route_chunks = route_chunks;
        stats.route_points = route_points;
        stats.route_points_drawn = route_points_drawn;

        if let Some(selected) = skip {
            route.visit_points_between(selected.start_m, selected.end_m, |pts| {
                scratch.stroke_path(target, vp, pts.iter().copied(), color_fn(super::palette::WARNING), SKIPPED_WEIGHT);
            });
            // Blue, so the replanned portion reads apart from the magenta route it replaces and
            // the warning-coloured span it avoids.
            if selected.detour.len() >= 2 {
                scratch.stroke_path(
                    target,
                    vp,
                    selected.detour.iter().copied(),
                    color_fn(super::palette::DETOUR),
                    ROUTE_WEIGHT,
                );
            }
        }
    }

    if !rx.breadcrumb.is_empty() {
        let trail = color_fn(breadcrumb_color(rx.settings.theme));
        scratch.stroke_path(target, vp, rx.breadcrumb.points(), trail, BREADCRUMB_WEIGHT);
    }
    rx.stats = stats;

    // Settlement names: over the terrain and the route ink, under the waypoints and the rider.
    crate::settlements::draw_labels(cv, vp, rx.settlements, &mut place);

    draw_waypoint_diamonds(cv, vp, rx.waypoints.as_slice(), rx.w, rx.h, waypoint_color(rx.settings.theme));
    let marker565 = if rx.navigation.off_route { super::palette::WARNING } else { scene.marker_color };
    if let Some(fix) = rx.state.user_fix {
        let (target, color_fn) = cv.split();
        scratch.draw_marker(target, vp, fix.lon, fix.lat, fix.course, color_fn(marker565));
    }
    if let Some(selected) = skip {
        let (x, y) = vp.to_screen(selected.candidate.0, selected.candidate.1);
        let c = Point::new(x, y);
        cv.disc(c, 10, super::palette::PARCHMENT);
        cv.disc(c, 7, super::palette::WARNING);
        cv.disc(c, 3, super::palette::INK);
    }
    cv.set_color_policy_enabled(true);
    Some(marker565)
}

/// Stroke width (px) of a previewed track. The whole route fits one small band, so the map's
/// riding-zoom [`ROUTE_WEIGHT`] would swallow the roads under it.
const TRACK_WEIGHT: u32 = 4;

/// Draw the base map with one track stroked over it, and nothing else: no rider, names, icons or
/// chevrons. It is the backdrop of a route or ride preview. The render clears the whole target
/// first. Returns `false`, having drawn nothing, when the frame streams no map.
pub(crate) fn draw_track_scene<D, F>(
    cv: &mut Canvas<D, F>,
    rx: &mut RenderFrame<'_, '_>,
    vp: &Viewport,
    track: &[(i32, i32)],
    color: u16,
) -> bool
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let Some(scene) = rx.scene else { return false };
    let rx = &mut rx.render;
    let Some(scratch) = rx.scratch.as_deref_mut() else {
        debug_assert!(false, "a map-drawing base needs the host's RenderScratch, but none was lent");
        return false;
    };
    cv.set_color_policy_enabled(false);
    rx.stats = render_base(cv, scene, scratch, vp, rx.settings.map_contours, rx.clock);
    let (target, color_fn) = cv.split();
    scratch.stroke_path(target, vp, track.iter().copied(), color_fn(color), TRACK_WEIGHT);
    cv.set_color_policy_enabled(true);
    true
}

/// Clear the target to the map's backdrop and render the base map through `vp`.
fn render_base<D, F>(
    cv: &mut Canvas<D, F>,
    scene: &obc_reader::Reader<'_>,
    scratch: &mut obc_render::RenderScratch,
    vp: &Viewport,
    contours: bool,
    clock: &dyn obc_render::Clock,
) -> obc_render::RenderStats
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let bg565 = scene.backdrop_style().map_or(DEFAULT_BG_RGB565, |style| style.color);
    let (target, color_fn) = cv.split();
    let bg = color_fn(bg565);
    // The rider's contour switch, restated every frame from `Settings` rather than held in the
    // scratch, so a flip lands on the next map frame with no reload and nothing to reset.
    // Suppression drops the terrain layer in the collect pass, so nothing is decoded.
    let cfg = obc_render::RenderConfig { terrain_layer: contours };
    scratch.render_timed(target, scene, vp, bg, cfg, color_fn, clock)
}

/// Side (px) of the box the rider mark owns. The chevron reaches 12 px ahead and 8 px out.
const RIDER_BOX_PX: i32 = 24;

/// The most chrome boxes one screen hands to [`label_reserved`]. Two Map frames are the widest, at
/// five. A panning frame holds the pan HUD's three, the low-battery cue and the scale bar; pan mode
/// suppresses the clock, every bottom pill and the effort band. An attached frame holds the clock,
/// the low-battery cue, a bottom pill, the scale bar and the effort band. No other screen asks for
/// more.
pub(crate) const MAX_CHROME: usize = 5;

/// A screen's own chrome boxes, as [`label_reserved`] takes them.
pub(crate) type Chrome = heapless::Vec<Rectangle, MAX_CHROME>;

/// The boxes [`label_reserved`] can return: a screen's chrome, the rider mark, and one diamond for
/// each resident waypoint.
pub(crate) const RESERVED: usize = MAX_CHROME + 1 + obc_route::MAX_WAYPOINTS;

/// The boxes the map draws over its own point marks, so no mark covers one. A mark may sit flush
/// against a box, so each one has to hold the ink it protects.
///
/// Every box a screen inks over the map comes in through `chrome`, measured for this frame: a
/// piece of chrome the frame does not draw reserves nothing, top edge and bottom alike. The rider
/// mark and the waypoint diamonds are the boxes this function measures itself, because both draw
/// after the marks and would else overprint them.
pub(crate) fn label_reserved(
    vp: &Viewport,
    fix: Option<Fix>,
    wpts: &[WptEntry],
    chrome: &[Rectangle],
) -> heapless::Vec<Rectangle, RESERVED> {
    // Past the capacity a push is dropped, and a dropped rider box is a name over the rider.
    debug_assert!(chrome.len() <= MAX_CHROME, "a screen handed more chrome than MAX_CHROME");
    let mut boxes = heapless::Vec::new();
    for r in chrome {
        let _ = boxes.push(*r);
    }
    if let Some(fix) = fix {
        let (x, y) = vp.to_screen(fix.lon, fix.lat);
        let r = RIDER_BOX_PX / 2;
        if boxes.push(rect(x - r, y - r, RIDER_BOX_PX, RIDER_BOX_PX)).is_err() {
            debug_assert!(false, "the rider box was dropped, so a name may cover the rider");
        }
    }
    for wp in wpts {
        let (x, y) = vp.to_screen(wp.lon, wp.lat);
        if waypoint_diamond(x, y, vp.w as i32, vp.h as i32).is_some() {
            let r = WAYPOINT_DIAMOND_R;
            // The vertices are at `-r` and `+r`, both ends, so the ink is `2r + 1` wide.
            let _ = boxes.push(rect(x - r, y - r, 2 * r + 1, 2 * r + 1));
        }
    }
    boxes
}

/// The four screen vertices `(top, bottom, left, right)` of a waypoint diamond centred at
/// `(cx, cy)`, or `None` when the centre lies more than one half-diagonal outside the `w`×`h`
/// panel. A diamond straddling an edge still draws; one wholly past it is dropped.
fn waypoint_diamond(cx: i32, cy: i32, w: i32, h: i32) -> Option<(Point, Point, Point, Point)> {
    let r = WAYPOINT_DIAMOND_R;
    if cx < -r || cx > w + r || cy < -r || cy > h + r {
        return None;
    }
    Some((Point::new(cx, cy - r), Point::new(cx, cy + r), Point::new(cx - r, cy), Point::new(cx + r, cy)))
}

/// Draw the route's named waypoints as small filled diamonds at each entry's own (`lon`, `lat`).
/// That is the stored coordinate, not snapped to the polyline, so a diamond may sit slightly off
/// it. The table is empty with no route loaded, so this is then a no-op.
fn draw_waypoint_diamonds(cv: &mut impl Surface, vp: &Viewport, wpts: &[WptEntry], w: i32, h: i32, color: u16) {
    for wp in wpts {
        let (sx, sy) = vp.to_screen(wp.lon, wp.lat);
        if let Some((top, bottom, left, right)) = waypoint_diamond(sx, sy, w, h) {
            cv.triangle(top, left, right, color);
            cv.triangle(bottom, left, right, color);
        }
    }
}

/// Pan-mode gesture bindings, active while [`AppState::pan`](crate::AppState::pan) is `Some`.
/// Up/Down applies the active Move or Zoom tool; `press` advances the mode ring; Select-hold
/// changes only an already-active Free axis and is inert in Route and Zoom. `back` exits to Follow.
pub(super) fn handle_pan(g: Gesture, cx: &mut Ctx) -> Transition {
    let has_route = cx.navigator.route_state().active_route.is_some();
    match g {
        Gesture::Step(n) => cx.state.pan_step(n, cx.navigator.route_state().route_total_m),
        Gesture::Press => cx.state.cycle_pan_mode(has_route),
        Gesture::Hold => cx.state.toggle_pan_free_axis(),
        Gesture::Back => cx.state.exit_pan(),
        Gesture::BackHold => {}
    }
    Transition::None
}

/// A compact bottom-centre status chip, the one warning slot on the map. The caller owns the
/// priority rule of what to say. Warning-orange, so it reads as an alert.
fn draw_status_chip(cv: &mut impl Surface, w: i32, h: i32, s: &str) {
    use super::palette::*;
    let font = Font::Body;
    let tw = text_width(s, font) as i32;
    let (pw, ph) = (tw + 28, CHIP_H);
    let px = (w - pw) / 2;
    let py = h - CHIP_H - CHIP_MARGIN;
    cv.round(rect(px, py, pw, ph), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, ph), 9, WARNING);
    cv.text(s, Point::new(w / 2, py + 5), font, TextAlign::Center, WARNING);
}

/// The per-line pitch inside the multi-line hint pill ([`Font::Label`] lines).
const HINT_LINE_PITCH: i32 = 22;
/// Padding between the pill edge and the first/last visible text pixels.
const HINT_PAD_Y: i32 = 8;

/// The browse-map start hint pill: calm ink on parchment, because warning-orange stays reserved for
/// the alert chip. The sentence wraps to two centred lines, because it cannot fit one line at 240 px
/// in even the smallest font. The pill height derives from the wrapped ink, so accents and
/// descenders are inside it.
fn draw_hint_chip(cv: &mut impl Surface, w: i32, h: i32, s: &str) {
    use super::palette::*;
    let font = Font::Label;
    let (l1, l2) = wrap2(s);
    let ink = hint_chip_ink(s);
    let ph = ink.end - ink.start + 2 * HINT_PAD_Y;
    let tw = (text_width(l1, font) as i32).max(text_width(l2, font) as i32);
    let pw = tw + 16;
    let px = (w - pw) / 2;
    let py = h - ph - CHIP_MARGIN;
    cv.round(rect(px, py, pw, ph), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, ph), 9, INK);
    let ty = py + HINT_PAD_Y - ink.start;
    cv.text(l1, Point::new(w / 2, ty), font, TextAlign::Center, INK);
    if !l2.is_empty() {
        cv.text(l2, Point::new(w / 2, ty + HINT_LINE_PITCH), font, TextAlign::Center, INK);
    }
}

/// The ink rows the hint pill's two wrapped lines cover, relative to the first line's cell top.
fn hint_chip_ink(s: &str) -> core::ops::Range<i32> {
    let font = Font::Label;
    let (l1, l2) = wrap2(s);
    let mut ink = obc_render::text::text_ink_bounds(l1, font).unwrap_or(0..0);
    if let Some(second) = obc_render::text::text_ink_bounds(l2, font) {
        ink.start = ink.start.min(second.start + HINT_LINE_PITCH);
        ink.end = ink.end.max(second.end + HINT_LINE_PITCH);
    }
    ink
}

/// The hint pill's height — its wrapped ink plus the padding. Pure, so the scale bar knows the band
/// it steps above before the pill draws.
fn hint_chip_height(s: &str) -> i32 {
    let ink = hint_chip_ink(s);
    ink.end - ink.start + 2 * HINT_PAD_Y
}

/// Split `s` into two balanced centred lines for the hint pill: the word break whose first line is
/// closest to half the string, so neither line orphans a single word. A string with no space falls
/// through as one line. Character count is a good proxy for width, because the font is monospace.
fn wrap2(s: &str) -> (&str, &str) {
    let mid = s.chars().count() as i32 / 2;
    let mut best: Option<(usize, i32)> = None; // (byte index of the space, |line1_len - mid|)
    for (idx, (b, ch)) in s.char_indices().enumerate() {
        if ch == ' ' {
            let d = (idx as i32 - mid).abs();
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((b, d));
            }
        }
    }
    match best {
        Some((b, _)) => (&s[..b], &s[b + 1..]),
        None => (s, ""),
    }
}

/// The status chip's band height and its inset from the bottom edge.
pub(crate) const CHIP_H: i32 = 36;
const CHIP_MARGIN: i32 = 10;

/// The full-width band a bottom pill owns. One box for all three pills, at the tallest any of them
/// reaches: a first line inked from the top of its cell and a second inked to the bottom of the
/// next, which is taller than any real pair in any language.
pub(crate) fn chip_band_box(w: i32, h: i32) -> Rectangle {
    let band = HINT_LINE_PITCH + Font::Label.line_height() as i32 + 2 * HINT_PAD_Y + CHIP_MARGIN;
    rect(0, h - band, w, band)
}

/// The Up/Down cue boxes the pan HUD inks over the map. Zoom draws its plus and minus at the top
/// and bottom; Free draws chevrons at the two edges of its axis; Route draws none, because the
/// moving route is its own feedback. Never more than one pair, so two boxes is the exact bound.
fn pan_cue_boxes(w: i32, h: i32, pan: Pan) -> heapless::Vec<Rectangle, 2> {
    use hud::*;
    let r = (CHEV_SPREAD + CHEV_HW + OUTLINE) as i32;
    let inset = CHEV_INSET as i32;
    let cue = |x: i32, y: i32| rect(x - r, y - r, 2 * r, 2 * r);
    let mut boxes = heapless::Vec::new();
    if pan.tool == PanTool::Zoom || pan.basis == PanBasis::Vertical {
        let _ = boxes.push(cue(w / 2, inset));
        let _ = boxes.push(cue(w / 2, h - inset));
    } else if pan.basis == PanBasis::Horizontal {
        let _ = boxes.push(cue(inset, h / 2));
        let _ = boxes.push(cue(w - inset, h / 2));
    }
    boxes
}

/// The box the back-to-you marker inks, or `None` while the rider is on the panel and no marker
/// draws. It is the bounds of the outlined arrow's own three vertices, so it turns with the
/// rider's bearing instead of assuming a square around the centre.
fn back_to_you_box(w: f32, h: f32, vp: &Viewport, fix: Fix) -> Option<Rectangle> {
    use hud::*;
    let (x, y, ux, uy) = back_to_you(w, h, vp, fix)?;
    let [a, b, c] = arrow_vertices((x, y), (ux, uy), (BACK_H + OUTLINE, BACK_W + OUTLINE));
    let (left, top) = (a.x.min(b.x).min(c.x), a.y.min(b.y).min(c.y));
    let (right, bottom) = (a.x.max(b.x).max(c.x), a.y.max(b.y).max(c.y));
    Some(rect(left, top, right - left + 1, bottom - top + 1))
}

/// Everything the pan HUD inks over the map that a settlement name must keep off, read from the
/// same values [`draw_pan_hud`] draws from. Empty with the camera attached, because then no HUD
/// draws. Three boxes is the exact bound.
///
/// The Inspect frame is deliberately not here. It traces the panel's own edge, where a name is
/// already clipped at the very pixels the frame inks, so reserving it would delete every
/// edge-anchored name for three columns.
pub(crate) fn pan_hud_boxes(
    w: i32,
    h: i32,
    pan: Option<Pan>,
    vp: &Viewport,
    fix: Option<Fix>,
) -> heapless::Vec<Rectangle, 3> {
    let mut boxes = heapless::Vec::new();
    let Some(pan) = pan else { return boxes };
    for r in pan_cue_boxes(w, h, pan) {
        let _ = boxes.push(r);
    }
    if let Some(r) = fix.and_then(|fix| back_to_you_box(w as f32, h as f32, vp, fix)) {
        let _ = boxes.push(r);
    }
    boxes
}

/// Everything the Map screen will ink over the map after the settlement names. Each argument is the
/// very thing that draws, so a piece of chrome the frame leaves out reserves nothing. The widest set
/// is [`MAX_CHROME`]; a box past it is dropped, which would be a name over the chrome.
#[allow(clippy::too_many_arguments)] // one argument per piece of chrome
fn map_chrome(
    w: i32,
    h: i32,
    chip_band: i32,
    bar: Option<&ScaleBar>,
    pan_hud: &[Rectangle],
    clock: bool,
    low_battery: bool,
    gauge: Option<Rectangle>,
) -> Chrome {
    let mut boxes: Chrome = heapless::Vec::new();
    let mut push = |r| {
        if boxes.push(r).is_err() {
            debug_assert!(false, "the map chrome holds more boxes than MAX_CHROME");
        }
    };
    if clock {
        push(clock_region(w));
    }
    if low_battery {
        push(low_battery_box());
    }
    if chip_band > 0 {
        push(chip_band_box(w, h));
    }
    if let Some(bar) = bar {
        push(bar.ink());
    }
    for r in pan_hud {
        push(*r);
    }
    if let Some(r) = gauge {
        push(r);
    }
    boxes
}

/// Approach radius (metres): in [`WaypointMode::Approach`] the chip appears once the next waypoint
/// is within this along-route distance ahead, and counts down to it.
pub(crate) const WAYPOINT_APPROACH_M: u32 = 500;

/// Half-diagonal (px) of the chip's ink diamond glyph.
const WPT_CHIP_DIAMOND_R: i32 = 4;
/// Horizontal inset (px) the pill may reach from each screen edge. Tight, so the name gets the full
/// remaining width and a long-but-common label reads whole beside its distance at 240 px.
const WPT_CHIP_INSET_X: i32 = 2;
/// Horizontal pad inside the pill (each side).
const WPT_CHIP_PAD_X: i32 = 4;
/// Gap between the diamond glyph and the name.
const WPT_CHIP_GAP_D: i32 = 3;
/// Gap between the name and the right-aligned distance, which the name budget reserves before the
/// fixed-width distance.
const WPT_CHIP_GAP_N: i32 = 4;

/// Whether the Map waypoint chip shows this frame, and which resident waypoint it names with the
/// along-route distance to go. Returns `Some((index, dist_to_go_m))` when the chip is up.
///
/// It shows only when all of these hold: the rider is not panning; the warning chip is down, which
/// its two states also make the along-route distance meaningless; `next_waypoint` is in range of
/// `wpts`; and the `mode` allows it. `dist_to_go` saturates, so it clamps to `0` during the
/// pass-linger and reads as "you are here" until the index advances.
fn waypoint_chip(
    mode: WaypointMode,
    panning: bool,
    no_fix: bool,
    off_route: bool,
    next_waypoint: Option<usize>,
    wpts: &[WptEntry],
    progress_m: u32,
) -> Option<(usize, u32)> {
    if panning || no_fix || off_route {
        return None;
    }
    let k = next_waypoint?;
    let dist_to_go = wpts.get(k)?.dist_along_m.saturating_sub(progress_m);
    match mode {
        WaypointMode::Off => None,
        WaypointMode::Approach => (dist_to_go <= WAYPOINT_APPROACH_M).then_some((k, dist_to_go)),
        WaypointMode::Always => Some((k, dist_to_go)),
    }
}

/// Draw the bottom-centre waypoint pill: a filled ink diamond, the name, and the right-aligned
/// distance. The whole pill is kept inside the panel by shrinking the name only, because the
/// distance is measured first and is never truncated.
fn draw_waypoint_chip(cv: &mut impl Surface, w: i32, h: i32, name: &str, dist: &str) {
    use super::palette::*;
    let font = Font::Body;
    let diamond_w = 2 * WPT_CHIP_DIAMOND_R + 1;
    let dist_w = text_width(dist, font) as i32;
    // Everything but the name is fixed; the name gets whatever remains inside the max pill width.
    let fixed_w = 2 * WPT_CHIP_PAD_X + diamond_w + WPT_CHIP_GAP_D + WPT_CHIP_GAP_N + dist_w;
    let name_budget = (w - 2 * WPT_CHIP_INSET_X) - fixed_w;
    let name = fit(name, name_budget, font);
    let name_w = text_width(&name, font) as i32;

    let pw = fixed_w + name_w;
    let px = (w - pw) / 2;
    let py = h - CHIP_H - CHIP_MARGIN;
    cv.round(rect(px, py, pw, CHIP_H), 9, PARCHMENT);
    cv.round_outline(rect(px, py, pw, CHIP_H), 9, INK);

    // Two triangles sharing the left and right vertices.
    let dcx = px + WPT_CHIP_PAD_X + WPT_CHIP_DIAMOND_R;
    let dcy = py + CHIP_H / 2;
    let r = WPT_CHIP_DIAMOND_R;
    let (left, right) = (Point::new(dcx - r, dcy), Point::new(dcx + r, dcy));
    cv.triangle(Point::new(dcx, dcy - r), left, right, INK);
    cv.triangle(Point::new(dcx, dcy + r), left, right, INK);

    // A text top at `py + 5` centres Body in the band, matching `draw_status_chip`.
    let ty = py + 5;
    let name_x = px + WPT_CHIP_PAD_X + diamond_w + WPT_CHIP_GAP_D;
    cv.text(&name, Point::new(name_x, ty), font, TextAlign::Left, INK);
    cv.text(dist, Point::new(px + pw - WPT_CHIP_PAD_X, ty), font, TextAlign::Right, INK);
}

/// Draw `s` with a one-pixel halo, so it stays readable over any map fill: the string four times at
/// ±1 px in `halo`, then once at `at` in `ink`.
pub(crate) fn halo_text(cv: &mut impl Surface, s: &str, at: Point, font: Font, align: TextAlign, ink: u16, halo: u16) {
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        cv.text(s, Point::new(at.x + dx, at.y + dy), font, align, halo);
    }
    cv.text(s, at, font, align, ink);
}

/// Top inset of the floating `HH:MM` digits.
const CLOCK_TOP: i32 = 8;

/// The rectangle the top-centre clock digits occupy: the dirty region
/// [`tick_timers`](MapScreen::tick_timers) reports, so the host clips the minute repaint to the
/// digits instead of re-rendering the whole map plane. Sized for a fixed five-glyph `HH:MM`, so the
/// region does not shift as the digits change, with two pixels of margin for the halo strokes.
pub fn clock_region(w: i32) -> Rectangle {
    let tw = text_width("00:00", Font::Body) as i32;
    let th = Font::Body.line_height() as i32;
    Rectangle::new(Point::new((w - tw) / 2 - 2, CLOCK_TOP - 2), Size::new(tw as u32 + 4, th as u32 + 4))
}

/// Whether the map's clock digits are up: the setting decides, and panning overrides it, because
/// the pan HUD's top cue owns the slot. The one home of that rule, so the pixels drawn, the box
/// reserved for them and the minute wake cannot disagree.
pub(crate) const fn clock_cue(map_clock: bool, panning: bool) -> bool {
    map_clock && !panning
}

/// Draw the top-centre `HH:MM` clock: bare ink digits floating on the map, with a one-pixel
/// parchment halo so they stay readable over dark terrain. One font step up from the rest of the
/// map chrome, and muted, because warning-orange stays reserved for alerts.
fn draw_clock(cv: &mut impl Surface, w: i32, now: DateTime) {
    use super::palette::*;
    let s = super::vocab::fmt::clock_hm(now.hour, now.minute);
    halo_text(cv, &s, Point::new(w / 2, CLOCK_TOP), Font::Body, TextAlign::Center, INK, PARCHMENT);
}

/// Whether the map's low-battery cue is up at `battery_pct`. The only thing a map base draws off
/// the gauge, and therefore the only battery fact [`RenderKeyKind::Map`](super::RenderKeyKind)
/// names. A boolean rather than the level, so a gauge tick that crosses nothing costs no render.
pub(crate) const fn low_battery_cue(battery_pct: u8) -> bool {
    battery_pct < crate::device_status::LOW_BATTERY_PCT
}

/// Top-left origin of the low-battery glyph, its shell size and its nub width. One set of values
/// answers both [`low_battery_box`] and [`draw_low_battery`], so the box that is reserved and the
/// pixels that are drawn cannot drift.
const BATTERY_AT: (i32, i32) = (10, 10);
const BATTERY_SIZE: (i32, i32) = (26, 13);
const BATTERY_NUB: i32 = 3;

/// The pixels the low-battery cue inks: the shell grown by its one-pixel ink halo, reaching right
/// to the nub, which carries no halo of its own.
fn low_battery_box() -> Rectangle {
    let ((x, y), (bw, bh)) = (BATTERY_AT, BATTERY_SIZE);
    rect(x - 1, y - 1, bw + BATTERY_NUB + 1, bh + 2)
}

/// Draw the low-battery cue: a small warning-red battery silhouette in the top-left corner. Filled
/// solid, because this is an alert and not a level readout, with an ink halo so it reads over any
/// terrain.
fn draw_low_battery(cv: &mut impl Surface) {
    use super::palette::*;
    let (x, y) = BATTERY_AT;
    let (bw, bh, nub) = (BATTERY_SIZE.0, BATTERY_SIZE.1, BATTERY_NUB);
    // The shell and nub grown by one pixel, so the glyph reads over any map colour.
    cv.round_outline(rect(x - 1, y - 1, bw + 2, bh + 2), 3, INK);
    cv.round_outline(rect(x, y, bw, bh), 3, WARNING);
    cv.round(rect(x + bw, y + bh / 3, nub, bh / 3), 1, WARNING);
    cv.round(rect(x + 3, y + 3, bw - 6, bh - 6), 1, WARNING);
}

/// The effort band's height: a 2 px black rule over 12 px of gauge.
pub(crate) const GAUGE_H: i32 = 14;

/// The effort gauge the Map draws this frame, or `None`. Pan mode drops it, because the pan HUD owns
/// the bottom edge. The one home of that rule, so the band drawn and the render key cannot disagree.
pub(crate) fn gauge_cue(
    recorder: &crate::recorder::RecorderMachine,
    limits: crate::effort::EffortLimits,
    panning: bool,
    w: i32,
) -> Option<Gauge> {
    if panning {
        return None;
    }
    recorder.gauge(limits, w)
}

/// The rows the effort band covers. The band is opaque, so a gauge that moves alone repaints only
/// these, with no map under them.
pub(crate) fn gauge_region(w: i32, h: i32) -> Rectangle {
    rect(0, h - GAUGE_H, w, GAUGE_H)
}

/// Draw the effort band: five equal zone slots split by 2 px black gaps, filled to the rider's
/// position through the zones in the current zone's colour.
fn draw_gauge(cv: &mut impl Surface, w: i32, h: i32, g: Gauge) {
    use super::palette::*;
    cv.fill(gauge_region(w, h), HUD);
    let top = h - GAUGE_H + 2;
    if g.fill_px > 0 {
        cv.fill(rect(0, top, g.fill_px as i32, GAUGE_H - 2), ZONE[g.zone as usize]);
    }
    let slot = w / 5;
    for i in 1..5 {
        cv.fill(rect(i * slot - 1, top, 2, GAUGE_H - 2), HUD);
    }
}

/// Largest on-screen width (px) the scale bar may reach: the chosen round distance is the biggest
/// `1/2/5 × 10ⁿ` that fits inside it. Long enough to read, short enough to clear the pan HUD's
/// centred bottom chevron.
const SCALE_TARGET_MAX_PX: f32 = 90.0;
/// The scale bar's left inset and the tick half-height.
const SCALE_MARGIN_X: i32 = 12;
/// Baseline inset from the bottom edge. The bar sits in the corner, and steps up past the chip band
/// while a bottom chip is up.
const SCALE_MARGIN_Y: i32 = 12;
/// Gap between a bottom chip band and the scale bar stepped above it.
const SCALE_CHIP_GAP: i32 = 12;
const SCALE_TICK_H: i32 = 5;

/// The scale bar one frame draws: its chosen length, its label and the row it sits on. One value
/// answers both [`ink`](ScaleBar::ink) and [`draw_scale_bar`], so the box that is reserved and the
/// pixels that are drawn cannot drift.
pub(crate) struct ScaleBar {
    bar_px: i32,
    label: heapless::String<8>,
    /// The baseline row: in the corner, or stepped above the bottom chip band when a chip is up. A
    /// taller band steps it proportionally.
    y: i32,
}

impl ScaleBar {
    /// The bar for this camera, or `None` at a degenerate zoom, where nothing draws.
    pub(crate) fn new(h: i32, chip_band: i32, mpp: f32, units: Units) -> Option<Self> {
        let (bar_px, label) = scale_bar_choice(mpp, units)?;
        let step = if chip_band > 0 { chip_band + CHIP_MARGIN + SCALE_CHIP_GAP } else { SCALE_MARGIN_Y };
        Some(ScaleBar { bar_px, label, y: h - step })
    }

    /// The pixels the bar inks, parchment halo included: the label row above the end ticks and the
    /// baseline, from the halo's left column to the wider of the haloed bar and the haloed label.
    /// This is what a settlement name must keep off, and it is a small box in the corner rather
    /// than the whole corner.
    pub(crate) fn ink(&self) -> Rectangle {
        let top = self.y - SCALE_TICK_H - Font::Label.line_height() as i32 - 2;
        // The bar's right halo column is one past its end tick; the label's is one past its last
        // glyph column, which `halo_text` does not count in the text width.
        let width = (self.bar_px + 1).max(text_width(&self.label, Font::Label) as i32) + 2;
        rect(SCALE_MARGIN_X - 1, top, width, self.y + 2 - top)
    }
}

/// Draw the scale bar at the bottom-left: a horizontal ink line with end ticks and a length label,
/// haloed in parchment so it reads over terrain. [`ScaleBar::new`] chose the distance.
fn draw_scale_bar(cv: &mut impl Surface, bar: &ScaleBar) {
    use super::palette::*;
    let (bar_px, label, y) = (bar.bar_px, &bar.label, bar.y);
    let x0 = SCALE_MARGIN_X;
    let x1 = x0 + bar_px;
    // Parchment halo: the same strokes offset by one pixel, drawn first.
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        cv.line(Point::new(x0 + dx, y + dy), Point::new(x1 + dx, y + dy), PARCHMENT);
        cv.line(Point::new(x0 + dx, y - SCALE_TICK_H + dy), Point::new(x0 + dx, y + dy), PARCHMENT);
        cv.line(Point::new(x1 + dx, y - SCALE_TICK_H + dy), Point::new(x1 + dx, y + dy), PARCHMENT);
    }
    cv.line(Point::new(x0, y), Point::new(x1, y), INK);
    cv.line(Point::new(x0, y - SCALE_TICK_H), Point::new(x0, y), INK);
    cv.line(Point::new(x1, y - SCALE_TICK_H), Point::new(x1, y), INK);
    let ly = y - SCALE_TICK_H - Font::Label.line_height() as i32 - 1;
    halo_text(cv, label, Point::new(x0, ly), Font::Label, TextAlign::Left, INK, PARCHMENT);
}

/// The 1/2/5 mantissa steps a scale bar chooses from, largest first.
const NICE_STEPS: [u32; 3] = [5, 2, 1];

/// The largest `1/2/5 × 10ⁿ` at or below `max`, or `None` below 1. A bounded loop rather than a
/// logarithm, because there is no libm here.
fn nice_125(max: f32) -> Option<u32> {
    if max < 1.0 {
        return None; // a sub-unit scale has no sensible round value
    }
    // Walk powers of ten up to the 10ⁿ at or just below `max`, then try 5·10ⁿ, 2·10ⁿ and 1·10ⁿ from
    // the decade above down, taking the largest that fits.
    let mut pow: u32 = 1;
    while (pow as f32) * 10.0 <= max {
        pow = pow.saturating_mul(10);
    }
    for decade in [pow.saturating_mul(10), pow] {
        for &m in &NICE_STEPS {
            let dist = m.saturating_mul(decade);
            if (dist as f32) <= max {
                return Some(dist);
            }
        }
    }
    None
}

/// Pick the scale bar's `(pixel width, label)` for the current `mpp` and unit system, or `None` for
/// a degenerate camera. The rule is the largest round `1/2/5 × 10ⁿ` distance, in the display unit
/// the label will use, whose on-screen width is at most [`SCALE_TARGET_MAX_PX`]. Imperial uses feet
/// below a mile and whole miles above it, so a bar says "2mi" and never "1.8mi". The distance is
/// derived straight from `mpp`, so the bar cannot disagree with the map's true scale.
fn scale_bar_choice(mpp: f32, units: Units) -> Option<(i32, heapless::String<8>)> {
    if !(mpp.is_finite() && mpp > 0.0) {
        return None;
    }
    // Work in the display unit's base: metres, or feet in imperial. `unit_per_px` is how many of
    // that base unit one screen pixel spans.
    let unit_per_px = if units.is_imperial() { mpp * crate::settings::FT_PER_M } else { mpp };
    let max_dist = SCALE_TARGET_MAX_PX * unit_per_px;
    // Imperial rounds in 1/2/5 miles once a mile fits, and in feet below that.
    let dist = if units.is_imperial() && max_dist >= crate::settings::FT_PER_MI as f32 {
        nice_125(max_dist / crate::settings::FT_PER_MI as f32)?.saturating_mul(crate::settings::FT_PER_MI)
    } else {
        nice_125(max_dist)?
    };
    let px = (dist as f32 / unit_per_px) as i32;
    if px >= 1 {
        return Some((px, scale_label(dist, units)));
    }
    None
}

/// Format a chosen scale distance, in metres or feet, as its bar label. The `1/2/5` values keep the
/// kilometre and mile forms to at most one decimal.
fn scale_label(dist: u32, units: Units) -> heapless::String<8> {
    use crate::settings::FT_PER_MI;
    let mut s: heapless::String<8> = heapless::String::new();
    if units.is_imperial() {
        if dist >= FT_PER_MI {
            // Whole miles for the round values that land there; the sub-mile foot values show a
            // single decimal.
            if dist.is_multiple_of(FT_PER_MI) {
                let _ = write!(s, "{}mi", dist / FT_PER_MI);
            } else {
                let _ = write!(s, "{}.{}mi", dist / FT_PER_MI, (dist % FT_PER_MI) * 10 / FT_PER_MI);
            }
        } else {
            let _ = write!(s, "{dist}ft");
        }
    } else if dist >= 1000 {
        if dist.is_multiple_of(1000) {
            let _ = write!(s, "{}km", dist / 1000);
        } else {
            let _ = write!(s, "{}.{}km", dist / 1000, (dist % 1000) / 100);
        }
    } else {
        let _ = write!(s, "{dist}m");
    }
    s
}

/// Pan-mode HUD geometry: every tunable pixel size in one place. The camera-travel-per-step knob
/// lives with the pan logic as [`crate::app::PAN_STEP_PX`].
mod hud {
    /// Active-axis chevron: tip `REACH` ahead of the centre, back corners `BACK` behind and
    /// ±`SPREAD` out, stroked at half-width `HW`, inset `INSET` from the edge.
    pub const CHEV_REACH: f32 = 9.0;
    pub const CHEV_BACK: f32 = 1.0;
    pub const CHEV_SPREAD: f32 = 10.0;
    pub const CHEV_HW: f32 = 2.5;
    pub const CHEV_INSET: f32 = 20.0;
    /// Ink halo thickness drawn behind every glyph so it reads over any map.
    pub const OUTLINE: f32 = 2.0;
    /// Inspect-state frame: two amber edge pixels and a one-pixel ink keyline inside. Three pixels
    /// is conspicuous without materially shrinking the map.
    pub const FRAME_AMBER_W: i32 = 2;
    pub const FRAME_INK_W: i32 = 1;
    /// Native-panel corner radius. Matches the simulator's display mask and the physical panel's
    /// rounded corners.
    pub const FRAME_RADIUS: u32 = 10;
    /// Zoom's bare plus/minus strokes use the same amber-with-ink-halo treatment as the chevrons.
    pub const ZOOM_GLYPH_HALF: f32 = 7.0;
    pub const ZOOM_GLYPH_HW: f32 = 2.5;
    /// Back-to-you marker: a filled triangle in the rider's marker colour, so it stays distinct
    /// from the hollow amber chevrons. Its half-height and half-width, its inset from the edge, and
    /// how far off-screen the rider must be first.
    pub const BACK_H: f32 = 8.0;
    pub const BACK_W: f32 = 7.0;
    pub const BACK_MARGIN: f32 = 14.0;
    pub const OFFSCREEN_MARGIN: f32 = 6.0;
}

#[inline]
fn pt(x: f32, y: f32) -> Point {
    Point::new(round_coord(x), round_coord(y))
}

/// The three screen vertices of an arrow centred at `center`, pointing along the unit direction
/// `dir`, with half-height and base half-width `size`: the tip, then the two base corners. One value
/// for the triangle that is drawn and the box reserved for it.
fn arrow_vertices(center: (f32, f32), dir: (f32, f32), size: (f32, f32)) -> [Point; 3] {
    let ((cx, cy), (ux, uy), (h, w)) = (center, dir, size);
    let (perpx, perpy) = (-uy, ux);
    let (bx, by) = (cx - ux * h, cy - uy * h); // base centre, opposite the tip
    [pt(cx + ux * h, cy + uy * h), pt(bx + perpx * w, by + perpy * w), pt(bx - perpx * w, by - perpy * w)]
}

/// A filled, ink-outlined triangle pointing along `(ux, uy)`. `h` and `w` are the half-height and
/// base half-width; the outline is the same triangle grown by [`hud::OUTLINE`], drawn first.
fn outlined_arrow(
    cv: &mut impl Surface,
    center: (f32, f32),
    dir: (f32, f32),
    size: (f32, f32),
    fill: u16,
    outline: u16,
) {
    let (h, w) = size;
    let [ot, obl, obr] = arrow_vertices(center, dir, (h + hud::OUTLINE, w + hud::OUTLINE));
    cv.triangle(ot, obl, obr, outline);
    let [t, bl, br] = arrow_vertices(center, dir, size);
    cv.triangle(t, bl, br, fill);
}

/// Draw the Inspect HUD over the already-rendered map: the amber frame, the active action's edge
/// cues, and a back-to-you marker once the rider is off-screen. Route mode needs no arrow, because
/// movement along the visible route is the feedback.
pub(super) fn draw_pan_hud(
    cv: &mut impl Surface,
    size: (f32, f32),
    pan: Pan,
    user_fix: Option<Fix>,
    marker: u16,
    vp: &Viewport,
) {
    use super::palette::*;
    use hud::*;
    let (w, h) = size;

    // The frame states the one thing every mode shares: this camera is detached.
    draw_inspect_frame(cv, w as i32, h as i32);

    // The marker draws first, so the chevrons render over it where they overlap.
    if let Some((bx, by, bux, buy)) = user_fix.and_then(|fix| back_to_you(w, h, vp, fix)) {
        outlined_arrow(cv, (bx, by), (bux, buy), (BACK_H, BACK_W), marker, INK);
    }

    // Free movement gets directional edge chevrons and Zoom gets plus and minus cues. Route
    // movement gets none: moving along the visible route is already direct feedback.
    if pan.tool == PanTool::Zoom {
        draw_zoom_cue(cv, (w / 2.0, CHEV_INSET), false);
        draw_zoom_cue(cv, (w / 2.0, h - CHEV_INSET), true);
    } else {
        let chevs = match pan.basis {
            PanBasis::Vertical => Some([((w / 2.0, CHEV_INSET), (0.0, -1.0)), ((w / 2.0, h - CHEV_INSET), (0.0, 1.0))]),
            PanBasis::Horizontal => {
                Some([((CHEV_INSET, h / 2.0), (-1.0, 0.0)), ((w - CHEV_INSET, h / 2.0), (1.0, 0.0))])
            }
            PanBasis::Route => None,
        };
        if let Some(chevs) = chevs {
            for (center, dir) in chevs {
                chevron(cv, center, dir, AMBER, INK);
            }
        }
    }
}

/// Draw the detached-camera frame: two concentric amber strokes follow the panel's rounded mask,
/// and a thin ink keyline keeps them legible over orange roads and pale map fills. The input plane's
/// hold bulge renders above this, so charging feedback wins temporarily without erasing the cue.
fn draw_inspect_frame(cv: &mut impl Surface, w: i32, h: i32) {
    use super::palette::*;
    use hud::*;
    let total_w = FRAME_AMBER_W + FRAME_INK_W;
    if w <= 2 * total_w || h <= 2 * total_w {
        return;
    }

    // Reducing the radius with each inset keeps the three strokes concentric, instead of making
    // the inner corners progressively squarer.
    for inset in 0..FRAME_AMBER_W {
        cv.round_outline(
            rect(inset, inset, w - 2 * inset, h - 2 * inset),
            FRAME_RADIUS.saturating_sub(inset as u32),
            AMBER,
        );
    }
    for offset in 0..FRAME_INK_W {
        let inset = FRAME_AMBER_W + offset;
        cv.round_outline(
            rect(inset, inset, w - 2 * inset, h - 2 * inset),
            FRAME_RADIUS.saturating_sub(inset as u32),
            INK,
        );
    }
}

/// A bare plus or minus glyph with the same round-ended amber stroke and ink halo as the pan
/// chevrons. The map stays visible around it, with no button disc competing with route furniture.
pub(super) fn draw_zoom_cue(cv: &mut impl Surface, center: (f32, f32), plus: bool) {
    use super::palette::*;
    use hud::*;
    let (cx, cy) = center;
    for (hw, color) in [(ZOOM_GLYPH_HW + OUTLINE, INK), (ZOOM_GLYPH_HW, AMBER)] {
        rounded_arm(cv, (cx - ZOOM_GLYPH_HALF, cy), (cx + ZOOM_GLYPH_HALF, cy), hw, color);
        if plus {
            rounded_arm(cv, (cx, cy - ZOOM_GLYPH_HALF), (cx, cy + ZOOM_GLYPH_HALF), hw, color);
        }
    }
}

/// Where to put the back-to-you marker for an off-screen rider: `(x, y, ux, uy)` is the edge
/// crossing of their bearing plus the unit direction toward them. `None` while the rider's own
/// marker is on-screen.
fn back_to_you(w: f32, h: f32, vp: &Viewport, fix: Fix) -> Option<(f32, f32, f32, f32)> {
    use hud::*;
    let (sxi, syi) = vp.to_screen(fix.lon, fix.lat);
    let (sx, sy) = (sxi as f32, syi as f32);
    let off =
        sx < -OFFSCREEN_MARGIN || sx > w + OFFSCREEN_MARGIN || sy < -OFFSCREEN_MARGIN || sy > h + OFFSCREEN_MARGIN;
    if !off {
        return None;
    }
    let (dx, dy) = (sx - w / 2.0, sy - h / 2.0);
    // alpha-max-plus-beta-min: a cheap |v| with no sqrt or libm, about 4 % off and invisible at
    // this size.
    let (adx, ady) = (dx.abs(), dy.abs());
    let mag = adx.max(ady) + 0.41 * adx.min(ady);
    if mag < 1.0 {
        return None;
    }
    let (ux, uy) = (dx / mag, dy / mag);
    // Clamp the bearing to the inset screen rectangle: the nearer of the two border crossings is
    // where the marker sits.
    let (hw, hh) = (w / 2.0 - BACK_MARGIN, h / 2.0 - BACK_MARGIN);
    let tx = if adx > 0.01 { hw / adx } else { f32::MAX };
    let ty = if ady > 0.01 { hh / ady } else { f32::MAX };
    let t = tx.min(ty);
    Some((w / 2.0 + dx * t, h / 2.0 + dy * t, ux, uy))
}

/// Draw one active-axis chevron pointing along `dir`, with an even ink halo. Both passes share the
/// centreline, so the halo stays uniform; growing a filled polygon instead would warp the arm
/// angle.
fn chevron(cv: &mut impl Surface, center: (f32, f32), dir: (f32, f32), fill: u16, outline: u16) {
    use hud::*;
    let (cx, cy) = center;
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux); // perpendicular = arm spread
    let tip = (cx + ux * CHEV_REACH, cy + uy * CHEV_REACH);
    let lb = (cx - ux * CHEV_BACK - px * CHEV_SPREAD, cy - uy * CHEV_BACK - py * CHEV_SPREAD);
    let rb = (cx - ux * CHEV_BACK + px * CHEV_SPREAD, cy - uy * CHEV_BACK + py * CHEV_SPREAD);
    // Ink halo first and wider, then the fill on top, on the same centreline.
    for (hw, color) in [(CHEV_HW + OUTLINE, outline), (CHEV_HW, fill)] {
        rounded_arm(cv, lb, tip, hw, color);
        rounded_arm(cv, tip, rb, hw, color);
        // The shared tip needs its own cap where the two strokes overlap, so the join is as round
        // as the plus and minus glyphs' endpoints.
        let r = round_coord(hw - 0.5).max(1) as u32;
        cv.disc(pt(tip.0, tip.1), r, color);
    }
}

/// Stroke `a`→`b` with round caps. Shared by the directional chevrons and the zoom glyphs, so both
/// have the same visual weight on the RGB222 panel.
fn rounded_arm(cv: &mut impl Surface, a: (f32, f32), b: (f32, f32), hw: f32, color: u16) {
    arm(cv, a, b, hw, color);
    // `disc(c, r)` spans diameter `2r+1`, so pass `hw-0.5` to make the cap exactly `hw` wide and
    // stop it bulging half a pixel past the arm.
    let r = round_coord(hw - 0.5).max(1) as u32;
    cv.disc(pt(a.0, a.1), r, color);
    cv.disc(pt(b.0, b.1), r, color);
}

/// Stroke segment `a`→`b` as a filled quad of half-width `hw`. The unit normal uses the
/// alpha-max-plus-beta-min approximation, with no sqrt or libm.
fn arm(cv: &mut impl Surface, a: (f32, f32), b: (f32, f32), hw: f32, color: u16) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let m = dx.abs().max(dy.abs()) + 0.41 * dx.abs().min(dy.abs());
    let s = if m > 0.001 { hw / m } else { 0.0 };
    let (nx, ny) = (-dy * s, dx * s);
    cv.triangle(pt(a.0 + nx, a.1 + ny), pt(b.0 + nx, b.1 + ny), pt(b.0 - nx, b.1 - ny), color);
    cv.triangle(pt(a.0 + nx, a.1 + ny), pt(b.0 - nx, b.1 - ny), pt(a.0 - nx, a.1 - ny), color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::harness::support::Buf;
    use crate::screen::test_ctx;
    use crate::screen::{Screen, Transition};
    use crate::Settings;
    use embedded_graphics::pixelcolor::Rgb888;

    fn shade(c: u16) -> Rgb888 {
        let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
        Rgb888::new(r, g, b)
    }

    /// The `(left, top, right, bottom)` bounds of everything drawn into `buf`, taking the untouched
    /// background as black. Panics on an empty buffer, because a cue that draws nothing pins
    /// nothing.
    fn drawn_box(buf: &Buf) -> (i32, i32, i32, i32) {
        let black = Rgb888::new(0, 0, 0);
        let mut drawn: Option<(i32, i32, i32, i32)> = None;
        for y in 0..320 {
            for x in 0..240 {
                if buf.get(x, y) != black {
                    let e = drawn.get_or_insert((x, y, x + 1, y + 1));
                    *e = (e.0.min(x), e.1.min(y), e.2.max(x + 1), e.3.max(y + 1));
                }
            }
        }
        drawn.expect("the cue draws something")
    }

    /// Assert the pixels a cue drew lie inside the box reserved for it. A settlement name may sit
    /// flush against that box, and the chrome draws after the names, so a column left out of it is
    /// a column the cue's halo erases from a glyph stroke.
    fn assert_inside(drawn: (i32, i32, i32, i32), reserved: Rectangle, what: &str) {
        let (l, t, r, b) = drawn;
        let (bx, by) = (reserved.top_left.x, reserved.top_left.y);
        let (bw, bh) = (reserved.size.width as i32, reserved.size.height as i32);
        assert!(
            l >= bx && t >= by && r <= bx + bw && b <= by + bh,
            "{what}: drawn ({l},{t})-({r},{b}) is outside the reserved ({bx},{by})-({},{})",
            bx + bw,
            by + bh,
        );
    }

    #[test]
    fn unavailable_route_geometry_renders_only_the_existing_off_route_labels() {
        use crate::harness::support::{build_min_obcm, Buf, OnceFix};
        use crate::screen::{apply, StatisticsScreen};
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_formats::io::{ByteSource, Error, SliceSource};
        use obc_ports::{Fix, RideClock, Sensors};
        use obc_route::{RouteIndex, RouteReader};

        struct Unreadable;
        impl ByteSource for Unreadable {
            fn len(&self) -> u64 {
                4096
            }
            fn read_at(&self, _: u64, _: &mut [u8]) -> Result<(), Error> {
                Err(Error::Io)
            }
        }
        let bytes = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../specs/vectors/route-plain.obcr"));
        let source = SliceSource(bytes);
        let index = RouteIndex::read(&source).unwrap();
        let good = RouteReader::new(&index, &source);
        let empty = RouteIndex::empty();
        let map_bytes = build_min_obcm(0);
        let map_source = SliceSource(&map_bytes);
        let tables = obc_reader::MapTables::parse(&map_source).unwrap();
        let cache = obc_reader::MapCache::new();
        let map = obc_reader::Reader::new(&map_source, &tables, &cache);
        let color = |c| {
            let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
            Rgb888::new(r, g, b)
        };
        for route in [RouteReader::new(&empty, &source), RouteReader::new(&index, &Unreadable)] {
            for units in [Units::Metric, Units::Imperial] {
                let mut app = crate::App::new(crate::AppState::new(0, 0, 0.05));
                app.set_settings(Settings { units, ..Settings::default() });
                let fix = Fix::at(good.start_lat, good.start_lon);
                app.tick(RideClock(0), Sensors::new(&mut OnceFix(Some(fix))), None);
                app.navigator.set_active_route(Some(0));
                app.navigator.refresh_route_profile(Some(&good));
                app.navigator.match_fix(fix, &route);
                assert!(app.navigator.route_state().off_route);
                assert_eq!(app.navigator.route_state().dist_to_route_m, u32::MAX);
                for statistics in [false, true] {
                    let screen = if statistics {
                        Screen::Statistics(StatisticsScreen::new())
                    } else {
                        Screen::Map(MapScreen::new())
                    };
                    apply(&mut app.ui.stack, Transition::Root(screen));
                    let mut actual = Buf::new(240, 320);
                    let mut scratch = Box::new(obc_render::RenderScratch::new());
                    app.render_frame(Some(&mut scratch), &mut actual, &map, Some(&route), 240.0, 320.0, color);
                    let mut expected = Buf::new(240, 320);
                    let mut canvas = Canvas::new(&mut expected, &color);
                    let rows = if statistics {
                        super::super::vocab::chrome::title_frame(
                            &mut canvas,
                            240,
                            320,
                            crate::t(Msg::StatsTitle, app.settings().language),
                            crate::t(Msg::StatsOff, app.settings().language).trim_end(),
                        );
                        8..28
                    } else {
                        draw_status_chip(
                            &mut canvas,
                            240,
                            320,
                            crate::t(Msg::MapOffRoute, app.settings().language).trim_end(),
                        );
                        (320 - CHIP_H - CHIP_MARGIN + 10)..(320 - CHIP_MARGIN - 10)
                    };
                    // The whole text row must match the label-only chrome, centring included.
                    for y in rows {
                        for x in 10..230 {
                            if statistics || (60..180).contains(&x) {
                                assert_eq!(actual.get(x, y), expected.get(x, y), "label at ({x}, {y})");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn translated_hint_pills_pad_actual_ink_evenly() {
        use crate::harness::support::Buf;
        use crate::screen::palette;
        use crate::settings::Language;
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_render::Canvas;
        let color = |c| {
            let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
            Rgb888::new(r, g, b)
        };
        for language in Language::ALL {
            let text = crate::i18n::t(Msg::MapPressToStart, language);
            let mut buf = Buf::new(240, 320);
            draw_hint_chip(&mut Canvas::new(&mut buf, &color), 240, 320, text);
            let height = hint_chip_height(text);
            let top = 320 - CHIP_MARGIN - height;
            let (first, second) = wrap2(text);
            let width = text_width(first, Font::Label).max(text_width(second, Font::Label)) as i32 + 16;
            let left = (240 - width) / 2;
            // The inner rectangle excludes the outline, including its rounded corners.
            let rows: std::vec::Vec<_> = (top + 7..top + height - 7)
                .filter(|&y| (left + 7..left + width - 7).any(|x| buf.get(x, y) == color(palette::INK)))
                .collect();
            assert_eq!(rows.first(), Some(&(top + HINT_PAD_Y)), "{language:?}: top padding");
            assert_eq!(rows.last(), Some(&(top + height - HINT_PAD_Y - 1)), "{language:?}: bottom padding");
        }
    }

    fn run(act: &mut Activity, rec: &mut crate::RecorderMachine, g: Gesture) -> Transition {
        let mut st = crate::AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { recorder: rec, ..test_ctx(&mut st, act, &mut settings) };
        MapScreen::new().handle(g, &mut cx)
    }

    #[test]
    fn browse_map_back_pops() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Idle); // no session, so a browse map
        assert!(matches!(run(&mut act, &mut rec, Gesture::Back), Transition::Pop));
    }

    #[test]
    fn browse_map_press_opens_the_start_card() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Idle);
        assert!(matches!(run(&mut act, &mut rec, Gesture::Press), Transition::Push(Screen::RideStart(_))));
        assert_eq!(act.mode, Mode::Idle, "opening the card doesn't touch the mode");
    }

    #[test]
    fn riding_map_keeps_the_sibling_and_pause_bindings() {
        let mut rec = crate::RecorderMachine::new();
        let mut act = Activity::new(Mode::Riding);
        rec.test_open();
        assert!(matches!(run(&mut act, &mut rec, Gesture::Back), Transition::Replace(Screen::Statistics(_))));
        let mut act = Activity::new(Mode::Riding);
        rec.test_open();
        assert!(matches!(run(&mut act, &mut rec, Gesture::Press), Transition::Push(Screen::RideControl(_))));
    }

    /// Across the whole zoom range the realised pixel width stays in a readable band: never wider
    /// than the target, and never so short that the bar cannot be read.
    #[test]
    fn scale_bar_fits_the_target_across_the_zoom_range() {
        // A sweep from riding-close to overview, in both unit systems.
        for &mpp in &[0.5f32, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 200.0, 400.0] {
            for units in [Units::Metric, Units::Imperial] {
                let (px, _label) = scale_bar_choice(mpp, units).expect("a real zoom yields a bar");
                assert!(
                    (30..=SCALE_TARGET_MAX_PX as i32).contains(&px),
                    "mpp={mpp} {units:?}: px={px} out of the readable band"
                );
            }
        }
    }

    /// Concrete labels at the metric and imperial cross-overs, so a change in the rounding or the
    /// format shows up as a wrong string.
    #[test]
    fn scale_bar_labels_are_correct() {
        assert_eq!(scale_bar_choice(1.0, Units::Metric).unwrap().1.as_str(), "50m");
        assert_eq!(scale_bar_choice(10.0, Units::Metric).unwrap().1.as_str(), "500m");
        assert_eq!(scale_bar_choice(50.0, Units::Metric).unwrap().1.as_str(), "2km");
        assert_eq!(scale_bar_choice(200.0, Units::Metric).unwrap().1.as_str(), "10km");
        // Imperial: sub-mile feet, then whole 1/2/5 miles past a mile, never "1.8mi".
        assert_eq!(scale_bar_choice(1.0, Units::Imperial).unwrap().1.as_str(), "200ft");
        assert_eq!(scale_bar_choice(10.0, Units::Imperial).unwrap().1.as_str(), "2000ft");
        assert_eq!(scale_bar_choice(50.0, Units::Imperial).unwrap().1.as_str(), "2mi");
        assert_eq!(scale_bar_choice(200.0, Units::Imperial).unwrap().1.as_str(), "10mi");
    }

    /// The reserved box holds every pixel the bar draws, halo columns included, whether the bar or
    /// its label is the wider of the two.
    #[test]
    fn the_scale_bar_ink_box_holds_every_pixel_the_bar_draws() {
        use obc_render::Canvas;
        // One case has a bar wider than its label and the other has it the other way round. The
        // chip band only moves the bar up the panel, so one case takes each position.
        for (mpp, units, chip_band, bar_is_wider) in
            [(10.0f32, Units::Metric, 0, true), (10.0, Units::Imperial, CHIP_H, false)]
        {
            let bar = ScaleBar::new(320, chip_band, mpp, units).expect("a real zoom yields a bar");
            let label_px = text_width(&bar.label, Font::Label) as i32;
            let case = (bar.label.as_str(), bar.bar_px, label_px);
            assert_eq!(bar.bar_px + 1 > label_px, bar_is_wider, "{case:?}: the case under test");

            let mut buf = Buf::new(240, 320);
            draw_scale_bar(&mut Canvas::new(&mut buf, &shade), &bar);
            let drawn = drawn_box(&buf);
            assert_inside(drawn, bar.ink(), &std::format!("{case:?}"));
            let (l, _, r, b) = drawn;
            let (bx, by) = (bar.ink().top_left.x, bar.ink().top_left.y);
            let (bw, bh) = (bar.ink().size.width as i32, bar.ink().size.height as i32);
            // The baseline and its halo run the bar's whole width, so three edges are exact. Only
            // the top edge is loose, by the label's top bearing.
            assert_eq!((l, r, b), (bx, if bar_is_wider { bx + bw } else { r }, by + bh), "{case:?}: the tight edges");
        }
    }

    /// Pan mode suppresses every bottom pill, but the pan HUD still inks its Up/Down cue there,
    /// after the names, so the cue comes in as chrome and a name under it is refused.
    #[test]
    fn the_pan_cue_keeps_a_settlement_name_off_the_bottom_of_the_panel() {
        let vp = Viewport::new(240.0, 320.0, 0, 0, 1.0);
        let states = pan_states();
        let pan_of = |want: &str| states.iter().find(|(name, _)| *name == want).expect("the state exists").1;
        let (vertical, zoom, horizontal, route) =
            (pan_of("free vertical"), pan_of("zoom"), pan_of("free horizontal"), pan_of("route"));

        // A name centred on the bottom-centre cue, and one clear of it in the middle of the panel.
        let under_cue = rect(72, 288, 96, 24);
        let clear = rect(72, 150, 96, 24);
        let placer = |pan| {
            let hud = pan_hud_boxes(240, 320, pan, &vp, None);
            PointPlacement::new(&label_reserved(
                &vp,
                None,
                &[],
                &map_chrome(240, 320, 0, None, &hud, false, false, None),
            ))
        };

        for (name, pan) in [("free vertical", vertical), ("zoom", zoom)] {
            let mut place = placer(Some(pan));
            assert!(!place.try_place(under_cue, 0), "{name}: a name under the cue is refused");
            assert!(place.try_place(clear, 0), "{name}: one clear of it is placed");
        }

        // Route movement draws no cue, and with no pill either, the bottom is bare map.
        assert!(placer(Some(route)).try_place(under_cue, 0), "route movement inks no cue, so the corner is free");

        // Free horizontal moves the pair to the sides, so the bottom centre comes back.
        assert_eq!(pan_cue_boxes(240, 320, horizontal).len(), 2, "one cue box for each side");
        let mut place = placer(Some(horizontal));
        assert!(!place.try_place(rect(0, 148, 60, 24), 0), "a name under the left cue is refused");
        assert!(place.try_place(under_cue, 0), "and the bottom centre is free");
    }

    /// The four pan states, built the way the gestures build them.
    fn pan_states() -> [(&'static str, Pan); 4] {
        let mut st = crate::AppState::new(0, 0, 1.0);
        st.enter_pan(false, 0);
        let vertical = st.pan.expect("pan mode is on");
        st.cycle_pan_mode(false);
        let zoom = st.pan.expect("pan mode is on");
        st.cycle_pan_mode(false);
        st.toggle_pan_free_axis();
        let horizontal = st.pan.expect("pan mode is on");
        st.enter_pan(true, 0);
        let route = st.pan.expect("pan mode is on");
        assert_eq!(
            [vertical.basis, zoom.basis, horizontal.basis, route.basis],
            [PanBasis::Vertical, PanBasis::Vertical, PanBasis::Horizontal, PanBasis::Route],
        );
        assert_eq!(zoom.tool, PanTool::Zoom);
        [("free vertical", vertical), ("zoom", zoom), ("free horizontal", horizontal), ("route", route)]
    }

    /// The top of the panel is measured like the bottom: only what a frame really inks up there is
    /// held back from the names. Pan mode hides the clock, so its top cue is held whatever the
    /// setting says.
    #[test]
    fn the_top_chrome_is_only_what_the_frame_inks() {
        let vp = Viewport::new(240.0, 320.0, 0, 0, 1.0);
        let placer = |map_clock: bool, low_battery: bool, pan: Option<Pan>| {
            let clock = clock_cue(map_clock, pan.is_some());
            let hud = pan_hud_boxes(240, 320, pan, &vp, None);
            PointPlacement::new(&label_reserved(
                &vp,
                None,
                &[],
                &map_chrome(240, 320, 0, None, &hud, clock, low_battery, None),
            ))
        };
        // One name under each piece of top chrome, each clear of the other two.
        let under_clock = rect(90, 8, 60, 24);
        let under_battery = rect(10, 10, 26, 13);
        let under_top_cue = rect(100, 10, 40, 24);
        for r in [under_clock, under_battery, under_top_cue] {
            let bottom = r.top_left.y + r.size.height as i32;
            assert!(bottom <= CLOCK_TOP + Font::Body.line_height() as i32 + 2, "the case sits in the old top band");
        }

        // The clock off and the charge healthy: the whole band is map again.
        let mut bare = placer(false, false, None);
        assert!(bare.try_place(under_battery, 0), "the corner is free");
        assert!(bare.try_place(under_clock, 0), "and so is the centre");

        // Each cue refuses its own name and no other.
        let mut clock_on = placer(true, false, None);
        assert!(!clock_on.try_place(under_clock, 0), "the clock refuses the name under the digits");
        assert!(clock_on.try_place(under_battery, 0), "and leaves the corner alone");
        let mut flat = placer(false, true, None);
        assert!(!flat.try_place(under_battery, 0), "a low charge refuses the name under the cue");
        assert!(flat.try_place(under_clock, 0), "and leaves the centre alone");

        // Panning, the HUD's top cue takes the centre whatever the clock setting is.
        let mut st = crate::AppState::new(0, 0, 1.0);
        st.enter_pan(false, 0);
        let vertical = st.pan.expect("pan mode is on");
        for map_clock in [false, true] {
            assert!(
                !placer(map_clock, false, Some(vertical)).try_place(under_top_cue, 0),
                "map_clock={map_clock}: the pan cue is reserved",
            );
        }
    }

    /// The top boxes hold every pixel they protect. The back-to-you marker points along the rider's
    /// bearing, so it must fit the same box at every angle.
    #[test]
    fn the_top_chrome_boxes_hold_every_pixel_they_draw() {
        use obc_render::Canvas;
        // The digits are monospace, so any time is the region's fixed five glyphs wide.
        let mut clock = Buf::new(240, 320);
        draw_clock(&mut Canvas::new(&mut clock, &shade), 240, dt(23, 59));
        assert_inside(drawn_box(&clock), clock_region(240), "the clock");

        let mut battery = Buf::new(240, 320);
        draw_low_battery(&mut Canvas::new(&mut battery, &shade));
        assert_inside(drawn_box(&battery), low_battery_box(), "the low-battery cue");

        // The marker around the rider, each one drawn the way `draw_pan_hud` draws it.
        let vp = Viewport::new(240.0, 320.0, 0, 0, 1.0);
        for (dx, dy) in [(0.0f32, -1.0f32), (0.7, -0.7), (1.0, 0.0), (0.7, 0.7), (0.0, 1.0), (-0.7, 0.7), (-1.0, 0.0)] {
            let fix = fix_at_screen(&vp, 120.0 + dx * 400.0, 160.0 + dy * 400.0);
            let (x, y, ux, uy) = back_to_you(240.0, 320.0, &vp, fix).expect("an off-panel rider draws a marker");
            let mut buf = Buf::new(240, 320);
            let mut cv = Canvas::new(&mut buf, &shade);
            outlined_arrow(
                &mut cv,
                (x, y),
                (ux, uy),
                (hud::BACK_H, hud::BACK_W),
                crate::screen::palette::AMBER,
                crate::screen::palette::INK,
            );
            let reserved = back_to_you_box(240.0, 320.0, &vp, fix).expect("…and reserves a box for it");
            assert_inside(drawn_box(&buf), reserved, &std::format!("the marker at ({dx}, {dy})"));
        }
    }

    /// The waypoint diamonds ink over the point marks, so each one the frame draws holds a box.
    #[test]
    fn a_waypoint_diamond_holds_the_pixels_it_draws_against_the_point_marks() {
        use obc_render::Canvas;
        let vp = Viewport::new(240.0, 320.0, 0, 0, 1.0);
        let at = |x: f32, y: f32| {
            let (lon, lat) = vp.to_map(x, y);
            WptEntry { lon, lat, ..wp(0, "Brunnen") }
        };
        let (on, off) = (at(120.0, 200.0), at(120.0, -400.0));

        let reserved = label_reserved(&vp, None, &[on.clone(), off], &[]);
        assert_eq!(reserved.len(), 1, "only a diamond the frame draws reserves a box");
        let mut buf = Buf::new(240, 320);
        draw_waypoint_diamonds(&mut Canvas::new(&mut buf, &shade), &vp, &[on], 240, 320, crate::screen::palette::INK);
        assert_inside(drawn_box(&buf), reserved[0], "the waypoint diamond");

        // A mark may sit flush against reserved chrome, so the box alone decides.
        let mut place = PointPlacement::new(&reserved);
        assert!(!place.try_place(rect(90, 192, 60, 16), 0), "a name over the diamond is refused");
        assert!(place.try_place(rect(90, 100, 60, 16), 0), "one clear of it is placed");
    }

    /// A [`Fix`] that lands on the given screen point of `vp`.
    fn fix_at_screen(vp: &Viewport, x: f32, y: f32) -> Fix {
        let (lon, lat) = vp.to_map(x, y);
        Fix::at(lat, lon)
    }

    /// The back-to-you marker draws after the names, so it comes in as chrome in every pan state,
    /// including the two that ink no Up/Down cue at the top at all.
    #[test]
    fn the_back_to_you_marker_keeps_a_name_off_its_box() {
        let vp = Viewport::new(240.0, 320.0, 0, 0, 1.0);
        let away = fix_at_screen(&vp, 66.0, -400.0);
        let marker = back_to_you_box(240.0, 320.0, &vp, away).expect("an off-panel rider draws the marker");
        assert!(marker.top_left.y < CLOCK_TOP, "the marker inks the top of the panel");
        let (mx, my) = (marker.top_left.x, marker.top_left.y);
        let (mw, mh) = (marker.size.width as i32, marker.size.height as i32);
        // A name centred on the marker, and one well clear of it.
        let under_marker = rect(mx + mw / 2 - 30, my + mh / 2 - 8, 60, 16);
        let clear = rect(72, 150, 96, 16);

        for (name, pan) in pan_states() {
            let hud = pan_hud_boxes(240, 320, Some(pan), &vp, Some(away));
            let chrome = map_chrome(240, 320, 0, None, &hud, false, false, None);
            let mut place = PointPlacement::new(&label_reserved(&vp, Some(away), &[], &chrome));
            assert!(!place.try_place(under_marker, 0), "{name}: a name under the marker is refused");
            assert!(place.try_place(clear, 0), "{name}: one clear of it is placed");
        }

        // A rider on the panel draws no marker, so nothing is held back for one.
        let home = fix_at_screen(&vp, 120.0, 160.0);
        assert!(back_to_you_box(240.0, 320.0, &vp, home).is_none(), "an on-panel rider needs no marker");
    }

    /// [`MAX_CHROME`]'s two worst cases, so the capacity claim fails here rather than dropping a
    /// box in a debug build nobody runs.
    #[test]
    fn max_chrome_holds_the_widest_frame() {
        let vp = Viewport::new(240.0, 320.0, 0, 0, 1.0);
        let bar = ScaleBar::new(320, CHIP_H, 10.0, Units::Metric).expect("a real zoom yields a bar");
        // Attached: the clock digits, the low-battery cue, a bottom pill's band, the scale bar and
        // the effort band.
        let band = Some(gauge_region(240, 320));
        assert_eq!(map_chrome(240, 320, CHIP_H, Some(&bar), &[], true, true, band).len(), 5);
        // Panning: the whole HUD, the low-battery cue and the bar. Zoom is the widest HUD, and the
        // clock and every pill are suppressed.
        let zoom = pan_states().into_iter().find(|(name, _)| *name == "zoom").expect("a zoom state").1;
        let away = fix_at_screen(&vp, 66.0, -400.0);
        let hud = pan_hud_boxes(240, 320, Some(zoom), &vp, Some(away));
        assert_eq!(hud.len(), 3, "two Up/Down cues and the marker");
        // Literals, so raising the constant does not make the claim true by itself.
        assert_eq!(map_chrome(240, 320, 0, Some(&bar), &hud, false, true, None).len(), 5);
        assert_eq!(MAX_CHROME, 5);
    }

    #[test]
    fn scale_bar_rejects_degenerate_zoom() {
        assert!(scale_bar_choice(f32::NAN, Units::Metric).is_none());
        assert!(scale_bar_choice(f32::INFINITY, Units::Metric).is_none());
        assert!(scale_bar_choice(0.0, Units::Metric).is_none());
        assert!(scale_bar_choice(-1.0, Units::Metric).is_none());
    }

    #[test]
    fn waypoint_diamond_vertices_and_cull() {
        let r = WAYPOINT_DIAMOND_R;
        let v = waypoint_diamond(100, 80, 240, 240).expect("on-panel centre draws");
        assert_eq!(
            v,
            (Point::new(100, 80 - r), Point::new(100, 80 + r), Point::new(100 - r, 80), Point::new(100 + r, 80))
        );
        // A centre exactly on the half-diagonal margin still draws.
        assert!(waypoint_diamond(-r, 120, 240, 240).is_some(), "just off the left edge still draws");
        assert!(waypoint_diamond(120, 240 + r, 240, 240).is_some(), "just past the bottom still draws");
        assert!(waypoint_diamond(-r - 1, 120, 240, 240).is_none(), "past the left margin culls");
        assert!(waypoint_diamond(240 + r + 1, 120, 240, 240).is_none(), "past the right margin culls");
        assert!(waypoint_diamond(120, -r - 1, 240, 240).is_none(), "above the top margin culls");
    }

    fn dt(hour: u8, minute: u8) -> DateTime {
        DateTime { year: 2025, month: 6, day: 29, hour, minute }
    }

    /// With the pill visible a rollover self-dirties only the pill region and arms the next-minute
    /// wake; hidden, it claims nothing and arms no wake.
    #[test]
    fn clock_tick_is_region_scoped_and_gated() {
        let w = 240;
        let mut scr = MapScreen::new();
        // `tracking = true` throughout, so the browse hint retires on the first poll and never
        // contends for the wake. The first observation initialises the baseline.
        let t0 = scr.tick_timers(0, dt(14, 40), 20_000, w, false, true, true);
        assert!(!t0.changed);
        assert_eq!(t0.next_wake_ms, Some(20_000));
        // A minute rollover fires, region-clipped to the pill, never a full-frame `None`.
        let t1 = scr.tick_timers(1_000, dt(14, 41), 60_000, w, false, true, true);
        assert!(t1.changed);
        assert_eq!(t1.region, Some(clock_region(w)));
        assert!(t1.region.unwrap().size.width < w as u32, "the pill region is a small band, not the whole width");
        // Hidden by the setting: no change and no wake, even across a rollover.
        let off = scr.tick_timers(2_000, dt(14, 42), 60_000, w, false, false, true);
        assert_eq!(off, ScreenTick::idle());
        // Hidden by pan, because the pan chevron owns the slot.
        let panned = scr.tick_timers(3_000, dt(14, 43), 60_000, w, true, true, true);
        assert_eq!(panned, ScreenTick::idle());
    }

    /// The first poll of a browse map arms the hint, and it fires exactly one clearing repaint at
    /// expiry and then stays down. The clock is off here, to isolate the hint's own wake.
    #[test]
    fn browse_hint_arms_on_entry_and_expires_once() {
        let mut scr = MapScreen::new();
        // Armed, chip up, wake at HINT_MS, but nothing changed: it was drawn on entry.
        let t0 = scr.tick_timers(0, dt(14, 40), 60_000, 240, false, false, false);
        assert!(!t0.changed);
        assert_eq!(t0.next_wake_ms, Some(HINT_MS));
        assert!(scr.hint.chip_up(), "the chip is up on entry");
        // Still up just before the deadline, with a shrinking residual wake.
        let mid = scr.tick_timers(HINT_MS - 1, dt(14, 40), 60_000, 240, false, false, false);
        assert!(!mid.changed);
        assert_eq!(mid.next_wake_ms, Some(1));
        assert!(scr.hint.chip_up());
        // At the deadline: one full-frame clearing repaint, then down.
        let expire = scr.tick_timers(HINT_MS, dt(14, 40), 60_000, 240, false, false, false);
        assert!(expire.changed);
        assert_eq!(expire.region, None, "the expiry is a full-frame change, not a clock-region tick");
        assert_eq!(expire.next_wake_ms, None);
        assert!(!scr.hint.chip_up(), "the chip is down once expired");
        // And it does not re-fire on a later poll.
        let after = scr.tick_timers(HINT_MS + 5_000, dt(14, 40), 60_000, 240, false, false, false);
        assert!(!after.changed);
        assert!(!scr.hint.chip_up());
    }

    /// A riding map never shows the hint: the first poll retires it, so the chip is down from the
    /// start and arms no hint wake.
    #[test]
    fn riding_map_never_shows_the_browse_hint() {
        let mut scr = MapScreen::new();
        let t = scr.tick_timers(0, dt(14, 40), 60_000, 240, false, false, true);
        assert!(!scr.hint.chip_up(), "tracking → the hint is suppressed");
        assert_eq!(t.next_wake_ms, None, "no hint wake armed on a riding map");
    }

    fn wp(dist_along_m: u32, name: &str) -> WptEntry {
        let mut n = heapless::String::new();
        let _ = n.push_str(name);
        WptEntry { dist_along_m, lon: 0, lat: 0, category: None, lateral_offset_m: 0, name: n }
    }

    /// The chip shows only when the rider is not panning, the warning chip is down, a next waypoint
    /// is in range, and the mode allows it. The approach radius holds to the exact metre.
    #[test]
    fn waypoint_chip_visibility_rules() {
        let wpts = [wp(0, "Brunnen"), wp(1700, "Pass Summit")];
        let next = Some(1); // Pass Summit, at 1700 m
        let approach = |p| waypoint_chip(WaypointMode::Approach, false, false, false, next, &wpts, p);
        // Approach: hidden beyond the radius, shown from exactly 500 m out (not 501 m).
        assert_eq!(approach(1000), None, "700 m out: still hidden");
        assert_eq!(approach(1199), None, "501 m out: still hidden");
        assert_eq!(approach(1200), Some((1, 500)), "exactly 500 m out: shown, counting 500");
        assert_eq!(approach(1201), Some((1, 499)), "inside the radius: shown, counting down");
        assert_eq!(
            waypoint_chip(WaypointMode::Always, false, false, false, next, &wpts, 0),
            Some((1, 1700)),
            "Always shows the far waypoint too"
        );
        assert_eq!(waypoint_chip(WaypointMode::Off, false, false, false, next, &wpts, 1200), None, "Off never shows");
        // The three suppressors, each over a frame that would otherwise show.
        assert_eq!(waypoint_chip(WaypointMode::Always, true, false, false, next, &wpts, 0), None, "panning hides it");
        assert_eq!(waypoint_chip(WaypointMode::Always, false, true, false, next, &wpts, 0), None, "no-fix hides it");
        assert_eq!(waypoint_chip(WaypointMode::Always, false, false, true, next, &wpts, 0), None, "off-route hides it");
        // No next waypoint, and a stale index past the table, both cull safely.
        assert_eq!(waypoint_chip(WaypointMode::Always, false, false, false, None, &wpts, 0), None, "no next waypoint");
        assert_eq!(
            waypoint_chip(WaypointMode::Always, false, false, false, Some(9), &wpts, 0),
            None,
            "stale index culls"
        );
    }

    /// While the resident index still points at a just-passed waypoint, `dist_to_go` clamps to 0,
    /// so the chip shows `0m` and never a wrapped distance.
    #[test]
    fn waypoint_chip_lingers_at_zero_past_the_waypoint() {
        let wpts = [wp(1700, "Pass Summit")];
        let got = waypoint_chip(WaypointMode::Approach, false, false, false, Some(0), &wpts, 1750);
        assert_eq!(got, Some((0, 0)), "50 m past: visible, distance clamped to 0");
        assert_eq!(crate::screen::vocab::fmt::distance_short(0, Units::Metric).as_str(), "0m", "…rendering as 0m");
    }

    /// The chip gives the name its whole remaining width on the 240 px panel, so "Pass Summit"
    /// reads in full rather than truncating. Recomputes the budget the way `draw_waypoint_chip`
    /// does.
    #[test]
    fn pass_summit_fits_the_approach_chip_at_240px() {
        let w = 240;
        let font = Font::Body;
        let diamond_w = 2 * WPT_CHIP_DIAMOND_R + 1;
        let dist_w = text_width("299m", font) as i32;
        let fixed_w = 2 * WPT_CHIP_PAD_X + diamond_w + WPT_CHIP_GAP_D + WPT_CHIP_GAP_N + dist_w;
        let name_budget = (w - 2 * WPT_CHIP_INSET_X) - fixed_w;
        assert_eq!(fit("Pass Summit", name_budget, font).as_str(), "Pass Summit", "the full name fits, no ellipsis");
    }

    /// The ink glyph is the last writer at the anchor, and every pixel one step off it holds halo
    /// or ink, never the bare background.
    #[test]
    fn halo_text_writes_the_halo_under_the_ink() {
        use crate::harness::support::Buf;
        use crate::screen::palette::{INK, PARCHMENT};
        use embedded_graphics::pixelcolor::Rgb888;
        use obc_render::Canvas;
        let color = |c| {
            let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
            Rgb888::new(r, g, b)
        };
        let at = Point::new(20, 8);
        // The bare glyph, for the set of pixels the ink pass owns.
        let mut plain = Buf::new(80, 48);
        Canvas::new(&mut plain, &color).text("12km", at, Font::Label, TextAlign::Left, INK);
        let ink_px: std::vec::Vec<_> = (0..48)
            .flat_map(|y| (0..80).map(move |x| (x, y)))
            .filter(|&(x, y)| plain.get(x, y) == color(INK))
            .collect();
        assert!(!ink_px.is_empty(), "the glyphs draw ink");

        let mut haloed = Buf::new(80, 48);
        halo_text(&mut Canvas::new(&mut haloed, &color), "12km", at, Font::Label, TextAlign::Left, INK, PARCHMENT);
        for &(x, y) in &ink_px {
            assert_eq!(haloed.get(x, y), color(INK), "ink is the last writer at ({x}, {y})");
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let c = haloed.get(x + dx, y + dy);
                assert!(c == color(INK) || c == color(PARCHMENT), "({}, {}) is covered", x + dx, y + dy);
            }
        }
        assert!(haloed.count(color(PARCHMENT)) > 0, "the halo is visible around the glyphs");
        assert_eq!(haloed.count(color(INK)), ink_px.len(), "the ink pass paints no more than the bare glyph");
    }
}
